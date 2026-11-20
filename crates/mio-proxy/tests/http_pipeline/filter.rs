use super::*;

#[tokio::test]
async fn drop_and_disable_interception_invalidate_pending_replies() {
    let origin = Origin::start(None, false).await;
    let (mut proxy, ca, mut events) = proxy(&origin, true).await;
    let request = tokio::spawn(client(
        proxy.address(),
        origin.url(),
        ca.clone(),
        false,
        false,
    ));
    let (_, reply) = next_intercept(&mut events).await;
    reply.send(InterceptDecision::Drop).unwrap();
    assert_eq!(
        tokio::time::timeout(DEADLINE, request)
            .await
            .unwrap()
            .unwrap()
            .status,
        403
    );
    assert_eq!(origin.received.load(Ordering::SeqCst), 0);
    let request = tokio::spawn(client(proxy.address(), origin.url(), ca, false, false));
    let (_, reply) = next_intercept(&mut events).await;
    proxy.set_intercept(false, false);
    assert_eq!(
        tokio::time::timeout(DEADLINE, request)
            .await
            .unwrap()
            .unwrap()
            .status,
        200
    );
    assert!(reply.is_closed());
    assert_eq!(origin.received.load(Ordering::SeqCst), 1);
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn conditional_filter_matches_before_accept_and_hot_update_does_not_release_pending() {
    tokio::time::timeout(DEADLINE, async {
        let mut origin = Origin::start(None, false).await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let hold_requests = Arc::new(AtomicBool::new(false));
        let captured_hold_requests = hold_requests.clone();
        let expected_listener_port = Arc::new(AtomicUsize::new(0));
        let captured_listener_port = expected_listener_port.clone();
        let callback_timeout = Duration::from_secs(2);
        let callback_tokens = Arc::new(Mutex::new(Vec::new()));
        let captured_tokens = callback_tokens.clone();
        let initial_filter: InterceptFilter = Arc::new(
            move |_, response_stage, _, _, _, ctx, cancel, (deadline, max_body)| {
                assert!(!cancel.is_cancelled());
                assert_eq!(max_body, 128);
                let now = std::time::Instant::now();
                assert!(deadline > now);
                assert!(deadline <= now + callback_timeout);
                captured_tokens.lock().unwrap().push(cancel.clone());
                captured
                    .lock()
                    .unwrap()
                    .push((response_stage, ctx.listener_port));
                Ok(usize::from(ctx.listener_port) == captured_listener_port.load(Ordering::SeqCst)
                    && !response_stage
                    && captured_hold_requests.load(Ordering::SeqCst))
            },
        );
        let retired_filter = Arc::downgrade(&initial_filter);
        let initial_transform: mio_proxy::FlowTransform = Arc::new(
            |record, response_stage, _, cancel, (deadline, max)| {
                assert!(!cancel.is_cancelled());
                assert!(deadline > std::time::Instant::now());
                assert_eq!(max, 128);
                if response_stage {
                    return Ok(false);
                }
                record.request.headers.push(Header {
                    name: "x-filter-transform".into(),
                    value: b"preserved".to_vec(),
                });
                Ok(true)
            },
        );
        let retired_transform = Arc::downgrade(&initial_transform);
        let (mut proxy, ca, mut events) = proxy_with(&origin, true, |config| {
            assert_eq!(config.listen.port(), 0);
            config.request_timeout = callback_timeout;
            config.max_body_bytes = 128;
            config.intercept_filter = Some(initial_filter);
            config.flow_transform = Some(initial_transform);
        })
        .await;
        let listener_port = proxy.address().port();
        assert_ne!(listener_port, 0);
        assert_ne!(listener_port, origin.address.port());
        expected_listener_port.store(usize::from(listener_port), Ordering::SeqCst);
        assert!(retired_filter.upgrade().is_some());
        assert!(retired_transform.upgrade().is_some());
        let first_response = client(proxy.address(), origin.url(), ca.clone(), false, false).await;
        assert_eq!(first_response.status, 200);
        assert_eq!(first_response.body, [0, 0xff, b'O']);
        assert_eq!(origin.received.load(Ordering::SeqCst), 1);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(false, listener_port), (true, listener_port)]
        );
        match origin.next().await {
            OriginEvent::Request { headers, body, .. } => {
                assert_eq!(body, [0, 0xfe, b'C']);
                assert!(headers.contains(&("x-filter-transform".into(), b"preserved".to_vec())));
            }
            event => panic!("expected the initially filtered source request, got {event:?}"),
        }
        let mut transformed_pending = false;
        loop {
            match events.recv().await.unwrap() {
                ProxyEvent::Captured { record, .. } => {
                    transformed_pending |= record.summary.state == FlowState::Pending
                        && record.request.headers.iter().any(|header| {
                            header.name == "x-filter-transform" && header.value == b"preserved"
                        });
                    if record.summary.state == FlowState::Complete {
                        assert_eq!(record.request.body, [0, 0xfe, b'C']);
                        break;
                    }
                    assert_eq!(record.summary.state, FlowState::Pending);
                }
                event => panic!("false filter must not intercept or discard transforms: {event:?}"),
            }
        }
        assert!(transformed_pending);
        proxy.set_transform(None);
        assert!(
            retired_transform.upgrade().is_none(),
            "a hot update must release the initial transform without waiting for Stop"
        );
        hold_requests.store(true, Ordering::SeqCst);
        let pending_task = tokio::spawn(client(
            proxy.address(),
            origin.url(),
            ca.clone(),
            false,
            false,
        ));
        let (pending_record, pending_reply) = next_intercept(&mut events).await;
        assert!(pending_record.response.is_none());
        assert_eq!(pending_record.summary.state, FlowState::Intercepted);
        assert_eq!(pending_record.request.body, [0, 0xfe, b'C']);
        let pending_id = pending_record.summary.id;
        assert_eq!(origin.received.load(Ordering::SeqCst), 1);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[
                (false, listener_port),
                (true, listener_port),
                (false, listener_port),
            ]
        );
        proxy.set_intercept_filter(Some(Arc::new(|_, _, _, _, _, _, _, _| Ok(false))));
        let barrier_url = format!("{}?barrier=1", origin.url());
        let barrier_response = client(proxy.address(), barrier_url.clone(), ca, false, false).await;
        assert_eq!(barrier_response.status, 200);
        assert_eq!(origin.received.load(Ordering::SeqCst), 2);
        loop {
            match events.recv().await.unwrap() {
                ProxyEvent::Captured { record, .. } => {
                    assert_ne!(record.summary.id, pending_id);
                    assert_eq!(record.request.url, barrier_url);
                    if record.summary.state == FlowState::Complete {
                        break;
                    }
                    assert_eq!(record.summary.state, FlowState::Pending);
                }
                event => panic!("updated false filter must bypass the later request: {event:?}"),
            }
        }
        let initial_filter_released = retired_filter.upgrade().is_none();
        assert!(!pending_task.is_finished());
        assert!(!pending_reply.is_closed());
        pending_reply
            .send(InterceptDecision::ForwardRequest(pending_record.request))
            .unwrap();
        assert_eq!(
            tokio::time::timeout(DEADLINE, pending_task)
                .await
                .unwrap()
                .unwrap()
                .status,
            200
        );
        assert_eq!(origin.received.load(Ordering::SeqCst), 3);
        loop {
            match events.recv().await.unwrap() {
                ProxyEvent::Captured { record, .. } => {
                    assert_eq!(record.summary.id, pending_id);
                    assert_eq!(record.request.body, [0, 0xfe, b'C']);
                    if record.summary.state == FlowState::Complete {
                        break;
                    }
                    assert_eq!(record.summary.state, FlowState::Pending);
                }
                event => panic!("forwarded pending request must complete normally: {event:?}"),
            }
        }
        proxy.stop().await.unwrap();
        origin.stop().await;
        assert_eq!(callback_tokens.lock().unwrap().len(), 3);
        assert!(
            callback_tokens
                .lock()
                .unwrap()
                .iter()
                .all(CancellationToken::is_cancelled)
        );
        assert!(retired_filter.upgrade().is_none());
        assert!(retired_transform.upgrade().is_none());
        assert!(
            initial_filter_released,
            "a hot update must release the initial filter before Stop while its held request stays pending"
        );
    })
    .await
    .expect("initial filtering and hot updates preserve actual pending admission");
}
