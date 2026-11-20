use std::ops::Range;

use super::syntax_highlight::TokenKind;

pub(super) fn tokenize_json(body: &str, offset: usize, out: &mut Vec<(Range<usize>, TokenKind)>) {
    let Ok(raw) = serde_json::from_str::<&serde_json::value::RawValue>(body) else {
        return;
    };
    let start = out.len();
    if !emit_raw_json_tokens(body, raw.get(), offset, out) {
        out.truncate(start);
    }
}

fn slice_range_in(whole: &str, part: &str) -> Range<usize> {
    let start = part.as_ptr() as usize - whole.as_ptr() as usize;
    start..start + part.len()
}

fn emit_raw_json_tokens(
    whole: &str,
    raw: &str,
    offset: usize,
    out: &mut Vec<(Range<usize>, TokenKind)>,
) -> bool {
    let trimmed = raw.trim();
    let range = slice_range_in(whole, trimmed);
    match trimmed.as_bytes().first().copied() {
        Some(b'"') => {
            out.push((offset + range.start..offset + range.end, TokenKind::JsonString));
            true
        }
        Some(b'-' | b'0'..=b'9') => {
            out.push((offset + range.start..offset + range.end, TokenKind::JsonNumber));
            true
        }
        Some(b't' | b'f' | b'n') => {
            out.push((offset + range.start..offset + range.end, TokenKind::JsonKeyword));
            true
        }
        Some(b'[') => {
            let Ok(items) = serde_json::from_str::<Vec<&serde_json::value::RawValue>>(trimmed)
            else {
                return false;
            };
            let mut prev_end = range.start + 1;
            for item in items {
                let item_str = item.get();
                let item_range = slice_range_in(whole, item_str);
                emit_punct_between(whole, prev_end..item_range.start, offset, out);
                if !emit_raw_json_tokens(whole, item_str, offset, out) {
                    return false;
                }
                prev_end = item_range.end;
            }
            emit_punct_between(whole, prev_end..range.end.saturating_sub(1), offset, out);
            true
        }
        Some(b'{') => {
            let inner = &trimmed[1..trimmed.len().saturating_sub(1)];
            let mut cursor = slice_range_in(whole, inner).start;
            let limit = slice_range_in(whole, inner).end;
            while cursor < limit {
                let rest = &whole[cursor..limit];
                let ws = rest.len() - rest.trim_start().len();
                cursor += ws;
                if cursor >= limit {
                    break;
                }
                if whole.as_bytes()[cursor] == b',' {
                    out.push((offset + cursor..offset + cursor + 1, TokenKind::QueryPunct));
                    cursor += 1;
                    continue;
                }
                let mut stream = serde_json::Deserializer::from_str(&whole[cursor..limit])
                    .into_iter::<&serde_json::value::RawValue>();
                let Some(Ok(key_raw)) = stream.next() else {
                    return false;
                };
                let key_range = slice_range_in(whole, key_raw.get());
                out.push((
                    offset + key_range.start..offset + key_range.end,
                    TokenKind::JsonKey,
                ));
                cursor = key_range.end;
                let after_key = &whole[cursor..limit];
                let Some(colon_rel) = after_key.find(':') else {
                    return false;
                };
                let colon_pos = cursor + colon_rel;
                out.push((
                    offset + colon_pos..offset + colon_pos + 1,
                    TokenKind::QueryPunct,
                ));
                cursor = colon_pos + 1;
                let mut val_stream = serde_json::Deserializer::from_str(&whole[cursor..limit])
                    .into_iter::<&serde_json::value::RawValue>();
                let Some(Ok(val_raw)) = val_stream.next() else {
                    return false;
                };
                let val_str = val_raw.get();
                let val_range = slice_range_in(whole, val_str);
                if !emit_raw_json_tokens(whole, val_str, offset, out) {
                    return false;
                }
                cursor = val_range.end;
            }
            true
        }
        _ => false,
    }
}

fn emit_punct_between(
    whole: &str,
    span: Range<usize>,
    offset: usize,
    out: &mut Vec<(Range<usize>, TokenKind)>,
) {
    for (rel, ch) in whole[span.clone()].char_indices() {
        if matches!(ch, ':' | ',') {
            let pos = span.start + rel;
            out.push((offset + pos..offset + pos + 1, TokenKind::QueryPunct));
        }
    }
}

pub(super) fn tokenize_markup(body: &str, offset: usize, out: &mut Vec<(Range<usize>, TokenKind)>) {
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        let Some(rel_lt) = body[i..].find('<') else {
            break;
        };
        i += rel_lt;
        if body[i..].starts_with("<!--") {
            let end = body[i + 4..].find("-->").map_or(len, |rel| i + 4 + rel + 3);
            out.push((offset + i..offset + end, TokenKind::JsonString));
            i = end;
            continue;
        }
        if body[i..].len() >= 9 && body[i..i + 9].eq_ignore_ascii_case("<!doctype") {
            let end = body[i..].find('>').map_or(len, |rel| i + rel + 1);
            out.push((offset + i..offset + end, TokenKind::JsonString));
            i = end;
            continue;
        }
        let is_close_tag = body[i..].starts_with("</");
        let bracket_end = if is_close_tag
            || body[i..].starts_with("<!")
            || body[i..].starts_with("<?")
        {
            (i + 2).min(len)
        } else {
            i + 1
        };
        out.push((offset + i..offset + bracket_end, TokenKind::XmlBracket));
        i = bracket_end;

        let tag_start = i;
        while i < len && is_xml_name_byte(bytes[i]) {
            i += 1;
        }
        let tag_name = &body[tag_start..i];
        if i > tag_start {
            out.push((offset + tag_start..offset + i, TokenKind::XmlTag));
        }

        let mut expect_attr_value = false;
        while i < len {
            if bytes[i].is_ascii_whitespace() {
                i += 1;
                continue;
            }
            if body[i..].starts_with("/>") || body[i..].starts_with("?>") {
                out.push((offset + i..offset + i + 2, TokenKind::XmlBracket));
                i += 2;
                break;
            }
            if bytes[i] == b'>' {
                out.push((offset + i..offset + i + 1, TokenKind::XmlBracket));
                i += 1;
                break;
            }
            if bytes[i] == b'<' {
                break;
            }
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                let quote = bytes[i];
                let s_start = i;
                i += 1;
                while i < len && bytes[i] != quote {
                    let step = body[i..].chars().next().map_or(1, char::len_utf8);
                    i += step;
                }
                if i < len {
                    i += 1;
                }
                out.push((offset + s_start..offset + i, TokenKind::XmlString));
                expect_attr_value = false;
                continue;
            }
            if bytes[i] == b'=' {
                out.push((offset + i..offset + i + 1, TokenKind::XmlString));
                i += 1;
                expect_attr_value = true;
                continue;
            }
            if expect_attr_value {
                let v_start = i;
                while i < len
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'>' | b'<')
                {
                    let step = body[i..].chars().next().map_or(1, char::len_utf8);
                    i += step;
                }
                if i > v_start {
                    out.push((offset + v_start..offset + i, TokenKind::XmlString));
                }
                expect_attr_value = false;
                continue;
            }
            if is_xml_name_byte(bytes[i]) {
                let a_start = i;
                while i < len && is_xml_name_byte(bytes[i]) {
                    i += 1;
                }
                out.push((offset + a_start..offset + i, TokenKind::XmlAttrName));
                continue;
            }
            let step = body[i..].chars().next().map_or(1, char::len_utf8);
            i += step;
        }

        if !is_close_tag && tag_name.eq_ignore_ascii_case("style") {
            let css_start = i;
            let css_end = find_close_style(body, css_start).unwrap_or(len);
            tokenize_css(&body[css_start..css_end], offset + css_start, out);
            i = css_end;
        }
    }
}

fn find_close_style(body: &str, from: usize) -> Option<usize> {
    let mut pos = from;
    while let Some(rel) = body[pos..].find("</") {
        let start = pos + rel;
        if body.get(start + 2..start + 7).is_some_and(|s| s.eq_ignore_ascii_case("style")) {
            return Some(start);
        }
        pos = start + 2;
    }
    None
}

fn tokenize_css(css: &str, offset: usize, out: &mut Vec<(Range<usize>, TokenKind)>) {
    let bytes = css.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_rule = false;
    while i < len {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if css[i..].starts_with("/*") {
            let end = css[i + 2..].find("*/").map_or(len, |rel| i + 2 + rel + 2);
            out.push((offset + i..offset + end, TokenKind::JsonString));
            i = end;
            continue;
        }
        if bytes[i] == b'{' {
            in_rule = true;
            i += 1;
            continue;
        }
        if bytes[i] == b'}' {
            in_rule = false;
            i += 1;
            continue;
        }
        if !in_rule {
            let start = i;
            while i < len && !matches!(bytes[i], b'{' | b'}') {
                let step = css[i..].chars().next().map_or(1, char::len_utf8);
                i += step;
            }
            let trimmed_end = css[start..i].trim_end().len() + start;
            if trimmed_end > start {
                out.push((offset + start..offset + trimmed_end, TokenKind::XmlAttrName));
            }
        } else {
            let prop_start = i;
            while i < len && !matches!(bytes[i], b':' | b';' | b'}' | b'\n') {
                let step = css[i..].chars().next().map_or(1, char::len_utf8);
                i += step;
            }
            let prop_end = css[prop_start..i].trim_end().len() + prop_start;
            if prop_end > prop_start {
                out.push((offset + prop_start..offset + prop_end, TokenKind::XmlAttrName));
            }
            if i < len && bytes[i] == b':' {
                i += 1;
                while i < len && !matches!(bytes[i], b';' | b'}') {
                    let step = css[i..].chars().next().map_or(1, char::len_utf8);
                    i += step;
                }
            }
            if i < len && bytes[i] == b';' {
                i += 1;
            }
        }
    }
}

fn is_xml_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.')
}
