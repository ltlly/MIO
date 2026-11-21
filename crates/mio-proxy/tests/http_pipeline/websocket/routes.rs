use super::*;

async fn route_roundtrip<I: AsyncRead + AsyncWrite + Unpin>(
    mut client: I,
    proxy: &mut mio_proxy::ProxyHandle,
    origin: &mut WsOrigin,
) {
    let response = ws_handshake(&mut client, &origin.url()).await;
    assert!(response.starts_with(b"HTTP/1.1 101"));
    assert!(matches!(
        origin.next().await,
        WsEvent::Handshake {
            upgrade: true,
            connection: true,
            key: true,
            ..
        }
    ));
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );
    let frame = Frame::new(2, vec![0, 0xff, 0xc3, 0x28]);
    client.write_all(&frame.encode(true)).await.unwrap();
    assert_origin_frame(origin, &frame).await;
    origin.commands.send(frame.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, frame);
    proxy.stop().await.unwrap();
    assert!(matches!(origin.next().await, WsEvent::Closed));
}

#[tokio::test]
async fn websocket_and_wss_preserve_authenticated_http_https_and_socks_routes() {
    for tls in [false, true] {
        for kind in [
            UpstreamKind::Http,
            UpstreamKind::Https(CertificateKind::Valid),
            UpstreamKind::Socks5,
        ] {
            let mut origin = WsOrigin::start(tls.then_some(CertificateKind::Valid)).await;
            let mut upstream = Upstream::start_at_origin(origin.address, kind, true, false).await;
            let route = upstream.route(proxy_credentials());
            let (mut proxy, ca, _events) = ws_proxy_with(&origin, 32, |config| {
                config.network.upstream_route = route;
            })
            .await;
            if tls {
                let authority = format!("localhost:{}", origin.address.port());
                let client = tls_stream(proxy.address(), &authority, ca, false).await;
                route_roundtrip(client, &mut proxy, &mut origin).await;
            } else {
                let client = TcpStream::connect(proxy.address()).await.unwrap();
                route_roundtrip(client, &mut proxy, &mut origin).await;
            }
            match upstream.next().await {
                UpstreamEvent::Route {
                    target,
                    authenticated,
                } => {
                    assert!(authenticated);
                    if tls || matches!(kind, UpstreamKind::Socks5) {
                        assert_eq!(target, format!("localhost:{}", origin.address.port()));
                    } else {
                        assert_eq!(target, origin.url());
                    }
                }
                event => panic!("expected authenticated route, got {event:?}"),
            }
            assert!(matches!(upstream.next().await, UpstreamEvent::Closed));
            origin.stop().await;
            upstream.stop().await;
        }
    }
}

#[tokio::test]
async fn wss_rejects_unknown_ca_wrong_name_and_expired_origin() {
    for (kind, trust) in [
        (CertificateKind::Valid, false),
        (CertificateKind::WrongName, true),
        (CertificateKind::Expired, true),
    ] {
        let mut origin = WsOrigin::start(Some(kind)).await;
        let (mut proxy, ca, mut events) = ws_proxy_with(&origin, 32, |config| {
            if !trust {
                config.tls.upstream.trust = UpstreamTls::System;
            }
        })
        .await;
        let authority = format!("localhost:{}", origin.address.port());
        let mut client = tls_stream(proxy.address(), &authority, ca, false).await;
        let response = ws_handshake(&mut client, "/websocket").await;
        assert!(response.starts_with(b"HTTP/1.1 502"));
        assert!(matches!(origin.next().await, WsEvent::TlsRejected));
        assert!(matches!(
            terminal_record(&mut events).await.summary.state,
            FlowState::Failed(_)
        ));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn websocket_server_initiated_and_simultaneous_close_complete_normally() {
    for simultaneous in [false, true] {
        let mut origin = WsOrigin::start(None).await;
        let (mut proxy, _, mut events) = ws_proxy(&origin).await;
        let mut client = TcpStream::connect(proxy.address()).await.unwrap();
        let response = ws_handshake(&mut client, &origin.url()).await;
        assert!(response.starts_with(b"HTTP/1.1 101"));
        assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
        assert_eq!(
            next_frame(&mut client).await,
            Frame::new(1, b"server-first".to_vec())
        );
        let mut payload = 1001_u16.to_be_bytes().to_vec();
        payload.extend_from_slice("结束".as_bytes());
        let close = Frame::new(8, payload);
        if simultaneous {
            let bytes = close.encode(true);
            let (client_result, origin_result) = tokio::join!(
                client.write_all(&bytes),
                origin.commands.send(close.clone()),
            );
            client_result.unwrap();
            origin_result.unwrap();
        } else {
            origin.commands.send(close.clone()).await.unwrap();
        }
        assert_eq!(next_frame(&mut client).await, close);
        if !simultaneous {
            client.write_all(&close.encode(true)).await.unwrap();
        }
        assert_origin_frame(&mut origin, &close).await;
        assert_eq!(
            terminal_record(&mut events).await.summary.state,
            FlowState::Complete
        );
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn websocket_silent_close_peer_fails_at_bounded_deadline() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, mut events) = ws_proxy_with(&origin, 32, |config| {
        config.limits.stop_timeout = Duration::from_millis(100);
    })
    .await;
    let mut client = TcpStream::connect(proxy.address()).await.unwrap();
    let response = ws_handshake(&mut client, &origin.url()).await;
    assert!(response.starts_with(b"HTTP/1.1 101"));
    assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );
    let close = Frame::new(8, 1000_u16.to_be_bytes().to_vec());
    client.write_all(&close.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &close).await;
    match terminal_record(&mut events).await.summary.state {
        FlowState::Failed(error) => assert!(error.contains("关闭握手")),
        state => panic!("silent peer must not be marked complete: {state:?}"),
    }
    assert!(matches!(origin.next().await, WsEvent::Closed));
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn websocket_rejects_forged_accept_and_unsolicited_negotiation() {
    for (accept, extra) in [
        ("invalid-accept", ""),
        (ACCEPT, "Sec-WebSocket-Protocol: unoffered\r\n"),
        (ACCEPT, "Sec-WebSocket-Extensions: permessage-deflate\r\n"),
    ] {
        let reply = format!(
            "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n{extra}\r\n"
        );
        let mut origin = WsOrigin::start_with_reply(None, Some(reply)).await;
        let (mut proxy, _, mut events) = ws_proxy(&origin).await;
        let mut client = TcpStream::connect(proxy.address()).await.unwrap();
        let response = ws_handshake(&mut client, &origin.url()).await;
        assert!(response.starts_with(b"HTTP/1.1 502"));
        assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
        assert!(matches!(
            terminal_record(&mut events).await.summary.state,
            FlowState::Failed(_)
        ));
        assert!(matches!(origin.next().await, WsEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}
