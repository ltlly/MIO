use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LexMode {
    Code,
    TemplateText,
    Interpolation { brace_depth: usize },
}

enum LexEvent<'a> {
    Ident { text: &'a str, range: Range<usize> },
    Dot,
    StringText(&'a str),
    OtherCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrevCodeToken<'a> {
    None,
    Dot,
    Ident(&'a str),
    StringLit,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ScriptRequirements {
    pub(super) needs_raw: bool,
    pub(super) needs_search_text: bool,
    pub(super) needs_request_response: bool,
}

impl ScriptRequirements {
    pub(super) fn analyze(script: &str) -> Self {
        let mut reqs = Self::default();
        let mut prev = PrevCodeToken::None;
        let mut prev_prev = PrevCodeToken::None;

        scan_script(script, |event| match event {
            LexEvent::Ident { text, .. } => {
                match text {
                    "raw" => reqs.needs_raw = true,
                    "search_text" => {
                        reqs.needs_search_text = true;
                        reqs.needs_raw = true;
                    }
                    "requestResponse" => reqs.needs_request_response = true,
                    "eval" => {
                        reqs.needs_raw = true;
                        reqs.needs_search_text = true;
                        reqs.needs_request_response = true;
                    }
                    "contains" if prev == PrevCodeToken::Dot => {
                        let known_safe_receiver = matches!(
                            prev_prev,
                            PrevCodeToken::StringLit
                                | PrevCodeToken::Ident(
                                    "body"
                                        | "raw"
                                        | "url"
                                        | "path"
                                        | "path_with_query"
                                        | "query"
                                        | "host"
                                        | "method"
                                        | "version"
                                        | "notes"
                                        | "mime_type"
                                        | "payload"
                                        | "highlight_color"
                                        | "raw_header"
                                        | "raw_payload"
                                )
                        );
                        if !known_safe_receiver {
                            reqs.needs_raw = true;
                            reqs.needs_search_text = true;
                        }
                    }
                    _ => {}
                }
                prev_prev = prev;
                prev = PrevCodeToken::Ident(text);
            }
            LexEvent::Dot => {
                prev_prev = prev;
                prev = PrevCodeToken::Dot;
            }
            LexEvent::StringText(text) => {
                if text.contains("raw") {
                    reqs.needs_raw = true;
                }
                if text.contains("search_text") {
                    reqs.needs_search_text = true;
                    reqs.needs_raw = true;
                }
                if text.contains("requestResponse") {
                    reqs.needs_request_response = true;
                }
                prev_prev = prev;
                prev = PrevCodeToken::StringLit;
            }
            LexEvent::OtherCode => {
                prev_prev = prev;
                prev = PrevCodeToken::Other;
            }
        });

        reqs
    }
}

fn is_rhai_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "return"
            | "true"
            | "false"
            | "let"
            | "const"
            | "if"
            | "else"
            | "switch"
            | "do"
            | "while"
            | "until"
            | "loop"
            | "for"
            | "in"
            | "continue"
            | "break"
            | "throw"
            | "try"
            | "catch"
    )
}

fn next_char_len(script: &str, idx: usize) -> usize {
    script[idx..].chars().next().map_or(0, char::len_utf8)
}

fn scan_quoted_string(script: &str, start: usize, quote: u8, stop_on_newline: bool) -> usize {
    let bytes = script.as_bytes();
    let len = bytes.len();
    let mut i = start + 1;
    while i < len {
        match bytes[i] {
            b'\\' => {
                i += 1;
                if i < len {
                    i += next_char_len(script, i);
                }
            }
            b'\n' if stop_on_newline => break,
            b if b == quote => {
                i += 1;
                break;
            }
            _ => {
                i += next_char_len(script, i);
            }
        }
    }
    i
}

fn scan_raw_string(script: &str, start: usize, hash_count: usize) -> usize {
    let bytes = script.as_bytes();
    let len = bytes.len();
    let mut i = start + hash_count + 1;
    while i < len {
        if bytes[i] == b'"'
            && i + 1 + hash_count <= len
            && bytes[i + 1..i + 1 + hash_count]
                .iter()
                .all(|&b| b == b'#')
        {
            return i + 1 + hash_count;
        }
        i += next_char_len(script, i);
    }
    len
}

fn scan_block_comment(script: &str, start: usize) -> usize {
    let bytes = script.as_bytes();
    let len = bytes.len();
    let mut i = start + 2;
    let mut depth = 1usize;
    while i < len && depth > 0 {
        if bytes[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if bytes[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
        } else {
            i += next_char_len(script, i);
        }
    }
    i
}

fn scan_script<'a>(script: &'a str, mut on_event: impl FnMut(LexEvent<'a>)) {
    let bytes = script.as_bytes();
    let len = bytes.len();
    let mut mode_stack = vec![LexMode::Code];
    let mut i = 0usize;

    while i < len {
        let mode = *mode_stack.last().unwrap_or(&LexMode::Code);
        match mode {
            LexMode::TemplateText => {
                let text_start = i;
                let mut closed_or_interpolated = false;
                while i < len {
                    match bytes[i] {
                        b'\\' => {
                            i += 1;
                            if i < len {
                                i += next_char_len(script, i);
                            }
                        }
                        b'`' => {
                            if i > text_start {
                                on_event(LexEvent::StringText(&script[text_start..i]));
                            }
                            mode_stack.pop();
                            i += 1;
                            closed_or_interpolated = true;
                            break;
                        }
                        b'$' if bytes.get(i + 1) == Some(&b'{') => {
                            if i > text_start {
                                on_event(LexEvent::StringText(&script[text_start..i]));
                            }
                            mode_stack.push(LexMode::Interpolation { brace_depth: 1 });
                            i += 2;
                            closed_or_interpolated = true;
                            break;
                        }
                        _ => {
                            i += next_char_len(script, i);
                        }
                    }
                }
                if !closed_or_interpolated && i > text_start {
                    on_event(LexEvent::StringText(&script[text_start..i]));
                }
            }
            LexMode::Code | LexMode::Interpolation { .. } => {
                let b = bytes[i];
                if b.is_ascii_whitespace() {
                    i += 1;
                    continue;
                }
                if bytes[i..].starts_with(b"//") {
                    i += 2;
                    while i < len && bytes[i] != b'\n' {
                        i += next_char_len(script, i);
                    }
                    continue;
                }
                if bytes[i..].starts_with(b"/*") {
                    i = scan_block_comment(script, i);
                    continue;
                }
                if b == b'"' {
                    let end = scan_quoted_string(script, i, b'"', false);
                    on_event(LexEvent::StringText(&script[i..end]));
                    i = end;
                    continue;
                }
                if b == b'\'' {
                    let end = scan_quoted_string(script, i, b'\'', true);
                    on_event(LexEvent::StringText(&script[i..end]));
                    i = end;
                    continue;
                }
                if b == b'#' {
                    if bytes.get(i + 1) == Some(&b'{') {
                        if let Some(LexMode::Interpolation { brace_depth }) = mode_stack.last_mut() {
                            *brace_depth += 1;
                        }
                        on_event(LexEvent::OtherCode);
                        i += 2;
                        continue;
                    }
                    let hash_count = bytes[i..].iter().take_while(|&&ch| ch == b'#').count();
                    if bytes.get(i + hash_count) == Some(&b'"') {
                        let end = scan_raw_string(script, i, hash_count);
                        on_event(LexEvent::StringText(&script[i..end]));
                        i = end;
                        continue;
                    }
                    on_event(LexEvent::OtherCode);
                    i += 1;
                    continue;
                }
                if b == b'`' {
                    mode_stack.push(LexMode::TemplateText);
                    on_event(LexEvent::OtherCode);
                    i += 1;
                    continue;
                }
                if b == b'{' {
                    if let Some(LexMode::Interpolation { brace_depth }) = mode_stack.last_mut() {
                        *brace_depth += 1;
                    }
                    on_event(LexEvent::OtherCode);
                    i += 1;
                    continue;
                }
                if b == b'}' {
                    if let Some(LexMode::Interpolation { brace_depth }) = mode_stack.last_mut() {
                        *brace_depth = brace_depth.saturating_sub(1);
                        if *brace_depth == 0 {
                            mode_stack.pop();
                        }
                    }
                    on_event(LexEvent::OtherCode);
                    i += 1;
                    continue;
                }
                if b == b'.' {
                    on_event(LexEvent::Dot);
                    i += 1;
                    continue;
                }
                if b.is_ascii_alphabetic() || b == b'_' {
                    let start = i;
                    i += 1;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    on_event(LexEvent::Ident {
                        text: &script[start..i],
                        range: start..i,
                    });
                    continue;
                }

                let ch = script[i..].chars().next().expect("i < len guarantees a char");
                if !ch.is_ascii() && ch.is_alphabetic() {
                    i += ch.len_utf8();
                    while i < len {
                        let next = script[i..].chars().next().expect("i < len guarantees a char");
                        if next.is_alphanumeric() || next == '_' {
                            i += next.len_utf8();
                        } else {
                            break;
                        }
                    }
                    on_event(LexEvent::OtherCode);
                    continue;
                }

                on_event(LexEvent::OtherCode);
                i += ch.len_utf8();
            }
        }
    }
}

/// Returns byte ranges of Rhai keywords in `script`, ignoring comments, strings, and template text.
pub fn script_keyword_ranges(script: &str) -> Vec<Range<usize>> {
    if script.is_empty() || script.len() > 64 * 1024 {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    scan_script(script, |event| {
        if let LexEvent::Ident { text, range } = event
            && is_rhai_keyword(text)
            && ranges.last().is_none_or(|prev: &Range<usize>| prev.end <= range.start)
        {
            ranges.push(range);
        }
    });
    ranges
}
