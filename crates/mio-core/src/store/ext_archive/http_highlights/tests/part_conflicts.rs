use super::*;

#[test]
fn project_save_conflict_in_any_annotation_rolls_back_workspace_recovery_and_all_annotations() {
    for save_as in [false, true] {
        for stale in 0..3 {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("source.mio");
            let copy = directory.path().join("not-published.mio");
            let (mut store, source, items) = fixture(3);
            store.save(Some(&path)).unwrap();
            let owner = project(&store);
            let cancel = CancellationToken::new();
            let flow = crate::store::tests::record();
            store.put(&flow).unwrap();
            let message = websocket_message(flow.summary.id);
            store.put_websocket(&message).unwrap();
            let ws_before = store
                .set_websocket_notes(owner, message.id, 0, "keep WS", &cancel)
                .unwrap();
            let http_before = store
                .set_ext_http_notes(owner, source, items[0].id, 0, "keep HTTP", &cancel)
                .unwrap();
            let highlight_before = store
                .set_ext_http_highlights(owner, &[change(source, items[0].id, 0, 1)], &cancel)
                .unwrap();
            let before = store.workspace().unwrap();
            let mut workspace = before.clone();
            workspace.open_request(None).unwrap();
            let recovery = crate::store::tests::record();
            let recovered_message = websocket_message(recovery.summary.id);
            let ws_notes = [crate::WebSocketNotesChange {
                message_id: message.id,
                expected_notes_revision: u64::from(stale != 0),
                text: "changed WS".into(),
            }];
            let http_notes = [ExtHttpNotesChange {
                source_id: source,
                item_id: items[2].id,
                expected_notes_revision: u64::from(stale != 1),
                text: "changed HTTP".into(),
            }];
            let highlights = [change(source, items[2].id, u64::from(stale != 2), 0)];
            assert!(
                store
                    .save_with_notes(
                        save_as.then_some(copy.as_path()),
                        (Some(&workspace), &ws_notes, &http_notes, &highlights),
                        &[recovery.clone().into()],
                        &[recovered_message.clone().into()],
                        &[],
                        &cancel,
                    )
                    .is_err(),
                "save_as={save_as}, stale={stale}"
            );
            for suffix in ["", "-wal", "-shm", "-journal"] {
                assert!(
                    !copy
                        .with_file_name(format!("not-published.mio{suffix}"))
                        .exists()
                );
            }
            assert_eq!(store.path.as_deref(), Some(path.as_path()));
            assert_eq!(store.workspace().unwrap(), before);
            assert_eq!(store.websocket_notes(owner, message.id).unwrap(), ws_before);
            assert_eq!(
                store.ext_http_notes(owner, source, items[0].id).unwrap(),
                http_before
            );
            assert_eq!(
                store
                    .ext_http_highlights(owner, &[change(source, items[0].id, 0, 0)])
                    .unwrap(),
                highlight_before
            );
            assert!(store.read(recovery.summary.id).is_err());
            assert!(store.read_websocket(recovered_message.id).is_err());
            assert_eq!(store.read_websocket(message.id).unwrap(), message);
            assert_eq!(store.ext_item(items[0].id).unwrap(), items[0]);
            assert_eq!(store.read_source_range(source, 0, 1024).unwrap(), [0; 1024]);
            store.close().unwrap();
        }
    }
}

#[test]
fn readonly_source_save_as_commits_all_annotations_and_recovery_without_changing_original() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("readonly-source.mio");
    let copy = directory.path().join("recovered-copy.mio");
    let (mut store, source, items) = fixture(3);
    store.save(Some(&path)).unwrap();
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let flow = crate::store::tests::record();
    store.put(&flow).unwrap();
    let message = websocket_message(flow.summary.id);
    store.put_websocket(&message).unwrap();
    let before = store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.open_request(None).unwrap();
    let recovery = crate::store::tests::record();
    let recovered_message = websocket_message(recovery.summary.id);
    let ws_notes = [crate::WebSocketNotesChange {
        message_id: recovered_message.id,
        expected_notes_revision: 0,
        text: "恢复 WS🙂".into(),
    }];
    let http_notes = [ExtHttpNotesChange {
        source_id: source,
        item_id: items[2].id,
        expected_notes_revision: 0,
        text: "# 恢复 HTTP\n**原文🙂**".into(),
    }];
    let highlights = [
        change(source, items[2].id, 0, 0),
        change(source, items[1].id, 0, 9),
    ];
    store.reject_writes(true);
    assert!(
        store
            .set_ext_http_highlights(owner, &highlights, &cancel)
            .is_err()
    );
    assert!(
        store
            .save_with_notes(
                None,
                (Some(&workspace), &ws_notes, &http_notes, &highlights),
                &[],
                &[],
                &[],
                &cancel
            )
            .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        store
            .save_with_notes(
                Some(&copy),
                (Some(&workspace), &ws_notes, &http_notes, &highlights),
                &[recovery.clone().into()],
                &[recovered_message.clone().into()],
                &[],
                &cancelled
            )
            .is_err()
    );
    assert!(!copy.exists());
    let (saved, bytes, (ws_receipts, http_receipts, highlight_receipts)) = store
        .save_with_notes(
            Some(&copy),
            (Some(&workspace), &ws_notes, &http_notes, &highlights),
            &[recovery.clone().into()],
            &[recovered_message.clone().into()],
            &[],
            &cancel,
        )
        .unwrap();
    assert_eq!(saved, copy);
    assert!(bytes > 0);
    assert_eq!(ws_receipts, [(recovered_message.id, 1)]);
    assert_eq!(http_receipts, [(source, items[2].id, 1)]);
    assert_eq!(
        highlight_receipts,
        [(source, items[2].id, 1), (source, items[1].id, 1)]
    );
    store.close().unwrap();
    for (target, changed) in [(&path, false), (&copy, true)] {
        let reopened = ProjectStore::open(target).unwrap();
        assert_eq!(
            reopened.workspace().unwrap(),
            if changed {
                workspace.clone()
            } else {
                before.clone()
            }
        );
        let note = reopened
            .ext_http_notes(owner, source, items[0].id)
            .unwrap();
        assert_eq!(note.revision, u64::from(changed));
        assert_eq!(
            note.text,
            if changed {
                http_notes[0].text.as_str()
            } else {
                ""
            }
        );
        let colors = reopened.ext_http_highlights(owner, &highlights).unwrap();
        assert_eq!(colors.len(), 2);
        for (index, (_, _, value)) in colors.iter().enumerate() {
            assert_eq!(value.revision, u64::from(changed));
            assert_eq!(
                value.color,
                if changed { highlights[index].color } else { 3 }
            );
        }
        assert_eq!(reopened.read_websocket(message.id).unwrap(), message);
        if changed {
            assert_eq!(reopened.read(recovery.summary.id).unwrap(), recovery);
            assert_eq!(
                reopened.read_websocket(recovered_message.id).unwrap(),
                recovered_message
            );
            assert_eq!(
                reopened
                    .websocket_notes(owner, recovered_message.id)
                    .unwrap()
                    .text,
                ws_notes[0].text
            );
        } else {
            assert!(reopened.read(recovery.summary.id).is_err());
            assert!(reopened.read_websocket(recovered_message.id).is_err());
        }
        for item in &items {
            assert_eq!(reopened.ext_item(item.id).unwrap(), *item);
        }
        assert_eq!(
            reopened.read_source_range(source, 0, 1024).unwrap(),
            [0; 1024]
        );
        reopened.close().unwrap();
    }
}

#[test]
fn cancelled_highlight_does_not_reread_source_for_conflict_recovery() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let (mut store, source, items) = fixture(3);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "ext_items",
                    ..
                }
            ) {
                observed.fetch_add(1, Ordering::Relaxed);
            }
            Authorization::Allow
        }));
    let error = store
        .set_ext_http_highlights(owner, &[change(source, items[2].id, 0, 1)], &cancel)
        .unwrap_err();
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    assert!(error.message.contains("取消"));
    assert_eq!(
        reads.load(Ordering::Relaxed),
        0,
        "a cancelled operation must not start a source-wide alias reread"
    );
    assert!(error.stored.is_empty());
    assert_eq!(
        store
            .connection
            .query_row::<usize, _, _>("SELECT count(*) FROM ext_http_highlights", [], |row| {
                row.get(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(store.ext_item(items[2].id).unwrap(), items[2]);
    store.close().unwrap();
}
