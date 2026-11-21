use super::*;

type PluginTarget<'a> = (
    crate::plugins::PluginHandle,
    ChannelIdentity,
    u64,
    Option<TabId>,
    &'a HashMap<Uuid, u64>,
    &'a PanelSnapshot,
);

impl PluginState {
    pub(crate) fn with_preferences(prefs: PluginPreferencesStore) -> Self {
        Self::with_stores(prefs, PluginRegistryStore::new(None))
    }

    pub(crate) fn with_stores(
        prefs: PluginPreferencesStore,
        registry: PluginRegistryStore,
    ) -> Self {
        Self {
            prefs,
            registry,
            last_auto_reload_check: None,
            auto_reload_timer: None,
            inspections: HashMap::new(),
            instances: HashMap::new(),
            multiplexed: HashMap::new(),
            panel_routes: HashMap::new(),
            closing_panels: HashSet::new(),
            next_panel_id: mio_plugin_api::protocol::PANEL_ID + 1,
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
        !self.instances.is_empty()
            || !self.multiplexed.is_empty()
            || !self.jobs.is_empty()
            || !self.network.is_empty()
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

    pub(super) fn plugin_target(&self, instance_id: Uuid) -> Result<PluginTarget<'_>> {
        ensure!(
            !self.quiesced && !self.plugins.stopping,
            "项目正在关闭；插件操作已拒绝"
        );
        if let Some(instance) = self.plugins.instances.get(&instance_id) {
            self.check_project(instance.info.identity.project)?;
            return Ok((
                instance.worker.handle(),
                instance.info.identity,
                mio_plugin_api::protocol::PANEL_ID,
                instance.info.tab_id,
                &instance.resources,
                &instance.panel,
            ));
        }
        let child = self
            .plugins
            .multiplexed
            .get(&instance_id)
            .context("插件实例已禁用或重载")?;
        self.check_project(child.info.identity.project)?;
        let parent = self
            .plugins
            .instances
            .get(&child.source_instance)
            .context("宿主插件实例已禁用或重载")?;
        self.check_project(parent.info.identity.project)?;
        Ok((
            parent.worker.handle(),
            child.info.identity,
            child.panel_id,
            child.info.tab_id,
            &child.resources,
            &child.panel,
        ))
    }

    pub(super) fn check_plugin_known(&self, instance_id: Uuid) -> Result<()> {
        self.plugin_target(instance_id).map(|_| ())
    }

    pub(super) fn plugin_tab(&self, tab_id: TabId, revision: u64) -> Result<WorkTab> {
        let workspace = self.store.metadata().workspace()?;
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
                is_suite_start: false,
                replaces_existing: false,
                autoload_plugin_id: None,
                binding: None,
                multiplexed_cleanup_on_fail: None,
                closing_panel: None,
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
                panel_id,
                snapshot,
                editor_bases,
                synced_ui_seq,
                rejected_ui_seqs,
            }) => {
                self.plugin_panel(
                    identity,
                    panel_id,
                    snapshot,
                    editor_bases,
                    synced_ui_seq,
                    rejected_ui_seqs,
                )
                .await;
                self.maybe_auto_reload_on_heartbeat();
            }
            Signal::Worker(PluginEvent::PanelSynced {
                identity,
                panel_id,
                snapshot,
                synced_ui_seq,
                rejected_ui_seqs,
            }) => {
                self.plugin_panel_synced(
                    identity,
                    panel_id,
                    snapshot,
                    synced_ui_seq,
                    rejected_ui_seqs,
                )
                .await;
            }
            Signal::Worker(PluginEvent::Error {
                identity,
                panel_id,
                failure,
            }) => {
                let target_identity = if panel_id == mio_plugin_api::protocol::PANEL_ID {
                    self.plugins
                        .instances
                        .get(&identity.instance_id)
                        .filter(|instance| instance.info.identity == identity)
                        .map(|_| identity)
                } else {
                    self.plugins
                        .instances
                        .get(&identity.instance_id)
                        .filter(|instance| instance.info.identity == identity)
                        .and_then(|_| {
                            self.plugins
                                .panel_routes
                                .get(&(identity.instance_id, panel_id))
                        })
                        .and_then(|virtual_id| self.plugins.multiplexed.get(virtual_id))
                        .map(|child| child.info.identity)
                };
                if let Some(target_identity) = target_identity {
                    self.emit(Event::PluginDiagnostic {
                        identity: target_identity,
                        message: failure.message,
                    })
                    .await;
                }
            }
            Signal::Worker(PluginEvent::PanelClosed { identity, panel_id }) => {
                if self
                    .plugins
                    .instances
                    .get(&identity.instance_id)
                    .is_some_and(|instance| instance.info.identity == identity)
                {
                    self.plugins
                        .closing_panels
                        .remove(&(identity.instance_id, panel_id));
                }
            }
            Signal::Worker(PluginEvent::Stopped { identity, reason }) => {
                if self
                    .plugins
                    .instances
                    .get(&identity.instance_id)
                    .is_some_and(|instance| instance.info.identity == identity)
                {
                    let (retired, retired_multiplexed) =
                        self.take_plugin_family(identity.instance_id, &reason);
                    self.plugin_job_start(
                        Uuid::new_v4(),
                        Some(identity.instance_id),
                        CancellationToken::new(),
                        false,
                        retire(retired, retired_multiplexed, None, None, reason),
                    );
                }
            }
        }
    }

    pub(super) async fn plugin_panel(
        &mut self,
        identity: ChannelIdentity,
        panel_id: u64,
        snapshot: std::sync::Arc<PanelSnapshot>,
        editor_bases: Vec<mio_plugin_api::panel::EditorSnapshot>,
        synced_ui_seq: u64,
        rejected_ui_seqs: Vec<u64>,
    ) {
        if identity.project != self.project {
            return;
        }
        if panel_id == mio_plugin_api::protocol::PANEL_ID {
            if let Some(instance) = self.plugins.instances.get_mut(&identity.instance_id)
                && instance.info.identity == identity
                && snapshot.revision > instance.panel.revision
            {
                instance.panel = snapshot.clone();
                self.emit(Event::PluginPanel {
                    identity,
                    snapshot,
                    editor_bases,
                    synced_ui_seq: Some(synced_ui_seq),
                    rejected_ui_seqs,
                })
                .await;
            }
        } else if self
            .plugins
            .instances
            .get(&identity.instance_id)
            .is_some_and(|instance| instance.info.identity == identity)
            && let Some(virtual_id) = self
                .plugins
                .panel_routes
                .get(&(identity.instance_id, panel_id))
                .copied()
            && let Some(child) = self.plugins.multiplexed.get_mut(&virtual_id)
            && snapshot.revision > child.panel.revision
        {
            child.panel = snapshot.clone();
            let child_identity = child.info.identity;
            self.emit(Event::PluginPanel {
                identity: child_identity,
                snapshot,
                editor_bases,
                synced_ui_seq: Some(synced_ui_seq),
                rejected_ui_seqs,
            })
            .await;
        }
    }

    pub(super) async fn plugin_panel_synced(
        &mut self,
        identity: ChannelIdentity,
        panel_id: u64,
        snapshot: std::sync::Arc<PanelSnapshot>,
        synced_ui_seq: u64,
        rejected_ui_seqs: Vec<u64>,
    ) {
        if identity.project != self.project {
            return;
        }
        if panel_id == mio_plugin_api::protocol::PANEL_ID {
            if let Some(instance) = self.plugins.instances.get_mut(&identity.instance_id)
                && instance.info.identity == identity
                && snapshot.revision >= instance.panel.revision
            {
                instance.panel = snapshot.clone();
                self.emit(Event::PluginPanel {
                    identity,
                    snapshot,
                    editor_bases: vec![],
                    synced_ui_seq: Some(synced_ui_seq),
                    rejected_ui_seqs,
                })
                .await;
            }
        } else if self
            .plugins
            .instances
            .get(&identity.instance_id)
            .is_some_and(|instance| instance.info.identity == identity)
            && let Some(virtual_id) = self
                .plugins
                .panel_routes
                .get(&(identity.instance_id, panel_id))
                .copied()
            && let Some(child) = self.plugins.multiplexed.get_mut(&virtual_id)
            && snapshot.revision >= child.panel.revision
        {
            child.panel = snapshot.clone();
            let child_identity = child.info.identity;
            self.emit(Event::PluginPanel {
                identity: child_identity,
                snapshot,
                editor_bases: vec![],
                synced_ui_seq: Some(synced_ui_seq),
                rejected_ui_seqs,
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
            let (retired, retired_multiplexed) =
                self.take_plugin_family(id, "消息编辑器所属工作标签已关闭");
            self.plugin_job_start(
                Uuid::new_v4(),
                Some(id),
                CancellationToken::new(),
                false,
                retire(
                    retired,
                    retired_multiplexed,
                    None,
                    None,
                    "消息编辑器所属工作标签已关闭".into(),
                ),
            );
        }
        let closed_multiplexed: Vec<_> = self
            .plugins
            .multiplexed
            .iter()
            .filter_map(|(id, child)| {
                child
                    .info
                    .tab_id
                    .filter(|tab| !workspace.tabs.iter().any(|t| t.id == *tab))
                    .map(|tab| (*id, tab))
            })
            .collect();
        let mut child_revocations = Vec::new();
        for (virtual_id, tab_id) in closed_multiplexed {
            if let Some((child, parent_handle)) =
                self.take_multiplexed_child(virtual_id, "消息编辑器所属工作标签已关闭")
            {
                child_revocations.push((child, parent_handle, tab_id));
            }
        }
        let mut stopped_children = Vec::with_capacity(child_revocations.len());
        for (child, parent_handle, tab_id) in child_revocations {
            stopped_children.push(child.info.identity);
            if let Some(handle) = parent_handle {
                let panel_id = child.panel_id;
                let source_instance = child.source_instance;
                let pending_revoke = handle.begin_revoke_panel_resource(panel_id, tab_id, true);
                self.plugins.closing_panels.insert((source_instance, panel_id));
                let job_id = Uuid::new_v4();
                self.plugin_job_start(
                    job_id,
                    Some(source_instance),
                    CancellationToken::new(),
                    false,
                    async move {
                        pending_revoke.wait().await?;
                        Ok(Output::Revoked)
                    },
                );
                if let Some(op) = self.plugins.operations.get_mut(&job_id) {
                    op.closing_panel = Some((source_instance, panel_id));
                }
            }
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
                (key.0 == handle.identity.instance_id && pending.call.panel_id == mio_plugin_api::protocol::PANEL_ID && matches!(&pending.call.operation, ServiceOperation::UpdateDraft { tab_id, .. } if resources.contains(tab_id))).then_some(*key)).collect();
            for key in keys {
                let pending = self.plugins.workspace.remove(&key).unwrap();
                pending.call.cancel.cancel();
                let _ = pending.call.reply.send(Err(Failure::new(
                    ErrorCode::Stale,
                    "工作标签已关闭；插件资源已撤权",
                )));
            }
            let pending_revoke =
                handle.begin_revoke_panel_resources(mio_plugin_api::protocol::PANEL_ID, resources);
            self.plugin_job_start(
                Uuid::new_v4(),
                Some(handle.identity.instance_id),
                CancellationToken::new(),
                false,
                async move {
                    pending_revoke.wait().await?;
                    Ok(Output::Revoked)
                },
            );
        }
        let mut child_resource_revocations = Vec::new();
        for child in self.plugins.multiplexed.values_mut() {
            let removed: Vec<_> = child
                .resources
                .keys()
                .filter(|id| **id != child.primary_resource && !workspace.tabs.iter().any(|tab| tab.id == **id))
                .copied()
                .collect();
            for id in &removed {
                child.resources.remove(id);
            }
            if !removed.is_empty()
                && let Some(parent) = self.plugins.instances.get(&child.source_instance)
            {
                child_resource_revocations.push((
                    child.source_instance,
                    child.panel_id,
                    parent.worker.handle(),
                    removed,
                ));
            }
        }
        for (source_instance, panel_id, handle, resources) in child_resource_revocations {
            let keys: Vec<_> = self
                .plugins
                .workspace
                .iter()
                .filter_map(|(key, pending)| {
                    (key.0 == source_instance
                        && pending.call.panel_id == panel_id
                        && matches!(
                            &pending.call.operation,
                            ServiceOperation::UpdateDraft { tab_id, .. } if resources.contains(tab_id)
                        ))
                    .then_some(*key)
                })
                .collect();
            for key in keys {
                let pending = self.plugins.workspace.remove(&key).unwrap();
                pending.call.cancel.cancel();
                let _ = pending.call.reply.send(Err(Failure::new(
                    ErrorCode::Stale,
                    "工作标签已关闭；子面板插件资源已撤权",
                )));
            }
            let pending_revoke = handle.begin_revoke_panel_resources(panel_id, resources);
            self.plugin_job_start(
                Uuid::new_v4(),
                Some(source_instance),
                CancellationToken::new(),
                false,
                async move {
                    pending_revoke.wait().await?;
                    Ok(Output::Revoked)
                },
            );
        }
        for identity in stopped_children {
            self.emit(Event::PluginStopped {
                identity,
                reason: "消息编辑器所属工作标签已关闭".into(),
            })
            .await;
        }
    }

    pub(crate) async fn stop_plugins(&mut self) -> Result<()> {
        self.plugins.stopping = true;
        self.plugins.auto_reload_timer = None;
        self.plugins.inspections.clear();
        for operation in self.plugins.operations.values() {
            operation.cancel.cancel();
        }
        let ids: Vec<_> = self.plugins.instances.keys().copied().collect();
        let mut retired = Vec::new();
        let mut retired_multiplexed = Vec::new();
        for id in ids {
            let (insts, mults) = self.take_plugin_family(id, "项目正在关闭；插件已停用");
            retired.extend(insts);
            retired_multiplexed.extend(mults);
        }
        if !retired.is_empty() || !retired_multiplexed.is_empty() {
            self.plugin_job_start(
                Uuid::new_v4(),
                None,
                CancellationToken::new(),
                false,
                retire(
                    retired,
                    retired_multiplexed,
                    None,
                    None,
                    "项目正在关闭；插件已停用".into(),
                ),
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
