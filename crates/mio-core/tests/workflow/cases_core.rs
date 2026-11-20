use super::*;

#[tokio::test]
async fn latest_chinese_draft_send_save_reopen_and_stale_project_rejection() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK").await.unwrap();
            request
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = project(&mut client).await;
        client.commands.send(Command::NewRepeater { project }).await.unwrap();
        let mut workspace = workspace(&mut client).await;
        let tab = &mut workspace.tabs[0];
        tab.draft.target = format!("http://{address}");
        tab.draft.version = HttpVersion::Http11;
        tab.draft.raw = format!("POST /capture HTTP/1.1\r\nHost: {address}\r\nX-Value: one\r\nX-Value: two\r\n\r\n最新中文🙂").into_bytes();
        tab.draft.revision += 1;
        let id = tab.id;
        let draft = tab.draft.clone();
        let sent_revision = draft.revision;
        workspace.revision += 1;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        client.commands.send(Command::Send { run_id: mio_core::Uuid::new_v4(), cancel: tokio_util::sync::CancellationToken::new(), project, tab_id: id, draft, settings: workspace.repeater_settings.clone() }).await.unwrap();
        let finished = event(&mut client, |e| matches!(e, Event::SendFinished { .. })).await;
        let flow_id = match finished {
            Event::SendFinished { tab_id, record, draft_revision, .. } => {
                assert_eq!(tab_id, id);
                assert_eq!(draft_revision, sent_revision);
                assert_eq!(record.summary.state, FlowState::Complete);
                assert_eq!(record.response.as_ref().unwrap().body, b"OK");
                record.summary.id
            }
            _ => unreachable!(),
        };
        let received = origin.await.unwrap();
        assert!(received.ends_with("最新中文🙂".as_bytes()));
        assert_eq!(String::from_utf8_lossy(&received).to_lowercase().matches("x-value:").count(), 2);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow.mio");
        client.commands.send(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path: Some(path.clone()), workspace: None }).await.unwrap();
        event(&mut client, |e| matches!(e, Event::ProjectSaved { .. })).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        let next = match event(&mut client, |e| matches!(e, Event::ProjectOpened { .. })).await {
            Event::ProjectOpened { project, .. } => project, _ => unreachable!(),
        };
        assert_ne!(project, next);
        client.commands.send(Command::NewRepeater { project }).await.unwrap();
        event(&mut client, |e| matches!(e, Event::Error { operation: "new_repeater", .. })).await;
        client.commands.send(Command::OpenProject { path }).await.unwrap();
        let reopened = event(&mut client, |e| matches!(e, Event::ProjectOpened { .. })).await;
        let reopened = match reopened {
            Event::ProjectOpened { project: key, workspace: actual, .. } => {
                assert_eq!(key.id, project.id);
                assert!(key.generation > project.generation);
                assert_eq!(actual, workspace);
                key
            }, _ => unreachable!(),
        };
        client.commands.send(Command::ReadFlow { project: reopened, selection_revision: 9, flow_id }).await.unwrap();
        match event(&mut client, |e| matches!(e, Event::FlowDetail { .. })).await {
            Event::FlowDetail { record, .. } => assert_eq!(record.request.body, "最新中文🙂".as_bytes()),
            _ => unreachable!(),
        }
        runtime.shutdown().unwrap();
    }).await.expect("workflow has a finite upper bound");
}

#[tokio::test]
async fn in_flight_cancel_waits_for_origin_eof_and_never_retries() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted, request_received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            receive_request(&mut stream).await;
            accepted.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(
                stream.read(&mut byte).await.unwrap(),
                0,
                "cancel must close actual origin socket"
            );
            let listener = listener.into_std().unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock,
                "POST must not be retried"
            );
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = project(&mut client).await;
        client
            .commands
            .send(Command::NewRepeater { project })
            .await
            .unwrap();
        let mut workspace = workspace(&mut client).await;
        let tab = &mut workspace.tabs[0];
        tab.draft.target = format!("http://{address}");
        tab.draft.raw = format!("POST /hold HTTP/1.1\r\nHost: {address}\r\n\r\nbody").into_bytes();
        tab.draft.revision += 1;
        let tab_id = tab.id;
        let draft = tab.draft.clone();
        workspace.revision += 1;
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await
            .unwrap();
        client
            .commands
            .send(Command::Send {
                settings: workspace.repeater_settings.clone(),
                run_id: mio_core::Uuid::new_v4(),
                cancel: tokio_util::sync::CancellationToken::new(),
                project,
                tab_id,
                draft: draft.clone(),
            })
            .await
            .unwrap();
        request_received.await.unwrap();
        let Event::SendStarted {
            run_id,
            draft_revision,
            ..
        } = event(&mut client, |event| {
            matches!(event, Event::SendStarted { .. })
        })
        .await
        else {
            unreachable!()
        };
        client
            .commands
            .send(Command::Send {
                settings: workspace.repeater_settings.clone(),
                run_id: mio_core::Uuid::new_v4(),
                cancel: tokio_util::sync::CancellationToken::new(),
                project,
                tab_id,
                draft,
            })
            .await
            .unwrap();
        event(&mut client, |e| matches!(e, Event::SendRejected { .. })).await;
        client
            .commands
            .send(Command::CancelSend {
                project,
                tab_id,
                run_id,
                draft_revision,
            })
            .await
            .unwrap();
        match event(&mut client, |e| matches!(e, Event::SendFinished { .. })).await {
            Event::SendFinished { record, .. } => {
                assert_eq!(record.summary.state, FlowState::Cancelled)
            }
            _ => unreachable!(),
        }
        origin.await.unwrap();
        runtime.shutdown().unwrap();
    })
    .await
    .expect("cancel has a finite upper bound");
}

#[tokio::test]
async fn quiesce_finishes_network_before_saving_the_last_flow_link() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted, request_received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            receive_request(&mut stream).await;
            accepted.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = project(&mut client).await;
        client
            .commands
            .send(Command::NewRepeater { project })
            .await
            .unwrap();
        let mut workspace = workspace(&mut client).await;
        workspace.tabs[0].draft.target = format!("http://{address}");
        workspace.tabs[0].draft.raw =
            format!("POST /hold HTTP/1.1\r\nHost: {address}\r\n\r\nfinal").into_bytes();
        workspace.tabs[0].draft.revision = 1;
        workspace.revision = 1;
        let tab_id = workspace.tabs[0].id;
        let draft = workspace.tabs[0].draft.clone();
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await
            .unwrap();
        client
            .commands
            .send(Command::Send {
                settings: workspace.repeater_settings.clone(),
                run_id: mio_core::Uuid::new_v4(),
                cancel: tokio_util::sync::CancellationToken::new(),
                project,
                tab_id,
                draft: draft.clone(),
            })
            .await
            .unwrap();
        request_received.await.unwrap();
        client
            .commands
            .send(Command::Quiesce { project })
            .await
            .unwrap();
        let terminal = event(&mut client, |e| {
            matches!(e, Event::SendFinished { .. } | Event::Quiesced { .. })
        })
        .await;
        let Event::SendFinished {
            record, persisted, ..
        } = terminal
        else {
            panic!("Quiesced cannot precede a terminal flow event")
        };
        assert!(persisted);
        assert_eq!(record.summary.state, FlowState::Cancelled);
        event(&mut client, |e| matches!(e, Event::Quiesced { .. })).await;
        origin.await.unwrap();
        client
            .commands
            .send(Command::Send {
                settings: workspace.repeater_settings.clone(),
                run_id: mio_core::Uuid::new_v4(),
                cancel: tokio_util::sync::CancellationToken::new(),
                project,
                tab_id,
                draft,
            })
            .await
            .unwrap();
        event(&mut client, |e| matches!(e, Event::SendRejected { .. })).await;
        workspace.tabs[0].last_flow = Some(record.summary.id);
        workspace.revision += 1;
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await
            .unwrap();
        event(&mut client, |e| matches!(e, Event::WorkspaceChanged { .. })).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("last-flow.mio");
        client
            .commands
            .send(Command::SaveProject {
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project,
                path: Some(path.clone()),
                workspace: None,
            })
            .await
            .unwrap();
        event(&mut client, |e| matches!(e, Event::ProjectSaved { .. })).await;
        client
            .commands
            .send(Command::OpenProject { path })
            .await
            .unwrap();
        let Event::ProjectOpened {
            workspace: actual, ..
        } = event(&mut client, |e| matches!(e, Event::ProjectOpened { .. })).await
        else {
            unreachable!()
        };
        assert_eq!(actual, workspace);
        runtime.shutdown().unwrap();
    })
    .await
    .expect("transition must be bounded");
}
