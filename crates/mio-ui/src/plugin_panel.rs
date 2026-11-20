use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
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
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, RenderImage, ScrollHandle,
    ScrollStrategy, Subscription, Task, TestSupportExt, UniformListScrollHandle, Window,
    accesskit::Role, canvas, div, img, prelude::*, px, relative, rgb, uniform_list,
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
    Ui { revision: u64, event: UiEvent },
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

pub(in crate::plugin_panel) struct Table {
    filter: Entity<InputState>,
    selected: Vec<String>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    horizontal: ScrollHandle,
    viewport_revision: u64,
    requested_offset: Option<u64>,
    _subscription: Subscription,
    _focus_subscriptions: Vec<Subscription>,
}

pub(in crate::plugin_panel) struct Drawing {
    model: panel::Canvas,
    image: Option<Arc<RenderImage>>,
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
    tabs: HashMap<u64, u64>,
    splits: HashMap<u64, f32>,
    checkboxes: HashMap<u64, bool>,
    selects: HashMap<u64, u32>,
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
            tabs: HashMap::new(),
            splits: HashMap::new(),
            checkboxes: HashMap::new(),
            selects: HashMap::new(),
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
                let active = self.tabs.get(&node.id).copied().unwrap_or(tabs.active);
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
                let active = self.tabs.get(&node.id).copied().unwrap_or(tabs.active);
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

    pub(crate) fn event_failed(
        &mut self,
        node: Option<u64>,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        for (id, table) in &mut self.tables {
            if node.is_none_or(|node| node == *id) {
                table.requested_offset = None;
            }
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
    let revision = document
        .snapshot
        .revision
        .checked_add(u64::from(changed || document.pending_rebase))
        .ok_or("插件文档 revision 已耗尽。")?;
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
        panel::Tone::Muted => rgb(0x6b6b6b).into(),
        panel::Tone::Accent => theme::accent(),
        panel::Tone::Error => rgb(0xb42318).into(),
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
