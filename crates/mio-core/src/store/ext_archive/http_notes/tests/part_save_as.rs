use super::*;

#[test]
fn project_save_conflicts_in_either_notes_batch_roll_back_both_notes_workspace_and_recovery() {
    for save_as in [false, true] {
        for stale_http in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let source_path = directory.path().join("atomic-source.mio");
            let copy = directory.path().join("not-published.mio");
            let (mut store, source, items) = fixture("original HTTP note");
            store.save(Some(&source_path)).unwrap();
            let owner = project(&store);
            let flow = crate::store::tests::record();
            store.put(&flow).unwrap();
            let message = websocket_message(flow.summary.id);
            store.put_websocket(&message).unwrap();
            let cancel = CancellationToken::new();
            let ws_before = store
                .set_websocket_notes(owner, message.id, 0, "keep WS", &cancel)
                .unwrap();
            let http_before = store
                .set_ext_http_notes(owner, source, items[0].id, 0, "keep HTTP", &cancel)
                .unwrap();
            let before = store.workspace().unwrap();
            let mut workspace = before.clone();
            workspace.open_request(None).unwrap();
            let recovery = crate::store::tests::record();
            let recovered_message = websocket_message(recovery.summary.id);
            let ws_notes = [WebSocketNotesChange {
                message_id: message.id,
                expected_notes_revision: u64::from(stale_http),
                text: "changed WS".into(),
            }];
            let http_notes = [ExtHttpNotesChange {
                source_id: source,
                item_id: items[2].id,
                expected_notes_revision: u64::from(!stale_http),
                text: "changed HTTP".into(),
            }];
            assert!(
                store
                    .save_with_notes(
                        save_as.then_some(copy.as_path()),
                        (Some(&workspace), &ws_notes, &http_notes, &[]),
                        &[recovery.clone().into()],
                        &[recovered_message.clone().into()],
                        &[],
                        &cancel,
                    )
                    .is_err(),
                "save_as={save_as}, stale_http={stale_http}"
            );
            for suffix in ["", "-wal", "-shm", "-journal"] {
                assert!(
                    !copy
                        .with_file_name(format!("not-published.mio{suffix}"))
                        .exists()
                );
            }
            assert_eq!(store.path.as_deref(), Some(source_path.as_path()));
            assert_eq!(store.workspace().unwrap(), before);
            assert_eq!(store.websocket_notes(owner, message.id).unwrap(), ws_before);
            assert_eq!(
                store.ext_http_notes(owner, source, items[0].id).unwrap(),
                http_before
            );
            assert!(store.read(recovery.summary.id).is_err());
            assert!(store.read_websocket(recovered_message.id).is_err());
            assert_eq!(store.read_websocket(message.id).unwrap(), message);
            assert_eq!(store.ext_item(items[0].id).unwrap(), items[0]);
            store.close().unwrap();
        }
    }
}

#[test]
fn readonly_source_save_as_commits_both_notes_workspace_and_recovered_message_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("readonly-source.mio");
    let copy = directory.path().join("recovered-copy.mio");
    let (mut store, source, items) = fixture("source HTTP note");
    store.save(Some(&source_path)).unwrap();
    let owner = project(&store);
    let flow = crate::store::tests::record();
    store.put(&flow).unwrap();
    let message = websocket_message(flow.summary.id);
    store.put_websocket(&message).unwrap();
    let cancel = CancellationToken::new();
    let ws_before = store
        .set_websocket_notes(owner, message.id, 0, "source WS note", &cancel)
        .unwrap();
    let http_before = store
        .set_ext_http_notes(owner, source, items[0].id, 0, "stored HTTP note", &cancel)
        .unwrap();
    let before = store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.open_request(None).unwrap();
    let recovery = crate::store::tests::record();
    let recovered_message = websocket_message(recovery.summary.id);
    let ws_notes = [
        WebSocketNotesChange {
            message_id: message.id,
            expected_notes_revision: 1,
            text: "copy WS note".into(),
        },
        WebSocketNotesChange {
            message_id: recovered_message.id,
            expected_notes_revision: 0,
            text: "恢复 WS🙂".into(),
        },
    ];
    let http_notes = [ExtHttpNotesChange {
        source_id: source,
        item_id: items[2].id,
        expected_notes_revision: 1,
        text: "# 恢复 HTTP\n**原文🙂**".into(),
    }];
    store.reject_writes(true);
    assert!(
        store
            .set_websocket_notes(owner, message.id, 1, &ws_notes[0].text, &cancel)
            .is_err()
    );
    assert!(
        store
            .set_ext_http_notes(owner, source, items[0].id, 1, &http_notes[0].text, &cancel)
            .is_err()
    );
    assert!(
        store
            .save_with_notes(
                None,
                (Some(&workspace), &ws_notes, &http_notes, &[]),
                &[],
                &[],
                &[],
                &cancel
            )
            .is_err()
    );
    let (path, bytes, (ws_receipts, http_receipts, highlight_receipts)) = store
        .save_with_notes(
            Some(&copy),
            (Some(&workspace), &ws_notes, &http_notes, &[]),
            &[recovery.clone().into()],
            &[recovered_message.clone().into()],
            &[],
            &cancel,
        )
        .unwrap();
    assert_eq!(path, copy);
    assert!(highlight_receipts.is_empty());
    assert!(bytes > 0);
    assert_eq!(ws_receipts, [(message.id, 2), (recovered_message.id, 1)]);
    assert_eq!(http_receipts, [(source, items[2].id, 2)]);
    assert_eq!(store.path.as_deref(), Some(copy.as_path()));
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(
        store.websocket_notes(owner, message.id).unwrap().text,
        ws_notes[0].text
    );
    assert_eq!(
        store
            .websocket_notes(owner, recovered_message.id)
            .unwrap()
            .text,
        ws_notes[1].text
    );
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[0].id)
            .unwrap()
            .text,
        http_notes[0].text
    );
    assert_eq!(
        store.read_websocket(recovered_message.id).unwrap(),
        recovered_message
    );
    store.close().unwrap();
    let original = ProjectStore::open(&source_path).unwrap();
    assert_eq!(original.workspace().unwrap(), before);
    assert_eq!(
        original.websocket_notes(owner, message.id).unwrap(),
        ws_before
    );
    assert_eq!(
        original
            .ext_http_notes(owner, source, items[0].id)
            .unwrap(),
        http_before
    );
    assert!(original.read(recovery.summary.id).is_err());
    assert!(original.read_websocket(recovered_message.id).is_err());
    original.close().unwrap();
    let reopened = ProjectStore::open(&copy).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(
        reopened.read_websocket(recovered_message.id).unwrap(),
        recovered_message
    );
    assert_eq!(
        reopened
            .ext_http_notes(owner, source, items[2].id)
            .unwrap()
            .text,
        http_notes[0].text
    );
    for item in items {
        assert_eq!(reopened.ext_item(item.id).unwrap(), item);
    }
    assert_eq!(
        reopened.read_source_range(source, 0, 4096).unwrap(),
        vec![0; 4096]
    );
    reopened.close().unwrap();
}

#[test]
fn readonly_save_as_preserves_independent_capacities_for_33_http_and_32_websocket_notes() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("independent-source.mio");
    let copy = directory.path().join("independent-copy.mio");
    let (mut store, source, items) = fixture_with_http_items("source", 33);
    let owner = project(&store);
    let flow = crate::store::tests::record();
    store.put(&flow).unwrap();
    let mut ws_messages = Vec::new();
    let mut ws_notes = Vec::new();
    for index in 0..32 {
        let mut message = websocket_message(flow.summary.id);
        message.sequence = index + 1;
        store.put_websocket(&message).unwrap();
        ws_notes.push(WebSocketNotesChange {
            message_id: message.id,
            expected_notes_revision: 0,
            text: format!("WS {index}🙂"),
        });
        ws_messages.push(message);
    }
    let http_notes: Vec<_> = items
        .iter()
        .filter(|item| item.collection == "root.f1.f0")
        .map(|item| ExtHttpNotesChange {
            source_id: source,
            item_id: item.id,
            expected_notes_revision: 0,
            text: format!("HTTP {}🙂", item.index),
        })
        .collect();
    assert_eq!((http_notes.len(), ws_notes.len()), (33, 32));
    let before = store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.open_request(None).unwrap();
    store.save(Some(&source_path)).unwrap();
    store.reject_writes(true);
    let cancel = CancellationToken::new();
    assert!(
        store
            .set_ext_http_notes(
                owner,
                source,
                http_notes[0].item_id,
                0,
                &http_notes[0].text,
                &cancel
            )
            .is_err()
    );
    assert!(
        store
            .set_websocket_notes(owner, ws_notes[0].message_id, 0, &ws_notes[0].text, &cancel)
            .is_err()
    );
    let (path, bytes, (ws_receipts, http_receipts, highlight_receipts)) = store
        .save_with_notes(
            Some(&copy),
            (Some(&workspace), &ws_notes, &http_notes, &[]),
            &[],
            &[],
            &[],
            &cancel,
        )
        .unwrap();
    assert_eq!(path, copy);
    assert!(highlight_receipts.is_empty());
    assert!(bytes > 0);
    assert_eq!(
        ws_receipts,
        ws_notes
            .iter()
            .map(|change| (change.message_id, 1))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        http_receipts,
        http_notes
            .iter()
            .map(|change| (source, change.item_id, 1))
            .collect::<Vec<_>>()
    );
    assert_eq!(store.workspace().unwrap(), workspace);
    store.close().unwrap();
    for (path, copied) in [(&source_path, false), (&copy, true)] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(
            reopened.workspace().unwrap(),
            if copied {
                workspace.clone()
            } else {
                before.clone()
            }
        );
        for change in &ws_notes {
            let notes = reopened.websocket_notes(owner, change.message_id).unwrap();
            assert_eq!(notes.revision, u64::from(copied));
            assert_eq!(notes.text, if copied { change.text.as_str() } else { "" });
        }
        for change in &http_notes {
            let notes = reopened
                .ext_http_notes(owner, source, change.item_id)
                .unwrap();
            assert_eq!(notes.revision, u64::from(copied));
            assert_eq!(
                notes.text,
                if copied {
                    change.text.as_str()
                } else {
                    "source"
                }
            );
        }
        for message in &ws_messages {
            assert_eq!(reopened.read_websocket(message.id).unwrap(), *message);
        }
        for item in &items {
            assert_eq!(reopened.ext_item(item.id).unwrap(), *item);
        }
        assert_eq!(
            reopened.read_source_range(source, 0, 4096).unwrap(),
            vec![0; 4096]
        );
        reopened.close().unwrap();
    }
}

#[test]
fn schema_21_upgrade_keeps_unmapped_source_notes_unknown_without_rewriting_source_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old-source.mio");
    let (mut store, source, mut items) = fixture("unused old note");
    for item in &mut items {
        let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
        metadata["record"]["mapping"]["value"]
            .as_object_mut()
            .unwrap()
            .remove("annotations");
        replace_metadata(&store, item, &metadata);
        item.metadata_json = serde_json::to_vec(&metadata).unwrap();
    }
    let workspace = store.workspace().unwrap();
    let report = store.ext_report(source).unwrap();
    store.save(Some(&path)).unwrap();
    store.close().unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DROP TABLE site_map_entries; DROP INDEX flows_source_started; DROP INDEX flows_source_status; DROP INDEX flows_source_capture; ALTER TABLE flows DROP COLUMN response_length; ALTER TABLE flows DROP COLUMN capture_number; ALTER TABLE flows DROP COLUMN elapsed_ms; ALTER TABLE flows DROP COLUMN response_bytes; ALTER TABLE flows DROP COLUMN status; ALTER TABLE flows DROP COLUMN source; ALTER TABLE flows DROP COLUMN listener_port; ALTER TABLE intruder_results DROP COLUMN highlight_revision; ALTER TABLE intruder_results DROP COLUMN highlight; ALTER TABLE intruder_results DROP COLUMN notes_revision; ALTER TABLE intruder_results DROP COLUMN notes; DROP TABLE ext_http_highlights; DROP TABLE websocket_originals; DROP TABLE ext_http_notes; PRAGMA user_version=21;",
        )
        .unwrap();
    connection.close().unwrap();
    let mut reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        crate::store::SCHEMA
    );
    assert_eq!(
        reopened
            .connection
            .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_notes", [], |row| row.get(0))
            .unwrap(),
        0
    );
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(reopened.ext_report(source).unwrap(), report);
    let owner = project(&reopened);
    assert!(
        reopened
            .ext_http_notes(owner, source, items[0].id)
            .is_err()
    );
    assert!(
        reopened
            .set_ext_http_notes(
                owner,
                source,
                items[0].id,
                0,
                "must not replace unknown",
                &CancellationToken::new()
            )
            .is_err()
    );
    let (total, rows) = reopened.history_page(0, 10, "").unwrap();
    assert_eq!((total, rows.len()), (3, 3));
    for row in rows {
        let HistoryRow::ExtProxy(row) = row else {
            panic!("unexpected live row")
        };
        assert!(row.notes_error.is_some());
        assert_eq!(row.notes_revision, 0);
        assert!(row.notes_preview.is_empty());
    }
    for item in items {
        assert_eq!(reopened.ext_item(item.id).unwrap(), item);
    }
    assert_eq!(
        reopened.read_source_range(source, 0, 4096).unwrap(),
        vec![0; 4096]
    );
    reopened.close().unwrap();
}
