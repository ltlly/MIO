use std::{
    collections::VecDeque,
    io::{self, IoSlice},
    net::IpAddr,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock, atomic::Ordering},
    task::{Context, Poll, ready},
    time::Duration,
};

use mio_plugin_api::HttpVersion;
use rama::{
    Layer, Service,
    error::extra::OpaqueError,
    extensions::{Egress, Extension, Extensions, ExtensionsRef},
    graceful::Shutdown,
    http::{
        Body, Request, Response, Version,
        client::{
            EasyHttpConnectorBuilder, EasyHttpWebClient, HttpPooledConnectorConfig,
            builder::ProxyStage,
            proxy::layer::{SetProxyAuthHttpHeaderLayer, SetProxyAuthHttpHeaderService},
        },
        conn::{H2ClientContextParams, TargetHttpVersion},
    },
    io::Io,
    net::{
        client::{
            ConnectRequest, ConnectionError, ConnectionErrorKind, ConnectorService,
            EstablishedClientConnection,
        },
        stream::SocketInfo,
    },
    rt::Executor,
    service::BoxService,
    tls::client::{NegotiatedTlsParameters, TlsClientConfig},
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_util::sync::CancellationToken;

use crate::{
    ObservedCertInfo, ProxyError, UpstreamRoute, message,
    tls::ClientTlsCacheKey,
    transport::{
        CancellableConnector, MioProxyConnectorLayer, PooledRequestCancelExt, PooledTimingSlotExt,
        ResolvedPeerIp, ResponseTiming, apply_hostname_overrides,
    },
};

use super::SendConfig;

pub(crate) async fn exchange(
    request: Request,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<Response, ProxyError> {
    Box::pin(exchange_with_metrics(request, config, cancel, exec))
        .await
        .map(|(response, _timing)| response)
}

pub(super) struct RawH1Io<S> {
    pub(super) inner: S,
    pub(super) pending: Option<(Arc<[u8]>, usize)>,
    pub(super) cookie_fields: Option<(Arc<[u8]>, usize)>,
    pub(super) cookie_head: Vec<u8>,
    pub(super) cookie_offset: usize,
}

impl<S: AsyncWrite + Unpin> RawH1Io<S> {
    fn poll_cookie_head(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.cookie_fields.is_some() && !self.cookie_head.is_empty() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP/1 请求头尚未完整，未转发部分 Cookie 字段",
            )));
        }
        while self.cookie_offset < self.cookie_head.len() {
            let written = ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.cookie_head[self.cookie_offset..])
            )?;
            if written == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.cookie_offset += written;
        }
        self.cookie_head = Vec::new();
        self.cookie_offset = 0;
        Poll::Ready(Ok(()))
    }

    fn accept_cookie_head(&mut self, input: &[u8]) -> io::Result<usize> {
        let limit = self
            .cookie_fields
            .as_ref()
            .expect("Cookie override exists")
            .1;
        for (index, byte) in input.iter().enumerate() {
            if self.cookie_head.len() == limit {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HTTP/1 请求头超过实际请求及上游认证的字节预算",
                ));
            }
            self.cookie_head.push(*byte);
            if self.cookie_head.ends_with(b"\r\n\r\n") {
                let (fields, _) = self.cookie_fields.take().expect("Cookie override exists");
                let original = std::mem::take(&mut self.cookie_head);
                let mut restored = Vec::with_capacity(original.len().saturating_add(fields.len()));
                let mut inserted = false;
                for line in original.split_inclusive(|byte| *byte == b'\n') {
                    let cookie = line
                        .iter()
                        .position(|byte| *byte == b':')
                        .is_some_and(|colon| line[..colon].eq_ignore_ascii_case(b"cookie"));
                    if cookie {
                        if !inserted {
                            restored.extend_from_slice(&fields);
                            inserted = true;
                        }
                    } else {
                        restored.extend_from_slice(line);
                    }
                }
                if !inserted {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "HTTP/1 发送请求缺少原 Cookie 字段，未合成新字段",
                    ));
                }
                self.cookie_head = restored;
                return Ok(index + 1);
            }
            if self.cookie_head.ends_with(b"\r\n")
                && !self.cookie_head[..self.cookie_head.len() - 2].contains(&b'\n')
                && !self.cookie_head.ends_with(b" HTTP/1.0\r\n")
                && !self.cookie_head.ends_with(b" HTTP/1.1\r\n")
            {
                self.cookie_fields = None;
                return Ok(index + 1);
            }
        }
        Ok(input.len())
    }
}

impl<S: ExtensionsRef> ExtensionsRef for RawH1Io<S> {
    fn extensions(&self) -> &Extensions {
        self.inner.extensions()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for RawH1Io<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for RawH1Io<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if let Some((bytes, offset)) = &mut this.pending {
            while *offset < bytes.len() {
                let written = ready!(Pin::new(&mut this.inner).poll_write(cx, &bytes[*offset..]))?;
                if written == 0 {
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
                *offset += written;
            }
            return Poll::Ready(Ok(buf.len()));
        }
        if this.cookie_fields.is_some() {
            return Poll::Ready(this.accept_cookie_head(buf));
        }
        ready!(this.poll_cookie_head(cx))?;
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_cookie_head(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_cookie_head(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if self.pending.is_some() || self.cookie_fields.is_some() || !self.cookie_head.is_empty() {
            let buf = bufs.iter().find(|buf| !buf.is_empty());
            self.poll_write(cx, buf.map_or(&[], |buf| &**buf))
        } else {
            Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.pending.is_none() && self.inner.is_write_vectored()
    }
}

fn apply_post_tls(
    conn_ext: &Extensions,
    observer: Option<(&str, u16, &Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    force_http10: bool,
    allow_http2_alpn_override: bool,
) {
    if let Some((host, port, callback)) = observer
        && let Some(tls_params) = conn_ext.get_ref::<NegotiatedTlsParameters>()
        && let Some(chain) = &tls_params.peer_certificate_chain
        && let Some(leaf) = chain.first()
        && let Ok(info) = crate::tls::parse_observed_cert(host, port, leaf.as_ref())
    {
        callback(info);
    }
    if force_http10 {
        conn_ext.insert(TargetHttpVersion(Version::HTTP_10));
    } else if allow_http2_alpn_override {
        conn_ext.insert(TargetHttpVersion(Version::HTTP_2));
    }
}

#[derive(Clone)]
struct RawH1Connector<S> {
    inner: S,
    raw_h1: Option<message::RawH1Request>,
    cookie_fields: Option<(Arc<[u8]>, usize)>,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    cert_observer: Option<(String, u16, Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    single_packet_barrier: Option<crate::SinglePacketBarrier>,
    cancel: CancellationToken,
}

impl<S, Input> Service<Input> for RawH1Connector<S>
where
    S: ConnectorService<Input>,
    S::Connection: Io + Unpin + ExtensionsRef,
    Input: Send + 'static,
{
    type Output = EstablishedClientConnection<RawH1Io<S::Connection>, Input>;
    type Error = ConnectionError;

    async fn serve(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let EstablishedClientConnection { input, mut conn } =
            Box::pin(self.inner.connect(input)).await?;
        let observer = self
            .cert_observer
            .as_ref()
            .map(|(h, p, cb)| (h.as_str(), *p, cb));
        apply_post_tls(
            conn.extensions(),
            observer,
            self.force_http10,
            self.allow_http2_alpn_override,
        );
        let is_h2 = conn
            .extensions()
            .get_ref::<TargetHttpVersion>()
            .is_some_and(|target| target.0 == Version::HTTP_2);
        let pending = match (&self.raw_h1, is_h2, &self.single_packet_barrier) {
            (Some(raw), false, Some(barrier)) if raw.bytes.len() > 1 => {
                raw.suppressed.store(true, Ordering::Release);
                let split_at = raw.bytes.len() - 1;
                conn.write_all(&raw.bytes[..split_at])
                    .await
                    .map_err(|err| {
                        ConnectionError::transport(err, ConnectionErrorKind::Unavailable)
                    })?;
                conn.flush()
                    .await
                    .map_err(|err| {
                        ConnectionError::transport(err, ConnectionErrorKind::Unavailable)
                    })?;
                barrier
                    .wait_or_cancel(&self.cancel)
                    .await
                    .map_err(|err| ConnectionError::transport(err, ConnectionErrorKind::Unavailable))?;
                Some((raw.bytes.clone(), split_at))
            }
            (Some(raw), false, maybe_barrier) => {
                raw.suppressed.store(true, Ordering::Release);
                if let Some(barrier) = maybe_barrier {
                    barrier.wait_or_cancel(&self.cancel).await.map_err(|err| {
                        ConnectionError::transport(err, ConnectionErrorKind::Unavailable)
                    })?;
                }
                Some((raw.bytes.clone(), 0))
            }
            (_, _, Some(barrier)) => {
                barrier
                    .wait_or_cancel(&self.cancel)
                    .await
                    .map_err(|err| ConnectionError::transport(err, ConnectionErrorKind::Unavailable))?;
                None
            }
            _ => None,
        };
        Ok(EstablishedClientConnection {
            input,
            conn: RawH1Io {
                inner: conn,
                pending,
                cookie_fields: (!is_h2 && self.raw_h1.is_none())
                    .then(|| self.cookie_fields.clone())
                    .flatten(),
                cookie_head: Vec::new(),
                cookie_offset: 0,
            },
        })
    }
}

#[derive(Clone)]
struct PooledCertObserverExt {
    host: String,
    port: u16,
    callback: Arc<dyn Fn(ObservedCertInfo) + Send + Sync>,
}

impl std::fmt::Debug for PooledCertObserverExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledCertObserverExt")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Extension for PooledCertObserverExt {}

#[derive(Clone)]
struct PooledTimingExt {
    timing: Arc<Mutex<ResponseTiming>>,
    is_direct: bool,
}

impl std::fmt::Debug for PooledTimingExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledTimingExt")
            .field("is_direct", &self.is_direct)
            .finish_non_exhaustive()
    }
}

impl Extension for PooledTimingExt {}

#[derive(Clone)]
struct PooledTlsPostConnector<S> {
    inner: S,
    force_http10: bool,
    allow_http2_alpn_override: bool,
}

impl<S, Input> Service<Input> for PooledTlsPostConnector<S>
where
    S: ConnectorService<Input>,
    S::Connection: Io + Unpin + ExtensionsRef,
    Input: ExtensionsRef + Send + 'static,
{
    type Output = EstablishedClientConnection<S::Connection, Input>;
    type Error = ConnectionError;

    async fn serve(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let EstablishedClientConnection { input, conn } =
            Box::pin(self.inner.connect(input)).await?;
        let observer = input.extensions().get_ref::<PooledCertObserverExt>();
        apply_post_tls(
            conn.extensions(),
            observer.map(|o| (o.host.as_str(), o.port, &o.callback)),
            self.force_http10,
            self.allow_http2_alpn_override,
        );
        Ok(EstablishedClientConnection { input, conn })
    }
}

#[derive(Clone, Default)]
struct PooledJitLayer;

impl<S> Layer<S> for PooledJitLayer {
    type Service = PooledJitService<SetProxyAuthHttpHeaderService<S>>;

    fn layer(&self, inner: S) -> Self::Service {
        PooledJitService {
            inner: SetProxyAuthHttpHeaderLayer::new().layer(inner),
        }
    }
}

#[derive(Clone)]
struct PooledJitService<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for PooledJitService<S>
where
    S: Service<Request<B>>,
    B: Send + 'static,
{
    type Output = S::Output;
    type Error = S::Error;

    async fn serve(&self, req: Request<B>) -> Result<Self::Output, Self::Error> {
        let _timing_guard = if let Some(timing_ext) = req.extensions().get_ref::<PooledTimingExt>()
        {
            if let Some(egress) = req.extensions().get_ref::<Egress<Extensions>>() {
                if timing_ext.is_direct
                    && let Some(socket_info) = egress.0.get_ref::<SocketInfo>()
                {
                    crate::lock_unpoison(&timing_ext.timing)
                        .set_peer_ip(socket_info.peer_addr().ip_addr);
                } else if let Some(resolved_ip) = egress.0.get_ref::<ResolvedPeerIp>() {
                    crate::lock_unpoison(&timing_ext.timing).set_peer_ip(resolved_ip.0);
                }
                egress
                    .0
                    .get_ref::<PooledTimingSlotExt>()
                    .map(|slot| slot.0.attach(timing_ext.timing.clone()))
            } else {
                None
            }
        } else {
            None
        };
        Box::pin(self.inner.serve(req)).await
    }
}

const POOLED_CLIENT_CACHE_LIMIT: usize = 32;
type PooledClient = BoxService<Request, Response, OpaqueError>;

struct PoolLifecycle {
    cancel: CancellationToken,
    _shutdown: Shutdown,
}

impl Drop for PoolLifecycle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Clone)]
struct PooledClientEntry {
    key: PooledClientKey,
    client: PooledClient,
    _lifecycle: Arc<PoolLifecycle>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PooledRouteKind {
    Direct,
    PlainProxy,
    HttpsProxy(ClientTlsCacheKey),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PooledClientKey {
    is_plain_http: bool,
    destination_tls: ClientTlsCacheKey,
    route_kind: PooledRouteKind,
    default_http_version: u8,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    http1_keep_alive: bool,
    http2_reuse: bool,
    hostname_overrides: Vec<(String, IpAddr)>,
    connection_pool_group_id: Option<u128>,
}

fn pooled_client_cache() -> &'static Mutex<VecDeque<PooledClientEntry>> {
    static CACHE: OnceLock<Mutex<VecDeque<PooledClientEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(POOLED_CLIENT_CACHE_LIMIT)))
}

#[cfg(test)]
pub(crate) fn pooled_client_cache_len() -> usize {
    crate::lock_unpoison(pooled_client_cache()).len()
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn finish_pooled_client<T, const PROXY: bool>(
    builder: EasyHttpConnectorBuilder<T, ProxyStage<PROXY>>,
    tls: TlsClientConfig,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    pool_config: HttpPooledConnectorConfig,
    exec: Executor,
) -> Result<PooledClient, ProxyError>
where
    T: ConnectorService<ConnectRequest, Connection: Io + Unpin + ExtensionsRef>,
{
    let client = builder
        .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
        .map_connector(|inner| PooledTlsPostConnector {
            inner,
            force_http10,
            allow_http2_alpn_override,
        })
        .with_default_http_connector::<Body>(exec)
        .without_proxy_route_failure_cache()
        // Erase the nested TLS/proxy/HTTP connector before pooling and route fallback
        // (matching Rama's with_proxy_route_failure_cache boundary) so pooled futures stay stack-safe.
        .map_connector(BoxService::new)
        .try_with_connection_pool(pool_config)
        .map_err(ProxyError::network)?
        .build_client::<Body, Body, _>()
        .with_jit_layer(PooledJitLayer);
    Ok(BoxService::new(client))
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn build_pooled_https_proxy_client(
    tls: TlsClientConfig,
    proxy_tls: TlsClientConfig,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    overrides: &[(String, IpAddr)],
    pool_config: HttpPooledConnectorConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<PooledClient, ProxyError> {
    let proxy_layer = MioProxyConnectorLayer::new(overrides, None)?;
    let stage = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(move |inner| CancellableConnector {
            inner,
            cancel: cancel.clone(),
            timing: None,
        })
        .with_default_dns_connector()
        .with_tls_proxy_support_using_rustls_config(proxy_tls)
        .with_custom_proxy_connector(proxy_layer);
    finish_pooled_client(
        stage,
        tls,
        default_http_version,
        force_http10,
        allow_http2_alpn_override,
        pool_config,
        exec,
    )
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn build_pooled_direct_client(
    tls: TlsClientConfig,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    pool_config: HttpPooledConnectorConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<PooledClient, ProxyError> {
    let stage = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(move |inner| CancellableConnector {
            inner,
            cancel: cancel.clone(),
            timing: None,
        })
        .with_default_dns_connector()
        .without_tls_proxy_support()
        .without_proxy_support();
    finish_pooled_client(
        stage,
        tls,
        default_http_version,
        force_http10,
        allow_http2_alpn_override,
        pool_config,
        exec,
    )
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn build_pooled_plain_proxy_client(
    tls: TlsClientConfig,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    overrides: &[(String, IpAddr)],
    pool_config: HttpPooledConnectorConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<PooledClient, ProxyError> {
    let proxy_layer = MioProxyConnectorLayer::new(overrides, None)?;
    let stage = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(move |inner| CancellableConnector {
            inner,
            cancel: cancel.clone(),
            timing: None,
        })
        .with_default_dns_connector()
        .without_tls_proxy_support()
        .with_custom_proxy_connector(proxy_layer);
    finish_pooled_client(
        stage,
        tls,
        default_http_version,
        force_http10,
        allow_http2_alpn_override,
        pool_config,
        exec,
    )
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn build_pooled_client_service(
    tls: TlsClientConfig,
    proxy_tls: Option<TlsClientConfig>,
    is_direct: bool,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    overrides: &[(String, IpAddr)],
    pool_config: HttpPooledConnectorConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<PooledClient, ProxyError> {
    if let Some(proxy_tls) = proxy_tls {
        build_pooled_https_proxy_client(
            tls,
            proxy_tls,
            default_http_version,
            force_http10,
            allow_http2_alpn_override,
            overrides,
            pool_config,
            cancel,
            exec,
        )
    } else if is_direct && overrides.is_empty() {
        build_pooled_direct_client(
            tls,
            default_http_version,
            force_http10,
            allow_http2_alpn_override,
            pool_config,
            cancel,
            exec,
        )
    } else {
        build_pooled_plain_proxy_client(
            tls,
            default_http_version,
            force_http10,
            allow_http2_alpn_override,
            overrides,
            pool_config,
            cancel,
            exec,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn get_or_build_pooled_client(
    key: PooledClientKey,
    tls: TlsClientConfig,
    proxy_tls: Option<TlsClientConfig>,
    is_direct: bool,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    overrides: &[(String, IpAddr)],
) -> Result<(PooledClient, Arc<PoolLifecycle>), ProxyError> {
    {
        let mut cache = crate::lock_unpoison(pooled_client_cache());
        if let Some(index) = cache.iter().position(|entry| entry.key == key) {
            let entry = cache.remove(index).expect("position came from this cache");
            let out = (entry.client.clone(), entry._lifecycle.clone());
            cache.push_back(entry);
            return Ok(out);
        }
    }
    let pool_config = HttpPooledConnectorConfig {
        max_total: if key.connection_pool_group_id.is_some() && !key.http2_reuse {
            1
        } else {
            64
        },
        max_concurrent_streams: 100,
        idle_timeout: Some(Duration::from_secs(60)),
        wait_for_pool_timeout: Some(Duration::from_secs(30)),
        ..HttpPooledConnectorConfig::default()
    };
    let pool_cancel = CancellationToken::new();
    let pool_shutdown = Shutdown::new(pool_cancel.clone().cancelled_owned());
    let exec = Executor::graceful(pool_shutdown.guard());
    let client = build_pooled_client_service(
        tls,
        proxy_tls,
        is_direct,
        default_http_version,
        force_http10,
        allow_http2_alpn_override,
        overrides,
        pool_config,
        pool_cancel.clone(),
        exec,
    )?;
    let lifecycle = Arc::new(PoolLifecycle {
        cancel: pool_cancel,
        _shutdown: pool_shutdown,
    });
    let entry = PooledClientEntry {
        key,
        client,
        _lifecycle: lifecycle,
    };
    let mut cache = crate::lock_unpoison(pooled_client_cache());
    if let Some(index) = cache.iter().position(|existing| existing.key == entry.key) {
        let existing = cache.remove(index).expect("position came from this cache");
        let out = (existing.client.clone(), existing._lifecycle.clone());
        cache.push_back(existing);
        return Ok(out);
    }
    while cache.len() >= POOLED_CLIENT_CACHE_LIMIT {
        cache.pop_front();
    }
    let out = (entry.client.clone(), entry._lifecycle.clone());
    cache.push_back(entry);
    Ok(out)
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn finish_direct_client<T>(
    builder: EasyHttpConnectorBuilder<T, ProxyStage<true>>,
    tls: TlsClientConfig,
    default_http_version: Version,
    exec: Executor,
    raw_h1: Option<message::RawH1Request>,
    cookie_fields: Option<(Arc<[u8]>, usize)>,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    cert_observer: Option<(String, u16, Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    single_packet_barrier: Option<crate::SinglePacketBarrier>,
    cancel: CancellationToken,
) -> PooledClient
where
    T: ConnectorService<ConnectRequest, Connection: Io + Unpin + ExtensionsRef>,
{
    let client = builder
        .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
        .map_connector(|inner| RawH1Connector {
            inner,
            raw_h1,
            cookie_fields,
            force_http10,
            allow_http2_alpn_override,
            cert_observer,
            single_packet_barrier,
            cancel,
        })
        .with_default_http_connector::<Body>(exec)
        .without_connection_pool()
        .build_client::<Body, Body, _>()
        .with_jit_layer(SetProxyAuthHttpHeaderLayer::new());
    BoxService::new(client)
}

struct PreparedExchange {
    raw_h1: Option<message::RawH1Request>,
    cookie_fields: Option<(Arc<[u8]>, usize)>,
    is_direct: bool,
    is_secure_scheme: bool,
    tls: TlsClientConfig,
    destination_tls_key: ClientTlsCacheKey,
    proxy_tls_and_key: Option<(TlsClientConfig, ClientTlsCacheKey)>,
    cert_observer: Option<(String, u16, Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    default_http_version: Version,
    force_http10: bool,
    allow_http2_alpn_override: bool,
}

fn prepare_exchange(request: &mut Request, config: &SendConfig) -> Result<PreparedExchange, ProxyError> {
    let raw_h1 = request
        .extensions()
        .get_ref::<message::RawH1Request>()
        .cloned();
    let cookie_fields =
        if raw_h1.is_none() && request.headers().get_all("cookie").iter().take(2).count() == 2 {
            let mut fields = Vec::new();
            let header_limit = request
                .headers()
                .ordered_iter()
                .try_fold(
                    request
                        .method()
                        .as_str()
                        .len()
                        .saturating_add(request.uri().to_string().len())
                        .saturating_add(16 * 1024),
                    |size, (name, value)| {
                        size.checked_add(name.as_str().len())
                            .and_then(|size| size.checked_add(value.as_bytes().len()))
                            .and_then(|size| size.checked_add(4))
                    },
                )
                .ok_or_else(|| ProxyError::invalid("HTTP 请求头字节计数溢出"))?;
            for (name, value) in request.headers().ordered_iter() {
                if name.as_str().eq_ignore_ascii_case("cookie") {
                    name.write_original(&mut fields);
                    fields.extend_from_slice(b": ");
                    fields.extend_from_slice(value.as_bytes());
                    fields.extend_from_slice(b"\r\n");
                }
            }
            Some((Arc::from(fields), header_limit))
        } else {
            None
        };
    let target_host = request.uri().host_str().map(|host| host.into_owned());
    let effective_route = target_host.as_deref().map_or_else(
        || config.network.upstream_route.clone(),
        |host| config.network.upstream_route.resolve_for_host(host),
    );
    let is_direct = matches!(effective_route, UpstreamRoute::Direct);
    request.extensions().insert(effective_route.rama()?);
    request.extensions().insert(H2ClientContextParams {
        keep_alive_while_idle: Some(config.http.http2_reuse),
        ..Default::default()
    });
    apply_hostname_overrides(request.extensions(), &config.network.hostname_overrides)?;
    if let Some(version) = config.http.version {
        *request.version_mut() = message::version(version);
    }
    let force_http10 = config.http.version == Some(HttpVersion::Http10);
    let allow_http2_alpn_override = config.http.allow_http2_alpn_override
        && config.http.version == Some(HttpVersion::Http2);
    let is_secure_scheme = request.uri().scheme_str().is_some_and(|scheme| {
        scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss")
    });
    let prefer_http2_by_default = is_secure_scheme
        && config.http.default_to_http2
        && !force_http10
        && raw_h1.is_none()
        && !request.headers().contains_key("upgrade");
    let tls_alpn_version = if allow_http2_alpn_override || prefer_http2_by_default {
        None
    } else {
        config.http.version
    };
    let client_identity = if is_secure_scheme
        && let Some(host) = target_host.as_deref()
    {
        crate::select_client_identity(&config.tls.client_certificates, host)
    } else {
        None
    };
    let (tls, destination_tls_key) = if !is_secure_scheme
        && matches!(
            config.tls.trust,
            crate::UpstreamTls::System | crate::UpstreamTls::Insecure
        ) {
        config.tls.options.validate()?;
        (
            TlsClientConfig::default(),
            ClientTlsCacheKey {
                trust: match config.tls.trust {
                    crate::UpstreamTls::Insecure => crate::tls::TrustModeKey::Insecure,
                    _ => crate::tls::TrustModeKey::System,
                },
                alpn: crate::tls::ClientAlpnMode::from_http_version(tls_alpn_version),
                options: config.tls.options,
                client_cert_sha256: None,
                store_server_cert_chain: false,
            },
        )
    } else {
        config.tls.trust.config_and_key(
            tls_alpn_version,
            &config.tls.options,
            client_identity,
            config.tls.on_server_cert_observed.is_some(),
        )?
    };
    let proxy_tls_and_key = effective_route
        .https_proxy_tls_config_and_key(&config.tls.client_certificates, &config.tls.options)?;
    let cert_observer = if is_secure_scheme {
        config.tls.on_server_cert_observed.clone().and_then(|callback| {
            let host = target_host.clone()?;
            let port = request.uri().port_u16().unwrap_or(443);
            Some((host, port, callback))
        })
    } else {
        None
    };
    let default_http_version = if force_http10 {
        Version::HTTP_10
    } else if allow_http2_alpn_override {
        Version::HTTP_2
    } else {
        Version::HTTP_11
    };
    Ok(PreparedExchange {
        raw_h1,
        cookie_fields,
        is_direct,
        is_secure_scheme,
        tls,
        destination_tls_key,
        proxy_tls_and_key,
        cert_observer,
        default_http_version,
        force_http10,
        allow_http2_alpn_override,
    })
}

fn map_client_error(err: OpaqueError) -> ProxyError {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(&err);
    while let Some(e) = cur {
        if let Some(proxy_err) = e.downcast_ref::<ProxyError>() {
            match proxy_err {
                ProxyError::Cancelled => return ProxyError::Cancelled,
                ProxyError::Timeout => return ProxyError::Timeout,
                _ => {}
            }
        }
        cur = e.source();
    }
    ProxyError::network(err)
}

fn attach_pooled_request_extensions(
    request: &mut Request,
    prepared: &PreparedExchange,
    timing: &Arc<Mutex<ResponseTiming>>,
    cancel: &CancellationToken,
) {
    if let Some((host, port, callback)) = &prepared.cert_observer {
        request.extensions().insert(PooledCertObserverExt {
            host: host.clone(),
            port: *port,
            callback: callback.clone(),
        });
    }
    request.extensions().insert(PooledTimingExt {
        timing: timing.clone(),
        is_direct: prepared.is_direct,
    });
    request
        .extensions()
        .insert(PooledRequestCancelExt(cancel.clone()));
}

/// Dedicated non-global single-connection pooled client scoped to a single NTLM handshake.
pub(super) struct EphemeralNtlmClient {
    client: PooledClient,
}

impl EphemeralNtlmClient {
    pub(super) fn for_request(
        request: &mut Request,
        config: &SendConfig,
        cancel: CancellationToken,
        exec: Executor,
    ) -> Result<Self, ProxyError> {
        let prepared = prepare_exchange(request, config)?;
        let proxy_tls = prepared.proxy_tls_and_key.map(|(tls, _)| tls);
        let pool_config = HttpPooledConnectorConfig {
            max_total: 1,
            max_concurrent_streams: 1,
            idle_timeout: Some(Duration::from_secs(30)),
            wait_for_pool_timeout: Some(config.limits.timeout),
            ..HttpPooledConnectorConfig::default()
        };
        let client = build_pooled_client_service(
            prepared.tls,
            proxy_tls,
            prepared.is_direct,
            prepared.default_http_version,
            prepared.force_http10,
            prepared.allow_http2_alpn_override,
            &config.network.hostname_overrides,
            pool_config,
            cancel,
            exec,
        )?;
        Ok(Self { client })
    }

    pub(super) async fn exchange(
        &self,
        mut request: Request,
        config: &SendConfig,
        cancel: CancellationToken,
    ) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
        let prepared = prepare_exchange(&mut request, config)?;
        let timing = Arc::new(Mutex::new(ResponseTiming::default()));
        attach_pooled_request_extensions(&mut request, &prepared, &timing, &cancel);
        if let Some(barrier) = &config.group.single_packet_barrier {
            barrier.wait_or_cancel(&cancel).await?;
        }
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            result = Box::pin(self.client.serve(request)) => result.map_err(map_client_error)?,
        };
        crate::lock_unpoison(&timing).lock();
        Ok((response, timing))
    }
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn build_direct_https_proxy_client(
    tls: TlsClientConfig,
    proxy_tls: TlsClientConfig,
    default_http_version: Version,
    exec: Executor,
    raw_h1: Option<message::RawH1Request>,
    cookie_fields: Option<(Arc<[u8]>, usize)>,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    cert_observer: Option<(String, u16, Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    single_packet_barrier: Option<crate::SinglePacketBarrier>,
    overrides: &[(String, IpAddr)],
    timing: Arc<Mutex<ResponseTiming>>,
    cancel: CancellationToken,
) -> Result<PooledClient, ProxyError> {
    let proxy_layer = MioProxyConnectorLayer::new(overrides, Some(timing.clone()))?;
    let c = cancel.clone();
    let stage = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(move |inner| CancellableConnector {
            inner,
            cancel: c,
            timing: Some(timing),
        })
        .with_default_dns_connector()
        .with_tls_proxy_support_using_rustls_config(proxy_tls)
        .with_custom_proxy_connector(proxy_layer);
    Ok(finish_direct_client(
        stage,
        tls,
        default_http_version,
        exec,
        raw_h1,
        cookie_fields,
        force_http10,
        allow_http2_alpn_override,
        cert_observer,
        single_packet_barrier,
        cancel,
    ))
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn build_direct_plain_client(
    tls: TlsClientConfig,
    default_http_version: Version,
    exec: Executor,
    raw_h1: Option<message::RawH1Request>,
    cookie_fields: Option<(Arc<[u8]>, usize)>,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    cert_observer: Option<(String, u16, Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    single_packet_barrier: Option<crate::SinglePacketBarrier>,
    overrides: &[(String, IpAddr)],
    timing: Arc<Mutex<ResponseTiming>>,
    cancel: CancellationToken,
) -> Result<PooledClient, ProxyError> {
    let proxy_layer = MioProxyConnectorLayer::new(overrides, Some(timing.clone()))?;
    let c = cancel.clone();
    let stage = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(move |inner| CancellableConnector {
            inner,
            cancel: c,
            timing: Some(timing),
        })
        .with_default_dns_connector()
        .without_tls_proxy_support()
        .with_custom_proxy_connector(proxy_layer);
    Ok(finish_direct_client(
        stage,
        tls,
        default_http_version,
        exec,
        raw_h1,
        cookie_fields,
        force_http10,
        allow_http2_alpn_override,
        cert_observer,
        single_packet_barrier,
        cancel,
    ))
}

#[inline(never)]
fn build_direct_client_service(
    prepared: PreparedExchange,
    overrides: &[(String, IpAddr)],
    timing: Arc<Mutex<ResponseTiming>>,
    single_packet_barrier: Option<crate::SinglePacketBarrier>,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<PooledClient, ProxyError> {
    if let Some((proxy_tls, _)) = prepared.proxy_tls_and_key {
        build_direct_https_proxy_client(
            prepared.tls,
            proxy_tls,
            prepared.default_http_version,
            exec,
            prepared.raw_h1,
            prepared.cookie_fields,
            prepared.force_http10,
            prepared.allow_http2_alpn_override,
            prepared.cert_observer,
            single_packet_barrier,
            overrides,
            timing,
            cancel,
        )
    } else {
        build_direct_plain_client(
            prepared.tls,
            prepared.default_http_version,
            exec,
            prepared.raw_h1,
            prepared.cookie_fields,
            prepared.force_http10,
            prepared.allow_http2_alpn_override,
            prepared.cert_observer,
            single_packet_barrier,
            overrides,
            timing,
            cancel,
        )
    }
}

pub(crate) async fn exchange_with_metrics(
    mut request: Request,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
    let prepared = prepare_exchange(&mut request, config)?;
    let timing = Arc::new(Mutex::new(ResponseTiming::default()));

    let version_allows_reuse = match config.http.version {
        Some(HttpVersion::Http2) => config.http.http2_reuse,
        Some(_) => config.http.http1_keep_alive,
        None => config.http.http1_keep_alive || config.http.http2_reuse,
    };
    let use_pool = config.http.use_connection_pool
        && version_allows_reuse
        && prepared.raw_h1.is_none()
        && prepared.cookie_fields.is_none()
        && !request.headers().contains_key("upgrade");

    let response = if use_pool {
        attach_pooled_request_extensions(&mut request, &prepared, &timing, &cancel);
        let (proxy_tls, route_kind) = match prepared.proxy_tls_and_key {
            Some((proxy_tls, proxy_key)) => {
                (Some(proxy_tls), PooledRouteKind::HttpsProxy(proxy_key))
            }
            None if prepared.is_direct => (None, PooledRouteKind::Direct),
            None => (None, PooledRouteKind::PlainProxy),
        };
        let default_http_version_key = if prepared.force_http10 {
            0
        } else if prepared.allow_http2_alpn_override {
            2
        } else {
            1
        };
        let pool_key = PooledClientKey {
            is_plain_http: !prepared.is_secure_scheme,
            destination_tls: prepared.destination_tls_key,
            route_kind,
            default_http_version: default_http_version_key,
            force_http10: prepared.force_http10,
            allow_http2_alpn_override: prepared.allow_http2_alpn_override,
            http1_keep_alive: config.http.http1_keep_alive,
            http2_reuse: config.http.http2_reuse,
            hostname_overrides: config.network.hostname_overrides.to_vec(),
            connection_pool_group_id: config.group.connection_pool_group_id,
        };
        let (client, _lifecycle) = get_or_build_pooled_client(
            pool_key,
            prepared.tls,
            proxy_tls,
            prepared.is_direct,
            prepared.default_http_version,
            prepared.force_http10,
            prepared.allow_http2_alpn_override,
            &config.network.hostname_overrides,
        )?;
        if let Some(barrier) = &config.group.single_packet_barrier {
            barrier.wait_or_cancel(&cancel).await?;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            result = Box::pin(client.serve(request)) => result.map_err(map_client_error)?,
        }
    } else {
        let client = build_direct_client_service(
            prepared,
            &config.network.hostname_overrides,
            timing.clone(),
            config.group.single_packet_barrier.clone(),
            cancel,
            exec,
        )?;
        Box::pin(client.serve(request))
            .await
            .map_err(map_client_error)?
    };
    crate::lock_unpoison(&timing).lock();
    Ok((response, timing))
}
