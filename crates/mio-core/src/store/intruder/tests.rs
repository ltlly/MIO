use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::HttpResponse;

mod admission_binding;
mod annotations_rollback;
mod baseline;
mod discard_core;
mod discard_regressions_a;
mod discard_regressions_b;
mod discard_retirement;
mod paging_corruption;
mod pause;
mod recovery;
pub(in crate::store::intruder) use annotations_rollback::deny_run_update;
pub(in crate::store::intruder) use paging_corruption::deny_large_data_reads;
mod title;

fn store() -> ProjectStore {
    let store = ProjectStore::create(None).unwrap();
    let exists: bool = store
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='intruder_runs')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        exists,
        "the real project creation path must install the Intruder schema"
    );
    store
}

fn spec(payloads: usize) -> IntruderSpec {
    IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 3,
        draft: Draft {
            revision: 7,
            target: "http://127.0.0.1:1".into(),
            raw: b"POST / HTTP/1.1\r\n\r\n\0".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 19, end: 20 }],
        payload_sets: vec![vec![vec![0, 255, 1]; payloads]],
        max_concurrent_requests: 2,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
    }
}

fn run(store: &mut ProjectStore, count: usize) -> IntruderRunSummary {
    let input = spec(count);
    let tab = bind(store, &input);
    store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 50)
        .unwrap()
}

fn bind(store: &mut ProjectStore, input: &IntruderSpec) -> TabId {
    let id = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.intruder.tabs.push(crate::IntruderTab {
        id,
        title: "fixture".into(),
        group: None,
        spec: input.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(id);
    workspace.revision += 1;
    store.save_workspace(&workspace).unwrap();
    id
}

fn pending() -> FlowRecord {
    let mut record = super::super::tests::record();
    record.summary.source = TrafficSource::Intruder;
    record.summary.state = FlowState::Pending;
    record.summary.elapsed_ms = 0;
    record
}

fn complete(before: &FlowRecord) -> FlowRecord {
    let mut record = before.clone();
    record.summary.revision += 1;
    record.summary.status = Some(201);
    record.summary.response_bytes = 3;
    record.summary.state = FlowState::Complete;
    record.summary.elapsed_ms = 1;
    record.response = Some(HttpResponse {
        status: 201,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: record.request.headers.clone(),
        body: vec![255, 0, 128],
        trailers: Vec::new(),
    });
    record
}

fn recovery_mutations(run: &IntruderRunSummary, request: &FlowRecord) -> Vec<IntruderMutation> {
    let mut expected = run.clone();
    expected.revision += 1;
    expected.dispatched += 1;
    let mut mutations = vec![IntruderMutation::Result {
        run: expected.clone(),
        ordinal: 0,
        result: Ok(Box::new(request.clone())),
    }];
    expected.revision += 1;
    expected.completed += 1;
    mutations.push(IntruderMutation::Result {
        run: expected.clone(),
        ordinal: 0,
        result: Ok(Box::new(complete(request))),
    });
    expected.revision += 1;
    expected.dispatched += 1;
    expected.completed += 1;
    mutations.push(IntruderMutation::Result {
        run: expected.clone(),
        ordinal: 1,
        result: Err("invalid request".into()),
    });
    expected.revision += 1;
    expected.state = IntruderRunState::Stopping;
    mutations.push(IntruderMutation::State {
        run: expected.clone(),
    });
    expected.revision += 1;
    expected.state = IntruderRunState::Stopped;
    mutations.push(IntruderMutation::State { run: expected });
    mutations
}

fn total_changes(store: &ProjectStore) -> u64 {
    store
        .connection
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap()
}

fn discard_proposal(store: &ProjectStore) -> Workspace {
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace
}

fn deny_discard_body_reads(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "messages",
            column_name: "head" | "body",
            ..
        }
        | AuthAction::Read {
            table_name: "intruder_payloads",
            column_name: "value",
            ..
        }
        | AuthAction::Read {
            table_name: "intruder_runs",
            column_name: "template" | "spec_meta",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn discard_workspace_bytes(connection: &Connection) -> Vec<u8> {
    connection
        .query_row(
            "SELECT value FROM main.metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
