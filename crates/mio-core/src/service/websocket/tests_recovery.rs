use super::tests::*;
use super::*;

#[tokio::test]
async fn recovery_connection_and_message_numbers_survive_filtering_duplicates_and_save_as() {
    let (events, _receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let first_flow = handshake();
    let second_flow = handshake();
    state.store.put(&first_flow).unwrap();
    state.store.put(&second_flow).unwrap();
    let first = message(first_flow.summary.id, 1);
    let second = message(second_flow.summary.id, 1);
    let mut third = message(first_flow.summary.id, 2);
    third.direction = WebSocketDirection::ServerToClient;
    state
        .capture_websocket(
            state.project,
            CapturedWebSocket {
                record: first.clone().into(),
                listener_port: Some(9091),
                original_payload: None,
            },
        )
        .unwrap();
    state.store.reject_writes(true);
    for (record, listener_port) in [
        (&second, 9092),
        (&third, 9091),
        (&second, 9092),
        (&first, 9091),
    ] {
        assert!(
            state
                .capture_websocket(
                    state.project,
                    CapturedWebSocket {
                        record: record.clone().into(),
                        listener_port: Some(listener_port),
                        original_payload: None,
                    }
                )
                .is_err()
        );
    }
    let before = state.websocket_page(None, 0, 10).unwrap();
    assert_eq!(before.0, 3);
    assert_eq!(
        before
            .1
            .iter()
            .map(|row| (row.id, row.ordinal, row.connection_id, row.listener_port))
            .collect::<Vec<_>>(),
        [
            (first.id, 1, 1, Some(9091)),
            (second.id, 2, 2, Some(9092)),
            (third.id, 3, 1, Some(9091))
        ]
    );
    assert_eq!(
        state.websocket_page(Some(first.flow_id), 1, 1).unwrap(),
        (2, vec![before.1[2].clone()])
    );
    assert_eq!(
        state.websocket_page(None, 1, 1).unwrap(),
        (3, vec![before.1[1].clone()])
    );
    let filter = WebSocketHistoryFilter {
        hide_outgoing: true,
        listener_port: Some(9091),
        ..Default::default()
    };
    let filtered = state
        .websocket_page_with_filter(None, 0, 1, &filter)
        .unwrap();
    assert_eq!(filtered, (1, vec![before.1[2].clone()]));
    assert_eq!(
        state
            .websocket_page_with_filter(None, 1, 1, &filter)
            .unwrap(),
        (1, vec![])
    );
    let pending_search = state.websocket_recovery_snapshot().unwrap();
    let pending_second = pending_search
        .iter()
        .find(|(row, _)| row.id == second.id)
        .unwrap();
    let latest_second = state
        .websocket_recovery
        .iter()
        .rev()
        .find(|capture| capture.record.id == second.id)
        .unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(
            &latest_second.record,
            &state.websocket_recovery.front().unwrap().record
        ),
        "duplicate fixture records must own distinct Arc allocations"
    );
    assert!(std::sync::Arc::ptr_eq(
        &pending_second.1.record,
        &latest_second.record
    ));
    assert_eq!(pending_second.1.record.as_ref(), &second);
    let search = WebSocketHistoryQuery {
        project: state.project,
        query_revision: 20,
        filter: WebSocketHistoryFilter {
            term: r"\x00".into(),
            regex: true,
            ..filter.clone()
        },
        flow_id: None,
        offset: 0,
        limit: 1,
        selected_message_id: Some(third.id),
    };
    let result = ProjectStore::websocket_query_snapshot(
        &state.store.websocket_query_path(),
        &search,
        &pending_search,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(result.selected_match, Some((third.id, true)));
    let search_before = (result.total, result.rows, result.selected_match);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery-numbers.mio");
    state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(path.clone()),
            workspace: None,
        })
        .await
        .unwrap();
    assert_eq!(state.websocket_page(None, 0, 10).unwrap(), before);
    assert_eq!(
        state
            .websocket_page_with_filter(None, 0, 1, &filter)
            .unwrap(),
        filtered
    );
    let reopened = ProjectStore::open(&path).unwrap();
    let result = ProjectStore::websocket_query_snapshot(
        &reopened.websocket_query_path(),
        &search,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        (result.total, result.rows, result.selected_match),
        search_before
    );
    assert_eq!(reopened.websocket_page(None, 0, 10, &[]).unwrap(), before);
    assert_eq!(
        reopened
            .websocket_page_with_recovery(None, 0, 1, &[], &[], &filter)
            .unwrap(),
        filtered
    );
    for record in [first, second, third] {
        assert_eq!(reopened.read_websocket(record.id).unwrap(), record);
    }
}

#[tokio::test]
async fn unsaved_websocket_messages_remain_readable_and_save_as_recovers_them_atomically() {
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut flow = handshake();
    state.store.put(&flow).unwrap();
    let first = message(flow.summary.id, 1);
    state
        .capture(ProxyEvent::WebSocketMessage {
            listener_port: 9091,
            project: state.project,
            record: first.clone(),
            original_payload: None,
        })
        .await
        .unwrap();
    state.store.reject_writes(true);
    let mut second = message(flow.summary.id, 2);
    second.kind = WebSocketKind::Text;
    second.direction = WebSocketDirection::ServerToClient;
    second.payload = "最后确认的中文".as_bytes().to_vec();
    let mut third = message(flow.summary.id, 3);
    third.kind = WebSocketKind::Close;
    third.payload = vec![3, 232];
    for record in [second.clone(), third.clone()] {
        assert!(
            state
                .capture(ProxyEvent::WebSocketMessage {
                    listener_port: 9091,
                    project: state.project,
                    record,
                    original_payload: None,
                })
                .await
                .is_err()
        );
    }
    flow.summary.revision += 1;
    flow.summary.state = FlowState::Cancelled;
    assert!(state.persist(&flow).is_err());
    assert_eq!(state.unsaved_count(), 3);
    state.storage_state().await;
    assert!(drain(&mut receiver).iter().any(|event| matches!(
        event,
        Event::StorageState {
            pending_records: 3,
            message: Some(_),
            ..
        }
    )));
    assert!(state.require_saved().is_err());
    assert!(
        state
            .command(Command::NewProject { path: None })
            .await
            .is_err(),
        "changing project must not discard unsaved WebSocket payloads"
    );
    let before = state.websocket_page(None, 0, 10).unwrap();
    assert_eq!(before.0, 3);
    assert_eq!(
        before.1.iter().map(|row| row.id).collect::<Vec<_>>(),
        [first.id, second.id, third.id]
    );
    assert_eq!(
        state.websocket_page(None, 1, 1).unwrap().1,
        [before.1[1].clone()]
    );
    assert_eq!(
        state.websocket_page(None, 2, 1).unwrap().1,
        [before.1[2].clone()]
    );
    state
        .command(Command::ReadWebSocket {
            project: state.project,
            selection_revision: 19,
            message_id: second.id,
        })
        .await
        .unwrap();
    assert!(matches!(receiver.try_recv().unwrap(),
        Event::WebSocketDetail { selection_revision: 19, record, .. } if *record == second));

    let directory = tempfile::tempdir().unwrap();
    let occupied = directory.path().join("occupied.mio");
    std::fs::write(&occupied, b"existing file").unwrap();
    assert!(
        state
            .command(Command::SaveProject {
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project: state.project,
                path: Some(occupied.clone()),
                workspace: None,
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(occupied).unwrap(), b"existing file");
    assert_eq!(state.unsaved_count(), 3);
    let path = directory.path().join("recovered.mio");
    state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(path.clone()),
            workspace: None,
        })
        .await
        .unwrap();
    assert_eq!(state.unsaved_count(), 0);
    assert_eq!(state.websocket_page(None, 0, 10).unwrap(), before);
    let reopened = ProjectStore::open(&path).unwrap();
    for record in [first, second, third] {
        assert_eq!(state.read_websocket(record.id).unwrap(), record);
        assert_eq!(reopened.read_websocket(record.id).unwrap(), record);
    }
    assert_eq!(reopened.read(flow.summary.id).unwrap(), flow);
}
