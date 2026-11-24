use std::ops::Range;

use gpui_kit::{ClipboardItem, Context, KeyDownEvent, Window};

use super::*;

impl HexGridView {
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

    pub(in crate::editing) fn replace_all_bytes(
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

    pub(super) fn handle_key_down(
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
        let mut chars = typed.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };
        if !ch.is_ascii() {
            return;
        }
        self.apply_typed_char(ch, window, cx);
    }
}
