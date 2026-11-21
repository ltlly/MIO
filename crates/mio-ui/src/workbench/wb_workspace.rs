use super::*;

impl Workbench {
    pub(in crate::workbench) fn apply_workspace_ack(
        &mut self,
        workspace: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_workspace = None;
        self.pending_proxy_interception = None;
        self.saving_project_snapshot = false;
        self.workspace.revision = workspace.revision;
        self.sync_websocket_repeater_editors(window, cx);
        self.tab_groups_workspace_acknowledged(workspace, cx);
        self.plugins_before_workspace_ack(workspace, window, cx);
        self.intruder_workspace_acknowledged(workspace, window, cx);
        self.workspace_acknowledged(workspace, window, cx);
        self.plugins_after_workspace_ack(workspace, window, cx);
        self.flush_ext_repeater(window, cx);
        self.flush_history_transfer(window, cx);
        self.resume_history_navigation(window, cx);
        self.refresh_repeater_history(window, cx);
    }

    pub(in crate::workbench) fn workspace_acknowledged(
        &mut self,
        saved: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ready: Vec<_> = self
            .pending_sends
            .iter()
            .filter_map(|(id, send)| {
                (send.acknowledged_by(*id, saved)
                    && self
                        .workspace
                        .tabs
                        .iter()
                        .any(|tab| tab.id == *id && tab.draft == send.draft))
                .then_some((*id, send.clone()))
            })
            .collect();
        let mut send_error = None;
        // A later CAS may coalesce settings; each request retains its activation snapshot.
        for (id, send) in ready {
            self.pending_sends.remove(&id);
            if !self.enqueue_send(id, send, cx) {
                send_error = self.error.clone();
            }
        }
        let obsolete: Vec<_> = self
            .pending_sends
            .iter()
            .filter_map(|(id, send)| {
                (!self
                    .workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.id == *id && tab.draft == send.draft))
                .then_some(*id)
            })
            .collect();
        if !obsolete.is_empty() {
            for id in obsolete {
                self.pending_sends.remove(&id);
            }
            send_error = Some("等待保存时草稿已变更，未发送旧稿；请再次点击发送。".into());
        }
        let saving_elsewhere = self.choosing_save_as
            || self
                .pending_project_save
                .as_ref()
                .is_some_and(Option::is_some);
        if self.workspace_dirty && !saving_elsewhere && !self.save_workspace(cx) {
            self.discard_pending_sends(cx);
            self.pending_project_save = None;
            self.cancel_transition(cx);
        }
        self.flush_project_save(window, cx);
        if let Some(error) = send_error {
            self.fail(error, cx);
        }
    }

    pub(in crate::workbench) fn flush_project_save(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expire_intruder_close_if_due(cx);
        if self.intruder.resource_pools.management.delete_pending()
            || self.websocket_notes_pending()
            || self.http_notes_pending()
            || self.http_highlights_pending()
            || self.project_saves_in_flight > 0
            || self.pending_workspace.is_some()
            || self.choosing_save_as
            || self
                .transition
                .as_ref()
                .is_some_and(|phase| *phase != TransitionPhase::Ready)
        {
            return;
        }
        let Some(project) = self.project else { return };
        let Some(path) = self.pending_project_save.clone() else {
            return;
        };
        if path.is_none() && self.workspace_dirty {
            return;
        }
        let websocket_notes = match self.snapshot_websocket_notes(window, cx) {
            Ok(notes) => notes,
            Err(error) => {
                self.pending_project_save = None;
                self.cancel_transition(cx);
                self.fail(error, cx);
                return;
            }
        };
        let ext_http_notes = match self.snapshot_http_notes(window, cx) {
            Ok(notes) => notes,
            Err(error) => {
                self.pending_project_save = None;
                self.cancel_transition(cx);
                self.fail(error, cx);
                return;
            }
        };
        let ext_http_highlights = match self.snapshot_http_highlights() {
            Ok(highlights) => highlights,
            Err(error) => {
                self.pending_project_save = None;
                self.cancel_transition(cx);
                self.fail(error, cx);
                return;
            }
        };
        let workspace = if path.is_some() && self.workspace_dirty {
            let revision = self.workspace.revision + 1;
            let mut snapshot = (*self.workspace).clone();
            snapshot.revision = revision;
            Some(snapshot)
        } else {
            None
        };
        let revision = workspace.as_ref().map(|workspace| workspace.revision);
        let transition_path = self
            .after_save
            .as_ref()
            .and_then(|_| path.as_ref().or(self.project_path.as_ref()).cloned());
        if let Some(path) = self.pending_project_save.take() {
            self.intruder_close_project_save_sent(path.as_ref());
            if self.enqueue(
                Command::SaveProject {
                    project,
                    path,
                    workspace,
                    websocket_notes: websocket_notes.clone(),
                    ext_http_notes: ext_http_notes.clone(),
                    ext_http_highlights: ext_http_highlights.clone(),
                },
                cx,
            ) {
                self.project_saves_in_flight += 1;
                self.project_notes_save = Some(websocket_notes);
                self.project_http_notes_save = Some(ext_http_notes);
                self.project_http_highlights_save = Some(ext_http_highlights);
                if let Some(path) = transition_path {
                    self.transition = Some(TransitionPhase::Saving(path));
                    self.status = "连接已停止，正在保存最后的响应与工作区…".into();
                }
                if let Some(revision) = revision {
                    self.pending_workspace = Some(revision);
                    self.saving_project_snapshot = true;
                    self.workspace_dirty = false;
                }
            } else {
                self.discard_pending_sends(cx);
                self.fail_intruder_result_close("项目保存命令未入队；结果和当前编辑仍保留。", cx);
                self.cancel_transition(cx);
            }
        }
    }
}
