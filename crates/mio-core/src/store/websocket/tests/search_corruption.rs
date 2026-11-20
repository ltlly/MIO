use super::*;

#[test]
fn corrupted_original_ownership_length_and_duplicate_payloads_fail_explicitly() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut edited = message(flow.summary.id, 1);
    edited.id = Uuid::from_u128(0xabcdef);
    store
        .put_captured_websocket(&CapturedWebSocket {
            record: edited.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(b"original".to_vec())),
        })
        .unwrap();
    let plain = message(flow.summary.id, 2);
    store.put_websocket(&plain).unwrap();

    store.connection.authorizer(Some(deny_body_reads));
    assert!(
        store.read_websocket_detail(edited.id).is_err(),
        "detail reads must not bypass the payload authorizer"
    );
    let request = query(
        &store,
        WebSocketHistoryFilter {
            term: "original".into(),
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
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].id, edited.id);
    assert!(result.rows[0].edited);
    let page = store.websocket_page(None, 0, 10, &[]).unwrap();
    assert_eq!(page.0, 2);
    assert!(
        page.1
            .iter()
            .find(|row| row.id == edited.id)
            .unwrap()
            .edited
    );
    assert!(!page.1.iter().find(|row| row.id == plain.id).unwrap().edited);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE websocket_originals SET body=zeroblob(?1)",
            [MAX_BODY as i64 + 1],
        )
        .unwrap();
    assert!(
        store
            .read_websocket_detail(edited.id)
            .unwrap_err()
            .to_string()
            .contains("16 MiB")
    );
    let oversized_search = query(
        &store,
        WebSocketHistoryFilter {
            term: "original".into(),
            ..Default::default()
        },
    );
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &oversized_search,
            &[],
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("16 MiB"),
        "search must refuse oversized originals instead of pretending no match"
    );
    store
        .connection
        .execute("UPDATE websocket_originals SET body='text'", [])
        .unwrap();
    assert!(
        store
            .read_websocket_detail(edited.id)
            .unwrap_err()
            .to_string()
            .contains("二进制")
    );
    let corrupt_search = query(
        &store,
        WebSocketHistoryFilter {
            term: "original".into(),
            ..Default::default()
        },
    );
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &corrupt_search,
            &[],
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("二进制"),
        "search must refuse corrupt originals even when the final payload matches"
    );
    store
        .connection
        .execute(
            "UPDATE websocket_originals SET body=x'6f726967696e616c'",
            [],
        )
        .unwrap();

    let pending_oversized = message(flow.summary.id, 3);
    let oversized_original = vec![b'x'; MAX_BODY + 1];
    let pending_summary = WebSocketSummary::from_message(
        &pending_oversized,
        flow.summary.url.clone(),
        3,
        1,
        None,
        true,
    );
    let pending = [(
        pending_summary,
        CapturedWebSocket {
            record: pending_oversized.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(oversized_original)),
        },
    )];
    let pending_request = query(
        &store,
        WebSocketHistoryFilter {
            term: "hit".into(),
            ..Default::default()
        },
    );
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &pending_request,
            &pending,
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("16 MiB"),
        "pending originals must not bypass the 16 MiB limit"
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &pending_request,
            &pending,
            &cancelled
        )
        .unwrap_err()
        .to_string()
        .contains("取消")
    );
    store
        .connection
        .execute("DELETE FROM websocket_originals", [])
        .unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", false)
        .unwrap();
    store
        .connection
        .execute("UPDATE websocket_payloads SET body=x'0002ff'", [])
        .unwrap();
    assert!(
        store
            .read_websocket_detail(plain.id)
            .unwrap_err()
            .to_string()
            .contains("长度")
    );
    let mismatched_search = query(
        &store,
        WebSocketHistoryFilter {
            term: "WS".into(),
            ..Default::default()
        },
    );
    assert!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &mismatched_search,
            &[],
            &CancellationToken::new()
        )
        .unwrap_err()
        .to_string()
        .contains("长度"),
        "search must refuse length-mismatched payloads instead of hiding the row"
    );
}

#[test]
fn duplicate_original_ownership_fails_explicitly_on_nocase_parent_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nocase-originals.mio");
    ProjectStore::create(Some(&path)).unwrap().close().unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF; DROP TABLE websocket_messages;
             CREATE TABLE websocket_messages (
                 ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
                 id TEXT NOT NULL UNIQUE COLLATE NOCASE,
                 revision INTEGER NOT NULL CHECK(revision=1),
                 flow TEXT NOT NULL REFERENCES flows(id) ON DELETE CASCADE,
                 sequence INTEGER NOT NULL CHECK(sequence>0),
                 direction INTEGER NOT NULL CHECK(direction IN (0,1)),
                 kind INTEGER NOT NULL CHECK(kind IN (1,2,8,9,10)),
                 payload_bytes INTEGER NOT NULL CHECK(payload_bytes BETWEEN 0 AND 16777216),
                 observed INTEGER NOT NULL,
                 UNIQUE(flow,sequence)
             );",
        )
        .unwrap();
    connection.close().unwrap();

    let mut store = ProjectStore::open(&path).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut record = message(flow.summary.id, 1);
    record.id = Uuid::from_u128(0xabcdef);
    record.payload = b"edited".to_vec();
    store
        .put_captured_websocket(&CapturedWebSocket {
            record: record.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(b"original".to_vec())),
        })
        .unwrap();
    store
        .connection
        .execute(
            "INSERT INTO websocket_originals(message,body) VALUES(?1,?2)",
            rusqlite::params![
                record.id.to_string().to_ascii_uppercase(),
                b"original".to_vec()
            ],
        )
        .unwrap();
    let error = store
        .read_websocket_detail(record.id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("重复"), "{error}");
    let request = query(
        &store,
        WebSocketHistoryFilter {
            term: "original".into(),
            ..Default::default()
        },
    );
    let error = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &request,
        &[],
        &CancellationToken::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("重复"), "{error}");
}
