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
