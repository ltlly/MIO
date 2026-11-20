use super::*;

#[tokio::test]
async fn random_cached_deadline_survives_unrelated_edits_start_capacity_and_failed_pending() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("cached random", Some(2));
        shared.delay = Some(IntruderPoolDelay::Random { milliseconds: 100 });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 2);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let first = admit(&mut state, input.clone());
        let sent = PoolInstant::now() - Duration::from_secs(1);
        // An in-range sub-millisecond marker distinguishes retention from every possible
        // integer-ms random re-sample, without a probabilistic equality/distribution test.
        let cached = PoolTiming {
            last_send: Some(sent),
            sent_count: 1,
            not_before: Some(sent + Duration::from_millis(100) + Duration::from_nanos(1)),
        };
        state.intruder.pool_timings.insert(shared.id, cached);
        let _second = admit(&mut state, input);
        assert_eq!(state.intruder.pool_timings[&shared.id], cached);
        for _ in 0..2 {
            let _ = state.intruder.dispatch_candidate_at(PoolInstant::now());
            state
                .intruder
                .check_pool_send(&state.intruder.runs[&first], PoolInstant::now())
                .unwrap();
        }
        shared.name = "renamed random".into();
        shared.max_concurrent_requests = Some(1);
        shared.automatic_throttling.enabled = true;
        install(&mut state, vec![shared.clone()]);
        assert_eq!(
            state.intruder.pool_timings[&shared.id], cached,
            "name/cap/automatic changes do not re-sample the same delay policy"
        );
        state
            .intruder
            .runs
            .get_mut(&first)
            .unwrap()
            .waiting_for_capacity = true;
        state.advance_intruder(first).await.unwrap();
        assert_eq!(state.intruder_run(first).unwrap().dispatched, 0);
        assert_eq!(state.intruder.pool_timings[&shared.id], cached);
        state
            .intruder
            .runs
            .get_mut(&first)
            .unwrap()
            .waiting_for_capacity = false;
        state.store.reject_writes(true);
        assert!(state.advance_intruder(first).await.is_err());
        assert_eq!(
            state.intruder.pool_timings[&shared.id], cached,
            "failed Pending must not replace the sampled deadline"
        );
        assert!(state.intruder.jobs.is_empty() && state.intruder.requests.is_empty());
        assert!(state.stop_intruder().await.is_err());
        state.store.reject_writes(false);
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        save(&mut state, Some(directory.path().join("random-unsent.mio")))
            .await
            .unwrap();
        assert_eq!(state.intruder.pool_timings[&shared.id], cached);
        shared.delay = Some(IntruderPoolDelay::Random { milliseconds: 101 });
        install(&mut state, vec![shared.clone()]);
        let changed = state.intruder.pool_timings[&shared.id];
        assert_eq!((changed.last_send, changed.sent_count), (Some(sent), 1));
        assert_ne!(changed.not_before, cached.not_before);
        let interval = changed.not_before.unwrap() - sent;
        assert!(interval >= Duration::from_millis(50) && interval <= Duration::from_millis(151));
        assert_eq!(interval.subsec_nanos() % 1_000_000, 0);
        assert!(!state.intruder.has_resources());
        no_extra(listener);
    })
    .await
    .expect("cached Random timing changes only for a committed policy change or actual dispatch");
}

#[tokio::test]
async fn a_random_delayed_pool_does_not_block_a_zero_random_pool() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            for expected in *b"AB" {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(request(&mut stream).await, expected);
                respond(&mut stream).await;
            }
            listener
        });
        let (mut state, _events) = state();
        let mut delayed = pool("delayed random", Some(1));
        delayed.delay = Some(IntruderPoolDelay::Random {
            milliseconds: 300_000,
        });
        let mut immediate = pool("zero random", Some(1));
        immediate.delay = Some(IntruderPoolDelay::Random { milliseconds: 0 });
        install(&mut state, vec![delayed.clone(), immediate.clone()]);
        let mut input = spec(format!("http://{address}"), 2);
        input.resource_pool = Some(IntruderPoolSelection::Existing(delayed.id));
        input.payload_sets = vec![vec![b"A".to_vec(), b"A".to_vec()]];
        let first = admit(&mut state, input.clone());
        input.resource_pool = Some(IntruderPoolSelection::Existing(immediate.id));
        input.payload_sets = vec![vec![b"B".to_vec()]];
        let second = admit(&mut state, input);
        state.advance_intruder(first).await.unwrap();
        finish_next(&mut state).await;
        let timing = state.intruder.pool_timings[&delayed.id];
        let interval = timing.not_before.unwrap() - timing.last_send.unwrap();
        assert!(
            interval >= Duration::from_millis(150_000)
                && interval <= Duration::from_millis(450_000)
        );
        assert_eq!(interval.subsec_nanos() % 1_000_000, 0);
        state.advance_intruder(first).await.unwrap();
        assert_eq!(state.intruder_run(first).unwrap().dispatched, 1);
        while state.intruder_run(second).unwrap().dispatched == 0 {
            let signal = state.intruder.next_signal(&state.events, false).await;
            state.intruder_signal(signal).await.unwrap();
        }
        finish_next(&mut state).await;
        state.advance_intruder(second).await.unwrap();
        assert_eq!(state.intruder.pool_timings[&delayed.id], timing);
        let zero = state.intruder.pool_timings[&immediate.id];
        assert_eq!(zero.not_before, zero.last_send);
        assert_eq!((timing.sent_count, zero.sent_count), (1, 1));
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
        no_extra(origin.await.unwrap());
    })
    .await
    .expect(
        "two tiny real sockets verify independent random pools without waiting on the long timer",
    );
}
