use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};

use super::replace_rules::{
    optional_string, optional_uuid, parse_match_replace_list, parse_repeater_patch, parse_sessions,
};
use super::strict_json::StrictValue;
use crate::{AdvancedScopeRule, ExtEmbeddedProjectConfig, ScopeProtocol, ScopeRule, TargetScope};

type ExtractedParts = (ExtEmbeddedProjectConfig, Vec<String>, Vec<String>);

pub(super) fn extract_sections(root: &BTreeMap<String, StrictValue>) -> Result<ExtractedParts> {
    let mut config = ExtEmbeddedProjectConfig::default();
    let mut skipped = Vec::new();
    let mut unmapped = Vec::new();
    let mut scope_sources = 0usize;
    let mut match_replace_sources = 0usize;
    let mut repeater_sources = 0usize;
    let mut session_sources = 0usize;

    for (key, value) in root {
        match key.as_str() {
            "proxy_listener" | "proxy_listeners" => skip_listeners(&mut skipped),
            "extender" | "extensions" | "plugins" => skip_plugins(&mut skipped),
            "scanner" | "live_audit" | "crawl" | "automated_tasks" | "collaborator" => {
                skip_automation(&mut skipped)
            }
            "scope" => {
                scope_sources += 1;
                match parse_scope(value, "scope", &mut unmapped) {
                    Ok(scope) => config.scope = Some(scope),
                    Err(_) => unmapped.push("scope (unmapped_archive_format)".into()),
                }
            }
            "target" => {
                let StrictValue::Object(target) = value else {
                    unmapped.push("target (unmapped_archive_format)".into());
                    continue;
                };
                for (sub_key, sub_value) in target {
                    match sub_key.as_str() {
                        "scope" => {
                            scope_sources += 1;
                            match parse_scope(sub_value, "target.scope", &mut unmapped) {
                                Ok(scope) => config.scope = Some(scope),
                                Err(_) => {
                                    unmapped.push("target.scope (unmapped_archive_format)".into())
                                }
                            }
                        }
                        "crawl" | "audit" => skip_automation(&mut skipped),
                        _ => unmapped.push(format!("target.{sub_key}")),
                    }
                }
            }
            "match_and_replace_rules" => {
                match_replace_sources += 1;
                match parse_match_replace_list(value, "match_and_replace_rules", &mut unmapped) {
                    Ok(rules) => config.match_and_replace_rules = Some(rules),
                    Err(_) => {
                        unmapped.push("match_and_replace_rules (unmapped_archive_format)".into())
                    }
                }
            }
            "proxy" => {
                let StrictValue::Object(proxy) = value else {
                    unmapped.push("proxy (unmapped_archive_format)".into());
                    continue;
                };
                for (sub_key, sub_value) in proxy {
                    match sub_key.as_str() {
                        "request_listeners" => {
                            if !matches!(sub_value, StrictValue::Array(items) if items.is_empty()) {
                                skip_listeners(&mut skipped);
                            }
                        }
                        "match_replace_rules" | "match_and_replace_rules" => {
                            match_replace_sources += 1;
                            let path = format!("proxy.{sub_key}");
                            match parse_match_replace_list(sub_value, &path, &mut unmapped) {
                                Ok(rules) => config.match_and_replace_rules = Some(rules),
                                Err(_) => unmapped.push(format!("{path} (unmapped_archive_format)")),
                            }
                        }
                        _ => unmapped.push(format!("proxy.{sub_key}")),
                    }
                }
            }
            "repeater" | "repeater_settings" => {
                repeater_sources += 1;
                match parse_repeater_patch(value, key, &mut unmapped) {
                    Ok(patch) => config.repeater = Some(patch),
                    Err(_) => unmapped.push(format!("{key} (unmapped_archive_format)")),
                }
            }
            "sessions" => {
                session_sources += 1;
                match parse_sessions(value, "sessions") {
                    Ok(sessions) => config.sessions = Some(sessions),
                    Err(_) => unmapped.push("sessions (unmapped_archive_format)".into()),
                }
            }
            "project_options" => {
                let StrictValue::Object(options) = value else {
                    unmapped.push("project_options (unmapped_archive_format)".into());
                    continue;
                };
                for (sub_key, sub_value) in options {
                    match sub_key.as_str() {
                        "repeater" => {
                            repeater_sources += 1;
                            match parse_repeater_patch(
                                sub_value,
                                "project_options.repeater",
                                &mut unmapped,
                            ) {
                                Ok(patch) => config.repeater = Some(patch),
                                Err(_) => unmapped
                                    .push("project_options.repeater (unmapped_archive_format)".into()),
                            }
                        }
                        "sessions" => {
                            session_sources += 1;
                            match parse_sessions(sub_value, "project_options.sessions") {
                                Ok(sessions) => config.sessions = Some(sessions),
                                Err(_) => unmapped
                                    .push("project_options.sessions (unmapped_archive_format)".into()),
                            }
                        }
                        "extender" | "extensions" => skip_plugins(&mut skipped),
                        "scanner" | "collaborator" => skip_automation(&mut skipped),
                        _ => unmapped.push(format!("project_options.{sub_key}")),
                    }
                }
            }
            _ => unmapped.push(key.clone()),
        }
    }
    ensure!(scope_sources <= 1, "项目配置包含多个冲突的 Scope 定义");
    ensure!(
        match_replace_sources <= 1,
        "项目配置包含多个冲突的 Match & Replace 定义"
    );
    ensure!(
        repeater_sources <= 1,
        "项目配置包含多个冲突的 Repeater 定义"
    );
    ensure!(session_sources <= 1, "项目配置包含多个冲突的 Sessions 定义");
    Ok((config, skipped, unmapped))
}

fn skip_once(skipped: &mut Vec<String>, label: &str) {
    if !skipped.iter().any(|existing| existing == label) {
        skipped.push(label.to_owned());
    }
}

fn skip_listeners(skipped: &mut Vec<String>) {
    skip_once(
        skipped,
        "proxy_listeners（自动跳过源项目监听端口以避免本机端口冲突）",
    );
}

fn skip_plugins(skipped: &mut Vec<String>) {
    skip_once(
        skipped,
        "extender/plugins（安全策略禁止从项目配置启用外部插件）",
    );
}

fn skip_automation(skipped: &mut Vec<String>) {
    skip_once(
        skipped,
        "scanner/automated_tasks（安全策略禁止在导入时启用自动扫描或后台任务）",
    );
}

fn parse_scope(value: &StrictValue, path: &str, unmapped: &mut Vec<String>) -> Result<TargetScope> {
    let StrictValue::Object(map) = value else {
        bail!("`{path}` 必须是 JSON 对象");
    };
    let mut advanced_mode = false;
    if let Some(mode) = map.get("advanced_mode") {
        let StrictValue::Bool(flag) = mode else {
            bail!("`{path}.advanced_mode` 必须是布尔值");
        };
        advanced_mode = *flag;
    }
    let mut scope = TargetScope {
        advanced_mode,
        ..TargetScope::default()
    };
    for (key, entry) in map {
        match key.as_str() {
            "advanced_mode" => {}
            "include" => parse_scope_entries(
                entry,
                &format!("{path}.include"),
                advanced_mode && !map.contains_key("advanced_include"),
                &mut scope.include,
                &mut scope.advanced_include,
                unmapped,
            )?,
            "exclude" => parse_scope_entries(
                entry,
                &format!("{path}.exclude"),
                advanced_mode && !map.contains_key("advanced_exclude"),
                &mut scope.exclude,
                &mut scope.advanced_exclude,
                unmapped,
            )?,
            "advanced_include" => parse_scope_entries(
                entry,
                &format!("{path}.advanced_include"),
                true,
                &mut scope.include,
                &mut scope.advanced_include,
                unmapped,
            )?,
            "advanced_exclude" => parse_scope_entries(
                entry,
                &format!("{path}.advanced_exclude"),
                true,
                &mut scope.exclude,
                &mut scope.advanced_exclude,
                unmapped,
            )?,
            _ => unmapped.push(format!("{path}.{key}")),
        }
    }
    scope.validate()?;
    Ok(scope)
}

fn parse_scope_entries(
    value: &StrictValue,
    path: &str,
    as_advanced: bool,
    simple_out: &mut Vec<ScopeRule>,
    advanced_out: &mut Vec<AdvancedScopeRule>,
    unmapped: &mut Vec<String>,
) -> Result<()> {
    let StrictValue::Array(items) = value else {
        unmapped.push(format!("{path} (unmapped_archive_format)"));
        return Ok(());
    };
    for (index, item) in items.iter().enumerate() {
        let entry_path = format!("{path}[{index}]");
        let parsed = parse_single_scope_entry(item, &entry_path, as_advanced);
        match parsed {
            Ok(ParsedScopeEntry::Simple(rule)) => simple_out.push(rule),
            Ok(ParsedScopeEntry::Advanced(rule)) => advanced_out.push(rule),
            Err(_) => unmapped.push(format!("{entry_path} (unmapped_archive_format)")),
        }
    }
    Ok(())
}

enum ParsedScopeEntry {
    Simple(ScopeRule),
    Advanced(AdvancedScopeRule),
}

fn parse_single_scope_entry(
    item: &StrictValue,
    entry_path: &str,
    as_advanced: bool,
) -> Result<ParsedScopeEntry> {
    let StrictValue::Object(rule) = item else {
        bail!("`{entry_path}` 必须是对象");
    };
    let id = optional_uuid(rule.get("id"), &format!("{entry_path}.id"))?;
    let enabled = match rule.get("enabled") {
        Some(StrictValue::Bool(flag)) => *flag,
        None => true,
        _ => bail!("`{entry_path}.enabled` 必须是布尔值"),
    };
    if !as_advanced && rule.contains_key("prefix") {
        let Some(StrictValue::String(prefix)) = rule.get("prefix") else {
            bail!("`{entry_path}.prefix` 必须是字符串");
        };
        let include_subdomains = match rule.get("include_subdomains") {
            Some(StrictValue::Bool(flag)) => *flag,
            None => false,
            _ => bail!("`{entry_path}.include_subdomains` 必须是布尔值"),
        };
        let candidate = ScopeRule {
            id,
            enabled,
            prefix: prefix.clone(),
            include_subdomains,
        };
        TargetScope {
            advanced_mode: false,
            include: vec![candidate.clone()],
            ..TargetScope::default()
        }
        .validate()?;
        Ok(ParsedScopeEntry::Simple(candidate))
    } else if as_advanced {
        let protocol = match rule.get("protocol") {
            None => ScopeProtocol::Any,
            Some(StrictValue::String(proto)) => match proto.trim().to_ascii_lowercase().as_str() {
                "any" => ScopeProtocol::Any,
                "http" => ScopeProtocol::Http,
                "https" => ScopeProtocol::Https,
                other => bail!("`{entry_path}.protocol` 不支持 `{other}`"),
            },
            _ => bail!("`{entry_path}.protocol` 必须是字符串"),
        };
        let candidate = AdvancedScopeRule {
            id,
            enabled,
            protocol,
            host: optional_string(rule.get("host"), &format!("{entry_path}.host"))?,
            port: optional_string(rule.get("port"), &format!("{entry_path}.port"))?,
            file: optional_string(rule.get("file"), &format!("{entry_path}.file"))?,
        };
        TargetScope {
            advanced_mode: true,
            advanced_include: vec![candidate.clone()],
            ..TargetScope::default()
        }
        .validate()?;
        Ok(ParsedScopeEntry::Advanced(candidate))
    } else {
        bail!("`{entry_path}` 缺少 `prefix` 字段");
    }
}
