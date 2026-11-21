use super::*;
async fn stream_roundtrip(tls: bool, ingress_h2: bool, egress_h2: bool, stop: bool) {
    let mut origin = SseOrigin::start(tls.then_some(egress_h2), None).await;
    let (mut proxy, ca, mut events) = start_proxy(&origin, |_| {}, 64).await;
    let mut client = SseClient::start(proxy.address(), origin.url(), ca, tls, ingress_h2);
    match origin.next().await {
        OriginEvent::Request { version, alpn, .. } => {
            assert_eq!(
                version,
                if egress_h2 {
                    Version::HTTP_2
                } else {
                    Version::HTTP_11
                }
            );
            assert_eq!(
                alpn.as_deref(),
                tls.then_some(if egress_h2 {
                    b"h2".as_slice()
                } else {
                    b"http/1.1".as_slice()
                })
            );
        }
        event => panic!("expected the actual origin request, got {event:?}"),
    }
    origin.send(FIRST).await;
    match client.next().await {
        StreamEvent::Head {
            status,
            version,
            alpn,
            headers,
        } => {
            assert_eq!(status, 200);
            assert_eq!(
                version,
                if ingress_h2 {
                    Version::HTTP_2
                } else {
                    Version::HTTP_11
                }
            );
            assert_eq!(
                alpn.as_deref(),
                tls.then_some(if ingress_h2 {
                    b"h2".as_slice()
                } else {
                    b"http/1.1".as_slice()
                })
            );
            assert!(!headers.contains_key("content-length"));
        }
        event => panic!("expected SSE response headers, got {event:?}"),
    }
    client.bytes(FIRST).await;
    assert!(origin.frames.is_some(), "origin EOF has not been released");
    assert!(!client.task.is_finished());
    let expected = if stop {
        proxy.stop().await.unwrap();
        assert!(matches!(client.next().await, StreamEvent::End(Err(_))));
        FIRST.to_vec()
    } else {
        origin.send(SECOND).await;
        client.bytes(SECOND).await;
        drop(origin.frames.take());
        let end = client.next().await;
        assert!(
            matches!(end, StreamEvent::End(Ok(()))),
            "TLS={tls}, ingress H2={ingress_h2}, egress H2={egress_h2}: {end:?}"
        );
        [FIRST, SECOND].concat()
    };
    let record = terminal(&mut events).await;
    assert_eq!(
        record.summary.state,
        if stop {
            FlowState::Cancelled
        } else {
            FlowState::Complete
        }
    );
    assert_eq!(record.summary.response_bytes, expected.len() as u64);
    assert_eq!(record.response.unwrap().body, expected);
    client.finish().await;
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn first_sse_event_precedes_origin_eof_for_http_https_and_mixed_h2_legs() {
    for (tls, ingress_h2, egress_h2) in [
        (false, false, false),
        (true, false, false),
        (true, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        stream_roundtrip(tls, ingress_h2, egress_h2, false).await;
    }
}

#[tokio::test]
async fn stop_closes_both_sse_legs_and_retains_partial_bytes() {
    for (tls, h2) in [(false, false), (true, false), (true, true)] {
        stream_roundtrip(tls, h2, h2, true).await;
    }
}

#[tokio::test]
async fn finite_content_length_sse_finishes_complete_without_a_false_drop() {
    let mut origin = SseOrigin::start(None, Some(FIRST.len())).await;
    let (mut proxy, ca, mut events) = start_proxy(&origin, |_| {}, 64).await;
    let mut client = SseClient::start(proxy.address(), origin.url(), ca, false, false);
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    origin.send(FIRST).await;
    assert!(matches!(
        client.next().await,
        StreamEvent::Head { status: 200, .. }
    ));
    client.bytes(FIRST).await;
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
async fn exact_sse_capture_limit_still_checks_source_eof_or_overflow_on_http_and_h2() {
    for h2 in [false, true] {
        for overflow in [false, true] {
            let mut origin = SseOrigin::start(h2.then_some(true), None).await;
            let (mut proxy, ca, mut events) = start_proxy(
                &origin,
                |config| config.limits.max_body_bytes = FIRST.len(),
                64,
            )
            .await;
            let mut client = SseClient::start(proxy.address(), origin.url(), ca, h2, h2);
            assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
            origin.send(FIRST).await;
            assert!(matches!(
                client.next().await,
                StreamEvent::Head { status: 200, .. }
            ));
            client.bytes(FIRST).await;
            if overflow {
                origin.send(SECOND).await;
            } else {
                drop(origin.frames.take());
            }
            let StreamEvent::End(result) = client.next().await else {
                panic!("the capture limit must not forward extra bytes or hide source EOS")
            };
            assert_eq!(result.is_err(), overflow);
            let record = terminal(&mut events).await;
            if overflow {
                assert!(matches!(record.summary.state, FlowState::Failed(_)));
                assert!(record.summary.response_bytes > FIRST.len() as u64);
            } else {
                assert_eq!(record.summary.state, FlowState::Complete);
                assert_eq!(record.summary.response_bytes, FIRST.len() as u64);
            }
            assert_eq!(record.response.unwrap().body, FIRST);
            client.finish().await;
            assert!(matches!(origin.next().await, OriginEvent::Closed));
            proxy.stop().await.unwrap();
            origin.stop().await;
        }
    }
}
