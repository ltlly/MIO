use std::ops::Range;

use super::syntax_highlight::TokenKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonContainer {
    Object { expect_key: bool },
    Array,
}

pub(super) fn tokenize_json(body: &str, offset: usize, out: &mut Vec<(Range<usize>, TokenKind)>) {
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= len {
        return;
    }
    if !matches!(
        bytes[i],
        b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'
    ) {
        return;
    }

    let mut stack: Vec<JsonContainer> = Vec::with_capacity(16);
    let mark_value = |stack: &mut Vec<JsonContainer>| {
        if let Some(JsonContainer::Object { expect_key }) = stack.last_mut() {
            *expect_key = false;
        }
    };

    while i < len {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'{' => {
                mark_value(&mut stack);
                stack.push(JsonContainer::Object { expect_key: true });
                i += 1;
            }
            b'[' => {
                mark_value(&mut stack);
                stack.push(JsonContainer::Array);
                i += 1;
            }
            b'}' => {
                if matches!(stack.last(), Some(JsonContainer::Object { .. })) {
                    stack.pop();
                }
                i += 1;
            }
            b']' => {
                if matches!(stack.last(), Some(JsonContainer::Array)) {
                    stack.pop();
                }
                i += 1;
            }
            b':' => {
                out.push((offset + i..offset + i + 1, TokenKind::QueryPunct));
                if let Some(JsonContainer::Object { expect_key }) = stack.last_mut() {
                    *expect_key = false;
                }
                i += 1;
            }
            b',' => {
                out.push((offset + i..offset + i + 1, TokenKind::QueryPunct));
                if let Some(JsonContainer::Object { expect_key }) = stack.last_mut() {
                    *expect_key = true;
                }
                i += 1;
            }
            b'"' => {
                let str_start = i;
                i += 1;
                while i < len {
                    let b = bytes[i];
                    if b == b'\\' {
                        i += 1;
                        if i < len {
                            if bytes[i] < 0x80 {
                                i += 1;
                            } else {
                                i += body[i..].chars().next().map_or(len - i, char::len_utf8);
                            }
                        }
                    } else if b == b'"' {
                        i += 1;
                        break;
                    } else if b < 0x80 {
                        i += 1;
                    } else {
                        i += body[i..].chars().next().map_or(len - i, char::len_utf8);
                    }
                }
                let mut j = i;
                while j < len && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let is_key = matches!(
                    stack.last(),
                    Some(JsonContainer::Object { expect_key: true })
                ) || bytes.get(j) == Some(&b':');
                mark_value(&mut stack);
                out.push((
                    offset + str_start..offset + i,
                    if is_key {
                        TokenKind::JsonKey
                    } else {
                        TokenKind::JsonString
                    },
                ));
            }
            b'-' | b'0'..=b'9' => {
                let num_start = i;
                if bytes[i] == b'-' {
                    i += 1;
                }
                let digit_start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == digit_start {
                    break;
                }
                if i < len && bytes[i] == b'.' {
                    let frac_start = i + 1;
                    if frac_start < len && bytes[frac_start].is_ascii_digit() {
                        i = frac_start + 1;
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                if i < len && matches!(bytes[i], b'e' | b'E') {
                    let mut exp_i = i + 1;
                    if exp_i < len && matches!(bytes[exp_i], b'+' | b'-') {
                        exp_i += 1;
                    }
                    let exp_digits = exp_i;
                    while exp_i < len && bytes[exp_i].is_ascii_digit() {
                        exp_i += 1;
                    }
                    if exp_i > exp_digits {
                        i = exp_i;
                    }
                }
                mark_value(&mut stack);
                out.push((offset + num_start..offset + i, TokenKind::JsonNumber));
            }
            b't' | b'f' | b'n' => {
                let rest = &body[i..];
                let kw_len = if rest.starts_with("true") || rest.starts_with("null") {
                    4
                } else if rest.starts_with("false") {
                    5
                } else {
                    break;
                };
                let kw_end = i + kw_len;
                if kw_end < len
                    && (bytes[kw_end].is_ascii_alphanumeric() || bytes[kw_end] == b'_')
                {
                    break;
                }
                mark_value(&mut stack);
                out.push((offset + i..offset + kw_end, TokenKind::JsonKeyword));
                i = kw_end;
            }
            _ => break,
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
