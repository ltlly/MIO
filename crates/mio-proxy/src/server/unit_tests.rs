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
    let (_mode, intercept) = watch::channel(InterceptMode {
        requests: false,
        responses: false,
    });
    let (processing_tx, processing) = watch::channel((None, None, None));
    let (ws_tx, websocket_transform) = watch::channel(None);
    let (ssl_tx, ssl_pass_through) = watch::channel(None);
    let (_plugin_hooks_tx, plugin_hooks) = watch::channel((None, None));
    let (drop_oos_tx, drop_out_of_scope) = watch::channel(None);
    let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    config.max_connections = 1;
    let state = Arc::new(State {
        project,
        listener_port: 1,
        config,
        events,
        cancel: cancel.clone(),
        intercept,
        processing,
        websocket_transform,
        ssl_pass_through,
        plugin_hooks,
        drop_out_of_scope,
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
        intercept: _mode,
        processing: processing_tx,
        websocket_transform: ws_tx,
        ssl_pass_through: ssl_tx,
        plugin_hooks: _plugin_hooks_tx,
        drop_out_of_scope: drop_oos_tx,
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
    config.redirect_to_host = Some("127.0.0.1".into());
    config.redirect_to_port = Some(9443);
    config.force_use_of_tls = true;
    config.strip_proxy_headers = true;
    config.remove_unsupported_accept_encodings = true;
    config.remove_100_continue_headers = true;
    config.use_http10_in_requests = true;
    config.set_request_connection_header = true;

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

    listener::apply_listener_target_overrides(&mut req, &config).unwrap();
    assert_eq!(req.url, "https://127.0.0.1:9443/v1/items?a=1");

    listener::apply_proxy_misc_request_transforms(&mut req, &config, false);
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
    keepalive_cfg.http1_keep_alive = true;
    keepalive_cfg.set_request_connection_header = true;
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
    listener::apply_proxy_misc_request_transforms(&mut ka_req, &keepalive_cfg, false);
    assert!(ka_req.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("connection") && h.value.eq_ignore_ascii_case(b"keep-alive")
    }));
    let send_cfg = crate::SendConfig {
        http1_keep_alive: true,
        set_connection_keep_alive: true,
        ..Default::default()
    };
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
    config.invisible_proxying = true;
    config.disable_web_interface = true;
    config.suppress_browser_error_messages = true;
    config.use_http10_in_responses = true;
    config.set_response_connection_close = true;
    config.drop_out_of_scope_fn = Some(Arc::new(|url: &str| url.contains("/blocked")));

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
