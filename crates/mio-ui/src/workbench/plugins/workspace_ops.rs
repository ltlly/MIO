use std::time::Instant;

use mio_core::{
    IntruderSpec, IntruderTab, PluginAction, PluginWorkspaceEdit, PluginWorkspaceRequest, Uuid,
    intruder::AttackType,
};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn plugins_workspace_rejected(
        &mut self,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.extensions.operations.iter().find_map(|(id, operation)| matches!(operation, Operation::Workspace { revision: pending, .. } if *pending == revision).then_some(*id));
        if let Some(id) = id
            && let Some(Operation::Workspace { mutation, .. }) =
                self.extensions.operations.remove(&id)
        {
            self.rollback_plugin_mutation(*mutation, window, cx);
        }
    }

    pub(in crate::workbench) fn rollback_plugin_mutation(
        &mut self,
        mutation: WorkspaceMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Preserve typing that happened while storage validated the proposal. Only the exact
        // unedited plugin mutation is rolled back; no IME composition is force-committed.
        match self.capture_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(_) => return,
        }
        match mutation {
            WorkspaceMutation::Repeater {
                before,
                proposed,
                active_before,
            } => {
                let id = proposed.id;
                let Some(index) = self.workspace.tabs.iter().position(|tab| tab.id == id) else {
                    return;
                };
                if let Some(before) = before {
                    if self.workspace.tabs[index].draft != proposed.draft {
                        return;
                    }
                    self.workspace.tabs[index].draft = before.draft;
                    self.apply_draft_to_native(id, window, cx);
                } else {
                    if self.workspace.tabs[index] != proposed {
                        return;
                    }
                    self.workspace.tabs.remove(index);
                    self.tab_editors.remove(&id);
                    self.tab_editor_lru.retain(|&existing| existing != id);
                    if self.workspace.active == Some(id) {
                        self.workspace.active = active_before
                            .filter(|id| self.workspace.tabs.iter().any(|tab| tab.id == *id))
                            .or_else(|| self.workspace.tabs.first().map(|tab| tab.id));
                        if let Some(next_id) = self.workspace.active {
                            self.ensure_tab_editor(next_id, window, cx);
                        }
                    }
                }
                cx.notify();
            }
            WorkspaceMutation::Intruder {
                proposed,
                active_before,
                orientation_before,
            } => {
                let id = proposed.id;
                let Some(index) = self
                    .workspace
                    .intruder
                    .tabs
                    .iter()
                    .position(|tab| tab.id == id)
                else {
                    return;
                };
                if self.workspace.intruder.tabs[index] != proposed {
                    return;
                }
                self.workspace.intruder.tabs.remove(index);
                self.intruder.editors.remove(&id);
                self.workspace.intruder.orientation = orientation_before;
                if self.workspace.intruder.active == Some(id) {
                    self.workspace.intruder.active = active_before
                        .filter(|id| self.workspace.intruder.tabs.iter().any(|tab| tab.id == *id))
                        .or_else(|| self.workspace.intruder.tabs.first().map(|tab| tab.id));
                }
                self.sync_intruder_editors(window, cx);
                cx.notify();
            }
        }
    }

    pub(in crate::workbench) fn cancel_plugin_interactions(&mut self, cx: &mut Context<Self>) {
        for instance in &self.extensions.instances {
            instance
                .panel
                .update(cx, |panel, cx| panel.cancel_interaction(cx));
        }
    }

    pub(in crate::workbench) fn plugins_after_workspace_ack(
        &mut self,
        saved: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waiting = std::mem::take(&mut self.extensions.create_after_save);
        for pending in waiting {
            if saved
                .tabs
                .iter()
                .any(|tab| tab.id == pending.tab_id && tab.draft == pending.draft)
            {
                self.start_plugin_editor(pending, cx);
            } else if self
                .workspace
                .tabs
                .iter()
                .any(|tab| tab.id == pending.tab_id && tab.draft == pending.draft)
            {
                self.extensions.create_after_save.push(pending);
            } else {
                self.fail("等待保存期间草稿改变，插件编辑器未加载旧稿；请重试。", cx);
            }
        }
        self.apply_plugin_workspace(window, cx);
    }

    pub(in crate::workbench) fn plugin_workspace_request(
        &mut self,
        request: PluginWorkspaceRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.extensions.workspace_requests.len() >= 16 {
            self.reject_plugin_workspace(request, "原生工作区操作队列已满。", cx);
            return;
        }
        self.extensions.workspace_requests.push_back(request);
        self.apply_plugin_workspace(window, cx);
    }

    pub(in crate::workbench) fn reject_plugin_workspace(
        &mut self,
        request: PluginWorkspaceRequest,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        self.plugin_control(
            PluginAction::RejectWorkspace {
                instance_id: request.identity.instance_id,
                request_id: request.request_id,
                message: message.into(),
            },
            Operation::Reject,
            cx,
        );
    }

    pub(in crate::workbench) fn apply_plugin_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_workspace.is_some() || self.choosing_save_as || self.saving_project_snapshot
        {
            return;
        }
        while let Some(request) = self.extensions.workspace_requests.pop_front() {
            if self.project != Some(request.identity.project)
                || self.transition.is_some()
                || request.cancel.is_cancelled()
                || request.deadline <= Instant::now()
            {
                self.reject_plugin_workspace(
                    request,
                    "项目已切换、操作已取消或超过 deadline。",
                    cx,
                );
                continue;
            }
            if !self
                .extensions
                .instance(request.identity.instance_id)
                .is_some_and(|instance| instance.live && instance.info.identity == request.identity)
            {
                self.reject_plugin_workspace(request, "插件实例已停止或失效。", cx);
                continue;
            }
            match self.capture_drafts(window, cx) {
                Ok(changed) => self.workspace_dirty |= changed,
                Err(error) => {
                    self.reject_plugin_workspace(request, &error, cx);
                    continue;
                }
            }
            if self.workspace_dirty {
                self.extensions.workspace_requests.push_front(request);
                // Persist existing local edits first. ApplyWorkspace may change only its one
                // authorized draft or append one tab relative to the acknowledged workspace.
                if !self.save_workspace(cx) {
                    let request = self.extensions.workspace_requests.pop_front().unwrap();
                    self.reject_plugin_workspace(request, "原生编辑尚未保存，未应用插件修改。", cx);
                }
                return;
            }
            let mut proposal = self.workspace.clone();
            let Some(revision) = proposal.revision.checked_add(1) else {
                self.reject_plugin_workspace(request, "工作区 revision 已耗尽。", cx);
                continue;
            };
            proposal.revision = revision;
            let mutation =
                match &request.edit {
                    PluginWorkspaceEdit::UpdateDraft {
                        tab_id,
                        expected_revision,
                        request: message,
                    } => {
                        let Some(tab) = proposal.tabs.iter_mut().find(|tab| {
                            tab.id == *tab_id && tab.draft.revision == *expected_revision
                        }) else {
                            self.reject_plugin_workspace(
                                request,
                                "草稿已改变或标签已关闭；没有覆盖原生编辑。",
                                cx,
                            );
                            continue;
                        };
                        let Some(revision) = expected_revision.checked_add(1) else {
                            self.reject_plugin_workspace(request, "草稿 revision 已耗尽。", cx);
                            continue;
                        };
                        tab.draft = Draft {
                            revision,
                            target: message.url.clone(),
                            raw: request_to_raw(message),
                            version: message.version,
                        };
                        let tab_id = *tab_id;
                        let proposed = proposal.tabs.iter().find(|tab| tab.id == tab_id).cloned();
                        (
                            tab_id,
                            WorkspaceMutation::Repeater {
                                before: self
                                    .workspace
                                    .tabs
                                    .iter()
                                    .find(|tab| tab.id == tab_id)
                                    .cloned(),
                                proposed: proposed.expect("proposal tab was just updated"),
                                active_before: self.workspace.active,
                            },
                        )
                    }
                    PluginWorkspaceEdit::OpenRepeater { request: message } => {
                        let tab_id = Uuid::new_v4();
                        let mut number = proposal.tabs.len() + proposal.closed.len() + 1;
                        while proposal
                            .tabs
                            .iter()
                            .chain(&proposal.closed)
                            .any(|tab| tab.title == number.to_string())
                        {
                            number += 1;
                        }
                        proposal.tabs.push(WorkTab {
                            id: tab_id,
                            title: number.to_string(),
                            group: None,
                            last_flow: None,
                            archive_response: None,
                            history: None,
                            draft: Draft {
                                revision: 1,
                                target: message.url.clone(),
                                raw: request_to_raw(message),
                                version: message.version,
                            },
                        });
                        proposal.active = Some(tab_id);
                        proposal.active_websocket_tab = None;
                        (
                            tab_id,
                            WorkspaceMutation::Repeater {
                                before: None,
                                proposed: proposal
                                    .tabs
                                    .last()
                                    .cloned()
                                    .expect("proposal tab was just pushed"),
                                active_before: self.workspace.active,
                            },
                        )
                    }
                    PluginWorkspaceEdit::OpenIntruder { request: message } => {
                        let tab_id = Uuid::new_v4();
                        let number = proposal
                            .intruder
                            .tabs
                            .iter()
                            .chain(&proposal.intruder.closed)
                            .filter_map(|tab| tab.title.parse::<u32>().ok())
                            .max()
                            .unwrap_or(0)
                            .saturating_add(1);
                        proposal.intruder.tabs.push(IntruderTab {
                            id: tab_id,
                            title: number.to_string(),
                            group: None,
                            spec: IntruderSpec {
                                revision: 1,
                                draft: Draft {
                                    revision: 1,
                                    target: message.url.clone(),
                                    raw: request_to_raw(message),
                                    version: message.version,
                                },
                                attack_type: AttackType::Sniper,
                                positions: Vec::new(),
                                payload_sets: vec![Vec::new()],
                                max_concurrent_requests: 1,
                                request_timeout_ms: 30_000,
                                update_host_header: true,
                                save_to_project: false,
                                baseline_request: false,
                                resource_pool: None,
                            },
                            last_run: None,
                        });
                        proposal.intruder.active = Some(tab_id);
                        (
                            tab_id,
                            WorkspaceMutation::Intruder {
                                proposed: proposal
                                    .intruder
                                    .tabs
                                    .last()
                                    .cloned()
                                    .expect("proposal tab was just pushed"),
                                active_before: self.workspace.intruder.active,
                                orientation_before: self.workspace.intruder.orientation,
                            },
                        )
                    }
                };
            let (tab_id, mutation) = mutation;
            if let Err(error) = proposal.validate_structure() {
                self.reject_plugin_workspace(request, &error.to_string(), cx);
                continue;
            }
            let instance = request.identity.instance_id;
            self.invalidate_plugin_commits(None, Some(tab_id), cx);
            let action = PluginAction::ApplyWorkspace {
                instance_id: instance,
                request_id: request.request_id,
                workspace: proposal.clone(),
                tab_id,
            };
            if self
                .plugin_action(
                    action,
                    Operation::Workspace {
                        instance,
                        tab_id,
                        revision,
                        mutation: Box::new(mutation),
                    },
                    cx,
                )
                .is_some()
            {
                proposal.revision = self.workspace.revision;
                self.workspace = proposal;
                self.pending_workspace = Some(revision);
                self.workspace_dirty = false;
                self.sync_tab_editors(window, cx);
                self.sync_intruder_editors(window, cx);
                self.apply_draft_to_native(tab_id, window, cx);
                if matches!(request.edit, PluginWorkspaceEdit::OpenRepeater { .. }) {
                    self.tool = Tool::Repeater;
                }
                if matches!(request.edit, PluginWorkspaceEdit::OpenIntruder { .. }) {
                    self.tool = Tool::Intruder;
                }
                return;
            }
            self.reject_plugin_workspace(request, "后台队列已满，未修改项目。", cx);
        }
    }
}
