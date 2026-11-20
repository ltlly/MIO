use super::*;

#[tokio::test]
async fn a_full_ui_queue_cannot_block_joined_ext_cleanup_or_drop_its_terminal_ack() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let inspection = import::inspect(path, &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    let inspection_id = inspection.info.id;
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let job_id = Uuid::new_v4();
    state
        .events
        .try_send(Event::ExtProgress {
            project: state.project,
            job_id,
            phase: ExtImportPhase::Snapshot,
            completed: 0,
            total: Some(72),
        })
        .unwrap();
    let (cleaned, cleanup_complete) = oneshot::channel();
    state.ext_archive.start(
        state.project,
        job_id,
        CancellationToken::new(),
        true,
        move |_, _| {
            inspection.release().unwrap();
            cleaned.send(()).unwrap();
            Output::Finished(ExtJobOutcome::Released { inspection_id }, None)
        },
    );
    cleanup_complete.await.unwrap();
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    let stopped = tokio::time::timeout(Duration::from_millis(250), state.stop_ext()).await;
    assert!(
        stopped.is_ok(),
        "UI backpressure must not hold a completed cleanup past the caller's bound"
    );
    stopped.unwrap().unwrap();
    assert!(state.ext_archive.active.is_none());
    assert!(!state.ext_archive.can_accept_commands());
    assert!(
        outcomes(&mut receiver).is_empty(),
        "the retained UI item is not the completion"
    );
    let signal = tokio::time::timeout(
        Duration::from_millis(250),
        state.ext_archive.next_signal(&state.events, false),
    )
    .await
    .unwrap();
    state.ext_signal(signal).await;
    assert!(
        matches!(outcomes(&mut receiver).as_slice(), [(id, ExtJobOutcome::Released { inspection_id: released })]
        if *id == job_id && *released == inspection_id)
    );
    assert!(state.ext_archive.can_accept_commands());
    assert!(receiver.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_snapshot_cleanup_cannot_make_stop_report_success() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let inspection = inspected_with_snapshot(path);
    let inspection_id = inspection.info.id;
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    state.ext_archive.inspection = Some((state.project, inspection));
    let original = std::fs::metadata(directory.path()).unwrap().permissions();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    state
        .release_ext(state.project, Uuid::new_v4(), inspection_id)
        .await;
    let stopped = tokio::time::timeout(Duration::from_secs(1), state.stop_ext()).await;
    std::fs::set_permissions(directory.path(), original).unwrap();
    let stopped = stopped.expect("permission failure is immediate, not a slow-cleanup test");
    assert!(
        stopped.is_err(),
        "the cleanup failure must reach the caller of Stop/Quiesce"
    );
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [(
            _,
            ExtJobOutcome::Failed {
                created_path: None,
                ..
            }
        )]
    ));
    assert_eq!(
        std::fs::read_dir(directory.path()).unwrap().count(),
        2,
        "the inaccessible parent prevented removal of the private snapshot directory"
    );
    assert!(state.ext_archive.cleanup.is_some());
    let saved_directory = tempfile::tempdir().unwrap();
    let destination = saved_directory.path().join("after-cleanup.mio");
    let blocked_save = state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(destination.clone()),
            workspace: None,
        })
        .await;
    let save_preserved = state.store.path.is_none() && !destination.exists();
    state
        .inspect_ext(
            state.project,
            Uuid::new_v4(),
            directory.path().join("not-opened.ext_archive"),
            CancellationToken::new(),
        )
        .await;
    state
        .convert_ext(
            state.project,
            Uuid::new_v4(),
            inspection_id,
            directory.path().join("not-created.mio"),
            CancellationToken::new(),
        )
        .await;
    assert!(state.ext_archive.active.is_none());
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [
            (_, ExtJobOutcome::Failed { .. }),
            (_, ExtJobOutcome::Failed { .. })
        ]
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    tokio::time::timeout(
        Duration::from_secs(1),
        state.command(Command::Quiesce {
            project: state.project,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(state.ext_archive.active.is_none());
    assert!(state.ext_archive.cleanup.is_none());
    assert!(state.ext_archive.worker_error.is_none());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    let mut quiesced = false;
    while let Ok(event) = receiver.try_recv() {
        assert!(
            !matches!(event, Event::ExtJob { .. }),
            "a cleanup retry must not issue a second terminal ACK for the failed job"
        );
        quiesced |= matches!(event, Event::Quiesced { .. });
    }
    assert!(quiesced);
    let saved = state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(destination.clone()),
            workspace: None,
        })
        .await;
    let closed = state.store.close();
    assert!(
        blocked_save.is_err() && save_preserved,
        "pending cleanup must reject Save without changing the current project: {blocked_save:?}"
    );
    saved.unwrap();
    closed.unwrap();
    assert!(destination.is_file());
}

#[tokio::test]
async fn a_rejected_command_and_joined_completion_keep_two_bounded_ordered_acks() {
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let accepted = Uuid::new_v4();
    state
        .events
        .try_send(Event::ExtProgress {
            project: state.project,
            job_id: accepted,
            phase: ExtImportPhase::Snapshot,
            completed: 0,
            total: None,
        })
        .unwrap();
    state.ext_archive.start(
        state.project,
        accepted,
        CancellationToken::new(),
        true,
        |_, _| Output::Finished(ExtJobOutcome::Cancelled, None),
    );
    let rejected = Uuid::new_v4();
    tokio::time::timeout(
        Duration::from_millis(250),
        state.inspect_ext(
            state.project,
            rejected,
            PathBuf::new(),
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    assert!(!state.ext_archive.can_accept_commands());
    tokio::time::timeout(Duration::from_millis(250), state.stop_ext())
        .await
        .unwrap()
        .unwrap();
    assert!(state.ext_archive.active.is_none());
    assert_eq!(state.ext_archive.acknowledgements.iter().flatten().count(), 2);
    assert!(outcomes(&mut receiver).is_empty());
    for expected in [rejected, accepted] {
        let signal = tokio::time::timeout(
            Duration::from_millis(250),
            state.ext_archive.next_signal(&state.events, false),
        )
        .await
        .unwrap();
        state.ext_signal(signal).await;
        let values = outcomes(&mut receiver);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0, expected);
        if expected == rejected {
            assert!(matches!(values[0].1, ExtJobOutcome::Failed { .. }));
            assert!(!state.ext_archive.can_accept_commands());
        } else {
            assert!(matches!(values[0].1, ExtJobOutcome::Cancelled));
            assert!(state.ext_archive.can_accept_commands());
        }
    }
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn timed_out_stop_retains_the_worker_and_global_shutdown_still_joins_cleanup() {
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let inspection = import::inspect(path, &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    let inspection_id = inspection.info.id;
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let job_id = Uuid::new_v4();
    state
        .events
        .try_send(Event::ExtProgress {
            project: state.project,
            job_id,
            phase: ExtImportPhase::Snapshot,
            completed: 0,
            total: None,
        })
        .unwrap();
    let (started, ready) = oneshot::channel();
    let (resume, gate) = oneshot::channel::<()>();
    let (cleaned, cleanup_complete) = oneshot::channel();
    let cancel = CancellationToken::new();
    state
        .ext_archive
        .start(state.project, job_id, cancel.clone(), true, move |_, _| {
            started.send(()).unwrap();
            let _ = gate.blocking_recv();
            inspection.release().unwrap();
            cleaned.send(()).unwrap();
            Output::Finished(ExtJobOutcome::Released { inspection_id }, None)
        });
    ready.await.unwrap();
    let stopped = state.stop_ext_until(tokio::time::Instant::now()).await;
    assert!(stopped.is_err());
    assert!(cancel.is_cancelled());
    assert!(state.ext_archive.active.is_some());
    assert!(state.ext_archive.stop_timed_out);
    state.cancel.cancel();
    let mut final_cleanup = Box::pin(state.finish_ext_shutdown());
    assert!(
        poll_fn(|cx| Poll::Ready(final_cleanup.as_mut().poll(cx)))
            .await
            .is_pending(),
        "the final shutdown must own the worker until the causal gate opens"
    );
    resume.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), final_cleanup)
        .await
        .unwrap();
    assert!(result.is_err(), "the exceeded Stop deadline stays visible");
    cleanup_complete.await.unwrap();
    assert!(state.ext_archive.active.is_none());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(outcomes(&mut receiver).is_empty());
    let signal = tokio::time::timeout(
        Duration::from_millis(250),
        state.ext_archive.next_signal(&state.events, false),
    )
    .await
    .unwrap();
    state.ext_signal(signal).await;
    assert!(
        matches!(outcomes(&mut receiver).as_slice(), [(id, ExtJobOutcome::Released { inspection_id: released })]
        if *id == job_id && *released == inspection_id)
    );
}

#[tokio::test]
async fn a_worker_panic_cannot_be_mistaken_for_confirmed_cleanup() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    state.ext_archive.start(
        state.project,
        Uuid::new_v4(),
        CancellationToken::new(),
        true,
        |_, _| panic!("synthetic worker failure"),
    );
    assert!(state.stop_ext().await.is_err());
    assert!(state.ext_archive.active.is_none());
    assert!(state.ext_archive.worker_error.is_some());
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [(_, ExtJobOutcome::Failed { .. })]
    ));
    assert!(state.finish_ext_shutdown().await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn unsaved_traffic_cannot_hide_ext_cleanup_failure_behind_a_quiesced_ack() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let inspection = inspected_with_snapshot(path);
    let (events, mut receiver) = mpsc::channel(16);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let record = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Repeater,
            method: "GET".into(),
            url: "http://localhost/unsaved".into(),
            status: None,
            state: FlowState::Cancelled,
            request_bytes: 0,
            response_bytes: 0,
            started_unix_ms: 1,
            elapsed_ms: 0,
        },
        request: parse_request(
            b"GET /unsaved HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "http://localhost/unsaved",
            HttpVersion::Http11,
        )
        .unwrap(),
        response: None,
    };
    state.store.reject_writes(true);
    assert!(state.persist(&record).is_err());
    assert_eq!(state.unsaved_count(), 1);
    state.ext_archive.inspection = Some((state.project, inspection));
    let original = std::fs::metadata(directory.path()).unwrap().permissions();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        state.command(Command::Quiesce {
            project: state.project,
        }),
    )
    .await;
    std::fs::set_permissions(directory.path(), original).unwrap();
    assert!(
        result.unwrap().is_err(),
        "unsaved records cannot turn unfinished External cleanup into successful Quiesce"
    );
    while let Ok(event) = receiver.try_recv() {
        assert!(
            !matches!(event, Event::Quiesced { .. }),
            "Quiesced is only valid after every External resource is reclaimed"
        );
    }
    assert!(state.ext_archive.cleanup.is_some());
    assert_eq!(state.unsaved_count(), 1);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    state.stop_ext().await.unwrap();
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
