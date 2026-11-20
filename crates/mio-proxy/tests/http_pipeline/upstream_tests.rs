use super::*;

#[tokio::test]
async fn upstream_routes_forward_both_protocols_and_delegate_dns() {
    for kind in [
        UpstreamKind::Http,
        UpstreamKind::Https(CertificateKind::Valid),
        UpstreamKind::Socks5,
    ] {
        for tls in [false, true] {
            let mut origin =
                Origin::start(tls.then_some((CertificateKind::Valid, true)), false).await;
            let mut upstream = Upstream::start(&origin, kind, true, false).await;
            let (mut proxy, ca, _events) =
                proxy_with_route(&origin, false, upstream.route(proxy_credentials())).await;
            let url = if tls {
                origin.url()
            } else {
                origin.url().replace("localhost", "only-at-proxy.invalid")
            };
            let response =
                tokio::time::timeout(DEADLINE, client(proxy.address(), url, ca, tls, tls))
                    .await
                    .unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.body, [0, 255, b'O']);
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
                event => panic!("expected actual upstream route, got {event:?}"),
            }
            match origin.next().await {
                OriginEvent::Request {
                    version,
                    headers,
                    body,
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
                    assert_eq!(body, [0, 254, b'C']);
                    assert_eq!(
                        headers
                            .iter()
                            .filter(|(name, _)| name == "x-client")
                            .count(),
                        2
                    );
                    assert!(
                        !headers
                            .iter()
                            .any(|(name, _)| name == "proxy-authorization")
                    );
                }
                event => panic!("expected actual origin request, got {event:?}"),
            }
            proxy.stop().await.unwrap();
            upstream.stop().await;
            origin.stop().await;
        }
    }
}

#[tokio::test]
async fn repeater_upstream_authentication_rejects_without_direct_fallback() {
    for kind in [
        UpstreamKind::Http,
        UpstreamKind::Https(CertificateKind::Valid),
        UpstreamKind::Socks5,
    ] {
        let mut origin = Origin::start(None, false).await;
        let mut upstream = Upstream::start(&origin, kind, true, false).await;
        for credentials in [
            None,
            Some(ProxyCredentials {
                username: "user".into(),
                password: "wrong-secret".into(),
            }),
        ] {
            let config = SendConfig {
                upstream_route: upstream.route(credentials),
                timeout: DEADLINE,
                ..SendConfig::default()
            };
            let result =
                mio_proxy::send(upstream_request(&origin), config, CancellationToken::new()).await;
            match result {
                Ok(response) => assert_eq!(response.status, 407),
                Err(error) => assert!(!error.to_string().contains("wrong-secret")),
            }
            loop {
                if matches!(upstream.next().await, UpstreamEvent::AuthRejected) {
                    break;
                }
            }
            assert_eq!(
                origin.received.load(Ordering::SeqCst),
                0,
                "authentication failure must not fall back to the reachable origin"
            );
        }
        let config = SendConfig {
            upstream_route: upstream.route(proxy_credentials()),
            timeout: DEADLINE,
            ..SendConfig::default()
        };
        let response = mio_proxy::send(upstream_request(&origin), config, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        match origin.next().await {
            OriginEvent::Request { headers, body, .. } => {
                assert_eq!(body, [0, 255, b'P']);
                assert!(
                    !headers
                        .iter()
                        .any(|(name, _)| name == "proxy-authorization")
                );
            }
            event => panic!("expected authenticated request, got {event:?}"),
        }
        upstream.stop().await;
        origin.stop().await;
    }
}

#[tokio::test]
async fn upstream_proxy_and_origin_tls_are_verified_independently() {
    for (kind, trusted) in [
        (CertificateKind::Valid, false),
        (CertificateKind::WrongName, true),
        (CertificateKind::Expired, true),
    ] {
        let origin = Origin::start(None, false).await;
        let mut upstream = Upstream::start(&origin, UpstreamKind::Https(kind), false, false).await;
        let mut route = upstream.route(None);
        if !trusted && let UpstreamRoute::Https { tls, .. } = &mut route {
            *tls = UpstreamTls::System;
        }
        let config = SendConfig {
            upstream_route: route,
            timeout: DEADLINE,
            ..SendConfig::default()
        };
        assert!(
            mio_proxy::send(upstream_request(&origin), config, CancellationToken::new())
                .await
                .is_err()
        );
        assert!(matches!(upstream.next().await, UpstreamEvent::TlsRejected));
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        upstream.stop().await;
        origin.stop().await;
    }
    for kind in [
        UpstreamKind::Http,
        UpstreamKind::Https(CertificateKind::Valid),
        UpstreamKind::Socks5,
    ] {
        let origin = Origin::start(Some((CertificateKind::Valid, true)), false).await;
        let upstream = Upstream::start(&origin, kind, true, false).await;
        let config = SendConfig {
            upstream_route: upstream.route(proxy_credentials()),
            timeout: DEADLINE,
            ..SendConfig::default()
        };
        assert!(
            mio_proxy::send(upstream_request(&origin), config, CancellationToken::new())
                .await
                .is_err(),
            "trusting the proxy must not trust the origin CA"
        );
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        upstream.stop().await;
        origin.stop().await;
    }
}

#[tokio::test]
async fn unavailable_upstream_never_connects_directly() {
    let origin = Origin::start(None, false).await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let endpoint = ProxyEndpoint {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        credentials: None,
    };
    drop(listener);
    for route in [
        UpstreamRoute::Http(endpoint.clone()),
        UpstreamRoute::Https {
            endpoint: endpoint.clone(),
            tls: UpstreamTls::System,
        },
        UpstreamRoute::Socks5(endpoint),
    ] {
        let config = SendConfig {
            upstream_route: route,
            timeout: DEADLINE,
            ..SendConfig::default()
        };
        assert!(
            mio_proxy::send(upstream_request(&origin), config, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
    }
    origin.stop().await;
}

#[tokio::test]
async fn cancel_upstream_handshake_closes_actual_socket_without_retry() {
    for kind in [
        UpstreamKind::Http,
        UpstreamKind::Https(CertificateKind::Valid),
        UpstreamKind::Socks5,
    ] {
        let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
        let mut upstream = Upstream::start(&origin, kind, true, true).await;
        let config = SendConfig {
            upstream_route: upstream.route(proxy_credentials()),
            upstream_tls: origin.trust(),
            timeout: DEADLINE,
            ..SendConfig::default()
        };
        let cancel = CancellationToken::new();
        let task = tokio::spawn(mio_proxy::send(
            upstream_request(&origin),
            config,
            cancel.clone(),
        ));
        assert!(matches!(upstream.next().await, UpstreamEvent::Route { .. }));
        cancel.cancel();
        assert!(matches!(
            tokio::time::timeout(DEADLINE, task).await.unwrap().unwrap(),
            Err(ProxyError::Cancelled)
        ));
        assert!(matches!(upstream.next().await, UpstreamEvent::Closed));
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        upstream.stop().await;
        origin.stop().await;
    }
}

#[tokio::test]
async fn proxy_stop_closes_active_upstream_handshakes() {
    for kind in [
        UpstreamKind::Http,
        UpstreamKind::Https(CertificateKind::Valid),
        UpstreamKind::Socks5,
    ] {
        let origin = Origin::start(Some((CertificateKind::Valid, false)), false).await;
        let mut upstream = Upstream::start(&origin, kind, true, true).await;
        let (mut proxy, ca, _events) =
            proxy_with_route(&origin, false, upstream.route(proxy_credentials())).await;
        let stream = tls_stream(
            proxy.address(),
            &format!("localhost:{}", origin.address.port()),
            ca,
            false,
        )
        .await;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        let driver = tokio::spawn(connection);
        let request = Request::builder()
            .uri(origin.url())
            .body(Full::new(Bytes::new()))
            .unwrap();
        let pending = tokio::spawn(async move { sender.send_request(request).await });
        assert!(matches!(upstream.next().await, UpstreamEvent::Route { .. }));
        proxy.stop().await.unwrap();
        assert!(matches!(upstream.next().await, UpstreamEvent::Closed));
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        assert!(
            tokio::time::timeout(DEADLINE, pending)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let _ = tokio::time::timeout(DEADLINE, driver).await.unwrap();
        upstream.stop().await;
        origin.stop().await;
    }
}
