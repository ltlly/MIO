use super::super::tests::fixture_with_http_items;
use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

#[test]
fn shared_note_batch_over_single_draft_cap_preserves_aliases_clear_and_save_as() {
    let (mut store, source, items) = fixture_with_http_items("source note", 65);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let ids: Vec<_> = items
        .iter()
        .filter(|item| item.collection == "root.f1.f0")
        .map(|item| (source, item.id))
        .collect();
    assert_eq!(ids.len(), 65);
    let loaded = store.ext_http_notes_batch(owner, &ids, &cancel).unwrap();
    assert_eq!(loaded.common_text.as_deref(), Some("source note"));
    let states = loaded.states;
    assert!(
        states
            .iter()
            .all(|&(_, _, revision, has_notes)| revision == 0 && has_notes)
    );
    let mut changes: Vec<_> = states
        .iter()
        .map(|&(source, item, revision, _)| (source, item, revision))
        .collect();
    changes[0].1 = items[2].id;
    let text = "# 批量备注\n**原文🙂**";
    let saved = store
        .set_ext_http_notes_batch(owner, &changes, text, &cancel)
        .unwrap();
    assert_eq!(
        saved,
        changes
            .iter()
            .map(|&(source, item, _)| (source, item, 1))
            .collect::<Vec<_>>()
    );
    for &(_, item) in &ids {
        assert_eq!(
            store.ext_http_notes(owner, source, item).unwrap().text,
            text
        );
    }
    assert_eq!(
        store
            .set_ext_http_notes_batch(owner, &saved, text, &cancel)
            .unwrap(),
        saved
    );
    let cleared = store
        .set_ext_http_notes_batch(owner, &saved, "", &cancel)
        .unwrap();
    assert!(cleared.iter().all(|&(_, _, revision)| revision == 2));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("shared-notes.mio");
    store.save(Some(&path)).unwrap();
    store.close().unwrap();
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .ext_http_notes_batch(owner, &ids, &cancel)
            .unwrap()
            .states,
        ids.iter()
            .map(|&(source, item)| (source, item, 2, false))
            .collect::<Vec<_>>()
    );
    for item in items {
        assert_eq!(reopened.ext_item(item.id).unwrap(), item);
    }
    assert_eq!(
        reopened.read_source_range(source, 0, 4096).unwrap(),
        [0; 4096]
    );
    reopened.close().unwrap();
}

#[test]
fn shared_note_batch_conflict_duplicate_owner_or_invalid_text_never_partially_writes() {
    let (mut store, source, items) = fixture_with_http_items("source note", 2);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let ids = [(source, items[0].id), (source, items[1].id)];
    let initial = [(source, items[0].id, 0), (source, items[1].id, 0)];
    let saved = store
        .set_ext_http_notes_batch(owner, &initial, "kept", &cancel)
        .unwrap();
    let failures = [
        (
            vec![(source, items[0].id, 1), (source, items[1].id, 0)],
            "stale".to_owned(),
        ),
        (
            vec![(source, items[0].id, 1), (source, items[2].id, 1)],
            "duplicate aliases".to_owned(),
        ),
        (saved.clone(), "a".repeat(65537)),
        (
            vec![(source, items[0].id, u64::MAX)],
            "bad revision".to_owned(),
        ),
        (
            vec![(Uuid::new_v4(), items[0].id, 1)],
            "foreign source".to_owned(),
        ),
        (vec![], "empty selection".to_owned()),
    ];
    for (changes, text) in failures {
        assert!(
            store
                .set_ext_http_notes_batch(owner, &changes, &text, &cancel)
                .is_err()
        );
        assert_eq!(
            store
                .ext_http_notes_batch(owner, &ids, &cancel)
                .unwrap()
                .states,
            ids.iter()
                .map(|&(source, item)| (source, item, 1, true))
                .collect::<Vec<_>>()
        );
        for &(_, item) in &ids {
            assert_eq!(
                store.ext_http_notes(owner, source, item).unwrap().text,
                "kept"
            );
        }
    }
    let error = store
        .set_ext_http_notes_batch(owner, &initial, "stale", &cancel)
        .unwrap_err();
    assert_eq!(
        error.stored,
        ids.iter()
            .map(|&(source, item)| (source, item, 1, true))
            .collect::<Vec<_>>()
    );
    store.close().unwrap();
}

#[test]
fn cancelled_shared_note_batch_does_not_read_source_or_overwrite_any_note() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (mut store, source, items) = fixture_with_http_items("source note", 2);
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
    let ids = [(source, items[2].id), (source, items[1].id)];
    assert!(store.ext_http_notes_batch(owner, &ids, &cancel).is_err());
    let error = store
        .set_ext_http_notes_batch(
            owner,
            &ids.map(|(source, item)| (source, item, 0)),
            "cancelled",
            &cancel,
        )
        .unwrap_err();
    assert!(error.stored.is_empty());
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store
            .ext_http_notes(owner, source, items[0].id)
            .unwrap()
            .text,
        "source note"
    );
    store.close().unwrap();
}

#[test]
fn common_note_text_compares_full_source_and_overlay_values_including_mixed_empty() {
    let text = format!("{}\n**中文🙂**\r\n", "a".repeat(300));
    let (mut store, source, items) = fixture_with_http_items(&text, 2);
    let owner = project(&store);
    let cancel = CancellationToken::new();
    let ids = [(source, items[2].id), (source, items[1].id)];
    let loaded = store.ext_http_notes_batch(owner, &ids, &cancel).unwrap();
    assert_eq!(loaded.common_text.as_deref(), Some(text.as_str()));
    assert!(
        loaded
            .states
            .iter()
            .all(|&(_, _, revision, has_notes)| revision == 0 && has_notes)
    );

    store
        .set_ext_http_notes(owner, source, items[0].id, 0, "temporary", &cancel)
        .unwrap();
    store
        .set_ext_http_notes(owner, source, items[0].id, 1, &text, &cancel)
        .unwrap();
    let loaded = store.ext_http_notes_batch(owner, &ids, &cancel).unwrap();
    assert_eq!(loaded.common_text.as_deref(), Some(text.as_str()));
    assert_eq!(
        loaded.states,
        [
            (source, items[2].id, 2, true),
            (source, items[1].id, 0, true)
        ]
    );

    store
        .set_ext_http_notes(owner, source, items[0].id, 2, "", &cancel)
        .unwrap();
    let mixed_empty = store.ext_http_notes_batch(owner, &ids, &cancel).unwrap();
    assert!(mixed_empty.common_text.is_none());
    assert_eq!(
        mixed_empty.states,
        [
            (source, items[2].id, 3, false),
            (source, items[1].id, 0, true)
        ]
    );
    store
        .set_ext_http_notes(owner, source, items[1].id, 0, "", &cancel)
        .unwrap();
    let empty = store.ext_http_notes_batch(owner, &ids, &cancel).unwrap();
    assert_eq!(empty.common_text.as_deref(), Some(""));
    assert!(empty.states.iter().all(|&(_, _, _, has_notes)| !has_notes));

    store
        .set_ext_http_notes(owner, source, items[0].id, 3, &format!("{text}A"), &cancel)
        .unwrap();
    store
        .set_ext_http_notes(owner, source, items[1].id, 1, &format!("{text}B"), &cancel)
        .unwrap();
    let mixed_suffix = store.ext_http_notes_batch(owner, &ids, &cancel).unwrap();
    assert!(
        mixed_suffix.common_text.is_none(),
        "identical 256-character previews do not prove equal full notes"
    );
    assert!(
        mixed_suffix
            .states
            .iter()
            .all(|&(_, _, _, has_notes)| has_notes)
    );
    for item in items {
        assert_eq!(store.ext_item(item.id).unwrap(), item);
    }
    store.close().unwrap();
}
