use super::*;

mod naming_and_duplicate;
mod protocol_and_rename;
pub(in crate::workbench) use naming_and_duplicate::extract_repeater_urlpath;
pub(super) use protocol_and_rename::sync_captured_draft_version;

#[cfg(test)]
thread_local! {
    static FORCE_CLOSE_UNSAVED_PROMPT_IN_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(in crate::workbench) fn set_force_close_unsaved_prompt_in_test(enabled: bool) {
    FORCE_CLOSE_UNSAVED_PROMPT_IN_TEST.with(|cell| cell.set(enabled));
}

impl Workbench {
    pub(in crate::workbench) fn record_closed_repeater_tab(&mut self, id: TabId, is_ws: bool) {
        let project = self.project;
        self.closed_repeater_order
            .retain(|(proj, existing, _)| !(*proj == project && *existing == id));
        self.closed_repeater_order.push((project, id, is_ws));
        if self.closed_repeater_order.len() > 512 {
            let excess = self.closed_repeater_order.len() - 512;
            self.closed_repeater_order.drain(..excess);
        }
    }
    pub(in crate::workbench) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.tool, Tool::Repeater | Tool::Intruder) {
            return;
        }
        if self.tool == Tool::Intruder {
            self.new_intruder_tab(window, cx);
            return;
        }
        if self.transition.is_some() {
            return;
        }
        let Some(project) = self.project else { return };
        if self.flush_workspace(window, cx) && self.enqueue(Command::NewRepeater { project }, cx) {
            self.tool = Tool::Repeater;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn new_websocket_repeater_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.workspace.websocket_tabs.len() >= mio_core::MAX_WEBSOCKET_REPEATER_TABS {
            self.fail("WebSocket Repeater 标签已达上限；无法新建。", cx);
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let mut next_num = 1usize;
        let title = loop {
            let candidate = format!("WS {next_num}");
            if !self
                .workspace
                .websocket_tabs
                .iter()
                .any(|tab| tab.title == candidate || tab.title == next_num.to_string())
            {
                break candidate;
            }
            next_num = next_num.saturating_add(1);
        };
        if self.enqueue(
            Command::WebSocketRepeater(mio_core::WebSocketRepeaterCommand::Open {
                project,
                operation_id: Uuid::new_v4(),
                title: Some(title),
                url: "wss://localhost:8443/".into(),
                handshake_headers: Vec::new(),
                source_flow_id: None,
                source_message_id: None,
                source_direction: mio_core::WebSocketDirection::ClientToServer,
                source_kind: mio_core::WebSocketKind::Text,
                source_payload: Vec::new(),
            }),
            cx,
        ) {
            self.tool = Tool::Repeater;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backend_available() && !self.flush_workspace(window, cx) {
            return;
        }
        if self.workspace.tabs.iter().any(|tab| tab.id == id) {
            self.cancel_plugin_interactions(cx);
            let tab = self.workspace.tabs.iter().find(|tab| tab.id == id).unwrap();
            if let Some(group_id) = tab.group
                && let Some(group) = self
                    .workspace
                    .groups
                    .iter_mut()
                    .find(|group| group.id == group_id)
            {
                group.collapsed = false;
            }
            self.workspace.active = Some(id);
            self.workspace.active_websocket_tab = None;
            self.ensure_tab_editor(id, window, cx);
            self.refresh_archive_response(window, cx);
            self.refresh_repeater_history(window, cx);
            if self.backend_available() {
                self.save_workspace(cx);
            }
            if let Some(editors) = self.tab_editors.get(&id) {
                editors
                    .request
                    .state()
                    .update(cx, |state, cx| state.focus(window, cx));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn close_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.workspace.intruder.tabs.iter().any(|tab| tab.id == id) {
            self.close_intruder_tab(id, window, cx);
            return;
        }
        if self
            .workspace
            .websocket_tabs
            .iter()
            .any(|tab| !tab.closed && tab.id == id)
        {
            self.close_websocket_repeater_tab(id, window, cx);
            return;
        }
        if let Some(send) = self.sending.get(&id) {
            let project = self.project;
            let run_id = send.run_id;
            let answer = window.prompt(
                PromptLevel::Warning,
                "此标签仍在发送请求",
                Some("取消会收回在途连接；结果未知的非幂等请求不会自动重发。"),
                &["取消发送并关闭", "保留标签"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(0) = answer.await {
                    let _ = this.update_in(cx, |view, window, cx| {
                        if view.project != project
                            || view.transition.is_some()
                            || !view.backend_available()
                        {
                            return;
                        }
                        let Some(send) = view.sending.get(&id) else {
                            view.close_tab(id, window, cx);
                            return;
                        };
                        if send.run_id == run_id && view.cancel_send(id, cx) {
                            view.close_after_send.insert(id);
                        }
                    });
                }
            })
            .detach();
            return;
        }
        let had_live_unsaved_edit = match (
            self.tab_editors.get(&id),
            self.workspace.tabs.iter().find(|tab| tab.id == id),
        ) {
            (Some(editors), Some(tab)) => {
                let live_changed = editors.dirty
                    || editors
                        .request
                        .bytes(window, cx)
                        .is_ok_and(|raw| raw != tab.draft.raw)
                    || editing::input_snapshot(&editors.target, window, cx)
                        .is_ok_and(|target| target.as_ref() != tab.draft.target)
                    || self.repeater_target_dialog_unsaved(cx);
                live_changed && editors.response_revision != Some(tab.draft.revision)
            }
            _ => false,
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        #[cfg(test)]
        let enforce_dirty_confirm = FORCE_CLOSE_UNSAVED_PROMPT_IN_TEST.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let enforce_dirty_confirm = true;
        let should_confirm = theme::confirm_on_close_tab_with_unsaved_draft()
            && (self.storage_error.is_some() || (enforce_dirty_confirm && had_live_unsaved_edit));
        if should_confirm {
            self.prompt_close_unpersisted_tab(id, window, cx);
            return;
        }
        self.finish_close_repeater_tab(id, window, cx);
    }

    pub(in crate::workbench) fn finish_close_repeater_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_sends.remove(&id);
        self.abort_sequential_group_queue_if_active_tab(id);
        if let Some(index) = self.workspace.tabs.iter().position(|tab| tab.id == id) {
            // The focused close control or editor must not outlive its tab's dispatch scope.
            self.focus.focus(window, cx);
            self.close_plugin_tab(id, cx);
            let tab = self.workspace.tabs.remove(index);
            self.workspace.closed.push(tab);
            self.record_closed_repeater_tab(id, false);
            if self.workspace.active == Some(id) {
                self.workspace.active = self
                    .workspace
                    .tabs
                    .get(index.min(self.workspace.tabs.len().saturating_sub(1)))
                    .map(|tab| tab.id);
                if self.workspace.active.is_none() {
                    self.workspace.active_websocket_tab = self
                        .workspace
                        .websocket_tabs
                        .iter()
                        .rev()
                        .find(|tab| !tab.closed)
                        .map(|tab| tab.id);
                }
            }
            self.tab_editors.remove(&id);
            self.tab_editor_lru.retain(|&existing| existing != id);
            if let Some(next_id) = self.workspace.active {
                self.ensure_tab_editor(next_id, window, cx);
            }
            self.refresh_archive_response(window, cx);
            self.refresh_repeater_history(window, cx);
            self.save_workspace(cx);
            self.status = "工作标签已关闭，可重新打开；草稿保存在当前项目。".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn restore_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.tool, Tool::Repeater | Tool::Intruder | Tool::Proxy) {
            return;
        }
        if self.tool == Tool::Intruder {
            self.restore_intruder_tab(window, cx);
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let next_recorded = {
            let mut found = None;
            while let Some(pos) = self
                .closed_repeater_order
                .iter()
                .rposition(|(proj, _, _)| *proj == self.project)
            {
                let (_, id, is_ws) = self.closed_repeater_order.remove(pos);
                if is_ws {
                    if self
                        .workspace
                        .websocket_tabs
                        .iter()
                        .any(|tab| tab.closed && tab.id == id)
                    {
                        found = Some((id, true));
                        break;
                    }
                } else if self.workspace.closed.iter().any(|tab| tab.id == id) {
                    found = Some((id, false));
                    break;
                }
            }
            found
        };
        match next_recorded {
            Some((id, false)) => {
                if let Some(pos) = self.workspace.closed.iter().position(|tab| tab.id == id) {
                    let tab = self.workspace.closed.remove(pos);
                    self.workspace.active = Some(tab.id);
                    self.workspace.active_websocket_tab = None;
                    self.workspace.tabs.push(tab);
                    self.tool = Tool::Repeater;
                    self.sync_tab_editors(window, cx);
                    self.save_workspace(cx);
                }
            }
            Some((id, true)) => {
                if let Some(tab) = self
                    .workspace
                    .websocket_tabs
                    .iter_mut()
                    .find(|tab| tab.closed && tab.id == id)
                {
                    tab.closed = false;
                    self.workspace.active = None;
                    self.workspace.active_websocket_tab = Some(id);
                    self.tool = Tool::Repeater;
                    self.sync_tab_editors(window, cx);
                    self.sync_websocket_repeater_editors(window, cx);
                    self.save_workspace(cx);
                }
            }
            None => {
                if let Some(tab) = self.workspace.closed.pop() {
                    self.workspace.active = Some(tab.id);
                    self.workspace.active_websocket_tab = None;
                    self.workspace.tabs.push(tab);
                    self.tool = Tool::Repeater;
                    self.sync_tab_editors(window, cx);
                    self.save_workspace(cx);
                } else if let Some(tab) = self
                    .workspace
                    .websocket_tabs
                    .iter_mut()
                    .rev()
                    .find(|tab| tab.closed)
                {
                    tab.closed = false;
                    let id = tab.id;
                    self.workspace.active = None;
                    self.workspace.active_websocket_tab = Some(id);
                    self.tool = Tool::Repeater;
                    self.sync_tab_editors(window, cx);
                    self.sync_websocket_repeater_editors(window, cx);
                    self.save_workspace(cx);
                }
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn close_tabs(
        &mut self,
        ids: Vec<TabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || ids.is_empty()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let busy = ids
            .iter()
            .filter(|id| self.sending.contains_key(id))
            .count();
        if busy == 0 {
            for id in ids {
                self.close_tab(id, window, cx);
            }
            return;
        }
        let project = self.project;
        let runs: HashMap<_, _> = ids
            .iter()
            .filter_map(|id| self.sending.get(id).map(|send| (*id, send.run_id)))
            .collect();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("{busy} 个标签仍在发送请求"),
            Some("关闭前会取消这些请求，已编辑草稿可通过“重新打开已关闭标签”恢复。"),
            &["取消发送并关闭", "保留标签"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = answer.await {
                let _ = this.update_in(cx, |view, window, cx| {
                    if view.project != project
                        || view.transition.is_some()
                        || !view.backend_available()
                    {
                        return;
                    }
                    for id in ids {
                        if let Some(send) = view.sending.get(&id) {
                            if runs.get(&id) == Some(&send.run_id) && view.cancel_send(id, cx) {
                                view.close_after_send.insert(id);
                            }
                        } else {
                            view.close_tab(id, window, cx);
                        }
                    }
                });
            }
        })
        .detach();
    }

    pub(in crate::workbench) fn step_tab(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Intruder {
            self.step_intruder_tab(direction, window, cx);
            return;
        }
        let tabs = ordered_tabs(&self.workspace.tabs);
        let mut combined: Vec<(TabId, bool)> =
            tabs.into_iter().map(|tab| (tab.id, false)).collect();
        combined.extend(
            self.workspace
                .websocket_tabs
                .iter()
                .filter(|tab| !tab.closed)
                .map(|tab| (tab.id, true)),
        );
        let len = combined.len();
        if len == 0 {
            return;
        }
        let current = self
            .workspace
            .active
            .or(self.workspace.active_websocket_tab);
        let index = combined
            .iter()
            .position(|(id, _)| Some(*id) == current)
            .unwrap_or(0);
        let next = (index as isize + direction).rem_euclid(len as isize) as usize;
        let (next, is_ws) = combined[next];
        if is_ws {
            self.select_websocket_repeater_tab(next, window, cx);
        } else {
            self.select_tab(next, window, cx);
        }
    }

    pub(in crate::workbench) fn move_active(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Intruder {
            self.move_intruder_tab_by(direction, window, cx);
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let tabs = ordered_tabs(&self.workspace.tabs);
        let Some(index) = tabs
            .iter()
            .position(|tab| Some(tab.id) == self.workspace.active)
        else {
            return;
        };
        let Some(next) = index
            .checked_add_signed(direction)
            .filter(|next| *next < tabs.len())
        else {
            return;
        };
        let id = tabs[index].id;
        let group = tabs[next].group;
        let mut order: Vec<_> = tabs.into_iter().map(|tab| tab.id).collect();
        order.swap(index, next);
        let order: HashMap<_, _> = order
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();
        if let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.group = group;
        }
        self.workspace.tabs.sort_by_key(|tab| order[&tab.id]);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn move_tab(
        &mut self,
        id: TabId,
        before: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let after = self.drop_after;
        self.drop_target = None;
        self.drop_after = false;
        if id == before || !self.flush_workspace(window, cx) {
            return;
        }
        let Some(from) = self.workspace.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let Some(to) = self.workspace.tabs.iter().position(|tab| tab.id == before) else {
            return;
        };
        let group = self.workspace.tabs[to].group;
        let mut tab = self.workspace.tabs.remove(from);
        tab.group = group;
        self.workspace.tabs.insert(
            (if from < to { to - 1 } else { to }) + usize::from(after),
            tab,
        );
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn move_to_group(
        &mut self,
        id: TabId,
        group: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.group = group;
        }
        if let Some(group_id) = group
            && let Some(group) = self
                .workspace
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
        {
            group.collapsed = false;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn move_websocket_tab_to_group(
        &mut self,
        id: TabId,
        group: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(ws_tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|tab| !tab.closed && tab.id == id)
        {
            ws_tab.group = group;
        }
        if let Some(group_id) = group
            && let Some(target_group) = self
                .workspace
                .groups
                .iter_mut()
                .find(|g| g.id == group_id)
        {
            target_group.collapsed = false;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn create_group_for_websocket_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        if self.workspace.groups.len() + self.workspace.intruder.groups.len() >= 512 {
            self.fail("工作组数量已达上限；无法新建分组。", cx);
            return;
        }
        let mut number = 1usize;
        while self
            .workspace
            .groups
            .iter()
            .any(|group| group.title == format!("Group {number}"))
        {
            number += 1;
        }
        let group_id = Uuid::new_v4();
        let new_group = TabGroup {
            id: group_id,
            title: format!("Group {number}"),
            collapsed: false,
            color: Some(7),
        };
        let mut candidate = self.workspace.clone();
        candidate.groups.push(new_group);
        if let Some(ws_tab) = candidate
            .websocket_tabs
            .iter_mut()
            .find(|tab| !tab.closed && tab.id == id)
        {
            ws_tab.group = Some(group_id);
        } else {
            return;
        }
        if let Err(error) = candidate.validate_structure() {
            self.fail(format!("无法为 WebSocket 标签创建分组：{error}"), cx);
            return;
        }
        self.workspace = candidate;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn new_group(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_tab_group_editor(None, Some(id), window, cx);
    }
}
