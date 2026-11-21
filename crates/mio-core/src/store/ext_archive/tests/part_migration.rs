use super::*;

#[test]
fn schema_one_preserves_http_websocket_and_ext_archive_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema-one.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let id = store.id;
    let flow = super::super::super::tests::record();
    store.put(&flow).unwrap();
    let websocket = WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flow.summary.id,
        sequence: 1,
        direction: WebSocketDirection::ServerToClient,
        kind: WebSocketKind::Binary,
        payload: vec![0, 255],
        observed_unix_ms: 5,
    };
    store.put_websocket(&websocket).unwrap();
    let source_id = Uuid::new_v4();
    store.begin_source(source_id, &initial(1)).unwrap();
    store.append_source_chunk(source_id, 0, &[255]).unwrap();
    store
        .finish_source(source_id, &final_report(1, &[], &[]))
        .unwrap();
    store.save(None).unwrap();
    store.close().unwrap();

    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.id, id);
    assert_eq!(
        reopened
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        super::super::super::SCHEMA
    );
    assert_eq!(reopened.read(flow.summary.id).unwrap(), flow);
    assert_eq!(reopened.read_websocket(websocket.id).unwrap(), websocket);
    assert_eq!(reopened.read_source_range(source_id, 0, 1).unwrap(), [255]);
    reopened.close().unwrap();
}

#[test]
fn legacy_ext_archive_sqlite_versions_are_rejected_readonly() {
    for schema in [2, 24] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.mio");
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
