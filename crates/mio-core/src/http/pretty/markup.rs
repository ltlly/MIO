use super::MAX_PRETTY_BYTES;

const MAX_MARKUP_DEPTH: usize = 256;
const MAX_MARKUP_NODES: u32 = 65_536;

#[derive(Debug)]
enum MarkupToken<'a> {
    Standalone(&'a str),
    Open {
        raw: &'a str,
    },
    Close {
        raw: &'a str,
    },
    ImplicitClose,
    RawBlock {
        open: &'a str,
        inner: &'a str,
        close: &'a str,
    },
    Text(&'a str),
}

pub(super) fn looks_like_html_document(trimmed: &str) -> bool {
    let trimmed = trimmed.trim_start_matches('\u{feff}').trim_start();
    let prefix = trimmed.get(..32).unwrap_or(trimmed).to_ascii_lowercase();
    prefix.starts_with("<!doctype html") || prefix.starts_with("<html")
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

fn format_css_block_lines(css: &str, base_depth: usize, lines: &mut Vec<String>) {
    let mut depth = base_depth;
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_comment = false;
    let mut chars = css.chars().peekable();
    let push_line = |buf: &mut String, d: usize, out: &mut Vec<String>| {
        let trimmed = buf.trim();
        if !trimmed.is_empty() {
            out.push(format!("{}{trimmed}", "  ".repeat(d)));
        }
        buf.clear();
    };
    while let Some(ch) = chars.next() {
        if in_comment {
            current.push(ch);
            if ch == '*' && chars.peek() == Some(&'/') {
                current.push(chars.next().unwrap());
                in_comment = false;
            }
            continue;
        }
        if let Some(q) = quote {
            current.push(ch);
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '/' if chars.peek() == Some(&'*') => {
                current.push(ch);
                current.push(chars.next().unwrap());
                in_comment = true;
            }
            '"' | '\'' => {
                quote = Some(ch);
                current.push(ch);
            }
            '{' => {
                current.push('{');
                push_line(&mut current, depth, lines);
                depth += 1;
            }
            ';' => {
                current.push(';');
                push_line(&mut current, depth, lines);
            }
            '}' => {
                push_line(&mut current, depth, lines);
                depth = depth.saturating_sub(1);
                lines.push(format!("{}}}", "  ".repeat(depth)));
            }
            '\r' | '\n' => {
                if !current.ends_with(' ') && !current.is_empty() {
                    current.push(' ');
                }
            }
            _ => current.push(ch),
        }
    }
    push_line(&mut current, depth, lines);
}

fn render_tokens(tokens: &[MarkupToken<'_>]) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(tokens.len());
    let mut depth = 0usize;
    let mut idx = 0;
    while idx < tokens.len() {
        let indent = "  ".repeat(depth);
        match &tokens[idx] {
            MarkupToken::Standalone(raw) => {
                lines.push(format!("{indent}{}", raw.trim()));
                idx += 1;
            }
            MarkupToken::Open { raw } => {
                if let (
                    Some(MarkupToken::Text(text)),
                    Some(MarkupToken::Close { raw: close_raw }),
                ) = (tokens.get(idx + 1), tokens.get(idx + 2))
                {
                    let trimmed = text.trim();
                    if !trimmed.contains('\n') && !trimmed.contains('\r') {
                        lines.push(format!("{indent}{raw}{trimmed}{close_raw}"));
                        idx += 3;
                        continue;
                    }
                }
                if let Some(MarkupToken::Close { raw: close_raw }) = tokens.get(idx + 1) {
                    lines.push(format!("{indent}{raw}{close_raw}"));
                    idx += 2;
                    continue;
                }
                lines.push(format!("{indent}{raw}"));
                depth += 1;
                idx += 1;
            }
            MarkupToken::Close { raw } => {
                depth = depth.saturating_sub(1);
                let close_indent = "  ".repeat(depth);
                lines.push(format!("{close_indent}{raw}"));
                idx += 1;
            }
            MarkupToken::ImplicitClose => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            MarkupToken::RawBlock { open, inner, close } => {
                let trimmed = inner.trim();
                let tag_after_lt = open.get(1..).map(|s| s.to_ascii_lowercase());
                let is_style = tag_after_lt.as_deref().is_some_and(|s| s.starts_with("style"));
                let is_title = tag_after_lt.as_deref().is_some_and(|s| s.starts_with("title"));
                if trimmed.is_empty() {
                    lines.push(format!("{indent}{open}{close}"));
                } else if is_style && trimmed.contains('{') && trimmed.contains('}') {
                    lines.push(format!("{indent}{open}"));
                    format_css_block_lines(trimmed, depth + 1, &mut lines);
                    lines.push(format!("{indent}{close}"));
                } else if is_title {
                    lines.push(format!("{indent}{open}"));
                    let child_indent = "  ".repeat(depth + 1);
                    for line in trimmed.replace("\r\n", "\n").lines() {
                        let t = line.trim();
                        if !t.is_empty() {
                            lines.push(format!("{child_indent}{t}"));
                        }
                    }
                    lines.push(format!("{indent}{close}"));
                } else if !trimmed.contains('\n') && !trimmed.contains('\r') {
                    lines.push(format!("{indent}{open}{trimmed}{close}"));
                } else {
                    lines.push(format!("{indent}{open}"));
                    lines.push(inner.replace("\r\n", "\n").trim_matches('\n').to_owned());
                    lines.push(format!("{indent}{close}"));
                }
                idx += 1;
            }
            MarkupToken::Text(text) => {
                let normalized = text.replace("\r\n", "\n");
                for line in normalized.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push(format!("{indent}{trimmed}"));
                    }
                }
                idx += 1;
            }
        }
    }
    lines.join("\n")
}

fn tokenize_markup<'a>(input: &'a str, html_mode: bool) -> Result<Vec<MarkupToken<'a>>, String> {
    let kind_label = if html_mode { "HTML" } else { "XML" };
    let mut rest = input.trim_start_matches('\u{feff}').trim();
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
            tokens.push(MarkupToken::Standalone(&rest[..end]));
            saw_element_or_decl = true;
            rest = &rest[end..];
        } else if rest.starts_with("<![CDATA[") {
            let Some(end) = rest[9..].find("]]>").map(|pos| pos + 12) else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：CDATA 未闭合；原始字节未改变。"
                ));
            };
            tokens.push(MarkupToken::Standalone(&rest[..end]));
            saw_element_or_decl = true;
            rest = &rest[end..];
        } else if rest.starts_with("<?") {
            let Some(end) = rest[2..].find("?>").map(|pos| pos + 4) else {
                return Err(format!(
                    "{kind_label} 解析失败，无法切换到 Pretty 视图：处理指令未闭合；原始字节未改变。"
                ));
            };
            tokens.push(MarkupToken::Standalone(&rest[..end]));
            saw_element_or_decl = true;
            rest = &rest[end..];
        } else if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case("<!doctype") {
            let end = scan_doctype_end(rest, kind_label)?;
            tokens.push(MarkupToken::Standalone(&rest[..end]));
            saw_element_or_decl = true;
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
            tokens.push(MarkupToken::Close {
                raw: &rest[..gt + 1],
            });
            rest = &rest[gt + 1..];
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
            saw_element_or_decl = true;
            rest = &rest[end..];
            if self_closing || (html_mode && is_html_void_tag(name)) {
                tokens.push(MarkupToken::Standalone(open_raw));
            } else if html_mode && is_html_raw_text_tag(name) {
                let lower = name.to_ascii_lowercase();
                let Some((close_start, close_end)) = find_raw_close_tag(rest, &lower) else {
                    return Err(format!(
                        "HTML 解析失败，无法切换到 Pretty 视图：标签 <{name}> 未闭合；原始字节未改变。"
                    ));
                };
                tokens.push(MarkupToken::RawBlock {
                    open: open_raw,
                    inner: &rest[..close_start],
                    close: &rest[close_start..close_end],
                });
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
                tokens.push(MarkupToken::Open { raw: open_raw });
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let chunk = &rest[..end];
            if !chunk.trim().is_empty() {
                tokens.push(MarkupToken::Text(chunk));
            }
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

pub(super) fn format_pretty_xml(input: &str) -> Result<String, String> {
    if input.len() > MAX_PRETTY_BYTES {
        return Err("XML 正文超过 2 MiB 美化限额；原始字节未改变。".into());
    }
    let clean = input.trim_start_matches('\u{feff}').trim();
    roxmltree::Document::parse_with_options(
        clean,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: MAX_MARKUP_NODES,
            entity_resolver: None,
        },
    )
    .map_err(|error| format!("XML 解析失败，无法切换到 Pretty 视图：{error}；原始字节未改变。"))?;
    let tokens = tokenize_markup(clean, false)?;
    Ok(render_tokens(&tokens))
}

pub(super) fn format_pretty_html(input: &str) -> Result<String, String> {
    if input.len() > MAX_PRETTY_BYTES {
        return Err("HTML 正文超过 2 MiB 美化限额；原始字节未改变。".into());
    }
    let clean = input.trim_start_matches('\u{feff}').trim();
    let tokens = tokenize_markup(clean, true)?;
    Ok(render_tokens(&tokens))
}
