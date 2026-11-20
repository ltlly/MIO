use super::*;

#[tokio::test]
async fn runtime_pool_edit_ack_backpressure_keeps_queued_stops_ahead_of_new_dispatch() {
    async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
        loop {
            let event = client
                .events
                .recv()
                .await
                .expect("Runtime remains connected");
            assert!(
                !matches!(event, Event::Error { .. }),
                "unexpected Runtime event: {event:?}"
            );
            if matches(&event) {
                return event;
            }
        }
    }

    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            seen_tx.send(()).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("runtime-pool.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        client.commands.send(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path: Some(path.clone()), workspace: None }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project)).await;
        let mut shared = pool("Runtime shared", Some(1));
        let initial_operation = Uuid::new_v4();
        client.commands.send(Command::SetIntruderResourcePools { project, operation_id: initial_operation, expected_revision: 0, pools: vec![shared.clone()], cancel: CancellationToken::new() }).await.unwrap();
        let Event::IntruderResourcePoolsUpdated { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderResourcePoolsUpdated { operation_id, .. } if *operation_id == initial_operation)).await else { unreachable!() };
        assert_eq!(result.unwrap().revision, 1);
        let mut input = spec(format!("http://{address}"), 1);
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let tabs = [Uuid::new_v4(), Uuid::new_v4()];
        for id in tabs {
            workspace.intruder.tabs.push(crate::IntruderTab { id, title: "shared".into(), group: None, spec: input.clone(), last_run: None });
        }
        workspace.intruder.active = Some(tabs[0]);
        workspace.revision += 1;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::WorkspaceChanged { project: owner, workspace: saved } if *owner == project && saved == &workspace)).await;
        let runs = [Uuid::new_v4(), Uuid::new_v4()];
        let cancels = [CancellationToken::new(), CancellationToken::new()];
        let mut first_seen = Some(seen);
        for index in 0..2 {
            client.commands.send(Command::StartIntruder { project, run_id: runs[index], tab_id: tabs[index], spec: Box::new(input.clone()), cancel: cancels[index].clone() }).await.unwrap();
            let Event::IntruderStarted { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderStarted { run_id, .. } if *run_id == runs[index])).await else { unreachable!() };
            assert_eq!(result.unwrap().resource_pool, Some(shared.id));
            if index == 0 { first_seen.take().unwrap().await.unwrap(); }
        }
        for (index, run_id) in runs.into_iter().enumerate() {
            client.commands.send(Command::QueryIntruderResults { project, run_id, query_revision: 50 + index as u64, after: None, limit: 2 }).await.unwrap();
            let Event::IntruderResults { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderResults { run_id: id, query_revision, .. } if *id == run_id && *query_revision == 50 + index as u64)).await else { unreachable!() };
            assert_eq!(result.unwrap().run.dispatched, u64::from(index == 0), "only the first run may occupy the shared slot");
        }
        client.commands.send(Command::QueryHistory { project, query_revision: 60, offset: 0, limit: 1, filter: String::new() }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 60, .. })).await;
        while let Ok(event) = client.events.try_recv() {
            assert!(!matches!(event, Event::Error { .. }), "unexpected event: {event:?}");
        }
        let capacity = client.events.max_capacity();
        for query_revision in 0..capacity as u64 {
            client.commands.send(Command::QueryIntruderRuns { project, query_revision, after: None, limit: 2 }).await.unwrap();
        }
        while client.events.len() != capacity { tokio::task::yield_now().await; }
        shared.max_concurrent_requests = Some(2);
        let operation = Uuid::new_v4();
        client.commands.send(Command::SetIntruderResourcePools { project, operation_id: operation, expected_revision: 1, pools: vec![shared], cancel: CancellationToken::new() }).await.unwrap();
        let disk = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let catalog_revision = || {
            let bytes: Vec<u8> = disk.query_row("SELECT value FROM metadata WHERE key='intruder_resource_pools'", [], |row| row.get(0)).unwrap();
            serde_json::from_slice::<IntruderResourcePools>(&bytes).unwrap().revision
        };
        while catalog_revision() != 2 { tokio::task::yield_now().await; }
        assert_eq!(client.events.len(), capacity, "committed cap increase still waits behind its ACK");
        // Stop the unsent run first, before the real socket's terminal HistoryChanged producer.
        for run_id in runs.into_iter().rev() {
            client.commands.send(Command::StopIntruder { project, run_id }).await.unwrap();
        }
        let stopped_count = || disk.query_row::<u64, _, _>("SELECT count(*) FROM intruder_runs WHERE state=2", [], |row| row.get(0)).unwrap();
        for query_revision in 0..capacity as u64 {
            let Event::IntruderRuns { project: owner, query_revision: actual, result } = client.events.recv().await.unwrap() else { panic!("controlled query replies precede the retained pool ACK") };
            assert_eq!((owner, actual), (project, query_revision));
            assert_eq!(result.unwrap().rows.len(), 2);
            if query_revision == 0 {
                while client.events.len() != capacity || stopped_count() != 2 {
                    tokio::task::yield_now().await;
                }
            }
        }
        let Event::IntruderResourcePoolsUpdated { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderResourcePoolsUpdated { operation_id, .. } if *operation_id == operation)).await else { unreachable!() };
        assert_eq!(result.unwrap().revision, 2);
        assert!(cancels.iter().all(CancellationToken::is_cancelled));
        for (index, run_id) in runs.into_iter().enumerate() {
            client.commands.send(Command::QueryIntruderResults { project, run_id, query_revision: 500 + index as u64, after: None, limit: 2 }).await.unwrap();
            let Event::IntruderResults { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderResults { run_id: id, query_revision, .. } if *id == run_id && *query_revision == 500 + index as u64)).await else { unreachable!() };
            let page = result.unwrap();
            assert_eq!(page.run.state, IntruderRunState::Stopped);
            assert_eq!(page.rows.len(), usize::from(index == 0));
        }
        let listener = origin.await.unwrap();
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra(listener);
    }).await.expect("pool-edit ACK backpressure keeps both queued Stops ahead of the newly available slot");
}
