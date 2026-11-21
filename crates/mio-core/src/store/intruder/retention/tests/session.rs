use super::*;

#[test]
fn retention_demotion_delete_cancel_and_commit_failures_roll_back() {
    for fault in 0..3 {
        let mut store = ProjectStore::create(None).unwrap();
        let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
        let before = raw_dataset(&store.connection, run.id);
        let workspace = workspace_bytes(&store);
        let cancel = CancellationToken::new();
        let injected = cancel.clone();
        store
            .connection
            .authorizer(Some(move |context: AuthContext<'_>| match context.action {
                AuthAction::Delete {
                    table_name: "flows",
                    ..
                } if fault == 0 => Authorization::Deny,
                AuthAction::Delete {
                    table_name: "intruder_payloads",
                    ..
                } if fault == 1 => {
                    injected.cancel();
                    Authorization::Allow
                }
                _ => Authorization::Allow,
            }));
        if fault == 2 {
            store.connection.commit_hook(Some(|| true));
        }
        let result = store.set_intruder_retention(
            project(&store),
            run.id,
            run.revision,
            IntruderRetention::Session,
            proposal(&store),
            &cancel,
        );
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        store.connection.commit_hook(None::<fn() -> bool>);
        assert!(result.is_err(), "fault {fault} must not report a transfer");
        assert_eq!(raw_dataset(&store.connection, run.id), before);
        assert_eq!(workspace_bytes(&store), workspace);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        assert!(store.intruder_sessions.runs.is_empty());
        assert_eq!(store.intruder_sessions.logical_bytes, 0);
        assert_eq!(retired_run(&store.connection, run.id).unwrap(), None);
    }
}

#[test]
fn retention_promotion_failure_keeps_ram_until_the_entire_commit_succeeds() {
    for fault in 0..3 {
        let mut store = ProjectStore::create(None).unwrap();
        let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
        let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
        let before = store.intruder_sessions.runs[&run.id].data.clone();
        let workspace = workspace_bytes(&store);
        store
            .connection
            .authorizer(Some(move |context: AuthContext<'_>| match context.action {
                AuthAction::Insert {
                    table_name: "intruder_results",
                    ..
                } if fault == 0 => Authorization::Deny,
                AuthAction::Update {
                    table_name: "metadata",
                    column_name: "value",
                    ..
                } if fault == 1 => Authorization::Deny,
                _ => Authorization::Allow,
            }));
        if fault == 2 {
            store.connection.commit_hook(Some(|| true));
        }
        let result = change(&mut store, &session.run, IntruderRetention::Project);
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        store.connection.commit_hook(None::<fn() -> bool>);
        assert!(result.is_err());
        assert_eq!(store.intruder_sessions.runs[&run.id].data, before);
        assert_eq!(workspace_bytes(&store), workspace);
        assert!(find_run(&store.connection, run.id).unwrap().is_none());
        assert!(!has_flow(&store.connection, flow.summary.id).unwrap());
        assert_eq!(
            retired_run(&store.connection, run.id).unwrap(),
            Some((session.run.revision, 1))
        );
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        let promoted = change(&mut store, &session.run, IntruderRetention::Project).unwrap();
        assert_eq!(promoted.run.revision, session.run.revision + 1);
        let mut newer = proposal(&store);
        newer.intruder.tabs[0].title = "newer workspace edit".into();
        store.save_workspace(&newer).unwrap();
        assert!(change(&mut store, &session.run, IntruderRetention::Project).is_err());
        assert!(change(&mut store, &run, IntruderRetention::Session).is_err());
        assert_eq!(store.workspace().unwrap(), newer);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    }
}

#[test]
fn retention_refuses_foreign_owner_generation_and_exhausted_release_revision() {
    let mut store = ProjectStore::create(None).unwrap();
    let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
    let owner = project(&store);
    let foreign = ProjectKey {
        id: Uuid::new_v4(),
        ..owner
    };
    assert!(
        store
            .set_intruder_retention(
                foreign,
                run.id,
                run.revision,
                IntruderRetention::Session,
                proposal(&store),
                &CancellationToken::new()
            )
            .is_err()
    );
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET revision=?1 WHERE id=?2",
            params![i64::MAX - 1, run.id.to_string()],
        )
        .unwrap();
    assert!(
        store
            .set_intruder_retention(
                owner,
                run.id,
                i64::MAX as u64 - 1,
                IntruderRetention::Session,
                proposal(&store),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(store.intruder_sessions.runs.is_empty());
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET revision=?1 WHERE id=?2",
            params![run.revision, run.id.to_string()],
        )
        .unwrap();
    let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
    for other in [
        foreign,
        ProjectKey {
            generation: owner.generation + 1,
            ..owner
        },
    ] {
        assert!(
            store
                .set_intruder_retention(
                    other,
                    run.id,
                    session.run.revision,
                    IntruderRetention::Project,
                    proposal(&store),
                    &CancellationToken::new()
                )
                .is_err()
        );
    }
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert_eq!(store.intruder_sessions.runs[&run.id].owner, owner);
}

#[test]
fn retention_collision_and_retirement_tampering_never_replace_ram() {
    for mutation in 0..5 {
        let mut store = ProjectStore::create(None).unwrap();
        let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
        let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
        let before = store.intruder_sessions.runs[&run.id].data.clone();
        match mutation {
            0 => {
                let record = store.intruder_sessions.runs[&run.id]
                    .data
                    .read_flow_record(flow.summary.id)
                    .unwrap()
                    .unwrap();
                let tx = store.connection.transaction().unwrap();
                store::messages::write_record(&tx, &record).unwrap();
                tx.commit().unwrap();
            }
            1 => {
                let mut wrong = session.run.clone();
                wrong.revision += 1;
                store
                    .connection
                    .execute(
                        "UPDATE metadata SET value=?1 WHERE key=?2",
                        params![retirement_bytes(&wrong).as_slice(), retirement_key(run.id)],
                    )
                    .unwrap();
            }
            2 => {
                store
                    .connection
                    .execute(
                        "DELETE FROM metadata WHERE key=?1",
                        [retirement_key(run.id)],
                    )
                    .unwrap();
            }
            3 => {
                let data = &store.intruder_sessions.runs[&run.id].data;
                let tx = store.connection.transaction().unwrap();
                data.write_to_project(
                    &tx,
                    &BTreeSet::new(),
                    data.run.revision,
                    &CancellationToken::new(),
                    |_| Ok(()),
                )
                .unwrap();
                tx.commit().unwrap();
            }
            4 => {
                store.connection.execute_batch("UPDATE metadata SET value=CAST(json_set(CAST(value AS TEXT),'$.future_scalar',7) AS BLOB) WHERE key='workspace'").unwrap();
            }
            _ => unreachable!(),
        }
        let disk = raw_dataset(&store.connection, run.id);
        let workspace = workspace_bytes(&store);
        assert!(change(&mut store, &session.run, IntruderRetention::Project).is_err());
        assert_eq!(store.intruder_sessions.runs[&run.id].data, before);
        assert_eq!(raw_dataset(&store.connection, run.id), disk);
        assert_eq!(workspace_bytes(&store), workspace);
    }
}

#[test]
fn retention_unknown_and_malformed_source_is_refused_without_rewriting_it() {
    for mutation in [
        "UPDATE intruder_runs SET spec_meta=CAST(json_set(CAST(spec_meta AS TEXT),'$.future_scalar',7) AS BLOB)",
        "UPDATE flows SET summary=CAST(json_set(CAST(summary AS TEXT),'$.future_scalar',7) AS BLOB)",
        "UPDATE messages SET head=CAST(json_set(CAST(head AS TEXT),'$.future_scalar',7) AS BLOB) WHERE side=0",
        "UPDATE metadata SET value=CAST(json_set(CAST(value AS TEXT),'$.future_scalar',7) AS BLOB) WHERE key='workspace'",
        "UPDATE messages SET body=CAST(body AS TEXT) WHERE side=0",
        "INSERT INTO messages SELECT flow,4,head,body FROM messages WHERE side=0",
        "DELETE FROM intruder_payloads WHERE ordinal=1",
        "DELETE FROM intruder_results WHERE ordinal=0",
        "UPDATE intruder_runs SET tab='AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA'",
    ] {
        let mut store = ProjectStore::create(None).unwrap();
        let (run, _) = finished(&mut store, Uuid::new_v4(), 1);
        let workspace = proposal(&store);
        store.connection.execute_batch(mutation).unwrap();
        let before = raw_dataset(&store.connection, run.id);
        let workspace_before = workspace_bytes(&store);
        let result = store.set_intruder_retention(
            project(&store),
            run.id,
            run.revision,
            IntruderRetention::Session,
            workspace,
            &CancellationToken::new(),
        );
        assert!(
            result.is_err(),
            "unsupported source must be preserved: {mutation}"
        );
        assert_eq!(raw_dataset(&store.connection, run.id), before);
        assert_eq!(workspace_bytes(&store), workspace_before);
        assert!(store.intruder_sessions.runs.is_empty());
        assert_eq!(retired_run(&store.connection, run.id).unwrap(), None);
    }
}
