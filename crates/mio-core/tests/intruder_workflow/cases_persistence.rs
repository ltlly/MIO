use super::*;

#[tokio::test]
async fn save_save_as_and_reopen_preserve_result_pages_and_reject_old_projects() {
    assert_last_run_round_trip(false).await;
}

#[tokio::test]
async fn cancelled_last_run_and_edited_spec_survive_save_save_as_and_both_reopens() {
    assert_last_run_round_trip(true).await;
}

async fn assert_last_run_round_trip(cancelled: bool) {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = receive_request(&mut stream).await;
                assert!(request.starts_with(b"POST /intruder HTTP/1.1\r\n"));
                requests.push(body(&request).to_vec());
                if cancelled {
                    seen.send(()).unwrap();
                    assert_eq!(
                        stream.read(&mut [0; 1]).await.unwrap(),
                        0,
                        "Stop must close the actual origin socket before saving"
                    );
                    return (listener, requests);
                }
                echo_response(&mut stream, &request).await;
            }
            (listener, requests)
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let original = opened(&mut client).await;
        let payloads = vec![b"first".to_vec(), vec![0, 0xff], b"third".to_vec()];
        let spec = spec(format!("http://{address}"), payloads.clone());
        let run_id = Uuid::new_v4();
        let mut workspace = Workspace::default();
        let tab_id = bind(&mut client, original, &mut workspace, &spec).await;
        start(
            &mut client,
            original,
            run_id,
            tab_id,
            &spec,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        if cancelled {
            received.await.unwrap();
            client
                .commands
                .send(Command::StopIntruder {
                    project: original,
                    run_id,
                })
                .await
                .unwrap();
        }
        let expected = terminal(&mut client, original, run_id).await;
        assert_eq!(
            expected.run.state,
            if cancelled {
                IntruderRunState::Stopped
            } else {
                IntruderRunState::Completed
            }
        );
        let result_count = if cancelled { 1 } else { 3 };
        assert_eq!(
            (
                expected.run.total,
                expected.run.dispatched,
                expected.run.completed
            ),
            (3, result_count as u64, result_count as u64)
        );
        assert_eq!(expected.rows.len(), result_count);
        assert_eq!(
            (expected.run.id, expected.run.tab_id, expected.run.spec_revision),
            (run_id, tab_id, spec.revision)
        );
        let (listener, requests) = origin.await.unwrap();
        assert_eq!(requests.as_slice(), &payloads[..result_count]);

        let edited = &mut workspace.intruder.tabs[0];
        edited.last_run = Some(run_id);
        edited.spec.revision += 1;
        edited.spec.draft.revision += 1;
        *edited.spec.draft.raw.last_mut().unwrap() = b'Y';
        edited.spec.payload_sets = vec![vec![b"edited-not-sent".to_vec()]];
        edited.spec.update_host_header = false;
        workspace.revision += 1;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("intruder.mio");
        let copy = directory.path().join("intruder-copy.mio");
        for (index, destination) in [Some(source.clone()), None, Some(copy.clone())]
            .into_iter()
            .enumerate()
        {
            if index == 1 {
                let edited = &mut workspace.intruder.tabs[0].spec;
                edited.revision += 1;
                edited.draft.revision += 1;
                *edited.draft.raw.last_mut().unwrap() = b'Z';
                workspace.revision += 1;
            }
            let expected_path = destination.as_ref().unwrap_or(&source).clone();
            client
                .commands
                .send(Command::SaveProject {
                    websocket_notes: Vec::new(), ext_http_notes: Vec::new(),
                    ext_http_highlights: Vec::new(),
                    project: original,
                    path: destination,
                    workspace: (index < 2).then(|| workspace.clone()),
                })
                .await
                .unwrap();
            if index < 2 {
                assert!(matches!(
                    event(&mut client, |event| matches!(
                        event,
                        Event::WorkspaceChanged { project, workspace: saved }
                            if *project == original && saved.revision == workspace.revision
                    )).await,
                    Event::WorkspaceChanged { project, workspace: saved }
                        if project == original && saved == workspace
                ));
            }
            match event(&mut client, |e| matches!(e, Event::ProjectSaved { .. })).await {
                Event::ProjectSaved { project, path, .. } => {
                    assert_eq!(project, original);
                    assert_eq!(path, expected_path);
                }
                _ => unreachable!(),
            }
        }
        client
            .commands
            .send(Command::NewProject { path: None })
            .await
            .unwrap();
        let mut previous = opened(&mut client).await;
        assert_ne!(previous.id, original.id);
        assert!(
            results(&mut client, original, run_id, 80, None, 1)
                .await
                .is_err()
        );

        for path in [source, copy] {
            client
                .commands
                .send(Command::OpenProject { path: path.clone() })
                .await
                .unwrap();
            let (project, restored_workspace) = match event(&mut client, |event| {
                matches!(event, Event::ProjectOpened { .. })
            })
            .await
            {
                Event::ProjectOpened {
                    project,
                    path: actual_path,
                    workspace,
                } => {
                    assert_eq!(actual_path, Some(path));
                    (project, workspace)
                }
                _ => unreachable!(),
            };
            assert_eq!(restored_workspace, workspace);
            assert_eq!(project.id, original.id);
            assert!(project.generation > previous.generation);
            let restored_tab = restored_workspace
                .intruder
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .unwrap();
            let restored_run = restored_tab.last_run.expect("saved tab must retain last_run");
            assert_eq!(restored_run, run_id);
            assert_eq!(restored_tab.id, expected.run.tab_id);
            assert_ne!(restored_tab.spec, spec);
            client
                .commands
                .send(Command::ReadIntruderSpec {
                    project,
                    run_id: restored_run,
                    selection_revision: project.generation,
                })
                .await
                .unwrap();
            match event(&mut client, |event| {
                matches!(event,
                    Event::IntruderSpec { project: key, run_id: id, selection_revision, .. }
                    if *key == project && *id == restored_run && *selection_revision == project.generation
                )
            })
            .await
            {
                Event::IntruderSpec { result, .. } => assert_eq!(*result.unwrap(), spec),
                _ => unreachable!(),
            }
            client
                .commands
                .send(Command::QueryIntruderRuns {
                    project,
                    query_revision: project.generation,
                    after: None,
                    limit: 1,
                })
                .await
                .unwrap();
            match event(&mut client, |event| {
                matches!(event,
                    Event::IntruderRuns { project: key, query_revision, .. }
                    if *key == project && *query_revision == project.generation
                )
            })
            .await
            {
                Event::IntruderRuns { result, .. } => {
                    let page = result.unwrap();
                    assert_eq!(page.rows, std::slice::from_ref(&expected.run));
                    assert_eq!(page.next_after, None);
                }
                _ => unreachable!(),
            }
            assert!(
                results(&mut client, previous, run_id, 81, None, 1)
                    .await
                    .is_err()
            );
            let mut after = None;
            for (index, expected_row) in expected.rows.iter().enumerate() {
                let page = results(&mut client, project, restored_run, 100 + index as u64, after, 1)
                    .await
                    .unwrap();
                assert!(page.persisted);
                assert_eq!(page.run, expected.run);
                assert_eq!(page.rows, std::slice::from_ref(expected_row));
                assert_eq!(page.rows[0].ordinal, index as u64);
                assert_eq!(page.next_after, (index + 1 < result_count).then_some(index as u64));
                after = Some(index as u64);
                let IntruderResultOutcome::Flow(flow) = &page.rows[0].outcome else {
                    panic!("saved result must still resolve its real flow")
                };
                assert_eq!(
                    flow.state,
                    if cancelled { FlowState::Cancelled } else { FlowState::Complete }
                );
                client
                    .commands
                    .send(Command::ReadFlow {
                        project,
                        selection_revision: 200 + index as u64,
                        flow_id: flow.id,
                    })
                    .await
                    .unwrap();
                match event(&mut client, |e| {
                    matches!(e, Event::FlowDetail {
                        project: key, selection_revision, ..
                    } if *key == project && *selection_revision == 200 + index as u64)
                })
                .await
                {
                    Event::FlowDetail { record, .. } => {
                        assert_eq!(record.summary, *flow);
                        assert_eq!(record.request.body, payloads[index]);
                        if cancelled {
                            assert!(record.response.is_none());
                        } else {
                            assert_eq!(record.response.as_ref().unwrap().body, payloads[index]);
                        }
                        let repeated: Vec<_> = record
                            .request
                            .headers
                            .iter()
                            .filter(|header| header.name.eq_ignore_ascii_case("X-Repeat"))
                            .map(|header| header.value.as_slice())
                            .collect();
                        assert_eq!(repeated, [b"one".as_slice(), b"two".as_slice()]);
                    }
                    _ => unreachable!(),
                }
            }
            let end = results(&mut client, project, restored_run, 300, after, 1)
                .await
                .unwrap();
            assert!(end.persisted && end.rows.is_empty());
            assert_eq!(end.next_after, None);
            previous = project;
        }
        shutdown(runtime).await;
        no_extra_connection(listener);
    })
    .await
    .expect("last_run, edited draft, frozen results, both reopens and cleanup are bounded");
}

#[tokio::test]
async fn pause_resume_and_idle_cancel_use_the_live_scheduler_without_replaying_posts() {
    tokio::time::timeout(DEADLINE, async {
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let mut workspace = Workspace::default();
        for resume in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let input = spec(
                format!("http://{}", listener.local_addr().unwrap()),
                vec![b"first".to_vec(), b"second".to_vec()],
            );
            let (seen, received) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = receive_request(&mut stream).await;
                assert_eq!(body(&request), b"first");
                seen.send(()).unwrap();
                released.await.unwrap();
                echo_response(&mut stream, &request).await;
                listener
            });
            let run_id = Uuid::new_v4();
            let tab_id = bind(&mut client, project, &mut workspace, &input).await;
            let cancel = CancellationToken::new();
            start(&mut client, project, run_id, tab_id, &input, cancel.clone())
                .await
                .unwrap();
            received.await.unwrap();
            let paused = set_paused(&mut client, project, run_id, true)
                .await
                .unwrap();
            assert_eq!(
                (paused.state, paused.dispatched, paused.completed),
                (IntruderRunState::Paused, 1, 0)
            );
            release.send(()).unwrap();
            let listener = origin.await.unwrap();
            let mut revision = 1;
            loop {
                let page = results(&mut client, project, run_id, revision, None, 2)
                    .await
                    .unwrap();
                assert!(page.persisted);
                assert_eq!(page.run.state, IntruderRunState::Paused);
                assert_eq!(page.run.dispatched, 1);
                if page.run.completed == 1 {
                    assert_eq!(page.rows.len(), 1);
                    assert!(matches!(&page.rows[0].outcome,
                        IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Complete));
                    break;
                }
                event(&mut client, |event| {
                    matches!(event, Event::IntruderChanged { project: key } if *key == project)
                })
                .await;
                revision += 1;
            }
            let listener = listener.into_std().unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
            let listener = TcpListener::from_std(listener).unwrap();
            let listener = if resume {
                let origin = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let request = receive_request(&mut stream).await;
                    assert_eq!(body(&request), b"second");
                    echo_response(&mut stream, &request).await;
                    listener
                });
                let resumed = set_paused(&mut client, project, run_id, false)
                    .await
                    .unwrap();
                assert_eq!(
                    (resumed.state, resumed.dispatched),
                    (IntruderRunState::Running, 1)
                );
                let page = terminal(&mut client, project, run_id).await;
                assert_eq!(page.run.state, IntruderRunState::Completed);
                assert_eq!((page.run.dispatched, page.run.completed), (2, 2));
                assert_eq!(page.rows.len(), 2);
                assert!(
                    set_paused(&mut client, project, run_id, false)
                        .await
                        .is_err()
                );
                origin.await.unwrap()
            } else {
                while let Ok(event) = client.events.try_recv() {
                    assert!(!matches!(event, Event::Error { .. }));
                }
                cancel.cancel();
                // Await a state change without sending a command that would wake the scheduler.
                event(&mut client, |event| {
                    matches!(event, Event::IntruderChanged { project: key } if *key == project)
                })
                .await;
                let page = terminal(&mut client, project, run_id).await;
                assert_eq!(page.run.state, IntruderRunState::Stopped);
                assert_eq!((page.run.dispatched, page.run.completed), (1, 1));
                assert!(
                    set_paused(&mut client, project, run_id, false)
                        .await
                        .is_err()
                );
                listener
            };
            no_extra_connection(listener);
        }
        shutdown(runtime).await;
    })
    .await
    .expect("live pause, explicit resume, idle cancellation and runtime join are bounded");
}
