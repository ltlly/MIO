use super::*;

#[tokio::test]
async fn fixed_and_increment_timers_gate_real_shared_pool_sends() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for policy in [
            IntruderPoolDelay::Fixed { milliseconds: 40 },
            IntruderPoolDelay::Increment {
                initial_ms: 20,
                increment_ms: 20,
            },
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let mut arrivals = Vec::new();
                for expected in *b"AB" {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    assert_eq!(request(&mut stream).await, expected);
                    arrivals.push(PoolInstant::now());
                    respond(&mut stream).await;
                }
                (listener, arrivals)
            });
            let (mut state, _events) = state();
            let mut shared = pool("timed shared", Some(1));
            shared.delay = Some(policy);
            install(&mut state, vec![shared.clone()]);
            let mut input = spec(format!("http://{address}"), 1);
            input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
            input.payload_sets = vec![vec![vec![b'A']]];
            let first = admit(&mut state, input.clone());
            input.payload_sets = vec![vec![vec![b'B']]];
            let second = admit(&mut state, input);
            state.advance_intruder(first).await.unwrap();
            let first_timing = state.intruder.pool_timings[&shared.id];
            assert_eq!(first_timing.sent_count, 1);
            assert_eq!(
                first_timing.not_before.unwrap() - first_timing.last_send.unwrap(),
                Duration::from_millis(40)
            );
            while state.intruder_run(second).unwrap().dispatched == 0 {
                let signal = state.intruder.next_signal(&state.events, false).await;
                state.intruder_signal(signal).await.unwrap();
            }
            let second_timing = state.intruder.pool_timings[&shared.id];
            assert_eq!(second_timing.sent_count, 2);
            assert!(second_timing.last_send.unwrap() >= first_timing.not_before.unwrap());
            let expected = if matches!(policy, IntruderPoolDelay::Fixed { .. }) {
                40
            } else {
                60
            };
            assert_eq!(
                second_timing.not_before.unwrap() - second_timing.last_send.unwrap(),
                Duration::from_millis(expected)
            );
            finish_next(&mut state).await;
            state.advance_intruder(second).await.unwrap();
            assert_eq!(
                state.intruder.pool_timings[&shared.id], second_timing,
                "response completion does not move the send anchor or counter"
            );
            assert_eq!(
                state.intruder_run(first).unwrap().state,
                IntruderRunState::Completed
            );
            assert!(!state.intruder.has_resources());
            let (listener, arrivals) = origin.await.unwrap();
            assert!(arrivals[1] >= first_timing.not_before.unwrap());
            no_extra(listener);
        }
    })
    .await
    .expect("two small real sends per confirmed policy share one timer and normally join");
}
