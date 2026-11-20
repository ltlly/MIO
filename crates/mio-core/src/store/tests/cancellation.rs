use super::*;

#[test]
fn cancelled_recovery_batch_rolls_back_workspace_and_remains_retryable() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cancelled.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let existing = record();
    store.put(&existing).unwrap();
    let before = store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.open_request(None).unwrap();
    let mut pending = record();
    pending.summary.state = FlowState::Pending;
    let recovery = [pending.clone().into()];
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if let AuthAction::Insert {
                table_name: "flows",
                ..
            } = context.action
            {
                trigger.cancel();
            }
            Authorization::Allow
        }));
    let result = store.save_with(None, Some(&workspace), &recovery, &[], &[], &cancel);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(cancel.is_cancelled());
    let error = result.expect_err("cancellation before commit must roll back the batch");
    assert!(error.to_string().contains("取消"), "{error}");
    assert_eq!(store.path.as_deref(), Some(path.as_path()));
    assert_eq!(store.workspace().unwrap(), before);
    assert_eq!(store.read(existing.summary.id).unwrap(), existing);
    assert!(store.read_optional(pending.summary.id).unwrap().is_none());

    store
        .save_with(
            None,
            Some(&workspace),
            &recovery,
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    pending.summary.revision += 1;
    pending.summary.state = FlowState::Cancelled;
    store.close().unwrap();
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    assert_eq!(reopened.read(pending.summary.id).unwrap(), pending);
    assert_eq!(reopened.read(existing.summary.id).unwrap(), existing);
}

#[test]
fn failed_save_as_never_changes_the_source_or_publishes_a_partial_target() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mio");
    let destination = directory.path().join("failed.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
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
                Some(&destination),
                Some(&workspace),
                &[added.clone().into(), conflicting.into()],
                &[],
                &[],
                &CancellationToken::new(),
            )
            .is_err()
    );
    assert_eq!(store.path.as_deref(), Some(source.as_path()));
    assert_eq!(store.workspace().unwrap(), before);
    assert_eq!(store.read(existing.summary.id).unwrap(), existing);
    assert!(store.read(added.summary.id).is_err());
    assert!(
        !destination.exists(),
        "only a complete snapshot may be published"
    );
}
