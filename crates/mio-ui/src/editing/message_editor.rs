use super::*;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::{
    Context, EntityId, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Render, RenderOnce, StyleRefinement, UniformListScrollHandle, WeakEntity, uniform_list,
};
use std::collections::HashMap;
use std::ops::Range;

std::thread_local! {
    static HEX_GRID_REGISTRY: RefCell<HashMap<EntityId, WeakEntity<HexGridView>>> =
        RefCell::new(HashMap::new());
}

pub(crate) fn hex_text_range_to_byte_range(
    text_range: Range<usize>,
    total_bytes: usize,
) -> Option<Range<usize>> {
    if text_range.is_empty() || total_bytes == 0 {
        return None;
    }
    let start_byte = text_range.start.saturating_add(1) / 3;
    let end_byte = (text_range.end.saturating_add(1) / 3).min(total_bytes);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageDisplayCharset {
    Utf8,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    EucKr,
    Iso8859_1,
    Windows1252,
    UsAscii,
    Other(&'static encoding_rs::Encoding),
}

impl MessageDisplayCharset {
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        let lower = label.trim().to_ascii_lowercase();
        match lower.as_str() {
            "" | "utf-8" | "utf8" => Some(Self::Utf8),
            "gbk" | "gb2312" | "cp936" | "ms936" | "x-gbk" => Some(Self::Gbk),
            "gb18030" => Some(Self::Gb18030),
            "big5" | "big5-hkscs" | "cn-big5" | "xx-big5" | "cp950" => Some(Self::Big5),
            "shift_jis" | "shift-jis" | "sjis" | "ms932" | "cp932" | "windows-31j" | "x-sjis" => {
                Some(Self::ShiftJis)
            }
            "euc-kr" | "euckr" | "cp949" | "ks_c_5601-1987" | "windows-949" => Some(Self::EucKr),
            "iso-8859-1" | "iso_8859-1" | "latin1" | "latin-1" => Some(Self::Iso8859_1),
            "windows-1252" | "cp1252" => Some(Self::Windows1252),
            "us-ascii" | "ascii" => Some(Self::UsAscii),
            other => encoding_rs::Encoding::for_label(other.as_bytes()).map(Self::Other),
        }
    }

    pub(crate) fn decode(self, bytes: &[u8]) -> Option<String> {
        match self {
            Self::Utf8 => std::str::from_utf8(bytes).ok().map(str::to_owned),
            Self::Iso8859_1 => Some(decode_latin1(bytes)),
            Self::UsAscii => Some(decode_ascii(bytes)),
            Self::Gbk => Some(
                encoding_rs::GBK
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Gb18030 => Some(
                encoding_rs::GB18030
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Big5 => Some(
                encoding_rs::BIG5
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::ShiftJis => Some(
                encoding_rs::SHIFT_JIS
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::EucKr => Some(
                encoding_rs::EUC_KR
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Windows1252 => Some(
                encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Other(enc) => Some(enc.decode_without_bom_handling(bytes).0.into_owned()),
        }
    }

    pub(crate) fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 | Self::UsAscii => text.as_bytes().to_vec(),
            Self::Iso8859_1 => {
                let mut out = Vec::with_capacity(text.len());
                let mut buf = [0u8; 4];
                for ch in text.chars() {
                    if (ch as u32) <= 0xFF {
                        out.push(ch as u8);
                    } else {
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                }
                out
            }
            Self::Gbk => encoding_rs::GBK.encode(text).0.into_owned(),
            Self::Gb18030 => encoding_rs::GB18030.encode(text).0.into_owned(),
            Self::Big5 => encoding_rs::BIG5.encode(text).0.into_owned(),
            Self::ShiftJis => encoding_rs::SHIFT_JIS.encode(text).0.into_owned(),
            Self::EucKr => encoding_rs::EUC_KR.encode(text).0.into_owned(),
            Self::Windows1252 => encoding_rs::WINDOWS_1252.encode(text).0.into_owned(),
            Self::Other(enc) => enc.encode(text).0.into_owned(),
        }
    }
}

fn extract_header_charset(bytes: &[u8]) -> Option<String> {
    let (_, headers) = parse_http_head_info(bytes)?;
    for header in headers {
        if !header.name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        let value = std::str::from_utf8(&header.value).ok()?;
        for part in value.split(';').skip(1) {
            let trimmed = part.trim();
            if let Some((key, raw_val)) = trimmed.split_once('=')
                && key.trim().eq_ignore_ascii_case("charset")
            {
                let cleaned = raw_val
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .trim();
                if !cleaned.is_empty() {
                    return Some(cleaned.to_ascii_lowercase());
                }
            }
        }
    }
    None
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn decode_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if b < 0x80 { char::from(b) } else { '\u{FFFD}' })
        .collect()
}

fn decode_named_charset(bytes: &[u8], charset_lower: &str) -> Option<String> {
    match MessageDisplayCharset::from_label(charset_lower) {
        Some(charset) => charset.decode(bytes),
        None => std::str::from_utf8(bytes).ok().map(str::to_owned),
    }
}

fn active_display_charset(original_wire_bytes: &[u8]) -> MessageDisplayCharset {
    let (mode, specific) = crate::theme::message_editor_charset();
    match mode {
        0 => extract_header_charset(original_wire_bytes)
            .and_then(|label| MessageDisplayCharset::from_label(&label))
            .unwrap_or(MessageDisplayCharset::Utf8),
        1 => MessageDisplayCharset::Utf8,
        2 => MessageDisplayCharset::Iso8859_1,
        _ => MessageDisplayCharset::from_label(&specific).unwrap_or(MessageDisplayCharset::Utf8),
    }
}

fn decode_message_bytes(bytes: &[u8]) -> Option<String> {
    let (mode, specific) = crate::theme::message_editor_charset();
    match mode {
        // 0: Recognize automatically based on message headers
        0 => {
            if let Some(charset) = extract_header_charset(bytes) {
                decode_named_charset(bytes, &charset)
            } else {
                std::str::from_utf8(bytes).ok().map(str::to_owned)
            }
        }
        // 1: Use platform default (UTF-8)
        1 => std::str::from_utf8(bytes).ok().map(str::to_owned),
        // 2: Display as raw bytes (1:1 ISO-8859-1 byte view)
        2 => Some(decode_latin1(bytes)),
        // 3: Use a specific character set
        _ => decode_named_charset(bytes, &specific.trim().to_ascii_lowercase()),
    }
}

fn encode_raw_text_bytes(text: &str, original_wire_bytes: &[u8]) -> Vec<u8> {
    if decode_message_bytes(original_wire_bytes).as_deref() == Some(text) {
        return original_wire_bytes.to_vec();
    }
    active_display_charset(original_wire_bytes).encode(text)
}

fn pretty_line_number_mapping(raw_bytes: &[u8], formatted: &str) -> Option<Vec<usize>> {
    const MAX_RAW_LINE_SCAN_BYTES: usize = 8 * 1024;
    const MAX_TOTAL_SCAN_BYTES: usize = 256 * 1024;

    let raw_text = std::str::from_utf8(raw_bytes).ok()?;
    let raw_lines: Vec<&str> = raw_text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let pretty_lines: Vec<&str> = formatted.split('\n').collect();
    if raw_lines.is_empty() || pretty_lines.is_empty() {
        return None;
    }
    if raw_lines.len() == pretty_lines.len()
        && raw_lines.iter().zip(&pretty_lines).all(|(r, p)| *r == *p)
    {
        return None;
    }
    let mut mapping = Vec::with_capacity(pretty_lines.len());
    let mut raw_idx = 0usize;
    let mut byte_pos = 0usize;
    let mut last_assigned = 0usize;
    let mut scanned_bytes = 0usize;
    let mut fast_fallback = false;

    for pretty_line in &pretty_lines {
        if fast_fallback {
            let line_no = raw_idx + 1;
            if raw_idx < raw_lines.len() && line_no > last_assigned {
                mapping.push(line_no);
                last_assigned = line_no;
            } else {
                mapping.push(0);
            }
            continue;
        }
        if byte_pos == 0 && raw_idx < raw_lines.len() && raw_lines[raw_idx] == *pretty_line {
            let line_no = raw_idx + 1;
            if line_no > last_assigned {
                mapping.push(line_no);
                last_assigned = line_no;
            } else {
                mapping.push(0);
            }
            raw_idx += 1;
            continue;
        }
        let needle = pretty_line.trim();
        if needle.is_empty() {
            if raw_idx < raw_lines.len() {
                if byte_pos > 0 && raw_idx + 1 < raw_lines.len() {
                    raw_idx += 1;
                    byte_pos = 0;
                }
                while raw_idx < raw_lines.len() && !raw_lines[raw_idx].trim().is_empty() {
                    raw_idx += 1;
                    byte_pos = 0;
                }
                let line_no = raw_idx + 1;
                if raw_idx < raw_lines.len() && line_no > last_assigned {
                    mapping.push(line_no);
                    last_assigned = line_no;
                    raw_idx += 1;
                    byte_pos = 0;
                } else {
                    mapping.push(0);
                }
            } else {
                mapping.push(0);
            }
            continue;
        }
        if raw_idx < raw_lines.len()
            && (raw_lines[raw_idx].len() > MAX_RAW_LINE_SCAN_BYTES
                || scanned_bytes >= MAX_TOTAL_SCAN_BYTES)
        {
            fast_fallback = true;
            let line_no = raw_idx + 1;
            if line_no > last_assigned {
                mapping.push(line_no);
                last_assigned = line_no;
            } else {
                mapping.push(0);
            }
            continue;
        }
        // Search current raw line from byte_pos, then subsequent raw lines.
        let mut found: Option<(usize, usize)> = None;
        if raw_idx < raw_lines.len() {
            let tail = raw_lines[raw_idx].get(byte_pos..).unwrap_or("");
            scanned_bytes = scanned_bytes.saturating_add(tail.len());
            if let Some(rel) = tail.find(needle) {
                found = Some((raw_idx, byte_pos + rel + needle.len()));
            }
        }
        if found.is_none() {
            for (cand_idx, raw_line) in raw_lines.iter().enumerate().skip(raw_idx + 1) {
                if raw_line.len() > MAX_RAW_LINE_SCAN_BYTES
                    || scanned_bytes >= MAX_TOTAL_SCAN_BYTES
                {
                    fast_fallback = true;
                    if raw_line.len() > MAX_RAW_LINE_SCAN_BYTES {
                        raw_idx = cand_idx;
                    }
                    break;
                }
                scanned_bytes = scanned_bytes.saturating_add(raw_line.len());
                if let Some(rel) = raw_line.find(needle) {
                    found = Some((cand_idx, rel + needle.len()));
                    break;
                }
            }
        }
        if fast_fallback {
            let line_no = raw_idx + 1;
            if raw_idx < raw_lines.len() && line_no > last_assigned {
                mapping.push(line_no);
                last_assigned = line_no;
            } else {
                mapping.push(0);
            }
            continue;
        }
        if let Some((hit_idx, next_pos)) = found {
            let line_no = hit_idx + 1;
            if line_no > last_assigned {
                mapping.push(line_no);
                last_assigned = line_no;
            } else {
                mapping.push(0);
            }
            raw_idx = hit_idx;
            byte_pos = next_pos;
        } else {
            mapping.push(0);
        }
    }
    // Only apply custom mapping if Pretty actually introduced synthetic lines (zeros).
    if mapping.len() == pretty_lines.len() && mapping.contains(&0) {
        Some(mapping)
    } else {
        None
    }
}

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
                let error = active.update(cx, |state, cx| {
                    if query.is_empty() {
                        let _ = state.set_search_regex("", case_insensitive, regex, cx);
                        state.close_search(cx);
                        return None;
                    }
                    if let Err(error) =
                        state.set_search_regex(query.to_string(), case_insensitive, regex, cx)
                    {
                        return Some(format!("搜索正则表达式无效：{error}"));
                    }
                    match step {
                        Some(true) => {
                            state.next_search_match(cx);
                        }
                        Some(false) => {
                            state.previous_search_match(cx);
                        }
                        None => {
                            if auto_scroll_cell.get() {
                                state.reveal_current_search_match(cx);
                            }
                        }
                    }
                    None
                });
                *search_error.borrow_mut() = error;
                cx.notify();
            })
        };
        let observe_editor_change = |editor: &Entity<EditorState>,
                                     auto_scroll_cell: Rc<Cell<bool>>,
                                     cx: &mut gpui_kit::Context<V>| {
            let auto_scroll_change = auto_scroll_cell.clone();
            let sub = cx.subscribe(editor, move |_, editor, event, cx| {
                if matches!(event, InputEvent::Change)
                    && editor.read(cx).search_session().is_active()
                {
                    let auto_scroll = auto_scroll_change.get();
                    editor.update(cx, |state, cx| {
                        state.sync_search_matcher();
                        if auto_scroll {
                            state.reveal_current_search_match(cx);
                        }
                    });
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
        let raw_subscription = observe_editor_change(&raw, auto_scroll_cell.clone(), cx);
        let hex_subscription = observe_editor_change(&hex, auto_scroll_cell.clone(), cx);
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
        HEX_GRID_REGISTRY.with(|reg| {
            reg.borrow_mut().insert(self.hex.entity_id(), grid.downgrade());
        });
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

    pub fn content_revision_key(
        &self,
        cx: &App,
    ) -> (gpui_kit::EntityId, u64, MessageView, bool, bool) {
        let active = self.state();
        (
            active.entity_id(),
            active.read(cx).document_revision(),
            self.view,
            self.pretty_active,
            self.hide_headers,
        )
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
        let text_range = self.hex.read(cx).selected_range();
        let total_bytes = self.current_byte_len(cx);
        hex_text_range_to_byte_range(text_range, total_bytes).or(grid_range)
    }

    pub(crate) fn replace_hex_byte_range(
        &mut self,
        byte_range: Range<usize>,
        replacement: &[u8],
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(u64, Range<usize>), String> {
        let mut bytes = self.bytes(window, cx)?;
        if byte_range.start > byte_range.end || byte_range.end > bytes.len() {
            return Err("目标 Hex 选区超出当前报文字节范围。".into());
        }
        bytes.splice(byte_range.clone(), replacement.iter().copied());
        self.original_wire_bytes = bytes.clone();
        let new_byte_end = byte_range.start + replacement.len();
        let new_byte_range = byte_range.start..new_byte_end;
        let new_text_range = hex_byte_range_to_text_range(new_byte_range.clone());
        let encoded = hex_encode(&bytes);
        let next_rev = self.hex.update(cx, |state, cx| {
            state.set_value(encoded, window, cx);
            state.set_selected_range(new_text_range.clone(), cx);
            state.document_revision()
        });
        let hex_grid = self.ensure_hex_grid(window, cx);
        hex_grid.update(cx, |grid, cx| {
            grid.sync_bytes_and_selection(
                bytes,
                (!new_byte_range.is_empty()).then_some(new_byte_range),
                next_rev,
                cx,
            );
        });
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

fn parse_http_head_info(bytes: &[u8]) -> Option<(usize, Vec<mio_core::Header>)> {
    if bytes.is_empty() {
        return None;
    }
    let newline = bytes.iter().position(|&b| b == b'\n')?;
    let eol_len = if newline > 0 && bytes[newline - 1] == b'\r' {
        2
    } else {
        1
    };
    let first_line = &bytes[..newline + 1 - eol_len];
    let normalized = if let Some(rest) = first_line.strip_prefix(b"HTTP/2.0 ") {
        let mut v = b"HTTP/1.1 ".to_vec();
        v.extend_from_slice(rest);
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else if let Some(rest) = first_line.strip_prefix(b"HTTP/2 ") {
        let mut v = b"HTTP/1.1 ".to_vec();
        v.extend_from_slice(rest);
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else if let Some(prefix) = first_line.strip_suffix(b" HTTP/2.0") {
        let mut v = prefix.to_vec();
        v.extend_from_slice(b" HTTP/1.1");
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else if let Some(prefix) = first_line.strip_suffix(b" HTTP/2") {
        let mut v = prefix.to_vec();
        v.extend_from_slice(b" HTTP/1.1");
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else {
        None
    };
    let input = normalized.as_deref().unwrap_or(bytes);
    let added = input.len() - bytes.len();
    let mut headers = vec![httparse::EMPTY_HEADER; 4096];
    let mut config = httparse::ParserConfig::default();
    config
        .allow_multiple_spaces_in_request_line_delimiters(true)
        .allow_multiple_spaces_in_response_status_delimiters(true)
        .allow_space_before_first_header_name(true)
        .allow_obsolete_multiline_headers_in_responses(true);
    let (status, parsed_headers) = if input.starts_with(b"HTTP/") {
        let mut resp = httparse::Response::new(&mut headers);
        let status = config.parse_response(&mut resp, input).ok()?;
        let list = resp
            .headers
            .iter()
            .map(|h| mio_core::Header {
                name: h.name.to_owned(),
                value: h.value.to_vec(),
            })
            .collect();
        (status, list)
    } else {
        let mut req = httparse::Request::new(&mut headers);
        let status = config.parse_request(&mut req, input).ok()?;
        let list = req
            .headers
            .iter()
            .map(|h| mio_core::Header {
                name: h.name.to_owned(),
                value: h.value.to_vec(),
            })
            .collect();
        (status, list)
    };
    match status {
        httparse::Status::Complete(offset) => Some((offset - added, parsed_headers)),
        httparse::Status::Partial => None,
    }
}

fn serialize_pretty_edit(
    edited: &str,
    original_wire: &[u8],
    baseline: Option<&str>,
) -> Vec<u8> {
    let Some((body_offset, edited_headers)) = parse_http_head_info(edited.as_bytes()) else {
        return edited.as_bytes().to_vec();
    };
    let head_part = &edited[..body_offset];
    let body_part = &edited[body_offset..];
    let normalized_head: String = head_part
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\r\n");
    let mut out = Vec::with_capacity(normalized_head.len() + 4 + body_part.len());
    out.extend_from_slice(normalized_head.as_bytes());
    out.extend_from_slice(b"\r\n\r\n");
    let orig_info = parse_http_head_info(original_wire);
    if let Some(base) = baseline
        && let Some((base_body_offset, _)) = parse_http_head_info(base.as_bytes())
        && &base[base_body_offset..] == body_part
        && let Some((orig_body_offset, ref orig_headers)) = orig_info
        && mio_core::is_chunked_headers(&edited_headers) == mio_core::is_chunked_headers(orig_headers)
    {
        out.extend_from_slice(&original_wire[orig_body_offset..]);
        return out;
    }
    let mut body_text = body_part;
    let mut extracted_trailers: Vec<mio_core::Header> = Vec::new();
    if mio_core::is_chunked_headers(&edited_headers)
        && let Some((orig_off, ref orig_hdrs)) = orig_info
        && mio_core::is_chunked_headers(orig_hdrs)
        && let Ok((_, orig_trailers)) =
            mio_core::decode_http_body(orig_hdrs, &original_wire[orig_off..])
        && !orig_trailers.is_empty()
    {
        let mut trailer_suffix = String::new();
        for t in &orig_trailers {
            trailer_suffix.push_str(&format!(
                "{}: {}\n",
                t.name,
                String::from_utf8_lossy(&t.value)
            ));
        }
        if let Some(stripped) = body_text.strip_suffix(&trailer_suffix) {
            body_text = stripped.strip_suffix('\n').unwrap_or(stripped);
            extracted_trailers = orig_trailers;
        }
    }
    let payload_bytes = body_text.as_bytes().to_vec();
    if mio_core::is_chunked_headers(&edited_headers)
        && mio_core::decode_http_body(&edited_headers, &payload_bytes).is_err()
    {
        if !payload_bytes.is_empty() {
            out.extend_from_slice(format!("{:x}\r\n", payload_bytes.len()).as_bytes());
            out.extend_from_slice(&payload_bytes);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n");
        for t in &extracted_trailers {
            out.extend_from_slice(t.name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(&t.value);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
    } else {
        out.extend_from_slice(&payload_bytes);
    }
    out
}

type EditorContextMenuBuilder =
    Rc<dyn Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static>;

#[derive(IntoElement)]
pub(crate) struct MessageEditorElement {
    state: Entity<EditorState>,
    style: StyleRefinement,
    readonly: bool,
    aria_label: Option<SharedString>,
    context_menu: Option<EditorContextMenuBuilder>,
}

impl MessageEditorElement {
    pub(crate) fn new(state: &Entity<EditorState>) -> Self {
        Self {
            state: state.clone(),
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
        let grid_entity = HEX_GRID_REGISTRY.with(|reg| {
            reg.borrow()
                .get(&self.state.entity_id())
                .and_then(WeakEntity::upgrade)
        });
        let caller_style = self.style;
        let mut base_editor = Editor::new(&self.state);
        if grid_entity.is_none() {
            *base_editor.style() = caller_style.clone();
        }
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
        if let Some(label) = self.aria_label.clone() {
            base_editor = base_editor.aria_label(label);
        }
        if let Some(menu) = self.context_menu.clone() {
            let menu_fn = menu.clone();
            base_editor = base_editor.context_menu(move |m, w, cx| menu_fn(m, w, cx));
        }
        let Some(grid) = grid_entity else {
            return base_editor.into_any_element();
        };
        grid.update(cx, |g, _| {
            g.readonly = self.readonly;
            g.context_menu = self.context_menu;
        });
        let _ = base_editor;
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
    pub(crate) bytes: Vec<u8>,
    pub(crate) selected_range: Option<Range<usize>>,
    pub(crate) selection_anchor: Option<usize>,
    pub(crate) active_cursor: Option<usize>,
    pub(crate) low_nibble_pending: bool,
    pub(crate) editing_ascii: bool,
    pub(crate) hovered_byte: Option<usize>,
    pub(crate) dragging: bool,
    pub(crate) readonly: bool,
    pub(crate) last_synced_revision: u64,
    pub(crate) scroll_handle: UniformListScrollHandle,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) context_menu: Option<EditorContextMenuBuilder>,
    _subscriptions: [Subscription; 2],
}

impl HexGridView {
    pub(crate) fn new(
        hex_editor: Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let byte_input = cx.new(|cx| InputState::new(window, cx).placeholder("Hex (如 41)"));
        let focus_handle = cx.focus_handle();
        let sub_change = cx.subscribe(&hex_editor, |this, editor, event, cx| {
            if matches!(event, InputEvent::Change) {
                let rev = editor.read(cx).document_revision();
                if rev != this.last_synced_revision {
                    let text = editor.read(cx).value();
                    if let Ok(decoded) = hex_decode(text.as_ref()) {
                        this.bytes = decoded;
                        this.last_synced_revision = rev;
                        if let Some(r) = &this.selected_range
                            && r.end > this.bytes.len()
                        {
                            this.selected_range = None;
                        }
                        cx.notify();
                    }
                }
            }
        });
        let sub_obs = cx.observe(&hex_editor, |this, editor, cx| {
            let text_range = editor.read(cx).selected_range();
            let mapped = hex_text_range_to_byte_range(text_range, this.bytes.len());
            if mapped != this.selected_range {
                this.selected_range = mapped.clone();
                this.active_cursor = mapped.map(|r| r.start);
                this.low_nibble_pending = false;
                cx.notify();
            }
        });
        Self {
            hex_editor,
            byte_input,
            bytes: Vec::new(),
            selected_range: None,
            selection_anchor: None,
            active_cursor: None,
            low_nibble_pending: false,
            editing_ascii: false,
            hovered_byte: None,
            dragging: false,
            readonly: false,
            last_synced_revision: 0,
            scroll_handle: UniformListScrollHandle::new(),
            focus_handle,
            context_menu: None,
            _subscriptions: [sub_change, sub_obs],
        }
    }

    pub(crate) fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window) || self.byte_input.focus_handle(cx).is_focused(window)
    }

    pub(crate) fn selected_byte_range(&self) -> Option<Range<usize>> {
        self.selected_range
            .clone()
            .filter(|r| r.start < r.end && r.end <= self.bytes.len())
    }

    pub(crate) fn sync_bytes_and_selection(
        &mut self,
        bytes: Vec<u8>,
        selected_range: Option<Range<usize>>,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        self.bytes = bytes;
        self.selected_range = selected_range.clone();
        self.active_cursor = selected_range.map(|r| r.start);
        self.low_nibble_pending = false;
        self.last_synced_revision = revision;
        cx.notify();
    }

    fn commit_bytes_to_editor(
        &mut self,
        next_selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let encoded = hex_encode(&self.bytes);
        let text_range = next_selection
            .clone()
            .map(hex_byte_range_to_text_range)
            .unwrap_or(0..0);
        self.selected_range = next_selection.clone();
        self.active_cursor = next_selection.map(|r| r.start);
        let rev = self.hex_editor.update(cx, |state, cx| {
            state.set_value(encoded, window, cx);
            state.set_selected_range(text_range, cx);
            state.document_revision()
        });
        self.last_synced_revision = rev;
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
        if byte_ix >= self.bytes.len() {
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
        let end = (anchor.max(byte_ix) + 1).min(self.bytes.len());
        let range = start..end;
        self.selected_range = Some(range.clone());
        self.active_cursor = Some(byte_ix);
        let text_range = hex_byte_range_to_text_range(range);
        self.hex_editor.update(cx, |state, cx| {
            state.set_selected_range(text_range, cx);
        });
        let val_hex = format!("{:02X}", self.bytes[byte_ix]);
        self.byte_input.update(cx, |inp, cx| {
            inp.set_value(val_hex, window, cx);
        });
        cx.notify();
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
        if self.readonly || self.bytes.is_empty() {
            return;
        }
        let target = self
            .selected_byte_range()
            .or_else(|| self.active_cursor.map(|c| c..c + 1))
            .unwrap_or(0..1);
        let cur_val = self.bytes.get(target.start).copied().unwrap_or(0);
        let replacement = self.parse_input_or_default(cur_val.wrapping_add(1), cx);
        let start = target.start;
        self.bytes.splice(target, replacement.iter().copied());
        let end = (start + replacement.len()).min(self.bytes.len());
        self.low_nibble_pending = false;
        self.commit_bytes_to_editor((start < end).then_some(start..end), window, cx);
    }

    pub(crate) fn insert_byte_before(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let insert_at = self
            .selected_byte_range()
            .map(|r| r.start)
            .or(self.active_cursor)
            .unwrap_or(0)
            .min(self.bytes.len());
        let inserted = self.parse_input_or_default(0x00, cx);
        let len = inserted.len();
        self.bytes.splice(insert_at..insert_at, inserted);
        self.low_nibble_pending = false;
        self.commit_bytes_to_editor(Some(insert_at..insert_at + len), window, cx);
    }

    pub(crate) fn append_byte(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        let start = self.bytes.len();
        let appended = self.parse_input_or_default(0x00, cx);
        let len = appended.len();
        self.bytes.extend_from_slice(&appended);
        self.low_nibble_pending = false;
        self.commit_bytes_to_editor(Some(start..start + len), window, cx);
    }

    pub(crate) fn delete_selected_bytes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly || self.bytes.is_empty() {
            return;
        }
        let Some(range) = self
            .selected_byte_range()
            .or_else(|| self.active_cursor.map(|c| c..c + 1))
        else {
            return;
        };
        if range.end > self.bytes.len() {
            return;
        }
        let start = range.start;
        self.bytes.drain(range);
        self.low_nibble_pending = false;
        let next_sel = if self.bytes.is_empty() {
            None
        } else {
            let idx = start.min(self.bytes.len() - 1);
            Some(idx..idx + 1)
        };
        self.commit_bytes_to_editor(next_sel, window, cx);
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.alt {
            if ks.key.eq_ignore_ascii_case("a") && !self.bytes.is_empty() {
                let all = 0..self.bytes.len();
                self.selected_range = Some(all.clone());
                self.active_cursor = Some(0);
                let text_range = hex_byte_range_to_text_range(all);
                self.hex_editor.update(cx, |s, cx| {
                    s.set_selected_range(text_range, cx);
                });
                cx.notify();
            }
            return;
        }
        match ks.key.as_str() {
            "left" if !self.bytes.is_empty() => {
                let cur = self.active_cursor.unwrap_or(0).saturating_sub(1);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "right" if !self.bytes.is_empty() => {
                let cur = (self.active_cursor.unwrap_or(0) + 1).min(self.bytes.len() - 1);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "up" if !self.bytes.is_empty() => {
                let cur = self.active_cursor.unwrap_or(0).saturating_sub(16);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "down" if !self.bytes.is_empty() => {
                let cur = (self.active_cursor.unwrap_or(0) + 16).min(self.bytes.len() - 1);
                self.select_byte(cur, ks.modifiers.shift, self.editing_ascii, window, cx);
                return;
            }
            "backspace" | "delete" if !self.readonly => {
                self.delete_selected_bytes(window, cx);
                return;
            }
            "insert" if !self.readonly => {
                self.insert_byte_before(window, cx);
                return;
            }
            _ => {}
        }
        if self.readonly || self.bytes.is_empty() {
            return;
        }
        let cursor = self
            .active_cursor
            .or_else(|| self.selected_range.as_ref().map(|r| r.start))
            .unwrap_or(0)
            .min(self.bytes.len() - 1);
        let typed = ks.key_char.as_deref().unwrap_or(ks.key.as_str());
        if typed.len() != 1 {
            return;
        }
        let ch = typed.chars().next().unwrap();
        if self.editing_ascii {
            if ch.is_ascii() && !ch.is_ascii_control() {
                self.bytes[cursor] = ch as u8;
                let next = (cursor + 1).min(self.bytes.len() - 1);
                self.commit_bytes_to_editor(Some(next..next + 1), window, cx);
            }
        } else if let Some(nibble) = ch.to_digit(16).map(|d| d as u8) {
            if !self.low_nibble_pending {
                self.bytes[cursor] = (self.bytes[cursor] & 0x0F) | (nibble << 4);
                self.low_nibble_pending = true;
                self.commit_bytes_to_editor(Some(cursor..cursor + 1), window, cx);
            } else {
                self.bytes[cursor] = (self.bytes[cursor] & 0xF0) | nibble;
                self.low_nibble_pending = false;
                let next = (cursor + 1).min(self.bytes.len() - 1);
                self.commit_bytes_to_editor(Some(next..next + 1), window, cx);
            }
        }
    }
}

impl Render for HexGridView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = crate::theme::is_dark();
        let total_bytes = self.bytes.len();
        let row_count = total_bytes.div_ceil(16).max(1);
        let readonly = self.readonly;
        let selected_range = self.selected_range.clone();
        let _hovered_byte = self.hovered_byte;
        let _active_cursor = self.active_cursor;
        let _low_nibble = self.low_nibble_pending;
        let _editing_ascii = self.editing_ascii;
        let offset_bg = rgb(if dark { 0x232529 } else { 0xf3f4f6 });
        let header_bg = rgb(if dark { 0x25272b } else { 0xededf0 });
        let border_col = crate::theme::border();
        let mono_font = crate::theme::mono_font_family();
        let hex_entity_id = self.hex_editor.entity_id();

        let status_summary = if let Some(ref range) = selected_range {
            let len = range.len();
            if len == 1
                && let Some(&b) = self.bytes.get(range.start)
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
                let bytes = &grid.bytes;
                let total = bytes.len();
                let sel = grid.selected_range.clone();
                let hov = grid.hovered_byte;
                let cur = grid.active_cursor;
                let low_nib = grid.low_nibble_pending;
                let ascii_focus = grid.editing_ascii;
                let sel_primary_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x265e9c } else { 0x3b82f6 }).into();
                let sel_sync_bg: gpui_kit::Hsla =
                    rgb(if dark { 0x2d4163 } else { 0xdbeafe }).into();
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
                                let is_hov = hov == Some(byte_ix);
                                let is_cur_nibble =
                                    cur == Some(byte_ix) && low_nib && !ascii_focus;
                                let hex_bg = if is_sel {
                                    if ascii_focus {
                                        sel_sync_bg
                                    } else {
                                        sel_primary_bg
                                    }
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
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(toolbar)
            .child(col_header)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .flex()
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
                    ),
            )
    }
}
