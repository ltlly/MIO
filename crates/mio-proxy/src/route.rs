use std::fmt;

use mio_plugin_api::HttpVersion;
use rama::{
    net::{
        Protocol,
        address::{Host, HostWithPort, ProxyAddress},
        client::ProxyRoute,
        user::{Basic, ProxyCredential},
    },
    tls::client::TlsClientConfig,
};

use crate::{ProxyError, UpstreamTls, UpstreamTlsOptions};

#[derive(Clone)]
pub struct ProxyCredentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for ProxyCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProxyCredentials([redacted])")
    }
}

#[derive(Clone)]
pub struct ProxyEndpoint {
    /// A host name or IP literal, never a URL or userinfo.
    pub host: String,
    pub port: u16,
    pub credentials: Option<ProxyCredentials>,
}

impl fmt::Debug for ProxyEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Do not echo an unvalidated host that may contain pasted credentials.
        f.debug_struct("ProxyEndpoint")
            .field("port", &self.port)
            .field("authenticated", &self.credentials.is_some())
            .finish_non_exhaustive()
    }
}

/// One explicit route, with no environment lookup or fallback to a direct socket.
/// Only the proxy host is resolved locally; destination names go to the proxy.
#[derive(Clone, Debug, Default)]
pub enum UpstreamRoute {
    #[default]
    Direct,
    /// Basic credentials are plaintext on this explicitly selected proxy hop.
    Http(ProxyEndpoint),
    Https {
        endpoint: ProxyEndpoint,
        /// Trust for the proxy itself, independent of the destination's trust.
        tls: UpstreamTls,
    },
    /// RFC 1929 credentials, when configured; destination DNS is remote.
    Socks5(ProxyEndpoint),
    /// RFC 1929 credentials, when configured; destination DNS is resolved locally first.
    Socks5LocalDns(ProxyEndpoint),
    /// SOCKS5 proxy with explicit host bypass patterns (e.g. `localhost`, `127.0.0.1`, `*.corp.local`).
    Socks5WithBypass {
        endpoint: ProxyEndpoint,
        local_dns: bool,
        bypass_hosts: Vec<String>,
    },
}

/// Matches a target host against a SOCKS bypass host pattern.
///
/// Supports exact hosts/IPs (`localhost`, `127.0.0.1`, `::1`), leading-dot suffixes
/// (`.corp.local`), subdomain wildcards (`*.corp.local`), and glob wildcards (`192.168.*`, `dev-?.local`).
pub fn matches_bypass_host(pattern: &str, host: &str) -> bool {
    let normalize = |raw: &str| {
        raw.trim()
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase()
    };
    let pat = normalize(pattern);
    let target = normalize(host);
    if pat.is_empty() || target.is_empty() {
        return false;
    }
    if pat == "*" {
        return true;
    }
    if pat.starts_with('.') {
        return target.ends_with(&pat) && target.len() > pat.len();
    }
    if !pat.contains('*') && !pat.contains('?') {
        return pat == target;
    }
    glob_match(pat.as_bytes(), target.as_bytes())
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star_idx: Option<usize> = None;
    let mut match_idx = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star_idx = Some(p);
            match_idx = t;
            p += 1;
        } else if let Some(star) = star_idx {
            p = star + 1;
            match_idx += 1;
            t = match_idx;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

impl UpstreamRoute {
    pub fn socks5_with_bypass(
        endpoint: ProxyEndpoint,
        local_dns: bool,
        bypass_hosts: Vec<String>,
    ) -> Self {
        if bypass_hosts.is_empty() {
            if local_dns {
                Self::Socks5LocalDns(endpoint)
            } else {
                Self::Socks5(endpoint)
            }
        } else {
            Self::Socks5WithBypass {
                endpoint,
                local_dns,
                bypass_hosts,
            }
        }
    }

    pub fn with_bypass_hosts(self, bypass_hosts: Vec<String>) -> Self {
        match self {
            Self::Socks5(endpoint) => Self::socks5_with_bypass(endpoint, false, bypass_hosts),
            Self::Socks5LocalDns(endpoint) => {
                Self::socks5_with_bypass(endpoint, true, bypass_hosts)
            }
            Self::Socks5WithBypass {
                endpoint,
                local_dns,
                ..
            } => Self::socks5_with_bypass(endpoint, local_dns, bypass_hosts),
            other => other,
        }
    }

    pub fn bypass_hosts(&self) -> &[String] {
        match self {
            Self::Socks5WithBypass { bypass_hosts, .. } => bypass_hosts,
            _ => &[],
        }
    }

    pub fn matches_socks_bypass(&self, target_host: &str) -> bool {
        match self {
            Self::Socks5WithBypass { bypass_hosts, .. } => bypass_hosts
                .iter()
                .any(|pattern| matches_bypass_host(pattern, target_host)),
            _ => false,
        }
    }

    pub fn resolve_for_host(&self, target_host: &str) -> Self {
        if self.matches_socks_bypass(target_host) {
            Self::Direct
        } else {
            self.clone()
        }
    }

    pub(crate) fn is_direct_or_local_dns(&self) -> bool {
        matches!(
            self,
            Self::Direct
                | Self::Socks5LocalDns(_)
                | Self::Socks5WithBypass {
                    local_dns: true,
                    ..
                }
        )
    }

    pub(crate) fn rama(&self) -> Result<ProxyRoute, ProxyError> {
        let (endpoint, protocol, socks) = match self {
            Self::Direct => return Ok(ProxyRoute::Direct),
            Self::Http(endpoint) => (endpoint, Protocol::HTTP, false),
            Self::Https { endpoint, .. } => (endpoint, Protocol::HTTPS, false),
            Self::Socks5(endpoint) => (endpoint, Protocol::SOCKS5H, true),
            Self::Socks5LocalDns(endpoint) => (endpoint, Protocol::SOCKS5, true),
            Self::Socks5WithBypass {
                endpoint,
                local_dns,
                ..
            } => (
                endpoint,
                if *local_dns {
                    Protocol::SOCKS5
                } else {
                    Protocol::SOCKS5H
                },
                true,
            ),
        };
        if endpoint.port == 0 || endpoint.host.len() > 253 {
            return Err(ProxyError::invalid("上游代理主机或端口无效"));
        }
        let host: Host = endpoint
            .host
            .parse()
            .map_err(|_| ProxyError::invalid("上游代理需要纯主机名或 IP；不能包含 URL 或凭据"))?;
        let credential = endpoint
            .credentials
            .as_ref()
            .map(|credentials| {
                let max = if socks { 255 } else { 4096 };
                if credentials.username.is_empty()
                    || credentials.username.len() > max
                    || credentials.password.len() > max
                    || (socks && credentials.password.is_empty())
                    || (!socks && credentials.username.contains(':'))
                    || credentials
                        .username
                        .bytes()
                        .chain(credentials.password.bytes())
                        .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
                {
                    return Err(ProxyError::invalid("上游认证字段无效或超过协议长度上限"));
                }
                let username = credentials
                    .username
                    .clone()
                    .try_into()
                    .map_err(|_| ProxyError::invalid("上游认证用户名不能为空"))?;
                let basic = if credentials.password.is_empty() {
                    Basic::new_insecure(username)
                } else {
                    Basic::new(
                        username,
                        credentials
                            .password
                            .clone()
                            .try_into()
                            .map_err(|_| ProxyError::invalid("上游认证密码无效"))?,
                    )
                };
                Ok(ProxyCredential::Basic(basic))
            })
            .transpose()?;
        Ok(ProxyRoute::Proxy(ProxyAddress {
            protocol: Some(protocol),
            address: HostWithPort::new(host, endpoint.port),
            credential,
        }))
    }

    pub(crate) fn tls_config_with_options(
        &self,
        client_certs: &[(crate::ClientCertificateRule, crate::ParsedClientIdentity)],
        tls_options: &UpstreamTlsOptions,
    ) -> Result<TlsClientConfig, ProxyError> {
        let (tls, identity) = match self {
            Self::Https { endpoint, tls } => (
                tls,
                crate::select_client_identity(client_certs, &endpoint.host),
            ),
            _ => (&UpstreamTls::System, None),
        };
        tls.config_and_key(Some(HttpVersion::Http11), tls_options, identity, false)
            .map(|(config, _)| config)
    }

    pub(crate) fn https_proxy_tls_config_and_key(
        &self,
        client_certs: &[(crate::ClientCertificateRule, crate::ParsedClientIdentity)],
        tls_options: &UpstreamTlsOptions,
    ) -> Result<Option<(TlsClientConfig, crate::tls::ClientTlsCacheKey)>, ProxyError> {
        let Self::Https { endpoint, tls } = self else {
            return Ok(None);
        };
        let identity = crate::select_client_identity(client_certs, &endpoint.host);
        tls.config_and_key(Some(HttpVersion::Http11), tls_options, identity, false)
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_validation_and_debug_do_not_expose_credentials() {
        let mut endpoint = ProxyEndpoint {
            host: "127.0.0.1".into(),
            port: 8080,
            credentials: Some(ProxyCredentials {
                username: "private-user".into(),
                password: "private-password".into(),
            }),
        };
        assert!(UpstreamRoute::Socks5(endpoint.clone()).rama().is_ok());
        let debug = format!("{:?}", UpstreamRoute::Http(endpoint.clone()));
        assert!(!debug.contains("private-user") && !debug.contains("private-password"));
        endpoint.host = "private-user:private-password@localhost".into();
        let error = UpstreamRoute::Http(endpoint.clone())
            .rama()
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-user") && !error.contains("private-password"));
        assert!(!format!("{endpoint:?}").contains("private-password"));
        endpoint.host = "localhost".into();
        endpoint.credentials.as_mut().unwrap().password = "x".repeat(256);
        assert!(UpstreamRoute::Socks5(endpoint).rama().is_err());
    }

    #[test]
    fn socks_bypass_hosts_matches_literals_wildcards_and_ipv6() {
        assert!(matches_bypass_host("localhost", "LOCALHOST."));
        assert!(!matches_bypass_host("localhost", "localhost.evil.com"));
        assert!(matches_bypass_host("127.0.0.1", "127.0.0.1"));
        assert!(matches_bypass_host("::1", "[::1]"));
        assert!(matches_bypass_host("*.corp.local", "api.corp.local"));
        assert!(matches_bypass_host("*.corp.local", "a.b.corp.local"));
        assert!(!matches_bypass_host("*.corp.local", "corp.local"));
        assert!(!matches_bypass_host("*.corp.local", "evilcorp.local"));
        assert!(matches_bypass_host(".corp.local", "svc.corp.local"));
        assert!(matches_bypass_host("192.168.*", "192.168.10.42"));
        assert!(matches_bypass_host("dev-?.internal", "dev-1.internal"));
        assert!(!matches_bypass_host("dev-?.internal", "dev-10.internal"));
        assert!(!matches_bypass_host("", "example.com"));

        let endpoint = ProxyEndpoint {
            host: "127.0.0.1".into(),
            port: 1080,
            credentials: None,
        };
        let route = UpstreamRoute::Socks5(endpoint).with_bypass_hosts(vec![
            "localhost".into(),
            "127.0.0.1".into(),
            "*.corp.local".into(),
        ]);
        assert!(matches!(
            route.resolve_for_host("localhost"),
            UpstreamRoute::Direct
        ));
        assert!(matches!(
            route.resolve_for_host("127.0.0.1"),
            UpstreamRoute::Direct
        ));
        assert!(matches!(
            route.resolve_for_host("git.corp.local"),
            UpstreamRoute::Direct
        ));
        assert!(matches!(
            route.resolve_for_host("public.example.com"),
            UpstreamRoute::Socks5WithBypass { .. }
        ));
        assert!(route.rama().is_ok());
    }
}
