use super::http_head::{
    decode_message_bytes, decode_message_bytes_with_charset, encode_raw_text_bytes,
    serialize_pretty_edit,
};
use super::*;
use std::ops::Range;

type HeaderPreviewResult = (String, Vec<std::ops::Range<usize>>, Vec<usize>);

impl MessageEditor {
    pub fn new<V: 'static>(window: &mut Window, cx: &mut gpui_kit::Context<V>) -> Self {
        let raw = new_editor(window, cx);
        let hex = new_plain_editor(window, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let replace = cx.new(|cx| InputState::new(window, cx).placeholder("替换为"));
        let selection_seq = Rc::new(Cell::new(0));
        let weak_owner = cx.entity().downgrade();
        let notify_owner: Rc<dyn Fn(&mut App)> = Rc::new(move |cx: &mut App| {
            let _ = weak_owner.update(cx, |_, cx| cx.notify());
        });
        let default_case_sensitive = crate::theme::highlight_search_case_sensitive_default();
        let default_regex = crate::theme::message_editor_search_regex_default();
        let default_auto_scroll = crate::theme::message_editor_search_autoscroll_default();
        let shared = Rc::new(RefCell::new(MessageEditorShared {
            view: MessageView::Raw,
            pretty_active: false,
            hide_headers: false,
            show_non_printable: false,
            case_sensitive: default_case_sensitive,
            regex: default_regex,
            auto_scroll_to_match: default_auto_scroll,
            header_preview: None,
            non_printable: None,
            hex_grid: None,
            search_error: None,
            wire_bytes: Vec::new(),
            hex_synced_revision: None,
        }));
        let subscription = {
            let shared = shared.clone();
            let raw = raw.clone();
            let hex = hex.clone();
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
                let (
                    active,
                    header_preview,
                    non_printable,
                    case_insensitive,
                    regex,
                    auto_scroll,
                    hex_grid,
                ) = {
                    let s = shared.borrow();
                    (
                        s.active_editor(&raw, &hex),
                        s.header_preview.clone(),
                        s.non_printable.clone(),
                        !s.case_sensitive,
                        s.regex,
                        s.auto_scroll_to_match,
                        s.hex_grid.as_ref().and_then(WeakEntity::upgrade),
                    )
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
                        None if auto_scroll => state.reveal_current_search_match(cx),
                        None => None,
                    };
                    Ok(revealed)
                });
                match result {
                    Ok(revealed) => {
                        shared.borrow_mut().search_error = None;
                        if active.entity_id() == hex.entity_id()
                            && let Some(grid) = hex_grid
                        {
                            if let Some(match_range) = revealed {
                                grid.update(cx, |g, cx| g.scroll_to_text_range(match_range, cx));
                            } else if query.is_empty()
                                || active.read(cx).search_session().matcher.is_empty()
                            {
                                grid.update(cx, |g, cx| g.clear_search_match(cx));
                            }
                        }
                    }
                    Err(error) => {
                        shared.borrow_mut().search_error = Some(error);
                        if active.entity_id() == hex.entity_id()
                            && let Some(grid) = hex_grid
                        {
                            grid.update(cx, |g, cx| g.clear_search_match(cx));
                        }
                    }
                }
                cx.notify();
            })
        };
        let observe_editor_change =
            |editor: &Entity<EditorState>,
             is_hex: bool,
             shared: Rc<RefCell<MessageEditorShared>>,
             cx: &mut gpui_kit::Context<V>| {
                let shared_change = shared;
                let sub = cx.subscribe(editor, move |_, editor, event, cx| {
                    if matches!(event, InputEvent::Change)
                        && editor.read(cx).search_session().is_active()
                    {
                        let (auto_scroll, hex_grid) = {
                            let s = shared_change.borrow();
                            (
                                s.auto_scroll_to_match,
                                s.hex_grid.as_ref().and_then(WeakEntity::upgrade),
                            )
                        };
                        let revealed = editor.update(cx, |state, cx| {
                            state.sync_search_matcher();
                            if auto_scroll {
                                state.reveal_current_search_match(cx)
                            } else {
                                None
                            }
                        });
                        if is_hex && let Some(grid) = hex_grid {
                            if let Some(match_range) = revealed {
                                grid.update(cx, |g, cx| g.scroll_to_text_range(match_range, cx));
                            } else if editor.read(cx).search_session().matcher.is_empty() {
                                grid.update(cx, |g, cx| g.clear_search_match(cx));
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
        let raw_subscription = observe_editor_change(&raw, false, shared.clone(), cx);
        let hex_subscription = observe_editor_change(&hex, true, shared.clone(), cx);
        let replace_subscription = cx.subscribe(&replace, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let raw_selection = observe_selection_state(&raw, selection_seq.clone(), cx);
        let hex_selection = observe_selection_state(&hex, selection_seq.clone(), cx);
        let default_soft_wrap = crate::theme::editor_soft_wrap();
        let raw_baseline_text = Some(raw.read(cx).value().to_string());
        let raw_baseline_revision = raw.update(cx, |state, cx| {
            state.set_soft_wrap(default_soft_wrap, window, cx);
            state.document_revision()
        });
        hex.update(cx, |state, cx| {
            state.set_soft_wrap(false, window, cx);
        });
        Self {
            shared,
            render_active: false,
            hide_headers_override: None,
            hidden_headers: None,
            header_preview_error: None,
            pretty_baseline_text: None,
            pretty_baseline_revision: None,
            raw_baseline_text,
            raw_baseline_revision: Some(raw_baseline_revision),
            hex_grid: None,
            raw_display_charset: MessageDisplayCharset::Utf8,
            raw_charset_pref: crate::theme::message_editor_charset(),
            soft_wrap: default_soft_wrap,
            raw,
            hex,
            search,
            replace,
            replace_open: false,
            pending_replace_all: None,
            auto_hex_fallback: false,
            selection_seq,
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

    pub fn view(&self) -> MessageView {
        self.shared.borrow().view
    }

    pub fn pretty_active(&self) -> bool {
        self.shared.borrow().pretty_active
    }

    pub fn hide_headers(&self) -> bool {
        self.shared.borrow().hide_headers
    }

    pub fn show_non_printable(&self) -> bool {
        self.shared.borrow().show_non_printable
    }

    pub fn case_sensitive(&self) -> bool {
        self.shared.borrow().case_sensitive
    }

    pub fn regex(&self) -> bool {
        self.shared.borrow().regex
    }

    pub fn set_regex(&mut self, regex: bool) {
        self.shared.borrow_mut().regex = regex;
    }

    pub fn auto_scroll_to_match(&self) -> bool {
        self.shared.borrow().auto_scroll_to_match
    }

    pub fn has_search_error(&self) -> bool {
        self.shared.borrow().search_error.is_some()
    }

    pub(crate) fn hex_grid(&self) -> Option<Entity<HexGridView>> {
        self.hex_grid.clone()
    }

    pub(crate) fn active_hex_grid(&self) -> Option<Entity<HexGridView>> {
        let s = self.shared.borrow();
        (!s.show_non_printable
            && !self.render_active
            && !s.hide_headers
            && !s.pretty_active
            && s.view == MessageView::Hex)
            .then(|| self.hex_grid.clone())
            .flatten()
    }

    #[cfg(test)]
    pub(crate) fn header_preview(&self) -> Option<Entity<EditorState>> {
        self.shared.borrow().header_preview.clone()
    }

    #[cfg(test)]
    pub(crate) fn non_printable(&self) -> Option<Entity<EditorState>> {
        self.shared.borrow().non_printable.clone()
    }

    fn attach_lazy_editor_observers(&mut self, editor: &Entity<EditorState>, cx: &mut App) {
        let shared_change = self.shared.clone();
        let notify_sub = self.notify_owner.clone();
        let sub = cx.subscribe(editor, move |editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && editor.read(cx).search_session().is_active() {
                let auto_scroll = shared_change.borrow().auto_scroll_to_match;
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
        if let Some(existing) = self.shared.borrow().header_preview.clone() {
            return existing;
        }
        let editor = new_editor(window, cx);
        let soft_wrap = self.soft_wrap;
        editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        self.attach_lazy_editor_observers(&editor, cx);
        self.shared.borrow_mut().header_preview = Some(editor.clone());
        editor
    }

    fn ensure_non_printable(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<EditorState> {
        if let Some(existing) = self.shared.borrow().non_printable.clone() {
            return existing;
        }
        let editor = new_editor(window, cx);
        let soft_wrap = self.soft_wrap;
        editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        self.attach_lazy_editor_observers(&editor, cx);
        self.shared.borrow_mut().non_printable = Some(editor.clone());
        editor
    }

    fn ensure_hex_grid(&mut self, window: &mut Window, cx: &mut App) -> Entity<HexGridView> {
        if let Some(existing) = self.hex_grid.clone() {
            return existing;
        }
        let hex_for_grid = self.hex.clone();
        let shared = self.shared.clone();
        let grid = cx.new(|cx| HexGridView::new(hex_for_grid, shared, window, cx));
        self.hex_grid = Some(grid.clone());
        self.shared.borrow_mut().hex_grid = Some(grid.downgrade());
        grid
    }

    pub(crate) fn hex_grid_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.hex_grid()
            .is_some_and(|grid| grid.read(cx).has_focus(window, cx))
    }

    pub(crate) fn selection_seq(&self) -> u64 {
        self.selection_seq.get()
    }

    pub(crate) fn is_selecting(&self, cx: &App) -> bool {
        self.state().read(cx).is_selecting()
    }

    pub(crate) fn is_readonly_preview(&self) -> bool {
        let s = self.shared.borrow();
        s.show_non_printable || self.render_active || s.hide_headers
    }

    #[cfg(test)]
    pub(crate) fn selected_bytes(&self, cx: &App) -> Option<(Vec<u8>, bool)> {
        let active_entity = self.state();
        let active = active_entity.read(cx);
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
        let mode = {
            let s = self.shared.borrow();
            if s.pretty_active {
                MessageView::Raw
            } else {
                s.view
            }
        };
        match mode {
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
        let (view, pretty_active) = {
            let s = self.shared.borrow();
            (s.view, s.pretty_active)
        };
        let (raw_bytes, source, can_write_back) = if self.is_readonly_preview() {
            if selected.bytes().all(|b| b.is_ascii_whitespace()) {
                return None;
            }
            (selected.as_bytes().to_vec(), SelectionSource::Utf8, false)
        } else {
            match if pretty_active {
                MessageView::Raw
            } else {
                view
            } {
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
                || (view == MessageView::Hex && self.hex_grid_has_focus(w, cx))
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
            writeback_editor: can_write_back.then_some(active_entity),
        })
    }

    pub fn state(&self) -> Entity<EditorState> {
        self.shared.borrow().active_editor(&self.raw, &self.hex)
    }

    pub fn current_byte_len(&self, cx: &App) -> usize {
        if self.pretty_active() {
            let raw_state = self.raw.read(cx);
            if self.pretty_baseline_revision == Some(raw_state.document_revision())
                || self
                    .pretty_baseline_text
                    .as_deref()
                    .is_some_and(|baseline| raw_state.value().as_ref() == baseline)
            {
                return self.shared.borrow().wire_bytes.len();
            }
            return raw_state.text().len();
        }
        match self.view() {
            MessageView::Raw => {
                let raw_state = self.raw.read(cx);
                if self.raw_baseline_revision == Some(raw_state.document_revision())
                    || self
                        .raw_baseline_text
                        .as_deref()
                        .is_some_and(|baseline| raw_state.value().as_ref() == baseline)
                {
                    return self.shared.borrow().wire_bytes.len();
                }
                raw_state.text().len()
            }
            MessageView::Hex => {
                let hex_state = self.hex.read(cx);
                let rev = hex_state.document_revision();
                let s = self.shared.borrow();
                if s.hex_synced_revision == Some(rev) {
                    return s.wire_bytes.len();
                }
                let hex_len = hex_state.text().len();
                if hex_len == 0 {
                    0
                } else {
                    hex_len.saturating_add(1) / 3
                }
            }
        }
    }

    pub fn selected_byte_len(&self, cx: &App) -> usize {
        let active_entity = self.state();
        let active = active_entity.read(cx);
        let range = active.selected_range();
        let len = range.len();
        if len == 0 {
            return 0;
        }
        if self.is_readonly_preview() || self.pretty_active() || self.view() == MessageView::Raw {
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
        self.shared.borrow_mut().hex_synced_revision = None;
        if let Some(grid) = self.hex_grid() {
            grid.update(cx, |grid, cx| {
                grid.reset_view_state(None, cx);
            });
        }
    }

    fn clear_non_printable_view(&self, window: &mut Window, cx: &mut App) {
        let Some(non_printable) = self.shared.borrow().non_printable.clone() else {
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
        let Some(header_preview) = self.shared.borrow().header_preview.clone() else {
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
        let (header_preview, non_printable) = {
            let s = self.shared.borrow();
            (s.header_preview.clone(), s.non_printable.clone())
        };
        let hex_grid = self.hex_grid();
        header_preview
            .as_ref()
            .is_some_and(|ed| ed.focus_handle(cx).is_focused(window))
            || non_printable
                .as_ref()
                .is_some_and(|ed| ed.focus_handle(cx).is_focused(window))
            || self.raw.focus_handle(cx).is_focused(window)
            || self.hex.focus_handle(cx).is_focused(window)
            || hex_grid
                .as_ref()
                .is_some_and(|grid| grid.read(cx).has_focus(window, cx))
            || self.search.focus_handle(cx).is_focused(window)
            || self.replace.focus_handle(cx).is_focused(window)
    }

    pub(crate) fn hex_selected_byte_range(&self, cx: &App) -> Option<Range<usize>> {
        let grid_range = self
            .hex_grid()
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
        let hex_rev = self.hex.read(cx).document_revision();
        let already_synced = self.shared.borrow().hex_synced_revision == Some(hex_rev);
        if !already_synced {
            let current_bytes = self.bytes(window, cx)?;
            if byte_range.start > byte_range.end || byte_range.end > current_bytes.len() {
                return Err("目标 Hex 选区超出当前报文字节范围。".into());
            }
            {
                let mut shared = self.shared.borrow_mut();
                shared.wire_bytes = current_bytes;
                shared.hex_synced_revision = Some(hex_rev);
            }
            let hex_grid = self.ensure_hex_grid(window, cx);
            hex_grid.update(cx, |grid, cx| {
                grid.reset_view_state(Some(byte_range.clone()), cx);
            });
            let result = hex_grid.update(cx, |grid, cx| {
                grid.replace_byte_range(byte_range, replacement, window, cx)
            });
            return Ok(result);
        }
        let wire_len = self.shared.borrow().wire_bytes.len();
        if byte_range.start > byte_range.end || byte_range.end > wire_len {
            return Err("目标 Hex 选区超出当前报文字节范围。".into());
        }
        let hex_grid = self.ensure_hex_grid(window, cx);
        let result = hex_grid.update(cx, |grid, cx| {
            grid.replace_byte_range(byte_range, replacement, window, cx)
        });
        Ok(result)
    }

    pub fn load(&mut self, bytes: &[u8], window: &mut Window, cx: &mut App) {
        self.shared.borrow_mut().wire_bytes = bytes.to_vec();
        self.pretty_baseline_text = None;
        self.pretty_baseline_revision = None;
        self.raw_baseline_text = None;
        self.raw_baseline_revision = None;
        let charset_pref = crate::theme::message_editor_charset();
        let (charset, decoded) = decode_message_bytes_with_charset(bytes);
        self.raw_display_charset = charset;
        self.raw_charset_pref = charset_pref.clone();
        let can_display_text = decoded.is_some();
        let (charset_mode, _) = charset_pref;
        let hide_headers = self.hide_headers_override.unwrap_or_else(|| {
            !bytes.is_empty()
                && (can_display_text || self.show_non_printable())
                && crate::theme::hide_headers_by_default(bytes.starts_with(b"HTTP/"))
        });
        let hide_headers = if !can_display_text && !self.show_non_printable() {
            false
        } else {
            hide_headers
        };
        {
            let mut shared = self.shared.borrow_mut();
            shared.hide_headers = hide_headers;
            shared.pretty_active = false;
        }
        self.header_preview_error = None;
        self.render_active = false;
        self.pending_replace_all = None;
        match decoded {
            Some(text) => {
                self.shared.borrow_mut().view = MessageView::Raw;
                self.auto_hex_fallback = false;
                if !bytes.is_empty()
                    && !self.show_non_printable()
                    && charset_mode != 2
                    && std::str::from_utf8(bytes).is_ok()
                    && crate::theme::pretty_print_by_default()
                    && let Ok(pretty_msg) = mio_core::format_pretty_message_with_line_map(bytes)
                {
                    let formatted = pretty_msg.text;
                    let pretty_mapping = pretty_msg.line_numbers;
                    self.pretty_baseline_text = Some(formatted.clone());
                    let rev = self.raw.update(cx, |state, cx| {
                        state.set_value(formatted, window, cx);
                        state.set_line_number_mapping(pretty_mapping, cx);
                        state.document_revision()
                    });
                    self.pretty_baseline_revision = Some(rev);
                    self.shared.borrow_mut().pretty_active = true;
                } else {
                    self.raw_baseline_text = Some(text.clone());
                    let rev = self.raw.update(cx, |state, cx| {
                        state.set_value(text, window, cx);
                        state.set_line_number_mapping(None, cx);
                        state.document_revision()
                    });
                    self.raw_baseline_revision = Some(rev);
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
                {
                    let mut shared = self.shared.borrow_mut();
                    shared.hex_synced_revision = Some(rev);
                    shared.view = MessageView::Hex;
                }
                let hex_grid = self.ensure_hex_grid(window, cx);
                hex_grid.update(cx, |grid, cx| {
                    grid.reset_view_state(None, cx);
                });
                self.auto_hex_fallback = true;
            }
        }
        if self.show_non_printable() {
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
        if hide_headers && let Err(error) = self.refresh_header_preview(window, cx) {
            self.header_preview_error = Some(error.clone());
            self.set_header_preview_content(error, Vec::new(), None, window, cx);
        } else if !hide_headers {
            self.clear_header_preview_view(window, cx);
        }
        let _ = self.update_search(None, window, cx);
    }

    pub(crate) fn apply_editor_preferences(&mut self, window: &mut Window, cx: &mut App) {
        let soft_wrap = crate::theme::editor_soft_wrap();
        self.soft_wrap = soft_wrap;
        let (header_preview, non_printable) = {
            let s = self.shared.borrow();
            (s.header_preview.clone(), s.non_printable.clone())
        };
        for editor in [header_preview.as_ref(), non_printable.as_ref(), Some(&self.raw)]
            .into_iter()
            .flatten()
        {
            editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        }
        let case_sensitive = crate::theme::highlight_search_case_sensitive_default();
        let regex = crate::theme::message_editor_search_regex_default();
        let auto_scroll = crate::theme::message_editor_search_autoscroll_default();
        let search_changed = {
            let mut s = self.shared.borrow_mut();
            let mut changed = false;
            if s.case_sensitive != case_sensitive {
                s.case_sensitive = case_sensitive;
                changed = true;
            }
            if s.regex != regex {
                s.regex = regex;
                changed = true;
            }
            if s.auto_scroll_to_match != auto_scroll {
                s.auto_scroll_to_match = auto_scroll;
                changed = true;
            }
            changed
        };
        if search_changed {
            self.pending_replace_all = None;
            let _ = self.update_search(None, window, cx);
        }
        if self.hide_headers_override.is_none() {
            let bytes = self.bytes_lossless(cx);
            self.shared.borrow_mut().hide_headers = !bytes.is_empty()
                && crate::theme::hide_headers_by_default(bytes.starts_with(b"HTTP/"));
        }
        if self.hide_headers() {
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
        let (header_preview, non_printable) = {
            let s = self.shared.borrow();
            (s.header_preview.clone(), s.non_printable.clone())
        };
        for editor in [header_preview.as_ref(), non_printable.as_ref(), Some(&self.raw)]
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
        let was_non_printable = self.show_non_printable();
        let next = !was_non_printable;
        let bytes = self.bytes(window, cx)?;
        let (hide_headers, pretty_active) = {
            let s = self.shared.borrow();
            (s.hide_headers, s.pretty_active)
        };
        let filtered_preview = if hide_headers {
            Some(self.format_header_preview(&bytes, pretty_active, next)?)
        } else {
            None
        };
        self.pending_replace_all = None;
        self.replace_open = false;
        let focused = self.state().focus_handle(cx).is_focused(window);
        if was_non_printable {
            self.shared.borrow_mut().show_non_printable = false;
            if let Some(non_printable) = self.shared.borrow().non_printable.clone() {
                non_printable.update(cx, |state, cx| {
                    state.set_highlighter_factory(
                        super::syntax_highlight::http_highlighter_factory(),
                        cx,
                    );
                    state.set_value("", window, cx);
                    state.close_search(cx);
                });
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
        self.shared.borrow_mut().show_non_printable = true;
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
        self.shared.borrow_mut().wire_bytes = bytes.clone();
        if self.pretty_active() {
            let (charset, decoded) = decode_message_bytes_with_charset(&bytes);
            self.raw_display_charset = charset;
            self.raw_charset_pref = crate::theme::message_editor_charset();
            if let Some(raw_text) = decoded {
                self.raw_baseline_text = Some(raw_text.clone());
                if self.raw.read(cx).value().as_ref() != raw_text {
                    self.raw
                        .update(cx, |state, cx| state.set_value(raw_text, window, cx));
                }
                self.raw_baseline_revision = Some(self.raw.read(cx).document_revision());
            } else {
                self.raw_baseline_text = None;
                self.raw_baseline_revision = None;
            }
        }
        self.pending_replace_all = None;
        self.pretty_baseline_revision = None;
        self.pretty_baseline_text = None;
        self.raw
            .update(cx, |state, cx| state.set_line_number_mapping(None, cx));
        {
            let mut s = self.shared.borrow_mut();
            s.pretty_active = false;
            s.show_non_printable = false;
            s.hide_headers = false;
        }
        self.header_preview_error = None;
        self.clear_non_printable_view(window, cx);
        self.clear_header_preview_view(window, cx);
        self.render_active = true;
        let _ = self.update_search(None, window, cx);
        Ok(())
    }

    pub fn bytes_lossless(&self, cx: &App) -> Vec<u8> {
        let (view, pretty_active) = {
            let s = self.shared.borrow();
            (s.view, s.pretty_active)
        };
        if pretty_active {
            let raw_state = self.raw.read(cx);
            if self.pretty_baseline_revision == Some(raw_state.document_revision()) {
                return self.shared.borrow().wire_bytes.clone();
            }
            let text = raw_state.value();
            if self.pretty_baseline_text.as_deref() == Some(text.as_ref()) {
                return self.shared.borrow().wire_bytes.clone();
            }
            let wire = self.shared.borrow().wire_bytes.clone();
            return serialize_pretty_edit(
                text.as_ref(),
                &wire,
                self.pretty_baseline_text.as_deref(),
            );
        }
        match view {
            MessageView::Raw => {
                let raw_state = self.raw.read(cx);
                let text = raw_state.value();
                if self.raw_baseline_revision == Some(raw_state.document_revision())
                    || self.raw_baseline_text.as_deref() == Some(text.as_ref())
                {
                    return self.shared.borrow().wire_bytes.clone();
                }
                let wire = self.shared.borrow().wire_bytes.clone();
                encode_raw_text_bytes(
                    text.as_ref(),
                    self.raw_display_charset,
                    &self.raw_charset_pref,
                    &wire,
                )
            }
            MessageView::Hex => {
                let hex_state = self.hex.read(cx);
                let rev = hex_state.document_revision();
                let s = self.shared.borrow();
                if s.hex_synced_revision == Some(rev) {
                    return s.wire_bytes.clone();
                }
                hex_decode(hex_state.value().as_ref()).unwrap_or_else(|_| s.wire_bytes.clone())
            }
        }
    }

    pub fn bytes(&self, window: &mut Window, cx: &mut App) -> Result<Vec<u8>, String> {
        let (view, pretty_active) = {
            let s = self.shared.borrow();
            (s.view, s.pretty_active)
        };
        if pretty_active {
            let text = snapshot(&self.raw, window, cx).map_err(str::to_owned)?;
            let raw_rev = self.raw.read(cx).document_revision();
            if self.pretty_baseline_revision == Some(raw_rev)
                || self.pretty_baseline_text.as_deref() == Some(text.as_ref())
            {
                return Ok(self.shared.borrow().wire_bytes.clone());
            }
            let wire = self.shared.borrow().wire_bytes.clone();
            return Ok(serialize_pretty_edit(
                text.as_ref(),
                &wire,
                self.pretty_baseline_text.as_deref(),
            ));
        }
        let source = match view {
            MessageView::Raw => &self.raw,
            MessageView::Hex => &self.hex,
        };
        let text = snapshot(source, window, cx).map_err(str::to_owned)?;
        let rev = source.read(cx).document_revision();
        match view {
            MessageView::Raw => {
                if self.raw_baseline_revision == Some(rev)
                    || self.raw_baseline_text.as_deref() == Some(text.as_ref())
                {
                    return Ok(self.shared.borrow().wire_bytes.clone());
                }
                let wire = self.shared.borrow().wire_bytes.clone();
                Ok(encode_raw_text_bytes(
                    text.as_ref(),
                    self.raw_display_charset,
                    &self.raw_charset_pref,
                    &wire,
                ))
            }
            MessageView::Hex => {
                let s = self.shared.borrow();
                if s.hex_synced_revision == Some(rev) {
                    return Ok(s.wire_bytes.clone());
                }
                drop(s);
                hex_decode(&text)
            }
        }
    }

    pub fn switch(
        &mut self,
        view: MessageView,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let (cur_view, pretty_active, show_non_printable, hide_headers) = {
            let s = self.shared.borrow();
            (
                s.view,
                s.pretty_active,
                s.show_non_printable,
                s.hide_headers,
            )
        };
        if view == cur_view
            && !pretty_active
            && !show_non_printable
            && !self.render_active
            && !hide_headers
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
        self.shared.borrow_mut().wire_bytes = bytes.clone();
        let filtered_preview = if hide_headers && view == MessageView::Raw {
            Some(self.format_header_preview(&bytes, false, false)?)
        } else {
            None
        };
        if view == cur_view {
            if view == MessageView::Raw {
                let (charset, decoded) = decode_message_bytes_with_charset(&bytes);
                let raw_text = decoded.ok_or_else(|| {
                    "消息含非 UTF-8 字节，请使用 Hex 视图查看原始字节。".to_owned()
                })?;
                self.raw_display_charset = charset;
                self.raw_charset_pref = crate::theme::message_editor_charset();
                self.raw_baseline_text = Some(raw_text.clone());
                let rev = self.raw.update(cx, |state, cx| {
                    if state.value().as_ref() != raw_text {
                        state.set_value(raw_text, window, cx);
                    }
                    state.set_line_number_mapping(None, cx);
                    state.document_revision()
                });
                self.raw_baseline_revision = Some(rev);
                self.pretty_baseline_text = None;
                self.pretty_baseline_revision = None;
                self.clear_hex_view(window, cx);
            }
            self.auto_hex_fallback = false;
            {
                let mut s = self.shared.borrow_mut();
                s.pretty_active = false;
                s.show_non_printable = false;
                if view == MessageView::Hex {
                    s.hide_headers = false;
                }
            }
            self.clear_non_printable_view(window, cx);
            self.render_active = false;
            if view == MessageView::Hex {
                self.hide_headers_override = None;
                self.clear_header_preview_view(window, cx);
            } else if !hide_headers {
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
        match view {
            MessageView::Raw => {
                let (charset, decoded) = decode_message_bytes_with_charset(&bytes);
                let value = decoded.ok_or_else(|| {
                    "消息含非 UTF-8 字节，请使用 Hex 视图查看原始字节。".to_owned()
                })?;
                self.raw_display_charset = charset;
                self.raw_charset_pref = crate::theme::message_editor_charset();
                self.raw_baseline_text = Some(value.clone());
                let rev = self.raw.update(cx, |state, cx| {
                    if state.value().as_ref() != value {
                        state.set_value(value, window, cx);
                    }
                    state.set_line_number_mapping(None, cx);
                    state.document_revision()
                });
                self.raw_baseline_revision = Some(rev);
                self.pretty_baseline_text = None;
                self.pretty_baseline_revision = None;
                self.clear_hex_view(window, cx);
            }
            MessageView::Hex => {
                let value = hex_encode(&bytes);
                let rev = self.hex.update(cx, |state, cx| {
                    if state.value().as_ref() != value {
                        state.set_value(value, window, cx);
                    }
                    state.document_revision()
                });
                self.shared.borrow_mut().hex_synced_revision = Some(rev);
                let hex_grid = self.ensure_hex_grid(window, cx);
                hex_grid.update(cx, |grid, cx| {
                    grid.reset_view_state(None, cx);
                });
                self.raw_baseline_text = None;
                self.raw_baseline_revision = None;
                self.pretty_baseline_text = None;
                self.pretty_baseline_revision = None;
            }
        }
        {
            let mut s = self.shared.borrow_mut();
            s.pretty_active = false;
            s.show_non_printable = false;
            if view == MessageView::Hex {
                s.hide_headers = false;
            }
            s.view = view;
        }
        self.clear_non_printable_view(window, cx);
        self.render_active = false;
        if view == MessageView::Hex {
            self.hide_headers_override = None;
            self.header_preview_error = None;
            self.clear_header_preview_view(window, cx);
        } else if !hide_headers {
            self.clear_header_preview_view(window, cx);
        }
        if let Some((preview, badges, line_numbers)) = filtered_preview {
            self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
            self.header_preview_error = None;
        }
        self.auto_hex_fallback = false;
        self.update_search(None, window, cx)?;
        if focused {
            self.state().update(cx, |state, cx| state.focus(window, cx));
        }
        Ok(())
    }

    pub fn switch_pretty(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if self.pretty_active() && !self.show_non_printable() && !self.render_active {
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
        let pretty_msg = mio_core::format_pretty_message_with_line_map(&bytes)?;
        let formatted = pretty_msg.text;
        let pretty_mapping = pretty_msg.line_numbers;
        self.shared.borrow_mut().wire_bytes = bytes.clone();
        self.pretty_baseline_text = Some(formatted.clone());
        let hide_headers = self.hide_headers();
        let filtered_preview = if hide_headers {
            Some(self.format_header_preview(&bytes, true, false)?)
        } else {
            None
        };
        let rev = self.raw.update(cx, |state, cx| {
            if state.value().as_ref() != formatted {
                state.set_value(formatted, window, cx);
            }
            state.set_line_number_mapping(pretty_mapping, cx);
            state.document_revision()
        });
        self.pretty_baseline_revision = Some(rev);
        self.raw_baseline_text = None;
        self.raw_baseline_revision = None;
        {
            let mut s = self.shared.borrow_mut();
            s.view = MessageView::Raw;
            s.pretty_active = true;
            s.show_non_printable = false;
        }
        self.auto_hex_fallback = false;
        self.clear_non_printable_view(window, cx);
        self.clear_hex_view(window, cx);
        if !hide_headers {
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
    ) -> Result<HeaderPreviewResult, String> {
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
        if !self.hide_headers() {
            return Ok(());
        }
        let bytes = self.bytes(window, cx)?;
        let (pretty_active, show_non_printable) = {
            let s = self.shared.borrow();
            (s.pretty_active, s.show_non_printable)
        };
        let (preview, badges, line_numbers) =
            self.format_header_preview(&bytes, pretty_active, show_non_printable)?;
        self.set_header_preview_content(preview, badges, Some(line_numbers), window, cx);
        self.shared.borrow_mut().hide_headers = true;
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
        if !self.hide_headers() {
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
        let (hide_headers, view, show_non_printable, pretty_active) = {
            let s = self.shared.borrow();
            (
                s.hide_headers,
                s.view,
                s.show_non_printable,
                s.pretty_active,
            )
        };
        if hide_headers {
            self.shared.borrow_mut().hide_headers = false;
            self.header_preview_error = None;
            self.clear_header_preview_view(window, cx);
            if let Some(header_preview) = self.shared.borrow().header_preview.clone() {
                header_preview.update(cx, |state, cx| state.close_search(cx));
            }
        } else {
            if self.render_active
                || (view == MessageView::Hex && !show_non_printable && !pretty_active)
            {
                return Err("请切换到 Raw、Pretty 或显示非打印字符预览后隐藏指定标头。".into());
            }
            let bytes = self.bytes(window, cx)?;
            let (preview, badges, line_numbers, error) =
                match self.format_header_preview(&bytes, pretty_active, show_non_printable) {
                    Ok((preview, badges, line_numbers)) => (preview, badges, Some(line_numbers), None),
                    Err(error) => (error.clone(), Vec::new(), None, Some(error)),
                };
            self.set_header_preview_content(preview, badges, line_numbers, window, cx);
            self.shared.borrow_mut().hide_headers = true;
            self.header_preview_error = error;
        }
        self.pending_replace_all = None;
        self.replace_open = false;
        self.hide_headers_override = Some(self.hide_headers());
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
        if self.is_readonly_preview() || (!self.pretty_active() && self.view() != MessageView::Raw) {
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
