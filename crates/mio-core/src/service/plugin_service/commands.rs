use super::*;

impl State {
    pub(crate) async fn plugin_command(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        action: PluginAction,
    ) -> Result<()> {
        self.check_project(project)?;
        ensure!(!operation_id.is_nil(), "插件操作 ID 无效");
        ensure!(
            !self.plugins.operations.contains_key(&operation_id),
            "插件操作 ID 重复"
        );
        if let PluginAction::Cancel {
            operation_id: target,
        } = action
        {
            let operation = self
                .plugins
                .operations
                .get(&target)
                .context("插件操作已结束")?;
            ensure!(
                operation.project == project && operation.report,
                "不能取消其它项目的操作"
            );
            operation.cancel.cancel();
            self.plugin_result(
                project,
                operation_id,
                Ok(PluginOutcome::CancellationRequested),
            )
            .await;
            return Ok(());
        }
        if let PluginAction::ApplyWorkspace {
            instance_id,
            request_id,
            workspace,
            tab_id,
        } = action
        {
            return self
                .plugin_apply_workspace(operation_id, (instance_id, request_id), workspace, tab_id)
                .await;
        }
        if let PluginAction::RejectWorkspace {
            instance_id,
            request_id,
            message,
        } = action
        {
            let pending = self
                .plugins
                .workspace
                .remove(&(instance_id, request_id))
                .context("插件工作区操作已结束")?;
            let _ = pending.call.reply.send(Err(Failure::new(
                ErrorCode::Stale,
                message.chars().take(1024).collect::<String>(),
            )));
            self.plugin_result(project, operation_id, Ok(PluginOutcome::WorkspaceRejected))
                .await;
            return Ok(());
        }
        if self.extensions_disabled && !matches!(action, PluginAction::Disable { .. }) {
            anyhow::bail!("当前项目已禁用扩展（Disable extensions）；插件未检查、加载或执行");
        }
        ensure!(
            self.plugins.operations.len() < MAX_OPERATIONS,
            "插件后台操作达到 32 个限额"
        );
        let cancel = CancellationToken::new();
        match action {
            PluginAction::Inspect { directory } => {
                ensure!(
                    self.plugins.inspections.len() < 16,
                    "已检查的插件包达到 16 个限额；请加载或重启项目后重试"
                );
                self.plugin_job_start(operation_id, None, cancel, true, async move {
                    Ok(Output::Inspected(
                        tokio::task::spawn_blocking(move || VerifiedPackage::read(&directory))
                            .await??,
                    ))
                });
            }
            PluginAction::Load {
                inspection_id,
                native_trusted,
                grants,
                replace_instance,
            } => {
                ensure!(
                    !self.quiesced && !self.plugins.stopping,
                    "项目正在关闭；插件未加载"
                );
                ensure!(
                    native_trusted,
                    "必须明确确认：native worker 是未受 OS 沙箱限制的可信本地程序"
                );
                let package = self
                    .plugins
                    .inspections
                    .get(&inspection_id)
                    .context("插件检查结果已失效")?
                    .clone();
                let trust = NativeTrust::confirmed(&package, grants.iter().copied())?;
                if let Some(id) = replace_instance {
                    let old = self.plugin_instance(id)?;
                    ensure!(
                        old.info.tab_id.is_none() && old.info.manifest.id == package.manifest.id,
                        "重载目标不是同一 Suite 插件"
                    );
                    self.plugin_idle(id)?;
                }
                self.plugin_capacity_for(replace_instance.is_some())?;
                self.plugins.inspections.remove(&inspection_id);
                self.start_plugin(
                    operation_id,
                    package,
                    trust,
                    grants,
                    LaunchTarget::Suite {
                        replace: replace_instance,
                        reloaded: false,
                    },
                    cancel,
                );
            }
            PluginAction::CreateEditor {
                source_instance,
                tab_id,
                draft_revision,
            } => {
                self.plugin_tab(tab_id, draft_revision)?;
                let source = self.plugin_instance(source_instance)?;
                ensure!(
                    source.info.tab_id.is_none()
                        && source
                            .info
                            .manifest
                            .contributions
                            .iter()
                            .any(|c| c == "message_editor"),
                    "此插件未提供独立消息编辑器"
                );
                ensure!(
                    source.info.grants.contains(&Permission::TrafficRead),
                    "自定义编辑器需要单独授权 traffic.read"
                );
                let (package, trust, grants) = (
                    source.package.clone(),
                    source.trust.clone(),
                    source.info.grants.clone(),
                );
                self.plugin_capacity()?;
                self.start_plugin(
                    operation_id,
                    package,
                    trust,
                    grants,
                    LaunchTarget::Editor {
                        source: source_instance,
                        tab_id,
                        revision: draft_revision,
                    },
                    cancel,
                );
            }
            PluginAction::Disable { instance_id } => {
                let identity = self.plugin_instance(instance_id)?.info.identity;
                let retired = self.take_plugin_family(instance_id, "插件已禁用");
                self.plugin_job_start(
                    operation_id,
                    Some(instance_id),
                    cancel,
                    true,
                    retire(retired, None, Some(identity), "插件已禁用".into()),
                );
            }
            PluginAction::Ui {
                instance_id,
                panel_revision,
                event,
            } => {
                let instance = self.plugin_instance(instance_id)?;
                ensure!(
                    panel_revision == instance.panel.revision,
                    "插件面板已更新；操作未提交，请在当前视图重试"
                );
                instance.worker.handle().try_event(event)?;
                self.plugin_result(project, operation_id, Ok(PluginOutcome::EventQueued))
                    .await;
            }
            PluginAction::OpenRequest {
                instance_id,
                tab_id,
                draft_revision,
            } => {
                self.plugin_idle(instance_id)?;
                self.start_plugin_open(
                    operation_id,
                    instance_id,
                    tab_id,
                    draft_revision,
                    false,
                    cancel,
                )?;
            }
            PluginAction::CommitEditor {
                instance_id,
                tab_id,
                draft_revision,
                editor,
            } => {
                self.plugin_idle(instance_id)?;
                let tab = self.plugin_tab(tab_id, draft_revision)?;
                let instance = self.plugin_instance(instance_id)?;
                ensure!(
                    instance.info.tab_id.is_none_or(|id| id == tab_id),
                    "消息编辑器属于其它标签"
                );
                ensure!(
                    instance.resources.get(&tab_id) == Some(&draft_revision),
                    "插件请求上下文已改变；未提交旧稿"
                );
                let handle = instance.worker.handle();
                let identity = instance.info.identity;
                let request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version)?;
                let task_cancel = cancel.clone();
                self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
                    let request = handle
                        .commit(
                            CommitEditor {
                                resource_id: tab_id.as_bytes().to_vec(),
                                message_revision: draft_revision,
                                request: Some(request.into()),
                                editor: Some(editor),
                            },
                            task_cancel,
                        )
                        .await?;
                    Ok(Output::Committed {
                        identity,
                        tab_id,
                        revision: draft_revision,
                        request,
                    })
                });
            }
            PluginAction::Reorder {
                instance_id,
                move_up,
            } => {
                let inst = self.plugin_instance(instance_id)?;
                ensure!(
                    inst.info.tab_id.is_none()
                        && !self.plugins.context_viewers.contains_key(&instance_id),
                    "仅主工具插件实例可调整执行顺序"
                );
                if let Some(pos) = self.plugins.order.iter().position(|id| *id == instance_id) {
                    let target = if move_up {
                        pos.saturating_sub(1)
                    } else {
                        (pos + 1).min(self.plugins.order.len().saturating_sub(1))
                    };
                    if pos != target {
                        self.plugins.order.swap(pos, target);
                        self.sync_proxy_plugin_hooks();
                    }
                }
                let order = self.plugins.order.clone();
                self.plugin_result(
                    project,
                    operation_id,
                    Ok(PluginOutcome::Reordered { order }),
                )
                .await;
            }
            PluginAction::OpenContextViewer {
                source_instance,
                context_id,
                revision,
                read_only,
                request,
            } => {
                self.start_open_context_viewer(
                    operation_id,
                    source_instance,
                    context_id,
                    revision,
                    read_only,
                    request,
                    cancel,
                )?;
            }
            PluginAction::CommitContextEditor {
                instance_id,
                context_id,
                revision,
                request,
                editor,
            } => {
                self.start_commit_context_editor(
                    operation_id,
                    instance_id,
                    context_id,
                    revision,
                    request,
                    editor,
                    cancel,
                )?;
            }
            PluginAction::GeneratePayloads {
                instance_id,
                tab_id,
                config,
                max_items,
            } => {
                self.start_generate_payloads(
                    operation_id,
                    instance_id,
                    tab_id,
                    config,
                    max_items,
                    cancel,
                )?;
            }
            PluginAction::ProcessPayloads {
                instance_id,
                tab_id,
                position,
                config,
                inputs,
            } => {
                self.start_process_payloads(
                    operation_id,
                    instance_id,
                    tab_id,
                    position,
                    config,
                    inputs,
                    cancel,
                )?;
            }
            PluginAction::OpenResponseViewer {
                source_instance,
                context_id,
                revision,
                read_only,
                response,
            } => {
                self.start_open_response_viewer(
                    operation_id,
                    source_instance,
                    context_id,
                    revision,
                    read_only,
                    response,
                    cancel,
                )?;
            }
            PluginAction::CommitResponseEditor {
                instance_id,
                context_id,
                revision,
                response,
                editor,
            } => {
                self.start_commit_response_editor(
                    operation_id,
                    instance_id,
                    context_id,
                    revision,
                    response,
                    editor,
                    cancel,
                )?;
            }
            PluginAction::InvokeContextMenu {
                instance_id,
                source,
                action_id,
                selection_start,
                selection_end,
                selected_bytes,
                request,
                response,
            } => {
                self.start_invoke_context_menu(
                    operation_id,
                    instance_id,
                    source,
                    action_id,
                    selection_start,
                    selection_end,
                    selected_bytes,
                    request,
                    response,
                    cancel,
                )?;
            }
            PluginAction::Reload { instance_id } => {
                self.start_reload_plugin(operation_id, instance_id, cancel)?;
            }
            PluginAction::SetAutoReload {
                instance_id,
                enabled,
            } => {
                let inst = self
                    .plugins
                    .instances
                    .get_mut(&instance_id)
                    .context("插件实例不存在")?;
                inst.auto_reload = enabled;
                self.plugin_result(
                    project,
                    operation_id,
                    Ok(PluginOutcome::AutoReloadUpdated {
                        instance_id,
                        enabled,
                    }),
                )
                .await;
            }
            PluginAction::Cancel { .. }
            | PluginAction::ApplyWorkspace { .. }
            | PluginAction::RejectWorkspace { .. } => unreachable!(),
        }
        Ok(())
    }
}
