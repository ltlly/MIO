use super::*;

mod markup;
use markup::{
    format_pretty_html, format_pretty_xml, looks_like_html_document, looks_like_xml_document,
};

const PRETTY_NON_UTF8_ERROR: &str =
    "消息含非 UTF-8 字节，无法切换到 Pretty 视图；请使用 Hex 视图查看原始字节。";
pub(super) const MAX_PRETTY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyKind {
    Json,
    Xml,
    Html,
    OtherSpecified,
    Unspecified,
}

fn is_http_start_line(first_line: &str) -> bool {
    if first_line.starts_with("HTTP/") {
        return true;
    }
    let mut parts = first_line.split_ascii_whitespace();
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(method), Some(target), Some(version), None)
            if token(method.as_bytes()) && !target.is_empty() && version.starts_with("HTTP/")
    )
}

fn header_body_kind(header_lines: &[&str]) -> BodyKind {
    for line in header_lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-type") {
            continue;
        }
        let essence = value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if essence.is_empty() {
            continue;
        }
        return if essence == "application/json" || essence.ends_with("+json") {
            BodyKind::Json
        } else if essence == "application/xml" || essence == "text/xml" || essence.ends_with("+xml")
        {
            BodyKind::Xml
        } else if essence == "text/html" {
            BodyKind::Html
        } else {
            BodyKind::OtherSpecified
        };
    }
    BodyKind::Unspecified
}

fn format_json_if_valid(body_str: &str, trimmed: &str) -> Option<String> {
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && body_str.len() <= MAX_PRETTY_BYTES
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Ok(pretty) = serde_json::to_string_pretty(&val)
    {
        Some(pretty)
    } else {
        None
    }
}

fn format_body_by_kind(kind: BodyKind, body_str: &str) -> std::result::Result<String, String> {
    let trimmed = body_str.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    match kind {
        BodyKind::Json => Ok(format_json_if_valid(body_str, trimmed)
            .unwrap_or_else(|| body_str.replace("\r\n", "\n"))),
        BodyKind::Xml => format_pretty_xml(body_str),
        BodyKind::Html => format_pretty_html(body_str),
        BodyKind::OtherSpecified => Ok(body_str.replace("\r\n", "\n")),
        BodyKind::Unspecified => {
            if let Some(pretty) = format_json_if_valid(body_str, trimmed) {
                Ok(pretty)
            } else if looks_like_html_document(trimmed) {
                format_pretty_html(body_str)
            } else if looks_like_xml_document(trimmed) {
                format_pretty_xml(body_str)
            } else {
                Ok(body_str.replace("\r\n", "\n"))
            }
        }
    }
}

fn is_chunked_header_lines(header_lines: &[&str]) -> bool {
    header_lines.iter().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn dechunk_pretty_body(body_str: &str) -> Option<(String, Vec<(String, String)>)> {
    let dummy_headers = [Header {
        name: "Transfer-Encoding".into(),
        value: b"chunked".to_vec(),
    }];
    if let Ok((decoded, trailers)) = super::body(&dummy_headers, body_str.as_bytes())
        && let Ok(text) = String::from_utf8(decoded)
    {
        let trailer_pairs = trailers
            .into_iter()
            .map(|h| (h.name, String::from_utf8_lossy(&h.value).into_owned()))
            .collect();
        return Some((text, trailer_pairs));
    }
    let normalized = if body_str.contains("\r\n") {
        None
    } else {
        Some(body_str.replace('\n', "\r\n"))
    };
    if let Some(crlf_body) = normalized
        && let Ok((decoded, trailers)) = super::body(&dummy_headers, crlf_body.as_bytes())
        && let Ok(text) = String::from_utf8(decoded)
    {
        let trailer_pairs = trailers
            .into_iter()
            .map(|h| (h.name, String::from_utf8_lossy(&h.value).into_owned()))
            .collect();
        return Some((text, trailer_pairs));
    }
    None
}

pub fn format_pretty_message(raw: &[u8]) -> std::result::Result<String, String> {
    if raw.is_empty() {
        return Ok(String::new());
    }
    let text = std::str::from_utf8(raw).map_err(|_| PRETTY_NON_UTF8_ERROR.to_owned())?;
    let split = text
        .find("\r\n\r\n")
        .map(|pos| (pos, 4))
        .into_iter()
        .chain(text.find("\n\n").map(|pos| (pos, 2)))
        .min_by_key(|(pos, _)| *pos);
    if let Some((head_end, delim_len)) = split {
        let head_str = &text[..head_end];
        let body_str = &text[head_end + delim_len..];
        let lines: Vec<&str> = head_str
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        if let Some(&first_line) = lines.first()
            && is_http_start_line(first_line)
        {
            let normalized_head = lines.join("\n");
            let kind = header_body_kind(&lines[1..]);
            if is_chunked_header_lines(&lines[1..])
                && let Some((dechunked, trailers)) = dechunk_pretty_body(body_str)
            {
                let mut pretty_body = if dechunked.trim().is_empty() {
                    dechunked.replace("\r\n", "\n")
                } else {
                    format_body_by_kind(kind, &dechunked)?
                };
                if !trailers.is_empty() {
                    if !pretty_body.is_empty() && !pretty_body.ends_with('\n') {
                        pretty_body.push('\n');
                    }
                    for (name, val) in trailers {
                        pretty_body.push_str(&format!("{name}: {val}\n"));
                    }
                }
                return Ok(format!("{normalized_head}\n\n{pretty_body}"));
            }
            let pretty_body = if body_str.trim().is_empty() {
                body_str.replace("\r\n", "\n")
            } else {
                format_body_by_kind(kind, body_str)?
            };
            return Ok(format!("{normalized_head}\n\n{pretty_body}"));
        }
    }
    let trimmed = text.trim();
    if let Some(pretty) = format_json_if_valid(text, trimmed) {
        return Ok(pretty);
    }
    if looks_like_html_document(trimmed) {
        return format_pretty_html(text);
    }
    if looks_like_xml_document(trimmed) {
        return format_pretty_xml(text);
    }
    Ok(text.replace("\r\n", "\n"))
}
