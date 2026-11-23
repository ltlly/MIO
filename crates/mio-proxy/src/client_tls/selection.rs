use super::*;
use std::net::{IpAddr, Ipv6Addr};

impl ClientCertificateRule {
    pub fn validate_host_pattern(pattern: &str) -> Result<String, ProxyError> {
        match parse_host_pattern(pattern)? {
            ParsedHostPattern::Any => Ok("*".into()),
            ParsedHostPattern::ExactIp(ip) => Ok(ip.to_string()),
            ParsedHostPattern::ExactDomain(domain) => Ok(domain),
            ParsedHostPattern::WildcardSubdomain(suffix) => Ok(format!("*.{suffix}")),
            ParsedHostPattern::Glob(glob) => Ok(glob),
        }
    }

    pub fn matches_host(&self, host: &str) -> bool {
        self.enabled && matches_host(&self.host_pattern, host)
    }

    pub fn parse_identity(&self) -> Result<ParsedClientIdentity, ProxyError> {
        ParsedClientIdentity::parse(self)
    }
}

/// Matches `host` against `pattern` (case-insensitive, strips `[]` around IPv6 literals).
///
/// Aligns with MIO Destination host wildcard semantics: `*.example.com` matches single- and
/// multi-label subdomains (`api.example.com`, `a.b.example.com`) and never matches the apex
/// `example.com` itself or IP literals. `*` matches any concrete host or IP, and glob patterns
/// (`10.0.*`, `api.*.example.invalid`) match concrete hosts/IPs via glob semantics.
pub fn matches_host(pattern: &str, host: &str) -> bool {
    let Ok(parsed_pattern) = parse_host_pattern(pattern) else {
        return false;
    };
    let Ok(parsed_host) = parse_host_pattern(host) else {
        return false;
    };
    let host_text = match &parsed_host {
        ParsedHostPattern::ExactIp(ip) => ip.to_string(),
        ParsedHostPattern::ExactDomain(dom) => dom.clone(),
        ParsedHostPattern::Any
        | ParsedHostPattern::WildcardSubdomain(_)
        | ParsedHostPattern::Glob(_) => return false,
    };
    match (parsed_pattern, parsed_host) {
        (ParsedHostPattern::Any, _) => true,
        (ParsedHostPattern::ExactIp(expected), ParsedHostPattern::ExactIp(actual)) => {
            expected == actual
        }
        (ParsedHostPattern::ExactDomain(expected), ParsedHostPattern::ExactDomain(actual)) => {
            expected == actual
        }
        (ParsedHostPattern::WildcardSubdomain(suffix), ParsedHostPattern::ExactDomain(actual)) => {
            actual.len() > suffix.len() + 1
                && actual.ends_with(&suffix)
                && actual.as_bytes()[actual.len() - suffix.len() - 1] == b'.'
        }
        (ParsedHostPattern::Glob(glob), _) => crate::route::matches_bypass_host(&glob, &host_text),
        _ => false,
    }
}

pub fn select_client_rule<'a>(
    rules: &'a [ClientCertificateRule],
    host: &str,
) -> Option<&'a ClientCertificateRule> {
    rules.iter().find(|rule| rule.matches_host(host))
}

pub fn select_client_identity<'a>(
    entries: &'a [(ClientCertificateRule, ParsedClientIdentity)],
    host: &str,
) -> Option<&'a ParsedClientIdentity> {
    entries
        .iter()
        .find(|(rule, _)| rule.matches_host(host))
        .map(|(_, identity)| identity)
}

pub fn resolve_client_identity(
    rules: &[ClientCertificateRule],
    host: &str,
) -> Result<Option<ParsedClientIdentity>, ProxyError> {
    select_client_rule(rules, host)
        .map(ParsedClientIdentity::parse)
        .transpose()
}

enum ParsedHostPattern {
    Any,
    ExactIp(IpAddr),
    ExactDomain(String),
    WildcardSubdomain(String),
    Glob(String),
}

fn is_valid_glob_host(raw: &str) -> bool {
    if raw.is_empty()
        || raw.len() > MAX_HOST_PATTERN_BYTES
        || raw.starts_with('.')
        || raw.ends_with('.')
        || raw.contains("..")
    {
        return false;
    }
    let colon_count = raw.bytes().filter(|&b| b == b':').count();
    if colon_count == 1 {
        return false;
    }
    if colon_count >= 2 {
        return raw
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'*' | b'?'));
    }
    let has_alnum = raw.bytes().any(|b| b.is_ascii_alphanumeric());
    has_alnum
        && raw.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'*' | b'?')
        })
}

fn parse_host_pattern(raw: &str) -> Result<ParsedHostPattern, ProxyError> {
    if raw.is_empty()
        || raw
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(ProxyError::invalid(
            "客户端证书主机模式不能为空或包含空白／控制字符",
        ));
    }
    if raw == "*" {
        return Ok(ParsedHostPattern::Any);
    }
    if raw.starts_with('[') || raw.ends_with(']') {
        let Some(inner) = raw
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        else {
            return Err(ProxyError::invalid("客户端证书 IPv6 主机括号不匹配"));
        };
        if inner.is_empty() || inner.len() > MAX_HOST_PATTERN_BYTES {
            return Err(ProxyError::invalid("客户端证书主机模式长度无效"));
        }
        if let Ok(ipv6) = inner.parse::<Ipv6Addr>() {
            return Ok(ParsedHostPattern::ExactIp(IpAddr::V6(ipv6)));
        }
        if (inner.contains('*') || inner.contains('?'))
            && inner.contains(':')
            && inner
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'*' | b'?'))
        {
            return Ok(ParsedHostPattern::Glob(inner.to_ascii_lowercase()));
        }
        return Err(ProxyError::invalid("客户端证书方括号主机须为有效 IPv6 字面量"));
    }
    if raw.contains('[') || raw.contains(']') || raw.len() > MAX_HOST_PATTERN_BYTES {
        return Err(ProxyError::invalid(
            "客户端证书主机模式格式无效或超过 253 字节上限",
        ));
    }
    if let Some(suffix) = raw.strip_prefix("*.") {
        if !suffix.contains('*') && !suffix.contains('?') {
            if suffix.parse::<IpAddr>().is_ok() || !is_valid_dns_name(suffix) {
                return Err(ProxyError::invalid(
                    "客户端证书通配符主机须为 *.domain 形式的合法域名",
                ));
            }
            return Ok(ParsedHostPattern::WildcardSubdomain(
                suffix.to_ascii_lowercase(),
            ));
        }
        if suffix.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'*' | b'?'))
        }) && suffix
            .split('.')
            .next_back()
            .is_some_and(|last| last.bytes().any(|b| b.is_ascii_alphabetic() || b == b'*'))
            && is_valid_glob_host(raw)
        {
            return Ok(ParsedHostPattern::Glob(raw.to_ascii_lowercase()));
        }
        return Err(ProxyError::invalid(
            "客户端证书通配符主机须为 *.domain 形式的合法域名",
        ));
    }
    if raw.contains('*') || raw.contains('?') {
        if is_valid_glob_host(raw) {
            return Ok(ParsedHostPattern::Glob(raw.to_ascii_lowercase()));
        }
        return Err(ProxyError::invalid(
            "客户端证书主机模式包含无效通配符或端口／URL 字符",
        ));
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Ok(ParsedHostPattern::ExactIp(ip));
    }
    if !raw.contains(':') && is_valid_dns_name(raw) {
        return Ok(ParsedHostPattern::ExactDomain(raw.to_ascii_lowercase()));
    }
    Err(ProxyError::invalid(
        "客户端证书主机模式须为纯主机名或 IP，不能包含端口或 URL",
    ))
}

fn is_valid_dns_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_HOST_PATTERN_BYTES
        || name.starts_with('.')
        || name.ends_with('.')
    {
        return false;
    }
    let mut last_label = "";
    for label in name.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return false;
        }
        last_label = label;
    }
    last_label.bytes().any(|byte| byte.is_ascii_alphabetic())
}
