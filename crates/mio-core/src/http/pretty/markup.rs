use super::{MAX_PRETTY_BYTES, PrettyLineBuilder, SourceLineMap};

const MAX_MARKUP_DEPTH: usize = 256;
const MAX_MARKUP_NODES: u32 = 65_536;
const MAX_CSS_PRETTY_BYTES: usize = MAX_PRETTY_BYTES * 2;

#[derive(Debug)]
enum MarkupToken<'a> {
    Standalone {
        raw: &'a str,
        offset: usize,
    },
    Open {
        raw: &'a str,
        offset: usize,
    },
    Close {
        raw: &'a str,
        offset: usize,
    },
    ImplicitClose,
    RawBlock {
        open: &'a str,
        open_offset: usize,
        inner: &'a str,
        inner_offset: usize,
        close: &'a str,
        close_offset: usize,
    },
    Text {
        raw: &'a str,
        offset: usize,
    },
}

pub(super) fn looks_like_html_document(trimmed: &str) -> bool {
    let trimmed = trimmed.trim_start_matches('\u{feff}').trim_start();
    trimmed
        .as_bytes()
        .get(..14)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"<!doctype html"))
        || trimmed
            .as_bytes()
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"<html"))
}

pub(super) fn looks_like_xml_document(trimmed: &str) -> bool {
    let trimmed = trimmed.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("<?xml") {
        return true;
    }
    if !trimmed.starts_with('<') || looks_like_html_document(trimmed) {
        return false;
    }
    trimmed
        .as_bytes()
        .get(1)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && trimmed.ends_with('>')
}

fn is_html_void_tag(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_html_raw_text_tag(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "script" | "style" | "textarea" | "title"
    )
}

fn is_html_optional_close_tag(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "html" | "head" | "body" | "p" | "li" | "dt" | "dd" | "option"
    )
}

fn html_tag_implicitly_closes(open_on_stack: &str, incoming_lower: &str) -> bool {
    match open_on_stack {
        "p" => matches!(incoming_lower, "p"),
        "li" => matches!(incoming_lower, "li"),
        "dt" | "dd" => matches!(incoming_lower, "dt" | "dd"),
        "option" => matches!(incoming_lower, "option" | "optgroup"),
        "head" => matches!(incoming_lower, "body"),
        _ => false,
    }
}

fn valid_tag_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

fn scan_tag_end(rest: &str, kind_label: &str, html_mode: bool) -> Result<(usize, bool), String> {
    let bytes = rest.as_bytes();
    let mut idx = 1;
    let mut quote: Option<u8> = None;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
            idx += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'<' => {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：起始标签未闭合；原始字节未改变。"
                ));
            }
            b'>' => {
                let before = rest[1..idx].trim_end();
                let self_closing = if let Some(without_slash) = before.strip_suffix('/') {
                    if html_mode {
                        let prev = without_slash.as_bytes().last().copied();
                        let in_unquoted_attr = prev.is_some_and(|b| {
                            !b.is_ascii_whitespace() && b != b'"' && b != b'\''
                        }) && without_slash.contains('=');
                        !in_unquoted_attr
                    } else {
                        true
                    }
                } else {
                    false
                };
                return Ok((idx + 1, self_closing));
            }
            _ => {}
        }
        idx += 1;
    }
    if quote.is_some() {
        Err(format!(
            "{kind_label} 解析失败，无法切换到 Pretty 视图：标签属性引号未闭合；原始字节未改变。"
        ))
    } else {
        Err(format!(
            "{kind_label} 解析失败，无法切换到 Pretty 视图：标签未闭合；原始字节未改变。"
        ))
    }
}

fn scan_doctype_end(rest: &str, kind_label: &str) -> Result<usize, String> {
    let bytes = rest.as_bytes();
    let mut idx = 2;
    let mut quote: Option<u8> = None;
    let mut bracket_depth = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
        } else {
            match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'>' if bracket_depth == 0 => return Ok(idx + 1),
                _ => {}
            }
        }
        idx += 1;
    }
    Err(format!(
        "{kind_label} 解析失败，无法切换到 Pretty 视图：DOCTYPE 声明未闭合；原始字节未改变。"
    ))
}

fn find_raw_close_tag(rest: &str, tag_lower: &str) -> Option<(usize, usize)> {
    let bytes = rest.as_bytes();
    let mut pos = 0;
    while let Some(rel) = rest[pos..].find("</") {
        let start = pos + rel;
        let name_start = start + 2;
        let name_end = name_start + tag_lower.len();
        if let Some(candidate) = rest.get(name_start..name_end)
            && candidate.eq_ignore_ascii_case(tag_lower)
        {
            let mut cursor = name_end;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'>') {
                return Some((start, cursor + 1));
            }
        }
        pos = start + 2;
    }
    None
}

fn format_css_block_lines(
    css: &str,
    base_depth: usize,
    css_base_offset: usize,
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) {
    let mut depth = base_depth.min(MAX_MARKUP_DEPTH);
    let mut current = String::new();
    let mut current_start_offset: Option<usize> = None;
    let mut quote: Option<char> = None;
    let mut in_comment = false;
    let mut chars = css.char_indices().peekable();
    let push_line = |buf: &mut String,
                     start_offset: &mut Option<usize>,
                     d: usize,
                     out: &mut PrettyLineBuilder| -> bool {
        let trimmed = buf.trim();
        let mut exceeded = out.text.len() >= MAX_CSS_PRETTY_BYTES;
        if !trimmed.is_empty() && !exceeded {
            let line_no = start_offset.map(|off| line_map.line_at(css_base_offset + off));
            let indent = "  ".repeat(d.min(MAX_MARKUP_DEPTH));
            let normalized = trimmed.replace("\r\n", "\n");
            for (idx, part) in normalized.split('\n').enumerate() {
                let clean = part.trim_end_matches('\r');
                out.push_line(&format!("{indent}{clean}"), line_no.map(|base| base + idx));
                if out.text.len() >= MAX_CSS_PRETTY_BYTES {
                    exceeded = true;
                    break;
                }
            }
        }
        buf.clear();
        *start_offset = None;
        exceeded
    };
    while let Some((byte_idx, ch)) = chars.next() {
        if out.text.len() >= MAX_CSS_PRETTY_BYTES {
            current.clear();
            return;
        }
        if in_comment {
            if current_start_offset.is_none() {
                current_start_offset = Some(byte_idx);
            }
            current.push(ch);
            if ch == '*' && chars.peek().is_some_and(|&(_, next)| next == '/') {
                current.push(chars.next().unwrap().1);
                in_comment = false;
            }
            continue;
        }
        if let Some(q) = quote {
            if current_start_offset.is_none() {
                current_start_offset = Some(byte_idx);
            }
            current.push(ch);
            if ch == '\\' {
                if let Some((_, next)) = chars.next() {
                    current.push(next);
                }
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '/' if chars.peek().is_some_and(|&(_, next)| next == '*') => {
                if current_start_offset.is_none() {
                    current_start_offset = Some(byte_idx);
                }
                current.push(ch);
                current.push(chars.next().unwrap().1);
                in_comment = true;
            }
            '"' | '\'' => {
                if current_start_offset.is_none() {
                    current_start_offset = Some(byte_idx);
                }
                quote = Some(ch);
                current.push(ch);
            }
            '{' => {
                if current_start_offset.is_none() {
                    current_start_offset = Some(byte_idx);
                }
                current.push('{');
                if push_line(&mut current, &mut current_start_offset, depth, out) {
                    return;
                }
                depth = depth.saturating_add(1).min(MAX_MARKUP_DEPTH);
            }
            ';' => {
                if current_start_offset.is_none() {
                    current_start_offset = Some(byte_idx);
                }
                current.push(';');
                if push_line(&mut current, &mut current_start_offset, depth, out) {
                    return;
                }
            }
            '}' => {
                if push_line(&mut current, &mut current_start_offset, depth, out) {
                    return;
                }
                depth = depth.saturating_sub(1);
                let indent = "  ".repeat(depth.min(MAX_MARKUP_DEPTH));
                out.push_line(
                    &format!("{indent}}}"),
                    Some(line_map.line_at(css_base_offset + byte_idx)),
                );
            }
            '\r' | '\n' => {
                if !current.ends_with(' ') && !current.is_empty() {
                    current.push(' ');
                }
            }
            _ => {
                if current_start_offset.is_none() && !ch.is_whitespace() {
                    current_start_offset = Some(byte_idx);
                }
                current.push(ch);
            }
        }
    }
    let _ = push_line(&mut current, &mut current_start_offset, depth, out);
}

fn push_prefixed_source_lines(
    indent: &str,
    raw_slice: &str,
    slice_offset: usize,
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) {
    let mut part_offset = 0usize;
    for (idx, part) in raw_slice.split('\n').enumerate() {
        let orig_line = line_map.line_at(slice_offset + part_offset);
        if idx == 0 {
            out.push_line(&format!("{indent}{part}"), Some(orig_line));
        } else {
            out.push_line(part, Some(orig_line));
        }
        part_offset += part.len() + 1;
    }
}

fn render_tokens(
    tokens: &[MarkupToken<'_>],
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) {
    let mut depth = 0usize;
    let mut idx = 0;
    while idx < tokens.len() {
        let indent = "  ".repeat(depth);
        match &tokens[idx] {
            MarkupToken::Standalone { raw, offset } => {
                let trim_lead = raw.len() - raw.trim_start().len();
                let trimmed = raw.trim();
                push_prefixed_source_lines(
                    &indent,
                    trimmed,
                    offset + trim_lead,
                    line_map,
                    out,
                );
                idx += 1;
            }
            MarkupToken::Open { raw, offset } => {
                if let (
                    Some(MarkupToken::Text { raw: text, .. }),
                    Some(MarkupToken::Close { raw: close_raw, .. }),
                ) = (tokens.get(idx + 1), tokens.get(idx + 2))
                {
                    let trimmed = text.trim();
                    if !trimmed.contains('\n') && !trimmed.contains('\r') {
                        out.push_multiline_from_base(
                            &format!("{indent}{raw}{trimmed}{close_raw}"),
                            line_map.line_at(*offset),
                        );
                        idx += 3;
                        continue;
                    }
                }
                if let Some(MarkupToken::Close { raw: close_raw, .. }) = tokens.get(idx + 1) {
                    out.push_multiline_from_base(
                        &format!("{indent}{raw}{close_raw}"),
                        line_map.line_at(*offset),
                    );
                    idx += 2;
                    continue;
                }
                push_prefixed_source_lines(&indent, raw, *offset, line_map, out);
                depth += 1;
                idx += 1;
            }
            MarkupToken::Close { raw, offset } => {
                depth = depth.saturating_sub(1);
                let close_indent = "  ".repeat(depth);
                push_prefixed_source_lines(&close_indent, raw, *offset, line_map, out);
                idx += 1;
            }
            MarkupToken::ImplicitClose => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            MarkupToken::RawBlock {
                open,
                open_offset,
                inner,
                inner_offset,
                close,
                close_offset,
            } => {
                let trimmed = inner.trim();
                let tag_after_lt = open.get(1..).map(|s| s.to_ascii_lowercase());
                let is_style = tag_after_lt.as_deref().is_some_and(|s| s.starts_with("style"));
                let is_title = tag_after_lt.as_deref().is_some_and(|s| s.starts_with("title"));
                if trimmed.is_empty() {
                    out.push_multiline_from_base(
                        &format!("{indent}{open}{close}"),
                        line_map.line_at(*open_offset),
                    );
                } else if is_style && trimmed.contains('{') && trimmed.contains('}') {
                    push_prefixed_source_lines(&indent, open, *open_offset, line_map, out);
                    let inner_lead = inner.len() - inner.trim_start().len();
                    format_css_block_lines(
                        trimmed,
                        depth + 1,
                        inner_offset + inner_lead,
                        line_map,
                        out,
                    );
                    push_prefixed_source_lines(&indent, close, *close_offset, line_map, out);
                } else if is_title {
                    push_prefixed_source_lines(&indent, open, *open_offset, line_map, out);
                    let child_indent = "  ".repeat(depth + 1);
                    let inner_lead = inner.len() - inner.trim_start().len();
                    let mut rel_offset = 0usize;
                    for raw_line in trimmed.split('\n') {
                        let t = raw_line.strip_suffix('\r').unwrap_or(raw_line).trim();
                        if !t.is_empty() {
                            let line_no = line_map.line_at(inner_offset + inner_lead + rel_offset);
                            out.push_line(&format!("{child_indent}{t}"), Some(line_no));
                        }
                        rel_offset += raw_line.len() + 1;
                    }
                    push_prefixed_source_lines(&indent, close, *close_offset, line_map, out);
                } else if !trimmed.contains('\n') && !trimmed.contains('\r') {
                    out.push_multiline_from_base(
                        &format!("{indent}{open}{trimmed}{close}"),
                        line_map.line_at(*open_offset),
                    );
                } else {
                    push_prefixed_source_lines(&indent, open, *open_offset, line_map, out);
                    let normalized = inner.replace("\r\n", "\n");
                    let leading_trimmed = normalized.trim_start_matches('\n');
                    let skipped_newlines = normalized.len() - leading_trimmed.len();
                    let block = leading_trimmed.trim_end_matches('\n');
                    let mut raw_lines = inner.split('\n').skip(skipped_newlines);
                    let inner_rel_start: usize = inner
                        .split('\n')
                        .take(skipped_newlines)
                        .map(|s| s.len() + 1)
                        .sum();
                    let mut inner_rel = inner_rel_start;
                    for block_line in block.split('\n') {
                        let orig_line = line_map.line_at(*inner_offset + inner_rel);
                        out.push_line(block_line, Some(orig_line));
                        if let Some(raw_s) = raw_lines.next() {
                            inner_rel += raw_s.len() + 1;
                        }
                    }
                    push_prefixed_source_lines(&indent, close, *close_offset, line_map, out);
                }
                idx += 1;
            }
            MarkupToken::Text { raw, offset } => {
                let mut rel_offset = 0usize;
                for line in raw.split('\n') {
                    let trimmed = line.strip_suffix('\r').unwrap_or(line).trim();
                    if !trimmed.is_empty() {
                        out.push_line(
                            &format!("{indent}{trimmed}"),
                            Some(line_map.line_at(*offset + rel_offset)),
                        );
                    }
                    rel_offset += line.len() + 1;
                }
                idx += 1;
            }
        }
    }
}

fn tokenize_markup<'a>(
    input: &'a str,
    base_offset: usize,
    html_mode: bool,
) -> Result<Vec<MarkupToken<'a>>, String> {
    let kind_label = if html_mode { "HTML" } else { "XML" };
    let mut rest = input;
    let mut cursor = base_offset;
    let mut tokens = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut saw_element_or_decl = false;
    let mut node_count = 0u32;

    while !rest.is_empty() {
        node_count += 1;
        if node_count > MAX_MARKUP_NODES {
            return Err(format!(
                "{kind_label} 节点数量超过 {MAX_MARKUP_NODES} 美化限额；原始字节未改变。"
            ));
        }
        if rest.starts_with("<!--") {
            let Some(end) = rest[4..].find("-->").map(|pos| pos + 7) else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：注释未闭合；原始字节未改变。"
                ));
            };
            tokens.push(MarkupToken::Standalone {
                raw: &rest[..end],
                offset: cursor,
            });
            saw_element_or_decl = true;
            cursor += end;
            rest = &rest[end..];
        } else if rest.starts_with("<![CDATA[") {
            let Some(end) = rest[9..].find("]]>").map(|pos| pos + 12) else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：CDATA 未闭合；原始字节未改变。"
                ));
            };
            tokens.push(MarkupToken::Standalone {
                raw: &rest[..end],
                offset: cursor,
            });
            saw_element_or_decl = true;
            cursor += end;
            rest = &rest[end..];
        } else if rest.starts_with("<?") {
            let Some(end) = rest[2..].find("?>").map(|pos| pos + 4) else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：处理指令未闭合；原始字节未改变。"
                ));
            };
            tokens.push(MarkupToken::Standalone {
                raw: &rest[..end],
                offset: cursor,
            });
            saw_element_or_decl = true;
            cursor += end;
            rest = &rest[end..];
        } else if rest
            .as_bytes()
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"<!doctype"))
        {
            let end = scan_doctype_end(rest, kind_label)?;
            tokens.push(MarkupToken::Standalone {
                raw: &rest[..end],
                offset: cursor,
            });
            saw_element_or_decl = true;
            cursor += end;
            rest = &rest[end..];
        } else if rest.starts_with("</") {
            let Some(gt) = rest[2..].find('>').map(|pos| pos + 2) else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：结束标签未闭合；原始字节未改变。"
                ));
            };
            let name = rest[2..gt].trim();
            if !valid_tag_name(name) {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：无效的结束标签 </{name}>；原始字节未改变。"
                ));
            }
            let Some(opened) = stack.pop() else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：多余的结束标签 </{name}>；原始字节未改变。"
                ));
            };
            let matched = if html_mode {
                opened.eq_ignore_ascii_case(name)
            } else {
                opened == name
            };
            if !matched {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：结束标签 </{name}> 与未闭合标签 <{opened}> 不匹配；原始字节未改变。"
                ));
            }
            let end = gt + 1;
            tokens.push(MarkupToken::Close {
                raw: &rest[..end],
                offset: cursor,
            });
            cursor += end;
            rest = &rest[end..];
        } else if rest.starts_with('<') {
            let (end, self_closing) = scan_tag_end(rest, kind_label, html_mode)?;
            let inside = rest[1..end - 1].trim();
            let inside = inside.strip_suffix('/').unwrap_or(inside).trim();
            let name = inside.split_ascii_whitespace().next().unwrap_or("");
            if !valid_tag_name(name) {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：无效的标签名 <{name}>；原始字节未改变。"
                ));
            }
            let open_raw = &rest[..end];
            let open_offset = cursor;
            saw_element_or_decl = true;
            cursor += end;
            rest = &rest[end..];
            if self_closing || (html_mode && is_html_void_tag(name)) {
                tokens.push(MarkupToken::Standalone {
                    raw: open_raw,
                    offset: open_offset,
                });
            } else if html_mode && is_html_raw_text_tag(name) {
                let lower = name.to_ascii_lowercase();
                let Some((close_start, close_end)) = find_raw_close_tag(rest, &lower) else {
                    return Err(format!(
                        "HTML 解析失败，无法切换到 Pretty 视图：标签 <{name}> 未闭合；原始字节未改变。"
                    ));
                };
                tokens.push(MarkupToken::RawBlock {
                    open: open_raw,
                    open_offset,
                    inner: &rest[..close_start],
                    inner_offset: cursor,
                    close: &rest[close_start..close_end],
                    close_offset: cursor + close_start,
                });
                cursor += close_end;
                rest = &rest[close_end..];
            } else {
                let lower = if html_mode {
                    name.to_ascii_lowercase()
                } else {
                    name.to_owned()
                };
                if html_mode {
                    while let Some(top) = stack.last() {
                        if html_tag_implicitly_closes(top, &lower) {
                            stack.pop();
                            tokens.push(MarkupToken::ImplicitClose);
                        } else {
                            break;
                        }
                    }
                }
                if stack.len() >= MAX_MARKUP_DEPTH {
                    return Err(format!(
                        "{kind_label} 嵌套深度超过 {MAX_MARKUP_DEPTH} 美化限额；原始字节未改变。"
                    ));
                }
                stack.push(lower);
                tokens.push(MarkupToken::Open {
                    raw: open_raw,
                    offset: open_offset,
                });
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let chunk = &rest[..end];
            if !chunk.trim().is_empty() {
                tokens.push(MarkupToken::Text {
                    raw: chunk,
                    offset: cursor,
                });
            }
            cursor += end;
            rest = &rest[end..];
        }
    }

    if html_mode {
        while stack.last().is_some_and(|tag| is_html_optional_close_tag(tag)) {
            stack.pop();
            tokens.push(MarkupToken::ImplicitClose);
        }
    }
    if let Some(unclosed) = stack.last() {
        return Err(format!(
            "{kind_label} 解析失败，无法切换到 Pretty 视图：标签 <{unclosed}> 未闭合；原始字节未改变。"
        ));
    }
    if !saw_element_or_decl {
        return Err(format!(
            "{kind_label} 解析失败，无法切换到 Pretty 视图：缺少有效的标签结构；原始字节未改变。"
        ));
    }
    Ok(tokens)
}

fn clean_markup_slice(input: &str) -> (&str, usize) {
    let without_bom = input.trim_start_matches('\u{feff}');
    let leading_trimmed = without_bom.trim_start();
    let start_offset = input.len() - leading_trimmed.len();
    (leading_trimmed.trim_end(), start_offset)
}

pub(super) fn format_pretty_xml_with_lines(
    input: &str,
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) -> Result<(), String> {
    if input.len() > MAX_PRETTY_BYTES {
        return Err("XML 正文超过 2 MiB 美化限额；原始字节未改变。".into());
    }
    let (clean, start_offset) = clean_markup_slice(input);
    roxmltree::Document::parse_with_options(
        clean,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: MAX_MARKUP_NODES,
            entity_resolver: None,
        },
    )
    .map_err(|error| format!("XML 解析失败，无法切换到 Pretty 视图：{error}；原始字节未改变。"))?;
    let tokens = tokenize_markup(clean, start_offset, false)?;
    render_tokens(&tokens, line_map, out);
    Ok(())
}

pub(super) fn format_pretty_html_with_lines(
    input: &str,
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) -> Result<(), String> {
    if input.len() > MAX_PRETTY_BYTES {
        return Err("HTML 正文超过 2 MiB 美化限额；原始字节未改变。".into());
    }
    let (clean, start_offset) = clean_markup_slice(input);
    let tokens = tokenize_markup(clean, start_offset, true)?;
    render_tokens(&tokens, line_map, out);
    Ok(())
}
