use super::*;

#[tokio::test]
async fn terminal_discard_replies_are_correlated_and_do_not_stop_another_run() {
    let (mut state, mut events) = state();
    let (finished, flow) = terminal_discard_run(&mut state);
    let project = state.project;
    let active = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
    state.set_intruder_paused(project, active, true).unwrap();
    let cancel = state.intruder.runs[&active].cancel.clone();
    for (owner, id, revision) in [
        (
            ProjectKey {
                generation: project.generation + 1,
                ..project
            },
            finished.id,
            finished.revision,
        ),
        (project, Uuid::new_v4(), finished.revision),
        (project, finished.id, finished.revision + 1),
        (
            project,
            active,
            state.intruder_run(active).unwrap().revision,
        ),
    ] {
        let operation_id = Uuid::new_v4();
        state
            .command(Command::DiscardIntruder {
                project: owner,
                run_id: id,
                operation_id,
                expected_revision: revision,
                workspace: Box::new(discard_workspace(&state)),
            })
            .await
            .unwrap();
        assert!(discard_reply(&mut events, owner, id, operation_id).is_err());
        assert_eq!(state.store.read(flow.summary.id).unwrap(), flow);
        assert!(!cancel.is_cancelled());
    }
    let operation_id = Uuid::new_v4();
    let proposed = discard_workspace(&state);
    state
        .command(Command::DiscardIntruder {
            project,
            run_id: finished.id,
            operation_id,
            expected_revision: finished.revision,
            workspace: Box::new(proposed.clone()),
        })
        .await
        .unwrap();
    let receipt = discard_reply(&mut events, project, finished.id, operation_id).unwrap();
    assert_eq!(receipt.workspace, proposed);
    assert_eq!((receipt.deleted_flows, receipt.retained_flows), (1, 0));
    assert!(
        state
            .store
            .find_intruder_run(finished.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state.intruder.runs[&active].summary.state,
        IntruderRunState::Paused
    );
    assert!(!cancel.is_cancelled());
    state.stop_intruder().await.unwrap();
    assert!(!state.intruder.has_resources());
}

#[tokio::test]
async fn terminal_discard_checks_owned_handles_even_if_disk_claims_terminal() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (mut state, _events) = state();
        let (finished, flow) = terminal_discard_run(&mut state);
        let project = state.project;
        let (release, released) = oneshot::channel();
        let record = flow.clone();
        let worker = state.intruder.jobs.spawn(async move {
            let _ = released.await;
            Completion {
                project,
                record,
                reason_phrase: None,
            }
        });
        let task_id = worker.id();
        state.intruder.requests.insert(
            task_id,
            ActiveRequest {
                run_id: finished.id,
                ordinal: 0,
                flow_id: flow.summary.id,
                resource_pool: None,
            },
        );
        let proposal = discard_workspace(&state);
        assert!(
            state
                .discard_intruder(project, finished.id, finished.revision, proposal.clone())
                .is_err()
        );
        assert!(!worker.is_finished());
        assert_eq!(state.intruder.jobs.len(), 1);
        state.intruder.requests.remove(&task_id);
        assert!(
            state
                .discard_intruder(project, finished.id, finished.revision, proposal.clone())
                .is_err(),
            "unregistered handles also prevent deletion"
        );
        assert_eq!(state.store.read(flow.summary.id).unwrap(), flow);
        release.send(()).unwrap();
        let (joined, completion) = state
            .intruder
            .jobs
            .join_next_with_id()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(joined, task_id);
        assert_eq!(completion.record, flow);
        assert!(worker.is_finished());
        state
            .discard_intruder(project, finished.id, finished.revision, proposal)
            .unwrap();
        assert!(
            state
                .store
                .find_intruder_run(finished.id)
                .unwrap()
                .is_none()
        );
    })
    .await
    .expect("discard never waits for or cancels a live sender");
}

#[tokio::test]
async fn terminal_discard_failure_keeps_recovery_until_explicit_save_and_retry() {
    let (mut state, mut events) = state();
    let project = state.project;
    let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
    state.store.reject_writes(true);
    assert!(state.advance_intruder(id).await.is_err());
    assert!(state.stop_intruder().await.is_err());
    assert!(!state.intruder.has_resources());
    let unsaved = state.intruder_result_page(id, None, 1).unwrap();
    let pending = state.intruder.recovery.len();
    let failure = state.storage_error.clone();
    let proposal = discard_workspace(&state);
    let operation_id = Uuid::new_v4();
    state
        .command(Command::DiscardIntruder {
            project,
            run_id: id,
            operation_id,
            expected_revision: unsaved.run.revision,
            workspace: Box::new(proposal.clone()),
        })
        .await
        .unwrap();
    assert!(
        discard_reply(&mut events, project, id, operation_id)
            .unwrap_err()
            .contains("尚未写盘")
    );
    assert_eq!(state.intruder.recovery.len(), pending);
    assert!(pending > 0);
    assert_eq!(state.storage_error, failure);
    assert_eq!(state.intruder_result_page(id, None, 1).unwrap(), unsaved);
    let directory = tempfile::tempdir().unwrap();
    save(&mut state, Some(directory.path().join("recovery.mio")))
        .await
        .unwrap();
    let persisted = state.intruder_result_page(id, None, 1).unwrap();
    assert!(persisted.persisted);
    assert_eq!(persisted.run, unsaved.run);
    assert_eq!(persisted.rows, unsaved.rows);
    assert_eq!(state.unsaved_count(), 0);
    state.store.reject_writes(true);
    assert!(
        state
            .discard_intruder(project, id, persisted.run.revision, proposal.clone())
            .is_err()
    );
    assert_eq!(state.intruder_result_page(id, None, 1).unwrap(), persisted);
    assert_eq!(
        state.unsaved_count(),
        0,
        "failed deletion must not queue a future destructive retry"
    );
    state.store.reject_writes(false);
    state
        .discard_intruder(project, id, persisted.run.revision, proposal)
        .unwrap();
    assert!(state.store.find_intruder_run(id).unwrap().is_none());
}

#[tokio::test]
async fn terminal_discard_commits_before_its_bounded_backpressured_receipt() {
    let (mut state, mut events) = state();
    let project = state.project;
    let (finished, _) = terminal_discard_run(&mut state);
    let capacity = state.events.max_capacity();
    for _ in 0..capacity {
        state
            .events
            .try_send(Event::HistoryChanged { project })
            .unwrap();
    }
    let operation_id = Uuid::new_v4();
    state
        .command(Command::DiscardIntruder {
            project,
            run_id: finished.id,
            operation_id,
            expected_revision: finished.revision,
            workspace: Box::new(discard_workspace(&state)),
        })
        .await
        .unwrap();
    assert!(
        state
            .store
            .find_intruder_run(finished.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(state.intruder.notifications.len(), 2);
    assert!(!state.intruder.can_accept_commands());
    for _ in 0..capacity {
        assert!(matches!(
            events.try_recv().unwrap(),
            Event::HistoryChanged { .. }
        ));
    }
    let signal = state.intruder.next_signal(&state.events, false).await;
    state.intruder_signal(signal).await.unwrap();
    discard_reply(&mut events, project, finished.id, operation_id).unwrap();
    let signal = state.intruder.next_signal(&state.events, false).await;
    state.intruder_signal(signal).await.unwrap();
    assert!(matches!(
        events.try_recv().unwrap(),
        Event::HistoryChanged { .. }
    ));
    assert!(state.intruder.can_accept_commands());
}
