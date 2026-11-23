use super::{tests::completed_flow, *};

#[tokio::test]
async fn regression_old_command_error_cannot_be_attributed_to_the_new_project() {
    let (events, mut receiver) = mpsc::channel(128);
    let cancel = CancellationToken::new();
    let state = State::new(events, cancel.clone()).unwrap();
    let previous = state.project;
    let (commands, incoming) = mpsc::channel(4);
    commands
        .send(Command::NewProject { path: None })
        .await
        .unwrap();
    commands
        .send(Command::QueryHistory {
            project: previous,
            query_revision: 1,
            offset: 0,
            limit: 1,
            filter: String::new(),
        })
        .await
        .unwrap();
    let receive = async {
        while let Some(event) = receiver.recv().await {
            if let Event::Error {
                project,
                operation: "query_history",
                ..
            } = event
            {
                cancel.cancel();
                return project;
            }
        }
        panic!("missing rejected stale-command result");
    };
    let (result, error_project) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(state.run(incoming), receive)
    })
    .await
    .unwrap();
    result.unwrap();
    assert_eq!(
        error_project,
        Some(previous),
        "a stale query must not display an error or change loading state in the new project"
    );
}

#[tokio::test]
async fn abnormal_send_task_always_has_a_terminal_tab_event() {
    let (events, mut receiver) = mpsc::channel(128);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut record = completed_flow();
    record.summary.state = FlowState::Pending;
    record.summary.status = None;
    record.response = None;
    state.store.put(&record).unwrap();
    let task = state
        .repeater.jobs
        .spawn(async { panic!("controlled worker panic") });
    let tab_id = Uuid::new_v4();
    state.repeater.sends.insert(
        tab_id,
        ActiveSend {
            cancel: CancellationToken::new(),
            task_id: task.id(),
            flow_id: record.summary.id,
            draft_revision: 9,
            progress_rx: None,
        },
    );
    let Some(Err(error)) = state.repeater.jobs.join_next().await else {
        panic!("expected a failed task")
    };
    state.failed_job(error).await;
    assert!(!state.repeater.sends.contains_key(&tab_id));
    let Event::SendFinished {
        tab_id: actual,
        draft_revision,
        record,
        persisted,
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("missing terminal tab event")
    };
    assert_eq!((actual, draft_revision, persisted), (tab_id, 9, true));
    assert!(matches!(record.summary.state, FlowState::Failed(_)));
    assert_eq!(state.store.read(record.summary.id).unwrap(), *record);
}

#[tokio::test]
async fn startup_preferences_and_start_project_record_recent_and_apply_config_modes() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let root = directory.path().canonicalize().unwrap();
    let prefs_path = root.join("preferences.json");
    let config_path = root.join("custom-config.json");
    let baseline_no_listeners = root.join("no-listeners.json");
    std::fs::write(
        &baseline_no_listeners,
        b"{\n  \"proxy\": {\n    \"request_listeners\": []\n  }\n}\n",
    )
    .unwrap();
    let pool_id = Uuid::new_v4();
    std::fs::write(
        &config_path,
        serde_json::to_vec(&serde_json::json!({
            "proxy": { "request_listeners": [] },
            "repeater": {
                "unpack_compressed_responses": false,
                "update_content_length": false
            },
            "intruder_resource_pools": {
                "revision": 1,
                "default_pool_id": pool_id,
                "pools": [{
                    "id": pool_id,
                    "name": "向导配置池",
                    "max_concurrent_requests": 4,
                    "delay": { "mode": "Fixed", "milliseconds": 250 },
                    "automatic_throttling": {
                        "enabled": true,
                        "status_429": true,
                        "status_503": true,
                        "other_enabled": false,
                        "other_codes": ""
                    }
                }]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let (events, mut receiver) = mpsc::channel(64);
    let mut state = State::with_preferences(
        events,
        CancellationToken::new(),
        crate::preferences::PreferencesStore::new(Some(prefs_path.clone())),
    )
    .unwrap();

    let op = Uuid::new_v4();
    state
        .command(Command::SetUserStartupPreferences {
            operation_id: op,
            expected_revision: 0,
            default_config: StartupDefaultConfig::ConfigFile {
                path: baseline_no_listeners.clone(),
            },
            disable_extensions: true,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::UserPreferencesSaved {
            operation_id,
            result: Ok(ref saved),
        } if operation_id == op
            && saved.startup.disable_extensions
            && saved.startup.default_config
                == Some(StartupDefaultConfig::ConfigFile {
                    path: baseline_no_listeners.clone()
                })
    ));

    state
        .command(Command::StartProject {
            path: None,
            create_new: true,
            options: ProjectStartOptions {
                name: Some("  临时验证项目  ".into()),
                config_mode: StartupDefaultConfig::ConfigFile {
                    path: baseline_no_listeners,
                },
                pause_automated_tasks: true,
                trust_project_file: true,
                disable_extensions: true,
            },
        })
        .await
        .unwrap();
    assert_eq!(
        state.store.workspace().unwrap().project_name.as_deref(),
        Some("临时验证项目")
    );
    assert!(state.extensions_disabled);
    while receiver.try_recv().is_ok() {}

    let plugin_op = Uuid::new_v4();
    state
        .command(Command::Plugin {
            project: state.project,
            operation_id: plugin_op,
            action: PluginAction::Inspect {
                directory: root.clone(),
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::PluginOperation {
            operation_id,
            result: Err(ref msg),
            ..
        } if operation_id == plugin_op && msg.contains("Disable extensions")
    ));

    let disk_path = root.join("wizard-disk.mio");
    state
        .command(Command::StartProject {
            path: Some(disk_path.clone()),
            create_new: true,
            options: ProjectStartOptions {
                name: Some("磁盘项目A".into()),
                config_mode: StartupDefaultConfig::ConfigFile {
                    path: config_path.clone(),
                },
                pause_automated_tasks: true,
                trust_project_file: true,
                disable_extensions: false,
            },
        })
        .await
        .unwrap();
    let mut seen_prefs = false;
    let mut opened_workspace = None;
    while let Ok(event) = receiver.try_recv() {
        match event {
            Event::UserPreferences {
                operation_id,
                state: pref_state,
            } if operation_id.is_nil() => {
                assert_eq!(pref_state.preferences.recent_projects.len(), 1);
                assert_eq!(pref_state.preferences.recent_projects[0].name, "磁盘项目A");
                assert_eq!(pref_state.preferences.recent_projects[0].path, disk_path);
                seen_prefs = true;
            }
            Event::ProjectOpened { workspace, .. } => {
                opened_workspace = Some(workspace);
            }
            _ => {}
        }
    }
    assert!(seen_prefs);
    let opened_workspace = opened_workspace.unwrap();
    assert_eq!(opened_workspace.project_name.as_deref(), Some("磁盘项目A"));
    assert_eq!(
        opened_workspace.repeater_settings,
        RepeaterSettings {
            unpack_compressed_responses: false,
            update_content_length: false,
            ..RepeaterSettings::default()
        }
    );
    assert_eq!(state.intruder.resource_pools.default_pool_id, Some(pool_id));
    assert_eq!(state.intruder.resource_pools.pools.len(), 1);
    assert_eq!(state.intruder.resource_pools.pools[0].name, "向导配置池");
    assert!(!state.extensions_disabled);

    state
        .command(Command::StartProject {
            path: None,
            create_new: true,
            options: ProjectStartOptions::default(),
        })
        .await
        .unwrap();
    while receiver.try_recv().is_ok() {}

    state
        .command(Command::StartProject {
            path: Some(disk_path.clone()),
            create_new: false,
            options: ProjectStartOptions {
                name: None,
                config_mode: StartupDefaultConfig::Project,
                pause_automated_tasks: true,
                trust_project_file: false,
                disable_extensions: false,
            },
        })
        .await
        .unwrap();
    assert!(
        state.extensions_disabled,
        "untrusted project file must disable extensions"
    );
    assert_eq!(
        state.store.workspace().unwrap().repeater_settings,
        RepeaterSettings {
            unpack_compressed_responses: false,
            update_content_length: false,
            ..RepeaterSettings::default()
        }
    );
    while receiver.try_recv().is_ok() {}

    state
        .command(Command::StartProject {
            path: None,
            create_new: true,
            options: ProjectStartOptions::default(),
        })
        .await
        .unwrap();
    state
        .command(Command::StartProject {
            path: Some(disk_path.clone()),
            create_new: false,
            options: ProjectStartOptions {
                name: Some("重置后名称".into()),
                config_mode: StartupDefaultConfig::Defaults,
                pause_automated_tasks: true,
                trust_project_file: true,
                disable_extensions: false,
            },
        })
        .await
        .unwrap();
    let reset_ws = state.store.workspace().unwrap();
    assert_eq!(reset_ws.project_name.as_deref(), Some("重置后名称"));
    assert_eq!(reset_ws.repeater_settings, RepeaterSettings::default());
    assert_eq!(
        state.intruder.resource_pools.pools[0].max_concurrent_requests,
        Some(10)
    );
    assert_eq!(state.intruder.resource_pools.pools[0].delay, None);
}

#[tokio::test]
async fn start_project_rejects_unknown_or_nonempty_listener_configs_without_side_effects() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let root = directory.path().canonicalize().unwrap();
    let unknown_cfg = root.join("unknown.json");
    let listener_cfg = root.join("listeners.json");
    std::fs::write(
        &unknown_cfg,
        br#"{"project_options":{"http":{"redirections":"always"}}}"#,
    )
    .unwrap();
    std::fs::write(
        &listener_cfg,
        br#"{"proxy":{"request_listeners":[{"listener_port":8080}]}}"#,
    )
    .unwrap();

    let (events, _receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let initial_project = state.project;
    for bad_cfg in [&unknown_cfg, &listener_cfg] {
        let target = root.join("must-not-exist.mio");
        let err = state
            .command(Command::StartProject {
                path: Some(target.clone()),
                create_new: true,
                options: ProjectStartOptions {
                    name: Some("不会创建".into()),
                    config_mode: StartupDefaultConfig::ConfigFile {
                        path: bad_cfg.clone(),
                    },
                    ..ProjectStartOptions::default()
                },
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("未映射") || err.to_string().contains("request_listeners"),
            "{err}"
        );
        assert!(!target.exists());
        assert_eq!(state.project, initial_project);
    }
}

#[tokio::test]
async fn multi_window_project_lock_rejects_concurrent_open_or_save_as_and_releases_on_switch_or_close()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let project_a = root.join("window-a.mio");
    let project_b = root.join("window-b.mio");

    let (events1, _rx1) = mpsc::channel(64);
    let mut win1 = State::new(events1, CancellationToken::new()).unwrap();
    win1.command(Command::StartProject {
        path: Some(project_a.clone()),
        create_new: true,
        options: ProjectStartOptions {
            name: Some("窗口A项目".into()),
            ..ProjectStartOptions::default()
        },
    })
    .await
    .unwrap();

    let mut pending_flow = completed_flow();
    pending_flow.summary.state = FlowState::Pending;
    pending_flow.summary.status = None;
    pending_flow.response = None;
    win1.store.put(&pending_flow).unwrap();

    let (events2, _rx2) = mpsc::channel(64);
    let mut win2 = State::new(events2, CancellationToken::new()).unwrap();
    let win2_initial = win2.project;
    let open_err = win2
        .command(Command::OpenProject {
            path: project_a.clone(),
        })
        .await
        .unwrap_err();
    assert!(
        open_err.to_string().contains("已在另一个 MIO 窗口中打开"),
        "unexpected error: {open_err}"
    );
    assert_eq!(win2.project, win2_initial);
    assert_eq!(
        win1.store.read(pending_flow.summary.id).unwrap().summary.state,
        FlowState::Pending,
        "rejected second-window open must not trigger crash recovery on live pending flows"
    );

    let start_err = win2
        .command(Command::StartProject {
            path: Some(project_a.clone()),
            create_new: false,
            options: ProjectStartOptions::default(),
        })
        .await
        .unwrap_err();
    assert!(start_err.to_string().contains("已在另一个 MIO 窗口中打开"));

    win2.command(Command::NewProject {
        path: Some(project_b.clone()),
    })
    .await
    .unwrap();
    assert_eq!(win2.store.path.as_deref(), Some(project_b.as_path()));

    let save_conflict = win1
        .command(Command::SaveProject {
            project: win1.project,
            path: Some(project_b.clone()),
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .await
        .unwrap_err();
    assert!(
        save_conflict.to_string().contains("已存在")
            || save_conflict.to_string().contains("已在另一个 MIO 窗口中打开")
    );

    win1.store.flows_mut().finish_pending(true, false).unwrap();
    win1.command(Command::NewProject { path: None })
        .await
        .unwrap();

    win2.command(Command::OpenProject {
        path: project_a.clone(),
    })
    .await
    .unwrap();
    assert_eq!(win2.store.path.as_deref(), Some(project_a.as_path()));

    win1.command(Command::OpenProject {
        path: project_b.clone(),
    })
    .await
    .unwrap();
    assert_eq!(win1.store.path.as_deref(), Some(project_b.as_path()));

    // Failed same-path reopen must not lose win1's active lock on project_b.
    let failed_reopen = win1
        .command(Command::StartProject {
            path: Some(project_b.clone()),
            create_new: false,
            options: ProjectStartOptions {
                config_mode: StartupDefaultConfig::ConfigFile {
                    path: root.join("missing-config.json"),
                },
                ..ProjectStartOptions::default()
            },
        })
        .await;
    assert!(failed_reopen.is_err());
    let still_locked = win2
        .command(Command::OpenProject {
            path: project_b.clone(),
        })
        .await
        .unwrap_err();
    assert!(still_locked.to_string().contains("已在另一个 MIO 窗口中打开"));
}

#[test]
fn sidecar_lock_waits_for_transient_holder_instead_of_immediate_collision() {
    use std::fs::OpenOptions;
    use std::sync::{Arc, Barrier};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let pref_path = root.join("preferences.json");

    let mut store1 = crate::preferences::PreferencesStore::new(Some(pref_path.clone()));
    let saved = store1
        .record_recent("首个窗口", root.join("first.mio"), 100)
        .unwrap();
    assert_eq!(saved.revision, 1);

    let lock_path = root.join("preferences.lock");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    held.lock().unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let b2 = barrier.clone();
    let releaser = std::thread::spawn(move || {
        b2.wait();
        std::thread::sleep(Duration::from_millis(80));
        drop(held);
    });
    barrier.wait();
    let mut store2 = crate::preferences::PreferencesStore::new(Some(pref_path));
    let merged = store2
        .record_recent("第二个窗口", root.join("second.mio"), 200)
        .unwrap();
    releaser.join().unwrap();
    assert_eq!(merged.revision, 2);
    assert_eq!(merged.recent_projects.len(), 2);
}

#[tokio::test]
async fn configured_browser_profiles_are_isolated_per_listener_port() {
    let (events1, _rx1) = mpsc::channel(32);
    let mut state1 = State::new(events1, CancellationToken::new()).unwrap();
    let p1 = state1.project;
    let mut ws1 = state1.store.workspace().unwrap();
    ws1.revision += 1;
    ws1.proxy_listeners = vec![ProxyListenerConfig {
        id: Uuid::new_v4(),
        enabled: true,
        bind_address: "127.0.0.1:0".into(),
        ..ProxyListenerConfig::default()
    }];
    state1.store.save_workspace(&ws1).unwrap();

    let (events2, _rx2) = mpsc::channel(32);
    let mut state2 = State::new(events2, CancellationToken::new()).unwrap();
    let p2 = state2.project;
    let mut ws2 = state2.store.workspace().unwrap();
    ws2.revision += 1;
    ws2.proxy_listeners = vec![ProxyListenerConfig {
        id: Uuid::new_v4(),
        enabled: true,
        bind_address: "127.0.0.1:0".into(),
        ..ProxyListenerConfig::default()
    }];
    state2.store.save_workspace(&ws2).unwrap();

    let temp = tempfile::tempdir().unwrap();
    let shared_identity = temp.path().join("identity");
    let bin = std::path::PathBuf::from("/usr/bin/true");
    if !bin.is_file() {
        return;
    }
    state1
        .open_configured_browser_with(
            p1,
            Some("http://127.0.0.1/".into()),
            BrowserRequestSession::Default,
            Some(shared_identity.clone()),
            Some(bin.clone()),
        )
        .await
        .unwrap();
    state2
        .open_configured_browser_with(
            p2,
            Some("http://127.0.0.1/".into()),
            BrowserRequestSession::Default,
            Some(shared_identity),
            Some(bin),
        )
        .await
        .unwrap();

    let port1 = state1.proxy.primary_address().unwrap().port();
    let port2 = state2.proxy.primary_address().unwrap().port();
    assert_ne!(port1, port2);
    let dir1 = temp.path().join(core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        state1.proxy.primary_address().unwrap(),
    ));
    let dir2 = temp.path().join(core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        state2.proxy.primary_address().unwrap(),
    ));
    assert_ne!(dir1, dir2);
    assert!(dir1.join("mio-ca.cer").is_file());
    assert!(dir2.join("mio-ca.cer").is_file());
}
