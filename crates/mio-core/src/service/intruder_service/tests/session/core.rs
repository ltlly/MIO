use super::*;

#[tokio::test]
async fn session_reservation_waits_without_spinning_and_resumes_after_real_completion() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_first, first_seen) = tokio::sync::oneshot::channel();
        let (release_first, first_release) = tokio::sync::oneshot::channel();
        let (seen_second, second_seen) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut first).await, 255);
            seen_first.send(()).unwrap();
            first_release.await.unwrap();
            first
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
            drop(first);
            let (mut second, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut second).await, 255);
            seen_second.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(second.read(&mut byte).await.unwrap(), 0);
            listener
        });
        let (mut state, _events) = state();
        state.store.set_intruder_session_quota(24 * 1024 * 1024);
        let mut input = spec(format!("http://{address}"), 2);
        input.save_to_project = false;
        let id = admit(&mut state, input);
        assert_eq!(
            state.intruder.runs[&id].summary.retention,
            IntruderRetention::Session
        );
        state.advance_intruder(id).await.unwrap();
        first_seen.await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert!(state.intruder.runs[&id].waiting_for_capacity);
        assert_eq!(state.intruder.runs[&id].summary.dispatched, 1);
        assert!(
            state.intruder.advance_candidate().is_none(),
            "reserved capacity must wait for a completion, not schedule another Advance"
        );
        state.set_intruder_paused(state.project, id, true).unwrap();
        state.set_intruder_paused(state.project, id, false).unwrap();
        assert!(state.intruder.advance_candidate().is_none());
        state
            .rename_intruder(state.project, id, Uuid::new_v4(), 0, "Session 名称")
            .unwrap();
        release_first.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        assert!(!state.intruder.runs[&id].waiting_for_capacity);
        assert_eq!(state.intruder.advance_candidate(), Some(id));
        state.advance_intruder(id).await.unwrap();
        second_seen.await.unwrap();
        state.intruder.runs[&id].cancel.cancel();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert!(!state.intruder.has_resources());
        let page = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Stopped);
        assert_eq!(page.run.retention, IntruderRetention::Session);
        assert_eq!(
            (page.run.dispatched, page.run.completed, page.rows.len()),
            (2, 2, 2)
        );
        assert_eq!(
            (page.run.title.as_str(), page.run.title_revision),
            ("Session 名称", 1)
        );
        assert!(!page.persisted);
        assert!(state.intruder.recovery.is_empty());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("two one-byte requests, causal release and Stop must join within five seconds");
}

#[tokio::test]
async fn session_capacity_exhaustion_fails_before_opening_a_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (mut state, _events) = state();
    state.store.set_intruder_session_quota(256 * 1024);
    let mut input = spec(format!("http://{address}"), 1);
    input.save_to_project = false;
    let id = admit(&mut state, input);
    assert!(state.advance_intruder(id).await.is_err());
    assert_eq!(state.intruder.advance_candidate(), Some(id));
    state.advance_intruder(id).await.unwrap();
    assert!(!state.intruder.has_resources());
    assert!(state.intruder.recovery.is_empty());
    let page = state.intruder_result_page(id, None, 10).unwrap();
    assert!(matches!(page.run.state, IntruderRunState::Failed(_)));
    assert_eq!((page.run.dispatched, page.run.completed), (0, 0));
    assert!(page.rows.is_empty());
    assert!(!page.persisted);
    assert_eq!(page.run.retention, IntruderRetention::Session);
    assert_eq!(
        listener.into_std().unwrap().accept().unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn session_failed_completion_recovers_only_in_ram_and_never_replays_the_request() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            seen.send(()).unwrap();
            released.await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\x00\xff",
                )
                .await
                .unwrap();
            listener
        });
        let (mut state, _events) = state();
        let mut input = spec(format!("http://{address}"), 1);
        input.save_to_project = false;
        let id = admit(&mut state, input);
        state.advance_intruder(id).await.unwrap();
        received.await.unwrap();
        state.store.reject_session_intruder_writes(id, true);
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        assert!(state.complete_intruder(completion).await.is_err());
        assert!(state.stop_intruder().await.is_err());
        assert!(!state.intruder.has_resources());
        let before = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!((before.run.dispatched, before.run.completed), (1, 1));
        let IntruderResultOutcome::Flow(summary) = &before.rows[0].outcome else {
            panic!("failed RAM commit retains its real flow")
        };
        let record = state.read_flow(summary.id).unwrap();
        assert_eq!(record.response.as_ref().unwrap().body, [0, 255]);
        let recovery = state.intruder.recovery.len();
        assert!(recovery > 0);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovered.mio");
        assert!(save(&mut state, Some(path.clone())).await.is_err());
        assert_eq!(state.intruder.recovery.len(), recovery);
        assert!(!path.exists());
        assert_eq!(state.read_flow(summary.id).unwrap(), record);
        state.store.reject_session_intruder_writes(id, false);
        save(&mut state, Some(path.clone())).await.unwrap();
        assert!(state.intruder.recovery.is_empty());
        let after = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(after, before);
        assert_eq!(state.read_flow(summary.id).unwrap(), record);
        let disk =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let disk_runs: u64 = disk
            .query_row("SELECT count(*) FROM intruder_runs", [], |row| row.get(0))
            .unwrap();
        let disk_flows: u64 = disk
            .query_row("SELECT count(*) FROM flows", [], |row| row.get(0))
            .unwrap();
        assert_eq!((disk_runs, disk_flows), (0, 0));
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("RAM write failure, explicit recovery and one real response are bounded");
}

#[tokio::test]
async fn live_promotion_preserves_request_identity_and_releases_capacity_without_stopping() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (first_seen_tx, first_seen) = tokio::sync::oneshot::channel();
        let (second_seen_tx, second_seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut first).await, 255);
            first_seen_tx.send(()).unwrap();
            let (mut second, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut second).await, 255);
            second_seen_tx.send(()).unwrap();
            released.await.unwrap();
            for stream in [&mut first, &mut second] {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\x00\xff").await.unwrap();
            }
            listener
        });
        let (mut state, _events) = state();
        let directory = tempfile::tempdir().unwrap();
        save(&mut state, Some(directory.path().join("live-promotion.mio"))).await.unwrap();
        state.store.set_intruder_session_quota(24 * 1024 * 1024);
        let mut input = spec(format!("http://{address}"), 2);
        input.save_to_project = false;
        let id = admit(&mut state, input.clone());
        state.advance_intruder(id).await.unwrap();
        first_seen.await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert!(state.intruder.runs[&id].waiting_for_capacity);
        state.rename_intruder(state.project, id, Uuid::new_v4(), 0, "运行中保存").unwrap();
        let before = state.intruder.runs[&id].summary.clone();
        let planned = state.intruder.runs[&id].plan.request(1).unwrap();
        let requests: HashMap<_, _> = state.intruder.requests.iter()
            .map(|(job, request)| (*job, (request.run_id, request.ordinal, request.flow_id))).collect();
        assert_eq!((state.intruder.jobs.len(), requests.len()), (1, 1));
        let mut workspace = state.store.workspace().unwrap();
        workspace.revision += 1;
        let receipt = state.set_intruder_retention(state.project, id, before.revision,
            IntruderRetention::Project, workspace.clone()).unwrap();
        let mut expected = before;
        expected.revision += 1;
        expected.retention = IntruderRetention::Project;
        assert_eq!(receipt.run, expected);
        assert_eq!(receipt.workspace, workspace);
        assert_eq!(state.intruder.runs[&id].summary, expected);
        assert_eq!(state.intruder_run_page(None, 10).unwrap().rows, [expected.clone()]);
        assert_eq!(state.intruder.runs[&id].active, 1);
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        assert!(!state.intruder.runs[&id].waiting_for_capacity);
        assert_eq!(state.intruder.runs[&id].plan.request(1).unwrap(), planned);
        assert_eq!(state.intruder.jobs.len(), 1);
        assert_eq!(state.intruder.requests.iter().map(|(job, request)|
            (*job, (request.run_id, request.ordinal, request.flow_id))).collect::<HashMap<_, _>>(), requests);
        assert!(!state.store.is_session_intruder_run(id));
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        let mut forbidden = workspace.clone();
        forbidden.revision += 1;
        assert!(state.set_intruder_retention(state.project, id, expected.revision,
            IntruderRetention::Session, forbidden.clone()).is_err());
        assert!(state.discard_intruder(state.project, id, expected.revision, forbidden).is_err());
        assert_eq!(state.intruder.runs[&id].summary, expected);
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        assert_eq!(state.intruder.advance_candidate(), Some(id));
        state.advance_intruder(id).await.unwrap();
        second_seen.await.unwrap();
        release.send(()).unwrap();
        while let Some(completion) = state.intruder.jobs.join_next_with_id().await {
            state.complete_intruder(completion).await.unwrap();
        }
        state.advance_intruder(id).await.unwrap();
        assert!(!state.intruder.has_resources());
        let page = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!((page.run.dispatched, page.run.completed, page.rows.len()), (2, 2, 2));
        assert_eq!((page.run.title.as_str(), page.run.title_revision), ("运行中保存", 1));
        assert!(page.persisted);
        for row in page.rows {
            let IntruderResultOutcome::Flow(flow) = row.outcome else { panic!("a real response was required") };
            assert_eq!(state.read_flow(flow.id).unwrap().response.unwrap().body, [0, 255]);
        }
        assert!(state.intruder.recovery.is_empty());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }).await.expect("promotion retains two tiny real requests and does not pause, stop, or join them");
}

#[tokio::test]
async fn failed_live_promotion_keeps_paused_request_and_requires_explicit_retry() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
            listener
        });
        let (mut state, _events) = state();
        let directory = tempfile::tempdir().unwrap();
        save(
            &mut state,
            Some(directory.path().join("paused-promotion.mio")),
        )
        .await
        .unwrap();
        let mut input = spec(format!("http://{address}"), 2);
        input.save_to_project = false;
        input.max_concurrent_requests = 1;
        let id = admit(&mut state, input);
        state.advance_intruder(id).await.unwrap();
        seen.await.unwrap();
        let before = state.set_intruder_paused(state.project, id, true).unwrap();
        let requests: HashMap<_, _> = state
            .intruder
            .requests
            .iter()
            .map(|(job, request)| (*job, (request.run_id, request.ordinal, request.flow_id)))
            .collect();
        let workspace = state.store.workspace().unwrap();
        let mut proposal = workspace.clone();
        proposal.revision += 1;
        state.store.reject_writes(true);
        assert!(
            state
                .set_intruder_retention(
                    state.project,
                    id,
                    before.revision,
                    IntruderRetention::Project,
                    proposal.clone()
                )
                .is_err()
        );
        assert_eq!(state.intruder.runs[&id].summary, before);
        assert_eq!(state.store.workspace().unwrap(), workspace);
        assert!(state.store.is_session_intruder_run(id));
        assert_eq!(state.intruder.jobs.len(), 1);
        assert_eq!(
            state
                .intruder
                .requests
                .iter()
                .map(|(job, request)| (*job, (request.run_id, request.ordinal, request.flow_id)))
                .collect::<HashMap<_, _>>(),
            requests
        );
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        assert!(state.intruder.recovery.is_empty());
        state.store.reject_writes(false);
        save(&mut state, None).await.unwrap();
        assert!(
            state.store.is_session_intruder_run(id),
            "ordinary Save cannot retry failed promotion"
        );
        assert_eq!(state.intruder.runs[&id].summary, before);
        let receipt = state
            .set_intruder_retention(
                state.project,
                id,
                before.revision,
                IntruderRetention::Project,
                proposal,
            )
            .unwrap();
        assert_eq!(receipt.run.state, IntruderRunState::Paused);
        assert_eq!(state.intruder.runs[&id].summary, receipt.run);
        assert_eq!(state.intruder.runs[&id].active, 1);
        assert_eq!(state.intruder.jobs.len(), 1);
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        assert!(
            state.intruder.advance_candidate().is_none(),
            "promotion cannot resume a paused plan"
        );
        let page = state.intruder_result_page(id, None, 10).unwrap();
        assert!(page.persisted);
        assert_eq!(page.run.state, IntruderRunState::Paused);
        assert_eq!((page.run.dispatched, page.run.completed), (1, 1));
        // Explicit test cleanup; promotion itself never requested Stop.
        state.request_intruder_stop(state.project, id).unwrap();
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert!(!state.intruder.has_resources());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("a failed promotion and explicit retry preserve the original paused socket");
}
