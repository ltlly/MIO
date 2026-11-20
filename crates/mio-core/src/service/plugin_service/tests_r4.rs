use std::fs;

use mio_plugin_api::panel::{Empty, UiEvent, ui_event};

use super::*;
use crate::plugins::wasm::{build_demo_wasm_module, build_envelope_wasm_module};

pub(super) fn write_wasm_package(
    dir: &std::path::Path,
    id: &str,
    version: &str,
    wasm_bytes: &[u8],
    permissions: &[&str],
    contributions: &[&str],
) {
    fs::write(dir.join("plugin.wasm"), wasm_bytes).unwrap();
    let manifest = serde_json::json!({
        "id": id,
        "version": version,
        "api_version": 1,
        "engine": "wasm",
        "entry": "plugin.wasm",
        "permissions": permissions,
        "contributions": contributions,
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

pub(super) async fn drain_until_plugin_op(
    state: &mut State,
    events: &mut mpsc::Receiver<Event>,
    target_op: Uuid,
) -> Result<PluginOutcome, String> {
    loop {
        while let Ok(ev) = events.try_recv() {
            if let Event::PluginOperation {
                operation_id,
                result,
                ..
            } = ev
                && operation_id == target_op
            {
                return result;
            }
        }
        let signal = state.plugins.next_signal().await;
        state.plugin_signal(signal).await;
    }
}

#[test]
fn runtime_start_with_preferences_persists_plugin_prefs_across_project_switches() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let user_pref_path = canonical.join("profile").join("preferences.json");
    let plugin_pref_path = canonical.join("profile").join("plugin-preferences.json");

    let pkg_dir = canonical.join("pkg-prefs");
    fs::create_dir_all(&pkg_dir).unwrap();
    write_wasm_package(
        &pkg_dir,
        "lab.prefs",
        "0.1.0",
        &build_envelope_wasm_module(),
        &["storage.kv"],
        &["suite_panel"],
    );

    let (runtime, mut client) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client.events.recv().await {
                break project;
            }
        };
        let insp_op = Uuid::new_v4();
        client
            .commands
            .send(Command::Plugin {
                project,
                operation_id: insp_op,
                action: PluginAction::Inspect {
                    directory: pkg_dir.clone(),
                },
            })
            .await
            .unwrap();
        let inspection_id = loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Inspected(info)),
                ..
            }) = client.events.recv().await
                && operation_id == insp_op
            {
                break info.id;
            }
        };
        let load_op = Uuid::new_v4();
        client
            .commands
            .send(Command::Plugin {
                project,
                operation_id: load_op,
                action: PluginAction::Load {
                    inspection_id,
                    native_trusted: true,
                    grants: vec![Permission::Storage],
                    replace_instance: None,
                },
            })
            .await
            .unwrap();
        let (instance_id, panel_rev) = loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Loaded { info, panel }),
                ..
            }) = client.events.recv().await
                && operation_id == load_op
            {
                break (info.identity.instance_id, panel.revision);
            }
        };
        client
            .commands
            .send(Command::Plugin {
                project,
                operation_id: Uuid::new_v4(),
                action: PluginAction::Ui {
                    instance_id,
                    panel_revision: panel_rev,
                    event: UiEvent {
                        node_id: 10,
                        event: Some(ui_event::Event::Click(Empty {})),
                        editors: vec![],
                    },
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginPanel { snapshot, .. }) = client.events.recv().await
                && snapshot.revision > panel_rev
            {
                break;
            }
        }
        assert!(
            plugin_pref_path.is_file(),
            "PreferencesSet must persist to sibling plugin-preferences.json"
        );

        client
            .commands
            .send(Command::NewProject { path: None })
            .await
            .unwrap();
        let next_project = loop {
            if let Some(Event::ProjectOpened { project: p2, .. }) = client.events.recv().await
                && p2.generation > project.generation
            {
                break p2;
            }
        };
        assert_ne!(next_project.id, project.id);
    });
    drop(client);
    runtime.shutdown().unwrap();

    let reopened = PluginPreferencesStore::new(Some(plugin_pref_path));
    assert_eq!(
        reopened.get("lab.prefs", "wasm_tag").unwrap().as_deref(),
        Some(&b"envelope-v13"[..])
    );
}

#[tokio::test]
async fn reload_at_max_instances_succeeds_and_auto_reload_enforces_permission_subset() {
    let dir = tempfile::tempdir().unwrap();
    write_wasm_package(
        dir.path(),
        "lab.autoreload",
        "0.1.0",
        &build_demo_wasm_module(),
        &["traffic.read"],
        &["suite_panel"],
    );
    let (tx, mut rx) = mpsc::channel(128);
    let mut state = State::new(tx, CancellationToken::new()).unwrap();

    let insp_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            insp_op,
            PluginAction::Inspect {
                directory: dir.path().to_owned(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Inspected(insp) = drain_until_plugin_op(&mut state, &mut rx, insp_op)
        .await
        .unwrap()
    else {
        panic!("expected inspected");
    };
    let load_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            load_op,
            PluginAction::Load {
                inspection_id: insp.id,
                native_trusted: true,
                grants: vec![Permission::TrafficRead],
                replace_instance: None,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info, .. } = drain_until_plugin_op(&mut state, &mut rx, load_op)
        .await
        .unwrap()
    else {
        panic!("expected loaded");
    };
    let first_id = info.identity.instance_id;

    // Fill dummy instances up to MAX_INSTANCES (16) and verify Reload still succeeds.
    let mut filler_guards = Vec::new();
    while state.plugins.instances.len() < MAX_INSTANCES {
        let filler_dir = tempfile::tempdir().unwrap();
        write_wasm_package(
            filler_dir.path(),
            "lab.filler",
            "1.0.0",
            &build_demo_wasm_module(),
            &["traffic.read"],
            &["suite_panel"],
        );
        let pkg = VerifiedPackage::read(filler_dir.path()).unwrap();
        let trust = NativeTrust::confirmed(&pkg, [Permission::TrafficRead]).unwrap();
        let id = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: state.project,
        };
        let panel = std::sync::Arc::new(PanelSnapshot {
            revision: 1,
            title: "filler".into(),
            root: None,
        });
        let (worker, _q) = crate::plugins::test_supervisor(id, pkg.manifest.clone(), panel.clone());
        let (_stx, srx) = mpsc::channel(16);
        let m_mod = pkg.manifest_modified;
        let m_sz = pkg.manifest_bytes.len() as u64;
        let e_mod = pkg.modified;
        let e_sz = pkg.executable_size;
        filler_guards.push((_q, _stx, filler_dir));
        state.plugins.instances.insert(
            id.instance_id,
            Instance {
                worker,
                services: srx,
                info: LoadedPlugin {
                    identity: id,
                    directory: pkg.directory.clone(),
                    manifest: pkg.manifest.clone(),
                    grants: vec![Permission::TrafficRead],
                    tab_id: None,
                },
                package: pkg,
                manifest_modified: m_mod,
                manifest_size: m_sz,
                entry_modified: e_mod,
                entry_size: e_sz,
                auto_reload: false,
                trust,
                panel,
                source: None,
                resources: HashMap::new(),
            },
        );
    }
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);
    assert!(state.plugin_capacity().is_err());

    let reload_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            reload_op,
            PluginAction::Reload {
                instance_id: first_id,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Reloaded {
        old_instance,
        info: reloaded_info,
        ..
    } = drain_until_plugin_op(&mut state, &mut rx, reload_op)
        .await
        .unwrap()
    else {
        panic!("expected reloaded at MAX_INSTANCES");
    };
    assert_eq!(old_instance, first_id);
    let second_id = reloaded_info.identity.instance_id;
    assert_ne!(second_id, first_id);
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);

    // Auto-reload detects safe version update on disk.
    assert!(state.check_auto_reload().is_empty());
    write_wasm_package(
        dir.path(),
        "lab.autoreload",
        "0.2.0",
        &build_envelope_wasm_module(),
        &["traffic.read"],
        &["suite_panel"],
    );
    let auto_ops = state.check_auto_reload();
    assert_eq!(auto_ops.len(), 1);
    let PluginOutcome::Reloaded { info: v2_info, .. } =
        drain_until_plugin_op(&mut state, &mut rx, auto_ops[0])
            .await
            .unwrap()
    else {
        panic!("expected auto-reload completion");
    };
    assert_eq!(v2_info.manifest.version, "0.2.0");
    let third_id = v2_info.identity.instance_id;

    // Auto-reload blocks silent permission escalation.
    write_wasm_package(
        dir.path(),
        "lab.autoreload",
        "0.3.0",
        &build_envelope_wasm_module(),
        &["traffic.read", "network.send"],
        &["suite_panel"],
    );
    let esc_ops = state.check_auto_reload();
    assert_eq!(esc_ops.len(), 1);
    let esc_err = drain_until_plugin_op(&mut state, &mut rx, esc_ops[0])
        .await
        .unwrap_err();
    assert!(esc_err.contains("拒绝热重载提权"), "{esc_err}");
    assert!(state.plugins.instances.contains_key(&third_id));
    state.stop_plugins().await.unwrap();
}

fn build_wasm_tar_gz(id: &str, version: &str) -> Vec<u8> {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    let wasm = build_demo_wasm_module();
    let manifest = serde_json::json!({
        "id": id,
        "version": version,
        "api_version": 1,
        "engine": "wasm",
        "entry": "plugin.wasm",
        "permissions": ["traffic.read"],
        "contributions": ["suite_panel"],
    })
    .to_string();
    let mut tar = Vec::new();
    for (name, mode, data) in [
        ("manifest.json", 0o644u32, manifest.as_bytes()),
        ("plugin.wasm", 0o644u32, wasm.as_slice()),
    ] {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let write_oct = |dst: &mut [u8], val: u64, width: usize| {
            let s = format!("{val:0width$o}", width = width);
            dst[..width].copy_from_slice(s.as_bytes());
        };
        write_oct(&mut header[100..108], u64::from(mode), 7);
        write_oct(&mut header[108..116], 0, 7);
        write_oct(&mut header[116..124], 0, 7);
        write_oct(&mut header[124..136], data.len() as u64, 11);
        write_oct(&mut header[136..148], 0, 11);
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let chk: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let chk_s = format!("{chk:06o}\0 ");
        header[148..156].copy_from_slice(chk_s.as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(data);
        let rem = data.len() % 512;
        if rem != 0 {
            tar.resize(tar.len() + (512 - rem), 0);
        }
    }
    tar.resize(tar.len() + 1024, 0);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

#[test]
fn persistent_plugins_survive_runtime_restart_toggle_disable_extensions_and_uninstall() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let profile_dir = canonical.join("profile");
    let user_pref_path = profile_dir.join("preferences.json");
    let plugins_dir = profile_dir.join("plugins");
    let archive_path = canonical.join("downloads").join("lab.persist.tar.gz");
    fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
    fs::write(&archive_path, build_wasm_tar_gz("lab.persist", "0.1.0")).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // 1. First runtime: inspect & load from single-file .tar.gz, then delete source archive.
    let (runtime1, mut client1) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    rt.block_on(async {
        let project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client1.events.recv().await {
                break project;
            }
        };
        let insp_op = Uuid::new_v4();
        client1
            .commands
            .send(Command::Plugin {
                project,
                operation_id: insp_op,
                action: PluginAction::Inspect {
                    directory: archive_path.clone(),
                },
            })
            .await
            .unwrap();
        let inspection_id = loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Inspected(info)),
                ..
            }) = client1.events.recv().await
                && operation_id == insp_op
            {
                break info.id;
            }
        };
        let load_op = Uuid::new_v4();
        client1
            .commands
            .send(Command::Plugin {
                project,
                operation_id: load_op,
                action: PluginAction::Load {
                    inspection_id,
                    native_trusted: true,
                    grants: vec![Permission::TrafficRead],
                    replace_instance: None,
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Loaded { info, .. }),
                ..
            }) = client1.events.recv().await
                && operation_id == load_op
            {
                assert_eq!(info.manifest.id, "lab.persist");
                break;
            }
        }
    });
    drop(client1);
    runtime1.shutdown().unwrap();

    fs::remove_file(&archive_path).unwrap();
    assert!(plugins_dir.join("installed.json").is_file());
    assert!(plugins_dir.join("packages/lab.persist/manifest.json").is_file());

    // 2. Second runtime: StartProject auto-loads persisted plugin even though original .tar.gz is gone.
    let (runtime2, mut client2) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    rt.block_on(async {
        let initial_project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client2.events.recv().await {
                break project;
            }
        };
        client2
            .commands
            .send(Command::StartProject {
                path: None,
                create_new: true,
                options: ProjectStartOptions::default(),
            })
            .await
            .unwrap();
        let p2 = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client2.events.recv().await
                && project.generation > initial_project.generation
            {
                break project;
            }
        };
        let mut saw_registry = false;
        let auto_instance = loop {
            match client2.events.recv().await.unwrap() {
                Event::InstalledPlugins {
                    project,
                    plugins_dir: dir,
                    extensions_disabled,
                    plugins,
                } if project == p2 => {
                    assert_eq!(dir, plugins_dir);
                    assert!(!extensions_disabled);
                    assert_eq!(plugins.len(), 1);
                    assert_eq!(plugins[0].id, "lab.persist");
                    assert!(plugins[0].enabled);
                    saw_registry = true;
                }
                Event::PluginAutoLoaded { project, info, .. } if project == p2 => {
                    assert_eq!(info.manifest.id, "lab.persist");
                    break info.identity.instance_id;
                }
                _ => {}
            }
        };
        assert!(saw_registry);

        // Uncheck Loaded -> PluginAction::Disable keeps package in installed.json with enabled=false.
        let disable_op = Uuid::new_v4();
        client2
            .commands
            .send(Command::Plugin {
                project: p2,
                operation_id: disable_op,
                action: PluginAction::Disable {
                    instance_id: auto_instance,
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Disabled { .. }),
                ..
            }) = client2.events.recv().await
                && operation_id == disable_op
            {
                break;
            }
        }
    });
    drop(client2);
    runtime2.shutdown().unwrap();

    // 3. Third runtime: disabled plugin stays idle until explicitly enabled.
    let (runtime3, mut client3) = Runtime::start_with_preferences(user_pref_path.clone()).unwrap();
    rt.block_on(async {
        let initial_project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client3.events.recv().await {
                break project;
            }
        };
        client3
            .commands
            .send(Command::StartProject {
                path: None,
                create_new: true,
                options: ProjectStartOptions::default(),
            })
            .await
            .unwrap();
        let p3 = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client3.events.recv().await
                && project.generation > initial_project.generation
            {
                break project;
            }
        };
        loop {
            match client3.events.recv().await.unwrap() {
                Event::InstalledPlugins {
                    project, plugins, ..
                } if project == p3 => {
                    assert_eq!(plugins.len(), 1);
                    assert!(!plugins[0].enabled);
                    break;
                }
                Event::PluginAutoLoaded { .. } => {
                    panic!("disabled persisted plugin must not auto-load")
                }
                _ => {}
            }
        }
        let enable_op = Uuid::new_v4();
        client3
            .commands
            .send(Command::Plugin {
                project: p3,
                operation_id: enable_op,
                action: PluginAction::EnableInstalled {
                    plugin_id: "lab.persist".into(),
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::InstalledEnabled { info, .. }),
                ..
            }) = client3.events.recv().await
                && operation_id == enable_op
            {
                assert_eq!(info.manifest.id, "lab.persist");
                break;
            }
        }
    });
    drop(client3);
    runtime3.shutdown().unwrap();

    // 4. Fourth runtime: disable_extensions suppresses auto-load, while Uninstall still deletes files.
    let (runtime4, mut client4) = Runtime::start_with_preferences(user_pref_path).unwrap();
    rt.block_on(async {
        let initial_project = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client4.events.recv().await {
                break project;
            }
        };
        client4
            .commands
            .send(Command::StartProject {
                path: None,
                create_new: true,
                options: ProjectStartOptions {
                    disable_extensions: true,
                    ..ProjectStartOptions::default()
                },
            })
            .await
            .unwrap();
        let p4 = loop {
            if let Some(Event::ProjectOpened { project, .. }) = client4.events.recv().await
                && project.generation > initial_project.generation
            {
                break project;
            }
        };
        loop {
            match client4.events.recv().await.unwrap() {
                Event::InstalledPlugins {
                    project,
                    extensions_disabled,
                    plugins,
                    ..
                } if project == p4 => {
                    assert!(extensions_disabled);
                    assert_eq!(plugins.len(), 1);
                    assert!(plugins[0].enabled);
                    break;
                }
                Event::PluginAutoLoaded { .. } => {
                    panic!("disable_extensions must suppress auto-loading")
                }
                _ => {}
            }
        }
        let enable_op = Uuid::new_v4();
        client4
            .commands
            .send(Command::Plugin {
                project: p4,
                operation_id: enable_op,
                action: PluginAction::EnableInstalled {
                    plugin_id: "lab.persist".into(),
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Err(message),
                ..
            }) = client4.events.recv().await
                && operation_id == enable_op
            {
                assert!(message.contains("Disable extensions"), "{message}");
                break;
            }
        }
        let uninstall_op = Uuid::new_v4();
        client4
            .commands
            .send(Command::Plugin {
                project: p4,
                operation_id: uninstall_op,
                action: PluginAction::Uninstall {
                    plugin_id: "lab.persist".into(),
                    instance_id: None,
                },
            })
            .await
            .unwrap();
        loop {
            if let Some(Event::PluginOperation {
                operation_id,
                result: Ok(PluginOutcome::Uninstalled { plugin_id, identity }),
                ..
            }) = client4.events.recv().await
                && operation_id == uninstall_op
            {
                assert_eq!(plugin_id, "lab.persist");
                assert!(identity.is_none());
                break;
            }
        }
    });
    drop(client4);
    runtime4.shutdown().unwrap();

    assert!(!plugins_dir.join("packages/lab.persist").exists());
    let store = crate::plugins::PluginRegistryStore::new(Some(plugins_dir));
    assert!(store.list().unwrap().is_empty());
}
