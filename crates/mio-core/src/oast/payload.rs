use super::*;

impl Default for SelfHostedOastConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            polling_url: None,
            server_domain: None,
            authorization_header: None,
            poll_interval_ms: DEFAULT_SELF_HOSTED_POLL_INTERVAL_MS,
        }
    }
}

impl SelfHostedOastConfig {
    pub fn validate(&self) -> Result<Option<Url>> {
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
            !is_blocked_cloud_domain(&polling_host),
            "public third-party Collaborator domains are blocked by default"
        );

        if let Some(domain) = self
            .server_domain
            .as_deref()
            .map(str::trim)
            .filter(|domain| !domain.is_empty())
        {
            let normalized_domain = domain.trim_end_matches('.').to_ascii_lowercase();
            ensure!(
                !normalized_domain.is_empty()
                    && !normalized_domain.chars().any(char::is_whitespace),
                "self-hosted server domain is invalid"
            );
            ensure!(
                !is_blocked_cloud_domain(&normalized_domain),
                "public third-party Collaborator domains are blocked by default"
            );
        }

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

pub(super) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub(super) fn truncate_str(value: &str, max_chars: usize) -> &str {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value, |(boundary, _)| &value[..boundary])
}

pub(super) fn truncate_lossy(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    truncate_str(&text, max_chars).to_owned()
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
