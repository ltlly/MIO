use super::*;

#[test]
fn proxy_and_repeater_workspace_tls_trust_is_used_by_actual_commands_and_survives_save_as() {
    std::thread::Builder::new()
        .name("tls-trust-test".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(run_tls_trust_test());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn run_tls_trust_test() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let fixture_ca = MitmAuthority::generate().unwrap();
        let good_tls = UpstreamTlsSettings {
            custom_ca_ders: vec![fixture_ca.certificate_der()],
            include_system_roots: false,
            insecure_skip_verify: false,
            ..Default::default()
        };
        let wrong_tls = UpstreamTlsSettings {
            custom_ca_ders: vec![MitmAuthority::generate().unwrap().certificate_der()],
            include_system_roots: false,
            insecure_skip_verify: false,
            ..Default::default()
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let target = format!("https://127.0.0.1:{}/workspace-tls", address.port());
        let forwarded_target = format!("http://{address}/workspace-tls");
        let mut config = ProxyConfig::new(fixture_ca);
        config.limits.request_timeout = Duration::from_secs(2);
        config.limits.stop_timeout = Duration::from_secs(2);
        let (fixture_events, _fixture_receiver) = mpsc::channel(64);
        let mut fixture_proxy = mio_proxy::start(
            config,
            ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
            fixture_events,
        )
        .await
        .unwrap();
        fixture_proxy.set_transform(Some(std::sync::Arc::new(move |record, response, _, cancel, (deadline, max)| {
            assert!(!cancel.is_cancelled());
            assert!(deadline > std::time::Instant::now());
            assert_eq!(max, 16 * 1024 * 1024);
            if response {
                return Ok(false);
            }
            record.request.url = forwarded_target.clone();
            record.request.version = HttpVersion::Http11;
            Ok(true)
        })));
        let origin = tokio::spawn(async move {
            for _ in 0..6 {
                let (mut stream, peer) = listener.accept().await.unwrap();
                assert!(peer.ip().is_loopback());
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    assert!(head.len() < 4096);
                    head.push(stream.read_u8().await.unwrap());
                }
                assert!(head.starts_with(b"POST /workspace-tls HTTP/1.1\r\n"));
                let length = std::str::from_utf8(&head)
                    .unwrap()
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .1
                    .trim()
                    .parse::<usize>()
                    .unwrap();
                assert_eq!(length, PAYLOAD.len());
                let mut body = [0; 3];
                stream.read_exact(&mut body).await.unwrap();
                assert_eq!(body, PAYLOAD);
                stream
                    .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                stream.write_all(RESPONSE).await.unwrap();
                assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            }
            listener
        });
        let (events, mut receiver) = mpsc::channel(128);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let product_ca = MitmAuthority::generate().unwrap();
        let product_ca_der = product_ca.certificate_der();
        state.proxy.ca = Some(product_ca);
        let mut request = tests::completed_flow().request;
        request.url = target.clone();
        request.body = PAYLOAD.to_vec();
        let mut workspace = state.store.workspace().unwrap();
        let tab_id = workspace.open_request(Some(&request)).unwrap();
        let draft = workspace.tabs.iter().find(|tab| tab.id == tab_id).unwrap().draft.clone();
        state.command(Command::SaveWorkspace { project: state.project, workspace }).await.unwrap();
        let mut observed = Vec::new();
        for (tls, user_certificates, succeeds) in [
            (good_tls.clone(), Vec::new(), true),
            (UpstreamTlsSettings::default(), Vec::new(), false),
            (wrong_tls.clone(), Vec::new(), false),
            (UpstreamTlsSettings::default(), good_tls.custom_ca_ders.clone(), true),
            (wrong_tls.clone(), good_tls.custom_ca_ders.clone(), false),
            (good_tls, wrong_tls.custom_ca_ders.clone(), true),
        ] {
            drain_events(&mut receiver);
            let operation_id = Uuid::new_v4();
            let expected_revision = state.preferences.query().preferences.revision;
            state.command(Command::SetUserTlsCertificates {
                operation_id,
                expected_revision,
                certificates: user_certificates.clone(),
            }).await.unwrap();
            assert!(matches!(receiver.try_recv().unwrap(), Event::UserPreferencesSaved {
                operation_id: actual,
                result: Ok(saved),
            } if actual == operation_id && saved.revision == expected_revision + 1 && saved.custom_ca_ders == user_certificates));
            let mut workspace = state.store.workspace().unwrap();
            workspace.revision += 1;
            workspace.upstream_proxy = UpstreamProxySettings {
                rules: vec![crate::UpstreamProxyRule {
                    protocol: UpstreamProxyProtocol::Http,
                    host: "127.0.0.1".into(),
                    port: fixture_proxy.address().port(),
                    ..crate::UpstreamProxyRule::default()
                }],
                ..UpstreamProxySettings::default()
            };
            workspace.upstream_tls = tls;
            for set in [&mut workspace.proxy_interception.request, &mut workspace.proxy_interception.response] {
                set.enabled = succeeds;
                set.rules = vec![ProxyInterceptRule {
                    id: Uuid::new_v4(), enabled: true, operator: ProxyInterceptOperator::And,
                    match_type: ProxyInterceptType::Protocol, relationship: ProxyInterceptRelation::IsHttps,
                    condition: String::new(),
                }];
            }
            state.command(Command::SaveWorkspace { project: state.project, workspace: workspace.clone() }).await.unwrap();
            assert!(matches!(receiver.try_recv().unwrap(), Event::WorkspaceChanged { project, workspace: saved }
                if project == state.project && saved == workspace));
            let run_id = Uuid::new_v4();
            state.command(Command::Send {
                project: state.project,
                tab_id,
                run_id,
                draft: draft.clone(),
                settings: RepeaterSettings { request_timeout_secs: 2, ..RepeaterSettings::default() },
                group_send: None,
                cancel: CancellationToken::new(),
            }).await.unwrap();
            let completion = state.repeater.jobs.join_next().await.unwrap().unwrap();
            let repeater_record = completion.record.clone();
            assert_eq!(completion.tab_id, tab_id);
            assert_eq!(completion.draft_revision, draft.revision);
            assert_eq!(repeater_record.summary.id, run_id);
            state.repeater_completed(Ok(completion)).await;
            state.flush_repeater_notifications().await;
            assert!(drain_events(&mut receiver).iter().any(|event| matches!(event, Event::SendFinished {
                project, tab_id: actual_tab, draft_revision, persisted: true, record,
            } if *project == state.project && *actual_tab == tab_id && *draft_revision == draft.revision && **record == repeater_record)));
            assert!(state.repeater.jobs.is_empty() && state.repeater.sends.is_empty());

            state.command(Command::SetIntercept { project: state.project, requests: succeeds, responses: succeeds }).await.unwrap();
            assert!(drain_events(&mut receiver).iter().any(|event| matches!(event, Event::ProxyState {
                project, intercept_requests, intercept_responses, ..
            } if *project == state.project && *intercept_requests == succeeds && *intercept_responses == succeeds)));
            state.command(Command::StartProxy {
                project: state.project,
                listen: "127.0.0.1:0".parse().unwrap(),
            }).await.unwrap();
            let product_address = state.proxy.primary.as_ref().unwrap().address();
            assert!(drain_events(&mut receiver).iter().any(|event| matches!(event, Event::ProxyState {
                project, address: Some(actual), ..
            } if *project == state.project && *actual == product_address)));
            let client_send = tokio::spawn(mio_proxy::send(
                request.clone(),
                SendConfig {
                    network: mio_proxy::SendNetworkConfig {
                        upstream_route: mio_proxy::UpstreamRoute::Http(mio_proxy::ProxyEndpoint {
                            host: "127.0.0.1".into(),
                            port: product_address.port(),
                            credentials: None,
                        }),
                        ..Default::default()
                    },
                    tls: mio_proxy::UpstreamTlsConfig {
                        trust: mio_proxy::UpstreamTls::CustomRoots {
                            certificates_der: vec![product_ca_der.clone()],
                            include_system: false,
                        },
                        ..Default::default()
                    },
                    limits: mio_proxy::SendLimits {
                        max_body_bytes: 1024,
                        timeout: Duration::from_secs(2),
                        ..Default::default()
                    },
                    http: mio_proxy::SendHttpConfig {
                        version: Some(HttpVersion::Http11),
                        ..Default::default()
                    },
                    ..SendConfig::default()
                },
                CancellationToken::new(),
            ));
            let mut held_stages = Vec::new();
            let proxy_record = loop {
                let event = state.proxy.events.recv().await.unwrap();
                let intercepted = match &event {
                    ProxyEvent::Intercepted { project, record, response_head_only, .. } => {
                        assert_eq!(*project, state.project);
                        assert!(succeeds, "untrusted root scenarios must not invent a response interception");
                        assert!(!response_head_only);
                        assert_eq!(record.request.url, target);
                        assert_eq!(record.request.body, PAYLOAD);
                        let stage = if let Some(response) = &record.response {
                            assert_eq!(response.status, 201);
                            assert_eq!(response.body, RESPONSE);
                            (InterceptStage::Response, Decision::ForwardResponse(response.clone()))
                        } else {
                            (InterceptStage::Request, Decision::ForwardRequest(record.request.clone()))
                        };
                        Some((record.summary.id, record.summary.revision, stage.0, stage.1))
                    }
                    _ => None,
                };
                let terminal = match &event {
                    ProxyEvent::Captured { project, record, .. }
                        if *project == state.project
                            && record.request.method == "POST"
                            && record.request.url == target
                            && record.request.body == PAYLOAD
                            && !matches!(record.summary.state, FlowState::Pending | FlowState::Intercepted) => Some(record.clone()),
                    _ => None,
                };
                state.capture(event).await.unwrap();
                if let Some((flow_id, revision, stage, decision)) = intercepted {
                    assert!(drain_events(&mut receiver).iter().any(|event| matches!(event,
                        Event::InterceptPending { project, stage: actual, response_head_only: false, record }
                            if *project == state.project && *actual == stage && record.summary.id == flow_id && record.summary.revision == revision)));
                    state.command(Command::Decide { project: state.project, flow_id, revision, decision }).await.unwrap();
                    held_stages.push(stage);
                }
                if let Some(record) = terminal {
                    break record;
                }
            };
            assert_eq!(held_stages, if succeeds {
                vec![InterceptStage::Request, InterceptStage::Response]
            } else { Vec::new() });
            let client_response = client_send.await.unwrap().unwrap();
            assert_eq!(client_response.status, if succeeds { 201 } else { 502 });
            if succeeds {
                assert_eq!(client_response.body, RESPONSE);
            }
            state.command(Command::StopProxy { project: state.project }).await.unwrap();
            state.flush_proxy_notifications().await;
            assert!(state.proxy.primary.is_none() && state.proxy.events.is_empty() && state.proxy.pending.is_empty());
            assert!(drain_events(&mut receiver).iter().any(|event| matches!(event, Event::ProxyState {
                project, address: None, cancel: None, ..
            } if *project == state.project)));
            for (record, source) in [
                (repeater_record, TrafficSource::Repeater),
                (proxy_record, TrafficSource::Proxy),
            ] {
                assert_eq!(record.summary.source, source);
                assert_eq!(record.request.method, "POST");
                assert_eq!(record.request.url, target);
                assert_eq!(record.request.version, HttpVersion::Http11);
                assert_eq!(record.request.body, PAYLOAD);
                if succeeds {
                    assert_eq!(record.summary.state, FlowState::Complete);
                    assert_eq!(record.summary.status, Some(201));
                    assert_eq!(record.response.as_ref().unwrap().body, RESPONSE);
                } else {
                    let FlowState::Failed(message) = &record.summary.state else {
                        panic!("untrusted TLS must retain its certificate error: {:?}", record.summary.state);
                    };
                    assert!(message.contains("UnknownIssuer") || message.contains("invalid peer certificate") || message.contains("证书"),
                        "a timeout or refused connection cannot stand in for certificate rejection: {message}");
                    assert!(record.response.is_none());
                }
                assert_eq!(state.store.read(record.summary.id).unwrap(), record);
                observed.push(record);
            }
        }
        fixture_proxy.stop().await.unwrap();
        assert!(fixture_proxy.is_joined());
        let listener = origin.await.unwrap();
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock,
            "System and wrong roots cannot establish extra direct or retried origin requests");
        let workspace = state.store.workspace().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("proxy-repeater-tls.mio");
        state.command(Command::SaveProject {
            project: state.project,
            path: Some(path.clone()),
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        }).await.unwrap();
        state.store.close().unwrap();
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(observed.len(), 12);
        for record in observed {
            assert_eq!(reopened.read(record.summary.id).unwrap(), record);
        }
        reopened.close().unwrap();
    }).await.expect("twelve actual TLS sends, independent user and legacy roots, Save As and all joins fit the shared 8s budget");
}
