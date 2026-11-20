use super::*;

impl Workbench {
    pub(in crate::workbench) fn open_websocket_repeater_from_history(
        &mut self,
        message_id: Uuid,
        use_original: bool,
        payload_override: Option<(WebSocketKind, Vec<u8>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        let _ = self.enqueue(
            Command::WebSocketRepeater(WebSocketRepeaterCommand::OpenFromHistory {
                project,
                operation_id: Uuid::new_v4(),
                message_id,
                use_original,
                payload_override,
            }),
            cx,
        );
        cx.notify();
    }

    pub(in crate::workbench) fn send_intercept_websocket_to_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.selected_intercept else {
            return;
        };
        let Some(pending) = self.intercepts.iter().find(|item| item.id() == selected) else {
            return;
        };
        let InterceptTarget::WebSocket { record, .. } = &pending.target else {
            return;
        };
        let message_id = record.id;
        let kind = record.kind;
        let bytes = match pending.editor.bytes(window, cx) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if bytes.len() > MAX_WS_PAYLOAD_BYTES {
            self.fail("WebSocket 消息超过 16 MiB 限额；未发送到 Repeater。", cx);
            return;
        }
        self.open_websocket_repeater_from_history(
            message_id,
            false,
            Some((kind, bytes)),
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn connect_websocket_repeater(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| !tab.closed && tab.id == tab_id)
            .cloned()
        else {
            return;
        };
        if tab.url.trim().is_empty() {
            self.fail("请输入有效的 WebSocket 目标 URL（ws:// 或 wss://）。", cx);
            return;
        }
        let already_connecting = self
            .websocket_repeater
            .editors
            .get(&tab_id)
            .is_some_and(|editors| editors.connecting.is_some() || editors.disconnecting);
        if already_connecting {
            return;
        }
        let cancel = CancellationToken::new();
        if self.enqueue(
            Command::WebSocketRepeater(WebSocketRepeaterCommand::Connect {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: tab.url,
                handshake_headers: tab.handshake_headers,
                source_flow_id: tab.source_flow_id,
                cancel: cancel.clone(),
            }),
            cx,
        ) && let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id)
        {
            editors.connecting = Some(cancel);
            editors.result = Some("正在连接…".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn disconnect_websocket_repeater(
        &mut self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) else {
            return;
        };
        if editors.disconnecting || (!editors.mode.is_connected() && editors.connecting.is_none()) {
            return;
        }
        if let Some(cancel) = editors.connecting.take() {
            cancel.cancel();
        }
        editors.disconnecting = true;
        editors.result = Some("正在断开连接…".into());
        if !self.enqueue(
            Command::WebSocketRepeater(WebSocketRepeaterCommand::Disconnect {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
            }),
            cx,
        ) && let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id)
        {
            editors.disconnecting = false;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn set_websocket_repeater_direction(
        &mut self,
        tab_id: TabId,
        direction: WebSocketDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let is_standalone = self
            .websocket_repeater
            .editors
            .get(&tab_id)
            .is_some_and(|editors| {
                matches!(
                    editors.mode,
                    WebSocketRepeaterConnectionMode::Standalone { .. }
                )
            });
        if is_standalone && direction != WebSocketDirection::ClientToServer {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|tab| !tab.closed && tab.id == tab_id)
            && tab.direction != direction
        {
            tab.direction = direction;
            self.save_workspace(cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn set_websocket_repeater_kind(
        &mut self,
        tab_id: TabId,
        kind: WebSocketKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|tab| !tab.closed && tab.id == tab_id)
            && tab.draft_kind != kind
        {
            tab.draft_kind = kind;
            self.save_workspace(cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn send_websocket_repeater(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.pending_records > 0 {
            self.fail("仍有流量尚未写入项目，请先保存或另存为恢复存储。", cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let (payload, url, mode, busy) = {
            let Some(editors) = self.websocket_repeater.editors.get(&tab_id) else {
                return;
            };
            let busy = editors.sending.is_some() || editors.connecting.is_some();
            let payload = match editors.draft.bytes(window, cx) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            let url = match editing::input_snapshot(&editors.url, window, cx) {
                Ok(url) => url.trim().to_owned(),
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            (payload, url, editors.mode, busy)
        };
        if busy {
            return;
        }
        if payload.len() > MAX_WS_PAYLOAD_BYTES {
            self.fail("WebSocket Repeater 草稿超过 16 MiB 限额；未发送。", cx);
            return;
        }
        if url.len() > MAX_WS_URL_BYTES {
            self.fail("WebSocket Repeater 目标 URL 超过 64 KiB 限额；未发送。", cx);
            return;
        }
        let Some(tab_index) = self
            .workspace
            .websocket_tabs
            .iter()
            .position(|tab| !tab.closed && tab.id == tab_id)
        else {
            return;
        };
        let kind = self.workspace.websocket_tabs[tab_index].draft_kind;
        if kind == WebSocketKind::Text && std::str::from_utf8(&payload).is_err() {
            self.fail(
                "WebSocket Text 帧必须是有效 UTF-8；请修正文本或切换为 Binary 类型。",
                cx,
            );
            return;
        }
        let Some(next_revision) = self.workspace.websocket_tabs[tab_index]
            .draft_revision
            .checked_add(1)
        else {
            self.fail("WebSocket Repeater 草稿 revision 已耗尽；未发送。", cx);
            return;
        };
        let direction = if matches!(mode, WebSocketRepeaterConnectionMode::Standalone { .. }) {
            WebSocketDirection::ClientToServer
        } else {
            self.workspace.websocket_tabs[tab_index].direction
        };
        let source_flow_id = self.workspace.websocket_tabs[tab_index].source_flow_id;
        {
            let tab = &mut self.workspace.websocket_tabs[tab_index];
            tab.url = url.clone();
            tab.direction = direction;
            tab.draft_payload = payload.clone();
            tab.draft_revision = next_revision;
        }
        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
            editors.loaded_revision = next_revision;
            editors.dirty = false;
        }
        self.save_workspace(cx);
        let cancel = CancellationToken::new();
        if self.enqueue(
            Command::WebSocketRepeater(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url,
                source_flow_id,
                direction,
                kind,
                payload,
                draft_revision: next_revision,
                cancel: cancel.clone(),
            }),
            cx,
        ) && let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id)
        {
            editors.sending = Some((next_revision, cancel));
            editors.result = Some("正在发送…".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_websocket_repeater_history(
        &mut self,
        tab_id: TabId,
        item_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payload = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| !tab.closed && tab.id == tab_id)
            .and_then(|tab| tab.history.iter().find(|item| item.id == item_id))
            .map(|item| item.payload.clone());
        let Some(payload) = payload else {
            return;
        };
        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
            editors.selected_history = Some(item_id);
            editors.history.load(&payload, window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn load_history_into_websocket_draft(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let (selected_id, mode, has_composition) = {
            let Some(editors) = self.websocket_repeater.editors.get(&tab_id) else {
                return;
            };
            if editors.sending.is_some() {
                return;
            }
            let composing = [&editors.draft.raw, &editors.draft.hex]
                .into_iter()
                .any(|input| {
                    input.update(cx, |state, cx| {
                        state.marked_text_range(window, cx).is_some()
                    })
                })
                || editors.draft.search.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                })
                || editors.url.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
            (editors.selected_history, editors.mode, composing)
        };
        if has_composition {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let Some(selected_id) = selected_id else {
            return;
        };
        let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|tab| !tab.closed && tab.id == tab_id)
        else {
            return;
        };
        let Some(item) = tab
            .history
            .iter()
            .find(|entry| entry.id == selected_id)
            .cloned()
        else {
            return;
        };
        let Some(next_revision) = tab.draft_revision.checked_add(1) else {
            self.fail(
                "WebSocket Repeater 草稿 revision 已耗尽；无法载入历史消息。",
                cx,
            );
            return;
        };
        tab.draft_payload = item.payload.clone();
        tab.draft_revision = next_revision;
        tab.draft_kind = item.kind;
        if !matches!(mode, WebSocketRepeaterConnectionMode::Standalone { .. }) {
            tab.direction = item.direction;
        } else {
            tab.direction = WebSocketDirection::ClientToServer;
        }
        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
            editors.draft.load(&item.payload, window, cx);
            editors.loaded_revision = next_revision;
            editors.dirty = false;
        }
        self.save_workspace(cx);
        self.status = "已将选中 WebSocket 历史消息载入发送框。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn load_specific_history_into_websocket_draft(
        &mut self,
        tab_id: TabId,
        item_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_websocket_repeater_history(tab_id, item_id, window, cx);
        self.load_history_into_websocket_draft(tab_id, window, cx);
    }

    pub(in crate::workbench) fn toggle_websocket_repeater_headers_panel(
        &mut self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) {
        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
            editors.headers_expanded = !editors.headers_expanded;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_websocket_repeater_auto_follow(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut load_latest = None;
        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
            editors.auto_follow_latest = !editors.auto_follow_latest;
            if editors.auto_follow_latest
                && let Some(latest) = self
                    .workspace
                    .websocket_tabs
                    .iter()
                    .find(|tab| !tab.closed && tab.id == tab_id)
                    .and_then(|tab| tab.history.last())
            {
                load_latest = Some((latest.id, latest.payload.clone()));
            }
        }
        if let Some((latest_id, payload)) = load_latest
            && let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id)
        {
            editors.selected_history = Some(latest_id);
            editors.history.load(&payload, window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn add_websocket_repeater_handshake_header(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.project.is_none() {
            return;
        }
        let (raw_name, raw_value) = {
            let Some(editors) = self.websocket_repeater.editors.get(&tab_id) else {
                return;
            };
            let name = match editing::input_snapshot(&editors.header_name, window, cx) {
                Ok(val) => val.trim().to_owned(),
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            let value = match editing::input_snapshot(&editors.header_value, window, cx) {
                Ok(val) => val.trim().to_owned(),
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            (name, value)
        };
        let (name, value) = if raw_value.is_empty()
            && let Some((parsed_name, parsed_val)) = raw_name.split_once(':')
        {
            (parsed_name.trim().to_owned(), parsed_val.trim().to_owned())
        } else {
            (raw_name, raw_value)
        };
        if name.is_empty() {
            self.fail("握手标头名称不能为空。", cx);
            return;
        }
        if value.contains(['\r', '\n', '\0']) || name.contains(['\r', '\n', '\0']) {
            self.fail("握手标头不能包含换行符或空字节。", cx);
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab_index) = self
            .workspace
            .websocket_tabs
            .iter()
            .position(|tab| !tab.closed && tab.id == tab_id)
        else {
            return;
        };
        let mut candidate = self.workspace.websocket_tabs[tab_index].clone();
        candidate.handshake_headers.push(Header {
            name: name.clone(),
            value: value.into_bytes(),
        });
        if let Err(error) = candidate.validate() {
            self.fail(format!("未添加握手标头：{error}"), cx);
            return;
        }
        self.workspace.websocket_tabs[tab_index] = candidate;
        if let Some(editors) = self.websocket_repeater.editors.get(&tab_id) {
            editors
                .header_name
                .update(cx, |state, cx| state.set_value("", window, cx));
            editors
                .header_value
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.save_workspace(cx);
        self.status = format!("已添加 WebSocket 握手标头「{name}」，重连会话时生效。");
        cx.notify();
    }

    pub(in crate::workbench) fn remove_websocket_repeater_handshake_header(
        &mut self,
        tab_id: TabId,
        header_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.project.is_none() {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|tab| !tab.closed && tab.id == tab_id)
        else {
            return;
        };
        if header_index >= tab.handshake_headers.len() {
            return;
        }
        let removed = tab.handshake_headers.remove(header_index);
        self.save_workspace(cx);
        self.status = format!("已移除握手标头「{}」。", removed.name);
        cx.notify();
    }

    pub(in crate::workbench) fn send_ws_repeater_history_to_decoder(
        &mut self,
        tab_id: TabId,
        item_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        if let Err(error) = editing::snapshot(&self.extra_tools.decoder_input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let Some(bytes) = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| !tab.closed && tab.id == tab_id)
            .and_then(|tab| tab.history.iter().find(|item| item.id == item_id))
            .map(|item| item.payload.clone())
        else {
            return;
        };
        const MAX_DECODER_BYTES: usize = 512 * 1024;
        if bytes.len() > MAX_DECODER_BYTES {
            self.fail(
                format!(
                    "Decoder 输入超过 {} KiB 上限，未覆盖原内容。",
                    MAX_DECODER_BYTES / 1024
                ),
                cx,
            );
            return;
        }
        let (display, raw_backup) = match String::from_utf8(bytes.clone()) {
            Ok(text) => (text, None),
            Err(_) => (editing::hex_encode(&bytes), Some(bytes)),
        };
        self.extra_tools
            .decoder_input
            .update(cx, |state, cx| state.set_value(display, window, cx));
        let rev = self.extra_tools.decoder_input.read(cx).document_revision();
        self.extra_tools.decoder_input_bytes = raw_backup.map(|b| (rev, b));
        self.extra_tools
            .decoder_output
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.extra_tools.decoder_output_bytes = None;
        self.extra_tools.decoder_mode = "".into();
        self.extra_tools.steps.clear();
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Decoder;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn send_ws_repeater_history_to_comparer(
        &mut self,
        tab_id: TabId,
        item_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let Some((ordinal, bytes)) = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| !tab.closed && tab.id == tab_id)
            .and_then(|tab| {
                tab.history
                    .iter()
                    .enumerate()
                    .find(|(_, item)| item.id == item_id)
                    .map(|(idx, item)| (idx + 1, item.payload.clone()))
            })
        else {
            return;
        };
        const MAX_COMPARER_ITEMS: usize = 32;
        const MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;
        if bytes.len() > MAX_COMPARER_ITEM_BYTES {
            self.fail(
                format!(
                    "Comparer 单项超过 {} KiB 上限，未添加。",
                    MAX_COMPARER_ITEM_BYTES / 1024
                ),
                cx,
            );
            return;
        }
        if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
            self.fail(
                format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                cx,
            );
            return;
        }
        let next_id = self
            .extra_tools
            .comparer_items
            .last()
            .map_or(1, |entry| entry.id + 1);
        self.extra_tools
            .comparer_items
            .push(tools_extra::ComparerItem {
                id: next_id,
                label: format!("{next_id}: WS 会话 #{ordinal} ({} bytes)", bytes.len()),
                bytes,
            });
        let count = self.extra_tools.comparer_items.len();
        self.extra_tools.comparer_selected = count - 1;
        if count >= 2 {
            self.extra_tools.left_index = count - 2;
            self.extra_tools.right_index = count - 1;
        }
        self.refresh_comparer(window, cx);
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Comparer;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn copy_ws_repeater_history_payload(
        &mut self,
        tab_id: TabId,
        item_id: Uuid,
        as_hex: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(bytes) = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| !tab.closed && tab.id == tab_id)
            .and_then(|tab| tab.history.iter().find(|item| item.id == item_id))
            .map(|item| item.payload.clone())
        else {
            return;
        };
        let content = if as_hex {
            editing::hex_encode(&bytes)
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(content));
        self.status = if as_hex {
            "已复制 WebSocket 历史消息载荷 Hex 到剪贴板。".into()
        } else {
            "已复制 WebSocket 历史消息载荷文本到剪贴板。".into()
        };
        cx.notify();
    }
}
