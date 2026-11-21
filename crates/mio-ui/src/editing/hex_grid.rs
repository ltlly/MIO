use std::collections::VecDeque;
use std::ops::Range;

use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{
    ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Render, RenderOnce, StyleRefinement, UniformListScrollHandle,
    uniform_list,
};

use super::*;

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

    pub(crate) fn toggle_insert_mode(&mut self, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        self.insert_mode = !self.insert_mode;
        self.low_nibble_pending = false;
        if !self.insert_mode {
            let max_idx = self.shared.borrow().wire_bytes.len().checked_sub(1);
            self.active_cursor = match (self.active_cursor, max_idx) {
                (Some(cur), Some(last)) => Some(cur.min(last)),
                (_, None) | (None, Some(_)) => None,
            };
            self.selection_anchor = match (self.selection_anchor, max_idx) {
                (Some(anc), Some(last)) => Some(anc.min(last)),
                _ => self.selection_anchor.and(max_idx),
            };
        }
        cx.notify();
    }

    pub(crate) fn replace_byte_range(
        &mut self,
        byte_range: Range<usize>,
        replacement: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (u64, Range<usize>) {
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        let (old_bytes, new_len) = {
            let mut shared = self.shared.borrow_mut();
            let old_bytes = shared.wire_bytes[byte_range.clone()].to_vec();
            shared
                .wire_bytes
                .splice(byte_range.clone(), replacement.iter().copied());
            (old_bytes, shared.wire_bytes.len())
        };
        self.low_nibble_pending = false;
        let new_end = byte_range.start + replacement.len();
        let next_sel = (!replacement.is_empty()).then_some(byte_range.start..new_end);
        let next_cursor = next_sel.as_ref().map(|r| r.start).or_else(|| {
            new_len
                .checked_sub(1)
                .map(|last| byte_range.start.min(last))
        });
        self.selection_anchor = next_cursor;
        self.push_undo_delta(HexUndoEntry {
            start: byte_range.start,
            old_bytes,
            new_bytes: replacement.to_vec(),
            prev_selection,
            prev_cursor,
            next_selection: next_sel.clone(),
            next_cursor,
            editing_ascii,
        });
        let rev = self.commit_bytes_to_editor(next_sel.clone(), window, cx);
        let new_text_range = hex_byte_range_to_text_range(byte_range.start..new_end);
        if next_sel.is_none() {
            self.active_cursor = next_cursor;
            self.last_synced_text_range = new_text_range.clone();
            self.hex_editor
                .update(cx, |state, cx| state.set_selected_range(new_text_range.clone(), cx));
            self.sync_byte_input_to_cursor(window, cx);
        }
        (rev, new_text_range)
    }

    pub(super) fn replace_all_bytes(
        &mut self,
        new_bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        let (prefix, old_mid, new_mid) = {
            let mut shared = self.shared.borrow_mut();
            let diff = diff_byte_slices(&shared.wire_bytes, &new_bytes);
            shared.wire_bytes = new_bytes;
            diff
        };
        self.low_nibble_pending = false;
        let next_end = prefix + new_mid.len();
        let next_sel = (prefix < next_end).then_some(prefix..next_end);
        let next_cursor = next_sel.as_ref().map(|r| r.start);
        self.selection_anchor = next_cursor;
        if !old_mid.is_empty() || !new_mid.is_empty() {
            self.push_undo_delta(HexUndoEntry {
                start: prefix,
                old_bytes: old_mid,
                new_bytes: new_mid,
                prev_selection,
                prev_cursor,
                next_selection: next_sel.clone(),
                next_cursor,
                editing_ascii,
            });
        }
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    fn commit_bytes_to_editor(
        &mut self,
        next_selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let encoded = hex_encode(&self.shared.borrow().wire_bytes);
        let text_range = next_selection
            .clone()
            .map(hex_byte_range_to_text_range)
            .unwrap_or(0..0);
        self.selected_range = next_selection.clone();
        self.active_cursor = next_selection.map(|r| r.start);
        self.last_synced_text_range = text_range.clone();
        let rev = self.hex_editor.update(cx, |state, cx| {
            state.replace_all_without_history(encoded, window, cx);
            state.set_selected_range(text_range, cx);
            state.document_revision()
        });
        self.shared.borrow_mut().hex_synced_revision = Some(rev);
        self.sync_byte_input_to_cursor(window, cx);
        cx.notify();
        rev
    }

    fn apply_history_delta(
        &mut self,
        entry: HexUndoEntry,
        undo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<HexUndoEntry> {
        let (remove_len, insert_bytes, target_sel, target_cursor) = if undo {
            (
                entry.new_bytes.len(),
                entry.old_bytes.as_slice(),
                entry.prev_selection.clone(),
                entry.prev_cursor,
            )
        } else {
            (
                entry.old_bytes.len(),
                entry.new_bytes.as_slice(),
                entry.next_selection.clone(),
                entry.next_cursor,
            )
        };
        let end = entry.start.checked_add(remove_len)?;
        let len = {
            let mut shared = self.shared.borrow_mut();
            if end > shared.wire_bytes.len() {
                return None;
            }
            shared
                .wire_bytes
                .splice(entry.start..end, insert_bytes.iter().copied());
            shared.wire_bytes.len()
        };
        self.editing_ascii = entry.editing_ascii;
        self.low_nibble_pending = false;
        let clamped_sel = target_sel.filter(|r| r.start < r.end && r.end <= len);
        let valid_end_cursor = if self.insert_mode {
            len
        } else {
            len.saturating_sub(1)
        };
        let restored_cursor = target_cursor
            .filter(|&c| (len > 0 && c <= valid_end_cursor) || (c == 0 && len == 0))
            .filter(|&c| self.insert_mode || c < len)
            .or_else(|| clamped_sel.as_ref().map(|r| r.start));
        self.selection_anchor = restored_cursor;
        self.commit_bytes_to_editor(clamped_sel, window, cx);
        self.active_cursor = restored_cursor;
        self.sync_byte_input_to_cursor(window, cx);
        Some(entry)
    }

    pub(crate) fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let Some(entry) = self.undo_stack.pop_back() else {
            return;
        };
        if let Some(applied) = self.apply_history_delta(entry, true, window, cx) {
            self.redo_stack.push_back(applied);
        }
    }

    pub(crate) fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let Some(entry) = self.redo_stack.pop_back() else {
            return;
        };
        if let Some(applied) = self.apply_history_delta(entry, false, window, cx) {
            while self.undo_stack.len() >= MAX_HEX_UNDO {
                self.undo_stack.pop_front();
            }
            self.undo_stack.push_back(applied);
        }
    }

    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        let len = self.shared.borrow().wire_bytes.len();
        if len == 0 {
            return;
        }
        let all = 0..len;
        let text_range = hex_byte_range_to_text_range(all.clone());
        self.selected_range = Some(all);
        self.selection_anchor = Some(0);
        self.active_cursor = Some(0);
        self.low_nibble_pending = false;
        self.last_synced_text_range = text_range.clone();
        self.hex_editor.update(cx, |s, cx| {
            s.set_selected_range(text_range, cx);
        });
        cx.notify();
    }

    pub(crate) fn select_byte(
        &mut self,
        byte_ix: usize,
        extend: bool,
        ascii_mode: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.shared.borrow().wire_bytes.len();
        if byte_ix >= len {
            return;
        }
        self.focus_handle.focus(window, cx);
        self.editing_ascii = ascii_mode;
        self.low_nibble_pending = false;
        let anchor = if extend {
            self.selection_anchor.unwrap_or(byte_ix)
        } else {
            self.selection_anchor = Some(byte_ix);
            byte_ix
        };
        let start = anchor.min(byte_ix);
        let end = (anchor.max(byte_ix) + 1).min(len);
        let range = start..end;
        self.selected_range = Some(range.clone());
        self.active_cursor = Some(byte_ix);
        let text_range = hex_byte_range_to_text_range(range);
        self.last_synced_text_range = text_range.clone();
        self.hex_editor.update(cx, |state, cx| {
            state.set_selected_range(text_range, cx);
        });
        self.sync_byte_input_to_cursor(window, cx);
        cx.notify();
    }

    pub(crate) fn copy_selection(&self, force_ascii: Option<bool>, cx: &mut Context<Self>) {
        let Some(range) = self.effective_byte_selection() else {
            return;
        };
        self.copy_byte_range(range, force_ascii, cx);
    }

    fn copy_byte_range(
        &self,
        range: Range<usize>,
        force_ascii: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let (text, meta) = {
            let shared = self.shared.borrow();
            let slice = &shared.wire_bytes[range];
            let as_ascii = force_ascii.unwrap_or(self.editing_ascii);
            let text = if as_ascii {
                String::from_utf8_lossy(slice).into_owned()
            } else {
                hex_encode(slice)
            };
            let meta = format!("{HEX_CLIPBOARD_META_PREFIX}{}", hex_encode(slice));
            (text, meta)
        };
        cx.write_to_clipboard(ClipboardItem::new_string_with_metadata(text, meta));
    }

    pub(crate) fn cut_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let Some(range) = self.effective_byte_selection() else {
            return;
        };
        self.copy_byte_range(range.clone(), None, cx);
        self.delete_byte_range(range, window, cx);
    }

    pub(crate) fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let bytes_to_paste = if let Some(exact) = item
            .metadata()
            .and_then(|meta| meta.strip_prefix(HEX_CLIPBOARD_META_PREFIX))
            .and_then(|hex_str| hex_decode(hex_str).ok())
        {
            exact
        } else {
            let Some(raw) = item.text() else {
                return;
            };
            if raw.is_empty() {
                return;
            }
            if !self.editing_ascii {
                hex_decode(raw.trim()).unwrap_or_else(|_| raw.into_bytes())
            } else {
                raw.into_bytes()
            }
        };
        if bytes_to_paste.is_empty() {
            return;
        }
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        self.low_nibble_pending = false;
        let len = bytes_to_paste.len();
        let selected_sel = self.selected_byte_range();
        let (insert_pos, old_bytes, new_len) = {
            let mut shared = self.shared.borrow_mut();
            let (insert_pos, old_bytes) = if let Some(sel) = selected_sel {
                let start = sel.start;
                let removed = shared.wire_bytes[sel.clone()].to_vec();
                shared.wire_bytes.splice(sel, bytes_to_paste.iter().copied());
                (start, removed)
            } else {
                let wire_len = shared.wire_bytes.len();
                let at = self.active_cursor.unwrap_or(wire_len).min(wire_len);
                shared.wire_bytes.splice(at..at, bytes_to_paste.iter().copied());
                (at, Vec::new())
            };
            (insert_pos, old_bytes, shared.wire_bytes.len())
        };
        let end = (insert_pos + len).min(new_len);
        let next_sel = (insert_pos < end).then_some(insert_pos..end);
        self.selection_anchor = (insert_pos < end).then_some(insert_pos);
        self.push_undo_delta(HexUndoEntry {
            start: insert_pos,
            old_bytes,
            new_bytes: bytes_to_paste,
            prev_selection,
            prev_cursor,
            next_selection: next_sel.clone(),
            next_cursor: next_sel.as_ref().map(|r| r.start),
            editing_ascii,
        });
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    fn parse_input_or_default(&self, default_byte: u8, cx: &App) -> Vec<u8> {
        let raw = self.byte_input.read(cx).value();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return vec![default_byte];
        }
        hex_decode(trimmed).unwrap_or_else(|_| vec![default_byte])
    }

    pub(crate) fn edit_selected_byte(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wire_len = self.shared.borrow().wire_bytes.len();
        if self.readonly || wire_len == 0 {
            return;
        }
        let last_idx = wire_len - 1;
        let target = self
            .selected_byte_range()
            .or_else(|| {
                self.active_cursor
                    .filter(|&c| c < wire_len)
                    .map(|c| c..c + 1)
            })
            .unwrap_or(last_idx..last_idx + 1);
        if target.end > wire_len {
            return;
        }
        let cur_val = self
            .shared
            .borrow()
            .wire_bytes
            .get(target.start)
            .copied()
            .unwrap_or(0);
        let replacement = self.parse_input_or_default(cur_val.wrapping_add(1), cx);
        let start = target.start;
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        let (old_bytes, new_len) = {
            let mut shared = self.shared.borrow_mut();
            let old_bytes = shared.wire_bytes[target.clone()].to_vec();
            shared.wire_bytes.splice(target, replacement.iter().copied());
            (old_bytes, shared.wire_bytes.len())
        };
        let end = (start + replacement.len()).min(new_len);
        let next_sel = (start < end).then_some(start..end);
        self.low_nibble_pending = false;
        self.selection_anchor = (start < end).then_some(start);
        self.push_undo_delta(HexUndoEntry {
            start,
            old_bytes,
            new_bytes: replacement,
            prev_selection,
            prev_cursor,
            next_selection: next_sel.clone(),
            next_cursor: next_sel.as_ref().map(|r| r.start),
            editing_ascii,
        });
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    pub(crate) fn insert_byte_before(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let wire_len = self.shared.borrow().wire_bytes.len();
        let insert_at = self
            .selected_byte_range()
            .map(|r| r.start)
            .or(self.active_cursor)
            .unwrap_or(0)
            .min(wire_len);
        let inserted = self.parse_input_or_default(0x00, cx);
        let len = inserted.len();
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        self.shared
            .borrow_mut()
            .wire_bytes
            .splice(insert_at..insert_at, inserted.iter().copied());
        self.low_nibble_pending = false;
        self.selection_anchor = Some(insert_at);
        let next_sel = Some(insert_at..insert_at + len);
        self.push_undo_delta(HexUndoEntry {
            start: insert_at,
            old_bytes: Vec::new(),
            new_bytes: inserted,
            prev_selection,
            prev_cursor,
            next_selection: next_sel.clone(),
            next_cursor: Some(insert_at),
            editing_ascii,
        });
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    pub(crate) fn append_byte(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let start = self.shared.borrow().wire_bytes.len();
        let appended = self.parse_input_or_default(0x00, cx);
        let len = appended.len();
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        self.shared
            .borrow_mut()
            .wire_bytes
            .extend_from_slice(&appended);
        self.low_nibble_pending = false;
        self.selection_anchor = Some(start);
        let next_sel = Some(start..start + len);
        self.push_undo_delta(HexUndoEntry {
            start,
            old_bytes: Vec::new(),
            new_bytes: appended,
            prev_selection,
            prev_cursor,
            next_selection: next_sel.clone(),
            next_cursor: Some(start),
            editing_ascii,
        });
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    pub(crate) fn delete_selected_bytes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly || self.shared.borrow().wire_bytes.is_empty() {
            return;
        }
        let Some(range) = self.effective_byte_selection() else {
            return;
        };
        self.delete_byte_range(range, window, cx);
    }

    fn delete_byte_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wire_len = self.shared.borrow().wire_bytes.len();
        if self.readonly || range.start >= range.end || range.end > wire_len {
            return;
        }
        let start = range.start;
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        let (old_bytes, new_len) = {
            let mut shared = self.shared.borrow_mut();
            let old_bytes = shared.wire_bytes[range.clone()].to_vec();
            shared.wire_bytes.drain(range);
            (old_bytes, shared.wire_bytes.len())
        };
        self.low_nibble_pending = false;
        let next_sel = if new_len == 0 {
            self.selection_anchor = None;
            None
        } else {
            let idx = start.min(new_len - 1);
            self.selection_anchor = Some(idx);
            Some(idx..idx + 1)
        };
        self.push_undo_delta(HexUndoEntry {
            start,
            old_bytes,
            new_bytes: Vec::new(),
            prev_selection,
            prev_cursor,
            next_selection: next_sel.clone(),
            next_cursor: next_sel.as_ref().map(|r| r.start),
            editing_ascii,
        });
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    fn apply_typed_char(&mut self, ch: char, window: &mut Window, cx: &mut Context<Self>) {
        let prev_selection = self.selected_range.clone();
        let prev_cursor = self.active_cursor;
        let editing_ascii = self.editing_ascii;
        let wire_len = self.shared.borrow().wire_bytes.len();
        if wire_len == 0 {
            if self.editing_ascii {
                if ch.is_ascii() && !ch.is_ascii_control() {
                    let b = ch as u8;
                    self.shared.borrow_mut().wire_bytes.push(b);
                    self.selection_anchor = Some(0);
                    self.low_nibble_pending = false;
                    let next = usize::from(self.insert_mode);
                    let next_sel = (next == 0).then_some(0..1);
                    self.push_undo_delta(HexUndoEntry {
                        start: 0,
                        old_bytes: Vec::new(),
                        new_bytes: vec![b],
                        prev_selection,
                        prev_cursor,
                        next_selection: next_sel.clone(),
                        next_cursor: Some(next),
                        editing_ascii,
                    });
                    self.commit_bytes_to_editor(next_sel, window, cx);
                    if self.insert_mode {
                        self.active_cursor = Some(next);
                        self.selection_anchor = Some(next);
                    }
                }
            } else if let Some(nibble) = ch.to_digit(16).map(|d| d as u8) {
                let b = nibble << 4;
                self.shared.borrow_mut().wire_bytes.push(b);
                self.low_nibble_pending = true;
                self.selection_anchor = Some(0);
                self.push_undo_delta(HexUndoEntry {
                    start: 0,
                    old_bytes: Vec::new(),
                    new_bytes: vec![b],
                    prev_selection,
                    prev_cursor,
                    next_selection: Some(0..1),
                    next_cursor: Some(0),
                    editing_ascii,
                });
                self.commit_bytes_to_editor(Some(0..1), window, cx);
            }
            return;
        }
        let max_cursor = if self.insert_mode && !self.low_nibble_pending {
            wire_len
        } else {
            wire_len - 1
        };
        let cursor = self
            .active_cursor
            .or_else(|| self.selected_range.as_ref().map(|r| r.start))
            .unwrap_or(0)
            .min(max_cursor);
        if self.editing_ascii {
            if ch.is_ascii() && !ch.is_ascii_control() {
                let b = ch as u8;
                if self.insert_mode {
                    let new_len = {
                        let mut shared = self.shared.borrow_mut();
                        shared.wire_bytes.insert(cursor, b);
                        shared.wire_bytes.len()
                    };
                    let next = cursor + 1;
                    let sel = (next < new_len).then_some(next..next + 1);
                    self.push_undo_delta(HexUndoEntry {
                        start: cursor,
                        old_bytes: Vec::new(),
                        new_bytes: vec![b],
                        prev_selection,
                        prev_cursor,
                        next_selection: sel.clone(),
                        next_cursor: Some(next),
                        editing_ascii,
                    });
                    self.commit_bytes_to_editor(sel, window, cx);
                    self.active_cursor = Some(next);
                    self.selection_anchor = Some(next);
                } else {
                    let (old_byte, next) = {
                        let mut shared = self.shared.borrow_mut();
                        let old_byte = shared.wire_bytes[cursor];
                        shared.wire_bytes[cursor] = b;
                        let next = (cursor + 1).min(shared.wire_bytes.len() - 1);
                        (old_byte, next)
                    };
                    self.selection_anchor = Some(next);
                    self.push_undo_delta(HexUndoEntry {
                        start: cursor,
                        old_bytes: vec![old_byte],
                        new_bytes: vec![b],
                        prev_selection,
                        prev_cursor,
                        next_selection: Some(next..next + 1),
                        next_cursor: Some(next),
                        editing_ascii,
                    });
                    self.commit_bytes_to_editor(Some(next..next + 1), window, cx);
                }
            }
        } else if let Some(nibble) = ch.to_digit(16).map(|d| d as u8) {
            if self.insert_mode && !self.low_nibble_pending {
                let b = nibble << 4;
                self.shared.borrow_mut().wire_bytes.insert(cursor, b);
                self.low_nibble_pending = true;
                self.selection_anchor = Some(cursor);
                self.push_undo_delta(HexUndoEntry {
                    start: cursor,
                    old_bytes: Vec::new(),
                    new_bytes: vec![b],
                    prev_selection,
                    prev_cursor,
                    next_selection: Some(cursor..cursor + 1),
                    next_cursor: Some(cursor),
                    editing_ascii,
                });
                self.commit_bytes_to_editor(Some(cursor..cursor + 1), window, cx);
            } else if !self.low_nibble_pending {
                let (old_byte, b) = {
                    let mut shared = self.shared.borrow_mut();
                    let old_byte = shared.wire_bytes[cursor];
                    let b = (old_byte & 0x0F) | (nibble << 4);
                    shared.wire_bytes[cursor] = b;
                    (old_byte, b)
                };
                self.low_nibble_pending = true;
                self.selection_anchor = Some(cursor);
                self.push_undo_delta(HexUndoEntry {
                    start: cursor,
                    old_bytes: vec![old_byte],
                    new_bytes: vec![b],
                    prev_selection,
                    prev_cursor,
                    next_selection: Some(cursor..cursor + 1),
                    next_cursor: Some(cursor),
                    editing_ascii,
                });
                self.commit_bytes_to_editor(Some(cursor..cursor + 1), window, cx);
            } else {
                let (old_byte, b, new_len) = {
                    let mut shared = self.shared.borrow_mut();
                    let old_byte = shared.wire_bytes[cursor];
                    let b = (old_byte & 0xF0) | nibble;
                    shared.wire_bytes[cursor] = b;
                    (old_byte, b, shared.wire_bytes.len())
                };
                self.low_nibble_pending = false;
                if self.insert_mode {
                    let next = cursor + 1;
                    let sel = (next < new_len).then_some(next..next + 1);
                    if let Some(last) = self
                        .undo_stack
                        .back_mut()
                        .filter(|e| e.start == cursor && e.new_bytes.len() == 1 && !e.editing_ascii)
                    {
                        last.new_bytes[0] = b;
                        last.next_selection = sel.clone();
                        last.next_cursor = Some(next);
                        self.redo_stack.clear();
                    } else {
                        self.push_undo_delta(HexUndoEntry {
                            start: cursor,
                            old_bytes: vec![old_byte],
                            new_bytes: vec![b],
                            prev_selection,
                            prev_cursor,
                            next_selection: sel.clone(),
                            next_cursor: Some(next),
                            editing_ascii,
                        });
                    }
                    self.commit_bytes_to_editor(sel, window, cx);
                    self.active_cursor = Some(next);
                    self.selection_anchor = Some(next);
                } else {
                    let next = (cursor + 1).min(new_len - 1);
                    let sel = Some(next..next + 1);
                    if let Some(last) = self
                        .undo_stack
                        .back_mut()
                        .filter(|e| e.start == cursor && e.new_bytes.len() == 1 && !e.editing_ascii)
                    {
                        last.new_bytes[0] = b;
                        last.next_selection = sel.clone();
                        last.next_cursor = Some(next);
                        self.redo_stack.clear();
                    } else {
                        self.push_undo_delta(HexUndoEntry {
                            start: cursor,
                            old_bytes: vec![old_byte],
                            new_bytes: vec![b],
                            prev_selection,
                            prev_cursor,
                            next_selection: sel.clone(),
                            next_cursor: Some(next),
                            editing_ascii,
                        });
                    }
                    self.selection_anchor = Some(next);
                    self.commit_bytes_to_editor(sel, window, cx);
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn handle_key_down_for_test(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_key_down(event, window, cx);
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let wire_len = self.shared.borrow().wire_bytes.len();
        if ks.modifiers.platform || ks.modifiers.control {
            if ks.modifiers.alt {
                return;
            }
            if ks.key.eq_ignore_ascii_case("a") {
                self.select_all(cx);
            } else if ks.key.eq_ignore_ascii_case("c") {
                self.copy_selection(None, cx);
            } else if ks.key.eq_ignore_ascii_case("x") {
                self.cut_selection(window, cx);
            } else if ks.key.eq_ignore_ascii_case("v") {
                self.paste_clipboard(window, cx);
            } else if ks.key.eq_ignore_ascii_case("z") {
                if ks.modifiers.shift {
                    self.redo(window, cx);
                } else {
                    self.undo(window, cx);
                }
            } else if ks.key.eq_ignore_ascii_case("y") {
                self.redo(window, cx);
            } else if ks.key == "home" && wire_len > 0 {
                self.select_byte(0, ks.modifiers.shift, self.editing_ascii, window, cx);
            } else if ks.key == "end" && wire_len > 0 {
                let last = wire_len - 1;
                self.select_byte(last, ks.modifiers.shift, self.editing_ascii, window, cx);
            }
            return;
        }
        if ks.modifiers.alt {
            return;
        }
        match ks.key.as_str() {
            "tab" => {
                self.editing_ascii = !self.editing_ascii;
                self.low_nibble_pending = false;
                cx.notify();
                return;
            }
            "home" if wire_len > 0 => {
                let cur = self.active_cursor.unwrap_or(0).min(wire_len - 1);
                let row_start = (cur / 16) * 16;
                self.select_byte(row_start, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "end" if wire_len > 0 => {
                let cur = self.active_cursor.unwrap_or(0).min(wire_len - 1);
                let row_end = ((cur / 16) * 16 + 15).min(wire_len - 1);
                self.select_byte(row_end, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "left" if wire_len > 0 => {
                let cur = self.active_cursor.unwrap_or(0).saturating_sub(1);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "right" if wire_len > 0 => {
                let cur = (self.active_cursor.unwrap_or(0) + 1).min(wire_len - 1);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "up" if wire_len > 0 => {
                let cur = self.active_cursor.unwrap_or(0).saturating_sub(16);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "down" if wire_len > 0 => {
                let cur = (self.active_cursor.unwrap_or(0) + 16).min(wire_len - 1);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "backspace" if !self.readonly => {
                if self.selected_byte_range().is_some() {
                    self.delete_selected_bytes(window, cx);
                } else if let Some(cur) = self.active_cursor
                    && cur > 0
                    && cur <= wire_len
                    && (self.insert_mode || cur == wire_len)
                {
                    self.delete_byte_range((cur - 1)..cur, window, cx);
                } else {
                    self.delete_selected_bytes(window, cx);
                }
                return;
            }
            "delete" if !self.readonly => {
                self.delete_selected_bytes(window, cx);
                return;
            }
            "insert" if !self.readonly => {
                self.insert_byte_before(window, cx);
                return;
            }
            _ => {}
        }
        if self.readonly {
            return;
        }
        let typed = ks.key_char.as_deref().unwrap_or(ks.key.as_str());
        if typed.len() != 1 {
            return;
        }
        let ch = typed.chars().next().unwrap();
        self.apply_typed_char(ch, window, cx);
    }
}

impl Render for HexGridView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = crate::theme::is_dark();
        let selected_range = self.selected_range.clone();
        let (total_bytes, status_summary) = {
            let shared = self.shared.borrow();
            let total_bytes = shared.wire_bytes.len();
            let summary = if let Some(ref range) = selected_range {
                let len = range.len();
                if len == 1
                    && let Some(&b) = shared.wire_bytes.get(range.start)
                {
                    format!(
                        "Offset: {:08X} · Byte: 0x{b:02X} ({b}) · Total: {total_bytes} B",
                        range.start
                    )
                } else {
                    format!(
                        "Range: {:08X}..{:08X} ({len} B / 0x{len:x}) · Total: {total_bytes} B",
                        range.start, range.end
                    )
                }
            } else {
                format!("Total: {total_bytes} bytes (0x{total_bytes:x})")
            };
            (total_bytes, summary)
        };
        let row_count = total_bytes.div_ceil(16).max(1);
        let readonly = self.readonly;
        let insert_mode = self.insert_mode;
        let can_undo = !readonly && !self.undo_stack.is_empty();
        let can_redo = !readonly && !self.redo_stack.is_empty();
        let has_selection = self.selected_byte_range().is_some()
            || self.active_cursor.is_some_and(|c| c < total_bytes);
        let offset_bg = rgb(if dark { 0x232529 } else { 0xf3f4f6 });
        let header_bg = rgb(if dark { 0x25272b } else { 0xededf0 });
        let border_col = crate::theme::border();
        let mono_font = crate::theme::mono_font_family();
        let hex_entity_id = self.hex_editor.entity_id();

        let toolbar = div()
            .id(("hex-grid-toolbar", hex_entity_id))
            .test_support()
            .w_full()
            .h(px(27.))
            .px(px(8.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(header_bg)
            .border_b_1()
            .border_color(border_col)
            .child(
                div()
                    .font_family(mono_font)
                    .text_size(px(11.5))
                    .text_color(crate::theme::muted_foreground())
                    .truncate()
                    .child(status_summary),
            )
            .child(div().flex_1())
            .when(!readonly, |bar| {
                bar.child(
                    div()
                        .w(px(74.))
                        .h(px(20.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(border_col)
                        .bg(crate::theme::surface())
                        .overflow_hidden()
                        .child(
                            Input::new(&self.byte_input)
                                .id(("hex-grid-byte-input", hex_entity_id))
                                .small()
                                .appearance(false)
                                .h(px(18.))
                                .px(px(4.)),
                        ),
                )
                .child(
                    Button::new(("hex-grid-mode-toggle", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .selected(insert_mode)
                        .label(if insert_mode { "插入" } else { "覆写" })
                        .tooltip("切换键盘直接键入时的覆写/插入字节模式")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_insert_mode(cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-edit-byte", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("修改字节")
                        .tooltip("修改选中字节 (Edit byte)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit_selected_byte(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-insert-before", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("前插字节")
                        .tooltip("在选中位置前插入字节 (Insert byte before)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.insert_byte_before(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-append-byte", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("追加字节")
                        .tooltip("在报文末尾追加字节 (Append byte)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.append_byte(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-delete-byte", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .label("删除字节")
                        .tooltip("删除选中字节 (Delete byte)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.delete_selected_bytes(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-undo", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .disabled(!can_undo)
                        .label("撤销")
                        .tooltip("撤销上次字节修改")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.undo(window, cx);
                        })),
                )
                .child(
                    Button::new(("hex-grid-redo", hex_entity_id))
                        .ghost()
                        .small()
                        .compact()
                        .rounded(ButtonRounded::Small)
                        .h(px(20.))
                        .px(px(6.))
                        .disabled(!can_redo)
                        .label("重做")
                        .tooltip("重做上次撤销的字节修改")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.redo(window, cx);
                        })),
                )
            });

        let mut col_header = div()
            .id(("hex-grid-column-header", hex_entity_id))
            .test_support()
            .w_full()
            .h(px(21.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(offset_bg)
            .border_b_1()
            .border_color(border_col)
            .font_family(mono_font)
            .text_size(px(11.5))
            .text_color(crate::theme::muted_foreground())
            .child(
                div()
                    .w(px(68.))
                    .h_full()
                    .px(px(5.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_r_1()
                    .border_color(border_col)
                    .child("Offset"),
            );

        let mut hex_cols_hdr = div()
            .px(px(5.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(1.5))
            .border_r_1()
            .border_color(border_col);
        for col in 0..16usize {
            hex_cols_hdr = hex_cols_hdr.child(
                div()
                    .w(px(18.))
                    .when(col == 7, |el| el.mr(px(4.)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(format!("{col:02X}")),
            );
        }
        col_header = col_header.child(hex_cols_hdr).child(
            div()
                .px(px(6.))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .child("ASCII (0..F)"),
        );

        let weak = cx.weak_entity();
        let list = uniform_list(
            ("hex-grid-rows", hex_entity_id),
            row_count,
            move |range, _, cx| {
                let Some(entity) = weak.upgrade() else {
                    return Vec::new();
                };
                let grid = entity.read(cx);
                let bytes = grid.bytes();
                let total = bytes.len();
                let sel = grid.selected_range.clone();
                let search_hit = grid.search_match_range.clone();
                let hov = grid.hovered_byte;
                let cur = grid.active_cursor;
                let low_nib = grid.low_nibble_pending;
                let ascii_focus = grid.editing_ascii;
                let sel_primary_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x265e9c } else { 0x3b82f6 }).into();
                let sel_sync_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x2d4163 } else { 0xdbeafe }).into();
                let search_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x6b5314 } else { 0xfde047 }).into();
                let hover_sync_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x32363e } else { 0xe5e7eb }).into();

                range
                    .map(|row_ix| {
                        let base_offset = row_ix * 16;
                        let offset_cell = div()
                            .id(("hex-row-offset", row_ix))
                            .test_support()
                            .w(px(68.))
                            .h_full()
                            .px(px(5.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .bg(offset_bg)
                            .border_r_1()
                            .border_color(border_col)
                            .text_color(crate::theme::muted_foreground())
                            .child(format!("{base_offset:08X}"));

                        let mut hex_cells = div()
                            .px(px(5.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(1.5))
                            .border_r_1()
                            .border_color(border_col);

                        let mut ascii_cells =
                            div().px(px(6.)).h_full().flex_shrink_0().flex().items_center();

                        for col in 0..16usize {
                            let byte_ix = base_offset + col;
                            if byte_ix < total {
                                let b = bytes[byte_ix];
                                let is_sel =
                                    sel.as_ref().is_some_and(|r| r.contains(&byte_ix));
                                let is_search =
                                    search_hit.as_ref().is_some_and(|r| r.contains(&byte_ix));
                                let is_hov = hov == Some(byte_ix);
                                let is_cur_nibble =
                                    cur == Some(byte_ix) && low_nib && !ascii_focus;
                                let hex_bg = if is_sel {
                                    if ascii_focus {
                                        sel_sync_bg
                                    } else {
                                        sel_primary_bg
                                    }
                                } else if is_search {
                                    search_bg
                                } else if is_hov {
                                    hover_sync_bg
                                } else {
                                    gpui_kit::transparent_black()
                                };
                                let hex_fg = if is_sel && !ascii_focus {
                                    rgb(0xffffff).into()
                                } else {
                                    crate::theme::foreground()
                                };
                                let ascii_bg = if is_sel {
                                    if ascii_focus {
                                        sel_primary_bg
                                    } else {
                                        sel_sync_bg
                                    }
                                } else if is_search {
                                    search_bg
                                } else if is_hov {
                                    hover_sync_bg
                                } else {
                                    gpui_kit::transparent_black()
                                };
                                let is_printable = (0x20..=0x7E).contains(&b);
                                let ascii_fg = if is_sel && ascii_focus {
                                    rgb(0xffffff).into()
                                } else if is_printable {
                                    crate::theme::foreground()
                                } else {
                                    crate::theme::muted_foreground()
                                };
                                let ascii_ch = if is_printable {
                                    char::from(b).to_string()
                                } else {
                                    ".".to_owned()
                                };

                                let w_down_hex = weak.clone();
                                let w_move_hex = weak.clone();
                                let w_down_ascii = weak.clone();
                                let w_move_ascii = weak.clone();

                                hex_cells = hex_cells.child(
                                    div()
                                        .id(("hex-byte-cell", byte_ix))
                                        .test_support()
                                        .w(px(18.))
                                        .h(px(18.))
                                        .when(col == 7, |el| el.mr(px(4.)))
                                        .rounded(px(2.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(hex_bg)
                                        .text_color(hex_fg)
                                        .when(is_cur_nibble, |el| {
                                            el.border_b_2().border_color(crate::theme::accent())
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |ev: &MouseDownEvent, window, cx| {
                                                let _ = w_down_hex.update(cx, |this, cx| {
                                                    this.dragging = true;
                                                    this.select_byte(
                                                        byte_ix,
                                                        ev.modifiers.shift,
                                                        false,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            },
                                        )
                                        .on_mouse_move(
                                            move |ev: &MouseMoveEvent, window, cx| {
                                                let _ = w_move_hex.update(cx, |this, cx| {
                                                    if ev.pressed_button != Some(MouseButton::Left) {
                                                        this.dragging = false;
                                                    }
                                                    if this.hovered_byte != Some(byte_ix) {
                                                        this.hovered_byte = Some(byte_ix);
                                                        cx.notify();
                                                    }
                                                    if ev.pressed_button == Some(MouseButton::Left)
                                                        && this.dragging
                                                    {
                                                        this.select_byte(
                                                            byte_ix, true, false, window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        )
                                        .child(format!("{b:02X}")),
                                );

                                ascii_cells = ascii_cells.child(
                                    div()
                                        .id(("hex-ascii-cell", byte_ix))
                                        .test_support()
                                        .w(px(8.5))
                                        .h(px(18.))
                                        .rounded(px(2.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(ascii_bg)
                                        .text_color(ascii_fg)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |ev: &MouseDownEvent, window, cx| {
                                                let _ = w_down_ascii.update(cx, |this, cx| {
                                                    this.dragging = true;
                                                    this.select_byte(
                                                        byte_ix,
                                                        ev.modifiers.shift,
                                                        true,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            },
                                        )
                                        .on_mouse_move(
                                            move |ev: &MouseMoveEvent, window, cx| {
                                                let _ = w_move_ascii.update(cx, |this, cx| {
                                                    if ev.pressed_button != Some(MouseButton::Left) {
                                                        this.dragging = false;
                                                    }
                                                    if this.hovered_byte != Some(byte_ix) {
                                                        this.hovered_byte = Some(byte_ix);
                                                        cx.notify();
                                                    }
                                                    if ev.pressed_button == Some(MouseButton::Left)
                                                        && this.dragging
                                                    {
                                                        this.select_byte(
                                                            byte_ix, true, true, window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        )
                                        .child(ascii_ch),
                                );
                            } else {
                                hex_cells = hex_cells.child(
                                    div()
                                        .w(px(18.))
                                        .h(px(18.))
                                        .when(col == 7, |el| el.mr(px(4.))),
                                );
                                ascii_cells = ascii_cells.child(div().w(px(8.5)).h(px(18.)));
                            }
                        }

                        div()
                            .id(("hex-grid-row", row_ix))
                            .test_support()
                            .w_full()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .font_family(mono_font)
                            .text_size(crate::theme::editor_font_size())
                            .child(offset_cell)
                            .child(hex_cells)
                            .child(ascii_cells)
                            .into_any_element()
                    })
                    .collect()
            },
        )
        .track_scroll(&self.scroll_handle)
        .size_full();

        let v_scrollbar = gpui_kit::component::scroll::Scrollbar::vertical(&self.scroll_handle)
            .id(("hex-grid-vscroll", hex_entity_id))
            .viewport_from_layout()
            .mode(gpui_kit::component::scroll::ScrollbarMode::Always);

        let caller_menu = self.context_menu.clone();
        let menu_focus = self.focus_handle.clone();
        let grid_body = div()
            .id(("hex-grid-body", hex_entity_id))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .flex()
            .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, window, cx| {
                let mut menu = if let Some(ref builder) = caller_menu {
                    builder(NativeMenu::new(), window, cx)
                } else {
                    NativeMenu::new()
                };
                // Keep HexGridView focused so keyboard & menu actions dispatch to hex handlers.
                menu_focus.focus(window, cx);
                if !menu.is_empty() {
                    menu = menu.separator();
                }
                menu = menu.submenu(
                    "Hex 字节编辑",
                    NativeMenu::new()
                        .menu_with_disabled("复制 Hex", !has_selection, Box::new(HexCopyHex))
                        .menu_with_disabled("复制 ASCII", !has_selection, Box::new(HexCopyAscii))
                        .menu_with_disabled("粘贴字节", readonly, Box::new(HexPasteBytes))
                        .separator()
                        .menu_with_disabled(
                            "修改选中字节",
                            readonly || total_bytes == 0,
                            Box::new(HexEditSelectedByte),
                        )
                        .menu_with_disabled("前插字节", readonly, Box::new(HexInsertBefore))
                        .menu_with_disabled("追加字节", readonly, Box::new(HexAppendByte))
                        .menu_with_disabled(
                            "删除选中字节",
                            readonly || total_bytes == 0,
                            Box::new(HexDeleteSelectedBytes),
                        )
                        .separator()
                        .menu_with_disabled("撤销字节修改", !can_undo, Box::new(HexUndo))
                        .menu_with_disabled("重做字节修改", !can_redo, Box::new(HexRedo))
                        .menu_with_check(
                            "插入模式（否则为覆写）",
                            insert_mode,
                            Box::new(HexToggleInsertMode),
                        ),
                );
                menu.show(ev.position, window, cx);
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(list),
            )
            .child(
                div()
                    .relative()
                    .w(px(12.))
                    .h_full()
                    .flex_shrink_0()
                    .bg(offset_bg)
                    .border_l_1()
                    .border_color(border_col)
                    .child(v_scrollbar),
            );

        div()
            .id(("hex-grid-view", hex_entity_id))
            .test_support()
            .key_context("HexGrid")
            .track_focus(&self.focus_handle)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(crate::theme::surface())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.dragging = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.dragging = false;
                }),
            )
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_action(cx.listener(|this, _: &HexCopyHex, _, cx| {
                this.copy_selection(Some(false), cx);
            }))
            .on_action(cx.listener(|this, _: &HexCopyAscii, _, cx| {
                this.copy_selection(Some(true), cx);
            }))
            .on_action(cx.listener(|this, _: &HexPasteBytes, window, cx| {
                this.paste_clipboard(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexEditSelectedByte, window, cx| {
                this.edit_selected_byte(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexInsertBefore, window, cx| {
                this.insert_byte_before(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexAppendByte, window, cx| {
                this.append_byte(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexDeleteSelectedBytes, window, cx| {
                this.delete_selected_bytes(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexUndo, window, cx| {
                this.undo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexRedo, window, cx| {
                this.redo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HexToggleInsertMode, _, cx| {
                this.toggle_insert_mode(cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                this.copy_selection(None, cx);
            }))
            .on_action(cx.listener(|this, _: &Cut, window, cx| {
                this.cut_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, window, cx| {
                this.paste_clipboard(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                this.undo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, window, cx| {
                this.redo(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.select_all(cx);
            }))
            .child(toolbar)
            .child(col_header)
            .child(grid_body)
    }
}
