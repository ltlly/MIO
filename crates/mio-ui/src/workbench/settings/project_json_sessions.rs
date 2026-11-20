use std::collections::BTreeMap;

use super::project_json_parse::{
    JVal, check_known_keys, opt_str, req_bool, req_num, req_str, req_uuid,
};
use mio_core::*;

pub(super) fn decode_sessions(o: &BTreeMap<String, JVal>) -> Result<SessionSettings, String> {
    check_known_keys(o, &["cookie_jar", "rules", "macros"], "sessions")?;
    let cookie_jar = match o.get("cookie_jar") {
        None => CookieJarSettings::default(),
        Some(JVal::Obj(jo)) => decode_cookie_jar(jo)?,
        _ => return Err("`sessions.cookie_jar` 必须是对象。".into()),
    };
    let macros = match o.get("macros") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(decode_macro)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`sessions.macros` 必须是数组。".into()),
    };
    let rules = match o.get("rules") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr.iter().map(decode_rule).collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`sessions.rules` 必须是数组。".into()),
    };
    Ok(SessionSettings {
        cookie_jar,
        rules,
        macros,
    })
}

fn decode_cookie_jar(o: &BTreeMap<String, JVal>) -> Result<CookieJarSettings, String> {
    check_known_keys(
        o,
        &[
            "monitor_proxy",
            "monitor_repeater",
            "monitor_intruder",
            "monitor_scanner",
            "monitor_sequencer",
            "monitor_extensions",
            "monitor_mio_ai",
            "entries",
        ],
        "cookie_jar",
    )?;
    let entries = match o.get("entries") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|item| {
                let JVal::Obj(eo) = item else {
                    return Err("Cookie Jar 条目必须是对象。".to_owned());
                };
                check_known_keys(
                    eo,
                    &[
                        "id",
                        "domain",
                        "host_only",
                        "path",
                        "name",
                        "value",
                        "secure",
                        "http_only",
                        "expires_unix_ms",
                    ],
                    "Cookie Jar 条目",
                )?;
                let expires_unix_ms = match eo.get("expires_unix_ms") {
                    None | Some(JVal::Null) => None,
                    Some(JVal::Num(n)) => Some(
                        i64::try_from(*n).map_err(|_| "Cookie 过期时间超出范围。".to_owned())?,
                    ),
                    _ => return Err("`expires_unix_ms` 必须是整数或 null。".into()),
                };
                Ok(CookieJarEntry {
                    id: req_uuid(eo, "id")?,
                    domain: req_str(eo, "domain")?,
                    host_only: req_bool(eo, "host_only", false)?,
                    path: opt_str(eo, "path", "/")?,
                    name: req_str(eo, "name")?,
                    value: req_str(eo, "value")?,
                    secure: req_bool(eo, "secure", false)?,
                    http_only: req_bool(eo, "http_only", false)?,
                    expires_unix_ms,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`cookie_jar.entries` 必须是数组。".into()),
    };
    Ok(CookieJarSettings {
        monitor_proxy: req_bool(o, "monitor_proxy", true)?,
        monitor_repeater: req_bool(o, "monitor_repeater", false)?,
        monitor_intruder: req_bool(o, "monitor_intruder", false)?,
        monitor_scanner: req_bool(o, "monitor_scanner", false)?,
        monitor_sequencer: req_bool(o, "monitor_sequencer", false)?,
        monitor_extensions: req_bool(o, "monitor_extensions", false)?,
        monitor_mio_ai: req_bool(o, "monitor_mio_ai", false)?,
        entries,
    })
}

fn decode_macro(item: &JVal) -> Result<SessionMacro, String> {
    let JVal::Obj(o) = item else {
        return Err("Session Macro 必须是对象。".into());
    };
    check_known_keys(
        o,
        &["id", "name", "policy", "max_retries", "timeout_ms", "steps"],
        "Session Macro",
    )?;
    let failure_policy = match opt_str(o, "policy", "abort")?.as_str() {
        "abort" => MacroFailurePolicy::Abort,
        "retry" => {
            let r = u8::try_from(req_num(o, "max_retries", 1)?)
                .map_err(|_| "Macro 重试次数无效。".to_owned())?;
            MacroFailurePolicy::Retry { max_retries: r }
        }
        other => return Err(format!("未知 Macro 失败策略：`{other}`。")),
    };
    let timeout_ms = u64::try_from(req_num(o, "timeout_ms", 10_000)?)
        .map_err(|_| "Macro 超时时间无效。".to_owned())?;
    let Some(JVal::Arr(step_arr)) = o.get("steps") else {
        return Err("`macro.steps` 必须是数组。".into());
    };
    let steps = step_arr
        .iter()
        .map(decode_macro_step)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SessionMacro {
        id: req_uuid(o, "id")?,
        name: req_str(o, "name")?,
        steps,
        failure_policy,
        timeout_ms,
    })
}

fn decode_macro_step(item: &JVal) -> Result<MacroStep, String> {
    let JVal::Obj(o) = item else {
        return Err("Macro 步骤必须是对象。".into());
    };
    check_known_keys(
        o,
        &[
            "id",
            "method",
            "url",
            "headers",
            "body_b64",
            "use_cookie_jar",
            "expected_status",
            "extractions",
        ],
        "Macro 步骤",
    )?;
    let headers = match o.get("headers") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|h| {
                let JVal::Obj(ho) = h else {
                    return Err("Macro 步骤请求头必须是对象。".to_owned());
                };
                check_known_keys(ho, &["name", "value_b64"], "Macro 步骤请求头")?;
                let value = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    req_str(ho, "value_b64")?,
                )
                .map_err(|_| "Macro 步骤请求头 Base64 解码失败。".to_owned())?;
                Ok(Header {
                    name: req_str(ho, "name")?,
                    value,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`headers` 必须是数组。".into()),
    };
    let body = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        opt_str(o, "body_b64", "")?,
    )
    .map_err(|_| "Macro 步骤正文 Base64 解码失败。".to_owned())?;
    let expected_status = match o.get("expected_status") {
        None | Some(JVal::Null) => None,
        Some(JVal::Num(n)) => {
            Some(u16::try_from(*n).map_err(|_| "Macro 预期状态码无效。".to_owned())?)
        }
        _ => return Err("`expected_status` 必须是整数或 null。".into()),
    };
    let extractions = match o.get("extractions") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|ex| {
                let JVal::Obj(eo) = ex else {
                    return Err("Macro 提取规则必须是对象。".into());
                };
                check_known_keys(
                    eo,
                    &[
                        "name",
                        "source_kind",
                        "arg1",
                        "arg2",
                        "target_kind",
                        "target_name",
                    ],
                    "Macro 提取规则",
                )?;
                let source = match req_str(eo, "source_kind")?.as_str() {
                    "delimiter" => MacroExtractSource::Delimiter {
                        start: req_str(eo, "arg1")?,
                        end: req_str(eo, "arg2")?,
                    },
                    "regex" => MacroExtractSource::Regex {
                        pattern: req_str(eo, "arg1")?,
                    },
                    "response_header" => MacroExtractSource::ResponseHeader {
                        header_name: req_str(eo, "arg1")?,
                    },
                    other => return Err(format!("未知提取源类型：`{other}`。")),
                };
                let target = match opt_str(eo, "target_kind", "param")?.as_str() {
                    "param" => MacroApplyTarget::UrlOrBodyParam,
                    "header" => MacroApplyTarget::Header {
                        header_name: opt_str(eo, "target_name", "")?,
                    },
                    "cookie" => MacroApplyTarget::Cookie {
                        cookie_name: opt_str(eo, "target_name", "")?,
                    },
                    other => return Err(format!("未知提取回填目标：`{other}`。")),
                };
                Ok(MacroExtraction {
                    name: req_str(eo, "name")?,
                    source,
                    target,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`extractions` 必须是数组。".into()),
    };
    Ok(MacroStep {
        id: req_uuid(o, "id")?,
        method: req_str(o, "method")?,
        url: req_str(o, "url")?,
        headers,
        body,
        use_cookie_jar: req_bool(o, "use_cookie_jar", true)?,
        expected_status,
        extractions,
    })
}

fn decode_rule(item: &JVal) -> Result<SessionRule, String> {
    let JVal::Obj(o) = item else {
        return Err("Session 规则必须是对象。".into());
    };
    check_known_keys(
        o,
        &[
            "id",
            "name",
            "enabled",
            "tools",
            "url_mode",
            "url_prefixes",
            "param_mode",
            "param_names",
            "header_contains",
            "actions",
        ],
        "Session 规则",
    )?;
    let tools = match o.get("tools") {
        None => SessionToolScope::default(),
        Some(JVal::Obj(to)) => {
            check_known_keys(
                to,
                &[
                    "proxy",
                    "repeater",
                    "intruder",
                    "scanner",
                    "sequencer",
                    "extensions",
                ],
                "Session 规则 tools",
            )?;
            SessionToolScope {
                proxy: req_bool(to, "proxy", false)?,
                repeater: req_bool(to, "repeater", true)?,
                intruder: req_bool(to, "intruder", true)?,
                scanner: req_bool(to, "scanner", false)?,
                sequencer: req_bool(to, "sequencer", false)?,
                extensions: req_bool(to, "extensions", false)?,
            }
        }
        _ => return Err("`tools` 必须是对象。".into()),
    };
    let str_arr = |key: &str| -> Result<Vec<String>, String> {
        let Some(val) = o.get(key) else {
            return Ok(Vec::new());
        };
        let JVal::Arr(arr) = val else {
            return Err(format!("`{key}` 必须是字符串数组。"));
        };
        arr.iter()
            .map(|v| match v {
                JVal::Str(s) => Ok(s.clone()),
                _ => Err(format!("`{key}` 元素必须是字符串。")),
            })
            .collect()
    };
    let url_scope = match opt_str(o, "url_mode", "all_urls")?.as_str() {
        "all_urls" => SessionUrlScope::AllUrls,
        "suite_scope" => SessionUrlScope::SuiteScope,
        "custom_prefixes" => SessionUrlScope::CustomPrefixes(str_arr("url_prefixes")?),
        other => return Err(format!("未知 Session URL 范围模式：`{other}`。")),
    };
    let param_condition = match opt_str(o, "param_mode", "none")?.as_str() {
        "none" => None,
        "require_present" => Some(SessionParamCondition::RequirePresent(str_arr(
            "param_names",
        )?)),
        "require_absent" => Some(SessionParamCondition::RequireAbsent(str_arr(
            "param_names",
        )?)),
        other => return Err(format!("未知参数条件模式：`{other}`。")),
    };
    let header_contains = match o.get("header_contains") {
        None | Some(JVal::Null) => None,
        Some(JVal::Str(s)) => Some(s.clone()),
        _ => return Err("`header_contains` 必须是字符串或 null。".into()),
    };
    let Some(JVal::Arr(act_arr)) = o.get("actions") else {
        return Err("`actions` 必须是数组。".into());
    };
    let actions = act_arr
        .iter()
        .map(decode_rule_action)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SessionRule {
        id: req_uuid(o, "id")?,
        name: req_str(o, "name")?,
        enabled: req_bool(o, "enabled", true)?,
        tools,
        condition: SessionRuleCondition {
            url_scope,
            param_condition,
            header_contains,
        },
        actions,
    })
}

fn decode_rule_action(item: &JVal) -> Result<SessionRuleAction, String> {
    let JVal::Obj(o) = item else {
        return Err("Session 规则动作必须是对象。".into());
    };
    match req_str(o, "type")?.as_str() {
        "use_cookie_jar" => {
            check_known_keys(
                o,
                &["type", "add_new_cookies", "update_existing_cookies"],
                "use_cookie_jar",
            )?;
            Ok(SessionRuleAction::UseCookieJar {
                add_new_cookies: req_bool(o, "add_new_cookies", true)?,
                update_existing_cookies: req_bool(o, "update_existing_cookies", true)?,
            })
        }
        "set_header" => {
            check_known_keys(
                o,
                &["type", "name", "value", "only_if_present"],
                "set_header",
            )?;
            Ok(SessionRuleAction::SetHeader {
                name: req_str(o, "name")?,
                value: req_str(o, "value")?,
                only_if_present: req_bool(o, "only_if_present", false)?,
            })
        }
        "set_cookie" => {
            check_known_keys(
                o,
                &["type", "name", "value", "only_if_present"],
                "set_cookie",
            )?;
            Ok(SessionRuleAction::SetCookie {
                name: req_str(o, "name")?,
                value: req_str(o, "value")?,
                only_if_present: req_bool(o, "only_if_present", false)?,
            })
        }
        "rewrite_url_param" => {
            check_known_keys(
                o,
                &["type", "name", "value", "remove", "only_if_present"],
                "rewrite_url_param",
            )?;
            Ok(SessionRuleAction::RewriteUrlParam {
                name: req_str(o, "name")?,
                value: opt_str(o, "value", "")?,
                remove: req_bool(o, "remove", false)?,
                only_if_present: req_bool(o, "only_if_present", false)?,
            })
        }
        "replace_request_text" => {
            check_known_keys(
                o,
                &["type", "match_pattern", "replace_with", "regex"],
                "replace_request_text",
            )?;
            Ok(SessionRuleAction::ReplaceRequestText {
                match_pattern: req_str(o, "match_pattern")?,
                replace_with: opt_str(o, "replace_with", "")?,
                regex: req_bool(o, "regex", false)?,
            })
        }
        "run_macro" => {
            check_known_keys(
                o,
                &[
                    "type",
                    "macro_id",
                    "update_cookies_from_jar",
                    "apply_extractions",
                ],
                "run_macro",
            )?;
            Ok(SessionRuleAction::RunMacro {
                macro_id: req_uuid(o, "macro_id")?,
                update_cookies_from_jar: req_bool(o, "update_cookies_from_jar", true)?,
                apply_extractions: req_bool(o, "apply_extractions", true)?,
            })
        }
        other => Err(format!("未知 Session 规则动作类型：`{other}`。")),
    }
}
