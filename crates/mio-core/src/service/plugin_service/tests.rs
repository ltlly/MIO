use super::workspace::validate_workspace_edit;
use super::*;
use mio_plugin_api::panel::{Editor, EditorFormat, Node, node};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, sync::Arc};

#[test]
fn plugin_new_repeater_rejects_an_injected_archive_response_reference() {
    let request = parse_request(
        b"GET /plugin HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n",
        "http://fixture.invalid",
        HttpVersion::Http11,
    )
    .unwrap();
    let stored = Workspace::default();
    let tab = WorkTab::from_request(Some(&request), "plugin request".into());
    let operation = ServiceOperation::OpenRepeater { request };
    let mut proposal = Workspace {
        revision: 1,
        active: Some(tab.id),
        tabs: vec![tab.clone()],
        ..Workspace::default()
    };
    validate_workspace_edit(&stored, &proposal, tab.id, &operation).unwrap();
    proposal.tabs[0].archive_response = Some(ExtResponseRef {
        source_id: Uuid::new_v4(),
        item_id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        draft_revision: 1,
    });
    let error = validate_workspace_edit(&stored, &proposal, tab.id, &operation).unwrap_err();
    assert!(error.to_string().contains("归档响应"), "{error}");
    assert!(stored.tabs.is_empty());
    assert_eq!(proposal.tabs[0].draft, tab.draft);
    assert_eq!(proposal.tabs[0].last_flow, None);
    proposal.tabs[0].archive_response = None;
    proposal.tabs[0].history = Some(RepeaterHistory {
        source: Some(RepeaterHistorySource {
            source_id: Uuid::new_v4(),
            parent_item_id: Uuid::new_v4(),
        }),
        cursor: None,
    });
    assert!(
        validate_workspace_edit(&stored, &proposal, tab.id, &operation).is_err(),
        "plugin new-tab proposals cannot inject archived history"
    );
}

struct Fixture {
    state: State,
    events: mpsc::Receiver<Event>,
    queue: mpsc::Sender<PluginEvent>,
    services: mpsc::Sender<ServiceRequest>,
    identity: ChannelIdentity,
    tab_id: TabId,
    initial: Arc<PanelSnapshot>,
    latest: Arc<PanelSnapshot>,
    _directory: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let entry = directory.path().join("fixture-worker");
    fs::write(&entry, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(directory.path().join("manifest.json"), br#"{"id":"lab.queue-test","version":"1","api_version":1,"engine":"native","entry":"fixture-worker","permissions":["traffic.read"],"contributions":["suite_panel","message_editor"]}"#).unwrap();
    let package = VerifiedPackage::read(directory.path()).unwrap();
    let grants = vec![Permission::TrafficRead];
    let trust = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events, receiver) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut workspace = state.store.workspace().unwrap();
    let mut tab = WorkTab::from_request(None, "queue test".into());
    tab.draft.target = "http://127.0.0.1/".into();
    let tab_id = tab.id;
    workspace.tabs.push(tab);
    workspace.active = Some(tab_id);
    workspace.revision += 1;
    state.store.save_workspace(&workspace).unwrap();
    let initial = Arc::new(PanelSnapshot {
        revision: 1,
        title: "queue test".into(),
        root: Some(Node::new(
            1,
            "request",
            node::Kind::Editor(Editor {
                document_id: 1,
                revision: 1,
                contents: vec![],
                format: EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    });
    let mut latest = initial.as_ref().clone();
    latest.revision += 1;
    if let Some(node::Kind::Editor(editor)) = latest.root.as_mut().unwrap().kind.as_mut() {
        editor.revision += 1;
        editor.contents = "已打开的最新草稿".as_bytes().to_vec();
    }
    let latest = Arc::new(latest);
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };
    let (worker, queue) =
        crate::plugins::test_supervisor(identity, package.manifest.clone(), initial.clone());
    let (services, service_receiver) = mpsc::channel(16);
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: Some(tab_id),
    };
    let manifest_modified = package.manifest_modified;
    let manifest_size = package.manifest_bytes.len() as u64;
    let entry_modified = package.modified;
    let entry_size = package.executable_size;
    state.plugins.instances.insert(
        identity.instance_id,
        Instance {
            worker,
            services: service_receiver,
            package,
            manifest_modified,
            manifest_size,
            entry_modified,
            entry_size,
            auto_reload: true,
            trust,
            info,
            panel: initial.clone(),
            source: None,
            resources: HashMap::from([(tab_id, 1)]),
        },
    );
    Fixture {
        state,
        events: receiver,
        queue,
        services,
        identity,
        tab_id,
        initial,
        latest,
        _directory: directory,
    }
}

#[tokio::test]
async fn opened_completion_cannot_publish_the_handshake_panel_ahead_of_its_update() {
    let mut f = fixture();
    f.queue
        .send(PluginEvent::Panel {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            snapshot: f.latest.clone(),
            editor_bases: vec![],
            synced_ui_seq: 0,
            rejected_ui_seqs: vec![],
        })
        .await
        .unwrap();
    let operation_id = Uuid::new_v4();
    let identity = f.identity;
    let tab_id = f.tab_id;
    let panel = f.latest.clone();
    f.state.plugin_job_start(
        operation_id,
        Some(identity.instance_id),
        CancellationToken::new(),
        true,
        async move {
            Ok(Output::Opened {
                identity,
                tab_id,
                revision: 1,
                loading: true,
                panel,
            })
        },
    );
    // Both queues are ready, but the completion is consumed first, as next_signal permits.
    let completion = f.state.plugins.jobs.join_next().await.unwrap();
    f.state.plugin_job(completion).await;
    loop {
        let event = f
            .events
            .try_recv()
            .expect("completion must be published without another worker frame");
        if let Event::PluginOperation {
            operation_id: actual,
            result,
            ..
        } = event
        {
            assert_eq!(actual, operation_id);
            let PluginOutcome::Loaded { panel, .. } = result.unwrap() else {
                panic!("loaded editor")
            };
            assert_eq!(
                panel, f.latest,
                "the Open ACK must carry its accepted panel, independent of event queue order"
            );
            break;
        }
    }
}

#[tokio::test]
async fn queued_older_panels_cannot_regress_content_or_discard_diagnostics() {
    let mut f = fixture();
    let editor_bases = vec![mio_plugin_api::panel::EditorSnapshot {
        document_id: 1,
        revision: 1,
        ..Default::default()
    }];
    f.state
        .plugin_signal(Signal::Worker(PluginEvent::Panel {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            snapshot: f.latest.clone(),
            editor_bases: editor_bases.clone(),
            synced_ui_seq: 3,
            rejected_ui_seqs: vec![5],
        }))
        .await;
    assert!(matches!(
        f.events.try_recv().unwrap(),
        Event::PluginPanel {
            editor_bases: bases,
            synced_ui_seq: Some(3),
            rejected_ui_seqs,
            ..
        } if bases == editor_bases && rejected_ui_seqs == [5]
    ));
    f.state
        .plugin_signal(Signal::Worker(PluginEvent::Panel {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            snapshot: f.initial,
            editor_bases: vec![],
            synced_ui_seq: 0,
            rejected_ui_seqs: vec![],
        }))
        .await;
    f.state
        .plugin_signal(Signal::Worker(PluginEvent::Error {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            failure: Failure::new(ErrorCode::Invalid, "causal diagnostic"),
        }))
        .await;
    assert_eq!(
        f.state.plugins.instances[&f.identity.instance_id].panel,
        f.latest
    );
    assert!(
        matches!(f.events.try_recv().unwrap(), Event::PluginDiagnostic { message, .. } if message == "causal diagnostic")
    );
    assert!(f.events.try_recv().is_err());
    assert!(!f.services.is_closed());
}

#[tokio::test]
async fn a_failed_open_releases_its_optimistic_resource_binding() {
    let mut f = fixture();
    f.state
        .plugins
        .instances
        .get_mut(&f.identity.instance_id)
        .unwrap()
        .resources
        .clear();
    f.state
        .plugin_command(
            f.identity.project,
            Uuid::new_v4(),
            PluginAction::OpenRequest {
                instance_id: f.identity.instance_id,
                tab_id: f.tab_id,
                draft_revision: 1,
            },
        )
        .await
        .unwrap();
    // This fixture's control receiver is closed, so the Open deterministically fails.
    let completed = f.state.plugins.jobs.join_next().await.unwrap();
    f.state.plugin_job(completed).await;
    assert!(
        !f.state.plugins.instances[&f.identity.instance_id]
            .resources
            .contains_key(&f.tab_id),
        "failed opens must not retain host-service authority or consume the handle budget"
    );
}

#[tokio::test]
async fn the_manager_reserves_no_more_than_the_worker_resource_budget() {
    let mut f = fixture();
    f.state
        .plugins
        .instances
        .get_mut(&f.identity.instance_id)
        .unwrap()
        .resources = (0..64).map(|_| (Uuid::new_v4(), 1)).collect();
    let result = f
        .state
        .plugin_command(
            f.identity.project,
            Uuid::new_v4(),
            PluginAction::OpenRequest {
                instance_id: f.identity.instance_id,
                tab_id: f.tab_id,
                draft_revision: 1,
            },
        )
        .await;
    assert!(
        result.is_err(),
        "the manager must reject a 65th resource before dispatching an Open"
    );
    assert_eq!(
        f.state.plugins.instances[&f.identity.instance_id]
            .resources
            .len(),
        64
    );
    assert!(f.state.plugins.jobs.is_empty());
}

#[tokio::test]
async fn closing_many_tabs_schedules_one_revoke_job_per_instance() {
    let mut f = fixture();
    let instance = f
        .state
        .plugins
        .instances
        .get_mut(&f.identity.instance_id)
        .unwrap();
    instance.info.tab_id = None;
    instance.resources = (0..64).map(|_| (Uuid::new_v4(), 1)).collect();
    f.state
        .plugin_workspace_changed(&Workspace::default())
        .await;
    assert!(
        f.state.plugins.instances[&f.identity.instance_id]
            .resources
            .is_empty()
    );
    assert_eq!(
        f.state.plugins.jobs.len(),
        1,
        "bulk tab closure must not allocate one background job per resource"
    );
}

#[tokio::test]
async fn plugin_panel_synced_updates_same_revision_and_ignores_older() {
    let mut f = fixture();
    let mut updated = f.latest.as_ref().clone();
    if let Some(node::Kind::Editor(editor)) = updated.root.as_mut().unwrap().kind.as_mut() {
        editor.contents = b"synced-same-rev".to_vec();
    }
    let updated = Arc::new(updated);

    f.state
        .plugin_signal(Signal::Worker(PluginEvent::Panel {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            snapshot: f.latest.clone(),
            editor_bases: vec![],
            synced_ui_seq: 1,
            rejected_ui_seqs: vec![],
        }))
        .await;
    assert!(matches!(
        f.events.try_recv().unwrap(),
        Event::PluginPanel {
            snapshot,
            synced_ui_seq: Some(1),
            rejected_ui_seqs,
            ..
        } if snapshot == f.latest && rejected_ui_seqs.is_empty()
    ));

    // Older revision PanelSynced must be ignored.
    f.state
        .plugin_signal(Signal::Worker(PluginEvent::PanelSynced {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            snapshot: f.initial.clone(),
            synced_ui_seq: 1,
            rejected_ui_seqs: vec![],
        }))
        .await;
    assert!(f.events.try_recv().is_err());
    assert_eq!(
        f.state.plugins.instances[&f.identity.instance_id].panel,
        f.latest
    );

    // Same revision PanelSynced must update cached panel and notify UI with synced_ui_seq & rejected_ui_seqs.
    f.state
        .plugin_signal(Signal::Worker(PluginEvent::PanelSynced {
            identity: f.identity,
            panel_id: protocol::PANEL_ID,
            snapshot: updated.clone(),
            synced_ui_seq: 7,
            rejected_ui_seqs: vec![9, 11],
        }))
        .await;
    assert!(matches!(
        f.events.try_recv().unwrap(),
        Event::PluginPanel {
            snapshot,
            editor_bases,
            synced_ui_seq: Some(7),
            rejected_ui_seqs,
            ..
        } if snapshot == updated && editor_bases.is_empty() && rejected_ui_seqs == [9, 11]
    ));
    assert_eq!(
        f.state.plugins.instances[&f.identity.instance_id].panel,
        updated
    );
}
