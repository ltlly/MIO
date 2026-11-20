use super::*;

async fn rename(
    state: &mut State,
    events: &mut mpsc::Receiver<Event>,
    project: ProjectKey,
    run_id: Uuid,
    operation_id: Uuid,
    expected_title_revision: u64,
    title: &str,
) -> std::result::Result<IntruderRunSummary, String> {
    state
        .command(Command::RenameIntruder {
            project,
            run_id,
            operation_id,
            expected_title_revision,
            title: title.into(),
        })
        .await
        .unwrap();
    while let Ok(event) = events.try_recv() {
        if let Event::IntruderRenamed {
            project: owner,
            run_id: actual,
            operation_id: operation,
            result,
        } = event
        {
            assert_eq!((owner, actual, operation), (project, run_id, operation_id));
            return result;
        }
        assert!(
            !matches!(event, Event::Error { .. }),
            "rename failures need their operation identity"
        );
    }
    panic!("rename must return a correlated receipt");
}

#[tokio::test]
async fn task_title_commands_reject_stale_project_operation_and_cas_without_queued_recovery() {
    let (mut state, mut events) = state();
    let project = state.project;
    let (original, _) = terminal_discard_run(&mut state);
    let operation = Uuid::new_v4();
    let first = rename(
        &mut state,
        &mut events,
        project,
        original.id,
        operation,
        0,
        "name",
    )
    .await
    .unwrap();
    assert_eq!(first.revision, original.revision);
    assert_eq!(first.title_revision, 1);
    assert!(
        rename(
            &mut state,
            &mut events,
            project,
            original.id,
            operation,
            0,
            "replay"
        )
        .await
        .is_err()
    );
    assert!(
        rename(
            &mut state,
            &mut events,
            project,
            original.id,
            Uuid::nil(),
            1,
            "nil operation"
        )
        .await
        .is_err()
    );
    let old = ProjectKey {
        generation: project.generation + 1,
        ..project
    };
    assert!(
        rename(
            &mut state,
            &mut events,
            old,
            original.id,
            Uuid::new_v4(),
            1,
            "old project"
        )
        .await
        .is_err()
    );
    assert!(
        rename(
            &mut state,
            &mut events,
            project,
            original.id,
            Uuid::new_v4(),
            1,
            &"中".repeat(1366)
        )
        .await
        .is_err()
    );
    assert!(state.intruder.recovery.is_empty());
    let directory = tempfile::tempdir().unwrap();
    save(&mut state, Some(directory.path().join("saved.mio")))
        .await
        .unwrap();
    assert_eq!(state.store.read_intruder_run(original.id).unwrap(), first);
    state.quiesced = true;
    assert!(
        rename(
            &mut state,
            &mut events,
            project,
            original.id,
            Uuid::new_v4(),
            1,
            "quiesced"
        )
        .await
        .is_err()
    );
    assert_eq!(state.store.read_intruder_run(original.id).unwrap(), first);
}

#[tokio::test]
async fn task_title_running_rename_survives_real_response_and_stop_without_changing_dispatch() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            seen.send(()).unwrap();
            released.await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
            listener
        });
        let (mut state, mut events) = state();
        let project = state.project;
        let id = admit(&mut state, spec(format!("http://{address}"), 2));
        state.advance_intruder(id).await.unwrap();
        received.await.unwrap();
        let before = state.intruder.runs[&id].summary.clone();
        let renamed = rename(
            &mut state,
            &mut events,
            project,
            id,
            Uuid::new_v4(),
            0,
            "running name",
        )
        .await
        .unwrap();
        assert_eq!(renamed.revision, before.revision);
        assert_eq!(state.intruder.runs[&id].summary, renamed);
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        state.request_intruder_stop(project, id).unwrap();
        state.advance_intruder(id).await.unwrap();
        assert_eq!(
            state.intruder.runs[&id].summary.state,
            IntruderRunState::Stopping
        );
        state.advance_intruder(id).await.unwrap();
        let page = state.intruder_result_page(id, None, 10).unwrap();
        assert_eq!(page.run.title, "running name");
        assert_eq!(page.run.title_revision, 1);
        assert_eq!(
            (page.run.dispatched, page.run.completed, page.rows.len()),
            (1, 1, 1)
        );
        assert_eq!(page.run.state, IntruderRunState::Stopped);
        assert!(!state.intruder.has_resources());
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("single loopback request, rename and join are bounded");
}
