use super::*;

#[tokio::test]
async fn confirmed_proxy_rules_hot_update_without_redeciding_held_requests_and_survive_reopen() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("conditional-proxy.mio");
        let copy = directory.path().join("conditional-proxy-copy.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: true, responses: false }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), cancel: Some(cancel), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        let unheld = send(proxy, target, "/static.gif?file=other", BODY).await;
        respond(&listener, "/static.gif?file=other", BODY).await;
        response(unheld).await;

        workspace.revision += 1;
        workspace.scope.include.push(ScopeRule { id: Uuid::new_v4(), enabled: true, prefix: format!("http://{target}/scope"), include_subdomains: false });
        workspace.proxy_interception.request.rules[3].enabled = true;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let out_of_scope = send(proxy, target, "/outside", BODY).await;
        respond(&listener, "/outside", BODY).await;
        response(out_of_scope).await;
        let held = send(proxy, target, "/scope/held", BODY).await;
        let Event::InterceptPending { project: owner, stage, response_head_only, record: pending } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(owner, project);
        assert_eq!(stage, InterceptStage::Request);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, BODY);
        assert_eq!(pending.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
        assert!(listener_pending(&listener));

        workspace.revision += 1;
        workspace.proxy_interception.request.enabled = false;
        client.commands.send(Command::SaveProject {
            project, path: None, workspace: Some(workspace.clone()),
            websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(),
        }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        event(&mut client, |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project)).await;
        assert!(listener_pending(&listener), "an already-held request must retain its admission decision after the saved rule update");
        let passed_after_save = send(proxy, target, "/scope/rules-off", BODY).await;
        respond(&listener, "/scope/rules-off", BODY).await;
        response(passed_after_save).await;
        let mut edited = pending.request.clone();
        edited.body[2] = b'X';
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(edited.clone()) }).await.unwrap();
        respond(&listener, "/scope/held", &edited.body).await;
        response(held).await;
        event(&mut client, |event| matches!(event, Event::InterceptResolved { flow_id, .. } if *flow_id == pending.summary.id)).await;

        let mut invalid = workspace.clone();
        invalid.revision += 1;
        invalid.proxy_interception.request.rules[0].condition = "(".into();
        client.commands.send(Command::SaveWorkspace { project, workspace: invalid.clone() }).await.unwrap();
        let Event::WorkspaceRejected { project: owner, revision, stored, message } = event(&mut client, |event| matches!(event, Event::WorkspaceRejected { .. })).await else { unreachable!() };
        assert_eq!(owner, project);
        assert_eq!(revision, invalid.revision);
        assert_eq!(stored, workspace);
        assert!(message.contains("正则"));

        workspace.revision += 1;
        workspace.proxy_interception.request.enabled = true;
        workspace.proxy_interception.request.rules = vec![ProxyInterceptRule {
            id: Uuid::new_v4(), enabled: true, operator: ProxyInterceptOperator::And,
            match_type: ProxyInterceptType::Request, relationship: ProxyInterceptRelation::ContainsParameters, condition: String::new(),
        }];
        client.commands.send(Command::SaveProject {
            project, path: None, workspace: Some(workspace.clone()),
            websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(),
        }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        event(&mut client, |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project)).await;
        let cancelled = send(proxy, target, "/scope/cancelled?name=%ZZ", BODY).await;
        let Event::InterceptPending { record: cancelled_record, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(cancelled_record.request.body, BODY);
        cancel.cancel();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, cancel: None, .. })).await;
        let mut cancelled = cancelled;
        let mut cancelled_wire = Vec::new();
        cancelled.read_to_end(&mut cancelled_wire).await.unwrap();
        assert!(TcpStream::connect(proxy).await.is_err());
        assert!(cancel.is_cancelled());
        assert!(listener_pending(&listener), "Cancel must never send the held binary POST");

        client.commands.send(Command::QueryHistory { project, query_revision: 500, offset: 0, limit: 10, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 500, .. })).await else { unreachable!() };
        assert_eq!(total, 5);
        assert_eq!(rows.len(), 5);
        let mut observed = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let mio_core::HistoryRow::Live(summary) = row else { panic!("only actual live proxy requests expected") };
            let selection_revision = index as u64 + 1;
            client.commands.send(Command::ReadFlow { project, selection_revision, flow_id: summary.id }).await.unwrap();
            let Event::FlowDetail { record, original, .. } = event(&mut client, |event| matches!(event,
                Event::FlowDetail { project: owner, selection_revision: actual, record, .. }
                    if *owner == project && *actual == selection_revision && record.summary.id == summary.id
            )).await else { unreachable!() };
            assert_eq!(record.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
            if summary.id == pending.summary.id {
                assert_eq!(record.summary.state, FlowState::Complete);
                assert_eq!(record.request, edited);
                assert_eq!(original.request.as_ref(), Some(&pending.request));
            } else {
                assert_eq!(record.request.body, BODY);
                assert_eq!(*original, OriginalMessages::default());
            }
            if summary.id == cancelled_record.summary.id {
                assert_eq!(record.summary.state, FlowState::Cancelled);
                assert!(record.response.is_none());
            } else {
                assert_eq!(record.summary.state, FlowState::Complete);
                let response = record.response.as_ref().unwrap();
                assert_eq!(response.body, [0, 255, b'R']);
                assert_eq!(response.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
            }
            observed.push((*record, *original));
        }
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        opened(&mut client).await;
        for path in [source, copy] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let Event::ProjectOpened { project: reopened, workspace: restored, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
            assert_eq!(restored, workspace);
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            let Event::ProxyState { address, cancel, .. } = event(&mut client, |event| matches!(event, Event::ProxyState { .. })).await else { unreachable!() };
            assert!(address.is_none() && cancel.is_none(), "reopening saved conditions must not restart network operations");
            for (index, (expected_record, expected_original)) in observed.iter().enumerate() {
                let selection_revision = index as u64 + 1;
                client.commands.send(Command::ReadFlow { project: reopened, selection_revision, flow_id: expected_record.summary.id }).await.unwrap();
                let Event::FlowDetail { record, original, .. } = event(&mut client, |event| matches!(event,
                    Event::FlowDetail { project: owner, selection_revision: actual, record, .. }
                        if *owner == reopened && *actual == selection_revision && record.summary.id == expected_record.summary.id
                )).await else { unreachable!() };
                assert_eq!(*record, *expected_record);
                assert_eq!(*original, *expected_original);
            }
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra_connection(listener);
    }).await.expect("conditional request hot updates, both saved copies and runtime join fit the existing eight second budget");
}

#[tokio::test]
async fn protocol_and_listener_rules_use_real_runtime_bound_port_for_both_stages_and_reopen() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("protocol-listener.mio");
        let copy = directory.path().join("protocol-listener-copy.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: true, responses: true }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), cancel: Some(cancel), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        assert_ne!(proxy.port(), 0);
        assert_ne!(proxy.port(), target.port(), "a target TCP port cannot stand in for the actual Proxy listener");
        let port_rule = |id| ProxyInterceptRule {
            id: Uuid::from_u128(id),
            enabled: true,
            operator: ProxyInterceptOperator::And,
            match_type: ProxyInterceptType::ListenerPort,
            relationship: ProxyInterceptRelation::Matches,
            condition: format!("^{}$", proxy.port()),
        };
        let protocol_rule = |id| ProxyInterceptRule {
            id: Uuid::from_u128(id),
            enabled: true,
            operator: ProxyInterceptOperator::And,
            match_type: ProxyInterceptType::Protocol,
            relationship: ProxyInterceptRelation::IsHttp,
            condition: String::new(),
        };
        workspace.revision += 1;
        workspace.proxy_interception.request.enabled = true;
        workspace.proxy_interception.response.enabled = true;
        workspace.proxy_interception.request.rules = vec![port_rule(1), protocol_rule(2)];
        workspace.proxy_interception.response.rules = vec![port_rule(3), protocol_rule(4)];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;

        let held = send(proxy, target, "/protocol-port/forward", BODY).await;
        let Event::InterceptPending { stage, response_head_only, record: pending, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Request);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, BODY);
        assert_eq!(pending.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
        assert!(listener_pending(&listener));
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(pending.request.clone()) }).await.unwrap();
        respond(&listener, "/protocol-port/forward", BODY).await;
        let Event::InterceptPending { stage, response_head_only, record: response_pending, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { stage: InterceptStage::Response, .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Response);
        assert!(!response_head_only);
        assert_eq!(response_pending.summary.id, pending.summary.id);
        let expected_response = response_pending.response.clone().unwrap();
        assert_eq!(expected_response.body, [0, 255, b'R']);
        assert_eq!(expected_response.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: response_pending.summary.revision, decision: Decision::ForwardResponse(expected_response) }).await.unwrap();
        response(held).await;

        workspace.revision += 1;
        workspace.proxy_interception.request.rules[1].relationship = ProxyInterceptRelation::IsHttps;
        workspace.proxy_interception.response.rules[1].relationship = ProxyInterceptRelation::IsHttps;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let plain = send(proxy, target, "/protocol-port/plain-not-https", BODY).await;
        respond(&listener, "/protocol-port/plain-not-https", BODY).await;
        response(plain).await;

        workspace.revision += 1;
        workspace.proxy_interception.request.rules[1].relationship = ProxyInterceptRelation::IsHttp;
        workspace.proxy_interception.response.rules[1].relationship = ProxyInterceptRelation::IsHttp;
        workspace.proxy_interception.request.rules[0].condition = format!("^{}$", target.port());
        workspace.proxy_interception.response.rules[0].condition = format!("^{}$", target.port());
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let wrong_port = send(proxy, target, "/protocol-port/not-target-port", BODY).await;
        respond(&listener, "/protocol-port/not-target-port", BODY).await;
        response(wrong_port).await;

        workspace.revision += 1;
        workspace.proxy_interception.request.rules[0].condition = format!("^{}$", proxy.port());
        workspace.proxy_interception.response.rules[0].condition = format!("^{}$", proxy.port());
        client.commands.send(Command::SaveProject {
            project, path: None, workspace: Some(workspace.clone()),
            websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(),
        }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        event(&mut client, |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project)).await;
        let mut cancelled = send(proxy, target, "/protocol-port/cancelled", BODY).await;
        let Event::InterceptPending { stage, record: cancelled_record, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Request);
        assert_eq!(cancelled_record.request.body, BODY);
        assert!(listener_pending(&listener));
        cancel.cancel();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, cancel: None, .. })).await;
        let mut cancelled_wire = Vec::new();
        cancelled.read_to_end(&mut cancelled_wire).await.unwrap();
        assert!(listener_pending(&listener), "Stop cannot forward a held binary POST");
        assert!(cancel.is_cancelled());
        assert!(TcpStream::connect(proxy).await.is_err());
        client.commands.send(Command::QueryHistory { project, query_revision: 700, offset: 0, limit: 10, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 700, .. })).await else { unreachable!() };
        assert_eq!(total, 4);
        assert_eq!(rows.len(), 4);
        let mut observed = Vec::new();
        for row in rows {
            let mio_core::HistoryRow::Live(summary) = row else { panic!("only loopback Proxy traffic expected") };
            let record = detail(&mut client, project, &summary).await;
            assert_eq!(record.request.body, BODY);
            assert_eq!(record.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
            if summary.id == cancelled_record.summary.id {
                assert_eq!(summary.state, FlowState::Cancelled);
                assert!(record.response.is_none());
            } else {
                assert_eq!(summary.state, FlowState::Complete);
                assert_eq!(record.response.as_ref().unwrap().body, [0, 255, b'R']);
                assert_eq!(record.response.as_ref().unwrap().headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).count(), 2);
            }
            observed.push(record);
        }
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        opened(&mut client).await;
        for path in [source, copy] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let Event::ProjectOpened { project: reopened, workspace: restored, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
            assert_eq!(restored, workspace);
            assert_eq!(restored.proxy_interception.request.rules[0].condition, format!("^{}$", proxy.port()));
            assert_eq!(restored.proxy_interception.response.rules[1].relationship, ProxyInterceptRelation::IsHttp);
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            let Event::ProxyState { address, cancel, .. } = next(&mut client).await else { panic!("reopen must report stopped Proxy") };
            assert!(address.is_none() && cancel.is_none());
            for expected in &observed {
                assert_eq!(detail(&mut client, reopened, &expected.summary).await, *expected);
            }
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra_connection(listener);
    }).await.expect("real listener port, protocol conditions, raw bytes and both saved copies remain within the original eight second budget");
}
