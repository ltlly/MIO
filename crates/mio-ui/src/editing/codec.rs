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
        ConvertKind::HtmlEncodeDecimal => Ok(html_encode_decimal(input)),
        ConvertKind::HtmlEncodeHex => Ok(html_encode_hex(input)),
        ConvertKind::HtmlDecode => Ok(html_decode(input)),
        ConvertKind::Base64Encode => Ok(STANDARD.encode(input.as_bytes())),
        ConvertKind::Base64Decode => base64_decode(input),
        ConvertKind::Base64UrlSafeEncode => Ok(URL_SAFE_NO_PAD.encode(input.as_bytes())),
        ConvertKind::Base64UrlSafeDecode => base64_url_safe_decode(input),
        ConvertKind::AsciiHexEncode => Ok(ascii_hex_encode(input)),
        ConvertKind::AsciiHexDecode => ascii_hex_decode(input),
        ConvertKind::ConstructJsString => Ok(construct_js_string(input.as_bytes())),
        ConvertKind::ConstructSqlChar => Ok(construct_sql_char(input.as_bytes())),
    }
}

pub(crate) fn convert_bytes(
    kind: ConvertKind,
    input_bytes: &[u8],
    input_text: &str,
) -> Result<Vec<u8>, String> {
    match kind {
        ConvertKind::UrlEncodeKey => Ok(url_encode_key_bytes(input_bytes).into_bytes()),
        ConvertKind::UrlEncodeAll => Ok(url_encode_all_bytes(input_bytes).into_bytes()),
        ConvertKind::UrlEncodeUnicode => {
            let text = std::str::from_utf8(input_bytes)
                .map_err(|_| "当前输入包含非 UTF-8 字节，无法执行 Unicode URL 编码。".to_owned())?;
            Ok(url_encode_unicode(text).into_bytes())
        }
        ConvertKind::UrlDecode => Ok(url_decode_bytes(input_text)),
        ConvertKind::HtmlEncodeKey => {
            let text = std::str::from_utf8(input_bytes)
                .map_err(|_| "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。".to_owned())?;
            Ok(html_encode_key(text).into_bytes())
        }
        ConvertKind::HtmlEncodeDecimal => {
            let text = std::str::from_utf8(input_bytes)
                .map_err(|_| "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。".to_owned())?;
            Ok(html_encode_decimal(text).into_bytes())
        }
        ConvertKind::HtmlEncodeHex => {
            let text = std::str::from_utf8(input_bytes)
                .map_err(|_| "当前输入包含非 UTF-8 字节，无法执行 HTML 编码。".to_owned())?;
            Ok(html_encode_hex(text).into_bytes())
        }
        ConvertKind::HtmlDecode => Ok(html_decode(input_text).into_bytes()),
        ConvertKind::Base64Encode => Ok(STANDARD.encode(input_bytes).into_bytes()),
        ConvertKind::Base64Decode => base64_decode_bytes(input_text),
        ConvertKind::Base64UrlSafeEncode => Ok(URL_SAFE_NO_PAD.encode(input_bytes).into_bytes()),
        ConvertKind::Base64UrlSafeDecode => base64_url_safe_decode_bytes(input_text),
        ConvertKind::AsciiHexEncode => Ok(ascii_hex_encode_bytes(input_bytes).into_bytes()),
        ConvertKind::AsciiHexDecode => hex_decode(input_text),
        ConvertKind::ConstructJsString => Ok(construct_js_string(input_bytes).into_bytes()),
        ConvertKind::ConstructSqlChar => Ok(construct_sql_char(input_bytes).into_bytes()),
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

fn construct_js_string(bytes: &[u8]) -> String {
    use std::fmt::Write;

    if bytes.is_empty() {
        return "\"\"".to_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let mut parts = Vec::with_capacity(text.chars().count());
        for ch in text.chars() {
            let literal = match ch {
                '\\' => "'\\\\'".to_owned(),
                '\'' => "'\\''".to_owned(),
                '\r' => "'\\r'".to_owned(),
                '\n' => "'\\n'".to_owned(),
                '\t' => "'\\t'".to_owned(),
                c if c.is_control() => {
                    let mut buf = String::new();
                    let mut utf16 = [0u16; 2];
                    for (idx, &mut unit) in c.encode_utf16(&mut utf16).iter_mut().enumerate() {
                        if idx > 0 {
                            buf.push_str(" + ");
                        }
                        write!(buf, "'\\u{unit:04x}'").expect("writing to a String cannot fail");
                    }
                    buf
                }
                c => format!("'{c}'"),
            };
            parts.push(literal);
        }
        return parts.join(" + ");
    }
    bytes
        .iter()
        .map(|byte| format!("'\\x{byte:02x}'"))
        .collect::<Vec<_>>()
        .join(" + ")
}

fn construct_sql_char(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "''".to_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes)
        && text.chars().all(|ch| (ch as u32) <= 0xFFFF)
    {
        let codes = text
            .chars()
            .map(|ch| format!("CHAR({})", ch as u32))
            .collect::<Vec<_>>();
        return codes.join("+");
    }
    bytes
        .iter()
        .map(|byte| format!("CHAR({byte})"))
        .collect::<Vec<_>>()
        .join("+")
}

const MAX_HTTP_HEADER_SLOTS: usize = 4096;

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

fn header_matches_filter(name: &str, rules: Option<&[String]>, default_rules: &[&str]) -> bool {
    match rules {
        Some(custom) => custom
            .iter()
            .any(|rule| name.eq_ignore_ascii_case(rule)),
        None => default_rules
            .iter()
            .any(|rule| name.eq_ignore_ascii_case(rule)),
    }
}

fn slice_offset(base: &[u8], sub: &[u8]) -> usize {
    sub.as_ptr() as usize - base.as_ptr() as usize
}

fn first_line_end(raw: &[u8], limit: usize) -> Option<(usize, &'static [u8])> {
    let head = &raw[..limit.min(raw.len())];
    let newline = head.iter().position(|&b| b == b'\n')?;
    if newline > 0 && head[newline - 1] == b'\r' {
        Some((newline + 1, b"\r\n"))
    } else {
        Some((newline + 1, b"\n"))
    }
}

fn rebuild_filtered_head(
    raw: &[u8],
    parsed_input: &[u8],
    added_start_line_bytes: usize,
    body_offset: usize,
    headers: &[httparse::Header<'_>],
    rules: Option<&[String]>,
    default_rules: &[&str],
) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::with_capacity(raw.len());
    let mut line_numbers = Vec::new();
    let mut retained_start = 0;
    let mut current_line = 1usize;
    for header in headers {
        if !header_matches_filter(header.name, rules, default_rules) {
            continue;
        }
        let name_start = slice_offset(parsed_input, header.name.as_bytes()) - added_start_line_bytes;
        let value_end = slice_offset(parsed_input, header.value) + header.value.len()
            - added_start_line_bytes;
        let line_start = raw[..name_start]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .expect("httparse header must follow a complete start line")
            + 1;
        let line_end = value_end
            + raw[value_end..body_offset]
                .iter()
                .position(|&byte| byte == b'\n')
                .expect("httparse complete header must end with a newline")
            + 1;
        let retained_slice = &raw[retained_start..line_start];
        let retained_lines = retained_slice.iter().filter(|&&byte| byte == b'\n').count();
        line_numbers.extend(current_line..current_line + retained_lines);
        current_line += retained_lines;
        out.extend_from_slice(retained_slice);
        let skipped_lines = raw[line_start..line_end]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        current_line += skipped_lines;
        retained_start = line_end;
    }
    let tail = &raw[retained_start..];
    let tail_newlines = tail.iter().filter(|&&byte| byte == b'\n').count();
    line_numbers.extend(current_line..=current_line + tail_newlines);
    out.extend_from_slice(tail);
    (out, line_numbers)
}

fn normalize_http2_start_line(line: &[u8]) -> Option<(Vec<u8>, bool)> {
    if let Some(rest) = line.strip_prefix(b"HTTP/2.0 ") {
        let mut normalized = Vec::with_capacity(line.len());
        normalized.extend_from_slice(b"HTTP/1.1 ");
        normalized.extend_from_slice(rest);
        return Some((normalized, true));
    }
    if let Some(rest) = line.strip_prefix(b"HTTP/2 ") {
        let mut normalized = Vec::with_capacity(line.len() + 2);
        normalized.extend_from_slice(b"HTTP/1.1 ");
        normalized.extend_from_slice(rest);
        return Some((normalized, true));
    }
    if let Some(prefix) = line.strip_suffix(b" HTTP/2.0") {
        let mut normalized = Vec::with_capacity(line.len());
        normalized.extend_from_slice(prefix);
        normalized.extend_from_slice(b" HTTP/1.1");
        return Some((normalized, false));
    }
    if let Some(prefix) = line.strip_suffix(b" HTTP/2") {
        let mut normalized = Vec::with_capacity(line.len() + 2);
        normalized.extend_from_slice(prefix);
        normalized.extend_from_slice(b" HTTP/1.1");
        return Some((normalized, false));
    }
    None
}

pub(crate) fn filter_http_headers_bytes(
    raw: &[u8],
    custom_headers: Option<&[String]>,
) -> Result<(Vec<u8>, Vec<usize>), String> {
    if raw.is_empty() {
        return Ok((Vec::new(), vec![1]));
    }
    let (start_end, eol) = first_line_end(raw, raw.len())
        .ok_or_else(|| "消息不包含可解析的 HTTP 起始行与标头块。".to_owned())?;
    let line_without_eol = &raw[..start_end - eol.len()];
    let normalized = normalize_http2_start_line(line_without_eol).map(|(mut line, _)| {
        line.extend_from_slice(&raw[start_end - eol.len()..]);
        line
    });
    let parsed_input = normalized.as_deref().unwrap_or(raw);
    let added_start_line_bytes = parsed_input.len() - raw.len();
    let mut headers = vec![httparse::EMPTY_HEADER; MAX_HTTP_HEADER_SLOTS];
    let mut config = httparse::ParserConfig::default();
    config
        .allow_multiple_spaces_in_request_line_delimiters(true)
        .allow_multiple_spaces_in_response_status_delimiters(true)
        .allow_space_before_first_header_name(true)
        .allow_obsolete_multiline_headers_in_responses(true);
    let (body_offset, parsed_headers, defaults, global_custom) = if raw.starts_with(b"HTTP/") {
        let mut response = httparse::Response::new(&mut headers);
        let status = config
            .parse_response(&mut response, parsed_input)
            .map_err(|error| format!("HTTP 响应解析失败（{error}），无法隐藏指定标头。"))?;
        let httparse::Status::Complete(offset) = status else {
            return Err("HTTP 响应标头未以空行完整结束，无法隐藏指定标头。".into());
        };
        (
            offset,
            response.headers,
            DEFAULT_HIDDEN_RESPONSE_HEADERS,
            custom_headers
                .is_none()
                .then(crate::theme::hidden_response_headers)
                .flatten(),
        )
    } else {
        let mut request = httparse::Request::new(&mut headers);
        let status = config
            .parse_request(&mut request, parsed_input)
            .map_err(|error| format!("HTTP 请求解析失败（{error}），无法隐藏指定标头。"))?;
        let httparse::Status::Complete(offset) = status else {
            return Err("HTTP 请求标头未以空行完整结束，无法隐藏指定标头。".into());
        };
        (
            offset,
            request.headers,
            DEFAULT_HIDDEN_REQUEST_HEADERS,
            custom_headers
                .is_none()
                .then(crate::theme::hidden_request_headers)
                .flatten(),
        )
    };
    Ok(rebuild_filtered_head(
        raw,
        parsed_input,
        added_start_line_bytes,
        body_offset - added_start_line_bytes,
        parsed_headers,
        custom_headers.or(global_custom.as_deref()),
        defaults,
    ))
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
    pub body_len: usize,
    pub summary: RenderedBodySummary,
}

pub(crate) fn parse_rendered_http_response(raw: &[u8]) -> RenderedHttpResponse {
    let (status_line, status_code, content_type, location, body_slice) =
        match mio_core::parse_response(raw, mio_core::HttpVersion::Http11) {
            Ok(resp) => {
                let version = match resp.version {
                    mio_core::HttpVersion::Http10 => "HTTP/1.0",
                    mio_core::HttpVersion::Http2 => "HTTP/2",
                    mio_core::HttpVersion::Http11 => "HTTP/1.1",
                };
                let mut ct = None;
                let mut loc = None;
                for header in &resp.headers {
                    if header.name.eq_ignore_ascii_case("content-type") && ct.is_none() {
                        ct = Some(String::from_utf8_lossy(&header.value).trim().to_owned());
                    } else if header.name.eq_ignore_ascii_case("location") && loc.is_none() {
                        loc = Some(String::from_utf8_lossy(&header.value).trim().to_owned());
                    }
                }
                (
                    format!("{version} {}", resp.status),
                    Some(resp.status),
                    ct,
                    loc,
                    resp.body,
                )
            }
            Err(_) => {
                let mut headers_buf = vec![httparse::EMPTY_HEADER; 2048];
                let mut parsed = httparse::Response::new(&mut headers_buf);
                match parsed.parse(raw) {
                    Ok(httparse::Status::Complete(offset)) => {
                        let ver = match parsed.version {
                            Some(0) => "HTTP/1.0",
                            _ => "HTTP/1.1",
                        };
                        let code = parsed.code;
                        let reason = parsed.reason.unwrap_or("").trim();
                        let status_line = match (code, reason.is_empty()) {
                            (Some(c), false) => format!("{ver} {c} {reason}"),
                            (Some(c), true) => format!("{ver} {c}"),
                            _ => ver.to_owned(),
                        };
                        let mut ct = None;
                        let mut loc = None;
                        for h in parsed.headers.iter() {
                            if h.name.eq_ignore_ascii_case("content-type") && ct.is_none() {
                                ct = Some(String::from_utf8_lossy(h.value).trim().to_owned());
                            } else if h.name.eq_ignore_ascii_case("location") && loc.is_none() {
                                loc = Some(String::from_utf8_lossy(h.value).trim().to_owned());
                            }
                        }
                        (status_line, code, ct, loc, raw[offset..].to_vec())
                    }
                    _ => (
                        "无标准 HTTP 状态行".to_owned(),
                        None,
                        None,
                        None,
                        raw.to_vec(),
                    ),
                }
            }
        };
    let parsed_mime = content_type
        .as_deref()
        .and_then(|ct| ct.parse::<mime::Mime>().ok());
    let mime_essence = parsed_mime
        .as_ref()
        .map(|m| m.essence_str().to_owned())
        .unwrap_or_default();
    let header_charset = parsed_mime
        .as_ref()
        .and_then(|m| m.get_param(mime::CHARSET).map(|v| v.as_str().to_owned()))
        .or_else(|| {
            content_type.as_deref().and_then(|ct| {
                ct.split(';').skip(1).find_map(|part| {
                    let (k, v) = part.trim().split_once('=')?;
                    k.trim()
                        .eq_ignore_ascii_case("charset")
                        .then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
                })
            })
        });
    let body_slice = body_slice.as_slice();
    let decoded_body_text = header_charset
        .as_deref()
        .and_then(|label| {
            super::message_editor::MessageDisplayCharset::from_label(label)
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

    RenderedHttpResponse {
        status_line,
        status_code,
        content_type,
        location,
        body_len: body_slice.len(),
        summary,
    }
}

pub(crate) fn extract_http_response_body(raw: &[u8]) -> Vec<u8> {
    if let Ok(resp) = mio_core::parse_response(raw, mio_core::HttpVersion::Http11) {
        return resp.body;
    }
    let mut headers_buf = vec![httparse::EMPTY_HEADER; 2048];
    let mut parsed = httparse::Response::new(&mut headers_buf);
    if let Ok(httparse::Status::Complete(offset)) = parsed.parse(raw) {
        return raw[offset..].to_vec();
    }
    raw.to_vec()
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
        let (width, height) = parse_svg_dimensions(&text);
        return Some(("SVG", "image/svg+xml", width, height));
    }
    None
}

fn parse_svg_dimensions(svg_text: &str) -> (Option<u32>, Option<u32>) {
    let parse_dim_val = |raw: &str| -> Option<u32> {
        let digits: String = raw
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<u32>().ok()
    };
    if let Ok(doc) = roxmltree::Document::parse(svg_text) {
        let root = doc.root_element();
        if root.has_tag_name("svg") {
            let w = root.attribute("width").and_then(parse_dim_val);
            let h = root.attribute("height").and_then(parse_dim_val);
            return (w, h);
        }
    }
    let html_doc = scraper::Html::parse_fragment(svg_text);
    if let Ok(sel) = scraper::Selector::parse("svg")
        && let Some(svg_el) = html_doc.select(&sel).next()
    {
        let w = svg_el.value().attr("width").and_then(parse_dim_val);
        let h = svg_el.value().attr("height").and_then(parse_dim_val);
        return (w, h);
    }
    (None, None)
}

fn normalize_visible_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_html_render_summary(html: &str) -> RenderedBodySummary {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| document.select(&sel).next())
        .map(|el| normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" ")))
        .filter(|s| !s.is_empty());

    let mut headings = Vec::new();
    if let Ok(h_sel) = Selector::parse("h1, h2, h3, h4, h5, h6") {
        for el in document.select(&h_sel).take(64) {
            let name = el.value().name();
            let level = name.as_bytes().get(1).map(|b| b - b'0').unwrap_or(1);
            let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
            if !text.is_empty() {
                headings.push((level, text));
            }
        }
    }

    let mut paragraphs = Vec::new();
    if let Ok(p_sel) = Selector::parse("p, li, blockquote, pre, td, th") {
        for el in document.select(&p_sel) {
            if paragraphs.len() >= 120 {
                break;
            }
            let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
            if !text.is_empty() && !paragraphs.contains(&text) {
                paragraphs.push(text);
            }
        }
    }
    if paragraphs.is_empty()
        && let Ok(body_sel) = Selector::parse("body")
        && let Some(body_el) = document.select(&body_sel).next()
    {
        let text = collect_visible_element_text(body_el);
        if !text.is_empty() {
            paragraphs.push(text);
        }
    }

    let mut links = Vec::new();
    if let Ok(a_sel) = Selector::parse("a[href]") {
        for el in document.select(&a_sel).take(200) {
            if let Some(href) = el.value().attr("href") {
                let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
                links.push(RenderedResponseLink {
                    href: href.to_owned(),
                    text,
                });
            }
        }
    }

    let mut forms = Vec::new();
    let ctrl_sel = Selector::parse("input, select, textarea, button").ok();
    if let Ok(form_sel) = Selector::parse("form") {
        for form_el in document.select(&form_sel).take(64) {
            let action = form_el.value().attr("action").unwrap_or("/").to_owned();
            let method = form_el
                .value()
                .attr("method")
                .map(|m| m.trim().to_ascii_uppercase())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "GET".into());
            let mut inputs = Vec::new();
            if let Some(ref c_sel) = ctrl_sel {
                for ctrl_el in form_el.select(c_sel).take(128) {
                    inputs.push(extract_form_control(ctrl_el));
                }
            }
            forms.push(RenderedResponseForm {
                action,
                method,
                inputs,
            });
        }
    }
    if forms.is_empty()
        && let Some(ref c_sel) = ctrl_sel
    {
        let standalone: Vec<RenderedResponseFormInput> = document
            .select(c_sel)
            .take(128)
            .map(extract_form_control)
            .filter(|c| !c.name.is_empty())
            .collect();
        if !standalone.is_empty() {
            forms.push(RenderedResponseForm {
                action: "(独立表单控件)".into(),
                method: "GET".into(),
                inputs: standalone,
            });
        }
    }

    let mut scripts = Vec::new();
    if let Ok(s_sel) = Selector::parse("script[src]") {
        for el in document.select(&s_sel).take(128) {
            if let Some(src) = el.value().attr("src") {
                let trimmed = src.trim();
                if !trimmed.is_empty() {
                    scripts.push(trimmed.to_owned());
                }
            }
        }
    }

    RenderedBodySummary::Html {
        title,
        headings,
        paragraphs,
        links,
        forms,
        scripts,
    }
}

fn extract_form_control(el: scraper::ElementRef<'_>) -> RenderedResponseFormInput {
    let tag_name = el.value().name();
    let name = el
        .value()
        .attr("name")
        .or_else(|| el.value().attr("id"))
        .unwrap_or("")
        .to_owned();
    let input_type = el
        .value()
        .attr("type")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if tag_name == "input" {
                "text".into()
            } else {
                tag_name.to_owned()
            }
        });
    let value = el
        .value()
        .attr("value")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if tag_name == "textarea" {
                el.text().collect::<String>()
            } else {
                String::new()
            }
        });
    RenderedResponseFormInput {
        name,
        input_type,
        value,
    }
}

fn collect_visible_element_text(root: scraper::ElementRef<'_>) -> String {
    let mut parts = Vec::new();
    collect_visible_nodes(root, &mut parts);
    normalize_visible_whitespace(&parts.join(" "))
}

fn collect_visible_nodes(el: scraper::ElementRef<'_>, out: &mut Vec<String>) {
    if matches!(
        el.value().name(),
        "script" | "style" | "noscript" | "template"
    ) {
        return;
    }
    for child in el.children() {
        match child.value() {
            scraper::Node::Text(t) => {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_owned());
                }
            }
            scraper::Node::Element(_) => {
                if let Some(child_el) = scraper::ElementRef::wrap(child) {
                    collect_visible_nodes(child_el, out);
                }
            }
            _ => {}
        }
    }
}
