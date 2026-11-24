use crate::{
    AuthConfig, CapturedFlow, CapturedFlowMetrics, MitmAuthority, ProxyError, SendConfig,
    SendGroupConfig, SendHttpConfig, SendLimits, SendNetworkConfig, StreamedResponse,
    UpstreamRoute, UpstreamTlsConfig, client, message,
};
use mio_plugin_api::{
    FlowRecord, FlowState, FlowSummary, HttpRequest, HttpResponse, HttpVersion, ProjectKey,
    TrafficSource, Uuid, WebSocketDirection, WebSocketMessage,
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
    tls::{rustls::server::TlsAcceptorService, server::TlsServerConfig},
};
use rama_http_backend::server::HttpServer;
use std::{
    convert::Infallible,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

mod config;
mod events;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterceptFilterOutcome {
    Intercept,
    Bypass,
    NeedsTargetIp,
}

impl InterceptFilterOutcome {
    pub const fn should_intercept(self) -> bool {
        matches!(self, Self::Intercept)
    }
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
        ) -> Result<InterceptFilterOutcome, ProxyError>
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
pub type AnnotationChecker = Arc<dyn Fn(mio_plugin_api::FlowId) -> bool + Send + Sync>;

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

#[derive(Clone)]
pub struct ProxyNetworkConfig {
    pub listen: SocketAddr,
    pub invisible_proxying: bool,
    pub redirect_to_host: Option<String>,
    pub redirect_to_port: Option<u16>,
    pub upstream_route: UpstreamRoute,
    pub upstream_route_resolver: Option<UpstreamRouteResolver>,
    pub hostname_overrides: Arc<[(String, IpAddr)]>,
}

#[derive(Clone)]
pub struct ProxyTlsConfig {
    pub ca: MitmAuthority,
    pub listener_protocols: crate::ListenerTlsProtocols,
    pub cert_mode: crate::ListenerCertMode,
    pub force_use_of_tls: bool,
    pub upstream: UpstreamTlsConfig,
    pub on_client_tls_failure: Option<ClientTlsFailureCallback>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyLimits {
    pub max_body_bytes: usize,
    pub max_connections: usize,
    pub request_timeout: Duration,
    pub streaming_timeout: Option<Duration>,
    pub intercept_timeout: Duration,
    pub stop_timeout: Duration,
}

#[derive(Clone)]
pub struct ProxyHttpConfig {
    pub enable_http2: bool,
    pub default_to_http2: bool,
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
    pub treat_event_stream_as_streaming: bool,
    pub strip_chunked_metadata_in_streaming: bool,
    pub understand_100_continue: bool,
    pub remove_100_continue_headers: bool,
    pub streaming_scope_matcher: Option<crate::StreamingScopeMatcher>,
}

#[derive(Clone, Default)]
pub struct ProxyHooks {
    pub intercept_requests: bool,
    pub intercept_responses: bool,
    pub intercept_filter: Option<InterceptFilter>,
    pub websocket_intercept_filter: Option<WebSocketInterceptFilter>,
    pub flow_transform: Option<FlowTransform>,
    pub websocket_transform: Option<WebSocketTransform>,
    pub ssl_pass_through: Option<SslPassThroughMatcher>,
    pub async_flow_hook: Option<AsyncFlowHook>,
    pub async_websocket_hook: Option<AsyncWebSocketHook>,
    pub drop_out_of_scope_fn: Option<DropOutOfScopeFn>,
    pub annotation_checker: Option<AnnotationChecker>,
}

#[derive(Clone, Default)]
pub struct ProxySharedResources {
    pub connections: Option<Arc<Semaphore>>,
    pub requests: Option<Arc<Semaphore>>,
    pub injectors: Option<WebSocketInjectorRegistry>,
}

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    pub network: ProxyNetworkConfig,
    pub tls: ProxyTlsConfig,
    pub limits: ProxyLimits,
    pub http: ProxyHttpConfig,
    pub auth: AuthConfig,
    pub hooks: ProxyHooks,
    pub shared: ProxySharedResources,
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
    pub requests: bool,
    pub responses: bool,
}

#[derive(Clone)]
pub(super) struct DynamicProxyState {
    pub ca: MitmAuthority,
    pub intercept: InterceptMode,
    pub flow_transform: Option<FlowTransform>,
    pub intercept_filter: Option<InterceptFilter>,
    pub websocket_intercept_filter: Option<WebSocketInterceptFilter>,
    pub websocket_transform: Option<WebSocketTransform>,
    pub ssl_pass_through: Option<SslPassThroughMatcher>,
    pub async_flow_hook: Option<AsyncFlowHook>,
    pub async_websocket_hook: Option<AsyncWebSocketHook>,
    pub drop_out_of_scope: Option<DropOutOfScopeFn>,
}

pub(super) struct StaticProxyConfig {
    pub network: ProxyNetworkConfig,
    pub tls: ProxyTlsConfig,
    pub limits: ProxyLimits,
    pub http: ProxyHttpConfig,
    pub auth: AuthConfig,
    pub annotation_checker: Option<AnnotationChecker>,
}

pub struct ProxyHandle {
    address: SocketAddr,
    cancel: CancellationToken,
    dynamic: watch::Sender<DynamicProxyState>,
    runner: Option<JoinHandle<Result<(), ProxyError>>>,
    stop_timeout: Duration,
    terminal_records: TerminalRecords,
    injectors: WebSocketInjectorRegistry,
}

struct State {
    project: ProjectKey,
    listener_port: u16,
    config: StaticProxyConfig,
    events: mpsc::Sender<ProxyEvent>,
    cancel: CancellationToken,
    dynamic: watch::Receiver<DynamicProxyState>,
    tasks: TaskTracker,
    requests: Arc<Semaphore>,
    exec: Executor,
    terminal_records: TerminalRecords,
    injectors: WebSocketInjectorRegistry,
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
