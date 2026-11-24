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
    state1.stop_proxy().await.unwrap();
    state2.stop_proxy().await.unwrap();
}

fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn temp_state_with_prefs(
    prefs_path: std::path::PathBuf,
) -> (State, mpsc::Receiver<Event>) {
    let (events, rx) = mpsc::channel(128);
    let state = State::with_preference_paths(
        events,
        CancellationToken::new(),
        crate::preferences::PreferencesStore::new(Some(prefs_path.clone())),
        Some(prefs_path),
    )
    .unwrap();
    (state, rx)
}

#[tokio::test]
async fn cross_window_ca_sync_hot_swaps_running_proxy_and_isolated_ca_does_not_write_disk() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let prefs_path = root.join("preferences.json");

    let (mut win1, mut rx1) = temp_state_with_prefs(prefs_path.clone());
    let (mut win2, mut rx2) = temp_state_with_prefs(prefs_path);

    let listen2: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    win2.command(Command::StartProxy {
        project: win2.project,
        listen: listen2,
    })
    .await
    .unwrap();
    let proxy2_addr = win2.proxy.primary_address().unwrap();

    let ca1 = win1.authority().await.unwrap();
    let ca2 = win2.proxy.ca.as_ref().unwrap().clone();
    assert_eq!(ca1.certificate_der(), ca2.certificate_der());

    win1.command(Command::RegenerateCa).await.unwrap();
    let new_ca = win1.proxy.ca.as_ref().unwrap().clone();
    assert_ne!(new_ca.certificate_der(), ca1.certificate_der());

    let mut saw_win1_fp = None;
    while let Ok(ev) = rx1.try_recv() {
        if let Event::CaRegenerated { fingerprint } = ev {
            saw_win1_fp = Some(fingerprint);
        }
    }
    let expected_fp = authority::fingerprint_sha256(&new_ca.certificate_der());
    assert_eq!(saw_win1_fp.as_deref(), Some(expected_fp.as_str()));

    // Window 2 syncs from disk, emits CaRegenerated, and hot-swaps its live proxy listener.
    win2.sync_shared_state_from_disk().await;
    assert_eq!(
        win2.proxy.ca.as_ref().unwrap().certificate_der(),
        new_ca.certificate_der()
    );
    let mut saw_win2_fp = None;
    while let Ok(ev) = rx2.try_recv() {
        if let Event::CaRegenerated { fingerprint } = ev {
            saw_win2_fp = Some(fingerprint);
        }
    }
    assert_eq!(saw_win2_fp.as_deref(), Some(expected_fp.as_str()));

    let mut conn = tokio::net::TcpStream::connect(proxy2_addr).await.unwrap();
    conn.write_all(b"GET http://mio/cert HTTP/1.1\r\nHost: mio\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut raw = Vec::new();
    conn.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(&raw[split..], new_ca.certificate_der().as_slice());
    win2.stop_proxy().await.unwrap();

    // Isolated state RegenerateCa / ImportCa must not alter the disk authority.json.
    let (iso_events, _iso_rx) = mpsc::channel(32);
    let mut isolated = State::new(iso_events, CancellationToken::new()).unwrap();
    let alt = MitmAuthority::generate().unwrap();
    let on_disk_before = authority::load_if_changed_at(&root.join("identity"), None)
        .unwrap()
        .unwrap()
        .0
        .certificate_der();
    isolated.command(Command::RegenerateCa).await.unwrap();
    isolated
        .command(Command::ImportCa {
            cert_pem: alt.certificate_pem(),
            key_pem: alt.private_key_pem(),
        })
        .await
        .unwrap();
    let on_disk_after = authority::load_if_changed_at(&root.join("identity"), None)
        .unwrap()
        .unwrap()
        .0
        .certificate_der();
    assert_eq!(on_disk_before, on_disk_after);
}

#[tokio::test]
async fn multi_window_listener_locks_reject_wildcard_overlap_and_rollback_atomic_rebind() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let prefs_path = root.join("preferences.json");

    let (mut win1, _rx1) = temp_state_with_prefs(prefs_path.clone());
    let (mut win2, mut rx2) = temp_state_with_prefs(prefs_path.clone());

    let port1 = free_loopback_port();
    let port2 = free_loopback_port();
    assert_ne!(port1, port2);

    // Win 1 listens on 0.0.0.0:port1.
    let wild_addr: std::net::SocketAddr = (std::net::Ipv4Addr::UNSPECIFIED, port1).into();
    win1.command(Command::StartProxy {
        project: win1.project,
        listen: wild_addr,
    })
    .await
    .unwrap();

    // Win 2 trying 127.0.0.1:port1 must be rejected cleanly via listener_lock.
    let loop_addr1: std::net::SocketAddr = (std::net::Ipv4Addr::LOCALHOST, port1).into();
    let err = win2
        .command(Command::StartProxy {
            project: win2.project,
            listen: loop_addr1,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("已被占用") && err.to_string().contains("Proxy listeners"),
        "unexpected conflict error: {err}"
    );
    assert!(!win2.proxy.is_running());

    // Listener lock layer allows non-overlapping IPv6 loopback on same port.
    let locks_dir = root.join("run").join("listener-locks");
    let v6_loop: std::net::SocketAddr = (std::net::Ipv6Addr::LOCALHOST, port1).into();
    let v6_lock = listener_lock::acquire_listener_lock(Some(&locks_dir), v6_loop).unwrap();
    drop(v6_lock);

    // Win 2 starts normally on port2, then attempts SaveWorkspace to move onto conflicting port1.
    let mut ws2 = win2.store.workspace().unwrap();
    ws2.revision += 1;
    let listener2 = ProxyListenerConfig {
        id: Uuid::new_v4(),
        enabled: true,
        bind_address: format!("127.0.0.1:{port2}"),
        ..ProxyListenerConfig::default()
    };
    ws2.proxy_listeners = vec![listener2.clone()];
    win2.command(Command::SaveWorkspace {
        project: win2.project,
        workspace: ws2.clone(),
    })
    .await
    .unwrap();
    let loop_addr2: std::net::SocketAddr = (std::net::Ipv4Addr::LOCALHOST, port2).into();
    win2.command(Command::StartProxy {
        project: win2.project,
        listen: loop_addr2,
    })
    .await
    .unwrap();
    while rx2.try_recv().is_ok() {}

    let mut bad_ws2 = ws2.clone();
    bad_ws2.revision += 1;
    bad_ws2.proxy_listeners.push(ProxyListenerConfig {
        id: Uuid::new_v4(),
        enabled: true,
        bind_address: format!("127.0.0.1:{port1}"),
        ..ProxyListenerConfig::default()
    });
    win2.command(Command::SaveWorkspace {
        project: win2.project,
        workspace: bad_ws2,
    })
    .await
    .unwrap();

    let mut saw_rejected = false;
    while let Ok(ev) = rx2.try_recv() {
        if let Event::WorkspaceRejected {
            stored, message, ..
        } = ev
        {
            assert_eq!(stored.proxy_listeners, vec![listener2.clone()]);
            assert!(message.contains("已被占用"), "{message}");
            saw_rejected = true;
        }
    }
    assert!(saw_rejected, "conflicting SaveWorkspace must emit WorkspaceRejected");
    assert_eq!(
        win2.store.workspace().unwrap().proxy_listeners,
        vec![listener2]
    );
    assert_eq!(
        win2.proxy.primary_address(),
        Some(loop_addr2),
        "atomic rebind failure must keep win2's previous listener alive"
    );

    win1.stop_proxy().await.unwrap();
    win2.stop_proxy().await.unwrap();

    // After StopProxy, port1 is released and Win 2 can bind it.
    win2.command(Command::StartProxy {
        project: win2.project,
        listen: loop_addr1,
    })
    .await
    .unwrap();
    win2.stop_proxy().await.unwrap();
}

#[tokio::test]
async fn browser_profile_and_default_port_fallback_isolate_windows_and_rotated_ca() {
    let base = tempfile::tempdir().unwrap();
    let p_default = core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        "127.0.0.1:8080".parse().unwrap(),
    );
    let p_other_port = core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        "127.0.0.1:8081".parse().unwrap(),
    );
    let p_lan_ip = core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        "192.0.2.10:8080".parse().unwrap(),
    );
    let p_v6_loopback = core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        "[::1]:8080".parse().unwrap(),
    );
    let p_v4_alt_loopback = core_commands_proxy::browser_profile_folder_name(
        "browser-profile",
        "127.0.0.2:8080".parse().unwrap(),
    );
    assert_eq!(p_default, "browser-profile");
    assert_eq!(p_other_port, "browser-profile-8081");
    assert_eq!(p_lan_ip, "browser-profile-192_0_2_10-8080");
    assert_eq!(p_v6_loopback, "browser-profile-__1-8080");
    assert_eq!(p_v4_alt_loopback, "browser-profile-127_0_0_2-8080");

    // Same listener port + rotated CA must switch away from stale SPKI profile dir.
    let first_dir = core_commands_proxy::resolve_ca_scoped_profile_dir(
        base.path(),
        "browser-profile",
        "spki-first",
    );
    assert_eq!(first_dir, base.path().join("browser-profile"));
    std::fs::create_dir_all(&first_dir).unwrap();
    authority::export(b"spki-first", &first_dir.join(".mio-ca-spki")).unwrap();
    assert_eq!(
        core_commands_proxy::resolve_ca_scoped_profile_dir(
            base.path(),
            "browser-profile",
            "spki-first",
        ),
        first_dir
    );
    let rotated_dir = core_commands_proxy::resolve_ca_scoped_profile_dir(
        base.path(),
        "browser-profile",
        "spki-rotated",
    );
    assert_ne!(rotated_dir, first_dir);
    assert!(
        rotated_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("browser-profile-ca-")
    );

    // Default-config fallback finds the next free port when primary port is occupied.
    let prefs_path = base.path().canonicalize().unwrap().join("preferences.json");
    let (mut win1, _rx1) = temp_state_with_prefs(prefs_path.clone());
    let (mut win2, mut rx2) = temp_state_with_prefs(prefs_path);
    let base_port = free_loopback_port();
    let occupied: std::net::SocketAddr = (std::net::Ipv4Addr::LOCALHOST, base_port).into();
    win1.command(Command::StartProxy {
        project: win1.project,
        listen: occupied,
    })
    .await
    .unwrap();

    win2.start_default_proxy_with_port_fallback(win2.project, occupied, 10)
        .await
        .unwrap();
    let fallback_addr = win2.proxy.primary_address().unwrap();
    assert_ne!(fallback_addr.port(), base_port);
    let ws2 = win2.store.workspace().unwrap();
    assert_eq!(ws2.proxy_listeners.len(), 1);
    assert_eq!(ws2.proxy_listeners[0].bind_address, fallback_addr.to_string());
    let mut saw_ws_changed = false;
    while let Ok(ev) = rx2.try_recv() {
        if matches!(ev, Event::WorkspaceChanged { .. }) {
            saw_ws_changed = true;
        }
    }
    assert!(saw_ws_changed);
    win1.stop_proxy().await.unwrap();
    win2.stop_proxy().await.unwrap();
}

#[tokio::test]
async fn ipc_slim_save_rejects_project_locked_by_another_window() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project_a = root.join("win-a.mio");
    let project_b = root.join("win-b.mio");

    let (events1, _rx1) = mpsc::channel(32);
    let mut win1 = State::new(events1, CancellationToken::new()).unwrap();
    win1.command(Command::StartProject {
        path: Some(project_a.clone()),
        create_new: true,
        options: ProjectStartOptions::default(),
    })
    .await
    .unwrap();

    let (events2, _rx2) = mpsc::channel(32);
    let mut win2 = State::new(events2, CancellationToken::new()).unwrap();
    win2.command(Command::StartProject {
        path: Some(project_b),
        create_new: true,
        options: ProjectStartOptions::default(),
    })
    .await
    .unwrap();

    let env = win2
        .handle_ai_ipc(crate::AiIpcRequest::SlimSave {
            path: project_a.to_string_lossy().into_owned(),
            options: None,
        })
        .await;
    assert!(!env.ok, "slim save onto project held by win1 must fail");
    assert!(
        env.error
            .as_deref()
            .unwrap()
            .contains("已在另一个 MIO 窗口中打开")
    );
}

#[test]
fn multi_window_ipc_falls_back_to_pid_socket_when_primary_is_occupied() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let prefs = root.join("preferences.json");
    let primary_sock = root.join("mio.sock");

    let (_rt1, client1) =
        Runtime::start_app_with_preferences_and_ipc(prefs.clone(), primary_sock.clone()).unwrap();
    assert_eq!(client1.ipc_socket_path.as_deref(), Some(primary_sock.as_path()));

    let (_rt2, client2) =
        Runtime::start_app_with_preferences_and_ipc(prefs, primary_sock.clone()).unwrap();
    let sock2 = client2
        .ipc_socket_path
        .clone()
        .expect("second instance must bind per-instance IPC socket");
    assert_ne!(sock2, primary_sock);

    let (env1, _) =
        crate::call_ai_ipc(&primary_sock, &crate::AiIpcRequest::Status, Duration::from_secs(2))
            .unwrap();
    let (env2, _) =
        crate::call_ai_ipc(&sock2, &crate::AiIpcRequest::Status, Duration::from_secs(2))
            .unwrap();
    assert!(env1.ok);
    assert!(env2.ok);
    let Some(crate::AiIpcPayload::Status {
        project_id: pid1, ..
    }) = env1.payload
    else {
        panic!("expected Status payload from instance 1");
    };
    let Some(crate::AiIpcPayload::Status {
        project_id: pid2, ..
    }) = env2.payload
    else {
        panic!("expected Status payload from instance 2");
    };
    assert_ne!(pid1, pid2);
}

#[tokio::test]
async fn apply_save_workspace_clearing_upstream_proxy_syncs_cache_before_reload() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project_path = root.join("upstream-clear.mio");
    let (events, _rx) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    state
        .command(Command::StartProject {
            path: Some(project_path),
            create_new: true,
            options: ProjectStartOptions::default(),
        })
        .await
        .unwrap();

    // Configure an upstream proxy rule pointing to a closed local port and start proxy.
    let closed_port = free_loopback_port();
    let mut ws1 = state.store.workspace().unwrap();
    ws1.revision += 1;
    ws1.upstream_proxy.rules.push(crate::UpstreamProxyRule {
        host: "127.0.0.1".into(),
        port: closed_port,
        ..crate::UpstreamProxyRule::default()
    });
    state
        .command(Command::SaveWorkspace {
            project: state.project,
            workspace: ws1,
        })
        .await
        .unwrap();
    state
        .command(Command::StartProxy {
            project: state.project,
            listen: (std::net::Ipv4Addr::LOCALHOST, 0).into(),
        })
        .await
        .unwrap();

    // Clear upstream_proxy rules with rebind: live proxy must use cleared rules, not stale cache.
    let mut ws2 = state.store.workspace().unwrap();
    ws2.revision += 1;
    ws2.upstream_proxy.rules.clear();
    state
        .command(Command::SaveWorkspace {
            project: state.project,
            workspace: ws2,
        })
        .await
        .unwrap();
    let ws_after = state.store.workspace().unwrap();
    assert!(
        ws_after.upstream_proxy
            .effective_for_runtime(&state.user_level_connections_cache)
            .rules
            .is_empty(),
        "user_level_connections_cache must clear before proxy reload"
    );

    // Start a real origin and verify live proxy reaches it directly (not routed to closed_port).
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let origin_port = listener.local_addr().unwrap().port();
    let origin_task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let (mut s, _) = listener.accept().await.unwrap();
        let mut req = [0u8; 1024];
        let _ = s.read(&mut req).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });

    let proxy_addr = state.proxy.primary_address().unwrap();
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut cli = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
    cli.write_all(
        format!("GET http://127.0.0.1:{origin_port}/direct HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut resp = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), cli.read_to_end(&mut resp))
        .await
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 200 OK"),
        "expected direct 200 OK after clearing upstream rule, got: {}",
        String::from_utf8_lossy(&resp)
    );

    origin_task.await.unwrap();
    state.stop_proxy().await.unwrap();
}
