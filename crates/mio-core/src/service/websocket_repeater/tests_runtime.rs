use std::{
    net::Ipv4Addr,
    sync::atomic::{AtomicUsize, Ordering},
};

use tokio::{io::AsyncWriteExt, net::TcpListener};

use super::tests_fixtures::{
    assert_offline_reopen_retains_tab, assert_quiesce_and_resume_retains_tab, read_http_head,
    read_ws_frame, spawn_origin_server, wait_event, write_ws_frame,
};
use super::*;

#[tokio::test]
async fn runtime_proxy_shared_bidirectional_inject_standalone_reconnect_and_offline_reopen() {
    use crate::{Command, Runtime, WebSocketHistoryFilter};

    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let origin_addr = listener.local_addr().unwrap();
        let accepted_connections = std::sync::Arc::new(AtomicUsize::new(0));
        let (c2s_tx, c2s_rx) = tokio::sync::oneshot::channel();
        let origin =
            spawn_origin_server(listener, &accepted_connections, c2s_tx);

        let (runtime, mut client) = Runtime::start().unwrap();

        let Event::ProjectOpened {
            project, workspace, ..
        } = wait_event(&mut client, &|e| {
            matches!(e, Event::ProjectOpened { .. })
        })
        .await
        else {
            unreachable!()
        };
        client
            .commands
            .send(Command::StartProxy {
                project,
                listen: "127.0.0.1:0".parse().unwrap(),
            })
            .await
            .unwrap();
        let Event::ProxyState {
            address: Some(proxy_addr),
            ..
        } = wait_event(&mut client, &|e| {
            matches!(e, Event::ProxyState { address: Some(_), .. })
        })
        .await
        else {
            unreachable!()
        };

        let mut ws_client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
        ws_client
            .write_all(
                format!(
                    "GET http://{origin_addr}/ws HTTP/1.1\r\nHost: {origin_addr}\r\nCookie: sid=live-cookie\r\nOrigin: http://app.local\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let resp_head = read_http_head(&mut ws_client).await;
        assert!(resp_head.starts_with("HTTP/1.1 101"));
        write_ws_frame(&mut ws_client, 1, b"seed-c2s", true).await;
        let _ = wait_event(&mut client, &|e| {
            matches!(e, Event::WebSocketHistoryChanged { .. })
        })
        .await;

        client
            .commands
            .send(Command::QueryWebSocketHistory {
                project,
                query_revision: 1,
                filter: WebSocketHistoryFilter::default(),
                flow_id: None,
                offset: 0,
                limit: 10,
                selected_message_id: None,
            })
            .await
            .unwrap();
        let Event::WebSocketHistoryPage { rows, .. } =
            wait_event(&mut client, &|e| {
                matches!(
                    e,
                    Event::WebSocketHistoryPage { query_revision: 1, .. }
                )
            })
            .await
        else {
            unreachable!()
        };
        assert_eq!(rows.len(), 1);
        let seed_msg_id = rows[0].id;
        let live_flow_id = rows[0].flow_id;

        // OpenFromHistory attaches to the live Proxy WS session in ProxyShared mode.
        client
            .commands
            .send(Command::WebSocketRepeater(
                WebSocketRepeaterCommand::OpenFromHistory {
                    project,
                    operation_id: Uuid::new_v4(),
                    message_id: seed_msg_id,
                    use_original: false,
                    payload_override: None,
                },
            ))
            .await
            .unwrap();
        let Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
            result: Ok(opened_tab),
            mode: WebSocketRepeaterConnectionMode::ProxyShared { flow_id },
            ..
        }) = wait_event(&mut client, &|e| {
            matches!(
                e,
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened { .. })
            )
        })
        .await
        else {
            panic!("expected ProxyShared opened tab");
        };
        assert_eq!(flow_id, live_flow_id);
        let tab_id = opened_tab.id;

        // Inject C2S and S2C on live ProxyShared connection.
        client
            .commands
            .send(Command::WebSocketRepeater(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: Some(live_flow_id),
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"injected-c2s".to_vec(),
                draft_revision: 2,
                cancel: CancellationToken::new(),
            }))
            .await
            .unwrap();
        let _ = wait_event(&mut client, &|e| {
            matches!(
                e,
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
                    draft_revision: 2,
                    result: Ok(_),
                    ..
                })
            )
        })
        .await;
        c2s_rx.await.unwrap();

        client
            .commands
            .send(Command::WebSocketRepeater(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: Some(live_flow_id),
                direction: WebSocketDirection::ServerToClient,
                kind: WebSocketKind::Text,
                payload: b"injected-s2c".to_vec(),
                draft_revision: 3,
                cancel: CancellationToken::new(),
            }))
            .await
            .unwrap();
        let _ = wait_event(&mut client, &|e| {
            matches!(
                e,
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
                    draft_revision: 3,
                    result: Ok(_),
                    ..
                })
            )
        })
        .await;
        let (op, client_received) = read_ws_frame(&mut ws_client, false).await;
        assert_eq!(
            (op, client_received.as_slice()),
            (1, b"injected-s2c".as_slice())
        );

        // Close live proxy connection and stop proxy so injector is reclaimed.
        drop(ws_client);
        client
            .commands
            .send(Command::StopProxy { project })
            .await
            .unwrap();
        let _ = wait_event(&mut client, &|e| {
            matches!(e, Event::ProxyState { address: None, .. })
        })
        .await;

        // Send after source drop must fail without auto-reconnecting.
        client
            .commands
            .send(Command::WebSocketRepeater(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: Some(live_flow_id),
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"must-fail".to_vec(),
                draft_revision: 4,
                cancel: CancellationToken::new(),
            }))
            .await
            .unwrap();
        let _ = wait_event(&mut client, &|e| {
            matches!(
                e,
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
                    draft_revision: 4,
                    result: Err(_),
                    ..
                })
            )
        })
        .await;
        assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

        // Explicit Connect opens Standalone session and exchanges frames.
        client
            .commands
            .send(Command::WebSocketRepeater(
                WebSocketRepeaterCommand::Connect {
                    project,
                    operation_id: Uuid::new_v4(),
                    tab_id,
                    url: String::new(),
                    handshake_headers: Vec::new(),
                    source_flow_id: None,
                    cancel: CancellationToken::new(),
                },
            ))
            .await
            .unwrap();
        let _ = wait_event(&mut client, &|e| {
            matches!(
                e,
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Connected {
                    mode: WebSocketRepeaterConnectionMode::Standalone { .. },
                    result: Ok(()),
                    ..
                })
            )
        })
        .await;

        client
            .commands
            .send(Command::WebSocketRepeater(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: None,
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"standalone-ping".to_vec(),
                draft_revision: 5,
                cancel: CancellationToken::new(),
            }))
            .await
            .unwrap();
        let mut got_sent = false;
        let mut got_recv = false;
        while !(got_sent && got_recv) {
            let ev = wait_event(&mut client, &|e| {
                matches!(
                    e,
                    Event::WebSocketRepeater(
                        WebSocketRepeaterEvent::Sent {
                            draft_revision: 5,
                            ..
                        } | WebSocketRepeaterEvent::Received { .. }
                    )
                )
            })
            .await;
            match ev {
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
                    draft_revision: 5,
                    result: Ok(item),
                    ..
                }) => {
                    assert_eq!(item.payload, b"standalone-ping");
                    got_sent = true;
                }
                Event::WebSocketRepeater(WebSocketRepeaterEvent::Received { item, .. }) => {
                    assert_eq!(item.payload, b"standalone-pong");
                    got_recv = true;
                }
                _ => {}
            }
        }

        assert_quiesce_and_resume_retains_tab(&mut client, project, tab_id).await;
        assert_offline_reopen_retains_tab(
            &mut client,
            project,
            tab_id,
            workspace,
            opened_tab,
            live_flow_id,
            &accepted_connections,
        )
        .await;

        origin.await.unwrap();
        runtime.shutdown().unwrap();
    })
    .await
    .unwrap();
}
