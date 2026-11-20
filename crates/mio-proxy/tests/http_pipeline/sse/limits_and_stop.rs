use super::*;
#[tokio::test]
async fn overlimit_sse_preserves_a_bounded_prefix_but_fails_the_actual_stream() {
    for h2 in [false, true] {
        let mut origin = SseOrigin::start(h2.then_some(true), None).await;
        let limit = FIRST.len() + 2;
        let (mut proxy, ca, mut events) =
            start_proxy(&origin, |config| config.max_body_bytes = limit, 64).await;
        let mut client = SseClient::start(proxy.address(), origin.url(), ca, h2, h2);
        assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
        origin.send(FIRST).await;
        assert!(matches!(
            client.next().await,
            StreamEvent::Head { status: 200, .. }
        ));
        client.bytes(FIRST).await;
        origin.send(SECOND).await;
        let mut extra = Vec::new();
        loop {
            match client.next().await {
                StreamEvent::Data(bytes) => extra.extend(bytes),
                StreamEvent::End(Err(_)) => break,
                event => panic!("body overflow must not fake a clean EOF: {event:?}"),
            }
        }
        assert!(extra.len() <= 2);
        assert_eq!(extra, SECOND[..extra.len()]);
        let record = terminal(&mut events).await;
        assert!(
            matches!(record.summary.state, FlowState::Failed(ref error) if error.contains("上限") && error.contains(&limit.to_string()))
        );
        assert!(record.summary.response_bytes > limit as u64);
        assert!(record.summary.response_bytes <= (FIRST.len() + SECOND.len()) as u64);
        assert_eq!(
            record.response.unwrap().body,
            [FIRST, &SECOND[..2]].concat()
        );
        client.finish().await;
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn source_sse_error_preserves_partial_bytes_and_reports_failure_not_eof() {
    for h2 in [false, true] {
        let mut origin = SseOrigin::start(h2.then_some(true), None).await;
        let (mut proxy, ca, mut events) = start_proxy(&origin, |_| {}, 64).await;
        let mut client = SseClient::start(proxy.address(), origin.url(), ca, h2, h2);
        assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
        origin.send(FIRST).await;
        assert!(matches!(
            client.next().await,
            StreamEvent::Head { status: 200, .. }
        ));
        client.bytes(FIRST).await;
        origin
            .frames
            .as_ref()
            .unwrap()
            .send(Err(std::io::Error::other("fixture source stream failed")))
            .await
            .unwrap();
        assert!(matches!(client.next().await, StreamEvent::End(Err(_))));
        let record = terminal(&mut events).await;
        assert!(matches!(record.summary.state, FlowState::Failed(_)));
        assert_eq!(record.summary.response_bytes, FIRST.len() as u64);
        assert_eq!(record.response.unwrap().body, FIRST);
        client.finish().await;
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn sse_trailers_preserve_duplicates_at_client_and_in_the_capture() {
    for h2 in [false, true] {
        let mut origin = SseOrigin::start(h2.then_some(true), None).await;
        let (mut proxy, ca, mut events) = start_proxy(&origin, |_| {}, 64).await;
        let mut client = SseClient::start(proxy.address(), origin.url(), ca, h2, h2);
        assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
        origin.send(FIRST).await;
        assert!(matches!(
            client.next().await,
            StreamEvent::Head { status: 200, .. }
        ));
        client.bytes(FIRST).await;
        let mut trailers = hyper::HeaderMap::new();
        trailers.append("x-stream-end", "one".parse().unwrap());
        trailers.append("x-stream-end", "two".parse().unwrap());
        origin
            .frames
            .as_ref()
            .unwrap()
            .send(Ok(HttpFrame::trailers(trailers)))
            .await
            .unwrap();
        drop(origin.frames.take());
        match client.next().await {
            StreamEvent::Trailers(trailers) => assert_eq!(
                trailers
                    .get_all("x-stream-end")
                    .iter()
                    .map(|value| value.as_bytes())
                    .collect::<Vec<_>>(),
                [b"one".as_slice(), b"two".as_slice()]
            ),
            event => panic!("expected preserved SSE trailers, got {event:?}"),
        }
        assert!(matches!(client.next().await, StreamEvent::End(Ok(()))));
        let record = terminal(&mut events).await;
        assert_eq!(record.summary.state, FlowState::Complete);
        let response = record.response.unwrap();
        assert_eq!(response.body, FIRST);
        assert_eq!(
            response.trailers,
            [
                Header {
                    name: "x-stream-end".into(),
                    value: b"one".to_vec()
                },
                Header {
                    name: "x-stream-end".into(),
                    value: b"two".to_vec()
                }
            ]
        );
        client.finish().await;
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn stopping_sse_with_a_full_event_queue_recovers_the_partial_terminal_record() {
    let mut origin = SseOrigin::start(None, None).await;
    let (mut proxy, ca, mut events) =
        start_proxy(&origin, |config| config.max_connections = 1, 1).await;
    let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    let initial = match events.recv().await.unwrap() {
        ProxyEvent::Captured { record, .. } => record,
        event => panic!("expected initial History record, got {event:?}"),
    };
    origin.send(FIRST).await;
    assert!(matches!(
        client.next().await,
        StreamEvent::Head { status: 200, .. }
    ));
    client.bytes(FIRST).await;
    assert_eq!(
        events.len(),
        1,
        "response-header record fills the only ordinary event slot"
    );
    proxy.stop().await.unwrap();
    assert!(matches!(client.next().await, StreamEvent::End(Err(_))));
    let queued = match events.try_recv().unwrap() {
        ProxyEvent::Captured { record, .. } => record,
        event => panic!("expected queued response head, got {event:?}"),
    };
    assert_eq!(queued.summary.id, initial.summary.id);
    assert_eq!(queued.summary.state, FlowState::Pending);
    let retained = proxy.drain_terminal_records();
    assert_eq!(retained.len(), 1);
    assert!(proxy.drain_terminal_records().is_empty());
    let stream = retained[0]
        .response_stream
        .as_ref()
        .expect("terminal mailbox keeps streaming provenance");
    assert_eq!(stream.original_status, 200);
    assert!(!stream.body_complete);
    let record = &retained[0].record;
    assert_eq!(record.summary.id, initial.summary.id);
    assert!(record.summary.revision > queued.summary.revision);
    assert_eq!(record.summary.state, FlowState::Cancelled);
    assert_eq!(record.summary.response_bytes, FIRST.len() as u64);
    assert_eq!(record.response.as_ref().unwrap().body, FIRST);
    client.finish().await;
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    origin.stop().await;
}

#[tokio::test]
async fn stopping_sse_reclaims_an_h2_stream_blocked_by_the_clients_receive_window() {
    tokio::time::timeout(DEADLINE, async {
        let mut origin = SseOrigin::start(Some(true), None).await;
        let (mut proxy, ca, mut events) = start_proxy(&origin, |_| {}, 64).await;
        let io = tls_stream(
            proxy.address(),
            &format!("localhost:{}", origin.address.port()),
            ca,
            true,
        )
        .await;
        let (mut sender, connection) =
            hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                .initial_stream_window_size(1)
                .handshake(TokioIo::new(io))
                .await
                .unwrap();
        let driver = tokio::spawn(connection);
        let request = Request::builder()
            .uri(origin.url())
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = tokio::spawn(async move { sender.send_request(request).await });
        assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
        origin.send(FIRST).await;
        let mut body = response.await.unwrap().unwrap().into_body();
        let first_byte = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(first_byte.as_ref(), &FIRST[..1]);
        assert!(origin.frames.is_some(), "the source has not released EOS");

        // Keeping the body unpolled exhausts its one-byte receive window.
        proxy.stop().await.unwrap();
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        let record = terminal(&mut events).await;
        assert_eq!(record.summary.state, FlowState::Cancelled);
        assert_eq!(record.summary.response_bytes, FIRST.len() as u64);
        assert_eq!(record.response.unwrap().body, FIRST);
        drop(body);
        let _ = driver.await.unwrap();
        origin.stop().await;
    })
    .await
    .expect("Stop and fixture cleanup must not wait for the client to consume DATA");
}

#[tokio::test]
async fn losing_the_sse_event_consumer_closes_both_legs_and_retains_partial_history() {
    let mut origin = SseOrigin::start(None, None).await;
    let (mut proxy, ca, events) = start_proxy(&origin, |_| {}, 4).await;
    let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    origin.send(FIRST).await;
    assert!(matches!(
        client.next().await,
        StreamEvent::Head { status: 200, .. }
    ));
    client.bytes(FIRST).await;
    drop(events);
    assert!(matches!(client.next().await, StreamEvent::End(Err(_))));
    client.finish().await;
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    proxy.stop().await.unwrap();
    let records = proxy.drain_terminal_records();
    assert_eq!(records.len(), 1);
    assert!(proxy.drain_terminal_records().is_empty());
    let stream = records[0]
        .response_stream
        .as_ref()
        .expect("disconnected consumer keeps streaming provenance");
    assert_eq!(stream.original_status, 200);
    assert!(!stream.body_complete);
    let record = &records[0].record;
    assert_eq!(record.summary.state, FlowState::Cancelled);
    assert_eq!(record.summary.response_bytes, FIRST.len() as u64);
    assert_eq!(record.response.as_ref().unwrap().body, FIRST);
    origin.stop().await;
}

#[tokio::test]
async fn a_disconnected_http_client_cancels_its_sse_origin_and_keeps_received_bytes() {
    let mut origin = SseOrigin::start(None, None).await;
    let (mut proxy, _, mut events) = start_proxy(&origin, |_| {}, 16).await;
    let mut client = TcpStream::connect(proxy.address()).await.unwrap();
    client
        .write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", origin.url()).as_bytes())
        .await
        .unwrap();
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    origin.send(FIRST).await;
    let response = tokio::time::timeout(DEADLINE, async {
        let mut response = Vec::new();
        let mut buffer = [0; 256];
        while !response.windows(FIRST.len()).any(|bytes| bytes == FIRST) {
            let count = client.read(&mut buffer).await.unwrap();
            assert!(
                count > 0,
                "the stream must remain open after the first event"
            );
            response.extend_from_slice(&buffer[..count]);
            assert!(response.len() <= 4096);
        }
        response
    })
    .await
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    drop(client);
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    let record = terminal(&mut events).await;
    assert_eq!(record.summary.state, FlowState::Cancelled);
    assert_eq!(record.response.unwrap().body, FIRST);
    assert!(origin.frames.is_some(), "source EOF was never released");
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn established_sse_is_not_killed_by_the_response_header_deadline() {
    let mut origin = SseOrigin::start(None, None).await;
    let header_deadline = Duration::from_millis(250);
    let (mut proxy, ca, mut events) = start_proxy(
        &origin,
        |config| config.request_timeout = header_deadline,
        64,
    )
    .await;
    let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    origin.send(FIRST).await;
    assert!(matches!(
        client.next().await,
        StreamEvent::Head { status: 200, .. }
    ));
    client.bytes(FIRST).await;
    // The configured deadline itself is the condition under test, not a scheduling sleep.
    assert!(
        tokio::time::timeout(header_deadline * 2, client.events.recv())
            .await
            .is_err(),
        "an established stream must outlive its header deadline"
    );
    origin.send(SECOND).await;
    client.bytes(SECOND).await;
    drop(origin.frames.take());
    assert!(matches!(client.next().await, StreamEvent::End(Ok(()))));
    assert_eq!(
        terminal(&mut events).await.summary.state,
        FlowState::Complete
    );
    client.finish().await;
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    proxy.stop().await.unwrap();
    origin.stop().await;
}
