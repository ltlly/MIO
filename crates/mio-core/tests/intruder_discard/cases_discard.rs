use super::*;

#[tokio::test]
async fn terminal_discard_preserves_other_attacks_and_copies_through_save_as_and_reopen() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let payloads = vec![vec![0, 255], b"B".to_vec()];
        let expected = payloads.clone();
        let origin = tokio::spawn(async move {
            for ordinal in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let body = request(&mut stream).await;
                assert_eq!(body, expected[ordinal % 2]);
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        let raw = b"POST /discard HTTP/1.1\r\nHost: fixture.invalid\r\nX-Repeat: one\r\nX-Repeat: two\r\nContent-Length: 1\r\n\r\nX".to_vec();
        let input = IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            positions: vec![Position { start: raw.len() - 1, end: raw.len() }],
            draft: Draft { revision: 1, target, raw, version: HttpVersion::Http11 },
            attack_type: AttackType::Sniper, payload_sets: vec![payloads.clone()],
            max_concurrent_requests: 1, request_timeout_ms: 2000, update_host_header: true, save_to_project: true,
        };
        let tab_id = Uuid::new_v4();
        workspace.intruder.tabs.push(IntruderTab {
            id: tab_id, title: "discard fixture".into(), group: None,
            spec: input.clone(), last_run: None,
        });
        workspace.intruder.active = Some(tab_id);
        workspace.revision += 1;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        assert!(matches!(event(&mut client, |event| matches!(event, Event::WorkspaceChanged { .. } | Event::WorkspaceRejected { .. })).await,
            Event::WorkspaceChanged { workspace: saved, .. } if saved == workspace));
        let mut runs = Vec::new();
        for run_id in [Uuid::from_u128(1), Uuid::from_u128(2)] {
            client.commands.send(Command::StartIntruder {
                project, run_id, tab_id, spec: Box::new(input.clone()), cancel: CancellationToken::new(),
            }).await.unwrap();
            match event(&mut client, |event| matches!(event, Event::IntruderStarted { run_id: id, .. } if *id == run_id)).await {
                Event::IntruderStarted { result, .. } => assert_eq!(result.unwrap().id, run_id),
                _ => unreachable!(),
            }
            let page = terminal(&mut client, project, run_id).await;
            assert_eq!(page.run.state, IntruderRunState::Completed);
            assert_eq!(page.rows.len(), 2);
            runs.push(page);
        }
        let listener = origin.await.unwrap();
        let first = &runs[0];
        let second = &runs[1];
        let IntruderResultOutcome::Flow(removed_flow) = &first.rows[0].outcome else { panic!("real response expected") };
        client.commands.send(Command::OpenRepeater { project, flow_id: removed_flow.id, original: false }).await.unwrap();
        let copy = match event(&mut client, |event| matches!(event, Event::RepeaterOpened { .. })).await {
            Event::RepeaterOpened { tab, .. } => tab,
            _ => unreachable!(),
        };
        assert!(copy.last_flow.is_none());
        assert!(copy.draft.raw.ends_with(&payloads[0]));
        workspace.tabs.push(copy.clone());
        workspace.intruder.tabs[0].last_run = Some(second.run.id);
        let mut closed = workspace.intruder.tabs[0].clone();
        closed.id = Uuid::new_v4();
        closed.last_run = Some(first.run.id);
        workspace.intruder.closed.push(closed);
        workspace.revision += 1;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mio");
        let copy_path = directory.path().join("copy.mio");
        save(&mut client, project, Some(source.clone()), Some(workspace.clone())).await;
        assert!(discard(&mut client, project, first.run.id, Uuid::new_v4(), first.run.revision, workspace.clone()).await.unwrap_err().contains("工作区"));
        assert_eq!(page(&mut client, project, first.run.id, 50).await.unwrap(), *first);

        let mut proposal = workspace.clone();
        proposal.revision += 1;
        proposal.intruder.tabs[0].spec.revision += 1;
        proposal.intruder.tabs[0].spec.draft.revision += 1;
        proposal.intruder.tabs[0].spec.draft.raw.push(b'Z');
        proposal.repeater_settings.unpack_compressed_responses = false;
        let operation = Uuid::new_v4();
        let mut expected = proposal.clone();
        expected.intruder.closed[0].last_run = None;
        let receipt = discard(&mut client, project, first.run.id, operation, first.run.revision, proposal.clone()).await.unwrap();
        assert_eq!(receipt.workspace, expected);
        assert_eq!((receipt.revision, receipt.deleted_flows, receipt.retained_flows), (first.run.revision + 1, 2, 0));
        assert!(discard(&mut client, project, first.run.id, operation, first.run.revision, proposal).await.unwrap_err().contains("已丢弃"));
        assert!(page(&mut client, project, first.run.id, 51).await.is_err());
        assert_eq!(page(&mut client, project, second.run.id, 52).await.unwrap(), *second);
        save(&mut client, project, None, None).await;
        save(&mut client, project, Some(copy_path.clone()), None).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        let (mut previous, _) = opened(&mut client).await;
        for path in [source, copy_path] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let (current, restored) = opened(&mut client).await;
            assert_eq!(current.id, project.id);
            assert!(current.generation > previous.generation);
            assert_eq!(restored, expected);
            assert_eq!(restored.tabs[0], copy);
            assert!(page(&mut client, previous, second.run.id, 60).await.is_err());
            assert!(page(&mut client, current, first.run.id, 61).await.is_err());
            assert_eq!(page(&mut client, current, second.run.id, 62).await.unwrap(), *second);
            client.commands.send(Command::QueryIntruderRuns {
                project: current, query_revision: 63, after: Some(first.run.id), limit: 1,
            }).await.unwrap();
            match event(&mut client, |event| matches!(event, Event::IntruderRuns { query_revision: 63, .. })).await {
                Event::IntruderRuns { result, .. } => {
                    let page = result.unwrap();
                    assert_eq!(page.rows.as_slice(), std::slice::from_ref(&second.run));
                    assert_eq!(page.next_after, None);
                }
                _ => unreachable!(),
            }
            client.commands.send(Command::ReadIntruderSpec {
                project: current, run_id: second.run.id, selection_revision: 64,
            }).await.unwrap();
            match event(&mut client, |event| matches!(event, Event::IntruderSpec { selection_revision: 64, .. })).await {
                Event::IntruderSpec { result, .. } => assert_eq!(*result.unwrap(), input),
                _ => unreachable!(),
            }
            client.commands.send(Command::ReadFlow {
                project: current, flow_id: removed_flow.id, selection_revision: 65,
            }).await.unwrap();
            assert!(matches!(event(&mut client, |event| matches!(event,
                Event::FlowDetail { selection_revision: 65, .. } | Event::FlowDetailError { selection_revision: 65, .. }
            )).await, Event::FlowDetailError { flow_id, .. } if flow_id == removed_flow.id));
            client.commands.send(Command::StartIntruder {
                project: current, run_id: first.run.id, tab_id,
                spec: Box::new(restored.intruder.tabs[0].spec.clone()), cancel: CancellationToken::new(),
            }).await.unwrap();
            assert!(matches!(event(&mut client, |event| matches!(event, Event::IntruderStarted { .. })).await,
                Event::IntruderStarted { result: Err(_), .. }));
            previous = current;
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock,
            "discard, failed stale Start and reopening must not send requests");
    }).await.expect("small loopback attacks, saves, reopens and runtime cleanup are bounded");
}
