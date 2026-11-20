use super::*;

#[tokio::test]
async fn https_send_preserves_request_target_on_h1_and_h2() {
    tokio::time::timeout(DEADLINE, async {
        let paths = [
            "/a/../b?first=%2F&x=%2e%2E&x=1",
            "/a/%2e%2E//b?",
            "/a/%5C/%5c?next=http://other.invalid/../x",
        ];
        for h2 in [false, true] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let (ca, config) = origin_certificate(CertificateKind::Valid, h2);
            let acceptor = TlsAcceptor::from(Arc::new(config));
            let origin = async {
                for _ in &paths {
                    let (stream, _) = listener.accept().await.unwrap();
                    let stream = acceptor.accept(stream).await.unwrap();
                    assert_eq!(
                        stream.get_ref().1.alpn_protocol(),
                        Some(if h2 { b"h2".as_slice() } else { b"http/1.1" }),
                    );
                    let service = service_fn(move |request: Request<Incoming>| async move {
                        assert_eq!(
                            request.version(),
                            if h2 {
                                Version::HTTP_2
                            } else {
                                Version::HTTP_11
                            },
                        );
                        let target = request.uri().path_and_query().unwrap().as_str();
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::copy_from_slice(
                            target.as_bytes(),
                        ))))
                    });
                    hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                        .unwrap();
                }
            };
            let sending = async {
                let mut responses = Vec::new();
                for path in &paths {
                    responses.push(
                        mio_proxy::send(
                            HttpRequest {
                                method: "GET".into(),
                                url: format!("https://{address}{path}"),
                                version: if h2 {
                                    HttpVersion::Http2
                                } else {
                                    HttpVersion::Http11
                                },
                                headers: Vec::new(),
                                body: Vec::new(),
                                trailers: Vec::new(),
                            },
                            SendConfig {
                                upstream_tls: UpstreamTls::CustomRoots {
                                    certificates_der: vec![ca.clone()],
                                    include_system: false,
                                },
                                timeout: Duration::from_secs(1),
                                http_version: Some(if h2 {
                                    HttpVersion::Http2
                                } else {
                                    HttpVersion::Http11
                                }),
                                ..SendConfig::default()
                            },
                            CancellationToken::new(),
                        )
                        .await,
                    );
                }
                responses
            };
            let ((), responses) = tokio::join!(origin, sending);
            for (path, response) in paths.iter().zip(responses) {
                let response = response.unwrap();
                assert_eq!(response.status, 200);
                assert_eq!(
                    response.version,
                    if h2 {
                        HttpVersion::Http2
                    } else {
                        HttpVersion::Http11
                    },
                );
                assert_eq!(
                    response.body,
                    path.as_bytes(),
                    "origin must receive the manual target"
                );
            }
        }
    })
    .await
    .expect("TLS request-target regression has a finite deadline");
}

#[tokio::test]
async fn proxy_rejects_literal_backslash_and_keeps_original_failed_record() {
    let origin = Origin::start(None, false).await;
    let (mut proxy, _, mut events) = proxy(&origin, false).await;
    let mut observed = Vec::new();
    for path in [r"/a\b", r"/a?b=\c"] {
        let url = format!("http://{}{path}", origin.address);
        let mut stream = TcpStream::connect(proxy.address()).await.unwrap();
        stream
            .write_all(
                format!("GET {url} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut status = [0; 12];
        tokio::time::timeout(DEADLINE, stream.read_exact(&mut status))
            .await
            .unwrap()
            .unwrap();
        drop(stream);
        let record = tokio::time::timeout(DEADLINE, async {
            loop {
                if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap()
                    && matches!(
                        record.summary.state,
                        FlowState::Complete | FlowState::Failed(_) | FlowState::Cancelled
                    )
                {
                    break record;
                }
            }
        })
        .await
        .unwrap();
        observed.push((url, status, record));
    }
    proxy.stop().await.unwrap();
    let received = origin.received.load(Ordering::SeqCst);
    origin.stop().await;

    for (url, status, record) in observed {
        assert_eq!(&status, b"HTTP/1.1 400");
        assert!(matches!(&record.summary.state, FlowState::Failed(message)
                if message.contains("SDK") && message.contains("反斜线")));
        assert_eq!(record.request.url, url);
        assert_eq!(record.summary.url, url);
        assert!(record.response.is_none());
    }
    assert_eq!(received, 0, "Proxy must not forward a rewritten target");
}
