use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
};
use mio_core::{
    DecodedInspectorSelection, InspectorSelectionCodec, decode_inspector_selection,
    format_pretty_message,
};

use super::*;
use crate::editing::{SelectionCandidate, SelectionIdentity, SelectionSource};

pub(super) const MAX_CYBERCHEF_INPUT_BYTES: usize = 64 * 1024;
const MAX_PREVIEW_DECODE_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum SelectionTransform {
    UrlEncode,
    UrlDecode,
    Base64Encode,
    Base64Decode,
    Base64UrlEncode,
    Base64UrlDecode,
    HtmlEncode,
    HtmlDecode,
    AsciiHexEncode,
    AsciiHexDecode,
}

pub(super) fn apply_selection_transform(
    transform: SelectionTransform,
    input: &str,
) -> Result<String, String> {
    match transform {
        SelectionTransform::UrlEncode => {
            editing::convert_selection(editing::ConvertKind::UrlEncodeKey, input)
        }
        SelectionTransform::UrlDecode => {
            editing::convert_selection(editing::ConvertKind::UrlDecode, input)
        }
        SelectionTransform::Base64Encode => Ok(STANDARD.encode(input.as_bytes())),
        SelectionTransform::Base64Decode => {
            editing::convert_selection(editing::ConvertKind::Base64Decode, input)
        }
        SelectionTransform::Base64UrlEncode => Ok(URL_SAFE_NO_PAD.encode(input.as_bytes())),
        SelectionTransform::Base64UrlDecode => {
            let clean: String = input.chars().filter(|ch| !ch.is_ascii_whitespace()).collect();
            if clean.is_empty() {
                return Err("当前没有可解码的 Base64URL 内容。".into());
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(clean.trim_end_matches('='))
                .or_else(|_| URL_SAFE.decode(&clean))
                .map_err(|_| "输入内容并非有效的 Base64URL 编码。".to_owned())?;
            String::from_utf8(bytes)
                .map_err(|_| "Base64URL 解码结果包含非 UTF-8 字节，无法显示为纯文本。".to_owned())
        }
        SelectionTransform::HtmlEncode => {
            editing::convert_selection(editing::ConvertKind::HtmlEncodeKey, input)
        }
        SelectionTransform::HtmlDecode => {
            editing::convert_selection(editing::ConvertKind::HtmlDecode, input)
        }
        SelectionTransform::AsciiHexEncode => {
            editing::convert_selection(editing::ConvertKind::AsciiHexEncode, input)
        }
        SelectionTransform::AsciiHexDecode => {
            editing::convert_selection(editing::ConvertKind::AsciiHexDecode, input)
        }
    }
}

pub(super) fn format_selection_stats(raw_bytes: &[u8], text: &str) -> String {
    let byte_len = raw_bytes.len();
    let char_len = text.chars().count();
    if byte_len == 1 {
        let code = raw_bytes[0];
        format!("1 字节 · {char_len} 字符 · 0x{code:02X} / {code}")
    } else if char_len == 1
        && let Some(ch) = text.chars().next()
    {
        let cp = u32::from(ch);
        format!("{byte_len} 字节 · 1 字符 · U+{cp:04X} / {cp}")
    } else {
        format!("{byte_len} 字节 · {char_len} 字符")
    }
}

pub(super) fn decode_hex_digits_loose(selected: &str) -> Option<Vec<u8>> {
    editing::decode_selected_hex_digits(selected)
}

pub(super) fn extract_editor_selection(
    editor: &MessageEditor,
    writable: bool,
    window: Option<&Window>,
    cx: &App,
) -> Option<SelectionCandidate> {
    editor.selection_candidate(writable, window, cx)
}

pub(super) fn extract_intruder_textarea_selection(
    state: &Entity<gpui_kit::component::input::TextareaState>,
    view: MessageView,
    window: Option<&Window>,
    cx: &App,
) -> Option<SelectionCandidate> {
    let editor_id = state.entity_id();
    let focused = window.is_some_and(|w| state.focus_handle(cx).is_focused(w));
    let state = state.read(cx);
    let range = state.selected_range();
    if range.is_empty() {
        return None;
    }
    let selected = state.selected_value();
    let clean = selected.replace('§', "");
    if clean.is_empty() {
        return None;
    }
    let (raw_bytes, source) = match view {
        MessageView::Raw => {
            if clean.bytes().all(|b| b.is_ascii_whitespace()) {
                return None;
            }
            (clean.into_bytes(), SelectionSource::Utf8)
        }
        MessageView::Hex => (decode_hex_digits_loose(&clean)?, SelectionSource::Hex),
    };
    Some(SelectionCandidate {
        identity: SelectionIdentity {
            editor_id,
            document_revision: state.document_revision(),
            range,
            source,
            writable: false,
        },
        raw_bytes,
        selection_seq: 0,
        focused,
        writeback_editor: None,
    })
}

pub(in crate::workbench::inspector) fn preview_for_candidate(
    candidate: &SelectionCandidate,
) -> InspectorSelectionPreview {
    build_selection_preview(
        candidate.raw_bytes.clone(),
        candidate.identity.source == SelectionSource::Utf8,
        candidate.identity.writable,
    )
}

impl SelectionCandidate {
    pub(in crate::workbench::inspector) fn build_preview(&self) -> InspectorSelectionPreview {
        preview_for_candidate(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct InspectorSelectionPreview {
    pub raw_bytes: Vec<u8>,
    pub original_text: String,
    pub decoded: String,
    pub chain_label: String,
    pub reversible_steps: Vec<InspectorSelectionCodec>,
    pub can_write_back: bool,
}

pub(super) fn build_cyberchef_url(raw_bytes: &[u8]) -> Result<String, String> {
    if raw_bytes.is_empty() {
        return Err("请先选中要在 CyberChef 中打开的内容。".into());
    }
    if raw_bytes.len() > MAX_CYBERCHEF_INPUT_BYTES {
        return Err(format!(
            "选区超过 {} KiB 上限，无法通过 URL 打开 CyberChef。",
            MAX_CYBERCHEF_INPUT_BYTES / 1024
        ));
    }
    let b64 = STANDARD.encode(raw_bytes);
    let mut escaped = String::with_capacity(b64.len() + 16);
    for byte in b64.bytes() {
        match byte {
            b'+' => escaped.push_str("%2B"),
            b'/' => escaped.push_str("%2F"),
            b'=' => escaped.push_str("%3D"),
            other => escaped.push(other as char),
        }
    }
    Ok(format!("https://gchq.github.io/CyberChef/#input={escaped}"))
}

fn is_printable_preview(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| !ch.is_control() || matches!(ch, '\r' | '\n' | '\t'))
}

fn decode_b64url_json_part(segment: &str) -> Option<String> {
    if segment.len() < 4
        || !segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'='))
    {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(segment.trim_end_matches('='))
        .or_else(|_| URL_SAFE.decode(segment))
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return None;
    }
    let pretty = format_pretty_message(trimmed.as_bytes()).ok()?;
    Some(pretty)
}

fn try_decode_jwt(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let mut parts = trimmed.split('.');
    let header_seg = parts.next()?;
    let payload_seg = parts.next()?;
    let sig_seg = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !sig_seg
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'='))
    {
        return None;
    }
    let header_json = decode_b64url_json_part(header_seg)?;
    let payload_json = decode_b64url_json_part(payload_seg)?;
    let sig_note = if sig_seg.is_empty() {
        "<empty (alg=none?)>".to_owned()
    } else {
        sig_seg.to_owned()
    };
    Some(format!(
        "Header:\n{header_json}\n\nPayload:\n{payload_json}\n\nSignature:\n{sig_note}"
    ))
}

fn try_decode_html_entities(input: &str) -> Option<String> {
    if !input.contains('&') || !input.contains(';') {
        return None;
    }
    let decoded = editing::convert_selection(editing::ConvertKind::HtmlDecode, input).ok()?;
    (decoded != input && is_printable_preview(&decoded)).then_some(decoded)
}

pub(super) fn build_selection_preview(
    raw_bytes: Vec<u8>,
    is_raw_utf8: bool,
    editor_writable: bool,
) -> InspectorSelectionPreview {
    if !is_raw_utf8 {
        if let Ok((smart_bytes, smart_steps)) =
            editing::smart_decode_bytes(&raw_bytes, MAX_PREVIEW_DECODE_BYTES)
            && !smart_steps.is_empty()
            && let Ok(text) = std::str::from_utf8(&smart_bytes)
            && is_printable_preview(text)
        {
            return InspectorSelectionPreview {
                original_text: editing::hex_encode(&raw_bytes),
                raw_bytes,
                decoded: text.to_owned(),
                chain_label: smart_steps.join(" → "),
                reversible_steps: Vec::new(),
                can_write_back: editor_writable,
            };
        }
        let preview = match std::str::from_utf8(&raw_bytes) {
            Ok(text) if is_printable_preview(text) => (text.to_owned(), "Hex → UTF-8".into()),
            _ => (editing::hex_encode(&raw_bytes), "Hex 字节".into()),
        };
        return InspectorSelectionPreview {
            original_text: editing::hex_encode(&raw_bytes),
            raw_bytes,
            decoded: preview.0,
            chain_label: preview.1,
            reversible_steps: Vec::new(),
            can_write_back: editor_writable,
        };
    }

    let original = String::from_utf8_lossy(&raw_bytes).into_owned();
    let core: DecodedInspectorSelection = decode_inspector_selection(&original);

    if let Some(jwt) = try_decode_jwt(&core.decoded) {
        let chain_label = if core.steps.is_empty() {
            "JWT".into()
        } else {
            format!("{} → JWT", core.chain_label())
        };
        return InspectorSelectionPreview {
            raw_bytes,
            original_text: original,
            decoded: jwt,
            chain_label,
            reversible_steps: Vec::new(),
            can_write_back: false,
        };
    }

    if !core.steps.is_empty() {
        let (decoded, chain_label, can_wb) =
            if let Some(html_decoded) = try_decode_html_entities(&core.decoded) {
                (
                    html_decoded,
                    format!("{} → HTML", core.chain_label()),
                    editor_writable,
                )
            } else {
                (core.decoded.clone(), core.chain_label(), editor_writable)
            };
        let reversible_steps = if chain_label.ends_with("HTML") || !can_wb {
            Vec::new()
        } else {
            core.steps
        };
        return InspectorSelectionPreview {
            raw_bytes,
            original_text: original,
            decoded,
            chain_label,
            reversible_steps,
            can_write_back: can_wb,
        };
    }

    if let Some(html_decoded) = try_decode_html_entities(&original) {
        return InspectorSelectionPreview {
            raw_bytes,
            original_text: original,
            decoded: html_decoded,
            chain_label: "HTML".into(),
            reversible_steps: Vec::new(),
            can_write_back: editor_writable,
        };
    }

    if let Ok((smart_bytes, smart_steps)) =
        editing::smart_decode_bytes(&raw_bytes, MAX_PREVIEW_DECODE_BYTES)
        && !smart_steps.is_empty()
        && let Ok(text) = std::str::from_utf8(&smart_bytes)
        && is_printable_preview(text)
    {
        return InspectorSelectionPreview {
            raw_bytes,
            original_text: original,
            decoded: text.to_owned(),
            chain_label: smart_steps.join(" → "),
            reversible_steps: Vec::new(),
            can_write_back: editor_writable,
        };
    }

    InspectorSelectionPreview {
        raw_bytes,
        original_text: original.clone(),
        decoded: original,
        chain_label: "原文".into(),
        reversible_steps: Vec::new(),
        can_write_back: editor_writable,
    }
}
