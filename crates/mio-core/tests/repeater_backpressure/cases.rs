use super::*;

#[derive(Clone, Copy, Debug)]
enum WaitingReply {
    History,
    Workspace,
    SendStarted,
}

async fn cancel_with_blocked_reply(waiting: WaitingReply) {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        let send_started_waiting = matches!(waiting, WaitingReply::SendStarted);
        let mut stream = if send_started_waiting {
            fixture.fill_events().await;
            fixture.send().await;
            fixture.receive_request().await
        } else {
            fixture.send().await;
            let stream = fixture.receive_request().await;
            assert!(matches!(next(&mut fixture.client).await, Event::SendStarted {
                project, tab_id, run_id, draft_revision,
            } if run_id == fixture.run_id && project == fixture.project && tab_id == fixture.workspace.tabs[0].id && draft_revision == fixture.workspace.tabs[0].draft.revision));
            assert!(matches!(next(&mut fixture.client).await, Event::HistoryChanged { .. }));
            fixture.fill_events().await;
            stream
        };
        let connection = Connection::open_with_flags(&fixture.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let initial = Fixture::stored(&connection);
        assert_eq!(initial.state, FlowState::Pending);
        match waiting {
            WaitingReply::History => fixture.client.commands.send(Command::QueryHistory {
                project: fixture.project, query_revision: WAITING_QUERY, offset: 0, limit: 16, filter: String::new(),
            }).await.unwrap(),
            WaitingReply::Workspace => {
                fixture.workspace.revision += 1;
                fixture.workspace.tabs[0].title = "saved while sending".into();
                fixture.client.commands.send(Command::SaveWorkspace {
                    project: fixture.project, workspace: fixture.workspace.clone(),
                }).await.unwrap();
                while Fixture::stored_workspace(&connection) != fixture.workspace {
                    tokio::task::yield_now().await;
                }
            }
            WaitingReply::SendStarted => {}
        }
        while fixture.client.commands.capacity() != fixture.client.commands.max_capacity() {
            tokio::task::yield_now().await;
        }
        assert_eq!(fixture.client.events.len(), 128);
        fixture.cancel.cancel();
        fixture.client.commands.try_send(Command::CancelSend {
            project: fixture.project, tab_id: fixture.workspace.tabs[0].id, run_id: fixture.run_id, draft_revision: fixture.workspace.tabs[0].draft.revision,
        }).unwrap();
        let mut eof = None;
        let reclaimed_without_drain = tokio::time::timeout(OBSERVATION_BUDGET, async {
            eof = Some(stream.read(&mut [0]).await.unwrap());
            while Fixture::stored(&connection).state != FlowState::Cancelled {
                tokio::task::yield_now().await;
            }
        }).await.is_ok();
        let remained_full = fixture.client.events.len() == 128;
        let before_delivery = Fixture::stored(&connection);
        eprintln!("{waiting:?}: before drain queue_full={remained_full}, origin_eof={eof:?}, terminal_persisted={reclaimed_without_drain}");

        // Release event delivery and join the red path before asserting the failed observation.
        for query_revision in 0..128 {
            let expected = if send_started_waiting { vec![] } else { vec![mio_core::HistoryRow::Live(initial.clone())] };
            assert!(matches!(next(&mut fixture.client).await, Event::HistoryPage {
                project, query_revision: revision, total, rows, ..
            } if project == fixture.project && revision == query_revision && total == expected.len() && rows == expected));
        }
        let reply = next(&mut fixture.client).await;
        match waiting {
            WaitingReply::History => assert!(matches!(reply, Event::HistoryPage {
                project, query_revision: WAITING_QUERY, total: 1, rows, ..
            } if project == fixture.project && rows == [mio_core::HistoryRow::Live(initial.clone())])),
            WaitingReply::Workspace => assert!(matches!(reply, Event::WorkspaceChanged {
                project, workspace,
            } if project == fixture.project && workspace == fixture.workspace)),
            WaitingReply::SendStarted => assert!(matches!(reply, Event::SendStarted {
                project, tab_id, run_id, draft_revision,
            } if run_id == fixture.run_id && project == fixture.project && tab_id == fixture.workspace.tabs[0].id && draft_revision == fixture.workspace.tabs[0].draft.revision)),
        }
        let Event::SendFinished { project, tab_id, draft_revision, record, persisted } = event(&mut fixture.client, |event| matches!(event, Event::SendFinished { .. })).await else { unreachable!() };
        assert_eq!((project, tab_id, draft_revision), (fixture.project, fixture.workspace.tabs[0].id, fixture.workspace.tabs[0].draft.revision));
        assert!(persisted);
        assert_eq!(record.summary.id, initial.id);
        assert_eq!(record.summary.state, FlowState::Cancelled);
        assert_eq!(record.request.body, BODY);
        assert!(record.response.is_none());
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        assert_eq!(Fixture::stored(&connection), record.summary);
        drop(connection);
        assert_eq!(page(&mut fixture.client, fixture.project, 400).await.as_slice(), std::slice::from_ref(&record.summary));
        fixture.save_and_reopen(&record).await;
        fixture.shutdown().await;
        eprintln!("{waiting:?}: runtime_joined=true, saved_and_reopened_twice=true, extra_connections=0");
        assert!(remained_full, "reclamation must not consume UI events");
        assert!(reclaimed_without_drain, "Repeater cancellation must close the real POST socket, join and persist within 3s while {waiting:?} owns the blocked UI reply");
        assert_eq!(eof, Some(0));
        assert_eq!(before_delivery, record.summary);
    }).await.expect("real Runtime, cancellation, save/reopen and cleanup fit the 8s fixture budget");
}

#[tokio::test]
async fn cancel_joins_and_persists_while_history_reply_is_blocked() {
    cancel_with_blocked_reply(WaitingReply::History).await;
}

#[tokio::test]
async fn cancel_joins_and_persists_while_workspace_ack_is_blocked() {
    cancel_with_blocked_reply(WaitingReply::Workspace).await;
}

#[tokio::test]
async fn cancel_before_send_started_delivery_still_reclaims_and_persists() {
    cancel_with_blocked_reply(WaitingReply::SendStarted).await;
}

async fn block_history_reply(fixture: &mut Fixture) {
    fixture.fill_events().await;
    fixture
        .client
        .commands
        .send(Command::QueryHistory {
            project: fixture.project,
            query_revision: WAITING_QUERY,
            offset: 0,
            limit: 16,
            filter: String::new(),
        })
        .await
        .unwrap();
    while fixture.client.commands.capacity() != fixture.client.commands.max_capacity() {
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.client.events.len(), 128);
}

#[tokio::test]
async fn cancelling_a_send_still_in_the_command_queue_never_opens_a_socket() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        block_history_reply(&mut fixture).await;
        fixture.send().await;
        fixture.cancel.cancel();
        fixture.client.commands.try_send(Command::CancelSend {
            project: fixture.project, tab_id: fixture.workspace.tabs[0].id,
            run_id: fixture.run_id, draft_revision: fixture.workspace.tabs[0].draft.revision,
        }).unwrap();
        assert_eq!(fixture.client.events.len(), 128);
        for query_revision in (0..128).chain(std::iter::once(WAITING_QUERY)) {
            assert!(matches!(next(&mut fixture.client).await, Event::HistoryPage {
                project, query_revision: revision, total: 0, rows, ..
            } if project == fixture.project && revision == query_revision && rows.is_empty()));
        }
        let rejected = fixture.client.events.recv().await.unwrap();
        assert!(matches!(rejected, Event::SendRejected {
            project, tab_id, run_id, draft_revision, message,
        } if project == fixture.project && tab_id == fixture.workspace.tabs[0].id
            && run_id == fixture.run_id && draft_revision == fixture.workspace.tabs[0].draft.revision
            && message.contains("已取消")));
        assert!(page(&mut fixture.client, fixture.project, 200).await.is_empty());
        save(&mut fixture.client, fixture.project, None).await;
        fixture.client.commands.send(Command::OpenProject { path: fixture.path.clone() }).await.unwrap();
        let (reopened, workspace) = opened(&mut fixture.client).await;
        assert_eq!(workspace, fixture.workspace);
        assert!(page(&mut fixture.client, reopened, 201).await.is_empty());
        fixture.shutdown().await;
    }).await.expect("queued cancellation, rejection, save/reopen and zero connections are bounded");
}

#[tokio::test]
async fn closing_a_full_receiver_reclaims_and_persists_before_runtime_shutdown() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        fixture.send().await;
        let mut stream = fixture.receive_request().await;
        let initial = page(&mut fixture.client, fixture.project, 200)
            .await
            .pop()
            .unwrap();
        assert_eq!(initial.state, FlowState::Pending);
        block_history_reply(&mut fixture).await;
        let connection =
            Connection::open_with_flags(&fixture.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        fixture.client.events.close();
        let reclaimed = tokio::time::timeout(OBSERVATION_BUDGET, async {
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            while Fixture::stored(&connection).state != FlowState::Cancelled
                || !fixture.client.commands.is_closed()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let summary = Fixture::stored(&connection);
        let remained_full = fixture.client.events.len() == 128;
        drop(connection);
        fixture.shutdown().await;
        assert!(
            reclaimed,
            "receiver closure must reclaim the actual sender before an explicit shutdown call"
        );
        assert!(remained_full);
        assert_eq!(summary.id, initial.id);
        assert_eq!(summary.state, FlowState::Cancelled);
        assert!(summary.revision > initial.revision);
    })
    .await
    .expect("receiver closure, real socket EOF, terminal persistence and join are bounded");
}

#[tokio::test]
async fn stale_project_run_and_revision_controls_cannot_cancel_a_fresh_saved_send() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        fixture.send().await;
        let mut old_stream = fixture.receive_request().await;
        let old_project = fixture.project;
        let old_run = fixture.run_id;
        let old_cancel = fixture.cancel.clone();
        fixture.cancel.cancel();
        fixture.client.commands.send(Command::CancelSend {
            project: fixture.project, tab_id: fixture.workspace.tabs[0].id,
            run_id: old_run, draft_revision: fixture.workspace.tabs[0].draft.revision,
        }).await.unwrap();
        let Event::SendFinished { record: old, persisted: true, .. } = event(&mut fixture.client, |event| matches!(event, Event::SendFinished { .. })).await else { panic!("old run must be durable") };
        assert_eq!(old.summary.id, old_run);
        assert_eq!(old.summary.state, FlowState::Cancelled);
        assert_eq!(old_stream.read(&mut [0]).await.unwrap(), 0);
        fixture.workspace.tabs[0].last_flow = Some(old_run);
        fixture.workspace.revision += 1;
        save_workspace(&mut fixture.client, fixture.project, &fixture.workspace).await;
        save(&mut fixture.client, fixture.project, None).await;
        fixture.client.commands.send(Command::OpenProject { path: fixture.path.clone() }).await.unwrap();
        let (reopened, workspace) = opened(&mut fixture.client).await;
        assert_eq!(reopened.id, old_project.id);
        assert!(reopened.generation > old_project.generation);
        assert_eq!(workspace, fixture.workspace);
        fixture.project = reopened;
        fixture.run_id = Uuid::new_v4();
        fixture.cancel = CancellationToken::new();
        fixture.send().await;
        let mut current_stream = fixture.receive_request().await;
        assert!(matches!(event(&mut fixture.client, |event| matches!(event, Event::SendStarted { .. })).await,
            Event::SendStarted { project, run_id, draft_revision, .. }
                if project == reopened && run_id == fixture.run_id && draft_revision == fixture.workspace.tabs[0].draft.revision));
        old_cancel.cancel();
        fixture.client.commands.send(Command::CancelSend {
            project: old_project, tab_id: fixture.workspace.tabs[0].id,
            run_id: fixture.run_id, draft_revision: fixture.workspace.tabs[0].draft.revision,
        }).await.unwrap();
        loop {
            match fixture.client.events.recv().await.unwrap() {
                Event::Error { project, operation: "cancel_send", .. } => {
                    assert_eq!(project, Some(old_project));
                    break;
                }
                Event::HistoryChanged { project } => assert_eq!(project, reopened),
                other => panic!("unexpected stale-project response: {other:?}"),
            }
        }
        for (run_id, draft_revision) in [
            (old_run, fixture.workspace.tabs[0].draft.revision),
            (fixture.run_id, fixture.workspace.tabs[0].draft.revision + 1),
        ] {
            fixture.client.commands.send(Command::CancelSend {
                project: reopened, tab_id: fixture.workspace.tabs[0].id, run_id, draft_revision,
            }).await.unwrap();
        }
        let rows = page(&mut fixture.client, reopened, 300).await;
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.id == fixture.run_id && row.state == FlowState::Pending));
        assert!(rows.contains(&old.summary));
        assert!(!fixture.cancel.is_cancelled());
        current_stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK").await.unwrap();
        let Event::SendFinished { record: current, persisted: true, .. } = event(&mut fixture.client, |event| matches!(event, Event::SendFinished { .. })).await else { panic!("fresh run must finish normally") };
        assert_eq!(current.summary.id, fixture.run_id);
        assert_eq!(current.summary.state, FlowState::Complete);
        assert_eq!(current.response.as_ref().unwrap().body, b"OK");
        assert_eq!(current_stream.read(&mut [0]).await.unwrap(), 0);
        fixture.send().await;
        loop {
            match fixture.client.events.recv().await.unwrap() {
                Event::SendRejected { project, run_id, draft_revision, message, .. } => {
                    assert_eq!((project, run_id, draft_revision), (reopened, fixture.run_id, fixture.workspace.tabs[0].draft.revision));
                    assert!(message.contains("run ID 已存在"));
                    break;
                }
                Event::HistoryChanged { project } => assert_eq!(project, reopened),
                other => panic!("duplicate run must not start: {other:?}"),
            }
        }
        assert_eq!(detail(&mut fixture.client, reopened, &old.summary).await, *old);
        assert_eq!(detail(&mut fixture.client, reopened, &current.summary).await, *current);
        save(&mut fixture.client, reopened, None).await;
        fixture.client.commands.send(Command::OpenProject { path: fixture.path.clone() }).await.unwrap();
        let (reopened, _) = opened(&mut fixture.client).await;
        let rows = page(&mut fixture.client, reopened, 301).await;
        assert_eq!(rows.len(), 2);
        assert!(rows.contains(&old.summary) && rows.contains(&current.summary));
        fixture.shutdown().await;
    }).await.expect("two real POSTs, stale controls, duplicate-run rejection, save/reopen and all joins are bounded");
}
