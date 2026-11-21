//! Rama is kept behind owned HTTP messages, bounded channels, and cancellation.

pub mod auth;
mod capture;
mod client;
pub mod client_tls;
pub mod group_send;
mod message;
mod route;
mod server;
pub mod streaming;
mod tls;
mod transport;

pub use auth::{
    AuthConfig, AuthCredentials, DigestChallenge, HttpAuthScheme, NtlmType2Challenge,
    PlatformAuthRuleConfig, UpstreamAuthRuleConfig, build_basic_authorization_value,
    build_ntlm_type1_header_value, build_ntlm_type3_header_value,
};
pub use capture::{CapturedFlow, CapturedFlowMetrics, StreamedResponse};
pub use client::{
    PreparedSend, SendConfig, SendGroupConfig, SendHttpConfig, SendLimits, SendMetrics,
    SendNetworkConfig, SendStreamOutcome, WebSocketExchange, WebSocketSession,
    connect_websocket_session, prepare_send, send, send_streaming, send_websocket,
};
pub use client_tls::{
    ClientCertificateFormat, ClientCertificateRule, ClientCertificateSummary,
    MAX_CLIENT_CERT_BYTES, MAX_CLIENT_CERT_PASSWORD_BYTES, ParsedClientIdentity, matches_host,
    resolve_client_identity, select_client_identity, select_client_rule,
};
pub use group_send::{RepeaterGroupSendMode, RepeaterGroupSendTicket, SinglePacketBarrier};
pub use route::{ProxyCredentials, ProxyEndpoint, UpstreamRoute, matches_bypass_host};
pub use server::{
    AnnotationChecker, AsyncFlowHook, AsyncWebSocketHook, BoxHookFuture, ClientTlsFailureCallback,
    DropOutOfScopeFn, FlowHookDecision, FlowTransform, InterceptDecision, InterceptEvalContext,
    InterceptFilter, InterceptFilterOutcome, ProxyConfig, ProxyEvent, ProxyHandle, ProxyHooks,
    ProxyHttpConfig, ProxyLimits, ProxyNetworkConfig, ProxySharedResources, ProxyTlsConfig,
    SslPassThroughMatcher, UpstreamRouteResolver, WebSocketHookDecision, WebSocketInjectorRegistry,
    WebSocketInterceptDecision, WebSocketInterceptFilter, WebSocketTransform, start,
};
pub use streaming::StreamingScopeMatcher;
pub use tls::{
    ListenerCertMode, ListenerTlsProtocols, MitmAuthority, ObservedCertInfo,
    ServerCertObservedCallback, TlsVersion, UpstreamTls, UpstreamTlsConfig, UpstreamTlsOptions,
    UpstreamTlsProtocolMode, parse_observed_cert,
};
pub use transport::build_dns_resolver;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("网络操作已取消")]
    Cancelled,
    #[error("网络操作超过期限")]
    Timeout,
    #[error("正文超过配置的 {limit} 字节上限；消息未被截断后发送")]
    BodyLimit { limit: usize },
    #[error("HTTP 消息无效：{0}")]
    InvalidMessage(String),
    #[error("TLS 验证或证书配置失败：{0}")]
    Tls(String),
    #[error("网络操作失败：{0}")]
    Network(String),
    #[error("拦截决定与当前阶段不匹配")]
    InterceptStage,
    #[error("拦截消费者已断开，消息未自动放行")]
    EventConsumerClosed,
    #[error("请求被丢弃")]
    Dropped,
    #[error("停止期限内未能回收全部网络任务")]
    StopDeadline,
}

impl ProxyError {
    fn tls(error: impl std::fmt::Display) -> Self {
        Self::Tls(error.to_string())
    }

    fn network(error: impl std::fmt::Display) -> Self {
        Self::Network(error.to_string())
    }

    fn invalid(error: impl std::fmt::Display) -> Self {
        Self::InvalidMessage(error.to_string())
    }
}

pub(crate) fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

pub(crate) fn lock_unpoison<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
