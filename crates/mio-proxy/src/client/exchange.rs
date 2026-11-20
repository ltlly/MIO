use std::{
    collections::VecDeque,
    future::Future,
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
    http::{
        Body, Request, Response, Version,
        client::{
            EasyHttpWebClient, HttpPooledConnectorConfig,
            proxy::layer::{SetProxyAuthHttpHeaderLayer, SetProxyAuthHttpHeaderService},
        },
        conn::{H2ClientContextParams, TargetHttpVersion},
    },
    io::Io,
    net::{
        client::{
            ConnectionError, ConnectionErrorKind, ConnectorService, EstablishedClientConnection,
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
    transport::{CancellableConnector, MioProxyConnectorLayer, ResponseTiming, apply_hostname_overrides},
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

#[derive(Clone)]
struct RawH1Connector<S> {
    inner: S,
    raw_h1: Option<message::RawH1Request>,
    cookie_fields: Option<(Arc<[u8]>, usize)>,
    force_http10: bool,
    allow_http2_alpn_override: bool,
    cert_observer: Option<(String, u16, Arc<dyn Fn(ObservedCertInfo) + Send + Sync>)>,
    single_packet_barrier: Option<crate::SinglePacketBarrier>,
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
        if let Some((host, port, callback)) = &self.cert_observer
            && let Some(tls_params) = conn.extensions().get_ref::<NegotiatedTlsParameters>()
            && let Some(chain) = &tls_params.peer_certificate_chain
            && let Some(leaf) = chain.first()
            && let Ok(info) = crate::tls::parse_observed_cert(host, *port, leaf.as_ref())
        {
            callback(info);
        }
        if self.force_http10 {
            conn.extensions().insert(TargetHttpVersion(Version::HTTP_10));
        } else if self.allow_http2_alpn_override {
            conn.extensions().insert(TargetHttpVersion(Version::HTTP_2));
        }
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
                barrier.wait().await;
                Some((raw.bytes.clone(), split_at))
            }
            (Some(raw), false, maybe_barrier) => {
                raw.suppressed.store(true, Ordering::Release);
                if let Some(barrier) = maybe_barrier {
                    barrier.wait().await;
                }
                Some((raw.bytes.clone(), 0))
            }
            (_, _, Some(barrier)) => {
                barrier.wait().await;
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
        if let Some(observer) = input.extensions().get_ref::<PooledCertObserverExt>()
            && let Some(tls_params) = conn.extensions().get_ref::<NegotiatedTlsParameters>()
            && let Some(chain) = &tls_params.peer_certificate_chain
            && let Some(leaf) = chain.first()
            && let Ok(info) =
                crate::tls::parse_observed_cert(&observer.host, observer.port, leaf.as_ref())
        {
            (observer.callback)(info);
        }
        if self.force_http10 {
            conn.extensions().insert(TargetHttpVersion(Version::HTTP_10));
        } else if self.allow_http2_alpn_override {
            conn.extensions().insert(TargetHttpVersion(Version::HTTP_2));
        }
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

    fn serve(
        &self,
        req: Request<B>,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send + '_ {
        if let Some(timing_ext) = req.extensions().get_ref::<PooledTimingExt>()
            && timing_ext.is_direct
            && let Some(egress) = req.extensions().get_ref::<Egress<Extensions>>()
            && let Some(socket_info) = egress.0.get_ref::<SocketInfo>()
        {
            timing_ext
                .timing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_peer_ip(socket_info.peer_addr().ip_addr);
        }
        self.inner.serve(req)
    }
}

const POOLED_CLIENT_CACHE_LIMIT: usize = 32;
type PooledClient = BoxService<Request, Response, OpaqueError>;

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

fn pooled_client_cache() -> &'static Mutex<VecDeque<(PooledClientKey, PooledClient)>> {
    static CACHE: OnceLock<Mutex<VecDeque<(PooledClientKey, PooledClient)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(POOLED_CLIENT_CACHE_LIMIT)))
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
    overrides: Arc<[(String, IpAddr)]>,
) -> Result<PooledClient, ProxyError> {
    {
        let mut cache = pooled_client_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = cache.iter().position(|(cached_key, _)| *cached_key == key) {
            let entry = cache.remove(index).expect("position came from this cache");
            let client = entry.1.clone();
            cache.push_back(entry);
            return Ok(client);
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
    let exec = Executor::default();
    let client: PooledClient = if let Some(proxy_tls) = proxy_tls {
        let client = EasyHttpWebClient::connector_builder()
            .with_default_transport_connector()
            .with_default_dns_connector()
            .with_tls_proxy_support_using_rustls_config(proxy_tls)
            .with_custom_proxy_connector(MioProxyConnectorLayer {
                overrides,
                timing: Arc::new(Mutex::new(ResponseTiming::default())),
            })
            .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
            .map_connector(|inner| PooledTlsPostConnector {
                inner,
                force_http10,
                allow_http2_alpn_override,
            })
            .with_default_http_connector::<Body>(exec)
            .without_proxy_route_failure_cache()
            .try_with_connection_pool(pool_config)
            .map_err(ProxyError::network)?
            .build_client::<Body, Body, _>()
            .with_jit_layer(PooledJitLayer);
        BoxService::new(client)
    } else if is_direct && overrides.is_empty() {
        let client = EasyHttpWebClient::connector_builder()
            .with_default_transport_connector()
            .with_default_dns_connector()
            .without_tls_proxy_support()
            .without_proxy_support()
            .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
            .map_connector(|inner| PooledTlsPostConnector {
                inner,
                force_http10,
                allow_http2_alpn_override,
            })
            .with_default_http_connector::<Body>(exec)
            .without_proxy_route_failure_cache()
            .try_with_connection_pool(pool_config)
            .map_err(ProxyError::network)?
            .build_client::<Body, Body, _>()
            .with_jit_layer(PooledJitLayer);
        BoxService::new(client)
    } else {
        let client = EasyHttpWebClient::connector_builder()
            .with_default_transport_connector()
            .with_default_dns_connector()
            .without_tls_proxy_support()
            .with_custom_proxy_connector(MioProxyConnectorLayer {
                overrides,
                timing: Arc::new(Mutex::new(ResponseTiming::default())),
            })
            .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
            .map_connector(|inner| PooledTlsPostConnector {
                inner,
                force_http10,
                allow_http2_alpn_override,
            })
            .with_default_http_connector::<Body>(exec)
            .without_proxy_route_failure_cache()
            .try_with_connection_pool(pool_config)
            .map_err(ProxyError::network)?
            .build_client::<Body, Body, _>()
            .with_jit_layer(PooledJitLayer);
        BoxService::new(client)
    };
    {
        let mut cache = pooled_client_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = cache.iter().position(|(cached_key, _)| *cached_key == key) {
            let entry = cache.remove(index).expect("position came from this cache");
            let existing = entry.1.clone();
            cache.push_back(entry);
            return Ok(existing);
        }
        while cache.len() >= POOLED_CLIENT_CACHE_LIMIT {
            cache.pop_front();
        }
        cache.push_back((key, client.clone()));
    }
    Ok(client)
}

pub(crate) async fn exchange_with_metrics(
    mut request: Request,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
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
        || config.upstream_route.clone(),
        |host| config.upstream_route.resolve_for_host(host),
    );
    let is_direct = matches!(effective_route, UpstreamRoute::Direct);
    request.extensions().insert(effective_route.rama()?);
    request.extensions().insert(H2ClientContextParams {
        keep_alive_while_idle: Some(config.http2_reuse),
        ..Default::default()
    });
    apply_hostname_overrides(request.extensions(), &config.hostname_overrides)?;
    if let Some(version) = config.http_version {
        *request.version_mut() = message::version(version);
    }
    let force_http10 = config.http_version == Some(HttpVersion::Http10);
    let allow_http2_alpn_override =
        config.allow_http2_alpn_override && config.http_version == Some(HttpVersion::Http2);
    let is_secure_scheme = request.uri().scheme_str().is_some_and(|scheme| {
        scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss")
    });
    let prefer_http2_by_default = is_secure_scheme
        && config.default_to_http2
        && !force_http10
        && raw_h1.is_none()
        && !request.headers().contains_key("upgrade");
    let tls_alpn_version = if allow_http2_alpn_override || prefer_http2_by_default {
        None
    } else {
        config.http_version
    };
    let client_identity = if is_secure_scheme
        && let Some(host) = target_host.as_deref()
    {
        crate::select_client_identity(&config.client_certificates, host)
    } else {
        None
    };
    let (tls, destination_tls_key) = if !is_secure_scheme
        && matches!(
            config.upstream_tls,
            crate::UpstreamTls::System | crate::UpstreamTls::Insecure
        ) {
        config.upstream_tls_options.validate()?;
        (
            TlsClientConfig::default(),
            ClientTlsCacheKey {
                trust: match config.upstream_tls {
                    crate::UpstreamTls::Insecure => crate::tls::TrustModeKey::Insecure,
                    _ => crate::tls::TrustModeKey::System,
                },
                alpn: crate::tls::ClientAlpnMode::from_http_version(tls_alpn_version),
                options: config.upstream_tls_options,
                client_cert_sha256: None,
                store_server_cert_chain: false,
            },
        )
    } else {
        config.upstream_tls.config_and_key(
            tls_alpn_version,
            &config.upstream_tls_options,
            client_identity,
            config.on_server_cert_observed.is_some(),
        )?
    };
    let proxy_tls_and_key = effective_route
        .https_proxy_tls_config_and_key(&config.client_certificates, &config.upstream_tls_options)?;
    let cert_observer = if is_secure_scheme {
        config.on_server_cert_observed.clone().and_then(|callback| {
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
    let timing = Arc::new(Mutex::new(ResponseTiming::default()));

    let version_allows_reuse = match config.http_version {
        Some(HttpVersion::Http2) => config.http2_reuse,
        Some(_) => config.http1_keep_alive,
        None => config.http1_keep_alive || config.http2_reuse,
    };
    let use_pool = config.use_connection_pool
        && version_allows_reuse
        && raw_h1.is_none()
        && cookie_fields.is_none()
        && !request.headers().contains_key("upgrade");

    let response = if use_pool {
        if let Some((host, port, callback)) = cert_observer {
            request.extensions().insert(PooledCertObserverExt {
                host,
                port,
                callback,
            });
        }
        request.extensions().insert(PooledTimingExt {
            timing: timing.clone(),
            is_direct,
        });
        let (proxy_tls, route_kind) = match proxy_tls_and_key {
            Some((proxy_tls, proxy_key)) => (Some(proxy_tls), PooledRouteKind::HttpsProxy(proxy_key)),
            None if is_direct => (None, PooledRouteKind::Direct),
            None => (None, PooledRouteKind::PlainProxy),
        };
        let default_http_version_key = if force_http10 {
            0
        } else if allow_http2_alpn_override {
            2
        } else {
            1
        };
        let pool_key = PooledClientKey {
            is_plain_http: !is_secure_scheme,
            destination_tls: destination_tls_key,
            route_kind,
            default_http_version: default_http_version_key,
            force_http10,
            allow_http2_alpn_override,
            http1_keep_alive: config.http1_keep_alive,
            http2_reuse: config.http2_reuse,
            hostname_overrides: config.hostname_overrides.to_vec(),
            connection_pool_group_id: config.connection_pool_group_id,
        };
        let client = get_or_build_pooled_client(
            pool_key,
            tls,
            proxy_tls,
            is_direct,
            default_http_version,
            force_http10,
            allow_http2_alpn_override,
            config.hostname_overrides.clone(),
        )?;
        if let Some(barrier) = &config.single_packet_barrier {
            barrier.wait().await;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            result = Box::pin(client.serve(request)) => result.map_err(ProxyError::network)?,
        }
    } else if let Some((proxy_tls, _)) = proxy_tls_and_key {
        let client = EasyHttpWebClient::connector_builder()
            .with_default_transport_connector()
            .map_connector(|inner| CancellableConnector {
                inner,
                cancel,
                timing: timing.clone(),
            })
            .with_default_dns_connector()
            .with_tls_proxy_support_using_rustls_config(proxy_tls)
            .with_custom_proxy_connector(MioProxyConnectorLayer {
                overrides: config.hostname_overrides.clone(),
                timing: timing.clone(),
            })
            .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
            .map_connector(|inner| RawH1Connector {
                inner,
                raw_h1,
                cookie_fields,
                force_http10,
                allow_http2_alpn_override,
                cert_observer,
                single_packet_barrier: config.single_packet_barrier.clone(),
            })
            .with_default_http_connector::<Body>(exec)
            .without_connection_pool()
            .build_client()
            .with_jit_layer(SetProxyAuthHttpHeaderLayer::new());
        Box::pin(client.serve(request))
            .await
            .map_err(ProxyError::network)?
    } else {
        let client = EasyHttpWebClient::connector_builder()
            .with_default_transport_connector()
            .map_connector(|inner| CancellableConnector {
                inner,
                cancel,
                timing: timing.clone(),
            })
            .with_default_dns_connector()
            .without_tls_proxy_support()
            .with_custom_proxy_connector(MioProxyConnectorLayer {
                overrides: config.hostname_overrides.clone(),
                timing: timing.clone(),
            })
            .with_tls_support_using_rustls_and_default_http_version(tls, default_http_version)
            .map_connector(|inner| RawH1Connector {
                inner,
                raw_h1,
                cookie_fields,
                force_http10,
                allow_http2_alpn_override,
                cert_observer,
                single_packet_barrier: config.single_packet_barrier.clone(),
            })
            .with_default_http_connector::<Body>(exec)
            .without_connection_pool()
            .build_client()
            .with_jit_layer(SetProxyAuthHttpHeaderLayer::new());
        Box::pin(client.serve(request))
            .await
            .map_err(ProxyError::network)?
    };
    timing
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .lock();
    Ok((response, timing))
}
