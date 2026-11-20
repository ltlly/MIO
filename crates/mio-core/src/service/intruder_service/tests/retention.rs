use super::*;

fn reply(
    events: &mut mpsc::Receiver<Event>,
    project: ProjectKey,
    run_id: Uuid,
    operation_id: Uuid,
) -> std::result::Result<IntruderRetentionReceipt, String> {
    while let Ok(event) = events.try_recv() {
        match event {
            Event::IntruderRetentionChanged {
                project: owner,
                run_id: run,
                operation_id: operation,
                result,
            } => {
                assert_eq!((owner, run, operation), (project, run_id, operation_id));
                return result;
            }
            Event::Error { .. } => {
                panic!("retention errors must retain their operation identity")
            }
            _ => {}
        }
    }
    panic!("retention must return a correlated receipt")
}

async fn change(
    state: &mut State,
    events: &mut mpsc::Receiver<Event>,
    run: &IntruderRunSummary,
    retention: IntruderRetention,
) -> std::result::Result<IntruderRetentionReceipt, String> {
    let project = state.project;
    let operation_id = Uuid::new_v4();
    state
        .command(Command::SetIntruderRetention {
            project,
            run_id: run.id,
            operation_id,
            expected_revision: run.revision,
            retention,
            workspace: Box::new(discard_workspace(state)),
        })
        .await
        .unwrap();
    reply(events, project, run.id, operation_id)
}

#[tokio::test]
async fn terminal_retention_correlates_rejections_and_does_not_cancel_another_run() {
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
        (
            ProjectKey {
                id: Uuid::new_v4(),
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
            .command(Command::SetIntruderRetention {
                project: owner,
                run_id: id,
                operation_id,
                expected_revision: revision,
                retention: IntruderRetention::Session,
                workspace: Box::new(discard_workspace(&state)),
            })
            .await
            .unwrap();
        assert!(reply(&mut events, owner, id, operation_id).is_err());
        assert_eq!(state.store.read(flow.summary.id).unwrap(), flow);
        assert!(!cancel.is_cancelled());
    }
    let session = change(
        &mut state,
        &mut events,
        &finished,
        IntruderRetention::Session,
    )
    .await
    .unwrap();
    assert!(
        !state
            .intruder_result_page(finished.id, None, 1)
            .unwrap()
            .persisted
    );
    assert_eq!(
        state.intruder.runs[&active].summary.state,
        IntruderRunState::Paused
    );
    assert!(!cancel.is_cancelled());
    assert!(!state.intruder.runs.contains_key(&finished.id));
    assert_eq!(state.unsaved_count(), 0);
    let promoted = change(
        &mut state,
        &mut events,
        &session.run,
        IntruderRetention::Project,
    )
    .await
    .unwrap();
    assert!(
        state
            .intruder_result_page(finished.id, None, 1)
            .unwrap()
            .persisted
    );
    assert_eq!(promoted.run.revision, finished.revision + 2);
    assert_eq!(state.store.read(flow.summary.id).unwrap(), flow);
    state.stop_intruder().await.unwrap();
    assert!(!state.intruder.has_resources());
}

#[tokio::test]
async fn terminal_retention_refuses_unjoined_or_unregistered_handles() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (mut state, _events) = state();
        let (finished, flow) = terminal_discard_run(&mut state);
        let project = state.project;
        let record = flow.clone();
        let (release, released) = oneshot::channel();
        let job = state.intruder.jobs.spawn(async move {
            released.await.unwrap();
            Completion {
                project,
                record,
                reason_phrase: None,
            }
        });
        state.intruder.requests.insert(
            job.id(),
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
                .set_intruder_retention(
                    project,
                    finished.id,
                    finished.revision,
                    IntruderRetention::Session,
                    proposal.clone()
                )
                .is_err()
        );
        state.intruder.requests.remove(&job.id());
        assert!(
            state
                .set_intruder_retention(
                    project,
                    finished.id,
                    finished.revision,
                    IntruderRetention::Session,
                    proposal.clone()
                )
                .is_err()
        );
        assert!(!job.is_finished());
        assert_eq!(state.store.read(flow.summary.id).unwrap(), flow);
        release.send(()).unwrap();
        state
            .intruder
            .jobs
            .join_next_with_id()
            .await
            .unwrap()
            .unwrap();
        state
            .set_intruder_retention(
                project,
                finished.id,
                finished.revision,
                IntruderRetention::Session,
                proposal,
            )
            .unwrap();
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
    })
    .await
    .expect("retention does not wait for or cancel senders");
}

#[tokio::test]
async fn terminal_retention_requires_explicit_recovery_repair_and_rejects_late_flow_writes() {
    let (mut state, mut events) = state();
    let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
    state.store.reject_writes(true);
    assert!(state.advance_intruder(id).await.is_err());
    assert!(state.stop_intruder().await.is_err());
    let page = state.intruder_result_page(id, None, 1).unwrap();
    assert_eq!(page.run.retention, IntruderRetention::Project);
    assert!(!page.persisted);
    let pending = state.intruder.recovery.len();
    let failure = state.storage_error.clone();
    assert!(
        change(
            &mut state,
            &mut events,
            &page.run,
            IntruderRetention::Session
        )
        .await
        .unwrap_err()
        .contains("尚未写盘")
    );
    assert_eq!(state.intruder.recovery.len(), pending);
    assert_eq!(state.storage_error, failure);
    assert_eq!(state.intruder_result_page(id, None, 1).unwrap(), page);
    let directory = tempfile::tempdir().unwrap();
    save(&mut state, Some(directory.path().join("repaired.mio")))
        .await
        .unwrap();
    while events.try_recv().is_ok() {}
    assert_eq!(state.unsaved_count(), 0);
    let saved = state.intruder_result_page(id, None, 1).unwrap();
    assert!(saved.persisted);
    let session = change(
        &mut state,
        &mut events,
        &saved.run,
        IntruderRetention::Session,
    )
    .await
    .unwrap();
    let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
        panic!("unsent flow stays inspectable")
    };
    let flow = state.read_flow(summary.id).unwrap();
    assert!(state.persist(&flow).is_err());
    assert_eq!(
        state.unsaved_count(),
        0,
        "late Session writes must not poison recovery"
    );
    assert!(state.storage_error.is_none());
    assert_eq!(state.read_flow(summary.id).unwrap(), flow);
    assert_eq!(state.intruder_run(id).unwrap(), session.run);
    assert!(state.store.put_intruder_result(id, 0, Ok(&flow)).is_err());
    assert!(!state.intruder.has_resources());
}

#[tokio::test]
async fn terminal_session_blocks_new_open_until_resolved_but_save_as_preserves_ownership() {
    let (mut state, mut events) = state();
    let (finished, flow) = terminal_discard_run(&mut state);
    let owner = state.project;
    let session = change(
        &mut state,
        &mut events,
        &finished,
        IntruderRetention::Session,
    )
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("must-not-be-created.mio");
    assert!(
        state
            .command(Command::NewProject {
                path: Some(blocked.clone())
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("Session")
    );
    assert!(!blocked.exists());
    let other = directory.path().join("other.mio");
    ProjectStore::create(Some(&other)).unwrap().close().unwrap();
    assert!(
        state
            .command(Command::OpenProject {
                path: other.clone()
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("Session")
    );
    assert_eq!(state.project, owner);
    let saved = directory.path().join("save-as.mio");
    save(&mut state, Some(saved.clone())).await.unwrap();
    assert_eq!(state.project, owner);
    assert_eq!(state.intruder_run(finished.id).unwrap(), session.run);
    assert_eq!(state.read_flow(flow.summary.id).unwrap(), flow);
    assert!(!state.intruder.has_resources());
    let disk = ProjectStore::open(&saved).unwrap();
    assert!(disk.find_intruder_run(finished.id).unwrap().is_none());
    assert!(disk.read_optional(flow.summary.id).unwrap().is_none());
    disk.close().unwrap();
    while events.try_recv().is_ok() {}
    state
        .discard_intruder(
            owner,
            finished.id,
            session.run.revision,
            discard_workspace(&state),
        )
        .unwrap();
    state
        .command(Command::OpenProject { path: other })
        .await
        .unwrap();
    assert_ne!(state.project, owner);
    assert_eq!(state.project.generation, owner.generation + 1);
    assert!(
        state
            .set_intruder_retention(
                owner,
                finished.id,
                session.run.revision,
                IntruderRetention::Project,
                discard_workspace(&state)
            )
            .is_err()
    );
}

#[tokio::test]
async fn terminal_retention_commits_before_backpressured_receipt_and_duplicate_never_rewrites() {
    let (mut state, mut events) = state();
    let (finished, _) = terminal_discard_run(&mut state);
    let project = state.project;
    let capacity = state.events.max_capacity();
    for _ in 0..capacity {
        state
            .events
            .try_send(Event::HistoryChanged { project })
            .unwrap();
    }
    let operation_id = Uuid::new_v4();
    let proposal = discard_workspace(&state);
    state
        .command(Command::SetIntruderRetention {
            project,
            run_id: finished.id,
            operation_id,
            expected_revision: finished.revision,
            retention: IntruderRetention::Session,
            workspace: Box::new(proposal.clone()),
        })
        .await
        .unwrap();
    assert!(
        !state
            .store
            .intruder_results(finished.id, None, 1)
            .unwrap()
            .persisted
    );
    assert!(!state.intruder.has_resources());
    assert_eq!(state.intruder.notifications.len(), 2);
    for _ in 0..capacity {
        events.try_recv().unwrap();
    }
    let signal = state.intruder.next_signal(&state.events, false).await;
    state.intruder_signal(signal).await.unwrap();
    let session = reply(&mut events, project, finished.id, operation_id).unwrap();
    let signal = state.intruder.next_signal(&state.events, false).await;
    state.intruder_signal(signal).await.unwrap();
    events.try_recv().unwrap();
    let mut newer = discard_workspace(&state);
    newer.intruder.tabs[0].title = "newer title".into();
    state.store.save_workspace(&newer).unwrap();
    state
        .command(Command::SetIntruderRetention {
            project,
            run_id: finished.id,
            operation_id,
            expected_revision: finished.revision,
            retention: IntruderRetention::Session,
            workspace: Box::new(proposal),
        })
        .await
        .unwrap();
    assert!(reply(&mut events, project, finished.id, operation_id).is_err());
    assert_eq!(state.store.workspace().unwrap(), newer);
    assert_eq!(state.intruder_run(finished.id).unwrap(), session.run);
}
