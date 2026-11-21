use super::*;

enum ViewerPayload {
    Request(HttpRequest),
    Response(HttpResponse),
}

impl ViewerPayload {
    fn is_response(&self) -> bool {
        matches!(self, Self::Response(_))
    }

    fn contribution(&self) -> &'static str {
        if self.is_response() {
            "message_response_editor"
        } else {
            "message_editor"
        }
    }
}

impl State {
    #[allow(clippy::too_many_arguments)]
    fn start_open_viewer(
        &mut self,
        operation_id: Uuid,
        source_instance: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        payload: ViewerPayload,
        cancel: CancellationToken,
    ) -> Result<()> {
        let is_response = payload.is_response();
        ensure!(
            !context_id.is_nil() && revision > 0,
            if is_response {
                "响应上下文 ID 或 revision 无效"
            } else {
                "上下文资源 ID 或 revision 无效"
            }
        );
        let source = self.plugin_instance(source_instance)?;
        let contribution = payload.contribution();
        ensure!(
            source.info.tab_id.is_none()
                && source
                    .info
                    .manifest
                    .contributions
                    .iter()
                    .any(|c| c == contribution),
            if is_response {
                "此插件未提供响应消息编辑器（message_response_editor）"
            } else {
                "此插件未提供独立消息编辑器"
            }
        );
        ensure!(
            source.info.grants.contains(&Permission::TrafficRead),
            if is_response {
                "响应消息编辑器需要授权 traffic.read"
            } else {
                "自定义编辑器需要单独授权 traffic.read"
            }
        );
        if !read_only {
            ensure!(
                source.info.grants.contains(&Permission::DraftWrite),
                if is_response {
                    "可写响应上下文需要授权 draft.write"
                } else {
                    "可写消息上下文需要授权 draft.write"
                }
            );
        }
        self.plugin_child_capacity()?;
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
            let cancel_msg = if is_response {
                "打开插件响应上下文已取消"
            } else {
                "打开插件上下文已取消"
            };
            let opened = async {
                match payload {
                    ViewerPayload::Request(request) => {
                        let panel = pending_open
                            .open_request(revision, read_only, request)
                            .await?;
                        Ok(Output::ContextOpened {
                            identity,
                            context_id,
                            revision,
                            panel,
                        })
                    }
                    ViewerPayload::Response(response) => {
                        let panel = pending_open
                            .open_response(revision, read_only, response)
                            .await?;
                        Ok(Output::ResponseOpened {
                            identity,
                            context_id,
                            revision,
                            panel,
                        })
                    }
                }
            };
            tokio::pin!(opened);
            tokio::select! {
                biased;
                _ = task_cancel.cancelled() => {
                    let _ = handle.revoke_panel_resource(panel_id, context_id, true).await;
                    anyhow::bail!(cancel_msg);
                }
                result = &mut opened => result,
            }
        });
        if let Some(op) = self.plugins.operations.get_mut(&operation_id) {
            op.multiplexed_cleanup_on_fail = Some(virtual_id);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start_commit_viewer(
        &mut self,
        operation_id: Uuid,
        instance_id: Uuid,
        context_id: Uuid,
        revision: u64,
        payload: ViewerPayload,
        editor: panel::EditorSnapshot,
        expected_panel: Option<Arc<panel::PanelSnapshot>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let is_response = payload.is_response();
        self.plugin_idle(instance_id)?;
        let (handle, identity, panel_id, _, _, panel) = self.plugin_target(instance_id)?;
        ensure!(
            self.plugins.context_viewers.get(&instance_id) == Some(&(context_id, revision, false)),
            if is_response {
                "插件响应编辑上下文不匹配或为只读；未提交"
            } else {
                "插件编辑上下文不匹配或为只读；未提交"
            }
        );
        let expected_snapshot = expected_panel.unwrap_or_else(|| Arc::new(panel.clone()));
        ensure!(
            panel.revision == expected_snapshot.revision
                || panel::commit_panel_semantically_matches(
                    Some(&panel),
                    &expected_snapshot,
                    &editor,
                ),
            if is_response {
                "插件面板内容已更新；旧视图的响应编辑提交未执行，请在当前视图重试"
            } else {
                "插件面板内容已更新；旧视图的编辑提交未执行，请在当前视图重试"
            }
        );
        let pending_commit =
            handle.begin_commit_with_snapshot(panel_id, Some(context_id), Some(expected_snapshot));
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            match payload {
                ViewerPayload::Request(request) => {
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
                }
                ViewerPayload::Response(response) => {
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
                }
            }
        });
        Ok(())
    }

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
        self.start_open_viewer(
            operation_id,
            source_instance,
            context_id,
            revision,
            read_only,
            ViewerPayload::Request(request),
            cancel,
        )
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
        self.start_commit_viewer(
            operation_id,
            instance_id,
            context_id,
            revision,
            ViewerPayload::Request(request),
            editor,
            expected_panel,
            cancel,
        )
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
        self.start_open_viewer(
            operation_id,
            source_instance,
            context_id,
            revision,
            read_only,
            ViewerPayload::Response(response),
            cancel,
        )
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
        self.start_commit_viewer(
            operation_id,
            instance_id,
            context_id,
            revision,
            ViewerPayload::Response(response),
            editor,
            expected_panel,
            cancel,
        )
    }
}
