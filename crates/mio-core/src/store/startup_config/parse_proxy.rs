use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use super::parse::deserialize_present;
use crate::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawExtSslPassThroughRule {
    #[serde(default)]
    id: Option<Uuid>,
    enabled: bool,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    host_pattern: Option<String>,
    #[serde(default)]
    port: Option<StringOrU16>,
    #[serde(default)]
    is_regex: Option<bool>,
    #[serde(default)]
    is_simple_match: Option<bool>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    file: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrU16 {
    Num(u16),
    Text(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSslPassThroughSection {
    #[serde(default, deserialize_with = "deserialize_present")]
    automatically_add_entries_on_client_ssl_negotiation_failure: Option<bool>,
    rules: Vec<RawExtSslPassThroughRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawExtWsMatchReplaceRule {
    #[serde(default)]
    id: Option<Uuid>,
    enabled: bool,
    #[serde(default)]
    direction: Option<crate::WsReplaceDirection>,
    #[serde(default)]
    rule_type: Option<String>,
    #[serde(default)]
    match_pattern: Option<String>,
    #[serde(default)]
    string_match: Option<String>,
    #[serde(default)]
    replace_with: Option<String>,
    #[serde(default)]
    string_replace: Option<String>,
    #[serde(default)]
    is_regex: Option<bool>,
    #[serde(default)]
    is_simple_match: Option<bool>,
    #[serde(default)]
    comment: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawProxyConfig {
    #[serde(default)]
    pub(super) request_listeners: [serde::de::IgnoredAny; 0],
    #[serde(default, deserialize_with = "deserialize_present")]
    pub(super) intercept_web_sockets_messages: Option<crate::ProxyWebSocketInterceptionSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub(super) ssl_pass_through: Option<RawSslPassThroughSection>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub(super) ws_match_replace_rules: Option<Vec<RawExtWsMatchReplaceRule>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub(super) response_modification: Option<crate::ProxyResponseModificationSettings>,
}

pub(super) type ParsedProxyExtras = (
    Option<crate::ProxyWebSocketInterceptionSettings>,
    Option<bool>,
    Option<Vec<crate::SslPassThroughRule>>,
    Option<Vec<crate::WsMatchAndReplaceRule>>,
    Option<crate::ProxyResponseModificationSettings>,
);

pub(super) fn parse_proxy_section(proxy: Option<RawProxyConfig>) -> Result<ParsedProxyExtras> {
    let Some(proxy) = proxy else {
        return Ok((None, None, None, None, None));
    };
    let (ssl_auto_add, ssl) = match proxy.ssl_pass_through {
        Some(section) => {
            let converted = convert_ssl_rules(section.rules)?;
            crate::http::validate_ssl_and_ws_rules(&converted, &[])?;
            (
                section.automatically_add_entries_on_client_ssl_negotiation_failure,
                Some(converted),
            )
        }
        None => (None, None),
    };
    let ws = match proxy.ws_match_replace_rules {
        Some(rules) => {
            let converted = convert_ws_rules(rules)?;
            crate::http::validate_ssl_and_ws_rules(&[], &converted)?;
            Some(converted)
        }
        None => None,
    };
    Ok((
        proxy.intercept_web_sockets_messages,
        ssl_auto_add,
        ssl,
        ws,
        proxy.response_modification,
    ))
}

fn convert_ssl_rules(
    rules: Vec<RawExtSslPassThroughRule>,
) -> Result<Vec<crate::SslPassThroughRule>> {
    let mut out = Vec::with_capacity(rules.len());
    for (idx, raw) in rules.into_iter().enumerate() {
        if let Some(proto) = &raw.protocol {
            ensure!(
                proto.is_empty()
                    || proto.eq_ignore_ascii_case("any")
                    || proto.eq_ignore_ascii_case("https"),
                "proxy.ssl_pass_through.rules[{idx}] 仅支持 https/any 协议"
            );
        }
        ensure!(
            raw.file.as_deref().is_none_or(str::is_empty),
            "proxy.ssl_pass_through.rules[{idx}] 不支持非空路径级过滤（CONNECT 仅含 host:port）"
        );
        ensure!(
            !(raw.host.is_some() && raw.host_pattern.is_some()),
            "proxy.ssl_pass_through.rules[{idx}] 不能同时指定 host 与 host_pattern"
        );
        let host_pattern = raw
            .host_pattern
            .or(raw.host)
            .context("proxy.ssl_pass_through.rules 缺少 host 或 host_pattern")?;
        let port = match raw.port {
            None => None,
            Some(StringOrU16::Num(p)) => {
                ensure!(p > 0, "SSL Pass Through 端口必须大于 0");
                Some(p)
            }
            Some(StringOrU16::Text(s)) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let parsed: u16 = trimmed
                        .trim_start_matches('^')
                        .trim_end_matches('$')
                        .parse()
                        .context("SSL Pass Through 端口必须是有效数字端口")?;
                    ensure!(parsed > 0, "SSL Pass Through 端口必须大于 0");
                    Some(parsed)
                }
            }
        };
        ensure!(
            !(raw.is_regex.is_some() && raw.is_simple_match.is_some()),
            "proxy.ssl_pass_through.rules[{idx}] 不能同时指定 is_regex 与 is_simple_match"
        );
        let is_regex = raw
            .is_regex
            .or_else(|| raw.is_simple_match.map(|simple| !simple))
            .unwrap_or(false);
        let id = raw.id.unwrap_or_else(|| {
            Uuid::from_u128(0x58e9f964_060e_4afc_9220_000000000001 + idx as u128)
        });
        out.push(crate::SslPassThroughRule {
            id,
            enabled: raw.enabled,
            host_pattern,
            port,
            is_regex,
        });
    }
    Ok(out)
}

fn convert_ws_rules(
    rules: Vec<RawExtWsMatchReplaceRule>,
) -> Result<Vec<crate::WsMatchAndReplaceRule>> {
    let mut out = Vec::with_capacity(rules.len());
    for (idx, raw) in rules.into_iter().enumerate() {
        ensure!(
            !(raw.direction.is_some() && raw.rule_type.is_some()),
            "proxy.ws_match_replace_rules[{idx}] 不能同时指定 direction 与 rule_type"
        );
        let direction = if let Some(dir) = raw.direction {
            dir
        } else if let Some(rt) = raw.rule_type.as_deref() {
            match rt {
                "client_to_server" | "Client to server" => {
                    crate::WsReplaceDirection::ClientToServer
                }
                "server_to_client" | "Server to client" => {
                    crate::WsReplaceDirection::ServerToClient
                }
                "both" | "Both" | "any" => crate::WsReplaceDirection::Both,
                _ => anyhow::bail!("不支持的 WebSocket 替换方向：{rt}"),
            }
        } else {
            crate::WsReplaceDirection::Both
        };
        ensure!(
            !(raw.match_pattern.is_some() && raw.string_match.is_some()),
            "proxy.ws_match_replace_rules[{idx}] 不能同时指定 match_pattern 与 string_match"
        );
        ensure!(
            !(raw.replace_with.is_some() && raw.string_replace.is_some()),
            "proxy.ws_match_replace_rules[{idx}] 不能同时指定 replace_with 与 string_replace"
        );
        ensure!(
            !(raw.is_regex.is_some() && raw.is_simple_match.is_some()),
            "proxy.ws_match_replace_rules[{idx}] 不能同时指定 is_regex 与 is_simple_match"
        );
        let match_pattern = raw
            .match_pattern
            .or(raw.string_match)
            .context("proxy.ws_match_replace_rules 缺少 match_pattern 或 string_match")?;
        let replace_with = raw.replace_with.or(raw.string_replace).unwrap_or_default();
        let is_regex = raw
            .is_regex
            .or_else(|| raw.is_simple_match.map(|simple| !simple))
            .unwrap_or(false);
        let id = raw.id.unwrap_or_else(|| {
            Uuid::from_u128(0x58e9f964_060e_4afc_9221_000000000001 + idx as u128)
        });
        out.push(crate::WsMatchAndReplaceRule {
            id,
            enabled: raw.enabled,
            direction,
            match_pattern,
            replace_with,
            is_regex,
            comment: raw.comment,
        });
    }
    Ok(out)
}
