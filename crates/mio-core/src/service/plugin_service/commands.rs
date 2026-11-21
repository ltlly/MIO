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
        if self.extensions_disabled
            && !matches!(
                action,
                PluginAction::Disable { .. } | PluginAction::Uninstall { .. }
            )
        {
            anyhow::bail!("当前项目已禁用扩展（Disable extensions）；插件未检查、加载或执行");
        }
        ensure!(
            self.plugins.operations.len() < MAX_OPERATIONS,
            "插件后台操作达到 64 个限额"
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
                let effective_replace = if let Some(id) = replace_instance {
                    let old = self.plugin_instance(id)?;
                    ensure!(
                        old.info.tab_id.is_none() && old.info.manifest.id == package.manifest.id,
                        "重载目标不是同一 Suite 插件"
                    );
                    self.plugin_idle(id)?;
                    Some(id)
                } else if self.plugins.registry.is_persistent() {
                    let existing_id = self
                        .plugins
                        .instances
                        .iter()
                        .find(|(id, inst)| {
                            inst.info.tab_id.is_none()
                                && !self.plugins.context_viewers.contains_key(id)
                                && inst.info.manifest.id == package.manifest.id
                        })
                        .map(|(id, _)| *id);
                    if let Some(id) = existing_id {
                        self.plugin_idle(id)?;
                    }
                    existing_id
                } else {
                    None
                };
                self.plugin_capacity_for(effective_replace.is_some())?;
                self.plugins.inspections.remove(&inspection_id);
                self.start_plugin(
                    operation_id,
                    package,
                    trust,
                    grants,
                    effective_replace,
                    false,
                    cancel,
                );
            }
            PluginAction::CreateEditor {
                source_instance,
                tab_id,
                draft_revision,
            } => {
                let tab = self.plugin_tab(tab_id, draft_revision)?;
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
                self.plugin_child_capacity()?;
                let request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version)?;
                let (virtual_id, panel_id, identity, handle, info) =
                    self.allocate_multiplexed_child(source_instance, Some(tab_id))?;
                let placeholder = std::sync::Arc::new(PanelSnapshot {
                    revision: 0,
                    title: info.manifest.id.clone(),
                    root: None,
                });
                self.plugins.multiplexed.insert(
                    virtual_id,
                    MultiplexedChild {
                        source_instance,
                        panel_id,
                        primary_resource: tab_id,
                        info,
                        panel: placeholder,
                        resources: HashMap::from([(tab_id, draft_revision)]),
                    },
                );
                self.plugins
                    .panel_routes
                    .insert((source_instance, panel_id), virtual_id);
                let pending_open = handle.begin_open(panel_id, tab_id);
                let task_cancel = cancel.clone();
                self.plugin_job_start(operation_id, Some(virtual_id), cancel, true, async move {
                    let opened = pending_open.open_request(draft_revision, false, request);
                    tokio::pin!(opened);
                    let panel = tokio::select! {
                        biased;
                        _ = task_cancel.cancelled() => {
                            let _ = handle.revoke_panel_resource(panel_id, tab_id, true).await;
                            anyhow::bail!("打开插件多路复用编辑器已取消");
                        }
                        result = &mut opened => result?,
                    };
                    Ok(Output::Opened {
                        identity,
                        tab_id,
                        revision: draft_revision,
                        loading: true,
                        panel,
                    })
                });
                if let Some(op) = self.plugins.operations.get_mut(&operation_id) {
                    op.multiplexed_cleanup_on_fail = Some(virtual_id);
                    op.binding = Some((tab_id, draft_revision, None));
                }
            }
            PluginAction::Disable { instance_id } => {
                if self.plugins.multiplexed.contains_key(&instance_id) {
                    self.check_plugin_known(instance_id)?;
                    let Some((child, parent_handle)) =
                        self.take_multiplexed_child(instance_id, "插件已禁用")
                    else {
                        anyhow::bail!("插件实例不存在");
                    };
                    let identity = child.info.identity;
                    let source_instance = child.source_instance;
                    let panel_id = child.panel_id;
                    let primary_resource = child.primary_resource;
                    let track_closing = parent_handle.is_some();
                    let pending_revoke = parent_handle
                        .map(|handle| handle.begin_revoke_panel_resource(panel_id, primary_resource, true));
                    if track_closing {
                        self.plugins.closing_panels.insert((source_instance, panel_id));
                    }
                    self.plugin_job_start(
                        operation_id,
                        Some(instance_id),
                        cancel,
                        true,
                        async move {
                            let close_result = if let Some(revoke) = pending_revoke {
                                revoke.wait().await.map_err(|e| e.to_string())
                            } else {
                                Ok(())
                            };
                            Ok(Output::Retired {
                                retired: vec![Retired {
                                    identity,
                                    result: close_result,
                                    network: Vec::new(),
                                }],
                                retired_multiplexed: Vec::new(),
                                candidate: None,
                                disabled: Some(identity),
                                uninstalled_plugin_id: None,
                                reason: "插件已禁用".into(),
                            })
                        },
                    );
                    if track_closing
                        && let Some(op) = self.plugins.operations.get_mut(&operation_id)
                    {
                        op.closing_panel = Some((source_instance, panel_id));
                    }
                } else {
                    let inst = self.plugin_instance(instance_id)?;
                    let identity = inst.info.identity;
                    let suite_manifest_id = (inst.info.tab_id.is_none()
                        && !self.plugins.context_viewers.contains_key(&instance_id))
                    .then(|| inst.info.manifest.id.clone());
                    let (retired, retired_multiplexed) =
                        self.take_plugin_family(instance_id, "插件已禁用");
                    if let Some(plugin_id) = suite_manifest_id
                        && let Err(err) = self.plugins.registry.set_enabled(&plugin_id, false)
                    {
                        self.error("plugin_registry", err).await;
                    }
                    self.plugin_job_start(
                        operation_id,
                        Some(instance_id),
                        cancel,
                        true,
                        retire(
                            retired,
                            retired_multiplexed,
                            None,
                            Some(identity),
                            "插件已禁用".into(),
                        ),
                    );
                }
            }
            PluginAction::Ui {
                instance_id,
                panel_revision,
                event_seq,
                event,
            } => {
                let (handle, _, panel_id, _, _, panel) = self.plugin_target(instance_id)?;
                ensure!(
                    panel_revision == panel.revision,
                    "插件面板已更新；操作未提交，请在当前视图重试"
                );
                handle.try_event_on_panel_with_snapshot_seq(
                    panel_id,
                    Arc::new(panel.clone()),
                    event_seq,
                    event,
                )?;
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
                expected_panel,
            } => {
                self.plugin_idle(instance_id)?;
                let tab = self.plugin_tab(tab_id, draft_revision)?;
                let (handle, identity, panel_id, bound_tab, resources, panel) =
                    self.plugin_target(instance_id)?;
                ensure!(
                    bound_tab.is_none_or(|id| id == tab_id),
                    "消息编辑器属于其它标签"
                );
                ensure!(
                    resources.get(&tab_id) == Some(&draft_revision),
                    "插件请求上下文已改变；未提交旧稿"
                );
                let expected_snapshot = expected_panel.unwrap_or_else(|| Arc::new(panel.clone()));
                ensure!(
                    panel.revision == expected_snapshot.revision
                        || panel::commit_panel_semantically_matches(
                            Some(panel),
                            &expected_snapshot,
                            &editor,
                        ),
                    "插件面板内容已更新；旧视图的编辑提交未执行，请在当前视图重试"
                );
                let request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version)?;
                let pending_commit = handle.begin_commit_with_snapshot(
                    panel_id,
                    Some(tab_id),
                    Some(expected_snapshot),
                );
                let task_cancel = cancel.clone();
                self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
                    let request = pending_commit
                        .commit_request(
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
                        self.sync_registry_order()?;
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
                expected_panel,
            } => {
                self.start_commit_context_editor(
                    operation_id,
                    instance_id,
                    context_id,
                    revision,
                    request,
                    editor,
                    expected_panel,
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
                expected_panel,
            } => {
                self.start_commit_response_editor(
                    operation_id,
                    instance_id,
                    context_id,
                    revision,
                    response,
                    editor,
                    expected_panel,
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
                let is_suite = self
                    .plugins
                    .instances
                    .get(&instance_id)
                    .is_some_and(|inst| {
                        inst.info.tab_id.is_none()
                            && !self.plugins.context_viewers.contains_key(&instance_id)
                    });
                let inst = self
                    .plugins
                    .instances
                    .get_mut(&instance_id)
                    .context("插件实例不存在")?;
                inst.auto_reload = enabled;
                if is_suite {
                    let plugin_id = inst.info.manifest.id.clone();
                    if let Err(err) = self.plugins.registry.set_auto_reload(&plugin_id, enabled) {
                        self.error("plugin_registry", err).await;
                    }
                }
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
            PluginAction::EnableInstalled { plugin_id } => {
                ensure!(
                    !self.quiesced && !self.plugins.stopping,
                    "项目正在关闭；扩展未启用"
                );
                let already_live = self.plugins.instances.iter().any(|(id, inst)| {
                    inst.info.tab_id.is_none()
                        && !self.plugins.context_viewers.contains_key(id)
                        && inst.info.manifest.id == plugin_id
                });
                ensure!(!already_live, "此扩展已在运行中");
                let entry = self
                    .plugins
                    .registry
                    .get(&plugin_id)?
                    .context("持久化扩展记录不存在")?;
                let stored_dir = self.plugins.registry.package_path(&entry)?;
                self.plugin_capacity()?;
                self.start_installed_plugin(operation_id, entry, stored_dir, false, cancel);
            }
            PluginAction::Uninstall {
                plugin_id,
                instance_id,
            } => {
                ensure!(!plugin_id.trim().is_empty(), "插件 ID 不能为空");
                self.plugins.registry.remove(&plugin_id)?;
                let target_id = instance_id
                    .filter(|id| {
                        self.plugins.instances.get(id).is_some_and(|inst| {
                            inst.info.tab_id.is_none()
                                && !self.plugins.context_viewers.contains_key(id)
                                && inst.info.manifest.id == plugin_id
                        })
                    })
                    .or_else(|| {
                        self.plugins
                            .instances
                            .iter()
                            .find(|(id, inst)| {
                                inst.info.tab_id.is_none()
                                    && !self.plugins.context_viewers.contains_key(id)
                                    && inst.info.manifest.id == plugin_id
                            })
                            .map(|(id, _)| *id)
                    });
                if let Some(id) = target_id {
                    let identity = self.plugin_instance(id)?.info.identity;
                    let (retired, retired_multiplexed) = self.take_plugin_family(id, "插件已卸载");
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        cancel,
                        true,
                        retire_with_uninstall(
                            retired,
                            retired_multiplexed,
                            None,
                            Some(identity),
                            Some(plugin_id),
                            "插件已卸载".into(),
                        ),
                    );
                } else {
                    self.plugin_result(
                        project,
                        operation_id,
                        Ok(PluginOutcome::Uninstalled {
                            plugin_id,
                            identity: None,
                        }),
                    )
                    .await;
                }
            }
            PluginAction::Cancel { .. }
            | PluginAction::ApplyWorkspace { .. }
            | PluginAction::RejectWorkspace { .. } => unreachable!(),
        }
        Ok(())
    }
}
