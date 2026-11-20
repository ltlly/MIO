use super::*;

#[tokio::test]
async fn conditional_filter_errors_retain_details_and_disabled_mode_never_calls_it() {
    tokio::time::timeout(DEADLINE, async {
        let mut origin = Origin::start(None, false).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let captured = calls.clone();
        let (mut proxy, ca, mut events) = proxy_with(&origin, true, |config| {
            config.request_timeout = Duration::from_secs(2);
            config.max_body_bytes = 128;
            config.intercept_filter = Some(Arc::new(
                move |_, _, _, _, _, ctx, cancel, (deadline, max_body)| {
                    assert_ne!(ctx.listener_port, 0);
                    assert!(!cancel.is_cancelled());
                    assert_eq!(max_body, 128);
                    let now = std::time::Instant::now();
                    assert!(deadline > now);
                    assert!(deadline <= now + Duration::from_secs(2));
                    captured.fetch_add(1, Ordering::SeqCst);
                    Err(ProxyError::InvalidMessage("condition parser limit".into()))
                },
            ));
        })
        .await;
        let failed_response = client(proxy.address(), origin.url(), ca.clone(), false, false).await;
        assert_eq!(failed_response.status, 400);
        assert!(String::from_utf8_lossy(&failed_response.body).contains("condition parser limit"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(origin.received.load(Ordering::SeqCst), 0);
        let ProxyEvent::Captured {
            record: initial,
            response_stream: None,
            ..
        } = events.recv().await.unwrap()
        else {
            panic!("the full original request must be captured before condition evaluation");
        };
        assert_eq!(initial.summary.state, FlowState::Pending);
        let ProxyEvent::Captured {
            record: failed,
            response_stream: None,
            ..
        } = events.recv().await.unwrap()
        else {
            panic!("condition errors must retain a visible terminal record");
        };
        assert!(matches!(
            &failed.summary.state,
            FlowState::Failed(message) if message.contains("condition parser limit")
        ));
        assert_eq!(failed.summary.id, initial.summary.id);
        assert!(failed.summary.revision > initial.summary.revision);
        assert_eq!(failed.request, initial.request);
        assert_eq!(failed.request.method, "POST");
        assert_eq!(failed.request.url, origin.url());
        assert_eq!(failed.request.body, [0, 0xfe, b'C']);
        assert_eq!(failed.summary.request_bytes, 3);
        assert!(failed.response.is_none());
        assert_eq!(
            failed
                .request
                .headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("x-client"))
                .map(|header| header.value.as_slice())
                .collect::<Vec<_>>(),
            vec![b"one".as_slice(), b"two".as_slice()]
        );
        proxy.set_intercept(false, false);
        let bypass_response = client(proxy.address(), origin.url(), ca, false, false).await;
        assert_eq!(bypass_response.status, 200);
        assert_eq!(bypass_response.body, [0, 0xff, b'O']);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(origin.received.load(Ordering::SeqCst), 1);
        match origin.next().await {
            OriginEvent::Request { body, .. } => assert_eq!(body, [0, 0xfe, b'C']),
            event => panic!("disabled mode must reach the real origin, got {event:?}"),
        }
        loop {
            match events.recv().await.unwrap() {
                ProxyEvent::Captured { record, .. } => {
                    assert_ne!(record.summary.id, failed.summary.id);
                    assert_eq!(record.request.body, [0, 0xfe, b'C']);
                    if record.summary.state == FlowState::Complete {
                        break;
                    }
                    assert_eq!(record.summary.state, FlowState::Pending);
                }
                event => panic!("disabled mode must not invoke the erroneous filter: {event:?}"),
            }
        }
        proxy.stop().await.unwrap();
        origin.stop().await;
    })
    .await
    .expect("condition errors preserve data and disabled modes never call the predicate");
}
