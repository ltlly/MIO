use anyhow::{Result, bail, ensure};

use super::{MAX_PAGE_TITLE_BYTES, MAX_TITLE_SCAN_BYTES};

const MAX_HTML_ENTITY_BYTES: usize = 32;

/// Extracts and normalizes the first HTML `<title>` within the leading
/// [`MAX_TITLE_SCAN_BYTES`] bytes of `body`, with an explicit check hook for
/// cooperative cancellation / deadline polling.
pub(crate) fn extract_html_page_title_checked(
    body: &[u8],
    check: &impl Fn() -> Result<()>,
) -> Result<String> {
    check()?;
    let window = &body[..body.len().min(MAX_TITLE_SCAN_BYTES)];
    let Some((content_start, self_closing)) = find_opening_title_tag(window, check)? else {
        return Ok(String::new());
    };
    if self_closing {
        return Ok(String::new());
    }
    let Some(content_end) = find_closing_title_tag(&window[content_start..], check)? else {
        bail!("Proxy 页面标题在响应前 256 KiB 内未闭合 </title>，未按空标题或截断标题匹配");
    };
    let raw_title = &window[content_start..content_start + content_end];
    ensure!(
        raw_title.len() <= MAX_TITLE_SCAN_BYTES,
        "Proxy 页面标题原始字节超过限额"
    );
    let decoded = decode_utf8_or_latin1(raw_title, check)?;
    let unescaped = decode_html_entities(&decoded, check)?;
    fold_html_title_whitespace(&unescaped, check)
}

/// Extracts and normalizes the first HTML `<title>` within the leading
/// [`MAX_TITLE_SCAN_BYTES`] bytes of `body`.
///
/// Returns `Ok(String::new())` when no `<title>` tag appears in the scan window,
/// and `Err` if an opening `<title>` tag is not closed within `MAX_TITLE_SCAN_BYTES`
/// or if the normalized result exceeds [`MAX_PAGE_TITLE_BYTES`] (16 KiB).
#[cfg(test)]
pub(crate) fn extract_html_page_title(body: &[u8]) -> Result<String> {
    extract_html_page_title_checked(body, &|| Ok(()))
}

fn find_opening_title_tag(
    window: &[u8],
    check: &impl Fn() -> Result<()>,
) -> Result<Option<(usize, bool)>> {
    let mut index = 0;
    while index + 6 <= window.len() {
        if index & 0x1fff == 0 {
            check()?;
        }
        if window[index] == b'<'
            && window[index + 1..index + 6].eq_ignore_ascii_case(b"title")
            && let Some(&delimiter) = window.get(index + 6)
        {
            if delimiter == b'>' {
                return Ok(Some((index + 7, false)));
            }
            if delimiter == b'/' && window.get(index + 7) == Some(&b'>') {
                return Ok(Some((index + 8, true)));
            }
            if is_html_space(delimiter) {
                let Some((tag_end, self_closing)) =
                    scan_html_open_tag_end(&window[index + 6..], check)?
                else {
                    bail!(
                        "Proxy 页面标题开标签 <title 在响应前 256 KiB 内未闭合 '>'，未按空标题匹配"
                    );
                };
                return Ok(Some((index + 6 + tag_end, self_closing)));
            }
        }
        index += 1;
    }
    check()?;
    Ok(None)
}

fn scan_html_open_tag_end(
    rest: &[u8],
    check: &impl Fn() -> Result<()>,
) -> Result<Option<(usize, bool)>> {
    let mut quote: Option<u8> = None;
    for (offset, &byte) in rest.iter().enumerate() {
        if offset & 0x1fff == 0 {
            check()?;
        }
        match quote {
            Some(active) if byte == active => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => {
                let before = rest[..offset].trim_ascii_end();
                let self_closing = before.ends_with(b"/");
                return Ok(Some((offset + 1, self_closing)));
            }
            None => {}
        }
    }
    check()?;
    Ok(None)
}

fn find_closing_title_tag(rest: &[u8], check: &impl Fn() -> Result<()>) -> Result<Option<usize>> {
    let mut index = 0;
    while index + 8 <= rest.len() {
        if index & 0x1fff == 0 {
            check()?;
        }
        if rest[index] == b'<'
            && rest[index + 1] == b'/'
            && rest[index + 2..index + 7].eq_ignore_ascii_case(b"title")
        {
            let mut cursor = index + 7;
            while let Some(&byte) = rest.get(cursor) {
                if is_html_space(byte) {
                    cursor += 1;
                } else {
                    break;
                }
            }
            if rest.get(cursor) == Some(&b'>') {
                return Ok(Some(index));
            }
        }
        index += 1;
    }
    check()?;
    Ok(None)
}

fn is_html_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

fn decode_utf8_or_latin1(bytes: &[u8], check: &impl Fn() -> Result<()>) -> Result<String> {
    check()?;
    if let Ok(utf8) = std::str::from_utf8(bytes) {
        return Ok(utf8.to_owned());
    }
    let mut decoded = String::with_capacity(bytes.len() * 2);
    for (index, &byte) in bytes.iter().enumerate() {
        if index & 0x1fff == 0 {
            check()?;
        }
        decoded.push(char::from(byte));
    }
    check()?;
    Ok(decoded)
}

fn decode_html_entities(input: &str, check: &impl Fn() -> Result<()>) -> Result<String> {
    let mut out = String::with_capacity(input.len().min(MAX_PAGE_TITLE_BYTES + 64));
    let mut rest = input;
    let mut steps = 0_usize;
    while let Some(amp) = rest.find('&') {
        steps = steps.wrapping_add(1);
        if steps & 0x3ff == 0 {
            check()?;
        }
        out.push_str(&rest[..amp]);
        let after_amp = &rest[amp + 1..];
        let window = &after_amp[..after_amp.floor_char_boundary(MAX_HTML_ENTITY_BYTES)];
        if let Some(semi) = window.find(';') {
            let candidate = &window[..semi];
            if !candidate.contains('&')
                && let Some(decoded) = decode_single_html_entity(candidate)
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
    check()?;
    Ok(out)
}

fn decode_single_html_entity(entity: &str) -> Option<char> {
    if entity.eq_ignore_ascii_case("amp") {
        return Some('&');
    }
    if entity.eq_ignore_ascii_case("lt") {
        return Some('<');
    }
    if entity.eq_ignore_ascii_case("gt") {
        return Some('>');
    }
    if entity.eq_ignore_ascii_case("quot") {
        return Some('"');
    }
    if entity == "apos" {
        return Some('\'');
    }
    if entity.eq_ignore_ascii_case("nbsp") {
        return Some('\u{00a0}');
    }
    let numeric = entity.strip_prefix('#')?;
    let codepoint = if let Some(hex) = numeric
        .strip_prefix('x')
        .or_else(|| numeric.strip_prefix('X'))
    {
        if hex.is_empty() || hex.len() > 8 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    } else {
        if numeric.is_empty() || numeric.len() > 8 || !numeric.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        numeric.parse::<u32>().ok()?
    };
    if codepoint == 0 {
        return None;
    }
    char::from_u32(codepoint)
}

fn fold_html_title_whitespace(input: &str, check: &impl Fn() -> Result<()>) -> Result<String> {
    let mut normalized = String::with_capacity(input.len().min(MAX_PAGE_TITLE_BYTES));
    let mut pending_space = false;
    for (index, ch) in input.chars().enumerate() {
        if index & 0x1fff == 0 {
            check()?;
        }
        if matches!(ch, ' ' | '\t' | '\n' | '\x0c' | '\r') {
            if !normalized.is_empty() {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            ensure!(
                normalized.len() < MAX_PAGE_TITLE_BYTES,
                "Proxy 页面标题规范化后超过 16 KiB 限额，未截断匹配"
            );
            normalized.push(' ');
            pending_space = false;
        }
        ensure!(
            normalized.len() + ch.len_utf8() <= MAX_PAGE_TITLE_BYTES,
            "Proxy 页面标题规范化后超过 16 KiB 限额，未截断匹配"
        );
        normalized.push(ch);
    }
    check()?;
    Ok(normalized)
}
