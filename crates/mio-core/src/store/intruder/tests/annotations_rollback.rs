use super::*;

#[test]
fn intruder_result_notes_and_highlights_use_independent_cas_revisions_and_preserve_original_bytes()
{
    let mut store = store();
    let run = run(&mut store, 2);
    let first = pending();
    store.put_intruder_result(run.id, 0, Ok(&first)).unwrap();
    store
        .put_intruder_result(run.id, 1, Err("invalid generated request"))
        .unwrap();
    let project = ProjectKey {
        id: store.id,
        generation: 1,
    };
    let empty = store
        .set_intruder_result_notes(project, run.id, 0, 0, b"", &CancellationToken::new())
        .unwrap();
    assert_eq!(empty, (1, Vec::<u8>::new()));
    let binary = store
        .set_intruder_result_notes(
            project,
            run.id,
            0,
            1,
            &[0, 255, b'A'],
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(binary, (2, vec![0, 255, b'A']));
    assert!(
        store
            .set_intruder_result_notes(project, run.id, 0, 1, b"stale", &CancellationToken::new())
            .is_err()
    );
    assert!(
        store
            .set_intruder_result_notes(
                project,
                run.id,
                0,
                2,
                &[0u8; 64 * 1024 + 1],
                &CancellationToken::new(),
            )
            .is_err()
    );
    assert!(
        store
            .set_intruder_result_notes(
                project,
                run.id,
                1,
                0,
                b"rejected",
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(
        store
            .set_intruder_result_notes(project, run.id, 2, 0, b"missing", &CancellationToken::new())
            .is_err()
    );
    let canceled = CancellationToken::new();
    canceled.cancel();
    assert!(
        store
            .set_intruder_result_notes(project, run.id, 0, 2, b"cancel", &canceled)
            .is_err()
    );
    let rejected_highlight = store
        .set_intruder_result_highlight(
            project,
            run.id,
            &[(0, 0), (1, 0)],
            Some(3),
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert!(rejected_highlight.to_string().contains("未发送"));
    let highlights = store
        .set_intruder_result_highlight(
            project,
            run.id,
            &[(0, 0)],
            Some(3),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(highlights, [(0, 1)]);
    assert!(
        store
            .set_intruder_result_highlight(
                project,
                run.id,
                &[(0, 0)],
                Some(10),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(
        store
            .set_intruder_result_highlight(project, run.id, &[], Some(1), &CancellationToken::new())
            .is_err()
    );
    let cleared = store
        .set_intruder_result_highlight(project, run.id, &[(0, 1)], None, &CancellationToken::new())
        .unwrap();
    assert_eq!(cleared, [(0, 2)]);
    let stale = store
        .set_intruder_result_highlight(
            project,
            run.id,
            &[(0, 0), (1, 0)],
            Some(4),
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert!(stale.to_string().contains("冲突"));
    let connection = store.test_intruder_connection(run.id).unwrap();
    let notes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT notes FROM intruder_results WHERE run=?1 AND ordinal=0",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(notes, Some(vec![0, 255, b'A']));
    let (highlight, revision): (Option<u8>, u64) = connection
        .query_row(
            "SELECT highlight,highlight_revision FROM intruder_results WHERE run=?1 AND ordinal=0",
            [run.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((highlight, revision), (None, 2));
    store
        .put_intruder_result(run.id, 0, Ok(&complete(&first)))
        .unwrap();
    let connection = store.test_intruder_connection(run.id).unwrap();
    let (notes_after, notes_revision_after, highlight_after, highlight_revision_after): (
        Option<Vec<u8>>,
        u64,
        Option<u8>,
        u64,
    ) = connection
        .query_row(
            "SELECT notes,notes_revision,highlight,highlight_revision FROM intruder_results WHERE run=?1 AND ordinal=0",
            [run.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(notes_after, Some(vec![0, 255, b'A']));
    assert_eq!(notes_revision_after, 2);
    assert_eq!((highlight_after, highlight_revision_after), (None, 2));
}

#[test]
fn stopping_refuses_new_work_and_terminal_states_require_joined_results() {
    let mut store = store();
    let run = run(&mut store, 2);
    let request = pending();
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    for state in [
        IntruderRunState::Stopped,
        IntruderRunState::Completed,
        IntruderRunState::Failed("failure".into()),
    ] {
        assert!(store.set_intruder_state(run.id, state).is_err());
    }
    let stopping = store
        .set_intruder_state(run.id, IntruderRunState::Stopping)
        .unwrap();
    assert_eq!(
        store
            .set_intruder_state(run.id, IntruderRunState::Stopping)
            .unwrap(),
        stopping
    );
    assert!(
        store
            .put_intruder_result(run.id, 1, Err("not dispatched"))
            .is_err()
    );
    assert!(
        store
            .set_intruder_state(run.id, IntruderRunState::Running)
            .is_err()
    );
    let mut cancelled = request.clone();
    cancelled.summary.revision += 1;
    cancelled.summary.state = FlowState::Cancelled;
    store
        .put_intruder_result(run.id, 0, Ok(&cancelled))
        .unwrap();
    let stopped = store
        .set_intruder_state(run.id, IntruderRunState::Stopped)
        .unwrap();
    assert_eq!(
        (stopped.dispatched, stopped.completed, stopped.total),
        (1, 1, 2)
    );
    assert_eq!(
        store
            .put_intruder_result(run.id, 0, Ok(&cancelled))
            .unwrap(),
        stopped
    );
    assert!(
        store
            .set_intruder_state(run.id, IntruderRunState::Running)
            .is_err()
    );
    assert!(
        store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .is_err()
    );
    assert!(
        store
            .put_intruder_result(run.id, 1, Ok(&pending()))
            .is_err()
    );
}

pub(in crate::store::intruder) fn deny_run_update(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Update {
            table_name: "intruder_runs",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn deny_result_insert(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Insert {
            table_name: "intruder_results",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[test]
fn write_failures_roll_back_flow_messages_links_and_run_counters_together() {
    let mut store = store();
    let run = run(&mut store, 2);
    let request = pending();
    store.connection.authorizer(Some(deny_result_insert));
    assert!(store.put_intruder_result(run.id, 0, Ok(&request)).is_err());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(store.read_optional(request.summary.id).unwrap().is_none());
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    let admitted = store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    store.connection.authorizer(Some(deny_run_update));
    assert!(
        store
            .put_intruder_result(run.id, 0, Ok(&complete(&request)))
            .is_err()
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(store.read(request.summary.id).unwrap(), request);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), admitted);
    assert_eq!(
        store.intruder_results(run.id, None, 1).unwrap().rows[0].outcome,
        IntruderResultOutcome::Flow(request.summary.clone())
    );
    assert_eq!(
        store
            .put_intruder_result(run.id, 0, Ok(&complete(&request)))
            .unwrap()
            .completed,
        1
    );
}

#[test]
fn transaction_internal_writers_share_commit_and_rollback_with_the_caller() {
    let mut store = store();
    let run = run(&mut store, 2);
    let request = pending();
    let finished = complete(&request);
    {
        let transaction = store.connection.transaction().unwrap();
        write_result(&transaction, run.id, 0, Ok(&request)).unwrap();
        write_result(&transaction, run.id, 1, Err("invalid request")).unwrap();
        write_result(&transaction, run.id, 0, Ok(&finished)).unwrap();
        let done = write_state(&transaction, run.id, IntruderRunState::Completed).unwrap();
        assert_eq!((done.dispatched, done.completed), (2, 2));
        assert_eq!(read_run(&transaction, run.id).unwrap(), done);
        assert_eq!(
            write_result(&transaction, run.id, 0, Ok(&finished)).unwrap(),
            done
        );
        assert_eq!(
            write_result(&transaction, run.id, 1, Err("invalid request")).unwrap(),
            done
        );
        assert_eq!(
            write_state(&transaction, run.id, IntruderRunState::Completed).unwrap(),
            done
        );
        transaction.rollback().unwrap();
    }
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert!(store.read_optional(request.summary.id).unwrap().is_none());
    assert!(
        store
            .intruder_results(run.id, None, 2)
            .unwrap()
            .rows
            .is_empty()
    );

    let transaction = store.connection.transaction().unwrap();
    write_result(&transaction, run.id, 0, Ok(&finished)).unwrap();
    write_result(&transaction, run.id, 1, Err("invalid request")).unwrap();
    let done = write_state(&transaction, run.id, IntruderRunState::Completed).unwrap();
    transaction.commit().unwrap();
    assert_eq!(store.read_intruder_run(run.id).unwrap(), done);
    assert_eq!(store.read(request.summary.id).unwrap(), finished);
    assert_eq!(
        store.intruder_results(run.id, None, 2).unwrap().rows.len(),
        2
    );
}
