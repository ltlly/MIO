use super::*;

impl State {
    pub(super) fn check_viewer_commit(
        &self,
        identity: ChannelIdentity,
        context_id: Uuid,
        revision: u64,
    ) -> Result<(), String> {
        self.plugin_instance(identity.instance_id)
            .map_err(|e| e.to_string())?;
        if self.plugins.context_viewers.get(&identity.instance_id)
            == Some(&(context_id, revision, false))
        {
            Ok(())
        } else {
            Err("插件上下文已撤权或只读；未应用".into())
        }
    }

    pub(super) async fn finish_viewer_open(
        &mut self,
        identity: ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        panel: std::sync::Arc<PanelSnapshot>,
        is_response: bool,
    ) -> Result<PluginOutcome, String> {
        self.plugin_instance(identity.instance_id)
            .map_err(|e| e.to_string())?;
        self.plugin_panel(identity, panel, vec![]).await;
        let instance = &self.plugins.instances[&identity.instance_id];
        let info = instance.info.clone();
        let panel = instance.panel.clone();
        Ok(if is_response {
            PluginOutcome::ResponseOpened {
                info,
                context_id,
                revision,
                panel,
            }
        } else {
            PluginOutcome::ContextOpened {
                info,
                context_id,
                revision,
                panel,
            }
        })
    }

    pub(super) fn plugin_idle(&self, instance_id: Uuid) -> Result<()> {
        ensure!(
            !self.plugins.operations.values().any(|operation| {
                !operation.background_passive && operation.instance == Some(instance_id)
            }),
            "此插件正在加载、提交或关闭；请等待当前操作"
        );
        Ok(())
    }

    pub(super) fn plugin_capacity(&self) -> Result<()> {
        self.plugin_capacity_for(false)
    }

    pub(super) fn plugin_capacity_for(&self, replacing: bool) -> Result<()> {
        let foreground_ops = self
            .plugins
            .operations
            .values()
            .filter(|op| !op.background_passive)
            .count();
        let in_flight_replaces = self
            .plugins
            .operations
            .values()
            .filter(|op| !op.background_passive && op.replaces_existing)
            .count();
        let occupied = (self.plugins.instances.len() + foreground_ops)
            .saturating_sub(in_flight_replaces + usize::from(replacing));
        ensure!(occupied < MAX_INSTANCES, "插件实例及启动任务达到 16 个限额");
        Ok(())
    }

    pub(super) fn maybe_auto_reload_on_heartbeat(&mut self) {
        let now = Instant::now();
        if self
            .plugins
            .last_auto_reload_check
            .is_some_and(|prev| now.saturating_duration_since(prev) < Duration::from_millis(200))
        {
            return;
        }
        self.plugins.last_auto_reload_check = Some(now);
        let _ = self.check_auto_reload();
    }

    pub(crate) fn check_auto_reload(&mut self) -> Vec<Uuid> {
        if self.quiesced || self.plugins.stopping || self.extensions_disabled {
            return Vec::new();
        }
        let candidates: Vec<Uuid> = self.plugins.order.clone();
        let mut triggered = Vec::new();
        for instance_id in candidates {
            if self.plugin_idle(instance_id).is_err() {
                continue;
            }
            let changed = {
                let Some(inst) = self.plugins.instances.get(&instance_id) else {
                    continue;
                };
                if !inst.auto_reload
                    || inst.info.tab_id.is_some()
                    || self.plugins.context_viewers.contains_key(&instance_id)
                {
                    continue;
                }
                let manifest_path = inst.info.directory.join("manifest.json");
                let entry_path = inst.info.directory.join(&inst.info.manifest.entry);
                let Ok(m_meta) = std::fs::metadata(&manifest_path) else {
                    continue;
                };
                let Ok(e_meta) = std::fs::metadata(&entry_path) else {
                    continue;
                };
                let Ok(m_time) = m_meta.modified() else {
                    continue;
                };
                let Ok(e_time) = e_meta.modified() else {
                    continue;
                };
                let m_size = m_meta.len();
                let e_size = e_meta.len();
                (m_time != inst.manifest_modified
                    || m_size != inst.manifest_size
                    || e_time != inst.entry_modified
                    || e_size != inst.entry_size)
                    .then_some((m_time, m_size, e_time, e_size))
            };
            let Some((m_time, m_size, e_time, e_size)) = changed else {
                continue;
            };
            if let Some(inst) = self.plugins.instances.get_mut(&instance_id) {
                inst.manifest_modified = m_time;
                inst.manifest_size = m_size;
                inst.entry_modified = e_time;
                inst.entry_size = e_size;
            }
            if self.plugins.operations.len() >= MAX_OPERATIONS {
                break;
            }
            let operation_id = Uuid::new_v4();
            if self
                .start_reload_plugin(operation_id, instance_id, CancellationToken::new())
                .is_ok()
            {
                triggered.push(operation_id);
            }
        }
        triggered
    }

    pub(super) fn start_plugin(
        &mut self,
        operation_id: Uuid,
        package: VerifiedPackage,
        trust: NativeTrust,
        grants: Vec<Permission>,
        target: LaunchTarget,
        cancel: CancellationToken,
    ) {
        let (replace, reloaded, source, open, context_open, response_open) = match target {
            LaunchTarget::Suite { replace, reloaded } => {
                (replace, reloaded, None, None, None, None)
            }
            LaunchTarget::Editor {
                source,
                tab_id,
                revision,
            } => (
                None,
                false,
                Some(source),
                Some((tab_id, revision)),
                None,
                None,
            ),
            LaunchTarget::ContextViewer {
                source,
                context_id,
                revision,
                read_only,
                request,
            } => (
                None,
                false,
                Some(source),
                None,
                Some((context_id, revision, read_only, request)),
                None,
            ),
            LaunchTarget::ResponseViewer {
                source,
                context_id,
                revision,
                read_only,
                response,
            } => (
                None,
                false,
                Some(source),
                None,
                None,
                Some((context_id, revision, read_only, response)),
            ),
        };
        let project = self.project;
        let task_cancel = cancel.clone();
        let replaces_existing = replace.is_some();
        self.plugin_job_start(operation_id, replace.or(source), cancel, true, async move {
            let (services, receiver) = mpsc::channel(16);
            let mut worker =
                PluginSupervisor::start(package.clone(), project, trust.clone(), services).await?;
            if task_cancel.is_cancelled() {
                worker.stop().await?;
                anyhow::bail!("插件加载已取消；新进程已回收");
            }
            let info = LoadedPlugin {
                identity: worker.identity,
                directory: package.directory.clone(),
                manifest: package.manifest.clone(),
                grants,
                tab_id: open.map(|(tab, _)| tab),
            };
            let panel = worker.initial_panel.clone();
            let manifest_modified = package.manifest_modified;
            let manifest_size = package.manifest_bytes.len() as u64;
            let entry_modified = package.modified;
            let entry_size = package.executable_size;
            Ok(Output::Started(Box::new(Candidate {
                instance: Instance {
                    worker,
                    services: receiver,
                    package,
                    manifest_modified,
                    manifest_size,
                    entry_modified,
                    entry_size,
                    auto_reload: info.manifest.engine == "wasm",
                    trust,
                    info,
                    panel,
                    source,
                    resources: HashMap::new(),
                },
                replace,
                reloaded,
                open,
                context_open,
                response_open,
            })))
        });
        if replaces_existing && let Some(op) = self.plugins.operations.get_mut(&operation_id) {
            op.replaces_existing = true;
        }
    }

    pub(super) fn start_plugin_open(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        tab_id: TabId,
        revision: u64,
        loading: bool,
        cancel: CancellationToken,
    ) -> Result<()> {
        let tab = self.plugin_tab(tab_id, revision)?;
        let instance = self.plugin_instance(instance_id)?;
        ensure!(
            instance.info.tab_id.is_none_or(|id| id == tab_id),
            "消息编辑器属于其它标签"
        );
        ensure!(
            instance.info.grants.contains(&Permission::TrafficRead),
            "插件尚未获准 traffic.read"
        );
        ensure!(
            instance.resources.contains_key(&tab_id) || instance.resources.len() < MAX_RESOURCES,
            "插件资源 handle 达到 64 个限额；未打开请求"
        );
        let request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version)?;
        let handle = instance.worker.handle();
        let identity = instance.info.identity;
        let previous = self
            .plugins
            .instances
            .get_mut(&instance_id)
            .unwrap()
            .resources
            .insert(tab_id, revision);
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let opened = handle.open_request(tab_id, revision, false, request);
            tokio::pin!(opened);
            let panel = tokio::select! {
                biased;
                _ = task_cancel.cancelled() => { handle.cancel(); anyhow::bail!("打开插件请求已取消；实例正在回收"); }
                result = &mut opened => result?,
            };
            Ok(Output::Opened { identity, tab_id, revision, loading, panel })
        });
        let operation = self.plugins.operations.get_mut(&operation_id).unwrap();
        operation.cleanup_failed = loading;
        operation.binding = Some((tab_id, revision, previous));
        Ok(())
    }

    pub(super) fn rollback_plugin_binding(&mut self, operation: &Operation) {
        if let Some((tab_id, revision, previous)) = operation.binding
            && let Some(instance_id) = operation.instance
            && let Some(instance) = self.plugins.instances.get_mut(&instance_id)
            && instance.resources.get(&tab_id) == Some(&revision)
        {
            if let Some(previous) = previous {
                instance.resources.insert(tab_id, previous);
            } else {
                instance.resources.remove(&tab_id);
            }
        }
    }

    pub(super) fn take_plugin_family(
        &mut self,
        instance_id: Uuid,
        reason: &str,
    ) -> Vec<(Instance, Vec<Network>)> {
        let ids: Vec<_> = self
            .plugins
            .instances
            .iter()
            .filter_map(|(id, instance)| {
                (*id == instance_id || instance.source == Some(instance_id)).then_some(*id)
            })
            .collect();
        let mut retired = Vec::new();
        for id in ids {
            self.plugins.order.retain(|item| *item != id);
            self.plugins.context_viewers.remove(&id);
            let mut instance = self.plugins.instances.remove(&id).unwrap();
            instance.worker.handle().cancel();
            for operation in self.plugins.operations.values() {
                if operation.instance == Some(id) {
                    operation.cancel.cancel();
                }
            }
            let pending: Vec<_> = self
                .plugins
                .workspace
                .keys()
                .filter(|(owner, _)| *owner == id)
                .copied()
                .collect();
            for key in pending {
                let request = self.plugins.workspace.remove(&key).unwrap().call;
                request.cancel.cancel();
                let _ = request
                    .reply
                    .send(Err(Failure::new(ErrorCode::Cancelled, reason)));
            }
            while let Ok(request) = instance.services.try_recv() {
                request.cancel.cancel();
                let _ = request
                    .reply
                    .send(Err(Failure::new(ErrorCode::Cancelled, reason)));
            }
            let keys: Vec<_> = self
                .plugins
                .network
                .keys()
                .filter(|(owner, _)| *owner == id)
                .copied()
                .collect();
            let network = keys
                .into_iter()
                .map(|key| {
                    let job = self.plugins.network.remove(&key).unwrap();
                    job.call.cancel.cancel();
                    job
                })
                .collect();
            retired.push((instance, network));
        }
        self.sync_proxy_plugin_hooks();
        retired
    }
}

pub(super) async fn retire(
    instances: Vec<(Instance, Vec<Network>)>,
    candidate: Option<Box<Candidate>>,
    disabled: Option<ChannelIdentity>,
    reason: String,
) -> Result<Output> {
    let mut tasks = JoinSet::new();
    for (mut instance, network) in instances {
        tasks.spawn(async move {
            instance.worker.handle().cancel();
            for job in &network {
                job.call.cancel.cancel();
            }
            let stop = instance.worker.stop();
            let io = async {
                let mut results = Vec::new();
                for job in network {
                    results.push((
                        job.call,
                        job.flow_id,
                        job.task.await.map_err(|error| {
                            format!("插件网络任务异常结束，结果未知，未重发：{error}")
                        }),
                    ));
                }
                results
            };
            let (result, network) = tokio::join!(stop, io);
            Retired {
                identity: instance.info.identity,
                result: result.map_err(|error| error.to_string()),
                network,
            }
        });
    }
    let mut retired = Vec::new();
    while let Some(result) = tasks.join_next().await {
        retired.push(result.context("插件回收任务异常退出")?);
    }
    Ok(Output::Retired {
        retired,
        candidate,
        disabled,
        reason,
    })
}
