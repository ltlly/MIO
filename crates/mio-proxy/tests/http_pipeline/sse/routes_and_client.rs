use super::*;
#[tokio::test]
async fn authenticated_upstream_routes_stream_sse_without_leaking_credentials() {
    for kind in [
        UpstreamKind::Http,
        UpstreamKind::Https(CertificateKind::Valid),
        UpstreamKind::Socks5,
    ] {
        for tls in [false, true] {
            let mut origin = SseOrigin::start(tls.then_some(true), None).await;
            let mut upstream = Upstream::start_at_origin(origin.address, kind, true, false).await;
            let route = upstream.route(proxy_credentials());
            let (mut proxy, ca, mut events) = start_proxy(
                &origin,
                |config| config.network.upstream_route = route,
                64,
            )
            .await;
            let url = if tls {
                origin.url()
            } else {
                origin.url().replace("localhost", "only-at-proxy.invalid")
            };
            let mut client = SseClient::start(proxy.address(), url, ca, tls, tls);
            match origin.next().await {
                OriginEvent::Request {
                    version,
                    alpn,
                    headers,
                    ..
                } => {
                    assert_eq!(
                        version,
                        if tls {
                            Version::HTTP_2
                        } else {
                            Version::HTTP_11
                        }
                    );
                    assert_eq!(alpn.as_deref(), tls.then_some(b"h2".as_slice()));
                    assert!(
                        !headers
                            .iter()
                            .any(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization"))
                    );
                }
                event => panic!("expected an authenticated actual origin request, got {event:?}"),
            }
            match upstream.next().await {
                UpstreamEvent::Route {
                    target,
                    authenticated,
                } => {
                    assert!(authenticated);
                    assert!(target.contains(if tls {
                        "localhost"
                    } else {
                        "only-at-proxy.invalid"
                    }));
                }
                event => panic!("expected the configured upstream route, got {event:?}"),
            }
            origin.send(FIRST).await;
            assert!(matches!(
                client.next().await,
                StreamEvent::Head { status: 200, .. }
            ));
            client.bytes(FIRST).await;
            assert!(origin.frames.is_some());
            drop(origin.frames.take());
            let end = client.next().await;
            assert!(
                matches!(end, StreamEvent::End(Ok(()))),
                "TLS={tls}: {end:?}"
            );
            let record = terminal(&mut events).await;
            assert_eq!(record.summary.state, FlowState::Complete);
            assert_eq!(record.response.unwrap().body, FIRST);
            client.finish().await;
            assert!(matches!(origin.next().await, OriginEvent::Closed));
            proxy.stop().await.unwrap();
            upstream.stop().await;
            origin.stop().await;
        }
    }
}

#[tokio::test]
async fn resetting_one_h2_sse_body_keeps_its_sibling_usable_and_saves_partial_bytes() {
    let mut origin = SseOrigin::start(Some(true), None).await;
    let mut sibling_origin = Origin::start(Some((CertificateKind::Valid, true)), false).await;
    let (mut proxy, ca, mut events) = start_proxy(
        &origin,
        |config| {
            if let UpstreamTls::CustomRoots {
                certificates_der, ..
            } = &mut config.tls.upstream.trust
            {
                certificates_der.push(sibling_origin.certificate.clone().unwrap());
            }
        },
        64,
    )
    .await;
    let io = tls_stream(
        proxy.address(),
        &format!("localhost:{}", origin.address.port()),
        ca,
        true,
    )
    .await;
    let (sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(io))
            .await
            .unwrap();
    let driver = tokio::spawn(connection);
    let mut stream_sender = sender.clone();
    let request = Request::builder()
        .uri(origin.url())
        .body(Full::new(Bytes::new()))
        .unwrap();
    let response = tokio::spawn(async move { stream_sender.send_request(request).await });
    assert!(matches!(
        origin.next().await,
        OriginEvent::Request {
            version: Version::HTTP_2,
            ..
        }
    ));
    origin.send(FIRST).await;
    let response = tokio::time::timeout(DEADLINE, response)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.version(), Version::HTTP_2);
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while bytes.len() < FIRST.len() {
        bytes.extend(body.frame().await.unwrap().unwrap().into_data().unwrap());
    }
    assert_eq!(bytes, FIRST);
    proxy.set_intercept(false, true);
    let mut sibling_sender = sender;
    let request = Request::builder()
        .uri(sibling_origin.url())
        .body(Full::new(Bytes::new()))
        .unwrap();
    let sibling = tokio::spawn(async move { sibling_sender.send_request(request).await });
    let (held, reply) = next_intercept(&mut events).await;
    assert_eq!(held.request.url, sibling_origin.url());
    assert_eq!(held.response.as_ref().unwrap().body, [0, 0xff, b'O']);
    assert!(matches!(
        sibling_origin.next().await,
        OriginEvent::Request {
            version: Version::HTTP_2,
            ..
        }
    ));
    assert!(matches!(sibling_origin.next().await, OriginEvent::Closed));
    drop(body);
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    let cancelled = terminal(&mut events).await;
    assert_eq!(cancelled.request.url, origin.url());
    assert_eq!(cancelled.summary.state, FlowState::Cancelled);
    assert_eq!(cancelled.response.unwrap().body, FIRST);
    assert!(!reply.is_closed());
    assert!(!sibling.is_finished());
    reply
        .send(InterceptDecision::ForwardResponse(held.response.unwrap()))
        .unwrap();
    let response = tokio::time::timeout(DEADLINE, sibling)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        &[0, 0xff, b'O']
    );
    let completed = terminal(&mut events).await;
    assert_eq!(completed.request.url, sibling_origin.url());
    assert_eq!(completed.summary.state, FlowState::Complete);
    proxy.stop().await.unwrap();
    let _ = tokio::time::timeout(DEADLINE, driver)
        .await
        .unwrap()
        .unwrap();
    origin.stop().await;
    sibling_origin.stop().await;
}

#[tokio::test]
async fn send_streaming_streams_over_h1_h2_and_authenticated_upstream_with_trailers() {
    for h2 in [false, true] {
        let mut origin = SseOrigin::start(Some(h2), None).await;
        let mut upstream = Upstream::start_at_origin(
            origin.address,
            UpstreamKind::Https(CertificateKind::Valid),
            true,
            false,
        )
        .await;
        let route = upstream.route(proxy_credentials());
        let mut config = SendConfig::default();
        config.network.upstream_route = route;
        config.tls.trust = origin.trust();
        config.http.version = Some(if h2 {
            HttpVersion::Http2
        } else {
            HttpVersion::Http11
        });
        config.limits.timeout = DEADLINE;
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let url = origin.url();
        let task = tokio::spawn(mio_proxy::send_streaming(
            HttpRequest {
                method: "GET".into(),
                url,
                version: if h2 {
                    HttpVersion::Http2
                } else {
                    HttpVersion::Http11
                },
                headers: vec![Header {
                    name: "te".into(),
                    value: b"trailers".to_vec(),
                }],
                body: Vec::new(),
                trailers: Vec::new(),
            },
            config,
            cancel.clone(),
            move |response| {
                progress_tx.send(response.clone()).unwrap();
            },
        ));

        match origin.next().await {
            OriginEvent::Request {
                version,
                alpn,
                headers,
                ..
            } => {
                assert_eq!(
                    version,
                    if h2 {
                        Version::HTTP_2
                    } else {
                        Version::HTTP_11
                    }
                );
                assert_eq!(
                    alpn.as_deref(),
                    Some(if h2 {
                        b"h2".as_slice()
                    } else {
                        b"http/1.1".as_slice()
                    })
                );
                assert!(
                    !headers
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization"))
                );
            }
            event => panic!("expected origin request, got {event:?}"),
        }
        match upstream.next().await {
            UpstreamEvent::Route {
                target,
                authenticated,
            } => {
                assert!(authenticated);
                assert!(target.contains("localhost"));
            }
            event => panic!("expected upstream route event, got {event:?}"),
        }

        let head_progress = tokio::time::timeout(DEADLINE, progress_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head_progress.status, 200);
        assert_eq!(
            head_progress.version,
            if h2 {
                HttpVersion::Http2
            } else {
                HttpVersion::Http11
            }
        );
        assert!(head_progress.body.is_empty());

        origin.send(FIRST).await;
        let mut current = head_progress;
        while current.body != FIRST {
            current = tokio::time::timeout(DEADLINE, progress_rx.recv())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(current.trailers.is_empty());

        origin.send(SECOND).await;
        let expected_body = [FIRST, SECOND].concat();
        while current.body != expected_body {
            current = tokio::time::timeout(DEADLINE, progress_rx.recv())
                .await
                .unwrap()
                .unwrap();
        }

        let mut trailers = hyper::HeaderMap::new();
        trailers.insert("x-stream-end", "complete".parse().unwrap());
        origin
            .frames
            .as_ref()
            .unwrap()
            .send(Ok(HttpFrame::trailers(trailers)))
            .await
            .unwrap();
        drop(origin.frames.take());

        let outcome = tokio::time::timeout(DEADLINE, task).await.unwrap().unwrap();
        assert!(outcome.error.is_none());
        let response = outcome.response.unwrap();
        assert_eq!(response.body, expected_body);
        assert_eq!(
            response.trailers,
            vec![Header {
                name: "x-stream-end".into(),
                value: b"complete".to_vec(),
            }]
        );
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        upstream.stop().await;
        origin.stop().await;
    }
}
