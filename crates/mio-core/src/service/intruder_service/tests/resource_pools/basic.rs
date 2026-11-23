use super::*;

#[tokio::test]
async fn unlimited_shared_pools_preserve_global_request_and_active_run_resource_limits() {
    // Exactly the internal boundary: MAX_REQUESTS_IN_FLIGHT one-byte loopback requests, no response bodies.
    tokio::time::timeout(Duration::from_secs(20), async {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(512).unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (accepted_tx, mut accepted_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let origin = tokio::spawn(async move {
            let mut readers = Vec::with_capacity(MAX_REQUESTS_IN_FLIGHT);
            for _ in 0..MAX_REQUESTS_IN_FLIGHT {
                let (mut stream, _) = listener.accept().await.unwrap();
                let accepted_tx = accepted_tx.clone();
                readers.push(tokio::spawn(async move {
                    assert_eq!(request(&mut stream).await, 255);
                    let _ = accepted_tx.send(());
                    stream
                }));
            }
            let mut streams = Vec::with_capacity(MAX_REQUESTS_IN_FLIGHT);
            for reader in readers {
                streams.push(reader.await.unwrap());
            }
            seen_tx.send(()).unwrap();
            for mut stream in streams {
                assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            }
            listener
        });
        let (mut state, mut events) = state();
        let drain = tokio::spawn(async move {
            while events.recv().await.is_some() {}
        });
        let pools = [
            pool("first unlimited", None),
            pool("second unlimited", None),
        ];
        let catalog = install(&mut state, pools.to_vec());
        let per_run = MAX_REQUESTS_IN_FLIGHT / MAX_RUNS;
        let mut ids = Vec::new();
        for index in 0..MAX_RUNS {
            let mut input = spec(
                format!("http://{address}"),
                if index == 0 { per_run + 1 } else { per_run },
            );
            input.request_timeout_ms = 15_000;
            input.max_concurrent_requests = 1;
            input.resource_pool = Some(IntruderPoolSelection::Existing(pools[index % 2].id));
            ids.push(admit(&mut state, input));
        }
        let mut overflow = spec(format!("http://{address}"), 1);
        overflow.resource_pool = Some(IntruderPoolSelection::Create(pool(
            "must not be created",
            None,
        )));
        let tab = bind(&mut state.store, &overflow);
        assert!(
            state
                .admit_intruder(
                    state.project,
                    Uuid::new_v4(),
                    tab,
                    overflow,
                    CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(state.intruder.resource_pools, catalog);
        assert_eq!(state.store.intruder_resource_pools().unwrap(), catalog);
        for id in &ids {
            for _ in 0..per_run {
                state.advance_intruder(*id).await.unwrap();
            }
            for _ in 0..per_run {
                accepted_rx.recv().await.unwrap();
            }
        }
        assert_eq!(state.intruder.requests.len(), MAX_REQUESTS_IN_FLIGHT);
        assert_eq!(state.intruder.jobs.len(), MAX_REQUESTS_IN_FLIGHT);
        assert!(state.intruder.advance_candidate().is_none());
        state.advance_intruder(ids[0]).await.unwrap();
        assert_eq!(
            state.intruder_run(ids[0]).unwrap().dispatched,
            per_run as u64,
            "neither unlimited pool may exceed the shared global boundary"
        );
        seen.await.unwrap();
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        for id in ids {
            let stopped = state.intruder_run(id).unwrap();
            assert_eq!(stopped.state, IntruderRunState::Stopped);
            assert_eq!((stopped.dispatched, stopped.completed), (per_run as u64, per_run as u64));
        }
        no_extra(origin.await.unwrap());
        drain.abort();
    })
    .await
    .expect("the sixteen-socket/eight-run resource boundary cancels and joins normally");
}

#[tokio::test]
async fn shared_pool_replaces_old_run_cap_and_releases_capacity_after_join() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let mut first = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(request(&mut stream).await, 255);
                first.push(stream);
            }
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            for mut stream in first {
                respond(&mut stream).await;
            }
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            respond(&mut stream).await;
            listener
        });
        let (mut state, mut events) = state();
        let drain = tokio::spawn(async move {
            while events.recv().await.is_some() {}
        });
        let shared = pool("shared", Some(2));
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{address}"), 2);
        input.max_concurrent_requests = 1;
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let first = admit(&mut state, input.clone());
        input.payload_sets[0].truncate(1);
        let second = admit(&mut state, input);
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(second).await.unwrap();
        assert_eq!(
            state.intruder.runs[&first].active, 2,
            "explicit shared pool replaces the old per-run cap of one"
        );
        assert_eq!(state.intruder.requests.len(), 2);
        assert_eq!(
            state.intruder_run(second).unwrap().dispatched,
            0,
            "the other run shares the same two slots"
        );
        assert!(
            state
                .intruder
                .requests
                .values()
                .all(|request| request.resource_pool == Some(shared.id))
        );
        seen.await.unwrap();
        let mut smaller = shared.clone();
        smaller.max_concurrent_requests = Some(1);
        install(&mut state, vec![smaller]);
        assert_eq!(
            state.intruder.runs[&first].active, 2,
            "lowering the cap does not cancel admitted sockets"
        );
        assert!(!state.intruder.runs[&first].cancel.is_cancelled());
        state.advance_intruder(second).await.unwrap();
        assert_eq!(state.intruder_run(second).unwrap().dispatched, 0);
        release.send(()).unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(second).await.unwrap();
        assert_eq!(
            state.intruder_run(second).unwrap().dispatched,
            0,
            "a single joined request leaves the remaining slot occupied"
        );
        finish_next(&mut state).await;
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(second).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(second).await.unwrap();
        assert!(!state.intruder.has_resources());
        for id in [first, second] {
            assert_eq!(
                state.intruder_run(id).unwrap().state,
                IntruderRunState::Completed
            );
        }
        no_extra(origin.await.unwrap());
        drain.abort();
    })
    .await
    .expect("three tiny requests share one pool, a live cap edit and normal joins");
}
