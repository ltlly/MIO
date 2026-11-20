use super::*;

const POST_BODY: &[u8; 5] = &[0, 255, b'P', 0, 128];
const FINAL_BODY: &[u8; 3] = &[0, 254, b'R'];

fn post_spec(target: String, pool_id: Uuid) -> IntruderSpec {
    let mut input = spec(target, 1);
    input.draft.raw = b"POST /auto-post HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 5\r\nX-Mio-Fixture: binary-repeat\r\n\r\nABCDE".to_vec();
    input.positions = vec![Position {
        start: input.draft.raw.len() - POST_BODY.len(),
        end: input.draft.raw.len(),
    }];
    input.payload_sets = vec![vec![POST_BODY.to_vec()]];
    input.resource_pool = Some(IntruderPoolSelection::Existing(pool_id));
    input
}

async fn read_post(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 4096, "bounded synthetic POST head");
        bytes.push(stream.read_u8().await.unwrap());
    }
    assert!(bytes.starts_with(b"POST /auto-post HTTP/1.1\r\n"));
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .to_ascii_lowercase()
            .contains("\r\ncontent-length: 5\r\n")
    );
    let mut body = [0; POST_BODY.len()];
    stream.read_exact(&mut body).await.unwrap();
    assert_eq!(&body, POST_BODY);
    bytes.extend_from_slice(&body);
    bytes
}

#[tokio::test]
async fn complete_429_retries_an_identical_binary_post_and_keeps_one_final_response() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let first_request = read_post(&mut first).await;
            let first_received = PoolInstant::now();
            first.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\x00\xffW").await.unwrap();
            assert_eq!(first.read(&mut [0; 1]).await.unwrap(), 0);
            let (mut second, _) = listener.accept().await.unwrap();
            let second_request = read_post(&mut second).await;
            let second_received = PoolInstant::now();
            assert_eq!(second_request, first_request, "automatic retry must not change any POST request bytes");
            second.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n").await.unwrap();
            second.write_all(FINAL_BODY).await.unwrap();
            assert_eq!(second.read(&mut [0; 1]).await.unwrap(), 0);
            assert!(second_received >= first_received + Duration::from_secs(1));
            listener
        });
        let (mut state, _events) = state();
        let mut shared = pool("binary POST retry", Some(1));
        shared.automatic_throttling.enabled = true;
        install(&mut state, vec![shared.clone()]);
        let input = post_spec(target, shared.id);
        let id = admit(&mut state, input.clone());
        state.advance_intruder(id).await.unwrap();
        let flow_id = state.intruder.requests.values().next().unwrap().flow_id;
        finish_next(&mut state).await;
        state.advance_intruder(id).await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!((page.run.dispatched, page.run.completed, page.rows.len()), (1, 1, 1));
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else { panic!("POST retry must keep one logical flow"); };
        assert_eq!((summary.id, summary.revision, summary.status), (flow_id, 2, Some(200)));
        assert_eq!(summary.state, FlowState::Complete);
        let record = state.store.read(flow_id).unwrap();
        assert_eq!(record.request.method, "POST");
        assert_eq!(record.request.body.as_slice(), POST_BODY.as_slice());
        assert_eq!(record.response.as_ref().unwrap().body.as_slice(), FINAL_BODY.as_slice());
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        let count: usize = state.store.test_intruder_connection(id).unwrap().query_row("SELECT count(*) FROM flows", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        no_extra(origin.await.unwrap());
    }).await.expect("one complete 429 retries an identical binary POST once within five seconds");
}

#[tokio::test]
async fn truncated_429_response_does_not_retry_a_post_with_an_unknown_final_result() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_post(&mut stream).await;
            stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 3\r\nConnection: close\r\n\r\nX").await.unwrap();
            stream.shutdown().await.unwrap();
            drop(stream);
            listener
        });
        let (mut state, _events) = state();
        let mut shared = pool("truncated POST response", Some(1));
        shared.automatic_throttling.enabled = true;
        install(&mut state, vec![shared.clone()]);
        let id = admit(&mut state, post_spec(target, shared.id));
        state.advance_intruder(id).await.unwrap();
        let listener = origin.await.unwrap();
        let completion = tokio::select! {
            completion = state.intruder.jobs.join_next_with_id() => completion.unwrap(),
            _ = listener.accept() => panic!("a 429 header with an incomplete body must not cause a second POST socket"),
        };
        state.complete_intruder(completion).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!((page.run.dispatched, page.run.completed, page.rows.len()), (1, 1, 1));
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else { panic!("unknown POST outcome retains its request flow"); };
        assert!(matches!(summary.state, FlowState::Failed(_)));
        assert_eq!(summary.status, None);
        let record = state.store.read(summary.id).unwrap();
        assert_eq!(record.request.method, "POST");
        assert_eq!(record.request.body.as_slice(), POST_BODY.as_slice());
        assert!(record.response.is_none());
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        no_extra(listener);
    }).await.expect("an incomplete 429 body fails once without replaying the POST within three seconds");
}
