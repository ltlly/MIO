use super::*;

fn saved_tab(state: &mut State, target: String) -> WorkTab {
    let mut workspace = state.store.workspace().unwrap();
    let mut tab = WorkTab::from_request(None, "history".into());
    tab.draft.target = target;
    tab.draft.raw = b"GET /original HTTP/1.1\r\nHost: header.invalid\r\n\r\n".to_vec();
    workspace.active = Some(tab.id);
    workspace.tabs.push(tab.clone());
    workspace.revision += 1;
    state.store.save_workspace(&workspace).unwrap();
    tab
}

async fn page(
    state: &mut State,
    receiver: &mut mpsc::Receiver<Event>,
    tab: TabId,
) -> RepeaterHistoryPage {
    while receiver.try_recv().is_ok() {}
    state
        .command(Command::QueryRepeaterHistory {
            project: state.project,
            query_revision: 9,
            tab_id: tab,
            start: 0,
            limit: 64,
        })
        .await
        .unwrap();
    let Event::RepeaterHistoryPage {
        project,
        query_revision: 9,
        tab_id,
        result: Ok(page),
    } = receiver.try_recv().unwrap()
    else {
        panic!("history returns a correlated page")
    };
    assert_eq!(project, state.project);
    assert_eq!(tab_id, tab);
    page
}

#[tokio::test]
async fn rejected_send_recovery_never_registers_history_or_opens_a_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let tab = saved_tab(
        &mut state,
        format!("http://{}", listener.local_addr().unwrap()),
    );
    let run_id = Uuid::new_v4();
    state.store.reject_writes(true);
    state
        .command(Command::Send {
            project: state.project,
            tab_id: tab.id,
            run_id,
            draft: tab.draft.clone(),
            settings: RepeaterSettings::default(),
            group_send: None,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let mut rejected = false;
    while let Ok(event) = receiver.try_recv() {
        if let Event::SendRejected { run_id: actual, .. } = event {
            assert_eq!(actual, run_id);
            rejected = true;
        }
        assert!(!matches!(event, Event::SendStarted { .. }));
    }
    assert!(rejected);
    assert!(state.sends.is_empty() && state.jobs.is_empty());
    assert_eq!(
        state.recovery.len(),
        1,
        "retain the original write-failure recovery record"
    );
    assert_eq!(state.recovery[0].record.summary.id, run_id);
    assert_eq!(page(&mut state, &mut receiver, tab.id).await.total, 0);
    state.store.reject_writes(false);
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    state
        .command(Command::SaveProject {
            project: state.project,
            path: Some(directory.path().join("recovered.mio")),
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(state.unsaved_count(), 0);
    assert_eq!(
        state.store.read(run_id).unwrap().summary.state,
        FlowState::Cancelled
    );
    assert_eq!(page(&mut state, &mut receiver, tab.id).await.total, 0);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn registered_history_uses_recovery_summary_and_selects_original_draft_without_send() {
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let tab = saved_tab(&mut state, "http://127.0.0.1:1".into());
    let mut finished = tests::completed_flow();
    finished.request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version).unwrap();
    finished.summary.url = finished.request.url.clone();
    finished.summary.method = finished.request.method.clone();
    finished.summary.request_bytes = finished.request.body.len() as u64;
    let mut pending = finished.clone();
    pending.summary.revision = 1;
    pending.summary.state = FlowState::Pending;
    pending.summary.status = None;
    pending.summary.response_bytes = 0;
    pending.summary.elapsed_ms = 0;
    pending.response = None;
    state
        .store
        .begin_repeater_send(tab.id, &tab.draft, &pending, &CancellationToken::new())
        .unwrap();
    state.recovery.push_back(finished.clone().into());
    let history = page(&mut state, &mut receiver, tab.id).await;
    assert_eq!(history.total, 1);
    assert_eq!(
        history.rows[0].id,
        RepeaterHistoryEntryId::Live(finished.summary.id)
    );
    assert_eq!(history.rows[0].status, Some(200));
    assert_eq!(history.rows[0].diagnostic, None);
    assert_eq!(
        state.store.read(finished.summary.id).unwrap().summary.state,
        FlowState::Pending
    );
    let workspace = state.store.workspace().unwrap();
    let operation_id = Uuid::new_v4();
    state
        .command(Command::SelectRepeaterHistory {
            project: state.project,
            operation_id,
            tab_id: tab.id,
            entry: history.rows[0].id,
            workspace_revision: workspace.revision,
            draft_revision: tab.draft.revision,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let Event::RepeaterHistorySelected {
        operation_id: actual,
        result: Ok(proposal),
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("select returns a proposal");
    };
    assert_eq!(actual, operation_id);
    assert_eq!(proposal.id, tab.id);
    assert_eq!(proposal.draft.raw, tab.draft.raw);
    assert_eq!(proposal.draft.revision, tab.draft.revision + 1);
    assert_eq!(proposal.last_flow, Some(finished.summary.id));
    assert_eq!(
        state.store.workspace().unwrap(),
        workspace,
        "reading never mutates the saved workspace"
    );
    assert!(state.jobs.is_empty() && state.sends.is_empty());
    state
        .command(Command::ReadFlow {
            project: state.project,
            selection_revision: 10,
            flow_id: finished.summary.id,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::FlowDetail { record, .. } if *record == finished)
    );
}

#[tokio::test]
async fn history_stale_project_and_cancel_return_correlated_errors() {
    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let tab = saved_tab(&mut state, "http://127.0.0.1:1".into());
    let stale = ProjectKey {
        id: Uuid::new_v4(),
        generation: state.project.generation,
    };
    state
        .command(Command::QueryRepeaterHistory {
            project: stale,
            query_revision: 4,
            tab_id: tab.id,
            start: 0,
            limit: 64,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::RepeaterHistoryPage { project, query_revision: 4, result: Err(_), .. } if project == stale)
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let operation_id = Uuid::new_v4();
    state
        .command(Command::SelectRepeaterHistory {
            project: state.project,
            operation_id,
            tab_id: tab.id,
            entry: RepeaterHistoryEntryId::Archive(0),
            workspace_revision: state.store.workspace().unwrap().revision,
            draft_revision: tab.draft.revision,
            cancel,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::RepeaterHistorySelected { operation_id: actual, result: Err(_), .. } if actual == operation_id)
    );
    assert!(state.jobs.is_empty() && state.sends.is_empty());
}
