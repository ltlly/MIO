use super::*;

#[tokio::test]
async fn multi_listeners_and_live_annotation_response_interception_work() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin_listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            for idx in 0..2 {
                let (mut stream, _) = origin_listener.accept().await.unwrap();
                let _ = receive_request(&mut stream).await;
                let body = format!("resp-{idx}");
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });

        let (runtime, mut client) = Runtime::start().unwrap();
        let project = project(&mut client).await;
        let mut workspace = Workspace {
            revision: 1,
            proxy_listeners: vec![
                mio_core::ProxyListenerConfig {
                    id: mio_core::Uuid::new_v4(),
                    enabled: true,
                    bind_address: "127.0.0.1:0".into(),
                    ..mio_core::ProxyListenerConfig::default()
                },
                mio_core::ProxyListenerConfig {
                    id: mio_core::Uuid::new_v4(),
                    enabled: true,
                    bind_address: "127.0.0.1:0".into(),
                    ..mio_core::ProxyListenerConfig::default()
                },
            ],
            ..Workspace::default()
        };
        workspace.proxy_interception.request = mio_core::ProxyInterceptRuleSet {
            enabled: true,
            rules: Vec::new(),
        };
        workspace.proxy_interception.response = mio_core::ProxyInterceptRuleSet {
            enabled: true,
            rules: vec![mio_core::ProxyInterceptRule {
                id: mio_core::Uuid::new_v4(),
                enabled: true,
                operator: mio_core::ProxyInterceptOperator::And,
                match_type: mio_core::ProxyInterceptType::Request,
                relationship: mio_core::ProxyInterceptRelation::WasAnnotated,
                condition: String::new(),
            }],
        };
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await
            .unwrap();
        event(&mut client, |e| matches!(e, Event::WorkspaceChanged { .. })).await;
        client
            .commands
            .send(Command::SetIntercept {
                project,
                requests: true,
                responses: true,
            })
            .await
            .unwrap();
        event(&mut client, |e| {
            matches!(e, Event::ProxyState { intercept_requests: true, .. })
        })
        .await;
        client
            .commands
            .send(Command::StartProxy {
                project,
                listen: "127.0.0.1:0".parse().unwrap(),
            })
            .await
            .unwrap();
        let Event::ProxyListeners { listeners, .. } = event(&mut client, |e| {
            matches!(e, Event::ProxyListeners { listeners, .. } if listeners.len() == 2)
        })
        .await
        else {
            unreachable!()
        };
        let addr_a = listeners[0].1;
        let addr_b = listeners[1].1;
        assert_ne!(addr_a.port(), addr_b.port());

        let send_via_proxy = |proxy_addr: std::net::SocketAddr, path: &'static str| {
            tokio::spawn(async move {
                let mut conn = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
                let req = format!(
                    "GET http://{origin_addr}{path} HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
                );
                conn.write_all(req.as_bytes()).await.unwrap();
                let mut resp = Vec::new();
                conn.read_to_end(&mut resp).await.unwrap();
                resp
            })
        };

        // Listener A: mark request as annotated -> response must be intercepted by WasAnnotated!
        let task_a = send_via_proxy(addr_a, "/first");
        let Event::InterceptPending {
            stage: mio_core::InterceptStage::Request,
            record: req_a,
            ..
        } = event(&mut client, |e| matches!(e, Event::InterceptPending { .. })).await
        else {
            panic!("expected request intercept on listener A")
        };
        client
            .commands
            .send(Command::SetLiveFlowAnnotated {
                project,
                flow_id: req_a.summary.id,
                annotated: true,
            })
            .await
            .unwrap();
        event(&mut client, |e| {
            matches!(e, Event::LiveFlowAnnotated { annotated: true, .. })
        })
        .await;
        client
            .commands
            .send(Command::Decide {
                project,
                flow_id: req_a.summary.id,
                revision: req_a.summary.revision,
                decision: mio_core::Decision::ForwardRequest(req_a.request.clone()),
            })
            .await
            .unwrap();
        let Event::InterceptPending {
            stage: mio_core::InterceptStage::Response,
            record: resp_a,
            ..
        } = event(&mut client, |e| matches!(e, Event::InterceptPending { .. })).await
        else {
            panic!("expected response intercept on annotated request")
        };
        client
            .commands
            .send(Command::Decide {
                project,
                flow_id: resp_a.summary.id,
                revision: resp_a.summary.revision,
                decision: mio_core::Decision::ForwardResponse(resp_a.response.unwrap()),
            })
            .await
            .unwrap();
        let bytes_a = task_a.await.unwrap();
        assert!(String::from_utf8_lossy(&bytes_a).ends_with("resp-0"));

        // Listener B: unannotated request -> response bypasses WasAnnotated interception!
        let task_b = send_via_proxy(addr_b, "/second");
        let Event::InterceptPending {
            stage: mio_core::InterceptStage::Request,
            record: req_b,
            ..
        } = event(&mut client, |e| matches!(e, Event::InterceptPending { .. })).await
        else {
            panic!("expected request intercept on listener B")
        };
        client
            .commands
            .send(Command::Decide {
                project,
                flow_id: req_b.summary.id,
                revision: req_b.summary.revision,
                decision: mio_core::Decision::ForwardRequest(req_b.request.clone()),
            })
            .await
            .unwrap();
        let bytes_b = task_b.await.unwrap();
        assert!(String::from_utf8_lossy(&bytes_b).ends_with("resp-1"));

        // Query history by each listener port
        for (idx, port) in [addr_a.port(), addr_b.port()].into_iter().enumerate() {
            let q_rev = (idx + 1) as u64;
            client
                .commands
                .send(Command::QueryHttpHistory {
                    query: mio_core::HttpHistoryQuery {
                        project,
                        query_revision: q_rev,
                        offset: 0,
                        limit: 10,
                        url_filter: String::new(),
                        filter: mio_core::HttpHistoryFilter {
                            listener_port: Some(port),
                            ..Default::default()
                        },
                        sort: mio_core::HttpHistorySort::default(),
                        selected_ids: Vec::new(),
                    },
                })
                .await
                .unwrap();
            let Event::HttpHistoryPage { result, .. } = event(&mut client, |e| {
                matches!(e, Event::HttpHistoryPage { query, .. } if query.query_revision == q_rev)
            })
            .await
            else {
                unreachable!()
            };
            assert_eq!(result.unwrap().total, 1);
        }

        client
            .commands
            .send(Command::StopProxy { project })
            .await
            .unwrap();
        event(&mut client, |e| {
            matches!(e, Event::ProxyState { address: None, .. })
        })
        .await;
        origin.await.unwrap();
        runtime.shutdown().unwrap();
    })
    .await
    .expect("multi-listener workflow must be bounded");
}
