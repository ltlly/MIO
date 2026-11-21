use mio_plugin_api::Header;

use super::MAX_MIME_SNIFF_BYTES;

pub(crate) fn infer_history_display_mime_type(headers: &[Header], body: &[u8]) -> &'static str {
    for header in headers.iter().filter(|header| header.name.eq_ignore_ascii_case("content-type")) {
        let Ok(value) = std::str::from_utf8(&header.value) else { continue; };
        let Ok(content_type) = value.parse::<mime::Mime>() else { continue; };
        let essence = content_type.essence_str();
        if essence == "application/json" || essence == "text/json" || essence.ends_with("+json") {
            return "JSON";
        }
        if let Some(category) = proxy_mime_from_content_type(essence) {
            return category;
        }
    }
    let json_prefix = &body[..body.len().min(64 * 1024)];
    let trimmed_json = strip_bom_and_ascii_whitespace(json_prefix);
    if matches!(trimmed_json.first(), Some(b'{' | b'['))
        && serde_json::Deserializer::from_slice(trimmed_json)
            .into_iter::<serde::de::IgnoredAny>()
            .next()
            .is_some_and(|result| result.is_ok())
    {
        return "JSON";
    }
    infer_proxy_mime_type(headers, body)
}

/// Infers the 8-class 外部归档 Proxy `Mime type` identifier (`"html"`, `"script"`,
/// `"xml"`, `"css"`, `"text"`, `"image"`, `"flash"`, `"binary"`), or `""` when
/// the response carries neither a `Content-Type` header nor any body bytes.
///
/// Precedence aligns with `store::ext_archive::http_query::infer_history_mime`, while
/// bounding body sniffing strictly to the first [`MAX_MIME_SNIFF_BYTES`] bytes.
pub(crate) fn infer_proxy_mime_type(headers: &[Header], body: &[u8]) -> &'static str {
    let mut saw_content_type = false;
    for header in headers {
        if !header.name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        let essence = header
            .value
            .split(|&byte| byte == b';')
            .next()
            .unwrap_or_default()
            .trim_ascii();
        if !essence.is_empty() {
            saw_content_type = true;
            let essence = String::from_utf8_lossy(essence).to_ascii_lowercase();
            if let Some(mime) = proxy_mime_from_content_type(&essence) {
                return mime;
            }
        }
    }
    if body.is_empty() {
        return if saw_content_type { "binary" } else { "" };
    }
    let prefix = &body[..body.len().min(MAX_MIME_SNIFF_BYTES)];
    sniff_proxy_mime(prefix).unwrap_or("binary")
}

fn proxy_mime_from_content_type(essence: &str) -> Option<&'static str> {
    let value = essence.strip_suffix('+').unwrap_or(essence);
    match value {
        "text/html" | "application/xhtml+xml" => Some("html"),
        "application/javascript"
        | "text/javascript"
        | "application/x-javascript"
        | "application/ecmascript"
        | "text/ecmascript" => Some("script"),
        "application/xml" | "text/xml" => Some("xml"),
        "text/css" => Some("css"),
        "application/json" | "text/json" => Some("text"),
        "image/png"
        | "image/gif"
        | "image/jpeg"
        | "image/jpg"
        | "image/bmp"
        | "image/webp"
        | "image/x-icon"
        | "image/vnd.microsoft.icon"
        | "image/svg+xml"
        | "image/avif" => Some("image"),
        "application/x-shockwave-flash" | "application/futuresplash" => Some("flash"),
        _ if value.starts_with("text/") || value.ends_with("+xml") || value.ends_with("+json") => {
            Some("text")
        }
        _ => None,
    }
}

fn sniff_proxy_mime(prefix: &[u8]) -> Option<&'static str> {
    const MAGIC: &[(&[u8], &str)] = &[
        (b"%PDF-", "binary"),
        (b"\x89PNG\r\n\x1a\n", "image"),
        (b"GIF89a", "image"),
        (b"GIF87a", "image"),
        (b"\xff\xd8\xff", "image"),
        (b"BM", "image"),
        (b"FWS", "flash"),
        (b"CWS", "flash"),
        (b"ZWS", "flash"),
        (b"PK\x03\x04", "binary"),
        (b"OggS", "binary"),
        (b"RIFF", "binary"),
    ];
    if prefix.len() >= 12 && &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WEBP" {
        return Some("image");
    }
    for (magic, mime) in MAGIC {
        if prefix.starts_with(magic) {
            return Some(*mime);
        }
    }
    let trimmed = strip_bom_and_ascii_whitespace(prefix);
    if starts_with_ignore_ascii_case(trimmed, b"<!doctype html")
        && trimmed
            .get(b"<!doctype html".len())
            .is_none_or(|&byte| byte.is_ascii_whitespace() || byte == b'>')
    {
        return Some("html");
    }
    if starts_with_ignore_ascii_case(trimmed, b"<html")
        && trimmed
            .get(b"<html".len())
            .is_none_or(|&byte| byte.is_ascii_whitespace() || byte == b'>' || byte == b'/')
    {
        return Some("html");
    }
    if starts_with_ignore_ascii_case(trimmed, b"<?xml")
        && trimmed
            .get(b"<?xml".len())
            .is_none_or(|&byte| byte.is_ascii_whitespace() || byte == b'?')
    {
        return Some("xml");
    }
    if trimmed.starts_with(b"{\"") || trimmed.starts_with(b"{\n") || trimmed.starts_with(b"{\r") {
        return Some("text");
    }
    has_valid_utf8_text_prefix(prefix).then_some("text")
}

fn strip_bom_and_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let without_bom = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    without_bom.trim_ascii_start()
}

fn starts_with_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .get(..needle.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(needle))
}

fn has_valid_utf8_text_prefix(prefix: &[u8]) -> bool {
    if prefix.contains(&0) {
        return false;
    }
    match std::str::from_utf8(prefix) {
        Ok(_) => true,
        Err(error) => {
            error.error_len().is_none()
                && prefix.len() == MAX_MIME_SNIFF_BYTES
                && error.valid_up_to() > 0
        }
    }
}
