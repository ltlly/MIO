use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[path = "tests/baseline.rs"]
mod baseline;

fn fixture(quota: u64) -> (ProjectStore, ProjectKey, IntruderRunSummary, IntruderSpec) {
    let mut store = ProjectStore::create(None).unwrap();
    store.set_intruder_session_quota(quota);
    let project = ProjectKey {
        id: store.id,
        generation: 1,
    };
    let spec = IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:1".into(),
            raw: b"POST / HTTP/1.1\r\n\r\nX".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 19, end: 20 }],
        payload_sets: vec![vec![vec![0, 255], vec![0, 1]]],
        max_concurrent_requests: 2,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: false,
    };
    let tab = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.tabs.push(crate::IntruderTab {
        id: tab,
        title: "session".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab);
    store.save_workspace(&workspace).unwrap();
    let run = store
        .create_intruder_run_with_retention(project, Uuid::new_v4(), tab, &spec, 50)
        .unwrap();
    (store, project, run, spec)
}

fn pending() -> FlowRecord {
    let request = crate::parse_request(
        b"POST / HTTP/1.1\r\n\r\nX",
        "http://127.0.0.1:1",
        HttpVersion::Http11,
    )
    .unwrap();
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Intruder,
            method: request.method.clone(),
            url: request.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: request.body.len() as u64,
            response_bytes: 0,
            elapsed_ms: 0,
            started_unix_ms: 1,
        },
        request,
        response: None,
    }
}

fn complete(before: &FlowRecord) -> FlowRecord {
    let mut record = before.clone();
    record.summary.revision += 1;
    record.summary.state = FlowState::Complete;
    record.summary.status = Some(200);
    record.summary.response_bytes = 2;
    record.response = Some(crate::HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: vec![0, 255],
        trailers: vec![],
    });
    record
}

fn assert_accounting(store: &ProjectStore, id: Uuid) {
    let dataset = &store.intruder_sessions.runs[&id];
    assert_eq!(
        dataset_logical_bytes(&dataset.connection, id, u64::MAX, &CancellationToken::new())
            .unwrap(),
        dataset.logical_bytes
    );
    assert_eq!(store.intruder_sessions.logical_bytes, dataset.logical_bytes);
    assert!(
        dataset
            .connection
            .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))
            .unwrap()
    );
    assert!(
        store.intruder_sessions.logical_bytes + store.intruder_sessions.reserved_bytes
            <= store.intruder_sessions.quota
    );
}

#[test]
fn live_session_start_results_save_as_and_terminal_promotion_preserve_all_data() {
    let (mut store, project, run, spec) = fixture(SESSION_LOGICAL_QUOTA);
    assert_eq!(run.retention, IntruderRetention::Session);
    assert!(find_run(&store.connection, run.id).unwrap().is_none());
    assert!(retired_run(&store.connection, run.id).unwrap().is_none());
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
    assert!(
        store
            .create_intruder_run(run.id, run.tab_id, &spec, 50)
            .is_err()
    );
    let first = pending();
    assert!(store.preflight_intruder_result(run.id, 0, &first).unwrap());
    store.put_intruder_result(run.id, 0, Ok(&first)).unwrap();
    let paused = store
        .set_intruder_state(run.id, IntruderRunState::Paused)
        .unwrap();
    assert_eq!(paused.retention, IntruderRetention::Session);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), paused);
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("ordinary.mio");
    let second_path = directory.path().join("copy.mio");
    store.save(Some(&first_path)).unwrap();
    store.save(None).unwrap();
    store.save(Some(&second_path)).unwrap();
    assert_eq!(store.read_intruder_run(run.id).unwrap(), paused);
    for path in [&first_path, &second_path] {
        let disk = ProjectStore::open(path).unwrap();
        assert!(disk.find_intruder_run(run.id).unwrap().is_none());
        assert!(disk.read_optional(first.summary.id).unwrap().is_none());
        disk.close().unwrap();
    }
    let stopping = store
        .set_intruder_state(run.id, IntruderRunState::Stopping)
        .unwrap();
    let mut premature = store.workspace().unwrap();
    premature.revision += 1;
    assert!(
        store
            .set_intruder_retention(
                project,
                run.id,
                stopping.revision,
                IntruderRetention::Project,
                premature,
                &CancellationToken::new(),
            )
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), stopping);
    let response = complete(&first);
    store.put_intruder_result(run.id, 0, Ok(&response)).unwrap();
    let stopped = store
        .set_intruder_state(run.id, IntruderRunState::Stopped)
        .unwrap();
    assert_eq!(store.intruder_runs(Some(run.id), 10).unwrap().rows.len(), 0);
    assert_accounting(&store, run.id);
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    let promoted = store
        .set_intruder_retention(
            project,
            run.id,
            stopped.revision,
            IntruderRetention::Project,
            workspace,
            &CancellationToken::new(),
        )
        .unwrap()
        .run;
    assert_eq!(promoted.retention, IntruderRetention::Project);
    assert!(!store.is_session_intruder_run(run.id));
    assert_eq!(store.intruder_sessions.reserved_bytes, 0);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
    store.save(None).unwrap();
    store.close().unwrap();
    let reopened = ProjectStore::open(&second_path).unwrap();
    assert_eq!(reopened.read_intruder_run(run.id).unwrap(), promoted);
    assert_eq!(reopened.read(response.summary.id).unwrap(), response);
    reopened.close().unwrap();
}

#[test]
fn live_session_reservations_wait_then_release_without_overcommitting_or_dropping_results() {
    let (mut store, _, run, _) = fixture(24 * 1024 * 1024);
    let first = pending();
    let second = pending();
    assert!(store.preflight_intruder_result(run.id, 0, &first).unwrap());
    store.put_intruder_result(run.id, 0, Ok(&first)).unwrap();
    assert_accounting(&store, run.id);
    assert!(!store.preflight_intruder_result(run.id, 1, &second).unwrap());
    assert_eq!(store.read_intruder_run(run.id).unwrap().dispatched, 1);
    store
        .put_intruder_result(run.id, 0, Ok(&complete(&first)))
        .unwrap();
    assert!(store.preflight_intruder_result(run.id, 1, &second).unwrap());
    store.put_intruder_result(run.id, 1, Ok(&second)).unwrap();
    store
        .put_intruder_result(run.id, 1, Ok(&complete(&second)))
        .unwrap();
    let done = store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();
    assert_eq!((done.dispatched, done.completed), (2, 2));
    assert_eq!(store.intruder_sessions.reserved_bytes, 0);
    assert_accounting(&store, run.id);
    let (mut small, _, run, _) = fixture(256 * 1024);
    assert!(
        small
            .preflight_intruder_result(run.id, 0, &pending())
            .is_err()
    );
    small
        .set_intruder_state(run.id, IntruderRunState::Failed("capacity".into()))
        .unwrap();
    assert_eq!(small.read_intruder_run(run.id).unwrap().dispatched, 0);
    assert_accounting(&small, run.id);
}

#[test]
fn live_session_ram_recovery_is_atomic_and_never_writes_project_data() {
    let (mut store, _, run, _) = fixture(SESSION_LOGICAL_QUOTA);
    let request = pending();
    assert!(
        store
            .preflight_intruder_result(run.id, 0, &request)
            .unwrap()
    );
    let original_reserve = store.intruder_sessions.reserved_bytes;
    store.intruder_sessions.runs[&run.id]
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Insert {
                table_name: "flows",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    assert!(store.put_intruder_result(run.id, 0, Ok(&request)).is_err());
    store.intruder_sessions.runs[&run.id]
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(store.intruder_sessions.reserved_bytes, original_reserve);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    let mut rejected = request.clone();
    rejected.summary.state = FlowState::Failed("not dispatched".into());
    let mut next = run.clone();
    next.revision += 1;
    next.dispatched = 1;
    next.completed = 1;
    let first = IntruderMutation::Result {
        run: next.clone(),
        ordinal: 0,
        result: Ok(Box::new(rejected.clone())),
    };
    let mut bad = next.clone();
    bad.revision += 2;
    bad.state = IntruderRunState::Failed("stop".into());
    assert!(
        store
            .retry_session_intruder_mutations(
                &[first, IntruderMutation::State { run: bad }],
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert!(
        store
            .read_session_intruder_flow(request.summary.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.intruder_sessions.reserved_bytes, original_reserve);
    let first = IntruderMutation::Result {
        run: next.clone(),
        ordinal: 0,
        result: Ok(Box::new(rejected.clone())),
    };
    next.revision += 1;
    next.state = IntruderRunState::Failed("stop".into());
    let batch = [first, IntruderMutation::State { run: next.clone() }];
    store
        .retry_session_intruder_mutations(&batch, &CancellationToken::new())
        .unwrap();
    store
        .retry_session_intruder_mutations(&batch, &CancellationToken::new())
        .unwrap();
    assert_eq!(store.read_intruder_run(run.id).unwrap(), next);
    assert_eq!(
        store
            .read_session_intruder_flow(request.summary.id)
            .unwrap(),
        Some(rejected)
    );
    assert!(find_run(&store.connection, run.id).unwrap().is_none());
    assert_eq!(store.intruder_sessions.reserved_bytes, 0);
    assert_accounting(&store, run.id);
}

#[test]
fn live_session_discard_reserves_id_and_preserves_other_project_flows() {
    let (mut store, project, run, spec) = fixture(SESSION_LOGICAL_QUOTA);
    let terminal = store
        .set_intruder_state(run.id, IntruderRunState::Stopped)
        .unwrap();
    let mut proposal = store.workspace().unwrap();
    proposal.revision += 1;
    store
        .discard_intruder_run(
            run.id,
            terminal.revision,
            proposal,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(!store.is_session_intruder_run(run.id));
    assert!(
        store
            .create_intruder_run_with_retention(project, run.id, run.tab_id, &spec, 50)
            .is_err()
    );
    assert_eq!(store.intruder_sessions.logical_bytes, 0);
    assert_eq!(store.intruder_sessions.reserved_bytes, 0);
}

#[test]
fn legacy_intruder_spec_metadata_defaults_project_without_rewriting_bytes() {
    let (mut store, _, run, mut spec) = fixture(SESSION_LOGICAL_QUOTA);
    let mut workspace = store.workspace().unwrap();
    spec.save_to_project = true;
    spec.revision += 1;
    workspace.intruder.tabs[0].spec = spec.clone();
    workspace.revision += 1;
    store.save_workspace(&workspace).unwrap();
    let disk = store
        .create_intruder_run(Uuid::new_v4(), run.tab_id, &spec, 51)
        .unwrap();
    let meta: Vec<u8> = store
        .connection
        .query_row(
            "SELECT spec_meta FROM intruder_runs WHERE id=?1",
            [disk.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&meta).unwrap();
    json.as_object_mut().unwrap().remove("save_to_project");
    let legacy = serde_json::to_vec(&json).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET spec_meta=?1 WHERE id=?2",
            params![&legacy, disk.id.to_string()],
        )
        .unwrap();
    assert_eq!(store.read_intruder_spec(disk.id).unwrap(), spec);
    let unchanged: Vec<u8> = store
        .connection
        .query_row(
            "SELECT spec_meta FROM intruder_runs WHERE id=?1",
            [disk.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, legacy);
}
