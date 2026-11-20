use super::*;

#[test]
fn result_query_reads_recovery_responses_for_length_without_reading_bodies() {
    let (mut store, run) = fixture(false, 2, false);

    let mut r0 = response(200);
    r0.summary.elapsed_ms = 50;
    r0.response.as_mut().unwrap().body = b"xy".to_vec();
    r0.summary.response_bytes = 2;
    insert_record(&mut store, run.id, 0, &r0);

    let mut pending = response(200);
    pending.summary.state = FlowState::Pending;
    pending.summary.status = None;
    pending.summary.response_bytes = 0;
    pending.response = None;
    insert_record(&mut store, run.id, 1, &pending);

    let before = store.read_intruder_run(run.id).unwrap();
    let mut latest = before.clone();
    latest.revision += 2;
    latest.completed = 2;
    latest.dispatched = 2;
    latest.state = IntruderRunState::Stopped;

    let mut completed = pending.summary.clone();
    completed.revision += 1;
    completed.status = Some(204);
    completed.state = FlowState::Complete;
    completed.elapsed_ms = 30;
    completed.response_bytes = 3;
    let recovered_response = HttpResponse {
        status: 204,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "X-R".into(),
            value: b"ab".to_vec(),
        }],
        body: b"abc".to_vec(),
        trailers: vec![],
    };
    let recovery = vec![
        IntruderResultSummary {
            ordinal: 0,
            outcome: IntruderResultOutcome::Flow(r0.summary.clone()),
        },
        IntruderResultSummary {
            ordinal: 1,
            outcome: IntruderResultOutcome::Flow(completed),
        },
    ];
    let recovery_responses: BTreeMap<u64, &HttpResponse> =
        BTreeMap::from([(1, &recovered_response)]);

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
        column: IntruderResultSortColumn::Length,
        ascending: true,
    };
    let page = store
        .query_intruder_result_view_with_responses(
            &q,
            Some(&latest),
            &recovery,
            &recovery_responses,
            &CancellationToken::new(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(ordinals(&page), [0, 1]);
    assert_eq!(row_lengths(&page), [Some(21), Some(39)]);

    q.sort.ascending = false;
    let page = store
        .query_intruder_result_view_with_responses(
            &q,
            Some(&latest),
            &recovery,
            &recovery_responses,
            &CancellationToken::new(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(ordinals(&page), [1, 0]);
    assert_eq!(row_lengths(&page), [Some(39), Some(21)]);

    assert_eq!(bodies.load(Ordering::SeqCst), 0);
    connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    store.close().unwrap();
}

#[test]
fn result_query_orders_admitted_baseline_as_request_zero_without_fabricating_flow() {
    for session in [false, true] {
        let (mut store, run) = fixture(session, 2, false);
        insert_record(&mut store, run.id, 0, &response(201));
        let before = store.read_intruder_run(run.id).unwrap();
        let run = store
            .request_intruder_baseline(
                ProjectKey {
                    id: store.id,
                    generation: 1,
                },
                run.id,
                before.revision,
                &CancellationToken::new(),
            )
            .unwrap();
        let mut query = query(&store, &run);
        let page = fetch(&store, &query);
        assert_eq!(ordinals(&page), [1, 0]);
        assert_eq!(page.total, 2);
        assert!(page.rows[0].result.is_none() && page.rows[0].payloads.is_empty());
        assert_eq!(page.rows[1].payloads[0].prefix, b"payload-0");
        query.sort.ascending = false;
        assert_eq!(ordinals(&fetch(&store, &query)), [0, 1]);
        store.close().unwrap();
    }
}

#[test]
fn result_query_overlays_recovery_before_global_filter_count_and_order() {
    for session in [false, true] {
        let (mut store, run) = fixture(session, 3, false);
        let mut pending = response(200);
        pending.summary.state = FlowState::Pending;
        pending.summary.status = None;
        pending.summary.response_bytes = 0;
        pending.response = None;
        insert_record(&mut store, run.id, 0, &pending);
        insert_record(&mut store, run.id, 1, &response(404));
        let before = store.read_intruder_run(run.id).unwrap();
        let original = store.intruder_results(run.id, None, 10).unwrap();
        let mut latest = before.clone();
        latest.revision += 2;
        latest.completed = 3;
        latest.dispatched = 3;
        latest.state = IntruderRunState::Stopped;
        let mut completed = pending.summary;
        completed.revision += 1;
        completed.status = Some(503);
        completed.state = FlowState::Complete;
        let recovered_503 = HttpResponse {
            status: 503,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![],
            body: vec![],
            trailers: vec![],
        };
        let recovery_responses = BTreeMap::from([(0, &recovered_503)]);
        let recovery = vec![
            IntruderResultSummary {
                ordinal: 0,
                outcome: IntruderResultOutcome::Flow(completed),
            },
            IntruderResultSummary {
                ordinal: 2,
                outcome: IntruderResultOutcome::Rejected("not sent after storage error".into()),
            },
        ];
        let mut query = query(&store, &run);
        query.filter.status_classes = [false, false, false, true];
        query.sort.column = IntruderResultSortColumn::Status;
        let page = store
            .query_intruder_result_view_with_responses(
                &query,
                Some(&latest),
                &recovery,
                &recovery_responses,
                &CancellationToken::new(),
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(ordinals(&page), [2, 0]);
        assert_eq!(page.total, 2);
        assert!(!page.persisted);
        assert_eq!(page.rows[1].result.as_ref(), Some(&recovery[0]));
        assert_eq!(page.rows[1].response_length, Some(36));
        assert!(
            store
                .query_intruder_result_view(
                    &query,
                    Some(&latest),
                    &recovery,
                    &CancellationToken::new(),
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(store.intruder_results(run.id, None, 10).unwrap(), original);
        let duplicate = vec![recovery[0].clone(), recovery[0].clone()];
        assert!(
            store
                .query_intruder_result_view_with_responses(
                    &query,
                    Some(&latest),
                    &duplicate,
                    &recovery_responses,
                    &CancellationToken::new(),
                    &CancellationToken::new()
                )
                .is_err()
        );
        let mut wrong = latest.clone();
        wrong.completed = 2;
        wrong.state = IntruderRunState::Stopping;
        assert!(
            store
                .query_intruder_result_view_with_responses(
                    &query,
                    Some(&wrong),
                    &recovery,
                    &recovery_responses,
                    &CancellationToken::new(),
                    &CancellationToken::new()
                )
                .is_err()
        );
        store.close().unwrap();
    }
}

#[test]
fn result_query_never_filters_away_corrupt_original_summaries_or_missing_ordinals() {
    for session in [false, true] {
        for corruption in ["duplicate", "missing", "count"] {
            let (mut store, run) = fixture(session, 2, false);
            let record = response(404);
            insert_record(&mut store, run.id, 0, &record);
            store
                .put_intruder_result(run.id, 1, Err("not sent"))
                .unwrap();
            let connection = store.intruder_connection(run.id).unwrap().0;
            let read_only = connection
                .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))
                .unwrap();
            assert_eq!(read_only, session);
            connection.pragma_update(None, "query_only", false).unwrap();
            match corruption {
                "duplicate" => {
                    let bytes = serde_json::to_string(&record.summary).unwrap();
                    let bytes =
                        bytes.replacen("\"status\":404", "\"status\":404,\"status\":200", 1);
                    connection
                        .execute(
                            "UPDATE flows SET summary=?1 WHERE id=?2",
                            params![bytes.as_bytes(), record.summary.id.to_string()],
                        )
                        .unwrap();
                }
                "missing" => {
                    connection
                        .execute(
                            "DELETE FROM intruder_results WHERE run=?1 AND ordinal=0",
                            [run.id.to_string()],
                        )
                        .unwrap();
                }
                "count" => {
                    connection
                        .execute(
                            "UPDATE intruder_runs SET completed=0 WHERE id=?1",
                            [run.id.to_string()],
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            connection
                .pragma_update(None, "query_only", read_only)
                .unwrap();
            let mut query = query(&store, &run);
            query.filter.status_classes = [true, false, false, false];
            assert!(
                store
                    .query_intruder_result_view(
                        &query,
                        None,
                        &[],
                        &CancellationToken::new(),
                        &CancellationToken::new()
                    )
                    .is_err(),
                "{corruption}, Session={session}"
            );
            store.close().unwrap();
        }
    }
}

#[test]
fn result_query_shares_page_budget_with_selected_summary_and_preserves_source() {
    let (mut store, run) = fixture(false, 3, false);
    for ordinal in 0..3 {
        let mut record = response(200);
        let url = format!("http://127.0.0.1/{}", "x".repeat(400 * 1024));
        record.summary.url = url.clone();
        record.request.url = url;
        store
            .put_intruder_result(run.id, ordinal, Ok(&record))
            .unwrap();
    }
    let mut query = query(&store, &run);
    query.selected_ordinal = Some(2);
    let page = fetch(&store, &query);
    assert_eq!(ordinals(&page), [0]);
    assert_eq!(page.selected_row.as_ref().unwrap().ordinal, 2);
    assert_eq!(page.next_offset, Some(1));
    let bytes = page
        .rows
        .iter()
        .chain(page.selected_row.iter())
        .map(|row| view_row_bytes(row).unwrap())
        .sum::<usize>();
    assert!(bytes <= PAGE_BYTES);
    query.offset = 1;
    let page = fetch(&store, &query);
    assert_eq!(ordinals(&page), [1, 2]);
    assert!(page.selected_row.is_none());
    assert_eq!(
        store.intruder_results(run.id, None, 1).unwrap().rows[0].ordinal,
        0
    );
    store.close().unwrap();
}
