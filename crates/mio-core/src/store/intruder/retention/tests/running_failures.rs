use super::*;

#[test]
fn running_session_promotion_copy_failure_rolls_back_ram_reservations_and_project() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut store = ProjectStore::create(None).unwrap();
    let (run, [first, second]) = running_promotion_fixture(&mut store, false);
    let before = store.intruder_sessions.runs[&run.id].data.clone();
    let disk_before = raw_dataset(&store.connection, run.id);
    let workspace = workspace_bytes(&store);
    let quota = (
        store.intruder_sessions.logical_bytes,
        store.intruder_sessions.reserved_bytes,
    );
    let attempted = Arc::new(AtomicBool::new(false));
    let copied = attempted.clone();
    store
        .connection
        .authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::Insert {
                table_name: "messages",
            } => {
                copied.store(true, Ordering::SeqCst);
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }));
    let failed = change(&mut store, &run, IntruderRetention::Project);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(failed.is_err());
    assert!(
        attempted.load(Ordering::SeqCst),
        "a valid running snapshot must reach the copy transaction before its injected write fault"
    );
    assert_eq!(raw_dataset(&store.connection, run.id), disk_before);
    assert_eq!(store.intruder_sessions.runs[&run.id].data, before);
    assert_eq!(workspace_bytes(&store), workspace);
    assert_eq!(
        (
            store.intruder_sessions.logical_bytes,
            store.intruder_sessions.reserved_bytes
        ),
        quota
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert_eq!(store.read(first.summary.id).unwrap(), first);
    assert_eq!(store.read(second.summary.id).unwrap(), second);
    assert!(
        store
            .set_intruder_retention(
                project(&store),
                run.id,
                run.revision - 1,
                IntruderRetention::Project,
                proposal(&store),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(
        store
            .set_intruder_retention(
                ProjectKey {
                    generation: 8,
                    ..project(&store)
                },
                run.id,
                run.revision,
                IntruderRetention::Project,
                proposal(&store),
                &CancellationToken::new()
            )
            .is_err()
    );
    let promoted = change(&mut store, &run, IntruderRetention::Project).unwrap();
    assert_eq!(
        (
            promoted.run.retention,
            promoted.run.dispatched,
            promoted.run.completed
        ),
        (IntruderRetention::Project, 2, 1)
    );
    assert!(store.intruder_sessions.runs.is_empty());
    assert_eq!(store.intruder_sessions.reserved_bytes, 0);
}

#[test]
fn running_session_promotion_rejects_stopping_and_inconsistent_pending_snapshots() {
    for fault in 0..4 {
        let mut store = ProjectStore::create(None).unwrap();
        let (mut run, [first, pending]) = running_promotion_fixture(&mut store, false);
        if fault == 0 {
            run = store
                .set_intruder_state(run.id, IntruderRunState::Stopping)
                .unwrap();
        } else {
            let dataset = store.intruder_sessions.runs.get_mut(&run.id).unwrap();
            if fault == 1 {
                dataset.data.run.completed = 0;
                dataset.run.completed = 0;
                run.completed = 0;
            } else {
                let resp_msg = dataset.data.flows[&first.summary.id].messages[&1].clone();
                dataset
                    .data
                    .flows
                    .get_mut(&pending.summary.id)
                    .unwrap()
                    .messages
                    .insert(if fault == 2 { 1 } else { 3 }, resp_msg);
                let revised = dataset.data.logical_bytes().unwrap();
                store.intruder_sessions.logical_bytes += revised - dataset.logical_bytes;
                dataset.logical_bytes = revised;
            }
        }
        let bytes = store.intruder_sessions.runs[&run.id].data.clone();
        let before_workspace = workspace_bytes(&store);
        let before_project = raw_dataset(&store.connection, run.id);
        let before_quota = (
            store.intruder_sessions.logical_bytes,
            store.intruder_sessions.reserved_bytes,
        );
        let error = change(&mut store, &run, IntruderRetention::Project)
            .unwrap_err()
            .to_string();
        match fault {
            0 => assert!(error.contains("状态不允许"), "{error}"),
            1 => assert!(error.contains("已完成结果数量"), "{error}"),
            _ => assert!(error.contains("在途结果不能包含响应"), "{error}"),
        }
        assert_eq!(store.intruder_sessions.runs[&run.id].data, bytes);
        assert_eq!(workspace_bytes(&store), before_workspace);
        assert_eq!(raw_dataset(&store.connection, run.id), before_project);
        assert_eq!(
            (
                store.intruder_sessions.logical_bytes,
                store.intruder_sessions.reserved_bytes
            ),
            before_quota
        );
    }
}

#[test]
fn task_title_session_rename_preserves_ticket_readonly_quota_and_explicit_promotion() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mio");
    let copy = directory.path().join("copy.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let (original, flow) = finished(&mut store, Uuid::new_v4(), 1);
    let named = store
        .rename_intruder_run(
            project(&store),
            original.id,
            0,
            "before",
            &CancellationToken::new(),
        )
        .unwrap();
    let kept = change(&mut store, &named, IntruderRetention::Session)
        .unwrap()
        .run;
    let ticket = retired_run(&store.connection, kept.id).unwrap();
    let bytes = store.intruder_sessions.logical_bytes;
    let renamed = store
        .rename_intruder_run(
            project(&store),
            kept.id,
            1,
            "内存名称🙂",
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(renamed.revision, kept.revision);
    assert_eq!(renamed.title_revision, 2);
    assert_eq!(retired_run(&store.connection, kept.id).unwrap(), ticket);
    assert_eq!(
        store.intruder_sessions.logical_bytes,
        bytes - 6 + "内存名称🙂".len() as u64
    );
    assert_eq!(store.read_intruder_run(kept.id).unwrap(), renamed);
    assert_eq!(
        store.intruder_runs(None, 10).unwrap().rows.as_slice(),
        std::slice::from_ref(&renamed)
    );
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    for path in [&source, &copy] {
        let disk = ProjectStore::open(path).unwrap();
        assert!(disk.find_intruder_run(kept.id).unwrap().is_none());
        assert_eq!(retired_run(&disk.connection, kept.id).unwrap(), ticket);
        disk.close().unwrap();
    }
    let promoted = change(&mut store, &renamed, IntruderRetention::Project)
        .unwrap()
        .run;
    assert_eq!(
        (promoted.title.as_str(), promoted.title_revision),
        ("内存名称🙂", 2)
    );
    assert_eq!(promoted.revision, kept.revision + 1);
    assert_eq!(store.intruder_sessions.logical_bytes, 0);
    store.save(None).unwrap();
    store.close().unwrap();
    let mut reopened = ProjectStore::open(&copy).unwrap();
    assert_eq!(reopened.read_intruder_run(kept.id).unwrap(), promoted);
    assert_eq!(reopened.read(flow.summary.id).unwrap(), flow);
    let discarded = reopened
        .discard_intruder_run(
            kept.id,
            promoted.revision,
            proposal(&reopened),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(discarded.revision, promoted.revision + 1);
    assert!(reopened.find_intruder_run(kept.id).unwrap().is_none());
    reopened.close().unwrap();
}

#[test]
fn task_title_session_failure_never_changes_name_ticket_quota_or_query_only() {
    let mut store = ProjectStore::create(None).unwrap();
    let (original, _) = finished(&mut store, Uuid::new_v4(), 1);
    let kept = change(&mut store, &original, IntruderRetention::Session)
        .unwrap()
        .run;
    let before = store.intruder_sessions.runs[&kept.id].data.clone();
    let ticket = retired_run(&store.connection, kept.id).unwrap();
    let bytes = store.intruder_sessions.logical_bytes;
    store.intruder_sessions.quota = bytes;
    assert!(
        store
            .rename_intruder_run(
                project(&store),
                kept.id,
                0,
                "over quota",
                &CancellationToken::new()
            )
            .is_err()
    );
    store.intruder_sessions.quota = SESSION_LOGICAL_QUOTA;
    store.set_session_intruder_write_hook(
        kept.id,
        Some(std::sync::Arc::new(|action| action != "rename")),
    );
    assert!(
        store
            .rename_intruder_run(
                project(&store),
                kept.id,
                0,
                "denied",
                &CancellationToken::new()
            )
            .is_err()
    );
    store.set_session_intruder_write_hook(kept.id, None);
    assert!(
        store
            .rename_intruder_run(
                project(&store),
                kept.id,
                1,
                "stale",
                &CancellationToken::new()
            )
            .is_err()
    );
    let foreign = ProjectKey {
        generation: 2,
        ..project(&store)
    };
    assert!(
        store
            .rename_intruder_run(
                foreign,
                kept.id,
                0,
                "foreign generation",
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(store.intruder_sessions.runs[&kept.id].data, before);
    assert_eq!(store.intruder_sessions.logical_bytes, bytes);
    assert_eq!(retired_run(&store.connection, kept.id).unwrap(), ticket);
    assert_eq!(store.read_intruder_run(kept.id).unwrap(), kept);
    store
        .discard_intruder_run(
            kept.id,
            kept.revision,
            proposal(&store),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(store.intruder_sessions.logical_bytes, 0);
}
