use super::*;

mod runtime;

const RAW: &[u8] = b"POST http://127.0.0.1:1/archived HTTP/1.1\r\nHost: 127.0.0.1:1\r\nX-A: \xff\r\nX-A:\tsecond\r\nContent-Length: 4\r\n\r\n\0\xff\r\n";

fn fixture() -> (State, mpsc::Receiver<Event>, ExtMessageSummary) {
    fixture_with_raw(RAW, "http://127.0.0.1:1/archived")
}

fn fixture_with_raw(
    raw: &[u8],
    request_target: &str,
) -> (State, mpsc::Receiver<Event>, ExtMessageSummary) {
    let (events, receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let source_id = Uuid::new_v4();
    let item = ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: "root.f2.f1".into(),
        index: 0,
        object_address: Some(72),
        state: "mapped".into(),
        metadata_json: b"{}".to_vec(),
    };
    let message = ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id,
        item_id: item.id,
        collection: item.collection.clone(),
        item_index: item.index,
        role: "current_request".into(),
        field_id: 4,
        raw_offset: 0,
        raw_bytes: raw.len() as u64,
        body_offset: Some(raw.len() as u64 - 4),
        declared_version: Some(HttpVersion::Http11),
        method: Some("POST".into()),
        request_target: Some(request_target.into()),
        status: None,
        diagnostic: None,
    };
    let mut report = ExtImportReport {
        repeater_active_tab: None,
        schema_floor: 228,
        schema_revision: 228,
        source_bytes: raw.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        gaps: vec!["Synthetic archive; connection target is unverified".into()],
        complete_archive: false,
    };
    state.store.begin_source(source_id, &report).unwrap();
    state.store.append_source_chunk(source_id, 0, raw).unwrap();
    state
        .store
        .put_ext_batch(std::slice::from_ref(&item), std::slice::from_ref(&message))
        .unwrap();
    report.indexed_items = 1;
    report.indexed_messages = 1;
    report.collections.push(ExtCollectionReport {
        path: item.collection,
        logical_items: Some(1),
        indexed_items: 1,
        message_count: 1,
        state: "available".into(),
    });
    report.complete_archive = true;
    state.store.finish_source(source_id, &report).unwrap();
    let mut workspace = state.store.workspace().unwrap();
    workspace.open_request(None).unwrap();
    state.store.save_workspace(&workspace).unwrap();
    (state, receiver, message)
}

async fn open(
    state: &mut State,
    receiver: &mut mpsc::Receiver<Event>,
    command: Command,
) -> std::result::Result<WorkTab, String> {
    let expected = match &command {
        Command::OpenExtRepeater {
            project,
            operation_id,
            source_id,
            item_id,
            message_id,
            workspace_revision,
        } => (
            *project,
            *operation_id,
            *source_id,
            *item_id,
            *message_id,
            *workspace_revision,
        ),
        _ => panic!("archive-open command required"),
    };
    tokio::time::timeout(Duration::from_secs(1), state.command(command))
        .await
        .expect("a small archive-open command must finish within the bound")
        .unwrap();
    let Event::ExtRepeaterOpened {
        project,
        operation_id,
        source_id,
        item_id,
        message_id,
        workspace_revision,
        result,
    } = receiver.try_recv().unwrap()
    else {
        panic!("success and rejection must both return the archive-open envelope")
    };
    assert_eq!(
        (
            project,
            operation_id,
            source_id,
            item_id,
            message_id,
            workspace_revision,
        ),
        expected
    );
    assert!(
        receiver.try_recv().is_err(),
        "one command must emit only its correlated result, not workspace or send events"
    );
    result
}

fn unchanged(state: &State, workspace: &Workspace) {
    assert_eq!(state.store.workspace().unwrap(), *workspace);
    assert_eq!(state.store.page(0, 1, "").unwrap().0, 0);
    assert!(state.repeater.sends.is_empty());
    assert!(state.repeater.jobs.is_empty());
    assert!(state.proxy.primary.is_none());
    assert!(state.proxy.pending.is_empty());
    assert!(state.proxy.ca.is_none());
}

#[tokio::test]
async fn archive_open_returns_an_unsent_binary_proposal_without_mutating_the_workspace() {
    let (mut state, mut receiver, message) = fixture();
    let project = state.project;
    let before = state.store.workspace().unwrap();
    let tab = open(
        &mut state,
        &mut receiver,
        Command::OpenExtRepeater {
            project,
            operation_id: Uuid::new_v4(),
            source_id: message.source_id,
            item_id: message.item_id,
            message_id: message.id,
            workspace_revision: before.revision,
        },
    )
    .await
    .unwrap();
    assert!(before.tabs.iter().all(|existing| existing.id != tab.id));
    assert_eq!(tab.title, "外部归档请求");
    assert_eq!(tab.group, None);
    assert_eq!(tab.last_flow, None);
    assert_eq!(tab.draft.revision, 1);
    assert_eq!(tab.draft.version, HttpVersion::Http11);
    assert_eq!(tab.draft.raw, RAW);
    assert!(tab.draft.target.is_empty());
    unchanged(&state, &before);
}

#[tokio::test]
async fn old_generation_wrong_workspace_revision_and_quiescence_reject_in_the_same_envelope() {
    let (mut state, mut receiver, message) = fixture();
    let project = state.project;
    let before = state.store.workspace().unwrap();
    let old = ProjectKey {
        generation: project.generation - 1,
        ..project
    };
    for (project, revision, quiesced, reason) in [
        (old, before.revision, false, "项目已切换"),
        (project, before.revision - 1, false, "revision"),
        (project, before.revision + 1, false, "revision"),
        (project, before.revision, true, "正在保存"),
    ] {
        state.quiesced = quiesced;
        let error = open(
            &mut state,
            &mut receiver,
            Command::OpenExtRepeater {
                project,
                operation_id: Uuid::new_v4(),
                source_id: message.source_id,
                item_id: message.item_id,
                message_id: message.id,
                workspace_revision: revision,
            },
        )
        .await
        .unwrap_err();
        assert!(error.contains(reason), "{error}");
        unchanged(&state, &before);
    }
}

#[tokio::test]
async fn wrong_archive_source_item_and_message_are_correlated_rejections_without_network_work() {
    let (mut state, mut receiver, message) = fixture();
    let project = state.project;
    let before = state.store.workspace().unwrap();
    for (source_id, item_id, message_id) in [
        (Uuid::new_v4(), message.item_id, message.id),
        (message.source_id, Uuid::new_v4(), message.id),
        (message.source_id, message.item_id, Uuid::new_v4()),
    ] {
        let error = open(
            &mut state,
            &mut receiver,
            Command::OpenExtRepeater {
                project,
                operation_id: Uuid::new_v4(),
                source_id,
                item_id,
                message_id,
                workspace_revision: before.revision,
            },
        )
        .await
        .unwrap_err();
        assert!(!error.is_empty());
        unchanged(&state, &before);
    }
}

#[tokio::test]
async fn saved_archive_tab_still_requires_an_explicit_target_before_send_and_never_retries() {
    let (mut state, mut receiver, message) = fixture();
    let project = state.project;
    let mut workspace = state.store.workspace().unwrap();
    let tab = open(
        &mut state,
        &mut receiver,
        Command::OpenExtRepeater {
            project,
            operation_id: Uuid::new_v4(),
            source_id: message.source_id,
            item_id: message.item_id,
            message_id: message.id,
            workspace_revision: workspace.revision,
        },
    )
    .await
    .unwrap();
    workspace.tabs.push(tab.clone());
    workspace.active = Some(tab.id);
    workspace.revision += 1;
    state
        .command(Command::SaveWorkspace {
            project,
            workspace: workspace.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::WorkspaceChanged { project: actual, workspace: saved }
            if actual == project && saved == workspace
    ));
    unchanged(&state, &workspace);
    while let Ok(event) = receiver.try_recv() {
        assert!(
            matches!(event, Event::ProjectDiskUsage { .. }),
            "unexpected event after SaveWorkspace: {event:?}"
        );
    }

    state
        .command(Command::Send {
            run_id: Uuid::new_v4(),
            cancel: tokio_util::sync::CancellationToken::new(),
            project,
            tab_id: tab.id,
            draft: tab.draft.clone(),
            settings: workspace.repeater_settings.clone(),
            group_send: None,
        })
        .await
        .unwrap();
    let Event::SendRejected {
        project: actual,
        tab_id,
        draft_revision,
        message,
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("an archived absolute request URL and Host must not supply the connection target")
    };
    assert_eq!((actual, tab_id, draft_revision), (project, tab.id, 1));
    assert_eq!(message, "连接目标必须为完整 http:// 或 https:// URL");
    unchanged(&state, &workspace);
    assert!(
        receiver.try_recv().is_err(),
        "rejection must not produce a SendStarted event or schedule a retry"
    );
}
