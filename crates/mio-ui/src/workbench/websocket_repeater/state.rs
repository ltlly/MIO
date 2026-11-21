use super::*;

fn create_editors(
    tab: &WebSocketRepeaterTab,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> WsRepeaterEditors {
    let id = tab.id;
    let mut draft = MessageEditor::new(window, cx);
    draft.load(&tab.draft_payload, window, cx);
    let history = MessageEditor::new(window, cx);
    let url = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(tab.url.clone())
            .placeholder("ws://127.0.0.1:8080/socket")
    });
    let header_name = cx.new(|cx| InputState::new(window, cx).placeholder("标头名称（例如 Authorization）"));
    let header_value = cx.new(|cx| InputState::new(window, cx).placeholder("标头内容（例如 Bearer token）"));
    let mut subscriptions = Vec::new();
    for input in [&draft.raw, &draft.hex] {
        let mut composing = false;
        subscriptions.push(cx.observe_in(input, window, move |_, input, window, cx| {
            let marked = input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            });
            if marked != composing {
                composing = marked;
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe(input, move |this, _, event, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(editors) = this.websocket_repeater.editors.get_mut(&id)
            {
                editors.dirty = true;
            }
            cx.notify();
        }));
    }
    let mut composing = false;
    subscriptions.push(cx.observe_in(&url, window, move |_, input, window, cx| {
        let marked = input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        });
        if marked != composing {
            composing = marked;
            cx.notify();
        }
    }));
    subscriptions.push(cx.subscribe(&url, move |this, _, event, cx| {
        if matches!(event, InputEvent::Change)
            && let Some(editors) = this.websocket_repeater.editors.get_mut(&id)
        {
            editors.dirty = true;
        }
        cx.notify();
    }));
    for input in [&header_name, &header_value] {
        let mut composing = false;
        subscriptions.push(cx.observe_in(input, window, move |_, input, window, cx| {
            let marked = input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            });
            if marked != composing {
                composing = marked;
                cx.notify();
            }
        }));
    }
    let selected_history = tab.history.last().map(|item| item.id);
    let mut editors = WsRepeaterEditors {
        draft,
        history,
        url,
        header_name,
        header_value,
        headers_expanded: false,
        auto_follow_latest: true,
        mode: WebSocketRepeaterConnectionMode::Disconnected,
        connecting: None,
        sending: None,
        disconnecting: false,
        selected_history,
        dirty: false,
        loaded_revision: tab.draft_revision,
        result: None,
        _subscriptions: subscriptions,
    };
    if let Some(item) = tab
        .history
        .iter()
        .rev()
        .find(|item| Some(item.id) == selected_history)
    {
        editors.history.load(&item.payload, window, cx);
    }
    editors
}

impl Workbench {
    pub(in crate::workbench) fn reset_websocket_repeater(&mut self) {
        for editors in self.websocket_repeater.editors.values_mut() {
            if let Some(cancel) = editors.connecting.take() {
                cancel.cancel();
            }
            if let Some((_, cancel)) = editors.sending.take() {
                cancel.cancel();
            }
        }
        self.websocket_repeater.editors.clear();
    }

    pub(in crate::workbench) fn websocket_repeater_backend_disconnected(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        for editors in self.websocket_repeater.editors.values_mut() {
            if let Some(cancel) = editors.connecting.take() {
                cancel.cancel();
            }
            if let Some((_, cancel)) = editors.sending.take() {
                cancel.cancel();
            }
            editors.disconnecting = false;
            editors.mode = WebSocketRepeaterConnectionMode::Disconnected;
            editors.result = Some("后台服务已断开；会话结果未确认。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn websocket_repeater_proxy_stopped(&mut self) {
        for editors in self.websocket_repeater.editors.values_mut() {
            if matches!(
                editors.mode,
                WebSocketRepeaterConnectionMode::ProxyShared { .. }
            ) {
                editors.mode = WebSocketRepeaterConnectionMode::Disconnected;
                editors.result = Some("Proxy 已停止；请点击连接建立独立会话。".into());
            }
        }
    }

    pub(in crate::workbench) fn websocket_repeater_unsaved(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.websocket_repeater.editors.values().any(|editors| {
            editors.dirty
                || [&editors.draft.raw, &editors.draft.hex]
                    .into_iter()
                    .any(|input| {
                        input.update(cx, |state, cx| {
                            state.marked_text_range(window, cx).is_some()
                        })
                    })
                || [&editors.url, &editors.header_name, &editors.header_value]
                    .into_iter()
                    .any(|input| {
                        input.update(cx, |state, cx| {
                            state.marked_text_range(window, cx).is_some()
                        })
                    })
        })
    }

    pub(in crate::workbench) fn sync_websocket_repeater_editors(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: HashSet<_> = self
            .workspace
            .websocket_tabs
            .iter()
            .filter(|tab| !tab.closed)
            .map(|tab| tab.id)
            .collect();
        for (id, editors) in &mut self.websocket_repeater.editors {
            if !ids.contains(id) {
                if let Some(cancel) = editors.connecting.take() {
                    cancel.cancel();
                }
                if let Some((_, cancel)) = editors.sending.take() {
                    cancel.cancel();
                }
            }
        }
        self.websocket_repeater
            .editors
            .retain(|id, _| ids.contains(id));
        if self.workspace.active.is_some() {
            self.workspace.active_websocket_tab = None;
        } else if let Some(active) = self.workspace.active_websocket_tab
            && !ids.contains(&active)
        {
            self.workspace.active_websocket_tab = None;
        }
        for tab in self
            .workspace
            .websocket_tabs
            .iter()
            .filter(|tab| !tab.closed)
        {
            let id = tab.id;
            if let Some(editors) = self.websocket_repeater.editors.get_mut(&id) {
                if !editors.dirty && editors.loaded_revision < tab.draft_revision {
                    editors.draft.load(&tab.draft_payload, window, cx);
                    editors
                        .url
                        .update(cx, |state, cx| state.set_value(tab.url.clone(), window, cx));
                    editors.loaded_revision = tab.draft_revision;
                }
                let valid_selection = editors
                    .selected_history
                    .is_some_and(|sel| tab.history.iter().any(|item| item.id == sel));
                if !valid_selection {
                    let fallback = tab.history.last().map(|item| item.id);
                    editors.selected_history = fallback;
                    if let Some(item) = tab.history.last() {
                        editors.history.load(&item.payload, window, cx);
                    } else {
                        editors.history.load(&[], window, cx);
                    }
                }
                continue;
            }
            let editors = create_editors(tab, window, cx);
            self.websocket_repeater.editors.insert(id, editors);
        }
    }

    pub(in crate::workbench) fn snapshot_websocket_repeater_drafts(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Vec<WsDraftSnapshot>, String> {
        let mut snapshots = Vec::new();
        for tab in &self.workspace.websocket_tabs {
            if tab.closed {
                continue;
            }
            let Some(editors) = self.websocket_repeater.editors.get(&tab.id) else {
                continue;
            };
            editing::input_snapshot(&editors.header_name, window, cx).map_err(str::to_owned)?;
            editing::input_snapshot(&editors.header_value, window, cx).map_err(str::to_owned)?;
            if !editors.dirty && self.workspace.active_websocket_tab != Some(tab.id) {
                continue;
            }
            let payload = editors.draft.bytes(window, cx)?;
            let url = editing::input_snapshot(&editors.url, window, cx)
                .map_err(str::to_owned)?
                .trim()
                .to_owned();
            if payload.len() > MAX_WS_PAYLOAD_BYTES {
                return Err("WebSocket Repeater 草稿超过 16 MiB 限额；修改仍保留。".into());
            }
            if url.len() > MAX_WS_URL_BYTES {
                return Err("WebSocket Repeater 目标 URL 超过 64 KiB 限额；修改仍保留。".into());
            }
            snapshots.push(WsDraftSnapshot {
                id: tab.id,
                url,
                payload,
            });
        }
        Ok(snapshots)
    }

    pub(in crate::workbench) fn apply_websocket_repeater_drafts(
        &mut self,
        snapshots: Vec<WsDraftSnapshot>,
    ) -> bool {
        let mut changed = false;
        for snapshot in snapshots {
            let Some(tab) = self
                .workspace
                .websocket_tabs
                .iter_mut()
                .find(|tab| tab.id == snapshot.id)
            else {
                continue;
            };
            if tab.draft_payload != snapshot.payload || tab.url != snapshot.url {
                tab.draft_payload = snapshot.payload;
                tab.url = snapshot.url;
                tab.draft_revision += 1;
                changed = true;
            }
            if let Some(editors) = self.websocket_repeater.editors.get_mut(&snapshot.id) {
                editors.loaded_revision = tab.draft_revision;
                editors.dirty = false;
            }
        }
        changed
    }

    pub(in crate::workbench) fn select_websocket_repeater_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backend_available() && !self.flush_workspace(window, cx) {
            return;
        }
        if self
            .workspace
            .websocket_tabs
            .iter()
            .any(|tab| !tab.closed && tab.id == id)
        {
            self.cancel_plugin_interactions(cx);
            self.workspace.active = None;
            self.workspace.active_websocket_tab = Some(id);
            if self.backend_available() {
                self.save_workspace(cx);
            }
            if let Some(editors) = self.websocket_repeater.editors.get(&id) {
                editors
                    .draft
                    .state()
                    .update(cx, |state, cx| state.focus(window, cx));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn close_websocket_repeater_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || !self.flush_workspace(window, cx) {
            return;
        }
        let open_ids: Vec<TabId> = self
            .workspace
            .websocket_tabs
            .iter()
            .filter(|tab| !tab.closed)
            .map(|tab| tab.id)
            .collect();
        let Some(pos) = open_ids.iter().position(|existing| *existing == id) else {
            return;
        };
        self.focus.focus(window, cx);
        if let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|tab| tab.id == id)
        {
            tab.closed = true;
            self.record_closed_repeater_tab(id, true);
        }
        if let Some(mut editors) = self.websocket_repeater.editors.remove(&id) {
            if let Some(cancel) = editors.connecting.take() {
                cancel.cancel();
            }
            if let Some((_, cancel)) = editors.sending.take() {
                cancel.cancel();
            }
        }
        if self.workspace.active_websocket_tab == Some(id) {
            let remaining: Vec<TabId> = open_ids.into_iter().filter(|item| *item != id).collect();
            if let Some(next_ws) = remaining
                .get(pos.min(remaining.len().saturating_sub(1)))
                .copied()
            {
                self.workspace.active_websocket_tab = Some(next_ws);
                self.workspace.active = None;
            } else {
                self.workspace.active_websocket_tab = None;
                self.workspace.active = self.workspace.tabs.last().map(|tab| tab.id);
                if let Some(id) = self.workspace.active {
                    self.ensure_tab_editor(id, window, cx);
                }
                self.refresh_archive_response(window, cx);
                self.refresh_repeater_history(window, cx);
            }
        }
        if let Some(project) = self.project {
            let _ = self.enqueue(
                Command::WebSocketRepeater(WebSocketRepeaterCommand::Close {
                    project,
                    tab_id: id,
                }),
                cx,
            );
        }
        self.save_workspace(cx);
        self.status = "WebSocket Repeater 标签已关闭。".into();
        cx.notify();
    }
}
