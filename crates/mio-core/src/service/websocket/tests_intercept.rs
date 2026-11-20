use super::tests::*;
use super::*;

#[tokio::test]
async fn websocket_intercept_decide_validates_utf8_persists_original_and_clears_on_stop() {
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let flow = handshake();
    state.store.put(&flow).unwrap();
    let mut held = message(flow.summary.id, 1);
    held.kind = WebSocketKind::Text;
    held.payload = b"hello-orig".to_vec();
    let (tx, rx) = oneshot::channel();
    state
        .capture(ProxyEvent::WebSocketIntercepted {
            project: state.project,
            url: flow.request.url.clone(),
            record: held.clone(),
            listener_port: 9091,
            reply: tx,
        })
        .await
        .unwrap();
    assert!(drain(&mut receiver).into_iter().any(|event| matches!(
        event,
        Event::WebSocketInterceptPending { ref url, ref record, .. }
            if url == &flow.request.url && **record == held
    )));

    // Invalid UTF-8 on Text frame must fail without consuming the pending intercept!
    let bad = state
        .command(Command::DecideWebSocket {
            project: state.project,
            message_id: held.id,
            decision: WebSocketDecision::Forward(vec![0xff, 0xfe]),
        })
        .await;
    assert!(bad.unwrap_err().to_string().contains("UTF-8"));
    assert!(state.pending_websockets.contains_key(&held.id));

    // Valid UTF-8 Forward sends decision and resolves intercept.
    state
        .command(Command::DecideWebSocket {
            project: state.project,
            message_id: held.id,
            decision: WebSocketDecision::Forward(b"hello-edited".to_vec()),
        })
        .await
        .unwrap();
    assert!(matches!(
        rx.await.unwrap(),
        mio_proxy::WebSocketInterceptDecision::Forward(ref bytes) if bytes == b"hello-edited"
    ));
    assert!(drain(&mut receiver).into_iter().any(|event| matches!(
        event,
        Event::WebSocketInterceptResolved { message_id, .. } if message_id == held.id
    )));

    let mut captured = held.clone();
    captured.payload = b"hello-edited".to_vec();
    state
        .capture(ProxyEvent::WebSocketMessage {
            project: state.project,
            record: captured.clone(),
            original_payload: Some(b"hello-orig".to_vec()),
            listener_port: 9091,
        })
        .await
        .unwrap();
    let _ = drain(&mut receiver);
    state
        .command(Command::ReadWebSocket {
            project: state.project,
            selection_revision: 7,
            message_id: captured.id,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WebSocketDetail {
            selection_revision: 7,
            ref record,
            original_payload: Some(ref orig),
            ..
        } if **record == captured && orig == b"hello-orig"
    ));
    let (_, rows) = state.websocket_page(None, 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].edited);

    // Pending WebSocket intercept is resolved if proxy finishes stopping.
    let held2 = message(flow.summary.id, 2);
    let (tx2, _rx2) = oneshot::channel();
    state
        .capture(ProxyEvent::WebSocketIntercepted {
            project: state.project,
            url: flow.request.url.clone(),
            record: held2.clone(),
            listener_port: 9091,
            reply: tx2,
        })
        .await
        .unwrap();
    let _ = drain(&mut receiver);
    state.finish_proxy_stop(true, Ok(())).unwrap();
    state.flush_proxy_notifications().await;
    assert!(drain(&mut receiver).into_iter().any(|event| matches!(
        event,
        Event::WebSocketInterceptResolved { message_id, .. } if message_id == held2.id
    )));
}

#[tokio::test]
async fn recovery_pagination_and_search_keep_edited_original_flags() {
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let flow = handshake();
    state.store.put(&flow).unwrap();
    let mut stored_edited = message(flow.summary.id, 1);
    stored_edited.payload = b"edited-final".to_vec();
    state
        .store
        .put_captured_websocket(&CapturedWebSocket {
            record: stored_edited.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(b"edited-original".to_vec())),
        })
        .unwrap();
    let mut pending_edited = message(flow.summary.id, 2);
    pending_edited.payload = b"pending-final".to_vec();
    state.store.reject_writes(true);
    assert!(
        state
            .capture_websocket(
                state.project,
                CapturedWebSocket {
                    record: pending_edited.clone().into(),
                    listener_port: Some(9091),
                    original_payload: Some(Arc::new(b"pending-original".to_vec())),
                },
            )
            .is_err()
    );
    let _ = drain(&mut receiver);

    let (_, rows) = state.websocket_page(None, 0, 10).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.id, row.edited))
            .collect::<Vec<_>>(),
        [(stored_edited.id, true), (pending_edited.id, true)]
    );
    let pending = state.websocket_recovery_snapshot().unwrap();
    for (term, regex, expected) in [
        ("edited-final", false, Some(stored_edited.id)),
        ("edited-original", false, Some(stored_edited.id)),
        ("pending-final", false, Some(pending_edited.id)),
        ("pending-original", false, Some(pending_edited.id)),
        ("final", true, Some(stored_edited.id)),
    ] {
        let request = WebSocketHistoryQuery {
            project: state.project,
            query_revision: 11,
            filter: WebSocketHistoryFilter {
                term: term.into(),
                regex,
                ..Default::default()
            },
            flow_id: None,
            offset: 0,
            limit: 10,
            selected_message_id: None,
        };
        let result = ProjectStore::websocket_query_snapshot(
            &state.store.websocket_query_path(),
            &request,
            &pending,
            &CancellationToken::new(),
        )
        .unwrap();
        let expected_total = if term == "final" { 2 } else { 1 };
        assert_eq!(result.total, expected_total, "{term}");
        assert_eq!(result.rows[0].id, expected.unwrap(), "{term}");
        assert!(result.rows[0].edited, "{term}");
    }
}
