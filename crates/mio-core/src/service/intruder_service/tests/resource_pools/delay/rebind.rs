use super::*;

#[tokio::test]
async fn timing_overflow_refuses_catalog_commit_and_unsent_dispatch_without_repeating_a_timer() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("overflow", None);
        let before = install(&mut state, vec![shared.clone()]);
        let timing = PoolTiming {
            last_send: Some(PoolInstant::now()),
            sent_count: u64::MAX,
            not_before: None,
        };
        state.intruder.pool_timings.insert(shared.id, timing);
        shared.delay = Some(IntruderPoolDelay::Increment {
            initial_ms: 1,
            increment_ms: 1,
        });
        assert!(
            state
                .set_intruder_resource_pools(
                    state.project,
                    Uuid::new_v4(),
                    before.revision,
                    vec![shared.clone()],
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(state.intruder.resource_pools, before);
        assert_eq!(state.store.intruder_resource_pools().unwrap(), before);
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 1);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let id = admit(&mut state, input);
        assert!(state.advance_intruder(id).await.is_err());
        assert_eq!(
            state.intruder_run(id).unwrap().state,
            IntruderRunState::Stopping
        );
        assert_eq!(state.intruder_run(id).unwrap().dispatched, 0);
        state.advance_intruder(id).await.unwrap();
        assert!(matches!(
            state.intruder_run(id).unwrap().state,
            IntruderRunState::Failed(_)
        ));
        assert!(state.intruder.advance_candidate().is_none());
        assert!(!state.intruder.has_resources());
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        no_extra(listener);
    })
    .await
    .expect(
        "overflow is rejected before catalog write or socket admission, then normally reclaimed",
    );
}

#[tokio::test]
async fn local_proxy_validation_rejection_does_not_consume_a_pool_send_or_deadline() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("local rejection", Some(1));
        shared.delay = Some(IntruderPoolDelay::Increment { initial_ms: 100, increment_ms: 100 });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 1);
        input.draft.raw = b"CONNECT / HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
        input.positions = vec![Position { start: input.draft.raw.len() - 1, end: input.draft.raw.len() }];
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        assert_eq!(parse_request(&input.draft.raw, &input.draft.target, input.draft.version).unwrap().method, "CONNECT", "this reproduces the core/proxy validation boundary");
        let expected_request = parse_request(
            b"CONNECT / HTTP/1.1\r\nContent-Length: 1\r\n\r\n\xff",
            &input.draft.target,
            input.draft.version,
        ).unwrap();
        let id = admit(&mut state, input);
        let before = state.intruder.pool_timings[&shared.id];
        state.advance_intruder(id).await.unwrap();
        while !state.intruder.jobs.is_empty() { finish_next(&mut state).await; }
        state.advance_intruder(id).await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!((page.run.dispatched, page.run.completed, page.rows.len()), (1, 1, 1));
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
            panic!("proxy validation rejection must retain the generated request as a readable Flow");
        };
        assert!(matches!(&summary.state, FlowState::Failed(message) if message.contains("CONNECT")));
        let stored = state.store.read(summary.id).unwrap();
        assert_eq!(&stored.summary, summary);
        assert_eq!(stored.request, expected_request);
        assert!(stored.response.is_none());
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        no_extra(listener);
        assert_eq!(state.intruder.pool_timings[&shared.id], before, "a locally rejected message never entered a network attempt and must not charge the pool");
    }).await.expect("CONNECT is rejected locally, accounted as a result, and opens no socket");
}
