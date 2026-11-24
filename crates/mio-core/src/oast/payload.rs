use super::*;

impl Default for SelfHostedOastConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            polling_url: None,
            server_domain: None,
            authorization_header: None,
            poll_interval_ms: DEFAULT_SELF_HOSTED_POLL_INTERVAL_MS,
            bind_address: None,
            http_port: None,
            dns_port: None,
            dns_answer_ip: None,
            allow_public_cloud_domains: false,
        }
    }
}

impl SelfHostedOastConfig {
    pub fn effective_bind_ip(&self) -> Result<IpAddr> {
        let Some(raw) = self
            .bind_address
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
        };
        let clean = raw
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(raw);
        clean
            .parse::<IpAddr>()
            .map_err(|_| anyhow!("Collaborator bind address is invalid: {raw}"))
    }

    pub fn effective_dns_answer_ip(&self, bound_ip: IpAddr) -> Result<Ipv4Addr> {
        if let Some(raw) = self
            .dns_answer_ip
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return raw
                .parse::<Ipv4Addr>()
                .map_err(|_| anyhow!("Collaborator DNS answer IPv4 is invalid: {raw}"));
        }
        Ok(match bound_ip {
            IpAddr::V4(v4) if !v4.is_unspecified() => v4,
            _ => Ipv4Addr::LOCALHOST,
        })
    }

    pub fn validate(&self) -> Result<Option<Url>> {
        let bind_ip = self.effective_bind_ip()?;
        let _ = self.effective_dns_answer_ip(bind_ip)?;

        if let Some(domain) = self
            .server_domain
            .as_deref()
            .map(str::trim)
            .filter(|domain| !domain.is_empty())
        {
            let normalized_domain = domain.trim_end_matches('.').to_ascii_lowercase();
            ensure!(
                !normalized_domain.is_empty()
                    && !normalized_domain.chars().any(|c| {
                        c.is_whitespace()
                            || c.is_control()
                            || matches!(c, '@' | '/' | '\\' | '?' | '#' | ':')
                    }),
                "self-hosted server domain is invalid"
            );
            ensure!(
                self.allow_public_cloud_domains || !is_blocked_cloud_domain(&normalized_domain),
                "public third-party Collaborator domains are blocked by default"
            );
        }

        if let Some(auth) = &self.authorization_header {
            ensure!(
                !auth.chars().any(|c| c.is_control() || matches!(c, '\r' | '\n')),
                "self-hosted authorization header contains invalid control characters"
            );
        }

        if !self.enabled {
            return Ok(None);
        }

        ensure!(
            self.poll_interval_ms >= MIN_SELF_HOSTED_POLL_INTERVAL_MS,
            "self-hosted polling interval must be at least {MIN_SELF_HOSTED_POLL_INTERVAL_MS} ms"
        );
        let raw_url = self.polling_url.as_deref().unwrap_or_default().trim();
        ensure!(!raw_url.is_empty(), "self-hosted polling URL is required");
        let polling_url = Url::parse(raw_url)
            .map_err(|error| anyhow!("invalid self-hosted polling URL: {error}"))?;
        ensure!(
            matches!(polling_url.scheme(), "http" | "https") && polling_url.host_str().is_some(),
            "self-hosted polling URL must be HTTP(S) with a host"
        );
        let polling_host = polling_url
            .host_str()
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        ensure!(
            self.allow_public_cloud_domains || !is_blocked_cloud_domain(&polling_host),
            "public third-party Collaborator domains are blocked by default"
        );

        Ok(Some(polling_url))
    }
}

fn is_blocked_cloud_domain(domain: &str) -> bool {
    BLOCKED_DEFAULT_CLOUD_DOMAINS
        .iter()
        .any(|blocked| domain == *blocked || domain.ends_with(&format!(".{blocked}")))
}

impl IssuedTokenBook {
    pub(super) fn insert(&mut self, payload: OastPayloadToken) {
        self.tokens.insert(payload.token.clone());
        self.payloads.push(payload);
    }

    pub(super) fn payloads(&self) -> Vec<OastPayloadToken> {
        self.payloads.clone()
    }

    pub(super) fn match_candidates(&self, candidates: &[String]) -> Option<String> {
        candidates
            .iter()
            .find_map(|candidate| self.match_candidate(candidate))
    }

    fn match_candidate(&self, candidate: &str) -> Option<String> {
        let lowered = candidate.to_ascii_lowercase();
        lowered
            .split(|character: char| !character.is_ascii_hexdigit())
            .find_map(|segment| {
                if segment.len() < TOKEN_HEX_LEN {
                    return None;
                }
                if self.tokens.contains(segment) {
                    return Some(segment.to_owned());
                }
                segment
                    .as_bytes()
                    .windows(TOKEN_HEX_LEN)
                    .find_map(|window| {
                        std::str::from_utf8(window)
                            .ok()
                            .filter(|token| self.tokens.contains(*token))
                            .map(str::to_owned)
                    })
            })
    }
}

pub(super) use crate::codec_util::now_unix_ms;

pub(super) fn truncate_str(value: &str, max_chars: usize) -> &str {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value, |(boundary, _)| &value[..boundary])
}

pub(super) fn generate_unique_token(existing_tokens: &HashSet<String>) -> String {
    loop {
        let uuid_bytes = Uuid::new_v4().as_u128().to_be_bytes();
        let entropy = u64::from_be_bytes(uuid_bytes[..8].try_into().unwrap_or_default())
            ^ u64::from_be_bytes(uuid_bytes[8..].try_into().unwrap_or_default());
        let token = format!("{entropy:016x}");
        if !existing_tokens.contains(&token) {
            return token;
        }
    }
}
