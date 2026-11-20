use super::*;

#[test]
fn repeater_recovery_replays_pairs_idempotently_through_save_as_and_both_reopens() {
    for already_final in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repeater.mio");
        let copy = directory.path().join("repeater-copy.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let (pending, received, decoded) = repeater_records();
        store.put(&pending).unwrap();
        if already_final {
            store.put_repeater_completion(&received, &decoded).unwrap();
        }
        let records = [
            received.clone().into(),
            decoded.clone().into(),
            received.clone().into(),
            decoded.clone().into(),
        ];
        let mut workspace = store.workspace().unwrap();
        workspace.revision += 1;
        workspace.repeater_settings.unpack_compressed_responses = false;
        store
            .save_with(
                None,
                Some(&workspace),
                &records,
                &[],
                &[],
                &CancellationToken::new(),
            )
            .unwrap();
        store
            .save_with(
                Some(&copy),
                None,
                &records,
                &[],
                &[],
                &CancellationToken::new(),
            )
            .unwrap();
        store.close().unwrap();
        for path in [&path, &copy] {
            let reopened = ProjectStore::open(path).unwrap();
            assert_eq!(reopened.workspace().unwrap(), workspace);
            assert_eq!(reopened.read(decoded.summary.id).unwrap(), decoded);
            assert_eq!(
                reopened.original(decoded.summary.id).unwrap(),
                OriginalMessages {
                    response: received.response.clone(),
                    ..OriginalMessages::default()
                }
            );
            reopened.close().unwrap();
        }
    }
}

#[test]
fn repeater_recovery_original_conflict_rolls_back_workspace_and_prior_pair() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeater-conflict.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let (pending, received, decoded) = repeater_records();
    store.put(&pending).unwrap();
    let (_, mut conflicting, saved) = repeater_records();
    store.put_repeater_completion(&conflicting, &saved).unwrap();
    let original = store.original(saved.summary.id).unwrap();
    conflicting.response.as_mut().unwrap().body.push(42);
    conflicting.summary.response_bytes += 1;
    let before = store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.revision += 1;
    workspace.repeater_settings.unpack_compressed_responses = false;
    let records = [
        received.into(),
        decoded.into(),
        conflicting.into(),
        saved.clone().into(),
    ];
    assert!(
        store
            .save_with(
                None,
                Some(&workspace),
                &records,
                &[],
                &[],
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(store.workspace().unwrap(), before);
    assert_eq!(store.read(pending.summary.id).unwrap(), pending);
    assert_eq!(
        store.original(pending.summary.id).unwrap(),
        OriginalMessages::default()
    );
    assert_eq!(store.read(saved.summary.id).unwrap(), saved);
    assert_eq!(store.original(saved.summary.id).unwrap(), original);
}
