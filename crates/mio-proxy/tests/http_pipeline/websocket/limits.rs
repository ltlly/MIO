use super::*;
#[tokio::test]
async fn wss_mitm_verifies_independent_roots_and_uses_h1_on_both_legs() {
    let mut origin = WsOrigin::start(Some(CertificateKind::Valid)).await;
    let (mut proxy, ca, _events) = ws_proxy(&origin).await;
    let authority = format!("localhost:{}", origin.address.port());
    let mut client = tls_stream(proxy.address(), &authority, ca, false).await;
    assert_eq!(client.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
    let response = ws_handshake(&mut client, "/websocket").await;
    assert!(response.starts_with(b"HTTP/1.1 101"));
    match origin.next().await {
        WsEvent::Handshake {
            upgrade,
            connection,
            key,
            alpn,
            ..
        } => {
            assert!(upgrade && connection && key);
            assert_eq!(alpn.as_deref(), Some(&b"http/1.1"[..]));
        }
        event => panic!("expected verified WSS handshake, got {event:?}"),
    }
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );
    let request = Frame::new(1, "真实 WSS 消息".as_bytes().to_vec());
    client.write_all(&request.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &request).await;
    origin.commands.send(request.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, request);

    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn websocket_stop_joins_active_upgrade_and_closes_both_actual_sockets() {
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

    tokio::time::timeout(DEADLINE, proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal_record(&mut events).await.summary.state,
        FlowState::Cancelled
    );
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(DEADLINE, client.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(matches!(origin.next().await, WsEvent::Closed));
    origin.stop().await;
}

#[tokio::test]
async fn websocket_control_pings_are_acknowledged_on_the_receiving_leg() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, _events) = ws_proxy(&origin).await;
    let mut client = TcpStream::connect(proxy.address()).await.unwrap();
    let response = ws_handshake(&mut client, &origin.url()).await;
    assert!(response.starts_with(b"HTTP/1.1 101"));
    assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );

    let ping = Frame::new(9, b"client-keepalive".to_vec());
    client.write_all(&ping.encode(true)).await.unwrap();
    assert_eq!(next_frame(&mut client).await, Frame::new(10, ping.payload));
    let marker = Frame::new(1, b"after-client-ping".to_vec());
    client.write_all(&marker.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &marker).await;

    let ping = Frame::new(9, b"server-keepalive".to_vec());
    origin.commands.send(ping.clone()).await.unwrap();
    assert_origin_frame(&mut origin, &Frame::new(10, ping.payload)).await;
    let marker = Frame::new(1, b"after-server-ping".to_vec());
    origin.commands.send(marker.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, marker);

    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn websocket_stop_reclaims_upgrade_with_full_message_event_queue() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, mut events) = ws_proxy_with(&origin, 1, |_| {}).await;
    let address = proxy.address();
    let url = origin.url();
    let client = tokio::spawn(async move {
        let mut client = TcpStream::connect(address).await.unwrap();
        let response = ws_handshake(&mut client, &url).await;
        assert!(response.starts_with(b"HTTP/1.1 101"));
        assert_eq!(
            next_frame(&mut client).await,
            Frame::new(1, b"server-first".to_vec())
        );
        client
    });
    for _ in 0..2 {
        assert!(matches!(
            tokio::time::timeout(DEADLINE, events.recv()).await.unwrap(),
            Some(ProxyEvent::Captured { .. })
        ));
    }
    let mut client = tokio::time::timeout(DEADLINE, client)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
    assert_eq!(events.len(), 1, "the actual first message fills the queue");
    client
        .write_all(&Frame::new(1, b"pending-observation".to_vec()).encode(true))
        .await
        .unwrap();
    tokio::time::timeout(DEADLINE, proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(origin.next().await, WsEvent::Closed));
    origin.stop().await;
}

#[tokio::test]
async fn websocket_rejects_invalid_and_oversized_messages_without_partial_forwarding() {
    let oversized = Frame::new(2, vec![1; 33]).encode(true);
    let mut fragments = Frame {
        fin: false,
        opcode: 2,
        payload: vec![2; 20],
    }
    .encode(true);
    fragments.extend(Frame::new(0, vec![3; 20]).encode(true));
    let unmasked = Frame::new(2, vec![4]).encode(false);
    let invalid_utf8 = Frame::new(1, vec![0xff]).encode(true);
    for bytes in [oversized, fragments, unmasked, invalid_utf8] {
        let mut origin = WsOrigin::start(None).await;
        let (mut proxy, _, mut events) =
            ws_proxy_with(&origin, 32, |config| config.max_body_bytes = 32).await;
        let mut client = TcpStream::connect(proxy.address()).await.unwrap();
        let response = ws_handshake(&mut client, &origin.url()).await;
        assert!(response.starts_with(b"HTTP/1.1 101"));
        assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
        assert_eq!(
            next_frame(&mut client).await,
            Frame::new(1, b"server-first".to_vec())
        );
        client.write_all(&bytes).await.unwrap();
        assert!(matches!(
            terminal_record(&mut events).await.summary.state,
            FlowState::Failed(_)
        ));
        assert!(matches!(origin.next().await, WsEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}
