use super::*;

#[tokio::test]
async fn response_predicate_receives_only_its_own_actual_hold_and_modification() {
    tokio::time::timeout(DEADLINE * 2, async {
        let mut origin = Origin::start(None, false).await;
        let callback_timeout = Duration::from_secs(2);
        let (mut proxy, ca, mut events) = proxy_with(&origin, true, |config| {
            config.limits.request_timeout = callback_timeout;
            config.limits.max_body_bytes = 128;
        })
        .await;
        let listener_port = proxy.address().port();
        assert_ne!(listener_port, 0);
        assert_ne!(listener_port, origin.address.port());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let callback_tokens = Arc::new(Mutex::new(Vec::new()));
        for (index, (hold_request, modify_request, transform_request, restore_request)) in [
            (false, false, None, false),
            (true, false, None, false),
            (true, true, None, false),
            (false, false, None, false),
            (false, true, Some(true), false),
            (true, false, Some(true), true),
            (false, false, Some(false), false),
        ]
        .into_iter()
        .enumerate()
        {
            let captured = calls.clone();
            let captured_tokens = callback_tokens.clone();
            proxy.set_intercept_filter(Some(Arc::new(
                move |record,
                      response_stage,
                      head_only,
                      held,
                      changed,
                      ctx,
                      cancel,
                      (deadline, max_body)| {
                    assert!(!cancel.is_cancelled());
                    assert_eq!(max_body, 128);
                    let now = std::time::Instant::now();
                    assert!(deadline > now);
                    assert!(deadline <= now + callback_timeout);
                    captured_tokens.lock().unwrap().push(cancel.clone());
                    captured.lock().unwrap().push((
                        record.summary.id,
                        response_stage,
                        head_only,
                        held,
                        changed,
                        ctx.listener_port,
                    ));
                    if ctx.listener_port == listener_port && (response_stage || hold_request) {
                        Ok(InterceptFilterOutcome::Intercept)
                    } else {
                        Ok(InterceptFilterOutcome::Bypass)
                    }
                },
            )));
            proxy.set_transform(match transform_request {
                Some(valid) => Some(Arc::new(
                    move |record, response_stage, _, cancel, (deadline, max)| {
                        assert!(!cancel.is_cancelled());
                        assert!(deadline > std::time::Instant::now());
                        assert_eq!(max, 128);
                        if response_stage {
                            return Ok(false);
                        }
                        record.request.body.push(0xa1);
                        if !valid {
                            record.request.method = "invalid method".into();
                            record.request.headers.push(Header {
                                name: "x-transform-rollback".into(),
                                value: b"must-not-reach-origin".to_vec(),
                            });
                        }
                        Ok(true)
                    },
                )),
                None => None,
            });
            let url = format!("{}?flags={index}", origin.url());
            let request_task = tokio::spawn(client(
                proxy.address(),
                url.clone(),
                ca.clone(),
                false,
                false,
            ));
            let request_id = if hold_request {
                let (mut record, reply) = next_intercept(&mut events).await;
                assert!(record.response.is_none());
                if restore_request {
                    assert_eq!(record.request.body, [0, 0xfe, b'C', 0xa1]);
                    assert_eq!(record.request.body.pop(), Some(0xa1));
                }
                assert_eq!(record.request.body, [0, 0xfe, b'C']);
                assert_eq!(record.request.url, url);
                if modify_request {
                    record.request.body.push(0xa1);
                }
                let id = record.summary.id;
                reply
                    .send(InterceptDecision::ForwardRequest(record.request))
                    .unwrap();
                Some(id)
            } else {
                None
            };
            let (record, reply) = next_intercept(&mut events).await;
            let flow_id = record.summary.id;
            if let Some(request_id) = request_id {
                assert_eq!(flow_id, request_id);
            }
            assert_eq!(record.request.url, url);
            assert_eq!(record.request.method, "POST");
            assert!(
                record
                    .request
                    .headers
                    .iter()
                    .all(|header| header.name != "x-transform-rollback")
            );
            let response = record.response.unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.body, [0, 0xff, b'O']);
            assert_eq!(
                &calls.lock().unwrap()[index * 2..],
                &[
                    (flow_id, false, false, false, false, listener_port),
                    (
                        flow_id,
                        true,
                        false,
                        hold_request,
                        modify_request,
                        listener_port
                    ),
                ]
            );
            let mut expected_body = vec![0, 0xfe, b'C'];
            if modify_request {
                expected_body.push(0xa1);
            }
            loop {
                match origin.next().await {
                    OriginEvent::Request {
                        version,
                        alpn,
                        headers,
                        body,
                    } => {
                        assert_eq!(version, Version::HTTP_11);
                        assert_eq!(alpn, None);
                        assert_eq!(body, expected_body);
                        assert!(
                            headers
                                .iter()
                                .all(|(name, _)| name != "x-transform-rollback")
                        );
                        assert_eq!(
                            headers
                                .iter()
                                .filter(|(name, _)| name == "x-client")
                                .map(|(_, value)| value.as_slice())
                                .collect::<Vec<_>>(),
                            vec![b"one".as_slice(), b"two".as_slice()]
                        );
                        break;
                    }
                    OriginEvent::Closed => {}
                    event => {
                        panic!("expected the real condition-selected source request: {event:?}")
                    }
                }
            }
            assert_eq!(origin.received.load(Ordering::SeqCst), index + 1);
            reply
                .send(InterceptDecision::ForwardResponse(response))
                .unwrap();
            let result = tokio::time::timeout(DEADLINE, request_task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.status, 200);
            assert_eq!(result.body, [0, 0xff, b'O']);
            loop {
                match events.recv().await.unwrap() {
                    ProxyEvent::Captured { record, .. } => {
                        assert_eq!(record.summary.id, flow_id);
                        assert_eq!(record.request.body, expected_body);
                        if record.summary.state == FlowState::Complete {
                            break;
                        }
                        assert_eq!(record.summary.state, FlowState::Pending);
                    }
                    event => panic!("forwarded condition-selected flow must complete: {event:?}"),
                }
            }
        }
        proxy.stop().await.unwrap();
        origin.stop().await;
        assert_eq!(callback_tokens.lock().unwrap().len(), 14);
        assert!(
            callback_tokens
                .lock()
                .unwrap()
                .iter()
                .all(CancellationToken::is_cancelled)
        );
    })
    .await
    .expect("each response receives its own bypass, forwarding, edit and transform flags");
}
