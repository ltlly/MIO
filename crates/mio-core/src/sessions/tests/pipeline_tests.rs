use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::intruder::{AttackType, Position};

#[tokio::test]
async fn proxy_repeater_and_intruder_pipelines_inject_sessions_and_fail_closed_without_direct_send()
{
    tokio::time::timeout(Duration::from_secs(10), async {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("sessions_pipeline.mio");
        let (
            auth_addr,
            fail_macro,
            auth_srv,
            target_addr,
            target_hits,
            seen_requests,
            target_srv,
        ) = spawn_auth_and_target_servers().await;

        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened {
            project,
            mut workspace,
            ..
        } = next_matching(&mut client, |e| matches!(e, Event::ProjectOpened { .. })).await
        else {
            unreachable!()
        };
        save_project_to_disk(&mut client, project, &project_path, &mut workspace).await;

        let macro_id = Uuid::new_v4();
        let rule_id = Uuid::new_v4();
        let rep_tab_id = Uuid::new_v4();
        let rep_draft = Draft {
            revision: 1,
            target: format!("http://{target_addr}"),
            raw: format!("GET /rep HTTP/1.1\r\nHost: {target_addr}\r\n\r\n").into_bytes(),
            version: HttpVersion::Http11,
        };
        let intr_raw = format!("GET /intr?item=§1§ HTTP/1.1\r\nHost: {target_addr}\r\n\r\n");
        let start_pos = intr_raw.find("§1§").unwrap();
        let clean_intr_raw = intr_raw.replace("§1§", "1").into_bytes();
        let intr_spec = IntruderSpec {
            revision: 1,
            draft: Draft {
                revision: 1,
                target: format!("http://{target_addr}"),
                raw: clean_intr_raw,
                version: HttpVersion::Http11,
            },
            attack_type: AttackType::Sniper,
            positions: vec![Position {
                start: start_pos,
                end: start_pos + 1,
            }],
            payload_sets: vec![vec![b"p1".to_vec()]],
            max_concurrent_requests: 1,
            request_timeout_ms: 5000,
            update_host_header: true,
            save_to_project: true,
            baseline_request: false,
            resource_pool: None,
        };
        let intr_tab_id = Uuid::new_v4();

        workspace.tabs.push(WorkTab {
            id: rep_tab_id,
            title: "1".into(),
            group: None,
            draft: rep_draft.clone(),
            last_flow: None,
            archive_response: None,
            history: None,
        });
        workspace.active = Some(rep_tab_id);
        workspace.intruder.tabs.push(IntruderTab {
            id: intr_tab_id,
            title: "1".into(),
            group: None,
            spec: intr_spec.clone(),
            last_run: None,
        });
        workspace.intruder.active = Some(intr_tab_id);
        workspace.proxy_listeners = vec![ProxyListenerConfig {
            id: Uuid::new_v4(),
            enabled: false,
            bind_address: "127.0.0.1:8080".into(),
            ..ProxyListenerConfig::default()
        }];
        workspace.sessions = SessionSettings {
            cookie_jar: CookieJarSettings {
                monitor_proxy: true,
                monitor_repeater: true,
                monitor_intruder: true,
                entries: vec![CookieJarEntry {
                    id: Uuid::new_v4(),
                    domain: target_addr.ip().to_string(),
                    host_only: true,
                    path: "/".into(),
                    name: "static_ck".into(),
                    value: "v_static".into(),
                    secure: false,
                    http_only: false,
                    expires_unix_ms: None,
                }],
                ..CookieJarSettings::default()
            },
            macros: vec![SessionMacro {
                id: macro_id,
                name: "AuthBootstrap".into(),
                steps: vec![MacroStep {
                    id: Uuid::new_v4(),
                    method: "GET".into(),
                    url: format!("http://{auth_addr}/bootstrap"),
                    headers: Vec::new(),
                    body: Vec::new(),
                    use_cookie_jar: true,
                    expected_status: Some(200),
                    extractions: vec![MacroExtraction {
                        name: "csrf_token".into(),
                        source: MacroExtractSource::Delimiter {
                            start: "csrf_token=".into(),
                            end: "".into(),
                        },
                        target: MacroApplyTarget::Header {
                            header_name: "X-Csrf".into(),
                        },
                    }],
                }],
                failure_policy: MacroFailurePolicy::Abort,
                timeout_ms: 3000,
            }],
            rules: Vec::new(),
        };
        // Fix delimiter end to use Regex so end isn't empty (validation enforces non-empty end).
        workspace.sessions.macros[0].steps[0].extractions[0].source = MacroExtractSource::Regex {
            pattern: "csrf_token=([A-Za-z0-9_]+)".into(),
        };
        workspace.sessions.rules.push(SessionRule {
            id: rule_id,
            name: "InjectAllTools".into(),
            enabled: true,
            tools: SessionToolScope {
                proxy: true,
                repeater: true,
                intruder: true,
                ..SessionToolScope::default()
            },
            condition: SessionRuleCondition {
                url_scope: SessionUrlScope::CustomPrefixes(vec![format!("http://{target_addr}/")]),
                param_condition: None,
                header_contains: None,
            },
            actions: vec![
                SessionRuleAction::RunMacro {
                    macro_id,
                    update_cookies_from_jar: true,
                    apply_extractions: true,
                },
                SessionRuleAction::SetHeader {
                    name: "X-Session-Env".into(),
                    value: "prod".into(),
                    only_if_present: false,
                },
                SessionRuleAction::RewriteUrlParam {
                    name: "tenant".into(),
                    value: "t1".into(),
                    remove: false,
                    only_if_present: false,
                },
            ],
        });
        workspace.revision += 1;
        let expected_rev = workspace.revision;
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await
            .unwrap();
        recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::WorkspaceChanged { workspace: w, .. } if w.revision == expected_rev)
        })
        .await;

        // Stale CAS write must be rejected and not disable the active rule.
        let mut stale_ws = workspace.clone();
        stale_ws.sessions.rules[0].enabled = false;
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: stale_ws,
            })
            .await
            .unwrap();
        recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::WorkspaceRejected { .. })
        })
        .await;

        // Start Proxy listener.
        client
            .commands
            .send(Command::StartProxy {
                project,
                listen: "127.0.0.1:0".parse().unwrap(),
            })
            .await
            .unwrap();
        let Event::ProxyState {
            address: Some(proxy_addr),
            ..
        } = recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::ProxyState { address: Some(_), .. })
        })
        .await
        else {
            unreachable!()
        };

        // (1) Proxy forward injects Cookie Jar + Macro Set-Cookie + Header + URL param.
        {
            let mut p_stream = TcpStream::connect(proxy_addr).await.unwrap();
            let req = format!(
                "GET http://{target_addr}/proxy-check HTTP/1.1\r\nHost: {target_addr}\r\nConnection: close\r\n\r\n"
            );
            p_stream.write_all(req.as_bytes()).await.unwrap();
            let mut resp_buf = String::new();
            p_stream.read_to_string(&mut resp_buf).await.unwrap();
            assert!(resp_buf.starts_with("HTTP/1.1 200"), "proxy resp: {resp_buf}");
        }

        // (2) Repeater send injects all session artifacts.
        let rep_run_1 = Uuid::new_v4();
        client
            .commands
            .send(Command::Send {
                project,
                tab_id: rep_tab_id,
                run_id: rep_run_1,
                draft: rep_draft.clone(),
                settings: RepeaterSettings::default(),
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let Event::SendFinished { record: rep_rec, .. } =
            recv_matching(&mut client, &mut workspace, |e| {
                matches!(e, Event::SendFinished { tab_id, .. } if *tab_id == rep_tab_id)
            })
            .await
        else {
            unreachable!()
        };
        assert_eq!(rep_rec.summary.state, FlowState::Complete);

        // (3) Intruder dispatch injects all session artifacts.
        let intr_run_1 = Uuid::new_v4();
        client
            .commands
            .send(Command::StartIntruder {
                project,
                run_id: intr_run_1,
                tab_id: intr_tab_id,
                spec: Box::new(intr_spec.clone()),
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        loop {
            client
                .commands
                .send(Command::QueryIntruderResults {
                    project,
                    run_id: intr_run_1,
                    query_revision: 1,
                    after: None,
                    limit: 10,
                })
                .await
                .unwrap();
            let Event::IntruderResults { result, .. } = recv_matching(&mut client, &mut workspace, |e| {
                matches!(e, Event::IntruderResults { run_id, .. } if *run_id == intr_run_1)
            })
            .await
            else {
                unreachable!()
            };
            let page = result.unwrap();
            if page.run.state == IntruderRunState::Completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(target_hits.load(Ordering::SeqCst), 3);
        {
            let captured = seen_requests.lock().unwrap();
            for raw in captured.iter() {
                let lower = raw.to_ascii_lowercase();
                assert!(lower.contains("tenant=t1"), "missing rewritten param: {raw}");
                assert!(
                    lower.contains("x-csrf: tok_pipeline_7"),
                    "missing macro header: {raw}"
                );
                assert!(
                    lower.contains("x-session-env: prod"),
                    "missing rule header: {raw}"
                );
                assert!(
                    lower.contains("static_ck=v_static")
                        && lower.contains("dyn_sid=macro_cookie_1"),
                    "missing static + macro cookies: {raw}"
                );
            }
        }

        // Now flip Macro server to fail (500): Proxy, Repeater, and Intruder MUST fail-closed (0 target hits).
        fail_macro.store(true, Ordering::SeqCst);
        let hits_before_fail = target_hits.load(Ordering::SeqCst);

        // Proxy fails closed (502, no origin hit).
        {
            let mut p_stream = TcpStream::connect(proxy_addr).await.unwrap();
            let req = format!(
                "GET http://{target_addr}/proxy-blocked HTTP/1.1\r\nHost: {target_addr}\r\nConnection: close\r\n\r\n"
            );
            p_stream.write_all(req.as_bytes()).await.unwrap();
            let mut resp_buf = String::new();
            p_stream.read_to_string(&mut resp_buf).await.unwrap();
            assert!(!resp_buf.starts_with("HTTP/1.1 200"), "must reject: {resp_buf}");
        }

        // Repeater fails closed.
        client
            .commands
            .send(Command::Send {
                project,
                tab_id: rep_tab_id,
                run_id: Uuid::new_v4(),
                draft: rep_draft.clone(),
                settings: RepeaterSettings::default(),
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let Event::SendFinished {
            record: failed_rep, ..
        } = recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::SendFinished { tab_id, .. } if *tab_id == rep_tab_id)
        })
        .await
        else {
            unreachable!()
        };
        assert!(matches!(failed_rep.summary.state, FlowState::Failed(_)));

        // Intruder fails closed.
        let intr_run_fail = Uuid::new_v4();
        client
            .commands
            .send(Command::StartIntruder {
                project,
                run_id: intr_run_fail,
                tab_id: intr_tab_id,
                spec: Box::new(intr_spec.clone()),
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let failed_page = loop {
            client
                .commands
                .send(Command::QueryIntruderResults {
                    project,
                    run_id: intr_run_fail,
                    query_revision: 2,
                    after: None,
                    limit: 10,
                })
                .await
                .unwrap();
            let Event::IntruderResults { result, .. } = recv_matching(&mut client, &mut workspace, |e| {
                matches!(e, Event::IntruderResults { run_id, .. } if *run_id == intr_run_fail)
            })
            .await
            else {
                unreachable!()
            };
            let page = result.unwrap();
            if page.run.state.is_terminal() {
                break page;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        };
        let IntruderResultOutcome::Flow(summary) = &failed_page.rows[0].outcome else {
            panic!("expected Flow outcome");
        };
        assert!(matches!(summary.state, FlowState::Failed(_)));
        assert_eq!(
            target_hits.load(Ordering::SeqCst),
            hits_before_fail,
            "failed session rules must NEVER fallback to direct origin connection"
        );

        // Drain any pending WorkspaceChanged notifications from Intruder terminal state before CAS update.
        while let Ok(ev) = client.events.try_recv() {
            if let Event::WorkspaceChanged { workspace: w, .. } = ev {
                workspace = w;
            }
        }

        // Disable the broken rule via valid CAS revision -> requests skip disabled rule and succeed!
        workspace.sessions.rules[0].enabled = false;
        workspace.revision += 1;
        let expected_rev_2 = workspace.revision;
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await
            .unwrap();
        recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::WorkspaceChanged { workspace: w, .. } if w.revision == expected_rev_2)
        })
        .await;

        client
            .commands
            .send(Command::Send {
                project,
                tab_id: rep_tab_id,
                run_id: Uuid::new_v4(),
                draft: rep_draft,
                settings: RepeaterSettings::default(),
                cancel: CancellationToken::new(),
            })
            .await
            .unwrap();
        let Event::SendFinished {
            record: bypassed_rep,
            ..
        } = recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::SendFinished { tab_id, .. } if *tab_id == rep_tab_id)
        })
        .await
        else {
            unreachable!()
        };
        assert_eq!(bypassed_rep.summary.state, FlowState::Complete);
        assert_eq!(target_hits.load(Ordering::SeqCst), hits_before_fail + 1);

        client
            .commands
            .send(Command::StopProxy { project })
            .await
            .unwrap();
        recv_matching(&mut client, &mut workspace, |e| {
            matches!(e, Event::ProxyState { address: None, .. })
        })
        .await;
        drop(client);
        runtime.shutdown().unwrap();
        auth_srv.abort();
        target_srv.abort();
    })
    .await
    .unwrap();
}
