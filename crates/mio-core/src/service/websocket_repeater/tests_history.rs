use mio_plugin_api::WebSocketMessage;

use super::*;

#[tokio::test]
async fn open_from_history_extracts_ws_url_filters_hop_headers_and_supports_original_and_override()
{
    use crate::{
        Command, FlowRecord, FlowState, FlowSummary, HttpRequest, HttpResponse, HttpVersion,
        TrafficSource,
    };

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let project_path = dir.path().canonicalize().unwrap().join("ws-open.mio");
    let (events, mut rx) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();

    let http_flow_id = Uuid::new_v4();
    let https_flow_id = Uuid::new_v4();
    for (flow_id, url) in [
        (http_flow_id, "http://127.0.0.1:18080/chat?room=1"),
        (https_flow_id, "https://example.test:8443/live"),
    ] {
        let flow = FlowRecord {
            summary: FlowSummary {
                id: flow_id,
                revision: 1,
                source: TrafficSource::Proxy,
                method: "GET".into(),
                url: url.into(),
                status: Some(101),
                state: FlowState::Complete,
                request_bytes: 0,
                response_bytes: 0,
                started_unix_ms: 10,
                elapsed_ms: 2,
            },
            request: HttpRequest {
                method: "GET".into(),
                url: url.into(),
                version: HttpVersion::Http11,
                headers: vec![
                    Header {
                        name: "Host".into(),
                        value: b"example.test".to_vec(),
                    },
                    Header {
                        name: "Upgrade".into(),
                        value: b"websocket".to_vec(),
                    },
                    Header {
                        name: "Connection".into(),
                        value: b"Upgrade".to_vec(),
                    },
                    Header {
                        name: "Sec-WebSocket-Key".into(),
                        value: b"dGhlIHNhbXBsZSBub25jZQ==".to_vec(),
                    },
                    Header {
                        name: "Sec-WebSocket-Version".into(),
                        value: b"13".to_vec(),
                    },
                    Header {
                        name: "Sec-WebSocket-Extensions".into(),
                        value: b"permessage-deflate".to_vec(),
                    },
                    Header {
                        name: "Content-Length".into(),
                        value: b"0".to_vec(),
                    },
                    Header {
                        name: "Transfer-Encoding".into(),
                        value: b"chunked".to_vec(),
                    },
                    Header {
                        name: "Cookie".into(),
                        value: b"sid=secret".to_vec(),
                    },
                    Header {
                        name: "Origin".into(),
                        value: b"https://app.example.test".to_vec(),
                    },
                    Header {
                        name: "Authorization".into(),
                        value: b"Bearer tok".to_vec(),
                    },
                    Header {
                        name: "Sec-WebSocket-Protocol".into(),
                        value: b"json.v1".to_vec(),
                    },
                    Header {
                        name: "X-Custom".into(),
                        value: b"keep-me".to_vec(),
                    },
                ],
                body: Vec::new(),
                trailers: Vec::new(),
            },
            response: Some(HttpResponse {
                status: 101,
                version: HttpVersion::Http11,
                reason_phrase: None,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            }),
        };
        state.store.put(&flow).unwrap();
    }

    let msg_http = WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: http_flow_id,
        sequence: 1,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Text,
        payload: b"edited-frame".to_vec(),
        observed_unix_ms: 100,
    };
    let msg_https = WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: https_flow_id,
        sequence: 1,
        direction: WebSocketDirection::ServerToClient,
        kind: WebSocketKind::Binary,
        payload: vec![1, 2, 3],
        observed_unix_ms: 101,
    };
    state
        .capture(mio_proxy::ProxyEvent::WebSocketMessage {
            project: state.project,
            record: msg_http.clone(),
            original_payload: Some(b"original-frame".to_vec()),
            listener_port: 8080,
        })
        .await
        .unwrap();
    state
        .capture(mio_proxy::ProxyEvent::WebSocketMessage {
            project: state.project,
            record: msg_https.clone(),
            original_payload: None,
            listener_port: 8080,
        })
        .await
        .unwrap();

    state
        .command(Command::SaveProject {
            project: state.project,
            path: Some(project_path.clone()),
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .await
        .unwrap();
    let stale_project = state.project;
    state
        .command(Command::OpenProject { path: project_path })
        .await
        .unwrap();
    while rx.try_recv().is_ok() {}

    // Stale project must be rejected cleanly.
    let stale_op = Uuid::new_v4();
    state
        .command(Command::WebSocketRepeater(
            WebSocketRepeaterCommand::OpenFromHistory {
                project: stale_project,
                operation_id: stale_op,
                message_id: msg_http.id,
                use_original: false,
                payload_override: None,
            },
        ))
        .await
        .unwrap();
    assert!(matches!(
        rx.try_recv().unwrap(),
        Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
            project,
            operation_id,
            result: Err(_),
            ..
        }) if project == stale_project && operation_id == stale_op
    ));

    // 1) Edited payload from HTTP upgrade -> ws://
    state
        .command(Command::WebSocketRepeater(
            WebSocketRepeaterCommand::OpenFromHistory {
                project: state.project,
                operation_id: Uuid::new_v4(),
                message_id: msg_http.id,
                use_original: false,
                payload_override: None,
            },
        ))
        .await
        .unwrap();
    let Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
        result: Ok(tab1),
        mode: WebSocketRepeaterConnectionMode::Disconnected,
        ..
    }) = rx.try_recv().unwrap()
    else {
        panic!("expected opened tab1");
    };
    assert_eq!(tab1.title, "WS 1");
    assert_eq!(tab1.url, "ws://127.0.0.1:18080/chat?room=1");
    assert_eq!(tab1.draft_payload, b"edited-frame");
    assert_eq!(tab1.draft_kind, WebSocketKind::Text);
    assert_eq!(tab1.direction, WebSocketDirection::ClientToServer);
    let names: Vec<&str> = tab1
        .handshake_headers
        .iter()
        .map(|h| h.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "Host",
            "Cookie",
            "Origin",
            "Authorization",
            "Sec-WebSocket-Protocol",
            "X-Custom",
        ]
    );

    // 2) Original payload from same message
    state
        .command(Command::WebSocketRepeater(
            WebSocketRepeaterCommand::OpenFromHistory {
                project: state.project,
                operation_id: Uuid::new_v4(),
                message_id: msg_http.id,
                use_original: true,
                payload_override: None,
            },
        ))
        .await
        .unwrap();
    let Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
        result: Ok(tab2), ..
    }) = rx.try_recv().unwrap()
    else {
        panic!("expected opened tab2");
    };
    assert_eq!(tab2.title, "WS 2");
    assert_eq!(tab2.draft_payload, b"original-frame");

    // 3) Override payload from HTTPS upgrade -> wss://
    state
        .command(Command::WebSocketRepeater(
            WebSocketRepeaterCommand::OpenFromHistory {
                project: state.project,
                operation_id: Uuid::new_v4(),
                message_id: msg_https.id,
                use_original: true,
                payload_override: Some((WebSocketKind::Text, b"custom-draft".to_vec())),
            },
        ))
        .await
        .unwrap();
    let Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
        result: Ok(tab3), ..
    }) = rx.try_recv().unwrap()
    else {
        panic!("expected opened tab3");
    };
    assert_eq!(tab3.title, "WS 3");
    assert_eq!(tab3.url, "wss://example.test:8443/live");
    assert_eq!(tab3.draft_kind, WebSocketKind::Text);
    assert_eq!(tab3.draft_payload, b"custom-draft");
    assert_eq!(tab3.direction, WebSocketDirection::ServerToClient);
}
