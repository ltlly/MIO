use super::http_head::{
    decode_message_bytes, encode_raw_text_bytes, pretty_line_number_mapping, serialize_pretty_edit,
};
use super::*;
use std::ops::Range;

impl MessageEditor {
    pub fn new<V: 'static>(window: &mut Window, cx: &mut gpui_kit::Context<V>) -> Self {
        let raw = new_editor(window, cx);
        let pretty = raw.clone();
        let hex = new_plain_editor(window, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let replace = cx.new(|cx| InputState::new(window, cx).placeholder("替换为"));
        let selection_seq = Rc::new(Cell::new(0));
        let view_cell = Rc::new(Cell::new(MessageView::Raw));
        let pretty_cell = Rc::new(Cell::new(false));
        let hide_headers_cell = Rc::new(Cell::new(false));
        let non_printable_cell = Rc::new(Cell::new(false));
        let header_preview_slot: Rc<RefCell<Option<Entity<EditorState>>>> =
            Rc::new(RefCell::new(None));
        let non_printable_slot: Rc<RefCell<Option<Entity<EditorState>>>> =
            Rc::new(RefCell::new(None));
        let weak_owner = cx.entity().downgrade();
        let notify_owner: Rc<dyn Fn(&mut App)> = Rc::new(move |cx: &mut App| {
            let _ = weak_owner.update(cx, |_, cx| cx.notify());
        });
        let default_case_sensitive = crate::theme::highlight_search_case_sensitive_default();
        let default_regex = crate::theme::message_editor_search_regex_default();
        let default_auto_scroll = crate::theme::message_editor_search_autoscroll_default();
        let case_sensitive_cell = Rc::new(Cell::new(default_case_sensitive));
        let regex_cell = Rc::new(Cell::new(default_regex));
        let auto_scroll_cell = Rc::new(Cell::new(default_auto_scroll));
        let search_error = Rc::new(RefCell::new(None));
        let subscription = {
            let header_preview_slot = header_preview_slot.clone();
            let non_printable_slot = non_printable_slot.clone();
            let raw = raw.clone();
            let hex = hex.clone();
            let view_cell = view_cell.clone();
            let pretty_cell = pretty_cell.clone();
            let hide_headers_cell = hide_headers_cell.clone();
            let non_printable_cell = non_printable_cell.clone();
            let case_sensitive_cell = case_sensitive_cell.clone();
            let regex_cell = regex_cell.clone();
            let auto_scroll_cell = auto_scroll_cell.clone();
            let search_error = search_error.clone();
            cx.subscribe(&search, move |_, search, event, cx| {
                let step = match event {
                    InputEvent::Change => None,
                    InputEvent::PressEnter { shift, .. } => Some(!shift),
                    _ => return,
                };
                if search.read(cx).has_marked_text() {
                    return;
                }
                let query = search.read(cx).value();
                let header_preview = header_preview_slot.borrow().clone();
                let non_printable = non_printable_slot.borrow().clone();
                let active: Entity<EditorState> = if hide_headers_cell.get()
                    && let Some(editor) = &header_preview
                {
                    editor.clone()
                } else if non_printable_cell.get()
                    && let Some(editor) = &non_printable
                {
                    editor.clone()
                } else if pretty_cell.get() {
                    raw.clone()
                } else {
                    match view_cell.get() {
                        MessageView::Raw => raw.clone(),
                        MessageView::Hex => hex.clone(),
                    }
                };
                for editor in [
                    header_preview.as_ref(),
                    non_printable.as_ref(),
                    Some(&raw),
                    Some(&hex),
                ]
                .into_iter()
                .flatten()
                {
                    if editor != &active {
                        editor.update(cx, |state, cx| state.close_search(cx));
                    }
                }
                let case_insensitive = !case_sensitive_cell.get();
                let regex = regex_cell.get();
                let result = active.update(cx, |state, cx| {
                    if query.is_empty() {
                        let _ = state.set_search_regex("", case_insensitive, regex, cx);
                        state.close_search(cx);
                        return Ok(None);
                    }
                    if let Err(error) =
                        state.set_search_regex(query.to_string(), case_insensitive, regex, cx)
                    {
                        return Err(format!("搜索正则表达式无效：{error}"));
                    }
                    let revealed = match step {
                        Some(true) => state.next_search_match(cx),
                        Some(false) => state.previous_search_match(cx),
                        None if auto_scroll_cell.get() => state.reveal_current_search_match(cx),
                        None => None,
                    };
                    Ok(revealed)
                });
                match result {
                    Ok(revealed) => {
                        *search_error.borrow_mut() = None;
                        if active.entity_id() == hex.entity_id() {
                            if let Some(match_range) = revealed {
                                super::hex_grid::scroll_hex_grid_to_text_range(
                                    hex.entity_id(),
                                    match_range,
                                    cx,
                                );
                            } else if query.is_empty()
                                || active.read(cx).search_session().matcher.is_empty()
                            {
                                super::hex_grid::clear_hex_grid_search_match(hex.entity_id(), cx);
                            }
                        }
                    }
                    Err(error) => {
                        *search_error.borrow_mut() = Some(error);
                        if active.entity_id() == hex.entity_id() {
                            super::hex_grid::clear_hex_grid_search_match(hex.entity_id(), cx);
                        }
                    }
                }
                cx.notify();
            })
        };
        let observe_editor_change = |editor: &Entity<EditorState>,
                                     is_hex: bool,
                                     auto_scroll_cell: Rc<Cell<bool>>,
                                     cx: &mut gpui_kit::Context<V>| {
            let auto_scroll_change = auto_scroll_cell.clone();
            let sub = cx.subscribe(editor, move |_, editor, event, cx| {
                if matches!(event, InputEvent::Change)
                    && editor.read(cx).search_session().is_active()
                {
                    let auto_scroll = auto_scroll_change.get();
                    let revealed = editor.update(cx, |state, cx| {
                        state.sync_search_matcher();
                        if auto_scroll {
                            state.reveal_current_search_match(cx)
                        } else {
                            None
                        }
                    });
                    if is_hex {
                        if let Some(match_range) = revealed {
                            super::hex_grid::scroll_hex_grid_to_text_range(
                                editor.entity_id(),
                                match_range,
                                cx,
                            );
                        } else if editor.read(cx).search_session().matcher.is_empty() {
                            super::hex_grid::clear_hex_grid_search_match(editor.entity_id(), cx);
                        }
                    }
                    cx.notify();
                }
            });
            let obs = cx.observe(editor, move |_, editor, cx| {
                if editor.read(cx).search_session().is_active()
                    && editor.read(cx).has_marked_text()
                {
                    editor.update(cx, |state, _| {
                        state.sync_search_matcher();
                    });
                }
            });
            Subscription::join(sub, obs)
        };
        let raw_subscription = observe_editor_change(&raw, false, auto_scroll_cell.clone(), cx);
        let hex_subscription = observe_editor_change(&hex, true, auto_scroll_cell.clone(), cx);
        let replace_subscription = cx.subscribe(&replace, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let raw_selection = observe_selection_state(&raw, selection_seq.clone(), cx);
        let hex_selection = observe_selection_state(&hex, selection_seq.clone(), cx);
        let default_soft_wrap = crate::theme::editor_soft_wrap();
        raw.update(cx, |state, cx| {
            state.set_soft_wrap(default_soft_wrap, window, cx);
        });
        hex.update(cx, |state, cx| {
            state.set_soft_wrap(false, window, cx);
        });
        Self {
            pretty,
            pretty_active: false,
            original_wire_bytes: Vec::new(),
            pretty_baseline_text: None,
            hide_headers: false,
            hide_headers_override: None,
            hidden_headers: None,
            header_preview_error: None,
            show_non_printable: false,
            render_active: false,
            header_preview: None,
            non_printable: None,
            header_preview_slot,
            non_printable_slot,
            soft_wrap: default_soft_wrap,
            raw,
            hex,
            hex_grid: None,
            search,
            replace,
            replace_open: false,
            pending_replace_all: None,
            case_sensitive: default_case_sensitive,
            regex: default_regex,
            auto_scroll_to_match: default_auto_scroll,
            search_error,
            view: MessageView::Raw,
            auto_hex_fallback: false,
            selection_seq,
            view_cell,
            pretty_cell,
            hide_headers_cell,
            non_printable_cell,
            case_sensitive_cell,
            regex_cell,
            auto_scroll_cell,
            notify_owner,
            _subscriptions: vec![
                subscription,
                raw_subscription,
                hex_subscription,
                replace_subscription,
                raw_selection,
                hex_selection,
            ],
        }
    }

    fn attach_lazy_editor_observers(&mut self, editor: &Entity<EditorState>, cx: &mut App) {
        let auto_scroll_change = self.auto_scroll_cell.clone();
        let notify_sub = self.notify_owner.clone();
        let sub = cx.subscribe(editor, move |editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && editor.read(cx).search_session().is_active() {
                let auto_scroll = auto_scroll_change.get();
                editor.update(cx, |state, cx| {
                    state.sync_search_matcher();
                    if auto_scroll {
                        state.reveal_current_search_match(cx);
                    }
                });
                notify_sub(cx);
            }
        });
        let obs = cx.observe(editor, move |editor, cx| {
            if editor.read(cx).search_session().is_active() && editor.read(cx).has_marked_text() {
                editor.update(cx, |state, _| {
                    state.sync_search_matcher();
                });
            }
        });
        let sel = observe_selection_state_app(
            editor,
            self.selection_seq.clone(),
            self.notify_owner.clone(),
            cx,
        );
        self._subscriptions.push(sub);
        self._subscriptions.push(obs);
        self._subscriptions.push(sel);
    }

    fn ensure_header_preview(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<EditorState> {
        if let Some(existing) = &self.header_preview {
            return existing.clone();
        }
        let editor = new_editor(window, cx);
        let soft_wrap = self.soft_wrap;
        editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        self.attach_lazy_editor_observers(&editor, cx);
        *self.header_preview_slot.borrow_mut() = Some(editor.clone());
        self.header_preview = Some(editor.clone());
        editor
    }

    fn ensure_non_printable(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<EditorState> {
        if let Some(existing) = &self.non_printable {
            return existing.clone();
        }
        let editor = new_editor(window, cx);
        let soft_wrap = self.soft_wrap;
        editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        self.attach_lazy_editor_observers(&editor, cx);
        *self.non_printable_slot.borrow_mut() = Some(editor.clone());
        self.non_printable = Some(editor.clone());
        editor
    }

    fn ensure_hex_grid(&mut self, window: &mut Window, cx: &mut App) -> Entity<HexGridView> {
        if let Some(existing) = &self.hex_grid {
            return existing.clone();
        }
        let hex_for_grid = self.hex.clone();
        let grid = cx.new(|cx| HexGridView::new(hex_for_grid, window, cx));
        super::hex_grid::register_hex_grid(self.hex.entity_id(), grid.downgrade());
        self.hex_grid = Some(grid.clone());
        grid
    }

    pub(crate) fn hex_grid_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.hex_grid
            .as_ref()
            .is_some_and(|grid| grid.read(cx).has_focus(window, cx))
    }

    pub(crate) fn selection_seq(&self) -> u64 {
        self.selection_seq.get()
    }

    pub(crate) fn is_selecting(&self, cx: &App) -> bool {
        self.state().read(cx).is_selecting()
    }

    pub(crate) fn is_readonly_preview(&self) -> bool {
        self.show_non_printable || self.render_active || self.hide_headers
    }

    #[cfg(test)]
    pub(crate) fn selected_bytes(&self, cx: &App) -> Option<(Vec<u8>, bool)> {
        let active = self.state().read(cx);
        if active.selected_range().is_empty() {
            return None;
        }
        let selected = active.selected_value();
        if selected.is_empty() {
            return None;
        }
        if self.is_readonly_preview() {
            return Some((selected.to_string().into_bytes(), false));
        }
        match if self.pretty_active { MessageView::Raw } else { self.view } {
            MessageView::Raw => Some((selected.to_string().into_bytes(), true)),
            MessageView::Hex => decode_selected_hex_digits(selected.as_ref()).map(|b| (b, false)),
        }
    }

    pub(crate) fn selection_candidate(
        &self,
        writable: bool,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<SelectionCandidate> {
        let active_entity = self.state();
        let active = active_entity.read(cx);
        let range = active.selected_range();
        if range.is_empty() {
            return None;
        }
        let selected = active.selected_value();
        if selected.is_empty() {
            return None;
        }
        let (raw_bytes, source, can_write_back) = if self.is_readonly_preview() {
            if selected.bytes().all(|b| b.is_ascii_whitespace()) {
                return None;
            }
            (selected.as_bytes().to_vec(), SelectionSource::Utf8, false)
        } else {
            match if self.pretty_active { MessageView::Raw } else { self.view } {
                MessageView::Raw => {
                    if selected.bytes().all(|b| b.is_ascii_whitespace()) {
                        return None;
                    }
                    (
                        selected.as_bytes().to_vec(),
                        SelectionSource::Utf8,
                        writable,
                    )
                }
                MessageView::Hex => {
                    let bytes = decode_selected_hex_digits(selected.as_ref())?;
                    (bytes, SelectionSource::Hex, writable)
                }
            }
        };
        let focused = window.is_some_and(|w| {
            active_entity.focus_handle(cx).is_focused(w)
                || (self.view == MessageView::Hex && self.hex_grid_has_focus(w, cx))
        });
        Some(SelectionCandidate {
            identity: SelectionIdentity {
                editor_id: active_entity.entity_id(),
                document_revision: active.document_revision(),
                range,
                source,
                writable: can_write_back,
            },
            raw_bytes,
            selection_seq: self.selection_seq.get(),
            focused,
            writeback_editor: can_write_back.then(|| active_entity.clone()),
        })
    }

    pub fn state(&self) -> &Entity<EditorState> {
        if self.hide_headers
            && let Some(editor) = &self.header_preview
        {
            return editor;
        }
        if self.show_non_printable
            && let Some(editor) = &self.non_printable
        {
            return editor;
        }
        if self.pretty_active {
            return &self.raw;
        }
        match self.view {
            MessageView::Raw => &self.raw,
            MessageView::Hex => &self.hex,
        }
    }

    pub fn current_byte_len(&self, cx: &App) -> usize {
        if self.pretty_active {
            let text_len = self.raw.read(cx).text().len();
            if self
                .pretty_baseline_text
                .as_ref()
                .is_some_and(|baseline| baseline.len() == text_len)
            {
                return self.original_wire_bytes.len();
            }
            return text_len;
        }
        match self.view {
            MessageView::Raw => self.raw.read(cx).text().len(),
            MessageView::Hex => {
                let hex_len = self.hex.read(cx).text().len();
                if hex_len == 0 {
                    0
                } else {
                    hex_len.saturating_add(1) / 3
                }
            }
        }
    }

    pub fn selected_byte_len(&self, cx: &App) -> usize {
        let active = self.state().read(cx);
        let range = active.selected_range();
        let len = range.len();
        if len == 0 {
            return 0;
        }
        if self.is_readonly_preview() || self.pretty_active || self.view == MessageView::Raw {
            return len;
        }
        // Hex view: count hex digits directly over small selections without allocating;
        // fall back to O(1) `hex_encode` density estimate for large selections.
        const EXACT_HEX_COUNT_LIMIT: usize = 4096;
        if len <= EXACT_HEX_COUNT_LIMIT {
            let text = active.text();
            if range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
            {
                let nibbles = text
                    .slice(range)
                    .chars()
                    .filter(|ch| ch.is_ascii_hexdigit())
                    .count();
                return nibbles / 2;
            }
        }
        len.saturating_add(1) / 3
    }

    fn clear_hex_view(&self, window: &mut Window, cx: &mut App) {
        if self.hex.read(cx).text().len() > 0 {
            self.hex
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        if let Some(grid) = &self.hex_grid {
            let rev = self.hex.read(cx).document_revision();
            grid.update(cx, |grid, cx| {
                grid.sync_bytes_and_selection(Vec::new(), None, rev, cx);
            });
        }
    }

    fn clear_non_printable_view(&self, window: &mut Window, cx: &mut App) {
        let Some(non_printable) = &self.non_printable else {
            return;
        };
        if non_printable.read(cx).text().len() > 0 {
            non_printable.update(cx, |state, cx| {
                state.set_highlighter_factory(
                    super::syntax_highlight::http_highlighter_factory(),
                    cx,
                );
                state.set_value("", window, cx);
            });
        }
    }

    fn clear_header_preview_view(&self, window: &mut Window, cx: &mut App) {
        let Some(header_preview) = &self.header_preview else {
            return;
        };
        let needs_clear = header_preview.read(cx).text().len() > 0
            || header_preview.read(cx).line_number_mapping().is_some();
        if needs_clear {
            header_preview.update(cx, |state, cx| {
                state.set_highlighter_factory(
                    super::syntax_highlight::http_highlighter_factory(),
                    cx,
                );
                state.set_value("", window, cx);
                state.set_line_number_mapping(None, cx);
            });
        }
    }

    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.header_preview
            .as_ref()
            .is_some_and(|ed| ed.focus_handle(cx).is_focused(window))
            || self
                .non_printable
                .as_ref()
                .is_some_and(|ed| ed.focus_handle(cx).is_focused(window))
            || self.raw.focus_handle(cx).is_focused(window)
            || self.hex.focus_handle(cx).is_focused(window)
            || self.hex_grid_has_focus(window, cx)
            || self.search.focus_handle(cx).is_focused(window)
            || self.replace.focus_handle(cx).is_focused(window)
    }

    pub(crate) fn hex_selected_byte_range(&self, cx: &App) -> Option<Range<usize>> {
        let grid_range = self
            .hex_grid
            .as_ref()
            .and_then(|grid| grid.read(cx).selected_byte_range());
        let hex_state = self.hex.read(cx);
        let text_range = hex_state.selected_range();
        let total_bytes = self.current_byte_len(cx);
        hex_text_range_to_byte_range_for_rope(hex_state.text(), text_range, total_bytes)
            .or(grid_range)
    }

    pub(crate) fn replace_hex_byte_range(
        &mut self,
        byte_range: Range<usize>,
        replacement: &[u8],
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(u64, Range<usize>), String> {
        let current_bytes = self.bytes(window, cx)?;
        if byte_range.start > byte_range.end || byte_range.end > current_bytes.len() {
            return Err("目标 Hex 选区超出当前报文字节范围。".into());
        }
        let hex_rev = self.hex.read(cx).document_revision();
        let hex_grid = self.ensure_hex_grid(window, cx);
        if hex_grid.read(cx).last_synced_revision != hex_rev {
            hex_grid.update(cx, |grid, cx| {
                grid.sync_bytes_and_selection(
                    current_bytes,
                    Some(byte_range.clone()),
                    hex_rev,
                    cx,
                );
            });
        }
        let (next_rev, new_text_range) = hex_grid.update(cx, |grid, cx| {
            grid.replace_byte_range(byte_range, replacement, window, cx)
        });
        self.original_wire_bytes = hex_grid.read(cx).bytes.clone();
        Ok((next_rev, new_text_range))
    }

    pub fn load(&mut self, bytes: &[u8], window: &mut Window, cx: &mut App) {
        self.original_wire_bytes = bytes.to_vec();
        self.pretty_baseline_text = None;
        let decoded = decode_message_bytes(bytes);
        let can_display_text = decoded.is_some();
        let (charset_mode, _) = crate::theme::message_editor_charset();
        self.hide_headers = self.hide_headers_override.unwrap_or_else(|| {
            !bytes.is_empty()
                && (can_display_text || self.show_non_printable)
                && crate::theme::hide_headers_by_default(bytes.starts_with(b"HTTP/"))
        });
        if !can_display_text && !self.show_non_printable {
            self.hide_headers = false;
        }
        self.hide_headers_cell.set(self.hide_headers);
        self.header_preview_error = None;
        self.pretty_active = false;
        self.pretty_cell.set(false);
        self.render_active = false;
        self.pending_replace_all = None;
        match decoded {
            Some(text) => {
                self.view = MessageView::Raw;
                self.auto_hex_fallback = false;
                if !bytes.is_empty()
                    && !self.show_non_printable
                    && charset_mode != 2
                    && std::str::from_utf8(bytes).is_ok()
                    && crate::theme::pretty_print_by_default()
                    && let Ok(formatted) = mio_core::format_pretty_message(bytes)
                {
                    let pretty_mapping = pretty_line_number_mapping(bytes, &formatted);
                    self.pretty_baseline_text = Some(formatted.clone());
                    self.raw.update(cx, |state, cx| {
                        state.set_value(formatted, window, cx);
                        state.set_line_number_mapping(pretty_mapping, cx);
                    });
                    self.pretty_active = true;
                    self.pretty_cell.set(true);
                } else {
                    self.raw.update(cx, |state, cx| {
                        state.set_value(text, window, cx);
                        state.set_line_number_mapping(None, cx);
                    });
                }
                self.clear_hex_view(window, cx);
            }
            None => {
                if self.raw.read(cx).text().len() > 0
                    || self.raw.read(cx).line_number_mapping().is_some()
                {
                    self.raw.update(cx, |state, cx| {
                        state.set_value("", window, cx);
                        state.set_line_number_mapping(None, cx);
                    });
                }
                let rev = self.hex.update(cx, |state, cx| {
                    state.set_value(hex_encode(bytes), window, cx);
                    state.document_revision()
                });
                let hex_grid = self.ensure_hex_grid(window, cx);
                hex_grid.update(cx, |grid, cx| {
                    grid.sync_bytes_and_selection(bytes.to_vec(), None, rev, cx);
                });
                self.view = MessageView::Hex;
                self.auto_hex_fallback = true;
            }
        }
        if self.show_non_printable {
            let soft_wrap = self.soft_wrap;
            let (visualized, badges) = super::codec::format_non_printable_preview_with_badges(bytes);
            let non_printable = self.ensure_non_printable(window, cx);
            non_printable.update(cx, |state, cx| {
                state.set_highlighter_factory(
                    super::syntax_highlight::http_highlighter_factory_with_badges(badges),
                    cx,
                );
                state.set_soft_wrap(soft_wrap, window, cx);
                state.set_value(visualized, window, cx);
            });
        } else {
            self.clear_non_printable_view(window, cx);
        }
        self.non_printable_cell.set(self.show_non_printable);
        self.view_cell.set(self.view);
        if self.hide_headers && let Err(error) = self.refresh_header_preview(window, cx) {
            self.header_preview_error = Some(error.clone());
            self.set_header_preview_content(error, Vec::new(), None, window, cx);
        } else if !self.hide_headers {
            self.clear_header_preview_view(window, cx);
        }
        let _ = self.update_search(None, window, cx);
    }

    pub(crate) fn apply_editor_preferences(&mut self, window: &mut Window, cx: &mut App) {
        let soft_wrap = crate::theme::editor_soft_wrap();
        self.soft_wrap = soft_wrap;
        for editor in [
            self.header_preview.as_ref(),
            self.non_printable.as_ref(),
            Some(&self.raw),
        ]
        .into_iter()
        .flatten()
        {
            editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        }
        let case_sensitive = crate::theme::highlight_search_case_sensitive_default();
        let regex = crate::theme::message_editor_search_regex_default();
        let auto_scroll = crate::theme::message_editor_search_autoscroll_default();
        let mut search_changed = false;
        if self.case_sensitive != case_sensitive {
            self.case_sensitive = case_sensitive;
            self.case_sensitive_cell.set(case_sensitive);
            search_changed = true;
        }
        if self.regex != regex {
            self.regex = regex;
            self.regex_cell.set(regex);
            search_changed = true;
        }
        if self.auto_scroll_to_match != auto_scroll {
            self.auto_scroll_to_match = auto_scroll;
            self.auto_scroll_cell.set(auto_scroll);
            search_changed = true;
        }
        if search_changed {
            self.pending_replace_all = None;
            let _ = self.update_search(None, window, cx);
        }
        if self.hide_headers_override.is_none() {
            let bytes = self.bytes_lossless(cx);
            self.hide_headers = !bytes.is_empty()
                && crate::theme::hide_headers_by_default(bytes.starts_with(b"HTTP/"));
            self.hide_headers_cell.set(self.hide_headers);
        }
        if self.hide_headers {
            if let Err(error) = self.set_hidden_headers(self.hidden_headers.clone(), window, cx) {
                self.header_preview_error = Some(error.clone());
                self.set_header_preview_content(error, Vec::new(), None, window, cx);
            }
            self.pending_replace_all = None;
            let _ = self.update_search(None, window, cx);
        } else {
            self.clear_header_preview_view(window, cx);
        }
    }

    pub fn toggle_soft_wrap(&mut self, window: &mut Window, cx: &mut App) {
        let next = !self.soft_wrap;
        self.soft_wrap = next;
        for editor in [
            self.header_preview.as_ref(),
            self.non_printable.as_ref(),
            Some(&self.raw),
        ]
        .into_iter()
        .flatten()
        {
            editor.update(cx, |state, cx| state.set_soft_wrap(next, window, cx));
        }
    }

    pub fn toggle_non_printable(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if self.search.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) || self.replace.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(COMPOSITION_PENDING.into());
        }
        let next = !self.show_non_printable;
        let bytes = self.bytes(window, cx)?;
        let filtered_preview = if self.hide_headers {
            Some(self.format_header_preview(&bytes, self.pretty_active, next)?)
        } else {
            None
        };
        self.pending_replace_all = None;
        self.replace_open = false;
        let focused = self.state().focus_handle(cx).is_focused(window);
        if self.show_non_printable {
            self.show_non_printable = false;
            self.non_printable_cell.set(false);
            if let Some(non_printable) = &self.non_printable {
                non_printable.update(cx, |state, cx| {
                    state.set_highlighter_factory(
                        super::syntax_highlight::http_highlighter_factory(),
                        cx,
                    );
                    state.set_value("", window, cx);
                });
                non_printable.update(cx, |state, cx| state.close_search(cx));
            }
            let query = self.search.read(cx).value();
            if let Some((preview, badges, line_numbers)) = filtered_preview {
                self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
                self.header_preview_error = None;
            }
            self.update_search(None, window, cx)?;
            if focused {
                self.state().update(cx, |state, cx| state.focus(window, cx));
            }
            let _ = query;
            return Ok(());
        } else {
            let soft_wrap = self.soft_wrap;
            let (visualized, badges) = super::codec::format_non_printable_preview_with_badges(&bytes);
            let non_printable = self.ensure_non_printable(window, cx);
            non_printable.update(cx, |state, cx| {
                state.set_highlighter_factory(
                    super::syntax_highlight::http_highlighter_factory_with_badges(badges),
                    cx,
                );
                state.set_soft_wrap(soft_wrap, window, cx);
                state.set_value(visualized, window, cx);
            });
            self.show_non_printable = true;
            self.non_printable_cell.set(true);
            self.render_active = false;
        }
        if let Some((preview, badges, line_numbers)) = filtered_preview {
            self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
            self.header_preview_error = None;
        }
        self.update_search(None, window, cx)?;
        if focused {
            self.state().update(cx, |state, cx| state.focus(window, cx));
        }
        Ok(())
    }

    pub fn switch_render(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if self.render_active {
            return Ok(());
        }
        if self.search.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) || self.replace.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(COMPOSITION_PENDING.into());
        }
        let bytes = self.bytes(window, cx)?;
        self.original_wire_bytes = bytes.clone();
        if self.pretty_active {
            if let Some(raw_text) = decode_message_bytes(&bytes)
                && self.raw.read(cx).value().as_ref() != raw_text
            {
                self.raw
                    .update(cx, |state, cx| state.set_value(raw_text, window, cx));
            }
        }
        self.pending_replace_all = None;
        self.pretty_active = false;
        self.pretty_cell.set(false);
        self.show_non_printable = false;
        self.non_printable_cell.set(false);
        self.hide_headers = false;
        self.hide_headers_cell.set(false);
        self.header_preview_error = None;
        self.clear_non_printable_view(window, cx);
        self.clear_header_preview_view(window, cx);
        self.render_active = true;
        let _ = self.update_search(None, window, cx);
        Ok(())
    }

    pub fn bytes_lossless(&self, cx: &App) -> Vec<u8> {
        if self.pretty_active {
            let text = self.raw.read(cx).value();
            if self.pretty_baseline_text.as_deref() == Some(text.as_ref()) {
                return self.original_wire_bytes.clone();
            }
            return serialize_pretty_edit(
                text.as_ref(),
                &self.original_wire_bytes,
                self.pretty_baseline_text.as_deref(),
            );
        }
        match self.view {
            MessageView::Raw => {
                let text = self.raw.read(cx).value();
                encode_raw_text_bytes(text.as_ref(), &self.original_wire_bytes)
            }
            MessageView::Hex => hex_decode(self.hex.read(cx).value().as_ref())
                .unwrap_or_else(|_| self.original_wire_bytes.clone()),
        }
    }

    pub fn bytes(&self, window: &mut Window, cx: &mut App) -> Result<Vec<u8>, String> {
        if self.pretty_active {
            let text = snapshot(&self.raw, window, cx).map_err(str::to_owned)?;
            if self.pretty_baseline_text.as_deref() == Some(text.as_ref()) {
                return Ok(self.original_wire_bytes.clone());
            }
            return Ok(serialize_pretty_edit(
                text.as_ref(),
                &self.original_wire_bytes,
                self.pretty_baseline_text.as_deref(),
            ));
        }
        let source = match self.view {
            MessageView::Raw => &self.raw,
            MessageView::Hex => &self.hex,
        };
        let text = snapshot(source, window, cx).map_err(str::to_owned)?;
        match self.view {
            MessageView::Raw => Ok(encode_raw_text_bytes(
                text.as_ref(),
                &self.original_wire_bytes,
            )),
            MessageView::Hex => hex_decode(&text),
        }
    }

    pub fn switch(
        &mut self,
        view: MessageView,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if view == self.view
            && !self.pretty_active
            && !self.show_non_printable
            && !self.render_active
            && !self.hide_headers
        {
            return Ok(());
        }
        if self.search.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) || self.replace.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(COMPOSITION_PENDING.into());
        }
        self.pending_replace_all = None;
        let focused = self.state().focus_handle(cx).is_focused(window);
        let bytes = self.bytes(window, cx)?;
        self.original_wire_bytes = bytes.clone();
        let filtered_preview = if self.hide_headers && view == MessageView::Raw {
            Some(self.format_header_preview(&bytes, false, false)?)
        } else {
            None
        };
        if view == self.view {
            if view == MessageView::Raw {
                let raw_text = decode_message_bytes(&bytes).ok_or_else(|| {
                    "消息含非 UTF-8 字节，请使用 Hex 视图查看原始字节。".to_owned()
                })?;
                self.raw.update(cx, |state, cx| {
                    if state.value().as_ref() != raw_text {
                        state.set_value(raw_text, window, cx);
                    }
                    state.set_line_number_mapping(None, cx);
                });
                self.clear_hex_view(window, cx);
            }
            self.auto_hex_fallback = false;
            self.pretty_active = false;
            self.pretty_cell.set(false);
            self.show_non_printable = false;
            self.non_printable_cell.set(false);
            self.clear_non_printable_view(window, cx);
            self.render_active = false;
            if view == MessageView::Hex {
                self.hide_headers = false;
                self.hide_headers_cell.set(false);
                self.hide_headers_override = None;
                self.clear_header_preview_view(window, cx);
            } else if !self.hide_headers {
                self.clear_header_preview_view(window, cx);
            }
            if let Some((preview, badges, line_numbers)) = filtered_preview {
                self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
                self.header_preview_error = None;
            }
            self.update_search(None, window, cx)?;
            if focused {
                self.state().update(cx, |state, cx| state.focus(window, cx));
            }
            return Ok(());
        }
        let (state, value) = match view {
            MessageView::Raw => (
                &self.raw,
                decode_message_bytes(&bytes)
                    .ok_or_else(|| "消息含非 UTF-8 字节，请使用 Hex 视图查看原始字节。".to_owned())?,
            ),
            MessageView::Hex => (&self.hex, hex_encode(&bytes)),
        };
        if state.read(cx).value().as_ref() != value {
            state.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        if view == MessageView::Hex {
            let rev = self.hex.read(cx).document_revision();
            let hex_grid = self.ensure_hex_grid(window, cx);
            hex_grid.update(cx, |grid, cx| {
                grid.sync_bytes_and_selection(bytes.clone(), None, rev, cx);
            });
        }
        self.pretty_active = false;
        self.pretty_cell.set(false);
        self.show_non_printable = false;
        self.non_printable_cell.set(false);
        self.clear_non_printable_view(window, cx);
        self.render_active = false;
        if view == MessageView::Hex {
            self.hide_headers = false;
            self.hide_headers_cell.set(false);
            self.hide_headers_override = None;
            self.header_preview_error = None;
            self.clear_header_preview_view(window, cx);
        } else {
            self.clear_hex_view(window, cx);
            if !self.hide_headers {
                self.clear_header_preview_view(window, cx);
            }
        }
        if let Some((preview, badges, line_numbers)) = filtered_preview {
            self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
            self.header_preview_error = None;
        }
        self.view = view;
        self.view_cell.set(view);
        self.auto_hex_fallback = false;
        self.update_search(None, window, cx)?;
        if focused {
            self.state().update(cx, |state, cx| state.focus(window, cx));
        }
        Ok(())
    }

    pub fn switch_pretty(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if self.pretty_active && !self.show_non_printable && !self.render_active {
            return Ok(());
        }
        if self.search.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) || self.replace.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(COMPOSITION_PENDING.into());
        }
        self.pending_replace_all = None;
        let focused = self.state().focus_handle(cx).is_focused(window);
        let bytes = self.bytes(window, cx)?;
        let formatted = mio_core::format_pretty_message(&bytes)?;
        let pretty_mapping = pretty_line_number_mapping(&bytes, &formatted);
        self.original_wire_bytes = bytes.clone();
        self.pretty_baseline_text = Some(formatted.clone());
        let filtered_preview = if self.hide_headers {
            Some(self.format_header_preview(&bytes, true, false)?)
        } else {
            None
        };
        self.raw.update(cx, |state, cx| {
            if state.value().as_ref() != formatted {
                state.set_value(formatted, window, cx);
            }
            state.set_line_number_mapping(pretty_mapping, cx);
        });
        self.view = MessageView::Raw;
        self.view_cell.set(MessageView::Raw);
        self.auto_hex_fallback = false;
        self.pretty_active = true;
        self.pretty_cell.set(true);
        self.show_non_printable = false;
        self.non_printable_cell.set(false);
        self.clear_non_printable_view(window, cx);
        self.clear_hex_view(window, cx);
        if !self.hide_headers {
            self.clear_header_preview_view(window, cx);
        }
        self.render_active = false;
        if let Some((preview, badges, line_numbers)) = filtered_preview {
            self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
            self.header_preview_error = None;
        }
        self.update_search(None, window, cx)?;
        if focused {
            self.state().update(cx, |state, cx| state.focus(window, cx));
        }
        Ok(())
    }

    fn format_header_preview(
        &self,
        bytes: &[u8],
        pretty: bool,
        non_printable: bool,
    ) -> Result<(String, Vec<std::ops::Range<usize>>, Vec<usize>), String> {
        let formatted;
        let source = if pretty && !non_printable {
            formatted = mio_core::format_pretty_message(bytes)?;
            formatted.as_bytes()
        } else {
            bytes
        };
        let (filtered, line_numbers) =
            filter_http_headers_bytes(source, self.hidden_headers.as_deref())?;
        if non_printable {
            let (preview, badges) = super::codec::format_non_printable_preview_with_badges(&filtered);
            return Ok((preview, badges, line_numbers));
        }
        let text = decode_message_bytes(&filtered).ok_or_else(|| {
            "消息包含非 UTF-8 字节，请开启显示非打印字符后隐藏标头，或使用 Hex 视图查看完整字节。".to_owned()
        })?;
        Ok((text, Vec::new(), line_numbers))
    }

    fn set_header_preview_content(
        &mut self,
        preview: String,
        badges: Vec<std::ops::Range<usize>>,
        line_numbers: Option<Vec<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let soft_wrap = self.soft_wrap;
        let header_preview = self.ensure_header_preview(window, cx);
        header_preview.update(cx, |state, cx| {
            state.set_highlighter_factory(
                if badges.is_empty() {
                    super::syntax_highlight::http_highlighter_factory()
                } else {
                    super::syntax_highlight::http_highlighter_factory_with_badges(badges)
                },
                cx,
            );
            state.set_soft_wrap(soft_wrap, window, cx);
            state.set_value(preview, window, cx);
            state.set_line_number_mapping(line_numbers, cx);
        });
    }

    pub(super) fn refresh_header_preview(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if !self.hide_headers {
            return Ok(());
        }
        let bytes = self.bytes(window, cx)?;
        let (preview, badges, line_numbers) =
            self.format_header_preview(&bytes, self.pretty_active, self.show_non_printable)?;
        self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
        self.hide_headers_cell.set(true);
        self.header_preview_error = None;
        Ok(())
    }

    pub(crate) fn set_hidden_headers(
        &mut self,
        headers: Option<Vec<String>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if self.has_any_composition(window, cx) {
            return Err(COMPOSITION_PENDING.into());
        }
        let validated = match headers {
            Some(list) => Some(validate_http_header_names(&list)?),
            None => None,
        };
        if !self.hide_headers {
            self.hidden_headers = validated;
            self.header_preview_error = None;
            return Ok(());
        }
        self.hidden_headers = validated;
        if let Err(error) = self.refresh_header_preview(window, cx) {
            self.header_preview_error = Some(error.clone());
            self.set_header_preview_content(error, Vec::new(), None, window, cx);
        }
        self.pending_replace_all = None;
        self.update_search(None, window, cx)
    }

    pub(crate) fn toggle_hide_headers(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if self.has_any_composition(window, cx) {
            return Err(COMPOSITION_PENDING.into());
        }
        let focused = self.state().focus_handle(cx).is_focused(window);
        if self.hide_headers {
            self.hide_headers = false;
            self.hide_headers_cell.set(false);
            self.header_preview_error = None;
            self.clear_header_preview_view(window, cx);
            if let Some(header_preview) = &self.header_preview {
                header_preview.update(cx, |state, cx| state.close_search(cx));
            }
        } else {
            if self.render_active || (self.view == MessageView::Hex && !self.show_non_printable && !self.pretty_active) {
                return Err("请切换到 Raw、Pretty 或显示非打印字符预览后隐藏指定标头。".into());
            }
            let bytes = self.bytes(window, cx)?;
            let (preview, badges, line_numbers, error) =
                match self.format_header_preview(&bytes, self.pretty_active, self.show_non_printable) {
                    Ok((preview, badges, line_numbers)) => (preview, badges, Some(line_numbers), None),
                    Err(error) => (error.clone(), Vec::new(), None, Some(error)),
                };
            self.set_header_preview_content(preview, badges, line_numbers, window, cx);
            self.hide_headers = true;
            self.hide_headers_cell.set(true);
            self.header_preview_error = error;
        }
        self.pending_replace_all = None;
        self.replace_open = false;
        self.hide_headers_override = Some(self.hide_headers);
        self.update_search(None, window, cx)?;
        if focused {
            self.state().update(cx, |state, cx| state.focus(window, cx));
        }
        Ok(())
    }

    pub(crate) fn apply_conversion(
        &mut self,
        kind: ConvertKind,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if readonly {
            return Err("当前消息为只读，不能原地转换选区。".into());
        }
        if self.is_readonly_preview() || (!self.pretty_active && self.view != MessageView::Raw) {
            return Err("请切换到 Raw 视图后再执行选区编码或解码转换。".into());
        }
        if self.raw.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(COMPOSITION_PENDING.into());
        }
        let range = self.raw.read(cx).selected_range();
        let reversed = self.raw.read(cx).is_selection_reversed();
        if range.is_empty() {
            return Err("请先在消息编辑器中选中要转换的文本。".into());
        }
        let selected = self.raw.read(cx).selected_value();
        let converted = convert_selection(kind, selected.as_ref())?;
        self.raw.update(cx, |state, cx| {
            state.replace(converted.clone(), window, cx);
            state.set_selected_range_with_direction(
                range.start..range.start + converted.len(),
                reversed,
                cx,
            );
        });
        Ok(())
    }
}
