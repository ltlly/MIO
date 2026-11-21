use super::*;

impl Workbench {
    pub(in crate::workbench) fn tool_message_bytes(
        &self,
        response: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Vec<u8>, String> {
        let bytes = match self.tool {
            Tool::Repeater => {
                let id = self.workspace.active.ok_or("没有活动的 Repeater 请求。")?;
                let editors = self.tab_editors.get(&id).ok_or("请求编辑器不可用。")?;
                if response {
                    editors.response.bytes(window, cx)?
                } else {
                    editors.request.bytes(window, cx)?
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                if self.selected_history.is_none() || self.detail_error.is_some() {
                    return Err("历史记录正文尚未读取完成。".into());
                }
                if let Some(HistoryEntryId::Live(flow_id)) = self.selected_history
                    && !self
                        .detail
                        .as_ref()
                        .is_some_and(|detail| detail.summary.id == flow_id)
                {
                    return Err("历史记录正文尚未读取完成。".into());
                }
                if matches!(self.selected_history, Some(HistoryEntryId::External { .. }))
                    && !self.imported_history_request_ready()
                {
                    return Err("归档消息正文尚未读取完成。".into());
                }
                if response
                    && matches!(self.selected_history, Some(HistoryEntryId::External { .. }))
                    && !self.imported_history_response_ready()
                {
                    return Err("归档响应正文尚未读取完成。".into());
                }
                if response {
                    self.history_response.bytes(window, cx)?
                } else {
                    self.history_request.bytes(window, cx)?
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let pending = self
                    .intercepts
                    .iter()
                    .find(|pending| Some(pending.id()) == self.selected_intercept)
                    .ok_or("没有选中的拦截消息。")?;
                match &pending.target {
                    InterceptTarget::Http { stage, record, .. } => {
                        if response {
                            if *stage == InterceptStage::Response {
                                pending.editor.bytes(window, cx)?
                            } else {
                                record
                                    .response
                                    .as_ref()
                                    .map(response_to_raw)
                                    .ok_or("当前请求没有响应。")?
                            }
                        } else if *stage == InterceptStage::Request {
                            pending.editor.bytes(window, cx)?
                        } else {
                            request_to_raw(&record.request)
                        }
                    }
                    InterceptTarget::WebSocket { .. } => {
                        if response {
                            return Err("WebSocket 消息不区分 HTTP 请求与响应。".into());
                        }
                        pending.editor.bytes(window, cx)?
                    }
                }
            }
            Tool::Intruder if !response => {
                let id = self
                    .workspace
                    .intruder
                    .active
                    .ok_or("没有活动的 Intruder 请求。")?;
                self.snapshot_intruder_spec(id, window, cx)?.draft.raw
            }
            Tool::Logger => {
                if self.extra_tools.logger_selected.is_none() {
                    return Err("没有选中的 Logger 流量记录。".into());
                }
                if response {
                    self.extra_tools.logger_response.bytes(window, cx)?
                } else {
                    self.extra_tools.logger_request.bytes(window, cx)?
                }
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => {
                if self.target_pane.selected_id.is_none() && self.target_pane.selected_row.is_none()
                {
                    return Err("没有选中的站点地图记录。".into());
                }
                if response {
                    self.target_pane.response.bytes(window, cx)?
                } else {
                    self.target_pane.request.bytes(window, cx)?
                }
            }
            Tool::Sequencer if !response => {
                if self.sequencer_pane.page != sequencer::SequencerPage::Live {
                    return Err("请切换到 Sequencer 实时采样页转送请求报文。".into());
                }
                self.sequencer_pane.request.bytes(window, cx)?
            }
            Tool::Dashboard
                if self.audit_dashboard.new_scan_open
                    && !response
                    && (self
                        .audit_dashboard
                        .scan_request_editor
                        .has_focus(window, cx)
                        || self.has_dashboard_intruder_selection()
                        || self.audit_dashboard.selected_finding_id.is_none()) =>
            {
                self.audit_dashboard.scan_request_editor.bytes(window, cx)?
            }
            Tool::Dashboard
                if !self.has_dashboard_intruder_selection()
                    && self.audit_dashboard.selected_finding_id.is_some() =>
            {
                if let Some(error) = &self.audit_dashboard.flow_error {
                    return Err(format!("读取关联流量失败：{error}"));
                }
                if self.audit_dashboard.pending_flow_read.is_some()
                    && self.audit_dashboard.loaded_flow.is_none()
                {
                    return Err(if response {
                        "关联响应报文尚未读取完成。".into()
                    } else {
                        "关联请求报文尚未读取完成。".into()
                    });
                }
                if response {
                    self.audit_dashboard
                        .issue_response_editor
                        .bytes(window, cx)?
                } else {
                    let issue_bytes = self
                        .audit_dashboard
                        .issue_request_editor
                        .bytes(window, cx)?;
                    if issue_bytes.is_empty() && self.audit_dashboard.new_scan_open {
                        self.audit_dashboard.scan_request_editor.bytes(window, cx)?
                    } else {
                        issue_bytes
                    }
                }
            }
            Tool::Organiser => {
                if self.organiser.selected_id.is_none() {
                    return Err("没有选中的 Organiser 条目。".into());
                }
                if response {
                    self.organiser.response.bytes(window, cx)?
                } else {
                    self.organiser.request.bytes(window, cx)?
                }
            }
            _ => return Err("当前上下文没有可转送的 HTTP 消息。".into()),
        };
        if bytes.is_empty() {
            return Err("消息正文尚未就绪或为空。".into());
        }
        Ok(bytes)
    }

    pub(in crate::workbench) fn focused_tool_selection(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Option<String>, String> {
        if self.tool == Tool::Intruder
            && let Some(id) = self.workspace.intruder.active
            && let Some(editors) = self.intruder.editors.get(&id)
        {
            let request = &editors.request;
            let state = request.state();
            if state.focus_handle(cx).is_focused(window) {
                return state.update(cx, |state, cx| {
                    if state.marked_text_range(window, cx).is_some() {
                        return Err(editing::COMPOSITION_PENDING.to_owned());
                    }
                    let range = state.selected_range();
                    if range.is_empty() {
                        return Ok(None);
                    }
                    let content = state.content();
                    let start = content.text()[..range.start].matches('§').count() * '§'.len_utf8();
                    let end = content.text()[..range.end].matches('§').count() * '§'.len_utf8();
                    let selected = content.text()[range.clone()].replace('§', "");
                    match request.view {
                        MessageView::Raw => Ok(Some(selected)),
                        MessageView::Hex => {
                            let digits = |offset: usize, markers: usize| {
                                content.text()[..offset - markers]
                                    .bytes()
                                    .filter(|byte| !byte.is_ascii_whitespace())
                                    .count()
                            };
                            if (digits(range.start, start) % 2 != 0)
                                || (digits(range.end, end) % 2 != 0)
                            {
                                return Err("Hex 选区必须位于完整字节边界。".into());
                            }
                            let bytes = editing::hex_decode(&selected)?;
                            Ok(Some(
                                String::from_utf8(bytes.clone())
                                    .unwrap_or_else(|_| editing::hex_encode(&bytes)),
                            ))
                        }
                    }
                });
            }
            return Ok(None);
        }
        let editor = match self.tool {
            Tool::Repeater => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .and_then(|editors| {
                    if editors.response.has_focus(window, cx) {
                        Some(&editors.response)
                    } else if editors.request.has_focus(window, cx) {
                        Some(&editors.request)
                    } else {
                        None
                    }
                }),
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                if self.history_response.has_focus(window, cx) {
                    Some(&*self.history_response)
                } else if self.history_request.has_focus(window, cx) {
                    Some(&*self.history_request)
                } else {
                    None
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .intercepts
                .iter()
                .find(|pending| {
                    Some(pending.id()) == self.selected_intercept
                        && pending.editor.has_focus(window, cx)
                })
                .map(|pending| &pending.editor),
            Tool::Logger => {
                if self.extra_tools.logger_response.has_focus(window, cx) {
                    Some(&self.extra_tools.logger_response)
                } else if self.extra_tools.logger_request.has_focus(window, cx) {
                    Some(&self.extra_tools.logger_request)
                } else {
                    None
                }
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => {
                if self.target_pane.response.has_focus(window, cx) {
                    Some(&self.target_pane.response)
                } else if self.target_pane.request.has_focus(window, cx) {
                    Some(&self.target_pane.request)
                } else {
                    None
                }
            }
            Tool::Sequencer if self.sequencer_pane.page == sequencer::SequencerPage::Live => {
                if self.sequencer_pane.request.has_focus(window, cx) {
                    Some(&self.sequencer_pane.request)
                } else {
                    None
                }
            }
            Tool::Dashboard => self.focused_dashboard_editor(window, cx),
            Tool::Organiser => {
                if self.organiser.response.has_focus(window, cx) {
                    Some(&self.organiser.response)
                } else if self.organiser.request.has_focus(window, cx) {
                    Some(&self.organiser.request)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(editor) = editor {
            editing::snapshot(editor.state(), window, cx).map_err(str::to_owned)?;
            let selected = editor.state().read(cx).selected_value();
            if !selected.is_empty() {
                if editor.view == MessageView::Hex && !editor.pretty_active {
                    let bytes = editing::hex_decode(&selected)?;
                    return Ok(Some(
                        String::from_utf8(bytes.clone())
                            .unwrap_or_else(|_| editing::hex_encode(&bytes)),
                    ));
                }
                return Ok(Some(selected.to_string()));
            }
        }
        Ok(None)
    }

    fn focused_dashboard_editor(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<&MessageEditor> {
        if self.audit_dashboard.new_scan_open
            && self
                .audit_dashboard
                .scan_request_editor
                .has_focus(window, cx)
        {
            return Some(&self.audit_dashboard.scan_request_editor);
        }
        if !self.has_dashboard_intruder_selection()
            && self.audit_dashboard.selected_finding_id.is_some()
        {
            if self
                .audit_dashboard
                .issue_response_editor
                .has_focus(window, cx)
            {
                return Some(&self.audit_dashboard.issue_response_editor);
            }
            if self
                .audit_dashboard
                .issue_request_editor
                .has_focus(window, cx)
            {
                return Some(&self.audit_dashboard.issue_request_editor);
            }
        }
        None
    }
}
