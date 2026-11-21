use super::*;

impl State {
    pub(super) async fn plugin_queue_workspace(
        &mut self,
        call: ServiceRequest,
        edit: PluginWorkspaceEdit,
    ) {
        let key = (call.identity.instance_id, call.request_id);
        if self.plugins.workspace.len() >= MAX_WORKSPACE_REQUESTS
            || self.plugins.workspace.contains_key(&key)
        {
            let _ = call.reply.send(Err(Failure::new(
                ErrorCode::Denied,
                "插件工作区等待队列已满或请求重复",
            )));
            return;
        }
        let event = PluginWorkspaceRequest {
            identity: call.identity,
            request_id: call.request_id,
            deadline: call.deadline.into_std(),
            cancel: call.cancel.clone(),
            edit,
        };
        let cancel = call.cancel.clone();
        let deadline = call.deadline;
        self.plugins.workspace.insert(
            key,
            PendingWorkspace {
                call,
                expired: Box::pin(async move {
                    tokio::select! {
                        _ = cancel.cancelled() => {},
                        _ = tokio::time::sleep_until(deadline) => {},
                    }
                }),
            },
        );
        self.emit(Event::PluginWorkspaceRequest(event)).await;
    }

    pub(super) async fn plugin_apply_workspace(
        &mut self,
        operation_id: Uuid,
        key: CallKey,
        workspace: Workspace,
        tab_id: TabId,
    ) -> Result<()> {
        let pending = self
            .plugins
            .workspace
            .remove(&key)
            .context("插件工作区请求已取消、超时或完成")?;
        let is_intruder = matches!(
            pending.call.operation,
            ServiceOperation::OpenIntruder { .. }
        );
        let result = (|| {
            self.plugin_check_call(&pending.call)?;
            self.require_saved()?;
            let stored = self.store.metadata().workspace()?;
            validate_workspace_edit(&stored, &workspace, tab_id, &pending.call.operation)?;
            // Recheck cancellation immediately before the atomic, single-writer commit.
            self.plugin_check_call(&pending.call)?;
            self.store.metadata_mut().save_workspace(&workspace)?;
            if is_intruder {
                Ok::<_, anyhow::Error>(None)
            } else {
                Ok(Some(
                    workspace
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .unwrap()
                        .draft
                        .revision,
                ))
            }
        })();
        match result {
            Ok(revision_opt) => {
                if let Some(revision) = revision_opt {
                    if pending.call.panel_id == protocol::PANEL_ID {
                        if let Some(instance) = self.plugins.instances.get_mut(&key.0) {
                            instance.resources.insert(tab_id, revision);
                        }
                    } else if let Some(virtual_id) = self
                        .plugins
                        .panel_routes
                        .get(&(key.0, pending.call.panel_id))
                        .copied()
                        && let Some(child) = self.plugins.multiplexed.get_mut(&virtual_id)
                    {
                        child.resources.insert(tab_id, revision);
                    }
                }
                self.emit(Event::WorkspaceChanged {
                    project: self.project,
                    workspace,
                })
                .await;
                let reply = match revision_opt {
                    Some(revision) => ServiceResult::Resource {
                        id: tab_id,
                        revision,
                    },
                    None => ServiceResult::Done,
                };
                let _ = pending.call.reply.send(Ok(reply));
                self.plugin_result(
                    self.project,
                    operation_id,
                    Ok(PluginOutcome::WorkspaceApplied),
                )
                .await;
            }
            Err(error) => {
                let message = error.to_string();
                let _ = pending
                    .call
                    .reply
                    .send(Err(Failure::new(ErrorCode::Stale, &message)));
                self.emit(Event::WorkspaceRejected {
                    project: self.project,
                    revision: workspace.revision,
                    stored: self.store.metadata().workspace()?,
                    message: message.clone(),
                })
                .await;
                self.plugin_result(self.project, operation_id, Err(message))
                    .await;
            }
        }
        Ok(())
    }
}

pub(super) fn validate_workspace_edit(
    stored: &Workspace,
    proposal: &Workspace,
    tab_id: TabId,
    operation: &ServiceOperation,
) -> Result<()> {
    proposal.validate()?;
    ensure!(
        stored.revision.checked_add(1) == Some(proposal.revision),
        "工作区已变更；插件提案未覆盖新内容"
    );
    let mut expected = stored.clone();
    expected.revision = proposal.revision;
    match operation {
        ServiceOperation::UpdateDraft {
            tab_id: resource,
            expected_revision,
            request,
        } => {
            ensure!(*resource == tab_id, "插件回写提案的资源 ID 不匹配");
            let tab = expected
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .context("工作标签已关闭")?;
            ensure!(
                tab.draft.revision == *expected_revision,
                "插件草稿基准 revision 已过期"
            );
            tab.draft = Draft {
                revision: expected_revision + 1,
                target: request.url.clone(),
                raw: request_to_raw(request),
                version: request.version,
            };
            // Focusing the edited tab is UI-owned; other tabs and workspace settings must not change.
            expected.active = proposal.active;
        }
        ServiceOperation::OpenRepeater { request } => {
            ensure!(
                !tab_id.is_nil()
                    && !stored
                        .tabs
                        .iter()
                        .chain(&stored.closed)
                        .any(|tab| tab.id == tab_id),
                "新标签不能复用已有资源 ID"
            );
            let tab = proposal
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .context("插件提案缺少新标签")?;
            ensure!(
                tab.draft.revision == 1
                    && tab.draft.target == request.url
                    && tab.draft.version == request.version
                    && tab.draft.raw == request_to_raw(request),
                "新标签内容与插件请求不一致"
            );
            ensure!(
                tab.last_flow.is_none()
                    && tab.archive_response.is_none()
                    && tab.history.is_none()
                    && tab.group.is_none(),
                "插件新标签不能伪造已发送结果、归档响应或工作组"
            );
            expected.tabs.push(tab.clone());
            expected.active = Some(tab_id);
        }
        ServiceOperation::OpenIntruder { request } => {
            ensure!(
                !tab_id.is_nil()
                    && !stored
                        .intruder
                        .tabs
                        .iter()
                        .chain(&stored.intruder.closed)
                        .any(|tab| tab.id == tab_id)
                    && !stored
                        .tabs
                        .iter()
                        .chain(&stored.closed)
                        .any(|tab| tab.id == tab_id),
                "新 Intruder 标签不能复用已有资源 ID"
            );
            let tab = proposal
                .intruder
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .context("插件提案缺少新 Intruder 标签")?;
            ensure!(
                tab.spec.draft.revision == 1
                    && tab.spec.draft.target == request.url
                    && tab.spec.draft.version == request.version
                    && tab.spec.draft.raw == request_to_raw(request),
                "新 Intruder 标签内容与插件请求不一致"
            );
            ensure!(
                tab.last_run.is_none() && tab.group.is_none(),
                "插件新 Intruder 标签不能伪造结果或工作组"
            );
            expected.intruder.tabs.push(tab.clone());
            expected.intruder.active = Some(tab_id);
        }
        _ => anyhow::bail!("此插件调用不允许修改工作区"),
    }
    ensure!(expected == *proposal, "插件提案包含未经请求的工作区修改");
    Ok(())
}
