use super::*;

#[test]
fn byte_character_search_counts_exactly_and_preserves_offscreen_selection() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut records = Vec::new();
    for (index, payload) in [
        "WS 合成".as_bytes(),
        "WS 合成".as_bytes(),
        &[255, 0],
        &[255, 0],
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = message(flow.summary.id, index as u64 + 1);
        record.payload = payload.to_vec();
        record.direction = if index % 2 == 0 {
            WebSocketDirection::ServerToClient
        } else {
            WebSocketDirection::ClientToServer
        };
        store
            .put_captured_websocket(&CapturedWebSocket {
                record: record.clone().into(),
                listener_port: Some(9091),
                original_payload: None,
            })
            .unwrap();
        records.push(record);
    }
    for (term, regex, case_sensitive, negative, count, first) in [
        ("WS", false, false, false, 2, Some(1)),
        ("ws", false, false, false, 2, Some(1)),
        ("ws", false, true, false, 0, None),
        ("合成", false, false, false, 2, Some(1)),
        (r"\xE5\x90\x88", true, false, false, 2, Some(1)),
        (r"\xFF", true, false, false, 2, Some(3)),
        ("ÿ", false, true, false, 0, None),
        ("合成|WS", true, false, false, 2, Some(1)),
        (r"\xFF", true, false, true, 2, Some(1)),
    ] {
        let request = query(
            &store,
            WebSocketHistoryFilter {
                term: term.into(),
                regex,
                case_sensitive,
                negative,
                ..Default::default()
            },
        );
        let result = ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &request,
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            (result.total, result.rows.first().map(|row| row.ordinal)),
            (count, first),
            "{term}"
        );
        assert_eq!(result.selected_match, None);
    }
    let mut request = query(
        &store,
        WebSocketHistoryFilter {
            term: "WS".into(),
            ..Default::default()
        },
    );
    request.selected_message_id = Some(records[1].id);
    let result = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &request,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(result.rows[0].id, records[0].id);
    assert_eq!(result.selected_match, Some((records[1].id, true)));
    request.offset = 1;
    request.filter.hide_incoming = true;
    let result = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &request,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert!(result.rows.is_empty());
    assert_eq!(result.selected_match, Some((records[1].id, true)));
    request.selected_message_id = Some(records[0].id);
    assert_eq!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &request,
            &[],
            &CancellationToken::new()
        )
        .unwrap()
        .selected_match,
        Some((records[0].id, false))
    );
    request.filter.term.clear();
    request.filter.listener_port = Some(9092);
    let result = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &request,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        (result.total, result.selected_match),
        (0, Some((records[0].id, false)))
    );
    for record in records {
        assert_eq!(store.read_websocket(record.id).unwrap(), record);
    }
}

#[test]
fn one_read_snapshot_keeps_total_page_and_selection_consistent_during_append() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut first = message(flow.summary.id, 1);
    first.payload = b"match".to_vec();
    store.put_websocket(&first).unwrap();
    let reader =
        Connection::open_with_flags(store.actual_path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let transaction = reader.unchecked_transaction().unwrap();
    assert_eq!(
        transaction
            .query_row("SELECT count(*) FROM websocket_messages", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    let mut second = message(flow.summary.id, 2);
    second.payload = b"match".to_vec();
    store.put_websocket(&second).unwrap();
    let mut request = query(
        &store,
        WebSocketHistoryFilter {
            term: "match".into(),
            ..Default::default()
        },
    );
    request.selected_message_id = Some(second.id);
    let (select, values) = history_sql(None, &[], &[], &request.filter).unwrap();
    let result = search_page(
        &transaction,
        &request,
        &[],
        select,
        values,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        (result.total, result.rows[0].id, result.selected_match),
        (1, first.id, Some((second.id, false)))
    );
    transaction.commit().unwrap();
    let result = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &request,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        (result.total, result.selected_match),
        (2, Some((second.id, true)))
    );
}

#[test]
fn recovery_search_overrides_stored_duplicates_and_does_not_make_conflicts_saveable() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut original = message(flow.summary.id, 1);
    original.payload = b"WS original".to_vec();
    store.put_websocket(&original).unwrap();
    let mut replaced = original.clone();
    replaced.payload = b"different".to_vec();
    let mut appended = message(flow.summary.id, 2);
    appended.payload = b"WS pending".to_vec();
    let pending: Vec<_> = [replaced, appended]
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let summary = WebSocketSummary::from_message(
                &record,
                flow.summary.url.clone(),
                index as u64 + 1,
                1,
                None,
                false,
            );
            (summary, CapturedWebSocket::from(record))
        })
        .collect();
    let mut request = query(
        &store,
        WebSocketHistoryFilter {
            term: "WS".into(),
            ..Default::default()
        },
    );
    request.selected_message_id = Some(original.id);
    let result = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &request,
        &pending,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        (result.total, result.rows[0].ordinal, result.selected_match),
        (1, 2, Some((original.id, false)))
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conflicting-search.mio");
    let captures: Vec<_> = pending.into_iter().map(|(_, capture)| capture).collect();
    assert!(
        store
            .save_with(
                Some(&path),
                None,
                &[],
                &captures,
                &[],
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(!path.exists());
    assert_eq!(store.read_websocket(original.id).unwrap(), original);
}

#[test]
fn invalid_search_patterns_identity_and_corrupt_payload_fail_explicitly() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let record = message(flow.summary.id, 1);
    store.put_websocket(&record).unwrap();
    for pattern in [
        "(".into(),
        "(?=WS)".into(),
        "a{10000000}".into(),
        format!("{}a{}", "(".repeat(129), ")".repeat(129)),
    ] {
        let request = query(
            &store,
            WebSocketHistoryFilter {
                term: pattern,
                regex: true,
                ..Default::default()
            },
        );
        let error = ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &request,
            &[],
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("正则表达式"));
    }
    let mut request = query(
        &store,
        WebSocketHistoryFilter {
            term: "x".repeat(4097),
            ..Default::default()
        },
    );
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &request,
            &[],
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("4096")
    );
    request.filter.term = "WS".into();
    request.project.id = Uuid::new_v4();
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &request,
            &[],
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("ID")
    );
    request.project.id = store.id;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        ProjectStore::websocket_query_snapshot(&store.actual_path(), &request, &[], &cancelled)
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
    store
        .connection
        .execute(
            "DELETE FROM websocket_payloads WHERE message=?1",
            [record.id.to_string()],
        )
        .unwrap();
    request.filter.negative = true;
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &request,
            &[],
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("正文缺失")
    );
}

#[test]
fn maximum_body_byte_view_is_bounded_and_observes_cancellation() {
    let payload = vec![255; MAX_BODY];
    let cancel = CancellationToken::new();
    let view = byte_character_view(&payload, &cancel).unwrap();
    assert_eq!(view.len(), MAX_BODY);
    assert_eq!(view.first(), Some(&255));
    assert_eq!(view.last(), Some(&255));
    cancel.cancel();
    assert!(byte_character_view(&payload, &cancel).is_err());

    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut c2s = message(flow.summary.id, 1);
    c2s.direction = WebSocketDirection::ClientToServer;
    c2s.payload = b"token=secret".to_vec();
    let mut s2c = message(flow.summary.id, 2);
    s2c.direction = WebSocketDirection::ServerToClient;
    s2c.payload = b"pong".to_vec();
    for rec in [&c2s, &s2c] {
        store
            .put_captured_websocket(&CapturedWebSocket {
                record: rec.clone().into(),
                listener_port: Some(9091),
                original_payload: None,
            })
            .unwrap();
    }
    let script = crate::convert_ws_filter_to_script(
        &WebSocketHistoryFilter {
            hide_incoming: true,
            term: "secret".into(),
            ..Default::default()
        },
        false,
    );
    assert!(crate::eval_ws_filter_script(
        &script,
        &flow.summary.url,
        WebSocketDirection::ClientToServer,
        b"token=secret",
        false,
        "",
        Some(9091),
    )
    .unwrap());
    let (total, rows) = store
        .websocket_page_with_recovery(
            None,
            0,
            10,
            &[],
            &[],
            &WebSocketHistoryFilter {
                filter_script: script,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, c2s.id);
}
