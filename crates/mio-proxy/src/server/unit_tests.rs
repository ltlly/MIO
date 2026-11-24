use super::*;
use bytes::Bytes;
use http_body_util::Full;
use hyper_util::rt::{TokioExecutor, TokioIo};
use mio_plugin_api::HttpVersion;
use std::convert::Infallible;
use std::net::Ipv4Addr;
use tokio::sync::mpsc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[tokio::test]
async fn repeated_h2_resets_cannot_accumulate_unbounded_finalizers() {
    let deadline = Duration::from_secs(2);
    let origin = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("http://{}/reset", origin.local_addr().unwrap());
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let (events, receiver) = mpsc::channel(1);
    events
        .try_send(ProxyEvent::Captured {
            project,
            record: new_record(HttpRequest {
                method: "GET".into(),
                url: url.clone(),
                version: HttpVersion::Http2,
                headers: vec![],
                body: vec![],
                trailers: vec![],
            }),
            response_stream: None,
            listener_port: None,
            capture_metrics: CapturedFlowMetrics::default(),
        })
        .unwrap();
    let cancel = CancellationToken::new();
    let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
    let tasks = TaskTracker::new();
    let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    config.limits.max_connections = 1;
    let (static_config, dynamic_init, _) = config.into_parts();
    let (dynamic_tx, dynamic) = watch::channel(dynamic_init);
    let state = Arc::new(State {
        project,
        listener_port: 1,
        config: static_config,
        events,
        cancel: cancel.clone(),
        dynamic,
        tasks: tasks.clone(),
        requests: Arc::new(Semaphore::new(1)),
        exec: Executor::graceful(shutdown.guard()),
        terminal_records: TerminalRecords::default(),
        injectors: WebSocketInjectorRegistry::new(),
    });
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_state = state.clone();
    let (entered, mut entries) = mpsc::channel(1);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut server = HttpServer::auto(server_state.exec.clone());
        server.h2_mut().set_max_concurrent_streams(1);
        let service_state = server_state.clone();
        let service = service_fn(move |request: Request| {
            let state = service_state.clone();
            let entered = entered.clone();
            async move {
                let finished = CancellationToken::new();
                let _finished = finished.clone().drop_guard();
                entered.send(finished).await.unwrap();
                Ok::<_, Infallible>(handle_http(request, state, None).await)
            }
        });
        tokio::select! {
            _ = server_state.cancel.cancelled() => {},
            _ = server.serve(TcpStream::new(stream), service) => {},
        }
    });
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let (sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
            .await
            .unwrap();
    let driver = tokio::spawn(connection);
    for _ in 0..4 {
        let mut sender = sender.clone();
        let request = hyper::Request::builder()
            .uri(&url)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let request = tokio::spawn(async move { sender.send_request(request).await });
        let finished = tokio::time::timeout(deadline, entries.recv())
            .await
            .unwrap()
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        tokio::time::timeout(deadline, finished.cancelled())
            .await
            .expect("the actual H2 reset must drop its server handler");
    }
    assert_eq!(
        receiver.len(),
        1,
        "the consumer never drains the saturated event queue"
    );
    let finalizers = tasks.len();
    let available = state.requests.available_permits();
    let requests = state.requests.clone();
    let terminal_records = state.terminal_records.clone();
    cancel.cancel();
    tasks.close();
    tokio::time::timeout(deadline, tasks.wait()).await.unwrap();
    assert_eq!(terminal_records.lock().unwrap().len(), 1);
    assert_eq!(requests.available_permits(), 0);
    tokio::time::timeout(deadline, server)
        .await
        .unwrap()
        .unwrap();
    drop(state);
    tokio::time::timeout(deadline, shutdown.shutdown())
        .await
        .unwrap();
    driver.abort();
    let _ = driver.await;
    assert_eq!(
        finalizers, 1,
        "a reset must retain its request budget until the terminal event is consumed or cancelled"
    );
    assert_eq!(
        available, 0,
        "queued terminal records still own their request budget"
    );
    let mut handle = ProxyHandle {
        address,
        cancel,
        dynamic: dynamic_tx,
        runner: None,
        stop_timeout: deadline,
        terminal_records,
        injectors: WebSocketInjectorRegistry::new(),
    };
    let retained = handle.drain_terminal_records();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].record.summary.state, FlowState::Cancelled);
    assert_eq!(requests.available_permits(), 1);
}

#[test]
fn listener_target_overrides_and_proxy_misc_transforms_work() {
    use mio_plugin_api::Header;

    let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    config.network.redirect_to_host = Some("127.0.0.1".into());
    config.network.redirect_to_port = Some(9443);
    config.tls.force_use_of_tls = true;
    config.http.strip_proxy_headers = true;
    config.http.remove_unsupported_accept_encodings = true;
    config.http.remove_100_continue_headers = true;
    config.http.use_http10_in_requests = true;
    config.http.set_request_connection_header = true;
    let (static_config, _, _) = config.into_parts();

    let mut req = HttpRequest {
        method: "POST".into(),
        url: "http://example.com:8080/v1/items?a=1".into(),
        version: HttpVersion::Http11,
        headers: vec![
            Header {
                name: "Host".into(),
                value: b"example.com:8080".to_vec(),
            },
            Header {
                name: "Proxy-Connection".into(),
                value: b"keep-alive".to_vec(),
            },
            Header {
                name: "Proxy-Authorization".into(),
                value: b"Basic dGVzdDp0ZXN0".to_vec(),
            },
            Header {
                name: "Accept-Encoding".into(),
                value: b"gzip, zstd, br;q=0.9, compress".to_vec(),
            },
            Header {
                name: "Expect".into(),
                value: b"100-continue".to_vec(),
            },
        ],
        body: b"hi".to_vec(),
        trailers: Vec::new(),
    };

    listener::apply_listener_target_overrides(&mut req, &static_config).unwrap();
    assert_eq!(req.url, "https://127.0.0.1:9443/v1/items?a=1");

    listener::apply_proxy_misc_request_transforms(&mut req, &static_config, false);
    assert_eq!(req.version, HttpVersion::Http10);
    assert!(!req.headers.iter().any(|h| h.name.to_ascii_lowercase().starts_with("proxy-")));
    assert!(!req.headers.iter().any(|h| h.name.eq_ignore_ascii_case("expect")));
    let ae = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("accept-encoding"))
        .unwrap();
    assert_eq!(ae.value, b"gzip, br;q=0.9");
    let conn = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("connection"))
        .unwrap();
    assert_eq!(conn.value, b"close");

    // Verify http1_keep_alive + set_request_connection_header preserves Connection: keep-alive outbound
    let mut keepalive_cfg = ProxyConfig::new(MitmAuthority::generate().unwrap());
    keepalive_cfg.http.http1_keep_alive = true;
    keepalive_cfg.http.set_request_connection_header = true;
    let (static_keepalive_cfg, _, _) = keepalive_cfg.into_parts();
    let mut ka_req = HttpRequest {
        method: "GET".into(),
        url: "http://example.com/keep".into(),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "Host".into(),
            value: b"example.com".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    listener::apply_proxy_misc_request_transforms(&mut ka_req, &static_keepalive_cfg, false);
    assert!(ka_req.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("connection") && h.value.eq_ignore_ascii_case(b"keep-alive")
    }));
    let mut send_cfg = crate::SendConfig::default();
    send_cfg.http.http1_keep_alive = true;
    send_cfg.http.set_connection_keep_alive = true;
    let wire_req = crate::message::to_request_with_config(&ka_req, &send_cfg).unwrap();
    assert_eq!(
        wire_req.headers().get("connection").and_then(|v| v.to_str().ok()),
        Some("keep-alive")
    );
}

#[tokio::test]
async fn proxy_misc_invisible_drop_scope_web_interface_and_suppress_errors() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Origin server that echoes request line and Host header.
    let origin = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        let mut count = 0usize;
        while let Ok((mut stream, _)) = origin.accept().await {
            count += 1;
            let mut buf = Vec::new();
            let mut tmp = [0u8; 512];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body = String::from_utf8_lossy(&buf).into_owned();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            if count >= 1 {
                break;
            }
        }
    });

    let ca = MitmAuthority::generate().unwrap();
    let mut config = ProxyConfig::new(ca);
    config.network.invisible_proxying = true;
    config.http.disable_web_interface = true;
    config.http.suppress_browser_error_messages = true;
    config.http.use_http10_in_responses = true;
    config.http.set_response_connection_close = true;
    config.hooks.drop_out_of_scope_fn = Some(Arc::new(|url: &str| url.contains("/blocked")));

    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let (events_tx, _events_rx) = mpsc::channel(64);
    let mut handle = start(config, project, events_tx).await.unwrap();

    // 1. Out-of-scope URL dropped before connecting upstream with suppressed empty 502.
    {
        let mut client = tokio::net::TcpStream::connect(handle.address()).await.unwrap();
        let req = format!(
            "GET /blocked HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 502 "), "got: {text}");
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
        assert!(body.is_empty(), "suppressed error must return empty body: {text}");
    }

    // 2. Origin-form invisible proxying succeeds and applies HTTP/1.0 + Connection: close.
    {
        let mut client = tokio::net::TcpStream::connect(handle.address()).await.unwrap();
        let req = format!(
            "GET /visible HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.0 200 OK\r\n"), "got: {text}");
        assert!(
            text.to_ascii_lowercase().contains("connection: close"),
            "got: {text}"
        );
        assert!(text.contains("GET /visible HTTP/1.1"), "got: {text}");
    }

    handle.stop().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), origin_task).await;
}

#[test]
fn proxy_config_to_send_config_preserves_auth_tls_and_http2_for_http_and_websocket() {
    use crate::{
        AuthCredentials, HttpAuthScheme, PlatformAuthRuleConfig, ProxyEndpoint, UpstreamAuthRuleConfig,
        UpstreamRoute, UpstreamTls, UpstreamTlsProtocolMode,
    };
    use std::net::IpAddr;

    let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    let proxy_endpoint = ProxyEndpoint {
        host: "127.0.0.1".into(),
        port: 8899,
        credentials: None,
    };
    config.network.upstream_route = UpstreamRoute::Http(proxy_endpoint.clone());
    config.network.hostname_overrides =
        Arc::from([(String::from("app.lab.invalid"), IpAddr::from([127, 0, 0, 42]))]);
    config.tls.upstream.trust = UpstreamTls::Insecure;
    config.tls.upstream.options.protocol_mode = UpstreamTlsProtocolMode::Tls12Only;
    config.limits.max_body_bytes = 2048;
    config.limits.request_timeout = Duration::from_secs(9);
    config.limits.streaming_timeout = Some(Duration::from_secs(17));
    config.http.default_to_http2 = true;
    config.http.http1_keep_alive = false;
    config.http.use_connection_pool = true;
    config.http.strip_proxy_headers = false;
    config.auth.platform_enabled = true;
    config.auth.platform_rules = Arc::from([PlatformAuthRuleConfig {
        enabled: true,
        destination_host: "*.lab.invalid".into(),
        credentials: AuthCredentials {
            scheme: HttpAuthScheme::Basic,
            username: "platform-user".into(),
            password: "platform-secret".into(),
            ..Default::default()
        },
    }]);
    config.auth.upstream_rules = Arc::from([UpstreamAuthRuleConfig {
        enabled: true,
        destination_host: "*".into(),
        proxy_host: "127.0.0.1".into(),
        proxy_port: 8899,
        credentials: AuthCredentials {
            scheme: HttpAuthScheme::Digest,
            username: "upstream-user".into(),
            password: "upstream-secret".into(),
            ..Default::default()
        },
    }]);

    let http_send = config.to_send_config("https://app.lab.invalid/api", HttpVersion::Http2, false);
    assert!(
        matches!(&http_send.network.upstream_route, UpstreamRoute::Http(ep) if ep.host == proxy_endpoint.host && ep.port == proxy_endpoint.port)
    );
    assert_eq!(http_send.network.hostname_overrides.len(), 1);
    assert!(matches!(http_send.tls.trust, UpstreamTls::Insecure));
    assert_eq!(http_send.tls.options.protocol_mode, UpstreamTlsProtocolMode::Tls12Only);
    assert_eq!(http_send.limits.max_body_bytes, 2048);
    assert_eq!(http_send.limits.timeout, Duration::from_secs(9));
    assert_eq!(http_send.limits.streaming_timeout, Some(Duration::from_secs(17)));
    assert_eq!(http_send.http.version, None);
    assert!(http_send.http.default_to_http2);
    assert!(!http_send.http.http1_keep_alive);
    assert!(http_send.http.use_connection_pool);
    assert!(!http_send.http.strip_proxy_headers);
    assert!(http_send.auth.platform_enabled);
    assert_eq!(http_send.auth.platform_rules.len(), 1);
    assert_eq!(http_send.auth.upstream_rules.len(), 1);

    let ws_send = config.to_send_config("wss://app.lab.invalid/ws", HttpVersion::Http11, true);
    assert!(
        matches!(&ws_send.network.upstream_route, UpstreamRoute::Http(ep) if ep.host == proxy_endpoint.host && ep.port == proxy_endpoint.port)
    );
    assert_eq!(ws_send.network.hostname_overrides.len(), 1);
    assert!(matches!(ws_send.tls.trust, UpstreamTls::Insecure));
    assert_eq!(ws_send.tls.options.protocol_mode, UpstreamTlsProtocolMode::Tls12Only);
    assert!(ws_send.auth.platform_enabled);
    assert_eq!(ws_send.auth.platform_rules.len(), 1);
    assert_eq!(ws_send.auth.upstream_rules.len(), 1);
    assert_eq!(ws_send.http.version, Some(HttpVersion::Http11));
    assert!(ws_send.http.default_to_http2);
    assert!(ws_send.http.http1_keep_alive);

    config.http.default_to_http2 = false;
    let h1_fallback =
        config.to_send_config("https://app.lab.invalid/api", HttpVersion::Http11, false);
    assert_eq!(h1_fallback.http.version, Some(HttpVersion::Http11));
}

#[tokio::test]
async fn invisible_tls_respects_ssl_passthrough_for_matching_sni() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let origin = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        if let Ok((mut stream, _)) = origin.accept().await {
            let mut peek = [0u8; 256];
            let n = stream.read(&mut peek).await.unwrap_or(0);
            if n > 0 {
                let _ = stream.write_all(b"RAW-TLS-PASSTHROUGH-OK").await;
            }
        }
    });

    let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    config.network.listen = "0.0.0.0:0".parse().unwrap();
    config.network.invisible_proxying = true;
    config.network.redirect_to_port = Some(origin_addr.port());
    config.network.hostname_overrides =
        Arc::from([(String::from("pinned.example.invalid"), origin_addr.ip())]);
    config.hooks.ssl_pass_through = Some(Arc::new(|host, port| {
        host.eq_ignore_ascii_case("pinned.example.invalid") && port != 0
    }));

    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let mut handle = start(config, project, events_tx).await.unwrap();
    let connect_addr: SocketAddr = (Ipv4Addr::LOCALHOST, handle.address().port()).into();

    // Minimal synthetic TLS ClientHello containing SNI = pinned.example.invalid
    let sni = b"pinned.example.invalid";
    let sni_entry_len = (1 + 2 + sni.len()) as u16;
    let sni_ext_len = (2 + sni_entry_len as usize) as u16;
    let extensions_total = (4 + sni_ext_len as usize) as u16;
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]); // TLS 1.2
    body.extend_from_slice(&[0u8; 32]); // Random
    body.push(0); // Session ID len
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // 1 cipher suite
    body.extend_from_slice(&[0x01, 0x00]); // 1 compression method (null)
    body.extend_from_slice(&extensions_total.to_be_bytes());
    body.extend_from_slice(&[0x00, 0x00]); // server_name extension
    body.extend_from_slice(&sni_ext_len.to_be_bytes());
    body.extend_from_slice(&sni_entry_len.to_be_bytes());
    body.push(0x00); // host_name
    body.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    body.extend_from_slice(sni);

    let hello_len = body.len();
    let mut record = Vec::new();
    record.push(0x16);
    record.extend_from_slice(&[0x03, 0x01]);
    record.extend_from_slice(&((hello_len + 4) as u16).to_be_bytes());
    record.push(0x01);
    record.push(((hello_len >> 16) & 0xff) as u8);
    record.push(((hello_len >> 8) & 0xff) as u8);
    record.push((hello_len & 0xff) as u8);
    record.extend_from_slice(&body);

    let mut client = tokio::net::TcpStream::connect(connect_addr).await.unwrap();
    client.write_all(&record).await.unwrap();
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();
    assert_eq!(reply, b"RAW-TLS-PASSTHROUGH-OK");
    assert!(events_rx.try_recv().is_err(), "pass-through should not capture MITM flows");

    handle.stop().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), origin_task).await;
}

#[test]
fn tunnel_prefix_classifier_and_authority_formatter_handle_edge_cases() {
    assert!(listener::looks_like_plain_http_prefix(
        b"GET /p2a HTTP/1.1\r\nHost: lab-plain.invalid\r\n\r\n"
    ));
    assert!(listener::looks_like_plain_http_prefix(
        b"POST /submit HTTP/1.0\r\n"
    ));
    assert!(listener::looks_like_plain_http_prefix(b"PRI * HTTP/2.0\r\n"));
    assert!(listener::looks_like_plain_http_prefix(b"GET"));
    assert!(listener::looks_like_plain_http_prefix(b"OPTIONS "));

    assert!(!listener::looks_like_plain_http_prefix(b""));
    assert!(!listener::looks_like_plain_http_prefix(&[0x16, 0x03, 0x01]));
    assert!(!listener::looks_like_plain_http_prefix(b"\x01\x02\x03\x04RAW\n"));
    assert!(!listener::looks_like_plain_http_prefix(b"1234 /path"));
    assert!(!listener::looks_like_plain_http_prefix(b"GET \x00/path"));
    assert!(!listener::looks_like_plain_http_prefix(
        b"ABCDEFGHIJKLMNOPQ"
    ));

    assert_eq!(
        listener::format_tunnel_authority("localhost", 443, 443),
        "localhost"
    );
    assert_eq!(
        listener::format_tunnel_authority("localhost", 8443, 443),
        "localhost:8443"
    );
    assert_eq!(
        listener::format_tunnel_authority("127.0.0.1", 80, 80),
        "127.0.0.1"
    );
    assert_eq!(
        listener::format_tunnel_authority("[::1]", 8080, 80),
        "[::1]:8080"
    );
    assert_eq!(
        listener::format_tunnel_authority("::1", 443, 443),
        "[::1]"
    );
}

#[tokio::test]
async fn set_authority_hot_swaps_cert_portal_and_leaf_signer_without_rebind() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fetch_ca_der(addr: SocketAddr) -> Vec<u8> {
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET http://mio/cert HTTP/1.1\r\nHost: mio\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut raw = Vec::new();
        conn.read_to_end(&mut raw).await.unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        raw[split..].to_vec()
    }

    async fn connect_tunnel(proxy_addr: SocketAddr, origin_port: u16) -> tokio::net::TcpStream {
        let mut tcp = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
        tcp.write_all(
            format!("CONNECT 127.0.0.1:{origin_port} HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        let mut resp = [0u8; 512];
        let n = tcp.read(&mut resp).await.unwrap();
        assert!(std::str::from_utf8(&resp[..n]).unwrap().contains("200"));
        tcp
    }

    fn tls_connector_for_ca(ca: &MitmAuthority) -> tokio_rustls::TlsConnector {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots
            .add(rama::crypto::pki_types::CertificateDer::from(
                ca.certificate_der(),
            ))
            .unwrap();
        let tls_cfg = Arc::new(
            tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(
                tokio_rustls::rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
        );
        tokio_rustls::TlsConnector::from(tls_cfg)
    }

    let origin_cert = rcgen::generate_simple_self_signed(["lab.example.invalid".to_owned()]).unwrap();
    let origin_cert_der =
        tokio_rustls::rustls::pki_types::CertificateDer::from(origin_cert.cert.der().to_vec());
    let origin_key_der = tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(
        tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(
            origin_cert.signing_key.serialize_der(),
        ),
    );
    let origin_tls = Arc::new(
        tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![origin_cert_der], origin_key_der)
        .unwrap(),
    );
    let acceptor = tokio_rustls::TlsAcceptor::from(origin_tls);

    let origin = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        while let Ok((stream, _)) = origin.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buf = [0u8; 2048];
                if tls.read(&mut buf).await.unwrap_or(0) > 0 {
                    let _ = tls
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                        .await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });

    let ca1 = MitmAuthority::generate().unwrap();
    let ca2 = MitmAuthority::generate().unwrap();
    assert_ne!(ca1.certificate_der(), ca2.certificate_der());

    let mut config = ProxyConfig::new(ca1.clone());
    config.network.listen = "127.0.0.1:0".parse().unwrap();
    config.tls.upstream.trust = crate::tls::UpstreamTls::Insecure;
    config.network.hostname_overrides =
        Arc::from([(String::from("lab.example.invalid"), origin_addr.ip())]);
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let (events_tx, _events_rx) = mpsc::channel(16);
    let mut handle = start(config, project, events_tx).await.unwrap();
    let proxy_addr = handle.address();

    assert_eq!(fetch_ca_der(proxy_addr).await, ca1.certificate_der());

    handle.set_authority(ca2.clone());
    assert_eq!(handle.address(), proxy_addr, "hot swap must keep same port");
    assert_eq!(fetch_ca_der(proxy_addr).await, ca2.certificate_der());

    let server_name = rama::crypto::pki_types::ServerName::try_from("lab.example.invalid").unwrap();
    let tcp_old = connect_tunnel(proxy_addr, origin_addr.port()).await;
    assert!(
        tls_connector_for_ca(&ca1)
            .connect(server_name.clone(), tcp_old)
            .await
            .is_err(),
        "stale CA must no longer validate leaf after hot swap"
    );

    let tcp_new = connect_tunnel(proxy_addr, origin_addr.port()).await;
    let mut tls = tls_connector_for_ca(&ca2)
        .connect(server_name, tcp_new)
        .await
        .unwrap();
    tls.write_all(b"GET /hot HTTP/1.1\r\nHost: lab.example.invalid\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut body = Vec::new();
    let _ = tls.read_to_end(&mut body).await;
    assert!(std::str::from_utf8(&body).unwrap().contains("ok"));

    handle.stop().await.unwrap();
    origin_task.abort();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_proxy_bind_rejects_wildcard_specific_overlap_and_allows_time_wait() {
    use tokio::io::AsyncWriteExt as _;

    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };

    // Hold 0.0.0.0:<ephemeral>, then verify 127.0.0.1:<same port> is rejected.
    let mut wild_cfg = ProxyConfig::new(MitmAuthority::generate().unwrap());
    wild_cfg.network.listen = "0.0.0.0:0".parse().unwrap();
    let (tx1, _rx1) = mpsc::channel(4);
    let mut wild_handle = start(wild_cfg, project, tx1).await.unwrap();
    let shared_port = wild_handle.address().port();

    let mut loop_cfg = ProxyConfig::new(MitmAuthority::generate().unwrap());
    loop_cfg.network.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, shared_port));
    let (tx2, _rx2) = mpsc::channel(4);
    let Err(err) = start(loop_cfg, project, tx2).await else {
        panic!("expected loopback overlap with wildcard listener to fail");
    };
    assert!(
        matches!(err, ProxyError::Network(_)),
        "expected network AddrInUse error, got {err}"
    );
    wild_handle.stop().await.unwrap();

    // Hold 127.0.0.1:<port>, then verify 0.0.0.0:<same port> is rejected.
    let mut loop_cfg2 = ProxyConfig::new(MitmAuthority::generate().unwrap());
    loop_cfg2.network.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, shared_port));
    let (tx3, _rx3) = mpsc::channel(4);
    let mut loop_handle = start(loop_cfg2, project, tx3).await.unwrap();

    // Create an active client connection + close to leave a TIME_WAIT socket after stop().
    let mut cli = tokio::net::TcpStream::connect(loop_handle.address())
        .await
        .unwrap();
    let _ = cli.shutdown().await;
    drop(cli);

    let mut wild_cfg2 = ProxyConfig::new(MitmAuthority::generate().unwrap());
    wild_cfg2.network.listen = SocketAddr::from((Ipv4Addr::UNSPECIFIED, shared_port));
    let (tx4, _rx4) = mpsc::channel(4);
    let Err(err2) = start(wild_cfg2, project, tx4).await else {
        panic!("expected wildcard overlap with loopback listener to fail");
    };
    assert!(
        matches!(err2, ProxyError::Network(_)),
        "expected wildcard overlap error, got {err2}"
    );

    loop_handle.stop().await.unwrap();

    // Rebinding the same loopback address right after stop (with TIME_WAIT sockets) must succeed.
    let mut rebind_cfg = ProxyConfig::new(MitmAuthority::generate().unwrap());
    rebind_cfg.network.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, shared_port));
    let (tx5, _rx5) = mpsc::channel(4);
    let mut rebind_handle = start(rebind_cfg, project, tx5).await.unwrap();
    rebind_handle.stop().await.unwrap();
}
