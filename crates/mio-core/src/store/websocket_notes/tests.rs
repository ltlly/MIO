use super::super::{CapturedWebSocket, tests::record as flow_record};
use super::*;
use crate::{
    WebSocketDirection, WebSocketHistoryFilter, WebSocketHistoryQuery, WebSocketKind,
    WebSocketMessage,
};

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

fn message(flow: Uuid, sequence: u64) -> WebSocketMessage {
    WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flow,
        sequence,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Binary,
        payload: vec![0, 255, 42],
        observed_unix_ms: 5,
    }
}

#[test]
fn notes_keep_unicode_and_independent_revisions_and_filter_without_rewriting_payload() {
    let mut store = ProjectStore::create(None).unwrap();
    let owner = project(&store);
    let flow = flow_record();
    store.put(&flow).unwrap();
    let first = message(flow.summary.id, 1);
    let second = message(flow.summary.id, 2);
    store.put_websocket(&first).unwrap();
    store.put_websocket(&second).unwrap();
    assert_eq!(
        store.websocket_notes(owner, first.id).unwrap(),
        WebSocketNotes::default()
    );
    let text = format!("多行\nUnicode🙂\0{}", "中🙂".repeat(300));
    let saved = store
        .set_websocket_notes(owner, first.id, 0, &text, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        saved,
        WebSocketNotes {
            revision: 1,
            text: text.clone()
        }
    );
    let query = WebSocketHistoryQuery {
        project: owner,
        query_revision: 1,
        filter: WebSocketHistoryFilter {
            only_with_notes: true,
            term: r"\xFF".into(),
            regex: true,
            ..Default::default()
        },
        flow_id: None,
        offset: 0,
        limit: 1,
        selected_message_id: Some(second.id),
    };
    let page = ProjectStore::websocket_query_snapshot(
        &store.actual_path(),
        &query,
        &[],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        (
            page.total,
            page.rows[0].id,
            page.rows[0].revision,
            page.rows[0].notes_revision
        ),
        (1, first.id, 1, 1)
    );
    assert_eq!(
        page.rows[0].notes_preview,
        text.chars().take(256).collect::<String>()
    );
    assert!(page.rows[0].notes_preview.len() <= 1024);
    assert_eq!(page.selected_match, Some((second.id, false)));
    assert_eq!(store.read_websocket(first.id).unwrap(), first);
    assert_eq!(store.websocket_notes(owner, first.id).unwrap(), saved);
    let cleared = store
        .set_websocket_notes(owner, first.id, 1, "", &CancellationToken::new())
        .unwrap();
    assert_eq!(
        cleared,
        WebSocketNotes {
            revision: 2,
            text: String::new()
        }
    );
    let stale = store
        .set_websocket_notes(owner, first.id, 0, "stale", &CancellationToken::new())
        .unwrap_err();
    assert_eq!(stale.stored, Some(cleared.clone()));
    assert_eq!(
        store
            .set_websocket_notes(owner, first.id, 2, "", &CancellationToken::new())
            .unwrap(),
        cleared
    );
    assert_eq!(
        ProjectStore::websocket_query_snapshot(
            &store.actual_path(),
            &query,
            &[],
            &CancellationToken::new()
        )
        .unwrap()
        .total,
        0
    );
    let maximum = "a".repeat(MAX_NOTES_BYTES);
    assert_eq!(
        store
            .set_websocket_notes(owner, first.id, 2, &maximum, &CancellationToken::new())
            .unwrap()
            .text,
        maximum
    );
    let oversized = format!("{maximum}中");
    assert!(
        store
            .set_websocket_notes(owner, first.id, 3, &oversized, &CancellationToken::new())
            .unwrap_err()
            .message
            .contains("64 KiB")
    );
    assert_eq!(store.read_websocket(first.id).unwrap(), first);
}

#[test]
fn cleared_note_save_as_reopen_keeps_empty_preview_and_saved_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cleared-notes.mio");
    let copy = directory.path().join("cleared-notes-copy.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let owner = project(&store);
    let flow = flow_record();
    store.put(&flow).unwrap();
    let record = message(flow.summary.id, 1);
    store.put_websocket(&record).unwrap();
    let cancel = CancellationToken::new();
    store
        .set_websocket_notes(owner, record.id, 0, "待清空🙂", &cancel)
        .unwrap();
    let cleared = store
        .set_websocket_notes(owner, record.id, 1, "", &cancel)
        .unwrap();
    assert_eq!(
        cleared,
        WebSocketNotes {
            revision: 2,
            text: String::new(),
        }
    );
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    let query = WebSocketHistoryQuery {
        project: owner,
        query_revision: 1,
        filter: WebSocketHistoryFilter::default(),
        flow_id: None,
        offset: 0,
        limit: 1,
        selected_message_id: Some(record.id),
    };
    for path in [&path, &copy] {
        let store = ProjectStore::open(path).unwrap();
        assert_eq!(store.websocket_notes(owner, record.id).unwrap(), cleared);
        let page = ProjectStore::websocket_query_snapshot(path, &query, &[], &cancel)
            .expect("cleared notes must not fail the history preview query");
        assert_eq!((page.total, page.rows.len()), (1, 1));
        assert_eq!(page.rows[0].id, record.id);
        assert_eq!(page.rows[0].notes_revision, cleared.revision);
        assert_eq!(page.rows[0].notes_preview, "");
        assert_eq!(page.selected_match, Some((record.id, true)));
        assert_eq!(store.read_websocket(record.id).unwrap(), record);
        store.close().unwrap();
    }
}

#[test]
fn readonly_source_save_as_commits_recovered_messages_and_notes_together() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mio");
    let copy = directory.path().join("notes-copy.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let owner = project(&store);
    let flow = flow_record();
    store.put(&flow).unwrap();
    let first = message(flow.summary.id, 1);
    store.put_websocket(&first).unwrap();
    store
        .set_websocket_notes(owner, first.id, 0, "source note", &CancellationToken::new())
        .unwrap();
    let recovered_flow = flow_record();
    let recovered = message(recovered_flow.summary.id, 1);
    let changes = vec![
        WebSocketNotesChange {
            message_id: first.id,
            expected_notes_revision: 1,
            text: "copy note".into(),
        },
        WebSocketNotesChange {
            message_id: recovered.id,
            expected_notes_revision: 0,
            text: "恢复\n新备注🙂".into(),
        },
    ];
    store.reject_writes(true);
    assert!(
        store
            .set_websocket_notes(
                owner,
                first.id,
                1,
                &changes[0].text,
                &CancellationToken::new()
            )
            .is_err()
    );
    let captures = [CapturedWebSocket::from(recovered.clone())];
    let (_, _, (receipts, http_receipts, highlight_receipts)) = store
        .save_with_notes(
            Some(&copy),
            (None, &changes, &[], &[]),
            &[recovered_flow.into()],
            &captures,
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(http_receipts.is_empty());
    assert!(highlight_receipts.is_empty());
    assert_eq!(receipts, [(first.id, 2), (recovered.id, 1)]);
    assert_eq!(store.path.as_deref(), Some(copy.as_path()));
    assert_eq!(
        store.websocket_notes(owner, recovered.id).unwrap().text,
        changes[1].text
    );
    assert_eq!(store.read_websocket(recovered.id).unwrap(), recovered);
    assert_eq!(store.read_websocket(first.id).unwrap(), first);
    let original = ProjectStore::open(&source).unwrap();
    assert_eq!(
        original.websocket_notes(owner, first.id).unwrap().text,
        "source note"
    );
    assert!(original.read_websocket(recovered.id).is_err());
    let same = [WebSocketNotesChange {
        expected_notes_revision: 2,
        ..changes[0].clone()
    }];
    assert_eq!(
        store
            .save_with_notes(
                None,
                (None, &same, &[], &[]),
                &[],
                &[],
                &[],
                &CancellationToken::new()
            )
            .unwrap()
            .2
            .0,
        [(first.id, 2)]
    );
}

#[test]
fn later_note_conflict_rolls_back_workspace_recovery_and_all_prior_notes() {
    for save_as in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("atomic-source.mio");
        let copy = directory.path().join("not-published.mio");
        let mut store = ProjectStore::create(Some(&source)).unwrap();
        let owner = project(&store);
        let flow = flow_record();
        store.put(&flow).unwrap();
        let original = message(flow.summary.id, 1);
        store.put_websocket(&original).unwrap();
        let notes = store
            .set_websocket_notes(owner, original.id, 0, "keep", &CancellationToken::new())
            .unwrap();
        let before = store.workspace().unwrap();
        let mut workspace = before.clone();
        workspace.open_request(None).unwrap();
        let new_flow = flow_record();
        let added = message(new_flow.summary.id, 1);
        let changes = [
            WebSocketNotesChange {
                message_id: added.id,
                expected_notes_revision: 0,
                text: "first succeeds then rolls back".into(),
            },
            WebSocketNotesChange {
                message_id: original.id,
                expected_notes_revision: 0,
                text: "stale".into(),
            },
        ];
        assert!(
            store
                .save_with_notes(
                    save_as.then_some(copy.as_path()),
                    (Some(&workspace), &changes, &[], &[]),
                    &[new_flow.clone().into()],
                    &[added.clone().into()],
                    &[],
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert!(!copy.exists());
        assert_eq!(store.path.as_deref(), Some(source.as_path()));
        assert_eq!(store.workspace().unwrap(), before);
        assert_eq!(store.websocket_notes(owner, original.id).unwrap(), notes);
        assert!(store.read(new_flow.summary.id).is_err());
        assert!(store.read_websocket(added.id).is_err());
        assert_eq!(store.read_websocket(original.id).unwrap(), original);
    }
}

#[test]
fn dependencies_cancellation_and_corrupt_notes_never_change_message_bytes() {
    let mut store = ProjectStore::create(None).unwrap();
    let owner = project(&store);
    let flow = flow_record();
    store.put(&flow).unwrap();
    let record = message(flow.summary.id, 1);
    store.put_websocket(&record).unwrap();
    let notes = store
        .set_websocket_notes(owner, record.id, 0, "kept", &CancellationToken::new())
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        store
            .set_websocket_notes(owner, record.id, 1, "cancelled", &cancel)
            .unwrap_err()
            .stored,
        Some(notes.clone())
    );
    store.connection.execute_batch("CREATE UNIQUE INDEX note_revision ON websocket_notes(revision); CREATE TABLE protected_note_reference(value INTEGER REFERENCES websocket_notes(revision) ON UPDATE CASCADE); INSERT INTO protected_note_reference VALUES(1);").unwrap();
    assert!(
        store
            .set_websocket_notes(
                owner,
                record.id,
                1,
                "must reject",
                &CancellationToken::new()
            )
            .unwrap_err()
            .message
            .contains("外键")
    );
    assert_eq!(store.websocket_notes(owner, record.id).unwrap(), notes);
    assert_eq!(
        store
            .connection
            .query_row("SELECT value FROM protected_note_reference", [], |row| row
                .get::<_, u64>(
                0
            ))
            .unwrap(),
        1
    );
    store
        .connection
        .execute(
            "UPDATE websocket_notes SET text=CAST(x'FF' AS TEXT) WHERE message=?1",
            [record.id.to_string()],
        )
        .unwrap();
    assert!(store.websocket_notes(owner, record.id).is_err());
    assert_eq!(store.read_websocket(record.id).unwrap(), record);
    assert!(
        store
            .websocket_notes(
                ProjectKey {
                    id: Uuid::nil(),
                    ..owner
                },
                record.id
            )
            .is_err()
    );
    assert!(store.websocket_notes(owner, Uuid::nil()).is_err());
}

#[test]
fn notes_batch_rejects_duplicate_or_nil_ids_and_excess_resources() {
    let change = WebSocketNotesChange {
        message_id: Uuid::new_v4(),
        expected_notes_revision: 0,
        text: String::new(),
    };
    assert!(validate_changes(&[change.clone(), change.clone()]).is_err());
    assert!(
        validate_changes(&[WebSocketNotesChange {
            message_id: Uuid::nil(),
            ..change.clone()
        }])
        .is_err()
    );
    let mut changes: Vec<_> = (0..64)
        .map(|_| WebSocketNotesChange {
            message_id: Uuid::new_v4(),
            ..change.clone()
        })
        .collect();
    assert!(validate_changes(&changes).is_ok());
    changes.push(change);
    assert!(validate_changes(&changes).is_err());
}
