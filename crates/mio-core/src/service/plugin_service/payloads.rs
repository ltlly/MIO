use super::*;

impl State {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_generate_payloads(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        tab_id: TabId,
        config: String,
        max_items: u32,
        cancel: CancellationToken,
    ) -> Result<()> {
        let instance = self.plugin_instance(instance_id)?;
        ensure!(
            instance
                .info
                .manifest
                .contributions
                .iter()
                .any(|c| c == "payload_generator")
                && instance.info.grants.contains(&Permission::PayloadProcess),
            "插件未声明 payload_generator 或未获授 payload.process"
        );
        let handle = instance.worker.handle();
        let identity = instance.info.identity;
        let capped = max_items.clamp(1, 10_000);
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let res = handle
                .call_payload(
                    protocol::PayloadCall {
                        op: protocol::PayloadOp::Generate as i32,
                        config,
                        inputs: vec![],
                        max_items: capped,
                    },
                    task_cancel,
                )
                .await?;
            Ok(Output::PayloadsGenerated {
                identity,
                tab_id,
                payloads: res.outputs,
            })
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_process_payloads(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        tab_id: TabId,
        position: Option<usize>,
        config: String,
        inputs: Vec<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let instance = self.plugin_instance(instance_id)?;
        ensure!(
            instance
                .info
                .manifest
                .contributions
                .iter()
                .any(|c| c == "payload_processor")
                && instance.info.grants.contains(&Permission::PayloadProcess),
            "插件未声明 payload_processor 或未获授 payload.process"
        );
        let handle = instance.worker.handle();
        let identity = instance.info.identity;
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let mut payloads = Vec::with_capacity(inputs.len());
            let mut batch = Vec::new();
            let mut batch_bytes = 0usize;
            for item in inputs {
                if !batch.is_empty()
                    && (batch.len() >= 2000 || batch_bytes + item.len() > 256 * 1024)
                {
                    let res = handle
                        .call_payload(
                            protocol::PayloadCall {
                                op: protocol::PayloadOp::Process as i32,
                                config: config.clone(),
                                inputs: std::mem::take(&mut batch),
                                max_items: 10_000,
                            },
                            task_cancel.clone(),
                        )
                        .await?;
                    payloads.extend(res.outputs);
                    batch_bytes = 0;
                }
                batch_bytes += item.len();
                batch.push(item);
            }
            if !batch.is_empty() {
                let res = handle
                    .call_payload(
                        protocol::PayloadCall {
                            op: protocol::PayloadOp::Process as i32,
                            config,
                            inputs: batch,
                            max_items: 10_000,
                        },
                        task_cancel,
                    )
                    .await?;
                payloads.extend(res.outputs);
            }
            Ok(Output::PayloadsProcessed {
                identity,
                tab_id,
                position,
                payloads,
            })
        });
        Ok(())
    }

    pub(super) fn start_reload_plugin(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        cancel: CancellationToken,
    ) -> Result<()> {
        ensure!(
            !self.quiesced && !self.plugins.stopping,
            "项目正在关闭；插件未重载"
        );
        let old = self.plugin_instance(instance_id)?;
        ensure!(
            old.info.tab_id.is_none() && !self.plugins.context_viewers.contains_key(&instance_id),
            "仅主工具插件实例可直接热重载"
        );
        self.plugin_idle(instance_id)?;
        self.plugin_capacity_for(true)?;
        let directory = old.info.directory.clone();
        let old_id = old.info.manifest.id.clone();
        let old_engine = old.info.manifest.engine.clone();
        let old_perms = old.package.manifest.permissions.clone();
        let old_package = old.package.clone();
        let old_auto_reload = old.auto_reload;
        let grants = old.info.grants.clone();
        let project = self.project;
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            if old_engine == "native" {
                tokio::task::spawn_blocking(move || old_package.unchanged()).await??;
            }
            let package =
                tokio::task::spawn_blocking(move || VerifiedPackage::read(&directory)).await??;
            ensure!(
                package.manifest.id == old_id && package.manifest.engine == old_engine,
                "插件 ID 或引擎已变更；请重新检查并确认授权"
            );
            ensure!(
                package
                    .manifest
                    .permissions
                    .iter()
                    .all(|p| old_perms.contains(p) && grants.contains(p)),
                "插件 manifest 请求了未确认的新增权限；已拒绝热重载提权，请重新检查并确认授权"
            );
            let allowed: Vec<_> = grants
                .into_iter()
                .filter(|p| package.manifest.permissions.contains(p))
                .collect();
            let trust = NativeTrust::confirmed(&package, allowed.iter().copied())?;
            let (services, receiver) = mpsc::channel(16);
            let mut worker =
                PluginSupervisor::start(package.clone(), project, trust.clone(), services).await?;
            if task_cancel.is_cancelled() {
                worker.stop().await?;
                anyhow::bail!("插件热重载已取消；新实例已回收");
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
                    auto_reload: old_auto_reload,
                    trust,
                    info,
                    panel,
                    source: None,
                    resources: HashMap::new(),
                },
                replace: Some(instance_id),
                reloaded: true,
                autoload: false,
                enable_installed: false,
                persist_install: None,
                open: None,
                context_open: None,
                response_open: None,
            })))
        });
        if let Some(op) = self.plugins.operations.get_mut(&operation_id) {
            op.replaces_existing = true;
            op.is_suite_start = true;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_invoke_context_menu(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        source: TrafficSource,
        action_id: String,
        selection_start: u64,
        selection_end: u64,
        selected_bytes: Vec<u8>,
        request: HttpRequest,
        response: Option<HttpResponse>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let instance = self.plugin_instance(instance_id)?;
        ensure!(
            instance
                .info
                .manifest
                .contributions
                .iter()
                .any(|c| c == "context_menu")
                && instance.info.grants.contains(&Permission::TrafficRead),
            "插件未声明 context_menu 贡献或未获授 traffic.read"
        );
        let can_open_repeater = instance.info.grants.contains(&Permission::RepeaterOpen);
        let can_write_draft = instance.info.grants.contains(&Permission::DraftWrite);
        let handle = instance.worker.handle();
        let identity = instance.info.identity;
        let src_dto = match source {
            TrafficSource::Proxy => protocol::SourceDto::Proxy,
            TrafficSource::Repeater => protocol::SourceDto::Repeater,
            TrafficSource::Intruder => protocol::SourceDto::Intruder,
            TrafficSource::Plugin(_) => protocol::SourceDto::Plugin,
            TrafficSource::Scanner => protocol::SourceDto::Scanner,
        } as i32;
        let action_id = if action_id.trim().is_empty() {
            "default".to_owned()
        } else {
            action_id
        };
        let selection_start = u32::try_from(selection_start).context("选区起点超出限额")?;
        let selection_end = u32::try_from(selection_end).context("选区终点超出限额")?;
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let res = handle
                .call_context_menu(
                    protocol::ContextMenuCall {
                        source: src_dto,
                        action_id,
                        selection_start,
                        selection_end,
                        selected_bytes,
                        request: Some(request.into()),
                        response: response.map(Into::into),
                    },
                    task_cancel,
                )
                .await?;
            let open_repeater = match res.open_repeater {
                Some(dto) => {
                    ensure!(
                        can_open_repeater,
                        "插件未获授 repeater.open；拒绝上下文菜单创建 Repeater 草稿"
                    );
                    Some(dto.try_into().map_err(anyhow::Error::msg)?)
                }
                None => None,
            };
            let replace_request = match res.replace_request {
                Some(raw) => {
                    ensure!(
                        can_write_draft,
                        "插件未获授 draft.write；拒绝上下文菜单替换编辑器请求"
                    );
                    ensure!(
                        raw.len() <= crate::http::MAX_BODY + 256 * 1024,
                        "插件替换的请求草稿超过大小限额"
                    );
                    Some(raw)
                }
                None => None,
            };
            Ok(Output::ContextMenuInvoked {
                identity,
                status_message: res.status_message,
                log_line: res.log_line,
                open_repeater,
                replace_request,
            })
        });
        Ok(())
    }
}
