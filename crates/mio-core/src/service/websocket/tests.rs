use super::*;

pub(super) async fn deliver_query(state: &mut State) {
    loop {
        let signal = tokio::time::timeout(
            Duration::from_secs(3),
            state.queries.websocket.next_signal(&state.events, false),
        )
        .await
        .unwrap();
        let delivered = matches!(signal, super::super::websocket_query::Signal::Ready(_));
        state.websocket_query_signal(signal);
        if delivered {
            break;
        }
    }
}

pub(super) fn handshake() -> FlowRecord {
    let request = parse_request(
        b"GET /socket HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "http://localhost/socket",
        HttpVersion::Http11,
    )
    .unwrap();
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: request.method.clone(),
            url: request.url.clone(),
            status: Some(101),
            state: FlowState::Pending,
            request_bytes: 0,
            response_bytes: 0,
            started_unix_ms: 1,
            elapsed_ms: 0,
        },
        request,
        response: Some(HttpResponse {
            status: 101,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: Vec::new(),
            trailers: Vec::new(),
            body: Vec::new(),
        }),
    }
}

pub(super) fn message(flow_id: FlowId, sequence: u64) -> WebSocketMessage {
    WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id,
        sequence,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Binary,
        payload: (0..=255).collect(),
        observed_unix_ms: sequence as i64,
    }
}

pub(super) fn drain(events: &mut mpsc::Receiver<Event>) -> Vec<Event> {
    let mut output = Vec::new();
    while let Ok(event) = events.try_recv() {
        output.push(event);
    }
    output
}

#[tokio::test]
async fn notes_commands_return_independent_revisions_and_current_conflict_values() {
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let flow = handshake();
    let record = message(flow.summary.id, 1);
    state.store.put(&flow).unwrap();
    state.store.put_websocket(&record).unwrap();
    state
        .command(Command::ReadWebSocketNotes {
            project: state.project,
            selection_revision: 1,
            message_id: record.id,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::WebSocketNotes { selection_revision: 1, result: Ok(WebSocketNotes { revision: 0, ref text }), .. } if text.is_empty())
    );
    let operation_id = Uuid::new_v4();
    let text = "第一行\n备注🙂".to_owned();
    state
        .command(Command::SetWebSocketNotes {
            project: state.project,
            operation_id,
            message_id: record.id,
            expected_notes_revision: 0,
            text: text.clone(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(drain(&mut receiver).into_iter().any(|event| matches!(event, Event::WebSocketNotesChanged { operation_id: actual, message_id, result: Ok(WebSocketNotes { revision: 1, text: actual_text }), .. } if actual == operation_id && message_id == record.id && actual_text == text)));
    state
        .command(Command::SetWebSocketNotes {
            project: state.project,
            operation_id: Uuid::new_v4(),
            message_id: record.id,
            expected_notes_revision: 0,
            text: "stale".into(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::WebSocketNotesChanged { result: Err(WebSocketNotesError { stored: Some(WebSocketNotes { revision: 1, text: actual }), .. }), .. } if actual == text)
    );
    let old = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    state
        .command(Command::ReadWebSocketNotes {
            project: old,
            selection_revision: 2,
            message_id: record.id,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WebSocketNotes { result: Err(_), .. }
    ));
    state
        .command(Command::SetWebSocketNotes {
            project: state.project,
            operation_id: Uuid::nil(),
            message_id: record.id,
            expected_notes_revision: 1,
            text: "invalid operation".into(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WebSocketNotesChanged { result: Err(_), .. }
    ));
    assert_eq!(state.read_websocket(record.id).unwrap(), record);
    assert_eq!(
        state
            .read_websocket_notes(state.project, record.id)
            .unwrap()
            .text,
        text
    );
}

#[tokio::test]
async fn recovery_notes_refuse_autosave_but_commit_atomically_with_save_as() {
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let flow = handshake();
    state.store.put(&flow).unwrap();
    let record = message(flow.summary.id, 1);
    state.store.reject_writes(true);
    assert!(
        state
            .capture_websocket(
                state.project,
                CapturedWebSocket {
                    record: record.clone().into(),
                    listener_port: Some(9091),
                    original_payload: None,
                }
            )
            .is_err()
    );
    state
        .command(Command::ReadWebSocketNotes {
            project: state.project,
            selection_revision: 3,
            message_id: record.id,
        })
        .await
        .unwrap();
    assert!(drain(&mut receiver).into_iter().any(|event| matches!(event, Event::WebSocketNotes { result: Ok(WebSocketNotes { revision: 0, ref text }), .. } if text.is_empty())));
    let text = "保留草稿并另存为\n🙂".to_owned();
    state
        .command(Command::SetWebSocketNotes {
            project: state.project,
            operation_id: Uuid::new_v4(),
            message_id: record.id,
            expected_notes_revision: 0,
            text: text.clone(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WebSocketNotesChanged {
            result: Err(WebSocketNotesError { stored: None, .. }),
            ..
        }
    ));
    let directory = tempfile::tempdir().unwrap();
    state
        .command(Command::SaveProject {
            project: state.project,
            path: Some(directory.path().join("recovery-notes.mio")),
            workspace: None,
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            websocket_notes: vec![WebSocketNotesChange {
                message_id: record.id,
                expected_notes_revision: 0,
                text: text.clone(),
            }],
        })
        .await
        .unwrap();
    assert!(drain(&mut receiver).into_iter().any(|event| matches!(event, Event::ProjectSaved { websocket_notes, .. } if websocket_notes == [(record.id, 1)])));
    assert_eq!(state.unsaved_count(), 0);
    assert_eq!(
        state
            .read_websocket_notes(state.project, record.id)
            .unwrap(),
        WebSocketNotes { revision: 1, text }
    );
    assert_eq!(state.read_websocket(record.id).unwrap(), record);
}

#[tokio::test]
async fn websocket_changes_are_coalesced_and_old_project_results_are_rejected() {
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let flow = handshake();
    state.store.put(&flow).unwrap();
    let first = message(flow.summary.id, 1);
    let second = message(flow.summary.id, 2);
    for record in [first.clone(), second.clone()] {
        state
            .capture(ProxyEvent::WebSocketMessage {
                listener_port: 9091,
                project: state.project,
                record,
                original_payload: None,
            })
            .await
            .unwrap();
    }
    assert_eq!(
        drain(&mut receiver)
            .iter()
            .filter(|event| matches!(event, Event::WebSocketHistoryChanged { .. }))
            .count(),
        1
    );
    let old_project = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    state
        .capture(ProxyEvent::WebSocketMessage {
            listener_port: 9091,
            project: old_project,
            record: message(flow.summary.id, 3),
            original_payload: None,
        })
        .await
        .unwrap();
    assert!(drain(&mut receiver).is_empty());
    state
        .command(Command::ReadWebSocket {
            project: old_project,
            selection_revision: 4,
            message_id: first.id,
        })
        .await
        .unwrap();
    assert!(matches!(receiver.try_recv().unwrap(),
        Event::WebSocketDetailError { project, selection_revision: 4, message_id, .. }
        if project == old_project && message_id == first.id));
    state
        .command(Command::QueryWebSocketHistory {
            selected_message_id: None,
            filter: Default::default(),
            project: state.project,
            query_revision: 11,
            flow_id: None,
            offset: 0,
            limit: 0,
        })
        .await
        .unwrap();
    deliver_query(&mut state).await;
    assert!(matches!(receiver.try_recv().unwrap(),
        Event::WebSocketHistoryError { project, query_revision: 11, .. }
        if project == state.project));
    let filter = WebSocketHistoryFilter {
        hide_incoming: true,
        listener_port: Some(9091),
        ..Default::default()
    };
    state
        .command(Command::QueryWebSocketHistory {
            selected_message_id: None,
            filter: filter.clone(),
            project: state.project,
            query_revision: 12,
            flow_id: Some(flow.summary.id),
            offset: 1,
            limit: 1,
        })
        .await
        .unwrap();
    deliver_query(&mut state).await;
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WebSocketHistoryPage { query_revision: 12, offset: 1, total: 2, rows, filter: echoed, .. }
            if echoed == filter && rows.len() == 1 && rows[0].id == second.id && rows[0].payload_bytes == 256
    ));
    state
        .command(Command::QueryWebSocketHistory {
            selected_message_id: None,
            project: state.project,
            query_revision: 13,
            filter: WebSocketHistoryFilter {
                listener_port: Some(0),
                ..Default::default()
            },
            flow_id: None,
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();
    deliver_query(&mut state).await;
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::WebSocketHistoryError {
        query_revision: 13, message, ..
    } if message.contains("监听端口"))
    );
    state
        .capture(ProxyEvent::WebSocketMessage {
            listener_port: 9091,
            project: state.project,
            record: message(flow.summary.id, 3),
            original_payload: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WebSocketHistoryChanged { .. }
    ));
    assert!(state.websocket_page(None, 0, 0).is_err());
    assert!(state.websocket_page(None, 0, 100_001).is_err());
}
