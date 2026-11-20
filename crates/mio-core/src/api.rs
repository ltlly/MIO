use mio_plugin_api::*;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub mod audit;
pub use crate::audit_engine::{AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence};
pub use crate::oast::{OastInteraction, OastPayloadToken, OastProtocol, SelfHostedOastConfig};
pub use audit::*;
mod ext_archive;
pub use ext_archive::*;
mod commands;
pub use commands::*;
mod events;
pub use events::*;
mod history;
pub use history::*;
mod intruder;
pub use intruder::*;
mod organiser;
pub use organiser::*;
mod plugin_types;
pub use plugin_types::*;
mod provenance;
pub use provenance::*;
mod proxy_settings;
pub use proxy_settings::*;
mod repeater_history;
pub use repeater_history::*;
mod resource_pools;
pub use resource_pools::*;
pub mod sequencer;
pub use sequencer::*;
mod sessions;
pub use sessions::*;
mod websocket;
pub mod websocket_repeater;
pub use crate::store::target_site_map::{
    MAX_SITE_MAP_DEPTH, MAX_SITE_MAP_NODES, MAX_SITE_MAP_PAGE_LIMIT, SiteMapFilter, SiteMapNode,
    SiteMapNodeKind, SiteMapQuery, SiteMapSnapshot, SiteMapEntryInput, SiteMapEntryReceipt, SiteMapEntryDetail,
};
pub use mio_proxy::{
    ClientCertificateFormat, ClientCertificateRule, ClientCertificateSummary,
    MAX_CLIENT_CERT_BYTES, MAX_CLIENT_CERT_PASSWORD_BYTES,
};
pub use websocket::*;
pub use websocket_repeater::*;
mod workspace_settings;
pub use workspace_settings::*;

pub const DEFAULT_PROXY_LISTENER: &str = "127.0.0.1:8080";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub revision: u64,
    pub target: String,
    pub raw: Vec<u8>,
    pub version: HttpVersion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkTab {
    pub id: TabId,
    pub title: String,
    pub group: Option<Uuid>,
    pub draft: Draft,
    pub last_flow: Option<FlowId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_response: Option<ExtResponseRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<RepeaterHistory>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: Uuid,
    pub title: String,
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_listener: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxy_listeners: Vec<ProxyListenerConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub client_certificates: Vec<ClientCertificateRule>,
    pub orientation: Orientation,
    /// Zero selects the default width; other values persist the user-resized rail.
    #[serde(default)]
    pub tab_sidebar_width: u16,
    pub groups: Vec<TabGroup>,
    pub tabs: Vec<WorkTab>,
    pub active: Option<TabId>,
    pub closed: Vec<WorkTab>,
    #[serde(default)]
    pub repeater_settings: RepeaterSettings,
    #[serde(default, skip_serializing_if = "NetworkHttpSettings::is_default")]
    pub network_http: NetworkHttpSettings,
    #[serde(default, skip_serializing_if = "UpstreamProxySettings::is_default")]
    pub upstream_proxy: UpstreamProxySettings,
    #[serde(default, skip_serializing_if = "UpstreamTlsSettings::is_default")]
    pub upstream_tls: UpstreamTlsSettings,
    #[serde(default, skip_serializing_if = "TargetScope::is_empty")]
    pub scope: TargetScope,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_and_replace_rules: Vec<MatchAndReplaceRule>,
    #[serde(default, skip_serializing_if = "ProxyInterceptionSettings::is_default")]
    pub proxy_interception: ProxyInterceptionSettings,
    #[serde(default, skip_serializing_if = "ProxyMessageSettings::is_default")]
    pub proxy_messages: ProxyMessageSettings,
    #[serde(
        default,
        skip_serializing_if = "ProxyWebSocketInterceptionSettings::is_default"
    )]
    pub proxy_websocket_interception: ProxyWebSocketInterceptionSettings,
    #[serde(default, skip_serializing_if = "SessionSettings::is_default")]
    pub sessions: SessionSettings,
    #[serde(default)]
    pub intruder: IntruderWorkspace,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub websocket_tabs: Vec<WebSocketRepeaterTab>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_websocket_tab: Option<TabId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub organiser_items: Vec<OrganiserItem>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tab_notes: std::collections::BTreeMap<TabId, String>,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub tasks_auto_start: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled_tasks: Vec<ScheduledTask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub organiser_collections: Vec<OrganiserCollection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_scan_checks: Vec<CustomScanCheck>,
    #[serde(default = "default_true_bool", skip_serializing_if = "is_true_bool")]
    pub live_passive_crawl_enabled: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            revision: 0,
            project_name: None,
            proxy_listener: None,
            proxy_listeners: Vec::new(),
            client_certificates: Vec::new(),
            orientation: Orientation::default(),
            tab_sidebar_width: 0,
            groups: Vec::new(),
            tabs: Vec::new(),
            active: None,
            closed: Vec::new(),
            repeater_settings: RepeaterSettings::default(),
            network_http: NetworkHttpSettings::default(),
            upstream_proxy: UpstreamProxySettings::default(),
            upstream_tls: UpstreamTlsSettings::default(),
            scope: TargetScope::default(),
            match_and_replace_rules: Vec::new(),
            proxy_interception: ProxyInterceptionSettings::default(),
            proxy_messages: ProxyMessageSettings::default(),
            proxy_websocket_interception: ProxyWebSocketInterceptionSettings::default(),
            sessions: SessionSettings::default(),
            intruder: IntruderWorkspace::default(),
            websocket_tabs: Vec::new(),
            active_websocket_tab: None,
            organiser_items: Vec::new(),
            tab_notes: std::collections::BTreeMap::new(),
            tasks_auto_start: true,
            scheduled_tasks: Vec::new(),
            organiser_collections: Vec::new(),
            custom_scan_checks: Vec::new(),
            live_passive_crawl_enabled: true,
        }
    }
}

impl Workspace {
    pub fn proxy_listener_or_default(&self) -> &str {
        if let Some(first) = self.proxy_listeners.iter().find(|l| l.enabled) {
            return first.bind_address.as_str();
        }
        if let Some(first) = self.proxy_listeners.first() {
            return first.bind_address.as_str();
        }
        self.proxy_listener
            .as_deref()
            .unwrap_or(DEFAULT_PROXY_LISTENER)
    }

    pub fn effective_proxy_listeners(&self) -> Vec<ProxyListenerConfig> {
        if !self.proxy_listeners.is_empty() {
            return self.proxy_listeners.clone();
        }
        vec![ProxyListenerConfig {
            id: Uuid::from_u128(1),
            enabled: true,
            bind_address: self.proxy_listener_or_default().to_owned(),
            ..ProxyListenerConfig::default()
        }]
    }
}

pub(super) const fn default_true_bool() -> bool {
    true
}

pub(super) const fn is_true_bool(val: &bool) -> bool {
    *val
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectStartOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub config_mode: crate::StartupDefaultConfig,
    #[serde(default = "default_true_bool")]
    pub pause_automated_tasks: bool,
    #[serde(default = "default_true_bool")]
    pub trust_project_file: bool,
    #[serde(default)]
    pub disable_extensions: bool,
}

impl Default for ProjectStartOptions {
    fn default() -> Self {
        Self {
            name: None,
            config_mode: crate::StartupDefaultConfig::default(),
            pause_automated_tasks: true,
            trust_project_file: true,
            disable_extensions: false,
        }
    }
}

pub struct Client {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    /// Out-of-band control: shutdown cannot wait behind a saturated data queue.
    pub shutdown: tokio_util::sync::CancellationToken,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalMessages {
    pub request: Option<HttpRequest>,
    pub response: Option<HttpResponse>,
    /// None means no streamed original; false means only its observed prefix is available.
    pub response_body_complete: Option<bool>,
    #[serde(default)]
    pub provenance: FlowProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InspectorSection {
    RequestAttributes,
    QueryParams,
    BodyParams,
    Cookies,
    RequestHeaders,
    ResponseHeaders,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectorItem {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectorData {
    pub attributes: Vec<InspectorItem>,
    pub query_params: Vec<InspectorItem>,
    pub body_params: Vec<InspectorItem>,
    pub cookies: Vec<InspectorItem>,
    pub request_headers: Vec<InspectorItem>,
    pub response_headers: Vec<InspectorItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectorMutation {
    Add {
        section: InspectorSection,
        name: String,
        value: String,
    },
    Update {
        section: InspectorSection,
        index: usize,
        name: String,
        value: String,
    },
    Remove {
        section: InspectorSection,
        index: usize,
    },
    Move {
        section: InspectorSection,
        from: usize,
        to: usize,
    },
}
