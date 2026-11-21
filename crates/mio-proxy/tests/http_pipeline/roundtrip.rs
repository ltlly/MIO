use super::*;

enum ClientHello {
    Whole,
    Fragmented,
}

async fn mutation_roundtrip(tls: bool, ingress_h2: bool, egress_h2: bool, hello: ClientHello) {
    let mut origin = Origin::start(tls.then_some((CertificateKind::Valid, egress_h2)), false).await;
    let (mut proxy, ca, mut events) = proxy(&origin, true).await;
    let address = proxy.address();
    let url = origin.url();
    let client = match hello {
        ClientHello::Whole => tokio::spawn(client(address, url, ca, tls, ingress_h2)),
        ClientHello::Fragmented => {
            assert!(tls);
            tokio::spawn(client_with_fragmented_hello(address, url, ca, ingress_h2))
        }
    };
    let (mut record, reply) = next_intercept(&mut events).await;
    assert_eq!(record.request.body, [0, 0xfe, b'C']);
    record.request.body.push(0xa1);
    record.request.headers.push(Header {
        name: "x-mio".into(),
        value: b"request".to_vec(),
    });
    reply
        .send(InterceptDecision::ForwardRequest(record.request))
        .unwrap();
    let (mut record, reply) = next_intercept(&mut events).await;
    let response = record.response.as_mut().unwrap();
    assert_eq!(response.body, [0, 0xff, b'O']);
    response.body.push(0xb2);
    response.headers.push(Header {
        name: "x-mio".into(),
        value: b"response".to_vec(),
    });
    reply
        .send(InterceptDecision::ForwardResponse(response.clone()))
        .unwrap();
    let result = tokio::time::timeout(DEADLINE, client)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.body, [0, 0xff, b'O', 0xb2]);
    assert_eq!(
        result
            .headers
            .iter()
            .filter(|(name, _)| name == "x-origin")
            .count(),
        2
    );
    assert!(
        result
            .headers
            .contains(&("x-mio".into(), b"response".to_vec()))
    );
    assert_eq!(
        result.version,
        if ingress_h2 {
            Version::HTTP_2
        } else {
            Version::HTTP_11
        }
    );
    assert_eq!(
        result.alpn,
        tls.then(|| if ingress_h2 {
            b"h2".to_vec()
        } else {
            b"http/1.1".to_vec()
        })
    );
    match origin.next().await {
        OriginEvent::Request {
            version,
            alpn,
            headers,
            body,
        } => {
            assert_eq!(
                version,
                if egress_h2 {
                    Version::HTTP_2
                } else {
                    Version::HTTP_11
                }
            );
            assert_eq!(
                alpn,
                tls.then(|| if egress_h2 {
                    b"h2".to_vec()
                } else {
                    b"http/1.1".to_vec()
                })
            );
            assert_eq!(body, [0, 0xfe, b'C', 0xa1]);
            assert_eq!(
                headers
                    .iter()
                    .filter(|(name, _)| name == "x-client")
                    .count(),
                2
            );
            assert!(headers.contains(&("x-mio".into(), b"request".to_vec())));
        }
        event => panic!("expected actual origin request, got {event:?}"),
    }
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap()
                && record.summary.state == FlowState::Complete
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn http_binary_and_duplicate_headers_roundtrip() {
    mutation_roundtrip(false, false, false, ClientHello::Whole).await;
}

#[tokio::test]
async fn https_h1_binary_and_duplicate_headers_roundtrip() {
    mutation_roundtrip(true, false, false, ClientHello::Whole).await;
}

#[tokio::test]
async fn https_independent_h2_and_mixed_legs() {
    mutation_roundtrip(true, true, true, ClientHello::Whole).await;
    mutation_roundtrip(true, true, false, ClientHello::Whole).await;
    mutation_roundtrip(true, false, true, ClientHello::Whole).await;
}

#[tokio::test]
async fn fragmented_client_hello_remains_mitm_for_https_and_h2() {
    mutation_roundtrip(true, false, false, ClientHello::Fragmented).await;
    mutation_roundtrip(true, true, true, ClientHello::Fragmented).await;
}

#[tokio::test]
async fn rejects_unknown_ca_wrong_name_and_expired_origin() {
    for (kind, trusted) in [
        (CertificateKind::Valid, false),
        (CertificateKind::WrongName, true),
        (CertificateKind::Expired, true),
    ] {
        let mut origin = Origin::start(Some((kind, false)), false).await;
        let ca = MitmAuthority::generate().unwrap();
        let certificate = ca.certificate_der();
        let mut config = ProxyConfig::new(ca);
        if trusted {
            config.tls.upstream.trust = origin.trust();
        }
        let (tx, mut events) = mpsc::channel(16);
        let mut proxy = mio_proxy::start(
            config,
            ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
            tx,
        )
        .await
        .unwrap();
        let result = tokio::time::timeout(
            DEADLINE,
            client(proxy.address(), origin.url(), certificate, true, false),
        )
        .await
        .unwrap();
        assert_eq!(result.status, 502);
        assert!(matches!(origin.next().await, OriginEvent::TlsRejected));
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap()
                    && matches!(record.summary.state, FlowState::Failed(_))
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn socks5_inbound_captures_plaintext_http_and_https_mitm() {
    async fn socks5_connect(proxy_addr: SocketAddr, host: &str, port: u16) -> TcpStream {
        let mut stream = TcpStream::connect(proxy_addr).await.unwrap();
        stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut method_reply = [0u8; 2];
        stream.read_exact(&mut method_reply).await.unwrap();
        assert_eq!(method_reply, [0x05, 0x00]);

        let host_bytes = host.as_bytes();
        let mut req = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
        req.extend_from_slice(host_bytes);
        req.extend_from_slice(&port.to_be_bytes());
        stream.write_all(&req).await.unwrap();

        let mut reply = [0u8; 10];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply[..2], &[0x05, 0x00]);
        stream
    }

    // 1. Plaintext HTTP over SOCKS5h
    let origin_http = Origin::start(None, false).await;
    let (mut proxy_http, _ca_http, mut events_http) = proxy(&origin_http, false).await;
    let http_port = origin_http.address.port();
    let mut stream = socks5_connect(proxy_http.address(), "127.0.0.1", http_port).await;
    stream
        .write_all(
            format!(
                "GET /socks5-plain HTTP/1.1\r\nHost: 127.0.0.1:{http_port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut raw_resp = Vec::new();
    stream.read_to_end(&mut raw_resp).await.unwrap();
    assert!(raw_resp.starts_with(b"HTTP/1.1 200"));
    let captured_plain = tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured { record, .. } = events_http.recv().await.unwrap()
                && record.summary.state == FlowState::Complete
            {
                break record;
            }
        }
    })
    .await
    .unwrap();
    assert!(captured_plain.request.url.ends_with("/socks5-plain"));
    proxy_http.stop().await.unwrap();
    origin_http.stop().await;

    // 2. HTTPS MITM over SOCKS5h
    let origin_tls = Origin::start(Some((CertificateKind::Valid, false)), false).await;
    let (mut proxy_tls, ca_der, mut events_tls) = proxy(&origin_tls, false).await;
    let tls_port = origin_tls.address.port();
    let raw_tunnel = socks5_connect(proxy_tls.address(), "localhost", tls_port).await;
    let connector = tls_connector(ca_der, false);
    let mut tls_conn = connector
        .connect(ServerName::try_from("localhost").unwrap(), raw_tunnel)
        .await
        .unwrap();
    tls_conn
        .write_all(
            format!(
                "GET /socks5-mitm HTTP/1.1\r\nHost: localhost:{tls_port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut decrypted_resp = Vec::new();
    tls_conn.read_to_end(&mut decrypted_resp).await.unwrap();
    assert!(decrypted_resp.starts_with(b"HTTP/1.1 200"));
    let captured_tls = tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured { record, .. } = events_tls.recv().await.unwrap()
                && record.summary.state == FlowState::Complete
            {
                break record;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        captured_tls.request.url,
        format!("https://localhost:{tls_port}/socks5-mitm")
    );
    proxy_tls.stop().await.unwrap();
    origin_tls.stop().await;
}
