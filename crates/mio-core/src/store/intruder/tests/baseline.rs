use super::*;

mod admission;

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

fn baseline_run(store: &mut ProjectStore, initial: bool) -> (IntruderRunSummary, IntruderSpec) {
    let mut input = spec(2);
    input.baseline_request = initial;
    let tab = bind(store, &input);
    let run = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 50)
        .unwrap();
    (run, input)
}

#[test]
fn baseline_initial_metadata_and_lightweight_progress_follow_its_own_terminal_row() {
    let mut store = store();
    let (run, input) = baseline_run(&mut store, true);
    assert_eq!(run.total, 3);
    assert_eq!(
        run.baseline,
        Some(IntruderBaseline {
            ordinal: 0,
            completed: false
        })
    );
    let base = pending();
    store.put_intruder_result(run.id, 0, Ok(&base)).unwrap();
    store
        .put_intruder_result(run.id, 1, Err("payload A rejected"))
        .unwrap();
    let payloads_done = store
        .put_intruder_result(run.id, 2, Err("payload B rejected"))
        .unwrap();
    assert_eq!(payloads_done.payload_progress(), Some((2, 2)));
    assert!(!payloads_done.baseline.unwrap().completed);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "intruder_runs",
                column_name: "spec_meta" | "template",
                ..
            }
            | AuthAction::Read {
                table_name: "intruder_payloads",
                column_name: "value",
                ..
            }
            | AuthAction::Read {
                table_name: "messages",
                column_name: "head" | "body",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    assert_eq!(store.read_intruder_run(run.id).unwrap(), payloads_done);
    let page = store.intruder_results(run.id, None, 1).unwrap();
    assert_eq!(page.run, payloads_done);
    assert_eq!(page.next_after, Some(0));
    assert_eq!(
        store
            .intruder_results(run.id, page.next_after, 1)
            .unwrap()
            .rows[0]
            .ordinal,
        1
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let finished = store
        .put_intruder_result(run.id, 0, Ok(&complete(&base)))
        .unwrap();
    assert_eq!(
        (finished.completed, finished.payload_progress()),
        (3, Some((2, 2)))
    );
    assert!(finished.baseline.unwrap().completed);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), finished);
    assert_eq!(
        store
            .put_intruder_result(run.id, 0, Ok(&complete(&base)))
            .unwrap(),
        finished
    );
}

#[test]
fn baseline_explicit_slot_preserves_existing_results_and_frozen_configuration() {
    for state in [
        IntruderRunState::Running,
        IntruderRunState::Paused,
        IntruderRunState::Completed,
    ] {
        let mut store = store();
        let (run, input) = baseline_run(&mut store, false);
        store.put_intruder_result(run.id, 0, Err("old A")).unwrap();
        if state == IntruderRunState::Completed {
            store.put_intruder_result(run.id, 1, Err("old B")).unwrap();
        }
        let before = store.set_intruder_state(run.id, state.clone()).unwrap();
        let old_rows = store.intruder_results(run.id, None, 10).unwrap().rows;
        let admitted = store
            .request_intruder_baseline(
                project(&store),
                run.id,
                before.revision,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(
            (
                admitted.id,
                admitted.dispatched,
                admitted.completed,
                admitted.total
            ),
            (before.id, before.dispatched, before.completed, 3)
        );
        assert_eq!(
            admitted.baseline,
            Some(IntruderBaseline {
                ordinal: before.dispatched,
                completed: false
            })
        );
        assert_eq!(
            admitted.state,
            if state == IntruderRunState::Paused {
                IntruderRunState::Paused
            } else {
                IntruderRunState::Running
            }
        );
        assert_eq!(
            store.intruder_results(run.id, None, 10).unwrap().rows,
            old_rows
        );
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
        assert!(
            store
                .request_intruder_baseline(
                    project(&store),
                    run.id,
                    admitted.revision,
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(store.read_intruder_run(run.id).unwrap(), admitted);
        if state == IntruderRunState::Paused {
            store
                .set_intruder_state(run.id, IntruderRunState::Running)
                .unwrap();
        }
        let after = store
            .put_intruder_result(run.id, before.dispatched, Err("baseline rejected"))
            .unwrap();
        assert!(after.baseline.unwrap().completed);
        assert_eq!(after.request_number(before.dispatched), Some(0));
        if before.dispatched == 1 {
            assert_eq!(after.request_number(2), Some(2));
            store
                .put_intruder_result(run.id, 2, Err("remaining B"))
                .unwrap();
        }
        assert_eq!(
            store
                .set_intruder_state(run.id, IntruderRunState::Completed)
                .unwrap()
                .payload_progress(),
            Some((2, 2))
        );
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    }
}

#[test]
fn baseline_admission_conflicts_cancellation_and_write_failure_preserve_old_run() {
    for failure in [
        "revision",
        "cancel",
        "write",
        "during_write",
        "stopping",
        "stopped",
        "failed",
        "missing",
    ] {
        let mut store = store();
        let (run, _) = baseline_run(&mut store, false);
        let token = CancellationToken::new();
        match failure {
            "cancel" => token.cancel(),
            "stopping" => {
                store
                    .set_intruder_state(run.id, IntruderRunState::Stopping)
                    .unwrap();
            }
            "stopped" => {
                store
                    .set_intruder_state(run.id, IntruderRunState::Stopped)
                    .unwrap();
            }
            "failed" => {
                store
                    .set_intruder_state(run.id, IntruderRunState::Failed("old failure".into()))
                    .unwrap();
            }
            "missing" => {
                store
                    .connection
                    .execute("UPDATE intruder_runs SET dispatched=1", [])
                    .unwrap();
            }
            _ => {}
        }
        let before = store.read_intruder_run(run.id).unwrap();
        if failure == "write" {
            store
                .connection
                .authorizer(Some(|context: AuthContext<'_>| match context.action {
                    AuthAction::Update { .. } => Authorization::Deny,
                    _ => Authorization::Allow,
                }));
        }
        if failure == "during_write" {
            let cancel = token.clone();
            store
                .connection
                .authorizer(Some(move |context: AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Update {
                            table_name: "intruder_runs",
                            ..
                        }
                    ) {
                        cancel.cancel();
                    }
                    Authorization::Allow
                }));
        }
        let expected = before.revision + u64::from(failure == "revision");
        assert!(
            store
                .request_intruder_baseline(project(&store), run.id, expected, &token)
                .is_err(),
            "{failure}"
        );
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
        let directory = tempfile::tempdir().unwrap();
        store
            .save(Some(&directory.path().join("unchanged.mio")))
            .unwrap();
        assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
    }
}

#[test]
fn baseline_legacy_metadata_and_actual_slot_are_checked_without_rewriting_bytes() {
    let mut store = store();
    let (run, input) = baseline_run(&mut store, false);
    let meta: Vec<u8> = store
        .connection
        .query_row(
            "SELECT spec_meta FROM intruder_runs WHERE id=?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&meta).unwrap();
    value.as_object_mut().unwrap().remove("baseline_request");
    let legacy = serde_json::to_vec(&value).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET spec_meta=?1 WHERE id=?2",
            params![legacy, run.id.to_string()],
        )
        .unwrap();
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    let after: Vec<u8> = store
        .connection
        .query_row(
            "SELECT spec_meta FROM intruder_runs WHERE id=?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after, legacy);
    value["baseline_request"] = serde_json::json!(true);
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET spec_meta=?1 WHERE id=?2",
            params![serde_json::to_vec(&value).unwrap(), run.id.to_string()],
        )
        .unwrap();
    assert!(store.read_intruder_spec(run.id).is_err());
    value["baseline_request"] = serde_json::json!("false");
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET spec_meta=?1 WHERE id=?2",
            params![serde_json::to_vec(&value).unwrap(), run.id.to_string()],
        )
        .unwrap();
    assert!(store.read_intruder_spec(run.id).is_err());
}

#[test]
fn baseline_pending_recovery_reopens_without_an_extra_request_or_second_interruption() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let (run, input) = baseline_run(&mut store, false);
    store.put_intruder_result(run.id, 0, Err("A")).unwrap();
    store.put_intruder_result(run.id, 1, Err("B")).unwrap();
    let finished = store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();
    let admitted = store
        .request_intruder_baseline(
            project(&store),
            run.id,
            finished.revision,
            &CancellationToken::new(),
        )
        .unwrap();
    let request = pending();
    let dispatched = store.put_intruder_result(run.id, 2, Ok(&request)).unwrap();
    assert_eq!(dispatched.baseline, admitted.baseline);
    store.close().unwrap();
    let mut reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.read_intruder_run(run.id).unwrap(), dispatched);
    assert_eq!(reopened.interrupt_intruder_runs().unwrap(), 1);
    let interrupted = reopened.read_intruder_run(run.id).unwrap();
    assert_eq!(interrupted.state, IntruderRunState::Interrupted);
    assert_eq!(
        interrupted.baseline,
        Some(IntruderBaseline {
            ordinal: 2,
            completed: true
        })
    );
    assert_eq!((interrupted.completed, interrupted.dispatched), (3, 3));
    assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), input);
    assert_eq!(reopened.interrupt_intruder_runs().unwrap(), 0);
    assert_eq!(reopened.read_intruder_run(run.id).unwrap(), interrupted);
}
