use super::*;

#[test]
fn absolute_target_rejects_backslash_in_raw_authority() {
    for request_target in [
        r"http://localhost:8912\a\b",
        r"http://localhost:8912\a/../b",
    ] {
        let raw = format!("GET {request_target} HTTP/1.1\r\n\r\n");
        let result = parse_request(raw.as_bytes(), "http://localhost:8912", HttpVersion::Http11);
        assert!(
            result.is_err(),
            "absolute authority must not absorb raw path bytes: {result:?}"
        );
    }
}

#[test]
fn raw_view_does_not_hide_backslash_in_raw_authority() {
    for url in [
        r"http://localhost:8912\a\b",
        r"http://localhost:8912\a/../b",
    ] {
        let request = HttpRequest {
            method: "GET".into(),
            url: url.into(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        };
        assert_eq!(
            request_to_raw(&request),
            format!("GET {url} HTTP/1.1\r\n\r\n").as_bytes(),
        );
    }
}

#[test]
fn preserved_path_still_rejects_authority_escape_credentials_and_fragments() {
    for request_target in [
        "//other.invalid/a/../b",
        "/\\other.invalid/a/../b",
        "http://other.invalid/a/../b",
        "https://localhost:8912/a/../b",
        "http://localhost:8913/a/../b",
        "http://user:secret@localhost:8912/a/../b",
        "http:///localhost:8912/a/../b",
        "http:////localhost:8912/a/../b",
        "/a/../b#fragment",
        "http://localhost:8912/a/../b#fragment",
    ] {
        let raw = format!("GET {request_target} HTTP/1.1\r\n\r\n");
        assert!(
            parse_request(raw.as_bytes(), "http://localhost:8912", HttpVersion::Http11).is_err(),
            "untrusted request-target must not escape the selected connection or hide a fragment"
        );
    }
}

#[tokio::test]
async fn literal_backslash_send_fails_before_connect() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let mut results = Vec::new();
        for path in DRAFT_ONLY_PATHS {
            let raw = format!("GET {path} HTTP/1.1\r\n\r\n");
            let request = parse_request(raw.as_bytes(), &target, HttpVersion::Http11).unwrap();
            results.push(
                mio_proxy::send(
                    request,
                    mio_proxy::SendConfig {
                        timeout: Duration::from_secs(1),
                        ..mio_proxy::SendConfig::default()
                    },
                    CancellationToken::new(),
                )
                .await,
            );
        }
        let incoming = listener.into_std().unwrap().accept();
        for result in results {
            assert!(
                matches!(&result, Err(mio_proxy::ProxyError::InvalidMessage(message))
                    if message.contains("SDK") && message.contains("反斜线")),
                "a literal backslash must fail explicitly before SDK normalization: {result:?}"
            );
        }
        assert_eq!(
            incoming.unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "unsupported raw targets must create neither an origin connection nor a request"
        );
    })
    .await
    .expect("unsupported request targets have a finite failure deadline");
}

#[tokio::test]
async fn loopback_receives_unmodified_request_targets() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let origin = async {
            let mut received = Vec::new();
            for _ in 0..PATHS.len() * 2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read_buf(&mut request).await.unwrap();
                    assert!(count > 0 && request.len() < 8192, "bounded request head");
                }
                let first = request.split(|byte| *byte == b'\r').next().unwrap();
                received.push(first.to_vec());
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
            received
        };
        let sending = async {
            let mut responses = Vec::new();
            for path in PATHS {
                for request_target in [path.to_string(), format!("{target}{path}")] {
                    let raw = format!("GET {request_target} HTTP/1.1\r\n\r\n");
                    let request =
                        parse_request(raw.as_bytes(), &target, HttpVersion::Http11).unwrap();
                    responses.push(
                        mio_proxy::send(
                            request,
                            mio_proxy::SendConfig {
                                timeout: Duration::from_secs(1),
                                http_version: Some(HttpVersion::Http11),
                                ..mio_proxy::SendConfig::default()
                            },
                            CancellationToken::new(),
                        )
                        .await,
                    );
                }
            }
            responses
        };
        let (received, responses) = tokio::join!(origin, sending);
        for response in responses {
            assert_eq!(response.unwrap().status, 200);
        }
        let expected: Vec<_> = PATHS
            .iter()
            .flat_map(|path| [path, path])
            .map(|path| format!("GET {path} HTTP/1.1").into_bytes())
            .collect();
        assert_eq!(received, expected, "origin must receive the manual target");
    })
    .await
    .expect("loopback request-target regression has a finite deadline");
}

#[test]
fn websocket_intercept_filter_matches_direction_and_scope_including_ws_schemes() {
    use crate::{ProxyWebSocketInterceptionSettings, ScopeRule, TargetScope};
    use mio_plugin_api::WebSocketDirection::{ClientToServer, ServerToClient};

    let cancel = CancellationToken::new();
    let req = |url: &str| crate::HttpRequest {
        method: "GET".into(),
        url: url.into(),
        version: HttpVersion::Http11,
        headers: Vec::new(),
        trailers: Vec::new(),
        body: Vec::new(),
    };
    let defaults = super::build_websocket_intercept_filter(
        ProxyWebSocketInterceptionSettings::default(),
        TargetScope::default(),
    )
    .unwrap();
    assert!(defaults(&req("ws://example.com/chat"), ClientToServer, &cancel).unwrap());
    assert!(defaults(&req("wss://example.com/chat"), ServerToClient, &cancel).unwrap());

    let only_c2s = super::build_websocket_intercept_filter(
        ProxyWebSocketInterceptionSettings {
            client_to_server_messages: true,
            server_to_client_messages: false,
            intercept_in_scope_only: false,
        },
        TargetScope::default(),
    )
    .unwrap();
    assert!(only_c2s(&req("http://example.com/chat"), ClientToServer, &cancel).unwrap());
    assert!(!only_c2s(&req("http://example.com/chat"), ServerToClient, &cancel).unwrap());

    let scope = TargetScope {
        include: vec![ScopeRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            prefix: "https://example.com/live".into(),
            include_subdomains: false,
        }],
        exclude: vec![ScopeRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            prefix: "https://example.com/live/skip".into(),
            include_subdomains: false,
        }],
        ..Default::default()
    };
    let scoped = super::build_websocket_intercept_filter(
        ProxyWebSocketInterceptionSettings {
            client_to_server_messages: true,
            server_to_client_messages: true,
            intercept_in_scope_only: true,
        },
        scope,
    )
    .unwrap();
    assert!(scoped(&req("wss://example.com/live/room"), ClientToServer, &cancel).unwrap());
    assert!(!scoped(&req("wss://example.com/live/skip"), ClientToServer, &cancel).unwrap());
    assert!(
        !scoped(
            &req("wss://other.example/live/room"),
            ServerToClient,
            &cancel
        )
        .unwrap()
    );
}

#[test]
fn proxy_intercept_extra_conditions_validate_stage_and_evaluate_ip_mime_title_and_annotations() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{
        DoesNotMatch, Matches, ResponseMatches, WasAnnotated, WasNotAnnotated,
    };
    use crate::ProxyInterceptType::{IpAddress, MimeType, PageTitle, Request};
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let limits = (deadline, MAX_BODY);

    for (match_type, relation) in [
        (MimeType, Matches),
        (MimeType, DoesNotMatch),
        (PageTitle, Matches),
        (PageTitle, DoesNotMatch),
        (Request, WasAnnotated),
        (Request, WasNotAnnotated),
        (IpAddress, ResponseMatches),
    ] {
        let mut invalid = crate::ProxyInterceptionSettings::default();
        invalid.request.rules = vec![interception_rule(1, And, match_type, relation, "x")];
        assert!(validate_proxy_interception_shape(&invalid).is_err());
    }
    for (match_type, relation) in [
        (IpAddress, ResponseMatches),
        (MimeType, ResponseMatches),
        (PageTitle, ResponseMatches),
        (IpAddress, WasAnnotated),
    ] {
        let mut invalid = crate::ProxyInterceptionSettings::default();
        invalid.response.enabled = true;
        invalid.response.rules = vec![interception_rule(2, And, match_type, relation, "x")];
        assert!(validate_proxy_interception_shape(&invalid).is_err());
    }

    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(1, And, IpAddress, Matches, r"^::1$")];
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(
        2,
        And,
        IpAddress,
        DoesNotMatch,
        r"^127\.0\.0\.1$",
    )];
    let ip_filter = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    let peer_v6 = mio_proxy::InterceptEvalContext {
        listener_port: 18080,
        target_ip: Some("[0:0:0:0:0:0:0:1]:443"),
        was_annotated: false,
    };
    assert!(
        ip_filter(
            &record, false, false, false, false, peer_v6, &cancel, limits
        )
        .unwrap()
    );
    assert!(ip_filter(&record, true, true, false, false, peer_v6, &cancel, limits).unwrap());
    assert!(
        ip_filter(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap_err()
        .to_string()
        .contains("目标 IP 尚未确定"),
        "DoesNotMatch must not auto-forward when target IP is unresolved"
    );
    record.request.url = "http://[::ffff:127.0.0.1]:8080/fixture".into();
    assert!(
        !ip_filter(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
    );

    let mut mime_settings = crate::ProxyInterceptionSettings::default();
    mime_settings.response.enabled = true;
    mime_settings.response.rules = vec![interception_rule(3, And, MimeType, Matches, "^image$")];
    let mime_filter =
        build_intercept_filter(mime_settings.clone(), crate::TargetScope::default()).unwrap();
    let resp = record.response.as_mut().unwrap();
    resp.headers.clear();
    resp.body = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    assert!(mime_filter(&record, true, false, false, false, peer_v6, &cancel, limits).unwrap());
    assert!(mime_filter(&record, true, true, false, false, peer_v6, &cancel, limits).is_err());
    mime_settings.response.rules[0].relationship = DoesNotMatch;
    let mime_neg =
        build_intercept_filter(mime_settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        mime_neg(&record, true, true, false, false, peer_v6, &cancel, limits)
            .unwrap_err()
            .to_string()
            .contains("无法嗅探流式响应体")
    );
    record.response.as_mut().unwrap().headers = vec![Header {
        name: "Content-Type".into(),
        value: b"text/html; charset=utf-8".to_vec(),
    }];
    assert!(mime_neg(&record, true, true, false, false, peer_v6, &cancel, limits).unwrap());
    mime_settings.response.rules[0].relationship = Matches;
    mime_settings.response.rules[0].condition = "^html$".into();
    let html_mime = build_intercept_filter(mime_settings, crate::TargetScope::default()).unwrap();
    assert!(html_mime(&record, true, true, false, false, peer_v6, &cancel, limits).unwrap());

    let mut title_settings = crate::ProxyInterceptionSettings::default();
    title_settings.response.enabled = true;
    title_settings.response.rules = vec![interception_rule(
        4,
        And,
        PageTitle,
        Matches,
        r"^A & B 中文$",
    )];
    let title_filter =
        build_intercept_filter(title_settings.clone(), crate::TargetScope::default()).unwrap();
    record.response.as_mut().unwrap().body =
        b"<html><head><TITLE>\n  A &amp; B   &#20013;&#25991;\t</title></head></html>".to_vec();
    assert!(title_filter(&record, true, false, false, false, peer_v6, &cancel, limits).unwrap());
    title_settings.response.rules[0].relationship = DoesNotMatch;
    let title_neg = build_intercept_filter(title_settings, crate::TargetScope::default()).unwrap();
    assert!(
        title_neg(&record, true, true, false, false, peer_v6, &cancel, limits)
            .unwrap_err()
            .to_string()
            .contains("尚未完整接收")
    );
    record.response.as_mut().unwrap().body = b"<html><head><title>unclosed".to_vec();
    assert!(title_neg(&record, true, false, false, false, peer_v6, &cancel, limits).is_err());
    let mut oversized_title = b"<title>".to_vec();
    oversized_title.extend(std::iter::repeat_n(
        b'x',
        super::extra_conditions::MAX_PAGE_TITLE_BYTES + 1,
    ));
    oversized_title.extend_from_slice(b"</title>");
    record.response.as_mut().unwrap().body = oversized_title;
    assert!(title_neg(&record, true, false, false, false, peer_v6, &cancel, limits).is_err());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        title_neg(
            &record, true, false, false, false, peer_v6, &cancelled, limits
        ),
        Err(mio_proxy::ProxyError::Cancelled)
    ));

    let mut ann_settings = crate::ProxyInterceptionSettings::default();
    ann_settings.response.enabled = true;
    ann_settings.response.rules = vec![interception_rule(5, And, Request, WasAnnotated, "")];
    let ann = build_intercept_filter(ann_settings.clone(), crate::TargetScope::default()).unwrap();
    ann_settings.response.rules[0].relationship = WasNotAnnotated;
    let unann = build_intercept_filter(ann_settings, crate::TargetScope::default()).unwrap();
    let annotated_ctx = mio_proxy::InterceptEvalContext {
        was_annotated: true,
        ..peer_v6
    };
    assert!(!ann(&record, true, true, false, false, peer_v6, &cancel, limits).unwrap());
    assert!(
        ann(
            &record,
            true,
            true,
            false,
            false,
            annotated_ctx,
            &cancel,
            limits
        )
        .unwrap()
    );
    assert!(unann(&record, true, true, false, false, peer_v6, &cancel, limits).unwrap());
    assert!(
        !unann(
            &record,
            true,
            true,
            false,
            false,
            annotated_ctx,
            &cancel,
            limits
        )
        .unwrap()
    );
}
