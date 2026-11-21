use super::*;
use mio_plugin_api::{WebSocketDirection, WebSocketKind};

#[tokio::test]
async fn websocket_observations_preserve_identity_order_and_payload_without_fabricated_pong() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, mut events) = ws_proxy(&origin).await;
    let mut client = TcpStream::connect(proxy.address()).await.unwrap();
    assert!(
        ws_handshake(&mut client, &origin.url())
            .await
            .starts_with(b"HTTP/1.1 101")
    );
    assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );
    let binary = Frame::new(2, (0..=255).collect::<Vec<u8>>());
    client.write_all(&binary.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &binary).await;
    origin.commands.send(binary.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, binary);
    let pong = Frame::new(10, vec![0, 0xff]);
    client.write_all(&pong.encode(true)).await.unwrap();
    let ping = Frame::new(9, vec![7]);
    client.write_all(&ping.encode(true)).await.unwrap();
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(10, ping.payload.clone())
    );
    let close = Frame::new(8, 1000_u16.to_be_bytes().to_vec());
    client.write_all(&close.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &close).await;
    origin.commands.send(close.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, close);

    let (terminal, messages) = tokio::time::timeout(DEADLINE, async {
        let mut messages = Vec::new();
        let mut project = None;
        loop {
            match events.recv().await.unwrap() {
                ProxyEvent::Captured {
                    project: owner,
                    record,
                    response_stream,
                    ..
                } => {
                    assert!(response_stream.is_none());
                    if let Some(expected) = project {
                        assert_eq!(owner, expected);
                    }
                    project = Some(owner);
                    if record.summary.state == FlowState::Complete {
                        return (record, messages);
                    }
                    assert_eq!(record.summary.state, FlowState::Pending);
                }
                ProxyEvent::WebSocketMessage {
                    project: owner,
                    record,
                    original_payload,
                    listener_port,
                } => {
                    assert_eq!(Some(owner), project);
                    assert_eq!(listener_port, proxy.address().port());
                    assert!(original_payload.is_none());
                    messages.push(record);
                }
                ProxyEvent::Intercepted { .. }
                | ProxyEvent::WebSocketIntercepted { .. }
                | ProxyEvent::WebSocketInterceptResolved { .. } => {
                    panic!("interception was disabled")
                }
            }
        }
    })
    .await
    .unwrap();
    use WebSocketDirection::{ClientToServer, ServerToClient};
    use WebSocketKind::{Binary, Close, Ping, Pong, Text};
    let expected = [
        (ServerToClient, Text, b"server-first".to_vec()),
        (ClientToServer, Binary, binary.payload.clone()),
        (ServerToClient, Binary, binary.payload),
        (ClientToServer, Pong, pong.payload),
        (ClientToServer, Ping, ping.payload),
        (ClientToServer, Close, close.payload.clone()),
        (ServerToClient, Close, close.payload),
    ];
    assert_eq!(messages.len(), expected.len());
    let mut ids = std::collections::HashSet::new();
    for (index, (message, (direction, kind, payload))) in
        messages.into_iter().zip(expected).enumerate()
    {
        assert!(ids.insert(message.id));
        assert_eq!(message.revision, 1);
        assert_eq!(message.flow_id, terminal.summary.id);
        assert_eq!(message.sequence, index as u64 + 1);
        assert_eq!(
            (message.direction, message.kind, message.payload),
            (direction, kind, payload)
        );
        assert!(message.observed_unix_ms > 0);
    }
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn websocket_message_interception_edits_drops_single_frame_and_passes_control_frames() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, mut events) = ws_proxy_with(&origin, 64, |config| {
        config.hooks.websocket_intercept_filter = Some(Arc::new(|_, _, _| Ok(true)));
    })
    .await;
    let mut client = TcpStream::connect(proxy.address()).await.unwrap();
    assert!(
        ws_handshake(&mut client, &origin.url())
            .await
            .starts_with(b"HTTP/1.1 101")
    );
    assert!(matches!(origin.next().await, WsEvent::Handshake { .. }));
    // Drain the initial server-first text frame before turning on interception.
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );

    proxy.set_intercept(true, true);

    // 1. Client sends Text "hello-orig" + Ping while Text is held in Intercept.
    client
        .write_all(&Frame::new(1, b"hello-orig".to_vec()).encode(true))
        .await
        .unwrap();
    let ping = Frame::new(9, b"ping1".to_vec());
    client.write_all(&ping.encode(true)).await.unwrap();
    // Ping must be acknowledged immediately even while Text is held!
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), next_frame(&mut client))
            .await
            .expect("Ping must receive Pong while data frame is intercepted"),
        Frame::new(10, b"ping1".to_vec())
    );

    let (held_c2s, reply_c2s) = tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::WebSocketIntercepted {
                url, record, reply, ..
            } = events.recv().await.unwrap()
            {
                assert_eq!(url, origin.url());
                return (record, reply);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(held_c2s.direction, WebSocketDirection::ClientToServer);
    assert_eq!(held_c2s.kind, WebSocketKind::Text);
    assert_eq!(held_c2s.payload, b"hello-orig");

    reply_c2s
        .send(mio_proxy::WebSocketInterceptDecision::Forward(
            b"hello-edited".to_vec(),
        ))
        .unwrap();
    assert_origin_frame(&mut origin, &Frame::new(1, b"hello-edited".to_vec())).await;

    let (observed_c2s, orig_c2s) = tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::WebSocketMessage {
                record,
                original_payload,
                ..
            } = events.recv().await.unwrap()
                && record.id == held_c2s.id
            {
                return (record, original_payload);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(observed_c2s.payload, b"hello-edited");
    assert_eq!(orig_c2s.as_deref(), Some(b"hello-orig".as_slice()));

    // 2. Client sends a message that gets Dropped; connection stays alive for the next message.
    client
        .write_all(&Frame::new(1, b"drop-me".to_vec()).encode(true))
        .await
        .unwrap();
    let (drop_record, reply_drop) = next_websocket_intercept(&mut events).await;
    assert_eq!(drop_record.payload, b"drop-me");
    reply_drop
        .send(mio_proxy::WebSocketInterceptDecision::Drop)
        .unwrap();

    // 3. Server sends Binary frame; intercepted, then turning master intercept off auto-forwards.
    origin
        .commands
        .send(Frame::new(2, vec![0xde, 0xad, 0xbe, 0xef]))
        .await
        .unwrap();
    let held_s2c_id = tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::WebSocketIntercepted { record, .. } = events.recv().await.unwrap() {
                assert_eq!(record.direction, WebSocketDirection::ServerToClient);
                assert_eq!(record.kind, WebSocketKind::Binary);
                assert_eq!(record.payload, vec![0xde, 0xad, 0xbe, 0xef]);
                return record.id;
            }
        }
    })
    .await
    .unwrap();
    proxy.set_intercept(false, false);
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(2, vec![0xde, 0xad, 0xbe, 0xef])
    );
    let mut saw_resolved = false;
    let mut saw_auto_forwarded = false;
    tokio::time::timeout(DEADLINE, async {
        while !(saw_resolved && saw_auto_forwarded) {
            match events.recv().await.unwrap() {
                ProxyEvent::WebSocketInterceptResolved { message_id, .. }
                    if message_id == held_s2c_id =>
                {
                    saw_resolved = true;
                }
                ProxyEvent::WebSocketMessage {
                    record,
                    original_payload,
                    ..
                } if record.id == held_s2c_id => {
                    assert!(original_payload.is_none());
                    assert_eq!(record.payload, vec![0xde, 0xad, 0xbe, 0xef]);
                    saw_auto_forwarded = true;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    // 4. Subsequent client message arrives at origin normally on the same open connection.
    let after_drop = Frame::new(1, b"after-drop".to_vec());
    client.write_all(&after_drop.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &after_drop).await;

    let close = Frame::new(8, 1000_u16.to_be_bytes().to_vec());
    client.write_all(&close.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &close).await;
    origin.commands.send(close.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, close);
    proxy.stop().await.unwrap();
    origin.stop().await;
}
