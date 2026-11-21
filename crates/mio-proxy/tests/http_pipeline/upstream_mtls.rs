use super::*;

#[tokio::test]
async fn mtls_origin_rejects_unauthenticated_and_succeeds_via_send_and_proxy() {
    let mut server_ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    server_ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    server_ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let server_ca_key = KeyPair::generate().unwrap();
    let server_ca_der = server_ca
        .self_signed(&server_ca_key)
        .unwrap()
        .der()
        .to_vec();
    let server_issuer = Issuer::new(server_ca, server_ca_key);
    let server_leaf_params =
        CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let server_leaf_key = KeyPair::generate().unwrap();
    let server_leaf = server_leaf_params
        .signed_by(&server_leaf_key, &server_issuer)
        .unwrap();

    let mut client_ca = CertificateParams::default();
    client_ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    client_ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut client_ca_dn = rcgen::DistinguishedName::new();
    client_ca_dn.push(rcgen::DnType::CommonName, "MIO Test Client CA");
    client_ca.distinguished_name = client_ca_dn;
    let client_ca_key = KeyPair::generate().unwrap();
    let client_ca_cert = client_ca.self_signed(&client_ca_key).unwrap();
    let client_issuer = Issuer::new(client_ca, client_ca_key);

    let mut client_leaf_params = CertificateParams::new(vec!["client.example.com".into()]).unwrap();
    client_leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    client_leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let mut client_dn = rcgen::DistinguishedName::new();
    client_dn.push(rcgen::DnType::CommonName, "mtls-client");
    client_leaf_params.distinguished_name = client_dn;
    let client_key = KeyPair::generate().unwrap();
    let client_leaf = client_leaf_params
        .signed_by(&client_key, &client_issuer)
        .unwrap();

    let mut client_roots = RootCertStore::empty();
    client_roots.add(client_ca_cert.der().clone()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(client_roots),
        provider.clone(),
    )
    .build()
    .unwrap();
    let mut server_config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server_leaf.der().clone()],
            PrivateKeyDer::try_from(server_leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    server_config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let server_cancel = CancellationToken::new();
    let server_task_cancel = server_cancel.clone();
    let server_task = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = server_task_cancel.cancelled() => break,
                Some(res) = tasks.join_next(), if !tasks.is_empty() => { res.unwrap(); },
                accepted = listener.accept() => {
                    let (stream, _) = accepted.unwrap();
                    let acceptor = acceptor.clone();
                    tasks.spawn(async move {
                        if let Ok(tls_stream) = acceptor.accept(stream).await {
                            let service = service_fn(|req: Request<Incoming>| async move {
                                let _ = req.into_body().collect().await;
                                Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"mtls-ok"))))
                            });
                            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
                            let _ = builder.serve_connection(TokioIo::new(tls_stream), service).await;
                        }
                    });
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    });

    let upstream_tls = UpstreamTls::CustomRoots {
        certificates_der: vec![server_ca_der],
        include_system: false,
    };
    let url = format!("https://localhost:{}/mtls", origin_addr.port());
    let req = HttpRequest {
        method: "GET".into(),
        url: url.clone(),
        version: HttpVersion::Http11,
        headers: Vec::new(),
        body: Vec::new(),
        trailers: Vec::new(),
    };
    let mut unauth_send_cfg = SendConfig::default();
    unauth_send_cfg.tls.trust = upstream_tls.clone();
    unauth_send_cfg.limits.timeout = DEADLINE;
    assert!(
        mio_proxy::send(req.clone(), unauth_send_cfg, CancellationToken::new())
            .await
            .is_err(),
        "unauthenticated client must fail mTLS handshake"
    );

    let rule = ClientCertificateRule {
        enabled: true,
        host_pattern: "localhost".into(),
        format: ClientCertificateFormat::Pem,
        certificate_bytes: client_leaf.pem().into_bytes(),
        private_key_pem: client_key.serialize_pem().into_bytes(),
        password: String::new(),
    };
    let identity = ParsedClientIdentity::parse(&rule).unwrap();
    let client_certs: Arc<[(ClientCertificateRule, ParsedClientIdentity)]> =
        Arc::from([(rule, identity)]);

    let mut direct_send_cfg = SendConfig::default();
    direct_send_cfg.tls.trust = upstream_tls.clone();
    direct_send_cfg.limits.timeout = DEADLINE;
    direct_send_cfg.tls.client_certificates = client_certs.clone();
    let direct_ok = mio_proxy::send(req, direct_send_cfg, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(direct_ok.status, 200);
    assert_eq!(direct_ok.body, b"mtls-ok");

    let ca = MitmAuthority::generate().unwrap();
    let ca_der = ca.certificate_der();
    let mut unauth_cfg = ProxyConfig::new(ca.clone());
    unauth_cfg.tls.upstream.trust = upstream_tls.clone();
    unauth_cfg.limits.request_timeout = DEADLINE;
    let (unauth_tx, _unauth_rx) = mpsc::channel(16);
    let mut unauth_proxy = mio_proxy::start(
        unauth_cfg,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        unauth_tx,
    )
    .await
    .unwrap();
    let unauth_resp = client(
        unauth_proxy.address(),
        url.clone(),
        ca_der.clone(),
        true,
        false,
    )
    .await;
    assert_ne!(unauth_resp.status, 200);
    unauth_proxy.stop().await.unwrap();

    let mut mtls_cfg = ProxyConfig::new(ca);
    mtls_cfg.tls.upstream.trust = upstream_tls;
    mtls_cfg.limits.request_timeout = DEADLINE;
    mtls_cfg.tls.upstream.client_certificates = client_certs;
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let mut mtls_proxy = mio_proxy::start(
        mtls_cfg,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events_tx,
    )
    .await
    .unwrap();
    let proxy_resp = client(mtls_proxy.address(), url, ca_der, true, false).await;
    assert_eq!(proxy_resp.status, 200);
    assert_eq!(proxy_resp.body, b"mtls-ok");
    if let Some(ProxyEvent::Captured { listener_port, .. }) = events_rx.recv().await {
        assert_eq!(listener_port, Some(mtls_proxy.address().port()));
    }
    mtls_proxy.stop().await.unwrap();
    server_cancel.cancel();
    tokio::time::timeout(DEADLINE, server_task)
        .await
        .unwrap()
        .unwrap();
}
