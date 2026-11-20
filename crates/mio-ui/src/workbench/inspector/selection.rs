#[cfg(test)]
use super::decode_ext::InspectorSelectionPreview;
use super::decode_ext::{
    SelectionTransform, apply_selection_transform, build_cyberchef_url, build_selection_preview,
    extract_editor_selection, extract_intruder_textarea_selection, format_selection_stats,
};
use super::*;
use crate::{editing::SelectionCandidate, plugin_panel::NativePanel};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use mio_core::RepeaterLayoutMode;

enum InspectorSelectionSurface<'a> {
    Message(&'a MessageEditor, bool),
    Panel(&'a Entity<NativePanel>),
    Candidate(Option<SelectionCandidate>, bool),
}

impl InspectorSelectionSurface<'_> {
    fn candidate(&self, window: Option<&Window>, cx: &App) -> Option<SelectionCandidate> {
        match self {
            Self::Message(editor, writable) => {
                extract_editor_selection(editor, *writable, window, cx)
            }
            Self::Panel(panel) => panel.read(cx).selection_candidate(window, cx),
            Self::Candidate(candidate, _) => candidate.clone(),
        }
    }

    fn is_selecting(&self, cx: &App) -> bool {
        match self {
            Self::Message(editor, _) => editor.is_selecting(cx),
            Self::Panel(panel) => panel.read(cx).is_selecting(cx),
            Self::Candidate(_, selecting) => *selecting,
        }
    }
}

impl Workbench {
    fn inspector_message_surfaces(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Vec<InspectorSelectionSurface<'_>> {
        let mut surfaces = Vec::with_capacity(3);
        if self.shutdown_requested || self.transition == Some(TransitionPhase::Switching) {
            return surfaces;
        }
        if self.ext_import.visible {
            if self.ext_import.browser_visible
                && let Some(editor) = self.ext_import.message_editor_ref()
            {
                surfaces.push(InspectorSelectionSurface::Message(editor, false));
            }
            return surfaces;
        }
        match self.tool {
            Tool::Repeater => {
                if self.workspace.active.is_none()
                    && let Some(ws_id) = self.workspace.active_websocket_tab
                {
                    if self
                        .workspace
                        .websocket_tabs
                        .iter()
                        .any(|tab| tab.id == ws_id && !tab.closed)
                        && let Some(editors) = self.websocket_repeater.editors.get(&ws_id)
                    {
                        let ws_writable = self.project.is_some()
                            && self.backend_available()
                            && self.transition.is_none()
                            && editors.connecting.is_none()
                            && !editors.disconnecting
                            && editors.sending.is_none();
                        surfaces.push(InspectorSelectionSurface::Message(
                            &editors.draft,
                            ws_writable,
                        ));
                        surfaces.push(InspectorSelectionSurface::Message(&editors.history, false));
                    }
                    return surfaces;
                }
                let Some(tab_id) = self.workspace.active else {
                    return surfaces;
                };
                let Some(editors) = self.tab_editors.get(&tab_id) else {
                    return surfaces;
                };
                let busy = self.sending.contains_key(&tab_id)
                    || self.pending_sends.contains_key(&tab_id)
                    || self.plugin_waiting(tab_id)
                    || self.extensions.active_messages.contains_key(&tab_id);
                let req_editable = self.project.is_some()
                    && self.backend_available()
                    && self.transition.is_none()
                    && !busy;
                let request_panel = self.extensions.repeater_request_panel(tab_id);
                let response_panel = self.extensions.repeater_response_panel(tab_id);
                if request_panel.is_none()
                    && response_panel.is_none()
                    && !self.extensions.active_messages.contains_key(&tab_id)
                    && !self.extensions.active_responses.contains_key(&tab_id)
                    && self.workspace.repeater_settings.layout == RepeaterLayoutMode::Combined
                {
                    let editor = match editors.combined_pane.get() {
                        RepeaterCombinedPane::Request => (&editors.request, req_editable),
                        RepeaterCombinedPane::Response => (&editors.response, false),
                    };
                    surfaces.push(InspectorSelectionSurface::Message(editor.0, editor.1));
                    return surfaces;
                }
                surfaces.push(match request_panel {
                    Some(panel) => InspectorSelectionSurface::Panel(panel),
                    None => InspectorSelectionSurface::Message(&editors.request, req_editable),
                });
                surfaces.push(match response_panel {
                    Some(panel) => InspectorSelectionSurface::Panel(panel),
                    None => InspectorSelectionSurface::Message(&editors.response, false),
                });
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let Some(pending) = self
                    .selected_intercept
                    .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                else {
                    return surfaces;
                };
                let panel = match pending.target {
                    InterceptTarget::Http {
                        stage: InterceptStage::Request,
                        ..
                    } => self.extensions.context_request_panel("intercept"),
                    InterceptTarget::Http {
                        stage: InterceptStage::Response,
                        ..
                    } => self.extensions.context_response_panel("intercept"),
                    InterceptTarget::WebSocket { .. } => None,
                };
                let editable = self.project.is_some()
                    && self.backend_available()
                    && self.transition.is_none()
                    && !pending.deciding;
                surfaces.push(match panel {
                    Some(panel) => InspectorSelectionSurface::Panel(panel),
                    None => InspectorSelectionSurface::Message(&pending.editor, editable),
                });
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                let is_imported = matches!(self.selected_history, Some(HistoryEntryId::External { .. }))
                    || (self.selected_history.is_none()
                        && self
                            .imported_history
                            .row
                            .as_ref()
                            .is_some_and(|r| matches!(r, HistoryRow::ExtProxy(_))));
                let owner = if is_imported {
                    "imported-history"
                } else {
                    "history"
                };
                if self.detail.is_some()
                    || self.imported_history.row.as_ref().is_some_and(|r| {
                        self.selected_history.is_none_or(|sel| sel == r.id())
                            && matches!(r, HistoryRow::ExtProxy(_))
                    })
                {
                    self.append_context_selection_surfaces(
                        &mut surfaces,
                        owner,
                        &self.history_request,
                        &self.history_response,
                    );
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                if self.websocket.detail.is_some() {
                    surfaces.push(InspectorSelectionSurface::Message(
                        &self.websocket.payload,
                        false,
                    ));
                }
            }
            Tool::Logger => self.append_context_selection_surfaces(
                &mut surfaces,
                "logger",
                &self.extra_tools.logger_request,
                &self.extra_tools.logger_response,
            ),
            Tool::Target
                if self.target_pane.page == target::TargetPage::SiteMap
                    && self.target_pane.site_map_subview == target::SiteMapSubView::Contents =>
            {
                self.append_context_selection_surfaces(
                    &mut surfaces,
                    "target",
                    &self.target_pane.request,
                    &self.target_pane.response,
                );
            }
            Tool::Organiser => self.append_context_selection_surfaces(
                &mut surfaces,
                "organiser",
                &self.organiser.request,
                &self.organiser.response,
            ),
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open {
                    surfaces.push(InspectorSelectionSurface::Message(
                        &self.audit_dashboard.scan_request_editor,
                        false,
                    ));
                }
                if !self.has_dashboard_intruder_selection()
                    && self.audit_dashboard.selected_finding_id.is_some_and(|id| {
                        self.audit_dashboard
                            .snapshot
                            .findings
                            .iter()
                            .any(|f| f.id == id && f.flow_id.is_some())
                    })
                    && self.audit_dashboard.flow_error.is_none()
                    && !(self.audit_dashboard.pending_flow_read.is_some()
                        && self.audit_dashboard.loaded_flow.is_none())
                {
                    let editor = match self.audit_dashboard.detail_tab {
                        audit_dashboard::IssueDetailTab::Request => {
                            Some(&self.audit_dashboard.issue_request_editor)
                        }
                        audit_dashboard::IssueDetailTab::Response => {
                            Some(&self.audit_dashboard.issue_response_editor)
                        }
                        audit_dashboard::IssueDetailTab::Advisory => None,
                    };
                    if let Some(editor) = editor {
                        surfaces.push(InspectorSelectionSurface::Message(editor, false));
                    }
                }
            }
            Tool::Collaborator => {
                surfaces.push(InspectorSelectionSurface::Message(
                    &self.collaborator_pane.raw_editor,
                    false,
                ));
            }
            Tool::Decoder => {
                let selecting = self.extra_tools.decoder_input.read(cx).is_selecting()
                    || self.extra_tools.decoder_output.read(cx).is_selecting();
                surfaces.push(InspectorSelectionSurface::Candidate(
                    self.extra_tools.decoder_selection_candidate(window, cx),
                    selecting,
                ));
            }
            Tool::Comparer => {
                let selecting = if self.extra_tools.comparer_side_by_side {
                    self.extra_tools.comparer_left_view.read(cx).is_selecting()
                        || self.extra_tools.comparer_right_view.read(cx).is_selecting()
                } else {
                    self.extra_tools.comparer_view.read(cx).is_selecting()
                };
                surfaces.push(InspectorSelectionSurface::Candidate(
                    self.extra_tools.comparer_selection_candidate(window, cx),
                    selecting,
                ));
            }
            _ => {}
        }
        surfaces
    }

    fn append_context_selection_surfaces<'a>(
        &'a self,
        surfaces: &mut Vec<InspectorSelectionSurface<'a>>,
        owner: &str,
        request: &'a MessageEditor,
        response: &'a MessageEditor,
    ) {
        surfaces.push(match self.extensions.context_request_panel(owner) {
            Some(panel) => InspectorSelectionSurface::Panel(panel),
            None => InspectorSelectionSurface::Message(request, false),
        });
        surfaces.push(match self.extensions.context_response_panel(owner) {
            Some(panel) => InspectorSelectionSurface::Panel(panel),
            None => InspectorSelectionSurface::Message(response, false),
        });
    }

    pub(in crate::workbench) fn current_inspector_selection(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<SelectionCandidate> {
        self.current_inspector_selection_opt(Some(window), cx)
    }

    pub(in crate::workbench) fn current_inspector_selection_opt(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<SelectionCandidate> {
        if self.shutdown_requested || self.transition == Some(TransitionPhase::Switching) {
            return None;
        }
        if !self.ext_import.visible {
            match self.tool {
                Tool::Intruder => {
                    let id = self.workspace.intruder.active?;
                    let editors = self.intruder.editors.get(&id)?;
                    return extract_intruder_textarea_selection(
                        editors.request.state(),
                        editors.request.view,
                        window,
                        cx,
                    );
                }
                Tool::Sequencer => return self.sequencer_pane.selection_candidate(window, cx),
                _ => {}
            }
        }
        editing::pick_selection_candidate(
            self.inspector_message_surfaces(window, cx)
                .into_iter()
                .filter_map(|surface| surface.candidate(window, cx)),
        )
    }

    pub(in crate::workbench) fn any_mounted_editor_selecting(
        &self,
        window: &Window,
        cx: &App,
    ) -> bool {
        if self.shutdown_requested || self.transition == Some(TransitionPhase::Switching) {
            return false;
        }
        if !self.ext_import.visible {
            match self.tool {
                Tool::Intruder => {
                    return self
                        .workspace
                        .intruder
                        .active
                        .and_then(|id| self.intruder.editors.get(&id))
                        .is_some_and(|editors| editors.request.state().read(cx).is_selecting());
                }
                Tool::Sequencer => return self.sequencer_pane.is_selecting(cx),
                _ => {}
            }
        }
        self.inspector_message_surfaces(Some(window), cx)
            .iter()
            .any(|surface| surface.is_selecting(cx))
    }

    #[cfg(test)]
    pub(super) fn current_selection_preview_with_window(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<InspectorSelectionPreview> {
        let candidate = self.current_inspector_selection_opt(window, cx)?;
        if self.inspector.selection_identity.as_ref() == Some(&candidate.identity)
            && let Some(preview) = self.inspector.selection_preview.as_ref()
            && preview.raw_bytes == candidate.raw_bytes
        {
            return Some(preview.clone());
        }
        Some(candidate.build_preview())
    }

    #[cfg(test)]
    pub(super) fn current_selection_preview(&self, cx: &App) -> Option<InspectorSelectionPreview> {
        self.current_selection_preview_with_window(None, cx)
    }

    pub(super) fn probe_inspector_selection_surfaces(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(Vec<(SelectionIdentity, bool, u64)>, bool)> {
        if self.shutdown_requested || self.transition == Some(TransitionPhase::Switching) {
            return Some((Vec::new(), false));
        }
        if !self.ext_import.visible {
            match self.tool {
                Tool::Intruder => {
                    let Some(id) = self.workspace.intruder.active else {
                        return Some((Vec::new(), false));
                    };
                    let Some(editors) = self.intruder.editors.get(&id) else {
                        return Some((Vec::new(), false));
                    };
                    let state_entity = editors.request.state();
                    let focused = state_entity.focus_handle(cx).is_focused(window);
                    let state = state_entity.read(cx);
                    let selecting = state.is_selecting();
                    let range = state.selected_range();
                    if range.is_empty() {
                        return Some((Vec::new(), selecting));
                    }
                    let source = match editors.request.view {
                        MessageView::Raw => editing::SelectionSource::Utf8,
                        MessageView::Hex => editing::SelectionSource::Hex,
                    };
                    return Some((
                        vec![(
                            SelectionIdentity {
                                editor_id: state_entity.entity_id(),
                                document_revision: state.document_revision(),
                                range,
                                source,
                                writable: false,
                            },
                            focused,
                            0,
                        )],
                        selecting,
                    ));
                }
                Tool::Sequencer => return None,
                _ => {}
            }
        }
        let surfaces = self.inspector_message_surfaces(Some(window), cx);
        let mut identities = Vec::with_capacity(surfaces.len());
        let mut any_selecting = false;
        for surface in surfaces {
            let InspectorSelectionSurface::Message(editor, writable) = surface else {
                return None;
            };
            let active_entity = editor.state();
            let active = active_entity.read(cx);
            any_selecting |= active.is_selecting();
            let range = active.selected_range();
            if range.is_empty() {
                continue;
            }
            let (source, can_write_back) = if editor.is_readonly_preview() {
                (editing::SelectionSource::Utf8, false)
            } else {
                match if editor.pretty_active {
                    MessageView::Raw
                } else {
                    editor.view
                } {
                    MessageView::Raw => (editing::SelectionSource::Utf8, writable),
                    MessageView::Hex => (editing::SelectionSource::Hex, writable),
                }
            };
            let focused = active_entity.focus_handle(cx).is_focused(window)
                || (editor.view == MessageView::Hex && editor.hex_grid_has_focus(window, cx));
            identities.push((
                SelectionIdentity {
                    editor_id: active_entity.entity_id(),
                    document_revision: active.document_revision(),
                    range,
                    source,
                    writable: can_write_back,
                },
                focused,
                editor.selection_seq(),
            ));
        }
        Some((identities, any_selecting))
    }

    pub(super) fn apply_inspector_selection_candidate(
        &mut self,
        candidate: Option<SelectionCandidate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.inspector.selection_editor.read(cx).has_marked_text() {
            return false;
        }
        let identity = candidate.as_ref().map(|c| c.identity.clone());
        let preview = candidate.as_ref().map(|c| {
            if self.inspector.selection_identity.as_ref() == Some(&c.identity)
                && let Some(preview) = self.inspector.selection_preview.as_ref()
                && preview.raw_bytes == c.raw_bytes
            {
                preview.clone()
            } else {
                c.build_preview()
            }
        });
        let text = preview
            .as_ref()
            .map_or_else(String::new, |p| p.decoded.clone());
        self.inspector.selection_loaded = preview
            .as_ref()
            .map(|p| (p.original_text.clone(), p.reversible_steps.clone()));
        self.inspector.selection_manual_override = false;
        self.inspector.selection_identity = identity;
        self.inspector.selection_preview = preview;
        self.inspector.selection_editor.update(cx, |state, cx| {
            state.set_value(text, window, cx);
        });
        true
    }

    pub(in crate::workbench) fn sync_inspector_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.inspector.selection_editor.read(cx).has_marked_text() {
            self.inspector.last_surface_snapshot = None;
            return;
        }
        let owner_matches =
            self.inspector.last_owner_key.as_ref() == Some(&self.current_inspector_owner_key());
        let probe = self.probe_inspector_selection_surfaces(window, cx);
        if owner_matches
            && let Some((snapshot, _)) = probe.as_ref()
            && self.inspector.last_surface_snapshot.as_ref() == Some(snapshot)
            && let Some(preview) = self.inspector.selection_preview.as_ref()
        {
            let text = preview.decoded.clone();
            self.inspector.selection_loaded = Some((
                preview.original_text.clone(),
                preview.reversible_steps.clone(),
            ));
            self.inspector.selection_manual_override = false;
            self.inspector.selection_editor.update(cx, |state, cx| {
                state.set_value(text, window, cx);
            });
            return;
        }
        let candidate = self.current_inspector_selection(window, cx);
        let synced = self.apply_inspector_selection_candidate(candidate, window, cx);
        self.inspector.last_surface_snapshot = if synced {
            probe.map(|(snapshot, _)| snapshot)
        } else {
            None
        };
    }

    pub(super) fn transform_inspector_selection(
        &mut self,
        transform: SelectionTransform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_text = match editing::snapshot(&self.inspector.selection_editor, window, cx) {
            Ok(text) => text.to_string(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        match apply_selection_transform(transform, &current_text) {
            Ok(converted) => {
                self.inspector.selection_manual_override = true;
                self.inspector.selection_editor.update(cx, |state, cx| {
                    state.set_value(converted, window, cx);
                });
                self.error = None;
                cx.notify();
            }
            Err(err) => self.fail(err, cx),
        }
    }

    pub(in crate::workbench) fn open_selection_in_cyberchef(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.current_inspector_selection(window, cx) else {
            self.fail("请先在消息编辑器中选中要在 CyberChef 中打开的内容。", cx);
            return;
        };
        let raw_bytes = if self.inspector.open
            && let Some(cached_id) = self.inspector.selection_identity.as_ref()
        {
            if cached_id != &current.identity {
                self.fail(
                    "当前活动选区已变化或过期，请先重新同步 Inspector 选区后再打开 CyberChef。",
                    cx,
                );
                return;
            }
            self.inspector
                .selection_preview
                .as_ref()
                .map_or_else(|| current.raw_bytes.clone(), |p| p.raw_bytes.clone())
        } else {
            current.raw_bytes
        };
        match build_cyberchef_url(&raw_bytes) {
            Ok(url) => {
                cx.open_url(&url);
                self.error = None;
                self.status = "已在浏览器中打开 CyberChef。".into();
                cx.notify();
            }
            Err(err) => self.fail(err, cx),
        }
    }

    pub(super) fn apply_inspector_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let decoded_text = match editing::snapshot(&self.inspector.selection_editor, window, cx) {
            Ok(text) => text.to_string(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let Some(cached_id) = self.inspector.selection_identity.clone() else {
            self.fail("当前没有可回写的已同步选区，请先选中请求 Raw 文本。", cx);
            return;
        };
        if !cached_id.writable {
            self.fail("当前选区来源为只读，不能通过 Inspector 回写。", cx);
            return;
        }
        let Some(current) = self.current_inspector_selection(window, cx) else {
            self.fail("目标选区已清除或不可见，未执行回写。", cx);
            return;
        };
        if current.identity != cached_id || current.writeback_editor.is_none() {
            self.fail(
                "目标选区已变化、过期或切换到其他编辑器，请重新同步后再回写。",
                cx,
            );
            return;
        }
        let editor = match self.tool {
            Tool::Repeater => {
                if self.workspace.active.is_none()
                    && let Some(ws_id) = self.workspace.active_websocket_tab
                {
                    if self.project.is_none()
                        || !self.backend_available()
                        || self.transition.is_some()
                    {
                        self.fail("当前 WebSocket 标签暂不能通过 Inspector 回写选区。", cx);
                        return;
                    }
                    let Some(editors) = self.websocket_repeater.editors.get_mut(&ws_id) else {
                        return;
                    };
                    if editors.connecting.is_some()
                        || editors.disconnecting
                        || editors.sending.is_some()
                    {
                        self.fail("当前 WebSocket 标签正在通信，暂不能回写选区。", cx);
                        return;
                    }
                    &mut editors.draft
                } else {
                let Some(tab_id) = self.workspace.active else {
                    return;
                };
                if self.project.is_none()
                    || !self.backend_available()
                    || self.transition.is_some()
                    || self.sending.contains_key(&tab_id)
                    || self.pending_sends.contains_key(&tab_id)
                    || self.plugin_waiting(tab_id)
                    || self.extensions.active_messages.contains_key(&tab_id)
                {
                    self.fail("当前 Repeater 标签暂不能通过 Inspector 回写选区。", cx);
                    return;
                }
                let Some(editors) = self.tab_editors.get_mut(&tab_id) else {
                    return;
                };
                if !self.extensions.active_responses.contains_key(&tab_id)
                    && self.extensions.repeater_response_panel(tab_id).is_none()
                    && self.workspace.repeater_settings.layout == RepeaterLayoutMode::Combined
                    && editors.combined_pane.get() != RepeaterCombinedPane::Request
                {
                    self.fail("当前处于响应视图，不能将选区回写到隐藏的请求草稿。", cx);
                    return;
                }
                &mut editors.request
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                if self.project.is_none() || !self.backend_available() || self.transition.is_some()
                {
                    self.fail("当前拦截消息暂不能通过 Inspector 回写选区。", cx);
                    return;
                }
                let Some(id) = self.selected_intercept else {
                    return;
                };
                let Some(pending) = self.intercepts.iter_mut().find(|p| p.id() == id) else {
                    return;
                };
                if pending.deciding {
                    self.fail("当前拦截消息正在决策，不能回写选区。", cx);
                    return;
                }
                let has_plugin_panel = match pending.target {
                    InterceptTarget::Http {
                        stage: InterceptStage::Request,
                        ..
                    } => self.extensions.context_request_panel("intercept").is_some(),
                    InterceptTarget::Http {
                        stage: InterceptStage::Response,
                        ..
                    } => self
                        .extensions
                        .context_response_panel("intercept")
                        .is_some(),
                    InterceptTarget::WebSocket { .. } => false,
                };
                if has_plugin_panel {
                    self.fail(
                        "当前正在使用插件视图，不能通过 Inspector 回写原生选区。",
                        cx,
                    );
                    return;
                }
                &mut pending.editor
            }
            _ => {
                self.fail("当前消息为只读，不能通过 Inspector 回写选区。", cx);
                return;
            }
        };

        if editor.is_readonly_preview() {
            self.fail("请切换到可编辑的 Raw 或 Pretty 视图后回写选区解码内容。", cx);
            return;
        }
        if !editor.pretty_active && editor.view == MessageView::Hex {
            if editor.hex.entity_id() != cached_id.editor_id {
                self.fail("选区所属编辑器与回写目标不一致，已拒绝跨面板回写。", cx);
                return;
            }
            if editor
                .hex
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            {
                self.fail(editing::COMPOSITION_PENDING, cx);
                return;
            }
            let (doc_rev, range) = {
                let state = editor.hex.read(cx);
                (state.document_revision(), state.selected_range())
            };
            if doc_rev != cached_id.document_revision || range != cached_id.range {
                self.fail("请求报文或选区范围已改变，请重新载入选区后再回写。", cx);
                return;
            }
            let Some(byte_range) = editor.hex_selected_byte_range(cx) else {
                self.fail("请先在 Hex 视图中选中要回写的完整字节范围。", cx);
                return;
            };
            let replacement_bytes = if !self.inspector.selection_manual_override
                && self
                    .inspector
                    .selection_preview
                    .as_ref()
                    .is_some_and(|p| p.chain_label == "Hex 字节")
            {
                editing::hex_decode(&decoded_text).unwrap_or_else(|_| decoded_text.into_bytes())
            } else {
                decoded_text.into_bytes()
            };
            let hex_entity_id = editor.hex.entity_id();
            match editor.replace_hex_byte_range(byte_range, &replacement_bytes, window, cx) {
                Ok((next_doc_rev, new_text_range)) => {
                    let next_identity = editing::SelectionIdentity {
                        editor_id: hex_entity_id,
                        document_revision: next_doc_rev,
                        range: new_text_range,
                        source: editing::SelectionSource::Hex,
                        writable: true,
                    };
                    let next_preview = build_selection_preview(replacement_bytes.clone(), false, true);
                    self.inspector.selection_identity = Some(next_identity.clone());
                    self.inspector.last_observed_selection = Some(next_identity);
                    self.inspector.last_surface_snapshot = None;
                    self.inspector.selection_preview = Some(next_preview);
                    self.inspector.selection_loaded =
                        Some((editing::hex_encode(&replacement_bytes), Vec::new()));
                    self.inspector.selection_manual_override = false;
                    if self.tool == Tool::Repeater
                        && let Ok(true) = self.capture_drafts(window, cx)
                    {
                        self.save_workspace(cx);
                    }
                    cx.notify();
                }
                Err(err) => self.fail(err, cx),
            }
            return;
        }
        if editor.raw.entity_id() != cached_id.editor_id {
            self.fail("选区所属编辑器与回写目标不一致，已拒绝跨面板回写。", cx);
            return;
        }
        if editor
            .raw
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let (doc_rev, range, reversed, current_selected) = {
            let state = editor.raw.read(cx);
            (
                state.document_revision(),
                state.selected_range(),
                state.is_selection_reversed(),
                state.selected_value().to_string(),
            )
        };
        if doc_rev != cached_id.document_revision || range != cached_id.range {
            self.fail("请求报文或选区范围已改变，请重新载入选区后再回写。", cx);
            return;
        }
        if range.is_empty() {
            self.fail("请先在请求编辑器中选中要回写的范围。", cx);
            return;
        }
        if current_selected.contains('§') {
            self.fail(
                "选区包含 § payload 标记；为保护标记配对，请仅选中标记内部文本后再回写。",
                cx,
            );
            return;
        }
        let steps = if self.inspector.selection_manual_override {
            Vec::new()
        } else {
            self.inspector
                .selection_preview
                .as_ref()
                .filter(|p| p.original_text == current_selected)
                .map(|p| p.reversible_steps.clone())
                .or_else(|| {
                    self.inspector
                        .selection_loaded
                        .as_ref()
                        .filter(|(orig, _)| orig == &current_selected)
                        .map(|(_, saved_steps)| saved_steps.clone())
                })
                .unwrap_or_else(|| decode_inspector_selection(&current_selected).steps)
        };
        let reencoded = match encode_inspector_selection(&decoded_text, &steps) {
            Ok(out) => out,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        if !reencoded.matches('§').count().is_multiple_of(2) {
            self.fail("回写结果包含未配对的 § payload 标记；原始字节未改变。", cx);
            return;
        }
        let new_end = range.start + reencoded.len();
        let new_range = range.start..new_end;
        let raw_entity_id = editor.raw.entity_id();
        let next_doc_rev = editor.raw.update(cx, |state, cx| {
            state.replace(reencoded.clone(), window, cx);
            state.set_selected_range_with_direction(new_range.clone(), reversed, cx);
            state.document_revision()
        });
        let next_identity = editing::SelectionIdentity {
            editor_id: raw_entity_id,
            document_revision: next_doc_rev,
            range: new_range,
            source: editing::SelectionSource::Utf8,
            writable: true,
        };
        let next_preview = build_selection_preview(reencoded.clone().into_bytes(), true, true);
        self.inspector.selection_identity = Some(next_identity.clone());
        self.inspector.last_observed_selection = Some(next_identity);
        self.inspector.last_surface_snapshot = None;
        self.inspector.selection_preview = Some(next_preview);
        self.inspector.selection_loaded = Some((reencoded, steps));
        self.inspector.selection_manual_override = false;
        if self.tool == Tool::Repeater
            && let Ok(true) = self.capture_drafts(window, cx)
        {
            self.save_workspace(cx);
        }
        cx.notify();
    }

    #[inline(never)]
    pub(super) fn render_inspector_selection_section(
        &self,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(preview) = self.inspector.selection_preview.as_ref() else {
            return div().into_any_element();
        };
        let source_writable = self
            .inspector
            .selection_identity
            .as_ref()
            .is_some_and(|id| id.writable);
        let show_transform_bar = editable && source_writable;
        let can_write_back = editable
            && source_writable
            && (preview.can_write_back || self.inspector.selection_manual_override);
        let is_expanded = self.inspector.selection_expanded;
        let byte_len = preview.raw_bytes.len();
        let count_pill_text = format!("{byte_len} (0x{byte_len:x})");
        let is_decoded = preview.chain_label != "原文";
        let codec_badge = if self.inspector.selection_manual_override {
            "手动转换".to_owned()
        } else {
            preview.chain_label.clone()
        };
        let subtle_border: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x34373c } else { 0xe2e2e2 }).into();
        let subheader_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
        let card_body_bg: gpui_kit::Hsla =
            theme::surface();
        let pill_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
        let mut container = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                Button::new("inspector-section-selection")
                    .ghost()
                    .rounded(ButtonRounded::None)
                    .w_full()
                    .h(px(38.))
                    .px(px(10.))
                    .bg(theme::toolbar_surface())
                    .accessibility_label("Selection")
                    .tooltip(if is_expanded {
                        "折叠选区解码检视分组"
                    } else {
                        "展开选区解码检视分组"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        this.inspector.selection_expanded = !this.inspector.selection_expanded;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui_kit::FontWeight::NORMAL)
                                    .text_color(theme::foreground())
                                    .child("Selection"),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .min_w(px(56.))
                                    .h(px(18.))
                                    .px(px(6.))
                                    .rounded(px(9.))
                                    .bg(pill_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(13.))
                                    .text_color(theme::foreground())
                                    .child(count_pill_text),
                            )
                            .child(div().w(px(12.)))
                            .child(
                                gpui_kit::component::Icon::new(if is_expanded {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(px(14.))
                                .text_color(theme::foreground()),
                            ),
                    ),
            );
        if !is_expanded {
            return container.into_any_element();
        }
        let editor_value = self.inspector.selection_editor.read(cx).value().to_string();
        let stats_label = format_selection_stats(&preview.raw_bytes, &editor_value);
        let summary_tip = format!(
            "编码链：{}（{}，点击重新同步选区）",
            preview.chain_label, stats_label
        );
        let line_count = editor_value.lines().count().max(1);
        let wrapped_rows = if self.inspector.selection_wrap {
            self.inspector
                .selection_editor
                .read(cx)
                .display_map()
                .wrap_row_count()
                .max(line_count)
        } else {
            line_count
        };
        let editor_h = if wrapped_rows <= 1 {
            px(41.)
        } else {
            px(21. + 20. * (wrapped_rows.min(5) as f32))
        };
        let editor_card = self.render_inspector_selection_editor_card(
            is_decoded,
            can_write_back,
            show_transform_bar,
            codec_badge,
            summary_tip,
            editor_h,
            subheader_bg,
            card_body_bg,
            subtle_border,
            cx,
        );
        let body_cards = div()
            .w_full()
            .flex()
            .flex_col()
            .px(px(10.))
            .pb(px(20.))
            .bg(theme::toolbar_surface())
            .child(editor_card);
        container = container.child(body_cards);
        container.into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_selection_editor_card(
        &self,
        is_decoded: bool,
        can_write_back: bool,
        show_transform_bar: bool,
        codec_badge: String,
        summary_tip: String,
        editor_h: gpui_kit::Pixels,
        subheader_bg: gpui_kit::Hsla,
        card_body_bg: gpui_kit::Hsla,
        subtle_border: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show_codec_badge = is_decoded || self.inspector.selection_manual_override;
        let wrap_enabled = self.inspector.selection_wrap;
        div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("inspector-selection-table-header")
                        .test_support()
                        .w_full()
                        .h(px(27.))
                        .bg(subheader_bg)
                        .border_b_1()
                        .border_color(subtle_border)
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .px(px(10.))
                                .border_r_1()
                                .border_color(subtle_border)
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .when(show_codec_badge, |hdr| {
                                    hdr.child(
                                        div()
                                            .font_family(theme::ui_font_family())
                                            .text_size(px(12.5))
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .text_color(theme::foreground())
                                            .child("Decoded from"),
                                    )
                                    .child(
                                        Button::new("inspector-selection-item")
                                            .ghost()
                                            .small()
                                            .compact()
                                            .rounded(ButtonRounded::Small)
                                            .h(px(19.))
                                            .px(px(6.))
                                            .border_1()
                                            .border_color(subtle_border)
                                            .bg(theme::surface())
                                            .accessibility_label(codec_badge.clone())
                                            .tooltip(summary_tip.clone())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.inspector.auto_opened_by_selection = false;
                                                this.sync_inspector_selection(window, cx);
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .font_family(theme::ui_font_family())
                                                    .text_size(px(12.))
                                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                    .text_color(theme::accent())
                                                    .child(codec_badge.clone()),
                                            ),
                                    )
                                })
                                .when(!show_codec_badge, |hdr| {
                                    hdr.child(
                                        Button::new("inspector-selection-item")
                                            .ghost()
                                            .small()
                                            .compact()
                                            .rounded(ButtonRounded::None)
                                            .h(px(21.))
                                            .px_0()
                                            .accessibility_label("Selected text")
                                            .tooltip(summary_tip)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.inspector.auto_opened_by_selection = false;
                                                this.sync_inspector_selection(window, cx);
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .font_family(theme::ui_font_family())
                                                    .text_size(px(13.))
                                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                    .text_color(theme::foreground())
                                                    .child("Selected text"),
                                            ),
                                    )
                                }),
                        )
                        .child(
                            Button::new("inspector-selection-wrap-toggle")
                                .ghost()
                                .small()
                                .compact()
                                .rounded(ButtonRounded::None)
                                .w(px(28.))
                                .h(px(27.))
                                .p_0()
                                .selected(wrap_enabled)
                                .toggled(wrap_enabled)
                                .when(wrap_enabled, |b| b.bg(rgb(0x26649d)))
                                .accessibility_label("切换 Selected text 自动换行")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.inspector.auto_opened_by_selection = false;
                                    let next = !this.inspector.selection_wrap;
                                    this.inspector.selection_wrap = next;
                                    this.inspector.selection_editor.update(cx, |state, cx| {
                                        state.set_soft_wrap(next, window, cx);
                                    });
                                    cx.notify();
                                }))
                                .child(
                                    gpui_kit::component::Icon::default()
                                        .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2 4h12M2 8h9a2 2 0 0 1 0 4H8m2-2-2 2 2 2M2 12h3\"/></svg>")
                                        .size(px(14.))
                                        .text_color(if wrap_enabled {
                                            gpui_kit::white()
                                        } else {
                                            theme::foreground()
                                        }),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("inspector-selection-editor")
                        .test_support()
                        .min_h(editor_h)
                        .h(editor_h)
                        .px(px(10.))
                        .py(px(8.))
                        .bg(card_body_bg)
                        .child(
                            Editor::new(&self.inspector.selection_editor)
                                .readonly(!show_transform_bar)
                                .bordered(false)
                                .text_area_paddings(gpui_kit::Edges::all(px(0.)))
                                .aria_label("选区解码内容")
                                .size_full(),
                        ),
                )
                .when(show_transform_bar || can_write_back, |card| {
                    card.child(self.render_inspector_selection_transform_bar(
                        show_transform_bar,
                        can_write_back,
                        subheader_bg,
                        subtle_border,
                        cx,
                    ))
                })
                .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_selection_transform_bar(
        &self,
        show_transform_bar: bool,
        can_write_back: bool,
        subheader_bg: gpui_kit::Hsla,
        subtle_border: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        div()
            .w_full()
            .px(px(8.))
            .py(px(5.))
            .bg(subheader_bg)
            .border_t_1()
            .border_color(subtle_border)
            .flex()
            .items_center()
            .gap(px(4.))
            .when(show_transform_bar, |row| {
                row.child(
                    Button::new("inspector-selection-convert-menu")
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(theme::scaled_px(22.))
                        .px(px(8.))
                        .border_1()
                        .border_color(theme::control_border())
                        .bg(theme::surface())
                        .label("转换 ▾")
                        .tooltip("对选区文本执行编码或解码转换")
                        .dropdown_menu(move |menu, _, _| {
                            [
                                ("URL 编码", SelectionTransform::UrlEncode),
                                ("URL 解码", SelectionTransform::UrlDecode),
                                ("Base64 编码", SelectionTransform::Base64Encode),
                                ("Base64 解码", SelectionTransform::Base64Decode),
                                ("Base64URL 编码", SelectionTransform::Base64UrlEncode),
                                ("Base64URL 解码", SelectionTransform::Base64UrlDecode),
                                ("HTML 编码", SelectionTransform::HtmlEncode),
                                ("HTML 解码", SelectionTransform::HtmlDecode),
                                ("ASCII Hex 编码", SelectionTransform::AsciiHexEncode),
                                ("ASCII Hex 解码", SelectionTransform::AsciiHexDecode),
                            ]
                            .into_iter()
                            .fold(menu, |menu, (item_label, tf)| {
                                let entity = weak.clone();
                                menu.item(PopupMenuItem::new(item_label).on_click(
                                    move |_, window, cx| {
                                        let _ = entity.update(cx, |this, cx| {
                                            this.inspector.auto_opened_by_selection = false;
                                            this.transform_inspector_selection(tf, window, cx);
                                        });
                                    },
                                ))
                            })
                        }),
                )
            })
            .child(
                Button::new("inspector-selection-copy")
                    .ghost()
                    .small()
                    .compact()
                    .rounded(ButtonRounded::Small)
                    .w(theme::scaled_px(22.))
                    .h(theme::scaled_px(22.))
                    .icon(IconName::Copy)
                    .accessibility_label("复制")
                    .tooltip("复制当前选区文本到剪贴板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let val = this
                            .inspector
                            .selection_editor
                            .read(cx)
                            .value()
                            .to_string();
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(val));
                        this.status = "已复制选区文本到剪贴板。".into();
                        cx.notify();
                    })),
            )
            .when(can_write_back, |row| {
                row.child(
                    Button::new("inspector-selection-load")
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .w(theme::scaled_px(22.))
                        .h(theme::scaled_px(22.))
                        .icon(IconName::RefreshCw)
                        .accessibility_label("载入选区")
                        .tooltip("重新提取并解码当前编辑器选区")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.inspector.auto_opened_by_selection = false;
                            this.sync_inspector_selection(window, cx);
                            cx.notify();
                        })),
                )
                .child(div().flex_1())
                .child(
                    Button::new("inspector-selection-apply")
                        .primary()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(theme::scaled_px(22.))
                        .px(px(8.))
                        .icon(IconName::Check)
                        .label("应用")
                        .tooltip("按原编码链逆向重编码并写回请求 Raw 选区")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.inspector.auto_opened_by_selection = false;
                            this.apply_inspector_selection(window, cx);
                        })),
                )
            })
            .into_any_element()
    }
}
