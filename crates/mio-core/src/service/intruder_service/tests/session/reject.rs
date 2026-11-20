use super::*;

#[test]
fn live_promotion_rejects_stale_revision_cancelled_stopping_and_broken_registration() {
    let (mut state, _events) = state();
    let mut input = spec("http://127.0.0.1:1".into(), 1);
    input.save_to_project = false;
    let id = admit(&mut state, input);
    let before = state.intruder.runs[&id].summary.clone();
    let workspace = state.store.workspace().unwrap();
    let mut proposal = workspace.clone();
    proposal.revision += 1;
    assert!(
        state
            .set_intruder_retention(
                state.project,
                id,
                before.revision + 1,
                IntruderRetention::Project,
                proposal.clone()
            )
            .is_err()
    );
    assert_eq!(state.store.workspace().unwrap(), workspace);
    assert_eq!(state.intruder.runs[&id].summary, before);
    assert!(!state.intruder.runs[&id].cancel.is_cancelled());
    // Simulate an inconsistent internal registration: the guard must reject before storage changes.
    state.intruder.runs.get_mut(&id).unwrap().active = 1;
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
    state.intruder.runs.get_mut(&id).unwrap().active = 0;
    assert_eq!(state.store.read_intruder_run(id).unwrap(), before);
    state.request_intruder_stop(state.project, id).unwrap();
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
    state
        .persist_intruder_state(id, IntruderRunState::Stopping)
        .unwrap();
    let stopping = state.intruder.runs[&id].summary.clone();
    assert!(
        state
            .set_intruder_retention(
                state.project,
                id,
                stopping.revision,
                IntruderRetention::Project,
                proposal
            )
            .is_err()
    );
    assert_eq!(state.store.read_intruder_run(id).unwrap(), stopping);
    assert_eq!(state.store.workspace().unwrap(), workspace);
    assert!(state.store.is_session_intruder_run(id));
    assert!(state.intruder.recovery.is_empty());
    assert!(state.intruder.jobs.is_empty() && state.intruder.requests.is_empty());
}

#[tokio::test]
async fn promoted_live_completion_failure_uses_only_project_recovery() {
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
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\x00\xff",
                )
                .await
                .unwrap();
            listener
        });
        let (mut state, _events) = state();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("promoted-recovery.mio");
        save(&mut state, Some(path.clone())).await.unwrap();
        let mut input = spec(format!("http://{address}"), 1);
        input.save_to_project = false;
        let id = admit(&mut state, input);
        state.advance_intruder(id).await.unwrap();
        seen.await.unwrap();
        let before = state.intruder.runs[&id].summary.clone();
        let mut workspace = state.store.workspace().unwrap();
        workspace.revision += 1;
        let promoted = state
            .set_intruder_retention(
                state.project,
                id,
                before.revision,
                IntruderRetention::Project,
                workspace,
            )
            .unwrap()
            .run;
        assert_eq!(state.intruder.runs[&id].summary, promoted);
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        state.store.reject_writes(true);
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        assert!(state.complete_intruder(completion).await.is_err());
        assert!(state.stop_intruder().await.is_err());
        assert!(!state.intruder.has_resources());
        let count = state.intruder.recovery.len();
        assert!(count > 0);
        assert!(state.intruder.recovery.iter().all(|mutation| {
            let run = mutation_run(mutation);
            run.id == id && run.retention == IntruderRetention::Project
        }));
        assert!(!state.store.is_session_intruder_run(id));
        let before = state.intruder_result_page(id, None, 10).unwrap();
        assert!(!before.persisted);
        assert_eq!(before.run.retention, IntruderRetention::Project);
        assert_eq!(
            state.intruder_run_page(None, 10).unwrap().rows.as_slice(),
            std::slice::from_ref(&before.run)
        );
        let IntruderResultOutcome::Flow(flow) = &before.rows[0].outcome else {
            panic!("the real completed response remains available")
        };
        let record = state.read_flow(flow.id).unwrap();
        assert_eq!(record.response.as_ref().unwrap().body, [0, 255]);
        state.retry_session_intruder_recovery().unwrap();
        assert_eq!(
            state.intruder.recovery.len(),
            count,
            "RAM recovery must leave every promoted Project mutation for disk recovery"
        );
        assert!(save(&mut state, None).await.is_err());
        assert_eq!(state.intruder.recovery.len(), count);
        state.store.reject_writes(false);
        save(&mut state, None).await.unwrap();
        assert!(state.intruder.recovery.is_empty());
        let after = state.intruder_result_page(id, None, 10).unwrap();
        assert!(after.persisted);
        assert_eq!(after.run, before.run);
        assert_eq!(after.rows, before.rows);
        assert_eq!(state.read_flow(flow.id).unwrap(), record);
        let disk =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let body: Vec<u8> = disk
            .query_row(
                "SELECT body FROM messages WHERE flow=?1 AND side=1",
                [flow.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(body, [0, 255]);
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("one promoted response with failed disk write and explicit recovery must not replay");
}

#[tokio::test]
async fn live_promotion_checks_each_registered_flow_and_ordinal_before_copying() {
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
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK").await.unwrap();
            listener
        });
        let (mut state, _events) = state();
        let mut input = spec(format!("http://{address}"), 1);
        input.save_to_project = false;
        let id = admit(&mut state, input);
        state.advance_intruder(id).await.unwrap();
        seen.await.unwrap();
        let before = state.intruder.runs[&id].summary.clone();
        let before_page = state.intruder_result_page(id, None, 10).unwrap();
        let workspace = state.store.workspace().unwrap();
        let mut proposal = workspace.clone();
        proposal.revision += 1;
        let (&job_id, request) = state.intruder.requests.iter().next().unwrap();
        let original = (request.run_id, request.ordinal, request.flow_id);
        let record = state.read_flow(original.2).unwrap();
        assert!(record.response.is_none());
        for altered in [
            (id, original.1, Uuid::new_v4()),
            (id, original.1 + 1, original.2),
        ] {
            let request = state.intruder.requests.get_mut(&job_id).unwrap();
            (request.run_id, request.ordinal, request.flow_id) = altered;
            assert_eq!((state.intruder.requests.len(), state.intruder.jobs.len()), (1, 1));
            assert_eq!(state.intruder.runs[&id].active, 1);
            assert!(
                state.set_intruder_retention(state.project, id, before.revision,
                    IntruderRetention::Project, proposal.clone()).is_err(),
                "unchanged registration counts cannot authorize a different flow or ordinal: {altered:?}"
            );
            assert_eq!(state.intruder.runs[&id].summary, before);
            assert_eq!(state.store.workspace().unwrap(), workspace);
            assert_eq!(state.intruder_result_page(id, None, 10).unwrap(), before_page);
            assert_eq!(state.read_flow(original.2).unwrap(), record);
            assert!(state.store.is_session_intruder_run(id));
            assert!(!state.intruder.runs[&id].cancel.is_cancelled());
            assert!(state.intruder.recovery.is_empty());
            assert_eq!((state.intruder.requests.len(), state.intruder.jobs.len()), (1, 1));
            let request = &state.intruder.requests[&job_id];
            assert_eq!((request.run_id, request.ordinal, request.flow_id), altered);
            let request = state.intruder.requests.get_mut(&job_id).unwrap();
            (request.run_id, request.ordinal, request.flow_id) = original;
        }
        let promoted = state.set_intruder_retention(state.project, id, before.revision,
            IntruderRetention::Project, proposal).unwrap().run;
        assert_eq!(promoted.retention, IntruderRetention::Project);
        assert_eq!(promoted.revision, before.revision + 1);
        let request = &state.intruder.requests[&job_id];
        assert_eq!((request.run_id, request.ordinal, request.flow_id), original);
        assert_eq!((state.intruder.requests.len(), state.intruder.jobs.len()), (1, 1));
        assert!(!state.intruder.runs[&id].cancel.is_cancelled());
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        assert!(!state.intruder.has_resources());
        let completed = state.intruder_result_page(id, None, 10).unwrap();
        assert!(completed.persisted);
        assert_eq!(completed.run.state, IntruderRunState::Completed);
        assert_eq!((completed.run.dispatched, completed.run.completed), (1, 1));
        assert_eq!(state.read_flow(original.2).unwrap().response.unwrap().body, b"OK");
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }).await.expect("one held request must reject corrupt registrations then promote without replay");
}
