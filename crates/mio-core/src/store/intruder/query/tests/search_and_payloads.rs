use super::*;

#[test]
fn result_query_cancellation_and_deadline_restore_connection_handlers_and_busy_timeout() {
    const WORK: &str = "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i<3000) SELECT sum(i) FROM n";
    for session in [false, true] {
        let (mut store, run) = fixture(session, 1, false);
        store
            .put_intruder_result(run.id, 0, Err("not sent"))
            .unwrap();
        let connection = store.intruder_connection(run.id).unwrap().0;
        let read_only = connection
            .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))
            .unwrap();
        assert_eq!(read_only, session);
        connection
            .busy_timeout(Duration::from_millis(4321))
            .unwrap();
        let query = query(&store, &run);
        let cancel = CancellationToken::new();
        let shutdown = CancellationToken::new();
        connection.authorizer(Some({
            let cancel = cancel.clone();
            move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "intruder_results",
                        ..
                    }
                ) {
                    cancel.cancel();
                }
                Authorization::Allow
            }
        }));
        assert!(
            store
                .query_intruder_result_view(&query, None, &[], &cancel, &shutdown)
                .is_err()
        );
        connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(
            connection
                .pragma_query_value(None, "busy_timeout", |row| row.get::<_, u64>(0))
                .unwrap(),
            4321
        );
        assert!(
            connection
                .query_row(WORK, [], |row| row.get::<_, u64>(0))
                .is_ok()
        );
        {
            let _scope = QueryScope::new(
                connection,
                Instant::now() - Duration::from_millis(1),
                &CancellationToken::new(),
                &shutdown,
                (!session).then(|| shutdown.clone()),
            )
            .unwrap();
            assert!(
                connection
                    .query_row(WORK, [], |row| row.get::<_, u64>(0))
                    .is_err()
            );
        }
        assert!(
            connection
                .query_row(WORK, [], |row| row.get::<_, u64>(0))
                .is_ok()
        );
        assert!(connection.is_autocommit());
        assert_eq!(
            connection
                .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))
                .unwrap(),
            read_only
        );
        let write = connection.execute(
            "UPDATE intruder_runs SET title=title WHERE id=?1",
            [run.id.to_string()],
        );
        if read_only {
            assert_eq!(
                write.unwrap_err().sqlite_error_code(),
                Some(rusqlite::ErrorCode::ReadOnly)
            );
        } else {
            assert_eq!(write.unwrap(), 1);
        }
        shutdown.cancel();
        assert!(
            store
                .connection
                .query_row(WORK, [], |row| row.get::<_, u64>(0))
                .is_err()
        );
        if session {
            assert!(
                connection
                    .query_row(WORK, [], |row| row.get::<_, u64>(0))
                    .is_ok()
            );
        }
        store.cancel_queries_with(CancellationToken::new());
        store.close().unwrap();
    }
}

#[test]
fn result_query_populates_grep_columns_filters_matched_and_sorts_by_grep_columns() {
    for session in [false, true] {
        let (mut store, run) = fixture(session, 4, false);

        let mut r0 = response(200);
        r0.response.as_mut().unwrap().body = b"hello <tok>zeta</tok> id=900".to_vec();
        r0.summary.response_bytes = r0.response.as_ref().unwrap().body.len() as u64;
        insert_record(&mut store, run.id, 0, &r0);

        let mut r1 = response(200);
        r1.response.as_mut().unwrap().body =
            b"admin hit <tok>alpha</tok> id=100 echo=payload-1".to_vec();
        r1.summary.response_bytes = r1.response.as_ref().unwrap().body.len() as u64;
        insert_record(&mut store, run.id, 1, &r1);

        let mut r2 = response(200);
        r2.response.as_mut().unwrap().body = b"plain body without markers".to_vec();
        r2.summary.response_bytes = r2.response.as_ref().unwrap().body.len() as u64;
        insert_record(&mut store, run.id, 2, &r2);

        store
            .put_intruder_result(run.id, 3, Err("rejected"))
            .unwrap();
        store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();

        let mut q = query(&store, &run);
        q.limit = 10;
        q.grep_config = IntruderGrepConfig {
            match_expressions: vec!["admin".into(), "re:id=\\d+".into()],
            match_case_sensitive: false,
            extract_rules: vec![
                IntruderGrepExtractRule::delimited("<tok>", "</tok>"),
                IntruderGrepExtractRule::regex(r"id=(\d+)"),
            ],
            grep_payloads: true,
            grep_payloads_case_sensitive: true,
        };

        let page = fetch(&store, &q);
        assert_eq!(page.total, 4);
        assert_eq!(page.rows[0].grep_matches, vec!["re:id=\\d+"]);
        assert_eq!(
            page.rows[0].grep_extracts,
            vec![
                ("<tok>..</tok>".into(), "zeta".into()),
                ("re:id=(\\d+)".into(), "900".into()),
            ]
        );
        assert_eq!(page.rows[0].grep_payload_hits, vec![false]);

        assert_eq!(page.rows[1].grep_matches, vec!["admin", "re:id=\\d+"]);
        assert_eq!(
            page.rows[1].grep_extracts,
            vec![
                ("<tok>..</tok>".into(), "alpha".into()),
                ("re:id=(\\d+)".into(), "100".into()),
            ]
        );
        assert_eq!(page.rows[1].grep_payload_hits, vec![true]);

        assert!(page.rows[2].grep_matches.is_empty());
        assert!(page.rows[2].grep_extracts.is_empty());
        assert_eq!(page.rows[2].grep_payload_hits, vec![false]);

        assert!(page.rows[3].grep_matches.is_empty());
        assert!(page.rows[3].grep_extracts.is_empty());
        assert!(page.rows[3].grep_payload_hits.is_empty());

        q.filter.only_grep_matched = true;
        let filtered_page = fetch(&store, &q);
        assert_eq!(ordinals(&filtered_page), [0, 1]);

        q.filter.only_grep_matched = false;
        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::GrepMatch(0),
            ascending: false,
        };
        assert_eq!(ordinals(&fetch(&store, &q)), [1, 0, 2, 3]);

        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::GrepExtract(0),
            ascending: true,
        };
        assert_eq!(ordinals(&fetch(&store, &q)), [2, 3, 1, 0]);

        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::GrepExtract(0),
            ascending: false,
        };
        assert_eq!(ordinals(&fetch(&store, &q)), [0, 1, 2, 3]);

        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::GrepPayload(0),
            ascending: false,
        };
        assert_eq!(ordinals(&fetch(&store, &q)), [1, 0, 2, 3]);

        store.close().unwrap();
    }
}

#[test]
fn result_query_rejects_wrong_session_generation_and_pre_cancel_without_reads() {
    let (store, run) = fixture(true, 1, false);
    let mut query = query(&store, &run);
    query.project.generation += 1;
    assert!(
        store
            .query_intruder_result_view(
                &query,
                None,
                &[],
                &CancellationToken::new(),
                &CancellationToken::new()
            )
            .is_err()
    );
    query.project.generation -= 1;
    let reads = Arc::new(AtomicUsize::new(0));
    store.connection.authorizer(Some({
        let reads = reads.clone();
        move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Read { .. }) {
                reads.fetch_add(1, Ordering::SeqCst);
            }
            Authorization::Allow
        }
    }));
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        store
            .query_intruder_result_view(&query, None, &[], &cancel, &CancellationToken::new())
            .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    store.close().unwrap();
}

#[test]
fn result_query_sorts_payloads_globally_with_full_blob_ties_and_offpage_selection() {
    let shared_prefix = vec![b'p'; 1200];
    let mut long_b = shared_prefix.clone();
    long_b.push(b'b');
    let mut long_a = shared_prefix;
    long_a.push(b'a');
    for session in [false, true] {
        let spec = IntruderSpec {
            revision: 1,
            draft: Draft {
                revision: 1,
                target: "http://127.0.0.1:9".into(),
                raw: b"GET / HTTP/1.1\r\n\r\nxy".to_vec(),
                version: HttpVersion::Http11,
            },
            attack_type: AttackType::Sniper,
            positions: vec![
                Position { start: 18, end: 19 },
                Position { start: 19, end: 20 },
            ],
            payload_sets: vec![vec![
                long_b.clone(),
                Vec::new(),
                long_a.clone(),
                vec![0xff, 0x00],
            ]],
            max_concurrent_requests: 1,
            request_timeout_ms: 1000,
            update_host_header: true,
            save_to_project: !session,
            baseline_request: true,
            resource_pool: None,
        };
        let mut store = ProjectStore::create(None).unwrap();
        let tab = Uuid::new_v4();
        let mut workspace = store.workspace().unwrap();
        workspace.revision += 1;
        workspace.intruder.active = Some(tab);
        workspace.intruder.tabs.push(crate::IntruderTab {
            id: tab,
            title: "payload sort query".into(),
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
        for ordinal in 0..run.total {
            insert_record(&mut store, run.id, ordinal, &response(200));
        }
        store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
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
        q.limit = 3;
        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::Payload,
            ascending: true,
        };
        q.selected_ordinal = Some(8);
        let first_page = fetch(&store, &q);
        assert_eq!(first_page.total, 9);
        assert_eq!(ordinals(&first_page), [0, 2, 6]);
        assert_eq!(first_page.selected_rank, Some(8));
        assert_eq!(first_page.selected_row.as_ref().unwrap().ordinal, 8);

        q.offset = 3;
        let second_page = fetch(&store, &q);
        assert_eq!(ordinals(&second_page), [3, 7, 1]);
        assert_eq!(second_page.selected_rank, Some(8));

        q.offset = 6;
        let third_page = fetch(&store, &q);
        assert_eq!(ordinals(&third_page), [5, 4, 8]);
        assert_eq!(third_page.selected_rank, Some(8));
        assert!(third_page.selected_row.is_none());

        q.offset = 0;
        q.sort.ascending = false;
        q.selected_ordinal = Some(0);
        let desc_first = fetch(&store, &q);
        assert_eq!(ordinals(&desc_first), [4, 8, 1]);
        assert_eq!(desc_first.selected_rank, Some(8));
        assert_eq!(desc_first.selected_row.as_ref().unwrap().ordinal, 0);
        assert_eq!(bodies.load(Ordering::SeqCst), 0);
        connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        store.close().unwrap();
    }
}

#[test]
fn result_query_filters_responses_by_search_regex_case_negative_and_sorts() {
    for session in [false, true] {
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
            payload_sets: vec![vec![
                b"z".to_vec(),
                b"a".to_vec(),
                b"m".to_vec(),
                b"r".to_vec(),
            ]],
            max_concurrent_requests: 1,
            request_timeout_ms: 1000,
            update_host_header: true,
            save_to_project: !session,
            baseline_request: true,
            resource_pool: None,
        };
        let mut store = ProjectStore::create(None).unwrap();
        let tab = Uuid::new_v4();
        let mut workspace = store.workspace().unwrap();
        workspace.revision += 1;
        workspace.intruder.active = Some(tab);
        workspace.intruder.tabs.push(crate::IntruderTab {
            id: tab,
            title: "search query".into(),
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
        let mut base = response(200);
        base.response.as_mut().unwrap().body = b"baseline-only".to_vec();
        base.summary.response_bytes = base.response.as_ref().unwrap().body.len() as u64;
        insert_record(&mut store, run.id, 0, &base);

        let mut r1 = response(201);
        r1.response.as_mut().unwrap().headers.push(crate::Header {
            name: "X-Token".into(),
            value: b"SecretToken".to_vec(),
        });
        r1.response.as_mut().unwrap().body = b"alpha body".to_vec();
        r1.summary.response_bytes = r1.response.as_ref().unwrap().body.len() as u64;
        insert_record(&mut store, run.id, 1, &r1);

        let mut r2 = response(404);
        r2.response.as_mut().unwrap().body = vec![
            b'b', b'i', b'n', 0xff, 0x00, b'S', b'e', b'c', b'r', b'e', b't', b't', b'o', b'k',
            b'e', b'n',
        ];
        r2.summary.response_bytes = r2.response.as_ref().unwrap().body.len() as u64;
        insert_record(&mut store, run.id, 2, &r2);

        let mut r3_pending = response(200);
        r3_pending.summary.state = FlowState::Pending;
        r3_pending.summary.status = None;
        r3_pending.summary.response_bytes = 0;
        r3_pending.response = None;
        assert!(
            store
                .preflight_intruder_result(run.id, 3, &r3_pending)
                .unwrap()
        );
        store
            .put_intruder_result(run.id, 3, Ok(&r3_pending))
            .unwrap();
        store
            .put_intruder_result(run.id, 4, Err("rejected without response"))
            .unwrap();

        let mut q = query(&store, &run);
        q.limit = 10;
        q.filter.search_term = "secrettoken".into();
        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::Payload,
            ascending: true,
        };
        q.selected_ordinal = Some(1);
        let page = fetch(&store, &q);
        assert_eq!(page.total, 2);
        assert_eq!(ordinals(&page), [2, 1]);
        assert_eq!(page.selected_rank, Some(1));

        q.filter.case_sensitive = true;
        q.filter.search_term = "SecretToken".into();
        let page = fetch(&store, &q);
        assert_eq!(ordinals(&page), [1]);

        q.filter.case_sensitive = false;
        q.filter.regex = true;
        q.filter.search_term = r"bin\xFF\x00".into();
        let page = fetch(&store, &q);
        assert_eq!(ordinals(&page), [2]);

        q.filter.negative_search = true;
        q.sort = IntruderResultSort {
            column: IntruderResultSortColumn::Request,
            ascending: true,
        };
        let page = fetch(&store, &q);
        assert_eq!(ordinals(&page), [0, 1, 3, 4]);

        for invalid in ["(".to_owned(), "(?=a)".to_owned(), "x".repeat(4097)] {
            let mut bad = query(&store, &run);
            bad.filter.search_term = invalid;
            bad.filter.regex = true;
            assert!(
                store
                    .query_intruder_result_view(
                        &bad,
                        None,
                        &[],
                        &CancellationToken::new(),
                        &CancellationToken::new(),
                    )
                    .is_err()
            );
        }
        store.close().unwrap();
    }
}
