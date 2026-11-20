use super::*;

#[tokio::test]
async fn stopped_silent_run_is_discarded_after_origin_eof_and_stays_retired_on_reopen() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let payloads = vec![vec![0, 255], b"never-dispatched".to_vec()];
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("stopped-source.mio");
        let copy_path = directory.path().join("stopped-copy.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened(&mut client).await;
        let raw = b"POST /stop-discard HTTP/1.1\r\nHost: fixture.invalid\r\nX-Repeat: one\r\nX-Repeat: two\r\nContent-Length: 1\r\n\r\nX".to_vec();
        let input = IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            positions: vec![Position { start: raw.len() - 1, end: raw.len() }],
            draft: Draft { revision: 1, target, raw, version: HttpVersion::Http11 },
            attack_type: AttackType::Sniper,
            payload_sets: vec![payloads.clone()],
            max_concurrent_requests: 1,
            request_timeout_ms: 30_000,
            update_host_header: true, save_to_project: true,
        };
        let tab_id = Uuid::new_v4();
        workspace.intruder.tabs.push(IntruderTab {
            id: tab_id,
            title: "stopped discard fixture".into(),
            group: None,
            spec: input.clone(),
            last_run: None,
        });
        workspace.intruder.active = Some(tab_id);
        workspace.revision += 1;
        save(&mut client, project, Some(source.clone()), Some(workspace.clone())).await;
        let run_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        client.commands.send(Command::StartIntruder {
            project, run_id, tab_id, spec: Box::new(input.clone()), cancel: cancel.clone(),
        }).await.unwrap();
        match event(&mut client, |event| matches!(event,
            Event::IntruderStarted { project: owner, run_id: id, .. } if *owner == project && *id == run_id
        )).await {
            Event::IntruderStarted { result, .. } => {
                let run = result.unwrap();
                assert_eq!(run.id, run_id);
                assert_eq!(run.state, IntruderRunState::Running);
                assert_eq!(run.total, 2);
            }
            _ => unreachable!(),
        }
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(request(&mut stream).await, payloads[0]);
        let active = page(&mut client, project, run_id, 10).await.unwrap();
        assert_eq!(active.run.state, IntruderRunState::Running);
        assert_eq!((active.run.dispatched, active.run.completed), (1, 0));
        assert_eq!(active.rows.len(), 1);
        assert!(!cancel.is_cancelled());
        workspace.intruder.tabs[0].last_run = Some(run_id);
        let mut closed = workspace.intruder.tabs[0].clone();
        closed.id = Uuid::new_v4();
        workspace.intruder.closed.push(closed);
        workspace.revision += 1;
        save(&mut client, project, None, Some(workspace.clone())).await;

        // The origin never responds or closes; the request timeout exceeds this entire test.
        cancel.cancel();
        client.commands.send(Command::StopIntruder { project, run_id }).await.unwrap();
        assert_eq!(stream.read(&mut [0_u8; 1]).await.unwrap(), 0,
            "Stop must close the real origin socket before any discard is submitted");
        drop(stream);
        let stopped = terminal(&mut client, project, run_id).await;
        assert_eq!(stopped.run.state, IntruderRunState::Stopped);
        assert_eq!((stopped.run.total, stopped.run.dispatched, stopped.run.completed), (2, 1, 1));
        assert!(stopped.persisted);
        assert_eq!(stopped.next_after, None);
        assert_eq!(stopped.rows.len(), 1);
        assert_eq!(stopped.rows[0].ordinal, 0);
        let IntruderResultOutcome::Flow(flow) = &stopped.rows[0].outcome else {
            panic!("the cancelled real POST must retain its flow until discard");
        };
        assert_eq!(flow.state, FlowState::Cancelled);
        let flow_id = flow.id;
        client.commands.send(Command::ReadFlow {
            project, flow_id, selection_revision: 20,
        }).await.unwrap();
        match event(&mut client, |event| matches!(event,
            Event::FlowDetail { selection_revision: 20, .. }
                | Event::FlowDetailError { selection_revision: 20, .. }
        )).await {
            Event::FlowDetail { project: owner, record, .. } => {
                assert_eq!(owner, project);
                assert_eq!(&record.summary, flow);
                assert_eq!(record.request.body, payloads[0]);
                assert!(record.response.is_none());
            }
            _ => panic!("the stopped POST must still be readable before discard"),
        }
        let mut proposal = workspace;
        proposal.revision += 1;
        proposal.intruder.tabs[0].spec.revision += 1;
        proposal.intruder.tabs[0].spec.draft.revision += 1;
        proposal.intruder.tabs[0].spec.draft.raw.push(b'Z');
        proposal.repeater_settings.unpack_compressed_responses = false;
        let mut expected = proposal.clone();
        expected.intruder.tabs[0].last_run = None;
        expected.intruder.closed[0].last_run = None;
        let operation_id = Uuid::new_v4();
        // Discard admission rejects a terminal run whose sender handles have not joined.
        let receipt = discard(&mut client, project, run_id, operation_id, stopped.run.revision, proposal.clone()).await.unwrap();
        assert_eq!(receipt.revision, stopped.run.revision + 1);
        assert_eq!((receipt.deleted_flows, receipt.retained_flows), (1, 0));
        assert_eq!(receipt.workspace, expected);
        assert!(page(&mut client, project, run_id, 21).await.is_err());
        assert!(discard(&mut client, project, run_id, operation_id, stopped.run.revision, proposal).await.is_err());
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
            assert!(page(&mut client, previous, run_id, 30).await.is_err());
            assert!(page(&mut client, current, run_id, 31).await.is_err());
            for (query_revision, after) in [(32, None), (33, Some(run_id))] {
                client.commands.send(Command::QueryIntruderRuns {
                    project: current, query_revision, after, limit: 1,
                }).await.unwrap();
                match event(&mut client, |event| matches!(event,
                    Event::IntruderRuns { project: owner, query_revision: revision, .. }
                        if *owner == current && *revision == query_revision
                )).await {
                    Event::IntruderRuns { result, .. } => {
                        let runs = result.unwrap();
                        assert!(runs.rows.is_empty());
                        assert_eq!(runs.next_after, None);
                    }
                    _ => unreachable!(),
                }
            }
            client.commands.send(Command::ReadFlow {
                project: current, flow_id, selection_revision: 34,
            }).await.unwrap();
            assert!(matches!(event(&mut client, |event| matches!(event,
                Event::FlowDetail { selection_revision: 34, .. }
                    | Event::FlowDetailError { selection_revision: 34, .. }
            )).await, Event::FlowDetailError { project: owner, flow_id: id, .. }
                if owner == current && id == flow_id));
            client.commands.send(Command::ReadIntruderSpec {
                project: current, run_id, selection_revision: 35,
            }).await.unwrap();
            assert!(matches!(event(&mut client, |event| matches!(event,
                Event::IntruderSpec { selection_revision: 35, .. }
            )).await, Event::IntruderSpec { project: owner, run_id: id, result: Err(_), .. }
                if owner == current && id == run_id));
            client.commands.send(Command::StartIntruder {
                project: current, run_id, tab_id,
                spec: Box::new(restored.intruder.tabs[0].spec.clone()),
                cancel: CancellationToken::new(),
            }).await.unwrap();
            assert!(matches!(event(&mut client, |event| matches!(event,
                Event::IntruderStarted { project: owner, run_id: id, .. }
                    if *owner == current && *id == run_id
            )).await, Event::IntruderStarted { result: Err(_), .. }));
            previous = current;
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock,
            "Stop, discard, repeated Start and reopening must never dispatch the remaining payload or replay the POST");
    }).await.expect("Stop, origin EOF, terminal join, discard, saves, reopens and cleanup are bounded");
}
