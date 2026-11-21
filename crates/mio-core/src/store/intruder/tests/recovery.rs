use super::*;

#[test]
fn reopening_only_marks_interrupted_after_explicit_atomic_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("intruder.mio");
    let mut store = store();
    let run = run(&mut store, 3);
    let in_flight = pending();
    let observed = complete(&pending());
    store
        .put_intruder_result(run.id, 0, Ok(&in_flight))
        .unwrap();
    store.put_intruder_result(run.id, 1, Ok(&observed)).unwrap();
    let mut unrelated = pending();
    unrelated.summary.source = TrafficSource::Repeater;
    store.put(&unrelated).unwrap();
    store.save(Some(&path)).unwrap();
    store.close().unwrap();
    let mut reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(
        reopened.read_intruder_run(run.id).unwrap().state,
        IntruderRunState::Running
    );
    assert_eq!(reopened.read(in_flight.summary.id).unwrap(), in_flight);
    reopened.connection.authorizer(Some(deny_large_data_reads));
    assert_eq!(reopened.interrupt_intruder_runs().unwrap(), 1);
    reopened
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let restored = reopened.read_intruder_run(run.id).unwrap();
    assert_eq!(
        (
            restored.state,
            restored.total,
            restored.dispatched,
            restored.completed
        ),
        (IntruderRunState::Interrupted, 3, 2, 2)
    );
    let failed = reopened.read(in_flight.summary.id).unwrap();
    assert!(
        matches!(failed.summary.state, FlowState::Failed(ref message) if message.contains("结果未知") && message.contains("未自动重发"))
    );
    assert_eq!(failed.request, in_flight.request);
    assert_eq!(reopened.read(observed.summary.id).unwrap(), observed);
    assert_eq!(reopened.read(unrelated.summary.id).unwrap(), unrelated);
    assert_eq!(reopened.interrupt_intruder_runs().unwrap(), 0);
    assert!(
        reopened
            .put_intruder_result(run.id, 0, Ok(&complete(&in_flight)))
            .is_err()
    );
    assert!(
        reopened
            .set_intruder_state(run.id, IntruderRunState::Running)
            .is_err()
    );
}

#[test]
fn failed_recovery_keeps_all_previous_pending_records_and_run_states() {
    let mut store = store();
    let run = run(&mut store, 1);
    let request = pending();
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET completed=1 WHERE id=?1",
            [run.id.to_string()],
        )
        .unwrap();
    let before = store.read_intruder_run(run.id).unwrap();
    assert!(store.interrupt_intruder_runs().is_err());
    assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
    assert_eq!(store.read(request.summary.id).unwrap(), request);
}

#[test]
fn a_million_planned_requests_do_not_create_a_million_result_rows() {
    let mut store = store();
    let mut input = spec(0);
    input.attack_type = AttackType::ClusterBomb;
    input.positions = vec![Position { start: 0, end: 0 }; 6];
    input.payload_sets = vec![(0..10).map(|value| vec![value]).collect(); 6];
    let tab = bind(&mut store, &input);
    let run = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
        .unwrap();
    assert_eq!(run.total, MAX_REQUESTS);
    let results: usize = store
        .connection
        .query_row("SELECT count(*) FROM intruder_results", [], |row| {
            row.get(0)
        })
        .unwrap();
    let payloads: usize = store
        .connection
        .query_row("SELECT count(*) FROM intruder_payloads", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!((results, payloads), (0, 60));
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    assert!(
        store
            .intruder_results(run.id, None, 500)
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn inconsistent_terminal_metadata_cannot_commit_a_result() {
    let mut store = store();
    let run = run(&mut store, 1);
    let request = pending();
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET state=3,completed=1 WHERE id=?1",
            [run.id.to_string()],
        )
        .unwrap();
    assert!(store.intruder_results(run.id, None, 1).is_err());
    assert!(store.put_intruder_result(run.id, 0, Ok(&request)).is_err());
    assert_eq!(store.read(request.summary.id).unwrap(), request);
}

#[test]
fn terminal_states_reject_pending_results_even_when_counters_claim_completion() {
    for (state, stored_state) in [
        (IntruderRunState::Stopped, 0),
        (IntruderRunState::Completed, 0),
        (IntruderRunState::Failed("failure".into()), 0),
        (IntruderRunState::Stopped, 2),
    ] {
        let mut store = store();
        let run = run(&mut store, 1);
        let request = pending();
        store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
        store
            .connection
            .execute(
                "UPDATE intruder_runs SET completed=1,state=?1 WHERE id=?2",
                params![stored_state, run.id.to_string()],
            )
            .unwrap();
        let before = store.read_intruder_run(run.id).unwrap();
        assert!(
            store.set_intruder_state(run.id, state).is_err(),
            "a completed counter cannot turn a still-pending result into a terminal run"
        );
        assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
        assert_eq!(store.read(request.summary.id).unwrap(), request);
    }
}

#[test]
fn terminal_states_reject_missing_results_even_when_counters_claim_completion() {
    let mut store = store();
    let run = run(&mut store, 1);
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET dispatched=1,completed=1 WHERE id=?1",
            [run.id.to_string()],
        )
        .unwrap();
    let before = store.read_intruder_run(run.id).unwrap();
    assert!(
        store
            .set_intruder_state(run.id, IntruderRunState::Stopped)
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
}

#[test]
fn recovery_batches_replay_final_results_and_stop_states_idempotently() {
    let mut store = store();
    let run = run(&mut store, 3);
    let request = pending();
    let mutations = recovery_mutations(&run, &request);
    store
        .apply_pending(None, &[], &[], &mutations, &CancellationToken::new())
        .unwrap();
    let stopped = store.read_intruder_run(run.id).unwrap();
    assert_eq!(stopped.state, IntruderRunState::Stopped);
    assert_eq!(
        (stopped.total, stopped.dispatched, stopped.completed),
        (3, 2, 2)
    );
    assert_eq!(store.read(request.summary.id).unwrap(), complete(&request));
    let page = store.intruder_results(run.id, None, 2).unwrap();
    assert_eq!(
        page.rows[1].outcome,
        IntruderResultOutcome::Rejected("invalid request".into())
    );
    store
        .apply_pending(None, &[], &[], &mutations, &CancellationToken::new())
        .unwrap();
    assert_eq!(store.read_intruder_run(run.id).unwrap(), stopped);
    assert_eq!(store.read(request.summary.id).unwrap(), complete(&request));
    assert_eq!(store.intruder_results(run.id, None, 2).unwrap(), page);
}

#[test]
fn recovery_rejects_conflicting_final_bodies_errors_and_old_revisions() {
    let mut store = store();
    let run = run(&mut store, 3);
    let request = pending();
    let mutations = recovery_mutations(&run, &request);
    store
        .apply_pending(None, &[], &[], &mutations, &CancellationToken::new())
        .unwrap();
    let stopped = store.read_intruder_run(run.id).unwrap();

    let mut changed_body = recovery_mutations(&run, &request);
    let IntruderMutation::Result {
        result: Ok(record), ..
    } = &mut changed_body[1]
    else {
        unreachable!("terminal result fixture")
    };
    record.response.as_mut().unwrap().body[0] ^= 1;
    assert!(
        store
            .apply_pending(None, &[], &[], &changed_body, &CancellationToken::new())
            .is_err()
    );

    let mut changed_error = recovery_mutations(&run, &request);
    let IntruderMutation::Result { result, .. } = &mut changed_error[2] else {
        unreachable!("rejected result fixture")
    };
    *result = Err("different failure".into());
    assert!(
        store
            .apply_pending(None, &[], &[], &changed_error, &CancellationToken::new())
            .is_err()
    );

    let mut old_revision = recovery_mutations(&run, &request);
    let IntruderMutation::State { run: expected } = old_revision.last_mut().unwrap() else {
        unreachable!("stopped state fixture")
    };
    expected.revision -= 1;
    assert!(
        store
            .apply_pending(None, &[], &[], &old_revision, &CancellationToken::new())
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), stopped);
    assert_eq!(store.read(request.summary.id).unwrap(), complete(&request));
    assert_eq!(
        store.intruder_results(run.id, None, 2).unwrap().rows[1].outcome,
        IntruderResultOutcome::Rejected("invalid request".into())
    );
}

#[test]
fn recovery_rolls_back_every_run_when_a_later_step_summary_conflicts() {
    let mut store = store();
    let first = run(&mut store, 3);
    let second = run(&mut store, 3);
    let first_request = pending();
    let second_request = pending();
    for change_revision in [false, true] {
        let mut mutations = recovery_mutations(&first, &first_request);
        let second_start = mutations.len();
        mutations.extend(recovery_mutations(&second, &second_request));
        let IntruderMutation::Result { run: claimed, .. } = &mut mutations[second_start] else {
            unreachable!("pending result fixture")
        };
        if change_revision {
            claimed.revision += 1;
        } else {
            claimed.tab_id = Uuid::new_v4();
        }
        assert!(
            store
                .apply_pending(None, &[], &[], &mutations, &CancellationToken::new())
                .is_err()
        );
        assert_eq!(store.read_intruder_run(first.id).unwrap(), first);
        assert_eq!(store.read_intruder_run(second.id).unwrap(), second);
        assert!(
            store
                .read_optional(first_request.summary.id)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .read_optional(second_request.summary.id)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .intruder_results(first.id, None, 2)
                .unwrap()
                .rows
                .is_empty()
        );
        assert!(
            store
                .intruder_results(second.id, None, 2)
                .unwrap()
                .rows
                .is_empty()
        );
    }
}
