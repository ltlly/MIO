use super::*;

#[test]
fn both_legacy_schemas_migrate_atomically_and_preserve_existing_http_and_websocket_data() {
    for schema in [1, 2] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.mio");
        let (id, flow, websocket) = downgrade_fixture(&path, schema);
        let mut store = ProjectStore::open(&path).unwrap();
        assert_eq!(store.id, id);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            super::super::super::SCHEMA
        );
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        assert!(store.ext_sources().unwrap().is_empty());
        if let Some(message) = &websocket {
            assert_eq!(store.read_websocket(message.id).unwrap(), *message);
        }
        let source_id = Uuid::new_v4();
        store.begin_source(source_id, &initial(1)).unwrap();
        store.append_source_chunk(source_id, 0, &[255]).unwrap();
        store
            .finish_source(source_id, &final_report(1, &[], &[]))
            .unwrap();
        store.save(None).unwrap();
        store.close().unwrap();
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.read_source_range(source_id, 0, 1).unwrap(), [255]);
        assert_eq!(reopened.read(flow.summary.id).unwrap(), flow);
    }
}

#[test]
fn failure_mid_migration_leaves_schema_version_prior_data_and_all_tables_unchanged() {
    for schema in [1, 2] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conflict.mio");
        let (_, flow, websocket) = downgrade_fixture(&path, schema);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE ext_items(untouched BLOB NOT NULL); INSERT INTO ext_items VALUES(x'00ff');").unwrap();
        connection.close().unwrap();
        let before = fs::read(&path).unwrap();
        assert!(ProjectStore::open(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let read = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            read.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            schema
        );
        assert_eq!(read.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('ext_sources','ext_source_chunks','ext_messages')", [], |row| row.get::<_, usize>(0)).unwrap(), 0);
        assert_eq!(
            read.query_row("SELECT untouched FROM ext_items", [], |row| row
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            [0, 255]
        );
        assert_eq!(
            super::super::super::read_record(&read, flow.summary.id).unwrap(),
            Some(flow)
        );
        let ws_count: usize = read.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('websocket_messages','websocket_payloads')", [], |row| row.get(0)).unwrap();
        assert_eq!(ws_count, if websocket.is_some() { 2 } else { 0 });
        if let Some(message) = websocket {
            assert_eq!(
                read.query_row(
                    "SELECT body FROM websocket_payloads WHERE message=?1",
                    [message.id.to_string()],
                    |row| row.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
                message.payload
            );
        }
    }
}

#[test]
fn finalization_rejects_missing_chunks_omitted_nested_collections_and_impossible_reports() {
    let mut store = ProjectStore::create(None).unwrap();
    let source_id = Uuid::new_v4();
    let report = initial(256);
    store.begin_source(source_id, &report).unwrap();
    assert!(
        store
            .finish_source(source_id, &final_report(256, &[], &[]))
            .is_err()
    );
    store.append_source_chunk(source_id, 0, &[0; 256]).unwrap();
    let mut nested = item(source_id, 0);
    nested.collection = "root.f2.f1[1].f2".into();
    nested.object_address = None;
    nested.state = "failed".into();
    store.put_ext_item(&nested).unwrap();
    assert!(
        store
            .finish_source(source_id, &final_report(256, &[], &[]))
            .is_err()
    );
    assert_eq!(store.ext_report(source_id).unwrap(), report);
    let report = final_report(256, &[nested], &[]);
    let mut duplicate = report.clone();
    duplicate.collections.push(report.collections[0].clone());
    duplicate.indexed_items *= 2;
    assert!(store.finish_source(source_id, &duplicate).is_err());
    let mut wrong = report.clone();
    wrong.schema_revision += 1;
    assert!(store.finish_source(source_id, &wrong).is_err());
    let mut wrong = report.clone();
    wrong.collections[0].logical_items = Some(0);
    assert!(store.finish_source(source_id, &wrong).is_err());
    let mut wrong = report.clone();
    wrong.source_bytes += 1;
    assert!(store.finish_source(source_id, &wrong).is_err());
    store
        .connection
        .execute("UPDATE ext_items SET item_index=1", [])
        .unwrap();
    assert!(
        store
            .finish_source(source_id, &report)
            .unwrap_err()
            .to_string()
            .contains("逻辑范围")
    );
    store
        .connection
        .execute("UPDATE ext_items SET item_index=0", [])
        .unwrap();
    store.finish_source(source_id, &report).unwrap();
}
