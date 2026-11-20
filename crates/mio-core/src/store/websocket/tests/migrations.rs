use std::fs;

use super::*;

#[test]
fn schema_one_upgrade_preserves_http_originals_workspace_and_unrelated_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.mio");
    let mut legacy = legacy_store(&path);
    let project_id = legacy.id;
    let original = flow_record();
    legacy.put(&original).unwrap();
    let mut edited = original.clone();
    edited.summary.revision += 1;
    edited.request.body.reverse();
    legacy.put(&edited).unwrap();
    let mut workspace = legacy.workspace().unwrap();
    workspace.open_request(Some(&edited.request)).unwrap();
    legacy.save_workspace(&workspace).unwrap();
    legacy
        .connection
        .execute("INSERT INTO metadata VALUES('extra',?1)", [vec![0, 255]])
        .unwrap();
    drop(legacy);

    let mut upgraded = ProjectStore::open(&path).unwrap();
    assert_eq!(upgraded.id, project_id);
    assert_eq!(
        upgraded
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        SCHEMA
    );
    assert_eq!(upgraded.workspace().unwrap(), workspace);
    assert_eq!(upgraded.read(edited.summary.id).unwrap(), edited);
    assert_eq!(
        upgraded.original(edited.summary.id).unwrap().request,
        Some(original.request)
    );
    let extra: Vec<u8> = upgraded
        .connection
        .query_row("SELECT value FROM metadata WHERE key='extra'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(extra, [0, 255]);
    assert_eq!(upgraded.websocket_page(None, 0, 1, &[]).unwrap().0, 0);
    let record = message(edited.summary.id, 1);
    upgraded.put_websocket(&record).unwrap();
    drop(upgraded);
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(reopened.read_websocket(record.id).unwrap(), record);
}

#[test]
fn schema_eighteen_assigns_connections_in_first_message_order_without_guessing_ports() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-eighteen.mio");
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
    store
        .connection
        .execute_batch("DROP TABLE site_map_entries; DROP INDEX flows_source_started; DROP INDEX flows_source_status; DROP INDEX flows_source_capture; ALTER TABLE flows DROP COLUMN response_length; ALTER TABLE flows DROP COLUMN capture_number; ALTER TABLE flows DROP COLUMN elapsed_ms; ALTER TABLE flows DROP COLUMN response_bytes; ALTER TABLE flows DROP COLUMN status; ALTER TABLE flows DROP COLUMN source; ALTER TABLE flows DROP COLUMN listener_port; ALTER TABLE intruder_results DROP COLUMN highlight_revision; ALTER TABLE intruder_results DROP COLUMN highlight; ALTER TABLE intruder_results DROP COLUMN notes_revision; ALTER TABLE intruder_results DROP COLUMN notes; DROP TABLE ext_http_highlights; DROP TABLE websocket_originals; DROP TABLE ext_http_notes; DROP TABLE repeater_history; DROP TABLE websocket_notes; DROP TABLE websocket_connections; PRAGMA user_version=18;")
        .unwrap();
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
fn nocase_parent_connections_migrate_as_one_and_keep_sequence_conflict_checks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nocase-connections.mio");
    ProjectStore::create(Some(&path)).unwrap().close().unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(&format!(
        "PRAGMA foreign_keys=ON; DROP TABLE flows;
         CREATE TABLE flows (
             id TEXT PRIMARY KEY COLLATE NOCASE,revision INTEGER NOT NULL,started INTEGER NOT NULL,
             url TEXT NOT NULL,method TEXT NOT NULL,state TEXT NOT NULL,summary BLOB NOT NULL,
             {},{},{},{},{},{},{});
         CREATE INDEX flows_started ON flows(started DESC,id);
         CREATE INDEX flows_state ON flows(state);
         CREATE INDEX flows_source_started ON flows(source,started DESC,id);
         CREATE INDEX flows_source_status ON flows(source,status);
         CREATE INDEX flows_source_capture ON flows(source,capture_number);",
        crate::store::FLOW_LISTENER_PORT_COLUMN,
        crate::store::FLOW_SOURCE_COLUMN,
        crate::store::FLOW_STATUS_COLUMN,
        crate::store::FLOW_RESPONSE_BYTES_COLUMN,
        crate::store::FLOW_ELAPSED_MS_COLUMN,
        crate::store::FLOW_CAPTURE_NUMBER_COLUMN,
        crate::store::FLOW_RESPONSE_LENGTH_COLUMN,
    )).unwrap();
    connection.close().unwrap();
    let mut store = ProjectStore::open(&path).unwrap();
    let mut flow = flow_record();
    flow.summary.id = Uuid::from_u128(0xabcdef);
    store.put(&flow).unwrap();
    let first = message(flow.summary.id, 1);
    let second = message(flow.summary.id, 2);
    for record in [&first, &second] {
        store.put_websocket(record).unwrap();
    }
    store
        .connection
        .execute(
            "UPDATE websocket_messages SET flow=?1 WHERE id=?2",
            params![
                flow.summary.id.to_string().to_ascii_uppercase(),
                second.id.to_string()
            ],
        )
        .unwrap();
    store
        .connection
        .execute_batch("DROP TABLE site_map_entries; DROP INDEX flows_source_started; DROP INDEX flows_source_status; DROP INDEX flows_source_capture; ALTER TABLE flows DROP COLUMN response_length; ALTER TABLE flows DROP COLUMN capture_number; ALTER TABLE flows DROP COLUMN elapsed_ms; ALTER TABLE flows DROP COLUMN response_bytes; ALTER TABLE flows DROP COLUMN status; ALTER TABLE flows DROP COLUMN source; ALTER TABLE flows DROP COLUMN listener_port; ALTER TABLE intruder_results DROP COLUMN highlight_revision; ALTER TABLE intruder_results DROP COLUMN highlight; ALTER TABLE intruder_results DROP COLUMN notes_revision; ALTER TABLE intruder_results DROP COLUMN notes; DROP TABLE ext_http_highlights; DROP TABLE websocket_originals; DROP TABLE ext_http_notes; DROP TABLE repeater_history; DROP TABLE websocket_notes; DROP TABLE websocket_connections; PRAGMA user_version=18;")
        .unwrap();
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
fn failed_schema_upgrade_rolls_back_every_new_table_and_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conflicted-schema.mio");
    let mut legacy = legacy_store(&path);
    let flow = flow_record();
    legacy.put(&flow).unwrap();
    legacy.connection.execute_batch("CREATE TABLE websocket_payloads(untouched BLOB NOT NULL); INSERT INTO websocket_payloads VALUES(x'00ff');").unwrap();
    drop(legacy);
    let before = fs::read(&path).unwrap();
    assert!(ProjectStore::open(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let read = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        read.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        1
    );
    let added: usize = read.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('websocket_messages','websocket_flow_ordinal')", [], |row| row.get(0)).unwrap();
    assert_eq!(added, 0);
    let untouched: Vec<u8> = read
        .query_row("SELECT untouched FROM websocket_payloads", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(untouched, [0, 255]);
    assert_eq!(
        crate::store::read_record(&read, flow.summary.id).unwrap(),
        Some(flow)
    );
}

#[test]
fn unknown_schemas_are_rejected_without_file_or_sidecar_changes() {
    for schema in [0, SCHEMA + 1, i32::MAX] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown.mio");
        let legacy = legacy_store(&path);
        legacy
            .connection
            .pragma_update(None, "user_version", schema)
            .unwrap();
        drop(legacy);
        let before = fs::read(&path).unwrap();
        assert!(ProjectStore::open(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
