use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::Button,
    checkbox::Checkbox,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    progress::Progress,
};
use gpui_kit::{
    AnyElement, App, AppContext, Bounds, Context, DispatchPhase, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, Modifiers, MouseButton,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, RenderImage, ScrollHandle, ScrollStrategy, Subscription, Task, TestSupportExt, UniformListScrollHandle, Window,
    accesskit::Role, canvas, div, prelude::*, px, relative, uniform_list,
};
use mio_core::panel::{
    self, Axis, EditorFormat, EditorSnapshot, Empty, KeyEvent, Node, PanelSnapshot, PointerEvent,
    PointerPhase, TableSelection, UiEvent, ViewportRequest, node, ui_event,
};

use crate::{editing, theme};

mod canvas;
mod events;
mod render;
mod snapshot;
mod table;
#[cfg(test)]
mod tests;

pub(crate) enum PanelEvent {
    Ui {
        revision: u64,
        seq: u64,
        event: UiEvent,
    },
    Edited,
    Error(String),
}

pub(in crate::plugin_panel) struct Document {
    node_id: u64,
    label: String,
    state: Entity<EditorState>,
    snapshot: EditorSnapshot,
    format: EditorFormat,
    read_only: bool,
    live: bool,
    unsaved: bool,
    pending_rebase: bool,
    selection_seq: Rc<Cell<u64>>,
    _subscription: Subscription,
    _selection_subscription: Subscription,
}

impl Document {
    pub(in crate::plugin_panel) fn retains_local(
        &self,
        editor: &panel::Editor,
        pending_input: bool,
        editor_bases: &[EditorSnapshot],
    ) -> bool {
        let same = editor.contents == self.snapshot.contents && editor.format == self.format as i32;
        pending_input
            || (self.unsaved
                && !same
                && !editor_bases.iter().any(|base| {
                    base.document_id == self.snapshot.document_id
                        && base.revision == self.snapshot.revision
                        && base.contents == self.snapshot.contents
                }))
    }
}

pub(in crate::plugin_panel) const MAX_UI_OVERLAY_HISTORY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::plugin_panel) enum ControlOverlay {
    Tab(u64),
    Split(f32),
    Checkbox(bool),
    Select(u32),
}

fn overlay_matches_nodes(old_node: &Node, new_node: &Node, overlay: ControlOverlay) -> bool {
    match (overlay, &old_node.kind, &new_node.kind) {
        (
            ControlOverlay::Tab(tab_id),
            Some(node::Kind::Tabs(old_t)),
            Some(node::Kind::Tabs(new_t)),
        ) => old_t
            .tabs
            .iter()
            .find_map(|t| {
                t.content
                    .as_ref()
                    .is_some_and(|c| c.id == tab_id)
                    .then_some(t.title.as_str())
            })
            .is_some_and(|old_title| {
                new_t.tabs.iter().any(|t| {
                    t.content.as_ref().is_some_and(|c| c.id == tab_id) && t.title == old_title
                })
            }),
        (
            ControlOverlay::Split(_),
            Some(node::Kind::Split(old_s)),
            Some(node::Kind::Split(new_s)),
        ) => old_s.axis == new_s.axis,
        (
            ControlOverlay::Checkbox(_),
            Some(node::Kind::Checkbox(old_c)),
            Some(node::Kind::Checkbox(new_c)),
        ) => new_c.enabled && old_c.label == new_c.label,
        (
            ControlOverlay::Select(idx),
            Some(node::Kind::Select(old_s)),
            Some(node::Kind::Select(new_s)),
        ) => {
            new_s.enabled
                && old_s.label == new_s.label
                && new_s
                    .options
                    .get(idx as usize)
                    .is_some_and(|opt| old_s.options.get(idx as usize) == Some(opt))
        }
        _ => false,
    }
}

#[derive(Default)]
pub(in crate::plugin_panel) struct OverlayStore {
    entries: HashMap<u64, VecDeque<(ControlOverlay, u64)>>,
}

impl OverlayStore {
    #[cfg(test)]
    pub(in crate::plugin_panel) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(in crate::plugin_panel) fn push(&mut self, node_id: u64, value: ControlOverlay, seq: u64) {
        let history = self.entries.entry(node_id).or_default();
        if history.len() >= MAX_UI_OVERLAY_HISTORY {
            history.pop_front();
        }
        history.push_back((value, seq));
    }

    fn latest(&self, node_id: u64) -> Option<ControlOverlay> {
        self.entries
            .get(&node_id)
            .and_then(|history| history.back())
            .map(|&(value, _)| value)
    }

    pub(in crate::plugin_panel) fn latest_tab(&self, node_id: u64) -> Option<u64> {
        match self.latest(node_id) {
            Some(ControlOverlay::Tab(tab_id)) => Some(tab_id),
            _ => None,
        }
    }

    pub(in crate::plugin_panel) fn latest_split(&self, node_id: u64) -> Option<f32> {
        match self.latest(node_id) {
            Some(ControlOverlay::Split(fraction)) => Some(fraction),
            _ => None,
        }
    }

    pub(in crate::plugin_panel) fn latest_checkbox(&self, node_id: u64) -> Option<bool> {
        match self.latest(node_id) {
            Some(ControlOverlay::Checkbox(checked)) => Some(checked),
            _ => None,
        }
    }

    pub(in crate::plugin_panel) fn latest_select(&self, node_id: u64) -> Option<u32> {
        match self.latest(node_id) {
            Some(ControlOverlay::Select(idx)) => Some(idx),
            _ => None,
        }
    }

    pub(in crate::plugin_panel) fn remove_for_seq(
        &mut self,
        node_id: u64,
        failed_seq: Option<u64>,
    ) {
        match failed_seq {
            None => {
                self.entries.remove(&node_id);
            }
            Some(target_seq) => {
                if let Some(history) = self.entries.get_mut(&node_id) {
                    history.retain(|&(_, seq)| seq != target_seq);
                    if history.is_empty() {
                        self.entries.remove(&node_id);
                    }
                }
            }
        }
    }

    pub(in crate::plugin_panel) fn prune(&mut self, sync_cutoff: u64, rejected_ui_seqs: &[u64]) {
        self.entries.retain(|_, history| {
            history.retain(|&(_, seq)| seq > sync_cutoff && !rejected_ui_seqs.contains(&seq));
            !history.is_empty()
        });
    }

    pub(in crate::plugin_panel) fn retain_semantic(
        &mut self,
        sync_cutoff: u64,
        rejected_ui_seqs: &[u64],
        old_root: Option<&Node>,
        new_root: Option<&Node>,
    ) {
        self.entries.retain(|&node_id, history| {
            let Some((old_node, new_node)) = old_root
                .and_then(|r| r.find(node_id))
                .zip(new_root.and_then(|r| r.find(node_id)))
            else {
                return false;
            };
            history.retain(|&(overlay, seq)| {
                seq > sync_cutoff
                    && !rejected_ui_seqs.contains(&seq)
                    && overlay_matches_nodes(old_node, new_node, overlay)
            });
            !history.is_empty()
        });
    }

    #[cfg(test)]
    pub(in crate::plugin_panel) fn contains_key(&self, node_id: u64) -> bool {
        self.entries.contains_key(&node_id)
    }

    #[cfg(test)]
    pub(in crate::plugin_panel) fn entries_for(&self, node_id: u64) -> Vec<(ControlOverlay, u64)> {
        self.entries
            .get(&node_id)
            .map(|history| history.iter().copied().collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingViewport {
    revision: u64,
    offset: u64,
    seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LoadingViewport {
    revision: u64,
    offset: u64,
    seq: u64,
}

pub(in crate::plugin_panel) struct TableViewportTracker {
    next_revision: u64,
    confirmed_revision: u64,
    pending: VecDeque<PendingViewport>,
    loading: Option<LoadingViewport>,
}

impl TableViewportTracker {
    pub(in crate::plugin_panel) fn new(initial_revision: u64) -> Self {
        Self {
            next_revision: initial_revision,
            confirmed_revision: initial_revision,
            pending: VecDeque::new(),
            loading: None,
        }
    }

    #[cfg(test)]
    pub(in crate::plugin_panel) fn next_revision(&self) -> u64 {
        self.next_revision
    }

    pub(in crate::plugin_panel) fn active_revision(&self) -> u64 {
        self.pending
            .back()
            .map_or(self.confirmed_revision, |p| p.revision)
    }

    #[cfg(test)]
    pub(in crate::plugin_panel) fn active_seq(&self) -> u64 {
        self.pending.back().map_or(0, |p| p.seq)
    }

    pub(in crate::plugin_panel) fn requested_offset(&self) -> Option<u64> {
        self.pending
            .back()
            .map(|p| p.offset)
            .or_else(|| self.loading.map(|l| l.offset))
    }

    pub(in crate::plugin_panel) fn request_page(&mut self, offset: u64, seq: u64) -> u64 {
        self.next_revision += 1;
        let revision = self.next_revision;
        if self.pending.len() >= MAX_UI_OVERLAY_HISTORY {
            self.pending.pop_front();
        }
        self.pending.push_back(PendingViewport {
            revision,
            offset,
            seq,
        });
        self.loading = None;
        revision
    }

    pub(in crate::plugin_panel) fn fail_request(
        &mut self,
        failed_seq: Option<u64>,
        fallback_confirmed: Option<u64>,
    ) {
        if let Some(target_seq) = failed_seq {
            let before_len = self.pending.len();
            self.pending.retain(|p| p.seq != target_seq);
            if self.pending.len() != before_len
                || self.loading.is_some_and(|l| l.seq == target_seq)
            {
                self.loading = None;
            }
        } else {
            self.pending.clear();
            self.loading = None;
            if let Some(vp_rev) = fallback_confirmed {
                self.confirmed_revision = vp_rev;
            }
        }
    }

    pub(in crate::plugin_panel) fn reconcile(
        &mut self,
        table: &panel::VirtualTable,
        newer_revision: bool,
        sync_cutoff: u64,
        rejected_ui_seqs: &[u64],
    ) {
        let prev_target = self
            .pending
            .back()
            .copied()
            .map(|p| (p.revision, p.offset, p.seq, false))
            .or_else(|| self.loading.map(|l| (l.revision, l.offset, l.seq, true)));
        self.next_revision = self.next_revision.max(table.viewport_revision);
        self.confirmed_revision = table.viewport_revision;
        self.pending.retain(|p| {
            p.revision > table.viewport_revision
                && (p.offset == 0 || p.offset < table.total_rows)
                && p.seq > sync_cutoff
                && !rejected_ui_seqs.contains(&p.seq)
        });
        if !self.pending.is_empty() {
            self.loading = None;
        } else if let Some((req_rev, req_offset, req_seq, already_loading)) = prev_target {
            let target_arrived = table.offset == req_offset;
            let out_of_range = req_offset > 0 && req_offset >= table.total_rows;
            let rejected = req_seq != 0 && rejected_ui_seqs.contains(&req_seq);
            let worker_published_loading = already_loading || newer_revision;
            if worker_published_loading
                && table.viewport_revision == req_rev
                && !target_arrived
                && !out_of_range
                && !rejected
            {
                self.loading = Some(LoadingViewport {
                    revision: req_rev,
                    offset: req_offset,
                    seq: req_seq,
                });
            } else {
                self.loading = None;
            }
        }
    }
}

pub(in crate::plugin_panel) struct Table {
    filter: Entity<InputState>,
    selected: Vec<String>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    horizontal: ScrollHandle,
    viewport: TableViewportTracker,
    _subscription: Subscription,
    _focus_subscriptions: Vec<Subscription>,
}

impl Table {
    pub(in crate::plugin_panel) fn active_viewport_revision(&self) -> u64 {
        self.viewport.active_revision()
    }

    pub(in crate::plugin_panel) fn requested_offset(&self) -> Option<u64> {
        self.viewport.requested_offset()
    }

    #[cfg(test)]
    pub(in crate::plugin_panel) fn active_viewport_seq(&self) -> u64 {
        self.viewport.active_seq()
    }
}

pub(in crate::plugin_panel) struct Drawing {
    model: panel::Canvas,
    prepared: Option<Arc<canvas::PreparedCanvas>>,
    images: Arc<Vec<Arc<RenderImage>>>,
    error: Option<String>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: HashMap<u64, CanvasFocus>,
    dirty: bool,
    pixel_budget: usize,
}

pub(in crate::plugin_panel) struct CanvasFocus {
    handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
pub(in crate::plugin_panel) struct Capture {
    node_id: u64,
    hit_id: u64,
    position: (f32, f32),
}

#[derive(Clone, Copy)]
pub(in crate::plugin_panel) struct Resize {
    node_id: u64,
    original: f32,
}

pub(crate) struct NativePanel {
    snapshot: Arc<PanelSnapshot>,
    documents: HashMap<u64, Document>,
    tables: HashMap<u64, Table>,
    drawings: HashMap<u64, Drawing>,
    geometry: HashMap<u64, Rc<Cell<Bounds<Pixels>>>>,
    overlays: OverlayStore,
    next_ui_seq: u64,
    focused_document: Option<u64>,
    capture: Option<Capture>,
    resize: Option<Resize>,
    recovery: Option<u64>,
    enabled: bool,
    error: Option<String>,
    image_task: Option<Task<()>>,
}

impl EventEmitter<PanelEvent> for NativePanel {}

impl NativePanel {
    pub(crate) fn new(
        snapshot: Arc<PanelSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, String> {
        snapshot.validate()?;
        let mut panel = Self {
            snapshot: snapshot.clone(),
            documents: HashMap::new(),
            tables: HashMap::new(),
            drawings: HashMap::new(),
            geometry: HashMap::new(),
            overlays: OverlayStore::default(),
            next_ui_seq: 0,
            focused_document: None,
            capture: None,
            resize: None,
            recovery: None,
            enabled: true,
            error: None,
            image_task: None,
        };
        panel.replace(snapshot, &[], window, cx)?;
        Ok(panel)
    }

    pub(crate) fn title(&self) -> &str {
        &self.snapshot.title
    }

    pub(crate) fn commit_snapshot(&self) -> Arc<PanelSnapshot> {
        if self.overlays.entries.is_empty() && self.tables.is_empty() {
            return self.snapshot.clone();
        }
        let mut snapshot = (*self.snapshot).clone();
        if let Some(root) = snapshot.root.as_mut() {
            self.apply_commit_overlays(root);
        }
        Arc::new(snapshot)
    }

    fn apply_commit_overlays(&self, node: &mut mio_core::panel::Node) {
        match &mut node.kind {
            Some(node::Kind::Row(c) | node::Kind::Column(c) | node::Kind::ScrollColumn(c)) => {
                for child in &mut c.children {
                    self.apply_commit_overlays(child);
                }
            }
            Some(node::Kind::Split(s)) => {
                if let Some(fraction) = self.overlays.latest_split(node.id) {
                    s.fraction = fraction;
                }
                if let Some(first) = s.first.as_deref_mut() {
                    self.apply_commit_overlays(first);
                }
                if let Some(second) = s.second.as_deref_mut() {
                    self.apply_commit_overlays(second);
                }
            }
            Some(node::Kind::Tabs(t)) => {
                if let Some(active) = self.overlays.latest_tab(node.id) {
                    t.active = active;
                }
                for tab in &mut t.tabs {
                    if let Some(content) = tab.content.as_deref_mut() {
                        self.apply_commit_overlays(content);
                    }
                }
            }
            Some(node::Kind::VirtualTable(t)) => {
                if let Some(table) = self.tables.get(&node.id) {
                    t.viewport_revision = table.active_viewport_revision();
                }
            }
            Some(node::Kind::Checkbox(c)) => {
                if let Some(checked) = self.overlays.latest_checkbox(node.id) {
                    c.checked = checked;
                }
            }
            Some(node::Kind::Select(s)) => {
                if let Some(selected) = self.overlays.latest_select(node.id) {
                    s.selected_index = selected;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn has_unsaved_edits(&self, cx: &App) -> bool {
        self.documents.values().any(|document| {
            document.unsaved
                || document.state.read(cx).value().as_ref()
                    != display_bytes(&document.snapshot.contents, document.format)
        })
    }

    pub(crate) fn writable_documents(&self) -> Vec<(u64, String)> {
        let mut result = Vec::new();
        if let Some(root) = &self.snapshot.root {
            let mut editors = Vec::new();
            root.editors(&mut editors);
            for (_, editor) in editors {
                if let Some(document) = self.documents.get(&editor.document_id)
                    && document.live
                    && !document.read_only
                {
                    result.push((editor.document_id, document.label.clone()));
                }
            }
        }
        result
    }

    pub(crate) fn is_selecting(&self, cx: &App) -> bool {
        let mut visible = Vec::new();
        if let Some(root) = &self.snapshot.root {
            self.visible_all_documents(root, &mut visible);
        }
        visible.into_iter().any(|id| {
            self.documents
                .get(&id)
                .is_some_and(|doc| doc.live && doc.state.read(cx).is_selecting())
        })
    }

    pub(crate) fn selection_candidate(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<editing::SelectionCandidate> {
        let root = self.snapshot.root.as_ref()?;
        let mut visible = Vec::new();
        self.visible_all_documents(root, &mut visible);
        let extract = |id: u64| -> Option<editing::SelectionCandidate> {
            let doc = self.documents.get(&id)?;
            if !doc.live {
                return None;
            }
            let state = doc.state.read(cx);
            let range = state.selected_range();
            if range.is_empty() {
                return None;
            }
            let selected = state.selected_value();
            if selected.is_empty() {
                return None;
            }
            let (raw_bytes, source) = match doc.format {
                EditorFormat::Binary => (
                    editing::decode_selected_hex_digits(selected.as_ref())?,
                    editing::SelectionSource::Hex,
                ),
                EditorFormat::Utf8 => {
                    if selected.bytes().all(|b| b.is_ascii_whitespace()) {
                        return None;
                    }
                    (selected.as_bytes().to_vec(), editing::SelectionSource::Utf8)
                }
            };
            let focused = window.is_some_and(|w| doc.state.focus_handle(cx).is_focused(w));
            Some(editing::SelectionCandidate {
                identity: editing::SelectionIdentity {
                    editor_id: doc.state.entity_id(),
                    document_revision: state.document_revision(),
                    range,
                    source,
                    writable: false,
                },
                raw_bytes,
                selection_seq: doc.selection_seq.get(),
                focused,
                writeback_editor: None,
            })
        };

        let mut candidates: Vec<editing::SelectionCandidate> =
            visible.iter().copied().filter_map(extract).collect();
        if candidates.is_empty() {
            return None;
        }
        if candidates.iter().any(|c| c.focused || c.selection_seq > 0) {
            return editing::pick_selection_candidate(candidates);
        }
        if let Some(focused_id) = self.focused_document.filter(|id| visible.contains(id))
            && let Some(doc) = self.documents.get(&focused_id)
            && let Some(pos) = candidates
                .iter()
                .position(|c| c.identity.editor_id == doc.state.entity_id())
        {
            return Some(candidates.swap_remove(pos));
        }
        candidates.into_iter().next()
    }

    pub(crate) fn focused_document(&self) -> Option<u64> {
        let mut visible = Vec::new();
        self.visible_documents(self.snapshot.root.as_ref().unwrap(), &mut visible);
        self.focused_document
            .filter(|id| visible.contains(id))
            .or_else(|| visible.first().copied())
            .or_else(|| {
                self.focused_document.filter(|id| {
                    self.documents
                        .get(id)
                        .is_some_and(|document| document.live && !document.read_only)
                })
            })
            .or_else(|| self.writable_documents().first().map(|(id, _)| *id))
    }

    pub(in crate::plugin_panel) fn remember_visible_document(&mut self) {
        let mut visible = Vec::new();
        self.visible_documents(self.snapshot.root.as_ref().unwrap(), &mut visible);
        if !self
            .focused_document
            .is_some_and(|id| visible.contains(&id))
            && let Some(id) = visible.first()
        {
            self.focused_document = Some(*id);
        }
    }

    pub(in crate::plugin_panel) fn visible_all_documents(
        &self,
        node: &Node,
        output: &mut Vec<u64>,
    ) {
        match node.kind.as_ref().unwrap() {
            node::Kind::Editor(editor) => output.push(editor.document_id),
            node::Kind::Row(container)
            | node::Kind::Column(container)
            | node::Kind::ScrollColumn(container) => {
                for child in &container.children {
                    self.visible_all_documents(child, output);
                }
            }
            node::Kind::Split(split) => {
                self.visible_all_documents(split.first.as_ref().unwrap(), output);
                self.visible_all_documents(split.second.as_ref().unwrap(), output);
            }
            node::Kind::Tabs(tabs) => {
                let active = self.overlays.latest_tab(node.id).unwrap_or(tabs.active);
                if let Some(child) = tabs
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.content.as_ref())
                    .find(|node| node.id == active)
                {
                    self.visible_all_documents(child, output);
                }
            }
            _ => {}
        }
    }

    pub(in crate::plugin_panel) fn visible_documents(&self, node: &Node, output: &mut Vec<u64>) {
        match node.kind.as_ref().unwrap() {
            node::Kind::Editor(editor) if !editor.read_only => output.push(editor.document_id),
            node::Kind::Row(container)
            | node::Kind::Column(container)
            | node::Kind::ScrollColumn(container) => {
                for child in &container.children {
                    self.visible_documents(child, output);
                }
            }
            node::Kind::Split(split) => {
                self.visible_documents(split.first.as_ref().unwrap(), output);
                self.visible_documents(split.second.as_ref().unwrap(), output);
            }
            node::Kind::Tabs(tabs) => {
                let active = self.overlays.latest_tab(node.id).unwrap_or(tabs.active);
                if let Some(child) = tabs
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.content.as_ref())
                    .find(|node| node.id == active)
                {
                    self.visible_documents(child, output);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn cancel_interaction(&mut self, cx: &mut Context<Self>) {
        self.cancel_pointer(cx);
        self.resize = None;
        cx.notify();
    }

    pub(crate) fn all_edits_saved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        self.capture_all(window, cx)?;
        Ok(!self.has_unsaved_edits(cx))
    }

    pub(crate) fn show_diagnostic(&mut self, message: &str, cx: &mut Context<Self>) {
        self.error = Some(message.to_owned());
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn event_failed(
        &mut self,
        node: Option<u64>,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        self.event_failed_with_seq(node, None, message, cx);
    }

    pub(crate) fn event_failed_with_seq(
        &mut self,
        node: Option<u64>,
        failed_seq: Option<u64>,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = node {
            let fallback_confirmed = self
                .snapshot
                .root
                .as_ref()
                .and_then(|root| root.find(id))
                .and_then(|n| n.kind.as_ref())
                .and_then(|kind| match kind {
                    node::Kind::VirtualTable(model) => Some(model.viewport_revision),
                    _ => None,
                });
            if let Some(table) = self.tables.get_mut(&id) {
                table.viewport.fail_request(failed_seq, fallback_confirmed);
            }
            self.overlays.remove_for_seq(id, failed_seq);
        }
        self.error = Some(message.to_owned());
        cx.notify();
    }

    pub(crate) fn mark_saved(&mut self, id: u64, revision: u64, cx: &mut Context<Self>) {
        if let Some(document) = self.documents.get_mut(&id)
            && document.snapshot.revision == revision
            && !document.pending_rebase
        {
            document.unsaved = false;
        }
        cx.notify();
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if !enabled {
            self.cancel_pointer(cx);
            self.resize = None;
        }
        self.enabled = enabled;
        cx.notify();
    }

    pub(crate) fn capture_editor(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<EditorSnapshot, String> {
        if !self.enabled {
            return Err("插件已停止，编辑内容可复制，但不能提交。".into());
        }
        let document = self.documents.get(&id).ok_or("插件编辑器已不存在。")?;
        if !document.live || document.read_only {
            return Err("此插件编辑器只读或已解除绑定。".into());
        }
        let captured = capture_document(document, window, cx)?;
        self.accept_native(id, &captured);
        Ok(captured)
    }
}

pub(in crate::plugin_panel) fn initial_snapshot(editor: &panel::Editor) -> EditorSnapshot {
    EditorSnapshot {
        document_id: editor.document_id,
        revision: editor.revision,
        contents: editor.contents.clone(),
        selection_start: 0,
        selection_end: 0,
    }
}

pub(in crate::plugin_panel) fn display_bytes(bytes: &[u8], format: EditorFormat) -> String {
    match format {
        EditorFormat::Utf8 => String::from_utf8(bytes.to_vec()).expect("validated UTF-8 editor"),
        EditorFormat::Binary => editing::hex_encode(bytes),
    }
}

pub(in crate::plugin_panel) fn capture_document(
    document: &Document,
    window: &mut Window,
    cx: &mut App,
) -> Result<EditorSnapshot, String> {
    let value = editing::snapshot(&document.state, window, cx).map_err(str::to_owned)?;
    let selection = document.state.read(cx).selected_range();
    let (contents, start, end) = match document.format {
        EditorFormat::Utf8 => (value.as_bytes().to_vec(), selection.start, selection.end),
        EditorFormat::Binary => {
            let contents = editing::hex_decode(&value)?;
            let start = value[..selection.start]
                .bytes()
                .filter(u8::is_ascii_hexdigit)
                .count()
                / 2;
            let end = value[..selection.end]
                .bytes()
                .filter(u8::is_ascii_hexdigit)
                .count()
                .div_ceil(2);
            (contents, start, end)
        }
    };
    let changed = contents != document.snapshot.contents;
    if document.read_only && changed {
        return Err("只读插件编辑器拒绝修改。".into());
    }
    let revision =
        document.snapshot.revision + u64::from(changed || document.pending_rebase);
    let snapshot = EditorSnapshot {
        document_id: document.snapshot.document_id,
        revision,
        contents,
        selection_start: start as u64,
        selection_end: end as u64,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

pub(in crate::plugin_panel) fn contains(rect: &panel::Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height
}

pub(in crate::plugin_panel) fn tone(value: i32) -> gpui_kit::Hsla {
    match panel::Tone::try_from(value).expect("validated panel tone") {
        panel::Tone::Foreground => theme::foreground(),
        panel::Tone::Muted => theme::muted_foreground(),
        panel::Tone::Accent => theme::accent(),
        panel::Tone::Error => theme::danger_fg(),
        panel::Tone::Background => theme::background(),
        panel::Tone::Border => theme::border(),
        panel::Tone::Selection => theme::selected(),
    }
}

pub(in crate::plugin_panel) fn walk<'a>(node: &'a Node, output: &mut Vec<&'a Node>) {
    output.push(node);
    match node.kind.as_ref().unwrap() {
        node::Kind::Row(container)
        | node::Kind::Column(container)
        | node::Kind::ScrollColumn(container) => {
            for child in &container.children {
                walk(child, output);
            }
        }
        node::Kind::Split(split) => {
            walk(split.first.as_ref().unwrap(), output);
            walk(split.second.as_ref().unwrap(), output);
        }
        node::Kind::Tabs(tabs) => {
            for tab in &tabs.tabs {
                walk(tab.content.as_ref().unwrap(), output);
            }
        }
        _ => {}
    }
}
