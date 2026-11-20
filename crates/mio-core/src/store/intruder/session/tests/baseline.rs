use super::*;

fn finish_fixture(store: &mut ProjectStore, id: Uuid) -> IntruderRunSummary {
    store.put_intruder_result(id, 0, Err("old A")).unwrap();
    store.put_intruder_result(id, 1, Err("old B")).unwrap();
    store
        .set_intruder_state(id, IntruderRunState::Completed)
        .unwrap()
}

#[test]
fn baseline_completed_session_rearms_state_reserve_and_preserves_a_ticketed_snapshot() {
    for ticketed in [false, true] {
        let (mut store, project, run, spec) = fixture(SESSION_LOGICAL_QUOTA);
        let mut before = finish_fixture(&mut store, run.id);
        if ticketed {
            for retention in [IntruderRetention::Project, IntruderRetention::Session] {
                let mut workspace = store.workspace().unwrap();
                workspace.revision += 1;
                before = store
                    .set_intruder_retention(
                        project,
                        run.id,
                        before.revision,
                        retention,
                        workspace,
                        &CancellationToken::new(),
                    )
                    .unwrap()
                    .run;
            }
        }
        let ticket = store.intruder_sessions.runs[&run.id].disk_ticket;
        assert_eq!(ticket.is_some(), ticketed);
        assert_eq!(store.intruder_sessions.runs[&run.id].state_reserve, 0);
        let old_rows = store.intruder_results(run.id, None, 10).unwrap().rows;
        let mut preparations = 0;
        let (admitted, prepared) = store
            .request_intruder_baseline_with(
                project,
                run.id,
                before.revision,
                &CancellationToken::new(),
                |input| {
                    preparations += 1;
                    Ok(input)
                },
            )
            .unwrap();
        assert_eq!(preparations, 1);
        assert_eq!(prepared, spec);
        assert_eq!(admitted.state, IntruderRunState::Running);
        assert_eq!(
            admitted.baseline,
            Some(IntruderBaseline {
                ordinal: 2,
                completed: false
            })
        );
        assert_eq!(
            store.intruder_sessions.runs[&run.id].state_reserve,
            STATE_RESERVE
        );
        assert_eq!(store.intruder_sessions.runs[&run.id].disk_ticket, ticket);
        assert_eq!(
            store.intruder_results(run.id, None, 10).unwrap().rows,
            old_rows
        );
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
        if ticketed {
            assert!(store.put_intruder_result(run.id, 0, Err("old A")).is_err());
            assert!(
                store
                    .preflight_intruder_result(run.id, 0, &pending())
                    .is_err()
            );
        }
        let request = pending();
        assert!(
            store
                .preflight_intruder_result(run.id, 2, &request)
                .unwrap()
        );
        let pending_run = store.put_intruder_result(run.id, 2, Ok(&request)).unwrap();
        let mut expected = pending_run.clone();
        expected.revision += 1;
        expected.completed += 1;
        expected.baseline.as_mut().unwrap().completed = true;
        let mutations = [IntruderMutation::Result {
            run: expected.clone(),
            ordinal: 2,
            result: Ok(Box::new(complete(&request))),
        }];
        store
            .retry_session_intruder_mutations(&mutations, &CancellationToken::new())
            .unwrap();
        assert_eq!(store.read_intruder_run(run.id).unwrap(), expected);
        store
            .retry_session_intruder_mutations(&mutations, &CancellationToken::new())
            .unwrap();
        assert_eq!(store.read_intruder_run(run.id).unwrap(), expected);
        let finished = store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
        assert_eq!(finished.payload_progress(), Some((2, 2)));
        assert_eq!(store.intruder_sessions.runs[&run.id].disk_ticket, ticket);
        assert_eq!(store.intruder_sessions.reserved_bytes, 0);
        assert_accounting(&store, run.id);
        let mut workspace = store.workspace().unwrap();
        workspace.revision += 1;
        let promoted = store
            .set_intruder_retention(
                project,
                run.id,
                finished.revision,
                IntruderRetention::Project,
                workspace,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(promoted.run.baseline, finished.baseline);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
        assert!(retired_run(&store.connection, run.id).unwrap().is_none());
        assert_eq!(
            &store.intruder_results(run.id, None, 10).unwrap().rows[..2],
            &old_rows
        );
    }
}

#[test]
fn baseline_session_rearm_capacity_and_write_failure_leave_no_mutation_or_reservation() {
    for failure in ["capacity", "write", "cancel"] {
        let (mut store, project, run, _) = fixture(SESSION_LOGICAL_QUOTA);
        let before = finish_fixture(&mut store, run.id);
        let logical = store.intruder_sessions.logical_bytes;
        let token = CancellationToken::new();
        match failure {
            "capacity" => store.intruder_sessions.quota = logical + 8,
            "write" => store.reject_session_intruder_writes(run.id, true),
            "cancel" => token.cancel(),
            _ => unreachable!(),
        }
        assert!(
            store
                .request_intruder_baseline(project, run.id, before.revision, &token)
                .is_err(),
            "{failure}"
        );
        store.reject_session_intruder_writes(run.id, false);
        assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
        assert_eq!(store.intruder_sessions.logical_bytes, logical);
        assert_eq!(store.intruder_sessions.reserved_bytes, 0);
        assert!(
            store.intruder_sessions.runs[&run.id]
                .reactivated_baseline
                .is_none()
        );
        assert_accounting(&store, run.id);
    }
}

#[test]
fn baseline_session_preparation_failure_preserves_pure_and_ticketed_data_and_reserves() {
    for ticketed in [false, true] {
        for failure in ["prepare", "cancel", "capacity"] {
            let (mut store, project, run, spec) = fixture(SESSION_LOGICAL_QUOTA);
            let mut before = finish_fixture(&mut store, run.id);
            if ticketed {
                for retention in [IntruderRetention::Project, IntruderRetention::Session] {
                    let mut workspace = store.workspace().unwrap();
                    workspace.revision += 1;
                    before = store
                        .set_intruder_retention(
                            project,
                            run.id,
                            before.revision,
                            retention,
                            workspace,
                            &CancellationToken::new(),
                        )
                        .unwrap()
                        .run;
                }
            }
            let rows = store.intruder_results(run.id, None, 10).unwrap().rows;
            let workspace = store.workspace().unwrap();
            let logical = store.intruder_sessions.logical_bytes;
            let reserved = store.intruder_sessions.reserved_bytes;
            let dataset = &store.intruder_sessions.runs[&run.id];
            let ticket = dataset.disk_ticket;
            let witness = dataset.reactivated_baseline;
            let state_reserve = dataset.state_reserve;
            let disk_run = find_run(&store.connection, run.id).unwrap();
            let disk_retirement = retired_run(&store.connection, run.id).unwrap();
            assert_eq!(ticket.is_some(), ticketed);
            if failure == "capacity" {
                store.intruder_sessions.quota = logical + 8;
            }
            let cancel = CancellationToken::new();
            let mut preparations = 0;
            let result = store.request_intruder_baseline_with(
                project,
                run.id,
                before.revision,
                &cancel,
                |input| {
                    preparations += 1;
                    assert_eq!(input, spec);
                    if failure == "prepare" {
                        bail!("fixture sender preparation refused")
                    }
                    if failure == "cancel" {
                        cancel.cancel();
                    }
                    Ok(())
                },
            );
            assert!(result.is_err(), "ticketed={ticketed}, failure={failure}");
            assert_eq!(preparations, 1);
            assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
            assert_eq!(store.intruder_results(run.id, None, 10).unwrap().rows, rows);
            assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
            assert_eq!(store.workspace().unwrap(), workspace);
            assert_eq!(store.intruder_sessions.logical_bytes, logical);
            assert_eq!(store.intruder_sessions.reserved_bytes, reserved);
            let dataset = &store.intruder_sessions.runs[&run.id];
            assert_eq!(dataset.disk_ticket, ticket);
            assert_eq!(dataset.reactivated_baseline, witness);
            assert_eq!(dataset.state_reserve, state_reserve);
            assert_eq!(find_run(&store.connection, run.id).unwrap(), disk_run);
            assert_eq!(
                retired_run(&store.connection, run.id).unwrap(),
                disk_retirement
            );
            assert_accounting(&store, run.id);
        }
    }
}
