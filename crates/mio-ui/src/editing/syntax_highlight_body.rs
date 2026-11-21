use std::ops::Range;

use super::syntax_highlight::TokenKind;

const MAX_JSON_STACK_DEPTH: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JsonContainer {
    Object { expect_key: bool },
    Array,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JsonNumPart {
    Sign,
    IntDigits,
    DotWaitDigit,
    FracDigits,
    ExpMarker,
    ExpSignWaitDigit,
    ExpDigits,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum JsonLexMode {
    #[default]
    Normal,
    InString {
        escaped: bool,
        is_key: bool,
    },
    InNumber {
        part: JsonNumPart,
    },
    InKeyword {
        expected: &'static [u8],
        pos: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct JsonLexState {
    pub(super) stack: Vec<JsonContainer>,
    pub(super) overflow_depth: usize,
    pub(super) mode: JsonLexMode,
    pub(super) started: bool,
    pub(super) active: bool,
}

impl JsonLexState {
    pub(super) fn new() -> Self {
        Self {
            stack: Vec::with_capacity(16),
            overflow_depth: 0,
            mode: JsonLexMode::Normal,
            started: false,
            active: true,
        }
    }

    fn mark_value(&mut self) {
        if self.overflow_depth == 0
            && let Some(JsonContainer::Object { expect_key }) = self.stack.last_mut()
        {
            *expect_key = false;
        }
    }

    fn in_object_key_position(&self) -> bool {
        self.overflow_depth == 0
            && matches!(self.stack.last(), Some(JsonContainer::Object { expect_key: true }))
    }
}

pub(super) fn tokenize_json_stateful(
    body: &str,
    offset: usize,
    out: &mut Vec<(Range<usize>, TokenKind)>,
) -> JsonLexState {
    let mut state = JsonLexState::new();
    lex_json_chunk(body, offset, &mut state, Some(out));
    state
}

pub(super) fn advance_json_state(chunk: &str, state: &mut JsonLexState) {
    lex_json_chunk(chunk, 0, state, None);
}

pub(super) fn tokenize_json_from_state(
    body: &str,
    offset: usize,
    state: &mut JsonLexState,
    out: &mut Vec<(Range<usize>, TokenKind)>,
) {
    lex_json_chunk(body, offset, state, Some(out));
}

fn finish_number_part(
    bytes: &[u8],
    i: &mut usize,
    mut part: JsonNumPart,
) -> Option<JsonNumPart> {
    let len = bytes.len();
    loop {
        match part {
            JsonNumPart::Sign => {
                if *i >= len {
                    return Some(JsonNumPart::Sign);
                }
                if !bytes[*i].is_ascii_digit() {
                    return None;
                }
                while *i < len && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                part = JsonNumPart::IntDigits;
            }
            JsonNumPart::IntDigits => {
                while *i < len && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                if *i >= len {
                    return Some(JsonNumPart::IntDigits);
                }
                if bytes[*i] == b'.' {
                    *i += 1;
                    part = JsonNumPart::DotWaitDigit;
                } else if matches!(bytes[*i], b'e' | b'E') {
                    *i += 1;
                    part = JsonNumPart::ExpMarker;
                } else {
                    return None;
                }
            }
            JsonNumPart::DotWaitDigit => {
                if *i >= len {
                    return Some(JsonNumPart::DotWaitDigit);
                }
                if !bytes[*i].is_ascii_digit() {
                    // Undo dot only if it was in this chunk? When at i > 0 with non-digit, keep prefix and stop number cleanly.
                    return None;
                }
                while *i < len && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                part = JsonNumPart::FracDigits;
            }
            JsonNumPart::FracDigits => {
                while *i < len && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                if *i >= len {
                    return Some(JsonNumPart::FracDigits);
                }
                if matches!(bytes[*i], b'e' | b'E') {
                    *i += 1;
                    part = JsonNumPart::ExpMarker;
                } else {
                    return None;
                }
            }
            JsonNumPart::ExpMarker => {
                if *i >= len {
                    return Some(JsonNumPart::ExpMarker);
                }
                if matches!(bytes[*i], b'+' | b'-') {
                    *i += 1;
                    part = JsonNumPart::ExpSignWaitDigit;
                } else if bytes[*i].is_ascii_digit() {
                    while *i < len && bytes[*i].is_ascii_digit() {
                        *i += 1;
                    }
                    part = JsonNumPart::ExpDigits;
                } else {
                    return None;
                }
            }
            JsonNumPart::ExpSignWaitDigit => {
                if *i >= len {
                    return Some(JsonNumPart::ExpSignWaitDigit);
                }
                if !bytes[*i].is_ascii_digit() {
                    return None;
                }
                while *i < len && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                part = JsonNumPart::ExpDigits;
            }
            JsonNumPart::ExpDigits => {
                while *i < len && bytes[*i].is_ascii_digit() {
                    *i += 1;
                }
                if *i >= len {
                    return Some(JsonNumPart::ExpDigits);
                }
                return None;
            }
        }
    }
}

fn scan_string_until_quote(
    body: &str,
    bytes: &[u8],
    i: &mut usize,
    escaped: &mut bool,
) -> bool {
    let len = bytes.len();
    if *escaped && *i < len {
        let step = if bytes[*i] < 0x80 {
            1
        } else {
            body[*i..].chars().next().map_or(len - *i, char::len_utf8)
        };
        *i += step;
        *escaped = false;
    }
    while *i < len {
        let b = bytes[*i];
        if b == b'\\' {
            *i += 1;
            if *i < len {
                let step = if bytes[*i] < 0x80 {
                    1
                } else {
                    body[*i..].chars().next().map_or(len - *i, char::len_utf8)
                };
                *i += step;
            } else {
                *escaped = true;
                return false;
            }
        } else if b == b'"' {
            *i += 1;
            return true;
        } else if b < 0x80 {
            *i += 1;
        } else {
            *i += body[*i..].chars().next().map_or(len - *i, char::len_utf8);
        }
    }
    false
}

fn lex_json_chunk(
    body: &str,
    offset: usize,
    state: &mut JsonLexState,
    mut out: Option<&mut Vec<(Range<usize>, TokenKind)>>,
) {
    if !state.active {
        return;
    }
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    if !state.started {
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
            state.active = false;
            return;
        }
        state.started = true;
    }

    match state.mode {
        JsonLexMode::Normal => {}
        JsonLexMode::InString {
            mut escaped,
            is_key,
        } => {
            let start = i;
            let closed = scan_string_until_quote(body, bytes, &mut i, &mut escaped);
            if !closed {
                if i > start
                    && let Some(ref mut tokens) = out
                {
                    tokens.push((
                        offset + start..offset + i,
                        if is_key {
                            TokenKind::JsonKey
                        } else {
                            TokenKind::JsonString
                        },
                    ));
                }
                state.mode = JsonLexMode::InString { escaped, is_key };
                return;
            }
            let mut j = i;
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let token_is_key = is_key || bytes.get(j) == Some(&b':');
            state.mark_value();
            if i > start
                && let Some(ref mut tokens) = out
            {
                tokens.push((
                    offset + start..offset + i,
                    if token_is_key {
                        TokenKind::JsonKey
                    } else {
                        TokenKind::JsonString
                    },
                ));
            }
            state.mode = JsonLexMode::Normal;
        }
        JsonLexMode::InNumber { part } => {
            let start = i;
            let next_part = finish_number_part(bytes, &mut i, part);
            if i > start
                && let Some(ref mut tokens) = out
            {
                tokens.push((offset + start..offset + i, TokenKind::JsonNumber));
            }
            if let Some(cont_part) = next_part {
                state.mode = JsonLexMode::InNumber { part: cont_part };
                return;
            }
            state.mark_value();
            state.mode = JsonLexMode::Normal;
        }
        JsonLexMode::InKeyword { expected, mut pos } => {
            let start = i;
            while i < len && (pos as usize) < expected.len() && bytes[i] == expected[pos as usize]
            {
                i += 1;
                pos += 1;
            }
            if i >= len && (pos as usize) < expected.len() {
                if i > start
                    && let Some(ref mut tokens) = out
                {
                    tokens.push((offset + start..offset + i, TokenKind::JsonKeyword));
                }
                state.mode = JsonLexMode::InKeyword { expected, pos };
                return;
            }
            if (pos as usize) != expected.len()
                || (i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_'))
            {
                state.active = false;
                state.mode = JsonLexMode::Normal;
                return;
            }
            if i > start
                && let Some(ref mut tokens) = out
            {
                tokens.push((offset + start..offset + i, TokenKind::JsonKeyword));
            }
            state.mark_value();
            state.mode = JsonLexMode::Normal;
        }
    }

    while i < len {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'{' => {
                state.mark_value();
                if state.overflow_depth == 0 && state.stack.len() < MAX_JSON_STACK_DEPTH {
                    state.stack.push(JsonContainer::Object { expect_key: true });
                } else {
                    state.overflow_depth += 1;
                }
                i += 1;
            }
            b'[' => {
                state.mark_value();
                if state.overflow_depth == 0 && state.stack.len() < MAX_JSON_STACK_DEPTH {
                    state.stack.push(JsonContainer::Array);
                } else {
                    state.overflow_depth += 1;
                }
                i += 1;
            }
            b'}' => {
                if state.overflow_depth > 0 {
                    state.overflow_depth -= 1;
                } else if matches!(state.stack.last(), Some(JsonContainer::Object { .. })) {
                    state.stack.pop();
                }
                i += 1;
            }
            b']' => {
                if state.overflow_depth > 0 {
                    state.overflow_depth -= 1;
                } else if matches!(state.stack.last(), Some(JsonContainer::Array)) {
                    state.stack.pop();
                }
                i += 1;
            }
            b':' => {
                if let Some(ref mut tokens) = out {
                    tokens.push((offset + i..offset + i + 1, TokenKind::QueryPunct));
                }
                if state.overflow_depth == 0
                    && let Some(JsonContainer::Object { expect_key }) = state.stack.last_mut()
                {
                    *expect_key = false;
                }
                i += 1;
            }
            b',' => {
                if let Some(ref mut tokens) = out {
                    tokens.push((offset + i..offset + i + 1, TokenKind::QueryPunct));
                }
                if state.overflow_depth == 0
                    && let Some(JsonContainer::Object { expect_key }) = state.stack.last_mut()
                {
                    *expect_key = true;
                }
                i += 1;
            }
            b'"' => {
                let str_start = i;
                i += 1;
                let expect_key = state.in_object_key_position();
                let mut escaped = false;
                let closed = scan_string_until_quote(body, bytes, &mut i, &mut escaped);
                if !closed {
                    if let Some(ref mut tokens) = out {
                        tokens.push((
                            offset + str_start..offset + i,
                            if expect_key {
                                TokenKind::JsonKey
                            } else {
                                TokenKind::JsonString
                            },
                        ));
                    }
                    state.mode = JsonLexMode::InString {
                        escaped,
                        is_key: expect_key,
                    };
                    return;
                }
                let mut j = i;
                while j < len && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let is_key = expect_key || bytes.get(j) == Some(&b':');
                state.mark_value();
                if let Some(ref mut tokens) = out {
                    tokens.push((
                        offset + str_start..offset + i,
                        if is_key {
                            TokenKind::JsonKey
                        } else {
                            TokenKind::JsonString
                        },
                    ));
                }
            }
            b'-' | b'0'..=b'9' => {
                let num_start = i;
                let initial_part = if bytes[i] == b'-' {
                    i += 1;
                    JsonNumPart::Sign
                } else {
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    JsonNumPart::IntDigits
                };
                if let Some(cont_part) = finish_number_part(bytes, &mut i, initial_part) {
                    if let Some(ref mut tokens) = out {
                        tokens.push((offset + num_start..offset + i, TokenKind::JsonNumber));
                    }
                    state.mode = JsonLexMode::InNumber { part: cont_part };
                    return;
                }
                if i == num_start || (i == num_start + 1 && bytes[num_start] == b'-') {
                    state.active = false;
                    break;
                }
                state.mark_value();
                if let Some(ref mut tokens) = out {
                    tokens.push((offset + num_start..offset + i, TokenKind::JsonNumber));
                }
            }
            b't' | b'f' | b'n' => {
                let expected: &'static [u8] = match bytes[i] {
                    b't' => b"true",
                    b'f' => b"false",
                    _ => b"null",
                };
                let rest = &bytes[i..];
                let mut matched = 0usize;
                while matched < rest.len()
                    && matched < expected.len()
                    && rest[matched] == expected[matched]
                {
                    matched += 1;
                }
                if matched == rest.len() && matched < expected.len() {
                    if let Some(ref mut tokens) = out {
                        tokens.push((offset + i..offset + len, TokenKind::JsonKeyword));
                    }
                    state.mode = JsonLexMode::InKeyword {
                        expected,
                        pos: matched as u8,
                    };
                    return;
                }
                if matched != expected.len() {
                    state.active = false;
                    break;
                }
                let kw_end = i + expected.len();
                if kw_end < len
                    && (bytes[kw_end].is_ascii_alphanumeric() || bytes[kw_end] == b'_')
                {
                    state.active = false;
                    break;
                }
                state.mark_value();
                if let Some(ref mut tokens) = out {
                    tokens.push((offset + i..offset + kw_end, TokenKind::JsonKeyword));
                }
                i = kw_end;
            }
            _ => {
                state.active = false;
                break;
            }
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
        if bytes[i..].len() >= 9 && bytes[i..i + 9].eq_ignore_ascii_case(b"<!doctype") {
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
    let bytes = body.as_bytes();
    let mut pos = from;
    while let Some(rel) = body[pos..].find("</") {
        let start = pos + rel;
        if bytes
            .get(start + 2..start + 7)
            .is_some_and(|s| s.eq_ignore_ascii_case(b"style"))
        {
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
