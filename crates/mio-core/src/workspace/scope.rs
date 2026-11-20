use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use super::*;

const SCOPE_REGEX_CACHE_LIMIT: usize = 256;

fn cached_scope_regex(pattern: &str) -> Option<regex::Regex> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<regex::Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = guard.get(pattern) {
            return cached.clone();
        }
    }
    let compiled = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(256 * 1024)
        .build()
        .ok();
    {
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.len() >= SCOPE_REGEX_CACHE_LIMIT && !guard.contains_key(pattern) {
            guard.clear();
        }
        guard.insert(pattern.to_owned(), compiled.clone());
    }
    compiled
}

impl ScopeRule {
    pub fn matches_url(&self, url: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let pattern = self.prefix.trim();
        if pattern.is_empty() {
            return false;
        }
        let Ok(parsed) = url::Url::parse(url) else {
            return url.starts_with(pattern);
        };
        if pattern.contains("://") {
            if let Ok(pat_url) = url::Url::parse(pattern) {
                if !parsed.scheme().eq_ignore_ascii_case(pat_url.scheme()) {
                    return false;
                }
                let parsed_host = parsed.host_str().map(str::to_ascii_lowercase);
                let pat_host = pat_url.host_str().map(str::to_ascii_lowercase);
                let host_matches = match (parsed_host.as_deref(), pat_host.as_deref()) {
                    (Some(host), Some(base)) if self.include_subdomains => {
                        let base = base.trim_start_matches('.');
                        host == base
                            || (base.parse::<std::net::IpAddr>().is_err()
                                && host.len() > base.len() + 1
                                && host.ends_with(base)
                                && host.as_bytes()[host.len() - base.len() - 1] == b'.')
                    }
                    (a, b) => a == b,
                };
                if !host_matches {
                    return false;
                }
                let parsed_port = parsed.port_or_known_default();
                if let Some(pat_port) = pat_url.port() {
                    if parsed_port != Some(pat_port) {
                        return false;
                    }
                } else if self.include_subdomains
                    && pat_url.port_or_known_default().is_some()
                    && parsed_port != pat_url.port_or_known_default()
                {
                    return false;
                }
                let pat_path = pat_url.path();
                return pat_path == "/" || parsed.path().starts_with(pat_path);
            }
            return url
                .to_ascii_lowercase()
                .starts_with(&pattern.to_ascii_lowercase());
        }
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let host_lower = host.to_ascii_lowercase();
        let pat_lower = pattern.trim_start_matches('.').to_ascii_lowercase();
        if let Some((pat_host, pat_port_str)) = pat_lower.rsplit_once(':')
            && let Ok(pat_port) = pat_port_str.parse::<u16>()
        {
            if parsed.port_or_known_default() != Some(pat_port) {
                return false;
            }
            return host_lower == pat_host || host_lower.ends_with(&format!(".{pat_host}"));
        }
        host_lower == pat_lower || host_lower.ends_with(&format!(".{pat_lower}"))
    }
}

impl TargetScope {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.include.len() + self.exclude.len() <= 256,
            "Target Scope 规则数量超过 256 条上限"
        );
        let mut ids = HashSet::new();
        for rule in self.include.iter().chain(&self.exclude) {
            ensure!(
                !rule.id.is_nil() && ids.insert(rule.id),
                "Scope 规则 ID 无效或重复"
            );
            let trimmed = rule.prefix.trim();
            ensure!(
                !trimmed.is_empty()
                    && rule.prefix.len() <= 2048
                    && !rule.prefix.chars().any(char::is_control),
                "Scope 规则前缀不能为空、包含控制字符或超过 2048 字节"
            );
        }
        ensure!(
            self.advanced_include.len() + self.advanced_exclude.len() <= 256,
            "Target Scope 高级规则数量超过 256 条上限"
        );
        for rule in self.advanced_include.iter().chain(&self.advanced_exclude) {
            ensure!(
                !rule.id.is_nil() && ids.insert(rule.id),
                "Scope 高级规则 ID 无效或重复"
            );
            ensure!(
                rule.host.len() <= 1024
                    && rule.port.len() <= 128
                    && rule.file.len() <= 2048
                    && !rule
                        .host
                        .chars()
                        .chain(rule.port.chars())
                        .chain(rule.file.chars())
                        .any(char::is_control),
                "Scope 高级规则字段超过长度上限"
            );
            ensure!(
                !rule.host.trim().is_empty()
                    || !rule.port.trim().is_empty()
                    || !rule.file.trim().is_empty()
                    || rule.protocol != ScopeProtocol::Any,
                "高级 Scope 规则不能所有匹配条件均为空"
            );
            for (label, pat) in [
                ("Host", rule.host.trim()),
                ("Port", rule.port.trim()),
                ("File", rule.file.trim()),
            ] {
                if !pat.is_empty() {
                    regex::RegexBuilder::new(pat)
                        .case_insensitive(true)
                        .size_limit(256 * 1024)
                        .build()
                        .with_context(|| format!("高级 Scope {label} 正则表达式无效"))?;
                }
            }
        }
        ensure!(
            self.out_of_scope_custom_rules.len() <= 256,
            "Target Scope 自定义范围外规则数量超过 256 条上限"
        );
        for rule in &self.out_of_scope_custom_rules {
            ensure!(
                !rule.id.is_nil() && ids.insert(rule.id),
                "Scope 自定义范围外规则 ID 无效或重复"
            );
            let trimmed = rule.prefix.trim();
            ensure!(
                !trimmed.is_empty()
                    && rule.prefix.len() <= 2048
                    && !rule.prefix.chars().any(char::is_control),
                "Scope 自定义范围外规则前缀不能为空、包含控制字符或超过 2048 字节"
            );
        }
        Ok(())
    }

    pub fn has_enabled_includes(&self) -> bool {
        if self.advanced_mode {
            self.advanced_include.iter().any(|r| r.enabled)
        } else {
            self.include.iter().any(|r| r.enabled)
        }
    }

    pub fn is_url_in_scope(&self, url: &str) -> bool {
        if self.advanced_mode {
            return self
                .advanced_include
                .iter()
                .any(|rule| rule.matches_url(url))
                && !self
                    .advanced_exclude
                    .iter()
                    .any(|rule| rule.matches_url(url));
        }
        if !self.include.iter().any(|rule| rule.matches_url(url)) {
            return false;
        }
        !self.exclude.iter().any(|rule| rule.matches_url(url))
    }

    pub fn should_drop_url(&self, url: &str) -> bool {
        if !self.drop_out_of_scope {
            return false;
        }
        if self.out_of_scope_use_custom {
            !self
                .out_of_scope_custom_rules
                .iter()
                .any(|rule| rule.matches_url(url))
        } else {
            !self.is_url_in_scope(url)
        }
    }
}

impl AdvancedScopeRule {
    pub fn split_url_patterns(url: &str) -> (ScopeProtocol, String, String, String) {
        let url = url.trim();
        let mut protocol = ScopeProtocol::Any;
        let mut host = String::new();
        let mut port = String::new();
        let mut path = String::new();
        if let Ok(parsed) = url::Url::parse(url) {
            protocol = if parsed.scheme().eq_ignore_ascii_case("https") {
                ScopeProtocol::Https
            } else if parsed.scheme().eq_ignore_ascii_case("http") {
                ScopeProtocol::Http
            } else {
                ScopeProtocol::Any
            };
            if let Some(h) = parsed.host_str() {
                host = regex::escape(h);
            }
            if let Some(p) = parsed.port_or_known_default() {
                port = p.to_string();
            }
            let parsed_path = parsed.path();
            if !parsed_path.is_empty() && parsed_path != "/" {
                path = format!("^{}", regex::escape(parsed_path));
            }
        } else if let Some((scheme_part, rest)) = url.split_once("://") {
            protocol = if scheme_part.eq_ignore_ascii_case("https") {
                ScopeProtocol::Https
            } else if scheme_part.eq_ignore_ascii_case("http") {
                ScopeProtocol::Http
            } else {
                ScopeProtocol::Any
            };
            let (authority, remainder) = rest.split_once(['/', '?', '#']).unwrap_or((rest, ""));
            if let Some((authority_host, authority_port)) = authority.rsplit_once(':') {
                host = regex::escape(authority_host);
                port = regex::escape(authority_port);
            } else {
                host = regex::escape(authority);
            }
            path = if remainder.is_empty() {
                String::new()
            } else {
                format!("^/{}", regex::escape(remainder.trim_start_matches('/')))
            };
        }
        (
            protocol,
            if host.is_empty() {
                String::new()
            } else {
                format!("^{host}$")
            },
            if port.is_empty() {
                String::new()
            } else {
                format!("^{port}$")
            },
            path,
        )
    }

    fn matches_protocol(&self, scheme: &str) -> bool {
        match self.protocol {
            ScopeProtocol::Any => true,
            ScopeProtocol::Http => scheme.eq_ignore_ascii_case("http"),
            ScopeProtocol::Https => scheme.eq_ignore_ascii_case("https"),
        }
    }

    pub fn matches_url(&self, url: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        if !self.matches_protocol(parsed.scheme()) {
            return false;
        }
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let host_pat = self.host.trim();
        if !host_pat.is_empty() {
            let Some(re) = cached_scope_regex(host_pat) else {
                return false;
            };
            if !re.is_match(host) {
                return false;
            }
        }
        let port_pattern = self.port.trim();
        if !port_pattern.is_empty() {
            let Some(port) = parsed.port_or_known_default() else {
                return false;
            };
            let Some(re) = cached_scope_regex(port_pattern) else {
                return false;
            };
            if !re.is_match(&port.to_string()) {
                return false;
            }
        }
        let file_pattern = self.file.trim();
        if file_pattern.is_empty() {
            return true;
        }
        let Some(re) = cached_scope_regex(file_pattern) else {
            return false;
        };
        match parsed.query() {
            Some(q) => {
                let mut path_and_query = String::with_capacity(parsed.path().len() + 1 + q.len());
                path_and_query.push_str(parsed.path());
                path_and_query.push('?');
                path_and_query.push_str(q);
                re.is_match(&path_and_query)
            }
            None => re.is_match(parsed.path()),
        }
    }
}
