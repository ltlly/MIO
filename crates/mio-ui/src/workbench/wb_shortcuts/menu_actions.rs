use std::sync::atomic::{AtomicU64, Ordering};

use mio_core::{
    BodyEncodingTarget, change_http_body_encoding_bytes, parse_clipboard_url_as_request,
    set_http_request_method_bytes, toggle_http_request_method_bytes,
};

use super::*;

static NEXT_COLLAB_INSERT_SEQ: AtomicU64 = AtomicU64::new(1);

impl Workbench {
    pub(in crate::workbench) fn select_tool(
        &mut self,
        tool: Tool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shutdown_requested
            || self.transition == Some(TransitionPhase::Switching)
            || self.ext_import.visible
        {
            return;
        }
        self.extensions.active_top_suite_tab = None;
        match tool {
            Tool::Dashboard => self.open_dashboard(cx),
            Tool::Target => {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Target;
                self.refresh_target_site_map(0, window, cx);
                cx.notify();
            }
            Tool::Proxy => {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Proxy;
                self.refresh_websocket_history(window, cx);
                cx.notify();
            }
            Tool::Repeater => {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Repeater;
                self.refresh_archive_response(window, cx);
                self.refresh_repeater_history(window, cx);
                cx.notify();
            }
            Tool::Collaborator => {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Collaborator;
                self.activate_collaborator(cx);
                cx.notify();
            }
            Tool::Organiser => {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Organiser;
                self.sync_organiser_table(window, cx);
                self.reload_selected_organiser_detail(window, cx);
                cx.notify();
            }
            Tool::Intruder
            | Tool::Sequencer
            | Tool::Decoder
            | Tool::Comparer
            | Tool::Logger
            | Tool::Extensions
            | Tool::Discover => {
                self.cancel_plugin_interactions(cx);
                self.tool = tool;
                cx.notify();
            }
        }
    }

    pub(in crate::workbench) fn toggle_tab_orientation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shutdown_requested
            || self.transition.is_some()
            || self.project.is_none()
            || self.ext_import.visible
        {
            return;
        }
        if self.tool == Tool::Intruder {
            let desired = match self.workspace.intruder.orientation {
                Orientation::Horizontal => Orientation::Vertical,
                Orientation::Vertical => Orientation::Horizontal,
            };
            self.set_intruder_orientation(desired, window, cx);
        } else {
            self.toggle_orientation(window, cx);
        }
    }

    fn apply_request_method_change(
        &mut self,
        desired_method: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let transform = |raw: &[u8]| match desired_method {
            Some(method) => set_http_request_method_bytes(raw, method),
            None => toggle_http_request_method_bytes(raw),
        };
        let outcome = match self.tool {
            Tool::Repeater => self.rewrite_repeater_request(window, cx, |raw| {
                let (next_raw, method) = transform(raw)?;
                Ok((None, next_raw, format!("已将请求方法切换为 {method}。")))
            }),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .rewrite_intercept_http_request(window, cx, |raw| {
                    let (next_raw, method) = transform(raw)?;
                    Ok((next_raw, format!("已将请求方法切换为 {method}。")))
                }),
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    self.fail("没有活动的 Intruder 请求。", cx);
                    return;
                };
                self.read_intruder_clean_request_bytes(id, window, cx)
                    .and_then(|raw| transform(&raw))
                    .and_then(|(next_raw, method)| {
                        self.apply_intruder_request_rewrite(id, None, next_raw, window, cx)?;
                        Ok(format!("已将请求方法切换为 {method}。"))
                    })
            }
            _ => Err("当前消息为只读或不支持切换 HTTP 请求方法。".into()),
        };
        match outcome {
            Ok(status) => {
                self.error = None;
                self.status = status;
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn change_request_method(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_request_method_change(None, window, cx);
    }

    pub(in crate::workbench) fn set_request_method_to(
        &mut self,
        desired_method: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_request_method_change(Some(desired_method), window, cx);
    }

    pub(in crate::workbench) fn change_request_body_encoding(
        &mut self,
        target: BodyEncodingTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let outcome = match self.tool {
            Tool::Repeater => self.rewrite_repeater_request(window, cx, |raw| {
                let (next_raw, label) = change_http_body_encoding_bytes(raw, target)?;
                Ok((None, next_raw, format!("已转换请求体编码为 {label}。")))
            }),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .rewrite_intercept_http_request(window, cx, |raw| {
                    let (next_raw, label) = change_http_body_encoding_bytes(raw, target)?;
                    Ok((next_raw, format!("已转换请求体编码为 {label}。")))
                }),
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    self.fail("没有活动的 Intruder 请求。", cx);
                    return;
                };
                self.read_intruder_clean_request_bytes(id, window, cx)
                    .and_then(|raw| change_http_body_encoding_bytes(&raw, target))
                    .and_then(|(next_raw, label)| {
                        self.apply_intruder_request_rewrite(id, None, next_raw, window, cx)?;
                        Ok(format!("已转换请求体编码为 {label}。"))
                    })
            }
            _ => Err("当前消息为只读或不支持转换 HTTP 请求体编码。".into()),
        };
        match outcome {
            Ok(status) => {
                self.error = None;
                self.status = status;
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn paste_url_as_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let clipboard_text = cx.read_from_clipboard().and_then(|item| item.text());
        let parsed = clipboard_text
            .as_deref()
            .ok_or_else(|| "剪贴板中没有可用的 URL 文本。".to_owned())
            .and_then(parse_clipboard_url_as_request);
        let (origin, next_raw) = match parsed {
            Ok(pair) => pair,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let outcome = match self.tool {
            Tool::Repeater => self.rewrite_repeater_request(window, cx, |_| {
                Ok((
                    Some(origin),
                    next_raw,
                    "已将剪贴板 URL 粘贴为 HTTP 请求。".into(),
                ))
            }),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .rewrite_intercept_http_request(window, cx, |_| {
                    Ok((next_raw, "已将剪贴板 URL 粘贴为 HTTP 请求。".into()))
                }),
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    self.fail("没有活动的 Intruder 请求。", cx);
                    return;
                };
                self.apply_intruder_request_rewrite(id, Some(origin), next_raw, window, cx)
                    .map(|()| "已将剪贴板 URL 粘贴为 HTTP 请求。".into())
            }
            _ => Err("当前消息为只读，不能粘贴 URL 为请求。".into()),
        };
        match outcome {
            Ok(status) => {
                self.error = None;
                self.status = status;
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn copy_prettified(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let bytes_result = match self.tool {
            Tool::Repeater if self.workspace.active.is_none() => {
                let Some(ws_id) = self.workspace.active_websocket_tab else {
                    self.fail("没有活动的消息可复制。", cx);
                    return;
                };
                let Some(editors) = self.websocket_repeater.editors.get(&ws_id) else {
                    self.fail("消息编辑器尚未载入。", cx);
                    return;
                };
                if editors.history.has_focus(window, cx) {
                    editors.history.bytes(window, cx)
                } else {
                    editors.draft.bytes(window, cx)
                }
            }
            Tool::Repeater => {
                let response = self
                    .workspace
                    .active
                    .and_then(|id| self.tab_editors.get(&id))
                    .is_some_and(|editors| editors.response.has_focus(window, cx));
                self.tool_message_bytes(response, window, cx)
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                if self.websocket.detail.is_none() {
                    Err("WebSocket 消息正文尚未读取完成。".into())
                } else {
                    self.websocket.payload.bytes(window, cx)
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .ok_or_else(|| "没有选中的拦截消息。".to_owned())
                .and_then(|pending| pending.editor.bytes(window, cx)),
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                let response = self.history_response.has_focus(window, cx);
                self.tool_message_bytes(response, window, cx)
            }
            Tool::Logger => {
                let response = self.extra_tools.logger_response.has_focus(window, cx);
                self.tool_message_bytes(response, window, cx)
            }
            Tool::Target => {
                let response = self.target_pane.response.has_focus(window, cx);
                self.tool_message_bytes(response, window, cx)
            }
            Tool::Organiser => {
                let response = self.organiser.response.has_focus(window, cx);
                self.tool_message_bytes(response, window, cx)
            }
            Tool::Dashboard => {
                let response = !self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .issue_response_editor
                        .has_focus(window, cx);
                self.tool_message_bytes(response, window, cx)
            }
            Tool::Collaborator => self.collaborator_pane.raw_editor.bytes(window, cx),
            Tool::Decoder => {
                let source = if self
                    .extra_tools
                    .decoder_output
                    .focus_handle(cx)
                    .is_focused(window)
                    && !self.extra_tools.decoder_output.read(cx).value().is_empty()
                {
                    &self.extra_tools.decoder_output
                } else {
                    &self.extra_tools.decoder_input
                };
                editing::snapshot(source, window, cx)
                    .map(|text| text.as_bytes().to_vec())
                    .map_err(str::to_owned)
            }
            _ => self.tool_message_bytes(false, window, cx),
        };
        let bytes = match bytes_result {
            Ok(bytes) if bytes.is_empty() => {
                self.fail("消息正文尚未就绪或为空。", cx);
                return;
            }
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let Ok(raw_utf8) = std::str::from_utf8(&bytes) else {
            self.fail(
                "消息包含非 UTF-8 二进制字节，无法复制美化报文；请切换到 Hex 视图复制。",
                cx,
            );
            return;
        };
        let formatted =
            mio_core::format_pretty_message(&bytes).unwrap_or_else(|_| raw_utf8.to_owned());
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(formatted));
        self.error = None;
        self.status = "已复制美化报文到剪贴板。".into();
        cx.notify();
    }

    fn rewrite_repeater_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        transform: impl FnOnce(&[u8]) -> Result<(Option<String>, Vec<u8>, String), String>,
    ) -> Result<String, String> {
        let Some(tab_id) = self.workspace.active else {
            return Err(if self.workspace.active_websocket_tab.is_some() {
                "WebSocket 标签不支持此 HTTP 请求操作。".into()
            } else {
                "没有活动的 Repeater 请求。".into()
            });
        };
        if !self.can_send(tab_id) || self.extensions.active_messages.contains_key(&tab_id) {
            return Err("当前 Repeater 请求不可编辑。".into());
        }
        if self.rename.as_ref().is_some_and(|rename| {
            rename
                .input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        }) {
            return Err(editing::COMPOSITION_PENDING.into());
        }
        let current_raw = {
            let editors = self
                .tab_editors
                .get(&tab_id)
                .ok_or_else(|| "请求编辑器不可用。".to_owned())?;
            if editors.response.has_focus(window, cx) {
                return Err("响应消息为只读，不能原地修改请求。".into());
            }
            let _ = editing::input_snapshot(&editors.target, window, cx).map_err(str::to_owned)?;
            editors.request.bytes(window, cx)?
        };
        let (next_origin, next_raw, status) = transform(&current_raw)?;
        let next_revision = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.draft.revision + 1)
            .ok_or_else(|| "当前标签页不存在。".to_owned())?;
        if let Some(active_tab) = self.workspace.tabs.iter_mut().find(|t| t.id == tab_id) {
            if let Some(origin) = &next_origin {
                active_tab.draft.target = origin.clone();
                active_tab.draft.version = HttpVersion::Http11;
            }
            active_tab.draft.raw = next_raw.clone();
            active_tab.draft.revision = next_revision;
        }
        if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
            if let Some(origin) = next_origin {
                editors
                    .target
                    .update(cx, |state, cx| state.set_value(origin, window, cx));
            }
            editors.request.load(&next_raw, window, cx);
            let _ = editors.request.switch(MessageView::Raw, window, cx);
            editors.loaded_revision = next_revision;
            editors.dirty = false;
        }
        let _ = self.save_workspace(cx);
        Ok(status)
    }

    fn rewrite_intercept_http_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        transform: impl FnOnce(&[u8]) -> Result<(Vec<u8>, String), String>,
    ) -> Result<String, String> {
        let sel = self.selected_intercept;
        let pending = self
            .intercepts
            .iter_mut()
            .find(|pending| Some(pending.id()) == sel)
            .ok_or_else(|| "没有选中的拦截请求。".to_owned())?;
        if pending.deciding {
            return Err("当前拦截消息为只读，请切换到可编辑的 Raw 视图。".into());
        }
        if !matches!(
            pending.target,
            InterceptTarget::Http {
                stage: InterceptStage::Request,
                ..
            }
        ) {
            return Err("仅支持在 HTTP 请求拦截阶段执行此操作。".into());
        }
        let raw = pending.editor.bytes(window, cx)?;
        let (next_raw, status) = transform(&raw)?;
        pending.editor.load(&next_raw, window, cx);
        let _ = pending.editor.switch(MessageView::Raw, window, cx);
        Ok(status)
    }

    pub(in crate::workbench) fn focus_message_editor_by_id(
        &self,
        slot: super::wb_editors::EditorSlot,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(editor) = self.message_editor_ref(slot) {
            let state = editor.state();
            state.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    pub(crate) fn open_scan_launcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let draft = match self.current_transfer_draft("配置扫描任务", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.cancel_plugin_interactions(cx);
        self.send_draft_to_scanner(
            draft,
            mio_core::AuditTaskMode::PassiveAndActive,
            false,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn copy_selected_editor_text(
        &mut self,
        slot: super::wb_editors::EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        self.focus_message_editor_by_id(slot, window, cx);
        match self.focused_tool_selection(window, cx) {
            Ok(Some(selected)) if !selected.is_empty() => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(selected));
                self.error = None;
                self.status = "已复制选中文本到剪贴板。".into();
                cx.notify();
            }
            Ok(_) => self.fail("请先在消息编辑器中选中要复制的文本。", cx),
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn save_selected_text_to_file(
        &mut self,
        slot: super::wb_editors::EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        self.focus_message_editor_by_id(slot, window, cx);
        self.save_focused_selected_text_to_file(window, cx);
    }

    pub(in crate::workbench) fn save_focused_selected_text_to_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let bytes = match self.focused_tool_selection(window, cx) {
            Ok(Some(selected)) if !selected.is_empty() => selected.into_bytes(),
            Ok(_) => {
                self.fail("请先在消息编辑器中选中要保存的文本。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.spawn_save_path_picker(
            Some("selection.txt"),
            false,
            "项目已切换，本次文件保存操作已取消。",
            "无法选择保存路径",
            window,
            cx,
            move |view, dest, _, cx| match mio_core::ui_files::write_export_file(&dest, &bytes) {
                Ok(()) => {
                    view.error = None;
                    view.status = format!(
                        "已将选中文本（{} 字节）保存到文件：{}",
                        bytes.len(),
                        dest.display()
                    );
                    cx.notify();
                }
                Err(error) => {
                    view.fail(format!("写入文件失败：{error}"), cx);
                }
            },
        );
    }

    pub(in crate::workbench) fn copy_to_file(
        &mut self,
        response_body_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let bytes_result = if response_body_only {
            self.tool_message_bytes(true, window, cx).map(|raw| {
                let payload = editing::extract_http_response_body(&raw);
                (payload, "response-body.bin")
            })
        } else {
            self.extract_copy_to_file_bytes(window, cx)
        };
        let (bytes, suggested_name) = match bytes_result {
            Ok((bytes, _)) if bytes.is_empty() => {
                self.fail("当前没有可保存的报文字节。", cx);
                return;
            }
            Ok(pair) => pair,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.spawn_save_path_picker(
            Some(suggested_name),
            false,
            "项目已切换，本次文件保存操作已取消。",
            "无法选择保存路径",
            window,
            cx,
            move |view, dest, _, cx| match mio_core::ui_files::write_export_file(&dest, &bytes) {
                Ok(()) => {
                    view.error = None;
                    view.status = format!(
                        "已将 {} 字节保存到文件：{}",
                        bytes.len(),
                        dest.display()
                    );
                    cx.notify();
                }
                Err(error) => {
                    view.fail(format!("写入文件失败：{error}"), cx);
                }
            },
        );
    }

    fn extract_copy_to_file_bytes(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(Vec<u8>, &'static str), String> {
        if let Some(selected) = self.focused_tool_selection(window, cx)?
            && !selected.is_empty()
        {
            return Ok((selected.into_bytes(), "selection.bin"));
        }
        match self.tool {
            Tool::Repeater if self.workspace.active.is_none() => {
                let ws_id = self
                    .workspace
                    .active_websocket_tab
                    .ok_or("没有活动的消息可保存。")?;
                let editors = self
                    .websocket_repeater
                    .editors
                    .get(&ws_id)
                    .ok_or("消息编辑器尚未载入。")?;
                if editors.history.has_focus(window, cx) {
                    Ok((editors.history.bytes(window, cx)?, "websocket-history.bin"))
                } else {
                    Ok((editors.draft.bytes(window, cx)?, "websocket-draft.bin"))
                }
            }
            Tool::Repeater => {
                let is_resp = self
                    .workspace
                    .active
                    .and_then(|id| self.tab_editors.get(&id))
                    .is_some_and(|ed| ed.response.has_focus(window, cx));
                let bytes = self.tool_message_bytes(is_resp, window, cx)?;
                Ok((
                    bytes,
                    if is_resp {
                        "response.http"
                    } else {
                        "request.http"
                    },
                ))
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                let is_resp = self.history_response.has_focus(window, cx);
                let bytes = self.tool_message_bytes(is_resp, window, cx)?;
                Ok((
                    bytes,
                    if is_resp {
                        "response.http"
                    } else {
                        "request.http"
                    },
                ))
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                if self.websocket.detail.is_none() {
                    return Err("WebSocket 消息正文尚未读取完成。".into());
                }
                Ok((self.websocket.payload.bytes(window, cx)?, "websocket.bin"))
            }
            Tool::Logger => {
                let is_resp = self.extra_tools.logger_response.has_focus(window, cx);
                let bytes = self.tool_message_bytes(is_resp, window, cx)?;
                Ok((
                    bytes,
                    if is_resp {
                        "response.http"
                    } else {
                        "request.http"
                    },
                ))
            }
            Tool::Target => {
                let is_resp = self.target_pane.response.has_focus(window, cx);
                let bytes = self.tool_message_bytes(is_resp, window, cx)?;
                Ok((
                    bytes,
                    if is_resp {
                        "response.http"
                    } else {
                        "request.http"
                    },
                ))
            }
            Tool::Organiser => {
                let is_resp = self.organiser.response.has_focus(window, cx);
                let bytes = self.tool_message_bytes(is_resp, window, cx)?;
                Ok((
                    bytes,
                    if is_resp {
                        "response.http"
                    } else {
                        "request.http"
                    },
                ))
            }
            Tool::Dashboard => {
                let is_resp = !self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .issue_response_editor
                        .has_focus(window, cx);
                let bytes = self.tool_message_bytes(is_resp, window, cx)?;
                Ok((
                    bytes,
                    if is_resp {
                        "response.http"
                    } else {
                        "request.http"
                    },
                ))
            }
            Tool::Collaborator => Ok((
                self.collaborator_pane.raw_editor.bytes(window, cx)?,
                "collaborator-request.http",
            )),
            Tool::Decoder => {
                let out_focused = self
                    .extra_tools
                    .decoder_output
                    .focus_handle(cx)
                    .is_focused(window);
                if out_focused
                    && let Some(raw) = &self.extra_tools.decoder_output_bytes
                {
                    return Ok((raw.clone(), "decoder-output.bin"));
                }
                let src = if out_focused
                    && !self.extra_tools.decoder_output.read(cx).value().is_empty()
                {
                    &self.extra_tools.decoder_output
                } else {
                    &self.extra_tools.decoder_input
                };
                let text = editing::snapshot(src, window, cx).map_err(str::to_owned)?;
                Ok((text.as_bytes().to_vec(), "decoder.bin"))
            }
            _ => {
                let bytes = self.tool_message_bytes(false, window, cx)?;
                Ok((bytes, "request.http"))
            }
        }
    }

    pub(in crate::workbench) fn paste_from_file(
        &mut self,
        base64_encode: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        self.spawn_open_path_picker(
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(
                    if base64_encode {
                        "选择文件并 Base64 编码插入"
                    } else {
                        "从文件粘贴字节"
                    }
                    .into(),
                ),
            },
            "项目已切换，本次从文件粘贴操作已取消。",
            "无法选择文件",
            window,
            cx,
            move |view, path, window, cx| {
                const MAX_PASTE_FILE_BYTES: u64 = 8 * 1024 * 1024;
                let file_bytes = match mio_core::ui_files::read_regular_file_bounded(
                    &path,
                    MAX_PASTE_FILE_BYTES,
                    true,
                    "插入文件",
                ) {
                    Ok(b) => b,
                    Err(err) => {
                        view.fail(err, cx);
                        return;
                    }
                };
                let insert_bytes = if base64_encode {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD
                        .encode(&file_bytes)
                        .into_bytes()
                } else {
                    file_bytes
                };
                match view.insert_bytes_into_editable_target(&insert_bytes, window, cx) {
                    Ok(()) => {
                        view.error = None;
                        view.status = format!(
                            "已从文件插入 {} 字节：{}",
                            insert_bytes.len(),
                            path.display()
                        );
                        cx.notify();
                    }
                    Err(err) => view.fail(err, cx),
                }
            },
        );
    }

    pub(in crate::workbench) fn insert_collaborator_payload(
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
        if let Err(err) = self.validate_editable_insert_target(window, cx) {
            self.fail(err, cx);
            return;
        }
        let seq = NEXT_COLLAB_INSERT_SEQ.fetch_add(1, Ordering::Relaxed);
        let label = format!("编辑器右键插入 #{seq}");
        let before_count = self.collaborator_pane.status.issued_tokens.len();
        let before_last_token = self
            .collaborator_pane
            .status
            .issued_tokens
            .last()
            .map(|t| t.token.clone());
        let before_error = self.collaborator_pane.error.clone();
        if !self.enqueue(
            Command::Collaborator(mio_core::CollaboratorCommand::GeneratePayloads {
                project,
                count: 1,
                label: label.clone(),
            }),
            cx,
        ) {
            return;
        }
        self.collaborator_pane.onboarding_dismissed = true;
        self.collaborator_pane.error = None;
        self.error = None;
        self.status = "正在生成并注册 Collaborator 回调载荷…".into();

        let owner_window = self.owner_window;
        let self_entity = cx.entity();
        let observer = cx.observe(&self_entity, move |view, entity, cx| {
            if view.project != Some(project) || !view.backend_available() {
                if view
                    .collaborator_insert_sub
                    .as_ref()
                    .is_some_and(|(id, _)| *id == seq)
                {
                    view.collaborator_insert_sub = None;
                }
                return;
            }
            let fresh = view
                .collaborator_pane
                .status
                .issued_tokens
                .iter()
                .rev()
                .find(|item| item.label == label)
                .or_else(|| {
                    let tokens = &view.collaborator_pane.status.issued_tokens;
                    (tokens.len() > before_count)
                        .then(|| tokens.last())
                        .flatten()
                        .filter(|item| Some(item.token.as_str()) != before_last_token.as_deref())
                })
                .map(|item| item.http_callback_url.clone());
            if let Some(payload) = fresh {
                if view
                    .collaborator_insert_sub
                    .as_ref()
                    .is_some_and(|(id, _)| *id == seq)
                {
                    view.collaborator_insert_sub = None;
                }
                let weak = entity.downgrade();
                cx.defer(move |cx| {
                    let _ = owner_window.update(cx, |_, window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            if view.project != Some(project) || !view.backend_available() {
                                return;
                            }
                            match view.insert_bytes_into_editable_target(
                                payload.as_bytes(),
                                window,
                                cx,
                            ) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!("已插入 Collaborator 回调载荷：{payload}");
                                    cx.notify();
                                }
                                Err(err) => view.fail(err, cx),
                            }
                        });
                    });
                });
                return;
            }
            if view.collaborator_pane.error.is_some()
                && view.collaborator_pane.error != before_error
                && view
                    .collaborator_insert_sub
                    .as_ref()
                    .is_some_and(|(id, _)| *id == seq)
            {
                view.collaborator_insert_sub = None;
            }
        });
        self.collaborator_insert_sub = Some((seq, observer));
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(15));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, _| {
                if view
                    .collaborator_insert_sub
                    .as_ref()
                    .is_some_and(|(id, _)| *id == seq)
                {
                    view.collaborator_insert_sub = None;
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn validate_editable_insert_target(
        &self,
        window: &mut Window,
        cx: &App,
    ) -> Result<(), String> {
        match self.tool {
            Tool::Repeater if self.workspace.active.is_some() => {
                let tab_id = self.workspace.active.unwrap();
                if !self.can_send(tab_id) || self.extensions.active_messages.contains_key(&tab_id) {
                    return Err("当前 Repeater 请求不可编辑。".into());
                }
                let editors = self
                    .tab_editors
                    .get(&tab_id)
                    .ok_or("请求编辑器不可用。")?;
                if editors.response.has_focus(window, cx) {
                    return Err("响应消息为只读，不能插入内容。".into());
                }
                Ok(())
            }
            Tool::Repeater => {
                let ws_id = self
                    .workspace
                    .active_websocket_tab
                    .ok_or("没有活动的编辑器可插入内容。")?;
                let editors = self
                    .websocket_repeater
                    .editors
                    .get(&ws_id)
                    .ok_or("WebSocket 编辑器尚未载入。")?;
                if editors.history.has_focus(window, cx) {
                    return Err("历史消息为只读，不能插入内容。".into());
                }
                Ok(())
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let sel = self.selected_intercept;
                let pending = self
                    .intercepts
                    .iter()
                    .find(|p| Some(p.id()) == sel)
                    .ok_or("没有选中的拦截消息。")?;
                if pending.deciding {
                    return Err("当前拦截消息正在提交，不能插入内容。".into());
                }
                Ok(())
            }
            Tool::Intruder => {
                let id = self
                    .workspace
                    .intruder
                    .active
                    .ok_or("没有活动的 Intruder 请求。")?;
                let editors = self
                    .intruder
                    .editors
                    .get(&id)
                    .ok_or("Intruder 编辑器尚未载入。")?;
                if editors.request.view != MessageView::Raw {
                    return Err("请切换到 Intruder Raw 视图后再插入内容。".into());
                }
                Ok(())
            }
            Tool::Decoder => Ok(()),
            Tool::Sequencer if self.sequencer_pane.page == sequencer::SequencerPage::Live => {
                if self.sequencer_pane.running.is_some() {
                    return Err("Sequencer 正在运行采样，不能修改请求。".into());
                }
                Ok(())
            }
            Tool::Dashboard if self.audit_dashboard.new_scan_open => {
                if !self.audit_can_operate() {
                    return Err("当前状态不可编辑扫描请求。".into());
                }
                Ok(())
            }
            _ => Err("当前编辑器为只读，不能插入内容。".into()),
        }
    }

    pub(in crate::workbench) fn url_encode_as_you_type_enabled(&self) -> bool {
        if self.user_preferences.busy() {
            self.user_preferences.message_editor.url_encode_as_you_type
        } else if let Some(confirmed) = &self.user_preferences.confirmed {
            confirmed.message_editor.url_encode_as_you_type
        } else {
            self.user_preferences.message_editor.url_encode_as_you_type
        }
    }

    pub(in crate::workbench) fn toggle_url_encode_as_you_type(&mut self, cx: &mut Context<Self>) {
        let next = !self.url_encode_as_you_type_enabled();
        self.user_preferences.message_editor.url_encode_as_you_type = next;
        editing::set_url_encode_as_you_type(next, cx);
        if let Some(confirmed) = self.user_preferences.confirmed.clone() {
            let mut message_editor = confirmed.message_editor.clone();
            message_editor.url_encode_as_you_type = next;
            let _ = self.set_user_workbench_preferences(
                confirmed.display,
                message_editor,
                confirmed.misc,
                cx,
            );
        }
        self.error = None;
        self.status = if next {
            "已开启「输入时 URL 编码」(URL-encode as you type)。".into()
        } else {
            "已关闭「输入时 URL 编码」(URL-encode as you type)。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn send_to_comparer_smart(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let response_focused = match self.tool {
            Tool::Repeater => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .is_some_and(|ed| ed.response.has_focus(window, cx)),
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                self.history_response.has_focus(window, cx)
            }
            Tool::Target => self.target_pane.response.has_focus(window, cx),
            Tool::Logger => self.extra_tools.logger_response.has_focus(window, cx),
            Tool::Organiser => self.organiser.response.has_focus(window, cx),
            Tool::Dashboard => {
                !self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .issue_response_editor
                        .has_focus(window, cx)
            }
            _ => false,
        };
        self.send_to_comparer(response_focused, window, cx);
    }

    pub(crate) fn engagement_find_references(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let url = self
            .current_transfer_draft("查找引用", window, cx)
            .ok()
            .and_then(|draft| {
                mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                    .ok()
                    .or_else(|| (!draft.target.is_empty()).then_some(draft.target))
            })
            .or_else(|| {
                self.resolve_active_issue_context(cx)
                    .map(|(url, _, _)| url)
            });
        let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
            self.fail("当前上下文没有可用于查找引用的 URL 或主机。", cx);
            return;
        };
        let search_term = url::Url::parse(&url)
            .ok()
            .map(|parsed| {
                let host = parsed.host_str().unwrap_or_default();
                let path = parsed.path();
                if !path.is_empty() && path != "/" {
                    path.to_owned()
                } else {
                    host.to_owned()
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or(url);
        self.global_search
            .input
            .update(cx, |input, cx| input.set_value(search_term.clone(), window, cx));
        self.open_global_search(window, cx);
        self.error = None;
        self.status = format!("已在全局搜索中查找引用：{search_term}");
        cx.notify();
    }

    pub(crate) fn engagement_discover_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let mut draft = match self.current_transfer_draft("发现内容", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let target = mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
            .unwrap_or_else(|_| draft.target.clone());
        draft.target = target.clone();
        draft.revision = draft.revision.max(1);
        self.cancel_plugin_interactions(cx);
        let prev_crawl = self.audit_dashboard.crawl_same_origin;
        self.audit_dashboard.crawl_same_origin = true;
        self.tool = Tool::Dashboard;
        self.activate_pro_dashboard(cx);
        let title = format!("发现内容 · {target}");
        let dispatched = self.dispatch_audit_scan(
            title,
            draft,
            mio_core::AuditTaskMode::PassiveOnly,
            cx,
        );
        self.audit_dashboard.crawl_same_origin = prev_crawl;
        if dispatched {
            self.error = None;
            self.status = format!("已启动同源内容发现扫描：{target}");
        }
        cx.notify();
    }

    pub(crate) fn engagement_schedule_task(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_filtered(
            Some(crate::workbench::settings::SettingsPage::ProjectTasks),
            None,
            false,
            window,
            cx,
        );
    }

    pub(crate) fn engagement_generate_csrf_poc(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let draft = match self.current_transfer_draft("生成 CSRF PoC", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let url = mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
            .unwrap_or_else(|_| draft.target.clone());
        self.open_csrf_poc_modal(&url, &draft.raw, cx);
    }

    pub(in crate::workbench) fn insert_bytes_into_editable_target(
        &mut self,
        bytes: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        match self.tool {
            Tool::Repeater if self.workspace.active.is_some() => {
                let tab_id = self.workspace.active.unwrap();
                if !self.can_send(tab_id) || self.extensions.active_messages.contains_key(&tab_id) {
                    return Err("当前 Repeater 请求不可编辑。".into());
                }
                let editors = self
                    .tab_editors
                    .get_mut(&tab_id)
                    .ok_or("请求编辑器不可用。")?;
                if editors.response.has_focus(window, cx) {
                    return Err("响应消息为只读，不能插入内容。".into());
                }
                insert_bytes_into_message_editor(&mut editors.request, bytes, window, cx)?;
                editors.dirty = true;
                let _ = self.flush_workspace(window, cx);
                Ok(())
            }
            Tool::Repeater => {
                let ws_id = self
                    .workspace
                    .active_websocket_tab
                    .ok_or("没有活动的编辑器可插入内容。")?;
                let editors = self
                    .websocket_repeater
                    .editors
                    .get_mut(&ws_id)
                    .ok_or("WebSocket 编辑器尚未载入。")?;
                if editors.history.has_focus(window, cx) {
                    return Err("历史消息为只读，不能插入内容。".into());
                }
                insert_bytes_into_message_editor(&mut editors.draft, bytes, window, cx)?;
                editors.dirty = true;
                Ok(())
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let sel = self.selected_intercept;
                let pending = self
                    .intercepts
                    .iter_mut()
                    .find(|p| Some(p.id()) == sel)
                    .ok_or("没有选中的拦截消息。")?;
                if pending.deciding {
                    return Err("当前拦截消息正在提交，不能插入内容。".into());
                }
                insert_bytes_into_message_editor(&mut pending.editor, bytes, window, cx)
            }
            Tool::Intruder => {
                let id = self
                    .workspace
                    .intruder
                    .active
                    .ok_or("没有活动的 Intruder 请求。")?;
                let editors = self
                    .intruder
                    .editors
                    .get_mut(&id)
                    .ok_or("Intruder 编辑器尚未载入。")?;
                if editors.request.view != MessageView::Raw {
                    return Err("请切换到 Intruder Raw 视图后再插入内容。".into());
                }
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    "插入文件包含非 UTF-8 二进制字节；请使用 Base64 编码插入。".to_owned()
                })?;
                if editors
                    .request
                    .raw
                    .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                {
                    return Err(editing::COMPOSITION_PENDING.into());
                }
                editors
                    .request
                    .raw
                    .update(cx, |s, cx| s.replace(text.to_owned(), window, cx));
                self.intruder_input_changed(id, window, cx);
                let _ = self.flush_workspace(window, cx);
                Ok(())
            }
            Tool::Decoder => {
                if self
                    .extra_tools
                    .decoder_input
                    .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                {
                    return Err(editing::COMPOSITION_PENDING.into());
                }
                let text = String::from_utf8(bytes.to_vec())
                    .unwrap_or_else(|_| editing::hex_encode(bytes));
                self.extra_tools
                    .decoder_input
                    .update(cx, |s, cx| s.replace(text, window, cx));
                Ok(())
            }
            Tool::Sequencer if self.sequencer_pane.page == sequencer::SequencerPage::Live => {
                if self.sequencer_pane.running.is_some() {
                    return Err("Sequencer 正在运行采样，不能修改请求。".into());
                }
                insert_bytes_into_message_editor(
                    &mut self.sequencer_pane.request,
                    bytes,
                    window,
                    cx,
                )
            }
            Tool::Dashboard if self.audit_dashboard.new_scan_open => {
                if !self.audit_can_operate() {
                    return Err("当前状态不可编辑扫描请求。".into());
                }
                insert_bytes_into_message_editor(
                    &mut self.audit_dashboard.scan_request_editor,
                    bytes,
                    window,
                    cx,
                )
            }
            _ => Err("当前编辑器为只读，不能插入内容。".into()),
        }
    }
}

fn insert_bytes_into_message_editor(
    editor: &mut MessageEditor,
    bytes: &[u8],
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    if editor.is_readonly_preview() {
        let target_view = if std::str::from_utf8(bytes).is_ok() && editor.view() == MessageView::Raw {
            MessageView::Raw
        } else {
            editor.view()
        };
        editor.switch(target_view, window, cx)?;
    }
    match editor.view() {
        MessageView::Raw => match std::str::from_utf8(bytes) {
            Ok(text) => {
                if editor
                    .raw
                    .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                {
                    return Err(editing::COMPOSITION_PENDING.into());
                }
                editor
                    .raw
                    .update(cx, |s, cx| s.replace(text.to_owned(), window, cx));
                Ok(())
            }
            Err(_) => {
                let current = editor.bytes(window, cx)?;
                let range = editor.raw.read(cx).selected_range();
                let start = range.start.min(current.len());
                let end = range.end.min(current.len());
                let mut next = Vec::with_capacity(current.len() - (end - start) + bytes.len());
                next.extend_from_slice(&current[..start]);
                next.extend_from_slice(bytes);
                next.extend_from_slice(&current[end..]);
                editor.load(&next, window, cx);
                Ok(())
            }
        },
        MessageView::Hex => {
            if editor
                .hex
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            {
                return Err(editing::COMPOSITION_PENDING.into());
            }
            let hex_chunk = editing::hex_encode(bytes);
            let replacement = if hex_chunk.is_empty() {
                String::new()
            } else {
                format!(" {hex_chunk} ")
            };
            editor
                .hex
                .update(cx, |s, cx| s.replace(replacement, window, cx));
            Ok(())
        }
    }
}
