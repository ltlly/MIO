use super::*;

mod retry_failures;
mod retry_policy;
mod session_recovery;

fn get_spec(target: String, pool: Uuid, session: bool) -> IntruderSpec {
    let mut input = spec(target, 1);
    input.draft.raw = b"GET /auto?q=X HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n".to_vec();
    let offset = input
        .draft
        .raw
        .iter()
        .position(|byte| *byte == b'X')
        .unwrap();
    input.positions = vec![Position {
        start: offset,
        end: offset + 1,
    }];
    input.payload_sets = vec![vec![b"A".to_vec()]];
    input.resource_pool = Some(IntruderPoolSelection::Existing(pool));
    input.save_to_project = !session;
    input
}

async fn read_get(stream: &mut tokio::net::TcpStream) {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 4096);
        head.push(stream.read_u8().await.unwrap());
    }
    assert!(head.starts_with(b"GET /auto?q=A HTTP/1.1\r\n"));
}

async fn reply(stream: &mut tokio::net::TcpStream, status: u16, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
}

async fn assert_one_retry(initial_status: u16, final_status: u16, automatic: IntruderAutoThrottle) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (first_closed_tx, first_closed) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            read_get(&mut first).await;
            reply(&mut first, initial_status, &[0, 255, b'W']).await;
            assert_eq!(first.read(&mut [0; 1]).await.unwrap(), 0);
            first_closed_tx.send(()).unwrap();
            let (mut second, _) = listener.accept().await.unwrap();
            read_get(&mut second).await;
            let retried_at = PoolInstant::now();
            reply(&mut second, final_status, &[0, 254, b'R']).await;
            assert_eq!(second.read(&mut [0; 1]).await.unwrap(), 0);
            (listener, retried_at)
        });
        let (mut state, _events) = state();
        let mut shared = pool("automatic", Some(1));
        shared.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 5000 });
        shared.automatic_throttling = automatic;
        install(&mut state, vec![shared.clone()]);
        let input = get_spec(format!("http://{address}"), shared.id, false);
        let id = admit(&mut state, input.clone());
        state.advance_intruder(id).await.unwrap();
        let timing = state.intruder.pool_timings[&shared.id];
        let flow = state.intruder.requests.values().next().unwrap().flow_id;
        first_closed.await.unwrap();
        assert_eq!(state.intruder.runs[&id].active, 1);
        assert_eq!(
            (state.intruder.jobs.len(), state.intruder.requests.len()),
            (1, 1)
        );
        assert!(
            !state.intruder.pool_has_capacity(&state.intruder.runs[&id]),
            "the logical job keeps its pool slot during backoff"
        );
        assert_eq!(
            state.store.read(flow).unwrap().summary.state,
            FlowState::Pending
        );
        assert!(state.store.read(flow).unwrap().response.is_none());
        shared.automatic_throttling.enabled = false;
        install(&mut state, vec![shared.clone()]);
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        // The already dispatched logical job uses its original status-code snapshot.
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!(
            (page.run.dispatched, page.run.completed, page.rows.len()),
            (1, 1, 1)
        );
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
            panic!("automatic retry keeps the original logical flow")
        };
        assert_eq!(summary.id, flow);
        assert_eq!(summary.revision, 2);
        assert_eq!(summary.status, Some(final_status));
        let final_record = state.store.read(flow).unwrap();
        assert_eq!(final_record.request.method, "GET");
        assert_eq!(final_record.response.as_ref().unwrap().body, [0, 254, b'R']);
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        let flow_count: u64 = state
            .store
            .test_intruder_connection(id)
            .unwrap()
            .query_row("SELECT count(*) FROM flows", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            flow_count, 1,
            "internal responses do not create extra persisted result flows"
        );
        assert_eq!(
            state.intruder.pool_timings[&shared.id], timing,
            "retry does not charge or move the logical first-dispatch delay"
        );
        assert!(!state.intruder.has_resources());
        let (listener, retried_at) = origin.await.unwrap();
        assert!(
            retried_at >= timing.last_send.unwrap() + Duration::from_secs(1),
            "the first automatic retry waits at least one second"
        );
        assert!(
            retried_at < timing.not_before.unwrap(),
            "the one-second automatic retry bypasses the configured five-second logical delay"
        );
        no_extra(listener);
    })
    .await
    .expect(
        "one explicit automatic retry keeps one logical result and normally joins both sockets",
    );
}
