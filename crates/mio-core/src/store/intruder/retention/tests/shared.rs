use super::*;

fn deny_data_reads(context: AuthContext<'_>) -> Authorization {
    match context.action {
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
    }
}

#[test]
fn retention_summary_pages_merge_stable_cursors_without_reading_bodies() {
    let mut store = ProjectStore::create(None).unwrap();
    let runs: Vec<_> = (1..=4)
        .map(|id| finished(&mut store, Uuid::from_u128(id), 20).0)
        .collect();
    let first = change(&mut store, &runs[0], IntruderRetention::Session).unwrap();
    let third = change(&mut store, &runs[2], IntruderRetention::Session).unwrap();
    store.connection.authorizer(Some(deny_data_reads));
    let page = store.intruder_runs(None, 1).unwrap();
    assert_eq!(page.rows[0].id, runs[0].id);
    assert_eq!(page.next_after, Some(runs[0].id));
    let results = store.intruder_results(first.run.id, None, 1).unwrap();
    assert_eq!(results.next_after, Some(0));
    assert!(!results.persisted);
    assert_eq!(
        store
            .intruder_results(first.run.id, results.next_after, 1)
            .unwrap()
            .rows[0]
            .ordinal,
        1
    );
    for dataset in store.intruder_sessions.runs.values() {
        assert_eq!(
            dataset.body_reads.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    change(&mut store, &third.run, IntruderRetention::Project).unwrap();
    store
        .discard_intruder_run(
            first.run.id,
            first.run.revision,
            proposal(&store),
            &CancellationToken::new(),
        )
        .unwrap();
    let mut cursor = page.next_after;
    let mut seen = vec![];
    loop {
        let page = store.intruder_runs(cursor, 1).unwrap();
        seen.extend(page.rows.iter().map(|row| row.id));
        if page.next_after.is_none() {
            break;
        }
        cursor = page.next_after;
    }
    assert_eq!(seen, runs[1..].iter().map(|run| run.id).collect::<Vec<_>>());
    assert!(store.intruder_results(first.run.id, None, 1).is_err());
}

#[test]
fn retention_merged_pages_keep_the_encoded_byte_budget() {
    let mut store = ProjectStore::create(None).unwrap();
    store.set_intruder_session_quota(1024 * 1024);
    let mut runs = vec![];
    for id in 1..=4 {
        let (run, _) = finished(&mut store, Uuid::from_u128(id), 1);
        store
            .connection
            .execute(
                "UPDATE intruder_runs SET state=5,error=?1 WHERE id=?2",
                params!["\0".repeat(MAX_ERROR_BYTES), run.id.to_string()],
            )
            .unwrap();
        runs.push(store.read_intruder_run(run.id).unwrap());
    }
    change(&mut store, &runs[0], IntruderRetention::Session).unwrap();
    change(&mut store, &runs[2], IntruderRetention::Session).unwrap();
    let mut cursor = None;
    let mut seen = vec![];
    loop {
        let page = store.intruder_runs(cursor, 500).unwrap();
        assert!(serde_json::to_vec(&page.rows).unwrap().len() <= PAGE_BYTES);
        seen.extend(page.rows.iter().map(|run| run.id));
        if page.next_after.is_none() {
            break;
        }
        assert_ne!(cursor, page.next_after);
        cursor = page.next_after;
    }
    assert_eq!(seen, runs.iter().map(|run| run.id).collect::<Vec<_>>());
}

#[test]
fn retention_keeps_shared_repeater_websocket_and_independent_copies() {
    let mut store = ProjectStore::create(None).unwrap();
    let (first, first_flow) = finished(&mut store, Uuid::new_v4(), 1);
    let (second, second_flow) = finished(&mut store, Uuid::new_v4(), 2);
    let mut workspace = proposal(&store);
    let mut tab =
        crate::WorkTab::from_request(Some(&first_flow.request), "independent owner".into());
    tab.last_flow = Some(first_flow.summary.id);
    workspace.closed.push(tab);
    store.save_workspace(&workspace).unwrap();
    let message = crate::WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: second_flow.summary.id,
        sequence: 1,
        direction: crate::WebSocketDirection::ServerToClient,
        kind: crate::WebSocketKind::Binary,
        payload: vec![0, 255],
        observed_unix_ms: 1,
    };
    store.put_websocket(&message).unwrap();
    let first = change(&mut store, &first, IntruderRetention::Session).unwrap();
    let second = change(&mut store, &second, IntruderRetention::Session).unwrap();
    assert!(has_flow(&store.connection, first_flow.summary.id).unwrap());
    assert!(has_flow(&store.connection, second_flow.summary.id).unwrap());
    assert_eq!(store.read(first_flow.summary.id).unwrap(), first_flow);
    assert_eq!(store.read(second_flow.summary.id).unwrap(), second_flow);
    assert_eq!(store.read_websocket(message.id).unwrap(), message);
    assert!(
        store.put(&first_flow).is_err(),
        "ordinary writes cannot replace a Session-owned identity"
    );
    store
        .connection
        .execute(
            "UPDATE messages SET body=X'123456' WHERE flow=?1 AND side=1",
            [first_flow.summary.id.to_string()],
        )
        .unwrap();
    assert!(store.read(first_flow.summary.id).is_err());
    assert!(change(&mut store, &first.run, IntruderRetention::Project).is_err());
    assert!(store.intruder_sessions.contains(first.run.id));
    let receipt = store
        .discard_intruder_run(
            first.run.id,
            first.run.revision,
            proposal(&store),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!((receipt.deleted_flows, receipt.retained_flows), (0, 1));
    assert_eq!(
        store
            .read(first_flow.summary.id)
            .unwrap()
            .response
            .unwrap()
            .body,
        [0x12, 0x34, 0x56]
    );
    change(&mut store, &second.run, IntruderRetention::Project).unwrap();
    assert_eq!(store.read_websocket(message.id).unwrap(), message);
}

#[test]
fn retention_shared_intruder_flow_can_return_from_either_session_owner() {
    let mut store = ProjectStore::create(None).unwrap();
    let (first, first_flow) = finished(&mut store, Uuid::new_v4(), 1);
    let (second, second_flow) = finished(&mut store, Uuid::new_v4(), 2);
    store.connection.execute_batch(
        "ALTER TABLE intruder_results RENAME TO old_results;
         CREATE TABLE intruder_results(run TEXT NOT NULL REFERENCES intruder_runs(id) ON DELETE CASCADE,
             ordinal INTEGER NOT NULL,flow TEXT REFERENCES flows(id),error TEXT,notes BLOB,notes_revision INTEGER NOT NULL DEFAULT 0,highlight INTEGER,highlight_revision INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(run,ordinal)) WITHOUT ROWID;
         INSERT INTO intruder_results SELECT * FROM old_results;
         DROP TABLE old_results;"
    ).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_results SET flow=?1 WHERE run=?2 AND ordinal=0",
            params![first_flow.summary.id.to_string(), second.id.to_string()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "DELETE FROM flows WHERE id=?1",
            [second_flow.summary.id.to_string()],
        )
        .unwrap();
    let first = change(&mut store, &first, IntruderRetention::Session).unwrap();
    let second = change(&mut store, &second, IntruderRetention::Session).unwrap();
    assert!(!has_flow(&store.connection, first_flow.summary.id).unwrap());
    assert_eq!(store.read(first_flow.summary.id).unwrap(), first_flow);
    change(&mut store, &first.run, IntruderRetention::Project).unwrap();
    assert_eq!(store.read(first_flow.summary.id).unwrap(), first_flow);
    assert!(
        !store
            .intruder_results(second.run.id, None, 2)
            .unwrap()
            .persisted
    );
    change(&mut store, &second.run, IntruderRetention::Project).unwrap();
    assert!(store.intruder_sessions.runs.is_empty());
    assert_eq!(
        store
            .connection
            .query_row::<u64, _, _>(
                "SELECT count(*) FROM intruder_results WHERE flow=?1",
                [first_flow.summary.id.to_string()],
                |row| row.get(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn retention_two_project_owners_keep_the_same_run_uuid_isolated_until_drop() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first-owner.mio");
    let second_path = directory.path().join("second-owner.mio");
    let mut first = ProjectStore::create(Some(&first_path)).unwrap();
    let mut second = ProjectStore::create(Some(&second_path)).unwrap();
    let id = Uuid::from_u128(0xabc);
    let (first_run, first_flow) = finished(&mut first, id, 1);
    let (second_run, second_flow) = finished(&mut second, id, 2);
    let first_owner = project(&first);
    let second_owner = project(&second);
    assert_ne!(first_owner.id, second_owner.id);
    let first_session = change(&mut first, &first_run, IntruderRetention::Session).unwrap();
    let second_session = change(&mut second, &second_run, IntruderRetention::Session).unwrap();
    assert!(
        first
            .set_intruder_retention(
                second_owner,
                id,
                first_session.run.revision,
                IntruderRetention::Project,
                proposal(&first),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert!(
        second
            .set_intruder_retention(
                first_owner,
                id,
                second_session.run.revision,
                IntruderRetention::Project,
                proposal(&second),
                &CancellationToken::new()
            )
            .is_err()
    );
    first.close().unwrap();
    let disk = ProjectStore::open(&first_path).unwrap();
    assert!(disk.find_intruder_run(id).unwrap().is_none());
    assert!(disk.read_optional(first_flow.summary.id).unwrap().is_none());
    disk.close().unwrap();
    assert_eq!(second.read_intruder_run(id).unwrap(), second_session.run);
    assert_eq!(second.read(second_flow.summary.id).unwrap(), second_flow);
    assert!(
        second
            .read_optional(first_flow.summary.id)
            .unwrap()
            .is_none()
    );
    second.close().unwrap();
    let disk = ProjectStore::open(&second_path).unwrap();
    assert!(disk.find_intruder_run(id).unwrap().is_none());
    assert!(
        disk.read_optional(second_flow.summary.id)
            .unwrap()
            .is_none()
    );
    disk.close().unwrap();
}

#[test]
fn retention_session_discard_is_atomic_and_releases_only_its_runtime_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime-only.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
    let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
    let workspace = workspace_bytes(&store);
    let mut invalid = proposal(&store);
    let mut tab =
        crate::WorkTab::from_request(Some(&flow.request), "cannot pin RAM-only ID".into());
    tab.last_flow = Some(flow.summary.id);
    invalid.tabs.push(tab);
    assert!(store.save_workspace(&invalid).is_err());
    assert!(store.require_intruder_sessions_resolved().is_err());
    assert_eq!(workspace_bytes(&store), workspace);
    store.reject_writes(true);
    assert!(
        store
            .discard_intruder_run(
                run.id,
                session.run.revision,
                proposal(&store),
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    store.reject_writes(false);
    let discarded = store
        .discard_intruder_run(
            run.id,
            session.run.revision,
            proposal(&store),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(discarded.revision, session.run.revision + 1);
    assert_eq!((discarded.deleted_flows, discarded.retained_flows), (1, 0));
    assert!(store.require_intruder_sessions_resolved().is_ok());
    assert!(store.read_optional(flow.summary.id).unwrap().is_none());
    let input = store.workspace().unwrap().intruder.tabs[0].spec.clone();
    assert!(
        store
            .create_intruder_run(run.id, run.tab_id, &input, 1)
            .is_err()
    );
    assert!(store.put_intruder_result(run.id, 0, Ok(&flow)).is_err());
    store.close().unwrap();
    let disk = ProjectStore::open(&path).unwrap();
    assert!(disk.find_intruder_run(run.id).unwrap().is_none());
    assert!(disk.read_optional(flow.summary.id).unwrap().is_none());
    assert_eq!(
        retired_run(&disk.connection, run.id).unwrap(),
        Some((discarded.revision, 1))
    );
    disk.close().unwrap();
}
