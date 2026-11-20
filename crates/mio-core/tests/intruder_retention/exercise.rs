use super::*;

pub(crate) async fn exercise(ending: SessionEnd) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}", listener.local_addr().unwrap());
    let origin = tokio::spawn(async move {
        for expected in *b"AB" {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = vec![];
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 8192);
                head.push(stream.read_u8().await.unwrap());
            }
            assert_eq!(stream.read_u8().await.unwrap(), expected);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\0\xffR",
                )
                .await
                .unwrap();
        }
        listener
    });
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("before.mio");
    let destination = directory.path().join("after.mio");
    let (runtime, mut client) = Runtime::start().unwrap();
    let (project, mut workspace) = opened(&mut client).await;
    save(&mut client, project, source.clone()).await;
    let raw =
        b"POST /retention HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
    let position = Position {
        start: raw.len() - 1,
        end: raw.len(),
    };
    let input = IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 1,
        draft: Draft {
            revision: 1,
            target,
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![position],
        payload_sets: vec![vec![b"A".to_vec(), b"B".to_vec()]],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
    };
    let tab_id = Uuid::new_v4();
    workspace.intruder.tabs.push(IntruderTab {
        id: tab_id,
        title: "retention".into(),
        group: None,
        spec: input.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab_id);
    workspace.revision += 1;
    save_workspace(&mut client, project, &workspace).await;
    let run_id = Uuid::new_v4();
    client
        .commands
        .send(Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec: Box::new(input.clone()),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    loop {
        match event(&mut client).await {
            Event::IntruderStarted {
                project: owner,
                run_id: id,
                result,
                ..
            } => {
                assert_eq!((owner, id), (project, run_id));
                let run = result.unwrap();
                assert_eq!(run.total, 2);
                assert_eq!(run.retention, IntruderRetention::Project);
                break;
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    let before = terminal_results(&mut client, project, run_id).await;
    assert_eq!(before.run.state, IntruderRunState::Completed);
    assert!(before.persisted, "StartIntruder remains Project-backed");
    assert_eq!(before.run.retention, IntruderRetention::Project);
    assert_eq!(before.rows.len(), 2);
    let mut records = vec![];
    for row in &before.rows {
        let IntruderResultOutcome::Flow(summary) = &row.outcome else {
            panic!("loopback request must have a flow")
        };
        assert_eq!(summary.state, FlowState::Complete);
        records.push(read_flow(&mut client, project, summary.id).await);
    }
    assert_eq!(records[0].request.body, b"A");
    assert_eq!(records[1].request.body, b"B");
    assert!(
        records
            .iter()
            .all(|record| record.response.as_ref().unwrap().body == [0, 255, b'R'])
    );
    let listener = origin.await.unwrap().into_std().unwrap();
    workspace.revision += 1;
    workspace.intruder.tabs[0].last_run = Some(run_id);
    save_workspace(&mut client, project, &workspace).await;
    workspace.revision += 1;
    let session = retention(
        &mut client,
        project,
        &before.run,
        IntruderRetention::Session,
        workspace,
    )
    .await;
    assert_eq!(session.run.revision, before.run.revision + 1);
    assert_eq!(session.workspace.intruder.tabs[0].last_run, None);
    let after = results(&mut client, project, run_id).await;
    assert!(!after.persisted);
    assert_eq!(after.run.retention, IntruderRetention::Session);
    assert_eq!(after.rows, before.rows);
    let session_runs = runs(&mut client, project, 350).await;
    assert_eq!(
        session_runs.rows.as_slice(),
        std::slice::from_ref(&session.run)
    );
    assert_eq!(session_runs.next_after, None);
    assert_eq!(read_spec(&mut client, project, run_id).await, input);
    for record in &records {
        assert_eq!(
            read_flow(&mut client, project, record.summary.id).await,
            *record
        );
    }
    let blocked = directory.path().join("blocked.mio");
    client
        .commands
        .send(Command::NewProject {
            path: Some(blocked.clone()),
        })
        .await
        .unwrap();
    loop {
        match event(&mut client).await {
            Event::Error {
                project: owner,
                operation: "new_project",
                message,
            } => {
                assert_eq!(owner, Some(project));
                assert!(message.contains("Session"));
                break;
            }
            Event::ProjectOpened { .. } => panic!("New must not drop retained Session data"),
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
    assert!(!blocked.exists());
    client
        .commands
        .send(Command::OpenProject {
            path: source.clone(),
        })
        .await
        .unwrap();
    loop {
        match event(&mut client).await {
            Event::Error {
                project: owner,
                operation: "open_project",
                message,
            } => {
                assert_eq!(owner, Some(project));
                assert!(message.contains("Session"));
                break;
            }
            Event::ProjectOpened { .. } => panic!("Open must not drop retained Session data"),
            Event::Error {
                operation, message, ..
            } => panic!("{operation}: {message}"),
            _ => {}
        }
    }
    save(&mut client, project, source).await;
    save(&mut client, project, destination.clone()).await;
    assert_eq!(results(&mut client, project, run_id).await, after);
    if matches!(ending, SessionEnd::Promote) {
        let mut workspace = session.workspace.clone();
        workspace.revision += 1;
        workspace.intruder.tabs[0].last_run = Some(run_id);
        let promoted = retention(
            &mut client,
            project,
            &session.run,
            IntruderRetention::Project,
            workspace,
        )
        .await;
        assert_eq!(promoted.run.revision, session.run.revision + 1);
        let page = results(&mut client, project, run_id).await;
        assert!(page.persisted);
        assert_eq!(page.run.retention, IntruderRetention::Project);
    }
    let discarded_workspace = if matches!(ending, SessionEnd::Discard) {
        let operation_id = Uuid::new_v4();
        let mut proposal = session.workspace.clone();
        proposal.revision += 1;
        proposal.intruder.tabs[0].last_run = Some(run_id);
        proposal.intruder.tabs[0].title = "discard receipt".into();
        let receipt = discard(
            &mut client,
            project,
            &session.run,
            operation_id,
            proposal.clone(),
        )
        .await
        .unwrap();
        proposal.intruder.tabs[0].last_run = None;
        assert_eq!(receipt.workspace, proposal);
        assert_eq!(receipt.revision, session.run.revision + 1);
        assert_eq!((receipt.deleted_flows, receipt.retained_flows), (2, 0));
        missing(&mut client, project, run_id, &records).await;

        let mut expected = receipt.workspace;
        expected.revision += 1;
        expected.intruder.tabs[0].title = "latest edit after discard".into();
        save_workspace(&mut client, project, &expected).await;
        let mut stale = expected.clone();
        stale.revision += 1;
        stale.intruder.tabs[0].title = "duplicate operation must not overwrite".into();
        assert!(
            discard(&mut client, project, &session.run, operation_id, stale)
                .await
                .is_err()
        );
        save(&mut client, project, destination.clone()).await;

        client
            .commands
            .send(Command::NewProject {
                path: Some(blocked.clone()),
            })
            .await
            .unwrap();
        let (new_project, mut new_workspace) = opened(&mut client).await;
        assert_ne!(new_project.id, project.id);
        assert_eq!(new_project.generation, project.generation + 1);
        assert!(blocked.exists());
        new_workspace.revision += 1;
        new_workspace.repeater_settings.unpack_compressed_responses = false;
        save_workspace(&mut client, new_project, &new_workspace).await;
        let mut stale = new_workspace.clone();
        stale.revision += 1;
        stale.repeater_settings.unpack_compressed_responses = true;
        assert!(
            discard(&mut client, project, &session.run, operation_id, stale)
                .await
                .unwrap_err()
                .contains("项目已切换")
        );
        client
            .commands
            .send(Command::OpenProject { path: blocked })
            .await
            .unwrap();
        let (new_reopened, restored) = opened(&mut client).await;
        assert_eq!(new_reopened.id, new_project.id);
        assert_eq!(new_reopened.generation, new_project.generation + 1);
        assert_eq!(
            restored, new_workspace,
            "old-project operations must not write into the new project"
        );
        missing(&mut client, new_reopened, run_id, &records).await;

        client
            .commands
            .send(Command::OpenProject {
                path: destination.clone(),
            })
            .await
            .unwrap();
        let (current, restored) = opened(&mut client).await;
        assert_eq!(current.id, project.id);
        assert_eq!(current.generation, new_reopened.generation + 1);
        assert_eq!(
            restored, expected,
            "duplicate discard must preserve the latest saved workspace"
        );
        let mut stale = expected.clone();
        stale.revision += 1;
        stale.intruder.tabs[0].title = "old generation must not overwrite".into();
        assert!(
            discard(
                &mut client,
                project,
                &session.run,
                operation_id,
                stale.clone()
            )
            .await
            .unwrap_err()
            .contains("项目已切换")
        );
        assert!(
            discard(&mut client, current, &session.run, operation_id, stale)
                .await
                .is_err()
        );
        missing(&mut client, current, run_id, &records).await;
        Some(expected)
    } else {
        None
    };
    stopped(runtime, &mut client).await;
    let (runtime, mut client) = Runtime::start().unwrap();
    opened(&mut client).await;
    client
        .commands
        .send(Command::OpenProject { path: destination })
        .await
        .unwrap();
    let (reopened, restored) = opened(&mut client).await;
    assert_eq!(reopened.id, project.id);
    if let Some(expected) = discarded_workspace {
        assert_eq!(
            restored, expected,
            "stale operations must not rewrite the reopened project"
        );
        missing(&mut client, reopened, run_id, &records).await;
    }
    let runs = runs(&mut client, reopened, 400).await;
    if matches!(ending, SessionEnd::Promote) {
        assert_eq!(runs.rows.len(), 1);
        assert_eq!(runs.rows[0].id, run_id);
        assert_eq!(runs.rows[0].retention, IntruderRetention::Project);
        assert_eq!(
            results(&mut client, reopened, run_id).await.rows,
            before.rows
        );
        assert_eq!(read_spec(&mut client, reopened, run_id).await, input);
        for record in records {
            assert_eq!(
                read_flow(&mut client, reopened, record.summary.id).await,
                record
            );
        }
    } else {
        assert!(
            runs.rows.is_empty(),
            "shutdown must not implicitly promote Session data"
        );
        query_results(&mut client, reopened, run_id, 500).await;
        loop {
            if let Event::IntruderResults {
                project: owner,
                run_id: id,
                query_revision: 500,
                result,
            } = event(&mut client).await
            {
                assert_eq!((owner, id), (reopened, run_id));
                assert!(result.is_err());
                break;
            }
        }
        client
            .commands
            .send(Command::ReadFlow {
                project: reopened,
                flow_id: records[0].summary.id,
                selection_revision: 501,
            })
            .await
            .unwrap();
        loop {
            match event(&mut client).await {
                Event::FlowDetailError {
                    project: owner,
                    selection_revision: 501,
                    flow_id,
                    ..
                } => {
                    assert_eq!((owner, flow_id), (reopened, records[0].summary.id));
                    break;
                }
                Event::FlowDetail {
                    selection_revision: 501,
                    ..
                } => panic!("Session-only flow leaked to disk"),
                Event::Error {
                    operation, message, ..
                } => panic!("{operation}: {message}"),
                _ => {}
            }
        }
    }
    stopped(runtime, &mut client).await;
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "retention, save and reopening must never replay requests"
    );
}
