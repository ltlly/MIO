use super::*;

mod menu_actions;
mod request_codec;

pub(crate) use request_codec::BodyEncodingTarget;
#[cfg(test)]
pub(crate) use request_codec::{
    change_http_body_encoding_bytes, set_http_request_method_bytes,
    toggle_http_request_method_bytes,
};

impl Workbench {
    pub(in crate::workbench) fn search_message_action(
        &mut self,
        _: &Search,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shutdown_requested
            || self.transition == Some(TransitionPhase::Switching)
            || self.rename.is_some()
            || self.intruder.rename.is_some()
        {
            return;
        }
        if self.ext_import.visible {
            if let Some(editor) = self.ext_import.visible_message_editor()
                && let Err(error) = editor.focus_search(window, cx)
            {
                self.fail(error, cx);
            }
            cx.notify();
            return;
        }
        if self.tool == Tool::Intruder {
            let Some(id) = self.workspace.intruder.active else {
                return;
            };
            let Some(editors) = self.intruder.editors.get(&id) else {
                return;
            };
            if editors.has_non_request_focus(window, cx) {
                return;
            }
            let state = editors.request.state().clone();
            let search = editors.search.clone();
            if state.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }) || search.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }) {
                self.fail(editing::COMPOSITION_PENDING, cx);
                return;
            }
            let selected = if state.focus_handle(cx).is_focused(window) {
                state.read(cx).selected_value()
            } else {
                Default::default()
            };
            search.update(cx, |input, cx| {
                if !selected.is_empty() && !selected.contains(['\r', '\n']) {
                    input.set_value(selected, window, cx);
                }
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            self.search_intruder(id, None, window, cx);
            return;
        }
        if self.tool == Tool::Proxy
            && (self.listener_input.focus_handle(cx).is_focused(window)
                || self.history_filter.focus_handle(cx).is_focused(window))
        {
            return;
        }
        let editor = match self.tool {
            Tool::Repeater => {
                if self.workspace.active.is_none()
                    && let Some(ws_id) = self.workspace.active_websocket_tab
                {
                    if let Some(editors) = self.websocket_repeater.editors.get_mut(&ws_id) {
                        if editors.url.focus_handle(cx).is_focused(window) {
                            return;
                        }
                        let target = if editors.history.has_focus(window, cx) {
                            &mut editors.history
                        } else {
                            &mut editors.draft
                        };
                        if let Err(error) = target.focus_search(window, cx) {
                            self.fail(error, cx);
                        }
                        cx.notify();
                    }
                    return;
                }
                let Some(id) = self.workspace.active else {
                    return;
                };
                let plugin_active = self.extensions.active_messages.contains_key(&id);
                let Some(editors) = self.tab_editors.get_mut(&id) else {
                    return;
                };
                if editors.target.focus_handle(cx).is_focused(window) {
                    return;
                }
                if plugin_active || editors.response.has_focus(window, cx) {
                    Some(&mut editors.response)
                } else {
                    Some(&mut editors.request)
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let id = self.selected_intercept;
                self.intercepts
                    .iter_mut()
                    .find(|pending| Some(pending.id()) == id)
                    .map(|pending| &mut pending.editor)
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                let has_detail = self.detail.is_some()
                    || self
                        .imported_history
                        .row
                        .as_ref()
                        .is_some_and(|row| self.selected_history == Some(row.id()));
                if !has_detail {
                    None
                } else if self.history_response.has_focus(window, cx) {
                    Some(&mut self.history_response)
                } else {
                    Some(&mut self.history_request)
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => self
                .websocket
                .detail
                .is_some()
                .then_some(&mut self.websocket.payload),
            Tool::Logger => {
                if self.extra_tools.logger_selected.is_none() {
                    None
                } else if self.extra_tools.logger_response.has_focus(window, cx) {
                    Some(&mut self.extra_tools.logger_response)
                } else {
                    Some(&mut self.extra_tools.logger_request)
                }
            }
            Tool::Target => {
                self.active_target_message_editor(window, cx);
                return;
            }
            Tool::Dashboard => {
                self.focus_active_audit_message_editor(window, cx);
                return;
            }
            Tool::Sequencer => self.active_sequencer_message_editor(window, cx),
            Tool::Collaborator => self.active_collaborator_message_editor(window, cx),
            Tool::Organiser => self.active_organiser_message_editor(window, cx),
            _ => None,
        };
        if let Some(editor) = editor
            && let Err(error) = editor.focus_search(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn convert_selection_action(
        &mut self,
        kind: &editing::ConvertKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            self.fail("正在切换或保存项目；未转换选区。", cx);
            return;
        }
        let result = match self.tool {
            Tool::Repeater => {
                if self.workspace.active.is_none()
                    && let Some(ws_id) = self.workspace.active_websocket_tab
                {
                    let rename_focused = self
                        .rename
                        .as_ref()
                        .is_some_and(|rename| rename.input.focus_handle(cx).is_focused(window));
                    let Some(editors) = self.websocket_repeater.editors.get_mut(&ws_id) else {
                        return;
                    };
                    let res = if editors.history.state().focus_handle(cx).is_focused(window) {
                        editors.history.apply_conversion(*kind, true, window, cx)
                    } else if rename_focused || editors.url.focus_handle(cx).is_focused(window) {
                        return;
                    } else {
                        let res = editors.draft.apply_conversion(*kind, false, window, cx);
                        if res.is_ok() {
                            editors.dirty = true;
                        }
                        res
                    };
                    if let Err(error) = res {
                        self.fail(error, cx);
                    }
                    cx.notify();
                    return;
                }
                let Some(id) = self.workspace.active else {
                    return;
                };
                let rename_focused = self
                    .rename
                    .as_ref()
                    .is_some_and(|rename| rename.input.focus_handle(cx).is_focused(window));
                let Some(editors) = self.tab_editors.get_mut(&id) else {
                    return;
                };
                if editors.response.state().focus_handle(cx).is_focused(window) {
                    editors.response.apply_conversion(*kind, true, window, cx)
                } else if rename_focused
                    || editors.target.focus_handle(cx).is_focused(window)
                    || !editors.request.has_focus(window, cx)
                {
                    return;
                } else {
                    let result = editors.request.apply_conversion(*kind, false, window, cx);
                    if result.is_ok() {
                        editors.dirty = true;
                    }
                    result
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                if self.listener_input.focus_handle(cx).is_focused(window)
                    || self.history_filter.focus_handle(cx).is_focused(window)
                {
                    return;
                }
                let Some(id) = self.selected_intercept else {
                    return;
                };
                let Some(pending) = self
                    .intercepts
                    .iter_mut()
                    .find(|pending| pending.id() == id)
                else {
                    return;
                };
                let readonly = pending.deciding;
                pending.editor.apply_conversion(*kind, readonly, window, cx)
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                if self
                    .history_request
                    .state()
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    self.history_request
                        .apply_conversion(*kind, true, window, cx)
                } else if self
                    .history_response
                    .state()
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    self.history_response
                        .apply_conversion(*kind, true, window, cx)
                } else {
                    Err("当前消息为只读，不能原地转换选区。".into())
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                self.copy_converted_selection_action(
                    &editing::CopyConvertedSelection(*kind),
                    window,
                    cx,
                );
                return;
            }
            Tool::Organiser | Tool::Collaborator => {
                if self.tool == Tool::Organiser
                    && (self.organiser.search.focus_handle(cx).is_focused(window)
                        || self.organiser.notes.focus_handle(cx).is_focused(window))
                {
                    return;
                }
                self.copy_converted_selection_action(
                    &editing::CopyConvertedSelection(*kind),
                    window,
                    cx,
                );
                return;
            }
            Tool::Decoder | Tool::Comparer => {
                self.convert_extra_tool_selection(*kind, window, cx);
                return;
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return;
                };
                if self.intruder_non_request_input_focused(id, window, cx) {
                    return;
                }
                let Some(editors) = self.intruder.editors.get_mut(&id) else {
                    return;
                };
                if editors.request.has_unplaced() {
                    Err("原 payload 位置范围无效且已保留；请先使用清除 § 或自动 § 修复。".into())
                } else if editors.request.view != MessageView::Raw {
                    Err("请切换到 Raw 视图后再执行选区编码或解码转换。".into())
                } else if editors.request.raw.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                }) {
                    Err(editing::COMPOSITION_PENDING.into())
                } else {
                    let range = editors.request.raw.read(cx).selected_range();
                    let reversed = editors.request.raw.read(cx).is_selection_reversed();
                    if range.is_empty() {
                        Err("请先在消息编辑器中选中要转换的文本。".into())
                    } else if editors.request.raw.read(cx).tokens().iter().any(|span| {
                        span.token().id().starts_with("intruder-position:")
                            && span.range().start < range.end
                            && range.start < span.range().end
                    }) {
                        Err(
                            "选区包含 § payload 标记；请先清除选区内的 § 标记或仅选中标记内部文本后再转换。"
                                .into(),
                        )
                    } else {
                        let selected = editors.request.raw.read(cx).selected_value();
                        match editing::convert_selection(*kind, selected.as_ref()) {
                            Ok(converted) => {
                                editors.request.raw.update(cx, |state, cx| {
                                    state.replace(converted.clone(), window, cx);
                                    state.set_selected_range_with_direction(
                                        range.start..range.start + converted.len(),
                                        reversed,
                                        cx,
                                    );
                                });
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    }
                }
            }
            Tool::Sequencer if self.sequencer_pane.page == sequencer::SequencerPage::Live => {
                if self.sequencer_pane.request.has_focus(window, cx) {
                    let readonly = self.sequencer_pane.running.is_some();
                    self.sequencer_pane
                        .request
                        .apply_conversion(*kind, readonly, window, cx)
                } else {
                    return;
                }
            }
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .scan_request_editor
                        .has_focus(window, cx)
                {
                    let readonly = !self.audit_can_operate()
                        || self.audit_dashboard.scan_request_editor.is_readonly_preview();
                    self.audit_dashboard
                        .scan_request_editor
                        .apply_conversion(*kind, readonly, window, cx)
                } else if !self.has_dashboard_intruder_selection()
                    && self.audit_dashboard.selected_finding_id.is_some()
                {
                    if self
                        .audit_dashboard
                        .issue_request_editor
                        .has_focus(window, cx)
                    {
                        self.audit_dashboard
                            .issue_request_editor
                            .apply_conversion(*kind, true, window, cx)
                    } else if self
                        .audit_dashboard
                        .issue_response_editor
                        .has_focus(window, cx)
                    {
                        self.audit_dashboard
                            .issue_response_editor
                            .apply_conversion(*kind, true, window, cx)
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => {
                if self.target_pane.request.has_focus(window, cx) {
                    self.target_pane
                        .request
                        .apply_conversion(*kind, true, window, cx)
                } else if self.target_pane.response.has_focus(window, cx) {
                    self.target_pane
                        .response
                        .apply_conversion(*kind, true, window, cx)
                } else {
                    return;
                }
            }
            Tool::Logger => {
                if self.extra_tools.logger_request.has_focus(window, cx) {
                    self.extra_tools
                        .logger_request
                        .apply_conversion(*kind, true, window, cx)
                } else if self.extra_tools.logger_response.has_focus(window, cx) {
                    self.extra_tools
                        .logger_response
                        .apply_conversion(*kind, true, window, cx)
                } else {
                    return;
                }
            }
            _ => return,
        };
        if let Err(error) = result {
            self.fail(error, cx);
        }
        cx.notify();
    }
}
