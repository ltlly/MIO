use super::*;

#[test]
fn snapshot_reopen_preserves_bodies_headers_workspace_and_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.mio");
    let mut store = ProjectStore::create(None).unwrap();
    let record = record();
    store.put(&record).unwrap();
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(Some(&record.request)).unwrap();
    store.save_workspace(&workspace).unwrap();
    let (_, total) = store.save(Some(&path)).unwrap();
    assert!(total >= fs::metadata(&path).unwrap().len());
    assert_eq!(store.read(record.summary.id).unwrap(), record);
    drop(store);
    let store = ProjectStore::open(&path).unwrap();
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read(record.summary.id).unwrap(), record);
    assert_eq!(store.page(0, 1, "中文").unwrap().1, vec![record.summary]);
}

#[test]
fn conflicts_foreign_files_and_overwrites_do_not_change_data() {
    let directory = tempfile::tempdir().unwrap();
    let foreign = directory.path().join("sample.ext_archive");
    fs::write(&foreign, b"not a MIO project").unwrap();
    let before = fs::read(&foreign).unwrap();
    assert!(ProjectStore::open(&foreign).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), before);
    let mut store = ProjectStore::create(None).unwrap();
    assert!(store.save(Some(&foreign)).is_err());
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(None).unwrap();
    store.save_workspace(&workspace).unwrap();
    assert!(store.save_workspace(&workspace).is_err());
    assert_eq!(store.workspace().unwrap(), workspace);
}

#[test]
fn regression_closing_a_tab_cannot_roll_back_its_latest_draft() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(None).unwrap();
    let stale = workspace.tabs[0].clone();
    workspace.tabs[0].draft.revision = 5;
    workspace.tabs[0]
        .draft
        .raw
        .extend_from_slice("新草稿".as_bytes());
    store.save_workspace(&workspace).unwrap();
    let latest = workspace.clone();
    workspace.tabs.clear();
    workspace.active = None;
    workspace.closed.push(stale);
    workspace.revision += 1;
    assert!(
        store.save_workspace(&workspace).is_err(),
        "closed tabs must retain their latest draft revision"
    );
    assert_eq!(store.workspace().unwrap(), latest);
}

#[test]
fn regression_same_revision_cannot_replace_flow_content() {
    let mut store = ProjectStore::create(None).unwrap();
    let original = record();
    store.put(&original).unwrap();
    store.put(&original).unwrap();
    let mut modified = original.clone();
    modified.request.body[0] = 2;
    assert!(
        store.put(&modified).is_err(),
        "same revision is idempotent, not last-writer-wins"
    );
    assert_eq!(store.read(original.summary.id).unwrap(), original);
}

#[test]
fn regression_removing_response_cannot_leave_stale_response_bytes() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut record = record();
    record.response = Some(HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: b"old response".to_vec(),
        trailers: vec![],
    });
    store.put(&record).unwrap();
    record.summary.revision += 1;
    record.response = None;
    store.put(&record).unwrap();
    assert!(store.read(record.summary.id).unwrap().response.is_none());
}

#[test]
fn regression_first_original_survives_repeated_edits_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("modified.mio");
    let mut store = ProjectStore::create(None).unwrap();
    let original = record();
    store.put(&original).unwrap();
    let mut modified = original.clone();
    for byte in [2, 3] {
        modified.summary.revision += 1;
        modified.request.body[0] = byte;
        store.put(&modified).unwrap();
    }
    store.save(Some(&path)).unwrap();
    drop(store);
    let store = ProjectStore::open(&path).unwrap();
    assert_eq!(store.read(modified.summary.id).unwrap(), modified);
    let original_body: Option<Vec<u8>> = store
        .connection
        .query_row(
            "SELECT body FROM messages WHERE flow=?1 AND side=2",
            [original.summary.id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(original_body, Some(original.request.body));
}

#[test]
fn regression_checkpoint_contention_cannot_strand_a_recovery_batch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("busy.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(None).unwrap();
    let mut pending = record();
    pending.summary.state = FlowState::Pending;
    let mut reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let snapshot = reader.transaction().unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT count(*) FROM flows", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        0
    );

    let first = store.save_with(
        None,
        Some(&workspace),
        &[pending.clone().into()],
        &[],
        &[],
        &CancellationToken::new(),
    );
    let (busy, _, _): (i32, i32, i32) = store
        .connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(
        busy, 1,
        "the established read snapshot causally blocks checkpointing"
    );
    snapshot.rollback().unwrap();

    // A durable WAL save may succeed without a checkpoint; any reported failure must be retryable.
    let retry = if first.is_ok() {
        store.save(None)
    } else {
        store.save_with(
            None,
            Some(&workspace),
            &[pending.clone().into()],
            &[],
            &[],
            &CancellationToken::new(),
        )
    };
    assert!(
        retry.is_ok(),
        "a failed save must not strand the same workspace/recovery batch: {retry:?}"
    );
    let expected_revision = pending.summary.revision + 1;
    pending.summary.revision = expected_revision;
    pending.summary.state = FlowState::Cancelled;
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read(pending.summary.id).unwrap(), pending);
    drop(store);
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(reopened.read(pending.summary.id).unwrap(), pending);
}

#[test]
fn regression_failed_recovery_batch_does_not_partially_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("atomic.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let existing = record();
    store.put(&existing).unwrap();
    let before = store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.open_request(None).unwrap();
    let added = record();
    let mut conflicting = existing.clone();
    conflicting.request.body.push(42);
    conflicting.summary.request_bytes = conflicting.request.body.len() as u64;
    assert!(
        store
            .save_with(
                None,
                Some(&workspace),
                &[added.clone().into(), conflicting.clone().into()],
                &[],
                &[],
                &CancellationToken::new(),
            )
            .is_err()
    );
    assert_eq!(
        store.workspace().unwrap(),
        before,
        "a later flow failure rolls back the workspace in the same batch"
    );
    assert!(
        store.read(added.summary.id).is_err(),
        "a failed batch cannot persist only its earlier flows"
    );
    assert_eq!(store.read(existing.summary.id).unwrap(), existing);

    conflicting.summary.revision += 1;
    store
        .save_with(
            None,
            Some(&workspace),
            &[added.clone().into(), conflicting.clone().into()],
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read(added.summary.id).unwrap(), added);
    assert_eq!(store.read(existing.summary.id).unwrap(), conflicting);
}
