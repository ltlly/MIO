use super::*;

pub(super) struct ClientResponse {
    pub(super) status: u16,
    pub(super) version: Version,
    pub(super) alpn: Option<Vec<u8>>,
    pub(super) headers: Vec<(String, Vec<u8>)>,
    pub(super) body: Vec<u8>,
}

pub(super) async fn connect_tunnel(address: SocketAddr, authority: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 8192);
        header.push(stream.read_u8().await.unwrap());
    }
    assert!(
        header.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&header)
    );
    stream
}

pub(super) async fn exchange<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: I,
    url: String,
    h2: bool,
    alpn: Option<Vec<u8>>,
) -> ClientResponse {
    exchange_with_relay(io, url, h2, alpn, None).await
}

async fn exchange_with_relay<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: I,
    url: String,
    h2: bool,
    alpn: Option<Vec<u8>>,
    relay: Option<(CancellationToken, JoinHandle<std::io::Result<()>>)>,
) -> ClientResponse {
    let mut request = Request::new(Full::new(Bytes::from_static(&[0, 0xfe, b'C'])));
    *request.method_mut() = hyper::Method::POST;
    *request.uri_mut() = url.parse().unwrap();
    request
        .headers_mut()
        .append("x-client", "one".parse().unwrap());
    request
        .headers_mut()
        .append("x-client", "two".parse().unwrap());
    let (mut h1_sender, mut h2_sender) = (None, None);
    let (response, driver) = if h2 {
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(io))
                .await
                .unwrap();
        let driver = tokio::spawn(connection);
        let response = sender.send_request(request).await.unwrap();
        h2_sender = Some(sender);
        (response, driver)
    } else {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
            .await
            .unwrap();
        let driver = tokio::spawn(connection);
        let response = sender.send_request(request).await.unwrap();
        h1_sender = Some(sender);
        (response, driver)
    };
    let status = response.status().as_u16();
    let version = response.version();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    if let Some((cancel, relay)) = relay {
        // Finish forwarding before dropping the HTTP driver that owns the duplex endpoint.
        cancel.cancel();
        tokio::time::timeout(DEADLINE, relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    drop((h1_sender, h2_sender));
    driver.abort();
    let _ = driver.await;
    ClientResponse {
        status,
        version,
        alpn,
        headers,
        body,
    }
}

pub(super) async fn client(
    address: SocketAddr,
    url: String,
    ca: Vec<u8>,
    tls: bool,
    h2: bool,
) -> ClientResponse {
    if !tls {
        return exchange(TcpStream::connect(address).await.unwrap(), url, false, None).await;
    }
    let authority = url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap();
    let stream = tls_stream(address, authority, ca, h2).await;
    let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
    exchange(stream, url, h2, alpn).await
}

pub(super) async fn tls_stream(
    address: SocketAddr,
    authority: &str,
    ca: Vec<u8>,
    h2: bool,
) -> tokio_rustls::client::TlsStream<TcpStream> {
    let stream = connect_tunnel(address, authority).await;
    tls_connector(ca, h2)
        .connect(ServerName::try_from("localhost").unwrap(), stream)
        .await
        .unwrap()
}

pub(super) fn tls_connector(ca: Vec<u8>, h2: bool) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ca)).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![if h2 {
        b"h2".to_vec()
    } else {
        b"http/1.1".to_vec()
    }];
    TlsConnector::from(Arc::new(config))
}

pub(super) async fn client_with_fragmented_hello(
    address: SocketAddr,
    url: String,
    ca: Vec<u8>,
    h2: bool,
) -> ClientResponse {
    let authority = url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap();
    let mut upstream = connect_tunnel(address, authority).await;
    upstream.set_nodelay(true).unwrap();
    let (client_io, mut relay_io) = tokio::io::duplex(8192);
    let (first_sent, first_received) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let relay_cancel = CancellationToken::new();
    let forwarding_cancel = relay_cancel.clone();
    let relay = tokio::spawn(async move {
        let mut header = [0; 5];
        relay_io.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 0x16, "first TLS record must contain a handshake");
        let length = u16::from_be_bytes([header[3], header[4]]) as usize;
        assert!((4..=16_384).contains(&length));
        let mut hello = vec![0; length];
        relay_io.read_exact(&mut hello).await.unwrap();
        assert_eq!(hello[0], 1, "Rustls must produce a real ClientHello");
        assert_eq!(
            u32::from_be_bytes([0, hello[1], hello[2], hello[3]]) as usize,
            length - 4
        );
        upstream.write_all(&header[..1]).await.unwrap();
        first_sent.send(()).unwrap();
        released.await.unwrap();
        // Separate TLS records force handshake fragmentation even if TCP coalesces writes.
        header[3..].copy_from_slice(&1_u16.to_be_bytes());
        upstream.write_all(&header[1..]).await.unwrap();
        upstream.write_all(&hello[..1]).await.unwrap();
        header[3..].copy_from_slice(&((length - 1) as u16).to_be_bytes());
        upstream.write_all(&header).await.unwrap();
        upstream.write_all(&hello[1..]).await.unwrap();
        tokio::select! {
            biased;
            _ = forwarding_cancel.cancelled() => {},
            result = tokio::io::copy_bidirectional(&mut relay_io, &mut upstream) => {
                result?;
            },
        }
        upstream.shutdown().await?;
        relay_io.shutdown().await
    });
    let request = tokio::spawn(async move {
        let stream = tls_connector(ca, h2)
            .connect(ServerName::try_from("localhost").unwrap(), client_io)
            .await
            .unwrap();
        let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        exchange_with_relay(stream, url, h2, alpn, Some((relay_cancel, relay))).await
    });
    tokio::time::timeout(DEADLINE, first_received)
        .await
        .expect("the first byte must be sent to the real proxy socket")
        .unwrap();
    assert!(!request.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(DEADLINE, request)
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn proxy(
    origin: &Origin,
    intercept: bool,
) -> (mio_proxy::ProxyHandle, Vec<u8>, mpsc::Receiver<ProxyEvent>) {
    proxy_with_route(origin, intercept, UpstreamRoute::Direct).await
}

pub(super) async fn proxy_with_route(
    origin: &Origin,
    intercept: bool,
    upstream_route: UpstreamRoute,
) -> (mio_proxy::ProxyHandle, Vec<u8>, mpsc::Receiver<ProxyEvent>) {
    proxy_with(origin, intercept, |config| {
        config.network.upstream_route = upstream_route;
    })
    .await
}

pub(super) async fn proxy_with(
    origin: &Origin,
    intercept: bool,
    configure: impl FnOnce(&mut ProxyConfig),
) -> (mio_proxy::ProxyHandle, Vec<u8>, mpsc::Receiver<ProxyEvent>) {
    let ca = MitmAuthority::generate().unwrap();
    let certificate = ca.certificate_der();
    let mut config = ProxyConfig::new(ca);
    config.tls.upstream.trust = origin.trust();
    config.hooks.intercept_requests = intercept;
    config.hooks.intercept_responses = intercept;
    config.limits.request_timeout = DEADLINE;
    configure(&mut config);
    let (events, receiver) = mpsc::channel(64);
    let handle = mio_proxy::start(
        config,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events,
    )
    .await
    .unwrap();
    (handle, certificate, receiver)
}

pub(super) async fn next_intercept(
    events: &mut mpsc::Receiver<ProxyEvent>,
) -> (
    mio_plugin_api::FlowRecord,
    tokio::sync::oneshot::Sender<InterceptDecision>,
) {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Intercepted { record, reply, .. } = events.recv().await.unwrap() {
                return (record, reply);
            }
        }
    })
    .await
    .unwrap()
}
