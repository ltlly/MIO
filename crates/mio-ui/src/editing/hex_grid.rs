use std::collections::VecDeque;
use std::ops::Range;

use gpui_kit::{
    Context, FocusHandle, Focusable, RenderOnce, StyleRefinement, UniformListScrollHandle,
};

use super::*;

mod ops;
mod render;

const MAX_HEX_UNDO: usize = 128;
const MAX_HEX_UNDO_BYTES: usize = 16 * 1024 * 1024;
const HEX_CLIPBOARD_META_PREFIX: &str = "mio-hex-bytes-v1:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexCopyHex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexCopyAscii;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexPasteBytes;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexEditSelectedByte;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexInsertBefore;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexAppendByte;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexDeleteSelectedBytes;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexUndo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexRedo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct HexToggleInsertMode;

fn count_hex_digits_in_chunks<'a>(
    chunks: impl Iterator<Item = &'a str>,
    range: Range<usize>,
    total_bytes: usize,
) -> Option<Range<usize>> {
    if range.is_empty() || total_bytes == 0 {
        return None;
    }
    let mut pos = 0usize;
    let mut before = 0usize;
    let mut inside = 0usize;
    'outer: for chunk in chunks {
        for &b in chunk.as_bytes() {
            if pos >= range.end {
                break 'outer;
            }
            if b.is_ascii_hexdigit() {
                if pos < range.start {
                    before += 1;
                } else {
                    inside += 1;
                }
            }
            pos += 1;
        }
    }
    if inside == 0 {
        return None;
    }
    let start_byte = (before / 2).min(total_bytes);
    let end_byte = (before + inside).div_ceil(2).min(total_bytes);
    (start_byte < end_byte).then_some(start_byte..end_byte)
}

pub(crate) fn hex_text_range_to_byte_range_for_rope(
    rope: &gpui_kit::base::input::Rope,
    text_range: Range<usize>,
    total_bytes: usize,
) -> Option<Range<usize>> {
    count_hex_digits_in_chunks(rope.chunks(), text_range, total_bytes)
}

#[cfg(test)]
pub(crate) fn hex_text_range_to_byte_range_in_str(
    text: &str,
    text_range: Range<usize>,
    total_bytes: usize,
) -> Option<Range<usize>> {
    count_hex_digits_in_chunks(std::iter::once(text), text_range, total_bytes)
}

#[cfg(test)]
pub(crate) fn hex_text_range_to_byte_range(
    text_range: Range<usize>,
    total_bytes: usize,
) -> Option<Range<usize>> {
    if text_range.is_empty() || total_bytes == 0 {
        return None;
    }
    let start_byte = text_range.start.saturating_add(1) / 3;
    let end_byte = (text_range.end.saturating_add(2) / 3).min(total_bytes);
    (start_byte < end_byte).then_some(start_byte..end_byte)
}

pub(crate) fn hex_byte_range_to_text_range(byte_range: Range<usize>) -> Range<usize> {
    if byte_range.is_empty() {
        let offset = byte_range.start.saturating_mul(3);
        return offset..offset;
    }
    let start = byte_range.start.saturating_mul(3);
    let end = byte_range.end.saturating_mul(3).saturating_sub(1);
    start..end
}

pub(super) type EditorContextMenuBuilder =
    Rc<dyn Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static>;

#[derive(Clone)]
struct HexUndoEntry {
    start: usize,
    old_bytes: Vec<u8>,
    new_bytes: Vec<u8>,
    prev_selection: Option<Range<usize>>,
    prev_cursor: Option<usize>,
    next_selection: Option<Range<usize>>,
    next_cursor: Option<usize>,
    editing_ascii: bool,
}

impl HexUndoEntry {
    fn retained_bytes(&self) -> usize {
        self.old_bytes.len().saturating_add(self.new_bytes.len())
    }
}

#[derive(IntoElement)]
pub(crate) struct MessageEditorElement {
    state: Entity<EditorState>,
    hex_grid: Option<Entity<HexGridView>>,
    style: StyleRefinement,
    readonly: bool,
    aria_label: Option<SharedString>,
    context_menu: Option<EditorContextMenuBuilder>,
}

impl MessageEditorElement {
    pub(crate) fn new(
        state: Entity<EditorState>,
        hex_grid: Option<Entity<HexGridView>>,
    ) -> Self {
        Self {
            state,
            hex_grid,
            style: StyleRefinement::default(),
            readonly: false,
            aria_label: None,
            context_menu: None,
        }
    }

    pub(crate) fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub(crate) fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    pub(crate) fn context_menu(
        mut self,
        builder: impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static,
    ) -> Self {
        self.context_menu = Some(Rc::new(builder));
        self
    }
}

impl Styled for MessageEditorElement {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for MessageEditorElement {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let caller_style = self.style;
        let Some(grid) = self.hex_grid else {
            let mut base_editor = Editor::new(&self.state);
            *base_editor.style() = caller_style;
            base_editor = base_editor
                .bordered(false)
                .readonly(self.readonly)
                .line_height(gpui_kit::relative(crate::theme::EDITOR_LINE_HEIGHT))
                .text_area_paddings(gpui_kit::Edges {
                    top: px(2.),
                    right: px(0.),
                    bottom: px(0.),
                    left: px(0.),
                });
            if let Some(label) = self.aria_label {
                base_editor = base_editor.aria_label(label);
            }
            if let Some(menu) = self.context_menu {
                base_editor = base_editor.context_menu(move |m, w, cx| menu(m, w, cx));
            }
            return base_editor.into_any_element();
        };
        grid.update(cx, |g, _| {
            g.readonly = self.readonly;
            g.context_menu = self.context_menu;
        });
        let mut container = div();
        *container.style() = caller_style;
        container
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(grid),
            )
            .into_any_element()
    }
}

pub(crate) struct HexGridView {
    pub(crate) hex_editor: Entity<EditorState>,
    pub(crate) byte_input: Entity<InputState>,
    shared: Rc<RefCell<MessageEditorShared>>,
    pub(crate) selected_range: Option<Range<usize>>,
    pub(crate) selection_anchor: Option<usize>,
    pub(crate) active_cursor: Option<usize>,
    pub(crate) low_nibble_pending: bool,
    pub(crate) editing_ascii: bool,
    pub(crate) insert_mode: bool,
    pub(crate) hovered_byte: Option<usize>,
    pub(crate) search_match_range: Option<Range<usize>>,
    pub(crate) dragging: bool,
    pub(crate) readonly: bool,
    pub(crate) last_synced_text_range: Range<usize>,
    undo_stack: VecDeque<HexUndoEntry>,
    redo_stack: VecDeque<HexUndoEntry>,
    pub(crate) scroll_handle: UniformListScrollHandle,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) context_menu: Option<EditorContextMenuBuilder>,
    _subscriptions: [Subscription; 2],
}

impl Focusable for HexGridView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn diff_byte_slices(old: &[u8], new: &[u8]) -> (usize, Vec<u8>, Vec<u8>) {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let old_rem = old.len() - prefix;
    let new_rem = new.len() - prefix;
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take(old_rem.min(new_rem))
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = old[prefix..old.len() - suffix].to_vec();
    let new_mid = new[prefix..new.len() - suffix].to_vec();
    (prefix, old_mid, new_mid)
}

impl HexGridView {
    pub(super) fn new(
        hex_editor: Entity<EditorState>,
        shared: Rc<RefCell<MessageEditorShared>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let byte_input = cx.new(|cx| InputState::new(window, cx).placeholder("Hex (如 41)"));
        let focus_handle = cx.focus_handle();
        let sub_change = cx.subscribe(&hex_editor, |this, editor, event, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let rev = editor.read(cx).document_revision();
            if this.shared.borrow().hex_synced_revision == Some(rev) {
                return;
            }
            let text = editor.read(cx).value();
            let Ok(decoded) = hex_decode(text.as_ref()) else {
                return;
            };
            let (prefix, old_mid, new_mid, new_len) = {
                let mut shared = this.shared.borrow_mut();
                let (prefix, old_mid, new_mid) = diff_byte_slices(&shared.wire_bytes, &decoded);
                let new_len = decoded.len();
                shared.wire_bytes = decoded;
                shared.hex_synced_revision = Some(rev);
                (prefix, old_mid, new_mid, new_len)
            };
            let prev_selection = this.selected_range.clone();
            let prev_cursor = this.active_cursor;
            let editing_ascii = this.editing_ascii;
            this.low_nibble_pending = false;
            if !old_mid.is_empty() || !new_mid.is_empty() {
                let next_end = prefix + new_mid.len();
                let next_sel = (prefix < next_end).then_some(prefix..next_end);
                let next_cursor = (new_len > 0).then_some(prefix.min(new_len));
                this.push_undo_delta(HexUndoEntry {
                    start: prefix,
                    old_bytes: old_mid,
                    new_bytes: new_mid,
                    prev_selection,
                    prev_cursor,
                    next_selection: next_sel,
                    next_cursor,
                    editing_ascii,
                });
            }
            if let Some(r) = &this.selected_range
                && r.end > new_len
            {
                this.selected_range = None;
            }
            if let Some(r) = &this.search_match_range
                && r.end > new_len
            {
                this.search_match_range = None;
            }
            if let Some(cur) = this.active_cursor
                && cur >= new_len
            {
                this.active_cursor = new_len.checked_sub(1);
            }
            cx.notify();
        });
        let sub_obs = cx.observe(&hex_editor, |this, editor, cx| {
            let state = editor.read(cx);
            let text_range = state.selected_range();
            if text_range == this.last_synced_text_range {
                return;
            }
            this.last_synced_text_range = text_range.clone();
            let total_bytes = this.shared.borrow().wire_bytes.len();
            let mapped =
                hex_text_range_to_byte_range_for_rope(state.text(), text_range, total_bytes);
            if mapped != this.selected_range {
                this.selected_range = mapped.clone();
                this.active_cursor = mapped.as_ref().map(|r| r.start);
                this.low_nibble_pending = false;
                if let Some(r) = &mapped {
                    this.scroll_handle
                        .scroll_to_item(r.start / 16, gpui_kit::ScrollStrategy::Center);
                }
                cx.notify();
            }
        });
        Self {
            hex_editor,
            byte_input,
            shared,
            selected_range: None,
            selection_anchor: None,
            active_cursor: None,
            low_nibble_pending: false,
            editing_ascii: false,
            insert_mode: false,
            hovered_byte: None,
            search_match_range: None,
            dragging: false,
            readonly: false,
            last_synced_text_range: 0..0,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            scroll_handle: UniformListScrollHandle::new(),
            focus_handle,
            context_menu: None,
            _subscriptions: [sub_change, sub_obs],
        }
    }

    pub(crate) fn bytes(&self) -> std::cell::Ref<'_, Vec<u8>> {
        std::cell::Ref::map(self.shared.borrow(), |s| &s.wire_bytes)
    }

    pub(super) fn scroll_to_text_range(
        &mut self,
        text_range: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let total_bytes = self.shared.borrow().wire_bytes.len();
        let byte_range = hex_text_range_to_byte_range_for_rope(
            self.hex_editor.read(cx).text(),
            text_range,
            total_bytes,
        );
        self.search_match_range = byte_range.clone();
        if let Some(ref r) = byte_range {
            self.scroll_handle
                .scroll_to_item(r.start / 16, gpui_kit::ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub(super) fn clear_search_match(&mut self, cx: &mut Context<Self>) {
        if self.search_match_range.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window) || self.byte_input.focus_handle(cx).is_focused(window)
    }

    pub(crate) fn selected_byte_range(&self) -> Option<Range<usize>> {
        let len = self.shared.borrow().wire_bytes.len();
        self.selected_range
            .clone()
            .filter(|r| r.start < r.end && r.end <= len)
    }

    fn effective_byte_selection(&self) -> Option<Range<usize>> {
        let len = self.shared.borrow().wire_bytes.len();
        self.selected_byte_range().or_else(|| {
            self.active_cursor
                .filter(|&c| c < len)
                .map(|c| c..c + 1)
        })
    }

    pub(super) fn reset_view_state(
        &mut self,
        selected_range: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = selected_range.clone();
        self.active_cursor = selected_range.as_ref().map(|r| r.start);
        self.selection_anchor = self.active_cursor;
        self.low_nibble_pending = false;
        self.search_match_range = None;
        self.dragging = false;
        self.last_synced_text_range = selected_range.map(hex_byte_range_to_text_range).unwrap_or(0..0);
        self.undo_stack.clear();
        self.redo_stack.clear();
        cx.notify();
    }

    fn total_undo_bytes(&self) -> usize {
        self.undo_stack
            .iter()
            .chain(self.redo_stack.iter())
            .map(HexUndoEntry::retained_bytes)
            .sum()
    }

    fn push_undo_delta(&mut self, entry: HexUndoEntry) {
        self.redo_stack.clear();
        if entry.retained_bytes() > MAX_HEX_UNDO_BYTES {
            self.undo_stack.clear();
            return;
        }
        while self.undo_stack.len() >= MAX_HEX_UNDO
            || (!self.undo_stack.is_empty()
                && self.total_undo_bytes().saturating_add(entry.retained_bytes())
                    > MAX_HEX_UNDO_BYTES)
        {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(entry);
    }

    fn sync_byte_input_to_cursor(&mut self, window: &mut Window, cx: &mut App) {
        if self.byte_input.focus_handle(cx).is_focused(window) {
            return;
        }
        let next = {
            let shared = self.shared.borrow();
            self.active_cursor
                .or_else(|| self.selected_range.as_ref().map(|r| r.start))
                .and_then(|idx| shared.wire_bytes.get(idx))
                .map(|b| format!("{b:02X}"))
                .unwrap_or_default()
        };
        if self.byte_input.read(cx).value().as_ref() != next {
            self.byte_input.update(cx, |inp, cx| inp.set_value(next, window, cx));
        }
    }
}
