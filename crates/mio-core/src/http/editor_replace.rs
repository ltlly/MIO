use std::ops::Range;

use regex::{Captures, Regex, RegexBuilder};

const EDITOR_REGEX_LIMIT: usize = 1024 * 1024;
const EDITOR_REGEX_NEST_LIMIT: u32 = 128;

fn compile_editor_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .size_limit(EDITOR_REGEX_LIMIT)
        .dfa_size_limit(EDITOR_REGEX_LIMIT)
        .nest_limit(EDITOR_REGEX_NEST_LIMIT)
        .build()
        .map_err(|error| format!("搜索正则表达式无效：{error}"))
}

fn push_bounded(out: &mut String, part: &str, max_output: usize) -> Result<(), String> {
    if part.len() > max_output.saturating_sub(out.len()) {
        return Err(format!(
            "替换结果超过 {max_output} 字节限额，原始字节未改变。"
        ));
    }
    out.push_str(part);
    Ok(())
}

fn expand_captures(
    captures: &Captures<'_>,
    mut replacement: &str,
    out: &mut String,
    max_output: usize,
) -> Result<(), String> {
    while let Some(dollar) = replacement.find('$') {
        push_bounded(out, &replacement[..dollar], max_output)?;
        replacement = &replacement[dollar + 1..];
        if let Some(rest) = replacement.strip_prefix('$') {
            push_bounded(out, "$", max_output)?;
            replacement = rest;
            continue;
        }
        let (name, rest) = if let Some(braced) = replacement.strip_prefix('{') {
            let Some(end) = braced.find('}') else {
                push_bounded(out, "$", max_output)?;
                continue;
            };
            (&braced[..end], &braced[end + 1..])
        } else {
            let len = replacement
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            if len == 0 {
                push_bounded(out, "$", max_output)?;
                continue;
            }
            (&replacement[..len], &replacement[len..])
        };
        let matched = match name.parse::<usize>() {
            Ok(index) => captures.get(index),
            Err(_) => captures.name(name),
        };
        if let Some(matched) = matched {
            push_bounded(out, matched.as_str(), max_output)?;
        }
        replacement = rest;
    }
    push_bounded(out, replacement, max_output)
}

pub fn expand_search_replace_one(
    text: &str,
    range: Range<usize>,
    pattern: &str,
    replacement: &str,
    case_insensitive: bool,
    regex: bool,
    max_output: usize,
) -> Result<String, String> {
    if pattern.is_empty() {
        return Err("搜索内容不能为空。".into());
    }
    if text.get(range.clone()).is_none() {
        return Err("当前匹配范围已失效，请重新搜索后再替换。".into());
    }
    let base_len = text.len() - range.len();
    if !regex {
        if replacement.len() > max_output.saturating_sub(base_len) {
            return Err(format!(
                "替换结果超过 {max_output} 字节限额，原始字节未改变。"
            ));
        }
        return Ok(replacement.to_owned());
    }
    let compiled = compile_editor_regex(pattern, case_insensitive)?;
    if !replacement.contains('$') {
        if replacement.len() > max_output.saturating_sub(base_len) {
            return Err(format!(
                "替换结果超过 {max_output} 字节限额，原始字节未改变。"
            ));
        }
        return Ok(replacement.to_owned());
    }
    let captures = compiled
        .captures_at(text, range.start)
        .filter(|caps| {
            caps.get(0)
                .is_some_and(|m| m.start() == range.start && m.end() == range.end)
        })
        .ok_or_else(|| "当前正则匹配已失效，请重新搜索后再替换。".to_owned())?;
    let mut expanded = String::new();
    expand_captures(
        &captures,
        replacement,
        &mut expanded,
        max_output.saturating_sub(base_len),
    )?;
    Ok(expanded)
}

pub fn expand_search_replace_all(
    text: &str,
    ranges: &[Range<usize>],
    pattern: &str,
    replacement: &str,
    case_insensitive: bool,
    regex: bool,
    max_output: usize,
) -> Result<String, String> {
    if pattern.is_empty() {
        return Err("搜索内容不能为空。".into());
    }
    if ranges.is_empty() {
        return Err("当前没有可替换的匹配项。".into());
    }
    let compiled = if regex {
        Some(compile_editor_regex(pattern, case_insensitive)?)
    } else {
        None
    };
    let mut out = String::with_capacity(text.len().min(max_output));
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor || text.get(range.clone()).is_none() {
            return Err("匹配范围重叠或已失效，原始字节未改变。".into());
        }
        push_bounded(&mut out, &text[cursor..range.start], max_output)?;
        if let Some(compiled) = &compiled
            && replacement.contains('$')
        {
            let captures = compiled
                .captures_at(text, range.start)
                .filter(|caps| {
                    caps.get(0)
                        .is_some_and(|m| m.start() == range.start && m.end() == range.end)
                })
                .ok_or_else(|| "当前正则匹配已失效，原始字节未改变。".to_owned())?;
            expand_captures(&captures, replacement, &mut out, max_output)?;
        } else {
            push_bounded(&mut out, replacement, max_output)?;
        }
        cursor = range.end;
    }
    push_bounded(&mut out, &text[cursor..], max_output)?;
    Ok(out)
}
