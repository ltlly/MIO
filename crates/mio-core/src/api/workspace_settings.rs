use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeaterRedirectMode {
    #[default]
    Never,
    OnSiteOnly,
    InScopeOnly,
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeaterLayoutMode {
    #[default]
    SideBySide,
    TopBottom,
    Combined,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProxyProtocol {
    #[default]
    Direct,
    Http,
    Https,
    Socks5,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamAuthType {
    #[default]
    None,
    Basic,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpstreamProxyRule {
    pub id: Uuid,
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    #[serde(default = "default_destination_wildcard")]
    pub destination_host: String,
    pub protocol: UpstreamProxyProtocol,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub auth_type: UpstreamAuthType,
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

const fn default_enabled_true() -> bool {
    true
}

fn default_destination_wildcard() -> String {
    "*".into()
}

impl Default for UpstreamProxyRule {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            enabled: true,
            destination_host: default_destination_wildcard(),
            protocol: UpstreamProxyProtocol::Http,
            host: String::new(),
            port: 8080,
            auth_type: UpstreamAuthType::None,
            username: String::new(),
            password: String::new(),
            description: String::new(),
        }
    }
}

impl std::fmt::Debug for UpstreamProxyRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamProxyRule")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("destination_host", &self.destination_host)
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("auth_type", &self.auth_type)
            .field(
                "authenticated",
                &(!self.username.is_empty() || !self.password.is_empty()),
            )
            .field("description", &self.description)
            .finish()
    }
}

pub const UPSTREAM_EXT_AUTH_PREFIX: &str = "mio-up-auth:";
pub const PLATFORM_AUTH_CFG_PREFIX: &str = "mio-plat-cfg:";
pub const PLATFORM_AUTH_RULE_PREFIX: &str = "mio-plat-rule:";

fn hex_encode_str(input: &str) -> String {
    mio_proxy::auth::to_hex_lower(input.as_bytes())
}

fn hex_decode_str(hex: &str) -> String {
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let Ok(s) = std::str::from_utf8(&bytes[i..i + 2])
            && let Ok(b) = u8::from_str_radix(s, 16)
        {
            out.push(b);
        }
        i += 2;
    }
    String::from_utf8(out).unwrap_or_default()
}

impl UpstreamProxyRule {
    pub fn is_internal_platform_auth_carrier(&self) -> bool {
        self.description.starts_with(PLATFORM_AUTH_CFG_PREFIX)
            || self.description.starts_with(PLATFORM_AUTH_RULE_PREFIX)
    }

    pub fn auth_scheme(&self) -> HttpAuthScheme {
        if let Some(rest) = self.description.strip_prefix(UPSTREAM_EXT_AUTH_PREFIX) {
            let meta = rest.split('|').next().unwrap_or("");
            let scheme_code = meta.split(';').next().unwrap_or("");
            let parsed = HttpAuthScheme::from_code(scheme_code);
            if parsed != HttpAuthScheme::None {
                return parsed;
            }
        }
        match self.auth_type {
            UpstreamAuthType::None => HttpAuthScheme::None,
            UpstreamAuthType::Basic => HttpAuthScheme::Basic,
        }
    }

    pub fn domain(&self) -> String {
        if let Some(rest) = self.description.strip_prefix(UPSTREAM_EXT_AUTH_PREFIX) {
            let meta = rest.split('|').next().unwrap_or("");
            for part in meta.split(';').skip(1) {
                if let Some(hex) = part.strip_prefix("d=") {
                    return hex_decode_str(hex);
                }
            }
        }
        String::new()
    }

    pub fn workstation(&self) -> String {
        if let Some(rest) = self.description.strip_prefix(UPSTREAM_EXT_AUTH_PREFIX) {
            let meta = rest.split('|').next().unwrap_or("");
            for part in meta.split(';').skip(1) {
                if let Some(hex) = part.strip_prefix("w=") {
                    return hex_decode_str(hex);
                }
            }
        }
        String::new()
    }

    pub fn user_description(&self) -> String {
        if let Some(rest) = self.description.strip_prefix(UPSTREAM_EXT_AUTH_PREFIX) {
            if let Some((_, desc)) = rest.split_once('|') {
                return desc.to_owned();
            }
            return String::new();
        }
        self.description.clone()
    }

    pub fn set_extended_auth(
        &mut self,
        scheme: HttpAuthScheme,
        domain: &str,
        workstation: &str,
        user_description: &str,
    ) {
        let clean_dom = domain.trim();
        let clean_ws = workstation.trim();
        let clean_desc = user_description.trim();
        self.auth_type = match scheme {
            HttpAuthScheme::None => UpstreamAuthType::None,
            _ => UpstreamAuthType::Basic,
        };
        if matches!(scheme, HttpAuthScheme::None | HttpAuthScheme::Basic)
            && clean_dom.is_empty()
            && clean_ws.is_empty()
        {
            self.description = clean_desc.to_owned();
        } else {
            self.description = format!(
                "{UPSTREAM_EXT_AUTH_PREFIX}{};d={};w={}|{}",
                scheme.code(),
                hex_encode_str(clean_dom),
                hex_encode_str(clean_ws),
                clean_desc
            );
        }
    }

    pub fn with_extended_auth(
        mut self,
        scheme: HttpAuthScheme,
        domain: &str,
        workstation: &str,
        user_description: &str,
    ) -> Self {
        self.set_extended_auth(scheme, domain, workstation, user_description);
        self
    }

    pub fn to_upstream_auth_config(&self) -> Option<UpstreamAuthRuleConfig> {
        if !self.enabled || self.is_internal_platform_auth_carrier() {
            return None;
        }
        let scheme = self.auth_scheme();
        if scheme == HttpAuthScheme::None {
            return None;
        }
        Some(UpstreamAuthRuleConfig {
            enabled: self.enabled,
            destination_host: self.destination_host.trim().to_owned(),
            proxy_host: self.host.trim().to_owned(),
            proxy_port: self.port,
            credentials: AuthCredentials {
                scheme,
                username: self.username.clone(),
                password: self.password.clone(),
                domain: self.domain(),
                workstation: self.workstation(),
            },
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SocksProxySettings {
    pub use_proxy: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default = "default_enabled_true")]
    pub dns_over_socks: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass_hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub override_project_only: bool,
}

impl Default for SocksProxySettings {
    fn default() -> Self {
        Self {
            use_proxy: false,
            host: String::new(),
            port: 0,
            username: String::new(),
            password: String::new(),
            dns_over_socks: true,
            bypass_hosts: Vec::new(),
            override_project_only: false,
        }
    }
}

impl SocksProxySettings {
    pub fn is_default(&self) -> bool {
        !self.use_proxy
            && self.host.is_empty()
            && self.port == 0
            && self.username.is_empty()
            && self.password.is_empty()
            && self.dns_over_socks
            && self.bypass_hosts.is_empty()
            && !self.override_project_only
    }
}

impl std::fmt::Debug for SocksProxySettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocksProxySettings")
            .field("use_proxy", &self.use_proxy)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("dns_over_socks", &self.dns_over_socks)
            .field(
                "authenticated",
                &(!self.username.is_empty() || !self.password.is_empty()),
            )
            .field("bypass_hosts", &self.bypass_hosts)
            .field("override_project_only", &self.override_project_only)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostnameResolutionRule {
    pub id: Uuid,
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    pub hostname: String,
    pub ip_address: String,
}

impl Default for HostnameResolutionRule {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            enabled: true,
            hostname: String::new(),
            ip_address: String::new(),
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpstreamProxySettings {
    pub protocol: UpstreamProxyProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<UpstreamProxyRule>,
    #[serde(default, skip_serializing_if = "SocksProxySettings::is_default")]
    pub socks: SocksProxySettings,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub open_ended_response_timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hostname_resolutions: Vec<HostnameResolutionRule>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub override_project_only: bool,
}

const fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl UpstreamProxySettings {
    pub fn is_default(&self) -> bool {
        self.protocol == UpstreamProxyProtocol::Direct
            && self.host.is_empty()
            && self.port == 0
            && self.username.is_empty()
            && self.password.is_empty()
            && self.rules.is_empty()
            && self.socks.is_default()
            && self.open_ended_response_timeout_secs == 0
            && self.hostname_resolutions.is_empty()
            && !self.override_project_only
    }

    pub fn effective_rules(&self) -> Vec<UpstreamProxyRule> {
        let visible_rules: Vec<UpstreamProxyRule> = self
            .rules
            .iter()
            .filter(|r| !r.is_internal_platform_auth_carrier())
            .cloned()
            .collect();
        if !visible_rules.is_empty() {
            return visible_rules;
        }
        if matches!(
            self.protocol,
            UpstreamProxyProtocol::Http | UpstreamProxyProtocol::Https
        ) && !self.host.trim().is_empty()
            && self.port > 0
        {
            let has_auth = !self.username.is_empty() || !self.password.is_empty();
            return vec![UpstreamProxyRule {
                id: Uuid::from_u128(0x1001),
                enabled: true,
                destination_host: "*".into(),
                protocol: self.protocol,
                host: self.host.clone(),
                port: self.port,
                auth_type: if has_auth {
                    UpstreamAuthType::Basic
                } else {
                    UpstreamAuthType::None
                },
                username: self.username.clone(),
                password: self.password.clone(),
                description: String::new(),
            }];
        }
        Vec::new()
    }

    pub fn effective_socks(&self) -> SocksProxySettings {
        if !self.socks.is_default() {
            return self.socks.clone();
        }
        if self.protocol == UpstreamProxyProtocol::Socks5 {
            return SocksProxySettings {
                use_proxy: true,
                host: self.host.clone(),
                port: self.port,
                username: self.username.clone(),
                password: self.password.clone(),
                dns_over_socks: true,
                bypass_hosts: Vec::new(),
                override_project_only: false,
            };
        }
        SocksProxySettings::default()
    }

    pub fn sync_legacy_summary(&mut self) {
        if self.socks.use_proxy {
            self.protocol = UpstreamProxyProtocol::Socks5;
            self.host = self.socks.host.clone();
            self.port = self.socks.port;
            self.username = self.socks.username.clone();
            self.password = self.socks.password.clone();
            return;
        }
        if let Some(rule) = self
            .rules
            .iter()
            .filter(|rule| !rule.is_internal_platform_auth_carrier())
            .find(|rule| rule.enabled && rule.destination_host.trim() == "*")
            .or_else(|| {
                self.rules
                    .iter()
                    .filter(|rule| !rule.is_internal_platform_auth_carrier())
                    .find(|rule| rule.enabled)
            })
        {
            self.protocol = rule.protocol;
            self.host = rule.host.clone();
            self.port = rule.port;
            if rule.auth_type == UpstreamAuthType::Basic {
                self.username = rule.username.clone();
                self.password = rule.password.clone();
            } else {
                self.username.clear();
                self.password.clear();
            }
            return;
        }
        self.protocol = UpstreamProxyProtocol::Direct;
        self.host.clear();
        self.port = 0;
        self.username.clear();
        self.password.clear();
    }

    pub fn retain_with_platform_auth(&mut self, real_rules: Vec<UpstreamProxyRule>) {
        let carriers: Vec<UpstreamProxyRule> = self
            .rules
            .iter()
            .filter(|r| r.is_internal_platform_auth_carrier())
            .cloned()
            .collect();
        self.rules = real_rules;
        self.rules.extend(carriers);
    }

    fn platform_auth_cfg_tuple(&self) -> (bool, bool, bool) {
        for rule in &self.rules {
            if let Some(rest) = rule.description.strip_prefix(PLATFORM_AUTH_CFG_PREFIX) {
                let mut enabled = false;
                let mut prompt = false;
                let mut over = false;
                for part in rest.split(';') {
                    match part {
                        "en=1" => enabled = true,
                        "pr=1" => prompt = true,
                        "ov=1" => over = true,
                        _ => {}
                    }
                }
                return (enabled, prompt, over);
            }
        }
        (false, false, false)
    }

    fn write_platform_auth_cfg_tuple(&mut self, enabled: bool, prompt: bool, over: bool) {
        self.rules
            .retain(|r| !r.description.starts_with(PLATFORM_AUTH_CFG_PREFIX));
        if enabled || prompt || over || !self.platform_auth_rules().is_empty() {
            self.rules.push(UpstreamProxyRule {
                id: Uuid::from_u128(0x58e9_f964_060e_4afc_911c_0000_0000_f000),
                enabled: false,
                destination_host: "*".into(),
                protocol: UpstreamProxyProtocol::Http,
                host: "127.0.0.1".into(),
                port: 8080,
                auth_type: UpstreamAuthType::None,
                username: String::new(),
                password: String::new(),
                description: format!(
                    "{PLATFORM_AUTH_CFG_PREFIX}en={};pr={};ov={}",
                    u8::from(enabled),
                    u8::from(prompt),
                    u8::from(over)
                ),
            });
        }
    }

    pub fn platform_auth_enabled(&self) -> bool {
        self.platform_auth_cfg_tuple().0
    }

    pub fn set_platform_auth_enabled(&mut self, enabled: bool) {
        let (_, prompt, over) = self.platform_auth_cfg_tuple();
        self.write_platform_auth_cfg_tuple(enabled, prompt, over);
    }

    pub fn platform_auth_prompt_on_failure(&self) -> bool {
        self.platform_auth_cfg_tuple().1
    }

    pub fn set_platform_auth_prompt_on_failure(&mut self, prompt: bool) {
        let (enabled, _, over) = self.platform_auth_cfg_tuple();
        self.write_platform_auth_cfg_tuple(enabled, prompt, over);
    }

    pub fn platform_auth_override_project_only(&self) -> bool {
        self.platform_auth_cfg_tuple().2
    }

    pub fn set_platform_auth_override_project_only(&mut self, over: bool) {
        let (enabled, prompt, _) = self.platform_auth_cfg_tuple();
        self.write_platform_auth_cfg_tuple(enabled, prompt, over);
    }

    pub fn platform_auth_rules(&self) -> Vec<PlatformAuthRule> {
        let mut out = Vec::new();
        for rule in &self.rules {
            let Some(rest) = rule.description.strip_prefix(PLATFORM_AUTH_RULE_PREFIX) else {
                continue;
            };
            let mut enabled = true;
            let mut auth_type = PlatformAuthType::Basic;
            let mut domain = String::new();
            let mut hostname = String::new();
            let mut username = String::new();
            let mut password = String::new();
            for part in rest.split(';') {
                if let Some(v) = part.strip_prefix("en=") {
                    enabled = v == "1";
                } else if let Some(v) = part.strip_prefix("t=") {
                    auth_type = PlatformAuthType::from_code(v);
                } else if let Some(v) = part.strip_prefix("u=") {
                    username = hex_decode_str(v);
                } else if let Some(v) = part.strip_prefix("p=") {
                    password = hex_decode_str(v);
                } else if let Some(v) = part.strip_prefix("d=") {
                    domain = hex_decode_str(v);
                } else if let Some(v) = part.strip_prefix("h=") {
                    hostname = hex_decode_str(v);
                }
            }
            out.push(PlatformAuthRule {
                id: rule.id,
                enabled,
                destination_host: rule.destination_host.clone(),
                auth_type,
                username,
                password,
                domain,
                hostname,
            });
        }
        out
    }

    pub fn set_platform_auth_rules(&mut self, rules: Vec<PlatformAuthRule>) {
        let (en, pr, ov) = self.platform_auth_cfg_tuple();
        self.rules
            .retain(|r| !r.description.starts_with(PLATFORM_AUTH_RULE_PREFIX));
        for item in rules {
            let desc = format!(
                "{PLATFORM_AUTH_RULE_PREFIX}en={};t={};u={};p={};d={};h={}",
                u8::from(item.enabled),
                item.auth_type.code(),
                hex_encode_str(&item.username),
                hex_encode_str(&item.password),
                hex_encode_str(&item.domain),
                hex_encode_str(&item.hostname),
            );
            self.rules.push(UpstreamProxyRule {
                id: if item.id.is_nil() {
                    Uuid::new_v4()
                } else {
                    item.id
                },
                enabled: false,
                destination_host: if item.destination_host.trim().is_empty() {
                    "*".into()
                } else {
                    item.destination_host.trim().to_owned()
                },
                protocol: UpstreamProxyProtocol::Http,
                host: "127.0.0.1".into(),
                port: 8080,
                auth_type: UpstreamAuthType::None,
                username: String::new(),
                password: String::new(),
                description: desc,
            });
        }
        self.write_platform_auth_cfg_tuple(en, pr, ov);
    }

    pub fn compiled_platform_auth_rules(&self) -> std::sync::Arc<[PlatformAuthRuleConfig]> {
        let list: Vec<PlatformAuthRuleConfig> = self
            .platform_auth_rules()
            .iter()
            .filter(|r| r.enabled)
            .map(PlatformAuthRule::to_proxy_config)
            .collect();
        std::sync::Arc::from(list)
    }

    pub fn compiled_upstream_auth_rules(&self) -> std::sync::Arc<[UpstreamAuthRuleConfig]> {
        let list: Vec<UpstreamAuthRuleConfig> = self
            .effective_rules()
            .iter()
            .filter_map(UpstreamProxyRule::to_upstream_auth_config)
            .collect();
        std::sync::Arc::from(list)
    }

    pub fn sync_user_level_cache(&self) {
        let cache = user_level_connections_cache();
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.override_project_only {
            guard.upstream_rules = self.effective_rules();
        }
        if !self.socks.override_project_only {
            guard.socks = self.effective_socks();
        }
        if !self.platform_auth_override_project_only() {
            guard.platform_auth_enabled = self.platform_auth_enabled();
            guard.platform_auth_prompt = self.platform_auth_prompt_on_failure();
            guard.platform_auth_rules = self.platform_auth_rules();
        }
    }

    pub fn effective_for_runtime(&self) -> Self {
        let mut out = self.clone();
        let cache = user_level_connections_cache();
        let guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !out.override_project_only && out.effective_rules().is_empty() && !guard.upstream_rules.is_empty() {
            out.retain_with_platform_auth(guard.upstream_rules.clone());
            out.sync_legacy_summary();
        }
        if !out.socks.override_project_only && out.effective_socks().is_default() && !guard.socks.is_default() {
            out.socks = guard.socks.clone();
            out.sync_legacy_summary();
        }
        if !out.platform_auth_override_project_only()
            && !out.platform_auth_enabled()
            && out.platform_auth_rules().is_empty()
            && (guard.platform_auth_enabled || !guard.platform_auth_rules.is_empty())
        {
            out.set_platform_auth_enabled(guard.platform_auth_enabled);
            out.set_platform_auth_prompt_on_failure(guard.platform_auth_prompt);
            out.set_platform_auth_rules(guard.platform_auth_rules.clone());
        }
        out
    }
}

#[derive(Default, Clone)]
struct UserLevelConnectionsCache {
    upstream_rules: Vec<UpstreamProxyRule>,
    socks: SocksProxySettings,
    platform_auth_enabled: bool,
    platform_auth_prompt: bool,
    platform_auth_rules: Vec<PlatformAuthRule>,
}

fn user_level_connections_cache() -> &'static std::sync::Mutex<UserLevelConnectionsCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<UserLevelConnectionsCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(UserLevelConnectionsCache::default()))
}

impl std::fmt::Debug for UpstreamProxySettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamProxySettings")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field(
                "authenticated",
                &(!self.username.is_empty() || !self.password.is_empty()),
            )
            .field("rules_len", &self.rules.len())
            .field("socks", &self.socks)
            .field(
                "open_ended_response_timeout_secs",
                &self.open_ended_response_timeout_secs,
            )
            .field("hostname_resolutions_len", &self.hostname_resolutions.len())
            .field("override_project_only", &self.override_project_only)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepeaterSettings {
    pub unpack_compressed_responses: bool,
    #[serde(default = "default_repeater_update_content_length")]
    pub update_content_length: bool,
    #[serde(default = "default_repeater_normalize_http1_line_endings")]
    pub normalize_http1_line_endings: bool,
    #[serde(default)]
    pub follow_redirects: RepeaterRedirectMode,
    #[serde(default = "default_repeater_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub layout: RepeaterLayoutMode,
    #[serde(default = "default_repeater_strip_connection_headers_over_http2")]
    pub strip_connection_headers_over_http2: bool,
    #[serde(default)]
    pub enforce_http1_connection_close: bool,
    #[serde(default = "default_repeater_auto_follow_redirections_max_hops")]
    pub auto_follow_redirections_max_hops: u8,
    #[serde(default)]
    pub http1_connection_reuse: bool,
    #[serde(default = "default_enabled_true")]
    pub http2_connection_reuse: bool,
    #[serde(default)]
    pub allow_http2_alpn_override: bool,
    #[serde(default)]
    pub process_cookies_in_redirects: bool,
    #[serde(default)]
    pub cross_domain_protocol_redirects: bool,
    #[serde(default = "default_streaming_timeout_secs")]
    pub streaming_response_timeout_secs: u64,
}

const fn default_repeater_update_content_length() -> bool {
    true
}

const fn default_repeater_normalize_http1_line_endings() -> bool {
    true
}

const fn default_repeater_request_timeout_secs() -> u64 {
    30
}

const fn default_repeater_strip_connection_headers_over_http2() -> bool {
    true
}

const fn default_repeater_auto_follow_redirections_max_hops() -> u8 {
    10
}

const fn default_streaming_timeout_secs() -> u64 {
    600
}

impl Default for RepeaterSettings {
    fn default() -> Self {
        Self {
            unpack_compressed_responses: true,
            update_content_length: default_repeater_update_content_length(),
            normalize_http1_line_endings: default_repeater_normalize_http1_line_endings(),
            follow_redirects: RepeaterRedirectMode::Never,
            request_timeout_secs: default_repeater_request_timeout_secs(),
            layout: RepeaterLayoutMode::SideBySide,
            strip_connection_headers_over_http2:
                default_repeater_strip_connection_headers_over_http2(),
            enforce_http1_connection_close: false,
            auto_follow_redirections_max_hops: default_repeater_auto_follow_redirections_max_hops(),
            http1_connection_reuse: false,
            http2_connection_reuse: true,
            allow_http2_alpn_override: false,
            process_cookies_in_redirects: false,
            cross_domain_protocol_redirects: false,
            streaming_response_timeout_secs: default_streaming_timeout_secs(),
        }
    }
}

impl RepeaterSettings {
    pub fn effective_max_redirect_hops(&self) -> usize {
        usize::from(self.auto_follow_redirections_max_hops.clamp(1, 20))
    }

    pub fn effective_streaming_timeout(&self) -> Option<std::time::Duration> {
        (self.streaming_response_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(self.streaming_response_timeout_secs.min(86_400)))
    }

    pub fn apply_protocol_options(&self, request: &mut HttpRequest, version: HttpVersion) {
        request.version = version;
        if version == HttpVersion::Http2 {
            if self.strip_connection_headers_over_http2 {
                let mut nominated = Vec::new();
                for header in &request.headers {
                    if header.name.eq_ignore_ascii_case("connection")
                        && let Ok(text) = std::str::from_utf8(&header.value)
                    {
                        for token in text.split(',') {
                            let trimmed = token.trim();
                            if !trimmed.is_empty()
                                && !trimmed.eq_ignore_ascii_case("content-length")
                            {
                                nominated.push(trimmed.to_owned());
                            }
                        }
                    }
                }
                request.headers.retain(|header| {
                    !matches!(
                        header.name.to_ascii_lowercase().as_str(),
                        "connection"
                            | "keep-alive"
                            | "transfer-encoding"
                            | "upgrade"
                            | "proxy-connection"
                    ) && !nominated
                        .iter()
                        .any(|name| header.name.eq_ignore_ascii_case(name))
                });
            }
        } else if self.enforce_http1_connection_close {
            let mut updated = false;
            request.headers.retain_mut(|header| {
                if !header.name.eq_ignore_ascii_case("connection") {
                    return true;
                }
                if updated {
                    return false;
                }
                header.value = b"close".to_vec();
                updated = true;
                true
            });
            if !updated {
                request.headers.push(Header {
                    name: "Connection".into(),
                    value: b"close".to_vec(),
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyListenerCertMode {
    #[default]
    PerHostCaSigned,
    SelfSigned,
    SpecificHostname,
    CustomCertificate,
}

impl ProxyListenerCertMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::PerHostCaSigned => "Per-host",
            Self::SelfSigned => "Self-signed",
            Self::SpecificHostname => "Fixed-host",
            Self::CustomCertificate => "Custom",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyListenerConfig {
    pub id: Uuid,
    pub enabled: bool,
    pub bind_address: String,
    #[serde(default)]
    pub invisible_proxying: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redirect_to_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_to_port: Option<u16>,
    #[serde(default)]
    pub force_use_of_tls: bool,
    #[serde(default)]
    pub certificate_mode: ProxyListenerCertMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub specific_cert_hostname: String,
    #[serde(default = "default_enabled_true")]
    pub enable_http2: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tls_protocols_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_cert_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_key_pem: Option<String>,
}

impl Default for ProxyListenerConfig {
    fn default() -> Self {
        Self {
            id: Uuid::from_u128(1),
            enabled: true,
            bind_address: crate::DEFAULT_PROXY_LISTENER.to_owned(),
            invisible_proxying: false,
            redirect_to_host: String::new(),
            redirect_to_port: None,
            force_use_of_tls: false,
            certificate_mode: ProxyListenerCertMode::PerHostCaSigned,
            specific_cert_hostname: String::new(),
            enable_http2: true,
            tls_protocols_label: String::new(),
            custom_cert_pem: None,
            custom_key_pem: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkHttpSettings {
    #[serde(default = "default_enabled_true")]
    pub default_to_http2: bool,
    #[serde(default = "default_enabled_true")]
    pub treat_event_stream_as_streaming: bool,
    #[serde(default = "default_enabled_true")]
    pub store_streaming_responses: bool,
    #[serde(default = "default_enabled_true")]
    pub strip_chunked_metadata_in_streaming: bool,
    #[serde(default = "default_enabled_true")]
    pub understand_100_continue: bool,
    #[serde(default)]
    pub remove_100_continue_headers: bool,
    #[serde(default = "default_enabled_true")]
    pub redirect_3xx: bool,
    #[serde(default = "default_enabled_true")]
    pub redirect_refresh_header: bool,
    #[serde(default = "default_enabled_true")]
    pub redirect_meta_refresh: bool,
    #[serde(default)]
    pub redirect_javascript: bool,
    #[serde(default)]
    pub redirect_any_location: bool,
    #[serde(default)]
    pub streaming_advanced_scope: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streaming_scope_rules: Vec<ScopeRule>,
}

impl Default for NetworkHttpSettings {
    fn default() -> Self {
        Self {
            default_to_http2: true,
            treat_event_stream_as_streaming: true,
            store_streaming_responses: true,
            strip_chunked_metadata_in_streaming: true,
            understand_100_continue: true,
            remove_100_continue_headers: false,
            redirect_3xx: true,
            redirect_refresh_header: true,
            redirect_meta_refresh: true,
            redirect_javascript: false,
            redirect_any_location: false,
            streaming_advanced_scope: false,
            streaming_scope_rules: Vec::new(),
        }
    }
}

impl NetworkHttpSettings {
    pub fn is_default(&self) -> bool {
        self.default_to_http2
            && self.treat_event_stream_as_streaming
            && self.store_streaming_responses
            && self.strip_chunked_metadata_in_streaming
            && self.understand_100_continue
            && !self.remove_100_continue_headers
            && self.redirect_3xx
            && self.redirect_refresh_header
            && self.redirect_meta_refresh
            && !self.redirect_javascript
            && !self.redirect_any_location
            && !self.streaming_advanced_scope
            && self.streaming_scope_rules.is_empty()
    }

    pub fn is_url_in_streaming_scope(&self, url: &str) -> bool {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return false;
        }
        let enabled_rules: Vec<&ScopeRule> = self
            .streaming_scope_rules
            .iter()
            .filter(|r| r.enabled && !r.prefix.trim().is_empty())
            .collect();
        if enabled_rules.is_empty() {
            return false;
        }
        for rule in enabled_rules {
            if rule.matches_url(trimmed) {
                return true;
            }
            if self.streaming_advanced_scope {
                let pat = rule.prefix.trim();
                if let Ok(re) = regex::RegexBuilder::new(pat)
                    .case_insensitive(true)
                    .size_limit(256 * 1024)
                    .build()
                    && re.is_match(trimmed)
                {
                    return true;
                }
                if let Ok(parsed) = url::Url::parse(trimmed)
                    && let Some(host) = parsed.host_str()
                    && mio_proxy::matches_bypass_host(pat, host)
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn build_streaming_scope_matcher(&self) -> Option<StreamingScopeMatcher> {
        if !self
            .streaming_scope_rules
            .iter()
            .any(|r| r.enabled && !r.prefix.trim().is_empty())
        {
            return None;
        }
        let clone = self.clone();
        Some(std::sync::Arc::new(move |url: &str| {
            clone.is_url_in_streaming_scope(url)
        }))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRule {
    pub id: Uuid,
    pub enabled: bool,
    pub prefix: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_subdomains: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeProtocol {
    #[default]
    Any,
    Http,
    Https,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvancedScopeRule {
    pub id: Uuid,
    pub enabled: bool,
    #[serde(default)]
    pub protocol: ScopeProtocol,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub file: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetScope {
    pub include: Vec<ScopeRule>,
    pub exclude: Vec<ScopeRule>,
    pub advanced_mode: bool,
    pub advanced_include: Vec<AdvancedScopeRule>,
    pub advanced_exclude: Vec<AdvancedScopeRule>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop_out_of_scope: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub out_of_scope_use_custom: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out_of_scope_custom_rules: Vec<ScopeRule>,
}

impl TargetScope {
    pub fn is_empty(&self) -> bool {
        !self.advanced_mode
            && self.include.is_empty()
            && self.exclude.is_empty()
            && self.advanced_include.is_empty()
            && self.advanced_exclude.is_empty()
            && !self.drop_out_of_scope
            && !self.out_of_scope_use_custom
            && self.out_of_scope_custom_rules.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledTaskAction {
    #[default]
    PauseTasks,
    ResumeTasks,
    RunScan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduledTask {
    pub id: Uuid,
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    pub action: ScheduledTaskAction,
    pub trigger_spec: String,
    pub description: String,
}

impl Default for ScheduledTask {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            enabled: true,
            action: ScheduledTaskAction::PauseTasks,
            trigger_spec: String::new(),
            description: String::new(),
        }
    }
}

pub use mio_plugin_api::protocol::{MatchAndReplaceRule, MatchReplaceTarget};

const MAX_SCOPE_JSON_BYTES: usize = 256 * 1024;

pub fn export_target_scope_json(scope: &TargetScope) -> Result<String, String> {
    scope.validate().map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(scope).map_err(|err| format!("序列化 Scope JSON 失败：{err}"))
}

pub fn parse_target_scope_json(raw: &str) -> Result<TargetScope, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("剪贴板中没有可导入的 Scope JSON 文本。".into());
    }
    if trimmed.len() > MAX_SCOPE_JSON_BYTES {
        return Err("Scope JSON 超过 256 KiB 上限；未导入。".into());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|err| format!("Scope JSON 格式无效：{err}"))?;
    let Some(obj) = value.as_object() else {
        return Err("Scope JSON 顶层必须是对象。".into());
    };
    if obj.is_empty() {
        return Err("Scope JSON 不能为空对象 `{}`。".into());
    }
    let mut unknown_paths = Vec::new();
    let mut decoder = serde_json::Deserializer::from_str(trimmed);
    let scope: TargetScope = serde_ignored::deserialize(&mut decoder, |path| {
        unknown_paths.push(path.to_string());
    })
    .map_err(|err| format!("Scope JSON 字段无效：{err}"))?;
    decoder
        .end()
        .map_err(|err| format!("Scope JSON 存在尾随内容：{err}"))?;
    if let Some(first) = unknown_paths.first() {
        return Err(format!("Scope JSON 包含未知字段 `{first}`。"));
    }
    scope.validate().map_err(|err| err.to_string())?;
    Ok(scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(version: HttpVersion) -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url: "https://example.com/api".into(),
            version,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"example.com".to_vec(),
                },
                Header {
                    name: "Connection".into(),
                    value: b"keep-alive, X-Custom-Hop".to_vec(),
                },
                Header {
                    name: "Keep-Alive".into(),
                    value: b"timeout=5".to_vec(),
                },
                Header {
                    name: "Transfer-Encoding".into(),
                    value: b"chunked".to_vec(),
                },
                Header {
                    name: "Upgrade".into(),
                    value: b"h2c".to_vec(),
                },
                Header {
                    name: "Proxy-Connection".into(),
                    value: b"keep-alive".to_vec(),
                },
                Header {
                    name: "X-Custom-Hop".into(),
                    value: b"drop-me".to_vec(),
                },
                Header {
                    name: "Accept".into(),
                    value: b"*/*".to_vec(),
                },
            ],
            body: Vec::new(),
            trailers: Vec::new(),
        }
    }

    #[test]
    fn repeater_settings_defaults_and_legacy_json_are_backward_compatible() {
        let defaults = RepeaterSettings::default();
        assert_eq!(defaults.layout, RepeaterLayoutMode::SideBySide);
        assert!(defaults.strip_connection_headers_over_http2);
        assert!(!defaults.enforce_http1_connection_close);
        assert_eq!(defaults.auto_follow_redirections_max_hops, 10);
        assert_eq!(defaults.effective_max_redirect_hops(), 10);
        assert!(!defaults.http1_connection_reuse);
        assert!(defaults.http2_connection_reuse);
        assert!(!defaults.allow_http2_alpn_override);
        assert!(!defaults.process_cookies_in_redirects);
        assert!(!defaults.cross_domain_protocol_redirects);
        assert_eq!(defaults.streaming_response_timeout_secs, 600);
        assert_eq!(
            defaults.effective_streaming_timeout(),
            Some(std::time::Duration::from_secs(600))
        );

        let legacy: RepeaterSettings = serde_json::from_str(
            r#"{"unpack_compressed_responses":false,"request_timeout_secs":15}"#,
        )
        .unwrap();
        assert!(!legacy.unpack_compressed_responses);
        assert!(legacy.update_content_length);
        assert!(legacy.normalize_http1_line_endings);
        assert_eq!(legacy.request_timeout_secs, 15);
        assert_eq!(legacy.layout, RepeaterLayoutMode::SideBySide);
        assert!(legacy.strip_connection_headers_over_http2);
        assert!(!legacy.enforce_http1_connection_close);
        assert_eq!(legacy.auto_follow_redirections_max_hops, 10);
        assert!(!legacy.http1_connection_reuse);
        assert!(legacy.http2_connection_reuse);
        assert!(!legacy.allow_http2_alpn_override);
        assert!(!legacy.process_cookies_in_redirects);
        assert!(!legacy.cross_domain_protocol_redirects);
        assert_eq!(legacy.streaming_response_timeout_secs, 600);

        let unlimited: RepeaterSettings =
            serde_json::from_str(r#"{"streaming_response_timeout_secs":0}"#).unwrap();
        assert_eq!(unlimited.effective_streaming_timeout(), None);
    }

    #[test]
    fn network_http_and_scope_json_are_backward_compatible() {
        let http_defaults: NetworkHttpSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(http_defaults, NetworkHttpSettings::default());
        assert!(http_defaults.is_default());
        assert!(http_defaults.redirect_3xx);
        assert!(http_defaults.redirect_refresh_header);
        assert!(http_defaults.redirect_meta_refresh);
        assert!(!http_defaults.redirect_javascript);
        assert!(!http_defaults.redirect_any_location);
        assert!(!http_defaults.streaming_advanced_scope);
        assert!(http_defaults.streaming_scope_rules.is_empty());

        let scope = TargetScope {
            include: vec![ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix: "https://example.com".into(),
                include_subdomains: true,
            }],
            drop_out_of_scope: true,
            out_of_scope_use_custom: true,
            out_of_scope_custom_rules: vec![ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix: "https://api.example.com".into(),
                include_subdomains: false,
            }],
            ..TargetScope::default()
        };
        assert!(!scope.is_empty());
        let json = export_target_scope_json(&scope).unwrap();
        let parsed = parse_target_scope_json(&json).unwrap();
        assert_eq!(parsed, scope);

        let legacy_scope_json = format!(
            r#"{{"include":[{{"id":"{}","enabled":true,"prefix":"https://legacy.test"}}]}}"#,
            Uuid::new_v4()
        );
        let parsed_legacy = parse_target_scope_json(&legacy_scope_json).unwrap();
        assert!(!parsed_legacy.include[0].include_subdomains);
        assert!(!parsed_legacy.drop_out_of_scope);
        assert!(!parsed_legacy.out_of_scope_use_custom);
        assert!(parsed_legacy.out_of_scope_custom_rules.is_empty());
    }

    #[test]
    fn http2_strips_hop_by_hop_headers_by_default_and_preserves_when_disabled() {
        let settings = RepeaterSettings::default();
        let mut stripped = sample_request(HttpVersion::Http11);
        settings.apply_protocol_options(&mut stripped, HttpVersion::Http2);
        assert_eq!(stripped.version, HttpVersion::Http2);
        let names: Vec<&str> = stripped.headers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["Host", "Accept"]);

        let mut keep_settings = RepeaterSettings {
            strip_connection_headers_over_http2: false,
            ..RepeaterSettings::default()
        };
        let mut preserved = sample_request(HttpVersion::Http2);
        keep_settings.apply_protocol_options(&mut preserved, HttpVersion::Http2);
        assert_eq!(preserved.headers.len(), 8);

        keep_settings.auto_follow_redirections_max_hops = 0;
        assert_eq!(keep_settings.effective_max_redirect_hops(), 1);
        keep_settings.auto_follow_redirections_max_hops = 99;
        assert_eq!(keep_settings.effective_max_redirect_hops(), 20);
    }

    #[test]
    fn http1_enforces_connection_close_when_enabled() {
        let settings = RepeaterSettings {
            enforce_http1_connection_close: true,
            ..RepeaterSettings::default()
        };
        let mut req_with_conn = sample_request(HttpVersion::Http11);
        req_with_conn.headers.push(Header {
            name: "connection".into(),
            value: b"keep-alive".to_vec(),
        });
        settings.apply_protocol_options(&mut req_with_conn, HttpVersion::Http11);
        let conn_headers: Vec<&Header> = req_with_conn
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("connection"))
            .collect();
        assert_eq!(conn_headers.len(), 1);
        assert_eq!(conn_headers[0].value, b"close");

        let mut req_without_conn = HttpRequest {
            method: "POST".into(),
            url: "http://example.com/".into(),
            version: HttpVersion::Http10,
            headers: vec![Header {
                name: "Host".into(),
                value: b"example.com".to_vec(),
            }],
            body: b"hi".to_vec(),
            trailers: Vec::new(),
        };
        settings.apply_protocol_options(&mut req_without_conn, HttpVersion::Http11);
        assert_eq!(req_without_conn.version, HttpVersion::Http11);
        assert!(
            req_without_conn
                .headers
                .iter()
                .any(|h| h.name == "Connection" && h.value == b"close")
        );
    }

    #[test]
    fn parse_target_scope_json_roundtrip_and_strict_validation() {
        let scope = TargetScope {
            include: vec![ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix: "https://example.invalid".into(),
                include_subdomains: true,
            }],
            exclude: vec![ScopeRule {
                id: Uuid::new_v4(),
                enabled: false,
                prefix: "https://example.invalid/logout".into(),
                include_subdomains: false,
            }],
            advanced_mode: true,
            advanced_include: vec![AdvancedScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                protocol: ScopeProtocol::Https,
                host: "^app\\.example\\.invalid$".into(),
                port: "443".into(),
                file: "^/api/.*$".into(),
            }],
            advanced_exclude: vec![],
            drop_out_of_scope: true,
            out_of_scope_use_custom: true,
            out_of_scope_custom_rules: vec![ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix: "https://allow.example.invalid".into(),
                include_subdomains: false,
            }],
        };

        let json = export_target_scope_json(&scope).expect("export scope must succeed");
        let parsed = parse_target_scope_json(&json).expect("roundtrip parse must succeed");
        assert_eq!(parsed, scope);

        let rule_id = Uuid::new_v4();
        let bad_cases = [
            "",
            "{}",
            "[]",
            r#"{"unknown":1}"#,
            &format!(
                r#"{{"include":[{{"id":"{rule_id}","enabled":true,"prefix":"https://example.invalid","extra":1}}]}}"#
            ),
            &format!(
                r#"{{"advanced_include":[{{"id":"{rule_id}","enabled":true,"host":"^example\\.invalid$","extra":1}}]}}"#
            ),
            r#"{"advanced_mode":true,"advanced_mode":false}"#,
        ];
        for case in bad_cases {
            assert!(
                parse_target_scope_json(case).is_err(),
                "expected failure for scope JSON: {case}"
            );
        }

        let oversized_prefix = "a".repeat(MAX_SCOPE_JSON_BYTES);
        let oversized = format!(
            r#"{{"include":[{{"id":"{rule_id}","enabled":true,"prefix":"https://example.invalid/{oversized_prefix}"}}]}}"#
        );
        assert!(oversized.len() > MAX_SCOPE_JSON_BYTES);
        let err = parse_target_scope_json(&oversized).expect_err("oversized scope JSON must fail");
        assert!(err.contains("256 KiB"));
    }
}
