use super::*;

#[test]
fn discard_removes_only_owned_run_data_and_preserves_every_other_workspace_edit() {
    let mut store = store();
    let first = run(&mut store, 2);
    let second = store
        .create_intruder_run(Uuid::new_v4(), first.tab_id, &spec(2), 51)
        .unwrap();
    let removed = complete(&pending());
    let kept = complete(&pending());
    store
        .put_intruder_result(first.id, 0, Ok(&removed))
        .unwrap();
    store
        .put_intruder_result(first.id, 1, Err("not sent"))
        .unwrap();
    store.put_intruder_result(second.id, 0, Ok(&kept)).unwrap();
    let first = store
        .set_intruder_state(first.id, IntruderRunState::Completed)
        .unwrap();
    let second = store
        .set_intruder_state(second.id, IntruderRunState::Stopped)
        .unwrap();
    store.connection.execute(
        "INSERT INTO messages SELECT flow,side+2,head,body FROM messages WHERE flow=?1 AND side IN (0,1)",
        [removed.summary.id.to_string()],
    ).unwrap();
    let mut copy = removed.clone();
    copy.summary.id = Uuid::new_v4();
    copy.summary.source = TrafficSource::Repeater;
    store.put(&copy).unwrap();

    let mut proposal = discard_proposal(&store);
    proposal.orientation = crate::Orientation::Vertical;
    proposal.repeater_settings.unpack_compressed_responses = false;
    proposal.intruder.tabs[0].last_run = Some(second.id);
    proposal.intruder.tabs[0].spec.revision += 1;
    proposal.intruder.tabs[0].spec.draft.revision += 1;
    proposal.intruder.tabs[0].spec.draft.raw.push(0xff);
    let mut old = proposal.intruder.tabs[0].clone();
    old.id = Uuid::new_v4();
    old.last_run = Some(first.id);
    proposal.intruder.tabs.push(old.clone());
    old.id = Uuid::new_v4();
    proposal.intruder.closed.push(old);
    let mut tab = crate::WorkTab::from_request(Some(&copy.request), "independent copy".into());
    tab.last_flow = Some(copy.summary.id);
    proposal.tabs.push(tab);
    let mut expected = proposal.clone();
    expected.intruder.tabs[1].last_run = None;
    expected.intruder.closed[0].last_run = None;

    store.connection.authorizer(Some(deny_discard_body_reads));
    let receipt = store
        .discard_intruder_run(
            first.id,
            first.revision,
            proposal,
            &CancellationToken::new(),
        )
        .unwrap();
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(receipt.workspace, expected);
    assert_eq!(store.workspace().unwrap(), expected);
    assert_eq!(
        (
            receipt.revision,
            receipt.deleted_flows,
            receipt.retained_flows
        ),
        (first.revision + 1, 1, 0)
    );
    assert!(store.find_intruder_run(first.id).unwrap().is_none());
    assert!(store.read_intruder_spec(first.id).is_err());
    assert!(store.intruder_results(first.id, None, 1).is_err());
    assert_eq!(store.read_intruder_run(second.id).unwrap(), second);
    assert_eq!(store.read_intruder_spec(second.id).unwrap(), spec(2));
    assert_eq!(store.read(kept.summary.id).unwrap(), kept);
    assert_eq!(store.read(copy.summary.id).unwrap(), copy);
    for (table, column, id) in [
        ("intruder_payloads", "run", first.id),
        ("intruder_results", "run", first.id),
        ("flows", "id", removed.summary.id),
        ("messages", "flow", removed.summary.id),
    ] {
        let rows: u64 = store
            .connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE {column}=?1"),
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "logical deletion must remove owned {table} rows");
    }
    assert!(
        !store
            .connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    let mut late = expected.clone();
    late.revision += 1;
    late.intruder.tabs[1].last_run = Some(first.id);
    assert!(
        store.save_workspace(&late).is_err(),
        "a later workspace must not restore a retired reference"
    );
    assert_eq!(store.workspace().unwrap(), expected);
}

#[test]
fn discard_keeps_flows_referenced_by_open_closed_or_websocket_data() {
    let mut store = store();
    let attack = run(&mut store, 3);
    let flows: Vec<_> = (0..3).map(|_| complete(&pending())).collect();
    for (ordinal, flow) in flows.iter().enumerate() {
        store
            .put_intruder_result(attack.id, ordinal as u64, Ok(flow))
            .unwrap();
    }
    let attack = store
        .set_intruder_state(attack.id, IntruderRunState::Completed)
        .unwrap();
    let mut previous = discard_proposal(&store);
    let mut open = crate::WorkTab::from_request(Some(&flows[0].request), "open owner".into());
    open.last_flow = Some(flows[0].summary.id);
    previous.tabs.push(open);
    store.save_workspace(&previous).unwrap();
    let message = crate::WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flows[2].summary.id,
        sequence: 1,
        direction: crate::WebSocketDirection::ServerToClient,
        kind: crate::WebSocketKind::Binary,
        payload: vec![0, 255],
        observed_unix_ms: 1,
    };
    store.put_websocket(&message).unwrap();
    let mut proposal = discard_proposal(&store);
    proposal.tabs[0].last_flow = None;
    let mut closed = crate::WorkTab::from_request(Some(&flows[1].request), "closed owner".into());
    closed.last_flow = Some(flows[1].summary.id);
    proposal.closed.push(closed);
    let receipt = store
        .discard_intruder_run(
            attack.id,
            attack.revision,
            proposal.clone(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(receipt.workspace, proposal);
    assert_eq!((receipt.deleted_flows, receipt.retained_flows), (0, 3));
    for flow in flows {
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    }
    assert_eq!(store.read_websocket(message.id).unwrap(), message);
    assert!(store.find_intruder_run(attack.id).unwrap().is_none());
}

#[test]
fn discard_rejects_live_states_and_stale_revisions_without_mutation() {
    for state in [
        IntruderRunState::Running,
        IntruderRunState::Paused,
        IntruderRunState::Stopping,
    ] {
        let mut store = store();
        let attack = run(&mut store, 1);
        let attack = store.set_intruder_state(attack.id, state).unwrap();
        let proposal = discard_proposal(&store);
        let changes = total_changes(&store);
        assert!(
            store
                .discard_intruder_run(
                    attack.id,
                    attack.revision,
                    proposal,
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(total_changes(&store), changes);
        assert_eq!(store.read_intruder_run(attack.id).unwrap(), attack);
    }
    let mut store = store();
    let attack = run(&mut store, 0);
    let attack = store
        .set_intruder_state(attack.id, IntruderRunState::Completed)
        .unwrap();
    for (revision, workspace) in [
        (attack.revision + 1, discard_proposal(&store)),
        (attack.revision, store.workspace().unwrap()),
    ] {
        let before = store.workspace().unwrap();
        let changes = total_changes(&store);
        assert!(
            store
                .discard_intruder_run(attack.id, revision, workspace, &CancellationToken::new())
                .is_err()
        );
        assert_eq!(total_changes(&store), changes);
        assert_eq!(store.workspace().unwrap(), before);
        assert_eq!(store.read_intruder_run(attack.id).unwrap(), attack);
    }
}

#[test]
fn discard_refuses_unknown_ownership_or_corrupt_terminal_results_before_writing() {
    for extension in [
        "CREATE TABLE linked_notes(flow TEXT REFERENCES flows(id) ON DELETE CASCADE,data BLOB)",
        "ALTER TABLE intruder_runs ADD COLUMN extension BLOB",
        "CREATE TABLE payload_copy AS SELECT * FROM intruder_payloads; DROP TABLE intruder_payloads; ALTER TABLE payload_copy RENAME TO intruder_payloads",
        "INSERT INTO metadata VALUES('unknown-owner',X'00')",
        "CREATE TRIGGER collateral AFTER DELETE ON intruder_runs BEGIN DELETE FROM flows; END",
        "CREATE TEMP TABLE intruder_results(run TEXT,ordinal INTEGER,flow TEXT,error TEXT)",
        "PRAGMA foreign_keys=OFF",
        "UPDATE flows SET state='pending'",
    ] {
        let mut store = store();
        let attack = run(&mut store, 1);
        let flow = complete(&pending());
        store.put_intruder_result(attack.id, 0, Ok(&flow)).unwrap();
        let attack = store
            .set_intruder_state(attack.id, IntruderRunState::Completed)
            .unwrap();
        let workspace = store.workspace().unwrap();
        let proposal = discard_proposal(&store);
        store.connection.execute_batch(extension).unwrap();
        let changes = total_changes(&store);
        assert!(
            store
                .discard_intruder_run(
                    attack.id,
                    attack.revision,
                    proposal,
                    &CancellationToken::new()
                )
                .is_err(),
            "{extension}"
        );
        assert_eq!(
            total_changes(&store),
            changes,
            "classification must precede mutation: {extension}"
        );
        assert_eq!(store.workspace().unwrap(), workspace);
        assert!(retired_run(&store.connection, attack.id).unwrap().is_none());
    }
}

#[test]
fn discard_write_failure_and_between_batch_cancellation_roll_back_all_changes() {
    for cancel_payload_batch in [false, true] {
        let mut store = store();
        let attack = run(&mut store, DISCARD_BATCH + 1);
        let flow = complete(&pending());
        store.put_intruder_result(attack.id, 0, Ok(&flow)).unwrap();
        let attack = store
            .set_intruder_state(attack.id, IntruderRunState::Stopped)
            .unwrap();
        let workspace = store.workspace().unwrap();
        let proposal = discard_proposal(&store);
        let cancel = CancellationToken::new();
        let injected = cancel.clone();
        store
            .connection
            .authorizer(Some(move |context: AuthContext<'_>| match context.action {
                AuthAction::Delete {
                    table_name: "intruder_payloads",
                    ..
                } if cancel_payload_batch => {
                    injected.cancel();
                    Authorization::Allow
                }
                AuthAction::Delete {
                    table_name: "flows",
                    ..
                } if !cancel_payload_batch => Authorization::Deny,
                _ => Authorization::Allow,
            }));
        let changes = total_changes(&store);
        assert!(
            store
                .discard_intruder_run(attack.id, attack.revision, proposal.clone(), &cancel)
                .is_err()
        );
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert!(
            total_changes(&store) > changes,
            "the failure must roll back earlier executed writes"
        );
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.read_intruder_run(attack.id).unwrap(), attack);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        assert_eq!(
            store.read_intruder_spec(attack.id).unwrap(),
            spec(DISCARD_BATCH + 1)
        );
        assert_eq!(
            store
                .intruder_results(attack.id, None, 1)
                .unwrap()
                .rows
                .len(),
            1
        );
        assert!(retired_run(&store.connection, attack.id).unwrap().is_none());
        store
            .discard_intruder_run(
                attack.id,
                attack.revision,
                proposal,
                &CancellationToken::new(),
            )
            .unwrap();
        assert!(store.find_intruder_run(attack.id).unwrap().is_none());
    }
}
