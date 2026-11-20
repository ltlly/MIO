use super::*;

mod advanced;
pub use advanced::*;
pub use crate::store::target_site_map::SiteMapItem;
pub use mio_proxy::{
    AuthCredentials, DigestChallenge, HttpAuthScheme, NtlmType2Challenge, PlatformAuthRuleConfig,
    RepeaterGroupSendMode, RepeaterGroupSendTicket, SinglePacketBarrier, StreamingScopeMatcher,
    UpstreamAuthRuleConfig, clear_repeater_group_send_tickets, register_repeater_group_send_ticket,
    take_repeater_group_send_ticket,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformAuthType {
    #[default]
    Basic,
    Digest,
    NtlmV1,
    NtlmV2,
}

impl PlatformAuthType {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Basic => "Basic",
            Self::Digest => "Digest",
            Self::NtlmV1 => "NTLM v1",
            Self::NtlmV2 => "NTLM v2",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Digest => "digest",
            Self::NtlmV1 => "ntlm_v1",
            Self::NtlmV2 => "ntlm_v2",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.trim().to_ascii_lowercase().as_str() {
            "digest" => Self::Digest,
            "ntlm_v1" | "ntlmv1" | "ntlm v1" => Self::NtlmV1,
            "ntlm_v2" | "ntlmv2" | "ntlm v2" => Self::NtlmV2,
            _ => Self::Basic,
        }
    }

    pub const fn to_scheme(self) -> HttpAuthScheme {
        match self {
            Self::Basic => HttpAuthScheme::Basic,
            Self::Digest => HttpAuthScheme::Digest,
            Self::NtlmV1 => HttpAuthScheme::NtlmV1,
            Self::NtlmV2 => HttpAuthScheme::NtlmV2,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlatformAuthRule {
    pub id: Uuid,
    pub enabled: bool,
    pub destination_host: String,
    pub auth_type: PlatformAuthType,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub hostname: String,
}

impl Default for PlatformAuthRule {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            enabled: true,
            destination_host: "*".into(),
            auth_type: PlatformAuthType::Basic,
            username: String::new(),
            password: String::new(),
            domain: String::new(),
            hostname: String::new(),
        }
    }
}

impl std::fmt::Debug for PlatformAuthRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformAuthRule")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("destination_host", &self.destination_host)
            .field("auth_type", &self.auth_type)
            .field("domain", &self.domain)
            .field("hostname", &self.hostname)
            .field(
                "authenticated",
                &(!self.username.is_empty() || !self.password.is_empty()),
            )
            .finish()
    }
}

impl PlatformAuthRule {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.id.is_nil(), "平台认证规则 ID 无效");
        crate::UpstreamProxyRule::validate_destination_host(&self.destination_host)?;
        anyhow::ensure!(!self.username.trim().is_empty(), "平台认证必须填写用户名");
        if self.auth_type == PlatformAuthType::Basic {
            anyhow::ensure!(
                !self.username.contains(':'),
                "Basic 平台认证用户名不能包含冒号"
            );
        }
        anyhow::ensure!(
            self.username.len() <= 512
                && self.password.len() <= 512
                && self.domain.len() <= 253
                && self.hostname.len() <= 253
                && !self
                    .username
                    .bytes()
                    .chain(self.password.bytes())
                    .chain(self.domain.bytes())
                    .chain(self.hostname.bytes())
                    .any(|b| matches!(b, b'\r' | b'\n' | 0)),
            "平台认证字段包含非法控制字符或超过长度上限"
        );
        Ok(())
    }

    pub fn matches_destination(&self, host: &str) -> bool {
        self.enabled && mio_proxy::matches_bypass_host(&self.destination_host, host)
    }

    pub fn to_proxy_config(&self) -> PlatformAuthRuleConfig {
        PlatformAuthRuleConfig {
            enabled: self.enabled,
            destination_host: self.destination_host.trim().to_owned(),
            credentials: AuthCredentials {
                scheme: self.auth_type.to_scheme(),
                username: self.username.clone(),
                password: self.password.clone(),
                domain: self.domain.trim().to_owned(),
                workstation: self.hostname.trim().to_owned(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyInterceptOperator {
    #[default]
    And,
    Or,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyInterceptType {
    #[default]
    DomainName,
    IpAddress,
    Protocol,
    HttpMethod,
    FileExtension,
    Url,
    Header,
    Request,
    CookieName,
    CookieValue,
    Body,
    ParamName,
    ParamValue,
    ContentTypeHeader,
    MimeType,
    PageTitle,
    StatusCode,
    ListenerPort,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyInterceptRelation {
    #[default]
    Matches,
    DoesNotMatch,
    RequestMatches,
    RequestDoesNotMatch,
    ResponseMatches,
    ResponseDoesNotMatch,
    IsHttp,
    IsHttps,
    ContainsParameters,
    DoesNotContainParameters,
    WasModified,
    WasNotModified,
    WasIntercepted,
    WasNotIntercepted,
    WasAnnotated,
    WasNotAnnotated,
    InScope,
    NotInScope,
}

impl ProxyInterceptRelation {
    pub fn is_regex(self) -> bool {
        matches!(
            self,
            Self::Matches
                | Self::DoesNotMatch
                | Self::RequestMatches
                | Self::RequestDoesNotMatch
                | Self::ResponseMatches
                | Self::ResponseDoesNotMatch
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyInterceptRule {
    pub id: Uuid,
    pub enabled: bool,
    pub operator: ProxyInterceptOperator,
    pub match_type: ProxyInterceptType,
    pub relationship: ProxyInterceptRelation,
    pub condition: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyInterceptRuleSet {
    pub enabled: bool,
    pub rules: Vec<ProxyInterceptRule>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyInterceptionSettings {
    pub request: ProxyInterceptRuleSet,
    pub response: ProxyInterceptRuleSet,
}

impl Default for ProxyInterceptionSettings {
    fn default() -> Self {
        use ProxyInterceptOperator::{And, Or};
        use ProxyInterceptRelation::{
            ContainsParameters, DoesNotMatch, InScope, Matches, WasIntercepted, WasModified,
        };
        use ProxyInterceptType::{
            ContentTypeHeader, FileExtension, HttpMethod, Request, StatusCode, Url,
        };
        let rule =
            |id, enabled, operator, match_type, relationship, condition: &str| ProxyInterceptRule {
                id: Uuid::from_u128(id),
                enabled,
                operator,
                match_type,
                relationship,
                condition: condition.into(),
            };
        Self {
            request: ProxyInterceptRuleSet {
                enabled: true,
                rules: vec![
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000001,
                        true,
                        And,
                        FileExtension,
                        DoesNotMatch,
                        "(^gif$|^jpg$|^png$|^css$|^js$|^ico$|^svg$|^eot$|^woff$|^woff2$|^ttf$)",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000002,
                        false,
                        Or,
                        Request,
                        ContainsParameters,
                        "",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000003,
                        false,
                        Or,
                        HttpMethod,
                        DoesNotMatch,
                        "(get|post)",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000004,
                        false,
                        And,
                        Url,
                        InScope,
                        "",
                    ),
                ],
            },
            response: ProxyInterceptRuleSet {
                enabled: false,
                rules: vec![
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000005,
                        true,
                        And,
                        ContentTypeHeader,
                        Matches,
                        "text",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000006,
                        false,
                        Or,
                        Request,
                        WasModified,
                        "",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000007,
                        false,
                        Or,
                        Request,
                        WasIntercepted,
                        "",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000008,
                        false,
                        And,
                        StatusCode,
                        DoesNotMatch,
                        "^304$",
                    ),
                    rule(
                        0x58e9f964_060e_4afc_911c_000000000009,
                        false,
                        And,
                        Url,
                        InScope,
                        "",
                    ),
                ],
            },
        }
    }
}

impl ProxyInterceptionSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        crate::http::validate_proxy_interception(self)
    }

    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyMiscSettings {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_http10_in_requests: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_http10_in_responses: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub set_response_connection_close: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub set_request_connection_header: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub strip_proxy_headers: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub remove_unsupported_accept_encodings: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub strip_sec_websocket_extensions: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppress_browser_error_messages: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_history_logging: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_out_of_scope_history_logging: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub use_http1_keep_alive: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_web_interface: bool,
}

impl Default for ProxyMiscSettings {
    fn default() -> Self {
        Self {
            use_http10_in_requests: false,
            use_http10_in_responses: false,
            set_response_connection_close: false,
            set_request_connection_header: true,
            strip_proxy_headers: true,
            remove_unsupported_accept_encodings: true,
            strip_sec_websocket_extensions: true,
            suppress_browser_error_messages: false,
            disable_history_logging: false,
            disable_out_of_scope_history_logging: false,
            use_http1_keep_alive: true,
            disable_web_interface: false,
        }
    }
}

impl ProxyMiscSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyMessageSettings {
    pub unpack_compressed_requests: bool,
    pub unpack_compressed_responses: bool,
    #[serde(
        default,
        skip_serializing_if = "ProxyResponseModificationSettings::is_default"
    )]
    pub response_modification: ProxyResponseModificationSettings,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub match_replace_in_scope_only: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ws_match_replace_in_scope_only: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub auto_update_request_content_length: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_fix_request_newlines: bool,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub auto_update_response_content_length: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ssl_pass_through_auto_add_on_failure: bool,
    #[serde(default, skip_serializing_if = "ProxyMiscSettings::is_default")]
    pub misc: ProxyMiscSettings,
}

impl Default for ProxyMessageSettings {
    fn default() -> Self {
        Self {
            unpack_compressed_requests: false,
            unpack_compressed_responses: true,
            response_modification: ProxyResponseModificationSettings::default(),
            match_replace_in_scope_only: false,
            ws_match_replace_in_scope_only: false,
            auto_update_request_content_length: true,
            auto_fix_request_newlines: false,
            auto_update_response_content_length: true,
            ssl_pass_through_auto_add_on_failure: false,
            misc: ProxyMiscSettings::default(),
        }
    }
}

impl ProxyMessageSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyWebSocketInterceptionSettings {
    pub client_to_server_messages: bool,
    pub server_to_client_messages: bool,
    pub intercept_in_scope_only: bool,
}

impl Default for ProxyWebSocketInterceptionSettings {
    fn default() -> Self {
        Self {
            client_to_server_messages: true,
            server_to_client_messages: true,
            intercept_in_scope_only: false,
        }
    }
}

impl ProxyWebSocketInterceptionSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsProtocolMode {
    #[default]
    AllSupported,
    Default,
    Custom,
}

impl TlsProtocolMode {
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::AllSupported)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedServerCert {
    pub host: String,
    pub port: u16,
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_to: String,
    #[serde(default)]
    pub san: Vec<String>,
    pub sha256_fingerprint: String,
    pub pem: String,
}

impl From<mio_proxy::ObservedCertInfo> for ObservedServerCert {
    fn from(info: mio_proxy::ObservedCertInfo) -> Self {
        Self {
            host: info.host,
            port: info.port,
            subject: info.subject,
            issuer: info.issuer,
            valid_from: info.valid_from,
            valid_to: info.valid_to,
            san: info.sans,
            sha256_fingerprint: info.sha256_fingerprint,
            pem: info.pem,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamTlsSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_ca_ders: Vec<Vec<u8>>,
    #[serde(default = "default_true_bool")]
    pub include_system_roots: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure_skip_verify: bool,
    #[serde(default, skip_serializing_if = "TlsProtocolMode::is_default")]
    pub protocol_mode: TlsProtocolMode,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_unsafe_renegotiation: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_session_resume: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub client_certs_override_project_only: bool,
}

impl Default for UpstreamTlsSettings {
    fn default() -> Self {
        Self {
            custom_ca_ders: Vec::new(),
            include_system_roots: true,
            insecure_skip_verify: false,
            protocol_mode: TlsProtocolMode::AllSupported,
            allow_unsafe_renegotiation: false,
            disable_session_resume: false,
            client_certs_override_project_only: false,
        }
    }
}

impl UpstreamTlsSettings {
    pub fn is_default(&self) -> bool {
        !self.insecure_skip_verify
            && self.custom_ca_ders.is_empty()
            && self.include_system_roots
            && self.protocol_mode == TlsProtocolMode::AllSupported
            && !self.allow_unsafe_renegotiation
            && !self.disable_session_resume
            && !self.client_certs_override_project_only
    }

    pub fn has_custom_root_override(&self) -> bool {
        !self.custom_ca_ders.is_empty() || !self.include_system_roots
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.custom_ca_ders.len() <= 32
                && self
                    .custom_ca_ders
                    .iter()
                    .all(|cert| !cert.is_empty() && cert.len() <= 64 * 1024),
            "自定义 TLS 根证书最多 32 个，且须为非空、不超过 64 KiB 的 DER 编码"
        );
        anyhow::ensure!(
            self.insecure_skip_verify
                || !self.custom_ca_ders.is_empty()
                || self.include_system_roots,
            "未配置自定义 TLS 根证书时不能关闭系统根证书信任"
        );
        self.to_proxy_tls().validate()?;
        Ok(())
    }

    pub fn to_proxy_tls(&self) -> mio_proxy::UpstreamTls {
        if self.insecure_skip_verify {
            mio_proxy::UpstreamTls::Insecure
        } else if !self.has_custom_root_override() {
            mio_proxy::UpstreamTls::System
        } else {
            mio_proxy::UpstreamTls::CustomRoots {
                certificates_der: self.custom_ca_ders.clone(),
                include_system: self.include_system_roots,
            }
        }
    }

    pub fn to_proxy_tls_options(&self) -> mio_proxy::UpstreamTlsOptions {
        let protocol_mode = match self.protocol_mode {
            TlsProtocolMode::AllSupported => mio_proxy::UpstreamTlsProtocolMode::AllSupported,
            TlsProtocolMode::Default | TlsProtocolMode::Custom => {
                mio_proxy::UpstreamTlsProtocolMode::Default
            }
        };
        mio_proxy::UpstreamTlsOptions {
            disable_session_resume: self.disable_session_resume,
            protocol_mode,
            tls_min_version: None,
            tls_max_version: None,
        }
    }

    pub fn certificate_display_name(certificate: &[u8]) -> anyhow::Result<String> {
        Ok(mio_proxy::UpstreamTls::certificate_display_name(
            certificate,
        )?)
    }

    pub fn parse_pem_or_der_cert(input: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            !input.is_empty() && input.len() <= 64 * 1024,
            "CA 证书数据不能为空且不能超过 64 KiB"
        );
        let der = if input
            .trim_ascii()
            .starts_with(b"-----BEGIN CERTIFICATE-----")
        {
            let text = std::str::from_utf8(input)?;
            let b64 = text
                .trim_ascii()
                .strip_prefix("-----BEGIN CERTIFICATE-----")
                .and_then(|rest| rest.strip_suffix("-----END CERTIFICATE-----"))
                .ok_or_else(|| anyhow::anyhow!("须提供一个完整的 PEM 证书块，不能包含其它内容"))?;
            anyhow::ensure!(
                !b64.contains("-----"),
                "每次只能添加一个 PEM 证书，不能包含其它证书或私钥块"
            );
            let cleaned = b64
                .bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect();
            let decoded = crate::intruder::apply_payload_processing_rule(
                &[cleaned],
                &crate::intruder::PayloadProcessingRule::Base64Decode,
            )
            .map_err(|e| anyhow::anyhow!("PEM 证书 Base64 解码失败：{e}"))?;
            decoded.into_iter().next().unwrap_or_default()
        } else {
            // DER signature bytes are opaque and must not be trimmed as text.
            input.to_vec()
        };
        mio_proxy::UpstreamTls::CustomRoots {
            certificates_der: vec![der.clone()],
            include_system: false,
        }
        .validate()?;
        Ok(der)
    }
}
