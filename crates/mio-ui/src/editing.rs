use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::Button,
    input::{Editor, EditorGutterStyle, EditorState, Input, InputEvent, InputState, WrappingIndent},
    native_menu::NativeMenu,
    scroll::{Scrollbar, ScrollbarHandle as _, ScrollbarMode},
};
use gpui_kit::{
    AnyElement, App, AppContext, Entity, EntityInputHandler, Focusable, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt, Window, div, prelude::FluentBuilder as _, px, rgb,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

mod codec;
mod hash;
mod message_editor;
mod replace;
mod search;
mod smart_decode;
pub(crate) mod syntax_highlight;
mod syntax_highlight_body;
#[cfg(test)]
mod tests_codec;
#[cfg(test)]
mod tests_large_perf;
#[cfg(test)]
mod tests_replace;
#[cfg(test)]
mod tests_search;
#[cfg(test)]
mod tests_syntax_highlight;

pub(crate) use codec::{
    DEFAULT_HIDDEN_REQUEST_HEADERS, DEFAULT_HIDDEN_RESPONSE_HEADERS,
    RenderedBodySummary, base64_decode_bytes, convert_bytes, convert_selection,
    extract_http_response_body, filter_http_headers_bytes,
    hex_decode, hex_encode, parse_rendered_http_response, validate_http_header_names,
};
pub(crate) use hash::{md5_digest, sha256_digest};
#[cfg(test)]
pub(crate) use hash::{md5_hex, sha256_hex};
pub(crate) use message_editor::{HexGridView, MessageEditorElement};
#[cfg(test)]
pub(crate) use message_editor::{
    MessageDisplayCharset, hex_byte_range_to_text_range, hex_text_range_to_byte_range,
};
pub(crate) use smart_decode::smart_decode_bytes;

pub(super) static NEXT_SELECTION_SEQ: AtomicU64 = AtomicU64::new(1);

pub(crate) const COMPOSITION_PENDING: &str = "输入法仍在组词，请先确认或取消候选，再重试操作。";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionSource {
    Utf8,
    Hex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectionIdentity {
    pub editor_id: gpui_kit::EntityId,
    pub document_revision: u64,
    pub range: std::ops::Range<usize>,
    pub source: SelectionSource,
    pub writable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SelectionCandidate {
    pub identity: SelectionIdentity,
    pub raw_bytes: Vec<u8>,
    pub selection_seq: u64,
    pub focused: bool,
    pub writeback_editor: Option<Entity<EditorState>>,
}

pub(crate) fn decode_selected_hex_digits(selected: &str) -> Option<Vec<u8>> {
    let nibbles: Vec<u8> = selected
        .bytes()
        .filter_map(|b| {
            (b as char)
                .to_digit(16)
                .filter(|_| b.is_ascii())
                .map(|d| d as u8)
        })
        .collect();
    if nibbles.len() < 2 {
        return None;
    }
    Some(
        nibbles
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (pair[0] << 4) | pair[1])
            .collect(),
    )
}

pub(crate) fn observe_selection_state<V: 'static>(
    editor: &Entity<EditorState>,
    selection_seq: Rc<Cell<u64>>,
    cx: &mut gpui_kit::Context<V>,
) -> Subscription {
    let weak_owner = cx.entity().downgrade();
    let notify_owner: Rc<dyn Fn(&mut App)> = Rc::new(move |cx: &mut App| {
        let _ = weak_owner.update(cx, |_, cx| cx.notify());
    });
    observe_selection_state_app(editor, selection_seq, notify_owner, cx)
}

pub(crate) fn observe_selection_state_app(
    editor: &Entity<EditorState>,
    selection_seq: Rc<Cell<u64>>,
    notify_owner: Rc<dyn Fn(&mut App)>,
    cx: &mut App,
) -> Subscription {
    let last_range = Cell::new((0usize, 0usize));
    let last_selecting = Cell::new(false);
    cx.observe(editor, move |editor, cx| {
        let st = editor.read(cx);
        let r = st.selected_range();
        let selecting = st.is_selecting();
        let pair = (r.start, r.end);
        let prev = last_range.get();
        let prev_sel = last_selecting.get();
        if pair == prev && selecting == prev_sel {
            return;
        }
        last_range.set(pair);
        last_selecting.set(selecting);
        if r.is_empty() && prev.0 == prev.1 && selecting == prev_sel {
            return;
        }
        if !r.is_empty() {
            selection_seq.set(NEXT_SELECTION_SEQ.fetch_add(1, Ordering::Relaxed));
        }
        (notify_owner)(cx);
    })
}

pub(crate) fn plain_editor_selection_candidate(
    editor: &Entity<EditorState>,
    selection_seq: u64,
    window: Option<&Window>,
    cx: &App,
) -> Option<SelectionCandidate> {
    let state = editor.read(cx);
    let range = state.selected_range();
    if range.is_empty() {
        return None;
    }
    let selected = state.selected_value();
    if selected.trim().is_empty() {
        return None;
    }
    let focused = window.is_some_and(|w| editor.focus_handle(cx).is_focused(w));
    Some(SelectionCandidate {
        identity: SelectionIdentity {
            editor_id: editor.entity_id(),
            document_revision: state.document_revision(),
            range,
            source: SelectionSource::Utf8,
            writable: false,
        },
        raw_bytes: selected.as_bytes().to_vec(),
        selection_seq,
        focused,
        writeback_editor: None,
    })
}

pub(crate) fn pick_selection_candidate(
    candidates: impl IntoIterator<Item = SelectionCandidate>,
) -> Option<SelectionCandidate> {
    let mut best: Option<SelectionCandidate> = None;
    for candidate in candidates {
        let replace = match &best {
            None => true,
            Some(prev) => match (candidate.focused, prev.focused) {
                (true, false) => true,
                (false, true) => false,
                _ => candidate.selection_seq > prev.selection_seq,
            },
        };
        if replace {
            best = Some(candidate);
        }
    }
    best
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct OpenSelectionInCyberChef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) enum ConvertKind {
    UrlEncodeKey,
    UrlEncodeAll,
    UrlEncodeUnicode,
    UrlDecode,
    HtmlEncodeKey,
    HtmlEncodeDecimal,
    HtmlEncodeHex,
    HtmlDecode,
    Base64Encode,
    Base64Decode,
    Base64UrlSafeEncode,
    Base64UrlSafeDecode,
    AsciiHexEncode,
    AsciiHexDecode,
    ConstructJsString,
    ConstructSqlChar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct CopyConvertedSelection(pub(crate) ConvertKind);

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) enum ExtendedConvertKind {
    HtmlEncodeAll,
    ConstructJsFromCharCode,
    ConstructMssqlChar,
    ConstructOracleChr,
    ConstructMysqlChar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct CopyExtendedConvertedSelection(pub(crate) ExtendedConvertKind);

#[cfg(not(test))]
static URL_ENCODE_AS_YOU_TYPE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static TEST_URL_ENCODE_AS_YOU_TYPE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn url_encode_as_you_type() -> bool {
    #[cfg(not(test))]
    {
        URL_ENCODE_AS_YOU_TYPE.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_URL_ENCODE_AS_YOU_TYPE.with(Cell::get)
    }
}

pub(crate) fn set_url_encode_as_you_type(enabled: bool) {
    #[cfg(not(test))]
    {
        URL_ENCODE_AS_YOU_TYPE.store(enabled, Ordering::Relaxed);
    }
    #[cfg(test)]
    {
        TEST_URL_ENCODE_AS_YOU_TYPE.with(|cell| cell.set(enabled));
    }
}

pub(crate) fn url_encode_typed_char(byte: u8) -> Option<&'static str> {
    match byte {
        b'&' => Some("%26"),
        b'=' => Some("%3D"),
        b'+' => Some("%2B"),
        b'#' => Some("%23"),
        b'%' => Some("%25"),
        b' ' => Some("%20"),
        _ => None,
    }
}

pub(crate) fn convert_selection_extended(kind: ExtendedConvertKind, input: &str) -> String {
    use std::fmt::Write;
    match kind {
        ExtendedConvertKind::HtmlEncodeAll => {
            let mut out = String::with_capacity(input.len().saturating_mul(5));
            for ch in input.chars() {
                match ch {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\'' => out.push_str("&#39;"),
                    other => {
                        let _ = write!(out, "&#{};", other as u32);
                    }
                }
            }
            out
        }
        ExtendedConvertKind::ConstructJsFromCharCode => {
            let mut units = Vec::new();
            let mut buf = [0u16; 2];
            for ch in input.chars() {
                for &mut unit in ch.encode_utf16(&mut buf) {
                    units.push(unit.to_string());
                }
            }
            format!("String.fromCharCode({})", units.join(","))
        }
        ExtendedConvertKind::ConstructMssqlChar => {
            if input.is_empty() {
                return "''".to_owned();
            }
            input
                .chars()
                .map(|ch| {
                    let code = ch as u32;
                    if code <= 255 {
                        format!("char({code})")
                    } else {
                        format!("nchar({code})")
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        }
        ExtendedConvertKind::ConstructOracleChr => {
            if input.is_empty() {
                return "''".to_owned();
            }
            input
                .chars()
                .map(|ch| format!("chr({})", ch as u32))
                .collect::<Vec<_>>()
                .join("||")
        }
        ExtendedConvertKind::ConstructMysqlChar => {
            if input.is_empty() {
                return "''".to_owned();
            }
            let bytes = input
                .as_bytes()
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("CHAR({bytes})")
        }
    }
}

pub(crate) fn convert_selected_editor_text_extended(
    editor: &Entity<EditorState>,
    kind: ExtendedConvertKind,
    window: &mut Window,
    cx: &mut App,
) -> Result<String, String> {
    if editor.update(cx, |state, cx| {
        state.marked_text_range(window, cx).is_some()
    }) {
        return Err(COMPOSITION_PENDING.into());
    }
    let selected = editor.read(cx).selected_value();
    if selected.is_empty() {
        return Err("请先在消息编辑器中选中要转换的文本。".into());
    }
    Ok(convert_selection_extended(kind, selected.as_ref()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(crate) struct InvokePluginContextMenu(pub(crate) mio_core::Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageView {
    Raw,
    Hex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingReplaceAll {
    pub view: MessageView,
    pub pretty_active: bool,
    pub document_revision: u64,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub match_count: usize,
}

pub(crate) struct MessageEditor {
    pub pretty: Entity<EditorState>,
    pub pretty_active: bool,
    pub(super) original_wire_bytes: Vec<u8>,
    pub(super) pretty_baseline_text: Option<String>,
    pub(crate) hide_headers: bool,
    hide_headers_override: Option<bool>,
    pub(crate) show_non_printable: bool,
    pub(crate) render_active: bool,
    pub(super) header_preview: Option<Entity<EditorState>>,
    pub(super) non_printable: Option<Entity<EditorState>>,
    header_preview_slot: Rc<RefCell<Option<Entity<EditorState>>>>,
    non_printable_slot: Rc<RefCell<Option<Entity<EditorState>>>>,
    pub(crate) hidden_headers: Option<Vec<String>>,
    pub(crate) header_preview_error: Option<String>,
    pub soft_wrap: bool,
    pub raw: Entity<EditorState>,
    pub hex: Entity<EditorState>,
    pub(crate) hex_grid: Option<Entity<HexGridView>>,
    pub search: Entity<InputState>,
    pub replace: Entity<InputState>,
    pub replace_open: bool,
    pub pending_replace_all: Option<PendingReplaceAll>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub auto_scroll_to_match: bool,
    pub search_error: Rc<RefCell<Option<String>>>,
    pub view: MessageView,
    pub(crate) auto_hex_fallback: bool,
    selection_seq: Rc<Cell<u64>>,
    view_cell: Rc<Cell<MessageView>>,
    pretty_cell: Rc<Cell<bool>>,
    hide_headers_cell: Rc<Cell<bool>>,
    non_printable_cell: Rc<Cell<bool>>,
    case_sensitive_cell: Rc<Cell<bool>>,
    regex_cell: Rc<Cell<bool>>,
    auto_scroll_cell: Rc<Cell<bool>>,
    notify_owner: Rc<dyn Fn(&mut App)>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchBarAction {
    Previous,
    Next,
    ToggleCaseSensitive,
    ToggleRegex,
    ToggleAutoScroll,
    ToggleReplace,
    Replace,
    RequestReplaceAll,
    ConfirmReplaceAll,
    CancelReplaceAll,
}

pub(crate) fn convert_selection_submenu(disabled: bool) -> NativeMenu {
    convert_selection_submenu_for(disabled, false)
}

pub(crate) fn copy_converted_selection_submenu(disabled: bool) -> NativeMenu {
    convert_selection_submenu_for(disabled, true)
}

fn convert_selection_submenu_for(disabled: bool, copy_to_clipboard: bool) -> NativeMenu {
    let action = |kind: ConvertKind| -> Box<dyn gpui_kit::Action> {
        if copy_to_clipboard {
            Box::new(CopyConvertedSelection(kind))
        } else {
            Box::new(kind)
        }
    };
    let ext_action = |kind: ExtendedConvertKind| -> Box<dyn gpui_kit::Action> {
        if copy_to_clipboard {
            Box::new(CopyExtendedConvertedSelection(kind))
        } else {
            Box::new(kind)
        }
    };
    NativeMenu::new()
        .submenu(
            "URL",
            NativeMenu::new()
                .menu_with_disabled(
                    "URL 编码（关键字符）",
                    disabled,
                    action(ConvertKind::UrlEncodeKey),
                )
                .menu_with_disabled(
                    "URL 编码（全部字符）",
                    disabled,
                    action(ConvertKind::UrlEncodeAll),
                )
                .menu_with_disabled(
                    "URL 编码（全部字符 Unicode）",
                    disabled,
                    action(ConvertKind::UrlEncodeUnicode),
                )
                .separator()
                .menu_with_disabled("URL 解码", disabled, action(ConvertKind::UrlDecode)),
        )
        .submenu(
            "HTML",
            NativeMenu::new()
                .menu_with_disabled(
                    "HTML 编码（关键字符）",
                    disabled,
                    action(ConvertKind::HtmlEncodeKey),
                )
                .menu_with_disabled(
                    "HTML 编码（全部字符）",
                    disabled,
                    ext_action(ExtendedConvertKind::HtmlEncodeAll),
                )
                .menu_with_disabled(
                    "HTML 编码（十进制数字实体）",
                    disabled,
                    action(ConvertKind::HtmlEncodeDecimal),
                )
                .menu_with_disabled(
                    "HTML 编码（十六进制数字实体）",
                    disabled,
                    action(ConvertKind::HtmlEncodeHex),
                )
                .separator()
                .menu_with_disabled("HTML 解码", disabled, action(ConvertKind::HtmlDecode)),
        )
        .submenu(
            "Base64",
            NativeMenu::new()
                .menu_with_disabled("Base64 编码", disabled, action(ConvertKind::Base64Encode))
                .menu_with_disabled("Base64 解码", disabled, action(ConvertKind::Base64Decode)),
        )
        .submenu(
            "Base64 URL",
            NativeMenu::new()
                .menu_with_disabled(
                    "Base64 URL 编码",
                    disabled,
                    action(ConvertKind::Base64UrlSafeEncode),
                )
                .menu_with_disabled(
                    "Base64 URL 解码",
                    disabled,
                    action(ConvertKind::Base64UrlSafeDecode),
                ),
        )
        .submenu(
            "ASCII Hex",
            NativeMenu::new()
                .menu_with_disabled(
                    "ASCII Hex 编码",
                    disabled,
                    action(ConvertKind::AsciiHexEncode),
                )
                .menu_with_disabled(
                    "ASCII Hex 解码",
                    disabled,
                    action(ConvertKind::AsciiHexDecode),
                ),
        )
        .submenu(
            "构造字符串 (Construct string)",
            NativeMenu::new()
                .menu_with_disabled(
                    "JavaScript",
                    disabled,
                    ext_action(ExtendedConvertKind::ConstructJsFromCharCode),
                )
                .menu_with_disabled(
                    "Microsoft SQL Server",
                    disabled,
                    ext_action(ExtendedConvertKind::ConstructMssqlChar),
                )
                .menu_with_disabled(
                    "Oracle",
                    disabled,
                    ext_action(ExtendedConvertKind::ConstructOracleChr),
                )
                .menu_with_disabled(
                    "MySQL",
                    disabled,
                    ext_action(ExtendedConvertKind::ConstructMysqlChar),
                ),
        )
        .separator()
        .menu_with_disabled(
            "在 CyberChef 中打开",
            disabled,
            Box::new(OpenSelectionInCyberChef),
        )
}

pub(crate) fn convert_selected_editor_text(
    editor: &Entity<EditorState>,
    kind: ConvertKind,
    window: &mut Window,
    cx: &mut App,
) -> Result<String, String> {
    if editor.update(cx, |state, cx| {
        state.marked_text_range(window, cx).is_some()
    }) {
        return Err(COMPOSITION_PENDING.into());
    }
    let selected = editor.read(cx).selected_value();
    if selected.is_empty() {
        return Err("请先在消息编辑器中选中要转换的文本。".into());
    }
    convert_selection(kind, selected.as_ref())
}

pub(crate) fn new_editor(window: &mut Window, cx: &mut App) -> Entity<EditorState> {
    new_editor_with_language("http", window, cx)
}

pub(crate) fn new_plain_editor(window: &mut Window, cx: &mut App) -> Entity<EditorState> {
    new_editor_with_language("plaintext", window, cx)
}

pub(crate) fn message_editor(state: &Entity<EditorState>) -> MessageEditorElement {
    MessageEditorElement::new(state)
}

fn new_editor_with_language(
    language: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let gutter_border: gpui_kit::Hsla =
        rgb(if crate::theme::is_dark() { 0x3c3f41 } else { 0xdfdfdf }).into();
    let entity = cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language(language)
            .folding(false)
            .indent_guides(false)
            .smart_indent(false)
            .searchable(false)
            .line_number(true)
            .precise_line_height(true)
            .wrapping_indent(WrappingIndent::None)
            .scroll_beyond_last_line(Some(0))
            .gutter_style(
                EditorGutterStyle::default()
                    .min_digits(2)
                    .min_width(px(23.))
                    .left_padding(px(2.))
                    .right_margin(px(5.))
                    .right_border_width(px(1.))
                    .right_border_color(gutter_border)
                    .text_left_padding(px(6.)),
            )
            .soft_wrap(crate::theme::editor_soft_wrap())
            .auto_close(false)
            .autocorrect(false);
        state.set_scrollbar_renderer(Some(Rc::new(|context, _window, _cx| {
            let axis = context.axis();
            let viewport = context.viewport_bounds();
            let content = context.content_size();
            let has_horizontal_overflow =
                !axis.is_vertical() && content.width > viewport.size.width;
            let dark = crate::theme::is_dark();
            let track_bg = rgb(if dark { 0x2b2d30 } else { 0xf7f7f7 });
            let thumb_bg = rgb(if dark { 0x5c626b } else { 0xc8c8c8 });
            let thumb_hover_bg = rgb(if dark { 0x707782 } else { 0xb0b0b0 });
            let scrollbar = Scrollbar::new(&context)
                .axis(axis)
                .mode(ScrollbarMode::Always)
                .styles(|styles| {
                    styles
                        .track(|s| s.width(px(10.)).bg(track_bg.into()))
                        .track_hover(|s| s.width(px(10.)).bg(track_bg.into()))
                        .track_active(|s| s.width(px(10.)).bg(track_bg.into()))
                        .thumb(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover_bg)
                        })
                        .thumb_active(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover_bg)
                        })
                });
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom(if has_horizontal_overflow {
                            px(10.)
                        } else {
                            px(0.)
                        })
                        .w(px(10.))
                        .bg(track_bg),
                )
                .when(has_horizontal_overflow, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(10.))
                            .bg(track_bg),
                    )
                })
                .child(scrollbar)
                .into_any_element()
        })));
        if language == "http" {
            state.set_highlighter_factory(syntax_highlight::http_highlighter_factory(), cx);
        }
        state
    });
    if language == "http" {
        let window_handle = window.window_handle();
        let reentrant = Rc::new(Cell::new(false));
        cx.subscribe(
            &entity,
            move |editor, event: &gpui_kit::component::input::InputEditsEvent, cx| {
                if reentrant.get() || !url_encode_as_you_type() {
                    return;
                }
                let state = editor.read(cx);
                if !state.is_editable() || state.has_marked_text() {
                    return;
                }
                let edits = event.edits();
                if edits.len() != 1 {
                    return;
                }
                let (edit, rope) = &edits[0];
                if edit.new_end_byte != edit.start_byte.saturating_add(1) {
                    return;
                }
                let inserted = rope.slice(edit.start_byte..edit.new_end_byte).to_string();
                let inserted_bytes = inserted.as_bytes();
                if inserted_bytes.len() != 1 {
                    return;
                }
                let byte = inserted_bytes[0];
                let Some(encoded) = url_encode_typed_char(byte) else {
                    return;
                };
                let start = edit.start_byte;
                let expected_rev = state.document_revision();
                let editor_weak = editor.downgrade();
                let reentrant_defer = reentrant.clone();
                cx.defer(move |cx| {
                    let _ = window_handle.update(cx, |_, window, cx| {
                        let Some(editor) = editor_weak.upgrade() else {
                            return;
                        };
                        if reentrant_defer.get() || !url_encode_as_you_type() {
                            return;
                        }
                        let should_replace = {
                            let st = editor.read(cx);
                            st.is_editable()
                                && !st.has_marked_text()
                                && st.document_revision() == expected_rev
                                && st.text().len() >= start + 1
                                && st.text().slice(start..start + 1).to_string().as_bytes()
                                    == [byte]
                        };
                        if !should_replace {
                            return;
                        }
                        reentrant_defer.set(true);
                        editor.update(cx, |st, cx| {
                            st.set_selected_range(start..start + 1, cx);
                            st.replace(encoded, window, cx);
                        });
                        reentrant_defer.set(false);
                    });
                });
            },
        )
        .detach();
    }
    entity
}

pub(crate) fn status_code_color(status: u16, dark: bool, highlighted: bool) -> gpui_kit::Hsla {
    if highlighted {
        return rgb(match status {
            200..=299 => 0x0b4f21,
            300..=399 => 0x05439c,
            400..=499 => 0x6e4200,
            500..=599 => 0x8c111b,
            _ => 0x1a1a1a,
        })
        .into();
    }
    rgb(match (status, dark) {
        (200..=299, false) => 0x116329,
        (200..=299, true) => 0x56d364,
        (300..=399, false) => 0x0969da,
        (300..=399, true) => 0x58a6ff,
        (400..=499, false) => 0x9a6700,
        (400..=499, true) => 0xf0883e,
        (500..=599, false) => 0xcf222e,
        (500..=599, true) => 0xff7b72,
        (_, false) => 0x222222,
        (_, true) => 0xdfe1e5,
    })
    .into()
}

pub(crate) fn snapshot(
    editor: &Entity<EditorState>,
    window: &mut Window,
    cx: &mut App,
) -> Result<SharedString, &'static str> {
    editor.update(cx, |state, cx| {
        // Clearing a marked range is not confirmation by the native input method.
        if state.marked_text_range(window, cx).is_some() {
            // Snapshots also inspect hidden editors; they must not move native focus.
            return Err(COMPOSITION_PENDING);
        }
        Ok(state.value())
    })
}

pub(crate) fn input_snapshot(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) -> Result<SharedString, &'static str> {
    input.update(cx, |state, cx| {
        if state.marked_text_range(window, cx).is_some() {
            return Err(COMPOSITION_PENDING);
        }
        Ok(state.value())
    })
}

#[cfg(test)]
mod extended_convert_and_url_encode_tests {
    use super::*;

    #[test]
    fn extended_convert_matches_ext_html_all_and_construct_string_targets() {
        assert_eq!(
            convert_selection_extended(ExtendedConvertKind::HtmlEncodeAll, "A<&"),
            "&#65;&lt;&amp;"
        );
        assert_eq!(
            convert_selection_extended(ExtendedConvertKind::ConstructJsFromCharCode, "AB"),
            "String.fromCharCode(65,66)"
        );
        assert_eq!(
            convert_selection_extended(ExtendedConvertKind::ConstructMssqlChar, "AB"),
            "char(65)+char(66)"
        );
        assert_eq!(
            convert_selection_extended(ExtendedConvertKind::ConstructOracleChr, "AB"),
            "chr(65)||chr(66)"
        );
        assert_eq!(
            convert_selection_extended(ExtendedConvertKind::ConstructMysqlChar, "AB"),
            "CHAR(65,66)"
        );
    }

    #[test]
    fn url_encode_typed_char_maps_key_characters_only() {
        assert_eq!(url_encode_typed_char(b'&'), Some("%26"));
        assert_eq!(url_encode_typed_char(b'='), Some("%3D"));
        assert_eq!(url_encode_typed_char(b'+'), Some("%2B"));
        assert_eq!(url_encode_typed_char(b'#'), Some("%23"));
        assert_eq!(url_encode_typed_char(b'%'), Some("%25"));
        assert_eq!(url_encode_typed_char(b' '), Some("%20"));
        assert_eq!(url_encode_typed_char(b'a'), None);
        assert_eq!(url_encode_typed_char(b'/'), None);
    }

    #[gpui_kit::test]
    fn url_encode_as_you_type_encodes_key_characters_in_http_editor(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        struct EditorHost(Entity<EditorState>);
        impl gpui_kit::Render for EditorHost {
            fn render(
                &mut self,
                _: &mut Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(message_editor(&self.0))
            }
        }
        cx.update(gpui_kit::init);
        set_url_encode_as_you_type(false);
        let (view, visual) = cx.add_window_view(|window, cx| {
            let ed = new_editor(window, cx);
            ed.update(cx, |s, cx| s.set_value("GET /?a=1", window, cx));
            EditorHost(ed)
        });
        set_url_encode_as_you_type(true);
        visual.update(|window, cx| {
            let editor = view.read(cx).0.clone();
            editor.update(cx, |state, cx| {
                let end = state.text().len();
                state.set_selected_range(end..end, cx);
                state.replace_text_in_range(None, "&", window, cx);
            });
        });
        cx.run_until_parked();
        let value = cx.read(|cx| view.read(cx).0.read(cx).value().to_string());
        assert_eq!(value, "GET /?a=1%26");
        set_url_encode_as_you_type(false);
    }
}
