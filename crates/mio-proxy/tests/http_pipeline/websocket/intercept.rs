use super::*;
use std::sync::Mutex;

#[tokio::test]
async fn websocket_handshake_request_and_response_interception_forward_real_edits() {
    let mut origin = WsOrigin::start(None).await;
    let (mut proxy, _, mut events) = ws_proxy_with(&origin, 32, |config| {
        config.intercept_requests = true;
        config.intercept_responses = true;
    })
    .await;
    let address = proxy.address();
    let url = origin.url().replacen("http://", "ws://", 1);
    let client = tokio::spawn(async move {
        let mut client = TcpStream::connect(address).await.unwrap();
        let response = ws_handshake(&mut client, &url).await;
        (client, response)
    });
    let (record, reply) = next_intercept(&mut events).await;
    assert!(record.response.is_none());
    let mut request = record.request;
    request.url = format!(
        "ws://localhost:{}/edited?from=intercept",
        origin.address.port()
    );
    reply
        .send(InterceptDecision::ForwardRequest(request))
        .unwrap();
    let (record, reply) = next_intercept(&mut events).await;
    let mut response = record.response.unwrap();
    assert_eq!(response.status, 101);
    response.headers.push(Header {
        name: "x-ws-response".into(),
        value: b"edited-response".to_vec(),
    });
    reply
        .send(InterceptDecision::ForwardResponse(response))
        .unwrap();
    let (mut client, response) = tokio::time::timeout(DEADLINE, client)
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 101"));
    assert!(
        String::from_utf8(response)
            .unwrap()
            .to_ascii_lowercase()
            .contains("x-ws-response: edited-response")
    );
    match origin.next().await {
        WsEvent::Handshake { path, .. } => assert_eq!(path, "/edited?from=intercept"),
        event => panic!("expected edited origin handshake, got {event:?}"),
    }
    assert_eq!(
        next_frame(&mut client).await,
        Frame::new(1, b"server-first".to_vec())
    );
    proxy.stop().await.unwrap();
    origin.stop().await;
}

#[tokio::test]
async fn websocket_conditional_predicate_shares_the_actual_handshake_flags() {
    tokio::time::timeout(DEADLINE, async {
        let cpu_budget = Duration::from_secs(2);
        let body_limit = 256;
        let mut origin = WsOrigin::start(None).await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let (mut proxy, _, mut events) = ws_proxy_with(&origin, 32, |config| {
            config.intercept_requests = true;
            config.intercept_responses = true;
            config.request_timeout = cpu_budget;
            config.max_body_bytes = body_limit;
            config.intercept_filter = Some(Arc::new(
                move |_,
                      response_stage,
                      head_only,
                      held,
                      changed,
                      ctx,
                      cancel,
                      (cpu_deadline, max_body_bytes)| {
                    assert!(!cancel.is_cancelled());
                    captured.lock().unwrap().push((
                        (response_stage, head_only, held, changed, ctx.listener_port),
                        cancel.clone(),
                        std::time::Instant::now(),
                        cpu_deadline,
                        max_body_bytes,
                    ));
                    Ok(true)
                },
            ));
        })
        .await;
        let address = proxy.address();
        let listener_port = address.port();
        assert_ne!(listener_port, 0);
        let proxy_cancel = proxy.cancellation_token();
        let url = origin.url().replacen("http://", "ws://", 1);
        let client = tokio::spawn(async move {
            let mut client = TcpStream::connect(address).await.unwrap();
            let response = ws_handshake(&mut client, &url).await;
            (client, response)
        });
        let (record, reply) = next_intercept(&mut events).await;
        assert!(record.response.is_none());
        let flow_id = record.summary.id;
        let mut request = record.request;
        request.url = format!("ws://localhost:{}/filtered-edit", origin.address.port());
        reply
            .send(InterceptDecision::ForwardRequest(request))
            .unwrap();
        let (record, reply) = next_intercept(&mut events).await;
        assert_eq!(record.summary.id, flow_id);
        let response = record.response.unwrap();
        assert_eq!(response.status, 101);
        reply
            .send(InterceptDecision::ForwardResponse(response))
            .unwrap();
        let (mut client, response) = tokio::time::timeout(DEADLINE, client)
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 101"));
        {
            let calls = seen.lock().unwrap();
            assert_eq!(
                calls.iter().map(|(flags, ..)| *flags).collect::<Vec<_>>(),
                vec![
                    (false, false, false, false, listener_port),
                    (true, false, true, true, listener_port),
                ]
            );
            for (_, cancel, observed, cpu_deadline, max_body_bytes) in calls.iter() {
                assert!(!cancel.is_cancelled());
                assert!(*cpu_deadline > *observed);
                assert!(*cpu_deadline <= *observed + cpu_budget);
                assert_eq!(*max_body_bytes, body_limit);
            }
        }
        assert_eq!(
            next_frame(&mut client).await,
            Frame::new(1, b"server-first".to_vec())
        );
        match origin.next().await {
            WsEvent::Handshake {
                path,
                upgrade,
                connection,
                key,
                ..
            } => {
                assert_eq!(path, "/filtered-edit");
                assert!(upgrade && connection && key);
            }
            event => panic!("expected the edited handshake, got {event:?}"),
        }
        let close = Frame::new(8, 1000_u16.to_be_bytes().to_vec());
        client.write_all(&close.encode(true)).await.unwrap();
        assert_origin_frame(&mut origin, &close).await;
        origin.commands.send(close.clone()).await.unwrap();
        assert_eq!(next_frame(&mut client).await, close);
        let terminal = terminal_record(&mut events).await;
        assert_eq!(terminal.summary.id, flow_id);
        assert_eq!(terminal.summary.state, FlowState::Complete);
        assert!(matches!(origin.next().await, WsEvent::Closed));
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
    })
    .await
    .expect("WebSocket handshake predicate preserves actual hold and edits");
}
