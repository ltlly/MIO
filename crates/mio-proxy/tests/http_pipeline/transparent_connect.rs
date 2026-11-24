use super::*;
use std::net::IpAddr;

async fn connect_http10_tunnel(address: SocketAddr, authority: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(format!("CONNECT {authority} HTTP/1.0\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 8192);
        header.push(
            tokio::time::timeout(DEADLINE, stream.read_u8())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(
        header.starts_with(b"HTTP/1.1 200") || header.starts_with(b"HTTP/1.0 200"),
        "unexpected CONNECT response: {}",
        String::from_utf8_lossy(&header)
    );
    stream
}

async fn socks5_connect_ipv4(proxy_addr: SocketAddr, ip: Ipv4Addr, port: u16) -> TcpStream {
    let mut stream = TcpStream::connect(proxy_addr).await.unwrap();
    stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut method_reply = [0u8; 2];
    stream.read_exact(&mut method_reply).await.unwrap();
    assert_eq!(method_reply, [0x05, 0x00]);

    let mut req = vec![0x05, 0x01, 0x00, 0x01];
    req.extend_from_slice(&ip.octets());
    req.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&req).await.unwrap();

    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply[..2], &[0x05, 0x00]);
    stream
}

async fn next_complete_flow(events: &mut mpsc::Receiver<ProxyEvent>) -> mio_plugin_api::FlowRecord {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap()
                && record.summary.state == FlowState::Complete
            {
                return record;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn ip_connect_signs_leaf_from_sni_and_captures_sni_url() {
    let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
    let (mut proxy, ca, mut events) = proxy(&origin, false).await;
    let tls_port = origin.address.port();
    let ip_authority = format!("127.0.0.1:{tls_port}");

    let tunnel = connect_http10_tunnel(proxy.address(), &ip_authority).await;
    let mut tls = tokio::time::timeout(
        DEADLINE,
        tls_connector(ca, false).connect(ServerName::try_from("localhost").unwrap(), tunnel),
    )
    .await
    .unwrap()
    .expect("proxy must issue SNI-matching leaf cert when CONNECT target is an IP");
    tls.write_all(b"GET /sni-ip HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = Vec::new();
    tokio::time::timeout(DEADLINE, tls.read_to_end(&mut resp))
        .await
        .unwrap()
        .unwrap();
    assert!(resp.starts_with(b"HTTP/1.1 200"));

    let record = next_complete_flow(&mut events).await;
    assert_eq!(record.request.url, format!("https://localhost:{tls_port}/sni-ip"));

    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn ip_connect_without_sni_signs_ip_cert_and_captures_ip_url() {
    let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
    let (mut proxy, ca, mut events) = proxy(&origin, false).await;
    let tls_port = origin.address.port();
    let ip_authority = format!("127.0.0.1:{tls_port}");

    let tunnel = connect_http10_tunnel(proxy.address(), &ip_authority).await;
    let mut tls = tokio::time::timeout(
        DEADLINE,
        tls_connector(ca, false).connect(ServerName::from(IpAddr::V4(Ipv4Addr::LOCALHOST)), tunnel),
    )
    .await
    .unwrap()
    .expect("proxy must fall back to IP leaf cert when ClientHello has no SNI");
    tls.write_all(
        format!("GET /no-sni-ip HTTP/1.1\r\nHost: {ip_authority}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    tokio::time::timeout(DEADLINE, tls.read_to_end(&mut resp))
        .await
        .unwrap()
        .unwrap();
    assert!(resp.starts_with(b"HTTP/1.1 200"));

    let record = next_complete_flow(&mut events).await;
    assert_eq!(
        record.request.url,
        format!("https://127.0.0.1:{tls_port}/no-sni-ip")
    );

    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn ip_connect_routes_plaintext_http_to_connect_ip_preserving_host_header() {
    let mut origin = Origin::start(None, false).await;
    let (mut proxy, _, mut events) = proxy(&origin, false).await;
    let http_port = origin.address.port();
    let ip_authority = format!("127.0.0.1:{http_port}");

    let mut tunnel = connect_http10_tunnel(proxy.address(), &ip_authority).await;
    tunnel
        .write_all(
            b"GET /plain-via-connect HTTP/1.1\r\nHost: lab-plain.invalid\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut resp = Vec::new();
    tokio::time::timeout(DEADLINE, tunnel.read_to_end(&mut resp))
        .await
        .unwrap()
        .unwrap();
    assert!(
        resp.starts_with(b"HTTP/1.1 200"),
        "expected 200 OK, got: {}",
        String::from_utf8_lossy(&resp)
    );

    let OriginEvent::Request { headers, .. } = origin.next().await else {
        panic!("expected origin request");
    };
    let host_header = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, val)| String::from_utf8_lossy(val).into_owned());
    assert_eq!(host_header.as_deref(), Some("lab-plain.invalid"));

    let record = next_complete_flow(&mut events).await;
    assert_eq!(
        record.request.url,
        format!("http://127.0.0.1:{http_port}/plain-via-connect")
    );

    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn ip_connect_sni_triggers_ssl_passthrough_for_matching_sni() {
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let ca_key = KeyPair::generate().unwrap();
    let origin_ca_der = ca.self_signed(&ca_key).unwrap().der().to_vec();
    let issuer = Issuer::new(ca, ca_key);

    let leaf_params = CertificateParams::new(vec!["pinned.example.invalid".into()]).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let expected_leaf_der = leaf_params
        .signed_by(&leaf_key, &issuer)
        .unwrap()
        .der()
        .to_vec();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut server_cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(expected_leaf_der.clone())],
            PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_addr = listener.local_addr().unwrap();
    let origin_port = origin_addr.port();
    let origin_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.unwrap();
        let mut req_buf = Vec::new();
        while !req_buf.ends_with(b"\r\n\r\n") {
            assert!(req_buf.len() < 4096);
            req_buf.push(tls.read_u8().await.unwrap());
        }
        assert!(req_buf.starts_with(b"GET /pinned HTTP/1.1\r\n"));
        tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\npinned-ok!")
            .await
            .unwrap();
        tls.flush().await.unwrap();
        let _ = tls.shutdown().await;
    });

    let mitm_ca = MitmAuthority::generate().unwrap();
    let mitm_ca_der = mitm_ca.certificate_der();
    let mut config = ProxyConfig::new(mitm_ca);
    config.limits.request_timeout = DEADLINE;
    config.network.hostname_overrides = Arc::from([(
        String::from("pinned.example.invalid"),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )]);
    config.hooks.ssl_pass_through = Some(Arc::new(move |host, port| {
        host.eq_ignore_ascii_case("pinned.example.invalid") && port == origin_port
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

    let tunnel = connect_http10_tunnel(proxy.address(), &format!("127.0.0.1:{origin_port}")).await;
    let mut client_tls = tokio::time::timeout(
        DEADLINE,
        tls_connector(origin_ca_der, false)
            .connect(ServerName::try_from("pinned.example.invalid").unwrap(), tunnel),
    )
    .await
    .unwrap()
    .expect("SNI-matched passthrough tunnel must deliver origin certificate");
    let peer_certs = client_tls.get_ref().1.peer_certificates().unwrap();
    assert_eq!(peer_certs[0].as_ref(), expected_leaf_der.as_slice());
    assert_ne!(peer_certs[0].as_ref(), mitm_ca_der.as_slice());

    client_tls
        .write_all(b"GET /pinned HTTP/1.1\r\nHost: pinned.example.invalid\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = Vec::new();
    tokio::time::timeout(DEADLINE, client_tls.read_to_end(&mut resp))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&resp);
    assert!(text.starts_with("HTTP/1.1 200 OK"));
    assert!(text.ends_with("pinned-ok!"));
    assert!(events_rx.try_recv().is_err());

    proxy.stop().await.unwrap();
    tokio::time::timeout(DEADLINE, origin_task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn ip_connect_rejects_raw_binary_as_tls_and_ignores_zero_byte_close() {
    let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
    let (mut proxy, _, mut events) = proxy(&origin, false).await;
    let authority = format!("127.0.0.1:{}", origin.address.port());

    // 1. Clean 0-byte close emits no failed CONNECT record.
    let empty_tunnel = connect_http10_tunnel(proxy.address(), &authority).await;
    drop(empty_tunnel);

    // 2. Non-HTTP raw binary is handed to TLS acceptor (not plaintext HTTP), failing cleanly.
    let mut raw_tunnel = connect_http10_tunnel(proxy.address(), &authority).await;
    raw_tunnel
        .write_all(b"\x01\x02\x03\x04RAW_BINARY_PROBE\n")
        .await
        .unwrap();
    let mut reply = Vec::new();
    let _ = tokio::time::timeout(DEADLINE, raw_tunnel.read_to_end(&mut reply))
        .await
        .unwrap();

    proxy.stop().await.unwrap();
    assert_eq!(origin.received.load(Ordering::SeqCst), 0);
    origin.stop().await;

    let mut failed_connects = Vec::new();
    while let Ok(ev) = events.try_recv() {
        if let ProxyEvent::Captured { record, .. } = ev
            && record.summary.state != FlowState::Pending
        {
            failed_connects.push((record.summary.method, record.summary.state));
        }
    }
    assert_eq!(failed_connects.len(), 1, "got: {failed_connects:?}");
    assert_eq!(failed_connects[0].0, "CONNECT");
    assert!(matches!(failed_connects[0].1, FlowState::Failed(_)));
}

#[tokio::test]
async fn socks5_ipv4_uses_sni_for_tls_and_preserves_host_on_plaintext_http() {
    // 1. Plaintext HTTP over SOCKS5 IPv4 tunnel preserving unresolvable Host header
    let mut origin_http = Origin::start(None, false).await;
    let (mut proxy_http, _, mut events_http) = proxy(&origin_http, false).await;
    let http_port = origin_http.address.port();

    let mut plain_sock =
        socks5_connect_ipv4(proxy_http.address(), Ipv4Addr::LOCALHOST, http_port).await;
    plain_sock
        .write_all(
            b"GET /socks5-ip-plain HTTP/1.1\r\nHost: lab-plain.invalid\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut plain_resp = Vec::new();
    tokio::time::timeout(DEADLINE, plain_sock.read_to_end(&mut plain_resp))
        .await
        .unwrap()
        .unwrap();
    assert!(plain_resp.starts_with(b"HTTP/1.1 200"));
    let OriginEvent::Request { headers, .. } = origin_http.next().await else {
        panic!("expected origin request");
    };
    let host_hdr = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, val)| String::from_utf8_lossy(val).into_owned());
    assert_eq!(host_hdr.as_deref(), Some("lab-plain.invalid"));
    let plain_record = next_complete_flow(&mut events_http).await;
    assert_eq!(
        plain_record.request.url,
        format!("http://127.0.0.1:{http_port}/socks5-ip-plain")
    );
    proxy_http.stop().await.unwrap();
    origin_http.stop().await;

    // 2. HTTPS MITM over SOCKS5 IPv4 tunnel using SNI for leaf certificate and URL
    let origin_tls = Origin::start(Some((CertificateKind::Valid, false)), false).await;
    let (mut proxy_tls, ca_der, mut events_tls) = proxy(&origin_tls, false).await;
    let tls_port = origin_tls.address.port();

    let tls_sock = socks5_connect_ipv4(proxy_tls.address(), Ipv4Addr::LOCALHOST, tls_port).await;
    let mut tls_conn = tokio::time::timeout(
        DEADLINE,
        tls_connector(ca_der, false).connect(ServerName::try_from("localhost").unwrap(), tls_sock),
    )
    .await
    .unwrap()
    .unwrap();
    tls_conn
        .write_all(b"GET /socks5-ip-sni HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut tls_resp = Vec::new();
    tokio::time::timeout(DEADLINE, tls_conn.read_to_end(&mut tls_resp))
        .await
        .unwrap()
        .unwrap();
    assert!(tls_resp.starts_with(b"HTTP/1.1 200"));
    let tls_record = next_complete_flow(&mut events_tls).await;
    assert_eq!(
        tls_record.request.url,
        format!("https://localhost:{tls_port}/socks5-ip-sni")
    );
    proxy_tls.stop().await.unwrap();
    origin_tls.stop().await;
}

#[tokio::test]
async fn fragmented_client_hello_over_ip_connect_still_matches_ssl_passthrough_by_sni() {
    use tokio_rustls::TlsAcceptor;

    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
    let ca_key = KeyPair::generate().unwrap();
    let origin_ca_der = ca.self_signed(&ca_key).unwrap().der().to_vec();
    let issuer = Issuer::new(ca, ca_key);

    let leaf_params = CertificateParams::new(vec!["frag-pinned.example.invalid".into()]).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let expected_leaf_der = leaf_params
        .signed_by(&leaf_key, &issuer)
        .unwrap()
        .der()
        .to_vec();

    let mut server_cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(expected_leaf_der)],
        PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
    )
    .unwrap();
    server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_port = listener.local_addr().unwrap().port();
    let origin_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.unwrap();
        let mut req_buf = Vec::new();
        while !req_buf.ends_with(b"\r\n\r\n") {
            assert!(req_buf.len() < 4096);
            req_buf.push(tls.read_u8().await.unwrap());
        }
        tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfrag-ok")
            .await
            .unwrap();
        let _ = tls.shutdown().await;
    });

    let mitm_ca = MitmAuthority::generate().unwrap();
    let mut config = ProxyConfig::new(mitm_ca);
    config.limits.request_timeout = DEADLINE;
    config.network.hostname_overrides = Arc::from([(
        String::from("frag-pinned.example.invalid"),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )]);
    config.hooks.ssl_pass_through = Some(Arc::new(move |host, port| {
        host.eq_ignore_ascii_case("frag-pinned.example.invalid") && port == origin_port
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

    // Build ClientHello first and split it into two delayed writes before SNI bytes arrive.
    let client_cfg = {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(origin_ca_der))
            .unwrap();
        let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(cfg)
    };
    let mut client_conn = rustls::ClientConnection::new(
        client_cfg,
        ServerName::try_from("frag-pinned.example.invalid").unwrap(),
    )
    .unwrap();
    let mut hello = Vec::new();
    while client_conn.wants_write() {
        client_conn.write_tls(&mut hello).unwrap();
    }
    assert!(hello.len() > 20);

    let mut tunnel =
        connect_http10_tunnel(proxy.address(), &format!("127.0.0.1:{origin_port}")).await;
    tunnel.write_all(&hello[..12]).await.unwrap();
    tunnel.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    tunnel.write_all(&hello[12..]).await.unwrap();
    tunnel.flush().await.unwrap();

    let mut tls_buf = [0u8; 8192];
    let mut app_done = false;
    while !app_done {
        let n = tokio::time::timeout(DEADLINE, tunnel.read(&mut tls_buf))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0, "EOF before TLS handshake/response");
        let mut rd = std::io::Cursor::new(&tls_buf[..n]);
        client_conn.read_tls(&mut rd).unwrap();
        let state = client_conn.process_new_packets().unwrap();
        if !client_conn.is_handshaking() && client_conn.wants_write() {
            let mut out = Vec::new();
            client_conn.write_tls(&mut out).unwrap();
            tunnel.write_all(&out).await.unwrap();
        }
        if !client_conn.is_handshaking() {
            use std::io::{Read as _, Write as _};
            client_conn
                .writer()
                .write_all(
                    b"GET /frag HTTP/1.1\r\nHost: frag-pinned.example.invalid\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let mut out = Vec::new();
            while client_conn.wants_write() {
                client_conn.write_tls(&mut out).unwrap();
            }
            if !out.is_empty() {
                tunnel.write_all(&out).await.unwrap();
            }
            if state.plaintext_bytes_to_read() > 0 {
                let mut plain = vec![0u8; state.plaintext_bytes_to_read()];
                client_conn.reader().read_exact(&mut plain).unwrap();
                if String::from_utf8_lossy(&plain).contains("frag-ok") {
                    app_done = true;
                }
            }
        }
    }
    assert!(events_rx.try_recv().is_err());

    proxy.stop().await.unwrap();
    tokio::time::timeout(DEADLINE, origin_task)
        .await
        .unwrap()
        .unwrap();
}
