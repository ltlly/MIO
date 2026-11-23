use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
    time::Instant,
};

use mio_plugin_api::{WebSocketDirection, WebSocketKind};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    ProxyResponseModificationSettings, SslPassThroughRule, WsMatchAndReplaceRule,
    WsReplaceDirection,
};

pub const MAX_PROXY_REGEX_SIZE: usize = 256 * 1024;
const UNHIDDEN_STYLE: &str =
    "border: 2px dashed #d97706 !important; background-color: #fffbeb !important;";

pub fn validate_ssl_and_ws_rules(
    ssl_pass_through_rules: &[SslPassThroughRule],
    ws_match_and_replace_rules: &[WsMatchAndReplaceRule],
) -> Result<()> {
    ensure!(
        ssl_pass_through_rules.len() <= 256,
        "SSL Pass Through 规则数量超过 256 条上限"
    );
    let mut ssl_ids = HashSet::new();
    for rule in ssl_pass_through_rules {
        ensure!(
            !rule.id.is_nil() && ssl_ids.insert(rule.id),
            "SSL Pass Through 规则 ID 无效或重复"
        );
        ensure!(
            !rule.host_pattern.trim().is_empty()
                && rule.host_pattern.len() <= 1024
                && !rule.host_pattern.chars().any(char::is_control),
            "SSL Pass Through 主机模式不能为空、包含控制字符或超过 1024 字节"
        );
        ensure!(
            rule.port.is_none_or(|port| port > 0),
            "SSL Pass Through 端口必须大于 0"
        );
        if rule.is_regex {
            compile_bounded_regex(&rule.host_pattern, true)
                .context("SSL Pass Through 主机正则表达式无效或超过 256 KiB 限额")?;
        } else {
            let pat = rule.host_pattern.trim();
            if crate::http::ip_range::parse_cidr_or_ip_range(pat)
                .map_err(|err| anyhow::anyhow!("SSL Pass Through IP 段无效：{err}"))?
                .is_none()
            {
                crate::validate_destination_glob_pattern(pat)
                    .map_err(|error| anyhow::anyhow!("SSL Pass Through 主机模式无效：{error}"))?;
            }
        }
    }
    ensure!(
        ws_match_and_replace_rules.len() <= 256,
        "WebSocket Match and Replace 规则数量超过 256 条上限"
    );
    let mut ws_ids = HashSet::new();
    for rule in ws_match_and_replace_rules {
        ensure!(
            !rule.id.is_nil() && ws_ids.insert(rule.id),
            "WebSocket Match and Replace 规则 ID 无效或重复"
        );
        ensure!(
            !rule.match_pattern.is_empty()
                && rule.match_pattern.len() <= 4096
                && rule.replace_with.len() <= 4096
                && rule.comment.len() <= 960
                && !rule.replace_with.contains('\0'),
            "WebSocket Match and Replace 规则不能为空、包含 NUL 或超过长度上限"
        );
        if rule.is_regex {
            compile_bounded_regex(&rule.match_pattern, false)
                .context("WebSocket Match and Replace 正则表达式无效或超过 256 KiB 限额")?;
        }
    }
    Ok(())
}

fn compile_bounded_regex(pattern: &str, case_insensitive: bool) -> Result<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(MAX_PROXY_REGEX_SIZE)
        .dfa_size_limit(MAX_PROXY_REGEX_SIZE)
        .build()
        .map_err(Into::into)
}

enum CompiledSslMatcher {
    Regex(Regex),
    IpRange(crate::http::ip_range::IpRangeMatcher),
    Glob,
}

pub fn build_ssl_pass_through_matcher(
    rules: &[SslPassThroughRule],
) -> Result<Option<mio_proxy::SslPassThroughMatcher>> {
    let mut compiled = Vec::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        let matcher = if rule.is_regex {
            CompiledSslMatcher::Regex(
                compile_bounded_regex(&rule.host_pattern, true)
                    .context("SSL Pass Through 正则表达式无效")?,
            )
        } else {
            let pat = rule.host_pattern.trim();
            if let Some(ip_range) = crate::http::ip_range::parse_cidr_or_ip_range(pat)
                .map_err(|err| anyhow::anyhow!("SSL Pass Through IP 段无效：{err}"))?
            {
                CompiledSslMatcher::IpRange(ip_range)
            } else {
                crate::validate_destination_glob_pattern(pat)
                    .map_err(|error| anyhow::anyhow!("SSL Pass Through 主机模式无效：{error}"))?;
                CompiledSslMatcher::Glob
            }
        };
        compiled.push((rule.clone(), matcher));
    }
    if compiled.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(move |host: &str, port: u16| {
        let normalized = host.trim().trim_matches(['[', ']']);
        compiled.iter().any(|(rule, matcher)| {
            if rule.port.is_some_and(|expected| expected != port) {
                return false;
            }
            match matcher {
                CompiledSslMatcher::Regex(re) => re.is_match(normalized),
                CompiledSslMatcher::IpRange(range) => range.matches_host(normalized),
                CompiledSslMatcher::Glob => {
                    mio_proxy::matches_bypass_host(rule.host_pattern.trim(), normalized)
                }
            }
        })
    })))
}

#[allow(dead_code)]
pub fn build_websocket_transform(
    rules: Vec<WsMatchAndReplaceRule>,
) -> Result<Option<mio_proxy::WebSocketTransform>> {
    build_websocket_transform_with_scope(rules, false, crate::TargetScope::default())
}

pub fn build_websocket_transform_with_scope(
    rules: Vec<WsMatchAndReplaceRule>,
    in_scope_only: bool,
    scope: crate::TargetScope,
) -> Result<Option<mio_proxy::WebSocketTransform>> {
    let mut compiled = Vec::new();
    for rule in rules.into_iter().filter(|rule| rule.enabled) {
        if rule.match_pattern.is_empty() {
            continue;
        }
        let regex = if rule.is_regex {
            Some(
                compile_bounded_regex(&rule.match_pattern, false)
                    .context("WebSocket Match and Replace 正则表达式无效或超过 256 KiB 限额")?,
            )
        } else {
            None
        };
        let bridge = MatchAndReplaceRule {
            id: rule.id,
            enabled: true,
            item_type: MatchReplaceTarget::RequestBody,
            match_pattern: rule.match_pattern.clone(),
            replace_with: rule.replace_with.clone(),
            regex: rule.is_regex,
            comment: rule.comment.clone(),
            script_name: String::new(),
            filter_script: String::new(),
        };
        compiled.push((rule.direction, bridge, regex));
    }
    if compiled.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(
        move |upgrade_request, direction, kind, payload, cancel, limits| {
            check_intercept_control(cancel, limits.0).map_err(intercept_control_error)?;
            if in_scope_only && !scope.is_url_in_scope(&upgrade_request.url) {
                let alt = upgrade_request
                    .url
                    .strip_prefix("ws://")
                    .map(|r| format!("http://{r}"))
                    .or_else(|| {
                        upgrade_request
                            .url
                            .strip_prefix("wss://")
                            .map(|r| format!("https://{r}"))
                    });
                if !alt.as_deref().is_some_and(|u| scope.is_url_in_scope(u)) {
                    return Ok(None);
                }
            }
            if kind != WebSocketKind::Text || payload.len() > limits.1.min(MAX_BODY) {
                return Ok(None);
            }
            let Ok(initial) = std::str::from_utf8(payload) else {
                return Ok(None);
            };
            let mut working: Option<String> = None;
            let max_out = limits.1.min(MAX_BODY);
            for (rule_dir, bridge, regex) in &compiled {
                let matches_dir = match rule_dir {
                    WsReplaceDirection::Both => true,
                    WsReplaceDirection::ClientToServer => {
                        direction == WebSocketDirection::ClientToServer
                    }
                    WsReplaceDirection::ServerToClient => {
                        direction == WebSocketDirection::ServerToClient
                    }
                };
                if !matches_dir {
                    continue;
                }
                let current = working.as_deref().unwrap_or(initial);
                if let Some(next) = match_replace::replace_text_with_rule(
                    current,
                    bridge,
                    regex.as_ref(),
                    max_out,
                    &|| check_intercept_control(cancel, limits.0),
                )
                .map_err(intercept_control_error)?
                {
                    working = Some(next);
                }
            }
            check_intercept_control(cancel, limits.0).map_err(intercept_control_error)?;
            Ok(working
                .filter(|next| next.as_bytes() != payload)
                .map(String::into_bytes))
        },
    )))
}

pub(crate) fn apply_response_modification(
    record: &mut FlowRecord,
    settings: ProxyResponseModificationSettings,
    max_body: usize,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<bool> {
    if settings.is_default() {
        return Ok(false);
    }
    let Some(response) = &mut record.response else {
        return Ok(false);
    };
    let check = || check_intercept_control(cancel, deadline);
    let mut changed = false;
    if settings.remove_secure_flag_from_cookies {
        changed |= strip_secure_cookie_flag(&mut response.headers, &check)?;
    }
    if settings.convert_https_links_to_http {
        changed |= convert_location_https_to_http(&mut response.headers, &check)?;
    }
    if response.body.is_empty() || response.body.len() > max_body.min(MAX_BODY) {
        return Ok(changed);
    }
    if !is_html_response(&response.headers) {
        return Ok(changed);
    }
    let Ok(html) = std::str::from_utf8(&response.body) else {
        return Ok(changed);
    };
    let Some(modified) =
        rewrite_html_form_controls(html, settings, max_body.min(MAX_BODY), &check)?
    else {
        return Ok(changed);
    };
    response.body = modified.into_bytes();
    record.summary.response_bytes = response.body.len() as u64;
    match_replace::update_single_content_length(&mut response.headers, response.body.len());
    Ok(true)
}

fn static_regex_rule(pattern: &str, replace_with: &str) -> (Regex, MatchAndReplaceRule) {
    let regex = compile_bounded_regex(pattern, false)
        .expect("static proxy transform regex must compile");
    let rule = MatchAndReplaceRule {
        match_pattern: pattern.into(),
        replace_with: replace_with.into(),
        regex: true,
        ..Default::default()
    };
    (regex, rule)
}

fn secure_cookie_rule() -> &'static (Regex, MatchAndReplaceRule) {
    static RULE: OnceLock<(Regex, MatchAndReplaceRule)> = OnceLock::new();
    RULE.get_or_init(|| static_regex_rule(r"(?i);\s*secure\b", ""))
}

fn https_scheme_rule() -> &'static (Regex, MatchAndReplaceRule) {
    static RULE: OnceLock<(Regex, MatchAndReplaceRule)> = OnceLock::new();
    RULE.get_or_init(|| static_regex_rule(r"(?i)https://", "http://"))
}

fn script_tag_rule() -> &'static (Regex, MatchAndReplaceRule) {
    static RULE: OnceLock<(Regex, MatchAndReplaceRule)> = OnceLock::new();
    RULE.get_or_init(|| static_regex_rule(r"(?is)<script\b[^>]*>.*?</script\s*>", ""))
}

fn object_tag_rule() -> &'static (Regex, MatchAndReplaceRule) {
    static RULE: OnceLock<(Regex, MatchAndReplaceRule)> = OnceLock::new();
    RULE.get_or_init(|| static_regex_rule(r"(?is)<object\b[^>]*>.*?</object\s*>", ""))
}

fn embed_tag_rule() -> &'static (Regex, MatchAndReplaceRule) {
    static RULE: OnceLock<(Regex, MatchAndReplaceRule)> = OnceLock::new();
    RULE.get_or_init(|| static_regex_rule(r"(?is)<embed\b[^>]*>(?:.*?</embed\s*>)?", ""))
}

fn strip_secure_cookie_flag(
    headers: &mut [Header],
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let (regex, rule) = secure_cookie_rule();
    let mut changed = false;
    for header in headers.iter_mut() {
        check()?;
        if !header.name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let Ok(value) = std::str::from_utf8(&header.value) else {
            continue;
        };
        if let Some(replaced) =
            match_replace::replace_text_with_rule(value, rule, Some(regex), MAX_HEAD, check)?
        {
            header.value = replaced.into_bytes();
            changed = true;
        }
    }
    Ok(changed)
}

fn convert_location_https_to_http(
    headers: &mut [Header],
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let (regex, rule) = https_scheme_rule();
    let mut changed = false;
    for header in headers.iter_mut() {
        check()?;
        if !header.name.eq_ignore_ascii_case("location") {
            continue;
        }
        let Ok(value) = std::str::from_utf8(&header.value) else {
            continue;
        };
        if let Some(replaced) =
            match_replace::replace_text_with_rule(value, rule, Some(regex), MAX_HEAD, check)?
        {
            header.value = replaced.into_bytes();
            changed = true;
        }
    }
    Ok(changed)
}

fn is_html_response(headers: &[Header]) -> bool {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-type"))
        .any(|header| {
            let media = std::str::from_utf8(&header.value)
                .unwrap_or_default()
                .split(';')
                .next()
                .unwrap_or_default()
                .trim();
            media.eq_ignore_ascii_case("text/html")
                || media.eq_ignore_ascii_case("application/xhtml+xml")
        })
}

fn rewrite_html_form_controls(
    html: &str,
    settings: ProxyResponseModificationSettings,
    max_output: usize,
    check: &impl Fn() -> Result<()>,
) -> Result<Option<String>> {
    let mut current = html.to_owned();
    if settings.remove_all_javascript {
        let (regex, rule) = script_tag_rule();
        if let Some(next) = match_replace::replace_text_with_rule(
            &current,
            rule,
            Some(regex),
            max_output,
            check,
        )? {
            current = next;
        }
    }
    if settings.remove_object_tags {
        for (regex, rule) in [object_tag_rule(), embed_tag_rule()] {
            if let Some(next) = match_replace::replace_text_with_rule(
                &current,
                rule,
                Some(regex),
                max_output,
                check,
            )? {
                current = next;
            }
        }
    }
    let bytes = current.as_bytes();
    let mut out = String::new();
    let mut copied = 0usize;
    let mut changed = current != html;
    let mut idx = 0usize;
    let mut next_check_at = 0usize;
    while idx < bytes.len() {
        if idx >= next_check_at {
            check()?;
            next_check_at = idx.saturating_add(4096);
        }
        if bytes[idx] != b'<' {
            idx += 1;
            continue;
        }
        let Some((tag_end, tag_name)) = scan_opening_tag(bytes, idx) else {
            idx += 1;
            continue;
        };
        let is_control = matches!(
            tag_name.as_str(),
            "input" | "button" | "select" | "textarea" | "fieldset" | "option" | "optgroup"
        ) || settings.remove_javascript_form_validation
            || settings.remove_all_javascript;
        if is_control
            && let Some(rewritten) =
                rewrite_single_tag(&current[idx..tag_end], &tag_name, settings, check)?
        {
            match_replace::append_replacement(&mut out, &current[copied..idx], max_output, check)?;
            match_replace::append_replacement(&mut out, &rewritten, max_output, check)?;
            copied = tag_end;
            changed = true;
        }
        idx = tag_end;
    }
    match_replace::append_replacement(&mut out, &current[copied..], max_output, check)?;
    if settings.convert_https_links_to_http {
        let (regex, rule) = https_scheme_rule();
        if let Some(next) =
            match_replace::replace_text_with_rule(&out, rule, Some(regex), max_output, check)?
        {
            out = next;
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    Ok((out != html).then_some(out))
}

fn scan_opening_tag(bytes: &[u8], start: usize) -> Option<(usize, String)> {
    let mut i = start + 1;
    if i >= bytes.len() || !bytes[i].is_ascii_alphabetic() {
        return None;
    }
    let name_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    let name = std::str::from_utf8(&bytes[name_start..i])
        .ok()?
        .to_ascii_lowercase();
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            quote = Some(b);
        } else if b == b'>' {
            return Some((i + 1, name));
        }
        i += 1;
    }
    None
}

struct ParsedAttr<'a> {
    raw: &'a str,
    name_lower: String,
    unquoted_value: Option<&'a str>,
}

fn parse_tag_attrs(attrs: &str) -> Vec<ParsedAttr<'_>> {
    let bytes = attrs.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let name_lower = attrs[start..i].to_ascii_lowercase();
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let mut unquoted_value = None;
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let q = bytes[j];
                j += 1;
                let val_start = j;
                while j < bytes.len() && bytes[j] != q {
                    j += 1;
                }
                unquoted_value = Some(&attrs[val_start..j]);
                if j < bytes.len() {
                    j += 1;
                }
            } else {
                let val_start = j;
                while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                unquoted_value = Some(&attrs[val_start..j]);
            }
            i = j;
        }
        out.push(ParsedAttr {
            raw: &attrs[start..i],
            name_lower,
            unquoted_value,
        });
    }
    out
}

fn rewrite_single_tag(
    tag: &str,
    tag_name: &str,
    settings: ProxyResponseModificationSettings,
    check: &impl Fn() -> Result<()>,
) -> Result<Option<String>> {
    check()?;
    let self_closing = tag.ends_with("/>");
    let close_len = if self_closing { 2 } else { 1 };
    let head_len = 1 + tag_name.len();
    let inner = &tag[head_len..tag.len() - close_len];
    let parsed = parse_tag_attrs(inner);
    let mut unhidden = false;
    let mut changed = false;
    let mut kept: Vec<String> = Vec::with_capacity(parsed.len() + 2);
    let mut style_idx: Option<usize> = None;

    for attr in parsed {
        if settings.enable_disabled_form_fields && attr.name_lower == "disabled" {
            changed = true;
            continue;
        }
        if settings.remove_input_field_length_limits
            && matches!(tag_name, "input" | "textarea")
            && matches!(attr.name_lower.as_str(), "maxlength" | "minlength")
        {
            changed = true;
            continue;
        }
        if settings.remove_javascript_form_validation
            && matches!(
                attr.name_lower.as_str(),
                "onsubmit"
                    | "onchange"
                    | "onblur"
                    | "oninput"
                    | "oninvalid"
                    | "onkeyup"
                    | "onkeydown"
                    | "onkeypress"
                    | "required"
                    | "pattern"
            )
        {
            changed = true;
            continue;
        }
        if settings.remove_all_javascript {
            if attr.name_lower.starts_with("on") {
                changed = true;
                continue;
            }
            if let Some(val) = attr.unquoted_value
                && val.trim_start().to_ascii_lowercase().starts_with("javascript:")
            {
                kept.push(format!("{}=\"#\"", attr.name_lower));
                changed = true;
                continue;
            }
        }
        if settings.unhide_hidden_form_fields
            && tag_name == "input"
            && attr.name_lower == "type"
            && attr
                .unquoted_value
                .is_some_and(|val| val.trim().eq_ignore_ascii_case("hidden"))
        {
            kept.push("type=\"text\" data-mio-unhidden=\"true\"".to_owned());
            unhidden = true;
            changed = true;
            continue;
        }
        if attr.name_lower == "style" {
            style_idx = Some(kept.len());
            if let Some(existing) = attr.unquoted_value {
                kept.push(format!("style=\"{existing}\""));
            } else {
                kept.push(attr.raw.to_owned());
            }
            continue;
        }
        kept.push(attr.raw.to_owned());
    }

    if unhidden && settings.highlight_unhidden_fields {
        if let Some(pos) = style_idx {
            let existing = kept[pos]
                .strip_prefix("style=\"")
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or_default()
                .trim_end_matches(';');
            kept[pos] = if existing.is_empty() {
                format!("style=\"{UNHIDDEN_STYLE}\"")
            } else {
                format!("style=\"{existing}; {UNHIDDEN_STYLE}\"")
            };
        } else {
            kept.push(format!("style=\"{UNHIDDEN_STYLE}\""));
        }
    }

    if !changed {
        return Ok(None);
    }
    let open = &tag[..head_len];
    let close = if self_closing { " />" } else { ">" };
    if kept.is_empty() {
        Ok(Some(format!("{open}{}", &tag[tag.len() - close_len..])))
    } else {
        Ok(Some(format!("{open} {}{close}", kept.join(" "))))
    }
}
