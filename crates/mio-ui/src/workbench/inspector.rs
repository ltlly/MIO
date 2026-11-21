use std::{cell::RefCell, rc::Rc};

use gpui_kit::component::input::EditorState;
use mio_core::{
    InspectorData, InspectorItem, InspectorMutation, InspectorSection, InspectorSelectionCodec,
    InterceptStage, apply_inspector_mutation, apply_inspector_response_mutation,
    decode_inspector_selection, encode_inspector_selection, inspect_http_messages,
};

use super::*;
use crate::editing::SelectionIdentity;

mod decode_ext;
mod render;
mod selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct MessageEditorRevKey {
    pub(in crate::workbench) state_id: gpui_kit::EntityId,
    pub(in crate::workbench) raw_id: gpui_kit::EntityId,
    pub(in crate::workbench) raw_rev: u64,
    pub(in crate::workbench) hex_rev: u64,
    pub(in crate::workbench) view: MessageView,
    pub(in crate::workbench) pretty_active: bool,
    pub(in crate::workbench) hide_headers: bool,
}

pub(in crate::workbench) fn message_editor_rev_key(
    ed: &MessageEditor,
    cx: &App,
) -> MessageEditorRevKey {
    MessageEditorRevKey {
        state_id: ed.state().entity_id(),
        raw_id: ed.raw.entity_id(),
        raw_rev: ed.raw.read(cx).document_revision(),
        hex_rev: ed.hex.read(cx).document_revision(),
        view: ed.view(),
        pretty_active: ed.pretty_active(),
        hide_headers: ed.hide_headers(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectorCacheKey {
    Empty,
    Editors {
        req: MessageEditorRevKey,
        resp: Option<MessageEditorRevKey>,
    },
    InterceptResponse {
        flow_id: FlowId,
        record_revision: u64,
        resp: MessageEditorRevKey,
    },
    IntruderEditor {
        editor_id: gpui_kit::EntityId,
        doc_rev: u64,
        view: MessageView,
        tab_id: TabId,
        draft_revision: u64,
    },
    IntruderDraft {
        tab_id: TabId,
        draft_revision: u64,
    },
    Collaborator {
        interaction_id: mio_core::Uuid,
        raw_len: usize,
    },
}

struct CachedInspectorEntry {
    key: InspectorCacheKey,
    data: Rc<InspectorData>,
    intruder_has_marker_char: bool,
}

type ParsedInspectorCache = RefCell<Option<CachedInspectorEntry>>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct InspectorOwnerKey {
    tool: Tool,
    proxy_page: ProxyPage,
    ext_import_visible: bool,
    ext_fragment: Option<(u64, mio_core::Uuid)>,
    active_repeater_tab: Option<TabId>,
    repeater_history_cursor: Option<mio_core::RepeaterHistoryCursor>,
    active_ws_tab: Option<TabId>,
    selected_history: Option<mio_core::HistoryEntryId>,
    history_original_view: bool,
    history_request_original_view: bool,
    history_response_original_view: bool,
    selected_ws_message: Option<mio_core::Uuid>,
    selected_intercept: Option<(mio_core::Uuid, Option<InterceptStage>)>,
    logger_selected: Option<FlowId>,
    target_selected: Option<mio_core::HistoryEntryId>,
    organiser_selected: Option<mio_core::Uuid>,
    dashboard_new_scan: bool,
    dashboard_finding: Option<mio_core::Uuid>,
    intruder_active: Option<TabId>,
    sequencer_page: sequencer::SequencerPage,
    collaborator_selected: Option<mio_core::Uuid>,
}

impl PendingIntercept {
    pub(in crate::workbench) fn is_response(&self) -> bool {
        matches!(
            self.target,
            InterceptTarget::Http {
                stage: InterceptStage::Response,
                ..
            }
        )
    }
}

pub(crate) struct InspectorPane {
    pub(crate) open: bool,
    pub(crate) auto_opened_by_selection: bool,
    pub(crate) dock_left: bool,
    pub(crate) tab_notes_open: bool,
    pub(crate) tab_notes_preview: bool,
    pub(crate) tab_notes_editor: Entity<EditorState>,
    pub(super) tab_notes_loaded_tab: Option<TabId>,
    pub(super) tab_notes_error: Option<String>,
    _tab_notes_sub: Subscription,
    pub(super) sections_scroll_handle: gpui_kit::ScrollHandle,
    pub(super) selection_identity: Option<SelectionIdentity>,
    pub(super) selection_preview: Option<decode_ext::InspectorSelectionPreview>,
    pub(super) last_observed_selection: Option<SelectionIdentity>,
    pub(super) last_surface_snapshot: Option<Vec<(SelectionIdentity, bool, u64)>>,
    pub(super) dismissed_selection: Option<SelectionIdentity>,
    last_owner_key: Option<InspectorOwnerKey>,
    pub(super) editor_open: bool,
    pub(super) detail_url_decoded: bool,
    expanded: [bool; 6],
    pub(crate) section_wrap: [bool; 6],
    pub(crate) selection_wrap: bool,
    selection_expanded: bool,
    editing_section: InspectorSection,
    editing_index: Option<usize>,
    name_input: Entity<InputState>,
    value_input: Entity<InputState>,
    selection_editor: Entity<EditorState>,
    selection_loaded: Option<(String, Vec<InspectorSelectionCodec>)>,
    pub(super) selection_manual_override: bool,
    parsed_cache: ParsedInspectorCache,
}

const MAX_TAB_NOTE_BYTES: usize = 64 * 1024;
const MAX_TAB_NOTES_COUNT: usize = 1024;

impl InspectorPane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let value_input = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        let selection_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .searchable(false)
                .line_number(false)
                .soft_wrap(false)
                .auto_close(false)
                .autocorrect(false)
        });
        let tab_notes_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .searchable(false)
                .line_number(false)
                .soft_wrap(true)
                .auto_close(false)
                .autocorrect(false)
        });
        let tab_notes_sub =
            cx.subscribe_in(&tab_notes_editor, window, |view, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    view.commit_active_repeater_tab_note(window, cx);
                }
            });
        Self {
            open: crate::theme::auto_expand_inspector(),
            auto_opened_by_selection: false,
            dock_left: false,
            tab_notes_open: false,
            tab_notes_preview: false,
            tab_notes_editor,
            tab_notes_loaded_tab: None,
            tab_notes_error: None,
            _tab_notes_sub: tab_notes_sub,
            sections_scroll_handle: gpui_kit::ScrollHandle::new(),
            selection_identity: None,
            selection_preview: None,
            last_observed_selection: None,
            last_surface_snapshot: None,
            dismissed_selection: None,
            last_owner_key: None,
            editor_open: false,
            detail_url_decoded: false,
            expanded: [false; 6],
            section_wrap: [false; 6],
            selection_wrap: false,
            selection_expanded: false,
            editing_section: InspectorSection::QueryParams,
            editing_index: None,
            name_input,
            value_input,
            selection_editor,
            selection_loaded: None,
            selection_manual_override: false,
            parsed_cache: RefCell::new(None),
        }
    }

    pub(in crate::workbench) fn clear_cache(&mut self) {
        self.editing_index = None;
        self.editor_open = false;
        self.detail_url_decoded = false;
        self.tab_notes_loaded_tab = None;
        self.tab_notes_error = None;
        self.selection_identity = None;
        self.selection_preview = None;
        self.last_observed_selection = None;
        self.last_surface_snapshot = None;
        self.dismissed_selection = None;
        self.last_owner_key = None;
        self.selection_loaded = None;
        self.selection_manual_override = false;
        *self.parsed_cache.borrow_mut() = None;
    }
}

fn section_index(section: InspectorSection) -> usize {
    match section {
        InspectorSection::RequestAttributes => 0,
        InspectorSection::QueryParams => 1,
        InspectorSection::BodyParams => 2,
        InspectorSection::Cookies => 3,
        InspectorSection::RequestHeaders => 4,
        InspectorSection::ResponseHeaders => 5,
    }
}

fn section_meta(
    section: InspectorSection,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
) {
    match section {
        InspectorSection::RequestAttributes => (
            "inspector-section-attributes",
            "Request attributes",
            "inspector-item-attributes",
            "inspector-remove-attributes",
            None,
        ),
        InspectorSection::QueryParams => (
            "inspector-section-query",
            "Request query parameters",
            "inspector-item-query",
            "inspector-remove-query",
            Some("inspector-target-query"),
        ),
        InspectorSection::BodyParams => (
            "inspector-section-body",
            "Request body parameters",
            "inspector-item-body",
            "inspector-remove-body",
            Some("inspector-target-body"),
        ),
        InspectorSection::Cookies => (
            "inspector-section-cookies",
            "Request cookies",
            "inspector-item-cookies",
            "inspector-remove-cookies",
            Some("inspector-target-cookies"),
        ),
        InspectorSection::RequestHeaders => (
            "inspector-section-request-headers",
            "Request headers",
            "inspector-item-request-headers",
            "inspector-remove-request-headers",
            Some("inspector-target-headers"),
        ),
        InspectorSection::ResponseHeaders => (
            "inspector-section-response-headers",
            "Response headers",
            "inspector-item-response-headers",
            "inspector-remove-response-headers",
            Some("inspector-target-response-headers"),
        ),
    }
}

fn section_singular_label(section: InspectorSection) -> &'static str {
    match section {
        InspectorSection::RequestAttributes => "Request attribute",
        InspectorSection::QueryParams => "Query parameter",
        InspectorSection::BodyParams => "Body parameter",
        InspectorSection::Cookies => "Request cookie",
        InspectorSection::RequestHeaders => "Request header",
        InspectorSection::ResponseHeaders => "Response header",
    }
}

fn editor_current_bytes(editor: &MessageEditor, cx: &App) -> Vec<u8> {
    editor.bytes_lossless(cx)
}

impl Workbench {
    pub(in crate::workbench) fn active_repeater_note_tab_id(&self) -> Option<TabId> {
        if self.ext_import.visible || self.tool != Tool::Repeater {
            return None;
        }
        if let Some(tab_id) = self.workspace.active
            && self.workspace.tabs.iter().any(|tab| tab.id == tab_id)
        {
            return Some(tab_id);
        }
        if self.workspace.active.is_none()
            && let Some(ws_id) = self.workspace.active_websocket_tab
            && self
                .workspace
                .websocket_tabs
                .iter()
                .any(|tab| tab.id == ws_id && !tab.closed)
        {
            return Some(ws_id);
        }
        None
    }

    pub(super) fn sync_repeater_tab_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_tab = self.active_repeater_note_tab_id();
        if self.inspector.tab_notes_loaded_tab == target_tab {
            return;
        }
        if self.inspector.tab_notes_editor.read(cx).has_marked_text() {
            return;
        }
        self.inspector.tab_notes_loaded_tab = target_tab;
        self.inspector.tab_notes_error = None;
        let note = target_tab
            .and_then(|tab_id| self.workspace.tab_notes.get(&tab_id))
            .cloned()
            .unwrap_or_default();
        self.inspector
            .tab_notes_editor
            .update(cx, |editor, cx| editor.set_value(note, window, cx));
    }

    pub(super) fn commit_active_repeater_tab_note(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.active_repeater_note_tab_id() else {
            return;
        };
        if self.inspector.tab_notes_loaded_tab != Some(tab_id) {
            return;
        }
        let Ok(snapshot) = editing::snapshot(&self.inspector.tab_notes_editor, window, cx) else {
            return;
        };
        let text = snapshot.to_string();
        if text.len() > MAX_TAB_NOTE_BYTES {
            self.inspector.tab_notes_error =
                Some("工作标签备注超过 64 KiB 上限，请精简内容后再保存。".into());
            cx.notify();
            return;
        }
        if text.is_empty() {
            self.inspector.tab_notes_error = None;
            if self.workspace.tab_notes.remove(&tab_id).is_some() {
                self.workspace_dirty = true;
                self.save_workspace(cx);
                cx.notify();
            }
            return;
        }
        if !self.workspace.tab_notes.contains_key(&tab_id)
            && self.workspace.tab_notes.len() >= MAX_TAB_NOTES_COUNT
        {
            self.inspector.tab_notes_error =
                Some("工作区标签备注数量已达 1024 条上限，请清理部分备注后再添加。".into());
            cx.notify();
            return;
        }
        self.inspector.tab_notes_error = None;
        if self.workspace.tab_notes.get(&tab_id) != Some(&text) {
            self.workspace.tab_notes.insert(tab_id, text);
            self.workspace_dirty = true;
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn validate_inspector_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        editing::snapshot(&self.inspector.selection_editor, window, cx)?;
        editing::snapshot(&self.inspector.tab_notes_editor, window, cx)?;
        editing::input_snapshot(&self.inspector.name_input, window, cx)?;
        editing::input_snapshot(&self.inspector.value_input, window, cx)?;
        Ok(())
    }

    pub(super) fn unfocus_inspector_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self
            .inspector
            .selection_editor
            .focus_handle(cx)
            .is_focused(window)
            || self
                .inspector
                .tab_notes_editor
                .focus_handle(cx)
                .is_focused(window)
            || self
                .inspector
                .name_input
                .focus_handle(cx)
                .is_focused(window)
            || self
                .inspector
                .value_input
                .focus_handle(cx)
                .is_focused(window);
        if focused {
            self.focus.focus(window, cx);
        }
    }

    pub(super) fn unfocus_inspector_detail_inputs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self
            .inspector
            .name_input
            .focus_handle(cx)
            .is_focused(window)
            || self
                .inspector
                .value_input
                .focus_handle(cx)
                .is_focused(window);
        if focused {
            self.focus.focus(window, cx);
        }
    }

    fn current_inspector_owner_key(&self) -> InspectorOwnerKey {
        let repeater_history_cursor = self
            .workspace
            .active
            .and_then(|id| self.workspace.tabs.iter().find(|tab| tab.id == id))
            .and_then(|tab| tab.history.as_ref())
            .and_then(|history| history.cursor.clone());
        let selected_intercept = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
            .map(|p| {
                let stage = match &p.target {
                    InterceptTarget::Http { stage, .. } => Some(*stage),
                    InterceptTarget::WebSocket { .. } => None,
                };
                (p.id(), stage)
            });
        InspectorOwnerKey {
            tool: self.tool,
            proxy_page: self.proxy_page,
            ext_import_visible: self.ext_import.visible && self.ext_import.browser_visible,
            ext_fragment: self.ext_import.selected_fragment_key(),
            active_repeater_tab: self.workspace.active,
            repeater_history_cursor,
            active_ws_tab: self.workspace.active_websocket_tab,
            selected_history: self.selected_history,
            history_original_view: self.history_original_view,
            history_request_original_view: self.history_request_original_view,
            history_response_original_view: self.history_response_original_view,
            selected_ws_message: self.websocket.selected,
            selected_intercept,
            logger_selected: self.extra_tools.logger_selected,
            target_selected: self
                .target_pane
                .selected_id
                .or_else(|| self.target_pane.selected_row.as_ref().map(HistoryRow::id)),
            organiser_selected: self.organiser.selected_id,
            dashboard_new_scan: self.audit_dashboard.new_scan_open,
            dashboard_finding: self.audit_dashboard.selected_finding_id,
            intruder_active: self.workspace.intruder.active,
            sequencer_page: self.sequencer_pane.page,
            collaborator_selected: self.collaborator_pane.selected_interaction,
        }
    }

    pub(in crate::workbench) fn open_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.ext_import.visible {
            if self.tool == Tool::Repeater && self.inspector.tab_notes_open {
                if let Err(error) = editing::snapshot(&self.inspector.tab_notes_editor, window, cx)
                {
                    self.fail(error, cx);
                    return false;
                }
                self.commit_active_repeater_tab_note(window, cx);
                let notes_focused = self
                    .inspector
                    .tab_notes_editor
                    .focus_handle(cx)
                    .is_focused(window);
                self.inspector.tab_notes_open = false;
                if notes_focused {
                    self.focus.focus(window, cx);
                }
            }
            if self.selected_http_notes().is_some() && self.http_notes.open {
                if !self.leave_http_notes(window, cx) {
                    return false;
                }
                let notes_focused = self.http_notes.editor.focus_handle(cx).is_focused(window);
                self.http_notes.open = false;
                if notes_focused {
                    self.focus.focus(window, cx);
                }
            }
            match self.tool {
                Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                    if self.websocket.selected.is_some() && self.websocket.notes.open {
                        if !self.leave_websocket_notes(window, cx) {
                            return false;
                        }
                        let notes_focused = self
                            .websocket
                            .notes
                            .editor
                            .focus_handle(cx)
                            .is_focused(window);
                        self.websocket.notes.open = false;
                        if notes_focused {
                            self.focus.focus(window, cx);
                        }
                    }
                }
                Tool::Organiser if self.organiser.notes_open => {
                    if let Err(error) = self.validate_organiser_composition(window, cx) {
                        self.fail(error, cx);
                        return false;
                    }
                    let notes_focused = self.organiser.notes.focus_handle(cx).is_focused(window);
                    self.organiser.notes_open = false;
                    if notes_focused {
                        self.focus.focus(window, cx);
                    }
                }
                _ => {}
            }
        }
        self.inspector.open = true;
        self.inspector.auto_opened_by_selection = false;
        self.inspector.dismissed_selection = None;
        self.sync_inspector_selection(window, cx);
        true
    }

    pub(in crate::workbench) fn dismiss_inspector_for_current_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.inspector.open
            && let Err(error) = self.validate_inspector_composition(window, cx)
        {
            self.fail(error, cx);
            return false;
        }
        self.unfocus_inspector_inputs(window, cx);
        self.inspector.editing_index = None;
        self.inspector.editor_open = false;
        self.inspector.detail_url_decoded = false;
        self.inspector.open = false;
        self.inspector.auto_opened_by_selection = false;
        self.inspector.dismissed_selection = self
            .current_inspector_selection(window, cx)
            .map(|c| c.identity)
            .or_else(|| self.inspector.selection_identity.clone());
        true
    }

    fn can_auto_open_over_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.ext_import.visible {
            return true;
        }
        if self.tool == Tool::Repeater && self.inspector.tab_notes_open {
            if editing::snapshot(&self.inspector.tab_notes_editor, window, cx).is_err() {
                return false;
            }
            self.commit_active_repeater_tab_note(window, cx);
            let notes_focused = self
                .inspector
                .tab_notes_editor
                .focus_handle(cx)
                .is_focused(window);
            self.inspector.tab_notes_open = false;
            if notes_focused {
                self.focus.focus(window, cx);
            }
        }
        if self.selected_http_notes().is_some() && self.http_notes.open {
            if self.capture_http_notes(window, cx).is_err() {
                return false;
            }
            self.send_http_notes(false, cx);
            let notes_focused = self.http_notes.editor.focus_handle(cx).is_focused(window);
            self.http_notes.open = false;
            if notes_focused {
                self.focus.focus(window, cx);
            }
        }
        match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                if self.websocket.selected.is_some() && self.websocket.notes.open {
                    if self.capture_websocket_notes(window, cx).is_err() {
                        return false;
                    }
                    self.send_websocket_notes(false, cx);
                    let notes_focused = self
                        .websocket
                        .notes
                        .editor
                        .focus_handle(cx)
                        .is_focused(window);
                    self.websocket.notes.open = false;
                    if notes_focused {
                        self.focus.focus(window, cx);
                    }
                }
                true
            }
            Tool::Organiser if self.organiser.notes_open => {
                if self.validate_organiser_composition(window, cx).is_err() {
                    return false;
                }
                let notes_focused = self.organiser.notes.focus_handle(cx).is_focused(window);
                self.organiser.notes_open = false;
                if notes_focused {
                    self.focus.focus(window, cx);
                }
                true
            }
            _ => true,
        }
    }

    pub(in crate::workbench) fn sync_inspector_auto_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ext_import.visible
            && self.tool == Tool::Repeater
            && self.workspace.repeater_settings.layout == mio_core::RepeaterLayoutMode::Combined
            && let Some(tab_id) = self.workspace.active
            && let Some(editors) = self.tab_editors.get(&tab_id)
        {
            let _ = self.sync_repeater_combined_state(tab_id, editors, window, cx);
        }
        self.sync_repeater_tab_notes(window, cx);
        if self.http_notes.open
            && let Some(id) = self.selected_http_notes()
            && self.http_notes.active != Some(id)
        {
            self.select_http_notes(window, cx);
        }
        let owner_key = self.current_inspector_owner_key();
        if self.inspector.last_owner_key != Some(owner_key.clone()) {
            if self.inspector.last_owner_key.is_some() {
                self.unfocus_inspector_detail_inputs(window, cx);
                self.inspector.editing_index = None;
                self.inspector.editor_open = false;
                self.inspector.detail_url_decoded = false;
            }
            self.inspector.last_owner_key = Some(owner_key);
            self.inspector.last_surface_snapshot = None;
        }
        let surface_probe = self.probe_inspector_selection_surfaces(window, cx);
        let (current_identity, any_selecting) = if let Some((snapshot, selecting)) =
            surface_probe.as_ref()
            && self.inspector.last_surface_snapshot.as_ref() == Some(snapshot)
            && self.inspector.selection_identity == self.inspector.last_observed_selection
        {
            (self.inspector.selection_identity.clone(), *selecting)
        } else {
            let candidate = self.current_inspector_selection(window, cx);
            let identity = candidate.as_ref().map(|c| c.identity.clone());
            let mut synced_ok = true;
            if identity != self.inspector.last_observed_selection {
                if self.inspector.dismissed_selection != identity {
                    self.inspector.dismissed_selection = None;
                }
                self.inspector.last_observed_selection = identity.clone();
                synced_ok = self.apply_inspector_selection_candidate(candidate, window, cx);
            } else if self.inspector.selection_identity != identity {
                synced_ok = self.apply_inspector_selection_candidate(candidate, window, cx);
            }
            self.inspector.last_surface_snapshot = if synced_ok {
                surface_probe.map(|(snapshot, _)| snapshot)
            } else {
                None
            };
            let selecting = self.any_mounted_editor_selecting(window, cx);
            (identity, selecting)
        };
        let has_selection = current_identity.is_some();
        if any_selecting {
            return;
        }
        if has_selection && self.tool != Tool::Intruder {
            if self.inspector.dismissed_selection != current_identity {
                if !self.inspector.open && self.can_auto_open_over_notes(window, cx) {
                    self.inspector.open = true;
                    self.inspector.auto_opened_by_selection = true;
                }
            }
        } else if !has_selection && self.inspector.auto_opened_by_selection {
            let inspector_busy = self
                .inspector
                .selection_editor
                .focus_handle(cx)
                .is_focused(window)
                || self
                    .inspector
                    .name_input
                    .focus_handle(cx)
                    .is_focused(window)
                || self
                    .inspector
                    .value_input
                    .focus_handle(cx)
                    .is_focused(window)
                || self.validate_inspector_composition(window, cx).is_err();
            if !crate::theme::auto_expand_inspector() && !inspector_busy {
                self.inspector.open = false;
                self.inspector.auto_opened_by_selection = false;
            }
        }
    }

    pub(super) fn inspector_supports_http_sections(&self) -> bool {
        if self.ext_import.visible {
            return false;
        }
        match self.tool {
            Tool::Repeater => {
                self.workspace.active.is_some()
                    || self.workspace.active_websocket_tab.is_some_and(|ws_id| {
                        self.workspace
                            .websocket_tabs
                            .iter()
                            .any(|tab| tab.id == ws_id && !tab.closed)
                    })
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .selected_intercept
                .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                .is_none_or(PendingIntercept::is_http),
            Tool::Proxy if self.proxy_page == ProxyPage::History => true,
            Tool::Logger
            | Tool::Organiser
            | Tool::Dashboard
            | Tool::Intruder
            | Tool::Collaborator => true,
            Tool::Sequencer => self.sequencer_pane.page == sequencer::SequencerPage::Live,
            Tool::Target => self.target_pane.page == target::TargetPage::SiteMap,
            _ => false,
        }
    }

    pub(super) fn is_websocket_repeater_inspector(&self) -> bool {
        !self.ext_import.visible
            && self.tool == Tool::Repeater
            && self.workspace.active.is_none()
            && self.workspace.active_websocket_tab.is_some_and(|ws_id| {
                self.workspace
                    .websocket_tabs
                    .iter()
                    .any(|tab| tab.id == ws_id && !tab.closed)
            })
    }

    pub(super) fn websocket_repeater_inspector_data(&self, cx: &App) -> Option<(Rc<InspectorData>, bool)> {
        if !self.is_websocket_repeater_inspector() {
            return None;
        }
        let ws_id = self.workspace.active_websocket_tab?;
        let tab = self
            .workspace
            .websocket_tabs
            .iter()
            .find(|tab| tab.id == ws_id && !tab.closed)?;
        let live_url = self
            .websocket_repeater
            .editors
            .get(&ws_id)
            .map(|ed| ed.url.read(cx).value().to_string())
            .unwrap_or_else(|| tab.url.clone());
        let editable = self.project.is_some()
            && self.backend_available()
            && self.transition.is_none()
            && self
                .websocket_repeater
                .editors
                .get(&ws_id)
                .is_some_and(|ed| ed.connecting.is_none() && !ed.disconnecting);
        let attributes = vec![InspectorItem {
            name: "URL".into(),
            value: live_url,
        }];
        let request_headers = tab
            .handshake_headers
            .iter()
            .map(|h| InspectorItem {
                name: h.name.clone(),
                value: String::from_utf8_lossy(&h.value).into_owned(),
            })
            .collect();
        Some((
            Rc::new(InspectorData {
                attributes,
                query_params: Vec::new(),
                body_params: Vec::new(),
                cookies: Vec::new(),
                request_headers,
                response_headers: Vec::new(),
            }),
            editable,
        ))
    }

    pub(super) fn inspector_context(&self, cx: &App) -> (Vec<u8>, Option<Vec<u8>>, bool, bool) {
        if self.ext_import.visible {
            return (Vec::new(), None, false, false);
        }
        match self.tool {
            Tool::Repeater => {
                let Some(tab_id) = self.workspace.active else {
                    if let Some((_, editable)) = self.websocket_repeater_inspector_data(cx) {
                        return (Vec::new(), None, editable, false);
                    }
                    return (Vec::new(), None, false, false);
                };
                let Some(editors) = self.tab_editors.get(&tab_id) else {
                    return (Vec::new(), None, false, false);
                };
                let req_bytes = editor_current_bytes(&editors.request, cx);
                let resp_bytes = editor_current_bytes(&editors.response, cx);
                let resp_opt = (!resp_bytes.is_empty()).then_some(resp_bytes);
                let busy = self.sending.contains_key(&tab_id)
                    || self.pending_sends.contains_key(&tab_id)
                    || self.plugin_waiting(tab_id)
                    || self.extensions.active_messages.contains_key(&tab_id);
                let editable = self.project.is_some()
                    && self.backend_available()
                    && self.transition.is_none()
                    && !busy;
                (req_bytes, resp_opt, editable, false)
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let Some(pending) = self
                    .selected_intercept
                    .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                else {
                    return (Vec::new(), None, false, false);
                };
                let InterceptTarget::Http { stage, record, .. } = &pending.target else {
                    return (Vec::new(), None, false, false);
                };
                let editor_bytes = editor_current_bytes(&pending.editor, cx);
                match stage {
                    InterceptStage::Request => {
                        let editable = self.project.is_some()
                            && self.backend_available()
                            && self.transition.is_none()
                            && !pending.deciding;
                        (editor_bytes, None, editable, false)
                    }
                    InterceptStage::Response => {
                        let req_bytes = request_to_raw(&record.request);
                        let resp_editable = self.project.is_some()
                            && self.backend_available()
                            && self.transition.is_none()
                            && pending.is_response()
                            && !pending.deciding;
                        (req_bytes, Some(editor_bytes), false, resp_editable)
                    }
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                let req_bytes = editor_current_bytes(&self.history_request, cx);
                let resp_bytes = editor_current_bytes(&self.history_response, cx);
                let resp_opt = (!resp_bytes.is_empty()).then_some(resp_bytes);
                (req_bytes, resp_opt, false, false)
            }
            Tool::Logger => {
                let req_bytes = editor_current_bytes(&self.extra_tools.logger_request, cx);
                let resp_bytes = editor_current_bytes(&self.extra_tools.logger_response, cx);
                let resp_opt = (!resp_bytes.is_empty()).then_some(resp_bytes);
                (req_bytes, resp_opt, false, false)
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => {
                let req_bytes = editor_current_bytes(&self.target_pane.request, cx);
                let resp_bytes = editor_current_bytes(&self.target_pane.response, cx);
                let resp_opt = (!resp_bytes.is_empty()).then_some(resp_bytes);
                (req_bytes, resp_opt, false, false)
            }
            Tool::Organiser => {
                let req_bytes = editor_current_bytes(&self.organiser.request, cx);
                let resp_bytes = editor_current_bytes(&self.organiser.response, cx);
                let resp_opt = (!resp_bytes.is_empty()).then_some(resp_bytes);
                (req_bytes, resp_opt, false, false)
            }
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open {
                    let req_bytes =
                        editor_current_bytes(&self.audit_dashboard.scan_request_editor, cx);
                    (req_bytes, None, false, false)
                } else {
                    let req_bytes =
                        editor_current_bytes(&self.audit_dashboard.issue_request_editor, cx);
                    let resp_bytes =
                        editor_current_bytes(&self.audit_dashboard.issue_response_editor, cx);
                    let resp_opt = (!resp_bytes.is_empty()).then_some(resp_bytes);
                    (req_bytes, resp_opt, false, false)
                }
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return (Vec::new(), None, false, false);
                };
                let Some(tab) = self.workspace.intruder.tabs.iter().find(|t| t.id == id) else {
                    return (Vec::new(), None, false, false);
                };
                if let Some(editors) = self.intruder.editors.get(&id) {
                    let content_text = editors.request.state().read(cx).value();
                    let clean = content_text.replace('§', "");
                    let live_bytes = match editors.request.view {
                        MessageView::Raw => clean.into_bytes(),
                        MessageView::Hex => editing::hex_decode(&clean)
                            .unwrap_or_else(|_| tab.spec.draft.raw.clone()),
                    };
                    let has_markers = editors.request.has_unplaced()
                        || !tab.spec.positions.is_empty()
                        || content_text.contains('§');
                    let editable = self.project.is_some()
                        && self.backend_available()
                        && self.transition.is_none()
                        && !has_markers;
                    return (live_bytes, None, editable, false);
                }
                (tab.spec.draft.raw.clone(), None, false, false)
            }
            Tool::Sequencer => {
                let req_bytes = editor_current_bytes(&self.sequencer_pane.request, cx);
                (req_bytes, None, false, false)
            }
            Tool::Collaborator => {
                let req_bytes = self
                    .collaborator_pane
                    .selected_interaction
                    .and_then(|id| {
                        self.collaborator_pane
                            .status
                            .interactions
                            .iter()
                            .find(|item| item.id == id && item.protocol == mio_core::OastProtocol::Http)
                    })
                    .map(|item| item.raw_request.clone())
                    .unwrap_or_default();
                (req_bytes, None, false, false)
            }
            _ => (Vec::new(), None, false, false),
        }
    }

    fn inspector_cache_key_and_editability(&self, cx: &App) -> (InspectorCacheKey, bool, bool) {
        if self.ext_import.visible {
            return (InspectorCacheKey::Empty, false, false);
        }
        match self.tool {
            Tool::Repeater => {
                let Some(tab_id) = self.workspace.active else {
                    if let Some((_, editable)) = self.websocket_repeater_inspector_data(cx) {
                        return (InspectorCacheKey::Empty, editable, false);
                    }
                    return (InspectorCacheKey::Empty, false, false);
                };
                let Some(editors) = self.tab_editors.get(&tab_id) else {
                    return (InspectorCacheKey::Empty, false, false);
                };
                let req_key = message_editor_rev_key(&editors.request, cx);
                let resp_key = message_editor_rev_key(&editors.response, cx);
                let busy = self.sending.contains_key(&tab_id)
                    || self.pending_sends.contains_key(&tab_id)
                    || self.plugin_waiting(tab_id)
                    || self.extensions.active_messages.contains_key(&tab_id);
                let editable = self.project.is_some()
                    && self.backend_available()
                    && self.transition.is_none()
                    && !busy;
                (
                    InspectorCacheKey::Editors {
                        req: req_key,
                        resp: Some(resp_key),
                    },
                    editable,
                    false,
                )
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let Some(pending) = self
                    .selected_intercept
                    .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                else {
                    return (InspectorCacheKey::Empty, false, false);
                };
                let InterceptTarget::Http { stage, record, .. } = &pending.target else {
                    return (InspectorCacheKey::Empty, false, false);
                };
                let editor_key = message_editor_rev_key(&pending.editor, cx);
                match stage {
                    InterceptStage::Request => {
                        let editable = self.project.is_some()
                            && self.backend_available()
                            && self.transition.is_none()
                            && !pending.deciding;
                        (
                            InspectorCacheKey::Editors {
                                req: editor_key,
                                resp: None,
                            },
                            editable,
                            false,
                        )
                    }
                    InterceptStage::Response => {
                        let resp_editable = self.project.is_some()
                            && self.backend_available()
                            && self.transition.is_none()
                            && pending.is_response()
                            && !pending.deciding;
                        (
                            InspectorCacheKey::InterceptResponse {
                                flow_id: record.summary.id,
                                record_revision: record.summary.revision,
                                resp: editor_key,
                            },
                            false,
                            resp_editable,
                        )
                    }
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => (
                InspectorCacheKey::Editors {
                    req: message_editor_rev_key(&self.history_request, cx),
                    resp: Some(message_editor_rev_key(&self.history_response, cx)),
                },
                false,
                false,
            ),
            Tool::Logger => (
                InspectorCacheKey::Editors {
                    req: message_editor_rev_key(&self.extra_tools.logger_request, cx),
                    resp: Some(message_editor_rev_key(&self.extra_tools.logger_response, cx)),
                },
                false,
                false,
            ),
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => (
                InspectorCacheKey::Editors {
                    req: message_editor_rev_key(&self.target_pane.request, cx),
                    resp: Some(message_editor_rev_key(&self.target_pane.response, cx)),
                },
                false,
                false,
            ),
            Tool::Organiser => (
                InspectorCacheKey::Editors {
                    req: message_editor_rev_key(&self.organiser.request, cx),
                    resp: Some(message_editor_rev_key(&self.organiser.response, cx)),
                },
                false,
                false,
            ),
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open {
                    (
                        InspectorCacheKey::Editors {
                            req: message_editor_rev_key(
                                &self.audit_dashboard.scan_request_editor,
                                cx,
                            ),
                            resp: None,
                        },
                        false,
                        false,
                    )
                } else {
                    (
                        InspectorCacheKey::Editors {
                            req: message_editor_rev_key(
                                &self.audit_dashboard.issue_request_editor,
                                cx,
                            ),
                            resp: Some(message_editor_rev_key(
                                &self.audit_dashboard.issue_response_editor,
                                cx,
                            )),
                        },
                        false,
                        false,
                    )
                }
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return (InspectorCacheKey::Empty, false, false);
                };
                let Some(tab) = self.workspace.intruder.tabs.iter().find(|t| t.id == id) else {
                    return (InspectorCacheKey::Empty, false, false);
                };
                if let Some(editors) = self.intruder.editors.get(&id) {
                    let state_entity = editors.request.state();
                    let doc_rev = state_entity.read(cx).document_revision();
                    let key = InspectorCacheKey::IntruderEditor {
                        editor_id: state_entity.entity_id(),
                        doc_rev,
                        view: editors.request.view,
                        tab_id: id,
                        draft_revision: tab.spec.draft.revision,
                    };
                    let cached_has_marker = self
                        .inspector
                        .parsed_cache
                        .borrow()
                        .as_ref()
                        .filter(|entry| entry.key == key)
                        .map(|entry| entry.intruder_has_marker_char);
                    let has_marker_char = cached_has_marker.unwrap_or_else(|| {
                        state_entity
                            .read(cx)
                            .text()
                            .chunks()
                            .any(|chunk| chunk.contains('§'))
                    });
                    let has_markers = editors.request.has_unplaced()
                        || !tab.spec.positions.is_empty()
                        || has_marker_char;
                    let editable = self.project.is_some()
                        && self.backend_available()
                        && self.transition.is_none()
                        && !has_markers;
                    return (key, editable, false);
                }
                (
                    InspectorCacheKey::IntruderDraft {
                        tab_id: id,
                        draft_revision: tab.spec.draft.revision,
                    },
                    false,
                    false,
                )
            }
            Tool::Sequencer => (
                InspectorCacheKey::Editors {
                    req: message_editor_rev_key(&self.sequencer_pane.request, cx),
                    resp: None,
                },
                false,
                false,
            ),
            Tool::Collaborator => {
                let key = self
                    .collaborator_pane
                    .selected_interaction
                    .and_then(|id| {
                        self.collaborator_pane
                            .status
                            .interactions
                            .iter()
                            .find(|item| {
                                item.id == id && item.protocol == mio_core::OastProtocol::Http
                            })
                    })
                    .map_or(InspectorCacheKey::Empty, |item| {
                        InspectorCacheKey::Collaborator {
                            interaction_id: item.id,
                            raw_len: item.raw_request.len(),
                        }
                    });
                (key, false, false)
            }
            _ => (InspectorCacheKey::Empty, false, false),
        }
    }

    pub(super) fn inspect_current_http(&self, cx: &App) -> (Rc<InspectorData>, bool, bool) {
        let (key, req_editable, resp_editable) = self.inspector_cache_key_and_editability(cx);
        let cached = {
            let guard = self.inspector.parsed_cache.borrow();
            guard
                .as_ref()
                .filter(|entry| entry.key == key)
                .map(|entry| entry.data.clone())
        };
        if let Some(data) = cached {
            return (data, req_editable, resp_editable);
        }
        let (req_bytes, resp_bytes, _, _) = self.inspector_context(cx);
        let intruder_has_marker_char = matches!(key, InspectorCacheKey::IntruderEditor { .. })
            && self
                .workspace
                .intruder
                .active
                .and_then(|id| self.intruder.editors.get(&id))
                .is_some_and(|editors| {
                    editors
                        .request
                        .state()
                        .read(cx)
                        .text()
                        .chunks()
                        .any(|chunk| chunk.contains('§'))
                });
        let parsed = Rc::new(inspect_http_messages(&req_bytes, resp_bytes.as_deref()));
        *self.inspector.parsed_cache.borrow_mut() = Some(CachedInspectorEntry {
            key,
            data: parsed.clone(),
            intruder_has_marker_char,
        });
        (parsed, req_editable, resp_editable)
    }

    pub(super) fn effective_inspector_section(&self, cx: &App) -> InspectorSection {
        let (_, req_editable, resp_editable) = self.inspector_cache_key_and_editability(cx);
        self.effective_inspector_section_for(req_editable, resp_editable)
    }

    pub(super) fn effective_inspector_section_for(
        &self,
        req_editable: bool,
        resp_editable: bool,
    ) -> InspectorSection {
        if self.is_websocket_repeater_inspector() {
            return match self.inspector.editing_section {
                InspectorSection::RequestAttributes | InspectorSection::RequestHeaders => {
                    self.inspector.editing_section
                }
                _ => InspectorSection::RequestHeaders,
            };
        }
        if resp_editable && !req_editable {
            InspectorSection::ResponseHeaders
        } else if req_editable
            && self.inspector.editing_section == InspectorSection::ResponseHeaders
        {
            InspectorSection::QueryParams
        } else {
            self.inspector.editing_section
        }
    }

    fn inspector_editable_request_bytes(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<u8>, String> {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some() {
            return Err("当前项目状态暂不允许通过 Inspector 修改请求。".into());
        }
        match self.tool {
            Tool::Repeater => {
                let Some(tab_id) = self.workspace.active else {
                    return Err("当前没有活动的 Repeater 请求。".into());
                };
                if self.sending.contains_key(&tab_id)
                    || self.pending_sends.contains_key(&tab_id)
                    || self.plugin_waiting(tab_id)
                    || self.extensions.active_messages.contains_key(&tab_id)
                {
                    return Err("当前请求正在发送或由插件接管，暂不能通过 Inspector 修改。".into());
                }
                let Some(editors) = self.tab_editors.get(&tab_id) else {
                    return Err("当前没有活动的 Repeater 编辑器。".into());
                };
                editors.request.bytes(window, cx)
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let Some(pending) = self
                    .selected_intercept
                    .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                else {
                    return Err("当前没有选中的拦截消息。".into());
                };
                if !matches!(
                    pending.target,
                    InterceptTarget::Http {
                        stage: InterceptStage::Request,
                        ..
                    }
                ) || pending.deciding
                {
                    return Err("当前拦截消息不可通过 Inspector 修改请求。".into());
                }
                pending.editor.bytes(window, cx)
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return Err("当前没有活动的 Intruder 请求模板。".into());
                };
                let Some(tab) = self.workspace.intruder.tabs.iter().find(|t| t.id == id) else {
                    return Err("当前没有活动的 Intruder 请求模板。".into());
                };
                let Some(editors) = self.intruder.editors.get(&id) else {
                    return Err("当前没有活动的 Intruder 编辑器。".into());
                };
                if editors.request.has_unplaced()
                    || !tab.spec.positions.is_empty()
                    || editors.request.state().read(cx).value().contains('§')
                {
                    return Err("当前 Intruder 模板含有 § 载荷位置标记，请先清除位置标记后再通过 Inspector 修改。".into());
                }
                let content = editors.request.state().update(cx, |state, cx| {
                    if state.marked_text_range(window, cx).is_some() {
                        return Err(editing::COMPOSITION_PENDING.to_owned());
                    }
                    Ok(state.value().to_string())
                })?;
                match editors.request.view {
                    MessageView::Raw => Ok(content.into_bytes()),
                    MessageView::Hex => editing::hex_decode(&content),
                }
            }
            _ => Err("当前消息为只读，不能通过 Inspector 修改。".into()),
        }
    }

    fn inspector_editable_response_bytes(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<u8>, String> {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some() {
            return Err("当前项目状态暂不允许通过 Inspector 修改响应。".into());
        }
        if self.tool != Tool::Proxy || self.proxy_page != ProxyPage::Intercept {
            return Err("响应标头仅支持在 Proxy 响应拦截中修改。".into());
        }
        let Some(pending) = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
        else {
            return Err("当前没有选中的拦截消息。".into());
        };
        if !pending.is_response() || pending.deciding {
            return Err("当前拦截消息不可通过 Inspector 修改响应标头。".into());
        }
        pending.editor.bytes(window, cx)
    }

    pub(in crate::workbench) fn apply_inspector_request_bytes(
        &mut self,
        next_raw: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.tool {
            Tool::Repeater => {
                let Some(tab_id) = self.workspace.active else {
                    return;
                };
                let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                let Some(next_revision) = tab.draft.revision.checked_add(1) else {
                    self.fail("草稿 revision 已耗尽；无法应用 Inspector 修改。", cx);
                    return;
                };
                if let Some(detected) = mio_core::detect_request_line_version(&next_raw) {
                    tab.draft.version = detected;
                }
                tab.draft.raw = next_raw.clone();
                tab.draft.revision = next_revision;
                if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                    let was_pretty = editors.request.pretty_active();
                    let prev_view = editors.request.view();
                    editors.request.load(&next_raw, window, cx);
                    if was_pretty {
                        let _ = editors.request.switch_pretty(window, cx);
                    } else {
                        let _ = editors.request.switch(prev_view, window, cx);
                    }
                    editors.loaded_revision = next_revision;
                    editors.dirty = false;
                }
                self.save_workspace(cx);
                cx.notify();
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let Some(id) = self.selected_intercept else {
                    return;
                };
                if let Some(pending) = self
                    .intercepts
                    .iter_mut()
                    .find(|p| p.id() == id && p.is_http())
                {
                    let was_pretty = pending.editor.pretty_active();
                    let prev_view = pending.editor.view();
                    pending.editor.load(&next_raw, window, cx);
                    if was_pretty {
                        let _ = pending.editor.switch_pretty(window, cx);
                    } else {
                        let _ = pending.editor.switch(prev_view, window, cx);
                    }
                }
                cx.notify();
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return;
                };
                let _ = self.change_intruder_spec(
                    id,
                    move |spec| {
                        spec.draft.raw = next_raw;
                    },
                    window,
                    cx,
                );
            }
            _ => {}
        }
    }

    fn handle_websocket_repeater_inspector_mutation(
        &mut self,
        mutation: InspectorMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some() {
            self.fail("当前项目状态暂不允许通过 Inspector 修改 WebSocket 握手配置。", cx);
            return false;
        }
        let Some(ws_id) = self.workspace.active_websocket_tab else {
            self.fail("当前没有活动的 WebSocket Repeater 标签。", cx);
            return false;
        };
        if self
            .websocket_repeater
            .editors
            .get(&ws_id)
            .is_some_and(|ed| ed.connecting.is_some() || ed.disconnecting)
        {
            self.fail("当前 WebSocket 标签正在连接或断开，暂不能修改握手配置。", cx);
            return false;
        }
        if let Err(err) = self.capture_drafts(window, cx) {
            self.fail(err, cx);
            return false;
        }
        let Some(tab) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|t| t.id == ws_id && !t.closed)
        else {
            return false;
        };
        let mut next_tab = tab.clone();
        match mutation {
            InspectorMutation::Update {
                section: InspectorSection::RequestAttributes,
                index: 0,
                value,
                ..
            } => {
                let trimmed = value.trim().to_owned();
                if trimmed.is_empty() {
                    self.fail("WebSocket 目标 URL 不能为空。", cx);
                    return false;
                }
                next_tab.url = trimmed;
            }
            InspectorMutation::Add {
                section: InspectorSection::RequestHeaders,
                name,
                value,
            } => {
                if value.contains(['\r', '\n', '\0']) || name.contains(['\r', '\n', '\0']) {
                    self.fail("握手标头名称或值不能包含换行符或空字符。", cx);
                    return false;
                }
                next_tab.handshake_headers.push(mio_core::Header {
                    name: name.trim().to_owned(),
                    value: value.into_bytes(),
                });
            }
            InspectorMutation::Update {
                section: InspectorSection::RequestHeaders,
                index,
                name,
                value,
            } => {
                if value.contains(['\r', '\n', '\0']) || name.contains(['\r', '\n', '\0']) {
                    self.fail("握手标头名称或值不能包含换行符或空字符。", cx);
                    return false;
                }
                let Some(header) = next_tab.handshake_headers.get_mut(index) else {
                    self.fail("握手标头索引超出范围。", cx);
                    return false;
                };
                header.name = name.trim().to_owned();
                header.value = value.into_bytes();
            }
            InspectorMutation::Remove {
                section: InspectorSection::RequestHeaders,
                index,
            } => {
                if index >= next_tab.handshake_headers.len() {
                    self.fail("握手标头索引超出范围。", cx);
                    return false;
                }
                next_tab.handshake_headers.remove(index);
            }
            InspectorMutation::Move {
                section: InspectorSection::RequestHeaders,
                from,
                to,
            } => {
                if from >= next_tab.handshake_headers.len() || to >= next_tab.handshake_headers.len() {
                    self.fail("握手标头索引超出范围。", cx);
                    return false;
                }
                if from != to {
                    let moved = next_tab.handshake_headers.remove(from);
                    next_tab.handshake_headers.insert(to, moved);
                }
            }
            _ => {
                self.fail("WebSocket 标签仅支持修改目标 URL 与握手标头。", cx);
                return false;
            }
        }
        let Some(next_rev) = next_tab.draft_revision.checked_add(1) else {
            self.fail("WebSocket 草稿 revision 已耗尽。", cx);
            return false;
        };
        next_tab.draft_revision = next_rev;
        if let Err(err) = next_tab.validate() {
            self.fail(err.to_string(), cx);
            return false;
        }
        let updated_url = next_tab.url.clone();
        if let Some(target) = self
            .workspace
            .websocket_tabs
            .iter_mut()
            .find(|t| t.id == ws_id && !t.closed)
        {
            *target = next_tab;
        }
        if let Some(editors) = self.websocket_repeater.editors.get_mut(&ws_id) {
            editors
                .url
                .update(cx, |s, cx| s.set_value(updated_url, window, cx));
            editors.loaded_revision = next_rev;
            editors.dirty = false;
        }
        self.save_workspace(cx);
        cx.notify();
        true
    }

    pub(super) fn handle_inspector_mutation(
        &mut self,
        mutation: InspectorMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_websocket_repeater_inspector() {
            return self.handle_websocket_repeater_inspector_mutation(mutation, window, cx);
        }
        let section = match &mutation {
            InspectorMutation::Add { section, .. }
            | InspectorMutation::Update { section, .. }
            | InspectorMutation::Remove { section, .. }
            | InspectorMutation::Move { section, .. } => *section,
        };
        if section == InspectorSection::ResponseHeaders {
            let response_raw = match self.inspector_editable_response_bytes(window, cx) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.fail(error, cx);
                    return false;
                }
            };
            return match apply_inspector_response_mutation(&response_raw, mutation) {
                Ok(next_raw) => {
                    self.apply_inspector_request_bytes(next_raw, window, cx);
                    true
                }
                Err(error) => {
                    self.fail(error, cx);
                    false
                }
            };
        }
        let request_raw = match self.inspector_editable_request_bytes(window, cx) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return false;
            }
        };
        match apply_inspector_mutation(&request_raw, mutation) {
            Ok(next_raw) => {
                self.apply_inspector_request_bytes(next_raw, window, cx);
                true
            }
            Err(error) => {
                self.fail(error, cx);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_cyberchef;
