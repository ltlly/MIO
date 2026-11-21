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

use crate::{AuthConfig, ProxyError, SinglePacketBarrier, UpstreamRoute, UpstreamTlsConfig};

mod download;
mod exchange;
#[cfg(test)]
use exchange::RawH1Io;
mod send;
#[cfg(test)]
mod tests;
mod websocket;
mod websocket_session;

pub use download::{
    DownloadConfig, DownloadOutcome, DownloadRouteResolver, DownloadSink, download_to_sink,
    download_to_writer,
};
pub(crate) use exchange::{exchange, exchange_with_metrics};
#[cfg(test)]
use send::{collect_response, send_owned};
pub(crate) use send::exchange_model_with_auth;
pub use send::{prepare_send, send, send_streaming};
pub use websocket::send_websocket;
pub(crate) use websocket::{validate_websocket_payload, websocket_config, websocket_outgoing_message};
pub use websocket_session::{WebSocketSession, connect_websocket_session};

#[derive(Clone, Default)]
pub struct SendNetworkConfig {
    pub upstream_route: UpstreamRoute,
    pub hostname_overrides: Arc<[(String, IpAddr)]>,
}

impl std::fmt::Debug for SendNetworkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendNetworkConfig")
            .field("upstream_route", &self.upstream_route)
            .field("hostname_overrides_len", &self.hostname_overrides.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendLimits {
    pub max_body_bytes: usize,
    pub timeout: Duration,
    pub streaming_timeout: Option<Duration>,
}

impl Default for SendLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 16 * 1024 * 1024,
            timeout: Duration::from_secs(30),
            streaming_timeout: None,
        }
    }
}

#[derive(Clone)]
pub struct SendHttpConfig {
    pub version: Option<HttpVersion>,
    pub default_to_http2: bool,
    pub allow_http2_alpn_override: bool,
    pub update_content_length: bool,
    pub normalize_http1_line_endings: bool,
    pub raw_http1_override: Option<Arc<[u8]>>,
    pub http1_keep_alive: bool,
    pub set_connection_keep_alive: bool,
    pub http2_reuse: bool,
    pub use_connection_pool: bool,
    pub strip_100_continue_headers: bool,
    pub strip_proxy_headers: bool,
    pub force_streaming_response: bool,
}

impl Default for SendHttpConfig {
    fn default() -> Self {
        Self {
            version: None,
            default_to_http2: false,
            allow_http2_alpn_override: false,
            update_content_length: true,
            normalize_http1_line_endings: true,
            raw_http1_override: None,
            http1_keep_alive: true,
            set_connection_keep_alive: false,
            http2_reuse: true,
            use_connection_pool: false,
            strip_100_continue_headers: false,
            strip_proxy_headers: true,
            force_streaming_response: false,
        }
    }
}

impl std::fmt::Debug for SendHttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendHttpConfig")
            .field("version", &self.version)
            .field("default_to_http2", &self.default_to_http2)
            .field("allow_http2_alpn_override", &self.allow_http2_alpn_override)
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
            .field(
                "strip_100_continue_headers",
                &self.strip_100_continue_headers,
            )
            .field("strip_proxy_headers", &self.strip_proxy_headers)
            .field("force_streaming_response", &self.force_streaming_response)
            .finish()
    }
}

#[derive(Clone, Debug, Default)]
pub struct SendGroupConfig {
    pub connection_pool_group_id: Option<u128>,
    pub single_packet_barrier: Option<SinglePacketBarrier>,
}

#[derive(Clone, Debug, Default)]
pub struct SendConfig {
    pub network: SendNetworkConfig,
    pub tls: UpstreamTlsConfig,
    pub limits: SendLimits,
    pub http: SendHttpConfig,
    pub auth: AuthConfig,
    pub group: SendGroupConfig,
}

enum PreparedPayload {
    Ready(Request),
    WithAuth(HttpRequest),
}

/// A locally validated request ready for one send. SDK types and its body stay private.
/// Preparation opens no socket and starts no SDK worker; dropping it is sufficient to cancel admission.
pub struct PreparedSend {
    payload: PreparedPayload,
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
