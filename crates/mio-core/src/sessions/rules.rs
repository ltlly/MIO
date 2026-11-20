use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};

use super::*;
use crate::*;

pub fn rule_matches_request(
    rule: &SessionRule,
    tool: SessionTool,
    request: &HttpRequest,
    scope: &TargetScope,
) -> bool {
    if !rule.enabled || !rule.tools.matches(tool) {
        return false;
    }
    let url_ok = match &rule.condition.url_scope {
        SessionUrlScope::AllUrls => true,
        SessionUrlScope::SuiteScope => scope.is_url_in_scope(&request.url),
        SessionUrlScope::CustomPrefixes(prefixes) => prefixes
            .iter()
            .any(|prefix| url_matches_prefix(&request.url, prefix)),
    };
    if !url_ok {
        return false;
    }
    if let Some(param_cond) = &rule.condition.param_condition {
        let present = collect_request_param_names(request);
        match param_cond {
            SessionParamCondition::RequirePresent(names) => {
                if !names
                    .iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .all(|req_name| present.contains(req_name))
                {
                    return false;
                }
            }
            SessionParamCondition::RequireAbsent(names) => {
                if names
                    .iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .any(|req_name| present.contains(req_name))
                {
                    return false;
                }
            }
        }
    }
    if let Some(needle) = &rule.condition.header_contains {
        let needle = needle.trim();
        if !needle.is_empty() {
            let lower_needle = needle.to_ascii_lowercase();
            let matched = request.headers.iter().any(|h| {
                let val = String::from_utf8_lossy(&h.value);
                format!("{}: {}", h.name, val)
                    .to_ascii_lowercase()
                    .contains(&lower_needle)
            });
            if !matched {
                return false;
            }
        }
    }
    true
}

fn url_matches_prefix(url: &str, prefix: &str) -> bool {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return false;
    }
    if let (Ok(parsed_url), Ok(parsed_prefix)) = (url::Url::parse(url), url::Url::parse(prefix))
        && prefix.contains("://")
    {
        if !parsed_url
            .scheme()
            .eq_ignore_ascii_case(parsed_prefix.scheme())
        {
            return false;
        }
        if parsed_url.host_str().map(str::to_ascii_lowercase)
            != parsed_prefix.host_str().map(str::to_ascii_lowercase)
        {
            return false;
        }
        if let Some(p_port) = parsed_prefix.port()
            && parsed_url.port_or_known_default() != Some(p_port)
        {
            return false;
        }
        let pref_path = parsed_prefix.path();
        if pref_path != "/" && !parsed_url.path().starts_with(pref_path) {
            return false;
        }
        if let Some(pref_q) = parsed_prefix.query() {
            return parsed_url.query().is_some_and(|q| q.starts_with(pref_q));
        }
        return true;
    }
    url.to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
}

pub(crate) fn collect_request_param_names(request: &HttpRequest) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Ok(parsed) = url::Url::parse(&request.url) {
        for (k, _) in parsed.query_pairs() {
            if !k.is_empty() {
                names.insert(k.into_owned());
            }
        }
    }
    if is_form_urlencoded(&request.headers)
        && let Ok(body_str) = std::str::from_utf8(&request.body)
    {
        for (k, _) in url::form_urlencoded::parse(body_str.as_bytes()) {
            if !k.is_empty() {
                names.insert(k.into_owned());
            }
        }
    }
    for header in &request.headers {
        if header.name.eq_ignore_ascii_case("cookie")
            && let Ok(raw) = std::str::from_utf8(&header.value)
        {
            for part in raw.split(';') {
                if let Some((k, _)) = part.trim().split_once('=') {
                    let k = k.trim();
                    if !k.is_empty() {
                        names.insert(k.to_owned());
                    }
                }
            }
        }
    }
    names
}

pub(crate) fn is_form_urlencoded(headers: &[Header]) -> bool {
    headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("content-type")
            && std::str::from_utf8(&h.value).is_ok_and(|v| {
                v.to_ascii_lowercase()
                    .contains("application/x-www-form-urlencoded")
            })
    })
}

pub fn set_request_header(
    request: &mut HttpRequest,
    name: &str,
    value: &str,
    only_if_present: bool,
) -> Result<bool> {
    ensure!(
        validation::valid_header_name(name),
        "请求头名称无效：{name}"
    );
    ensure!(
        validation::valid_header_value(value),
        "请求头值包含非法控制字符或超过长度上限"
    );
    let before = request.headers.clone();
    let mut found = false;
    request.headers.retain_mut(|h| {
        if h.name.eq_ignore_ascii_case(name) {
            if !found {
                h.name = name.to_owned();
                h.value = value.as_bytes().to_vec();
                found = true;
                true
            } else {
                false
            }
        } else {
            true
        }
    });
    if !found && !only_if_present {
        request.headers.push(Header {
            name: name.to_owned(),
            value: value.as_bytes().to_vec(),
        });
    }
    Ok(request.headers != before)
}

fn encode_query_component(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

fn decode_query_key(raw_key: &str) -> String {
    url::form_urlencoded::parse(raw_key.as_bytes())
        .next()
        .map(|(k, _)| k.into_owned())
        .unwrap_or_else(|| raw_key.to_owned())
}

pub fn rewrite_url_query_param(
    url_str: &str,
    name: &str,
    value: &str,
    remove: bool,
    only_if_present: bool,
) -> Result<(String, bool)> {
    ensure!(
        !name.trim().is_empty() && !name.contains(['\r', '\n', '\0', '&', '=', '#']),
        "URL 参数名称无效"
    );
    if !remove {
        ensure!(
            !value.contains(['\r', '\n', '\0', '#']),
            "URL 参数值包含非法字符"
        );
    }
    let _ = url::Url::parse(url_str).context("请求 URL 无效，无法重写查询参数")?;
    let (before_frag, frag) = match url_str.split_once('#') {
        Some((head, tail)) => (head, Some(tail)),
        None => (url_str, None),
    };
    let (base, existing_query) = match before_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_frag, None),
    };
    let encoded_pair = format!(
        "{}={}",
        encode_query_component(name),
        encode_query_component(value)
    );
    let mut segments: Vec<String> = Vec::new();
    let mut matched = false;
    if let Some(q) = existing_query
        && !q.is_empty()
    {
        for seg in q.split('&') {
            let raw_key = seg.split_once('=').map_or(seg, |(k, _)| k);
            if raw_key == name || decode_query_key(raw_key) == name {
                if !remove && !matched {
                    segments.push(encoded_pair.clone());
                }
                matched = true;
            } else {
                segments.push(seg.to_owned());
            }
        }
    }
    if !matched {
        if remove || only_if_present {
            return Ok((url_str.to_owned(), false));
        }
        segments.push(encoded_pair);
    }
    let mut rebuilt = base.to_owned();
    if !segments.is_empty() {
        rebuilt.push('?');
        rebuilt.push_str(&segments.join("&"));
    }
    if let Some(f) = frag {
        rebuilt.push('#');
        rebuilt.push_str(f);
    }
    let _ = url::Url::parse(&rebuilt).context("重写参数后的 URL 无效")?;
    let changed = rebuilt != url_str;
    Ok((rebuilt, changed))
}

pub(crate) fn rewrite_urlencoded_body_if_present(
    request: &mut HttpRequest,
    name: &str,
    value: &str,
) -> bool {
    if !is_form_urlencoded(&request.headers) {
        return false;
    }
    let Ok(body_str) = std::str::from_utf8(&request.body) else {
        return false;
    };
    if body_str.is_empty() {
        return false;
    }
    let encoded_pair = format!(
        "{}={}",
        encode_query_component(name),
        encode_query_component(value)
    );
    let mut matched = false;
    let mut out_segments = Vec::new();
    for seg in body_str.split('&') {
        let raw_key = seg.split_once('=').map_or(seg, |(k, _)| k);
        if raw_key == name || decode_query_key(raw_key) == name {
            if !matched {
                out_segments.push(encoded_pair.clone());
            }
            matched = true;
        } else {
            out_segments.push(seg.to_owned());
        }
    }
    if !matched {
        return false;
    }
    let new_body = out_segments.join("&").into_bytes();
    if new_body == request.body {
        return false;
    }
    request.body = new_body;
    sync_content_length_if_present(request);
    true
}

pub(crate) fn sync_content_length_if_present(request: &mut HttpRequest) {
    let len_bytes = request.body.len().to_string().into_bytes();
    for header in &mut request.headers {
        if header.name.eq_ignore_ascii_case("content-length") {
            header.value = len_bytes;
            break;
        }
    }
}

pub fn replace_request_text(
    request: &mut HttpRequest,
    match_pattern: &str,
    replace_with: &str,
    regex: bool,
) -> Result<bool> {
    ensure!(!match_pattern.is_empty(), "ReplaceRequestText 匹配内容不能为空");
    let compiled_re = if regex {
        Some(
            regex::RegexBuilder::new(match_pattern)
                .size_limit(1024 * 1024)
                .build()
                .map_err(|e| anyhow::anyhow!("ReplaceRequestText 正则表达式无效：{e}"))?,
        )
    } else {
        None
    };
    let replace_str = |input: &str| -> (String, bool) {
        if let Some(re) = &compiled_re {
            let out = re.replace_all(input, replace_with).into_owned();
            let changed = out != input;
            (out, changed)
        } else if input.contains(match_pattern) {
            (input.replace(match_pattern, replace_with), true)
        } else {
            (input.to_owned(), false)
        }
    };

    let before = request.clone();
    if let (Ok(parsed), (next_url, true)) = (url::Url::parse(&request.url), replace_str(&request.url))
        && !next_url.contains(['\r', '\n', '\0', ' '])
    {
        if url::Url::parse(&next_url).is_ok() {
            request.url = next_url;
        } else if let Some(path_q) = parsed.path().strip_prefix('/') {
            let full_pq = match parsed.query() {
                Some(q) => format!("/{path_q}?{q}"),
                None => format!("/{path_q}"),
            };
            let (next_pq, pq_changed) = replace_str(&full_pq);
            if pq_changed && next_pq.starts_with('/') {
                let mut updated = parsed.clone();
                if let Some((p, q)) = next_pq.split_once('?') {
                    updated.set_path(p);
                    updated.set_query(Some(q));
                } else {
                    updated.set_path(&next_pq);
                    updated.set_query(None);
                }
                request.url = updated.into();
            }
        }
    }

    for header in &mut request.headers {
        if header.name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        let line = format!("{}: {}", header.name, String::from_utf8_lossy(&header.value));
        let (next_line, line_changed) = replace_str(&line);
        if line_changed
            && !next_line.contains(['\r', '\n', '\0'])
            && let Some((n, v)) = next_line.split_once(':')
        {
            let n = n.trim();
            let v = v.trim_start();
            if validation::valid_header_name(n) && validation::valid_header_value(v) {
                header.name = n.to_owned();
                header.value = v.as_bytes().to_vec();
                continue;
            }
        }
        if let Ok(val_str) = std::str::from_utf8(&header.value) {
            let (next_val, val_changed) = replace_str(val_str);
            if val_changed && validation::valid_header_value(&next_val) {
                header.value = next_val.into_bytes();
            }
        }
    }

    if let Ok(body_str) = std::str::from_utf8(&request.body) {
        let (next_body, body_changed) = replace_str(body_str);
        if body_changed {
            request.body = next_body.into_bytes();
            sync_content_length_if_present(request);
        }
    }

    Ok(*request != before)
}

pub fn apply_rule_sync_action(
    action: &SessionRuleAction,
    cookie_jar: &[CookieJarEntry],
    request: &mut HttpRequest,
    now_unix_ms: i64,
) -> Result<bool> {
    match action {
        SessionRuleAction::UseCookieJar {
            add_new_cookies,
            update_existing_cookies,
        } => apply_cookie_jar_to_request(
            cookie_jar,
            request,
            *add_new_cookies,
            *update_existing_cookies,
            now_unix_ms,
        ),
        SessionRuleAction::SetHeader {
            name,
            value,
            only_if_present,
        } => set_request_header(request, name, value, *only_if_present),
        SessionRuleAction::SetCookie {
            name,
            value,
            only_if_present,
        } => cookie_jar::set_request_cookie(request, name, value, *only_if_present),
        SessionRuleAction::RewriteUrlParam {
            name,
            value,
            remove,
            only_if_present,
        } => {
            let (next_url, changed) =
                rewrite_url_query_param(&request.url, name, value, *remove, *only_if_present)?;
            if changed {
                request.url = next_url;
            }
            Ok(changed)
        }
        SessionRuleAction::ReplaceRequestText {
            match_pattern,
            replace_with,
            regex,
        } => replace_request_text(request, match_pattern, replace_with, *regex),
        SessionRuleAction::RunMacro { .. } => {
            bail!("RunMacro 须通过异步会话执行器运行")
        }
    }
}
