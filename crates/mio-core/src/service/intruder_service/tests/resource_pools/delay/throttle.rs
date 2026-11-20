use super::*;

#[tokio::test]
async fn delayed_wait_and_failed_admission_do_not_advance_the_shared_send_clock() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            respond(&mut stream).await;
            listener
        });
        let (mut state, _events) = state();
        let mut shared = pool("long increment", Some(1));
        shared.delay = Some(IntruderPoolDelay::Increment {
            initial_ms: 300_000,
            increment_ms: 300_000,
        });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{address}"), 1);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let first = admit(&mut state, input.clone());
        state.advance_intruder(first).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(first).await.unwrap();
        let timing = state.intruder.pool_timings[&shared.id];
        let second = admit(&mut state, input);
        for _ in 0..2 {
            state.advance_intruder(second).await.unwrap();
        }
        assert_eq!(state.intruder_run(second).unwrap().dispatched, 0);
        assert!(state.intruder.jobs.is_empty() && state.intruder.requests.is_empty());
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        shared.delay = None;
        install(&mut state, vec![shared.clone()]);
        let disabled = state.intruder.pool_timings[&shared.id];
        assert_eq!(
            (disabled.last_send, disabled.sent_count, disabled.not_before),
            (timing.last_send, 1, None)
        );
        state.store.reject_writes(true);
        assert!(state.advance_intruder(second).await.is_err());
        assert_eq!(state.intruder.pool_timings[&shared.id], disabled);
        assert!(state.stop_intruder().await.is_err());
        state.store.reject_writes(false);
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        save(&mut state, Some(directory.path().join("timed-unsent.mio")))
            .await
            .unwrap();
        assert_eq!(
            state.intruder.pool_timings[&shared.id], disabled,
            "explicit recovery Save cannot count or resend the failed admission"
        );
        assert!(!state.intruder.has_resources());
        assert_eq!(state.unsaved_count(), 0);
        no_extra(origin.await.unwrap());
    })
    .await
    .expect("one sent request and a refused admission preserve the exact shared clock");
}

#[tokio::test(start_paused = true)]
async fn an_expired_pool_timer_still_waits_for_event_capacity_and_cancellation_wins() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, mut events) = state();
        let mut shared = pool("blocked timer", None);
        shared.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 100 });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 1);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let id = admit(&mut state, input);
        let cancel = state.intruder.runs[&id].cancel.clone();
        let now = PoolInstant::now();
        state.intruder.pool_timings.insert(
            shared.id,
            PoolTiming::default().after_send(now, shared.delay).unwrap(),
        );
        state.intruder.queried();
        while events.try_recv().is_ok() {}
        for _ in 0..events.max_capacity() {
            state
                .events
                .try_send(Event::IntruderChanged {
                    project: state.project,
                })
                .unwrap();
        }
        let mut signal = Box::pin(state.intruder.next_signal(&state.events, false));
        std::future::poll_fn(|cx| {
            assert!(signal.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        tokio::time::advance(Duration::from_millis(100)).await;
        // Poll through the existing yield-to-commands stage before asserting the capacity wait.
        std::future::poll_fn(|cx| {
            assert!(signal.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        std::future::poll_fn(|cx| {
            assert!(
                signal.as_mut().poll(cx).is_pending(),
                "an expired timer cannot bypass the full event queue"
            );
            Poll::Ready(())
        })
        .await;
        cancel.cancel();
        let signal = signal.await;
        assert!(matches!(signal, Signal::Advance(run) if run == id));
        state.intruder_signal(signal).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert_eq!(
            state.intruder_run(id).unwrap().state,
            IntruderRunState::Stopped
        );
        assert_eq!(state.intruder_run(id).unwrap().dispatched, 0);
        assert!(state.intruder.jobs.is_empty() && state.intruder.requests.is_empty());
        assert_eq!(state.intruder.pool_timings[&shared.id].sent_count, 1);
        assert_eq!(events.len(), events.max_capacity());
        no_extra(listener);
    })
    .await
    .expect("virtual-time expiry still honors event capacity and bounded cancellation");
}
