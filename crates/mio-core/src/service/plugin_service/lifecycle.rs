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
                let Some((m_time, m_size, e_time, e_size)) = inst.package.poll_source_fingerprint() else {
                    continue;
                };
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
        let is_suite_target = matches!(target, LaunchTarget::Suite { .. });
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
        let registry_opt = (is_suite_target && self.plugins.registry.is_persistent())
            .then(|| self.plugins.registry.clone());
        self.plugin_job_start(operation_id, replace.or(source), cancel, true, async move {
            let mut staging_opt = None;
            let (package, trust, original_source) = if let Some(registry) = registry_opt {
                let original_source = package.directory.clone();
                let source_pkg = package;
                let source_clone = source_pkg.clone();
                match tokio::task::spawn_blocking(move || {
                    registry.stage_install_package(&source_clone)
                })
                .await??
                {
                    Some((installed_pkg, staging)) => {
                        staging_opt = Some(staging);
                        let installed_trust =
                            match NativeTrust::confirmed(&installed_pkg, grants.iter().copied()) {
                                Ok(t) => t,
                                Err(err) => {
                                    if let Some(s) = staging_opt.take() {
                                        s.rollback();
                                    }
                                    return Err(err);
                                }
                            };
                        (installed_pkg, installed_trust, Some(original_source))
                    }
                    None => (source_pkg, trust, None),
                }
            } else {
                (package, trust, None)
            };
            let (services, receiver) = mpsc::channel(16);
            let mut worker = match PluginSupervisor::start(
                package.clone(),
                project,
                trust.clone(),
                services,
            )
            .await
            {
                Ok(worker) => worker,
                Err(err) => {
                    if let Some(s) = staging_opt.take() {
                        s.rollback();
                    }
                    return Err(err);
                }
            };
            if task_cancel.is_cancelled() {
                worker.stop().await?;
                if let Some(s) = staging_opt.take() {
                    s.rollback();
                }
                anyhow::bail!("插件加载已取消；新进程已回收");
            }
            let persist_install = staging_opt.map(|staging| PersistInstallCommit {
                staging,
                original_source,
            });
            let info = LoadedPlugin {
                identity: worker.identity,
                directory: package.directory.clone(),
                manifest: package.manifest.clone(),
                grants,
                tab_id: open.map(|(tab, _)| tab),
            };
            let panel = worker.initial_panel.clone();
            let (manifest_modified, manifest_size, entry_modified, entry_size) =
                package.poll_source_fingerprint().unwrap_or((
                    package.manifest_modified,
                    package.manifest_bytes.len() as u64,
                    package.modified,
                    package.executable_size,
                ));
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
                autoload: false,
                enable_installed: false,
                persist_install,
                open,
                context_open,
                response_open,
            })))
        });
        if replaces_existing && let Some(op) = self.plugins.operations.get_mut(&operation_id) {
            op.replaces_existing = true;
        }
    }

    pub(super) fn start_installed_plugin(
        &mut self,
        operation_id: Uuid,
        entry: InstalledPluginEntry,
        stored_dir: PathBuf,
        autoload: bool,
        cancel: CancellationToken,
    ) {
        let project = self.project;
        let task_cancel = cancel.clone();
        let plugin_id = entry.id.clone();
        self.plugin_job_start(operation_id, None, cancel, !autoload, async move {
            let package =
                tokio::task::spawn_blocking(move || VerifiedPackage::read(&stored_dir)).await??;
            ensure!(
                package.manifest.id == entry.id && package.manifest.engine == entry.engine,
                "持久化插件清单与已保存记录不匹配；拒绝自动加载"
            );
            let allowed: Vec<_> = entry
                .grants
                .into_iter()
                .filter(|p| package.manifest.permissions.contains(p))
                .collect();
            let trust = NativeTrust::confirmed(&package, allowed.iter().copied())?;
            let (services, receiver) = mpsc::channel(16);
            let mut worker =
                PluginSupervisor::start(package.clone(), project, trust.clone(), services).await?;
            if task_cancel.is_cancelled() {
                worker.stop().await?;
                anyhow::bail!("持久化插件加载已取消；新进程已回收");
            }
            let info = LoadedPlugin {
                identity: worker.identity,
                directory: package.directory.clone(),
                manifest: package.manifest.clone(),
                grants: allowed,
                tab_id: None,
            };
            let panel = worker.initial_panel.clone();
            let (manifest_modified, manifest_size, entry_modified, entry_size) =
                package.poll_source_fingerprint().unwrap_or((
                    package.manifest_modified,
                    package.manifest_bytes.len() as u64,
                    package.modified,
                    package.executable_size,
                ));
            Ok(Output::Started(Box::new(Candidate {
                instance: Instance {
                    worker,
                    services: receiver,
                    package,
                    manifest_modified,
                    manifest_size,
                    entry_modified,
                    entry_size,
                    auto_reload: entry.auto_reload,
                    trust,
                    info,
                    panel,
                    source: None,
                    resources: HashMap::new(),
                },
                replace: None,
                reloaded: false,
                autoload,
                enable_installed: !autoload,
                persist_install: None,
                open: None,
                context_open: None,
                response_open: None,
            })))
        });
        if autoload && let Some(op) = self.plugins.operations.get_mut(&operation_id) {
            op.autoload_plugin_id = Some(plugin_id);
        }
    }

    pub(in crate::service) async fn restore_installed_plugins(&mut self) {
        if !self.plugins.registry.is_persistent() {
            return;
        }
        let Some(plugins_dir) = self.plugins.registry.root_dir().map(Path::to_path_buf) else {
            return;
        };
        let entries = match self.plugins.registry.list() {
            Ok(entries) => entries,
            Err(err) => {
                self.error("plugin_autoload", err).await;
                return;
            }
        };
        let summaries = entries
            .iter()
            .map(|entry| {
                let dir = self
                    .plugins
                    .registry
                    .package_path(entry)
                    .unwrap_or_else(|_| plugins_dir.join("packages").join(&entry.package_dir));
                let (entry_path, contributions) =
                    crate::plugins::read_manifest(&dir.join("manifest.json"))
                        .ok()
                        .and_then(|bytes| {
                            serde_json::from_slice::<crate::plugins::PluginManifest>(&bytes).ok()
                        })
                        .map(|m| (m.entry, m.contributions))
                        .unwrap_or_else(|| (PathBuf::new(), Vec::new()));
                InstalledPluginInfo {
                    id: entry.id.clone(),
                    version: entry.version.clone(),
                    engine: entry.engine.clone(),
                    directory: dir,
                    entry: entry_path,
                    grants: entry.grants.clone(),
                    contributions,
                    enabled: entry.enabled,
                    auto_reload: entry.auto_reload,
                }
            })
            .collect();
        self.emit(Event::InstalledPlugins {
            project: self.project,
            plugins_dir,
            extensions_disabled: self.extensions_disabled,
            plugins: summaries,
        })
        .await;
        if self.extensions_disabled || self.quiesced || self.plugins.stopping {
            return;
        }
        for entry in entries.into_iter().filter(|e| e.enabled) {
            if let Err(err) = self.plugin_capacity() {
                self.emit(Event::PluginAutoLoadFailed {
                    project: self.project,
                    plugin_id: entry.id,
                    message: err.to_string(),
                })
                .await;
                continue;
            }
            if self.plugins.operations.len() >= MAX_OPERATIONS {
                self.emit(Event::PluginAutoLoadFailed {
                    project: self.project,
                    plugin_id: entry.id,
                    message: "插件后台操作达到 32 个限额，未能自动加载".into(),
                })
                .await;
                continue;
            }
            let stored_dir = match self.plugins.registry.package_path(&entry) {
                Ok(dir) => dir,
                Err(err) => {
                    self.emit(Event::PluginAutoLoadFailed {
                        project: self.project,
                        plugin_id: entry.id,
                        message: err.to_string(),
                    })
                    .await;
                    continue;
                }
            };
            self.start_installed_plugin(
                Uuid::new_v4(),
                entry,
                stored_dir,
                true,
                CancellationToken::new(),
            );
        }
    }

    pub(super) fn sort_plugin_order_by_registry(&mut self) {
        let Ok(entries) = self.plugins.registry.list() else {
            return;
        };
        if entries.is_empty() {
            return;
        }
        let mut rank = HashMap::with_capacity(entries.len());
        for (idx, entry) in entries.into_iter().enumerate() {
            rank.insert(entry.id, idx);
        }
        let instances = &self.plugins.instances;
        self.plugins.order.sort_by_key(|id| {
            instances
                .get(id)
                .and_then(|inst| rank.get(&inst.info.manifest.id).copied())
                .map_or((1usize, 0usize), |idx| (0usize, idx))
        });
    }

    pub(super) fn sync_registry_order(&self) {
        if !self.plugins.registry.is_persistent() {
            return;
        }
        let ordered_ids: Vec<String> = self
            .plugins
            .order
            .iter()
            .filter_map(|id| {
                self.plugins
                    .instances
                    .get(id)
                    .filter(|inst| {
                        inst.info.tab_id.is_none() && !self.plugins.context_viewers.contains_key(id)
                    })
                    .map(|inst| inst.info.manifest.id.clone())
            })
            .collect();
        let _ = self.plugins.registry.reorder_by_ids(&ordered_ids);
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
    retire_with_uninstall(instances, candidate, disabled, None, reason).await
}

pub(super) async fn retire_with_uninstall(
    instances: Vec<(Instance, Vec<Network>)>,
    candidate: Option<Box<Candidate>>,
    disabled: Option<ChannelIdentity>,
    uninstalled_plugin_id: Option<String>,
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
        uninstalled_plugin_id,
        reason,
    })
}
