use super::*;
use crate::{
    Header, HttpResponse, HttpVersion, IntruderGrepConfig, IntruderGrepExtractRule,
    IntruderResultFilter, IntruderResultSort,
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

mod recovery;
mod search_and_payloads;

fn fixture(session: bool, count: usize, baseline: bool) -> (ProjectStore, IntruderRunSummary) {
    let spec = IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:9".into(),
            raw: b"GET / HTTP/1.1\r\n\r\nx".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 18, end: 19 }],
        payload_sets: vec![
            (0..count)
                .map(|index| format!("payload-{index}").into_bytes())
                .collect(),
        ],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: !session,
        baseline_request: baseline,
        resource_pool: None,
    };
    let mut store = ProjectStore::create(None).unwrap();
    let tab = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.active = Some(tab);
    workspace.intruder.tabs.push(crate::IntruderTab {
        id: tab,
        title: "query fixture".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    store.save_workspace(&workspace).unwrap();
    let run = store
        .create_intruder_run_with_retention(
            ProjectKey {
                id: store.id,
                generation: 1,
            },
            Uuid::new_v4(),
            tab,
            &spec,
            10,
        )
        .unwrap();
    (store, run)
}

fn query(store: &ProjectStore, run: &IntruderRunSummary) -> IntruderResultViewQuery {
    IntruderResultViewQuery {
        project: ProjectKey {
            id: store.id,
            generation: 1,
        },
        run_id: run.id,
        view_id: Uuid::new_v4(),
        query_revision: 1,
        offset: 0,
        limit: 2,
        filter: IntruderResultFilter::default(),
        sort: IntruderResultSort::default(),
        selected_ordinal: None,
        grep_config: IntruderGrepConfig::default(),
    }
}

fn response(status: u16) -> FlowRecord {
    let mut record = crate::store::tests::record();
    record.summary.source = TrafficSource::Intruder;
    record.summary.state = FlowState::Complete;
    record.summary.status = Some(status);
    record.summary.response_bytes = 3;
    record.response = Some(HttpResponse {
        status,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: vec![255, 0, 42],
        trailers: vec![],
    });
    record
}

fn insert_record(store: &mut ProjectStore, run: Uuid, ordinal: u64, record: &FlowRecord) {
    let mut pending = record.clone();
    pending.summary.state = FlowState::Pending;
    pending.summary.status = None;
    pending.summary.response_bytes = 0;
    pending.response = None;
    assert!(
        store
            .preflight_intruder_result(run, ordinal, &pending)
            .unwrap()
    );
    store.put_intruder_result(run, ordinal, Ok(record)).unwrap();
}

fn fetch(store: &ProjectStore, query: &IntruderResultViewQuery) -> IntruderResultViewPage {
    store
        .query_intruder_result_view(
            query,
            None,
            &[],
            &CancellationToken::new(),
            &CancellationToken::new(),
        )
        .unwrap()
}

fn ordinals(page: &IntruderResultViewPage) -> Vec<u64> {
    page.rows.iter().map(|row| row.ordinal).collect()
}

fn row_lengths(page: &IntruderResultViewPage) -> Vec<Option<u64>> {
    page.rows.iter().map(|row| row.response_length).collect()
}

#[test]
fn result_query_globally_filters_sorts_and_keeps_offpage_selection_without_bodies() {
    for session in [false, true] {
        let (mut store, run) = fixture(session, 5, false);
        for (ordinal, status) in [500, 201, 404, 202].into_iter().enumerate() {
            insert_record(&mut store, run.id, ordinal as u64, &response(status));
        }
        store
            .put_intruder_result(run.id, 4, Err("not sent"))
            .unwrap();
        store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
        let original = store.intruder_results(run.id, None, 10).unwrap();
        let connection = store.intruder_connection(run.id).unwrap().0;
        let bodies = Arc::new(AtomicUsize::new(0));
        let authorizer = || {
            let bodies = bodies.clone();
            move |context: AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "messages",
                    column_name: "body",
                    ..
                } => {
                    bodies.fetch_add(1, Ordering::SeqCst);
                    Authorization::Deny
                }
                AuthAction::Insert { .. }
                | AuthAction::Update { .. }
                | AuthAction::Delete { .. } => Authorization::Deny,
                _ => Authorization::Allow,
            }
        };
        connection.authorizer(Some(authorizer()));
        if session {
            store.connection.authorizer(Some(authorizer()));
        }
        let mut query = query(&store, &run);
        query.filter.status_classes = [true, false, false, true];
        query.sort.column = IntruderResultSortColumn::Status;
        query.offset = 1;
        query.selected_ordinal = Some(0);
        let page = fetch(&store, &query);
        assert_eq!(page.total, 4);
        assert_eq!(ordinals(&page), [1, 3]);
        assert_eq!(page.next_offset, Some(3));
        assert_eq!(page.selected_rank, Some(3));
        assert_eq!(page.selected_row.as_ref().unwrap().ordinal, 0);
        assert_eq!(page.rows[0].payloads[0].prefix, b"payload-1");
        assert_eq!(page.persisted, !session);
        query.offset = 3;
        let page = fetch(&store, &query);
        assert_eq!(ordinals(&page), [0]);
        assert_eq!(page.selected_rank, Some(3));
        assert!(page.selected_row.is_none());
        assert!(page.next_offset.is_none());
        query.selected_ordinal = Some(2);
        let page = fetch(&store, &query);
        assert!(page.selected_rank.is_none() && page.selected_row.is_none());
        query.offset = 50;
        assert_eq!(fetch(&store, &query).total, 4);
        assert_eq!(bodies.load(Ordering::SeqCst), 0);
        connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        if session {
            store
                .connection
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        }
        assert_eq!(store.intruder_results(run.id, None, 10).unwrap(), original);
        store.close().unwrap();
    }
}

#[test]
fn result_query_filters_and_reads_notes_and_highlight_overlays() {
    for session in [false, true] {
        let (mut store, run) = fixture(session, 3, false);
        insert_record(&mut store, run.id, 0, &response(200));
        insert_record(&mut store, run.id, 1, &response(201));
        store
            .put_intruder_result(run.id, 2, Err("not sent"))
            .unwrap();
        let project = ProjectKey {
            id: store.id,
            generation: 1,
        };
        store
            .set_intruder_result_notes(
                project,
                run.id,
                0,
                0,
                b"\xffbinary",
                &CancellationToken::new(),
            )
            .unwrap();
        store
            .set_intruder_result_highlight(
                project,
                run.id,
                &[(1, 0)],
                Some(3),
                &CancellationToken::new(),
            )
            .unwrap();
        let mut q = query(&store, &run);
        q.limit = 10;
        let page = fetch(&store, &q);
        assert_eq!(page.rows[0].notes_revision, 1);
        assert_eq!(page.rows[0].notes_preview, "\u{fffd}binary");
        assert_eq!(page.rows[1].highlight, Some(3));
        assert_eq!(page.rows[1].highlight_revision, 1);
        q.filter.only_with_notes = true;
        assert_eq!(ordinals(&fetch(&store, &q)), [0]);
        q.filter.only_with_notes = false;
        q.filter.only_highlighted = true;
        assert_eq!(ordinals(&fetch(&store, &q)), [1]);
        q.filter.only_highlighted = false;
        store.close().unwrap();
    }
}

#[test]
fn result_query_sorts_response_received_and_length_globally_and_fills_lengths() {
    let (mut store, run) = fixture(false, 5, false);

    let mut r0 = response(200);
    r0.summary.elapsed_ms = 100;
    r0.response.as_mut().unwrap().body = b"aaaa".to_vec();
    r0.summary.response_bytes = 4;
    insert_record(&mut store, run.id, 0, &r0);

    let mut r1 = response(201);
    r1.summary.elapsed_ms = 40;
    r1.response.as_mut().unwrap().headers = vec![Header {
        name: "X-H".into(),
        value: b"1".to_vec(),
    }];
    r1.response.as_mut().unwrap().body = b"bb".to_vec();
    r1.response.as_mut().unwrap().trailers = vec![Header {
        name: "X-End".into(),
        value: b"z".to_vec(),
    }];
    r1.summary.response_bytes = 2;
    insert_record(&mut store, run.id, 1, &r1);

    let mut r2 = response(500);
    r2.summary.elapsed_ms = 70;
    r2.response.as_mut().unwrap().body = Vec::new();
    r2.summary.response_bytes = 0;
    insert_record(&mut store, run.id, 2, &r2);

    let mut failed = response(200);
    failed.summary.state = FlowState::Failed("boom".into());
    failed.summary.status = None;
    failed.summary.response_bytes = 0;
    failed.summary.elapsed_ms = 999;
    failed.response = None;
    insert_record(&mut store, run.id, 3, &failed);

    store
        .put_intruder_result(run.id, 4, Err("not sent"))
        .unwrap();
    store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();

    let expected = [
        crate::http::intruder_response_len(
            r0.response.as_ref().unwrap().version,
            r0.response.as_ref().unwrap().status,
            None,
            &r0.response.as_ref().unwrap().headers,
            r0.summary.response_bytes,
        ),
        crate::http::intruder_response_len(
            r1.response.as_ref().unwrap().version,
            r1.response.as_ref().unwrap().status,
            None,
            &r1.response.as_ref().unwrap().headers,
            r1.summary.response_bytes,
        ),
        crate::http::intruder_response_len(
            r2.response.as_ref().unwrap().version,
            r2.response.as_ref().unwrap().status,
            None,
            &r2.response.as_ref().unwrap().headers,
            r2.summary.response_bytes,
        ),
    ];
    assert_eq!(expected, [23, 34, 38]);

    let connection = store.intruder_connection(run.id).unwrap().0;
    let bodies = Arc::new(AtomicUsize::new(0));
    connection.authorizer(Some({
        let bodies = bodies.clone();
        move |context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "messages",
                column_name: "body",
                ..
            } => {
                bodies.fetch_add(1, Ordering::SeqCst);
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }
    }));

    let mut q = query(&store, &run);
    q.limit = 10;

    q.sort = IntruderResultSort {
        column: IntruderResultSortColumn::ResponseReceived,
        ascending: true,
    };
    let asc = fetch(&store, &q);
    assert_eq!(ordinals(&asc), [3, 4, 1, 2, 0]);
    assert_eq!(
        row_lengths(&asc),
        [None, None, Some(34), Some(38), Some(23)]
    );

    q.sort.ascending = false;
    let desc = fetch(&store, &q);
    assert_eq!(ordinals(&desc), [0, 2, 1, 3, 4]);
    assert_eq!(
        row_lengths(&desc),
        [Some(23), Some(38), Some(34), None, None]
    );

    q.sort = IntruderResultSort {
        column: IntruderResultSortColumn::Length,
        ascending: true,
    };
    let len_asc = fetch(&store, &q);
    assert_eq!(ordinals(&len_asc), [3, 4, 0, 1, 2]);
    assert_eq!(
        row_lengths(&len_asc),
        [None, None, Some(23), Some(34), Some(38)]
    );

    q.sort.ascending = false;
    let len_desc = fetch(&store, &q);
    assert_eq!(ordinals(&len_desc), [2, 1, 0, 3, 4]);
    assert_eq!(
        row_lengths(&len_desc),
        [Some(38), Some(34), Some(23), None, None]
    );

    assert_eq!(bodies.load(Ordering::SeqCst), 0);
    connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    store.close().unwrap();
}
