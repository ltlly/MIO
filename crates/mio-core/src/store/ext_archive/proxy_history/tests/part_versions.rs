use super::*;

#[test]
fn four_message_versions_keep_original_response_columns_and_reopen_exact_bytes() {
    let (mut store, source, items) = fixture(0, true, true);
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "ext_source_chunks",
                column_name: "data",
            }
            | AuthAction::Read {
                table_name: "messages",
                column_name: "body",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    let (total, rows) = store.history_page(0, 64, "").unwrap();
    assert_eq!(total, 4);
    let row = archived(&rows[3]);
    assert_eq!(row.item.id, items[0].id);
    assert_eq!(row.item.message_count, 4);
    let original = row.response.as_ref().unwrap();
    let edited = row.edited_response.as_ref().unwrap();
    assert_eq!(
        (original.field_id, original.status, original.raw_bytes),
        (18, Some(201), 140)
    );
    assert_eq!(
        (edited.field_id, edited.status, edited.raw_bytes),
        (20, Some(202), 155)
    );
    assert_eq!(edited.role, "edited_response");
    assert_ne!(original.id, edited.id);
    assert_eq!(original.item_id, edited.item_id);
    assert_eq!(store.history_page(0, 64, "PUT").unwrap().0, 0);
    assert_eq!(store.history_page(0, 64, "/fixture").unwrap().0, 2);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let mut original_bytes = response();
    original_bytes.insert(original_bytes.len() - 7, b'p');
    assert_eq!(
        store
            .ext_message_fragment(original.id, 0, 140)
            .unwrap()
            .bytes,
        original_bytes
    );
    assert_eq!(
        store
            .ext_message_fragment(edited.id, 0, 155)
            .unwrap()
            .bytes,
        changed_response()
    );
    let report = read_report(&store.connection, source).unwrap();
    let source_bytes = store
        .read_source_range(source, 0, report.source_bytes as usize)
        .unwrap();
    let mut workspace = store.workspace().unwrap();
    let tab = store
        .ext_repeater_tab(
            source,
            row.item.id,
            row.edited_request.as_ref().unwrap().id,
            &workspace,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(tab.draft.raw, EDITED_REQUEST);
    assert_eq!(tab.draft.target, "http://127.0.0.1:18765");
    assert!(tab.archive_response.is_none() && tab.last_flow.is_none());
    workspace.revision += 1;
    workspace.active = Some(tab.id);
    workspace.tabs.push(tab);
    store.save_workspace(&workspace).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("responses.mio");
    let copy = directory.path().join("responses-copy.mio");
    store.save(Some(&path)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&path, &copy] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(
            reopened.history_page(0, 64, "").unwrap(),
            (total, rows.clone())
        );
        assert_eq!(
            reopened
                .ext_message_fragment(original.id, 0, 140)
                .unwrap()
                .bytes,
            original_bytes
        );
        assert_eq!(
            reopened
                .ext_message_fragment(edited.id, 0, 155)
                .unwrap()
                .bytes,
            changed_response()
        );
        assert_eq!(
            reopened
                .read_source_range(source, 0, report.source_bytes as usize)
                .unwrap(),
            source_bytes
        );
        assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
        reopened.close().unwrap();
    }
}

#[test]
fn response_versions_cannot_supply_send_targets_and_edited_bindings_are_strict() {
    let (store, source, items) = fixture(0, true, true);
    let (_, rows) = store.history_page(0, 64, "18765").unwrap();
    let row = archived(&rows[0]);
    let workspace = store.workspace().unwrap();
    for message in [
        row.response.as_ref().unwrap(),
        row.edited_response.as_ref().unwrap(),
    ] {
        assert!(request_origin(&store.connection, message).is_err());
        assert!(
            store
                .ext_repeater_tab(
                    source,
                    items[0].id,
                    message.id,
                    &workspace,
                    &CancellationToken::new()
                )
                .is_err()
        );
    }
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.page(0, 1, "").unwrap().0, 0);
    let original_metadata: Value = serde_json::from_slice(&items[0].metadata_json).unwrap();
    assert!(original_metadata["record"]["mapping"]["value"]["response"]["binding"].is_null());
    for invalid in ["source", "item", "message", "field", "raw", "role"] {
        let mut metadata = original_metadata.clone();
        let field = &mut metadata["record"]["mapping"]["value"]["edited_response"];
        let mut summary = row.edited_response.as_ref().unwrap().clone();
        match invalid {
            "source" => field["binding"]["source_id"] = json!(Uuid::new_v4()),
            "item" => field["binding"]["item_id"] = json!(Uuid::new_v4()),
            "message" => field["binding"]["message_id"] = json!(Uuid::new_v4()),
            "field" => field["id"] = json!(19),
            "raw" => field["observation"]["value"]["address"] = json!(0),
            "role" => summary.role = "edited_request".into(),
            _ => unreachable!(),
        }
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![
                    items[0].id.to_string(),
                    serde_json::to_vec(&metadata).unwrap()
                ],
            )
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE ext_messages SET summary=?2 WHERE id=?1",
                params![
                    summary.id.to_string(),
                    serde_json::to_vec(&summary).unwrap()
                ],
            )
            .unwrap();
        assert!(store.history_page(0, 64, "18765").is_err(), "{invalid}");
        assert!(
            store
                .ext_repeater_tab(
                    source,
                    items[0].id,
                    summary.id,
                    &workspace,
                    &CancellationToken::new()
                )
                .is_err(),
            "{invalid}"
        );
    }
    for state in ["missing", "null", "failed"] {
        let (store, _, items) = fixture(0, true, false);
        let mut metadata: Value = serde_json::from_slice(&items[0].metadata_json).unwrap();
        metadata["record"]["mapping"]["value"]["edited_response"] =
            json!({"id": 20, "observation": {"state": state}});
        store
            .connection
            .execute(
                "UPDATE ext_items SET metadata=?2 WHERE id=?1",
                params![
                    items[0].id.to_string(),
                    serde_json::to_vec(&metadata).unwrap()
                ],
            )
            .unwrap();
        let (_, rows) = store.history_page(0, 64, "18765").unwrap();
        assert!(archived(&rows[0]).edited_response.is_none());
        assert!(archived(&rows[0]).response.is_some());
    }
    let (store, source, _) = fixture(0, true, true);
    let (_, rows) = store.history_page(0, 64, "18765").unwrap();
    let edited = archived(&rows[0]).edited_response.as_ref().unwrap();
    let report = read_report(&store.connection, source).unwrap();
    let mut raw = store
        .read_source_range(source, 0, report.source_bytes as usize)
        .unwrap();
    raw[edited.raw_offset as usize] = 0;
    store
        .connection
        .execute(
            "UPDATE ext_source_chunks SET codec=0,data=?2 WHERE source=?1 AND ordinal=0",
            params![source.to_string(), raw],
        )
        .unwrap();
    store.connection.execute(
        "UPDATE ext_messages SET summary=CAST(json_set(CAST(summary AS TEXT),'$.status',NULL,'$.declared_version',NULL,'$.diagnostic','controlled invalid response head') AS BLOB) WHERE field_id=20",
        [],
    ).unwrap();
    let (_, rows) = store.history_page(0, 64, "18765").unwrap();
    let edited = archived(&rows[0]).edited_response.as_ref().unwrap();
    assert_eq!(edited.status, None);
    assert_eq!(
        edited.diagnostic.as_deref(),
        Some("controlled invalid response head")
    );
    let bytes = store
        .ext_message_fragment(edited.id, 0, edited.raw_bytes as usize)
        .unwrap()
        .bytes;
    assert_eq!(bytes[0], 0);
    assert_eq!(&bytes[bytes.len() - 3..], &[255, 0, 129]);
}

#[test]
fn four_summary_budget_keeps_short_pages_and_rejects_an_oversized_single_row() {
    let (store, source, items) = fixture(0, true, true);
    let mut query = store
        .connection
        .prepare("SELECT summary FROM ext_messages")
        .unwrap();
    let summaries: Vec<ExtMessageSummary> = query
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .map(|bytes| serde_json::from_slice(&bytes.unwrap()).unwrap())
        .collect();
    drop(query);
    for maximum in [false, true] {
        for original in &summaries {
            let mut summary = original.clone();
            summary.diagnostic = Some(String::new());
            let length = if maximum {
                MAX_SUMMARY_BYTES - serde_json::to_vec(&summary).unwrap().len()
            } else {
                700 * 1024
            };
            summary.diagnostic = Some("x".repeat(length));
            let bytes = serde_json::to_vec(&summary).unwrap();
            assert!(bytes.len() <= MAX_SUMMARY_BYTES);
            store
                .connection
                .execute(
                    "UPDATE ext_messages SET summary=?2 WHERE id=?1",
                    params![summary.id.to_string(), bytes],
                )
                .unwrap();
        }
        if maximum {
            let error = store.history_page(0, 64, "fixture").unwrap_err();
            assert!(error.to_string().contains("未省略后续条目"));
            let detail = store.ext_item_detail(source, items[0].id, 0, 1).unwrap();
            assert_eq!(detail.messages.rows.len(), 1);
            assert_eq!(detail.messages.next_offset, Some(1));
        } else {
            let (total, first) = store.history_page(0, 64, "fixture").unwrap();
            assert_eq!(total, 2);
            assert_eq!(first.len(), 1);
            let (next_total, second) = store.history_page(first.len(), 64, "fixture").unwrap();
            assert_eq!(next_total, total);
            assert_eq!(second.len(), 1);
            assert_ne!(first[0].id(), second[0].id());
            assert!(store.history_page(2, 64, "fixture").unwrap().1.is_empty());
        }
    }
}

#[test]
fn proxy_time_metadata_keeps_old_missing_and_corrupt_observations_explicit() {
    let (store, source, items) = fixture(0, false, false);
    let item = &items[0];
    let before = item.metadata_json.clone();
    let baseline: Value = serde_json::from_slice(&before).unwrap();
    let (time, error) = source_time(item, &baseline);
    assert!(time.is_none() && error.unwrap().contains("未映射"));
    let timestamp = 1794126317491_i64;
    let valid = json!({"id":11,"binding":{"source_id":source,"item_id":item.id},"observation":{"state":"value","value":timestamp}});
    for mode in [
        "valid",
        "missing",
        "null",
        "failed",
        "wrong-field",
        "wrong-source",
        "wrong-item",
        "string",
        "overflow",
    ] {
        let mut metadata = baseline.clone();
        let mut field = valid.clone();
        match mode {
            "valid" => {}
            "missing" | "null" | "failed" => field["observation"] = json!({"state":mode}),
            "wrong-field" => field["id"] = json!(10),
            "wrong-source" => field["binding"]["source_id"] = json!(Uuid::new_v4()),
            "wrong-item" => field["binding"]["item_id"] = json!(Uuid::new_v4()),
            "string" => field["observation"]["value"] = json!(timestamp.to_string()),
            "overflow" => field["observation"]["value"] = json!(u64::MAX),
            _ => unreachable!(),
        }
        metadata["record"]["mapping"]["value"]["time"] = field;
        let (time, error) = source_time(item, &metadata);
        if mode == "valid" {
            assert_eq!((time, error), (Some(timestamp), None));
        } else {
            assert!(time.is_none() && error.is_some(), "{mode}");
        }
    }
    assert_eq!(store.ext_item(item.id).unwrap().metadata_json, before);
    store.close().unwrap();
}
