use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::intruder::{AttackType, Position};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

mod backpressure;
mod recovery;

fn pending_fixture(
    retention: IntruderRetention,
) -> (State, mpsc::Receiver<Event>, Uuid, Vec<FlowRecord>) {
    let (sender, receiver) = mpsc::channel(32);
    let mut state = State::new(sender, CancellationToken::new()).unwrap();
    let raw = b"POST /query HTTP/1.1\r\nHost: 127.0.0.1:9\r\nContent-Length: 1\r\n\r\nX".to_vec();
    let start = raw.len() - 1;
    let payloads = vec![vec![b'A'], vec![0xff, b'B']];
    let spec = IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:9".into(),
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position {
            start,
            end: start + 1,
        }],
        payload_sets: vec![payloads.clone()],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: retention == IntruderRetention::Project,
        baseline_request: false,
        resource_pool: None,
    };
    let tab = Uuid::new_v4();
    let mut workspace = state.store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.active = Some(tab);
    workspace.intruder.tabs.push(IntruderTab {
        id: tab,
        title: "query fixture".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    state.store.save_workspace(&workspace).unwrap();
    let id = Uuid::new_v4();
    state
        .store
        .create_intruder_run_with_retention(state.project, id, tab, &spec, 10)
        .unwrap();
    let mut completed = Vec::new();
    for (ordinal, (payload, status)) in payloads.into_iter().zip([201, 404]).enumerate() {
        let mut record = crate::service::tests::completed_flow();
        record.summary.source = TrafficSource::Intruder;
        record.summary.url = "http://127.0.0.1:9/query".into();
        record.summary.request_bytes = payload.len() as u64;
        record.summary.status = Some(status);
        record.request.url = record.summary.url.clone();
        record.request.body = payload;
        record.response.as_mut().unwrap().status = status;
        let mut pending = record.clone();
        pending.summary.revision = 1;
        pending.summary.state = FlowState::Pending;
        pending.summary.status = None;
        pending.summary.response_bytes = 0;
        pending.response = None;
        assert!(
            state
                .store
                .preflight_intruder_result(id, ordinal as u64, &pending)
                .unwrap()
        );
        state
            .store
            .put_intruder_result(id, ordinal as u64, Ok(&pending))
            .unwrap();
        completed.push(record);
    }
    (state, receiver, id, completed)
}

fn fixture(retention: IntruderRetention) -> (State, mpsc::Receiver<Event>, Uuid, Vec<FlowRecord>) {
    let (mut state, receiver, id, records) = pending_fixture(retention);
    for (ordinal, record) in records.iter().enumerate() {
        state
            .store
            .put_intruder_result(id, ordinal as u64, Ok(record))
            .unwrap();
    }
    (state, receiver, id, records)
}

fn query(project: ProjectKey, run_id: Uuid, revision: u64) -> IntruderResultViewQuery {
    IntruderResultViewQuery {
        project,
        run_id,
        view_id: Uuid::new_v4(),
        query_revision: revision,
        offset: 0,
        limit: 20,
        filter: IntruderResultFilter::default(),
        sort: IntruderResultSort::default(),
        selected_ordinal: None,
        grep_config: crate::IntruderGrepConfig::default(),
    }
}

async fn request(
    state: &mut State,
    receiver: &mut mpsc::Receiver<Event>,
    query: IntruderResultViewQuery,
    cancel: CancellationToken,
) -> std::result::Result<IntruderResultViewPage, String> {
    state
        .command(Command::QueryIntruderResultView {
            query: query.clone(),
            cancel,
        })
        .await
        .unwrap();
    loop {
        if let Event::IntruderResultView {
            query: actual,
            result,
        } = receiver.try_recv().unwrap()
        {
            assert_eq!(
                actual, query,
                "the reply must preserve the complete query identity"
            );
            return result;
        }
    }
}

#[tokio::test]
async fn cancelled_foreign_and_invalid_view_queries_do_not_read_the_store() {
    let (mut state, mut events, id, _) = fixture(IntruderRetention::Project);
    let reads = Arc::new(AtomicUsize::new(0));
    state
        .store
        .test_intruder_connection(id)
        .unwrap()
        .authorizer(Some({
            let reads = reads.clone();
            move |context: AuthContext<'_>| {
                if matches!(context.action, AuthAction::Read { .. }) {
                    reads.fetch_add(1, Ordering::SeqCst);
                }
                Authorization::Allow
            }
        }));
    let mut inputs = Vec::new();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    inputs.push((query(state.project, id, 1), cancelled));
    let mut foreign = query(state.project, id, 2);
    foreign.project.generation += 1;
    inputs.push((foreign, CancellationToken::new()));
    let mut missing_view = query(state.project, id, 3);
    missing_view.view_id = Uuid::nil();
    inputs.push((missing_view, CancellationToken::new()));
    inputs.push((
        query(state.project, Uuid::nil(), 4),
        CancellationToken::new(),
    ));
    inputs.push((query(state.project, id, 0), CancellationToken::new()));
    let mut empty_page = query(state.project, id, 5);
    empty_page.limit = 0;
    inputs.push((empty_page, CancellationToken::new()));
    for (query, cancel) in inputs {
        assert!(
            request(&mut state, &mut events, query, cancel)
                .await
                .is_err()
        );
    }
    state
        .store
        .test_intruder_connection(id)
        .unwrap()
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn project_and_session_queries_return_payloads_without_reading_message_bodies() {
    for retention in [IntruderRetention::Project, IntruderRetention::Session] {
        let (mut state, mut events, id, records) = fixture(retention);
        if retention == IntruderRetention::Project {
            state
                .store
                .test_intruder_connection(id)
                .unwrap()
                .authorizer(Some(|context: AuthContext<'_>| match context.action {
                    AuthAction::Read {
                        table_name: "messages",
                        column_name: "body",
                        ..
                    } => Authorization::Deny,
                    _ => Authorization::Allow,
                }));
        }
        let mut view = query(state.project, id, 1);
        view.limit = 1;
        view.sort = IntruderResultSort {
            column: IntruderResultSortColumn::Status,
            ascending: false,
        };
        view.selected_ordinal = Some(1);
        let page = request(&mut state, &mut events, view, CancellationToken::new())
            .await
            .unwrap();
        if retention == IntruderRetention::Project {
            state
                .store
                .test_intruder_connection(id)
                .unwrap()
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        }
        assert_eq!(page.run.id, id);
        assert_eq!(page.run.retention, retention);
        assert_eq!(page.persisted, retention == IntruderRetention::Project);
        assert_eq!(
            (page.total, page.next_offset, page.selected_rank),
            (2, Some(1), Some(0))
        );
        assert!(
            page.selected_row.is_none(),
            "the selected row already belongs to the page"
        );
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].ordinal, 1);
        assert_eq!(page.rows[0].payloads[0].prefix, [0xff, b'B']);
        assert_eq!(page.rows[0].payloads[0].raw_bytes, 2);
        assert_eq!(
            page.rows[0].result.as_ref().unwrap().outcome,
            IntruderResultOutcome::Flow(records[1].summary.clone())
        );
        for record in records {
            assert_eq!(state.store.read(record.summary.id).unwrap(), record);
        }
        assert!(!state.intruder.has_resources());
    }
}
