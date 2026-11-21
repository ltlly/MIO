use super::*;

pub(super) fn compile_rule_regex(pattern: &str) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .ok()
}

pub(super) fn replace_text_with_rule(
    text: &str,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    max_output: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<Option<String>> {
    check()?;
    let mut replaced = String::new();
    let mut copied_until = 0;
    let mut matched = false;
    if rule.regex {
        let owned;
        let regex = match compiled_regex {
            Some(regex) => regex,
            None => {
                let Some(regex) = compile_rule_regex(&rule.match_pattern) else {
                    return Ok(None);
                };
                owned = regex;
                &owned
            }
        };
        if rule.replace_with.contains('$') {
            for captures in regex.captures_iter(text) {
                check()?;
                let found = captures.get(0).context("Proxy 替换正则缺少完整匹配")?;
                append_replacement(
                    &mut replaced,
                    &text[copied_until..found.start()],
                    max_output,
                    check,
                )?;
                append_capture_replacement(
                    &captures,
                    &rule.replace_with,
                    &mut replaced,
                    max_output,
                    check,
                )?;
                copied_until = found.end();
                matched = true;
            }
        } else {
            for found in regex.find_iter(text) {
                check()?;
                append_replacement(
                    &mut replaced,
                    &text[copied_until..found.start()],
                    max_output,
                    check,
                )?;
                append_replacement(&mut replaced, &rule.replace_with, max_output, check)?;
                copied_until = found.end();
                matched = true;
            }
        }
    } else {
        for (start, found) in text.match_indices(rule.match_pattern.as_str()) {
            check()?;
            append_replacement(&mut replaced, &text[copied_until..start], max_output, check)?;
            append_replacement(&mut replaced, &rule.replace_with, max_output, check)?;
            copied_until = start + found.len();
            matched = true;
        }
    }
    if !matched {
        check()?;
        return Ok(None);
    }
    append_replacement(&mut replaced, &text[copied_until..], max_output, check)?;
    Ok((replaced != text).then_some(replaced))
}

pub(super) fn append_replacement(
    output: &mut String,
    fragment: &str,
    max_output: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<()> {
    check()?;
    if fragment.len() > max_output.saturating_sub(output.len()) {
        return Err(UnpackError::Failed(
            "Proxy Match and replace 超过实际消息限额，未分配或转发超限消息".into(),
        )
        .into());
    }
    let needed = output.len() + fragment.len();
    if needed > output.capacity() {
        let capacity = output
            .capacity()
            .saturating_mul(2)
            .max(needed)
            .min(max_output);
        output
            .try_reserve_exact(capacity - output.len())
            .map_err(|_| UnpackError::Failed("Proxy Match and replace 消息内存分配失败".into()))?;
    }
    output.push_str(fragment);
    Ok(())
}

fn append_capture_replacement(
    captures: &regex::Captures<'_>,
    mut replacement: &str,
    output: &mut String,
    max_output: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<()> {
    while let Some(dollar) = replacement.find('$') {
        append_replacement(output, &replacement[..dollar], max_output, check)?;
        replacement = &replacement[dollar + 1..];
        if let Some(escaped) = replacement.strip_prefix('$') {
            append_replacement(output, "$", max_output, check)?;
            replacement = escaped;
            continue;
        }
        let (reference, remaining) = if let Some(braced) = replacement.strip_prefix('{') {
            let Some(end) = braced.find('}') else {
                append_replacement(output, "$", max_output, check)?;
                continue;
            };
            (&braced[..end], &braced[end + 1..])
        } else {
            let length = replacement
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            if length == 0 {
                append_replacement(output, "$", max_output, check)?;
                continue;
            }
            (&replacement[..length], &replacement[length..])
        };
        let captured = match reference.parse::<usize>() {
            Ok(index) => captures.get(index),
            Err(_) => captures.name(reference),
        };
        if let Some(captured) = captured {
            append_replacement(output, captured.as_str(), max_output, check)?;
        }
        replacement = remaining;
    }
    append_replacement(output, replacement, max_output, check)
}

fn headers_wire_bytes(headers: &[Header]) -> usize {
    headers
        .iter()
        .map(|header| header.name.len() + header.value.len() + 4)
        .sum()
}

pub(super) fn valid_header_value_bytes(value: &[u8]) -> bool {
    value
        .iter()
        .all(|byte| (*byte >= 32 && *byte != 127) || *byte == b'\t')
}

pub(super) fn apply_header_rule(
    headers: &mut Vec<Header>,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    check()?;
    if rule.match_pattern.is_empty() {
        if headers.len() >= 1024 {
            return Ok(false);
        }
        if let Some((name, value)) = rule.replace_with.split_once(':') {
            let new_name = name.trim();
            let new_val = value.trim().as_bytes();
            let added_bytes = new_name.len() + new_val.len() + 4;
            if !token(new_name.as_bytes())
                || !valid_header_value_bytes(new_val)
                || headers_wire_bytes(headers).saturating_add(added_bytes) > MAX_HEAD
            {
                return Ok(false);
            }
            headers.push(Header {
                name: new_name.to_owned(),
                value: new_val.to_vec(),
            });
            return Ok(true);
        }
        return Ok(false);
    }

    let mut changed = false;
    let mut index = 0;
    while index < headers.len() {
        check()?;
        let Ok(val_str) = std::str::from_utf8(&headers[index].value) else {
            index += 1;
            continue;
        };
        if headers[index]
            .name
            .len()
            .saturating_add(val_str.len())
            .saturating_add(2)
            > MAX_HEAD
        {
            index += 1;
            continue;
        }
        let line = format!("{}: {}", headers[index].name, val_str);
        let replaced = match replace_text_with_rule(&line, rule, compiled_regex, MAX_HEAD, check) {
            Ok(replaced) => replaced,
            Err(error)
                if matches!(
                    error.downcast_ref::<UnpackError>(),
                    Some(UnpackError::Failed(_))
                ) =>
            {
                index += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some(replaced) = replaced {
            if replaced.trim().is_empty() {
                headers.remove(index);
                changed = true;
                continue;
            }
            if let Some((new_name, new_val)) = replaced.split_once(':') {
                let new_name = new_name.trim();
                let new_val = new_val.trim().as_bytes();
                let old_bytes = headers[index].name.len() + headers[index].value.len() + 4;
                let new_bytes = new_name.len() + new_val.len() + 4;
                let next_total = headers_wire_bytes(headers)
                    .saturating_sub(old_bytes)
                    .saturating_add(new_bytes);
                if token(new_name.as_bytes())
                    && valid_header_value_bytes(new_val)
                    && headers.len() <= 1024
                    && next_total <= MAX_HEAD
                {
                    headers[index].name = new_name.to_owned();
                    headers[index].value = new_val.to_vec();
                    changed = true;
                }
            }
        }
        index += 1;
    }
    Ok(changed)
}

pub(super) fn update_single_content_length(headers: &mut [Header], body_len: usize) {
    let chunked = headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("transfer-encoding")
            && header
                .value
                .split(|byte| *byte == b',')
                .any(|encoding| encoding.trim_ascii().eq_ignore_ascii_case(b"chunked"))
    });
    if chunked {
        return;
    }
    let mut content_length_index = None;
    for (index, header) in headers.iter().enumerate() {
        if header.name.eq_ignore_ascii_case("content-length") {
            if content_length_index.is_some() {
                return;
            }
            content_length_index = Some(index);
        }
    }
    if let Some(index) = content_length_index {
        headers[index].value = body_len.to_string().into_bytes();
    }
}

fn apply_body_rule(
    headers: &mut [Header],
    body: &mut Vec<u8>,
    summary_bytes: &mut u64,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    max_body: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    if rule.match_pattern.is_empty() || body.len() > MAX_BODY {
        return Ok(false);
    }
    let Ok(body_str) = std::str::from_utf8(body) else {
        return Ok(false);
    };
    let Some(replaced) = replace_text_with_rule(
        body_str,
        rule,
        compiled_regex,
        max_body.min(MAX_BODY),
        check,
    )?
    else {
        return Ok(false);
    };
    *body = replaced.into_bytes();
    *summary_bytes = body.len() as u64;
    update_single_content_length(headers, body.len());
    Ok(true)
}

use mio_plugin_api::{FlowState, FlowSummary, TrafficSource};

fn request_target_from_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|_| absolute_request_target(url))
        .unwrap_or_else(|| url.to_owned())
}

fn apply_first_line_rule(
    request: &mut HttpRequest,
    summary: &mut FlowSummary,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    if rule.match_pattern.is_empty() {
        return Ok(false);
    }
    let first_line = format!(
        "{} {} {}",
        request.method,
        request_target_from_url(&request.url),
        request.version.as_str()
    );
    let Some(replaced) =
        replace_text_with_rule(&first_line, rule, compiled_regex, MAX_HEAD, check)?
    else {
        return Ok(false);
    };
    let parts: Vec<&str> = replaced.split_ascii_whitespace().collect();
    if parts.len() != 3 || !token(parts[0].as_bytes()) {
        return Ok(false);
    }
    let Ok(new_version) = mio_script::version_from_script(parts[2]) else {
        return Ok(false);
    };
    let Ok(base) = Url::parse(&request.url) else {
        return Ok(false);
    };
    let new_target = if parts[1].starts_with("http://") || parts[1].starts_with("https://") {
        let Some(target) = absolute_request_target(parts[1]) else {
            return Ok(false);
        };
        target
    } else if parts[1].starts_with('/') && !parts[1].starts_with("//") {
        parts[1].to_owned()
    } else {
        return Ok(false);
    };
    if new_target.contains('#')
        || new_target
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Ok(false);
    }
    let next_url = format!("{}{new_target}", &base[..Position::BeforePath]);
    if Url::parse(&next_url).is_err() {
        return Ok(false);
    }
    request.method = parts[0].to_owned();
    request.url = next_url.clone();
    request.version = new_version;
    summary.method = request.method.clone();
    summary.url = next_url;
    Ok(true)
}

fn replace_param_token(
    current: &str,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    max_output: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<Option<String>> {
    if rule.match_pattern.is_empty() {
        return Ok((current != rule.replace_with).then(|| rule.replace_with.clone()));
    }
    replace_text_with_rule(current, rule, compiled_regex, max_output, check)
}

fn replace_encoded_params(
    input: &str,
    replace_name: bool,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    max_output: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<Option<String>> {
    if input.is_empty() {
        return Ok(None);
    }
    let mut segments = Vec::new();
    let mut changed = false;
    for segment in input.split('&') {
        check()?;
        if segment.is_empty() {
            segments.push(String::new());
            continue;
        }
        let (name, value) = segment.split_once('=').map_or((segment, None), |(k, v)| (k, Some(v)));
        if replace_name {
            let next = replace_param_token(name, rule, compiled_regex, max_output, check)?;
            if let Some(next_name) = next {
                changed = true;
                segments.push(match value {
                    Some(val) => format!("{next_name}={val}"),
                    None => next_name,
                });
            } else {
                segments.push(segment.to_owned());
            }
        } else if let Some(val) = value {
            if let Some(next_val) =
                replace_param_token(val, rule, compiled_regex, max_output, check)?
            {
                changed = true;
                segments.push(format!("{name}={next_val}"));
            } else {
                segments.push(segment.to_owned());
            }
        } else if rule.match_pattern.is_empty() && !rule.replace_with.is_empty() {
            changed = true;
            segments.push(format!("{name}={}", rule.replace_with));
        } else {
            segments.push(segment.to_owned());
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut output = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            append_replacement(&mut output, "&", max_output, check)?;
        }
        append_replacement(&mut output, segment, max_output, check)?;
    }
    Ok((output != input).then_some(output))
}

fn apply_param_rule(
    record: &mut FlowRecord,
    replace_name: bool,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    max_body: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut changed = false;
    if let Some((base, query)) = record.request.url.split_once('?')
        && let Some(next_query) =
            replace_encoded_params(query, replace_name, rule, compiled_regex, MAX_HEAD, check)?
        && !next_query
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        let next_url = if next_query.is_empty() {
            base.to_owned()
        } else {
            format!("{base}?{next_query}")
        };
        if Url::parse(&next_url).is_ok() {
            record.request.url = next_url.clone();
            record.summary.url = next_url;
            changed = true;
        }
    }
    if crate::sessions::rules::is_form_urlencoded(&record.request.headers)
        && record.request.body.len() <= max_body.min(MAX_BODY)
        && let Ok(body_str) = std::str::from_utf8(&record.request.body)
        && let Some(next_body) = replace_encoded_params(
            body_str,
            replace_name,
            rule,
            compiled_regex,
            max_body.min(MAX_BODY),
            check,
        )?
    {
        record.request.body = next_body.into_bytes();
        record.summary.request_bytes = record.request.body.len() as u64;
        update_single_content_length(&mut record.request.headers, record.request.body.len());
        changed = true;
    }
    Ok(changed)
}

pub(super) fn apply_single_rule(
    record: &mut FlowRecord,
    response_stage: bool,
    response_head_only: bool,
    rule: &MatchAndReplaceRule,
    compiled_regex: Option<&Regex>,
    max_body: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    check()?;
    if !response_stage {
        match rule.item_type {
            MatchReplaceTarget::RequestHeader => {
                apply_header_rule(&mut record.request.headers, rule, compiled_regex, check)
            }
            MatchReplaceTarget::RequestBody => apply_body_rule(
                &mut record.request.headers,
                &mut record.request.body,
                &mut record.summary.request_bytes,
                rule,
                compiled_regex,
                max_body,
                check,
            ),
            MatchReplaceTarget::RequestFirstLine => apply_first_line_rule(
                &mut record.request,
                &mut record.summary,
                rule,
                compiled_regex,
                check,
            ),
            MatchReplaceTarget::RequestParamName => {
                apply_param_rule(record, true, rule, compiled_regex, max_body, check)
            }
            MatchReplaceTarget::RequestParamValue => {
                apply_param_rule(record, false, rule, compiled_regex, max_body, check)
            }
            MatchReplaceTarget::RequestScript => {
                mio_script::execute_http_match_replace_script(&rule.filter_script, record, false)
            }
            MatchReplaceTarget::ResponseHeader
            | MatchReplaceTarget::ResponseBody
            | MatchReplaceTarget::ResponseScript => Ok(false),
        }
    } else if let Some(response) = &mut record.response {
        match rule.item_type {
            MatchReplaceTarget::ResponseHeader => {
                apply_header_rule(&mut response.headers, rule, compiled_regex, check)
            }
            MatchReplaceTarget::ResponseBody if !response_head_only => apply_body_rule(
                &mut response.headers,
                &mut response.body,
                &mut record.summary.response_bytes,
                rule,
                compiled_regex,
                max_body,
                check,
            ),
            MatchReplaceTarget::ResponseScript if !response_head_only => {
                mio_script::execute_http_match_replace_script(&rule.filter_script, record, true)
            }
            MatchReplaceTarget::RequestHeader
            | MatchReplaceTarget::RequestBody
            | MatchReplaceTarget::RequestFirstLine
            | MatchReplaceTarget::RequestParamName
            | MatchReplaceTarget::RequestParamValue
            | MatchReplaceTarget::RequestScript
            | MatchReplaceTarget::ResponseScript
            | MatchReplaceTarget::ResponseBody => Ok(false),
        }
    } else {
        Ok(false)
    }
}

pub fn test_match_and_replace_rule(
    rule: &MatchAndReplaceRule,
    raw_message: &[u8],
    is_response: bool,
) -> Result<Vec<u8>> {
    let compiled = if rule.item_type.is_script() {
        mio_script::validate_filter_script(&rule.filter_script)?;
        None
    } else if rule.regex && !rule.match_pattern.is_empty() {
        Some(
            RegexBuilder::new(&rule.match_pattern)
                .size_limit(1024 * 1024)
                .dfa_size_limit(1024 * 1024)
                .build()
                .context("Match and replace 正则表达式无效")?,
        )
    } else {
        None
    };
    let (request, response) = if is_response {
        let (response, _) =
            parse_imported_response(raw_message).context("测试 HTTP 响应报文无法解析")?;
        let request = HttpRequest {
            method: "GET".into(),
            url: "http://example.com/".into(),
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: b"example.com".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        };
        (request, Some(response))
    } else {
        let (request, _) = parse_imported_request(raw_message, Some("http://example.com:80"))
            .context("测试 HTTP 请求报文无法解析")?;
        (request, None)
    };
    let mut record = FlowRecord {
        summary: FlowSummary {
            id: crate::Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: request.method.clone(),
            url: request.url.clone(),
            status: response.as_ref().map(|resp| resp.status),
            state: FlowState::Complete,
            request_bytes: request.body.len() as u64,
            response_bytes: response.as_ref().map_or(0, |resp| resp.body.len() as u64),
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request,
        response,
    };
    apply_single_rule(
        &mut record,
        is_response,
        false,
        rule,
        compiled.as_ref(),
        MAX_BODY,
        &|| Ok(()),
    )?;
    Ok(if is_response {
        response_to_raw(record.response.as_ref().context("测试响应缺失")?)
    } else {
        request_to_raw(&record.request)
    })
}

pub fn apply_match_and_replace(
    record: &mut FlowRecord,
    response_stage: bool,
    response_head_only: bool,
    rules: &[MatchAndReplaceRule],
) -> bool {
    let mut changed = false;
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if rule.regex && !rule.match_pattern.is_empty() {
            let Some(regex) = compile_rule_regex(&rule.match_pattern) else {
                continue;
            };
            changed |= apply_single_rule(
                record,
                response_stage,
                response_head_only,
                rule,
                Some(&regex),
                MAX_BODY,
                &|| Ok(()),
            )
            .unwrap_or(false);
        } else {
            changed |= apply_single_rule(
                record,
                response_stage,
                response_head_only,
                rule,
                None,
                MAX_BODY,
                &|| Ok(()),
            )
            .unwrap_or(false);
        }
    }
    changed
}
