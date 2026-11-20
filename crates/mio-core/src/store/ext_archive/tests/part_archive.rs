use super::*;

#[test]
fn repeater_active_report_accepts_old_absent_field_and_valid_unfinished_source_slot() {
    let legacy = initial(8);
    let encoded = encode_report(&legacy).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert!(value.get("repeater_active_tab").is_none());
    assert_eq!(
        serde_json::from_slice::<ExtImportReport>(&encoded).unwrap(),
        legacy
    );
    let mut report = legacy;
    report.schema_floor = 229;
    report.schema_revision = 229;
    report.repeater_active_tab = Some(1);
    report.collections.push(ExtCollectionReport {
        path: "root.f2.f1".into(),
        logical_items: Some(2),
        indexed_items: 0,
        message_count: 0,
        state: "available; items not yet indexed".into(),
    });
    assert!(!report.complete_archive);
    assert_eq!(
        serde_json::from_slice::<ExtImportReport>(&encode_report(&report).unwrap()).unwrap(),
        report
    );
    for kind in 0..6 {
        let mut invalid = report.clone();
        match kind {
            0 => invalid.schema_floor = 228,
            1 => invalid.schema_revision = 228,
            2 => invalid.repeater_active_tab = Some(2),
            3 => invalid.collections[0].logical_items = None,
            4 => invalid.collections.clear(),
            5 => invalid.collections.push(invalid.collections[0].clone()),
            _ => unreachable!(),
        }
        assert!(
            encode_report(&invalid).is_err(),
            "invalid active-slot report {kind}"
        );
    }
}

#[test]
fn completed_repeater_active_report_is_immutable_and_invalid_stored_slot_is_rejected() {
    let mut store = ProjectStore::create(None).unwrap();
    let source_id = Uuid::new_v4();
    let mut report = initial(8);
    report.schema_floor = 229;
    report.schema_revision = 229;
    report.repeater_active_tab = Some(0);
    report.collections.push(ExtCollectionReport {
        path: "root.f2.f1".into(),
        logical_items: Some(2),
        indexed_items: 0,
        message_count: 0,
        state: "available; source slots not indexed".into(),
    });
    store.begin_source(source_id, &report).unwrap();
    store
        .append_source_chunk(source_id, 0, b"archive!")
        .unwrap();
    report.complete_archive = true;
    store.finish_source(source_id, &report).unwrap();
    let original: Vec<u8> = store
        .connection
        .query_row(
            "SELECT report FROM ext_sources WHERE id=?1",
            [source_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut changed = report.clone();
    changed.repeater_active_tab = Some(1);
    assert!(
        store
            .finish_source(source_id, &changed)
            .unwrap_err()
            .to_string()
            .contains("不可修改")
    );
    assert_eq!(store.ext_report(source_id).unwrap(), report);
    let saved: Vec<u8> = store
        .connection
        .query_row(
            "SELECT report FROM ext_sources WHERE id=?1",
            [source_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, original);
    changed.repeater_active_tab = Some(2);
    let invalid = serde_json::to_vec(&changed).unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=?1 WHERE id=?2",
            params![invalid, source_id.to_string()],
        )
        .unwrap();
    assert!(
        store.ext_report(source_id).is_err(),
        "read_report must apply the same active-slot validation as writes"
    );
    let after_read: Vec<u8> = store
        .connection
        .query_row(
            "SELECT report FROM ext_sources WHERE id=?1",
            [source_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after_read, invalid,
        "rejecting malformed report must not rewrite source metadata"
    );
    assert_eq!(
        store.read_source_range(source_id, 0, 8).unwrap(),
        b"archive!"
    );
    store.close().unwrap();
}
