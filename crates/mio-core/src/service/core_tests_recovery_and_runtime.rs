use super::{tests::completed_flow, *};

#[tokio::test]
async fn nil_user_preferences_set_is_rejected_before_any_profile_write() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let parent = directory.path().canonicalize().unwrap().join("profile");
    let path = parent.join("preferences.json");
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::with_preferences(
        events,
        CancellationToken::new(),
        crate::preferences::PreferencesStore::new(Some(path.clone())),
    )
    .unwrap();
    state
        .command(Command::SetUserPreferences {
            operation_id: Uuid::nil(),
            expected_revision: 0,
            running: RunningAttackClose::Delete,
            finished: FinishedAttackClose::Delete,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::UserPreferencesSaved {
        operation_id, result: Err(_),
    } if operation_id.is_nil()),
        "a profile mutation requires a non-nil operation identity"
    );
    assert!(
        !parent.exists(),
        "reject nil before creating the profile, lock, or preferences file"
    );
    assert_eq!(
        state.preferences.query().preferences,
        UserPreferences::default()
    );
    assert!(receiver.try_recv().is_err());
    state
        .command(Command::SetUserTlsCertificates {
            operation_id: Uuid::nil(),
            expected_revision: 0,
            certificates: Vec::new(),
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::UserPreferencesSaved {
        operation_id, result: Err(_),
    } if operation_id.is_nil())
    );
    assert!(!parent.exists());
}

#[tokio::test]
async fn user_preferences_commands_are_global_correlated_and_survive_project_replacement() {
    let (events, mut receiver) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let before = state.store.workspace().unwrap();
    let operation_id = Uuid::new_v4();
    state
        .command(Command::SetUserPreferences {
            operation_id,
            expected_revision: 0,
            running: RunningAttackClose::Continue,
            finished: FinishedAttackClose::KeepMemory,
        })
        .await
        .unwrap();
    let Event::UserPreferencesSaved {
        operation_id: actual,
        result: Ok(saved),
    } = receiver.try_recv().unwrap()
    else {
        panic!("global preferences need a correlated save acknowledgement")
    };
    assert_eq!(actual, operation_id);
    assert_eq!(saved.revision, 1);
    assert_eq!(
        state.store.workspace().unwrap(),
        before,
        "user preferences do not mutate project workspace"
    );
    state
        .replace_project(ProjectStore::create(None).unwrap())
        .await
        .unwrap();
    while receiver.try_recv().is_ok() {}
    let query = Uuid::new_v4();
    state
        .command(Command::QueryUserPreferences {
            operation_id: query,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::UserPreferences { operation_id, state } if operation_id == query && state.preferences == saved && state.load_error.is_none())
    );
    let conflict = Uuid::new_v4();
    state
        .command(Command::SetUserPreferences {
            operation_id: conflict,
            expected_revision: 0,
            running: RunningAttackClose::Delete,
            finished: FinishedAttackClose::Delete,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::UserPreferencesSaved { operation_id, result: Err(_) } if operation_id == conflict)
    );
    assert_eq!(state.preferences.query().preferences, saved);
}

#[tokio::test]
async fn broken_user_preferences_report_load_error_without_overwriting_source() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let path = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("preferences.json");
    std::fs::write(&path, b"broken user preferences").unwrap();
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::with_preferences(
        events,
        CancellationToken::new(),
        crate::preferences::PreferencesStore::new(Some(path.clone())),
    )
    .unwrap();
    let operation_id = Uuid::new_v4();
    state
        .command(Command::QueryUserPreferences { operation_id })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::UserPreferences { operation_id: actual, state } if actual == operation_id && state.load_error.is_some() && state.preferences == UserPreferences::default())
    );
    state
        .command(Command::SetUserPreferences {
            operation_id,
            expected_revision: 0,
            running: RunningAttackClose::Continue,
            finished: FinishedAttackClose::SaveProject,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::UserPreferencesSaved { result: Err(_), .. }
    ));
    assert_eq!(std::fs::read(path).unwrap(), b"broken user preferences");
}

#[tokio::test]
async fn regression_storage_failure_still_completes_send_and_save_recovers_response() {
    let (events, mut receiver) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let tab_id = Uuid::new_v4();
    let record = completed_flow();
    let mut pending = record.clone();
    pending.summary.revision = 1;
    pending.summary.state = FlowState::Pending;
    pending.summary.status = None;
    pending.response = None;
    state.store.put(&pending).unwrap();
    let task = tokio::spawn(async {});
    let task_id = task.id();
    task.await.unwrap();
    state.repeater.sends.insert(
        tab_id,
        ActiveSend {
            cancel: CancellationToken::new(),
            task_id,
            flow_id: record.summary.id,
            draft_revision: 7,
            progress_rx: None,
        },
    );
    state.store.reject_writes(true);
    let result = state
        .complete(Completion {
            project: state.project,
            tab_id,
            draft_revision: 7,
            received: None,
            record: record.clone(),
        })
        .await;
    assert!(result.is_err(), "storage failure must be visible");
    assert!(!state.repeater.sends.contains_key(&tab_id));
    let mut finished = false;
    while let Ok(event) = receiver.try_recv() {
        if let Event::SendFinished {
            tab_id: actual,
            record: actual_record,
            persisted,
            ..
        } = event
        {
            assert_eq!(actual, tab_id);
            assert_eq!(*actual_record, record);
            assert!(!persisted);
            finished = true;
        }
    }
    assert!(
        finished,
        "database failure cannot leave a sent tab permanently busy"
    );
    let directory = tempfile::tempdir().unwrap();
    // Save As must recover even when the source connection remains read-only.
    state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(directory.path().join("recovered.mio")),
            workspace: None,
        })
        .await
        .unwrap();
    assert_eq!(
        state.store.read(record.summary.id).unwrap(),
        record,
        "save retries persistence, not the HTTP request"
    );
}

#[tokio::test]
async fn regression_recovery_keeps_original_messages_visible_before_save_as() {
    for persisted_revisions in 0..=2 {
        let (events, mut receiver) = mpsc::channel(128);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let original = completed_flow();
        if persisted_revisions > 0 {
            state.persist(&original).unwrap();
        }
        let mut changed = original.clone();
        changed.summary.revision += 1;
        changed.request.body = b"first edit".to_vec();
        changed.response.as_mut().unwrap().body = b"first response edit".to_vec();
        if persisted_revisions > 1 {
            state.persist(&changed).unwrap();
        }
        state.store.reject_writes(true);
        if persisted_revisions == 0 {
            assert!(state.persist(&original).is_err());
        }
        if persisted_revisions < 2 {
            assert!(state.persist(&changed).is_err());
        }
        changed.summary.revision += 1;
        changed.request.body = b"latest edit".to_vec();
        changed.response = None;
        assert!(state.persist(&changed).is_err());

        state
            .command(Command::ReadFlow {
                project: state.project,
                selection_revision: 7,
                flow_id: original.summary.id,
            })
            .await
            .unwrap();
        let Event::FlowDetail {
            record,
            original: before,
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("missing flow detail");
        };
        assert_eq!(*record, changed);
        assert_eq!(
            before.request.as_ref(),
            Some(&original.request),
            "the first pre-edit request remains visible while writes are unavailable"
        );
        assert_eq!(before.response.as_ref(), original.response.as_ref());
        state
            .command(Command::OpenRepeater {
                project: state.project,
                flow_id: original.summary.id,
                original: true,
            })
            .await
            .unwrap();
        let Event::RepeaterOpened { tab, .. } = receiver.try_recv().unwrap() else {
            panic!("missing repeater draft");
        };
        let request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version).unwrap();
        assert_eq!(
            request.body, original.request.body,
            "open original cannot silently use the latest recovery revision"
        );

        let directory = tempfile::tempdir().unwrap();
        state
            .command(Command::SaveProject {
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project: state.project,
                path: Some(directory.path().join("recovered.mio")),
                workspace: None,
            })
            .await
            .unwrap();
        assert_eq!(
            state.store.original(original.summary.id).unwrap(),
            *before,
            "the recovery view and persisted view must agree"
        );
    }
}

#[tokio::test]
async fn regression_shutdown_is_bounded_with_a_saturated_event_queue() {
    use std::future::Future;
    let (events, _receiver) = mpsc::channel(1);
    let shutdown = CancellationToken::new();
    let state = State::new(events, shutdown.clone()).unwrap();
    let (commands, receive_commands) = mpsc::channel(1);
    commands
        .send(Command::QueryHistory {
            project: state.project,
            query_revision: 1,
            offset: 0,
            limit: 1,
            filter: String::new(),
        })
        .await
        .unwrap();
    let run = state.run(receive_commands);
    tokio::pin!(run);
    std::future::poll_fn(|cx| {
        assert!(
            run.as_mut().poll(cx).is_pending(),
            "event queue is saturated before requesting shutdown"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(commands.capacity(), 0);
    shutdown.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), run)
            .await
            .unwrap()
            .is_ok(),
        "shutdown must not wait for an unconsumed UI event queue"
    );
}

#[tokio::test]
async fn regression_repeater_open_has_only_one_workspace_writer() {
    let (events, _receiver) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let before = state.store.workspace().unwrap();
    state
        .command(Command::NewRepeater {
            project: state.project,
        })
        .await
        .unwrap();
    assert_eq!(
        state.store.workspace().unwrap(),
        before,
        "UI merges new tabs before the single workspace CAS writer persists them"
    );
}

#[tokio::test]
async fn regression_workspace_rejection_supplies_the_last_persisted_revision() {
    let (events, mut receiver) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let before = state.store.workspace().unwrap();
    let mut next = before.clone();
    next.open_request(None).unwrap();
    next.repeater_settings.unpack_compressed_responses = false;
    state.store.reject_writes(true);
    let _ = state
        .command(Command::SaveWorkspace {
            project: state.project,
            workspace: next.clone(),
        })
        .await;
    let rejected = receiver.try_recv();
    assert!(
        matches!(rejected, Ok(Event::WorkspaceRejected { revision, stored, .. }) if revision == next.revision && stored == before),
        "a failed write must allow retrying the newest draft with the last persisted revision"
    );
    state.store.reject_writes(false);
    state
        .command(Command::SaveWorkspace {
            project: state.project,
            workspace: next.clone(),
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv(), Ok(Event::WorkspaceChanged { workspace, .. }) if workspace == next)
    );
}
