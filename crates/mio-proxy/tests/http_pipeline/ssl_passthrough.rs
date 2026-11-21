use super::*;

fn custom_origin_tls() -> (Vec<u8>, Vec<u8>, TlsAcceptor) {
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let ca_der = ca.self_signed(&ca_key).unwrap().der().to_vec();
    let issuer = Issuer::new(ca, ca_key);

    let leaf_params = CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
    let leaf_der = leaf_cert.der().to_vec();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut server_cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(leaf_der.clone())],
            PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    (ca_der, leaf_der, TlsAcceptor::from(Arc::new(server_cfg)))
}

#[tokio::test]
async fn ssl_passthrough_delivers_native_origin_certificate_and_stop_proxy_interrupts_active_tunnel()
 {
    let (origin_ca_der, expected_leaf_der, acceptor) = custom_origin_tls();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_addr = listener.local_addr().unwrap();
    let origin_port = origin_addr.port();
    let (peer_closed_tx, peer_closed_rx) = oneshot::channel();

    let origin_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.unwrap();
        let mut req_buf = Vec::new();
        while !req_buf.ends_with(b"\r\n\r\n") {
            assert!(req_buf.len() < 4096);
            req_buf.push(tls.read_u8().await.unwrap());
        }
        assert!(req_buf.starts_with(b"GET /pinned HTTP/1.1\r\n"));
        tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\npinned-ok!")
            .await
            .unwrap();
        tls.flush().await.unwrap();
        // Keep the tunnel alive until StopProxy tears down the TCP stream.
        let mut byte = [0u8; 1];
        let eof = tls.read(&mut byte).await;
        let _ = peer_closed_tx.send(matches!(eof, Ok(0) | Err(_)));
    });

    let mitm_ca = MitmAuthority::generate().unwrap();
    let mitm_ca_der = mitm_ca.certificate_der();
    // Intentionally leave proxy upstream_tls on System (not trusting origin_ca_der) to prove
    // the proxy never terminates or re-handshakes TLS when SSL Pass Through matches!
    let mut config = ProxyConfig::new(mitm_ca);
    config.limits.request_timeout = DEADLINE;
    config.hooks.ssl_pass_through = Some(Arc::new(move |host, port| {
        host == "127.0.0.1" && port == origin_port
    }));
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let mut proxy = mio_proxy::start(
        config,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events_tx,
    )
    .await
    .unwrap();

    let authority = format!("127.0.0.1:{origin_port}");
    let tunnel = tokio::time::timeout(DEADLINE, connect_tunnel(proxy.address(), &authority))
        .await
        .unwrap();
    tunnel.set_nodelay(true).unwrap();

    // Client trusts ONLY origin_ca_der (and NOT mitm_ca_der).
    let connector = tls_connector(origin_ca_der, false);
    let mut client_tls = tokio::time::timeout(
        DEADLINE,
        connector.connect(
            ServerName::from(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)),
            tunnel,
        ),
    )
    .await
    .unwrap()
    .expect("native TLS handshake directly with origin must succeed");

    let peer_certs = client_tls
        .get_ref()
        .1
        .peer_certificates()
        .expect("origin certificate chain must be present");
    assert_eq!(peer_certs[0].as_ref(), expected_leaf_der.as_slice());
    assert_ne!(peer_certs[0].as_ref(), mitm_ca_der.as_slice());

    client_tls
        .write_all(b"GET /pinned HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    client_tls.flush().await.unwrap();

    let mut resp_head = Vec::new();
    while !resp_head.ends_with(b"\r\n\r\n") {
        resp_head.push(
            tokio::time::timeout(DEADLINE, client_tls.read_u8())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let mut body = [0u8; 10];
    tokio::time::timeout(DEADLINE, client_tls.read_exact(&mut body))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&body, b"pinned-ok!");
    assert!(
        events_rx.try_recv().is_err(),
        "opaque SSL Pass Through tunnel must not synthesize decrypted HTTP history"
    );

    // Verify StopProxy immediately interrupts the active pass-through tunnel and reclaims tasks.
    tokio::time::timeout(DEADLINE, proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(proxy.is_joined());

    let mut probe = [0u8; 1];
    let client_eof = tokio::time::timeout(DEADLINE, client_tls.read(&mut probe))
        .await
        .unwrap();
    assert!(matches!(client_eof, Ok(0) | Err(_)));
    assert!(
        tokio::time::timeout(DEADLINE, peer_closed_rx)
            .await
            .unwrap()
            .unwrap()
    );
    tokio::time::timeout(DEADLINE, origin_task)
        .await
        .unwrap()
        .unwrap();
}
