use super::*;

#[test]
fn history_script_reads_complete_source_notes_and_saved_notes() {
    let text = format!("{}:SOURCE_NOTE_TAIL", "原始备注🙂".repeat(100));
    let (mut store, source, items) = fixture(&text);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let mut query = crate::HttpHistoryQuery {
        project: owner,
        query_revision: 1,
        offset: 0,
        limit: 20,
        url_filter: String::new(),
        filter: crate::HttpHistoryFilter {
            filter_script: "notes.contains(\"SOURCE_NOTE_TAIL\")".into(),
            ..Default::default()
        },
        sort: crate::HttpHistorySort::default(),
        selected_ids: Vec::new(),
    };
    let source_results = store.query_http_history(&query, &cancel).unwrap();
    assert_eq!(source_results.total, 3);
    for row in &source_results.rows {
        let HistoryRow::ExtProxy(row) = row else {
            panic!("预期归档记录")
        };
        assert_eq!(row.notes_preview.chars().count(), 256);
        assert!(!row.notes_preview.contains("SOURCE_NOTE_TAIL"));
    }
    let saved = format!("{}:SAVED_NOTE_TAIL", "保存备注🙂".repeat(100));
    store
        .set_ext_http_notes(owner, source, items[0].id, 0, &saved, &cancel)
        .unwrap();
    query.filter.filter_script = "notes.contains(\"SAVED_NOTE_TAIL\")".into();
    let saved_results = store.query_http_history(&query, &cancel).unwrap();
    assert_eq!(saved_results.total, 1);
    assert_eq!(
        saved_results.rows[0].id(),
        crate::HistoryEntryId::External { source_id: source, item_id: items[0].id }
    );
    store.close().unwrap();
}

#[test]
fn source_notes_aliases_and_repeated_slots_keep_independent_cas_clear_and_both_reopens() {
    let text = format!("# 原始备注\n**粗体🙂**\0{}", "中🙂".repeat(300));
    let (mut store, source, items) = fixture(&text);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "ext_source_chunks",
                column_name: "data",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    assert_eq!(
        store.ext_http_notes(owner, source, items[2].id).unwrap(),
        ExtHttpNotes {
            revision: 0,
            text: text.clone()
        }
    );
    let (total, rows) = store.history_page(0, 10, "").unwrap();
    assert_eq!((total, rows.len()), (3, 3));
    for row in rows {
        let HistoryRow::ExtProxy(row) = row else {
            panic!("unexpected live flow")
        };
        assert_eq!(row.notes_revision, 0);
        assert_eq!(
            row.notes_preview,
            text.chars().take(256).collect::<String>()
        );
        assert!(row.notes_error.is_none());
    }
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let first = store
        .set_ext_http_notes(owner, source, items[2].id, 0, "edited\n🙂", &cancel)
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(
        store.ext_http_notes(owner, source, items[0].id).unwrap(),
        first
    );
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[3].id)
            .unwrap()
            .text,
        text
    );
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[4].id)
            .unwrap()
            .text,
        text
    );
    let stale = store
        .set_ext_http_notes(owner, source, items[0].id, 0, "stale", &cancel)
        .unwrap_err();
    assert_eq!(stale.stored, Some(first.clone()));
    assert_eq!(
        store
            .set_ext_http_notes(owner, source, items[0].id, 1, &first.text, &cancel)
            .unwrap(),
        first
    );
    let cleared = store
        .set_ext_http_notes(owner, source, items[2].id, 1, "", &cancel)
        .unwrap();
    assert_eq!(
        cleared,
        ExtHttpNotes {
            revision: 2,
            text: String::new()
        }
    );
    assert_eq!(
        store
            .connection
            .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_notes", [], |row| row.get(0))
            .unwrap(),
        1
    );
    let directory = tempfile::tempdir().unwrap();
    let saved = directory.path().join("notes.mio");
    let copy = directory.path().join("notes-copy.mio");
    store.save(Some(&saved)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&saved, &copy] {
        let store = ProjectStore::open(path).unwrap();
        assert_eq!(
            store
                .ext_http_notes(project(&store), source, items[0].id)
                .unwrap(),
            cleared
        );
        let (total, rows) = store.history_page(0, 10, "").unwrap();
        assert_eq!((total, rows.len()), (3, 3));
        let row = rows
            .iter()
            .find_map(|row| match row {
                HistoryRow::ExtProxy(row) if row.item.id == items[0].id => Some(row),
                _ => None,
            })
            .expect("cleared note owner remains in the history page");
        assert_eq!(row.notes_revision, cleared.revision);
        assert_eq!(row.notes_preview, "");
        assert_eq!(row.notes_error, None);
        assert_eq!(
            store
                .ext_http_notes(project(&store), source, items[1].id)
                .unwrap()
                .text,
            text
        );
        for item in &items {
            assert_eq!(store.ext_item(item.id).unwrap(), *item);
        }
        assert_eq!(
            store.read_source_range(source, 0, 4096).unwrap(),
            vec![0; 4096]
        );
        assert_eq!(store.page(0, 1, "").unwrap().0, 0);
        store.close().unwrap();
    }
}

#[test]
fn unknown_source_notes_report_errors_without_hiding_rows_and_explicit_nulls_are_empty() {
    for mode in [
        "missing",
        "failed",
        "null-parent",
        "null-note",
        "bad-binding",
        "bad-length",
    ] {
        let (mut store, source, items) = fixture("source🙂");
        for item in &items {
            let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
            let annotations = &mut metadata["record"]["mapping"]["value"]["annotations"];
            match mode {
                "missing" => *annotations = Value::Null,
                "failed" => annotations["observation"] = json!({"state":"failed"}),
                "null-parent" => annotations["observation"] = json!({"state":"null"}),
                "null-note" => {
                    annotations["observation"]["value"]["mapping"]["value"]["note"]["observation"] =
                        json!({"state":"null"})
                }
                "bad-binding" => annotations["binding"]["source_id"] = json!(Uuid::new_v4()),
                "bad-length" => {
                    annotations["observation"]["value"]["mapping"]["value"]["note"]["observation"]
                        ["value"]["mapping"]["value"]["length"]["observation"]["value"] = json!(1)
                }
                _ => unreachable!(),
            }
            replace_metadata(&store, item, &metadata);
        }
        let allowed = mode.starts_with("null-");
        let result = store.ext_http_notes(project(&store), source, items[0].id);
        assert_eq!(result.is_ok(), allowed, "{mode}");
        if allowed {
            assert_eq!(result.unwrap(), ExtHttpNotes::default());
        }
        let (total, rows) = store.history_page(0, 10, "").unwrap();
        assert_eq!((total, rows.len()), (3, 3));
        for row in rows {
            let HistoryRow::ExtProxy(row) = row else {
                panic!("unexpected live row")
            };
            assert_eq!(row.notes_error.is_none(), allowed, "{mode}");
            assert!(row.notes_preview.is_empty());
        }
        if !allowed {
            assert!(
                store
                    .set_ext_http_notes(
                        project(&store),
                        source,
                        items[0].id,
                        0,
                        "replace",
                        &CancellationToken::new()
                    )
                    .is_err()
            );
        }
        assert_eq!(
            store
                .connection
                .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_notes", [], |row| row
                    .get(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn zero_logical_length_requires_a_verified_nonempty_utf16_backing_array() {
    for capacity in [0, 1] {
        let (mut store, source, items) = fixture("unused");
        for item in &items {
            let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
            let note = &mut metadata["record"]["mapping"]["value"]["annotations"]["observation"]["value"]
                ["mapping"]["value"]["note"]["observation"]["value"]["mapping"]["value"];
            note["length"]["observation"]["value"] = json!(0);
            note["text"] = json!({"state":"value","value":""});
            let array = &mut note["backing"]["observation"]["value"];
            array["element_count"] = json!(capacity);
            array["payload_bytes"] = json!(capacity * 2);
            array["total_size"] = json!((capacity * 2_u64).max(1) + 8);
            replace_metadata(&store, item, &metadata);
        }
        let owner = project(&store);
        let read = store.ext_http_notes(owner, source, items[0].id);
        let (total, rows) = store.history_page(0, 10, "").unwrap();
        assert_eq!((total, rows.len()), (3, 3));
        for row in rows {
            let HistoryRow::ExtProxy(row) = row else {
                panic!("unexpected live row")
            };
            assert_eq!(row.notes_error.is_some(), capacity == 0);
            assert!(row.notes_preview.is_empty());
        }
        if capacity == 0 {
            assert!(read.unwrap_err().to_string().contains("容量无效"));
            assert!(
                store
                    .set_ext_http_notes(
                        owner,
                        source,
                        items[0].id,
                        0,
                        "replacement",
                        &CancellationToken::new()
                    )
                    .is_err()
            );
            assert_eq!(
                store
                    .connection
                    .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_notes", [], |row| row
                        .get(0))
                    .unwrap(),
                0
            );
        } else {
            assert_eq!(read.unwrap(), ExtHttpNotes::default());
            assert_eq!(
                store
                    .set_ext_http_notes(
                        owner,
                        source,
                        items[0].id,
                        0,
                        "replacement",
                        &CancellationToken::new()
                    )
                    .unwrap()
                    .revision,
                1
            );
        }
        store.close().unwrap();
    }
}

#[test]
fn invalid_owner_ids_project_cancel_and_overlimits_never_write_notes() {
    let (mut store, source, items) = fixture("source");
    let owner = project(&store);
    for (project, source, item) in [
        (owner, Uuid::nil(), items[0].id),
        (owner, source, Uuid::nil()),
        (
            ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
            source,
            items[0].id,
        ),
        (
            ProjectKey {
                id: owner.id,
                generation: 0,
            },
            source,
            items[0].id,
        ),
        (owner, Uuid::new_v4(), items[0].id),
        (owner, source, items[5].id),
    ] {
        assert!(store.ext_http_notes(project, source, item).is_err());
        assert!(
            store
                .set_ext_http_notes(project, source, item, 0, "x", &CancellationToken::new())
                .is_err()
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        store
            .set_ext_http_notes(owner, source, items[0].id, 0, "x", &cancel)
            .is_err()
    );
    let during_insert = CancellationToken::new();
    let observed = during_insert.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Insert {
                    table_name: "ext_http_notes"
                }
            ) {
                observed.cancel();
            }
            Authorization::Allow
        }));
    assert!(
        store
            .set_ext_http_notes(
                owner,
                source,
                items[0].id,
                0,
                "cancel before commit",
                &during_insert
            )
            .is_err()
    );
    assert!(during_insert.is_cancelled());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(
        store
            .set_ext_http_notes(
                owner,
                source,
                items[0].id,
                0,
                &"x".repeat(65537),
                &CancellationToken::new()
            )
            .is_err()
    );
    let change = ExtHttpNotesChange {
        source_id: source,
        item_id: items[0].id,
        expected_notes_revision: 0,
        text: "x".into(),
    };
    assert!(validate_changes(&vec![change; 65]).is_err());
    assert_eq!(
        store
            .connection
            .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_notes", [], |row| row.get(0))
            .unwrap(),
        0
    );
    let mut metadata: Value = serde_json::from_slice(&items[2].metadata_json).unwrap();
    metadata["slot"]["value"] = json!(999);
    replace_metadata(&store, &items[2], &metadata);
    assert!(
        store
            .set_ext_http_notes(
                owner,
                source,
                items[2].id,
                0,
                "alias-forged",
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[0].id)
            .unwrap()
            .text,
        "source"
    );
}
