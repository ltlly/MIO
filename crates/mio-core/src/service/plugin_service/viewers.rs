use super::*;

impl State {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_open_context_viewer(
        &mut self,
        operation_id: Uuid,
        source_instance: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Result<()> {
        ensure!(
            !context_id.is_nil() && revision > 0,
            "上下文资源 ID 或 revision 无效"
        );
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
        if !read_only {
            ensure!(
                source.info.grants.contains(&Permission::DraftWrite),
                "可写消息上下文需要授权 draft.write"
            );
        }
        let supports_multiplex = source.worker.supports_panel_multiplexing;
        let (package, trust, grants) = (
            source.package.clone(),
            source.trust.clone(),
            source.info.grants.clone(),
        );
        self.plugin_child_capacity()?;
        if supports_multiplex {
            let (virtual_id, panel_id, identity, handle, info) =
                self.allocate_multiplexed_child(source_instance, None)?;
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
                    primary_resource: context_id,
                    info,
                    panel: placeholder,
                    resources: HashMap::from([(context_id, revision)]),
                },
            );
            self.plugins
                .panel_routes
                .insert((source_instance, panel_id), virtual_id);
            self.plugins
                .context_viewers
                .insert(virtual_id, (context_id, revision, read_only));
            let pending_open = handle.begin_open(panel_id, context_id);
            let task_cancel = cancel.clone();
            self.plugin_job_start(operation_id, Some(virtual_id), cancel, true, async move {
                let opened = pending_open.open_request(
                    revision,
                    read_only,
                    request,
                );
                tokio::pin!(opened);
                let panel = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        let _ = handle.revoke_panel_resource(panel_id, context_id, true).await;
                        anyhow::bail!("打开插件上下文已取消");
                    }
                    result = &mut opened => result?,
                };
                Ok(Output::ContextOpened {
                    identity,
                    context_id,
                    revision,
                    panel,
                })
            });
            if let Some(op) = self.plugins.operations.get_mut(&operation_id) {
                op.multiplexed_cleanup_on_fail = Some(virtual_id);
            }
        } else {
            self.start_plugin(
                operation_id,
                package,
                trust,
                grants,
                LaunchTarget::ContextViewer {
                    source: source_instance,
                    context_id,
                    revision,
                    read_only,
                    request,
                },
                cancel,
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_commit_context_editor(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        context_id: Uuid,
        revision: u64,
        request: HttpRequest,
        editor: panel::EditorSnapshot,
        expected_panel: Option<Arc<panel::PanelSnapshot>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.plugin_idle(instance_id)?;
        let (handle, identity, panel_id, _, _, panel) = self.plugin_target(instance_id)?;
        ensure!(
            self.plugins.context_viewers.get(&instance_id) == Some(&(context_id, revision, false)),
            "插件编辑上下文不匹配或为只读；未提交"
        );
        let expected_snapshot = expected_panel.unwrap_or_else(|| Arc::new(panel.clone()));
        ensure!(
            panel.revision == expected_snapshot.revision
                || panel::commit_panel_semantically_matches(
                    Some(&panel),
                    &expected_snapshot,
                    &editor,
                ),
            "插件面板内容已更新；旧视图的编辑提交未执行，请在当前视图重试"
        );
        let pending_commit =
            handle.begin_commit_with_snapshot(panel_id, Some(context_id), Some(expected_snapshot));
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let updated = pending_commit
                .commit_request(
                    CommitEditor {
                        resource_id: context_id.as_bytes().to_vec(),
                        message_revision: revision,
                        request: Some(request.into()),
                        editor: Some(editor),
                    },
                    task_cancel,
                )
                .await?;
            Ok(Output::ContextCommitted {
                identity,
                context_id,
                revision,
                request: updated,
            })
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_open_response_viewer(
        &mut self,
        operation_id: Uuid,
        source_instance: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
        cancel: CancellationToken,
    ) -> Result<()> {
        ensure!(
            !context_id.is_nil() && revision > 0,
            "响应上下文 ID 或 revision 无效"
        );
        let source = self.plugin_instance(source_instance)?;
        ensure!(
            source.info.tab_id.is_none()
                && source
                    .info
                    .manifest
                    .contributions
                    .iter()
                    .any(|c| c == "message_response_editor"),
            "此插件未提供响应消息编辑器（message_response_editor）"
        );
        ensure!(
            source.info.grants.contains(&Permission::TrafficRead),
            "响应消息编辑器需要授权 traffic.read"
        );
        if !read_only {
            ensure!(
                source.info.grants.contains(&Permission::DraftWrite),
                "可写响应上下文需要授权 draft.write"
            );
        }
        let supports_multiplex = source.worker.supports_panel_multiplexing;
        let (package, trust, grants) = (
            source.package.clone(),
            source.trust.clone(),
            source.info.grants.clone(),
        );
        self.plugin_child_capacity()?;
        if supports_multiplex {
            let (virtual_id, panel_id, identity, handle, info) =
                self.allocate_multiplexed_child(source_instance, None)?;
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
                    primary_resource: context_id,
                    info,
                    panel: placeholder,
                    resources: HashMap::from([(context_id, revision)]),
                },
            );
            self.plugins
                .panel_routes
                .insert((source_instance, panel_id), virtual_id);
            self.plugins
                .context_viewers
                .insert(virtual_id, (context_id, revision, read_only));
            let pending_open = handle.begin_open(panel_id, context_id);
            let task_cancel = cancel.clone();
            self.plugin_job_start(operation_id, Some(virtual_id), cancel, true, async move {
                let opened = pending_open.open_response(
                    revision,
                    read_only,
                    response,
                );
                tokio::pin!(opened);
                let panel = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        let _ = handle.revoke_panel_resource(panel_id, context_id, true).await;
                        anyhow::bail!("打开插件响应上下文已取消");
                    }
                    result = &mut opened => result?,
                };
                Ok(Output::ResponseOpened {
                    identity,
                    context_id,
                    revision,
                    panel,
                })
            });
            if let Some(op) = self.plugins.operations.get_mut(&operation_id) {
                op.multiplexed_cleanup_on_fail = Some(virtual_id);
            }
        } else {
            self.start_plugin(
                operation_id,
                package,
                trust,
                grants,
                LaunchTarget::ResponseViewer {
                    source: source_instance,
                    context_id,
                    revision,
                    read_only,
                    response,
                },
                cancel,
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_commit_response_editor(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        context_id: Uuid,
        revision: u64,
        response: HttpResponse,
        editor: panel::EditorSnapshot,
        expected_panel: Option<Arc<panel::PanelSnapshot>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.plugin_idle(instance_id)?;
        let (handle, identity, panel_id, _, _, panel) = self.plugin_target(instance_id)?;
        ensure!(
            self.plugins.context_viewers.get(&instance_id) == Some(&(context_id, revision, false)),
            "插件响应编辑上下文不匹配或为只读；未提交"
        );
        let expected_snapshot = expected_panel.unwrap_or_else(|| Arc::new(panel.clone()));
        ensure!(
            panel.revision == expected_snapshot.revision
                || panel::commit_panel_semantically_matches(
                    Some(&panel),
                    &expected_snapshot,
                    &editor,
                ),
            "插件面板内容已更新；旧视图的响应编辑提交未执行，请在当前视图重试"
        );
        let pending_commit =
            handle.begin_commit_with_snapshot(panel_id, Some(context_id), Some(expected_snapshot));
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let updated = pending_commit
                .commit_response(
                    protocol::CommitResponseEditor {
                        resource_id: context_id.as_bytes().to_vec(),
                        message_revision: revision,
                        response: Some(response.into()),
                        editor: Some(editor),
                    },
                    task_cancel,
                )
                .await?;
            Ok(Output::ResponseCommitted {
                identity,
                context_id,
                revision,
                response: updated,
            })
        });
        Ok(())
    }
}
