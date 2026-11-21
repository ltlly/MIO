use super::*;
use std::sync::Mutex;

async fn head_intercept(
    events: &mut mpsc::Receiver<ProxyEvent>,
) -> (
    mio_plugin_api::FlowRecord,
    oneshot::Sender<InterceptDecision>,
) {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Intercepted {
                record,
                response_head_only,
                reply,
                ..
            } = events.recv().await.unwrap()
            {
                assert!(
                    response_head_only,
                    "SSE interception must explicitly identify headers-only"
                );
                let response = record.response.as_ref().unwrap();
                assert!(response.body.is_empty());
                assert!(response.trailers.is_empty());
                return (record, reply);
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn sse_header_intercept_forwards_edits_without_buffering_after_mime_change() {
    let mut origin = SseOrigin::start(None, None).await;
    let (mut proxy, ca, mut events) = start_proxy(
        &origin,
        |config| config.hooks.intercept_responses = true,
        64,
    )
    .await;
    let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    let (held, reply) = head_intercept(&mut events).await;
    let mut response = held.response.unwrap();
    response
        .headers
        .retain(|header| !header.name.eq_ignore_ascii_case("content-type"));
    response.headers.push(Header {
        name: "content-type".into(),
        value: b"application/octet-stream".to_vec(),
    });
    response.headers.push(Header {
        name: "x-edited".into(),
        value: b"headers only".to_vec(),
    });
    reply
        .send(InterceptDecision::ForwardResponse(response))
        .unwrap();
    origin.send(FIRST).await;
    match client.next().await {
        StreamEvent::Head {
            status, headers, ..
        } => {
            assert_eq!(status, 200);
            assert_eq!(headers["content-type"], "application/octet-stream");
            assert_eq!(headers["x-edited"], "headers only");
        }
        event => panic!("expected edited response head, got {event:?}"),
    }
    client.bytes(FIRST).await;
    assert!(
        origin.frames.is_some(),
        "the edited MIME must not switch back to buffering"
    );
    drop(origin.frames.take());
    assert!(matches!(client.next().await, StreamEvent::End(Ok(()))));
    let record = terminal(&mut events).await;
    assert_eq!(record.summary.state, FlowState::Complete);
    assert_eq!(record.response.unwrap().body, FIRST);
    client.finish().await;
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn sse_conditional_predicate_receives_head_only_and_actual_request_flags() {
    tokio::time::timeout(DEADLINE, async {
        let cpu_budget = Duration::from_secs(2);
        let body_limit = 256;
        for (intercept_request, edit_request) in [(false, false), (true, false), (true, true)] {
            let mut origin = SseOrigin::start(None, None).await;
            let seen = Arc::new(Mutex::new(Vec::new()));
            let captured = seen.clone();
            let (mut proxy, ca, mut events) = start_proxy(
                &origin,
                |config| {
                    config.hooks.intercept_requests = true;
                    config.hooks.intercept_responses = true;
                    config.limits.request_timeout = cpu_budget;
                    config.limits.max_body_bytes = body_limit;
                    config.hooks.intercept_filter = Some(Arc::new(
                        move |record,
                              response_stage,
                              head_only,
                              held,
                              changed,
                              ctx,
                              cancel,
                              (cpu_deadline, max_body_bytes)| {
                            assert!(!cancel.is_cancelled());
                            captured.lock().unwrap().push((
                                (
                                    record.summary.id,
                                    response_stage,
                                    head_only,
                                    held,
                                    changed,
                                    ctx.listener_port,
                                ),
                                cancel.clone(),
                                std::time::Instant::now(),
                                cpu_deadline,
                                max_body_bytes,
                            ));
                            if response_stage || intercept_request {
                                Ok(InterceptFilterOutcome::Intercept)
                            } else {
                                Ok(InterceptFilterOutcome::Bypass)
                            }
                        },
                    ));
                },
                64,
            )
            .await;
            let listener_port = proxy.address().port();
            assert_ne!(listener_port, 0);
            let proxy_cancel = proxy.cancellation_token();
            let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
            let request_id = if intercept_request {
                let (held, reply) = next_intercept(&mut events).await;
                assert!(held.response.is_none());
                let request_id = held.summary.id;
                let mut request = held.request;
                if edit_request {
                    request.headers.push(Header {
                        name: "x-filter-edit".into(),
                        value: b"preserved".to_vec(),
                    });
                }
                reply
                    .send(InterceptDecision::ForwardRequest(request))
                    .unwrap();
                Some(request_id)
            } else {
                None
            };
            match origin.next().await {
                OriginEvent::Request {
                    version,
                    alpn,
                    headers,
                    body,
                } => {
                    assert_eq!(version, Version::HTTP_11);
                    assert!(alpn.is_none());
                    assert!(body.is_empty());
                    assert_eq!(
                        headers
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case("x-filter-edit"))
                            .map(|(_, value)| value.as_slice()),
                        edit_request.then_some(b"preserved".as_slice()),
                    );
                }
                event => panic!("expected the actual origin request, got {event:?}"),
            }
            let (held, reply) = head_intercept(&mut events).await;
            assert_eq!(held.summary.state, FlowState::Intercepted);
            let flow_id = held.summary.id;
            if let Some(request_id) = request_id {
                assert_eq!(flow_id, request_id);
            }
            reply
                .send(InterceptDecision::ForwardResponse(held.response.unwrap()))
                .unwrap();
            origin.send(FIRST).await;
            match client.next().await {
                StreamEvent::Head { status, .. } => assert_eq!(status, 200),
                event => panic!("expected streamed response, got {event:?}"),
            }
            client.bytes(FIRST).await;
            assert!(origin.frames.is_some(), "origin EOF has not been released");
            assert!(!client.task.is_finished());
            {
                let calls = seen.lock().unwrap();
                assert_eq!(
                    calls.iter().map(|(flags, ..)| *flags).collect::<Vec<_>>(),
                    vec![
                        (flow_id, false, false, false, false, listener_port),
                        (
                            flow_id,
                            true,
                            true,
                            intercept_request,
                            edit_request,
                            listener_port,
                        ),
                    ],
                );
                for (_, cancel, observed, cpu_deadline, max_body_bytes) in calls.iter() {
                    assert!(!cancel.is_cancelled());
                    assert!(*cpu_deadline > *observed);
                    assert!(*cpu_deadline <= *observed + cpu_budget);
                    assert_eq!(*max_body_bytes, body_limit);
                }
            }
            drop(origin.frames.take());
            assert!(matches!(client.next().await, StreamEvent::End(Ok(()))));
            let record = terminal(&mut events).await;
            assert_eq!(record.summary.id, flow_id);
            assert_eq!(record.summary.state, FlowState::Complete);
            assert_eq!(record.summary.response_bytes, FIRST.len() as u64);
            assert_eq!(record.response.unwrap().body, FIRST);
            client.finish().await;
            assert!(matches!(origin.next().await, OriginEvent::Closed));
            assert!(!proxy_cancel.is_cancelled());
            assert!(
                seen.lock()
                    .unwrap()
                    .iter()
                    .all(|(_, cancel, ..)| !cancel.is_cancelled())
            );
            proxy.stop().await.unwrap();
            assert!(proxy_cancel.is_cancelled());
            assert!(
                seen.lock()
                    .unwrap()
                    .iter()
                    .all(|(_, cancel, ..)| cancel.is_cancelled())
            );
            origin.stop().await;
        }
    })
    .await
    .expect("SSE supplies bounded response headers and real request-interception flags");
}

#[tokio::test]
async fn sse_header_intercept_rejects_body_trailers_or_provisional_status_and_closes_origin() {
    for edit in ["body", "trailers", "status"] {
        let mut origin = SseOrigin::start(None, None).await;
        let (mut proxy, ca, mut events) = start_proxy(
            &origin,
            |config| config.hooks.intercept_responses = true,
            64,
        )
        .await;
        let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
        assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
        let (held, reply) = head_intercept(&mut events).await;
        let mut response = held.response.unwrap();
        match edit {
            "body" => response.body = b"replacement".to_vec(),
            "trailers" => response.trailers.push(Header {
                name: "x-edit".into(),
                value: b"replacement".to_vec(),
            }),
            "status" => response.status = 101,
            _ => unreachable!(),
        }
        reply
            .send(InterceptDecision::ForwardResponse(response))
            .unwrap();
        assert!(matches!(
            client.next().await,
            StreamEvent::Head { status: 400, .. }
        ));
        loop {
            match client.next().await {
                StreamEvent::Data(_) => {}
                StreamEvent::End(Ok(())) => break,
                event => panic!("expected an explicit invalid-edit response, got {event:?}"),
            }
        }
        let record = terminal(&mut events).await;
        let FlowState::Failed(error) = record.summary.state else {
            panic!("the invalid {edit} edit must fail")
        };
        assert!(error.contains(if edit == "status" {
            "临时响应"
        } else {
            "响应头"
        }));
        let response = record.response.unwrap();
        assert!(response.body.is_empty());
        assert!(response.trailers.is_empty());
        assert_eq!(response.status, 200);
        client.finish().await;
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}

#[tokio::test]
async fn sse_header_intercept_drop_or_no_body_status_ends_the_source() {
    for replacement_status in [None, Some(204), Some(304)] {
        let drop_response = replacement_status.is_none();
        let mut origin = SseOrigin::start(None, None).await;
        let (mut proxy, ca, mut events) = start_proxy(
            &origin,
            |config| config.hooks.intercept_responses = true,
            64,
        )
        .await;
        let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
        assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
        let (held, reply) = head_intercept(&mut events).await;
        if drop_response {
            reply.send(InterceptDecision::Drop).unwrap();
        } else {
            let mut response = held.response.unwrap();
            response.status = replacement_status.unwrap();
            reply
                .send(InterceptDecision::ForwardResponse(response))
                .unwrap();
        }
        match client.next().await {
            StreamEvent::Head { status, .. } => {
                assert_eq!(status, replacement_status.unwrap_or(403));
            }
            event => panic!("expected final HTTP response headers, got {event:?}"),
        }
        loop {
            match client.next().await {
                StreamEvent::Data(_) if drop_response => {}
                StreamEvent::End(Ok(())) => break,
                event => panic!("expected EOF without an SSE stream, got {event:?}"),
            }
        }
        let record = terminal(&mut events).await;
        assert_eq!(
            record.summary.state,
            if drop_response {
                FlowState::Dropped
            } else {
                FlowState::Complete
            }
        );
        assert!(record.response.unwrap().body.is_empty());
        client.finish().await;
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        proxy.stop().await.unwrap();
        origin.stop().await;
    }
}
