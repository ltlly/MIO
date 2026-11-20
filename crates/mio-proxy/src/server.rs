use crate::{
    CapturedFlow, CapturedFlowMetrics, MitmAuthority, ProxyError, SendConfig, StreamedResponse, UpstreamRoute,
    UpstreamTls, client, message,
};
use mio_plugin_api::{
    FlowRecord, FlowState, FlowSummary, HttpRequest, HttpResponse, ProjectKey, TrafficSource, Uuid,
    WebSocketDirection, WebSocketMessage,
};
use rama::{
    Service,
    graceful::Shutdown,
    http::{Body, Method, Request, Response, StatusCode, io::upgrade::handle_upgrade},
    io::GracefulIo,
    net::uri::Uri,
    rt::Executor,
    service::service_fn,
    tcp::TcpStream,
    tls::rustls::server::TlsAcceptorService,
};
use rama_http_backend::server::HttpServer;
use std::{
    convert::Infallible,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

mod config;
mod hook_chain;
mod intercept;
mod listener;
mod socks5;
mod sse;
mod ssl_passthrough;
mod websocket;

pub use listener::start;
pub use websocket::WebSocketInjectorRegistry;

#[cfg(test)]
mod terminal_tests;
#[cfg(test)]
mod unit_tests;

#[cfg(test)]
use listener::handle_http;
use listener::new_record;
#[cfg(test)]
use std::net::Ipv4Addr;

struct TerminalRecord {
    capture: CapturedFlow,
    _permit: Arc<tokio::sync::OwnedSemaphorePermit>,
}

type TerminalRecords = Arc<Mutex<Vec<TerminalRecord>>>;

pub type FlowTransform = Arc<
    dyn Fn(
            &mut FlowRecord,
            bool,
            bool,
            &CancellationToken,
            (Instant, usize),
        ) -> Result<bool, ProxyError>
        + Send
        + Sync,
>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterceptEvalContext<'a> {
    pub listener_port: u16,
    pub target_ip: Option<&'a str>,
    pub was_annotated: bool,
}

pub type InterceptFilter = Arc<
    dyn Fn(
            &FlowRecord,
            bool,
            bool,
            bool,
            bool,
            InterceptEvalContext<'_>,
            &CancellationToken,
            (Instant, usize),
        ) -> Result<bool, ProxyError>
        + Send
        + Sync,
>;

pub type WebSocketInterceptFilter = Arc<
    dyn Fn(&HttpRequest, WebSocketDirection, &CancellationToken) -> Result<bool, ProxyError>
        + Send
        + Sync,
>;

pub type WebSocketTransform = Arc<
    dyn Fn(
            &HttpRequest,
            WebSocketDirection,
            mio_plugin_api::WebSocketKind,
            &[u8],
            &CancellationToken,
            (Instant, usize),
        ) -> Result<Option<Vec<u8>>, ProxyError>
        + Send
        + Sync,
>;

pub type SslPassThroughMatcher = Arc<dyn Fn(&str, u16) -> bool + Send + Sync>;
pub type UpstreamRouteResolver = Arc<dyn Fn(&str) -> UpstreamRoute + Send + Sync>;
pub type DropOutOfScopeFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;
pub type ClientTlsFailureCallback = Arc<dyn Fn(String, u16) + Send + Sync>;
pub type ServerCertObservedCallback = Arc<dyn Fn(crate::ObservedCertInfo) + Send + Sync>;

type ProcessingState = (
    Option<FlowTransform>,
    Option<InterceptFilter>,
    Option<WebSocketInterceptFilter>,
);

pub type BoxHookFuture<T> = Pin<Box<dyn Future<Output = Result<T, ProxyError>> + Send>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowHookDecision {
    Pass,
    ModifyRequest(HttpRequest),
    ModifyResponse(HttpResponse),
    ForceIntercept {
        request: Option<HttpRequest>,
        response: Option<HttpResponse>,
    },
    Drop,
}

pub type AsyncFlowHook = Arc<
    dyn Fn(
            FlowRecord,
            bool, // response_stage
            bool, // response_head_only
            CancellationToken,
            Instant, // deadline
        ) -> BoxHookFuture<FlowHookDecision>
        + Send
        + Sync,
>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketHookDecision {
    Pass,
    Modify(Vec<u8>),
    ForceIntercept(Option<Vec<u8>>),
    Drop,
}

pub type AsyncWebSocketHook = Arc<
    dyn Fn(
            Uuid,   // flow_id
            Uuid,   // message_id
            String, // upgrade url
            WebSocketDirection,
            mio_plugin_api::WebSocketKind,
            Vec<u8>, // payload
            CancellationToken,
            Instant, // deadline
        ) -> BoxHookFuture<WebSocketHookDecision>
        + Send
        + Sync,
>;

pub(super) type PluginHooks = (Option<AsyncFlowHook>, Option<AsyncWebSocketHook>);

#[derive(Clone)]
pub struct ProxyConfig {
    pub listen: SocketAddr,
    pub ca: MitmAuthority,
    pub upstream_route: UpstreamRoute,
    pub upstream_route_resolver: Option<UpstreamRouteResolver>,
    pub upstream_tls: UpstreamTls,
    pub upstream_tls_options: crate::UpstreamTlsOptions,
    pub hostname_overrides: Arc<[(String, IpAddr)]>,
    pub intercept_requests: bool,
    pub intercept_responses: bool,
    pub intercept_filter: Option<InterceptFilter>,
    pub websocket_intercept_filter: Option<WebSocketInterceptFilter>,
    pub websocket_transform: Option<WebSocketTransform>,
    pub ssl_pass_through: Option<SslPassThroughMatcher>,
    pub flow_transform: Option<FlowTransform>,
    pub async_flow_hook: Option<AsyncFlowHook>,
    pub async_websocket_hook: Option<AsyncWebSocketHook>,
    pub max_body_bytes: usize,
    pub max_connections: usize,
    pub request_timeout: Duration,
    pub streaming_timeout: Option<Duration>,
    pub intercept_timeout: Duration,
    pub stop_timeout: Duration,
    pub shared_connections: Option<Arc<tokio::sync::Semaphore>>,
    pub shared_requests: Option<Arc<tokio::sync::Semaphore>>,
    pub shared_injectors: Option<WebSocketInjectorRegistry>,
    pub client_certificates: Arc<[(crate::ClientCertificateRule, crate::ParsedClientIdentity)]>,
    pub annotation_checker: Option<Arc<dyn Fn(mio_plugin_api::FlowId) -> bool + Send + Sync>>,
    // Per-listener routing & TLS options
    pub invisible_proxying: bool,
    pub redirect_to_host: Option<String>,
    pub redirect_to_port: Option<u16>,
    pub force_use_of_tls: bool,
    pub enable_http2: bool,
    pub tls_protocol_filter: crate::ListenerTlsProtocols,
    pub cert_mode: crate::ListenerCertMode,
    // Proxy Misc HTTP/WS transforms
    pub use_http10_in_requests: bool,
    pub use_http10_in_responses: bool,
    pub http1_keep_alive: bool,
    pub use_connection_pool: bool,
    pub set_response_connection_close: bool,
    pub set_request_connection_header: bool,
    pub strip_proxy_headers: bool,
    pub remove_unsupported_accept_encodings: bool,
    pub strip_sec_websocket_extensions: bool,
    pub disable_web_interface: bool,
    pub suppress_browser_error_messages: bool,
    // Scope, Streaming & 100-Continue
    pub drop_out_of_scope_fn: Option<DropOutOfScopeFn>,
    pub treat_event_stream_as_streaming: bool,
    pub strip_chunked_metadata_in_streaming: bool,
    pub understand_100_continue: bool,
    pub remove_100_continue_headers: bool,
    pub default_to_http2: bool,
    pub streaming_scope_matcher: Option<crate::StreamingScopeMatcher>,
    pub platform_auth_enabled: bool,
    pub platform_auth_rules: Arc<[crate::PlatformAuthRuleConfig]>,
    pub upstream_auth_rules: Arc<[crate::UpstreamAuthRuleConfig]>,
    // TLS failure & observed server certificate callbacks
    pub on_client_tls_failure: Option<ClientTlsFailureCallback>,
    pub on_server_cert_observed: Option<ServerCertObservedCallback>,
}

#[derive(Debug)]
pub enum ProxyEvent {
    Captured {
        project: ProjectKey,
        record: FlowRecord,
        response_stream: Option<StreamedResponse>,
        listener_port: Option<u16>,
        capture_metrics: CapturedFlowMetrics,
    },
    Intercepted {
        project: ProjectKey,
        record: FlowRecord,
        /// The live response has no editable body or trailers yet.
        response_head_only: bool,
        response_stream: Option<StreamedResponse>,
        listener_port: Option<u16>,
        capture_metrics: CapturedFlowMetrics,
        reply: oneshot::Sender<InterceptDecision>,
    },
    WebSocketIntercepted {
        project: ProjectKey,
        url: String,
        record: WebSocketMessage,
        listener_port: u16,
        reply: oneshot::Sender<WebSocketInterceptDecision>,
    },
    WebSocketInterceptResolved {
        project: ProjectKey,
        message_id: Uuid,
    },
    WebSocketMessage {
        project: ProjectKey,
        record: WebSocketMessage,
        original_payload: Option<Vec<u8>>,
        listener_port: u16,
    },
}

#[derive(Debug)]
pub enum InterceptDecision {
    ForwardRequest(HttpRequest),
    ForwardResponse(HttpResponse),
    Drop,
}

#[derive(Debug)]
pub enum WebSocketInterceptDecision {
    Forward(Vec<u8>),
    Drop,
}

#[derive(Clone, Copy)]
pub(super) struct InterceptMode {
    requests: bool,
    responses: bool,
}

pub struct ProxyHandle {
    address: SocketAddr,
    cancel: CancellationToken,
    intercept: watch::Sender<InterceptMode>,
    processing: watch::Sender<ProcessingState>,
    websocket_transform: watch::Sender<Option<WebSocketTransform>>,
    ssl_pass_through: watch::Sender<Option<SslPassThroughMatcher>>,
    plugin_hooks: watch::Sender<PluginHooks>,
    drop_out_of_scope: watch::Sender<Option<DropOutOfScopeFn>>,
    runner: Option<JoinHandle<Result<(), ProxyError>>>,
    stop_timeout: Duration,
    terminal_records: TerminalRecords,
    injectors: WebSocketInjectorRegistry,
}

struct State {
    project: ProjectKey,
    listener_port: u16,
    config: ProxyConfig,
    events: mpsc::Sender<ProxyEvent>,
    cancel: CancellationToken,
    intercept: watch::Receiver<InterceptMode>,
    processing: watch::Receiver<ProcessingState>,
    websocket_transform: watch::Receiver<Option<WebSocketTransform>>,
    ssl_pass_through: watch::Receiver<Option<SslPassThroughMatcher>>,
    plugin_hooks: watch::Receiver<PluginHooks>,
    drop_out_of_scope: watch::Receiver<Option<DropOutOfScopeFn>>,
    tasks: TaskTracker,
    requests: Arc<Semaphore>,
    exec: Executor,
    terminal_records: TerminalRecords,
    injectors: WebSocketInjectorRegistry,
}

impl State {
    pub(super) fn resolve_upstream_route_for_host(&self, host: &str) -> UpstreamRoute {
        let route = if let Some(resolver) = &self.config.upstream_route_resolver {
            resolver(host)
        } else {
            self.config.upstream_route.clone()
        };
        route.resolve_for_host(host)
    }

    pub(super) fn resolve_upstream_route_for_url(&self, url: &str) -> UpstreamRoute {
        if let Ok(uri) = url.parse::<Uri>()
            && let Some(host) = uri.host_str()
        {
            return self.resolve_upstream_route_for_host(&host);
        }
        self.config.upstream_route.clone()
    }

    pub(super) fn listener_tls_options(&self) -> crate::tls::ListenerTlsOptions {
        crate::tls::ListenerTlsOptions {
            enable_http2: self.config.enable_http2,
            protocol_filter: self.config.tls_protocol_filter,
            cert_mode: self.config.cert_mode.clone(),
        }
    }

    pub(super) fn should_drop_out_of_scope(&self, url: &str) -> bool {
        self.drop_out_of_scope
            .borrow()
            .as_ref()
            .is_some_and(|drop_fn| drop_fn(url))
    }

    pub(super) fn notify_client_tls_failure(&self, host: String, port: u16) {
        if let Some(callback) = &self.config.on_client_tls_failure {
            let clean = host.trim().trim_matches(['[', ']']).to_owned();
            callback(clean, port);
        }
    }

    pub(super) fn error_response(&self, error: &ProxyError) -> Response {
        message::error_response_with_options(error, self.config.suppress_browser_error_messages)
    }

    pub(super) fn effective_passthrough_target(&self, host: &str, port: u16) -> (String, u16) {
        let effective_host = self
            .config
            .redirect_to_host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .unwrap_or(host)
            .to_owned();
        let effective_port = self
            .config
            .redirect_to_port
            .filter(|&p| p > 0)
            .unwrap_or(port);
        (effective_host, effective_port)
    }
}

struct ActiveFlow {
    state: Arc<State>,
    record: Option<FlowRecord>,
    response_stream: Option<StreamedResponse>,
    capture_metrics: CapturedFlowMetrics,
    request_was_intercepted: bool,
    request_was_modified: bool,
    started: Instant,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
}
