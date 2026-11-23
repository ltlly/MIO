use mio_core::WebSocketRepeaterCommand;

use super::*;

const MAX_BODY: usize = 16 * 1024 * 1024;
const MAX_DECODER_BYTES: usize = 4 * 1024 * 1024;
const MAX_COMPARER_ITEMS: usize = 64;
const MAX_COMPARER_ITEM_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(in crate::workbench) enum PendingWsTransfer {
    Repeater { use_original: bool },
    Decoder,
    Comparer,
}

impl Workbench {
    pub(in crate::workbench::websocket) fn send_selected_websocket_to_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message_id) = self.websocket.selected else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        let use_original = self.websocket.detail.is_some() && self.websocket.original_view;
        self.send_websocket_to_repeater(message_id, use_original, window, cx);
    }

    pub(in crate::workbench::websocket) fn send_websocket_to_repeater(
        &mut self,
        message_id: Uuid,
        use_original: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let summary = self.find_websocket_summary(message_id, cx);
        if summary.as_ref().is_none_or(|row| !row.flow_id.is_nil()) {
            self.open_websocket_repeater_from_history(message_id, use_original, None, window, cx);
            return;
        }
        let summary = summary.expect("checked archived summary");
        if self.websocket.selected == Some(message_id)
            && let Some(detail) = self
                .websocket
                .detail
                .as_deref()
                .filter(|record| record.id == message_id)
        {
            let source_payload =
                if use_original && let Some(original) = &self.websocket.original_payload {
                    original.clone()
                } else {
                    detail.payload.clone()
                };
            if source_payload.len() > MAX_BODY {
                self.fail("WebSocket 消息超过 16 MiB 限额；未发送到 Repeater。", cx);
                return;
            }
            let source_direction = detail.direction;
            let source_kind = detail.kind;
            if !self.flush_workspace(window, cx) {
                return;
            }
            let title = self.next_archived_ws_repeater_title();
            let url = normalize_ws_url(&summary.url);
            let _ = self.enqueue(
                Command::WebSocketRepeater(WebSocketRepeaterCommand::Open {
                    project,
                    operation_id: Uuid::new_v4(),
                    title: Some(title),
                    url,
                    handshake_headers: Vec::new(),
                    source_flow_id: None,
                    source_message_id: Some(message_id),
                    source_direction,
                    source_kind,
                    source_payload,
                }),
                cx,
            );
            cx.notify();
            return;
        }
        self.select_websocket(message_id, window, cx);
        if self.websocket.selected == Some(message_id) {
            self.websocket.pending_transfer =
                Some((message_id, PendingWsTransfer::Repeater { use_original }));
        }
    }

    pub(in crate::workbench::websocket) fn send_websocket_to_decoder(
        &mut self,
        target_id: Option<Uuid>,
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
        let Some(message_id) = target_id.or(self.websocket.selected) else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        if self.websocket.selected == Some(message_id)
            && self
                .websocket
                .detail
                .as_deref()
                .is_some_and(|record| record.id == message_id)
        {
            let payload = match self.extract_websocket_decoder_payload(window, cx) {
                Ok(payload) => payload,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            let (bytes, maybe_text) = payload;
            if bytes.len() > MAX_DECODER_BYTES {
                self.fail(
                    format!("Decoder 输入超过 {MAX_DECODER_BYTES} 字节上限，未覆盖原内容。"),
                    cx,
                );
                return;
            }
            let (display, raw_backup) = match maybe_text {
                Some(text) => (text, None),
                None => (editing::hex_encode(&bytes), Some(bytes)),
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
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Decoder;
            self.focus.focus(window, cx);
            cx.notify();
            return;
        }
        self.select_websocket(message_id, window, cx);
        if self.websocket.selected == Some(message_id) {
            self.websocket.pending_transfer = Some((message_id, PendingWsTransfer::Decoder));
        }
    }

    pub(in crate::workbench::websocket) fn send_websocket_to_comparer(
        &mut self,
        target_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let Some(message_id) = target_id.or(self.websocket.selected) else {
            self.fail("请先选择一条 WebSocket 消息。", cx);
            return;
        };
        if self.websocket.selected == Some(message_id)
            && let Some(bytes) = self.current_websocket_payload_bytes(message_id)
        {
            if bytes.len() > MAX_COMPARER_ITEM_BYTES {
                self.fail(
                    format!("Comparer 单项超过 {MAX_COMPARER_ITEM_BYTES} 字节上限，未添加。"),
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
            let tag = self
                .find_websocket_summary(message_id, cx)
                .map(|row| format!("WebSocket #{}", row.ordinal))
                .unwrap_or_else(|| "WebSocket".into());
            self.extra_tools
                .comparer_items
                .push(tools_extra::ComparerItem {
                    id: next_id,
                    label: format!("{next_id}: {tag} ({} bytes)", bytes.len()),
                    bytes,
                });
            let count = self.extra_tools.comparer_items.len();
            if count >= 2 {
                self.extra_tools.left_index = count - 2;
                self.extra_tools.right_index = count - 1;
            }
            self.refresh_websocket_comparer_preview(window, cx);
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.focus.focus(window, cx);
            cx.notify();
            return;
        }
        self.select_websocket(message_id, window, cx);
        if self.websocket.selected == Some(message_id) {
            self.websocket.pending_transfer = Some((message_id, PendingWsTransfer::Comparer));
        }
    }

    pub(in crate::workbench::websocket) fn complete_pending_websocket_transfer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target_id, action)) = self.websocket.pending_transfer.take() else {
            return;
        };
        if self.websocket.selected != Some(target_id) {
            return;
        }
        match action {
            PendingWsTransfer::Repeater { use_original } => {
                self.send_websocket_to_repeater(target_id, use_original, window, cx);
            }
            PendingWsTransfer::Decoder => {
                self.send_websocket_to_decoder(Some(target_id), window, cx);
            }
            PendingWsTransfer::Comparer => {
                self.send_websocket_to_comparer(Some(target_id), window, cx);
            }
        }
    }

    pub(in crate::workbench::websocket) fn copy_websocket_url(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        let Some(url) = self
            .websocket
            .selected_summary
            .as_ref()
            .map(|row| row.url.clone())
            .filter(|url| !url.is_empty())
        else {
            self.fail("没有选中的 WebSocket URL。", cx);
            return;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url));
        self.error = None;
        self.status = "已复制 URL 到剪贴板。".into();
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn modify_websocket_scope(
        &mut self,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || self.backend_disconnected {
            return;
        }
        let Some(url) = self
            .websocket
            .selected_summary
            .as_ref()
            .map(|row| row.url.as_str())
            .filter(|url| !url.is_empty())
        else {
            self.fail("没有选中的 WebSocket URL。", cx);
            return;
        };
        let Some(prefix) = ws_origin_to_http_prefix(url) else {
            self.fail("WebSocket URL 格式无效，无法更新范围规则。", cx);
            return;
        };
        self.add_scope_rule(self.project, include, prefix, cx);
    }

    fn find_websocket_summary(&self, message_id: Uuid, cx: &App) -> Option<WebSocketSummary> {
        self.websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .find(|row| row.id == message_id)
            .cloned()
            .or_else(|| {
                self.websocket
                    .selected_summary
                    .clone()
                    .filter(|row| row.id == message_id)
            })
    }

    fn current_websocket_payload_bytes(&self, message_id: Uuid) -> Option<Vec<u8>> {
        let detail = self
            .websocket
            .detail
            .as_deref()
            .filter(|record| record.id == message_id)?;
        if self.websocket.original_view
            && let Some(original) = &self.websocket.original_payload
        {
            Some(original.clone())
        } else {
            Some(detail.payload.clone())
        }
    }

    fn extract_websocket_decoder_payload(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(Vec<u8>, Option<String>), String> {
        let editor = &self.websocket.payload;
        if editor.has_focus(window, cx) {
            editing::snapshot(&editor.state(), window, cx).map_err(str::to_owned)?;
            let selected = editor.state().read(cx).selected_value();
            if !selected.is_empty() {
                if editor.view() == MessageView::Hex && !editor.pretty_active() {
                    let bytes = editing::hex_decode(&selected)?;
                    let text = String::from_utf8(bytes.clone()).ok();
                    return Ok((bytes, text));
                }
                let text = selected.to_string();
                return Ok((text.as_bytes().to_vec(), Some(text)));
            }
        }
        let id = self
            .websocket
            .selected
            .ok_or("请先选择一条 WebSocket 消息。")?;
        let bytes = self
            .current_websocket_payload_bytes(id)
            .ok_or("WebSocket 消息正文尚未读取完成。")?;
        let text = String::from_utf8(bytes.clone()).ok();
        Ok((bytes, text))
    }

    fn next_archived_ws_repeater_title(&self) -> String {
        let mut next = 1usize;
        loop {
            let candidate = format!("WS {next}");
            if !self
                .workspace
                .websocket_tabs
                .iter()
                .any(|tab| tab.title == candidate)
            {
                return candidate;
            }
            next = next.saturating_add(1);
        }
    }

    fn refresh_websocket_comparer_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &self.extra_tools;
        let text = match (
            pane.comparer_items.get(pane.left_index),
            pane.comparer_items.get(pane.right_index),
        ) {
            (Some(left), Some(right)) if pane.comparer_items.len() >= 2 => {
                super::transfer_diff::render_simple_diff(&left.bytes, &right.bytes, pane.byte_mode)
            }
            _ => "请选择两个消息进行比较。\nModified: 0  Added: 0  Deleted: 0".into(),
        };
        self.extra_tools
            .comparer_view
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }
}

fn normalize_ws_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url.to_owned()
    }
}

fn ws_origin_to_http_prefix(url: &str) -> Option<String> {
    let (scheme, tail) = url.split_once("://")?;
    let http_scheme = match scheme {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        _ => return None,
    };
    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let authority = &tail[..authority_end];
    if authority.is_empty() {
        return None;
    }
    Some(format!("{http_scheme}://{authority}"))
}
