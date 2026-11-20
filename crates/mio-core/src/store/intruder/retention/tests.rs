use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::{Type, ValueRef};

use super::*;

mod project;
mod running_failures;
mod session;
mod shared;

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 7,
    }
}

fn proposal(store: &ProjectStore) -> Workspace {
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace
}

fn running_promotion_fixture(
    store: &mut ProjectStore,
    paused: bool,
) -> (IntruderRunSummary, [FlowRecord; 2]) {
    let (previous, response) = finished(store, Uuid::new_v4(), 1);
    let mut spec = store.read_intruder_spec(previous.id).unwrap();
    spec.save_to_project = false;
    spec.revision += 1;
    let mut workspace = proposal(store);
    workspace.intruder.tabs[0].spec = spec.clone();
    store.save_workspace(&workspace).unwrap();
    let id = Uuid::new_v4();
    store
        .create_intruder_run_with_retention(project(store), id, previous.tab_id, &spec, 2)
        .unwrap();
    store
        .rename_intruder_run(
            project(store),
            id,
            0,
            "运行中保存🙂",
            &CancellationToken::new(),
        )
        .unwrap();
    let mut first = response.clone();
    first.summary.id = Uuid::new_v4();
    first.summary.state = FlowState::Pending;
    first.summary.status = None;
    first.summary.response_bytes = 0;
    first.summary.elapsed_ms = 0;
    first.response = None;
    assert!(store.preflight_intruder_result(id, 0, &first).unwrap());
    store.put_intruder_result(id, 0, Ok(&first)).unwrap();
    first.summary.revision += 1;
    first.summary.state = FlowState::Complete;
    first.summary.status = Some(200);
    first.summary.response_bytes = response.summary.response_bytes;
    first.response = response.response;
    store.put_intruder_result(id, 0, Ok(&first)).unwrap();
    let mut second = first.clone();
    second.summary.id = Uuid::new_v4();
    second.summary.revision = 1;
    second.summary.state = FlowState::Pending;
    second.summary.status = None;
    second.summary.response_bytes = 0;
    second.response = None;
    assert!(store.preflight_intruder_result(id, 1, &second).unwrap());
    let mut run = store.put_intruder_result(id, 1, Ok(&second)).unwrap();
    if paused {
        run = store
            .set_intruder_state(id, IntruderRunState::Paused)
            .unwrap();
    }
    assert_eq!((run.dispatched, run.completed), (2, 1));
    assert!(store.intruder_sessions.reserved_bytes > 0);
    (run, [first, second])
}

#[test]
fn running_session_promotion_keeps_pending_rows_pause_and_future_completion() {
    for paused in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("running-promotion.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let (run, [first, mut second]) = running_promotion_fixture(&mut store, paused);
        let spec = store.read_intruder_spec(run.id).unwrap();
        let project_rows: u64 = store
            .connection
            .query_row("SELECT count(*) FROM intruder_runs", [], |row| row.get(0))
            .unwrap();
        let mut workspace = proposal(&store);
        workspace.intruder.tabs[0].last_run = Some(run.id);
        let promoted = store
            .set_intruder_retention(
                project(&store),
                run.id,
                run.revision,
                IntruderRetention::Project,
                workspace.clone(),
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(promoted.workspace, workspace);
        let expected = IntruderRunSummary {
            revision: run.revision + 1,
            retention: IntruderRetention::Project,
            ..run.clone()
        };
        assert_eq!(promoted.run, expected);
        assert_eq!(store.read_intruder_run(run.id).unwrap(), expected);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
        assert_eq!(store.read(first.summary.id).unwrap(), first);
        assert_eq!(store.read(second.summary.id).unwrap(), second);
        assert_eq!(
            store
                .connection
                .query_row::<u64, _, _>("SELECT count(*) FROM intruder_runs", [], |row| row.get(0))
                .unwrap(),
            project_rows + 1
        );
        assert!(store.intruder_sessions.runs.is_empty());
        assert_eq!(
            (
                store.intruder_sessions.logical_bytes,
                store.intruder_sessions.reserved_bytes
            ),
            (0, 0)
        );
        assert!(
            change(&mut store, &expected, IntruderRetention::Session).is_err(),
            "running promotion must not enable reverse transfer"
        );
        assert!(
            store
                .discard_intruder_run(
                    run.id,
                    expected.revision,
                    proposal(&store),
                    &CancellationToken::new()
                )
                .is_err(),
            "running promotion must not enable Discard before terminal"
        );
        second.summary.revision += 1;
        second.summary.state = FlowState::Complete;
        second.summary.status = Some(200);
        second.summary.response_bytes = first.summary.response_bytes;
        second.response = first.response.clone();
        let after = store.put_intruder_result(run.id, 1, Ok(&second)).unwrap();
        assert_eq!(
            (after.dispatched, after.completed, after.state.clone()),
            (2, 2, run.state)
        );
        assert_eq!(
            (after.title.as_str(), after.title_revision),
            ("运行中保存🙂", 1)
        );
        let stopped = store
            .set_intruder_state(run.id, IntruderRunState::Stopped)
            .unwrap();
        store.save(None).unwrap();
        store.close().unwrap();
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.read_intruder_run(run.id).unwrap(), stopped);
        assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), spec);
        assert_eq!(reopened.read(first.summary.id).unwrap(), first);
        assert_eq!(reopened.read(second.summary.id).unwrap(), second);
        assert_eq!(reopened.workspace().unwrap(), workspace);
        reopened.close().unwrap();
    }
}

#[test]
fn running_session_promotion_reopens_pending_and_interrupts_once_without_replay() {
    for paused in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("promoted-pending.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let (run, [finished, pending]) = running_promotion_fixture(&mut store, paused);
        let spec = store.read_intruder_spec(run.id).unwrap();
        let mut workspace = proposal(&store);
        workspace.intruder.tabs[0].last_run = Some(run.id);
        let promoted = store
            .set_intruder_retention(
                project(&store),
                run.id,
                run.revision,
                IntruderRetention::Project,
                workspace.clone(),
                &CancellationToken::new(),
            )
            .unwrap()
            .run;
        store.save(None).unwrap();
        // This store fixture has no sender; closing the connection models persisted pending state.
        store.close().unwrap();
        let mut reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.read_intruder_run(run.id).unwrap(), promoted);
        assert_eq!(reopened.read(pending.summary.id).unwrap(), pending);
        assert_eq!(reopened.read(finished.summary.id).unwrap(), finished);
        assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), spec);
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(reopened.interrupt_intruder_runs().unwrap(), 1);
        let interrupted = reopened.read_intruder_run(run.id).unwrap();
        let mut expected = promoted;
        expected.revision += 1;
        expected.completed = expected.dispatched;
        expected.state = IntruderRunState::Interrupted;
        assert_eq!(interrupted, expected);
        let failed = reopened.read(pending.summary.id).unwrap();
        assert!(
            matches!(&failed.summary.state, FlowState::Failed(message) if message.contains("结果未知") && message.contains("未自动重发"))
        );
        let mut expected_flow = pending.clone();
        expected_flow.summary.revision += 1;
        expected_flow.summary.state = failed.summary.state.clone();
        assert_eq!(failed, expected_flow);
        assert_eq!(reopened.read(finished.summary.id).unwrap(), finished);
        assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), spec);
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(reopened.interrupt_intruder_runs().unwrap(), 0);
        assert_eq!(reopened.read_intruder_run(run.id).unwrap(), interrupted);
        assert_eq!(reopened.read(pending.summary.id).unwrap(), failed);
        assert!(
            reopened
                .set_intruder_state(run.id, IntruderRunState::Running)
                .is_err()
        );
        reopened.close().unwrap();
    }
}

fn finished(store: &mut ProjectStore, id: Uuid, started: i64) -> (IntruderRunSummary, FlowRecord) {
    let spec = IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 3,
        draft: Draft {
            revision: 7,
            target: "http://127.0.0.1:1".into(),
            raw: b"POST / HTTP/1.1\r\n\r\nX".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 19, end: 20 }],
        payload_sets: vec![vec![vec![0, 255], vec![], vec![0, 255]]],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
    };
    let mut workspace = proposal(store);
    let tab = Uuid::new_v4();
    workspace.intruder.tabs.push(crate::IntruderTab {
        id: tab,
        title: "retention fixture".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab);
    store.save_workspace(&workspace).unwrap();
    store.create_intruder_run(id, tab, &spec, started).unwrap();
    let mut flow = store::tests::record();
    flow.summary.source = TrafficSource::Intruder;
    flow.summary.state = FlowState::Complete;
    flow.summary.status = Some(200);
    flow.summary.response_bytes = 3;
    flow.response = Some(crate::HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: vec![255, 0, 42],
        trailers: vec![],
    });
    store.put_intruder_result(id, 0, Ok(&flow)).unwrap();
    store
        .put_intruder_result(id, 1, Err("rejected\0ordinal"))
        .unwrap();
    let run = store
        .set_intruder_state(id, IntruderRunState::Stopped)
        .unwrap();
    (run, flow)
}

fn change(
    store: &mut ProjectStore,
    run: &IntruderRunSummary,
    retention: IntruderRetention,
) -> Result<IntruderRetentionReceipt> {
    store.set_intruder_retention(
        project(store),
        run.id,
        run.revision,
        retention,
        proposal(store),
        &CancellationToken::new(),
    )
}

type RawCell = (Type, Vec<u8>);

fn raw_dataset(connection: &Connection, id: Uuid) -> Vec<Vec<Vec<RawCell>>> {
    DATA.iter()
        .map(|&(table, columns, selector)| {
            let mut query = connection
                .prepare(&select(table, columns, selector))
                .unwrap();
            let count = query.column_count();
            query
                .query_map([id.to_string()], |row| {
                    (0..count)
                        .map(|index| {
                            let value = row.get_ref(index)?;
                            let bytes = match value {
                                ValueRef::Null => Vec::new(),
                                ValueRef::Integer(value) => value.to_le_bytes().to_vec(),
                                ValueRef::Real(value) => value.to_bits().to_le_bytes().to_vec(),
                                ValueRef::Text(value) | ValueRef::Blob(value) => value.to_vec(),
                            };
                            Ok((value.data_type(), bytes))
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
}

fn workspace_bytes(store: &ProjectStore) -> Vec<u8> {
    store
        .connection
        .query_row(
            "SELECT value FROM metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
