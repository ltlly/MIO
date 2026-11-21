use super::*;

#[test]
fn discard_retirement_blocks_replay_and_preserves_deleted_cursor_across_save_as() {
    let mut store = store();
    let input = spec(3);
    let tab = bind(&mut store, &input);
    let first = store
        .create_intruder_run(Uuid::from_u128(1), tab, &input, 10)
        .unwrap();
    let second = store
        .create_intruder_run(Uuid::from_u128(2), tab, &input, 20)
        .unwrap();
    let request = pending();
    let mutations = recovery_mutations(&first, &request);
    store
        .apply_pending(None, &[], &[], &mutations, &CancellationToken::new())
        .unwrap();
    let first = store.read_intruder_run(first.id).unwrap();
    let second = store
        .set_intruder_state(second.id, IntruderRunState::Stopped)
        .unwrap();
    let page = store.intruder_runs(None, 1).unwrap();
    assert_eq!(page.next_after, Some(first.id));
    let proposal = discard_proposal(&store);
    let mut competing = ProjectStore::open(&store.test_actual_path()).unwrap();
    assert!(competing.find_intruder_run(first.id).unwrap().is_some());
    let receipt = store
        .discard_intruder_run(
            first.id,
            first.revision,
            proposal.clone(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        competing
            .create_intruder_run(first.id, tab, &input, 10)
            .is_err()
    );
    competing.close().unwrap();
    let error = store
        .discard_intruder_run(
            first.id,
            first.revision,
            proposal,
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("已丢弃"));
    assert_eq!(store.workspace().unwrap(), receipt.workspace);
    assert!(
        store
            .create_intruder_run(first.id, tab, &input, 10)
            .is_err()
    );
    assert!(
        store
            .put_intruder_result(first.id, 0, Ok(&request))
            .is_err()
    );
    assert!(
        store
            .set_intruder_state(first.id, IntruderRunState::Running)
            .is_err()
    );
    assert!(
        store
            .apply_pending(None, &[], &[], &mutations, &CancellationToken::new())
            .is_err()
    );
    assert_eq!(
        store
            .intruder_runs(page.next_after, 1)
            .unwrap()
            .rows
            .as_slice(),
        std::slice::from_ref(&second)
    );
    assert!(store.intruder_runs(Some(Uuid::from_u128(99)), 1).is_err());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("discarded.mio");
    let copy = directory.path().join("discarded-copy.mio");
    store.save(Some(&source)).unwrap();
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [source, copy] {
        let mut reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), receipt.workspace);
        assert_eq!(
            retired_run(&reopened.connection, first.id).unwrap(),
            Some((receipt.revision, 10))
        );
        assert_eq!(
            reopened
                .intruder_runs(Some(first.id), 1)
                .unwrap()
                .rows
                .as_slice(),
            std::slice::from_ref(&second)
        );
        assert!(
            reopened
                .read_optional(request.summary.id)
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .create_intruder_run(first.id, tab, &input, 10)
                .is_err()
        );
        reopened.close().unwrap();
    }
}

#[test]
fn malformed_retirement_records_fail_closed_without_loading_unbounded_values() {
    for value in [
        rusqlite::types::Value::Blob(vec![0; 15]),
        rusqlite::types::Value::Blob(vec![0; 17]),
        rusqlite::types::Value::Blob(vec![0; 16]),
        rusqlite::types::Value::Text("not a retirement".into()),
    ] {
        let mut store = store();
        let input = spec(1);
        let tab = bind(&mut store, &input);
        let id = Uuid::new_v4();
        store
            .connection
            .execute(
                "INSERT INTO metadata VALUES(?1,?2)",
                params![retirement_key(id), value],
            )
            .unwrap();
        let changes = total_changes(&store);
        assert!(store.create_intruder_run(id, tab, &input, 1).is_err());
        assert!(store.intruder_runs(Some(id), 1).is_err());
        assert_eq!(total_changes(&store), changes);
    }
}

#[test]
fn discard_accepts_interrupted_and_failed_runs_but_not_exhausted_revisions() {
    for interrupted in [false, true] {
        let mut store = store();
        let attack = run(&mut store, 1);
        let flow = complete(&pending());
        store.put_intruder_result(attack.id, 0, Ok(&flow)).unwrap();
        if interrupted {
            store.interrupt_intruder_runs().unwrap();
        } else {
            store
                .set_intruder_state(
                    attack.id,
                    IntruderRunState::Failed("fixture failure".into()),
                )
                .unwrap();
        }
        let terminal = store.read_intruder_run(attack.id).unwrap();
        let proposal = discard_proposal(&store);
        store
            .discard_intruder_run(
                attack.id,
                terminal.revision,
                proposal,
                &CancellationToken::new(),
            )
            .unwrap();
        assert!(store.read_optional(flow.summary.id).unwrap().is_none());
    }
}

#[test]
fn discard_pages_result_deletion_without_reading_message_bodies() {
    let mut store = store();
    let attack = run(&mut store, DISCARD_BATCH + 1);
    let flow = complete(&pending());
    let transaction = store.connection.transaction().unwrap();
    for ordinal in 0..DISCARD_BATCH {
        write_result(&transaction, attack.id, ordinal as u64, Err("not sent")).unwrap();
    }
    write_result(&transaction, attack.id, DISCARD_BATCH as u64, Ok(&flow)).unwrap();
    let terminal = write_state(&transaction, attack.id, IntruderRunState::Completed).unwrap();
    transaction.commit().unwrap();
    let proposal = discard_proposal(&store);
    store.connection.authorizer(Some(deny_discard_body_reads));
    let receipt = store
        .discard_intruder_run(
            attack.id,
            terminal.revision,
            proposal,
            &CancellationToken::new(),
        )
        .unwrap();
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!((receipt.deleted_flows, receipt.retained_flows), (1, 0));
    assert!(store.read_optional(flow.summary.id).unwrap().is_none());
    let rows: u64 = store
        .connection
        .query_row(
            "SELECT count(*) FROM intruder_results WHERE run=?1",
            [attack.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}
