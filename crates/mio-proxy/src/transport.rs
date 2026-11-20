use rama::{
    Layer,
    Service,
    dns::client::{
        GlobalDnsResolver,
        resolver::{BoxDnsAddressResolver, DnsAddressResolver, DnsAddresssResolverOverwrite},
    },
    error::{BoxError, BoxErrorExt as _},
    extensions::{Extensions, ExtensionsRef},
    http::client::{
        MaybeProxiedConnection, ProxyConnector, ProxyConnectorLayer,
        proxy::layer::HttpProxyConnectorLayer,
    },
    io::{GracefulIo, Io},
    net::{
        AuthorityInputExt, ConnectorTargetInputExt, Protocol, ProtocolInputExt,
        address::{Domain, DomainTrie, Host, HostWithPort},
        client::{
            ConnectionError, ConnectionErrorKind, ConnectorService, ConnectorTarget,
            EstablishedClientConnection, ProxyRoute,
        },
        stream::SocketInfo,
        user::ProxyCredential,
    },
    proxy::socks5::{Socks5Client, Socks5ProxyConnectorLayer},
};
use std::{
    io::{self, IoSlice},
    net::IpAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Instant,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use crate::ProxyError;

pub fn build_dns_resolver(overrides: &[(String, IpAddr)]) -> Result<BoxDnsAddressResolver, ProxyError> {
    if overrides.is_empty() {
        return Ok(GlobalDnsResolver::new().into_box_dns_address_resolver());
    }
    let mut trie = DomainTrie::<IpAddr>::new();
    for (hostname, ip) in overrides {
        let clean = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
        let domain: Domain = clean
            .parse()
            .map_err(|_| ProxyError::invalid(format!("主机名解析覆盖域名无效：{hostname}")))?;
        trie.insert_domain(domain, *ip);
    }
    Ok((trie, GlobalDnsResolver::new()).into_box_dns_address_resolver())
}

pub(crate) fn apply_hostname_overrides(
    extensions: &Extensions,
    overrides: &[(String, IpAddr)],
) -> Result<(), ProxyError> {
    if overrides.is_empty() {
        return Ok(());
    }
    let mut trie = DomainTrie::<IpAddr>::new();
    for (hostname, ip) in overrides {
        let clean = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
        let domain: Domain = clean
            .parse()
            .map_err(|_| ProxyError::invalid(format!("主机名解析覆盖域名无效：{hostname}")))?;
        trie.insert_domain(domain, *ip);
    }
    extensions.insert(DnsAddresssResolverOverwrite::new(trie));
    Ok(())
}

pub(crate) fn resolve_hostname_override(
    overrides: &[(String, IpAddr)],
    host: &str,
) -> Option<IpAddr> {
    let clean = host.trim().trim_matches(['[', ']']).trim_end_matches('.');
    overrides
        .iter()
        .find(|(pattern, _)| pattern.trim().trim_end_matches('.').eq_ignore_ascii_case(clean))
        .map(|(_, ip)| *ip)
}

#[derive(Clone, Default)]
pub(crate) struct MioProxyConnectorLayer {
    pub overrides: Arc<[(String, IpAddr)]>,
    pub timing: Arc<Mutex<ResponseTiming>>,
}

#[derive(Clone)]
pub(crate) struct MioProxyConnector<S> {
    inner: S,
    fallback: ProxyConnector<S>,
    overrides: Arc<[(String, IpAddr)]>,
    timing: Arc<Mutex<ResponseTiming>>,
}

impl<S: Clone> Layer<S> for MioProxyConnectorLayer {
    type Service = MioProxyConnector<S>;

    fn layer(&self, inner: S) -> Self::Service {
        let fallback = ProxyConnectorLayer::optional(
            Socks5ProxyConnectorLayer::required(),
            HttpProxyConnectorLayer::required().with_tls_proxy_support(true),
        )
        .into_layer(inner.clone());
        MioProxyConnector {
            inner,
            fallback,
            overrides: self.overrides.clone(),
            timing: self.timing.clone(),
        }
    }
}

impl<S, Input> Service<Input> for MioProxyConnector<S>
where
    S: ConnectorService<Input, Connection: Io + Unpin + ExtensionsRef> + Clone,
    Input: ConnectorTargetInputExt + AuthorityInputExt + ProtocolInputExt + ExtensionsRef + Send + 'static,
{
    type Output = EstablishedClientConnection<MaybeProxiedConnection<S::Connection>, Input>;
    type Error = ConnectionError;

    async fn serve(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let is_socks_local_dns = matches!(
            input.extensions().get_ref::<ProxyRoute>(),
            Some(ProxyRoute::Proxy(addr)) if addr.protocol == Some(Protocol::SOCKS5)
        );
        if !is_socks_local_dns {
            return self.fallback.serve(input).await;
        }
        let Some(ProxyRoute::Proxy(proxy_info)) = input.extensions().get_ref::<ProxyRoute>().cloned() else {
            unreachable!()
        };
        let authority = input.authority().ok_or_else(|| {
            ConnectionError::local(
                BoxError::from_static_str("socks5 local-dns connector: authority missing"),
                ConnectionErrorKind::InvalidInput,
            )
        })?;
        let Some(mut connect_authority) = authority.into_host_with_port(input.protocol_default_port()) else {
            return Err(ConnectionError::local(
                BoxError::from_static_str("socks5 local-dns connector: port missing"),
                ConnectionErrorKind::InvalidInput,
            ));
        };
        if let Host::Name(domain) = &connect_authority.host {
            let domain_str = domain.as_str();
            let resolved_ip = if let Some(ip) = resolve_hostname_override(&self.overrides, domain_str) {
                ip
            } else {
                let mut addrs = tokio::net::lookup_host((domain_str, connect_authority.port))
                    .await
                    .map_err(|err| ConnectionError::transport(err, ConnectionErrorKind::Unavailable))?;
                addrs
                    .next()
                    .map(|sa| sa.ip())
                    .ok_or_else(|| {
                        ConnectionError::transport(
                            BoxError::from_static_str(
                            "socks5 local-dns lookup returned no addresses",
                            ),
                            ConnectionErrorKind::Unavailable,
                        )
                    })?
            };
            connect_authority = HostWithPort::new(Host::Address(resolved_ip), connect_authority.port);
        }
        if let Host::Address(ip) = connect_authority.host {
            self.timing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_peer_ip(ip);
        }
        input
            .extensions()
            .insert(ConnectorTarget(proxy_info.address.clone()));
        let EstablishedClientConnection { input, mut conn } = self
            .inner
            .connect(input)
            .await
            .map_err(|err| err.context("establish connection to socks5 proxy"))?;
        let mut client = Socks5Client::new();
        if let Some(ProxyCredential::Basic(basic)) = &proxy_info.credential {
            client.set_auth(basic.clone());
        }
        client
            .handshake_connect(&mut conn, &connect_authority)
            .await
            .map_err(|err| ConnectionError::transport(err, ConnectionErrorKind::Protocol))?;
        Ok(EstablishedClientConnection {
            input,
            conn: MaybeProxiedConnection::socks(conn),
        })
    }
}

/// Shared observation window for one send attempt's first response byte.
///
/// The connection wrapper records the first read after the most recent write,
/// and the caller locks the value once the exchange returns.
#[derive(Debug, Default)]
pub(crate) struct ResponseTiming {
    first_response_byte: Option<Instant>,
    peer_ip: Option<IpAddr>,
    locked: bool,
}

impl ResponseTiming {
    fn record_read(&mut self) {
        if !self.locked && self.first_response_byte.is_none() {
            self.first_response_byte = Some(Instant::now());
        }
    }

    fn reset_read(&mut self) {
        if !self.locked {
            self.first_response_byte = None;
        }
    }

    pub(crate) fn set_peer_ip(&mut self, ip: IpAddr) {
        let normalized = match ip {
            IpAddr::V4(v4) => IpAddr::V4(v4),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(mapped) => IpAddr::V4(mapped),
                None => IpAddr::V6(v6),
            },
        };
        self.peer_ip = Some(normalized);
    }

    pub(crate) fn peer_ip(&self) -> Option<IpAddr> {
        self.peer_ip
    }

    pub(crate) fn lock(&mut self) {
        self.locked = true;
    }

    pub(crate) fn first_response_byte(&self) -> Option<Instant> {
        self.first_response_byte
    }
}

pub(crate) struct TimingIo<S> {
    inner: S,
    timing: Arc<Mutex<ResponseTiming>>,
}

impl<S> TimingIo<S> {
    fn new(inner: S, timing: Arc<Mutex<ResponseTiming>>) -> Self {
        Self { inner, timing }
    }

    fn project_inner(self: Pin<&mut Self>) -> Pin<&mut S> {
        // SAFETY: `inner` is never moved out of `self`, so projecting into it
        // preserves the pinning guarantee of the wrapped connection.
        unsafe { self.map_unchecked_mut(|this| &mut this.inner) }
    }
}

impl<S: ExtensionsRef> ExtensionsRef for TimingIo<S> {
    fn extensions(&self) -> &Extensions {
        self.inner.extensions()
    }
}

impl<S: AsyncRead> AsyncRead for TimingIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let timing = Arc::clone(&self.timing);
        let result = self.project_inner().poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result
            && buf.filled().len() > before
        {
            timing.lock().unwrap().record_read();
        }
        result
    }
}

impl<S: AsyncWrite> AsyncWrite for TimingIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let timing = Arc::clone(&self.timing);
        let result = self.project_inner().poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &result
            && *n > 0
        {
            timing.lock().unwrap().reset_read();
        }
        result
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project_inner().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project_inner().poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let timing = Arc::clone(&self.timing);
        let result = self.project_inner().poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(n)) = &result
            && *n > 0
        {
            timing.lock().unwrap().reset_read();
        }
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// Owns cancellation at the socket, including drivers spawned inside the SDK.
#[derive(Clone)]
pub(crate) struct CancellableConnector<S> {
    pub inner: S,
    pub cancel: CancellationToken,
    pub timing: Arc<Mutex<ResponseTiming>>,
}

impl<S, Input> Service<Input> for CancellableConnector<S>
where
    S: ConnectorService<Input>,
    S::Connection: Io + ExtensionsRef,
    Input: ExtensionsRef + Send + 'static,
{
    type Output = EstablishedClientConnection<
        GracefulIo<Pin<Box<WaitForCancellationFutureOwned>>, TimingIo<S::Connection>>,
        Input,
    >;
    type Error = ConnectionError;

    async fn serve(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let EstablishedClientConnection { input, conn } = self.inner.connect(input).await?;
        let is_direct = matches!(
            input.extensions().get_ref::<ProxyRoute>(),
            None | Some(ProxyRoute::Direct)
        );
        if is_direct && let Some(socket_info) = conn.extensions().get_ref::<SocketInfo>() {
            self.timing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_peer_ip(socket_info.peer_addr().ip_addr);
        }
        Ok(EstablishedClientConnection {
            input,
            conn: GracefulIo::new(
                Box::pin(self.cancel.clone().cancelled_owned()),
                TimingIo::new(conn, self.timing.clone()),
            ),
        })
    }
}
