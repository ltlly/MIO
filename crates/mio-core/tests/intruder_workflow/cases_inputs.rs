use super::*;

#[tokio::test]
async fn duplicate_run_id_never_replays_an_active_or_completed_post() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            seen.send(()).unwrap();
            released.await.unwrap();
            echo_response(&mut stream, &request).await;
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let spec = spec(format!("http://{address}"), vec![b"once".to_vec()]);
        let run_id = Uuid::new_v4();
        let tab_id = bind(&mut client, project, &mut Workspace::default(), &spec).await;
        start(
            &mut client,
            project,
            run_id,
            tab_id,
            &spec,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        received.await.unwrap();
        assert!(
            start(
                &mut client,
                project,
                run_id,
                tab_id,
                &spec,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .contains("run ID 已存在")
        );
        release.send(()).unwrap();
        let before = terminal(&mut client, project, run_id).await;
        assert_eq!(before.run.state, IntruderRunState::Completed);
        assert_eq!(
            (
                before.run.total,
                before.run.dispatched,
                before.run.completed
            ),
            (1, 1, 1)
        );
        assert!(
            start(
                &mut client,
                project,
                run_id,
                tab_id,
                &spec,
                CancellationToken::new(),
            )
            .await
            .unwrap_err()
            .contains("run ID 已存在")
        );
        let after = terminal(&mut client, project, run_id).await;
        assert_eq!(after, before);
        let listener = origin.await.unwrap();
        shutdown(runtime).await;
        no_extra_connection(listener);
    })
    .await
    .expect("duplicate run identity must not start another network operation");
}

#[tokio::test]
async fn invalid_generated_method_is_a_persisted_rejection_without_network_io() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let mut spec = spec(
            format!("http://{}", listener.local_addr().unwrap()),
            vec![b"P\0ST".to_vec()],
        );
        spec.positions = vec![Position { start: 0, end: 4 }];
        let run_id = Uuid::new_v4();
        let tab_id = bind(&mut client, project, &mut Workspace::default(), &spec).await;
        start(
            &mut client,
            project,
            run_id,
            tab_id,
            &spec,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let page = terminal(&mut client, project, run_id).await;
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!(
            (page.run.total, page.run.dispatched, page.run.completed),
            (1, 1, 1)
        );
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].ordinal, 0);
        assert!(
            matches!(&page.rows[0].outcome, IntruderResultOutcome::Rejected(message)
            if !message.is_empty())
        );
        shutdown(runtime).await;
        no_extra_connection(listener);
    })
    .await
    .expect("invalid generated HTTP is rejected without sending a packet");
}

#[tokio::test]
async fn start_binds_the_acknowledged_open_draft_and_running_inputs_stay_frozen() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let input = spec(
            format!("http://{}", listener.local_addr().unwrap()),
            vec![b"before".to_vec()],
        );
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        assert!(
            start(
                &mut client,
                project,
                Uuid::new_v4(),
                Uuid::new_v4(),
                &input,
                CancellationToken::new()
            )
            .await
            .is_err()
        );
        let mut workspace = Workspace::default();
        let tab = bind(&mut client, project, &mut workspace, &input).await;
        workspace.intruder.tabs[0].spec.payload_sets[0][0] = b"acknowledged".to_vec();
        workspace.intruder.tabs[0].spec.revision += 1;
        workspace.revision += 1;
        save_workspace(&mut client, project, &workspace).await;
        let admitted = workspace.intruder.tabs[0].spec.clone();
        assert!(
            start(
                &mut client,
                project,
                Uuid::new_v4(),
                tab,
                &input,
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .contains("已保存 revision")
        );
        let mut same_revision = admitted.clone();
        same_revision.update_host_header = false;
        assert!(
            start(
                &mut client,
                project,
                Uuid::new_v4(),
                tab,
                &same_revision,
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .contains("已保存 revision")
        );

        workspace
            .intruder
            .closed
            .push(workspace.intruder.tabs.remove(0));
        workspace.intruder.active = None;
        workspace.revision += 1;
        save_workspace(&mut client, project, &workspace).await;
        assert!(
            start(
                &mut client,
                project,
                Uuid::new_v4(),
                tab,
                &admitted,
                CancellationToken::new()
            )
            .await
            .unwrap_err()
            .contains("已关闭")
        );
        workspace
            .intruder
            .tabs
            .push(workspace.intruder.closed.pop().unwrap());
        workspace.intruder.active = Some(tab);
        workspace.revision += 1;
        save_workspace(&mut client, project, &workspace).await;
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(
            start(
                &mut client,
                project,
                Uuid::new_v4(),
                tab,
                &admitted,
                cancelled
            )
            .await
            .unwrap_err()
            .contains("已取消")
        );
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock
        );

        // A fresh origin makes the one admitted request and its causal response observable.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        workspace.intruder.tabs[0].spec.draft.target =
            format!("http://{}", listener.local_addr().unwrap());
        workspace.intruder.tabs[0].spec.draft.revision += 1;
        workspace.intruder.tabs[0].spec.revision += 1;
        workspace.revision += 1;
        save_workspace(&mut client, project, &workspace).await;
        let admitted = workspace.intruder.tabs[0].spec.clone();
        let (seen, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            seen.send(body(&request).to_vec()).unwrap();
            released.await.unwrap();
            echo_response(&mut stream, &request).await;
            listener
        });
        let run_id = Uuid::new_v4();
        start(
            &mut client,
            project,
            run_id,
            tab,
            &admitted,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(received.await.unwrap(), b"acknowledged");
        workspace.intruder.tabs[0].spec.payload_sets[0][0] = b"edited while running".to_vec();
        workspace.intruder.tabs[0].spec.revision += 1;
        workspace.intruder.tabs[0].last_run = Some(run_id);
        workspace.revision += 1;
        save_workspace(&mut client, project, &workspace).await;
        client
            .commands
            .send(Command::ReadIntruderSpec {
                project,
                run_id,
                selection_revision: 99,
            })
            .await
            .unwrap();
        match event(&mut client, |event| {
            matches!(
                event,
                Event::IntruderSpec {
                    selection_revision: 99,
                    ..
                }
            )
        })
        .await
        {
            Event::IntruderSpec { result, .. } => assert_eq!(*result.unwrap(), admitted),
            _ => unreachable!(),
        }
        release.send(()).unwrap();
        let page = terminal(&mut client, project, run_id).await;
        assert_eq!(
            (page.run.total, page.run.dispatched, page.run.completed),
            (1, 1, 1)
        );
        assert_eq!(page.run.state, IntruderRunState::Completed);
        let listener = origin.await.unwrap();
        shutdown(runtime).await;
        no_extra_connection(listener);
    })
    .await
    .expect("workspace ACK, rejection, frozen execution and socket cleanup are bounded");
}
