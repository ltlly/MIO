use std::sync::atomic::{AtomicBool, Ordering};

use mio_plugin_api::{Header, HttpVersion};
use rama::extensions::ExtensionsRef;
use rama::http::HeaderValue;
use rama::io::Io;
use rama::tls::rustls::dep::rustls;
use tokio::io::AsyncReadExt;

use super::*;

impl State {
    pub(super) async fn serve_http_connection<IO, S>(&self, io: IO, service: S)
    where
        IO: Io + ExtensionsRef,
        S: Service<Request, Output = Response, Error = Infallible> + Clone,
    {
        if self.config.http.enable_http2 {
            let mut server = HttpServer::auto(self.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(self.config.http.http1_keep_alive);
            server.h2_mut().set_max_concurrent_streams(
                self.config.limits.max_connections.min(u32::MAX as usize) as u32,
            );
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {},
                _ = server.serve(io, service) => {},
            }
        } else {
            let mut server = HttpServer::new_http1(self.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(self.config.http.http1_keep_alive);
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {},
                _ = server.serve(io, service) => {},
            }
        }
    }

    pub(super) async fn serve_tls_http_connection<IO, S>(
        &self,
        io: IO,
        tls_config: TlsServerConfig,
        service: S,
        store_client_hello: bool,
    ) -> Result<(), rama::error::BoxError>
    where
        IO: Io + Unpin + ExtensionsRef,
        S: Service<Request, Output = Response, Error = Infallible> + Clone,
    {
        if self.config.http.enable_http2 {
            let mut server = HttpServer::auto(self.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(self.config.http.http1_keep_alive);
            server.h2_mut().set_max_concurrent_streams(
                self.config.limits.max_connections.min(u32::MAX as usize) as u32,
            );
            TlsAcceptorService::new(tls_config, server.service(service), store_client_hello)
                .serve(io)
                .await
        } else {
            let mut server = HttpServer::new_http1(self.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(self.config.http.http1_keep_alive);
            TlsAcceptorService::new(tls_config, server.service(service), store_client_hello)
                .serve(io)
                .await
        }
    }
}

impl ActiveFlow {
    pub(super) async fn finish(&mut self, result: Result<(), &ProxyError>) {
        let record = self.record.as_mut().expect("active flow owns its record");
        record.summary.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        record.summary.revision += 1;
        record.summary.state = match result {
            Ok(()) => FlowState::Complete,
            Err(ProxyError::Cancelled) => FlowState::Cancelled,
            Err(ProxyError::Dropped) => FlowState::Dropped,
            Err(error) => FlowState::Failed(error.to_string()),
        };
        self.state
            .terminal(
                &mut self.record,
                &mut self.response_stream,
                &self.capture_metrics,
                self.permit.clone(),
            )
            .await;
    }
}

impl Drop for ActiveFlow {
    fn drop(&mut self) {
        let Some(mut record) = self.record.take() else {
            return;
        };
        // HTTP/2 resets drop the handler future before its normal terminal event.
        record.summary.state = FlowState::Cancelled;
        record.summary.revision += 1;
        record.summary.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let state = self.state.clone();
        let permit = self.permit.clone();
        let response_stream = self.response_stream.take();
        let capture_metrics = self.capture_metrics.clone();
        if state.cancel.is_cancelled() || state.events.is_closed() {
            if state.events.is_closed() {
                state.cancel.cancel();
            }
            if let Ok(slot) = state.events.try_reserve() {
                slot.send(ProxyEvent::Captured {
                    project: state.project,
                    record,
                    response_stream,
                    capture_metrics,
                    listener_port: (state.listener_port != 0).then_some(state.listener_port),
                });
            } else {
                state.retain_terminal(
                    CapturedFlow {
                        record,
                        response_stream,
                        capture_metrics,
                        listener_port: (state.listener_port != 0).then_some(state.listener_port),
                    },
                    permit,
                );
            }
            return;
        }
        self.state.tasks.spawn(async move {
            let mut record = Some(record);
            let mut response_stream = response_stream;
            state
                .terminal(&mut record, &mut response_stream, &capture_metrics, permit)
                .await;
        });
    }
}

#[cfg(target_os = "macos")]
fn try_bind_without_reuseaddr(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

    let domain = match addr {
        SocketAddr::V4(_) => libc::AF_INET,
        SocketAddr::V6(_) => libc::AF_INET6,
    };
    let raw_fd = unsafe { libc::socket(domain, libc::SOCK_STREAM, 0) };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    match addr {
        SocketAddr::V4(v4) => {
            let sin = libc::sockaddr_in {
                sin_len: std::mem::size_of::<libc::sockaddr_in>() as u8,
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: v4.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.ip().octets()),
                },
                sin_zero: [0; 8],
            };
            if unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    (&raw const sin).cast::<libc::sockaddr>(),
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        SocketAddr::V6(v6) => {
            let one: libc::c_int = 1;
            if unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::IPPROTO_IPV6,
                    libc::IPV6_V6ONLY,
                    (&raw const one).cast::<libc::c_void>(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let sin6 = libc::sockaddr_in6 {
                sin6_len: std::mem::size_of::<libc::sockaddr_in6>() as u8,
                sin6_family: libc::AF_INET6 as libc::sa_family_t,
                sin6_port: v6.port().to_be(),
                sin6_flowinfo: v6.flowinfo(),
                sin6_addr: libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                },
                sin6_scope_id: v6.scope_id(),
            };
            if unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    (&raw const sin6).cast::<libc::sockaddr>(),
                    std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    if unsafe { libc::listen(fd.as_raw_fd(), 1024) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let std_listener = std::net::TcpListener::from(fd);
    std_listener.set_nonblocking(true)?;
    Ok(std_listener)
}

async fn bind_proxy_listener(addr: SocketAddr) -> Result<TcpListener, ProxyError> {
    if addr.port() == 0 {
        return TcpListener::bind(addr).await.map_err(ProxyError::network);
    }
    #[cfg(target_os = "macos")]
    {
        match try_bind_without_reuseaddr(addr) {
            Ok(std_listener) => TcpListener::from_std(std_listener).map_err(ProxyError::network),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                let probe_addr = if addr.ip().is_unspecified() {
                    match addr {
                        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::LOCALHOST, addr.port())),
                        SocketAddr::V6(_) => {
                            SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, addr.port()))
                        }
                    }
                } else {
                    addr
                };
                match tokio::time::timeout(
                    Duration::from_millis(50),
                    tokio::net::TcpStream::connect(probe_addr),
                )
                .await
                {
                    Ok(Err(connect_err))
                        if connect_err.kind() == std::io::ErrorKind::ConnectionRefused =>
                    {
                        TcpListener::bind(addr).await.map_err(ProxyError::network)
                    }
                    _ => Err(ProxyError::network(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        format!("地址 {addr} 或同端口通配监听已被占用"),
                    ))),
                }
            }
            Err(err) => Err(ProxyError::network(err)),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        TcpListener::bind(addr).await.map_err(ProxyError::network)
    }
}

pub async fn start(
    config: ProxyConfig,
    project: ProjectKey,
    events: mpsc::Sender<ProxyEvent>,
) -> Result<ProxyHandle, ProxyError> {
    if config.limits.max_connections == 0
        || config.limits.max_body_bytes == 0
        || config.limits.request_timeout.is_zero()
        || config.limits.intercept_timeout.is_zero()
    {
        return Err(ProxyError::invalid("连接/正文预算及请求期限必须大于零"));
    }
    if config.network.redirect_to_port == Some(0) {
        return Err(ProxyError::invalid("重定向目标端口必须大于零"));
    }
    config.tls.cert_mode.validate()?;
    config
        .tls
        .upstream
        .trust
        .config_with_options(None, &config.tls.upstream.options)?;
    config.network.upstream_route.rama()?;
    config.network.upstream_route.tls_config_with_options(
        &config.tls.upstream.client_certificates,
        &config.tls.upstream.options,
    )?;
    let listener = bind_proxy_listener(config.network.listen).await?;
    let address = listener.local_addr().map_err(ProxyError::network)?;
    let cancel = CancellationToken::new();
    let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
    let exec = Executor::graceful(shutdown.guard());
    let tasks = TaskTracker::new();
    let (static_config, dynamic_init, shared) = config.into_parts();
    let (dynamic_tx, dynamic_rx) = watch::channel(dynamic_init);
    let stop_timeout = static_config.limits.stop_timeout;
    let connections = shared
        .connections
        .unwrap_or_else(|| Arc::new(Semaphore::new(static_config.limits.max_connections)));
    let requests = shared
        .requests
        .unwrap_or_else(|| Arc::new(Semaphore::new(static_config.limits.max_connections)));
    let terminal_records = TerminalRecords::default();
    let injectors = shared.injectors.unwrap_or_default();
    let state = Arc::new(State {
        project,
        listener_port: address.port(),
        requests,
        config: static_config,
        events,
        cancel: cancel.clone(),
        dynamic: dynamic_rx,
        tasks: tasks.clone(),
        exec,
        terminal_records: terminal_records.clone(),
        injectors: injectors.clone(),
    });
    let runner = tokio::spawn(async move {
        let result = async {
            loop {
                let permit = tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => break,
                    _ = state.events.closed() => break,
                    permit = connections.clone().acquire_owned() => permit.map_err(ProxyError::network)?,
                };
                let accepted = tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => break,
                    _ = state.events.closed() => break,
                    result = listener.accept() => result,
                };
                let (stream, _) = accepted.map_err(ProxyError::network)?;
                stream.set_nodelay(true).map_err(ProxyError::network)?;
                let state = state.clone();
                tasks.spawn(async move {
                    let permit = Arc::new(permit);
                    match socks5::peek_first_byte(
                        &stream,
                        state.config.limits.request_timeout,
                        &state.cancel,
                    )
                    .await
                    {
                        Ok(Some(socks5::SOCKS5_LEAD_BYTE)) => {
                            socks5::serve_socks5(stream, state, permit).await;
                            return;
                        }
                        Ok(Some(socks5::TLS_CLIENT_HELLO_LEAD_BYTE))
                            if state.config.network.invisible_proxying =>
                        {
                            serve_invisible_tls(stream, state, permit).await;
                            return;
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => return,
                        Err(ProxyError::Cancelled) => return,
                        Err(error) => {
                            let host = crate::route::format_host_for_authority(
                                &state.config.network.listen.ip().to_string(),
                            );
                            let failed = HttpRequest {
                                method: "CONNECT".into(),
                                url: format!("proxy://{host}:{}", state.listener_port),
                                version: HttpVersion::Http11,
                                headers: Vec::new(),
                                body: Vec::new(),
                                trailers: Vec::new(),
                            };
                            state.failure(failed, &error).await;
                            return;
                        }
                    }
                    let io = GracefulIo::new(
                        Box::pin(state.cancel.clone().cancelled_owned()),
                        TcpStream::new(stream),
                    );
                    let service_state = state.clone();
                    let service = service_fn(move |request: Request| {
                        let state = service_state.clone();
                        let permit = permit.clone();
                        async move { Ok::<_, Infallible>(handle_entry(request, state, permit).await) }
                    });
                    state.serve_http_connection(io, service).await;
                });
            }
            Ok(())
        }
        .await;
        state.cancel.cancel();
        drop(listener);
        tasks.close();
        tasks.wait().await;
        drop(state);
        shutdown.shutdown().await;
        tasks.wait().await;
        result
    });
    Ok(ProxyHandle {
        address,
        cancel,
        dynamic: dynamic_tx,
        runner: Some(runner),
        stop_timeout,
        terminal_records,
        injectors,
    })
}

pub(super) fn parse_client_hello_sni(data: &[u8]) -> Option<String> {
    if data.len() < 9 || data[0] != 0x16 || data[1] != 0x03 {
        return None;
    }
    let record_len = u16::from_be_bytes([data[3], data[4]]) as usize;
    let record_end = data.len().min(5usize.saturating_add(record_len));
    let handshake = data.get(5..record_end)?;
    if handshake.len() < 4 || handshake[0] != 0x01 {
        return None;
    }
    let hello_len = ((handshake[1] as usize) << 16)
        | ((handshake[2] as usize) << 8)
        | (handshake[3] as usize);
    let hello_end = handshake.len().min(4usize.saturating_add(hello_len));
    let body = handshake.get(4..hello_end)?;
    if body.len() < 35 {
        return None;
    }
    let mut cursor = 34usize;
    let session_id_len = *body.get(cursor)? as usize;
    cursor = cursor.checked_add(1 + session_id_len)?;
    let cs_bytes = body.get(cursor..cursor.checked_add(2)?)?;
    let cipher_suites_len = u16::from_be_bytes([cs_bytes[0], cs_bytes[1]]) as usize;
    cursor = cursor.checked_add(2 + cipher_suites_len)?;
    let comp_len = *body.get(cursor)? as usize;
    cursor = cursor.checked_add(1 + comp_len)?;
    let ext_len_bytes = body.get(cursor..cursor.checked_add(2)?)?;
    let ext_len = u16::from_be_bytes([ext_len_bytes[0], ext_len_bytes[1]]) as usize;
    cursor = cursor.checked_add(2)?;
    let ext_end = body.len().min(cursor.saturating_add(ext_len));
    while cursor + 4 <= ext_end {
        let ext_type = u16::from_be_bytes([body[cursor], body[cursor + 1]]);
        let item_len = u16::from_be_bytes([body[cursor + 2], body[cursor + 3]]) as usize;
        cursor += 4;
        let Some(ext_payload) = body.get(cursor..cursor.checked_add(item_len)?) else {
            break;
        };
        if ext_type == 0x0000 {
            if ext_payload.len() < 2 {
                return None;
            }
            let list_len = u16::from_be_bytes([ext_payload[0], ext_payload[1]]) as usize;
            let list_end = ext_payload.len().min(2usize.saturating_add(list_len));
            let mut pos = 2usize;
            while pos + 3 <= list_end {
                let name_type = ext_payload[pos];
                let name_len = u16::from_be_bytes([ext_payload[pos + 1], ext_payload[pos + 2]]) as usize;
                pos += 3;
                let Some(name_bytes) = ext_payload.get(pos..pos.checked_add(name_len)?) else {
                    break;
                };
                if name_type == 0x00 {
                    return std::str::from_utf8(name_bytes)
                        .ok()
                        .map(crate::route::normalize_host)
                        .filter(|s| !s.is_empty());
                }
                pos += name_len;
            }
            return None;
        }
        cursor += item_len;
    }
    None
}

async fn serve_invisible_tls(
    stream: tokio::net::TcpStream,
    state: Arc<State>,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
    let _permit = permit;
    let fallback_host = state
        .config
        .network
        .redirect_to_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .or_else(|| match &state.config.tls.cert_mode {
            crate::ListenerCertMode::SpecificHostname(host) if !host.trim().is_empty() => {
                Some(host.trim())
            }
            _ => None,
        })
        .unwrap_or("localhost")
        .to_owned();
    let has_passthrough = state.dynamic.borrow().ssl_pass_through.is_some();
    if has_passthrough {
        let mut peek_buf = [0u8; 4096];
        let peeked = tokio::select! {
            biased;
            _ = state.cancel.cancelled() => return,
            res = tokio::time::timeout(state.config.limits.request_timeout, stream.peek(&mut peek_buf)) => res,
        };
        let peeked_len = match peeked {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => return,
            Ok(Err(err)) => {
                if is_benign_tunnel_disconnect(&err) {
                    return;
                }
                let failed = HttpRequest {
                    method: "CONNECT".into(),
                    url: format!(
                        "{fallback_host}:{}",
                        state.config.network.redirect_to_port.unwrap_or(443)
                    ),
                    version: HttpVersion::Http11,
                    headers: Vec::new(),
                    body: Vec::new(),
                    trailers: Vec::new(),
                };
                state.failure(failed, &ProxyError::network(err)).await;
                return;
            }
            Err(_) => {
                let failed = HttpRequest {
                    method: "CONNECT".into(),
                    url: format!(
                        "{fallback_host}:{}",
                        state.config.network.redirect_to_port.unwrap_or(443)
                    ),
                    version: HttpVersion::Http11,
                    headers: Vec::new(),
                    body: Vec::new(),
                    trailers: Vec::new(),
                };
                state.failure(failed, &ProxyError::Timeout).await;
                return;
            }
        };
        let sni = parse_client_hello_sni(&peek_buf[..peeked_len]);
        let candidate_host = sni.as_deref().unwrap_or(&fallback_host);
        let candidate_port = state.config.network.redirect_to_port.unwrap_or(443);
        if ssl_passthrough::should_pass_through(&state, candidate_host, candidate_port) {
            let (pt_host, pt_port) =
                state.effective_passthrough_target(candidate_host, candidate_port);
            let failed_request = HttpRequest {
                method: "CONNECT".into(),
                url: format!("{pt_host}:{pt_port}"),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            if let Err(error) =
                ssl_passthrough::relay_stream(stream, &pt_host, pt_port, &state).await
                && !matches!(error, ProxyError::Cancelled)
            {
                state.failure(failed_request, &error).await;
            }
            return;
        }
    }
    let observed_sni = Arc::new(std::sync::Mutex::new(None::<String>));
    let ca = state.dynamic.borrow().ca.clone();
    let tls_config = ca.invisible_server_config(
        state.listener_tls_options(),
        fallback_host.clone(),
        observed_sni.clone(),
    );
    let io = GracefulIo::new(
        Box::pin(state.cancel.clone().cancelled_owned()),
        TcpStream::new(stream),
    );
    let served_any = Arc::new(AtomicBool::new(false));
    let served_flag = served_any.clone();
    let handler_state = state.clone();
    let sni_for_req = observed_sni.clone();
    let fallback_for_req = fallback_host.clone();
    let service = service_fn(move |request: Request| {
        served_flag.store(true, Ordering::Relaxed);
        let state = handler_state.clone();
        let authority = request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_owned)
            .or_else(|| crate::lock_unpoison(&sni_for_req).clone())
            .unwrap_or_else(|| fallback_for_req.clone());
        async move {
            Ok::<_, Infallible>(
                handle_http_with_ingress(
                    request,
                    state,
                    Some(message::TunnelTarget::Tls(authority)),
                    true,
                )
                .await,
            )
        }
    });
    let run = async {
        let serve_result = state
            .serve_tls_http_connection(io, tls_config, service, true)
            .await;
        serve_result
            .or_else(|err| {
                if served_any.load(Ordering::Relaxed)
                    || is_benign_tunnel_disconnect(err.as_ref())
                {
                    Ok(())
                } else {
                    let sni_host = crate::lock_unpoison(&observed_sni)
                        .clone()
                        .unwrap_or_else(|| fallback_host.clone());
                    let port = state.config.network.redirect_to_port.unwrap_or(443);
                    state.notify_client_tls_failure(sni_host, port);
                    Err(err)
                }
            })
            .map_err(ProxyError::network)
    };
    tokio::select! {
        biased;
        _ = state.cancel.cancelled() => {},
        result = run => {
            if let Err(error) = result {
                let sni_host = crate::lock_unpoison(&observed_sni)
                    .clone()
                    .unwrap_or_else(|| fallback_host.clone());
                let failed_request = HttpRequest {
                    method: "CONNECT".into(),
                    url: format!("{sni_host}:{}", state.config.network.redirect_to_port.unwrap_or(443)),
                    version: HttpVersion::Http11,
                    headers: Vec::new(),
                    body: Vec::new(),
                    trailers: Vec::new(),
                };
                state.failure(failed_request, &error).await;
            }
        }
    }
}

pub(super) fn format_tunnel_authority(host: &str, port: u16, default_port: u16) -> String {
    let formatted =
        crate::route::format_host_for_authority(crate::route::strip_host_brackets(host));
    if port == default_port {
        formatted
    } else {
        format!("{formatted}:{port}")
    }
}

pub(super) fn looks_like_plain_http_prefix(bytes: &[u8]) -> bool {
    if bytes.is_empty()
        || bytes[0] == socks5::TLS_CLIENT_HELLO_LEAD_BYTE
        || !bytes[0].is_ascii_alphabetic()
    {
        return false;
    }
    let probe = &bytes[..bytes.len().min(24)];
    if let Some(sp) = probe.iter().position(|&b| b == b' ') {
        if sp == 0 {
            return false;
        }
        let method_ok = probe[..sp]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        let after_ok = sp + 1 == probe.len() || (0x21..=0x7E).contains(&probe[sp + 1]);
        return method_ok && after_ok;
    }
    bytes.len() <= 16 && bytes.iter().all(|b| b.is_ascii_uppercase() || *b == b'-')
}

async fn read_tunnel_prefix(
    io: &mut rama::http::io::upgrade::Upgraded,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>, ProxyError> {
    let mut buf = [0u8; 4096];
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        res = io.read(&mut buf) => match res {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(err) if is_benign_tunnel_disconnect(&err) => Ok(None),
            Err(err) => Err(ProxyError::network(err)),
        },
    }
}

async fn handle_entry(
    request: Request,
    state: Arc<State>,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> Response {
    if request.method() != Method::CONNECT {
        return handle_http(request, state, None).await;
    }
    let failed_request = message::invalid_request_head(&request);
    let authority = request.uri().to_string();
    let result = async {
        let uri: Uri = format!("https://{authority}/")
            .parse()
            .map_err(ProxyError::invalid)?;
        let host = uri
            .host_str()
            .ok_or_else(|| ProxyError::invalid("CONNECT 目标缺少主机"))?;
        let port = uri
            .port_u16()
            .ok_or_else(|| ProxyError::invalid("CONNECT 目标缺少端口"))?;
        let _: rama::net::address::Host = crate::route::strip_host_brackets(&host)
            .parse()
            .map_err(|_| ProxyError::invalid("CONNECT 目标主机无效"))?;
        if ssl_passthrough::should_pass_through(&state, &host, port) {
            let (pt_host, pt_port) = state.effective_passthrough_target(&host, port);
            return ssl_passthrough::spawn_passthrough(
                &request,
                pt_host,
                pt_port,
                state.clone(),
                permit,
                failed_request.clone(),
            );
        }
        let tls_options = state.listener_tls_options();
        let tls_host = host.to_string();
        let tls_port = port;
        let target_is_ip = crate::route::normalize_host(&tls_host)
            .parse::<IpAddr>()
            .is_ok();
        let upgrade = handle_upgrade(&request);
        let task_state = state.clone();
        let mut tunnel_request = failed_request.clone();
        state.tasks.spawn(async move {
            let _permit = permit;
            let run = async {
                let outcome: Result<(), ProxyError> = async {
                    let mut upgraded = upgrade.await.map_err(ProxyError::network)?;
                    let Some(prefix) = read_tunnel_prefix(&mut upgraded, &task_state.cancel).await?
                    else {
                        return Ok(());
                    };
                    let plain_http = looks_like_plain_http_prefix(&prefix);
                    let lead_byte = prefix[0];
                    let passthrough_sni = if !plain_http
                        && target_is_ip
                        && lead_byte == socks5::TLS_CLIENT_HELLO_LEAD_BYTE
                        && task_state.dynamic.borrow().ssl_pass_through.is_some()
                    {
                        parse_client_hello_sni(&prefix)
                    } else {
                        None
                    };
                    let rewound =
                        rama::io::PrefixedIo::new(std::io::Cursor::new(prefix), upgraded);
                    if plain_http {
                        let plain_authority = format_tunnel_authority(&tls_host, tls_port, 80);
                        let handler_state = task_state.clone();
                        let service = service_fn(move |request: Request| {
                            let state = handler_state.clone();
                            let plain_authority = plain_authority.clone();
                            async move {
                                Ok::<_, Infallible>(
                                    handle_http_with_ingress(
                                        request,
                                        state,
                                        Some(message::TunnelTarget::PlainHttp(plain_authority)),
                                        true,
                                    )
                                    .await,
                                )
                            }
                        });
                        task_state.serve_http_connection(rewound, service).await;
                        return Ok(());
                    }
                    if let Some(sni) = passthrough_sni
                        && ssl_passthrough::should_pass_through(&task_state, &sni, tls_port)
                    {
                        let (pt_host, pt_port) =
                            task_state.effective_passthrough_target(&sni, tls_port);
                        tunnel_request.url = format!(
                            "{}:{pt_port}",
                            crate::route::format_host_for_authority(&pt_host)
                        );
                        return ssl_passthrough::relay_stream(
                            rewound,
                            &pt_host,
                            pt_port,
                            &task_state,
                        )
                        .await;
                    }
                    let observed_sni = Arc::new(Mutex::new(None::<String>));
                    let ca = task_state.dynamic.borrow().ca.clone();
                    let config = tokio::time::timeout(
                        task_state.config.limits.request_timeout,
                        ca.connect_server_config(&tls_host, &tls_options, observed_sni.clone()),
                    )
                    .await
                    .map_err(|_| ProxyError::Timeout)??;
                    let served_any = Arc::new(AtomicBool::new(false));
                    let served_flag = served_any.clone();
                    let handler_state = task_state.clone();
                    let sni_for_req = observed_sni.clone();
                    let service = service_fn(move |request: Request| {
                        served_flag.store(true, Ordering::Relaxed);
                        let state = handler_state.clone();
                        let effective_authority = if target_is_ip {
                            if let Some(sni) = crate::lock_unpoison(&sni_for_req).clone() {
                                format_tunnel_authority(&sni, tls_port, 443)
                            } else {
                                authority.clone()
                            }
                        } else {
                            authority.clone()
                        };
                        async move {
                            Ok::<_, Infallible>(
                                handle_http_with_ingress(
                                    request,
                                    state,
                                    Some(message::TunnelTarget::Tls(effective_authority)),
                                    true,
                                )
                                .await,
                            )
                        }
                    });
                    let serve_result = task_state
                        .serve_tls_http_connection(rewound, config, service, true)
                        .await;
                    serve_result
                        .or_else(|err| {
                            if served_any.load(Ordering::Relaxed)
                                || is_benign_tunnel_disconnect(err.as_ref())
                            {
                                Ok(())
                            } else {
                                let observed = crate::lock_unpoison(&observed_sni).clone();
                                let effective_host = if let Some(ref sni) = observed {
                                    tunnel_request.url = format!(
                                        "{}:{tls_port}",
                                        crate::route::format_host_for_authority(sni)
                                    );
                                    sni.clone()
                                } else {
                                    tls_host.clone()
                                };
                                task_state.notify_client_tls_failure(effective_host, tls_port);
                                Err(err)
                            }
                        })
                        .map_err(ProxyError::network)
                }
                .await;
                if let Err(error) = outcome
                    && !matches!(error, ProxyError::Cancelled)
                    && !task_state.cancel.is_cancelled()
                {
                    task_state.failure(tunnel_request, &error).await;
                }
            };
            tokio::select! {
                biased;
                _ = task_state.cancel.cancelled() => {},
                _ = run => {},
            }
        });
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::OK;
        Ok(response)
    }
    .await;
    match result {
        Ok(response) => response,
        Err(error) => {
            state.failure(failed_request, &error).await;
            state.error_response(&error)
        }
    }
}

pub(in crate::server) fn is_benign_tunnel_disconnect(
    err: &(dyn std::error::Error + 'static),
) -> bool {
    let mut saw_disconnect = false;
    let mut cur = Some(err);
    while let Some(e) = cur {
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => return false,
                std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected => saw_disconnect = true,
                _ => {}
            }
        }
        if e.downcast_ref::<rustls::Error>().is_some() {
            return false;
        }
        cur = e.source();
    }
    saw_disconnect
}

fn maybe_serve_local_ca_portal(request: &Request, state: &State) -> Option<Response> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return None;
    }
    let uri = request.uri();
    let host_cow = uri.host_str().or_else(|| {
        request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(|h| std::borrow::Cow::Borrowed(h.split(':').next().unwrap_or(h)))
    })?;
    let host = host_cow.trim_end_matches('.');
    if !host.eq_ignore_ascii_case("mio") {
        return None;
    }
    let path_cow = uri.path_or_root();
    let path = path_cow.as_ref();
    let head_only = request.method() == Method::HEAD;
    let ca = state.dynamic.borrow().ca.clone();
    match path {
        "/cert" | "/cert.der" | "/mio-ca.cer" | "/cacert.der" => {
            let der = ca.certificate_der();
            let mut resp = Response::new(rama::http::Body::from(if head_only {
                Vec::new()
            } else {
                der
            }));
            *resp.status_mut() = rama::http::StatusCode::OK;
            resp.headers_mut().insert(
                "content-type",
                rama::http::HeaderValue::from_static("application/x-x509-ca-cert"),
            );
            resp.headers_mut().insert(
                "content-disposition",
                rama::http::HeaderValue::from_static("attachment; filename=\"mio-ca.der\""),
            );
            Some(resp)
        }
        "/cert.pem" | "/mio-ca.pem" => {
            let pem = ca.certificate_pem().into_bytes();
            let mut resp = Response::new(rama::http::Body::from(if head_only {
                Vec::new()
            } else {
                pem
            }));
            *resp.status_mut() = rama::http::StatusCode::OK;
            resp.headers_mut().insert(
                "content-type",
                rama::http::HeaderValue::from_static("application/x-pem-file"),
            );
            resp.headers_mut().insert(
                "content-disposition",
                rama::http::HeaderValue::from_static("attachment; filename=\"mio-ca.pem\""),
            );
            Some(resp)
        }
        "/" | "" => {
            const PORTAL_HTML: &str = "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>MIO Proxy</title><style>body{margin:0;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#f4f5f7;color:#1f2328}header{background:#fff;border-bottom:1px solid #d0d7de;padding:14px 28px;display:flex;align-items:center;justify-content:space-between}.brand{font-weight:700;font-size:16px}.actions a{display:inline-block;margin-left:10px;padding:7px 14px;border-radius:4px;text-decoration:none;font-size:13px;font-weight:600}.primary{background:#d95b26;color:#fff}.secondary{background:#fff;color:#1f2328;border:1px solid #c4c9d0}main{max-width:720px;margin:48px auto;background:#fff;border:1px solid #d0d7de;border-radius:6px;padding:28px}</style></head><body><header><div class=\"brand\">MIO — MITM Inspection &amp; Orchestration</div><div class=\"actions\"><a class=\"primary\" href=\"/cert\">CA Certificate (DER)</a><a class=\"secondary\" href=\"/cert.pem\">CA Certificate (PEM)</a></div></header><main><h2>欢迎使用 MIO 拦截代理</h2><p>当前浏览器流量已成功接入 MIO 本地代理监听器。点击右上角按钮即可下载用户级持久化根证书（<code>MIO local CA</code>），导入系统或浏览器受信任根证书颁发机构后即可拦截检视 HTTPS / HTTP/2 / WSS 流量。</p></main></body></html>";
            let mut resp = Response::new(rama::http::Body::from(if head_only {
                ""
            } else {
                PORTAL_HTML
            }));
            *resp.status_mut() = rama::http::StatusCode::OK;
            resp.headers_mut().insert(
                "content-type",
                rama::http::HeaderValue::from_static("text/html; charset=utf-8"),
            );
            Some(resp)
        }
        _ => None,
    }
}

pub(in crate::server) async fn handle_http(
    request: Request,
    state: Arc<State>,
    tunnel: Option<message::TunnelTarget>,
) -> Response {
    handle_http_with_ingress(request, state, tunnel, false).await
}

pub(in crate::server) async fn handle_http_with_ingress(
    request: Request,
    state: Arc<State>,
    tunnel: Option<message::TunnelTarget>,
    allow_origin_form: bool,
) -> Response {
    if tunnel.is_none()
        && !state.config.http.disable_web_interface
        && let Some(portal_response) = maybe_serve_local_ca_portal(&request, &state)
    {
        return portal_response;
    }
    let started = Instant::now();
    if tunnel.is_none()
        && !allow_origin_form
        && request.uri().scheme().is_none()
        && !state.config.network.invisible_proxying
        && state
            .config
            .network
            .redirect_to_host
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        let error = ProxyError::invalid("缺少请求目标 authority（如需直连请启用隐形代理）");
        state
            .failure(message::invalid_request_head(&request), &error)
            .await;
        return state.error_response(&error);
    }
    let mut model = match message::request_head_with_fallback(
        &request,
        tunnel.as_ref(),
        state.config.network.redirect_to_host.as_deref(),
    ) {
        Ok(model) => model,
        Err(error) => {
            state
                .failure(message::invalid_request_head(&request), &error)
                .await;
            return state.error_response(&error);
        }
    };
    if let Err(error) = apply_listener_target_overrides(&mut model, &state.config) {
        state.failure(model, &error).await;
        return state.error_response(&error);
    }
    if state.should_drop_out_of_scope(&model.url) {
        return state.error_response(&ProxyError::Dropped);
    }
    if !state.config.http.understand_100_continue
        && !state.config.http.remove_100_continue_headers
        && message::has_header_token(request.headers(), "expect", "100-continue")
    {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::EXPECTATION_FAILED;
        return resp;
    }
    let is_ws = websocket::requested(&request);
    apply_proxy_misc_request_transforms(&mut model, &state.config, is_ws);
    let permit = match state.requests.clone().try_acquire_owned() {
        Ok(permit) => Arc::new(permit),
        Err(_) => {
            let error = ProxyError::Network("活动请求达到配置上限".into());
            state.failure(model, &error).await;
            return state.error_response(&error);
        }
    };
    let mut active = ActiveFlow {
        state: state.clone(),
        record: Some(new_record(model)),
        response_stream: None,
        capture_metrics: CapturedFlowMetrics::default(),
        request_was_intercepted: false,
        request_was_modified: false,
        started,
        permit: permit.clone(),
    };
    if is_ws {
        return websocket::handle(request, active).await;
    }
    let cancel = state.cancel.child_token();
    let cancel_on_drop = cancel.clone().drop_guard();
    let record = active.record.as_mut().expect("active flow owns its record");
    let ingress_version = message::model_version(request.version());
    let upstream = process_http(
        request,
        &state,
        record,
        permit,
        cancel.clone(),
        &mut active.request_was_intercepted,
        &mut active.request_was_modified,
    )
    .await;
    let upstream = upstream.map(|(response, deadline, capture_metrics)| {
        active.capture_metrics = capture_metrics;
        (response, deadline)
    });
    let result = match upstream {
        Ok((response, _))
            if sse::detected_or_in_scope(
                &response,
                &record.request,
                state.config.http.treat_event_stream_as_streaming,
                state.config.http.streaming_scope_matcher.as_ref(),
            ) =>
        {
            return sse::handle(response, active, ingress_version, cancel, cancel_on_drop).await;
        }
        Ok((response, deadline)) => {
            let status = response.status().as_u16();
            let version = message::model_version(response.version());
            let reason_phrase = active.capture_metrics.reason_phrase.clone();
            let headers = message::headers(response.headers());
            async {
                let (body, trailers) = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
                    result = tokio::time::timeout_at(deadline, message::body(response.into_body(), state.config.limits.max_body_bytes)) => result.map_err(|_| ProxyError::Timeout)??,
                };
                cancel.cancel();
                record.summary.status = Some(status);
                record.summary.response_bytes = body.len() as u64;
                record.response = Some(HttpResponse { status, version, reason_phrase, headers, body, trailers });
                state.intercept(record, true, None, &active.capture_metrics, active.request_was_intercepted, active.request_was_modified).await?;
                let response_client_version = if state.config.http.use_http10_in_responses
                    && ingress_version != HttpVersion::Http2
                {
                    HttpVersion::Http10
                } else {
                    ingress_version
                };
                let mut outgoing = message::to_response(
                    record.response.as_ref().expect("upstream response is present"),
                    state.config.limits.max_body_bytes,
                    response_client_version,
                    record.request.method.eq_ignore_ascii_case("HEAD"),
                )?;
                if state.config.http.set_response_connection_close
                    && response_client_version != HttpVersion::Http2
                {
                    outgoing
                        .headers_mut()
                        .insert("connection", HeaderValue::from_static("close"));
                }
                Ok(outgoing)
            }
            .await
        }
        Err(error) => Err(error),
    };
    cancel.cancel();
    active.finish(result.as_ref().map(|_| ())).await;
    result.unwrap_or_else(|error| state.error_response(&error))
}

pub(in crate::server) fn apply_listener_target_overrides(
    model: &mut HttpRequest,
    config: &StaticProxyConfig,
) -> Result<(), ProxyError> {
    let redirect_host = config
        .network
        .redirect_to_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    let redirect_port = config.network.redirect_to_port.filter(|&p| p > 0);
    if redirect_host.is_none() && redirect_port.is_none() && !config.tls.force_use_of_tls {
        return Ok(());
    }
    let uri: Uri = model.url.parse().map_err(ProxyError::invalid)?;
    let original_scheme = uri.scheme_str().unwrap_or("http");
    let effective_scheme = if config.tls.force_use_of_tls {
        if original_scheme.eq_ignore_ascii_case("ws") || original_scheme.eq_ignore_ascii_case("wss")
        {
            "wss"
        } else {
            "https"
        }
    } else {
        original_scheme
    };
    let original_host = uri
        .host_str()
        .ok_or_else(|| ProxyError::invalid("缺少请求目标主机"))?;
    let host_raw = redirect_host.unwrap_or(original_host.as_ref());
    let formatted_host = crate::route::format_host_for_authority(host_raw);
    let effective_port = redirect_port.or_else(|| uri.port_u16());
    let mut origin_target = rama::bytes::BytesMut::new();
    uri.write_http_origin_form(&mut origin_target)
        .map_err(ProxyError::invalid)?;
    let path_and_query = std::str::from_utf8(&origin_target).map_err(ProxyError::invalid)?;
    model.url = match effective_port {
        Some(port) => format!("{effective_scheme}://{formatted_host}:{port}{path_and_query}"),
        None => format!("{effective_scheme}://{formatted_host}{path_and_query}"),
    };
    Ok(())
}

pub(in crate::server) fn apply_proxy_misc_request_transforms(
    model: &mut HttpRequest,
    config: &StaticProxyConfig,
    is_websocket: bool,
) {
    if config.http.strip_proxy_headers {
        model.headers.retain(|header| {
            !header
                .name
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("proxy-"))
        });
    }
    if config.http.remove_unsupported_accept_encodings {
        filter_unsupported_accept_encodings(&mut model.headers);
    }
    if config.http.remove_100_continue_headers {
        message::strip_expect_100_continue_vec(&mut model.headers);
    }
    if is_websocket && config.http.strip_sec_websocket_extensions {
        model
            .headers
            .retain(|header| !header.name.eq_ignore_ascii_case("sec-websocket-extensions"));
    }
    if !is_websocket {
        if config.http.use_http10_in_requests {
            model.version = HttpVersion::Http10;
        }
        if config.http.set_request_connection_header && model.version != HttpVersion::Http2 {
            let value = if !config.http.http1_keep_alive || config.http.use_http10_in_requests {
                b"close".to_vec()
            } else {
                b"keep-alive".to_vec()
            };
            let mut updated = false;
            model.headers.retain_mut(|header| {
                if !header.name.eq_ignore_ascii_case("connection") {
                    return true;
                }
                if updated {
                    return false;
                }
                header.value = value.clone();
                updated = true;
                true
            });
            if !updated {
                model.headers.push(Header {
                    name: "Connection".into(),
                    value,
                });
            }
        }
    }
}

fn filter_unsupported_accept_encodings(headers: &mut Vec<Header>) {
    headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case("accept-encoding") {
            return true;
        }
        let Ok(text) = std::str::from_utf8(&header.value) else {
            return true;
        };
        let kept: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|part| {
                let coding = part.split(';').next().unwrap_or("").trim();
                matches!(
                    coding.to_ascii_lowercase().as_str(),
                    "gzip" | "x-gzip" | "deflate" | "br" | "identity"
                )
            })
            .collect();
        if kept.is_empty() {
            false
        } else {
            header.value = kept.join(", ").into_bytes();
            true
        }
    });
}

pub(in crate::server) fn new_record(model: HttpRequest) -> FlowRecord {
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: model.method.clone(),
            url: model.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: 0,
            response_bytes: 0,
            started_unix_ms: crate::now_unix_ms(),
            elapsed_ms: 0,
        },
        request: model,
        response: None,
    }
}

pub(in crate::server) async fn process_http(
    request: Request,
    state: &Arc<State>,
    record: &mut FlowRecord,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    cancel: CancellationToken,
    request_was_intercepted: &mut bool,
    request_was_modified: &mut bool,
) -> Result<(Response, tokio::time::Instant, CapturedFlowMetrics), ProxyError> {
    let (body, trailers) = tokio::select! {
        biased;
        _ = state.cancel.cancelled() => return Err(ProxyError::Cancelled),
        result = tokio::time::timeout(state.config.limits.request_timeout, message::body(request.into_body(), state.config.limits.max_body_bytes)) => result.map_err(|_| ProxyError::Timeout)??,
    };
    record.request.body = body;
    record.request.trailers = trailers;
    record.summary.request_bytes = record.request.body.len() as u64;
    state
        .emit(ProxyEvent::Captured {
            project: state.project,
            record: record.clone(),
            response_stream: None,
            listener_port: (state.listener_port != 0).then_some(state.listener_port),
            capture_metrics: CapturedFlowMetrics::default(),
        })
        .await?;
    (*request_was_intercepted, *request_was_modified) = state
        .intercept(
            record,
            false,
            None,
            &CapturedFlowMetrics::default(),
            false,
            false,
        )
        .await?;
    let config = state.send_config(&record.request.url, record.request.version, false);
    message::validate_request_with_config(&record.request, &config)?;
    let outbound_model = record.request.clone();
    let deadline = tokio::time::Instant::now() + config.limits.timeout;
    let (sender, response) = oneshot::channel();
    // Keep the driver and request budget until cancellation has actually joined the socket.
    state.tasks.spawn(async move {
        let _permit = permit;
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        let exec = Executor::graceful(shutdown.guard());
        let attempt_started = Instant::now();
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = client::exchange_model_with_auth(outbound_model, &config, deadline, cancel.clone(), exec) => {
                match result {
                    Ok((resp, timing)) => {
                        let exchange_returned = Instant::now();
                        let reason_phrase = message::reason_phrase(&resp);
                        let (remote_ip, received) = {
                            let guard = crate::lock_unpoison(&timing);
                            (
                                guard.peer_ip(),
                                guard.first_response_byte().or(Some(exchange_returned)),
                            )
                        };
                        let response_received_ms = received.map(|received| {
                            received
                                .saturating_duration_since(attempt_started)
                                .as_millis()
                                .min(u64::MAX as u128) as u64
                        });
                        Ok((
                            resp,
                            CapturedFlowMetrics {
                                remote_ip,
                                response_received_ms,
                                reason_phrase,
                            },
                        ))
                    }
                    Err(err) => Err(err),
                }
            },
        };
        if sender.send(result).is_ok() {
            cancel.cancelled().await;
        }
        cancel.cancel();
        shutdown.shutdown().await;
    });
    let (response, metrics) = response.await.map_err(ProxyError::network)??;
    Ok((response, deadline, metrics))
}
