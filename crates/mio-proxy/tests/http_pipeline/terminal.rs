use super::*;

#[tokio::test]
async fn stop_retains_terminal_request_when_events_are_full_or_closed() {
    for close_consumer in [false, true] {
        let mut origin = Origin::start(None, true).await;
        let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
        config.limits.max_connections = 1;
        let (sender, receiver) = mpsc::channel(1);
        let mut receiver = Some(receiver);
        let mut proxy = mio_proxy::start(
            config,
            ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
            sender,
        )
        .await
        .unwrap();
        let mut client = TcpStream::connect(proxy.address()).await.unwrap();
        client
            .write_all(
                format!(
                    "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\n",
                    origin.url(),
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        client.write_all(&[0, 0xff, b'T']).await.unwrap();
        match origin.next().await {
            OriginEvent::Request { body, .. } => assert_eq!(body, [0, 0xff, b'T']),
            event => panic!("expected the actual source request, got {event:?}"),
        }
        assert_eq!(receiver.as_ref().unwrap().len(), 1);
        if close_consumer {
            drop(receiver.take());
        }
        proxy.stop().await.unwrap();
        let mut byte = [0];
        assert!(matches!(
            tokio::time::timeout(DEADLINE, client.read(&mut byte))
                .await
                .unwrap(),
            Ok(0) | Err(_)
        ));
        assert!(matches!(origin.next().await, OriginEvent::Closed));
        assert_eq!(origin.received.load(Ordering::SeqCst), 1);
        let retained = proxy.drain_terminal_records();
        assert_eq!(
            retained.len(),
            1,
            "Stop must preserve the final record outside a saturated/disconnected ordinary event channel"
        );
        assert!(proxy.drain_terminal_records().is_empty());
        assert!(retained[0].response_stream.is_none());
        let record = &retained[0].record;
        assert_eq!(record.summary.state, FlowState::Cancelled);
        assert_eq!(record.request.body, [0, 0xff, b'T']);
        assert_eq!(record.summary.request_bytes, 3);
        assert_eq!(record.summary.url, origin.url());
        if let Some(mut receiver) = receiver {
            let ProxyEvent::Captured {
                record: initial, ..
            } = receiver.try_recv().unwrap()
            else {
                panic!("expected the initial pending record");
            };
            assert_eq!(initial.summary.state, FlowState::Pending);
            assert_eq!(initial.summary.id, record.summary.id);
            assert!(initial.summary.revision < record.summary.revision);
        }
        origin.stop().await;
    }
}
