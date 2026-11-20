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
            LaunchTarget::ContextViewer {
                source: source_instance,
                context_id,
                revision,
                read_only,
                request,
            },
            cancel,
        );
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
        cancel: CancellationToken,
    ) -> Result<()> {
        self.plugin_idle(instance_id)?;
        let instance = self.plugin_instance(instance_id)?;
        ensure!(
            self.plugins.context_viewers.get(&instance_id) == Some(&(context_id, revision, false)),
            "插件编辑上下文不匹配或为只读；未提交"
        );
        let handle = instance.worker.handle();
        let identity = instance.info.identity;
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let updated = handle
                .commit(
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
            LaunchTarget::ResponseViewer {
                source: source_instance,
                context_id,
                revision,
                read_only,
                response,
            },
            cancel,
        );
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
        cancel: CancellationToken,
    ) -> Result<()> {
        self.plugin_idle(instance_id)?;
        let instance = self.plugin_instance(instance_id)?;
        ensure!(
            self.plugins.context_viewers.get(&instance_id) == Some(&(context_id, revision, false)),
            "插件响应编辑上下文不匹配或为只读；未提交"
        );
        let handle = instance.worker.handle();
        let identity = instance.info.identity;
        let task_cancel = cancel.clone();
        self.plugin_job_start(operation_id, Some(instance_id), cancel, true, async move {
            let updated = handle
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
