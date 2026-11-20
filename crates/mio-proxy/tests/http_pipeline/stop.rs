use super::*;

#[tokio::test]
async fn stop_closes_connect_waiting_for_tls_or_client_hello_tail_without_peer_eof() {
    for prefix_length in [0, 1] {
        let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
        let (mut proxy, ca, _events) = proxy(&origin, false).await;
        let mut stream = tokio::time::timeout(
            DEADLINE,
            connect_tunnel(
                proxy.address(),
                &format!("localhost:{}", origin.address.port()),
            ),
        )
        .await
        .unwrap();
        stream.set_nodelay(true).unwrap();
        let connector = tls_connector(ca, false);
        let mut session = rustls::ClientConnection::new(
            connector.config().clone(),
            ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        let mut hello = Vec::new();
        session.write_tls(&mut hello).unwrap();
        assert_eq!(hello[0], 0x16);
        stream.write_all(&hello[..prefix_length]).await.unwrap();
        proxy.stop().await.unwrap();
        let mut byte = [0];
        let read = tokio::time::timeout(DEADLINE, stream.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)));
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        assert!(TcpStream::connect(proxy.address()).await.is_err());
        origin.stop().await;
    }
}

#[tokio::test]
async fn repeater_cancel_closes_silent_origin_and_does_not_retry_post() {
    let mut origin = Origin::start(None, true).await;
    let cancel = CancellationToken::new();
    let task = tokio::spawn(mio_proxy::send(
        HttpRequest {
            method: "POST".into(),
            url: origin.url(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: vec![0, 0xff],
            trailers: Vec::new(),
        },
        SendConfig::default(),
        cancel.clone(),
    ));
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(DEADLINE, task).await.unwrap().unwrap(),
        Err(ProxyError::Cancelled)
    ));
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    assert_eq!(origin.received.load(Ordering::SeqCst), 1);
    origin.stop().await;
}

#[tokio::test]
async fn body_limit_fails_instead_of_truncating_and_sending() {
    let origin = Origin::start(None, false).await;
    let config = SendConfig {
        max_body_bytes: 2,
        ..SendConfig::default()
    };
    let result = mio_proxy::send(
        HttpRequest {
            method: "POST".into(),
            url: origin.url(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: vec![0, 1, 2],
            trailers: Vec::new(),
        },
        config,
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(result, Err(ProxyError::BodyLimit { limit: 2 })));
    assert_eq!(origin.received.load(Ordering::SeqCst), 0);
    origin.stop().await;
}

async fn expect_failed_record(events: &mut mpsc::Receiver<ProxyEvent>, method: &str) {
    let record = tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap()
                && matches!(record.summary.state, FlowState::Failed(_))
            {
                return record;
            }
        }
    })
    .await
    .expect("protocol errors must be visible in History");
    assert_eq!(record.summary.method, method);
}

#[tokio::test]
async fn history_reports_missing_request_authority() {
    let origin = Origin::start(None, false).await;
    let (mut proxy, _, mut events) = proxy(&origin, false).await;
    let mut stream = TcpStream::connect(proxy.address()).await.unwrap();
    stream.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    let mut response = [0; 128];
    let count = tokio::time::timeout(DEADLINE, stream.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response[..count].starts_with(b"HTTP/1.0 400"));
    expect_failed_record(&mut events, "GET").await;
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn history_reports_invalid_connect_authority() {
    let origin = Origin::start(None, false).await;
    let (mut proxy, _, mut events) = proxy(&origin, false).await;
    let mut stream = TcpStream::connect(proxy.address()).await.unwrap();
    stream
        .write_all(b"CONNECT localhost HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut response = [0; 128];
    let count = tokio::time::timeout(DEADLINE, stream.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response[..count].starts_with(b"HTTP/1.1 400"));
    expect_failed_record(&mut events, "CONNECT").await;
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn history_reports_tls_sni_mismatch_after_connect() {
    let origin = Origin::start(None, false).await;
    let (mut proxy, ca, mut events) = proxy(&origin, false).await;
    let stream = connect_tunnel(
        proxy.address(),
        &format!("localhost:{}", origin.address.port()),
    )
    .await;
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ca)).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let result = tokio::time::timeout(
        DEADLINE,
        TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("wrong.invalid").unwrap(), stream),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    expect_failed_record(&mut events, "CONNECT").await;
    assert_eq!(origin.received.load(Ordering::SeqCst), 0);
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
#[expect(deprecated)]
async fn preconnect_reset_or_post_request_reset_never_emits_spurious_failed_connect() {
    let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
    let (mut proxy, ca, mut events) = proxy(&origin, false).await;
    let authority = format!("localhost:{}", origin.address.port());

    // 1. Browser Happy Eyeballs preconnect tunnel aborted via TCP RST before TLS ClientHello.
    let preconnect = connect_tunnel(proxy.address(), &authority).await;
    preconnect.set_linger(Some(Duration::ZERO)).unwrap();
    drop(preconnect);

    // 2. HTTPS tunnel serves a normal request, then client closes idle pool socket with TCP RST.
    let stream = connect_tunnel(proxy.address(), &authority).await;
    stream.set_linger(Some(Duration::ZERO)).unwrap();
    let mut tls = tls_connector(ca, false)
        .connect(ServerName::try_from("localhost").unwrap(), stream)
        .await
        .unwrap();
    tls.write_all(
        format!("GET /ok HTTP/1.1\r\nHost: {authority}\r\nConnection: keep-alive\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = [0u8; 512];
    let n = tokio::time::timeout(DEADLINE, tls.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(buf[..n].starts_with(b"HTTP/1.1 200"));
    drop(tls);

    proxy.stop().await.unwrap();
    origin.stop().await;

    let mut terminal = Vec::new();
    while let Ok(ev) = events.try_recv() {
        if let ProxyEvent::Captured { record, .. } = ev
            && record.summary.state != FlowState::Pending
        {
            terminal.push((record.summary.method, record.summary.state));
        }
    }
    assert_eq!(
        terminal,
        vec![("GET".to_owned(), FlowState::Complete)],
        "idle or preconnect TCP RST on CONNECT tunnels must not pollute HTTP history: {terminal:?}"
    );
}

pub(super) async fn active_stop<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: I,
    h2: bool,
    origin: &mut Origin,
    proxy: &mut mio_proxy::ProxyHandle,
) {
    let request = Request::builder()
        .method("POST")
        .uri(origin.url())
        .body(Full::new(Bytes::from_static(b"non-idempotent")))
        .unwrap();
    let (pending, driver) = if h2 {
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(io))
                .await
                .unwrap();
        (
            tokio::spawn(async move { sender.send_request(request).await }),
            tokio::spawn(connection),
        )
    } else {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
            .await
            .unwrap();
        (
            tokio::spawn(async move { sender.send_request(request).await }),
            tokio::spawn(connection),
        )
    };
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    proxy.stop().await.unwrap();
    let result = tokio::time::timeout(DEADLINE, pending)
        .await
        .unwrap()
        .unwrap();
    if let Ok(response) = result {
        assert_eq!(response.status().as_u16(), 503);
    }
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    assert_eq!(origin.received.load(Ordering::SeqCst), 1);
    driver.abort();
    let _ = driver.await;
}
