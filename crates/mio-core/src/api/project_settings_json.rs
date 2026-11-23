use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::*;

pub const PROJECT_SETTINGS_JSON_FORMAT: &str = "mio.project_settings.v1";
pub const MAX_PROJECT_SETTINGS_JSON_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectSettingsPatch {
    pub repeater_settings: Option<RepeaterSettings>,
    pub network_http: Option<NetworkHttpSettings>,
    pub upstream_proxy: Option<UpstreamProxySettings>,
    pub upstream_tls: Option<UpstreamTlsSettings>,
    pub proxy_listeners: Option<Vec<ProxyListenerConfig>>,
    pub client_certificates: Option<Vec<ClientCertificateRule>>,
    pub scope: Option<TargetScope>,
    pub match_and_replace_rules: Option<Vec<MatchAndReplaceRule>>,
    pub ssl_pass_through_rules: Option<Vec<SslPassThroughRule>>,
    pub ws_match_and_replace_rules: Option<Vec<WsMatchAndReplaceRule>>,
    pub proxy_interception: Option<ProxyInterceptionSettings>,
    pub proxy_messages: Option<ProxyMessageSettings>,
    pub proxy_websocket_interception: Option<ProxyWebSocketInterceptionSettings>,
    pub sessions: Option<SessionSettings>,
    pub tasks_auto_start: Option<bool>,
    pub scheduled_tasks: Option<Vec<ScheduledTask>>,
}

impl ProjectSettingsPatch {
    fn validate_present(&self) -> Result<(), String> {
        let mut probe = Workspace::default();
        self.apply_inner(&mut probe, true)
    }

    pub fn apply_to(&self, ws: &mut Workspace) -> Result<(), String> {
        self.apply_inner(ws, false)
    }

    fn apply_inner(&self, ws: &mut Workspace, validate_certs: bool) -> Result<(), String> {
        let mut candidate = ws.clone();
        if let Some(v) = &self.repeater_settings {
            candidate.repeater_settings = v.clone();
        }
        if let Some(v) = &self.network_http {
            candidate.network_http = v.clone();
        }
        if let Some(v) = &self.upstream_proxy {
            candidate.upstream_proxy = v.clone();
        }
        if let Some(v) = &self.upstream_tls {
            candidate.upstream_tls = v.clone();
        }
        if let Some(v) = &self.proxy_listeners {
            candidate.proxy_listeners = v.clone();
        }
        if let Some(v) = &self.client_certificates {
            candidate.client_certificates = v.clone();
        }
        if let Some(v) = &self.scope {
            candidate.scope = v.clone();
        }
        if let Some(v) = &self.match_and_replace_rules {
            candidate.match_and_replace_rules = v.clone();
        }
        if let Some(v) = &self.ssl_pass_through_rules {
            candidate.ssl_pass_through_rules = v.clone();
        }
        if let Some(v) = &self.ws_match_and_replace_rules {
            candidate.ws_match_and_replace_rules = v.clone();
        }
        if let Some(v) = &self.proxy_interception {
            candidate.proxy_interception = v.clone();
        }
        if let Some(v) = &self.proxy_messages {
            candidate.proxy_messages = *v;
        }
        if let Some(v) = &self.proxy_websocket_interception {
            candidate.proxy_websocket_interception = *v;
        }
        if let Some(v) = &self.sessions {
            candidate.sessions = v.clone();
        }
        if let Some(v) = self.tasks_auto_start {
            candidate.tasks_auto_start = v;
        }
        if let Some(v) = &self.scheduled_tasks {
            candidate.scheduled_tasks = v.clone();
        }
        let subset = ProjectSettingsSubset::from_workspace(&candidate);
        if validate_certs {
            subset.validate()?;
        } else {
            for listener in &candidate.proxy_listeners {
                if listener.redirect_to_port == Some(0) {
                    return Err("Proxy 监听器重定向端口必须在 1 到 65535 之间。".into());
                }
            }
            crate::validate_ssl_and_ws_rules(
                &candidate.ssl_pass_through_rules,
                &candidate.ws_match_and_replace_rules,
            )
            .map_err(|err| err.to_string())?;
            candidate
                .validate_structure()
                .map_err(|err| err.to_string())?;
        }
        *ws = candidate;
        Ok(())
    }

    pub fn into_subset(self) -> ProjectSettingsSubset {
        let mut ws = Workspace::default();
        let _ = self.apply_to(&mut ws);
        ProjectSettingsSubset::from_workspace(&ws)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSettingsSubset {
    pub repeater_settings: RepeaterSettings,
    pub network_http: NetworkHttpSettings,
    pub upstream_proxy: UpstreamProxySettings,
    pub upstream_tls: UpstreamTlsSettings,
    pub proxy_listeners: Vec<ProxyListenerConfig>,
    pub client_certificates: Vec<ClientCertificateRule>,
    pub scope: TargetScope,
    pub match_and_replace_rules: Vec<MatchAndReplaceRule>,
    pub ssl_pass_through_rules: Vec<SslPassThroughRule>,
    pub ws_match_and_replace_rules: Vec<WsMatchAndReplaceRule>,
    pub proxy_interception: ProxyInterceptionSettings,
    pub proxy_messages: ProxyMessageSettings,
    pub proxy_websocket_interception: ProxyWebSocketInterceptionSettings,
    pub sessions: SessionSettings,
    pub tasks_auto_start: bool,
    pub scheduled_tasks: Vec<ScheduledTask>,
}

impl Default for ProjectSettingsSubset {
    fn default() -> Self {
        Self {
            repeater_settings: RepeaterSettings::default(),
            network_http: NetworkHttpSettings::default(),
            upstream_proxy: UpstreamProxySettings::default(),
            upstream_tls: UpstreamTlsSettings::default(),
            proxy_listeners: Vec::new(),
            client_certificates: Vec::new(),
            scope: TargetScope::default(),
            match_and_replace_rules: Vec::new(),
            ssl_pass_through_rules: Vec::new(),
            ws_match_and_replace_rules: Vec::new(),
            proxy_interception: ProxyInterceptionSettings::default(),
            proxy_messages: ProxyMessageSettings::default(),
            proxy_websocket_interception: ProxyWebSocketInterceptionSettings::default(),
            sessions: SessionSettings::default(),
            tasks_auto_start: true,
            scheduled_tasks: Vec::new(),
        }
    }
}

impl ProjectSettingsSubset {
    pub fn from_workspace(ws: &Workspace) -> Self {
        Self {
            repeater_settings: ws.repeater_settings.clone(),
            network_http: ws.network_http.clone(),
            upstream_proxy: ws.upstream_proxy.clone(),
            upstream_tls: ws.upstream_tls.clone(),
            proxy_listeners: ws.proxy_listeners.clone(),
            client_certificates: ws.client_certificates.clone(),
            scope: ws.scope.clone(),
            match_and_replace_rules: ws.match_and_replace_rules.clone(),
            ssl_pass_through_rules: ws.ssl_pass_through_rules.clone(),
            ws_match_and_replace_rules: ws.ws_match_and_replace_rules.clone(),
            proxy_interception: ws.proxy_interception.clone(),
            proxy_messages: ws.proxy_messages,
            proxy_websocket_interception: ws.proxy_websocket_interception,
            sessions: ws.sessions.clone(),
            tasks_auto_start: ws.tasks_auto_start,
            scheduled_tasks: ws.scheduled_tasks.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=3600).contains(&self.repeater_settings.request_timeout_secs) {
            return Err("Repeater 请求超时必须在 1 到 3600 秒之间".into());
        }
        if self.repeater_settings.streaming_response_timeout_secs > 86_400 {
            return Err("Repeater 流式响应超时必须在 0 到 86400 秒之间".into());
        }
        if !(1..=20).contains(&self.repeater_settings.auto_follow_redirections_max_hops) {
            return Err("Repeater 最大重定向跳数必须在 1 到 20 之间".into());
        }
        for listener in &self.proxy_listeners {
            if listener.redirect_to_port == Some(0) {
                return Err("Proxy 监听器重定向端口必须在 1 到 65535 之间。".into());
            }
        }
        crate::validate_ssl_and_ws_rules(
            &self.ssl_pass_through_rules,
            &self.ws_match_and_replace_rules,
        )
        .map_err(|err| err.to_string())?;

        let probe = Workspace {
            repeater_settings: self.repeater_settings.clone(),
            network_http: self.network_http.clone(),
            upstream_proxy: self.upstream_proxy.clone(),
            upstream_tls: self.upstream_tls.clone(),
            proxy_listeners: self.proxy_listeners.clone(),
            client_certificates: self.client_certificates.clone(),
            scope: self.scope.clone(),
            match_and_replace_rules: self.match_and_replace_rules.clone(),
            ssl_pass_through_rules: self.ssl_pass_through_rules.clone(),
            ws_match_and_replace_rules: self.ws_match_and_replace_rules.clone(),
            proxy_interception: self.proxy_interception.clone(),
            proxy_messages: self.proxy_messages,
            proxy_websocket_interception: self.proxy_websocket_interception,
            sessions: self.sessions.clone(),
            tasks_auto_start: self.tasks_auto_start,
            scheduled_tasks: self.scheduled_tasks.clone(),
            ..Workspace::default()
        };
        probe.validate().map_err(|err| err.to_string())
    }
}

pub fn export_project_settings_json(ws: &Workspace) -> Result<String, String> {
    let subset = ProjectSettingsSubset::from_workspace(ws);
    subset.validate()?;
    let doc = ProjectSettingsExportDoc::from(&subset);
    let encoded = serde_json::to_string_pretty(&doc)
        .map_err(|err| format!("序列化项目设置 JSON 失败：{err}"))?;
    if encoded.len() > MAX_PROJECT_SETTINGS_JSON_BYTES {
        return Err("项目设置 JSON 超过 512 KiB 上限，无法导出。".into());
    }
    Ok(encoded)
}

pub fn parse_project_settings_patch(raw: &str) -> Result<ProjectSettingsPatch, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("剪贴板中没有可导入的项目设置 JSON 文本。".into());
    }
    if trimmed.len() > MAX_PROJECT_SETTINGS_JSON_BYTES {
        return Err("项目设置 JSON 超过 512 KiB 上限；未导入。".into());
    }

    let mut unknown_paths = Vec::new();
    let mut decoder = serde_json::Deserializer::from_str(trimmed);
    let doc: ProjectSettingsImportDoc = serde_ignored::deserialize(&mut decoder, |path| {
        unknown_paths.push(path.to_string());
    })
    .map_err(|err| format!("项目设置 JSON 格式或字段无效：{err}"))?;
    decoder
        .end()
        .map_err(|err| format!("项目设置 JSON 末尾包含多余字符：{err}"))?;
    if let Some(first) = unknown_paths.first() {
        return Err(format!("项目设置 JSON 包含未知字段 `{first}`。"));
    }

    let has_any_section = doc.has_any_section();
    if doc.format.is_none() && !has_any_section {
        return Err("项目设置 JSON 不能为空对象 `{}`。".into());
    }
    if let Some(ref tag) = doc.format
        && tag != PROJECT_SETTINGS_JSON_FORMAT
    {
        return Err(format!("不支持的项目设置 JSON 版本：`{tag}`。"));
    }
    if !has_any_section {
        return Err("项目设置 JSON 未包含任何可导入的项目配置段。".into());
    }

    let patch = doc.into_patch()?;
    patch.validate_present()?;
    Ok(patch)
}

pub fn parse_project_settings_json(raw: &str) -> Result<ProjectSettingsSubset, String> {
    let patch = parse_project_settings_patch(raw)?;
    let subset = patch.into_subset();
    subset.validate()?;
    Ok(subset)
}

fn deserialize_some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(raw: &str, err_msg: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|_| err_msg.to_owned())
}

const fn default_macro_max_retries() -> u8 {
    1
}

const fn default_macro_timeout_ms() -> u64 {
    10_000
}

#[derive(Serialize)]
struct ProjectSettingsExportDoc<'a> {
    format: &'static str,
    repeater_settings: &'a RepeaterSettings,
    network_http: &'a NetworkHttpSettings,
    upstream_proxy: &'a UpstreamProxySettings,
    upstream_tls: UpstreamTlsJson,
    proxy_listeners: &'a [ProxyListenerConfig],
    client_certificates: Vec<ClientCertificateJson>,
    scope: &'a TargetScope,
    match_and_replace_rules: &'a [MatchAndReplaceRule],
    ssl_pass_through_rules: &'a [SslPassThroughRule],
    ws_match_and_replace_rules: &'a [WsMatchAndReplaceRule],
    proxy_interception: ProxyInterceptionSettingsJson,
    proxy_messages: ProxyMessageSettingsJson,
    proxy_websocket_interception: ProxyWebSocketInterceptionSettingsJson,
    sessions: SessionSettingsJson,
    tasks_auto_start: bool,
    scheduled_tasks: &'a [ScheduledTask],
}

impl<'a> From<&'a ProjectSettingsSubset> for ProjectSettingsExportDoc<'a> {
    fn from(s: &'a ProjectSettingsSubset) -> Self {
        Self {
            format: PROJECT_SETTINGS_JSON_FORMAT,
            repeater_settings: &s.repeater_settings,
            network_http: &s.network_http,
            upstream_proxy: &s.upstream_proxy,
            upstream_tls: UpstreamTlsJson::from(&s.upstream_tls),
            proxy_listeners: &s.proxy_listeners,
            client_certificates: s
                .client_certificates
                .iter()
                .map(ClientCertificateJson::from)
                .collect(),
            scope: &s.scope,
            match_and_replace_rules: &s.match_and_replace_rules,
            ssl_pass_through_rules: &s.ssl_pass_through_rules,
            ws_match_and_replace_rules: &s.ws_match_and_replace_rules,
            proxy_interception: ProxyInterceptionSettingsJson::from(&s.proxy_interception),
            proxy_messages: ProxyMessageSettingsJson::from(s.proxy_messages),
            proxy_websocket_interception: ProxyWebSocketInterceptionSettingsJson::from(
                s.proxy_websocket_interception,
            ),
            sessions: SessionSettingsJson::from(&s.sessions),
            tasks_auto_start: s.tasks_auto_start,
            scheduled_tasks: &s.scheduled_tasks,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ProjectSettingsImportDoc {
    #[serde(default, deserialize_with = "deserialize_some")]
    format: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some")]
    repeater_settings: Option<RepeaterSettings>,
    #[serde(default, deserialize_with = "deserialize_some")]
    network_http: Option<NetworkHttpSettings>,
    #[serde(default, deserialize_with = "deserialize_some")]
    upstream_proxy: Option<UpstreamProxyImportJson>,
    #[serde(default, deserialize_with = "deserialize_some")]
    upstream_tls: Option<UpstreamTlsImportJson>,
    #[serde(default, deserialize_with = "deserialize_some")]
    proxy_listeners: Option<Vec<ProxyListenerConfig>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    client_certificates: Option<Vec<ClientCertificateJson>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    scope: Option<TargetScope>,
    #[serde(default, deserialize_with = "deserialize_some")]
    match_and_replace_rules: Option<Vec<MatchAndReplaceRule>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    ssl_pass_through_rules: Option<Vec<SslPassThroughRule>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    ws_match_and_replace_rules: Option<Vec<WsMatchAndReplaceRule>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    proxy_interception: Option<ProxyInterceptionSettingsJson>,
    #[serde(default, deserialize_with = "deserialize_some")]
    proxy_messages: Option<ProxyMessageSettingsJson>,
    #[serde(default, deserialize_with = "deserialize_some")]
    proxy_websocket_interception: Option<ProxyWebSocketInterceptionSettingsJson>,
    #[serde(default, deserialize_with = "deserialize_some")]
    sessions: Option<SessionSettingsJson>,
    #[serde(default, deserialize_with = "deserialize_some")]
    tasks_auto_start: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_some")]
    scheduled_tasks: Option<Vec<ScheduledTask>>,
}

impl ProjectSettingsImportDoc {
    fn has_any_section(&self) -> bool {
        self.repeater_settings.is_some()
            || self.network_http.is_some()
            || self.upstream_proxy.is_some()
            || self.upstream_tls.is_some()
            || self.proxy_listeners.is_some()
            || self.client_certificates.is_some()
            || self.scope.is_some()
            || self.match_and_replace_rules.is_some()
            || self.ssl_pass_through_rules.is_some()
            || self.ws_match_and_replace_rules.is_some()
            || self.proxy_interception.is_some()
            || self.proxy_messages.is_some()
            || self.proxy_websocket_interception.is_some()
            || self.sessions.is_some()
            || self.tasks_auto_start.is_some()
            || self.scheduled_tasks.is_some()
    }

    fn into_patch(self) -> Result<ProjectSettingsPatch, String> {
        let upstream_tls = self
            .upstream_tls
            .map(UpstreamTlsImportJson::into_core)
            .transpose()?;
        let client_certificates = self
            .client_certificates
            .map(|certs| {
                certs
                    .into_iter()
                    .map(ClientCertificateJson::into_core)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let sessions = self
            .sessions
            .map(SessionSettingsJson::into_core)
            .transpose()?;

        Ok(ProjectSettingsPatch {
            repeater_settings: self.repeater_settings,
            network_http: self.network_http,
            upstream_proxy: self
                .upstream_proxy
                .map(UpstreamProxyImportJson::into_core),
            upstream_tls,
            proxy_listeners: self.proxy_listeners,
            client_certificates,
            scope: self.scope,
            match_and_replace_rules: self.match_and_replace_rules,
            ssl_pass_through_rules: self.ssl_pass_through_rules,
            ws_match_and_replace_rules: self.ws_match_and_replace_rules,
            proxy_interception: self
                .proxy_interception
                .map(ProxyInterceptionSettings::from),
            proxy_messages: self.proxy_messages.map(ProxyMessageSettings::from),
            proxy_websocket_interception: self
                .proxy_websocket_interception
                .map(ProxyWebSocketInterceptionSettings::from),
            sessions,
            tasks_auto_start: self.tasks_auto_start,
            scheduled_tasks: self.scheduled_tasks,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct UpstreamTlsJson {
    custom_ca_ders_b64: Vec<String>,
    include_system_roots: bool,
    insecure_skip_verify: bool,
    protocol_mode: TlsProtocolMode,
    custom_protocols: Vec<String>,
    custom_cipher_suites: Vec<String>,
    disable_session_resume: bool,
    client_certs_override_project_only: bool,
}

impl Default for UpstreamTlsJson {
    fn default() -> Self {
        Self::from(&UpstreamTlsSettings::default())
    }
}

impl From<&UpstreamTlsSettings> for UpstreamTlsJson {
    fn from(tls: &UpstreamTlsSettings) -> Self {
        Self {
            custom_ca_ders_b64: tls.custom_ca_ders.iter().map(|d| b64_encode(d)).collect(),
            include_system_roots: tls.include_system_roots,
            insecure_skip_verify: tls.insecure_skip_verify,
            protocol_mode: tls.protocol_mode,
            custom_protocols: tls.custom_protocols.clone(),
            custom_cipher_suites: tls.custom_cipher_suites.clone(),
            disable_session_resume: tls.disable_session_resume,
            client_certs_override_project_only: tls.client_certs_override_project_only,
        }
    }
}

impl UpstreamTlsJson {
    #[cfg(test)]
    #[allow(dead_code)]
    fn into_core(self) -> Result<UpstreamTlsSettings, String> {
        UpstreamTlsImportJson {
            custom_ca_ders_b64: self.custom_ca_ders_b64,
            include_system_roots: self.include_system_roots,
            insecure_skip_verify: self.insecure_skip_verify,
            protocol_mode: self.protocol_mode,
            custom_protocols: Some(self.custom_protocols),
            custom_cipher_suites: self.custom_cipher_suites,
            disable_session_resume: self.disable_session_resume,
            client_certs_override_project_only: self.client_certs_override_project_only,
            allow_unsafe_renegotiation: None,
        }
        .into_core()
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct UpstreamTlsImportJson {
    custom_ca_ders_b64: Vec<String>,
    include_system_roots: bool,
    insecure_skip_verify: bool,
    protocol_mode: TlsProtocolMode,
    custom_protocols: Option<Vec<String>>,
    custom_cipher_suites: Vec<String>,
    disable_session_resume: bool,
    client_certs_override_project_only: bool,
    allow_unsafe_renegotiation: Option<bool>,
}

impl Default for UpstreamTlsImportJson {
    fn default() -> Self {
        let base = UpstreamTlsJson::default();
        Self {
            custom_ca_ders_b64: base.custom_ca_ders_b64,
            include_system_roots: base.include_system_roots,
            insecure_skip_verify: base.insecure_skip_verify,
            protocol_mode: base.protocol_mode,
            custom_protocols: None,
            custom_cipher_suites: base.custom_cipher_suites,
            disable_session_resume: base.disable_session_resume,
            client_certs_override_project_only: base.client_certs_override_project_only,
            allow_unsafe_renegotiation: None,
        }
    }
}

impl UpstreamTlsImportJson {
    fn into_core(self) -> Result<UpstreamTlsSettings, String> {
        let _ = self.allow_unsafe_renegotiation;
        let custom_ca_ders = self
            .custom_ca_ders_b64
            .iter()
            .map(|s| b64_decode(s, "上游 TLS 自定义根证书 Base64 解码失败。"))
            .collect::<Result<Vec<_>, _>>()?;
        let custom_protocols = match self.custom_protocols {
            Some(list) => list,
            None if self.protocol_mode == TlsProtocolMode::Custom => {
                vec!["TLSv1.3".into(), "TLSv1.2".into()]
            }
            None => Vec::new(),
        };
        Ok(UpstreamTlsSettings {
            custom_ca_ders,
            include_system_roots: self.include_system_roots,
            insecure_skip_verify: self.insecure_skip_verify,
            protocol_mode: self.protocol_mode,
            custom_protocols,
            custom_cipher_suites: self.custom_cipher_suites,
            disable_session_resume: self.disable_session_resume,
            client_certs_override_project_only: self.client_certs_override_project_only,
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct UpstreamProxyImportJson {
    protocol: Option<UpstreamProxyProtocol>,
    host: String,
    port: u16,
    username: String,
    password: String,
    rules: Vec<UpstreamProxyRule>,
    socks: SocksProxySettings,
    open_ended_response_timeout_secs: u64,
    hostname_resolutions: Vec<HostnameResolutionRule>,
    platform_auth: PlatformAuthSettings,
    override_project_only: bool,
}

impl UpstreamProxyImportJson {
    fn into_core(self) -> UpstreamProxySettings {
        let mut rules = self.rules;
        if rules.is_empty()
            && let Some(protocol) = self.protocol
            && protocol != UpstreamProxyProtocol::Direct
            && (!self.host.trim().is_empty() || self.port != 0)
        {
            let auth_type = if self.username.is_empty() && self.password.is_empty() {
                UpstreamAuthType::None
            } else {
                UpstreamAuthType::Basic
            };
            rules.push(UpstreamProxyRule {
                protocol,
                host: self.host,
                port: self.port,
                auth_type,
                username: self.username,
                password: self.password,
                ..UpstreamProxyRule::default()
            });
        }
        UpstreamProxySettings {
            rules,
            socks: self.socks,
            open_ended_response_timeout_secs: self.open_ended_response_timeout_secs,
            hostname_resolutions: self.hostname_resolutions,
            platform_auth: self.platform_auth,
            override_project_only: self.override_project_only,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ClientCertificateJson {
    #[serde(default = "default_true_bool")]
    enabled: bool,
    host_pattern: String,
    #[serde(default)]
    format: ClientCertificateFormat,
    certificate_b64: String,
    #[serde(default)]
    private_key_pem_b64: String,
    #[serde(default)]
    password: String,
}

impl From<&ClientCertificateRule> for ClientCertificateJson {
    fn from(c: &ClientCertificateRule) -> Self {
        Self {
            enabled: c.enabled,
            host_pattern: c.host_pattern.clone(),
            format: c.format,
            certificate_b64: b64_encode(&c.certificate_bytes),
            private_key_pem_b64: b64_encode(&c.private_key_pem),
            password: c.password.clone(),
        }
    }
}

impl ClientCertificateJson {
    fn into_core(self) -> Result<ClientCertificateRule, String> {
        let certificate_bytes =
            b64_decode(&self.certificate_b64, "客户端证书 Base64 解码失败。")?;
        let private_key_pem =
            b64_decode(&self.private_key_pem_b64, "客户端证书私钥 Base64 解码失败。")?;
        Ok(ClientCertificateRule {
            enabled: self.enabled,
            host_pattern: self.host_pattern,
            format: self.format,
            certificate_bytes,
            private_key_pem,
            password: self.password,
        })
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum ProxyInterceptOperatorJson {
    #[serde(alias = "and")]
    And,
    #[serde(alias = "or")]
    Or,
}

impl From<ProxyInterceptOperator> for ProxyInterceptOperatorJson {
    fn from(op: ProxyInterceptOperator) -> Self {
        match op {
            ProxyInterceptOperator::And => Self::And,
            ProxyInterceptOperator::Or => Self::Or,
        }
    }
}

impl From<ProxyInterceptOperatorJson> for ProxyInterceptOperator {
    fn from(op: ProxyInterceptOperatorJson) -> Self {
        match op {
            ProxyInterceptOperatorJson::And => Self::And,
            ProxyInterceptOperatorJson::Or => Self::Or,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ProxyInterceptRuleJson {
    id: Uuid,
    #[serde(default = "default_true_bool")]
    enabled: bool,
    operator: ProxyInterceptOperatorJson,
    match_type: ProxyInterceptType,
    relationship: ProxyInterceptRelation,
    #[serde(default)]
    condition: String,
}

impl From<&ProxyInterceptRule> for ProxyInterceptRuleJson {
    fn from(r: &ProxyInterceptRule) -> Self {
        Self {
            id: r.id,
            enabled: r.enabled,
            operator: ProxyInterceptOperatorJson::from(r.operator),
            match_type: r.match_type,
            relationship: r.relationship,
            condition: r.condition.clone(),
        }
    }
}

impl From<ProxyInterceptRuleJson> for ProxyInterceptRule {
    fn from(r: ProxyInterceptRuleJson) -> Self {
        Self {
            id: r.id,
            enabled: r.enabled,
            operator: r.operator.into(),
            match_type: r.match_type,
            relationship: r.relationship,
            condition: r.condition,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ProxyInterceptRuleSetJson {
    enabled: bool,
    rules: Vec<ProxyInterceptRuleJson>,
}

impl From<&ProxyInterceptRuleSet> for ProxyInterceptRuleSetJson {
    fn from(set: &ProxyInterceptRuleSet) -> Self {
        Self {
            enabled: set.enabled,
            rules: set.rules.iter().map(ProxyInterceptRuleJson::from).collect(),
        }
    }
}

impl From<ProxyInterceptRuleSetJson> for ProxyInterceptRuleSet {
    fn from(set: ProxyInterceptRuleSetJson) -> Self {
        Self {
            enabled: set.enabled,
            rules: set.rules.into_iter().map(Into::into).collect(),
        }
    }
}

fn default_request_rule_set_json() -> ProxyInterceptRuleSetJson {
    ProxyInterceptRuleSetJson::from(&ProxyInterceptionSettings::default().request)
}

fn default_response_rule_set_json() -> ProxyInterceptRuleSetJson {
    ProxyInterceptRuleSetJson::from(&ProxyInterceptionSettings::default().response)
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct ProxyInterceptionSettingsJson {
    #[serde(default = "default_request_rule_set_json")]
    request: ProxyInterceptRuleSetJson,
    #[serde(default = "default_response_rule_set_json")]
    response: ProxyInterceptRuleSetJson,
}

impl Default for ProxyInterceptionSettingsJson {
    fn default() -> Self {
        Self::from(&ProxyInterceptionSettings::default())
    }
}

impl From<&ProxyInterceptionSettings> for ProxyInterceptionSettingsJson {
    fn from(s: &ProxyInterceptionSettings) -> Self {
        Self {
            request: ProxyInterceptRuleSetJson::from(&s.request),
            response: ProxyInterceptRuleSetJson::from(&s.response),
        }
    }
}

impl From<ProxyInterceptionSettingsJson> for ProxyInterceptionSettings {
    fn from(s: ProxyInterceptionSettingsJson) -> Self {
        Self {
            request: s.request.into(),
            response: s.response.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct ProxyMessageSettingsJson {
    unpack_compressed_requests: bool,
    unpack_compressed_responses: bool,
    match_replace_in_scope_only: bool,
    ws_match_replace_in_scope_only: bool,
    auto_update_request_content_length: bool,
    auto_fix_request_newlines: bool,
    auto_update_response_content_length: bool,
    ssl_pass_through_auto_add_on_failure: bool,
    response_modification: ProxyResponseModificationSettings,
    misc: ProxyMiscSettings,
}

impl Default for ProxyMessageSettingsJson {
    fn default() -> Self {
        Self::from(ProxyMessageSettings::default())
    }
}

impl From<ProxyMessageSettings> for ProxyMessageSettingsJson {
    fn from(m: ProxyMessageSettings) -> Self {
        Self {
            unpack_compressed_requests: m.unpack_compressed_requests,
            unpack_compressed_responses: m.unpack_compressed_responses,
            match_replace_in_scope_only: m.match_replace_in_scope_only,
            ws_match_replace_in_scope_only: m.ws_match_replace_in_scope_only,
            auto_update_request_content_length: m.auto_update_request_content_length,
            auto_fix_request_newlines: m.auto_fix_request_newlines,
            auto_update_response_content_length: m.auto_update_response_content_length,
            ssl_pass_through_auto_add_on_failure: m.ssl_pass_through_auto_add_on_failure,
            response_modification: m.response_modification,
            misc: m.misc,
        }
    }
}

impl From<ProxyMessageSettingsJson> for ProxyMessageSettings {
    fn from(m: ProxyMessageSettingsJson) -> Self {
        Self {
            unpack_compressed_requests: m.unpack_compressed_requests,
            unpack_compressed_responses: m.unpack_compressed_responses,
            match_replace_in_scope_only: m.match_replace_in_scope_only,
            ws_match_replace_in_scope_only: m.ws_match_replace_in_scope_only,
            auto_update_request_content_length: m.auto_update_request_content_length,
            auto_fix_request_newlines: m.auto_fix_request_newlines,
            auto_update_response_content_length: m.auto_update_response_content_length,
            ssl_pass_through_auto_add_on_failure: m.ssl_pass_through_auto_add_on_failure,
            response_modification: m.response_modification,
            misc: m.misc,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct ProxyWebSocketInterceptionSettingsJson {
    client_to_server_messages: bool,
    server_to_client_messages: bool,
    intercept_in_scope_only: bool,
}

impl Default for ProxyWebSocketInterceptionSettingsJson {
    fn default() -> Self {
        Self::from(ProxyWebSocketInterceptionSettings::default())
    }
}

impl From<ProxyWebSocketInterceptionSettings> for ProxyWebSocketInterceptionSettingsJson {
    fn from(ws: ProxyWebSocketInterceptionSettings) -> Self {
        Self {
            client_to_server_messages: ws.client_to_server_messages,
            server_to_client_messages: ws.server_to_client_messages,
            intercept_in_scope_only: ws.intercept_in_scope_only,
        }
    }
}

impl From<ProxyWebSocketInterceptionSettingsJson> for ProxyWebSocketInterceptionSettings {
    fn from(ws: ProxyWebSocketInterceptionSettingsJson) -> Self {
        Self {
            client_to_server_messages: ws.client_to_server_messages,
            server_to_client_messages: ws.server_to_client_messages,
            intercept_in_scope_only: ws.intercept_in_scope_only,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct SessionSettingsJson {
    cookie_jar: CookieJarSettings,
    rules: Vec<SessionRuleJson>,
    macros: Vec<SessionMacroJson>,
}

impl From<&SessionSettings> for SessionSettingsJson {
    fn from(s: &SessionSettings) -> Self {
        Self {
            cookie_jar: s.cookie_jar.clone(),
            rules: s.rules.iter().map(SessionRuleJson::from).collect(),
            macros: s.macros.iter().map(SessionMacroJson::from).collect(),
        }
    }
}

impl SessionSettingsJson {
    fn into_core(self) -> Result<SessionSettings, String> {
        let macros = self
            .macros
            .into_iter()
            .map(SessionMacroJson::into_core)
            .collect::<Result<Vec<_>, _>>()?;
        let rules = self
            .rules
            .into_iter()
            .map(SessionRuleJson::into_core)
            .collect();
        Ok(SessionSettings {
            cookie_jar: self.cookie_jar,
            rules,
            macros,
        })
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MacroPolicyKindJson {
    #[default]
    Abort,
    Retry,
}

#[derive(Serialize, Deserialize)]
struct SessionMacroJson {
    id: Uuid,
    name: String,
    #[serde(default)]
    policy: MacroPolicyKindJson,
    #[serde(default = "default_macro_max_retries")]
    max_retries: u8,
    #[serde(default = "default_macro_timeout_ms")]
    timeout_ms: u64,
    steps: Vec<MacroStepJson>,
}

impl From<&SessionMacro> for SessionMacroJson {
    fn from(m: &SessionMacro) -> Self {
        let (policy, max_retries) = match m.failure_policy {
            MacroFailurePolicy::Abort => (MacroPolicyKindJson::Abort, 0),
            MacroFailurePolicy::Retry { max_retries } => (MacroPolicyKindJson::Retry, max_retries),
        };
        Self {
            id: m.id,
            name: m.name.clone(),
            policy,
            max_retries,
            timeout_ms: m.timeout_ms,
            steps: m.steps.iter().map(MacroStepJson::from).collect(),
        }
    }
}

impl SessionMacroJson {
    fn into_core(self) -> Result<SessionMacro, String> {
        let failure_policy = match self.policy {
            MacroPolicyKindJson::Abort => MacroFailurePolicy::Abort,
            MacroPolicyKindJson::Retry => MacroFailurePolicy::Retry {
                max_retries: self.max_retries,
            },
        };
        let steps = self
            .steps
            .into_iter()
            .map(MacroStepJson::into_core)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SessionMacro {
            id: self.id,
            name: self.name,
            steps,
            failure_policy,
            timeout_ms: self.timeout_ms,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct MacroHeaderJson {
    name: String,
    value_b64: String,
}

impl From<&Header> for MacroHeaderJson {
    fn from(h: &Header) -> Self {
        Self {
            name: h.name.clone(),
            value_b64: b64_encode(&h.value),
        }
    }
}

impl MacroHeaderJson {
    fn into_core(self) -> Result<Header, String> {
        let value = b64_decode(&self.value_b64, "Macro 步骤请求头 Base64 解码失败。")?;
        Ok(Header {
            name: self.name,
            value,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct MacroStepJson {
    id: Uuid,
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<MacroHeaderJson>,
    #[serde(default)]
    body_b64: String,
    #[serde(default = "default_true_bool")]
    use_cookie_jar: bool,
    #[serde(default)]
    expected_status: Option<u16>,
    #[serde(default)]
    extractions: Vec<MacroExtractionJson>,
}

impl From<&MacroStep> for MacroStepJson {
    fn from(st: &MacroStep) -> Self {
        Self {
            id: st.id,
            method: st.method.clone(),
            url: st.url.clone(),
            headers: st.headers.iter().map(MacroHeaderJson::from).collect(),
            body_b64: b64_encode(&st.body),
            use_cookie_jar: st.use_cookie_jar,
            expected_status: st.expected_status,
            extractions: st
                .extractions
                .iter()
                .map(MacroExtractionJson::from)
                .collect(),
        }
    }
}

impl MacroStepJson {
    fn into_core(self) -> Result<MacroStep, String> {
        let headers = self
            .headers
            .into_iter()
            .map(MacroHeaderJson::into_core)
            .collect::<Result<Vec<_>, _>>()?;
        let body = b64_decode(&self.body_b64, "Macro 步骤正文 Base64 解码失败。")?;
        let extractions = self
            .extractions
            .into_iter()
            .map(MacroExtractionJson::into_core)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MacroStep {
            id: self.id,
            method: self.method,
            url: self.url,
            headers,
            body,
            use_cookie_jar: self.use_cookie_jar,
            expected_status: self.expected_status,
            extractions,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MacroExtractSourceKindJson {
    Delimiter,
    Regex,
    ResponseHeader,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MacroApplyTargetKindJson {
    #[default]
    Param,
    Header,
    Cookie,
}

#[derive(Serialize, Deserialize)]
struct MacroExtractionJson {
    name: String,
    source_kind: MacroExtractSourceKindJson,
    arg1: String,
    #[serde(default, deserialize_with = "deserialize_some")]
    arg2: Option<String>,
    #[serde(default)]
    target_kind: MacroApplyTargetKindJson,
    #[serde(default)]
    target_name: String,
}

impl From<&MacroExtraction> for MacroExtractionJson {
    fn from(ex: &MacroExtraction) -> Self {
        let (source_kind, arg1, arg2) = match &ex.source {
            MacroExtractSource::Delimiter { start, end } => (
                MacroExtractSourceKindJson::Delimiter,
                start.clone(),
                Some(end.clone()),
            ),
            MacroExtractSource::Regex { pattern } => (
                MacroExtractSourceKindJson::Regex,
                pattern.clone(),
                Some(String::new()),
            ),
            MacroExtractSource::ResponseHeader { header_name } => (
                MacroExtractSourceKindJson::ResponseHeader,
                header_name.clone(),
                Some(String::new()),
            ),
        };
        let (target_kind, target_name) = match &ex.target {
            MacroApplyTarget::UrlOrBodyParam => (MacroApplyTargetKindJson::Param, String::new()),
            MacroApplyTarget::Header { header_name } => {
                (MacroApplyTargetKindJson::Header, header_name.clone())
            }
            MacroApplyTarget::Cookie { cookie_name } => {
                (MacroApplyTargetKindJson::Cookie, cookie_name.clone())
            }
        };
        Self {
            name: ex.name.clone(),
            source_kind,
            arg1,
            arg2,
            target_kind,
            target_name,
        }
    }
}

impl MacroExtractionJson {
    fn into_core(self) -> Result<MacroExtraction, String> {
        let source = match self.source_kind {
            MacroExtractSourceKindJson::Delimiter => MacroExtractSource::Delimiter {
                start: self.arg1,
                end: self
                    .arg2
                    .ok_or_else(|| "字段 `arg2` 必须是字符串。".to_owned())?,
            },
            MacroExtractSourceKindJson::Regex => MacroExtractSource::Regex { pattern: self.arg1 },
            MacroExtractSourceKindJson::ResponseHeader => {
                MacroExtractSource::ResponseHeader {
                    header_name: self.arg1,
                }
            }
        };
        let target = match self.target_kind {
            MacroApplyTargetKindJson::Param => MacroApplyTarget::UrlOrBodyParam,
            MacroApplyTargetKindJson::Header => MacroApplyTarget::Header {
                header_name: self.target_name,
            },
            MacroApplyTargetKindJson::Cookie => MacroApplyTarget::Cookie {
                cookie_name: self.target_name,
            },
        };
        Ok(MacroExtraction {
            name: self.name,
            source,
            target,
        })
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionUrlModeJson {
    #[default]
    AllUrls,
    SuiteScope,
    CustomPrefixes,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionParamModeJson {
    #[default]
    None,
    RequirePresent,
    RequireAbsent,
}

// Internally tagged enum buffers via serde::Content, bypassing serde_ignored; deny_unknown_fields
// enforces strict rejection inside every variant.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum SessionRuleActionJson {
    UseCookieJar {
        #[serde(default = "default_true_bool")]
        add_new_cookies: bool,
        #[serde(default = "default_true_bool")]
        update_existing_cookies: bool,
    },
    SetHeader {
        name: String,
        value: String,
        #[serde(default)]
        only_if_present: bool,
    },
    SetCookie {
        name: String,
        value: String,
        #[serde(default)]
        only_if_present: bool,
    },
    RewriteUrlParam {
        name: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        remove: bool,
        #[serde(default)]
        only_if_present: bool,
    },
    ReplaceRequestText {
        match_pattern: String,
        #[serde(default)]
        replace_with: String,
        #[serde(default)]
        regex: bool,
    },
    RunMacro {
        macro_id: Uuid,
        #[serde(default = "default_true_bool")]
        update_cookies_from_jar: bool,
        #[serde(default = "default_true_bool")]
        apply_extractions: bool,
    },
}

impl From<&SessionRuleAction> for SessionRuleActionJson {
    fn from(a: &SessionRuleAction) -> Self {
        match a {
            SessionRuleAction::UseCookieJar {
                add_new_cookies,
                update_existing_cookies,
            } => Self::UseCookieJar {
                add_new_cookies: *add_new_cookies,
                update_existing_cookies: *update_existing_cookies,
            },
            SessionRuleAction::SetHeader {
                name,
                value,
                only_if_present,
            } => Self::SetHeader {
                name: name.clone(),
                value: value.clone(),
                only_if_present: *only_if_present,
            },
            SessionRuleAction::SetCookie {
                name,
                value,
                only_if_present,
            } => Self::SetCookie {
                name: name.clone(),
                value: value.clone(),
                only_if_present: *only_if_present,
            },
            SessionRuleAction::RewriteUrlParam {
                name,
                value,
                remove,
                only_if_present,
            } => Self::RewriteUrlParam {
                name: name.clone(),
                value: value.clone(),
                remove: *remove,
                only_if_present: *only_if_present,
            },
            SessionRuleAction::ReplaceRequestText {
                match_pattern,
                replace_with,
                regex,
            } => Self::ReplaceRequestText {
                match_pattern: match_pattern.clone(),
                replace_with: replace_with.clone(),
                regex: *regex,
            },
            SessionRuleAction::RunMacro {
                macro_id,
                update_cookies_from_jar,
                apply_extractions,
            } => Self::RunMacro {
                macro_id: *macro_id,
                update_cookies_from_jar: *update_cookies_from_jar,
                apply_extractions: *apply_extractions,
            },
        }
    }
}

impl From<SessionRuleActionJson> for SessionRuleAction {
    fn from(a: SessionRuleActionJson) -> Self {
        match a {
            SessionRuleActionJson::UseCookieJar {
                add_new_cookies,
                update_existing_cookies,
            } => Self::UseCookieJar {
                add_new_cookies,
                update_existing_cookies,
            },
            SessionRuleActionJson::SetHeader {
                name,
                value,
                only_if_present,
            } => Self::SetHeader {
                name,
                value,
                only_if_present,
            },
            SessionRuleActionJson::SetCookie {
                name,
                value,
                only_if_present,
            } => Self::SetCookie {
                name,
                value,
                only_if_present,
            },
            SessionRuleActionJson::RewriteUrlParam {
                name,
                value,
                remove,
                only_if_present,
            } => Self::RewriteUrlParam {
                name,
                value,
                remove,
                only_if_present,
            },
            SessionRuleActionJson::ReplaceRequestText {
                match_pattern,
                replace_with,
                regex,
            } => Self::ReplaceRequestText {
                match_pattern,
                replace_with,
                regex,
            },
            SessionRuleActionJson::RunMacro {
                macro_id,
                update_cookies_from_jar,
                apply_extractions,
            } => Self::RunMacro {
                macro_id,
                update_cookies_from_jar,
                apply_extractions,
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SessionRuleJson {
    id: Uuid,
    name: String,
    #[serde(default = "default_true_bool")]
    enabled: bool,
    #[serde(default)]
    tools: SessionToolScope,
    #[serde(default)]
    url_mode: SessionUrlModeJson,
    #[serde(default)]
    url_prefixes: Vec<String>,
    #[serde(default)]
    param_mode: SessionParamModeJson,
    #[serde(default)]
    param_names: Vec<String>,
    #[serde(default)]
    header_contains: Option<String>,
    actions: Vec<SessionRuleActionJson>,
}

impl From<&SessionRule> for SessionRuleJson {
    fn from(r: &SessionRule) -> Self {
        let (url_mode, url_prefixes) = match &r.condition.url_scope {
            SessionUrlScope::AllUrls => (SessionUrlModeJson::AllUrls, Vec::new()),
            SessionUrlScope::SuiteScope => (SessionUrlModeJson::SuiteScope, Vec::new()),
            SessionUrlScope::CustomPrefixes(list) => {
                (SessionUrlModeJson::CustomPrefixes, list.clone())
            }
        };
        let (param_mode, param_names) = match &r.condition.param_condition {
            None => (SessionParamModeJson::None, Vec::new()),
            Some(SessionParamCondition::RequirePresent(names)) => {
                (SessionParamModeJson::RequirePresent, names.clone())
            }
            Some(SessionParamCondition::RequireAbsent(names)) => {
                (SessionParamModeJson::RequireAbsent, names.clone())
            }
        };
        Self {
            id: r.id,
            name: r.name.clone(),
            enabled: r.enabled,
            tools: r.tools,
            url_mode,
            url_prefixes,
            param_mode,
            param_names,
            header_contains: r.condition.header_contains.clone(),
            actions: r.actions.iter().map(SessionRuleActionJson::from).collect(),
        }
    }
}

impl SessionRuleJson {
    fn into_core(self) -> SessionRule {
        let url_scope = match self.url_mode {
            SessionUrlModeJson::AllUrls => SessionUrlScope::AllUrls,
            SessionUrlModeJson::SuiteScope => SessionUrlScope::SuiteScope,
            SessionUrlModeJson::CustomPrefixes => {
                SessionUrlScope::CustomPrefixes(self.url_prefixes)
            }
        };
        let param_condition = match self.param_mode {
            SessionParamModeJson::None => None,
            SessionParamModeJson::RequirePresent => {
                Some(SessionParamCondition::RequirePresent(self.param_names))
            }
            SessionParamModeJson::RequireAbsent => {
                Some(SessionParamCondition::RequireAbsent(self.param_names))
            }
        };
        SessionRule {
            id: self.id,
            name: self.name,
            enabled: self.enabled,
            tools: self.tools,
            condition: SessionRuleCondition {
                url_scope,
                param_condition,
                header_contains: self.header_contains,
            },
            actions: self.actions.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_settings_json_roundtrip_all_sections() {
        let mut ws = Workspace::default();
        ws.repeater_settings.unpack_compressed_responses = false;
        ws.repeater_settings.follow_redirects = RepeaterRedirectMode::InScopeOnly;
        ws.repeater_settings.request_timeout_secs = 45;
        ws.repeater_settings.layout = RepeaterLayoutMode::Combined;
        ws.repeater_settings.http1_connection_reuse = true;
        ws.repeater_settings.http2_connection_reuse = false;
        ws.repeater_settings.allow_http2_alpn_override = true;
        ws.repeater_settings.process_cookies_in_redirects = true;
        ws.repeater_settings.cross_domain_protocol_redirects = true;
        ws.repeater_settings.streaming_response_timeout_secs = 123;

        ws.network_http.redirect_3xx = false;
        ws.network_http.redirect_javascript = true;
        ws.network_http.redirect_any_location = true;
        ws.network_http.streaming_advanced_scope = true;
        ws.network_http.streaming_scope_rules.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://stream.example.invalid/events".into(),
            include_subdomains: true,
        });

        ws.upstream_proxy.override_project_only = true;
        ws.upstream_proxy.rules.push(UpstreamProxyRule {
            id: Uuid::new_v4(),
            enabled: true,
            destination_host: "*.lab.example.invalid".into(),
            protocol: UpstreamProxyProtocol::Https,
            host: "proxy.example.invalid".into(),
            port: 8443,
            auth_type: UpstreamAuthType::Basic,
            username: "u1".into(),
            password: "p1".into(),
            domain: String::new(),
            workstation: String::new(),
            description: "lab proxy".into(),
        });
        ws.upstream_proxy.rules.push(UpstreamProxyRule {
            id: Uuid::new_v4(),
            enabled: true,
            destination_host: "*.ntlm.example.invalid".into(),
            protocol: UpstreamProxyProtocol::Http,
            host: "ntlm-proxy.example.invalid".into(),
            port: 3128,
            auth_type: UpstreamAuthType::NtlmV2,
            username: "u2".into(),
            password: "p2".into(),
            domain: "LAB".into(),
            workstation: "WKSTN1".into(),
            description: "ntlm proxy".into(),
        });
        ws.upstream_proxy.platform_auth = PlatformAuthSettings {
            enabled: true,
            prompt_on_failure: true,
            override_project_only: true,
            rules: vec![PlatformAuthRule {
                id: Uuid::new_v4(),
                enabled: true,
                destination_host: "app.example.invalid".into(),
                auth_type: PlatformAuthType::NtlmV2,
                username: "alice".into(),
                password: "secret".into(),
                domain: "LAB".into(),
                hostname: "CLIENT1".into(),
            }],
        };
        ws.upstream_proxy.socks = SocksProxySettings {
            use_proxy: false,
            host: "127.0.0.1".into(),
            port: 1080,
            username: String::new(),
            password: String::new(),
            dns_over_socks: false,
            bypass_hosts: vec!["localhost".into(), "*.example.invalid".into()],
            override_project_only: true,
        };
        ws.upstream_proxy
            .hostname_resolutions
            .push(HostnameResolutionRule {
                id: Uuid::new_v4(),
                enabled: true,
                hostname: "app.example.invalid".into(),
                ip_address: "127.0.0.1".into(),
            });

        let ca_der = mio_proxy::MitmAuthority::generate()
            .expect("generate test CA")
            .certificate_der();
        ws.upstream_tls.custom_ca_ders = vec![ca_der];
        ws.upstream_tls.include_system_roots = false;
        ws.upstream_tls.protocol_mode = TlsProtocolMode::Custom;
        ws.upstream_tls.custom_protocols = vec!["TLSv1.3".into(), "TLSv1.2".into()];
        ws.upstream_tls.custom_cipher_suites = vec![
            "TLS13_CHACHA20_POLY1305_SHA256".into(),
            "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256".into(),
        ];
        ws.upstream_tls.disable_session_resume = true;
        ws.upstream_tls.client_certs_override_project_only = true;

        let listener_auth = mio_proxy::MitmAuthority::generate().expect("generate listener cert");
        let listener_cert_pem = listener_auth.certificate_pem();
        let listener_key_pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            b64_encode(&listener_auth.private_key_der())
        );

        ws.proxy_listeners.push(ProxyListenerConfig {
            id: Uuid::new_v4(),
            enabled: true,
            bind_address: "127.0.0.1:8082".into(),
            invisible_proxying: true,
            redirect_to_host: "backend.example.invalid".into(),
            redirect_to_port: Some(8443),
            force_use_of_tls: true,
            certificate_mode: ProxyListenerCertMode::CustomCertificate,
            specific_cert_hostname: String::new(),
            enable_http2: false,
            tls_protocols_label: "TLSv1.3 only".into(),
            custom_cert_pem: Some(listener_cert_pem),
            custom_key_pem: Some(listener_key_pem),
        });

        ws.client_certificates.push(ClientCertificateRule {
            enabled: true,
            host_pattern: "*.example.invalid".into(),
            format: ClientCertificateFormat::Pkcs12,
            certificate_bytes: vec![0x30, 0x82, 0x01, 0x00],
            private_key_pem: Vec::new(),
            password: "secret".into(),
        });

        ws.scope.include.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://example.invalid".into(),
            include_subdomains: true,
        });
        ws.scope.advanced_mode = true;
        ws.scope.advanced_include.push(AdvancedScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            protocol: ScopeProtocol::Https,
            host: "^app\\.example\\.invalid$".into(),
            port: "443".into(),
            file: "^/api/.*$".into(),
        });
        ws.scope.drop_out_of_scope = true;
        ws.scope.out_of_scope_use_custom = true;
        ws.scope.out_of_scope_custom_rules.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://allow.example.invalid".into(),
            include_subdomains: false,
        });

        ws.match_and_replace_rules.push(MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: "^X-Old:.*".into(),
            replace_with: "X-New: 1".into(),
            regex: true,
            comment: "rewrite header".into(),
            script_name: String::new(),
            filter_script: String::new(),
        });
        ws.ssl_pass_through_rules.push(SslPassThroughRule {
            id: Uuid::new_v4(),
            enabled: true,
            host_pattern: "*.pin.example.invalid".into(),
            port: Some(8443),
            is_regex: false,
        });
        ws.ws_match_and_replace_rules.push(WsMatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            direction: WsReplaceDirection::ServerToClient,
            match_pattern: "secret".into(),
            replace_with: "redacted".into(),
            is_regex: false,
            comment: "ws mask".into(),
        });

        ws.proxy_interception.request.enabled = false;
        ws.proxy_interception.request.rules.push(ProxyInterceptRule {
            id: Uuid::new_v4(),
            enabled: true,
            operator: ProxyInterceptOperator::Or,
            match_type: ProxyInterceptType::Url,
            relationship: ProxyInterceptRelation::Matches,
            condition: "https://example\\.invalid/admin".into(),
        });

        ws.proxy_messages.unpack_compressed_requests = true;
        ws.proxy_messages.match_replace_in_scope_only = true;
        ws.proxy_messages.response_modification.unhide_hidden_form_fields = true;
        ws.proxy_messages.misc.disable_history_logging = true;

        ws.proxy_websocket_interception.client_to_server_messages = false;
        ws.proxy_websocket_interception.intercept_in_scope_only = true;

        let macro_id = Uuid::new_v4();
        ws.sessions.cookie_jar.monitor_repeater = true;
        ws.sessions.cookie_jar.monitor_mio_ai = true;
        ws.sessions.cookie_jar.entries.push(CookieJarEntry {
            id: Uuid::new_v4(),
            domain: "example.invalid".into(),
            host_only: true,
            path: "/app".into(),
            name: "sid".into(),
            value: "v1".into(),
            secure: true,
            http_only: true,
            expires_unix_ms: Some(1_900_000_000_000),
        });
        ws.sessions.macros.push(SessionMacro {
            id: macro_id,
            name: "refresh-token".into(),
            steps: vec![MacroStep {
                id: Uuid::new_v4(),
                method: "POST".into(),
                url: "https://example.invalid/token".into(),
                headers: vec![Header {
                    name: "X-Token".into(),
                    value: b"abc".to_vec(),
                }],
                body: b"grant=refresh".to_vec(),
                use_cookie_jar: true,
                expected_status: Some(200),
                extractions: vec![
                    MacroExtraction {
                        name: "csrf".into(),
                        source: MacroExtractSource::Delimiter {
                            start: "csrf=\"".into(),
                            end: "\"".into(),
                        },
                        target: MacroApplyTarget::Header {
                            header_name: "X-CSRF".into(),
                        },
                    },
                    MacroExtraction {
                        name: "sid".into(),
                        source: MacroExtractSource::Regex {
                            pattern: "sid=([a-z0-9]+)".into(),
                        },
                        target: MacroApplyTarget::Cookie {
                            cookie_name: "sid".into(),
                        },
                    },
                    MacroExtraction {
                        name: "etag".into(),
                        source: MacroExtractSource::ResponseHeader {
                            header_name: "ETag".into(),
                        },
                        target: MacroApplyTarget::UrlOrBodyParam,
                    },
                ],
            }],
            failure_policy: MacroFailurePolicy::Retry { max_retries: 2 },
            timeout_ms: 5_000,
        });
        ws.sessions.rules.push(SessionRule {
            id: Uuid::new_v4(),
            name: "session-rule".into(),
            enabled: true,
            tools: SessionToolScope {
                proxy: true,
                repeater: true,
                intruder: false,
                scanner: true,
                sequencer: false,
                extensions: true,
                mio_ai: true,
            },
            condition: SessionRuleCondition {
                url_scope: SessionUrlScope::CustomPrefixes(vec![
                    "https://example.invalid/api".into(),
                ]),
                param_condition: Some(SessionParamCondition::RequirePresent(vec!["sid".into()])),
                header_contains: Some("Authorization".into()),
            },
            actions: vec![
                SessionRuleAction::UseCookieJar {
                    add_new_cookies: true,
                    update_existing_cookies: false,
                },
                SessionRuleAction::SetHeader {
                    name: "X-Env".into(),
                    value: "lab".into(),
                    only_if_present: false,
                },
                SessionRuleAction::SetCookie {
                    name: "ck".into(),
                    value: "1".into(),
                    only_if_present: true,
                },
                SessionRuleAction::RewriteUrlParam {
                    name: "token".into(),
                    value: "v2".into(),
                    remove: false,
                    only_if_present: false,
                },
                SessionRuleAction::ReplaceRequestText {
                    match_pattern: "OLD".into(),
                    replace_with: "NEW".into(),
                    regex: false,
                },
                SessionRuleAction::RunMacro {
                    macro_id,
                    update_cookies_from_jar: true,
                    apply_extractions: true,
                },
            ],
        });

        ws.tasks_auto_start = false;
        ws.scheduled_tasks.push(ScheduledTask {
            id: Uuid::new_v4(),
            enabled: true,
            action: ScheduledTaskAction::RunScan,
            trigger_spec: "09:00 | daily".into(),
            description: "morning scan".into(),
        });

        let exported = export_project_settings_json(&ws).expect("export must succeed");
        assert!(exported.contains("\"format\": \"mio.project_settings.v1\""));
        assert!(exported.contains("\"custom_ca_ders_b64\""));
        assert!(exported.contains("\"certificate_b64\""));
        assert!(exported.contains("\"private_key_pem_b64\""));
        assert!(exported.contains("\"platform_auth\""));
        assert!(exported.contains("\"ssl_pass_through_rules\""));
        assert!(exported.contains("\"ws_match_and_replace_rules\""));
        assert!(exported.contains("\"domain\""));
        assert!(exported.contains("\"workstation\""));

        let parsed = parse_project_settings_json(&exported).expect("parse must succeed");
        assert_eq!(parsed, ProjectSettingsSubset::from_workspace(&ws));
        assert_eq!(parsed.upstream_proxy.rules.len(), 2);
        assert_eq!(
            parsed.upstream_proxy.rules[1].auth_type,
            UpstreamAuthType::NtlmV2
        );
        assert_eq!(parsed.upstream_proxy.rules[1].domain, "LAB");
        assert_eq!(parsed.upstream_proxy.rules[1].workstation, "WKSTN1");
        assert_eq!(parsed.upstream_proxy.rules[1].description, "ntlm proxy");
        assert_eq!(parsed.upstream_proxy.platform_auth.rules.len(), 1);
        assert_eq!(parsed.match_and_replace_rules.len(), 1);
        assert_eq!(parsed.ssl_pass_through_rules.len(), 1);
        assert_eq!(parsed.ws_match_and_replace_rules.len(), 1);
    }

    #[test]
    fn project_settings_json_applies_defaults_for_omitted_sections() {
        let rule_id = Uuid::new_v4();
        let partial = format!(
            r#"{{
              "format": "mio.project_settings.v1",
              "repeater_settings": {{"request_timeout_secs": 45}},
              "scope": {{"include": [{{"id": "{rule_id}", "enabled": true, "prefix": "https://legacy.example.invalid"}}]}},
              "proxy_interception": {{"request": {{"enabled": false, "rules": []}}}},
              "proxy_messages": {{"unpack_compressed_requests": true}},
              "proxy_websocket_interception": {{"intercept_in_scope_only": true}}
            }}"#
        );

        let parsed = parse_project_settings_json(&partial).expect("partial v1 JSON must parse");
        assert_eq!(parsed.repeater_settings.request_timeout_secs, 45);
        assert_eq!(
            parsed.repeater_settings.streaming_response_timeout_secs,
            600
        );
        assert!(parsed.repeater_settings.http2_connection_reuse);
        assert!(parsed.network_http.is_default());
        assert!(parsed.upstream_tls.is_default());
        assert!(parsed.proxy_listeners.is_empty());
        assert!(parsed.tasks_auto_start);
        assert!(parsed.scheduled_tasks.is_empty());
        assert!(parsed.ssl_pass_through_rules.is_empty());
        assert!(parsed.ws_match_and_replace_rules.is_empty());
        assert!(parsed.upstream_proxy.platform_auth.is_default());
        assert!(!parsed.scope.include[0].include_subdomains);
        assert!(!parsed.proxy_interception.request.enabled);
        assert!(parsed.proxy_interception.request.rules.is_empty());
        assert_eq!(
            parsed.proxy_interception.response,
            ProxyInterceptionSettings::default().response
        );
        assert!(parsed.proxy_messages.unpack_compressed_requests);
        assert!(parsed.proxy_messages.unpack_compressed_responses);
        assert!(parsed.proxy_websocket_interception.client_to_server_messages);
        assert!(parsed.proxy_websocket_interception.server_to_client_messages);
        assert!(parsed.proxy_websocket_interception.intercept_in_scope_only);
    }

    #[test]
    fn project_settings_json_rejects_unknown_fields_and_bad_format() {
        let rule_id = Uuid::new_v4();
        let valid_rule_id = Uuid::new_v4();
        let bad_cases = [
            "",
            "{}",
            "[]",
            r#"{"format": "mio.project_settings.v1"}"#,
            r#"{"format": 123, "tasks_auto_start": false}"#,
            r#"{"format": "mio.project_settings.v99", "tasks_auto_start": false}"#,
            r#"{"tasks_auto_start": true, "unknown_top": 1}"#,
            r#"{"tasks_auto_start": null}"#,
            r#"{"repeater_settings": {"unknown_nested": true}}"#,
            r#"{"upstream_tls": {"custom_ca_ders": []}}"#,
            &format!(
                r#"{{"scope": {{"include": [{{"id": "{rule_id}", "enabled": true, "prefix": "https://example.invalid", "extra": 1}}]}}}}"#
            ),
            &format!(
                r#"{{"sessions": {{"rules": [{{"id": "{valid_rule_id}", "name": "r1", "actions": [{{"type": "set_header", "name": "X-A", "value": "1", "unexpected": true}}]}}]}}}}"#
            ),
            r#"{"tasks_auto_start": true} trailing"#,
            r#"{"tasks_auto_start": true, "tasks_auto_start": false}"#,
        ];

        for case in bad_cases {
            assert!(
                parse_project_settings_json(case).is_err(),
                "expected error for input: {case}"
            );
        }
    }

    #[test]
    fn project_settings_json_enforces_512kib_size_limit() {
        let padding = "a".repeat(MAX_PROJECT_SETTINGS_JSON_BYTES);
        let oversized = format!(r#"{{"tasks_auto_start": true, "comment": "{padding}"}}"#);
        assert!(oversized.len() > MAX_PROJECT_SETTINGS_JSON_BYTES);
        let err = parse_project_settings_json(&oversized).expect_err("oversized JSON must fail");
        assert!(err.contains("512 KiB"));

        let mut oversized_export = Workspace::default();
        oversized_export.scope.include.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: format!("https://example.invalid/{padding}"),
            include_subdomains: false,
        });
        assert!(export_project_settings_json(&oversized_export).is_err());

        let legacy_v022 = r#"{
            "format": "mio.project_settings.v1",
            "upstream_proxy": {
                "protocol": "http",
                "host": "127.0.0.1",
                "port": 8888,
                "username": "",
                "password": "",
                "rules": [],
                "socks": { "use_proxy": false, "host": "", "port": 1080, "username": "", "password": "", "dns_over_socks": false, "bypass_hosts": [], "override_project_only": false },
                "open_ended_response_timeout_secs": 0,
                "hostname_resolutions": [],
                "override_project_only": false
            },
            "upstream_tls": {
                "custom_ca_ders_b64": [],
                "include_system_roots": true,
                "insecure_skip_verify": false,
                "protocol_mode": "custom",
                "allow_unsafe_renegotiation": false,
                "disable_session_resume": false,
                "client_certs_override_project_only": false
            }
        }"#;
        let legacy_parsed = parse_project_settings_json(legacy_v022).unwrap();
        assert_eq!(legacy_parsed.upstream_tls.protocol_mode, TlsProtocolMode::Custom);
        assert_eq!(
            legacy_parsed.upstream_tls.custom_protocols,
            vec!["TLSv1.3", "TLSv1.2"]
        );
        assert_eq!(legacy_parsed.upstream_proxy.rules.len(), 1);
        assert_eq!(legacy_parsed.upstream_proxy.rules[0].host, "127.0.0.1");
        assert_eq!(legacy_parsed.upstream_proxy.rules[0].port, 8888);

        assert!(
            parse_project_settings_json(
                r#"{"upstream_tls": {"protocol_mode": "custom", "custom_protocols": []}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn project_settings_json_validates_bounds_base64_and_workspace_rules() {
        let mut bad_export = Workspace::default();
        bad_export.repeater_settings.request_timeout_secs = 0;
        assert!(export_project_settings_json(&bad_export).is_err());

        let mut bad_stream = Workspace::default();
        bad_stream.repeater_settings.streaming_response_timeout_secs = 86_401;
        assert!(export_project_settings_json(&bad_stream).is_err());

        let listener_id = Uuid::new_v4();
        let macro_id = Uuid::new_v4();
        let step_id = Uuid::new_v4();
        let invalid_cases = [
            r#"{"repeater_settings": {"request_timeout_secs": 0}}"#,
            r#"{"repeater_settings": {"request_timeout_secs": 3601}}"#,
            r#"{"repeater_settings": {"streaming_response_timeout_secs": 86401}}"#,
            r#"{"repeater_settings": {"auto_follow_redirections_max_hops": 0}}"#,
            r#"{"repeater_settings": {"auto_follow_redirections_max_hops": 21}}"#,
            &format!(
                r#"{{"proxy_listeners": [{{"id": "{listener_id}", "enabled": true, "bind_address": "127.0.0.1:8080", "redirect_to_port": 0}}]}}"#
            ),
            &format!(
                r#"{{"proxy_listeners": [{{"id": "{listener_id}", "enabled": true, "bind_address": "not-an-ip:8080"}}]}}"#
            ),
            r#"{"upstream_tls": {"custom_ca_ders_b64": ["not-valid-base64!"]}}"#,
            r#"{"client_certificates": [{"host_pattern": "example.invalid", "certificate_b64": "!!!"}]}"#,
            &format!(
                r#"{{"sessions": {{"macros": [{{"id": "{macro_id}", "name": "m1", "steps": [{{"id": "{step_id}", "method": "GET", "url": "https://example.invalid/", "body_b64": "???"}}]}}]}}}}"#
            ),
            &format!(
                r#"{{"sessions": {{"macros": [{{"id": "{macro_id}", "name": "m1", "steps": [{{"id": "{step_id}", "method": "GET", "url": "https://example.invalid/", "extractions": [{{"name": "x", "source_kind": "delimiter", "arg1": "a="}}]}}]}}]}}}}"#
            ),
            &format!(
                r#"{{"ssl_pass_through_rules": [{{"id": "{listener_id}", "enabled": true, "host_pattern": "(", "is_regex": true}}]}}"#
            ),
            &format!(
                r#"{{"ws_match_and_replace_rules": [{{"id": "{listener_id}", "enabled": true, "match_pattern": "(", "replace_with": "x", "is_regex": true}}]}}"#
            ),
            &format!(
                r#"{{"upstream_proxy": {{"platform_auth": {{"enabled": true, "rules": [{{"id": "{listener_id}", "enabled": true, "destination_host": "*", "auth_type": "basic", "username": ""}}]}}}}}}"#
            ),
        ];

        for case in invalid_cases {
            assert!(
                parse_project_settings_json(case).is_err(),
                "expected validation failure for input: {case}"
            );
        }

        let all_interfaces = format!(
            r#"{{"proxy_listeners": [{{"id": "{listener_id}", "enabled": true, "bind_address": "0.0.0.0:8080"}}]}}"#
        );
        let parsed_all = parse_project_settings_json(&all_interfaces).unwrap();
        assert_eq!(parsed_all.proxy_listeners[0].bind_address, "0.0.0.0:8080");
    }

    #[test]
    fn project_settings_patch_merges_partial_json_without_wiping_existing_sections() {
        let listener_id = Uuid::new_v4();
        let existing_rule_id = Uuid::new_v4();
        let new_scope_id = Uuid::new_v4();
        let mut ws = Workspace {
            proxy_listeners: vec![ProxyListenerConfig {
                id: listener_id,
                enabled: true,
                bind_address: "0.0.0.0:8080".into(),
                ..ProxyListenerConfig::default()
            }],
            match_and_replace_rules: vec![MatchAndReplaceRule {
                id: existing_rule_id,
                enabled: true,
                item_type: MatchReplaceTarget::RequestHeader,
                match_pattern: "User-Agent: old".into(),
                replace_with: "User-Agent: new".into(),
                regex: false,
                comment: "keep me".into(),
                script_name: String::new(),
                filter_script: String::new(),
            }],
            tasks_auto_start: false,
            ..Workspace::default()
        };

        let partial = format!(
            r#"{{
              "format": "mio.project_settings.v1",
              "scope": {{"include": [{{"id": "{new_scope_id}", "enabled": true, "prefix": "https://app.example.invalid"}}]}}
            }}"#
        );
        let patch = parse_project_settings_patch(&partial).unwrap();
        patch.apply_to(&mut ws).unwrap();

        assert_eq!(ws.scope.include.len(), 1);
        assert_eq!(ws.scope.include[0].id, new_scope_id);
        assert_eq!(ws.proxy_listeners.len(), 1);
        assert_eq!(ws.proxy_listeners[0].bind_address, "0.0.0.0:8080");
        assert_eq!(ws.match_and_replace_rules.len(), 1);
        assert_eq!(ws.match_and_replace_rules[0].id, existing_rule_id);
        assert!(!ws.tasks_auto_start);
    }
}
