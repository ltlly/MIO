use std::{
    net::Ipv4Addr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use mio_plugin_api::{Header, WebSocketMessage};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;

struct MockInjector {
    flow_id: FlowId,
    alive: AtomicBool,
    injected: Mutex<Vec<(WebSocketDirection, WebSocketKind, Vec<u8>)>>,
}

impl ProxyWebSocketInjector for MockInjector {
    fn contains(&self, flow_id: FlowId) -> bool {
        self.flow_id == flow_id && self.alive.load(Ordering::Acquire)
    }

    fn inject_directed(
        &self,
        flow_id: FlowId,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        _timeout: Duration,
    ) -> PinBoxInject {
        let ok = self.contains(flow_id);
        if ok {
            self.injected
                .lock()
                .unwrap()
                .push((direction, kind, payload.clone()));
        }
        Box::pin(async move {
            if !ok {
                return Err(mio_proxy::ProxyError::Network("源连接已断开".into()));
            }
            Ok(WebSocketMessage {
                id: Uuid::new_v4(),
                revision: 1,
                flow_id,
                sequence: 3,
                direction,
                kind,
                payload,
                observed_unix_ms: 1000,
            })
        })
    }
}

#[tokio::test]
async fn reuses_live_proxy_connection_then_reconnects_standalone_and_joins_on_project_stop() {
    const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
    const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(io.read_u8().await.unwrap());
        }
        let reply = format!(
            "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {ACCEPT}\r\n\r\n"
        );
        io.write_all(reply.as_bytes()).await.unwrap();
        // Read masked client text frame and echo unmasked server text + close on disconnect.
        let _fin_op = io.read_u8().await.unwrap();
        let len_mask = io.read_u8().await.unwrap();
        let len = (len_mask & 0x7f) as usize;
        let mut mask = [0u8; 4];
        io.read_exact(&mut mask).await.unwrap();
        let mut payload = vec![0u8; len];
        io.read_exact(&mut payload).await.unwrap();
        for (idx, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[idx % 4];
        }
        assert_eq!(payload, b"hello-standalone");
        let response_payload = b"server-push";
        let mut out = vec![0x81, response_payload.len() as u8];
        out.extend_from_slice(response_payload);
        io.write_all(&out).await.unwrap();
        let mut rest = [0u8; 64];
        let _ = io.read(&mut rest).await;
    });

    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let source_flow = Uuid::new_v4();
    let mock = Arc::new(MockInjector {
        flow_id: source_flow,
        alive: AtomicBool::new(true),
        injected: Mutex::new(Vec::new()),
    });
    let mut service = WebSocketRepeaterService::new(project);
    service.set_connect_config(SendConfig::default());
    service.set_test_injector(Some(mock.clone()));

    let opened = service
        .handle(WebSocketRepeaterCommand::Open {
            project,
            operation_id: Uuid::new_v4(),
            title: Some("1".into()),
            url: format!("ws://127.0.0.1:{}/ws", addr.port()),
            handshake_headers: vec![Header {
                name: "Sec-WebSocket-Key".into(),
                value: KEY.as_bytes().to_vec(),
            }],
            source_flow_id: Some(source_flow),
            source_message_id: None,
            source_direction: WebSocketDirection::ServerToClient,
            source_kind: WebSocketKind::Text,
            source_payload: b"inject-s2c".to_vec(),
        })
        .await
        .unwrap();
    let tab_id = match opened {
        WebSocketRepeaterEvent::Opened {
            result: Ok(tab),
            mode: WebSocketRepeaterConnectionMode::ProxyShared { flow_id },
            ..
        } => {
            assert_eq!(flow_id, source_flow);
            tab.id
        }
        other => panic!("unexpected open event: {other:?}"),
    };

    assert!(
        service
            .handle(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: Some(source_flow),
                direction: WebSocketDirection::ServerToClient,
                kind: WebSocketKind::Text,
                payload: b"inject-s2c".to_vec(),
                draft_revision: 1,
                cancel: CancellationToken::new(),
            })
            .await
            .is_none()
    );
    assert!(matches!(
        service.next_signal().await.unwrap(),
        WebSocketRepeaterEvent::Sent {
            result: Ok(ref item),
            ..
        } if item.direction == WebSocketDirection::ServerToClient && item.payload == b"inject-s2c"
    ));

    // Source connection drops; Send must fail without silent auto-reconnect.
    mock.alive.store(false, Ordering::Release);
    assert!(matches!(
        service
            .handle(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: Some(source_flow),
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"after-drop".to_vec(),
                draft_revision: 2,
                cancel: CancellationToken::new(),
            })
            .await,
        Some(WebSocketRepeaterEvent::Sent { result: Err(_), .. })
    ));

    // Reconnect establishes an independent standalone session and receives peer pushes.
    assert!(
        service
            .handle(WebSocketRepeaterCommand::Connect {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                handshake_headers: Vec::new(),
                source_flow_id: None,
                cancel: CancellationToken::new(),
            })
            .await
            .is_none()
    );
    assert!(matches!(
        service.next_signal().await.unwrap(),
        WebSocketRepeaterEvent::Connected {
            mode: WebSocketRepeaterConnectionMode::Standalone { .. },
            result: Ok(()),
            ..
        }
    ));
    assert!(
        service
            .handle(WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: None,
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"hello-standalone".to_vec(),
                draft_revision: 3,
                cancel: CancellationToken::new(),
            })
            .await
            .is_none()
    );
    let mut saw_sent = false;
    let mut saw_received = false;
    while !(saw_sent && saw_received) {
        match service.next_signal().await.unwrap() {
            WebSocketRepeaterEvent::Sent {
                result: Ok(item), ..
            } => {
                assert_eq!(item.payload, b"hello-standalone");
                saw_sent = true;
            }
            WebSocketRepeaterEvent::Received { item, .. } => {
                assert_eq!(item.payload, b"server-push");
                saw_received = true;
            }
            other => panic!("unexpected signal: {other:?}"),
        }
    }

    // Reopen restores history offline without opening any network session.
    let saved_tabs = service.workspace_tabs(project);
    assert_eq!(saved_tabs[0].history.len(), 3);
    service.stop_project(project).await.unwrap();
    assert!(!service.has_resources());
    server.await.unwrap();

    let reopened_project = ProjectKey {
        id: project.id,
        generation: 2,
    };
    service.set_project(reopened_project);
    service.restore_workspace_tabs(reopened_project, saved_tabs);
    assert!(!service.has_resources());
    assert_eq!(
        service.tab_mode(reopened_project, tab_id),
        WebSocketRepeaterConnectionMode::Disconnected
    );
}
