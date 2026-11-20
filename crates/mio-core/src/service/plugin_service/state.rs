use super::*;

impl PluginState {
    pub(crate) fn with_preferences(prefs: PluginPreferencesStore) -> Self {
        let (passive_tx, passive_rx) = mpsc::channel(64);
        Self {
            prefs,
            passive_tx,
            passive_rx,
            last_auto_reload_check: None,
            auto_reload_timer: None,
            inspections: HashMap::new(),
            instances: HashMap::new(),
            order: Vec::new(),
            context_viewers: HashMap::new(),
            operations: HashMap::new(),
            jobs: JoinSet::new(),
            network: HashMap::new(),
            workspace: HashMap::new(),
            stopping: false,
            cleanup_error: None,
        }
    }

    pub(crate) fn has_resources(&self) -> bool {
        !self.instances.is_empty() || !self.jobs.is_empty() || !self.network.is_empty()
    }

    pub(crate) fn cancel_network(&self) {
        for network in self.network.values() {
            network.call.cancel.cancel();
        }
    }

    pub(crate) async fn next_signal(&mut self) -> Signal {
        std::future::poll_fn(|cx| {
            if let Poll::Ready(Some(result)) = self.jobs.poll_join_next(cx) {
                return Poll::Ready(Signal::Job(result));
            }
            for (key, network) in &mut self.network {
                if let Poll::Ready(result) = Pin::new(&mut network.task).poll(cx) {
                    return Poll::Ready(Signal::Network(*key, result));
                }
            }
            for (key, pending) in &mut self.workspace {
                if pending.expired.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Signal::WorkspaceExpired(*key));
                }
            }
            if let Poll::Ready(Some((project, record))) = self.passive_rx.poll_recv(cx) {
                return Poll::Ready(Signal::PassiveFlow(project, record));
            }
            for instance in self.instances.values_mut() {
                match instance.worker.events.poll_recv(cx) {
                    Poll::Ready(Some(event)) => return Poll::Ready(Signal::Worker(event)),
                    Poll::Ready(None) => {
                        return Poll::Ready(Signal::Worker(PluginEvent::Stopped {
                            identity: instance.info.identity,
                            reason: "插件事件通道已关闭".into(),
                        }));
                    }
                    Poll::Pending => {}
                }
                if let Poll::Ready(Some(call)) = instance.services.poll_recv(cx) {
                    return Poll::Ready(Signal::Service(call));
                }
            }
            let has_watchable = !self.stopping
                && self.order.iter().any(|id| {
                    self.instances.get(id).is_some_and(|inst| {
                        inst.auto_reload
                            && inst.info.tab_id.is_none()
                            && !self.context_viewers.contains_key(id)
                    })
                });
            if has_watchable {
                let timer = self.auto_reload_timer.get_or_insert_with(|| {
                    Box::pin(tokio::time::sleep(Duration::from_millis(500)))
                });
                if timer.as_mut().poll(cx).is_ready() {
                    self.auto_reload_timer = None;
                    return Poll::Ready(Signal::AutoReloadTick);
                }
            } else {
                self.auto_reload_timer = None;
            }
            Poll::Pending
        })
        .await
    }
}

impl State {
    pub(crate) async fn plugin_result(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        result: Result<PluginOutcome, String>,
    ) {
        self.emit(Event::PluginOperation {
            project,
            operation_id,
            result,
        })
        .await;
    }

    pub(super) fn plugin_instance(&self, instance_id: Uuid) -> Result<&Instance> {
        let instance = self
            .plugins
            .instances
            .get(&instance_id)
            .context("插件实例已禁用或重载")?;
        self.check_project(instance.info.identity.project)?;
        ensure!(
            !self.quiesced && !self.plugins.stopping,
            "项目正在关闭；插件操作已拒绝"
        );
        Ok(instance)
    }

    pub(super) fn plugin_tab(&self, tab_id: TabId, revision: u64) -> Result<WorkTab> {
        let workspace = self.store.workspace()?;
        let tab = workspace
            .tabs
            .into_iter()
            .find(|tab| tab.id == tab_id)
            .context("工作标签已关闭")?;
        ensure!(
            revision > 0 && tab.draft.revision == revision,
            "草稿已改变或尚未保存；未使用旧 revision"
        );
        Ok(tab)
    }

    pub(super) fn plugin_job_start(
        &mut self,
        operation_id: Uuid,
        instance: Option<Uuid>,
        cancel: CancellationToken,
        report: bool,
        future: impl Future<Output = Result<Output>> + Send + 'static,
    ) {
        let task = self.plugins.jobs.spawn(async move {
            Completion {
                operation_id,
                result: future.await.map_err(|error| error.to_string()),
            }
        });
        self.plugins.operations.insert(
            operation_id,
            Operation {
                project: self.project,
                instance,
                cancel,
                task: task.id(),
                report,
                cleanup_failed: false,
                background_passive: false,
                replaces_existing: false,
                binding: None,
            },
        );
    }

    pub(crate) async fn plugin_signal(&mut self, signal: Signal) {
        match signal {
            Signal::Job(result) => self.plugin_job(result).await,
            Signal::Service(call) => self.plugin_service(call).await,
            Signal::AutoReloadTick => {
                self.plugins.last_auto_reload_check = Some(Instant::now());
                let _ = self.check_auto_reload();
            }
            Signal::PassiveFlow(project, record) => {
                if project == self.project {
                    while let Some(done) =
                        std::future::poll_fn(|cx| match self.jobs.poll_join_next(cx) {
                            Poll::Ready(item) => Poll::Ready(item),
                            Poll::Pending => Poll::Ready(None),
                        })
                        .await
                    {
                        self.repeater_completed(done).await;
                    }
                    self.spawn_plugin_passive_checks(&record);
                }
            }
            Signal::Network(key, result) => {
                if let Some(network) = self.plugins.network.remove(&key) {
                    let result = result.map_err(|error| error.to_string());
                    if let Err(error) = self
                        .finish_plugin_network(network.call, network.flow_id, result)
                        .await
                    {
                        self.error("plugin_network", error).await;
                    }
                }
            }
            Signal::WorkspaceExpired(key) => {
                if let Some(pending) = self.plugins.workspace.remove(&key) {
                    pending.call.cancel.cancel();
                    let _ = pending.call.reply.send(Err(Failure::new(
                        ErrorCode::Cancelled,
                        "插件工作区请求已取消或超时；未回写草稿",
                    )));
                }
            }
            Signal::Worker(PluginEvent::Panel {
                identity,
                snapshot,
                editor_bases,
            }) => {
                self.plugin_panel(identity, snapshot, editor_bases).await;
                self.maybe_auto_reload_on_heartbeat();
            }
            Signal::Worker(PluginEvent::Error { identity, failure }) => {
                if self
                    .plugins
                    .instances
                    .get(&identity.instance_id)
                    .is_some_and(|instance| instance.info.identity == identity)
                {
                    self.emit(Event::PluginDiagnostic {
                        identity,
                        message: failure.message,
                    })
                    .await;
                }
            }
            Signal::Worker(PluginEvent::Stopped { identity, reason }) => {
                if self
                    .plugins
                    .instances
                    .get(&identity.instance_id)
                    .is_some_and(|instance| instance.info.identity == identity)
                {
                    let retired = self.take_plugin_family(identity.instance_id, &reason);
                    self.plugin_job_start(
                        Uuid::new_v4(),
                        Some(identity.instance_id),
                        CancellationToken::new(),
                        false,
                        retire(retired, None, None, reason),
                    );
                }
            }
        }
    }

    pub(super) async fn plugin_panel(
        &mut self,
        identity: ChannelIdentity,
        snapshot: std::sync::Arc<PanelSnapshot>,
        editor_bases: Vec<mio_plugin_api::panel::EditorSnapshot>,
    ) {
        if let Some(instance) = self.plugins.instances.get_mut(&identity.instance_id)
            && instance.info.identity == identity
            && identity.project == self.project
            && snapshot.revision > instance.panel.revision
        {
            instance.panel = snapshot.clone();
            self.emit(Event::PluginPanel {
                identity,
                snapshot,
                editor_bases,
            })
            .await;
        }
    }

    pub(crate) async fn plugin_workspace_changed(&mut self, workspace: &Workspace) {
        let closed_instances: Vec<_> = self
            .plugins
            .instances
            .iter()
            .filter_map(|(id, instance)| {
                instance
                    .info
                    .tab_id
                    .filter(|tab| !workspace.tabs.iter().any(|t| t.id == *tab))
                    .map(|_| *id)
            })
            .collect();
        for id in closed_instances {
            let retired = self.take_plugin_family(id, "消息编辑器所属工作标签已关闭");
            self.plugin_job_start(
                Uuid::new_v4(),
                Some(id),
                CancellationToken::new(),
                false,
                retire(retired, None, None, "消息编辑器所属工作标签已关闭".into()),
            );
        }
        let mut revocations = Vec::new();
        for instance in self.plugins.instances.values_mut() {
            let removed: Vec<_> = instance
                .resources
                .keys()
                .filter(|id| !workspace.tabs.iter().any(|tab| tab.id == **id))
                .copied()
                .collect();
            for id in &removed {
                instance.resources.remove(id);
            }
            if !removed.is_empty() {
                revocations.push((instance.worker.handle(), removed));
            }
        }
        for (handle, resources) in revocations {
            let keys: Vec<_> = self.plugins.workspace.iter().filter_map(|(key, pending)|
                (key.0 == handle.identity.instance_id && matches!(&pending.call.operation, ServiceOperation::UpdateDraft { tab_id, .. } if resources.contains(tab_id))).then_some(*key)).collect();
            for key in keys {
                let pending = self.plugins.workspace.remove(&key).unwrap();
                pending.call.cancel.cancel();
                let _ = pending.call.reply.send(Err(Failure::new(
                    ErrorCode::Stale,
                    "工作标签已关闭；插件资源已撤权",
                )));
            }
            self.plugin_job_start(
                Uuid::new_v4(),
                Some(handle.identity.instance_id),
                CancellationToken::new(),
                false,
                async move {
                    let mut first_error = None;
                    for id in resources {
                        if let Err(error) = handle.revoke_resource(id).await {
                            first_error.get_or_insert(error);
                        }
                    }
                    if let Some(error) = first_error {
                        return Err(error);
                    }
                    Ok(Output::Revoked)
                },
            );
        }
    }

    pub(crate) async fn stop_plugins(&mut self) -> Result<()> {
        self.plugins.stopping = true;
        self.plugins.auto_reload_timer = None;
        self.plugins.inspections.clear();
        while self.plugins.passive_rx.try_recv().is_ok() {}
        for operation in self.plugins.operations.values() {
            operation.cancel.cancel();
        }
        let ids: Vec<_> = self.plugins.instances.keys().copied().collect();
        let mut retired = Vec::new();
        for id in ids {
            retired.extend(self.take_plugin_family(id, "项目正在关闭；插件已停用"));
        }
        if !retired.is_empty() {
            self.plugin_job_start(
                Uuid::new_v4(),
                None,
                CancellationToken::new(),
                false,
                retire(retired, None, None, "项目正在关闭；插件已停用".into()),
            );
        }
        self.plugins.cancel_network();
        while self.plugins.has_resources() {
            let signal = self.plugins.next_signal().await;
            self.plugin_signal(signal).await;
        }
        self.plugins.stopping = false;
        if let Some(error) = self.plugins.cleanup_error.take() {
            anyhow::bail!(error);
        }
        Ok(())
    }
}
