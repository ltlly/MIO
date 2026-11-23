use std::collections::HashSet;

use anyhow::{Context, Result, ensure};

use crate::*;

pub fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

pub fn valid_cookie_value(value: &str) -> bool {
    value.len() <= 8192 && !value.bytes().any(|b| matches!(b, 0..=0x1f | 0x7f | b';'))
}

pub fn valid_macro_step_url(raw: &str) -> bool {
    url::Url::parse(raw)
        .map(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
        .unwrap_or(false)
}

pub(crate) fn valid_header_name(name: &str) -> bool {
    name.len() <= 256 && crate::http::token(name.as_bytes())
}

pub(crate) fn valid_header_value(value: &str) -> bool {
    value.len() <= 16 * 1024 && !value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f)
}

pub fn normalize_cookie_domain(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(&trimmed);
    if let Ok(std::net::IpAddr::V6(v6)) = unbracketed.parse::<std::net::IpAddr>() {
        return Ok(v6.to_string());
    }
    ensure!(
        !trimmed.is_empty()
            && trimmed.len() <= 253
            && !trimmed.contains("://")
            && !trimmed.contains(['/', '?', '#', '@', ':', ' '])
            && !trimmed.chars().any(char::is_control),
        "Cookie 域名格式无效：{raw}"
    );
    Ok(trimmed)
}

impl CookieJarEntry {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.id.is_nil(), "Cookie Jar 条目 ID 不能为空");
        let normalized = normalize_cookie_domain(&self.domain)?;
        ensure!(
            self.domain == normalized,
            "Cookie 域名须为规范化小写主机名（不含前导点）：{}",
            self.domain
        );
        if self.domain.parse::<std::net::IpAddr>().is_ok() {
            ensure!(self.host_only, "IP 字面量 Cookie 必须为 host_only");
        }
        ensure!(
            self.path.starts_with('/')
                && self.path.len() <= 2048
                && !self.path.contains(['\r', '\n', '\0', ';'])
                && !self.path.chars().any(char::is_control),
            "Cookie Path 必须以 '/' 开头且不包含控制字符或分号"
        );
        ensure!(
            valid_cookie_name(&self.name),
            "Cookie 名称无效：{}",
            self.name
        );
        ensure!(
            valid_cookie_value(&self.value),
            "Cookie 值包含非法控制字符、分号或超过长度上限"
        );
        Ok(())
    }
}

impl SessionSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.cookie_jar.entries.len() <= MAX_COOKIE_JAR_ENTRIES,
            "Cookie Jar 条目数量超过 {MAX_COOKIE_JAR_ENTRIES} 上限"
        );
        let mut cookie_ids = HashSet::new();
        let mut cookie_keys = HashSet::new();
        for entry in &self.cookie_jar.entries {
            entry.validate()?;
            ensure!(cookie_ids.insert(entry.id), "Cookie Jar 条目 ID 重复");
            ensure!(
                cookie_keys.insert((
                    entry.domain.as_str(),
                    entry.host_only,
                    entry.path.as_str(),
                    entry.name.as_str(),
                )),
                "Cookie Jar 存在同作用域重复条目：{} ({}{})",
                entry.name,
                entry.domain,
                entry.path
            );
        }
        ensure!(
            self.macros.len() <= MAX_SESSION_MACROS,
            "Session Macro 数量超过 {MAX_SESSION_MACROS} 上限"
        );
        let mut macro_ids = HashSet::new();
        for mac in &self.macros {
            validate_macro(mac)?;
            ensure!(macro_ids.insert(mac.id), "Session Macro ID 重复");
        }
        ensure!(
            self.rules.len() <= MAX_SESSION_RULES,
            "Session handling rules 数量超过 {MAX_SESSION_RULES} 上限"
        );
        let mut rule_ids = HashSet::new();
        for rule in &self.rules {
            validate_rule(rule, &macro_ids)?;
            ensure!(rule_ids.insert(rule.id), "Session 规则 ID 重复");
        }
        Ok(())
    }
}

fn validate_macro(mac: &SessionMacro) -> Result<()> {
    ensure!(!mac.id.is_nil(), "Macro ID 不能为空");
    let name = mac.name.trim();
    ensure!(
        !name.is_empty() && mac.name.len() <= 256 && !mac.name.chars().any(char::is_control),
        "Macro 名称不能为空、包含控制字符或超过 256 字节"
    );
    ensure!(
        !mac.steps.is_empty() && mac.steps.len() <= MAX_MACRO_STEPS,
        "Macro 必须包含 1 到 {MAX_MACRO_STEPS} 个步骤"
    );
    ensure!(
        (100..=60_000).contains(&mac.timeout_ms),
        "Macro 超时必须在 100 到 60000 毫秒之间"
    );
    if let MacroFailurePolicy::Retry { max_retries } = mac.failure_policy {
        ensure!(
            (1..=3).contains(&max_retries),
            "Macro 重试次数必须在 1 到 3 次之间"
        );
    }
    let mut step_ids = HashSet::new();
    for step in &mac.steps {
        ensure!(
            !step.id.is_nil() && step_ids.insert(step.id),
            "Macro 步骤 ID 无效或重复"
        );
        ensure!(
            crate::http::token(step.method.as_bytes()) && step.method.len() <= 32,
            "Macro 步骤 HTTP 方法无效"
        );
        let parsed = url::Url::parse(&step.url).context("Macro 步骤 URL 格式无效")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
            "Macro 步骤 URL 必须为包含主机的 http 或 https 地址"
        );
        ensure!(step.headers.len() <= 128, "Macro 步骤请求头过多");
        for header in &step.headers {
            ensure!(valid_header_name(&header.name), "Macro 步骤请求头名称无效");
            ensure!(
                header.value.len() <= 16 * 1024
                    && !header
                        .value
                        .iter()
                        .any(|b| (*b < 0x20 && *b != b'\t') || *b == 0x7f),
                "Macro 步骤请求头包含非法控制字符或过长"
            );
        }
        ensure!(
            step.body.len() <= 1024 * 1024,
            "Macro 单步正文不能超过 1 MiB"
        );
        if let Some(status) = step.expected_status {
            ensure!(
                (100..=599).contains(&status),
                "Macro 预期状态码必须在 100 到 599 之间"
            );
        }
        ensure!(
            step.extractions.len() <= MAX_MACRO_EXTRACTIONS,
            "Macro 单步参数提取数量超过 {MAX_MACRO_EXTRACTIONS} 上限"
        );
        for ext in &step.extractions {
            validate_extraction(ext)?;
        }
    }
    Ok(())
}

fn validate_extraction(ext: &MacroExtraction) -> Result<()> {
    let name = ext.name.trim();
    ensure!(
        !name.is_empty() && ext.name.len() <= 256 && !ext.name.chars().any(char::is_control),
        "Macro 提取参数名称无效"
    );
    match &ext.source {
        MacroExtractSource::Delimiter { start, end } => {
            ensure!(
                !start.is_empty() && start.len() <= 1024 && !end.is_empty() && end.len() <= 1024,
                "Macro 分隔符提取的起止边界不能为空且不能超过 1024 字节"
            );
        }
        MacroExtractSource::Regex { pattern } => {
            ensure!(
                !pattern.is_empty() && pattern.len() <= 2048,
                "Macro 正则表达式不能为空或超过 2048 字节"
            );
            regex::RegexBuilder::new(pattern)
                .size_limit(512 * 1024)
                .build()
                .map_err(|e| anyhow::anyhow!("Macro 提取正则表达式无效：{e}"))?;
        }
        MacroExtractSource::ResponseHeader { header_name } => {
            ensure!(valid_header_name(header_name), "Macro 提取响应头名称无效");
        }
    }
    match &ext.target {
        MacroApplyTarget::UrlOrBodyParam => {}
        MacroApplyTarget::Header { header_name } => {
            let target_name = if header_name.is_empty() {
                &ext.name
            } else {
                header_name
            };
            ensure!(
                valid_header_name(target_name),
                "Macro 回填目标请求头名称无效"
            );
        }
        MacroApplyTarget::Cookie { cookie_name } => {
            let target_name = if cookie_name.is_empty() {
                &ext.name
            } else {
                cookie_name
            };
            ensure!(
                valid_cookie_name(target_name),
                "Macro 回填目标 Cookie 名称无效"
            );
        }
    }
    Ok(())
}

fn validate_rule(rule: &SessionRule, macro_ids: &HashSet<Uuid>) -> Result<()> {
    ensure!(!rule.id.is_nil(), "Session 规则 ID 不能为空");
    let name = rule.name.trim();
    ensure!(
        !name.is_empty() && rule.name.len() <= 256 && !rule.name.chars().any(char::is_control),
        "Session 规则名称不能为空、包含控制字符或超过 256 字节"
    );
    if let SessionUrlScope::CustomPrefixes(prefixes) = &rule.condition.url_scope {
        ensure!(
            !prefixes.is_empty() && prefixes.len() <= 64,
            "自定义 URL 前缀条件须包含 1 到 64 条前缀"
        );
        for prefix in prefixes {
            ensure!(
                !prefix.trim().is_empty()
                    && prefix.len() <= 2048
                    && !prefix.chars().any(char::is_control),
                "Session 规则 URL 前缀无效"
            );
        }
    }
    if let Some(cond) = &rule.condition.param_condition {
        let names = match cond {
            SessionParamCondition::RequirePresent(names)
            | SessionParamCondition::RequireAbsent(names) => names,
        };
        ensure!(
            !names.is_empty()
                && names.len() <= 32
                && names.iter().all(|n| !n.trim().is_empty()
                    && n.len() <= 256
                    && !n.chars().any(char::is_control)),
            "Session 规则参数条件名称无效"
        );
    }
    if let Some(header_sub) = &rule.condition.header_contains {
        ensure!(
            header_sub.len() <= 1024 && !header_sub.contains(['\r', '\n', '\0']),
            "Session 规则请求头匹配条件无效"
        );
    }
    ensure!(
        !rule.actions.is_empty() && rule.actions.len() <= MAX_RULE_ACTIONS,
        "Session 规则必须包含 1 到 {MAX_RULE_ACTIONS} 个动作"
    );
    for action in &rule.actions {
        match action {
            SessionRuleAction::UseCookieJar {
                add_new_cookies,
                update_existing_cookies,
            } => {
                ensure!(
                    *add_new_cookies || *update_existing_cookies,
                    "Use Cookie Jar 动作至少须启用更新已有或添加新 Cookie"
                );
            }
            SessionRuleAction::SetHeader { name, value, .. } => {
                ensure!(valid_header_name(name), "SetHeader 动作请求头名称无效");
                ensure!(
                    valid_header_value(value),
                    "SetHeader 动作请求头值包含非法控制字符或超过上限"
                );
            }
            SessionRuleAction::SetCookie { name, value, .. } => {
                ensure!(valid_cookie_name(name), "SetCookie 动作 Cookie 名称无效");
                ensure!(
                    valid_cookie_value(value),
                    "SetCookie 动作 Cookie 值包含非法控制字符、分号或超过上限"
                );
            }
            SessionRuleAction::RewriteUrlParam {
                name,
                value,
                remove,
                ..
            } => {
                ensure!(
                    !name.trim().is_empty()
                        && name.len() <= 512
                        && !name.contains(['\r', '\n', '\0', '&', '=', '#']),
                    "RewriteUrlParam 参数名称无效"
                );
                if !*remove {
                    ensure!(
                        value.len() <= 4096 && !value.contains(['\r', '\n', '\0', '#']),
                        "RewriteUrlParam 参数值包含非法字符或过长"
                    );
                }
            }
            SessionRuleAction::ReplaceRequestText {
                match_pattern,
                replace_with,
                regex,
            } => {
                ensure!(
                    !match_pattern.is_empty()
                        && match_pattern.len() <= 4096
                        && replace_with.len() <= 4096
                        && !replace_with.contains(['\r', '\n', '\0']),
                    "ReplaceRequestText 动作匹配模式不能为空，且不能超过 4096 字节或包含非法换行/NUL"
                );
                if *regex {
                    regex::RegexBuilder::new(match_pattern)
                        .size_limit(1024 * 1024)
                        .build()
                        .map_err(|e| anyhow::anyhow!("ReplaceRequestText 正则表达式无效：{e}"))?;
                }
            }
            SessionRuleAction::RunMacro { macro_id, .. } => {
                ensure!(
                    macro_ids.contains(macro_id),
                    "Session 规则引用的 Macro ({macro_id}) 不存在"
                );
            }
        }
    }
    Ok(())
}
