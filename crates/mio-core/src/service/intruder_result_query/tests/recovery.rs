use super::*;

fn pending_results(state: &mut State, id: Uuid, records: &[FlowRecord]) -> IntruderRunSummary {
    let mut run = state.store.read_intruder_run(id).unwrap();
    for (ordinal, record) in records.iter().enumerate() {
        run.revision += 1;
        run.completed += 1;
        state.intruder.recovery.push(IntruderMutation::Result {
            run: run.clone(),
            ordinal: ordinal as u64,
            result: Ok(Box::new(record.clone())),
        });
        if ordinal == 0 {
            state.intruder.recovery.push(IntruderMutation::Result {
                run: run.clone(),
                ordinal: ordinal as u64,
                result: Ok(Box::new(record.clone())),
            });
        }
    }
    run
}

#[tokio::test]
async fn latest_recovery_results_are_merged_before_global_sort_filter_and_selection() {
    for retention in [IntruderRetention::Project, IntruderRetention::Session] {
        let (mut state, mut events, id, records) = pending_fixture(retention);
        let disk = state.store.intruder_results(id, None, 2).unwrap();
        let current = pending_results(&mut state, id, &records);
        let mut sorted = query(state.project, id, 1);
        sorted.limit = 1;
        sorted.sort = IntruderResultSort {
            column: IntruderResultSortColumn::Status,
            ascending: true,
        };
        sorted.selected_ordinal = Some(1);
        let page = request(&mut state, &mut events, sorted, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(page.run, current);
        assert!(!page.persisted);
        assert_eq!(
            (page.total, page.next_offset, page.selected_rank),
            (2, Some(1), Some(1))
        );
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].ordinal, 0);
        assert_eq!(
            page.rows[0].result.as_ref().unwrap().outcome,
            IntruderResultOutcome::Flow(records[0].summary.clone())
        );
        let selected = page.selected_row.unwrap();
        assert_eq!(selected.ordinal, 1);
        assert_eq!(
            selected.result.unwrap().outcome,
            IntruderResultOutcome::Flow(records[1].summary.clone())
        );
        assert_eq!(selected.payloads[0].prefix, [0xff, b'B']);

        let mut filtered = query(state.project, id, 2);
        filtered.filter.status_classes = [false, false, true, false];
        filtered.selected_ordinal = Some(0);
        let page = request(&mut state, &mut events, filtered, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].ordinal, 1);
        assert_eq!(page.selected_rank, None);
        assert_eq!(page.selected_row, None);
        assert_eq!(state.intruder.recovery.len(), 3);
        assert_eq!(state.store.intruder_results(id, None, 2).unwrap(), disk);
        for (ordinal, record) in records.iter().enumerate() {
            let IntruderMutation::Result {
                result: Ok(pending),
                ..
            } = &state.intruder.recovery[if ordinal == 0 { 0 } else { 2 }]
            else {
                panic!("queries must retain complete pending results")
            };
            assert_eq!(pending.as_ref(), record);
            assert!(
                state
                    .store
                    .read(record.summary.id)
                    .unwrap()
                    .response
                    .is_none()
            );
        }
    }
}

#[tokio::test]
async fn pending_run_state_without_result_recovery_is_explicitly_unsaved() {
    let (mut state, mut events, id, _) = fixture(IntruderRetention::Project);
    let disk = state.store.intruder_results(id, None, 2).unwrap();
    assert!(disk.persisted);
    let mut completed = disk.run.clone();
    completed.revision += 1;
    completed.state = IntruderRunState::Completed;
    state.intruder.recovery.push(IntruderMutation::State {
        run: completed.clone(),
    });
    let view = query(state.project, id, 1);
    let page = request(&mut state, &mut events, view, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(page.run, completed);
    assert!(!page.persisted);
    assert_eq!(page.total, 2);
    assert_eq!(state.intruder.recovery.len(), 1);
    assert_eq!(state.store.intruder_results(id, None, 2).unwrap(), disk);
}

#[tokio::test]
async fn failed_payload_read_preserves_full_recovery_for_the_next_query() {
    let (mut state, mut events, id, records) = pending_fixture(IntruderRetention::Project);
    let disk = state.store.intruder_results(id, None, 2).unwrap();
    let current = pending_results(&mut state, id, &records);
    state
        .store
        .test_intruder_connection(id)
        .unwrap()
        .authorizer(Some(
            |context: rusqlite::hooks::AuthContext<'_>| match context.action {
                rusqlite::hooks::AuthAction::Read {
                    table_name: "intruder_payloads",
                    column_name: "value",
                    ..
                } => rusqlite::hooks::Authorization::Deny,
                _ => rusqlite::hooks::Authorization::Allow,
            },
        ));
    let view = query(state.project, id, 1);
    let failed = request(&mut state, &mut events, view, CancellationToken::new()).await;
    state
        .store
        .test_intruder_connection(id)
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    assert!(failed.is_err());
    assert_eq!(state.intruder.recovery.len(), 3);
    assert_eq!(state.pending_intruder_run(id), Some(current.clone()));
    assert_eq!(state.store.intruder_results(id, None, 2).unwrap(), disk);
    let view = query(state.project, id, 2);
    let page = request(&mut state, &mut events, view, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(page.run, current);
    assert_eq!(page.total, 2);
    assert!(!page.persisted);
    for (index, record) in records.iter().enumerate() {
        assert_eq!(
            page.rows[index].result.as_ref().unwrap().outcome,
            IntruderResultOutcome::Flow(record.summary.clone())
        );
    }
    assert_eq!(state.intruder.recovery.len(), 3);
    assert_eq!(state.store.intruder_results(id, None, 2).unwrap(), disk);
}

#[tokio::test]
async fn search_filter_matches_unpersisted_recovery_responses_and_sorts_by_payload() {
    for retention in [IntruderRetention::Project, IntruderRetention::Session] {
        let (mut state, mut events, id, mut records) = pending_fixture(retention);
        records[0].response.as_mut().unwrap().body = b"recovery-alpha-hit".to_vec();
        records[0].summary.response_bytes = records[0].response.as_ref().unwrap().body.len() as u64;
        records[1].response.as_mut().unwrap().body = vec![0xff, b'B', b'I', b'N'];
        records[1].summary.response_bytes = records[1].response.as_ref().unwrap().body.len() as u64;
        let _ = pending_results(&mut state, id, &records);

        let mut view = query(state.project, id, 1);
        view.filter.search_term = "recovery-alpha".into();
        let page = request(&mut state, &mut events, view, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].ordinal, 0);

        let mut negative = query(state.project, id, 2);
        negative.filter.search_term = "recovery-alpha".into();
        negative.filter.negative_search = true;
        negative.sort = IntruderResultSort {
            column: IntruderResultSortColumn::Payload,
            ascending: false,
        };
        let page = request(&mut state, &mut events, negative, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].ordinal, 1);
    }
}
