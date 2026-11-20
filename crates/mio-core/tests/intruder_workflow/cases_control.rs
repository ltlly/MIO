use super::*;

#[tokio::test]
async fn cancel_joins_and_persists_while_pause_ack_is_blocked_on_full_event_queue() {
    assert_cancel_with_blocked_reply(true).await;
}

#[tokio::test]
async fn cancel_joins_and_persists_while_history_reply_is_blocked_on_full_event_queue() {
    assert_cancel_with_blocked_reply(false).await;
}

#[tokio::test]
async fn closing_the_receiver_reclaims_a_run_with_a_blocked_control_reply() {
    tokio::time::timeout(DEADLINE, async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("closed-receiver.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let input = spec(
            format!("http://{}", listener.local_addr().unwrap()),
            vec![b"once".to_vec(), b"never".to_vec()],
        );
        let (seen, received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(body(&receive_request(&mut stream).await), b"once");
            seen.send(()).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
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
        event(&mut client, |event| {
            matches!(event, Event::ProjectSaved { .. })
        })
        .await;
        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let tab = bind(&mut client, project, &mut Workspace::default(), &input).await;
        start(&mut client, project, id, tab, &input, cancel.clone())
            .await
            .unwrap();
        received.await.unwrap();
        results(&mut client, project, id, 1, None, 1).await.unwrap();
        // Consume the run invalidation released by the query before reserving every event slot.
        event(
            &mut client,
            |event| matches!(event, Event::IntruderChanged { project: owner } if *owner == project),
        )
        .await;
        while client.events.try_recv().is_ok() {}
        let capacity = client.events.max_capacity();
        for query_revision in 0..capacity as u64 {
            client
                .commands
                .send(Command::QueryIntruderRuns {
                    project,
                    query_revision,
                    after: None,
                    limit: 1,
                })
                .await
                .unwrap();
        }
        while client.events.len() != capacity {
            tokio::task::yield_now().await;
        }
        client
            .commands
            .send(Command::SetIntruderPaused {
                project,
                run_id: id,
                operation_id: Uuid::new_v4(),
                paused: true,
            })
            .await
            .unwrap();
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let phase = || {
            connection
                .query_row(
                    "SELECT state,paused,completed FROM intruder_runs WHERE id=?1",
                    [id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .unwrap()
        };
        while phase() != (0, 1, 0) {
            tokio::task::yield_now().await;
        }
        assert_eq!(client.events.len(), capacity);
        client.events.close();
        let listener = origin.await.unwrap();
        let saved_before_explicit_shutdown = tokio::time::timeout(Duration::from_secs(3), async {
            while phase() != (2, 0, 1) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        shutdown(runtime).await;
        no_extra_connection(listener);
        assert!(cancel.is_cancelled());
        assert!(
            saved_before_explicit_shutdown,
            "receiver closure must reclaim and persist before the explicit runtime join"
        );
    })
    .await
    .expect("closing a full receiver still cancels, persists and joins its real socket");
}

async fn assert_cancel_with_blocked_reply(control: bool) {
    tokio::time::timeout(DEADLINE, async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("blocked-pause.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        client.commands.send(Command::SaveProject {
            websocket_notes: Vec::new(), ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project, path: Some(path.clone()), workspace: None,
        }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProjectSaved { .. })).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let input = spec(format!("http://{}", listener.local_addr().unwrap()),
            vec![b"once".to_vec(), b"never".to_vec()]);
        let (seen, received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            seen.send(body(&request).to_vec()).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let run_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let mut workspace = Workspace::default();
        let tab_id = bind(&mut client, project, &mut workspace, &input).await;
        start(&mut client, project, run_id, tab_id, &input, cancel.clone()).await.unwrap();
        assert_eq!(received.await.unwrap(), b"once");
        let other_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_input = spec(format!("http://{}", other_listener.local_addr().unwrap()),
            vec![b"other-first".to_vec(), b"other-second".to_vec()]);
        let (other_seen, other_received) = oneshot::channel();
        let (release_other, other_released) = oneshot::channel();
        let other_origin = tokio::spawn(async move {
            let (mut stream, _) = other_listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            assert_eq!(body(&request), b"other-first");
            other_seen.send(()).unwrap();
            other_released.await.unwrap();
            echo_response(&mut stream, &request).await;
            other_listener
        });
        let other_id = Uuid::new_v4();
        let other_cancel = CancellationToken::new();
        let other_tab = bind(&mut client, project, &mut workspace, &other_input).await;
        start(&mut client, project, other_id, other_tab, &other_input, other_cancel.clone()).await.unwrap();
        other_received.await.unwrap();
        let page = results(&mut client, project, run_id, 1, None, 2).await.unwrap();
        assert_eq!((page.run.dispatched, page.run.completed), (1, 0));
        client.commands.send(Command::QueryHistory {
            project, query_revision: 1, offset: 0, limit: 1, filter: String::new(),
        }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::HistoryPage { .. })).await;
        while let Ok(event) = client.events.try_recv() {
            assert!(!matches!(event, Event::Error { .. }));
        }
        let capacity = client.events.max_capacity();
        for query_revision in 0..capacity as u64 {
            client.commands.send(Command::QueryIntruderRuns {
                project, query_revision, after: None, limit: 1,
            }).await.unwrap();
        }
        while client.events.len() != capacity {
            tokio::task::yield_now().await;
        }
        let operation_id = Uuid::new_v4();
        let command = if control {
            Command::SetIntruderPaused { project, run_id, operation_id, paused: true }
        } else {
            Command::QueryHistory { project, query_revision: u64::MAX,
                offset: 0, limit: 1, filter: String::new() }
        };
        client.commands.send(command).await.unwrap();
        while client.commands.capacity() != client.commands.max_capacity() {
            tokio::task::yield_now().await;
        }
        // Read-only inspection proves the control ran while every UI slot remains occupied.
        let connection = rusqlite::Connection::open_with_flags(&path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let phase = |id: Uuid| {
            connection.query_row(
                "SELECT state,paused,dispatched,completed FROM intruder_runs WHERE id=?1",
                [id.to_string()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
            ).unwrap()
        };
        while phase(run_id) != (0, i64::from(control), 1, 0) {
            tokio::task::yield_now().await;
        }
        assert_eq!(client.events.len(), capacity);
        cancel.cancel();
        release_other.send(()).unwrap();
        let listener = origin.await.unwrap();
        let other_listener = other_origin.await.unwrap();
        let stopped_without_drain = tokio::time::timeout(Duration::from_secs(3), async {
            while phase(run_id) != (2, 0, 1, 1) || phase(other_id) != (0, 0, 1, 1) {
                tokio::task::yield_now().await;
            }
        }).await.is_ok();
        assert_eq!(client.events.len(), capacity);
        assert!(!other_cancel.is_cancelled());
        let other_listener = other_listener.into_std().unwrap();
        assert_eq!(other_listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        let other_listener = TcpListener::from_std(other_listener).unwrap();
        let other_origin = tokio::spawn(async move {
            let (mut stream, _) = other_listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            assert_eq!(body(&request), b"other-second");
            echo_response(&mut stream, &request).await;
            other_listener
        });
        // Drain and join even on the red path before asserting the cancellation observation.
        let mut after_one_slot = None;
        for query_revision in 0..capacity as u64 {
            match client.events.recv().await.unwrap() {
                Event::IntruderRuns { project: key, query_revision: revision, result } => {
                    assert_eq!((key, revision), (project, query_revision));
                    assert!(result.is_ok());
                }
                other => panic!("unexpected event occupying the controlled queue: {other:?}"),
            }
            if query_revision == 0 {
                while client.events.len() != capacity { tokio::task::yield_now().await; }
                after_one_slot = Some((phase(run_id), phase(other_id)));
            }
        }
        if control {
            let ack = client.events.recv().await.unwrap();
            let Event::IntruderRunControl { project: key, run_id: id, operation_id: operation, result } = ack else {
                panic!("the retained control must precede notifications produced during its wait")
            };
            assert_eq!((key, id, operation), (project, run_id, operation_id));
            let acknowledged = result.unwrap();
            assert_eq!(acknowledged.state, IntruderRunState::Paused);
            assert_eq!((acknowledged.revision, acknowledged.dispatched, acknowledged.completed),
                (page.run.revision + 1, 1, 0));
        } else {
            let reply = client.events.recv().await.unwrap();
            let Event::HistoryPage { project: key, query_revision, rows, .. } = reply else {
                panic!("the retained query must precede notifications produced during its wait")
            };
            assert_eq!((key, query_revision), (project, u64::MAX));
            assert_eq!(rows.len(), 1);
            let mio_core::HistoryRow::Live(summary) = &rows[0] else {
                panic!("the Intruder request must remain a live history row");
            };
            assert_eq!(summary.state, FlowState::Pending);
        }
        assert!(matches!(client.events.recv().await.unwrap(),
            Event::HistoryChanged { project: key } if key == project));
        let page = terminal(&mut client, project, run_id).await;
        assert!(page.persisted);
        assert_eq!((page.run.state, page.run.dispatched, page.run.completed),
            (IntruderRunState::Stopped, 1, 1));
        assert!(matches!(page.rows.as_slice(), [row]
            if matches!(&row.outcome, IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled)));
        let other_page = terminal(&mut client, project, other_id).await;
        assert_eq!((other_page.run.state, other_page.run.dispatched, other_page.run.completed),
            (IntruderRunState::Completed, 2, 2));
        assert!(other_page.rows.iter().all(|row| matches!(&row.outcome,
            IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Complete)));
        let other_listener = other_origin.await.unwrap();
        shutdown(runtime).await;
        no_extra_connection(listener);
        no_extra_connection(other_listener);
        assert!(stopped_without_drain,
            "run cancellation must join and persist within 3s while the reply is blocked, and retain other in-flight responses without dispatching again");
        assert_eq!(after_one_slot, Some(((2, 0, 1, 1), (0, 0, 1, 1))));
    }).await.expect("blocked reply, independent runs, durable readback and cleanup are bounded");
}
