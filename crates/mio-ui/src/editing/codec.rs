use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};

use super::ConvertKind;

pub(crate) fn convert_selection(kind: ConvertKind, input: &str) -> Result<String, String> {
    match kind {
        ConvertKind::UrlEncodeKey => Ok(url_encode_key(input)),
        ConvertKind::UrlEncodeAll => Ok(url_encode_all(input)),
        ConvertKind::UrlEncodeUnicode => Ok(url_encode_unicode(input)),
        ConvertKind::UrlDecode => url_decode(input),
        ConvertKind::HtmlEncodeKey => Ok(html_encode_key(input)),
        ConvertKind::HtmlEncodeAll => Ok(html_encode_all(input)),
        ConvertKind::HtmlEncodeDecimal => Ok(html_encode_decimal(input)),
        ConvertKind::HtmlEncodeHex => Ok(html_encode_hex(input)),
        ConvertKind::HtmlDecode => Ok(html_decode(input)),
        ConvertKind::Base64Encode => Ok(STANDARD.encode(input.as_bytes())),
        ConvertKind::Base64Decode => base64_decode(input),
        ConvertKind::Base64UrlSafeEncode => Ok(URL_SAFE_NO_PAD.encode(input.as_bytes())),
        ConvertKind::Base64UrlSafeDecode => base64_url_safe_decode(input),
        ConvertKind::AsciiHexEncode => Ok(ascii_hex_encode(input)),
        ConvertKind::AsciiHexDecode => ascii_hex_decode(input),
        ConvertKind::ConstructJsFromCharCode => Ok(construct_js_from_char_code(input)),
        ConvertKind::ConstructMssqlChar => Ok(construct_mssql_char(input)),
        ConvertKind::ConstructOracleChr => Ok(construct_oracle_chr(input)),
        ConvertKind::ConstructMysqlChar => Ok(construct_mysql_char(input.as_bytes())),
    }
}

pub(crate) fn convert_bytes(
    kind: ConvertKind,
    input_bytes: &[u8],
    input_text: &str,
) -> Result<Vec<u8>, String> {
    let require_utf8 = |err_msg: &'static str| {
        std::str::from_utf8(input_bytes).map_err(|_| err_msg.to_owned())
    };
    match kind {
        ConvertKind::UrlEncodeKey => Ok(url_encode_key_bytes(input_bytes).into_bytes()),
        ConvertKind::UrlEncodeAll => Ok(url_encode_all_bytes(input_bytes).into_bytes()),
        ConvertKind::UrlEncodeUnicode => Ok(url_encode_unicode(require_utf8(
            "当前输入包含非 UTF-8 字节，无法执行 Unicode URL 编码。",
        )?)
        .into_bytes()),
        ConvertKind::UrlDecode => Ok(url_decode_bytes(input_text)),
        ConvertKind::HtmlEncodeKey => Ok(html_encode_key(require_utf8(
            "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。",
        )?)
        .into_bytes()),
        ConvertKind::HtmlEncodeAll => Ok(html_encode_all(require_utf8(
            "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。",
        )?)
        .into_bytes()),
        ConvertKind::HtmlEncodeDecimal => Ok(html_encode_decimal(require_utf8(
            "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。",
        )?)
        .into_bytes()),
        ConvertKind::HtmlEncodeHex => Ok(html_encode_hex(require_utf8(
            "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。",
        )?)
        .into_bytes()),
        ConvertKind::HtmlDecode => Ok(html_decode(input_text).into_bytes()),
        ConvertKind::Base64Encode => Ok(STANDARD.encode(input_bytes).into_bytes()),
        ConvertKind::Base64Decode => base64_decode_bytes(input_text),
        ConvertKind::Base64UrlSafeEncode => Ok(URL_SAFE_NO_PAD.encode(input_bytes).into_bytes()),
        ConvertKind::Base64UrlSafeDecode => base64_url_safe_decode_bytes(input_text),
        ConvertKind::AsciiHexEncode => Ok(ascii_hex_encode_bytes(input_bytes).into_bytes()),
        ConvertKind::AsciiHexDecode => hex_decode(input_text),
        ConvertKind::ConstructJsFromCharCode => Ok(construct_js_from_char_code(require_utf8(
            "当前输入包含非 UTF-8 字节，无法构造 JS 字符串。",
        )?)
        .into_bytes()),
        ConvertKind::ConstructMssqlChar => Ok(construct_mssql_char(require_utf8(
            "当前输入包含非 UTF-8 字节，无法构造 SQL 字符串。",
        )?)
        .into_bytes()),
        ConvertKind::ConstructOracleChr => Ok(construct_oracle_chr(require_utf8(
            "当前输入包含非 UTF-8 字节，无法构造 SQL 字符串。",
        )?)
        .into_bytes()),
        ConvertKind::ConstructMysqlChar => Ok(construct_mysql_char(input_bytes).into_bytes()),
    }
}

fn url_encode_key_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            write!(out, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    out
}

fn url_encode_key(input: &str) -> String {
    url_encode_key_bytes(input.as_bytes())
}

fn url_encode_all_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len().saturating_mul(3));
    for &byte in bytes {
        write!(out, "%{byte:02X}").expect("writing to a String cannot fail");
    }
    out
}

fn url_encode_all(input: &str) -> String {
    url_encode_all_bytes(input.as_bytes())
}

fn url_encode_unicode(input: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(input.len().saturating_mul(6));
    let mut utf16 = [0u16; 2];
    for ch in input.chars() {
        for &mut unit in ch.encode_utf16(&mut utf16) {
            write!(out, "%u{unit:04X}").expect("writing to a String cannot fail");
        }
    }
    out
}

pub(super) fn parse_hex_byte(bytes: &[u8]) -> Option<u8> {
    if bytes.len() < 2 {
        return None;
    }
    let hi = (bytes[0] as char)
        .to_digit(16)
        .filter(|_| bytes[0].is_ascii())? as u8;
    let lo = (bytes[1] as char)
        .to_digit(16)
        .filter(|_| bytes[1].is_ascii())? as u8;
    Some((hi << 4) | lo)
}

pub(super) fn parse_hex_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 4 {
        return None;
    }
    let mut value = 0u16;
    for &byte in &bytes[..4] {
        let digit = (byte as char).to_digit(16).filter(|_| byte.is_ascii())? as u16;
        value = (value << 4) | digit;
    }
    Some(value)
}

fn push_char_utf8(out: &mut Vec<u8>, ch: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

pub(crate) fn url_decode_bytes(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 6 <= bytes.len()
                    && matches!(bytes[index + 1], b'u' | b'U')
                    && let Some(first_unit) = parse_hex_u16(&bytes[index + 2..index + 6])
                {
                    if (0xD800..=0xDBFF).contains(&first_unit)
                        && index + 12 <= bytes.len()
                        && bytes[index + 6] == b'%'
                        && matches!(bytes[index + 7], b'u' | b'U')
                        && let Some(second_unit) = parse_hex_u16(&bytes[index + 8..index + 12])
                        && (0xDC00..=0xDFFF).contains(&second_unit)
                    {
                        let codepoint = 0x10000
                            + (((first_unit as u32 - 0xD800) << 10)
                                | (second_unit as u32 - 0xDC00));
                        if let Some(ch) = char::from_u32(codepoint) {
                            push_char_utf8(&mut out, ch);
                            index += 12;
                            continue;
                        }
                    }
                    if let Some(ch) = char::from_u32(first_unit as u32) {
                        push_char_utf8(&mut out, ch);
                        index += 6;
                        continue;
                    }
                }
                if index + 3 <= bytes.len()
                    && let Some(decoded) = parse_hex_byte(&bytes[index + 1..index + 3])
                {
                    out.push(decoded);
                    index += 3;
                    continue;
                }
                out.push(b'%');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

fn url_decode(input: &str) -> Result<String, String> {
    String::from_utf8(url_decode_bytes(input)).map_err(|_| {
        "URL 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。".into()
    })
}

fn html_encode_key(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn html_encode_decimal(input: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(input.len().saturating_mul(5));
    for ch in input.chars() {
        write!(out, "&#{};", ch as u32).expect("writing to a String cannot fail");
    }
    out
}

fn html_encode_hex(input: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(input.len().saturating_mul(6));
    for ch in input.chars() {
        write!(out, "&#x{:x};", ch as u32).expect("writing to a String cannot fail");
    }
    out
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        _ => {
            let numeric = entity.strip_prefix('#')?;
            let codepoint = if let Some(hex) = numeric
                .strip_prefix('x')
                .or_else(|| numeric.strip_prefix('X'))
            {
                if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return None;
                }
                u32::from_str_radix(hex, 16).ok()?
            } else {
                if numeric.is_empty() || !numeric.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                numeric.parse::<u32>().ok()?
            };
            char::from_u32(codepoint)
        }
    }
}

fn html_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after_amp = &rest[amp + 1..];
        if let Some(semi) = after_amp.find(';') {
            let candidate = &after_amp[..semi];
            if !candidate.contains('&')
                && let Some(decoded) = decode_html_entity(candidate)
            {
                out.push(decoded);
                rest = &after_amp[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    out
}

pub(crate) fn base64_decode_bytes(input: &str) -> Result<Vec<u8>, String> {
    let filtered: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if filtered.is_empty() {
        return Err("所选内容不是有效的 Base64 编码。".into());
    }
    STANDARD
        .decode(&filtered)
        .or_else(|_| STANDARD_NO_PAD.decode(&filtered))
        .or_else(|_| URL_SAFE.decode(&filtered))
        .or_else(|_| URL_SAFE_NO_PAD.decode(&filtered))
        .map_err(|_| String::from("所选内容不是有效的 Base64 编码。"))
}

fn base64_decode(input: &str) -> Result<String, String> {
    let bytes = base64_decode_bytes(input)?;
    String::from_utf8(bytes).map_err(|_| {
        "Base64 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。"
            .into()
    })
}

pub(crate) fn base64_url_safe_decode_bytes(input: &str) -> Result<Vec<u8>, String> {
    let filtered: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if filtered.is_empty() {
        return Err("所选内容不是有效的 Base64 URL-safe 编码。".into());
    }
    URL_SAFE
        .decode(&filtered)
        .or_else(|_| URL_SAFE_NO_PAD.decode(&filtered))
        .or_else(|_| STANDARD.decode(&filtered))
        .or_else(|_| STANDARD_NO_PAD.decode(&filtered))
        .map_err(|_| String::from("所选内容不是有效的 Base64 URL-safe 编码。"))
}

fn base64_url_safe_decode(input: &str) -> Result<String, String> {
    let bytes = base64_url_safe_decode_bytes(input)?;
    String::from_utf8(bytes).map_err(|_| {
        "Base64 URL-safe 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。"
            .into()
    })
}

fn html_encode_all(input: &str) -> String {
    use std::fmt::Write;

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

fn construct_js_from_char_code(input: &str) -> String {
    let mut units = Vec::new();
    let mut buf = [0u16; 2];
    for ch in input.chars() {
        for &mut unit in ch.encode_utf16(&mut buf) {
            units.push(unit.to_string());
        }
    }
    format!("String.fromCharCode({})", units.join(","))
}

fn construct_mssql_char(input: &str) -> String {
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

fn construct_oracle_chr(input: &str) -> String {
    if input.is_empty() {
        return "''".to_owned();
    }
    input
        .chars()
        .map(|ch| format!("chr({})", ch as u32))
        .collect::<Vec<_>>()
        .join("||")
}

fn construct_mysql_char(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "''".to_owned();
    }
    let formatted = bytes
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("CHAR({formatted})")
}

pub(crate) const DEFAULT_HIDDEN_REQUEST_HEADERS: &[&str] = &[
    "accept",
    "accept-encoding",
    "accept-language",
    "cache-control",
    "if-modified-since",
    "if-none-match",
    "pragma",
    "priority",
    "sec-ch-ua",
    "sec-ch-ua-arch",
    "sec-ch-ua-bitness",
    "sec-ch-ua-full-version",
    "sec-ch-ua-full-version-list",
    "sec-ch-ua-mobile",
    "sec-ch-ua-model",
    "sec-ch-ua-platform",
    "sec-ch-ua-platform-version",
    "sec-ch-ua-wow64",
    "sec-fetch-dest",
    "sec-fetch-mode",
    "sec-fetch-site",
    "sec-fetch-user",
    "upgrade-insecure-requests",
    "user-agent",
    "x-requested-with",
];

pub(crate) const DEFAULT_HIDDEN_RESPONSE_HEADERS: &[&str] = &[
    "accept-ranges",
    "alt-svc",
    "cache-control",
    "cf-ray",
    "connection",
    "content-security-policy",
    "cross-origin-embedder-policy",
    "date",
    "etag",
    "expires",
    "last-modified",
    "nel",
    "p3p",
    "permissions-policy",
    "pragma",
    "referrer-policy",
    "report-to",
    "server",
    "set-cookie",
    "strict-transport-security",
    "vary",
    "via",
    "x-content-security-policy",
    "x-content-type-options",
    "x-frame-options",
    "x-ua-compatible",
    "x-xss-protection",
];

pub(crate) fn validate_http_header_names(names: &[String]) -> Result<Vec<String>, String> {
    mio_core::UserMessageEditorSettings::validate_hidden_header_names(names)
        .map_err(|error| error.to_string())?;
    Ok(names.to_vec())
}

pub(crate) fn filter_http_headers_bytes(
    raw: &[u8],
    custom_headers: Option<&[String]>,
) -> Result<(Vec<u8>, Vec<usize>), String> {
    let global_custom = if custom_headers.is_none() {
        if raw.starts_with(b"HTTP/") {
            crate::theme::hidden_response_headers()
        } else {
            crate::theme::hidden_request_headers()
        }
    } else {
        None
    };
    mio_core::filter_http_headers_bytes(
        raw,
        custom_headers.or(global_custom.as_deref()),
        DEFAULT_HIDDEN_REQUEST_HEADERS,
        DEFAULT_HIDDEN_RESPONSE_HEADERS,
    )
}

pub(crate) fn format_non_printable_preview_with_badges(
    bytes: &[u8],
) -> (String, Vec<std::ops::Range<usize>>) {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len().saturating_add(bytes.len() / 8));
    let mut badges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                let start = out.len();
                out.push_str(" \\r   \\n \n");
                badges.push(start..start + 4);
                badges.push(start + 5..start + 9);
                index += 2;
            }
            b'\r' => {
                let start = out.len();
                out.push_str(" \\r ");
                badges.push(start..start + 4);
                index += 1;
            }
            b'\n' => {
                let start = out.len();
                out.push_str(" \\n \n");
                badges.push(start..start + 4);
                index += 1;
            }
            b'\t' => {
                out.push_str("⇥\t");
                index += 1;
            }
            b' ' => {
                out.push(' ');
                index += 1;
            }
            byte if byte < 0x20 || byte == 0x7f => {
                write!(out, "\\x{byte:02x}").expect("writing to a String cannot fail");
                index += 1;
            }
            byte if byte < 0x80 => {
                out.push(byte as char);
                index += 1;
            }
            byte => {
                let expected_len = match byte {
                    0xC2..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    0xF0..=0xF4 => 4,
                    _ => 0,
                };
                if expected_len > 0
                    && let Some(slice) = bytes.get(index..index + expected_len)
                    && let Ok(valid) = std::str::from_utf8(slice)
                    && let Some(ch) = valid.chars().next()
                {
                    if ch.is_control() || matches!(ch, '\u{200B}'..='\u{200F}' | '\u{FEFF}') {
                        write!(out, "\\u{{{:04x}}}", ch as u32)
                            .expect("writing to a String cannot fail");
                    } else {
                        out.push(ch);
                    }
                    index += expected_len;
                } else {
                    write!(out, "\\x{byte:02x}").expect("writing to a String cannot fail");
                    index += 1;
                }
            }
        }
    }
    (out, badges)
}

fn ascii_hex_encode_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

fn ascii_hex_encode(input: &str) -> String {
    ascii_hex_encode_bytes(input.as_bytes())
}

fn ascii_hex_decode(input: &str) -> Result<String, String> {
    let bytes = hex_decode(input)?;
    String::from_utf8(bytes).map_err(|_| {
        "ASCII Hex 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。"
            .into()
    })
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut text = String::with_capacity(bytes.len().saturating_mul(3));
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            text.push(if index % 16 == 0 { '\n' } else { ' ' });
        }
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

pub(crate) fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(text.len() / 3);
    let mut high = None;
    for (offset, character) in text.char_indices() {
        if character.is_ascii_whitespace() {
            continue;
        }
        let nibble = character
            .to_digit(16)
            .filter(|_| character.is_ascii())
            .ok_or_else(|| format!("Hex 第 {} 字节位置不是十六进制数字。", offset + 1))?
            as u8;
        if let Some(first) = high.take() {
            bytes.push((first << 4) | nibble);
        } else {
            high = Some(nibble);
        }
    }
    if high.is_some() {
        return Err("Hex 必须为完整字节：每个字节需要两位十六进制数字。".into());
    }
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedResponseFormInput {
    pub name: String,
    pub input_type: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedResponseForm {
    pub action: String,
    pub method: String,
    pub inputs: Vec<RenderedResponseFormInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedResponseLink {
    pub href: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RenderedBodySummary {
    Html {
        title: Option<String>,
        headings: Vec<(u8, String)>,
        paragraphs: Vec<String>,
        links: Vec<RenderedResponseLink>,
        forms: Vec<RenderedResponseForm>,
        scripts: Vec<String>,
    },
    Json {
        pretty: String,
        byte_len: usize,
    },
    Image {
        format_name: &'static str,
        mime_type: &'static str,
        width: Option<u32>,
        height: Option<u32>,
        byte_len: usize,
    },
    PlainText {
        text: String,
        byte_len: usize,
    },
    Binary {
        byte_len: usize,
        hex_preview: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedHttpResponse {
    pub status_line: String,
    pub status_code: Option<u16>,
    pub content_type: Option<String>,
    pub location: Option<String>,
    pub body: Vec<u8>,
    pub body_len: usize,
    pub summary: RenderedBodySummary,
}

pub(crate) fn parse_rendered_http_response(raw: &[u8]) -> RenderedHttpResponse {
    let mio_core::ParsedHttpResponseHead {
        status_line,
        status_code,
        content_type,
        location,
        body,
    } = mio_core::parse_http_response_parts(raw);
    let (mime_essence, header_charset) = content_type
        .as_deref()
        .map(mio_core::extract_content_type_essence_and_charset)
        .unwrap_or_default();
    let body_slice = body.as_slice();
    let decoded_body_text = header_charset
        .as_deref()
        .and_then(|label| {
            super::MessageDisplayCharset::from_label(label)
                .and_then(|cs| cs.decode(body_slice))
        })
        .or_else(|| std::str::from_utf8(body_slice).ok().map(str::to_owned));

    let summary = if let Some((format_name, mime_type, width, height)) =
        detect_image_metadata(body_slice, &mime_essence)
    {
        RenderedBodySummary::Image {
            format_name,
            mime_type,
            width,
            height,
            byte_len: body_slice.len(),
        }
    } else if mime_essence.contains("html") || looks_like_html_bytes(body_slice) {
        let html_text = decoded_body_text
            .clone()
            .unwrap_or_else(|| String::from_utf8_lossy(body_slice).into_owned());
        extract_html_render_summary(&html_text)
    } else if mime_essence == "application/json"
        || mime_essence.ends_with("+json")
        || serde_json::from_slice::<serde_json::Value>(body_slice).is_ok()
    {
        if let Some(ref text) = decoded_body_text {
            let pretty = serde_json::from_str::<serde_json::Value>(text.trim())
                .ok()
                .and_then(|val| serde_json::to_string_pretty(&val).ok())
                .unwrap_or_else(|| text.trim().to_owned());
            RenderedBodySummary::Json {
                pretty,
                byte_len: body_slice.len(),
            }
        } else {
            RenderedBodySummary::Binary {
                byte_len: body_slice.len(),
                hex_preview: hex_encode(&body_slice[..body_slice.len().min(256)]),
            }
        }
    } else if let Some(text) = decoded_body_text {
        RenderedBodySummary::PlainText {
            text: text.chars().take(16384).collect(),
            byte_len: body_slice.len(),
        }
    } else {
        RenderedBodySummary::Binary {
            byte_len: body_slice.len(),
            hex_preview: hex_encode(&body_slice[..body_slice.len().min(256)]),
        }
    };

    let body_len = body.len();
    RenderedHttpResponse {
        status_line,
        status_code,
        content_type,
        location,
        body,
        body_len,
        summary,
    }
}

pub(crate) fn extract_http_response_body_and_content_type(
    raw: &[u8],
) -> (Vec<u8>, Option<String>) {
    let parsed = mio_core::parse_http_response_parts(raw);
    (parsed.body, parsed.content_type)
}

pub(crate) fn extract_http_response_body(raw: &[u8]) -> Vec<u8> {
    mio_core::parse_http_response_parts(raw).body
}

fn looks_like_html_bytes(body: &[u8]) -> bool {
    let trimmed = body.trim_ascii_start();
    let prefix = &trimmed[..trimmed.len().min(256)];
    let lower = String::from_utf8_lossy(prefix).to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.starts_with("<head")
        || lower.starts_with("<body")
        || lower.contains("<title")
        || lower.contains("<form")
}

fn detect_image_metadata(
    body: &[u8],
    mime_essence: &str,
) -> Option<(&'static str, &'static str, Option<u32>, Option<u32>)> {
    if let Ok(format) = image::guess_format(body) {
        let info = match format {
            image::ImageFormat::Png => Some(("PNG", "image/png")),
            image::ImageFormat::Jpeg => Some(("JPEG", "image/jpeg")),
            image::ImageFormat::Gif => Some(("GIF", "image/gif")),
            image::ImageFormat::WebP => Some(("WebP", "image/webp")),
            _ => None,
        };
        if let Some((format_name, mime_type)) = info {
            let dims = imagesize::blob_size(body)
                .ok()
                .map(|sz| (sz.width as u32, sz.height as u32))
                .or_else(|| {
                    image::ImageReader::new(std::io::Cursor::new(body))
                        .with_guessed_format()
                        .ok()
                        .and_then(|r| r.into_dimensions().ok())
                });
            let (w, h) = dims.unzip();
            return Some((format_name, mime_type, w, h));
        }
    }
    let trimmed = body.trim_ascii_start();
    if mime_essence == "image/svg+xml"
        || trimmed.starts_with(b"<svg")
        || (trimmed.starts_with(b"<?xml")
            && String::from_utf8_lossy(&trimmed[..trimmed.len().min(512)]).contains("<svg"))
    {
        let text = String::from_utf8_lossy(trimmed);
        let (width, height) = mio_core::html_extract::parse_svg_dimensions(&text);
        return Some(("SVG", "image/svg+xml", width, height));
    }
    None
}

fn extract_html_render_summary(html: &str) -> RenderedBodySummary {
    let summary = mio_core::html_extract::extract_html_summary(html);
    RenderedBodySummary::Html {
        title: summary.title,
        headings: summary.headings,
        paragraphs: summary.paragraphs,
        links: summary
            .links
            .into_iter()
            .map(|l| RenderedResponseLink {
                href: l.href,
                text: l.text,
            })
            .collect(),
        forms: summary
            .forms
            .into_iter()
            .map(|f| RenderedResponseForm {
                action: f.action,
                method: f.method,
                inputs: f
                    .inputs
                    .into_iter()
                    .map(|i| RenderedResponseFormInput {
                        name: i.name,
                        input_type: i.input_type,
                        value: i.value,
                    })
                    .collect(),
            })
            .collect(),
        scripts: summary.scripts,
    }
}
