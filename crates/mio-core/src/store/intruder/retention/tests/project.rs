use super::*;

#[test]
fn retention_result_write_cannot_reuse_a_session_flow_in_another_run() {
    let mut store = ProjectStore::create(None).unwrap();
    let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
    let spec = store.read_intruder_spec(run.id).unwrap();
    let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
    let pending = store
        .create_intruder_run(Uuid::new_v4(), run.tab_id, &spec, 2)
        .unwrap();
    let before = raw_dataset(&store.connection, pending.id);
    let memory = store.intruder_sessions.runs[&run.id].data.clone();
    let quota = store.intruder_sessions.logical_bytes;

    let result = store.put_intruder_result(pending.id, 0, Ok(&flow));

    assert!(
        result.is_err(),
        "another run must not write a Session-owned flow back to disk"
    );
    assert_eq!(raw_dataset(&store.connection, pending.id), before);
    assert_eq!(store.intruder_sessions.runs[&run.id].data, memory);
    assert_eq!(store.intruder_sessions.logical_bytes, quota);
    assert_eq!(store.read_intruder_run(pending.id).unwrap(), pending);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), session.run);
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert!(!has_flow(&store.connection, flow.summary.id).unwrap());
}

#[test]
fn retention_recovery_write_cannot_publish_a_session_flow_in_save_or_save_as() {
    for save_as in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mio");
        let destination = directory.path().join("destination.mio");
        let mut store = ProjectStore::create(Some(&source)).unwrap();
        let (run, flow) = finished(&mut store, Uuid::new_v4(), 1);
        let spec = store.read_intruder_spec(run.id).unwrap();
        let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
        let pending = store
            .create_intruder_run(Uuid::new_v4(), run.tab_id, &spec, 2)
            .unwrap();
        let before = raw_dataset(&store.connection, pending.id);
        let workspace = workspace_bytes(&store);
        let memory = store.intruder_sessions.runs[&run.id].data.clone();
        let quota = store.intruder_sessions.logical_bytes;
        let mut completed = pending.clone();
        completed.revision += 1;
        completed.dispatched = 1;
        completed.completed = 1;
        let mutations = [store::IntruderMutation::Result {
            run: completed,
            ordinal: 0,
            result: Ok(Box::new(flow.clone())),
        }];

        let result = store.save_with(
            save_as.then_some(destination.as_path()),
            None,
            &[],
            &[],
            &mutations,
            &CancellationToken::new(),
        );

        assert!(
            result.is_err(),
            "recovery must not publish a Session-owned flow (SaveAs={save_as})"
        );
        assert_eq!(store.path.as_ref(), Some(&source));
        assert!(!destination.exists());
        assert_eq!(raw_dataset(&store.connection, pending.id), before);
        assert_eq!(workspace_bytes(&store), workspace);
        assert_eq!(store.intruder_sessions.runs[&run.id].data, memory);
        assert_eq!(store.intruder_sessions.logical_bytes, quota);
        assert_eq!(store.read_intruder_run(pending.id).unwrap(), pending);
        assert_eq!(store.read_intruder_run(run.id).unwrap(), session.run);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        assert!(!has_flow(&store.connection, flow.summary.id).unwrap());
    }
}

#[test]
fn retention_snapshot_adoption_rejects_changed_session_flows() {
    for shared in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mio");
        let destination = directory.path().join("published.mio");
        let mut store = ProjectStore::create(Some(&source)).unwrap();
        let (run, flow) = finished(&mut store, Uuid::new_v4(), 10);
        if shared {
            let mut workspace = proposal(&store);
            let mut tab = crate::WorkTab::from_request(Some(&flow.request), "shared flow".into());
            tab.last_flow = Some(flow.summary.id);
            workspace.tabs.push(tab);
            store.save_workspace(&workspace).unwrap();
        }
        let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
        let expected = store.intruder_results(run.id, None, 10).unwrap();
        let workspace = store.workspace().unwrap();
        let mut copy = Connection::open(&destination).unwrap();
        {
            let backup = rusqlite::backup::Backup::new(&store.connection, &mut copy).unwrap();
            assert_eq!(backup.step(-1).unwrap(), rusqlite::backup::StepResult::Done);
        }
        if shared {
            copy.execute(
                "UPDATE messages SET body=X'00' WHERE flow=?1 AND side=1",
                [flow.summary.id.to_string()],
            )
            .unwrap();
        } else {
            let record = store.intruder_sessions.runs[&run.id]
                .data
                .read_flow_record(flow.summary.id)
                .unwrap()
                .unwrap();
            let tx = copy.transaction().unwrap();
            store::messages::write_record(&tx, &record).unwrap();
            tx.commit().unwrap();
        }
        copy.close().unwrap();
        let reopened = ProjectStore::open(&destination).unwrap();
        assert_eq!(reopened.id, store.id);

        let result = store.adopt_snapshot(reopened);

        assert!(
            result.is_err(),
            "Save As must preserve Session flow identities (shared={shared})"
        );
        assert_eq!(store.path.as_ref(), Some(&source));
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.intruder_results(run.id, None, 10).unwrap(), expected);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        change(&mut store, &session.run, IntruderRetention::Project).unwrap();
        store.close().unwrap();
    }
}

#[test]
fn retention_snapshot_adoption_rejects_changed_ticket_and_keeps_source_ram() {
    for replace_ticket in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mio");
        let destination = directory.path().join("published.mio");
        let mut store = ProjectStore::create(Some(&source)).unwrap();
        let (run, flow) = finished(&mut store, Uuid::new_v4(), 10);
        let session = change(&mut store, &run, IntruderRetention::Session).unwrap();
        let expected = store.intruder_results(run.id, None, 10).unwrap();
        let workspace = store.workspace().unwrap();
        let mut copy = Connection::open(&destination).unwrap();
        {
            let backup = rusqlite::backup::Backup::new(&store.connection, &mut copy).unwrap();
            assert_eq!(backup.step(-1).unwrap(), rusqlite::backup::StepResult::Done);
        }
        if replace_ticket {
            let mut other = session.run.clone();
            other.revision += 1;
            copy.execute(
                "UPDATE metadata SET value=?1 WHERE key=?2",
                params![retirement_bytes(&other).as_slice(), retirement_key(run.id)],
            )
            .unwrap();
        } else {
            copy.execute(
                "DELETE FROM metadata WHERE key=?1",
                [retirement_key(run.id)],
            )
            .unwrap();
        }
        copy.close().unwrap();
        let reopened = ProjectStore::open(&destination).unwrap();
        assert_eq!(reopened.id, store.id);

        let result = store.adopt_snapshot(reopened);

        assert!(
            result.is_err(),
            "Save As adoption must validate live Session tickets"
        );
        assert_eq!(store.path.as_ref(), Some(&source));
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.intruder_results(run.id, None, 10).unwrap(), expected);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        assert!(store.intruder_sessions.contains(run.id));
        change(&mut store, &session.run, IntruderRetention::Project).unwrap();
        store.close().unwrap();
    }
}

#[test]
fn retention_round_trip_preserves_frozen_data_and_all_message_versions() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mio");
    let destination = directory.path().join("save-as.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let owner = project(&store);
    let (first, flow) = finished(&mut store, Uuid::new_v4(), 10);
    let (second, second_flow) = finished(&mut store, Uuid::new_v4(), 11);
    store.connection.execute(
        "INSERT INTO messages SELECT flow,side+2,head,?2 FROM messages WHERE flow=?1 AND side IN (0,1)",
        params![flow.summary.id.to_string(), [0_u8, 254, 1, 2].as_slice()],
    ).unwrap();
    let original = store.original(flow.summary.id).unwrap();
    assert_eq!(original.request.as_ref().unwrap().body, [0, 254, 1, 2]);
    assert_eq!(original.response.as_ref().unwrap().body, [0, 254, 1, 2]);
    let before = raw_dataset(&store.connection, first.id);
    let frozen = store.read_intruder_spec(first.id).unwrap();
    let rows = store.intruder_results(first.id, None, 10).unwrap().rows;
    let mut copy = flow.clone();
    copy.summary.id = Uuid::new_v4();
    copy.summary.source = TrafficSource::Repeater;
    store.put(&copy).unwrap();
    let mut workspace = proposal(&store);
    workspace.intruder.tabs[0].last_run = Some(first.id);
    workspace.intruder.tabs[1].last_run = Some(second.id);
    workspace.orientation = crate::Orientation::Vertical;
    workspace.intruder.tabs[0].spec.revision += 1;
    workspace.intruder.tabs[0].spec.draft.revision += 1;
    workspace.intruder.tabs[0].spec.draft.raw.push(0xff);
    let receipt = store
        .set_intruder_retention(
            owner,
            first.id,
            first.revision,
            IntruderRetention::Session,
            workspace.clone(),
            &CancellationToken::new(),
        )
        .unwrap();
    workspace.intruder.tabs[0].last_run = None;
    assert_eq!(receipt.workspace, workspace);
    assert_eq!(receipt.run.revision, first.revision + 1);
    assert_eq!(receipt.run.retention, IntruderRetention::Session);
    assert_eq!(receipt.retention, receipt.run.retention);
    assert_eq!(store.read_intruder_run(first.id).unwrap(), receipt.run);
    assert_eq!(
        store.find_intruder_run(first.id).unwrap(),
        Some(receipt.run.clone())
    );
    assert_eq!(
        store.intruder_runs(None, 10).unwrap().rows,
        [receipt.run.clone(), second.clone()]
    );
    assert_eq!(store.workspace().unwrap(), workspace);
    assert!(find_run(&store.connection, first.id).unwrap().is_none());
    assert!(!has_flow(&store.connection, flow.summary.id).unwrap());
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert_eq!(store.original(flow.summary.id).unwrap(), original);
    assert_eq!(store.read_intruder_spec(first.id).unwrap(), frozen);
    let page = store.intruder_results(first.id, None, 10).unwrap();
    assert!(!page.persisted);
    assert_eq!(page.rows, rows);
    assert_eq!(page.run, receipt.run);
    let memory = &store.intruder_sessions.runs[&first.id];
    assert_eq!(memory.owner, owner);
    assert_eq!(memory.data.logical_bytes().unwrap(), memory.logical_bytes);
    assert!(receipt.session_logical_bytes > 0);
    assert_eq!(
        receipt.session_logical_bytes,
        store.intruder_sessions.logical_bytes
    );
    store.save(None).unwrap();
    store.save(Some(&destination)).unwrap();
    assert_eq!(store.intruder_sessions.runs[&first.id].owner, owner);
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    for path in [&source, &destination] {
        let disk = ProjectStore::open(path).unwrap();
        assert!(disk.find_intruder_run(first.id).unwrap().is_none());
        assert!(disk.read_optional(flow.summary.id).unwrap().is_none());
        assert_eq!(disk.read(copy.summary.id).unwrap(), copy);
        assert_eq!(disk.read(second_flow.summary.id).unwrap(), second_flow);
        disk.close().unwrap();
    }
    let mut workspace = proposal(&store);
    workspace.intruder.tabs[0].last_run = Some(first.id);
    let promoted = store
        .set_intruder_retention(
            owner,
            first.id,
            receipt.run.revision,
            IntruderRetention::Project,
            workspace.clone(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(promoted.run.revision, first.revision + 2);
    assert_eq!(promoted.run.retention, IntruderRetention::Project);
    assert_eq!(promoted.retention, promoted.run.retention);
    assert!(
        store
            .intruder_results(first.id, None, 10)
            .unwrap()
            .persisted
    );
    assert_eq!(promoted.session_logical_bytes, 0);
    assert!(store.intruder_sessions.runs.is_empty());
    assert_eq!(store.intruder_sessions.logical_bytes, 0);
    let after = raw_dataset(&store.connection, first.id);
    assert_eq!(before[0][0][..2], after[0][0][..2]);
    assert_eq!(before[0][0][3..], after[0][0][3..]);
    assert_eq!(before[1..], after[1..]);
    store.close().unwrap();
    let disk = ProjectStore::open(&destination).unwrap();
    assert_eq!(disk.workspace().unwrap(), workspace);
    assert_eq!(disk.read_intruder_run(first.id).unwrap(), promoted.run);
    assert_eq!(
        disk.intruder_results(first.id, None, 10).unwrap().rows,
        rows
    );
    assert_eq!(disk.read_intruder_spec(first.id).unwrap(), frozen);
    assert_eq!(disk.original(flow.summary.id).unwrap(), original);
    disk.close().unwrap();
}

#[test]
fn retention_quota_is_checked_before_copy_and_never_evicts_another_run() {
    let mut store = ProjectStore::create(None).unwrap();
    let (first, first_flow) = finished(&mut store, Uuid::new_v4(), 1);
    let (second, second_flow) = finished(&mut store, Uuid::new_v4(), 2);
    let size = dataset_logical_bytes(
        &store.connection,
        first.id,
        u64::MAX,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(size < 16 * 1024);
    store.set_intruder_session_quota(size - 1);
    let before = raw_dataset(&store.connection, first.id);
    let workspace = workspace_bytes(&store);
    let error = change(&mut store, &first, IntruderRetention::Session).unwrap_err();
    assert!(error.to_string().contains("逻辑 RAM"));
    assert_eq!(raw_dataset(&store.connection, first.id), before);
    assert_eq!(workspace_bytes(&store), workspace);
    assert!(store.intruder_sessions.runs.is_empty());
    store.set_intruder_session_quota(size);
    let receipt = change(&mut store, &first, IntruderRetention::Session).unwrap();
    let error = change(&mut store, &second, IntruderRetention::Session).unwrap_err();
    assert!(error.to_string().contains("逻辑 RAM"));
    assert_eq!(store.read(first_flow.summary.id).unwrap(), first_flow);
    assert_eq!(store.read(second_flow.summary.id).unwrap(), second_flow);
    assert_eq!(store.read_intruder_run(first.id).unwrap(), receipt.run);
    assert_eq!(store.read_intruder_run(second.id).unwrap(), second);
    assert_eq!(store.intruder_sessions.logical_bytes, size);
    assert_eq!(store.intruder_sessions.runs.len(), 1);
}
