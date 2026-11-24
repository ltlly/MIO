use rama::{
    Layer, Service,
    dns::client::{
        GlobalDnsResolver,
        resolver::{BoxDnsAddressResolver, DnsAddressResolver, DnsAddresssResolverOverwrite},
    },
    error::{BoxError, BoxErrorExt as _},
    extensions::{Extension, Extensions, ExtensionsRef},
    http::client::{
        EasyHttpWebClient, MaybeProxiedConnection, ProxyConnector, ProxyConnectorLayer,
        proxy::layer::HttpProxyConnectorLayer,
    },
    io::{GracefulIo, Io},
    net::{
        AuthorityInputExt, ConnectorTargetInputExt, Protocol, ProtocolInputExt,
        address::{Domain, DomainTrie, Host, HostWithPort},
        client::{
            ConnectRequest, ConnectionError, ConnectionErrorKind, ConnectorService,
            ConnectorTarget, EstablishedClientConnection, ProxyRoute,
        },
        stream::SocketInfo,
        user::ProxyCredential,
    },
    proxy::socks5::{Socks5Client, Socks5ProxyConnectorLayer},
    tls::client::TlsClientConfig,
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

pub(crate) fn build_hostname_override_trie(
    overrides: &[(String, IpAddr)],
) -> Result<DomainTrie<IpAddr>, ProxyError> {
    let mut trie = DomainTrie::<IpAddr>::new();
    for (hostname, ip) in overrides {
        let clean = crate::route::normalize_host(hostname);
        let domain: Domain = clean
            .parse()
            .map_err(|_| ProxyError::invalid(format!("主机名解析覆盖域名无效：{hostname}")))?;
        trie.insert_domain(domain, *ip);
    }
    Ok(trie)
}

pub fn build_dns_resolver(
    overrides: &[(String, IpAddr)],
) -> Result<BoxDnsAddressResolver, ProxyError> {
    if overrides.is_empty() {
        return Ok(GlobalDnsResolver::new().into_box_dns_address_resolver());
    }
    let trie = build_hostname_override_trie(overrides)?;
    Ok((trie, GlobalDnsResolver::new()).into_box_dns_address_resolver())
}

pub(crate) fn apply_hostname_overrides(
    extensions: &Extensions,
    overrides: &[(String, IpAddr)],
) -> Result<(), ProxyError> {
    if overrides.is_empty() {
        return Ok(());
    }
    let trie = build_hostname_override_trie(overrides)?;
    extensions.insert(DnsAddresssResolverOverwrite::new(trie));
    Ok(())
}

pub(crate) fn resolve_hostname_override(
    overrides: &[(String, IpAddr)],
    host: &str,
) -> Option<IpAddr> {
    if overrides.is_empty() {
        return None;
    }
    let clean = crate::route::normalize_host(host);
    let domain: Domain = clean.parse().ok()?;
    let trie = build_hostname_override_trie(overrides).ok()?;
    trie.get_value(&domain).copied()
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedPeerIp(pub IpAddr);

impl Extension for ResolvedPeerIp {}

#[derive(Clone)]
pub(crate) struct PooledTimingSlotExt(pub TimingSlot);

impl std::fmt::Debug for PooledTimingSlotExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledTimingSlotExt").finish_non_exhaustive()
    }
}

impl Extension for PooledTimingSlotExt {}

#[derive(Clone)]
pub(crate) struct PooledRequestCancelExt(pub CancellationToken);

impl std::fmt::Debug for PooledRequestCancelExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledRequestCancelExt").finish_non_exhaustive()
    }
}

impl Extension for PooledRequestCancelExt {}

#[derive(Clone, Default)]
pub(crate) struct MioProxyConnectorLayer {
    overrides_trie: Option<Arc<DomainTrie<IpAddr>>>,
    timing: Option<Arc<Mutex<ResponseTiming>>>,
}

impl MioProxyConnectorLayer {
    pub(crate) fn new(
        overrides: &[(String, IpAddr)],
        timing: Option<Arc<Mutex<ResponseTiming>>>,
    ) -> Result<Self, ProxyError> {
        let overrides_trie = if overrides.is_empty() {
            None
        } else {
            Some(Arc::new(build_hostname_override_trie(overrides)?))
        };
        Ok(Self {
            overrides_trie,
            timing,
        })
    }
}

#[derive(Clone)]
pub(crate) struct MioProxyConnector<S> {
    inner: S,
    fallback: ProxyConnector<S>,
    overrides_trie: Option<Arc<DomainTrie<IpAddr>>>,
    timing: Option<Arc<Mutex<ResponseTiming>>>,
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
            overrides_trie: self.overrides_trie.clone(),
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
        let proxy_info = match input.extensions().get_ref::<ProxyRoute>().cloned() {
            Some(ProxyRoute::Proxy(addr)) if addr.protocol == Some(Protocol::SOCKS5) => addr,
            _ => return self.fallback.serve(input).await,
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
        let req_cancel = input
            .extensions()
            .get_ref::<PooledRequestCancelExt>()
            .map(|ext| ext.0.clone());
        if let Host::Name(domain) = &connect_authority.host {
            let domain_str = domain.as_str();
            let normalized_domain: Option<Domain> =
                crate::route::normalize_host(domain_str).parse().ok();
            let resolved_ip = if let Some(ip) = self.overrides_trie.as_ref().and_then(|trie| {
                normalized_domain
                    .as_ref()
                    .and_then(|norm| trie.get_value(norm).copied())
                    .or_else(|| trie.get_value(domain).copied())
            }) {
                ip
            } else {
                let lookup = tokio::net::lookup_host((domain_str, connect_authority.port));
                let mut addrs = tokio::select! {
                    biased;
                    _ = async {
                        match &req_cancel {
                            Some(token) => token.cancelled().await,
                            None => std::future::pending::<()>().await,
                        }
                    } => {
                        return Err(ConnectionError::transport(
                            ProxyError::Cancelled,
                            ConnectionErrorKind::Unavailable,
                        ));
                    }
                    res = lookup => res.map_err(|err| ConnectionError::transport(err, ConnectionErrorKind::Unavailable))?,
                };
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
        let resolved_ip = match connect_authority.host {
            Host::Address(ip) => Some(ip),
            _ => None,
        };
        if let (Some(ip), Some(timing)) = (resolved_ip, &self.timing) {
            crate::lock_unpoison(timing).set_peer_ip(ip);
        }
        input
            .extensions()
            .insert(ConnectorTarget(proxy_info.address.clone()));
        let EstablishedClientConnection { input, mut conn } = self
            .inner
            .connect(input)
            .await
            .map_err(|err| err.context("establish connection to socks5 proxy"))?;
        if let Some(ip) = resolved_ip {
            conn.extensions().insert(ResolvedPeerIp(ip));
        }
        let mut client = Socks5Client::new();
        if let Some(ProxyCredential::Basic(basic)) = &proxy_info.credential {
            client.set_auth(basic.clone());
        }
        tokio::select! {
            biased;
            _ = async {
                match &req_cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                return Err(ConnectionError::transport(
                    ProxyError::Cancelled,
                    ConnectionErrorKind::Unavailable,
                ));
            }
            res = client.handshake_connect(&mut conn, &connect_authority) => {
                res.map_err(|err| ConnectionError::transport(err, ConnectionErrorKind::Protocol))?;
            }
        }
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

#[derive(Clone)]
struct AttachedTiming {
    timing: Arc<Mutex<ResponseTiming>>,
    armed: bool,
}

#[derive(Clone, Default)]
pub(crate) struct TimingSlot {
    active: Arc<Mutex<Vec<AttachedTiming>>>,
}

pub(crate) struct TimingAttachGuard {
    slot: TimingSlot,
    timing: Arc<Mutex<ResponseTiming>>,
}

impl Drop for TimingAttachGuard {
    fn drop(&mut self) {
        let mut active = crate::lock_unpoison(&self.slot.active);
        if let Some(pos) = active
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.timing, &self.timing))
        {
            active.remove(pos);
        }
    }
}

impl TimingSlot {
    pub(crate) fn attach(&self, timing: Arc<Mutex<ResponseTiming>>) -> TimingAttachGuard {
        crate::lock_unpoison(&self.active).push(AttachedTiming {
            timing: timing.clone(),
            armed: false,
        });
        TimingAttachGuard {
            slot: self.clone(),
            timing,
        }
    }

    fn on_write(&self) {
        let mut active = crate::lock_unpoison(&self.active);
        for entry in active.iter_mut() {
            if !entry.armed {
                entry.armed = true;
                crate::lock_unpoison(&entry.timing).reset_read();
            }
        }
    }

    fn on_read(&self) {
        let active = crate::lock_unpoison(&self.active);
        for entry in active.iter() {
            if entry.armed {
                crate::lock_unpoison(&entry.timing).record_read();
            }
        }
    }
}

enum TimingTarget {
    Direct(Arc<Mutex<ResponseTiming>>),
    Pooled(TimingSlot),
}

impl TimingTarget {
    fn on_read(&self) {
        match self {
            Self::Direct(timing) => crate::lock_unpoison(timing).record_read(),
            Self::Pooled(slot) => slot.on_read(),
        }
    }

    fn on_write(&self) {
        match self {
            Self::Direct(timing) => crate::lock_unpoison(timing).reset_read(),
            Self::Pooled(slot) => slot.on_write(),
        }
    }
}

pub(crate) struct TimingIo<S> {
    inner: S,
    target: TimingTarget,
}

impl<S> TimingIo<S> {
    fn direct(inner: S, timing: Arc<Mutex<ResponseTiming>>) -> Self {
        Self {
            inner,
            target: TimingTarget::Direct(timing),
        }
    }

    fn pooled(inner: S, slot: TimingSlot) -> Self {
        Self {
            inner,
            target: TimingTarget::Pooled(slot),
        }
    }

    fn project_inner(self: Pin<&mut Self>) -> Pin<&mut S> {
        self.project_parts().0
    }

    fn project_parts(self: Pin<&mut Self>) -> (Pin<&mut S>, &TimingTarget) {
        // SAFETY: `inner` is never moved out of `self`, so re-pinning `&mut this.inner`
        // preserves the structural pinning invariant while allowing shared access to `target`.
        let this = unsafe { self.get_unchecked_mut() };
        (unsafe { Pin::new_unchecked(&mut this.inner) }, &this.target)
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
        let (inner, target) = self.project_parts();
        let result = inner.poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result
            && buf.filled().len() > before
        {
            target.on_read();
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
        let (inner, target) = self.project_parts();
        let result = inner.poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &result
            && *n > 0
        {
            target.on_write();
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
        let (inner, target) = self.project_parts();
        let result = inner.poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(n)) = &result
            && *n > 0
        {
            target.on_write();
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
    pub timing: Option<Arc<Mutex<ResponseTiming>>>,
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
        let req_cancel = input
            .extensions()
            .get_ref::<PooledRequestCancelExt>()
            .map(|ext| ext.0.clone());
        let EstablishedClientConnection { input, conn } = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                return Err(ConnectionError::transport(
                    ProxyError::Cancelled,
                    ConnectionErrorKind::Unavailable,
                ));
            }
            _ = async {
                match &req_cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                return Err(ConnectionError::transport(
                    ProxyError::Cancelled,
                    ConnectionErrorKind::Unavailable,
                ));
            }
            res = self.inner.connect(input) => res?,
        };
        let is_direct = matches!(
            input.extensions().get_ref::<ProxyRoute>(),
            None | Some(ProxyRoute::Direct)
        );
        let peer_ip = is_direct
            .then(|| conn.extensions().get_ref::<SocketInfo>())
            .flatten()
            .map(|info| info.peer_addr().ip_addr);
        let timed_conn = match &self.timing {
            Some(timing) => {
                if let Some(ip) = peer_ip {
                    crate::lock_unpoison(timing).set_peer_ip(ip);
                }
                TimingIo::direct(conn, timing.clone())
            }
            None => {
                let slot = TimingSlot::default();
                conn.extensions().insert(PooledTimingSlotExt(slot.clone()));
                TimingIo::pooled(conn, slot)
            }
        };
        Ok(EstablishedClientConnection {
            input,
            conn: GracefulIo::new(Box::pin(self.cancel.clone().cancelled_owned()), timed_conn),
        })
    }
}

pub(crate) fn build_passthrough_connector(
    proxy_tls: TlsClientConfig,
    overrides: &[(String, IpAddr)],
    cancel: CancellationToken,
) -> Result<
    impl ConnectorService<ConnectRequest, Connection: Io + Unpin + ExtensionsRef>,
    ProxyError,
> {
    let proxy_layer = MioProxyConnectorLayer::new(overrides, None)?;
    Ok(EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(move |inner| CancellableConnector {
            inner,
            cancel: cancel.clone(),
            timing: None,
        })
        .with_default_dns_connector()
        .with_tls_proxy_support_using_rustls_config(proxy_tls)
        .with_custom_proxy_connector(proxy_layer)
        .build_connector())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_hostname_override_matches_exact_and_wildcard_domains_via_trie() {
        let exact_ip = IpAddr::from([127, 0, 0, 10]);
        let wild_ip = IpAddr::from([127, 0, 0, 20]);
        let overrides = [
            (String::from("exact.lab.invalid"), exact_ip),
            (String::from("*.wild.lab.invalid"), wild_ip),
        ];

        assert_eq!(
            resolve_hostname_override(&overrides, "exact.lab.invalid"),
            Some(exact_ip)
        );
        assert_eq!(
            resolve_hostname_override(&overrides, "EXACT.LAB.INVALID."),
            Some(exact_ip)
        );
        assert_eq!(
            resolve_hostname_override(&overrides, "api.wild.lab.invalid"),
            Some(wild_ip)
        );
        assert_eq!(
            resolve_hostname_override(&overrides, "nested.api.wild.lab.invalid"),
            Some(wild_ip)
        );
        assert_eq!(resolve_hostname_override(&overrides, "other.lab.invalid"), None);
    }
}
