use anyhow::{Context, Result, bail, ensure};

use super::strict_json::{StrictValue, to_serde_value};
use crate::{
    ExtRepeaterSettingsPatch, MatchAndReplaceRule, MatchReplaceTarget, RepeaterRedirectMode,
    SessionSettings, Uuid, Workspace,
};

pub(super) fn parse_match_replace_list(
    value: &StrictValue,
    path: &str,
    unmapped: &mut Vec<String>,
) -> Result<Vec<MatchAndReplaceRule>> {
    let StrictValue::Array(items) = value else {
        bail!("`{path}` 必须是数组");
    };
    let mut rules = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let entry_path = format!("{path}[{index}]");
        match parse_single_match_replace_rule(item, &entry_path) {
            Ok(ParsedMatchRule::Rule(rule)) => rules.push(rule),
            Ok(ParsedMatchRule::UnmappedType(other)) => {
                unmapped.push(format!("{entry_path}.rule_type={other}"));
            }
            Err(_) => unmapped.push(format!("{entry_path} (unmapped_archive_format)")),
        }
    }
    Workspace {
        match_and_replace_rules: rules.clone(),
        ..Workspace::default()
    }
    .validate()?;
    Ok(rules)
}

enum ParsedMatchRule {
    Rule(MatchAndReplaceRule),
    UnmappedType(String),
}

fn parse_single_match_replace_rule(
    item: &StrictValue,
    entry_path: &str,
) -> Result<ParsedMatchRule> {
    let StrictValue::Object(map) = item else {
        bail!("`{entry_path}` 必须是对象");
    };
    let raw_type = map
        .get("item_type")
        .or_else(|| map.get("rule_type"))
        .context(format!("`{entry_path}` 缺少 `item_type` 或 `rule_type`"))?;
    let StrictValue::String(kind) = raw_type else {
        bail!("`{entry_path}` 规则类型必须是字符串");
    };
    let normalized_kind = kind.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let item_type = match normalized_kind.as_str() {
        "request_header" => MatchReplaceTarget::RequestHeader,
        "request_body" => MatchReplaceTarget::RequestBody,
        "response_header" => MatchReplaceTarget::ResponseHeader,
        "response_body" => MatchReplaceTarget::ResponseBody,
        _ => return Ok(ParsedMatchRule::UnmappedType(kind.clone())),
    };
    let enabled = match map.get("enabled").or_else(|| map.get("is_enabled")) {
        Some(StrictValue::Bool(flag)) => *flag,
        None => true,
        _ => bail!("`{entry_path}.enabled` 必须是布尔值"),
    };
    let match_pattern = optional_string(
        map.get("match_pattern").or_else(|| map.get("string_match")),
        &format!("{entry_path}.match_pattern"),
    )?;
    let replace_with = optional_string(
        map.get("replace_with")
            .or_else(|| map.get("string_replace")),
        &format!("{entry_path}.replace_with"),
    )?;
    let regex = match map.get("regex").or_else(|| map.get("regex_match")) {
        Some(StrictValue::Bool(flag)) => *flag,
        Some(_) => bail!("`{entry_path}` 布尔匹配模式字段必须是布尔值"),
        None => false,
    };
    let regex = match map.get("is_simple_match") {
        Some(StrictValue::Bool(simple)) => !*simple,
        Some(_) => bail!("`{entry_path}.is_simple_match` 必须是布尔值"),
        None => regex,
    };
    let rule = MatchAndReplaceRule {
        id: optional_uuid(map.get("id"), &format!("{entry_path}.id"))?,
        enabled,
        item_type,
        match_pattern,
        replace_with,
        regex,
        comment: optional_string(map.get("comment"), &format!("{entry_path}.comment"))?,
        script_name: String::new(),
        filter_script: String::new(),
    };
    Workspace {
        match_and_replace_rules: vec![rule.clone()],
        ..Workspace::default()
    }
    .validate()?;
    Ok(ParsedMatchRule::Rule(rule))
}

pub(super) fn parse_repeater_patch(
    value: &StrictValue,
    path: &str,
    unmapped: &mut Vec<String>,
) -> Result<ExtRepeaterSettingsPatch> {
    let StrictValue::Object(map) = value else {
        bail!("`{path}` 必须是对象");
    };
    let mut patch = ExtRepeaterSettingsPatch::default();
    for (key, field) in map {
        match key.as_str() {
            "unpack_compressed_responses" => match expect_bool(field, path, key) {
                Ok(flag) => patch.unpack_compressed_responses = Some(flag),
                Err(_) => unmapped.push(format!("{path}.{key} (unmapped_archive_format)")),
            },
            "update_content_length" => match expect_bool(field, path, key) {
                Ok(flag) => patch.update_content_length = Some(flag),
                Err(_) => unmapped.push(format!("{path}.{key} (unmapped_archive_format)")),
            },
            "normalize_http1_line_endings" => match expect_bool(field, path, key) {
                Ok(flag) => patch.normalize_http1_line_endings = Some(flag),
                Err(_) => unmapped.push(format!("{path}.{key} (unmapped_archive_format)")),
            },
            "follow_redirects" => {
                if let Some(mode) = parse_follow_redirects(field) {
                    patch.follow_redirects = Some(mode);
                } else {
                    unmapped.push(format!("{path}.follow_redirects (unmapped_archive_format)"));
                }
            }
            "request_timeout_secs" => {
                let timeout = match field {
                    StrictValue::Uint(value) if (1..=3600).contains(value) => Some(*value),
                    StrictValue::Int(value) if (1..=3600).contains(value) => Some(*value as u64),
                    _ => None,
                };
                if let Some(timeout) = timeout {
                    patch.request_timeout_secs = Some(timeout);
                } else {
                    unmapped.push(format!(
                        "{path}.request_timeout_secs (unmapped_archive_format)"
                    ));
                }
            }
            _ => unmapped.push(format!("{path}.{key}")),
        }
    }
    Ok(patch)
}

fn parse_follow_redirects(field: &StrictValue) -> Option<RepeaterRedirectMode> {
    match field {
        StrictValue::Bool(false) => Some(RepeaterRedirectMode::Never),
        StrictValue::Bool(true) => Some(RepeaterRedirectMode::Always),
        StrictValue::String(raw) => {
            let normalized = raw.trim().to_ascii_lowercase().replace(['-', ' '], "_");
            match normalized.as_str() {
                "never" | "none" | "false" | "disabled" | "no" => Some(RepeaterRedirectMode::Never),
                "on_site" | "on_site_only" | "same_host" | "same_site" => {
                    Some(RepeaterRedirectMode::OnSiteOnly)
                }
                "in_scope" | "in_scope_only" => Some(RepeaterRedirectMode::InScopeOnly),
                "always" | "true" | "all" | "yes" => Some(RepeaterRedirectMode::Always),
                _ => serde_json::from_value(to_serde_value(field)).ok(),
            }
        }
        _ => None,
    }
}

pub(super) fn parse_sessions(value: &StrictValue, path: &str) -> Result<SessionSettings> {
    ensure!(
        matches!(value, StrictValue::Object(_)),
        "`{path}` 必须是对象"
    );
    let mut unknown = false;
    let json = serde_json::to_vec(&to_serde_value(value))?;
    let mut decoder = serde_json::Deserializer::from_slice(&json);
    let sessions: SessionSettings = serde_ignored::deserialize(&mut decoder, |_| unknown = true)
        .with_context(|| format!("`{path}` 包含无效的会话规则或 Cookie Jar 字段"))?;
    ensure!(!unknown, "`{path}` 包含未映射的会话配置字段；未部分导入");
    sessions.validate()?;
    Ok(sessions)
}

fn expect_bool(value: &StrictValue, path: &str, key: &str) -> Result<bool> {
    match value {
        StrictValue::Bool(flag) => Ok(*flag),
        _ => bail!("`{path}.{key}` 必须是布尔值"),
    }
}

pub(super) fn optional_string(value: Option<&StrictValue>, path: &str) -> Result<String> {
    match value {
        None => Ok(String::new()),
        Some(StrictValue::String(text)) => Ok(text.clone()),
        Some(_) => bail!("`{path}` 必须是字符串"),
    }
}

pub(super) fn optional_uuid(value: Option<&StrictValue>, path: &str) -> Result<Uuid> {
    match value {
        None => Ok(Uuid::new_v4()),
        Some(StrictValue::String(raw)) => {
            let id = Uuid::parse_str(raw).with_context(|| format!("`{path}` 不是合法 UUID"))?;
            ensure!(!id.is_nil(), "`{path}` 不能是全零 UUID");
            Ok(id)
        }
        Some(_) => bail!("`{path}` 必须是 UUID 字符串"),
    }
}
