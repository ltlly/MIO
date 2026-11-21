use std::fs;

use super::*;

#[test]
fn schema_one_preserves_http_originals_workspace_and_unrelated_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema-one.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let project_id = store.id;
    let original = flow_record();
    store.put(&original).unwrap();
    let mut edited = original.clone();
    edited.summary.revision += 1;
    edited.request.body.reverse();
    store.put(&edited).unwrap();
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(Some(&edited.request)).unwrap();
    store.save_workspace(&workspace).unwrap();
    store
        .connection
        .execute("INSERT INTO metadata VALUES('extra',?1)", [vec![0, 255]])
        .unwrap();
    store.save(None).unwrap();
    store.close().unwrap();

    let mut reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.id, project_id);
    assert_eq!(
        reopened
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        SCHEMA
    );
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(reopened.read(edited.summary.id).unwrap(), edited);
    assert_eq!(
        reopened.original(edited.summary.id).unwrap().request,
        Some(original.request)
    );
    let extra: Vec<u8> = reopened
        .connection
        .query_row("SELECT value FROM metadata WHERE key='extra'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(extra, [0, 255]);
    assert_eq!(reopened.websocket_page(None, 0, 1, &[]).unwrap().0, 0);
    let record = message(edited.summary.id, 1);
    reopened.put_websocket(&record).unwrap();
    reopened.save(None).unwrap();
    reopened.close().unwrap();
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(reopened.read_websocket(record.id).unwrap(), record);
}

#[test]
fn schema_one_assigns_connections_in_first_message_order_without_guessing_ports() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ws-order.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let first_flow = flow_record();
    let second_flow = flow_record();
    store.put(&first_flow).unwrap();
    store.put(&second_flow).unwrap();
    let records = [
        message(second_flow.summary.id, 1),
        message(first_flow.summary.id, 1),
        message(second_flow.summary.id, 2),
    ];
    for record in &records {
        store.put_websocket(record).unwrap();
    }
    store.save(None).unwrap();
    store.close().unwrap();

    let reopened = ProjectStore::open(&path).unwrap();
    let (_, rows) = reopened.websocket_page(None, 0, 10, &[]).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.ordinal, row.connection_id, row.listener_port))
            .collect::<Vec<_>>(),
        [(1, 1, None), (2, 2, None), (3, 1, None)]
    );
    for record in records {
        assert_eq!(reopened.read_websocket(record.id).unwrap(), record);
    }
    let connection_ids = reopened.websocket_numbers().unwrap();
    assert_eq!(connection_ids, (4, 3));
}

#[test]
fn schema_one_connections_keep_sequence_conflict_checks_on_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections-reopen.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let mut flow = flow_record();
    flow.summary.id = Uuid::from_u128(0xabcdef);
    store.put(&flow).unwrap();
    let first = message(flow.summary.id, 1);
    let second = message(flow.summary.id, 2);
    for record in [&first, &second] {
        store.put_websocket(record).unwrap();
    }
    store.save(None).unwrap();
    store.close().unwrap();

    let mut store = ProjectStore::open(&path).unwrap();
    assert_eq!(store.websocket_numbers().unwrap(), (3, 2));
    store.put_websocket(&second).unwrap();
    assert!(
        store
            .put_websocket(&message(flow.summary.id, 2))
            .unwrap_err()
            .to_string()
            .contains("消息序号冲突")
    );
    let third = message(flow.summary.id, 3);
    store.put_websocket(&third).unwrap();
    let (_, rows) = store
        .websocket_page(Some(flow.summary.id), 0, 10, &[])
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.ordinal, row.connection_id, row.listener_port))
            .collect::<Vec<_>>(),
        [(1, 1, None), (2, 1, None), (3, 1, None)]
    );
    assert_eq!(
        store.websocket_connection(flow.summary.id).unwrap(),
        Some((1, None))
    );
    for record in [first, second, third] {
        assert_eq!(store.read_websocket(record.id).unwrap(), record);
    }
}

#[test]
fn listener_conflicts_reject_both_new_and_idempotent_messages_without_changing_numbers() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let record = message(flow.summary.id, 1);
    let capture = CapturedWebSocket {
        record: record.clone().into(),
        listener_port: Some(9091),
        original_payload: None,
    };
    store.put_captured_websocket(&capture).unwrap();
    store.put_captured_websocket(&capture).unwrap();
    let before = store.websocket_page(None, 0, 10, &[]).unwrap();
    for record in [record, message(flow.summary.id, 2)] {
        let error = store
            .put_captured_websocket(&CapturedWebSocket {
                record: record.into(),
                listener_port: Some(9092),
                original_payload: None,
            })
            .unwrap_err();
        assert!(error.to_string().contains("监听端口冲突"));
    }
    assert_eq!(store.websocket_page(None, 0, 10, &[]).unwrap(), before);
    assert_eq!(store.websocket_numbers().unwrap(), (2, 2));
}

#[test]
fn edited_originals_survive_reopen_and_idempotent_writes_and_page_without_reading_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("edited-original.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut record = message(flow.summary.id, 1);
    record.payload = b"edited".to_vec();
    store
        .put_captured_websocket(&CapturedWebSocket {
            record: record.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(b"original".to_vec())),
        })
        .unwrap();
    let unchanged = message(flow.summary.id, 2);
    store
        .put_captured_websocket(&CapturedWebSocket {
            record: unchanged.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(unchanged.payload.clone())),
        })
        .unwrap();
    store
        .put_captured_websocket(&CapturedWebSocket {
            record: record.clone().into(),
            listener_port: Some(9091),
            original_payload: Some(Arc::new(b"original".to_vec())),
        })
        .unwrap();
    let (total, rows) = store.websocket_page(None, 0, 10, &[]).unwrap();
    assert_eq!(total, 2);
    assert_eq!(
        rows.iter()
            .map(|row| (row.id, row.edited))
            .collect::<Vec<_>>(),
        [(record.id, true), (unchanged.id, false)]
    );
    store.connection.authorizer(Some(deny_body_reads));
    let (total, rows) = store.websocket_page(None, 0, 10, &[]).unwrap();
    assert_eq!(total, 2);
    assert!(rows.iter().any(|row| row.id == record.id && row.edited));
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store.read_websocket_detail(record.id).unwrap(),
        (record.clone(), Some(b"original".to_vec()))
    );
    assert_eq!(
        store.read_websocket_detail(unchanged.id).unwrap(),
        (unchanged.clone(), None)
    );
    store.close().unwrap();
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(
        reopened.read_websocket_detail(record.id).unwrap(),
        (record, Some(b"original".to_vec()))
    );
    assert_eq!(
        reopened.read_websocket_detail(unchanged.id).unwrap(),
        (unchanged, None)
    );
    let originals: usize = reopened
        .connection
        .query_row("SELECT count(*) FROM websocket_originals", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(originals, 1);
}

#[test]
fn unknown_schemas_are_rejected_without_file_or_sidecar_changes() {
    for schema in [0, SCHEMA + 1, 2, 18, i32::MAX] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        store
            .connection
            .pragma_update(None, "user_version", schema)
            .unwrap();
        store.checkpoint().unwrap();
        store
            .connection
            .execute_batch("PRAGMA journal_mode=DELETE;")
            .unwrap();
        store.close().unwrap();
        let before = fs::read(&path).unwrap();
        assert!(ProjectStore::open(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
