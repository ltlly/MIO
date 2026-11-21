use std::{net::IpAddr, sync::Arc, time::Duration};

use mio_plugin_api::{HttpRequest, HttpResponse, HttpVersion, WebSocketMessage};
use rama::http::Request;
#[cfg(test)]
use rama::{
    graceful::Shutdown,
    http::{Body, Response},
    rt::Executor,
};
#[cfg(test)]
use std::io::IoSlice;

use crate::{ObservedCertInfo, ProxyError, UpstreamRoute, UpstreamTls, UpstreamTlsOptions};

mod exchange;
#[cfg(test)]
use exchange::RawH1Io;
mod send;
#[cfg(test)]
mod tests;
mod websocket;
mod websocket_session;

pub(crate) use exchange::{exchange, exchange_with_metrics};
#[cfg(test)]
use send::{collect_response, send_owned};
pub(crate) use send::exchange_model_with_auth;
pub use send::{prepare_send, send, send_streaming};
pub use websocket::send_websocket;
pub(crate) use websocket::{websocket_outgoing_message, websocket_received_message};
pub use websocket_session::{WebSocketSession, connect_websocket_session};

#[derive(Clone)]
pub struct SendConfig {
    pub upstream_route: UpstreamRoute,
    pub upstream_tls: UpstreamTls,
    pub upstream_tls_options: UpstreamTlsOptions,
    pub max_body_bytes: usize,
    pub timeout: Duration,
    pub streaming_timeout: Option<Duration>,
    pub http_version: Option<HttpVersion>,
    pub update_content_length: bool,
    pub normalize_http1_line_endings: bool,
    pub raw_http1_override: Option<Arc<[u8]>>,
    pub http1_keep_alive: bool,
    pub set_connection_keep_alive: bool,
    pub http2_reuse: bool,
    pub use_connection_pool: bool,
    pub allow_http2_alpn_override: bool,
    pub strip_100_continue_headers: bool,
    pub strip_proxy_headers: bool,
    pub on_server_cert_observed: Option<Arc<dyn Fn(ObservedCertInfo) + Send + Sync>>,
    pub client_certificates: Arc<[(crate::ClientCertificateRule, crate::ParsedClientIdentity)]>,
    pub hostname_overrides: Arc<[(String, IpAddr)]>,
    pub default_to_http2: bool,
    pub force_streaming_response: bool,
    pub connection_pool_group_id: Option<u128>,
    pub single_packet_barrier: Option<crate::SinglePacketBarrier>,
    pub platform_auth_enabled: bool,
    pub platform_auth_rules: Arc<[crate::PlatformAuthRuleConfig]>,
    pub upstream_auth_rules: Arc<[crate::UpstreamAuthRuleConfig]>,
}

impl std::fmt::Debug for SendConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SendConfig")
            .field("upstream_route", &self.upstream_route)
            .field("upstream_tls", &self.upstream_tls)
            .field("upstream_tls_options", &self.upstream_tls_options)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("timeout", &self.timeout)
            .field("streaming_timeout", &self.streaming_timeout)
            .field("http_version", &self.http_version)
            .field("update_content_length", &self.update_content_length)
            .field(
                "normalize_http1_line_endings",
                &self.normalize_http1_line_endings,
            )
            .field(
                "raw_http1_override_configured",
                &self.raw_http1_override.is_some(),
            )
            .field("http1_keep_alive", &self.http1_keep_alive)
            .field("set_connection_keep_alive", &self.set_connection_keep_alive)
            .field("http2_reuse", &self.http2_reuse)
            .field("use_connection_pool", &self.use_connection_pool)
            .field("allow_http2_alpn_override", &self.allow_http2_alpn_override)
            .field(
                "strip_100_continue_headers",
                &self.strip_100_continue_headers,
            )
            .field("strip_proxy_headers", &self.strip_proxy_headers)
            .field(
                "on_server_cert_observed_configured",
                &self.on_server_cert_observed.is_some(),
            )
            .field("client_certificates_len", &self.client_certificates.len())
            .field("hostname_overrides_len", &self.hostname_overrides.len())
            .field("default_to_http2", &self.default_to_http2)
            .field("force_streaming_response", &self.force_streaming_response)
            .field("connection_pool_group_id", &self.connection_pool_group_id)
            .field(
                "single_packet_barrier_configured",
                &self.single_packet_barrier.is_some(),
            )
            .field("platform_auth_enabled", &self.platform_auth_enabled)
            .field("platform_auth_rules_len", &self.platform_auth_rules.len())
            .field("upstream_auth_rules_len", &self.upstream_auth_rules.len())
            .finish()
    }
}

impl Default for SendConfig {
    fn default() -> Self {
        Self {
            upstream_route: UpstreamRoute::Direct,
            upstream_tls: UpstreamTls::System,
            upstream_tls_options: UpstreamTlsOptions::default(),
            max_body_bytes: 16 * 1024 * 1024,
            timeout: Duration::from_secs(30),
            streaming_timeout: None,
            http_version: None,
            update_content_length: true,
            normalize_http1_line_endings: true,
            raw_http1_override: None,
            http1_keep_alive: true,
            set_connection_keep_alive: false,
            http2_reuse: true,
            use_connection_pool: false,
            allow_http2_alpn_override: false,
            strip_100_continue_headers: false,
            strip_proxy_headers: true,
            on_server_cert_observed: None,
            client_certificates: Arc::from([]),
            hostname_overrides: Arc::from([]),
            default_to_http2: false,
            force_streaming_response: false,
            connection_pool_group_id: None,
            single_packet_barrier: None,
            platform_auth_enabled: false,
            platform_auth_rules: Arc::from([]),
            upstream_auth_rules: Arc::from([]),
        }
    }
}

/// A locally validated request ready for one send. SDK types and its body stay private.
/// Preparation opens no socket and starts no SDK worker; dropping it is sufficient to cancel admission.
pub struct PreparedSend {
    request: Request,
    config: SendConfig,
}

/// Response plus the elapsed time from starting a single send attempt until the
/// first response byte arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendMetrics {
    pub response: HttpResponse,
    /// Elapsed milliseconds from starting this single send attempt until the first response byte arrives.
    pub response_received_ms: u64,
    /// Non-canonical HTTP/1 reason phrase bytes when the origin sent a custom reason phrase.
    pub reason_phrase: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct SendStreamOutcome {
    pub response: Option<HttpResponse>,
    pub error: Option<ProxyError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSocketExchange {
    pub request: HttpRequest,
    pub response: HttpResponse,
    pub sent: WebSocketMessage,
    pub received: Option<WebSocketMessage>,
}

enum SendInput {
    Message(HttpRequest),
    Prepared(Request),
}
