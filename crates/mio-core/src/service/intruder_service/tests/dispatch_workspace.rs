use super::*;

#[test]
fn start_requires_an_open_persisted_intruder_tab() {
    let (mut state, _events) = state();
    let id = Uuid::new_v4();
    let result = state.admit_intruder(
        state.project,
        id,
        Uuid::new_v4(),
        spec("http://127.0.0.1:1".into(), 1),
        CancellationToken::new(),
    );
    assert!(result.is_err(), "an unsaved Intruder tab must not start");
    assert!(state.store.find_intruder_run(id).unwrap().is_none());
    assert!(!state.intruder.has_resources());
}

#[test]
fn start_rejects_a_zero_request_configuration() {
    let (mut state, _events) = state();
    let id = Uuid::new_v4();
    let input = spec("http://127.0.0.1:1".into(), 0);
    let tab = bind(&mut state.store, &input);
    let result = state.admit_intruder(state.project, id, tab, input, CancellationToken::new());
    assert!(result.unwrap_err().to_string().contains("payload 列表为空"));
    assert!(state.store.find_intruder_run(id).unwrap().is_none());
    assert!(!state.intruder.has_resources());
}

#[tokio::test]
async fn invalid_dispatch_workspace_never_falls_back_and_recovery_does_not_send() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for mutation in 0..5 {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (mut state, mut events) = state();
            let id = admit(
                &mut state,
                spec(format!("http://{}", listener.local_addr().unwrap()), 2),
            );
            let connection = rusqlite::Connection::open(state.store.test_actual_path()).unwrap();
            let original: Vec<u8> = connection
                .query_row("SELECT value FROM metadata WHERE key='workspace'", [], |row| row.get(0))
                .unwrap();
            let preferences_directory = tempfile::tempdir().unwrap();
            let preferences_path = preferences_directory.path().canonicalize().unwrap().join("preferences.json");
            let invalid = if mutation == 0 {
                b"invalid JSON".to_vec()
            } else if mutation == 4 {
                assert!(state.store.workspace().unwrap().upstream_tls.is_default());
                state.preferences = crate::preferences::PreferencesStore::new(Some(preferences_path.clone()));
                let certificates = vec![mio_proxy::MitmAuthority::generate().unwrap().certificate_der()];
                let save_operation = Uuid::new_v4();
                state.command(Command::SetUserTlsCertificates {
                    operation_id: save_operation,
                    expected_revision: 0,
                    certificates: certificates.clone(),
                }).await.unwrap();
                assert!(matches!(events.try_recv().unwrap(), Event::UserPreferencesSaved {
                    operation_id,
                    result: Ok(saved),
                } if operation_id == save_operation && saved.revision == 1 && saved.custom_ca_ders == certificates));
                std::fs::write(&preferences_path, b"broken confirmed user TLS preferences").unwrap();
                let query_operation = Uuid::new_v4();
                state.command(Command::QueryUserPreferences { operation_id: query_operation }).await.unwrap();
                assert!(matches!(events.try_recv().unwrap(), Event::UserPreferences {
                    operation_id,
                    state: confirmed,
                } if operation_id == query_operation && confirmed.load_error.is_some()));
                original.clone()
            } else {
                let mut workspace: serde_json::Value = serde_json::from_slice(&original).unwrap();
                match mutation {
                    1 => workspace["upstream_tls"] = serde_json::json!({"custom_ca_ders": [[48, 0]]}),
                    2 => workspace["upstream_tls"] = serde_json::json!({"include_system_roots": false}),
                    _ => {
                        workspace["upstream_proxy"] = serde_json::json!({
                            "rules": [{
                                "id": Uuid::new_v4(),
                                "enabled": true,
                                "destination_host": "*",
                                "protocol": "http",
                                "host": "",
                                "port": 8080
                            }]
                        })
                    }
                }
                serde_json::to_vec(&workspace).unwrap()
            };
            connection
                .execute("UPDATE metadata SET value=?1 WHERE key='workspace'", [&invalid])
                .unwrap();
            if mutation == 4 {
                assert!(state.store.workspace().is_ok(), "only the confirmed user profile must be broken");
            } else {
                let error = state.store.workspace().unwrap_err().to_string();
                assert!(error.contains(match mutation { 0 => "JSON", 1 => "DER", 2 => "系统根", _ => "上游代理" }), "fixture must invalidate the intended dispatch configuration: {error}");
            }
            let error = state.advance_intruder(id).await.unwrap_err().to_string();
            assert!(error.contains("未发送"), "a failed config read cannot admit a default Direct/System send: {error}");
            if mutation == 4 {
                assert!(error.contains("用户 TLS 设置未确认"), "the failed dispatch must not use System instead of the broken confirmed user profile: {error}");
                assert_eq!(std::fs::read(&preferences_path).unwrap(), b"broken confirmed user TLS preferences");
            }
            assert!(state.intruder.jobs.is_empty());
            assert!(state.intruder.requests.is_empty());
            assert!(state.intruder.runs[&id].cancel.is_cancelled());
            assert_eq!(state.intruder_run(id).unwrap().state, IntruderRunState::Stopping);
            state.advance_intruder(id).await.unwrap();
            let failed = state.intruder_result_page(id, None, 2).unwrap();
            assert!(matches!(&failed.run.state, IntruderRunState::Failed(message) if message.contains("配置") && message.contains("未发送")));
            assert_eq!((failed.run.dispatched, failed.run.completed), (0, 0));
            assert!(failed.rows.is_empty());
            assert!(!state.intruder.has_resources());
            let still_invalid: Vec<u8> = connection
                .query_row("SELECT value FROM metadata WHERE key='workspace'", [], |row| row.get(0))
                .unwrap();
            assert_eq!(still_invalid, invalid, "failed dispatch must not rewrite unknown/broken metadata");
            connection
                .execute("UPDATE metadata SET value=?1 WHERE key='workspace'", [&original])
                .unwrap();
            drop(connection);
            let directory = tempfile::tempdir().unwrap();
            let saved = directory.path().join("unsent-invalid-config.mio");
            save(&mut state, Some(saved.clone())).await.unwrap();
            assert_eq!(state.intruder_result_page(id, None, 2).unwrap(), failed);
            let reopened = ProjectStore::open(&saved).unwrap();
            assert_eq!(reopened.intruder_results(id, None, 2).unwrap(), failed);
            reopened.close().unwrap();
            if mutation == 4 {
                assert_eq!(std::fs::read(preferences_path).unwrap(), b"broken confirmed user TLS preferences");
            }
            assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        }
    })
    .await
    .expect("invalid workspace rejection, local recovery and no-send checks are bounded");
}

#[tokio::test]
async fn invalid_dispatch_workspace_cancels_owned_post_and_keeps_completed_results() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut first).await, 0xff);
            first.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\0\xffR").await.unwrap();
            assert_eq!(first.read(&mut [0]).await.unwrap(), 0);
            let (mut second, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut second).await, 0xff);
            seen.send(()).unwrap();
            assert_eq!(second.read(&mut [0]).await.unwrap(), 0, "the rejected next dispatch must cancel its owned in-flight POST");
            listener
        });
        let (mut state, _events) = state();
        let id = admit(&mut state, spec(format!("http://{address}"), 3));
        state.advance_intruder(id).await.unwrap();
        let completed = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completed).await.unwrap();
        let before = state.intruder_result_page(id, None, 3).unwrap();
        let IntruderResultOutcome::Flow(first) = &before.rows[0].outcome else { panic!("the completed binary response must be readable") };
        let original_record = state.read_flow(first.id).unwrap();
        assert_eq!(original_record.response.as_ref().unwrap().body, [0, 255, b'R']);
        assert!(state.intruder.can_accept_commands());
        state.advance_intruder(id).await.unwrap();
        received.await.unwrap();
        let connection = rusqlite::Connection::open(state.store.test_actual_path()).unwrap();
        let original: Vec<u8> = connection
            .query_row("SELECT value FROM metadata WHERE key='workspace'", [], |row| row.get(0))
            .unwrap();
        connection.execute("UPDATE metadata SET value=?1 WHERE key='workspace'", [b"invalid JSON".as_slice()]).unwrap();
        assert!(state.store.workspace().is_err());
        assert!(state.advance_intruder(id).await.is_err());
        assert_eq!(state.intruder.jobs.len(), 1, "only the already-owned request may remain before joining");
        assert_eq!(state.intruder.runs[&id].active, 1);
        assert!(state.intruder.runs[&id].cancel.is_cancelled());
        let cancelled = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(cancelled).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        let listener = origin.await.unwrap();
        let failed = state.intruder_result_page(id, None, 3).unwrap();
        assert!(matches!(&failed.run.state, IntruderRunState::Failed(message) if message.contains("配置") && message.contains("未发送")));
        assert_eq!((failed.run.dispatched, failed.run.completed, failed.rows.len()), (2, 2, 2));
        assert_eq!(state.read_flow(first.id).unwrap(), original_record);
        assert!(matches!(&failed.rows[1].outcome, IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled));
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        connection.execute("UPDATE metadata SET value=?1 WHERE key='workspace'", [&original]).unwrap();
        drop(connection);
        let directory = tempfile::tempdir().unwrap();
        let copy = directory.path().join("failed-config-results.mio");
        save(&mut state, Some(copy.clone())).await.unwrap();
        let reopened = ProjectStore::open(&copy).unwrap();
        assert_eq!(reopened.intruder_results(id, None, 3).unwrap(), failed);
        assert_eq!(reopened.read(first.id).unwrap(), original_record);
        reopened.close().unwrap();
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("config rejection cancels the live socket, joins and preserves binary results");
}

#[tokio::test]
async fn intruder_workspace_tls_trust_is_used_by_actual_send_and_strictly_rejects_other_roots() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let ca = mio_proxy::MitmAuthority::generate().unwrap();
        let good_tls = crate::UpstreamTlsSettings {
            custom_ca_ders: vec![ca.certificate_der()],
            include_system_roots: false,
            insecure_skip_verify: false,
            ..Default::default()
        };
        let wrong_tls = crate::UpstreamTlsSettings {
            custom_ca_ders: vec![mio_proxy::MitmAuthority::generate().unwrap().certificate_der()],
            include_system_roots: false,
            insecure_skip_verify: false,
            ..Default::default()
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let target = format!("https://127.0.0.1:{}/tls-root", address.port());
        let forwarded_target = format!("http://{address}/intruder");
        let mut config = mio_proxy::ProxyConfig::new(ca);
        config.limits.request_timeout = Duration::from_secs(2);
        config.limits.stop_timeout = Duration::from_secs(2);
        let (proxy_events, _proxy_receiver) = mpsc::channel(64);
        let mut fixture_proxy = mio_proxy::start(config, ProjectKey { id: Uuid::new_v4(), generation: 1 }, proxy_events).await.unwrap();
        fixture_proxy.set_transform(Some(std::sync::Arc::new(move |record, response, _, cancel, (deadline, max)| {
            assert!(!cancel.is_cancelled());
            assert!(deadline > std::time::Instant::now());
            assert_eq!(max, 16 * 1024 * 1024);
            if response { return Ok(false); }
            record.request.url = forwarded_target.clone();
            record.request.version = HttpVersion::Http11;
            Ok(true)
        })));
        let origin = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, peer) = listener.accept().await.unwrap();
                assert!(peer.ip().is_loopback());
                assert_eq!(request(&mut stream).await, 255);
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\0\xfeR").await.unwrap();
                assert_eq!(stream.read(&mut [0]).await.unwrap(), 0, "the real binary POST cannot be retried on its connection");
            }
            listener
        });
        let (mut state, mut events) = state();
        let mut observed = Vec::new();
        for (tls, user_certificates, succeeds) in [
            (good_tls.clone(), Vec::new(), true),
            (crate::UpstreamTlsSettings::default(), Vec::new(), false),
            (wrong_tls.clone(), Vec::new(), false),
            (crate::UpstreamTlsSettings::default(), good_tls.custom_ca_ders.clone(), true),
            (wrong_tls.clone(), good_tls.custom_ca_ders.clone(), false),
            (good_tls, wrong_tls.custom_ca_ders.clone(), true),
        ] {
            while events.try_recv().is_ok() {}
            let operation_id = Uuid::new_v4();
            let expected_revision = state.preferences.query().preferences.revision;
            state.command(Command::SetUserTlsCertificates {
                operation_id,
                expected_revision,
                certificates: user_certificates.clone(),
            }).await.unwrap();
            assert!(matches!(events.try_recv().unwrap(), Event::UserPreferencesSaved {
                operation_id: actual,
                result: Ok(saved),
            } if actual == operation_id && saved.revision == expected_revision + 1 && saved.custom_ca_ders == user_certificates));
            let mut workspace = state.store.workspace().unwrap();
            workspace.revision += 1;
            workspace.upstream_proxy = crate::UpstreamProxySettings {
                rules: vec![crate::UpstreamProxyRule {
                    protocol: crate::UpstreamProxyProtocol::Http,
                    host: "127.0.0.1".into(),
                    port: fixture_proxy.address().port(),
                    ..crate::UpstreamProxyRule::default()
                }],
                ..crate::UpstreamProxySettings::default()
            };
            workspace.upstream_tls = tls;
            state.store.save_workspace(&workspace).unwrap();
            let input = spec(target.clone(), 1);
            let tab = bind(&mut state.store, &input);
            let id = Uuid::new_v4();
            state.command(Command::StartIntruder {
                project: state.project,
                run_id: id,
                tab_id: tab,
                spec: Box::new(input),
                cancel: CancellationToken::new(),
            }).await.unwrap();
            assert!(matches!(events.try_recv().unwrap(), Event::IntruderStarted { run_id, result: Ok(_), .. } if run_id == id));
            while state.intruder.runs.contains_key(&id) {
                let signal = state.intruder.next_signal(&state.events, false).await;
                state.intruder_signal(signal).await.unwrap();
            }
            let page = state.intruder_result_page(id, None, 1).unwrap();
            assert_eq!((page.run.dispatched, page.run.completed), (1, 1));
            let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else { panic!("TLS result must retain its actual request detail") };
            let record = state.read_flow(summary.id).unwrap();
            assert_eq!(record.request.body, [255]);
            if succeeds {
                assert_eq!(record.summary.state, FlowState::Complete);
                assert_eq!(record.response.as_ref().unwrap().body, [0, 254, b'R']);
            } else {
                let FlowState::Failed(message) = &record.summary.state else {
                    panic!("untrusted TLS must fail with a certificate error: {:?}", record.summary.state);
                };
                assert!(message.contains("UnknownIssuer") || message.contains("invalid peer certificate") || message.contains("证书"), "a timeout or refused connection cannot stand in for certificate rejection: {message}");
                assert!(record.response.is_none(), "untrusted TLS must fail before HTTP response delivery");
            }
            assert!(state.intruder.requests.is_empty());
            assert!(!state.intruder.has_resources());
            observed.push((id, page, record));
        }
        fixture_proxy.stop().await.unwrap();
        assert!(fixture_proxy.is_joined());
        let listener = origin.await.unwrap();
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock, "wrong roots and System must not fall back to a direct origin connection");
        let directory = tempfile::tempdir().unwrap();
        let copy = directory.path().join("tls-intruder.mio");
        save(&mut state, Some(copy.clone())).await.unwrap();
        let reopened = ProjectStore::open(&copy).unwrap();
        for (id, page, record) in observed {
            assert_eq!(reopened.intruder_results(id, None, 1).unwrap(), page);
            assert_eq!(reopened.read(record.summary.id).unwrap(), record);
        }
        reopened.close().unwrap();
    })
    .await
    .expect("strict private-root and user TLS send, six retained results and all joins are bounded");
}
