use super::*;

#[test]
fn corrupt_item_fields_are_gated_and_do_not_disappear_from_pages() {
    let (mut store, source_id) = pending();
    let record = item(source_id, 0);
    finish(&mut store, source_id, std::slice::from_ref(&record), &[]);
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    for (column, original, bad_values) in [
        (
            "id",
            Value::Text(record.id.to_string()),
            vec![
                Value::Text("z".repeat(36)),
                Value::Text("x".repeat(4097)),
                Value::Blob(vec![b'x'; 36]),
            ],
        ),
        (
            "collection",
            Value::Text(record.collection.clone()),
            vec![
                Value::Text(String::new()),
                Value::Text("x".repeat(MAX_COLLECTION_BYTES + 1)),
                Value::Blob(vec![b'x']),
            ],
        ),
        (
            "state",
            Value::Text(record.state.clone()),
            vec![
                Value::Text(String::new()),
                Value::Text("x".repeat(MAX_COLLECTION_BYTES + 1)),
                Value::Blob(vec![b'x']),
            ],
        ),
        (
            "item_index",
            Value::Integer(record.index as i64),
            vec![Value::Integer(-1), Value::Real(0.5), Value::Blob(vec![0])],
        ),
        (
            "object_address",
            Value::Integer(record.object_address.unwrap() as i64),
            vec![
                Value::Integer(-1),
                Value::Real(0.5),
                Value::Text("not an address".into()),
                Value::Blob(vec![0]),
            ],
        ),
    ] {
        for bad in bad_values {
            store
                .connection
                .execute(&format!("UPDATE ext_items SET {column}=?1"), [bad])
                .unwrap();
            assert!(
                store.ext_item_page(source_id, None, 0, 1).is_err(),
                "{column}"
            );
            assert!(
                store.ext_item_detail(source_id, record.id, 0, 1).is_err(),
                "{column}"
            );
            store
                .connection
                .execute(&format!("UPDATE ext_items SET {column}=?1"), [&original])
                .unwrap();
        }
    }
    assert_eq!(
        store.ext_item_page(source_id, None, 0, 1).unwrap().rows,
        [summary(&record, 0)]
    );
}

#[test]
fn detail_metadata_and_summary_are_type_size_and_ownership_checked() {
    let (mut store, source_id) = pending();
    let record = item(source_id, 0);
    let original_message = message(&record, 15);
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&record),
        std::slice::from_ref(&original_message),
    );
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    for bad in [
        Value::Blob(vec![b' '; MAX_METADATA_BYTES + 1]),
        Value::Text("{}".into()),
        Value::Blob(b"not JSON".to_vec()),
    ] {
        store
            .connection
            .execute("UPDATE ext_items SET metadata=?1", [bad])
            .unwrap();
        assert!(store.ext_item_detail(source_id, record.id, 0, 1).is_err());
        assert_eq!(
            store
                .ext_item_page(source_id, None, 0, 1)
                .unwrap()
                .rows
                .len(),
            1
        );
    }
    store
        .connection
        .execute("UPDATE ext_items SET metadata=?1", [&record.metadata_json])
        .unwrap();
    for bad in [
        Value::Blob(vec![0; MAX_SUMMARY_BYTES + 1]),
        Value::Text("{}".into()),
        Value::Blob(b"not JSON".to_vec()),
    ] {
        store
            .connection
            .execute("UPDATE ext_messages SET summary=?1", [bad])
            .unwrap();
        assert!(store.ext_item_detail(source_id, record.id, 0, 1).is_err());
    }
    for field in [
        "id",
        "source",
        "item",
        "collection",
        "index",
        "field",
        "range",
        "body",
        "status",
    ] {
        let mut corrupt = original_message.clone();
        match field {
            "id" => corrupt.id = Uuid::new_v4(),
            "source" => corrupt.source_id = Uuid::new_v4(),
            "item" => corrupt.item_id = Uuid::new_v4(),
            "collection" => corrupt.collection = "other".into(),
            "index" => corrupt.item_index += 1,
            "field" => corrupt.field_id += 1,
            "range" => corrupt.raw_offset = RAW.len() as u64,
            "body" => corrupt.body_offset = Some(corrupt.raw_bytes + 1),
            "status" => corrupt.status = Some(99),
            _ => unreachable!(),
        }
        store
            .connection
            .execute(
                "UPDATE ext_messages SET summary=?1",
                [serde_json::to_vec(&corrupt).unwrap()],
            )
            .unwrap();
        assert!(
            store.ext_item_detail(source_id, record.id, 0, 1).is_err(),
            "{field}"
        );
    }
    store
        .connection
        .execute(
            "UPDATE ext_messages SET summary=?1",
            [serde_json::to_vec(&original_message).unwrap()],
        )
        .unwrap();
    for bad in [Value::Text("x".repeat(4097)), Value::Blob(vec![b'x'; 36])] {
        store
            .connection
            .execute("UPDATE ext_messages SET id=?1", [bad])
            .unwrap();
        assert!(store.ext_item_detail(source_id, record.id, 0, 1).is_err());
        store
            .connection
            .execute(
                "UPDATE ext_messages SET id=?1",
                [original_message.id.to_string()],
            )
            .unwrap();
    }
    for bad in [Value::Integer(-1), Value::Integer(256), Value::Real(15.5)] {
        store
            .connection
            .execute("UPDATE ext_messages SET field_id=?1", [bad])
            .unwrap();
        assert!(store.ext_item_detail(source_id, record.id, 0, 1).is_err());
    }
    store
        .connection
        .execute(
            "UPDATE ext_messages SET field_id=?1",
            [original_message.field_id],
        )
        .unwrap();
    assert_eq!(
        store
            .ext_item_detail(source_id, record.id, 0, 1)
            .unwrap()
            .messages
            .rows,
        [original_message]
    );
}

#[test]
fn sql_gates_oversize_owners_and_payloads_before_rust_materialization() {
    let (mut store, source_id) = pending();
    let record = item(source_id, 0);
    let message = message(&record, 15);
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&record),
        std::slice::from_ref(&message),
    );
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_items SET collection=CAST(zeroblob(?1) AS TEXT),
        state=CAST(zeroblob(?1) AS TEXT),metadata=zeroblob(?2)",
            params![MAX_COLLECTION_BYTES + 1, MAX_METADATA_BYTES + 1],
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_messages SET id=CAST(zeroblob(4097) AS TEXT),summary=zeroblob(?1)",
            [MAX_SUMMARY_BYTES + 1],
        )
        .unwrap();
    let mut query = store
        .connection
        .prepare(&format!(
            "SELECT {ITEM_COLUMNS},{ITEM_METADATA_COLUMNS}
        FROM ext_items i WHERE i.source=?1 AND i.id=?2"
        ))
        .unwrap();
    let mut rows = query
        .query(params![
            source_id.to_string(),
            record.id.to_string(),
            MAX_METADATA_BYTES,
            Null,
            MAX_COLLECTION_BYTES,
        ])
        .unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(row.get::<_, Option<String>>(2).unwrap(), None);
    assert_eq!(row.get::<_, Option<String>>(6).unwrap(), None);
    assert_eq!(row.get::<_, usize>(9).unwrap(), MAX_METADATA_BYTES + 1);
    assert_eq!(row.get::<_, Option<Vec<u8>>>(10).unwrap(), None);

    let mut query = store.connection.prepare(ITEM_MESSAGE_SELECT).unwrap();
    let mut rows = query
        .query(params![
            source_id.to_string(),
            record.id.to_string(),
            1,
            0,
            MAX_SUMMARY_BYTES,
            MAX_COLLECTION_BYTES,
        ])
        .unwrap();
    let row = rows.next().unwrap().unwrap();
    assert_eq!(row.get::<_, Option<String>>(0).unwrap(), None);
    assert_eq!(row.get::<_, Option<String>>(4).unwrap(), None);
    assert_eq!(row.get::<_, usize>(8).unwrap(), MAX_SUMMARY_BYTES + 1);
    assert_eq!(row.get::<_, Option<Vec<u8>>>(9).unwrap(), None);
}
