use super::*;

impl Workbench {
    pub(in crate::workbench) fn is_message_soft_wrap(&self, slot: EditorSlot) -> bool {
        self.message_editor_ref(slot)
            .map_or_else(theme::editor_soft_wrap, |editor| editor.soft_wrap)
    }

    pub(in crate::workbench) fn toggle_message_soft_wrap(
        &mut self,
        slot: EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(slot) {
            editor.toggle_soft_wrap(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn is_message_replace_readonly(&self, slot: EditorSlot) -> bool {
        match slot {
            EditorSlot::RepeaterRequest => {
                self.project.is_none() || self.transition == Some(TransitionPhase::Switching)
            }
            EditorSlot::Intercept => {
                !self.backend_available()
                    || self.transition.is_some()
                    || self
                        .selected_intercept
                        .and_then(|sel| self.intercepts.iter().find(|p| p.id() == sel))
                        .is_none_or(|p| p.deciding)
            }
            EditorSlot::WsRepeaterDraft => {
                self.project.is_none() || self.transition.is_some() || !self.backend_available()
            }
            _ => true,
        }
    }

    pub(in crate::workbench) fn message_editor_mut(
        &mut self,
        slot: EditorSlot,
    ) -> Option<&mut MessageEditor> {
        match slot {
            EditorSlot::HistoryRequest => Some(&mut self.history_request),
            EditorSlot::HistoryResponse => Some(&mut self.history_response),
            EditorSlot::LoggerRequest => Some(&mut self.extra_tools.logger_request),
            EditorSlot::LoggerResponse => Some(&mut self.extra_tools.logger_response),
            EditorSlot::TargetRequest => Some(&mut self.target_pane.request),
            EditorSlot::TargetResponse => Some(&mut self.target_pane.response),
            EditorSlot::OrganiserRequest => Some(&mut self.organiser.request),
            EditorSlot::OrganiserResponse => Some(&mut self.organiser.response),
            EditorSlot::DashboardIssueRequest => Some(&mut self.audit_dashboard.issue_request_editor),
            EditorSlot::DashboardIssueResponse => {
                Some(&mut self.audit_dashboard.issue_response_editor)
            }
            EditorSlot::WebsocketPayload => Some(&mut self.websocket.payload),
            EditorSlot::ExtMessage => self.ext_import.message_editor(),
            EditorSlot::Intercept => self
                .intercepts
                .iter_mut()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .map(|pending| &mut pending.editor),
            EditorSlot::RepeaterRequest => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get_mut(&id))
                .map(|editors| &mut editors.request),
            EditorSlot::RepeaterResponse => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get_mut(&id))
                .map(|editors| &mut editors.response),
            EditorSlot::WsRepeaterDraft => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get_mut(&id))
                .map(|editors| &mut editors.draft),
            EditorSlot::WsRepeaterHistory => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get_mut(&id))
                .map(|editors| &mut editors.history),
        }
    }

    pub(in crate::workbench) fn switch_message_view(
        &mut self,
        slot: EditorSlot,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(slot)
            && let Err(error) = editor.switch(view, window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_pretty_view(
        &mut self,
        slot: EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(slot)
            && let Err(error) = editor.switch_pretty(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_render_view(
        &mut self,
        slot: EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(slot)
            && let Err(error) = editor.switch_render(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_message_non_printable(
        &mut self,
        slot: EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(slot)
            && let Err(error) = editor.toggle_non_printable(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_message_hide_headers(
        &mut self,
        slot: EditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(slot)
            && let Err(error) = editor.toggle_hide_headers(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn handle_message_search_action(
        &mut self,
        slot: EditorSlot,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let readonly = self.is_message_replace_readonly(slot);
        if let Some(editor) = self.message_editor_mut(slot)
            && let Err(error) =
                editor.handle_search_bar_action_editable(action, readonly, window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn copy_converted_selection_action(
        &mut self,
        action: &editing::CopyConvertedSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            self.fail("正在切换或保存项目；未转换选区。", cx);
            return;
        }
        let pick_pair = |req: &MessageEditor, resp: &MessageEditor| {
            let req_state = req.state().clone();
            let resp_state = resp.state().clone();
            if resp.has_focus(window, cx)
                || (!req.has_focus(window, cx)
                    && !resp_state.read(cx).selected_range().is_empty()
                    && req_state.read(cx).selected_range().is_empty())
            {
                resp_state
            } else {
                req_state
            }
        };
        let target_state = match self.tool {
            Tool::Repeater => {
                if self.workspace.active.is_none()
                    && let Some(ws_id) = self.workspace.active_websocket_tab
                {
                    self.websocket_repeater
                        .editors
                        .get(&ws_id)
                        .map(|editors| pick_pair(&editors.draft, &editors.history))
                } else {
                    self.workspace
                        .active
                        .and_then(|id| self.tab_editors.get(&id))
                        .map(|editors| pick_pair(&editors.request, &editors.response))
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .map(|pending| pending.editor.state().clone()),
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                Some(pick_pair(&self.history_request, &self.history_response))
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                Some(self.websocket.payload.state().clone())
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return;
                };
                let Some(editors) = self.intruder.editors.get(&id) else {
                    return;
                };
                let state = editors.request.state().clone();
                let result = if state.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                {
                    Err(editing::COMPOSITION_PENDING.into())
                } else {
                    let selected = state.read(cx).selected_value();
                    if selected.is_empty() {
                        Err("请先在消息编辑器中选中要转换的文本。".into())
                    } else {
                        editing::convert_selection(action.0, selected.as_ref())
                    }
                };
                self.finish_copy_converted_selection(result, cx);
                return;
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => Some(
                pick_pair(&self.target_pane.request, &self.target_pane.response),
            ),
            Tool::Logger => Some(pick_pair(
                &self.extra_tools.logger_request,
                &self.extra_tools.logger_response,
            )),
            Tool::Organiser => Some(pick_pair(&self.organiser.request, &self.organiser.response)),
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .scan_request_editor
                        .has_focus(window, cx)
                {
                    Some(self.audit_dashboard.scan_request_editor.state().clone())
                } else {
                    Some(pick_pair(
                        &self.audit_dashboard.issue_request_editor,
                        &self.audit_dashboard.issue_response_editor,
                    ))
                }
            }
            Tool::Sequencer => Some(self.sequencer_pane.request.state().clone()),
            Tool::Collaborator => Some(self.collaborator_pane.raw_editor.state().clone()),
            _ => None,
        };
        let Some(state) = target_state else {
            return;
        };
        let result = editing::convert_selected_editor_text(&state, action.0, window, cx);
        self.finish_copy_converted_selection(result, cx);
    }

    fn finish_copy_converted_selection(
        &mut self,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(converted) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(converted));
                self.error = None;
                self.status = "已转换选区并复制到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn copy_links_in_response(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            self.copy_selected_history_links(window, cx);
            return;
        }
        if self.tool == Tool::Target && self.target_pane.page == target::TargetPage::SiteMap {
            self.copy_selected_target_links(window, cx);
            return;
        }
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let raw_response = match self.tool_message_bytes(true, window, cx) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.fail("当前没有可提取链接的 HTTP 响应报文。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let base_url = match self
            .current_transfer_draft("提取响应链接", window, cx)
            .and_then(|draft| {
                mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                    .map_err(|error| error.to_string())
            }) {
                Ok(url) => url,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
        let links = match crate::workbench::wb_render_history::extract_response_links_list(
            &base_url,
            &raw_response,
        ) {
            Ok(links) => links,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if links.is_empty() {
            self.error = None;
            self.status = "选中响应中未提取到有效的页面 URL 链接。".into();
            cx.notify();
            return;
        }
        let count = links.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(links.join("\n")));
        self.error = None;
        self.status = format!("已提取并复制 {count} 条页面链接到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn export_selected_history_xml(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_active_item_xml(window, cx);
    }

    pub(crate) fn open_current_request_in_configured_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_current_request_in_browser_with_session(false, window, cx);
    }

    pub(crate) fn open_current_request_in_browser_with_session(
        &mut self,
        original_session: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.ext_import.visible
        {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let (url, raw_request) = match self
            .current_transfer_draft("在浏览器中发起请求", window, cx)
            .and_then(|draft| {
                if draft.raw.is_empty() {
                    return Err("当前没有已加载的 HTTP 请求报文。".into());
                }
                mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                    .map(|url| (url, draft.raw))
                    .map_err(|error| error.to_string())
            }) {
            Ok(pair) => pair,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.fail(
                "配置浏览器仅支持打开 http:// 或 https:// 协议的请求 URL。",
                cx,
            );
            return;
        }
        let session = if original_session {
            mio_core::BrowserRequestSession::OriginalSession { raw_request }
        } else {
            mio_core::BrowserRequestSession::CurrentSession
        };
        if self.enqueue(
            Command::OpenConfiguredBrowser {
                project,
                url: Some(url.clone()),
                session,
            },
            cx,
        ) {
            self.error = None;
            self.status = format!("正在配置浏览器中打开请求：{url}");
            cx.notify();
        }
    }

    pub(crate) fn open_current_response_in_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let raw = match self.tool_message_bytes(true, window, cx) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.fail("当前没有可在浏览器中打开的 HTTP 响应报文。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let (body, content_type) = editing::extract_http_response_body_and_content_type(&raw);
        if body.is_empty() {
            self.fail("当前 HTTP 响应正文为空，无法在浏览器中打开。", cx);
            return;
        }
        let content_type = content_type
            .as_deref()
            .unwrap_or("text/html; charset=utf-8");
        self.open_rendered_response_in_browser(&body, content_type, cx);
    }
}
