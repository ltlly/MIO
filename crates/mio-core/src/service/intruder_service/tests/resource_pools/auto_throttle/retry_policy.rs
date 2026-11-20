use super::*;

#[tokio::test]
async fn matching_429_retries_with_a_frozen_policy_and_preserves_only_the_final_response() {
    assert_one_retry(
        429,
        200,
        IntruderAutoThrottle {
            enabled: true,
            ..IntruderAutoThrottle::default()
        },
    )
    .await;
}

#[tokio::test]
async fn matching_503_uses_the_same_bounded_retry_policy() {
    assert_one_retry(
        503,
        200,
        IntruderAutoThrottle {
            enabled: true,
            status_429: false,
            status_503: true,
            ..IntruderAutoThrottle::default()
        },
    )
    .await;
}

#[tokio::test]
async fn matching_other_status_uses_the_validated_code_snapshot() {
    assert_one_retry(
        200,
        201,
        IntruderAutoThrottle {
            enabled: true,
            status_429: false,
            other_enabled: true,
            other_codes: " 200,200 ".into(),
            ..IntruderAutoThrottle::default()
        },
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn six_real_429_attempts_exhaust_the_helper_and_keep_the_sixth_response_in_one_result() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    enum SocketProgress {
        Received(u8),
        Closed(u8),
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (progress_tx, mut progress) = tokio::sync::mpsc::channel(12);
    let request_count = Arc::new(AtomicUsize::new(0));
    let observed_requests = Arc::clone(&request_count);
    let origin_stop = CancellationToken::new();
    let stop_origin = origin_stop.clone();
    let origin = tokio::spawn(async move {
        let mut closed = Vec::new();
        for attempt in 1_u8..=6 {
            let (mut stream, _) = tokio::select! {
                biased;
                _ = stop_origin.cancelled() => break,
                accepted = listener.accept() => accepted.unwrap(),
            };
            read_get(&mut stream).await;
            observed_requests.fetch_add(1, Ordering::SeqCst);
            progress_tx
                .send(SocketProgress::Received(attempt))
                .await
                .unwrap();
            // Every response differs, so retaining an earlier 429 cannot pass final validation.
            reply(&mut stream, 429, &[attempt, 0, 255]).await;
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            closed.push(attempt);
            progress_tx
                .send(SocketProgress::Closed(attempt))
                .await
                .unwrap();
        }
        (listener, closed)
    });
    let (mut state, _events) = state();
    let mut shared = pool("six real attempts", Some(1));
    shared.automatic_throttling.enabled = true;
    install(&mut state, vec![shared.clone()]);
    let mut input = get_spec(format!("http://{address}"), shared.id, false);
    // Manual virtual advancement is capped below this timeout, so an OS readiness delay cannot
    // accidentally consume a sender deadline. No real request is allowed to wait five minutes.
    input.request_timeout_ms = 300_000;
    let id = admit(&mut state, input.clone());
    let cancel = state.intruder.runs[&id].cancel.clone();
    let guard_stop = CancellationToken::new();
    let stop_guard = guard_stop.clone();
    let timed_out = Arc::new(AtomicBool::new(false));
    let expired = Arc::clone(&timed_out);
    let watchdog_cancel = cancel.clone();
    let clock_guard = tokio::spawn(async move {
        let wall_start = std::time::Instant::now();
        while !stop_guard.is_cancelled() {
            if wall_start.elapsed() >= Duration::from_secs(5) {
                expired.store(true, Ordering::SeqCst);
                watchdog_cancel.cancel();
            }
            // A runnable task disables Tokio's idle auto-advance while real socket I/O settles.
            tokio::task::yield_now().await;
        }
    });
    let virtual_start = PoolInstant::now();
    state.advance_intruder(id).await.unwrap();
    let timing = state.intruder.pool_timings[&shared.id];
    let flow = state.intruder.requests.values().next().unwrap().flow_id;
    let mut received = Vec::new();
    let mut closed_events = Vec::new();
    let mut advance_allowed = false;
    let mut progress_open = true;
    let mut exceeded_virtual_budget = false;
    let completion = {
        let mut joined = Box::pin(state.intruder.jobs.join_next_with_id());
        loop {
            tokio::select! {
                biased;
                result = &mut joined => break result.expect("the logical job remains owned"),
                event = progress.recv(), if progress_open => match event {
                    Some(SocketProgress::Received(attempt)) => {
                        received.push(attempt);
                        advance_allowed = false;
                    }
                    Some(SocketProgress::Closed(attempt)) => {
                        closed_events.push(attempt);
                        advance_allowed = true;
                    }
                    None => progress_open = false,
                },
                _ = tokio::task::yield_now(), if advance_allowed => {
                    if virtual_start.elapsed() >= Duration::from_secs(90) {
                        exceeded_virtual_budget = true;
                        cancel.cancel();
                        advance_allowed = false;
                    } else {
                        // Advance only after the prior real socket reached EOF, pausing again
                        // when the next request arrives. Small virtual steps allow SDK drain
                        // to finish before its retry sleep is registered, without real sleeps.
                        tokio::time::advance(Duration::from_millis(10)).await;
                    }
                },
            }
        }
    };
    state.complete_intruder(completion).await.unwrap();
    state.advance_intruder(id).await.unwrap();
    state.stop_intruder().await.unwrap();
    if request_count.load(Ordering::SeqCst) < 6 {
        origin_stop.cancel();
    }
    let (listener, closed) = origin.await.unwrap();
    while let Ok(event) = progress.try_recv() {
        match event {
            SocketProgress::Received(attempt) => received.push(attempt),
            SocketProgress::Closed(attempt) => closed_events.push(attempt),
        }
    }
    guard_stop.cancel();
    clock_guard.await.unwrap();
    assert!(
        !timed_out.load(Ordering::SeqCst),
        "six small real sockets must complete within the wall-time guard"
    );
    assert!(
        !exceeded_virtual_budget,
        "the bounded helper must finish without approaching an HTTP timeout"
    );
    assert!(
        virtual_start.elapsed() >= Duration::from_secs(31),
        "all five backoffs execute on the virtual clock, without thirty-one seconds of real waiting"
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 6);
    assert_eq!(received, [1, 2, 3, 4, 5, 6]);
    assert_eq!(closed_events, [1, 2, 3, 4, 5, 6]);
    assert_eq!(closed, [1, 2, 3, 4, 5, 6]);
    let page = state.intruder_result_page(id, None, 2).unwrap();
    assert_eq!(page.run.state, IntruderRunState::Completed);
    assert_eq!(
        (page.run.dispatched, page.run.completed, page.rows.len()),
        (1, 1, 1)
    );
    let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
        panic!("six physical attempts still finish one logical flow")
    };
    assert_eq!(summary.id, flow);
    assert_eq!(summary.revision, 2);
    assert_eq!(summary.status, Some(429));
    assert_eq!(
        state.store.read(flow).unwrap().response.unwrap().body,
        [6, 0, 255]
    );
    assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
    let flow_count: u64 = state
        .store
        .test_intruder_connection(id)
        .unwrap()
        .query_row("SELECT count(*) FROM flows", [], |row| row.get(0))
        .unwrap();
    assert_eq!(flow_count, 1);
    assert_eq!(state.intruder.pool_timings[&shared.id], timing);
    assert!(!state.intruder.has_resources());
    assert!(state.intruder.requests.is_empty());
    no_extra(listener);
}
