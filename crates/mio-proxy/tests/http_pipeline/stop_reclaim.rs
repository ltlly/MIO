use super::stop::active_stop;
use super::*;

#[tokio::test]
async fn stop_reclaims_active_http_https_and_h2_with_silent_origins() {
    let mut origin = Origin::start(None, true).await;
    let (mut proxy, _, _events) = proxy(&origin, false).await;
    let stream = TcpStream::connect(proxy.address()).await.unwrap();
    active_stop(stream, false, &mut origin, &mut proxy).await;
    origin.stop().await;
    for h2 in [false, true] {
        let mut origin = Origin::start(Some((CertificateKind::Valid, h2)), true).await;
        let (mut proxy, ca, _events) = crate::proxy(&origin, false).await;
        let stream = tls_stream(
            proxy.address(),
            &format!("localhost:{}", origin.address.port()),
            ca,
            h2,
        )
        .await;
        active_stop(stream, h2, &mut origin, &mut proxy).await;
        origin.stop().await;
    }
}

#[tokio::test]
async fn stop_is_bounded_while_interception_event_queue_is_full() {
    let origin = Origin::start(None, false).await;
    let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    config.intercept_requests = true;
    let (events, receiver) = mpsc::channel(1);
    let mut proxy = mio_proxy::start(
        config,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events,
    )
    .await
    .unwrap();
    let mut stream = TcpStream::connect(proxy.address()).await.unwrap();
    stream
        .write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", origin.url()).as_bytes())
        .await
        .unwrap();
    tokio::time::timeout(DEADLINE, async {
        while receiver.len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    proxy.request_stop();
    assert_eq!(origin.received.load(Ordering::SeqCst), 0);
    let mut byte = [0];
    assert!(matches!(
        tokio::time::timeout(DEADLINE, stream.read(&mut byte))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    assert!(
        TcpStream::connect(proxy.address()).await.is_err(),
        "synchronous stop closes the listener without waiting for the event consumer"
    );
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn cancelling_one_h2_stream_does_not_cancel_its_sibling() {
    let mut origin = Origin::start(Some((CertificateKind::Valid, true)), true).await;
    let (mut proxy, ca, mut events) = proxy(&origin, false).await;
    let stream = tls_stream(
        proxy.address(),
        &format!("localhost:{}", origin.address.port()),
        ca,
        true,
    )
    .await;
    let (sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
            .await
            .unwrap();
    let driver = tokio::spawn(connection);
    let mut blocked_sender = sender.clone();
    let request = Request::builder()
        .uri(origin.url())
        .body(Full::new(Bytes::new()))
        .unwrap();
    let blocked = tokio::spawn(async move { blocked_sender.send_request(request).await });
    assert!(matches!(origin.next().await, OriginEvent::Request { .. }));
    let pending = match tokio::time::timeout(DEADLINE, events.recv())
        .await
        .unwrap()
        .unwrap()
    {
        ProxyEvent::Captured { record, .. } => record,
        event => panic!("expected pending History record, got {event:?}"),
    };
    assert_eq!(pending.summary.state, FlowState::Pending);
    assert_eq!(pending.request.url, origin.url());
    proxy.set_intercept(false, true);
    let mut sibling_sender = sender;
    let request = Request::builder()
        .uri(origin.url().replace("/test", "/quick"))
        .body(Full::new(Bytes::new()))
        .unwrap();
    let sibling = tokio::spawn(async move { sibling_sender.send_request(request).await });
    let (held, reply) = next_intercept(&mut events).await;
    assert_ne!(held.summary.id, pending.summary.id);
    assert!(held.request.url.ends_with("/quick"));
    assert_eq!(held.summary.state, FlowState::Intercepted);
    assert_eq!(held.response.as_ref().unwrap().body, [0, 0xff, b'O']);
    match origin.next().await {
        OriginEvent::Request { version, alpn, .. } => {
            assert_eq!(version, Version::HTTP_2);
            assert_eq!(alpn.as_deref(), Some(b"h2".as_slice()));
        }
        event => panic!("expected active sibling at the origin, got {event:?}"),
    }
    // The completed origin exchange closes before the response intercept is released.
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    blocked.abort();
    assert!(blocked.await.unwrap_err().is_cancelled());
    assert!(matches!(origin.next().await, OriginEvent::Closed));
    assert!(
        !reply.is_closed(),
        "reset must not cancel the already active sibling"
    );
    assert!(!sibling.is_finished());
    reply
        .send(InterceptDecision::ForwardResponse(held.response.unwrap()))
        .unwrap();
    let response = tokio::time::timeout(DEADLINE, sibling)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        &[0, 0xff, b'O']
    );
    tokio::time::timeout(DEADLINE, async {
        let mut cancelled = false;
        let mut completed_sibling = false;
        while !cancelled || !completed_sibling {
            if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap() {
                if record.summary.id == pending.summary.id {
                    assert_eq!(record.summary.state, FlowState::Cancelled);
                    assert!(record.summary.revision > pending.summary.revision);
                    assert_eq!(record.request, pending.request);
                    cancelled = true;
                } else if record.summary.id == held.summary.id
                    && record.summary.state == FlowState::Complete
                {
                    assert!(record.request.url.ends_with("/quick"));
                    assert!(record.summary.revision > held.summary.revision);
                    completed_sibling = true;
                }
            }
        }
    })
    .await
    .expect("cancelled H2 stream must reach a History terminal state before Proxy Stop");
    assert_eq!(origin.received.load(Ordering::SeqCst), 2);
    proxy.stop().await.unwrap();
    driver.abort();
    let _ = driver.await;
    origin.stop().await;
}
