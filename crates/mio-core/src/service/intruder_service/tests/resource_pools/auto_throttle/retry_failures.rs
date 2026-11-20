use super::*;

#[tokio::test]
async fn disabled_or_unselected_statuses_finish_without_automatic_retry() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for (status, automatic) in [
            (429, IntruderAutoThrottle::default()),
            (
                429,
                IntruderAutoThrottle {
                    enabled: true,
                    status_429: false,
                    status_503: true,
                    ..IntruderAutoThrottle::default()
                },
            ),
            (
                404,
                IntruderAutoThrottle {
                    enabled: true,
                    ..IntruderAutoThrottle::default()
                },
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_get(&mut stream).await;
                reply(&mut stream, status, b"one").await;
                listener
            });
            let (mut state, _events) = state();
            let mut shared = pool("no match", Some(1));
            shared.automatic_throttling = automatic;
            install(&mut state, vec![shared.clone()]);
            let id = admit(
                &mut state,
                get_spec(format!("http://{address}"), shared.id, false),
            );
            state.advance_intruder(id).await.unwrap();
            finish_next(&mut state).await;
            state.advance_intruder(id).await.unwrap();
            let page = state.intruder_result_page(id, None, 2).unwrap();
            let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
                panic!("the first response remains inspectable")
            };
            assert_eq!(summary.status, Some(status));
            assert_eq!(
                state.store.read(summary.id).unwrap().response.unwrap().body,
                b"one"
            );
            assert!(!state.intruder.has_resources());
            no_extra(origin.await.unwrap());
        }
    })
    .await
    .expect("disabled and unselected status codes finish their single real request");
}

#[tokio::test]
async fn cancellation_during_automatic_wait_stops_without_a_second_socket() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (closed_tx, closed) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_get(&mut stream).await;
            reply(&mut stream, 429, b"wait").await;
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            closed_tx.send(()).unwrap();
            listener
        });
        let (mut state, _events) = state();
        let mut shared = pool("cancel retry", Some(1));
        shared.automatic_throttling.enabled = true;
        install(&mut state, vec![shared.clone()]);
        let id = admit(
            &mut state,
            get_spec(format!("http://{address}"), shared.id, false),
        );
        state.advance_intruder(id).await.unwrap();
        let timing = state.intruder.pool_timings[&shared.id];
        closed.await.unwrap();
        assert_eq!(
            (state.intruder.jobs.len(), state.intruder.requests.len()),
            (1, 1)
        );
        state.request_intruder_stop(state.project, id).unwrap();
        state.stop_intruder().await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Stopped);
        assert_eq!(
            (page.run.dispatched, page.run.completed, page.rows.len()),
            (1, 1, 1)
        );
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
            panic!("cancelled logical request keeps its flow")
        };
        assert_eq!(summary.state, FlowState::Cancelled);
        assert!(
            state.store.read(summary.id).unwrap().response.is_none(),
            "an intermediate 429 is not misrepresented as the cancelled final response"
        );
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        assert!(!state.intruder.has_resources());
        no_extra(origin.await.unwrap());
    })
    .await
    .expect("cancellation wakes the automatic wait and joins without retransmission");
}

#[tokio::test]
async fn unknown_network_failure_and_timeout_do_not_trigger_status_based_retries() {
    tokio::time::timeout(Duration::from_secs(3), async {
        for timeout in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_get(&mut stream).await;
                // No HTTP response exists; the result is unknown, not an automatic retry status.
                if timeout {
                    assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
                }
                drop(stream);
                listener
            });
            let (mut state, _events) = state();
            let mut shared = pool("network failure", Some(1));
            shared.automatic_throttling.enabled = true;
            install(&mut state, vec![shared.clone()]);
            let mut input = get_spec(format!("http://{address}"), shared.id, false);
            input.request_timeout_ms = 100;
            let id = admit(&mut state, input);
            state.advance_intruder(id).await.unwrap();
            finish_next(&mut state).await;
            state.advance_intruder(id).await.unwrap();
            let page = state.intruder_result_page(id, None, 2).unwrap();
            let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
                panic!("unknown failure keeps the sent request")
            };
            assert!(matches!(summary.state, FlowState::Failed(_)));
            if timeout {
                assert!(
                    matches!(&summary.state, FlowState::Failed(error) if error.contains("期限"))
                );
            }
            assert_eq!(summary.status, None);
            assert!(state.store.read(summary.id).unwrap().response.is_none());
            assert!(!state.intruder.has_resources());
            no_extra(origin.await.unwrap());
        }
    })
    .await
    .expect("unknown network failure and timeout each return once without status-based retry");
}
