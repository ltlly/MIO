use super::*;
#[tokio::test]
async fn websocket_upgrade_preserves_handshake_and_coalesced_bytes() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, mut events) = ws_proxy(&origin).await;
    let mut client = TcpStream::connect(proxy.address()).await.unwrap();
    let response = ws_handshake(&mut client, &origin.url()).await;
    let observed = origin.next().await;
    if !response.starts_with(b"HTTP/1.1 101") {
        proxy.stop().await.unwrap();
        origin.stop().await;
        panic!(
            "actual WS upgrade failed: response={}, origin={observed:?}",
            String::from_utf8_lossy(&response)
        );
    }
    assert!(matches!(
        observed,
        WsEvent::Handshake {
            upgrade: true,
            connection: true,
            key: true,
            alpn: None,
            ..
        }
    ));
    assert_eq!(
        tokio::time::timeout(DEADLINE, read_frame(&mut client, false))
            .await
            .unwrap()
            .unwrap(),
        Frame::new(1, b"server-first".to_vec())
    );
    let close = Frame::new(8, 1000_u16.to_be_bytes().to_vec());
    client.write_all(&close.encode(true)).await.unwrap();
    match origin.next().await {
        WsEvent::Frame(frame) => assert_eq!(frame, close),
        event => panic!("expected origin close frame, got {event:?}"),
    }
    origin.commands.send(close.clone()).await.unwrap();
    assert_eq!(
        tokio::time::timeout(DEADLINE, read_frame(&mut client, false))
            .await
            .unwrap()
            .unwrap(),
        close
    );
    let terminal = terminal_record(&mut events).await;
    assert_eq!(terminal.summary.state, FlowState::Complete);
    assert_eq!(terminal.summary.status, Some(101));
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn websocket_transfers_binary_and_fragmented_utf8_in_both_directions() {
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

    let binary = Frame::new(2, (0..=255).collect::<Vec<u8>>());
    client.write_all(&binary.encode(true)).await.unwrap();
    assert_origin_frame(&mut origin, &binary).await;
    origin.commands.send(binary.clone()).await.unwrap();
    assert_eq!(next_frame(&mut client).await, binary);

    let text = "中文🙂 fragmentation".as_bytes();
    let fragments = [
        Frame {
            fin: false,
            opcode: 1,
            payload: text[..1].to_vec(),
        },
        Frame {
            fin: false,
            opcode: 0,
            payload: text[1..7].to_vec(),
        },
        Frame::new(0, text[7..].to_vec()),
    ];
    for (index, fragment) in fragments.iter().enumerate() {
        client.write_all(&fragment.encode(true)).await.unwrap();
        if index == 0 {
            let ping = Frame::new(9, b"fragment-ping".to_vec());
            client.write_all(&ping.encode(true)).await.unwrap();
            assert_eq!(next_frame(&mut client).await, Frame::new(10, ping.payload));
        }
    }
    assert_origin_frame(&mut origin, &Frame::new(1, text.to_vec())).await;
    for (index, fragment) in fragments.into_iter().enumerate() {
        origin.commands.send(fragment).await.unwrap();
        if index == 0 {
            let ping = Frame::new(9, b"fragment-server-ping".to_vec());
            origin.commands.send(ping.clone()).await.unwrap();
            assert_origin_frame(&mut origin, &Frame::new(10, ping.payload)).await;
        }
    }
    assert_eq!(next_frame(&mut client).await, Frame::new(1, text.to_vec()));

    proxy.stop().await.unwrap();
    origin.stop().await;
}
