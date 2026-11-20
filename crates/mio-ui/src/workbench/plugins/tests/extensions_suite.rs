use super::*;

fn suite_manifest(id: &str, engine: &str, contributions: &[&str]) -> PluginManifest {
    PluginManifest {
        id: id.into(),
        version: "1.0".into(),
        api_version: 1,
        engine: engine.into(),
        entry: PathBuf::from(if engine == "wasm" {
            "plugin.wasm"
        } else {
            "worker"
        }),
        permissions: vec![
            Permission::TrafficRead,
            Permission::TrafficModify,
            Permission::PayloadProcess,
            Permission::RepeaterOpen,
        ],
        contributions: contributions.iter().map(|c| c.to_string()).collect(),
    }
}

fn install_suite_plugin(
    fixture: &Fixture,
    id: &str,
    engine: &str,
    contributions: &[&str],
    cx: &mut TestAppContext,
) -> LoadedPlugin {
    let info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: PathBuf::from(format!("/tmp/mio-suite-fixture-{id}")),
        manifest: suite_manifest(id, engine, contributions),
        grants: vec![
            Permission::TrafficRead,
            Permission::TrafficModify,
            Permission::PayloadProcess,
            Permission::RepeaterOpen,
        ],
        tab_id: None,
    };
    let status_text = format!("{id} ({engine}) 运行中");
    let snapshot = Arc::new(PanelSnapshot {
        revision: 1,
        title: id.into(),
        root: Some(Node::new(
            1,
            "状态",
            node::Kind::Text(mio_core::panel::Text {
                text: status_text,
                tone: 0,
            }),
        )),
    });
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.install_plugin(info.clone(), snapshot, None, window, cx);
            view.extensions.selected = Some(info.identity.instance_id);
        });
        window.click("tool-extensions", cx);
        window.render_frame(cx);
    })
    .unwrap();
    info
}

#[gpui_kit::test]
fn wasm_trust_reorder_logs_open_intruder_and_payload_plugins(cx: &mut TestAppContext) {
    use mio_core::{HttpRequest, HttpVersion};

    let mut fixture = fixture(cx, vec![tab("1")], 64);
    drain(&mut fixture.commands);

    // 1. Inspect & confirm sandboxed WASM plugin via Extensions UI.
    let directory = cx.read(|cx| fixture.view.read(cx).extensions.directory.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.click("extensions-tab-settings", cx);
        window.render_frame(cx);
        window.click(("input", directory.entity_id()), cx);
        window.input("/tmp/mio-wasm-fixture", cx);
        window.click("plugin-inspect", cx);
    })
    .unwrap();
    let inspect_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action: PluginAction::Inspect { .. },
                ..
            } => Some(operation_id),
            _ => None,
        })
        .unwrap();
    let wasm_manifest = suite_manifest(
        "lab.wasm",
        "wasm",
        &["suite_panel", "payload_generator", "payload_processor"],
    );
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: inspect_op,
            result: Ok(PluginOutcome::Inspected(PluginInspection {
                id: Uuid::new_v4(),
                directory: "/tmp/mio-wasm-fixture".into(),
                executable: "/tmp/mio-wasm-fixture/plugin.wasm".into(),
                manifest: wasm_manifest.clone(),
            })),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("plugin-native-trust", cx);
        window.click(("plugin-grant", Permission::PayloadProcess as usize), cx);
        window.click("plugin-load", cx);
    })
    .unwrap();
    let load_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::Load {
                        native_trusted: true,
                        grants,
                        ..
                    },
                ..
            } if grants == vec![Permission::PayloadProcess] => Some(operation_id),
            _ => None,
        })
        .unwrap();
    let first = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: PathBuf::from("/tmp/mio-wasm-fixture"),
        manifest: wasm_manifest,
        grants: vec![Permission::PayloadProcess, Permission::RepeaterOpen],
        tab_id: None,
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: load_op,
            result: Ok(PluginOutcome::Loaded {
                info: first.clone(),
                panel: panel(b"wasm".to_vec()),
            }),
        })
        .unwrap();
    cx.run_until_parked();

    // 2. Install second suite plugin, reorder, and verify Output / Errors log clear.
    let second = install_suite_plugin(&fixture, "lab.second", "wasm", &["suite_panel"], cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-move-up", cx);
    })
    .unwrap();
    let reorder_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::Reorder {
                        instance_id,
                        move_up: true,
                    },
                ..
            } if instance_id == second.identity.instance_id => Some(operation_id),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: reorder_op,
            result: Ok(PluginOutcome::Reordered {
                order: vec![second.identity.instance_id, first.identity.instance_id],
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.extensions.instances[0].info.identity.instance_id,
            second.identity.instance_id
        );
        assert!(!view.extensions.logs.is_empty());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("extensions-detail-output", cx);
        window.render_frame(cx);
        window.click("plugin-clear-logs", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(fixture.view.read(cx).extensions.logs.is_empty()));

    // 3. Verify OpenIntruder workspace proposal & Intruder Payload Generator / Processor buttons.
    fixture
        .events
        .try_send(Event::PluginWorkspaceRequest(PluginWorkspaceRequest {
            identity: first.identity,
            request_id: 7,
            deadline: Instant::now() + Duration::from_secs(5),
            cancel: tokio_util::sync::CancellationToken::new(),
            edit: PluginWorkspaceEdit::OpenIntruder {
                request: HttpRequest {
                    method: "GET".into(),
                    url: "http://127.0.0.1:9001/item?x=1".into(),
                    version: HttpVersion::Http11,
                    headers: vec![],
                    body: vec![],
                    trailers: vec![],
                },
            },
        }))
        .unwrap();
    cx.run_until_parked();
    let (apply_op, proposed_ws, intr_id) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::ApplyWorkspace {
                        workspace, tab_id, ..
                    },
                ..
            } => Some((operation_id, workspace, tab_id)),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: proposed_ws,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: apply_op,
            result: Ok(PluginOutcome::WorkspaceApplied),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Intruder);
        assert_eq!(view.workspace.intruder.active, Some(intr_id));
    });

    let gen_btn = SharedString::from(format!(
        "intruder-plugin-gen-{}",
        first.identity.instance_id
    ));
    let proc_btn = SharedString::from(format!(
        "intruder-plugin-proc-{}",
        first.identity.instance_id
    ));
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        let req_entity = fixture.view.read(cx).intruder.editors[&intr_id]
            .request
            .state()
            .entity_id();
        window.click(("input", req_entity), cx);
        window.click("intruder-position-add", cx);
        window.render_frame(cx);
        window.click(gen_btn, cx);
    })
    .unwrap();
    let cmds = drain(&mut fixture.commands);
    let pos_ws = cmds
        .iter()
        .find_map(|c| match c {
            Command::SaveWorkspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        })
        .unwrap();
    let gen_op = cmds
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action: PluginAction::GeneratePayloads { .. },
                ..
            } => Some(operation_id),
            _ => None,
        })
        .unwrap();
    acknowledge(&fixture, pos_ws, cx);
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: gen_op,
            result: Ok(PluginOutcome::PayloadsGenerated {
                identity: first.identity,
                tab_id: intr_id,
                payloads: vec![b"alpha".to_vec(), b"beta".to_vec()],
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let saved_ws = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        saved_ws.intruder.tabs[0].spec.payload_sets[0],
        vec![b"alpha".to_vec(), b"beta".to_vec()]
    );
    acknowledge(&fixture, saved_ws, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(proc_btn, cx);
    })
    .unwrap();
    let (proc_op, proc_pos) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::ProcessPayloads {
                        position, inputs, ..
                    },
                ..
            } if inputs == vec![b"alpha".to_vec(), b"beta".to_vec()] => {
                Some((operation_id, position))
            }
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: proc_op,
            result: Ok(PluginOutcome::PayloadsProcessed {
                identity: first.identity,
                tab_id: intr_id,
                position: proc_pos,
                payloads: vec![b"p:alpha".to_vec(), b"p:beta".to_vec()],
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0]
                .spec
                .payload_sets[0],
            vec![b"p:alpha".to_vec(), b"p:beta".to_vec()]
        );
    });
}

#[gpui_kit::test]
fn ext_extensions_subtabs_search_details_and_error_logs(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("1")], 64);
    let alpha = install_suite_plugin(&fixture, "lab.alpha", "wasm", &["suite_panel"], cx);
    let beta = install_suite_plugin(&fixture, "lab.beta", "native", &["suite_panel"], cx);
    drain(&mut fixture.commands);

    // 1. Verify top External sub-tabs switch cleanly and return to Installed.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("extensions-tab-installed").selected(),
            Some(true)
        );
        assert!(window.find("plugin-instance-list").visible());
        assert!(window.find("extensions-detail-table").visible());

        window.click("extensions-tab-plugin-store", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("extensions-tab-plugin-store").selected(),
            Some(true)
        );
        assert!(window.find("extensions-plugin-store-view").visible());

        window.click("extensions-tab-apis", cx);
        window.render_frame(cx);
        assert_eq!(window.find("extensions-tab-apis").selected(), Some(true));
        assert!(window.find("extensions-apis-view").visible());

        window.click("extensions-tab-custom-scan-checks", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("extensions-tab-custom-scan-checks").selected(),
            Some(true)
        );
        assert!(window.find("extensions-custom-scan-checks-view").visible());

        window.click("extensions-tab-script-library", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("extensions-tab-script-library").selected(),
            Some(true)
        );
        assert!(window.find("extensions-script-library-view").visible());

        window.click("extensions-tab-settings", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("extensions-tab-settings").selected(),
            Some(true)
        );
        assert!(window.find("extensions-settings-view").visible());

        window.click("extensions-tab-installed", cx);
        window.render_frame(cx);
        assert!(window.find("plugin-instance-list").visible());
    })
    .unwrap();

    // 2. Verify live search filters the installed extensions table.
    let alpha_row = SharedString::from(format!("plugin-instance-{}", alpha.identity.instance_id));
    let beta_row = SharedString::from(format!("plugin-instance-{}", beta.identity.instance_id));
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(window.find(alpha_row.clone()).visible());
        assert!(window.find(beta_row.clone()).visible());
        window.click("extensions-search-input", cx);
        window.input("alpha", cx);
        window.render_frame(cx);
        assert!(window.find(alpha_row.clone()).visible());
        assert!(window.try_find(beta_row.clone()).is_none());
        window.click("extensions-search-clear", cx);
        window.render_frame(cx);
        assert!(window.find(beta_row.clone()).visible());
    })
    .unwrap();

    // 3. Verify Output and Errors bottom sub-tabs separate logs and clear independently.
    fixture.view.update(cx, |view, cx| {
        view.extensions.push_log("alpha stdout line");
        view.plugin_diagnostic(alpha.identity, "alpha stderr warning".into(), false, cx);
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("extensions-detail-output", cx);
        window.render_frame(cx);
        assert!(window.find("plugin-output-log").visible());
        window.click("extensions-detail-errors", cx);
        window.render_frame(cx);
        assert!(window.find("plugin-error-log").visible());
        window.click("plugin-clear-errors", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.extensions.error_logs.is_empty());
        assert!(!view.extensions.logs.is_empty());
    });
}

#[gpui_kit::test]
fn auto_reload_script_library_custom_scan_checks_and_api_export_work(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("1")], 64);
    let alpha = install_suite_plugin(&fixture, "lab.auto", "native", &["suite_panel"], cx);
    let alpha_id = alpha.identity.instance_id;
    drain(&mut fixture.commands);

    // 1. Auto-reload checkbox dispatches real PluginAction::SetAutoReload and handles outcome.
    let ar_checkbox = SharedString::from(format!("plugin-autoreload-{alpha_id}"));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(ar_checkbox, cx);
    })
    .unwrap();
    let ar_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::SetAutoReload {
                        instance_id,
                        enabled: true,
                    },
                ..
            } if instance_id == alpha_id => Some(operation_id),
            _ => None,
        })
        .expect("expected PluginAction::SetAutoReload");
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: ar_op,
            result: Ok(PluginOutcome::AutoReloadUpdated {
                instance_id: alpha_id,
                enabled: true,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.extensions.auto_reload.contains(&alpha_id));
        assert!(view.status.contains("Auto-reload"));
    });

    // 2. MioScript library CRUD + real compilation validation against temp library file.
    let tmp_dir = tempfile::tempdir().unwrap();
    let lib_path = tmp_dir.path().join("http-filter-scripts.json");
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.extensions.script_library.library_path = Some(lib_path.clone());
            view.extensions.script_library.loaded = false;
            view.extensions.page = ExtensionsPage::ScriptPresetLibrary;
            cx.notify();
        });
        window.render_frame(cx);
        fixture.view.update(cx, |view, cx| {
            view.extensions
                .script_library
                .name_input
                .update(cx, |inp, cx| inp.set_value("错误状态筛选", window, cx));
            view.extensions
                .script_library
                .code_editor
                .update(cx, |ed, cx| ed.set_value("response.status >= 400", window, cx));
            view.validate_script_library_editor(cx);
            assert!(
                view.extensions
                    .script_library
                    .feedback
                    .as_ref()
                    .is_some_and(|(ok, _)| *ok)
            );
            view.save_script_library_script(window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    assert!(lib_path.exists());
    cx.read(|cx| {
        let scripts = &fixture.view.read(cx).extensions.script_library.scripts;
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, "错误状态筛选");
    });

    // 3. Custom scan checks CRUD + workspace persistence + live eval_custom_scan_check_script.
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.extensions.page = ExtensionsPage::CustomScanChecks;
            view.extensions
                .custom_checks
                .name_input
                .update(cx, |inp, cx| inp.set_value("调试令牌检测", window, cx));
            view.extensions
                .custom_checks
                .description_input
                .update(cx, |inp, cx| inp.set_value("检测响应泄露调试令牌", window, cx));
            view.test_custom_scan_check(cx);
            let test_out = view
                .extensions
                .custom_checks
                .test_output
                .as_ref()
                .expect("test output populated");
            assert!(test_out.0);
            assert!(test_out.1.contains("X-Debug-Secret"));
            view.save_custom_scan_check(window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    let saved_ws = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("expected SaveWorkspace for custom scan check");
    assert_eq!(saved_ws.custom_scan_checks.len(), 1);
    assert_eq!(saved_ws.custom_scan_checks[0].name, "调试令牌检测");
    assert!(saved_ws.custom_scan_checks[0].enabled);

    // 4. APIs export payloads are valid JSON schema & Markdown docs.
    let api_json = super::super::render_suite::build_mio_plugin_api_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&api_json).unwrap();
    assert_eq!(parsed["name"], "mio-plugin-api");
    assert!(parsed["manifest_schema"]["properties"]["engine"].is_object());
    let api_md = super::super::render_suite::build_mio_plugin_api_markdown();
    assert!(api_md.contains("MIO Extensions API"));
    assert!(api_md.contains("traffic.read"));
}

#[gpui_kit::test]
fn persistent_plugins_ui_autoload_toggle_and_uninstall(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("1")], 64);
    drain(&mut fixture.commands);

    let plugin_id = "lab.persisted";
    let dir = PathBuf::from("/tmp/mio-persisted-profile/plugins/packages/lab.persisted");
    fixture
        .events
        .try_send(Event::InstalledPlugins {
            project: fixture.project,
            plugins_dir: PathBuf::from("/tmp/mio-persisted-profile/plugins"),
            extensions_disabled: false,
            plugins: vec![mio_core::InstalledPluginInfo {
                id: plugin_id.into(),
                version: "0.2.0".into(),
                engine: "native".into(),
                directory: dir.clone(),
                entry: PathBuf::from("bin/worker"),
                grants: vec![Permission::TrafficRead],
                contributions: vec!["suite_panel".into()],
                enabled: true,
                auto_reload: false,
            }],
        })
        .unwrap();
    cx.run_until_parked();

    let placeholder_id = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.extensions.instances.len(), 1);
        assert!(!view.extensions.instances[0].live);
        assert!(view.extensions.instances[0].persisted);
        view.extensions.instances[0].info.identity.instance_id
    });

    let loaded_info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: dir,
        manifest: suite_manifest(plugin_id, "native", &["suite_panel"]),
        grants: vec![Permission::TrafficRead],
        tab_id: None,
    };
    let live_id = loaded_info.identity.instance_id;
    fixture
        .events
        .try_send(Event::PluginAutoLoaded {
            project: fixture.project,
            info: loaded_info.clone(),
            panel: Arc::new(PanelSnapshot {
                revision: 1,
                title: "持久化扩展面板".into(),
                root: Some(Node::new(
                    1,
                    "状态",
                    node::Kind::Text(mio_core::panel::Text {
                        text: "ready".into(),
                        tone: 0,
                    }),
                )),
            }),
            auto_reload: false,
            order: vec![live_id],
        })
        .unwrap();
    cx.run_until_parked();

    let top_tab = SharedString::from(format!("tool-plugin-{}", live_id));
    let loaded_cb = SharedString::from(format!("plugin-loaded-{live_id}"));
    let live_row = SharedString::from(format!("plugin-instance-{live_id}"));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.render_frame(cx);
        assert!(window.find(top_tab.clone()).visible());
        assert!(window.find(live_row.clone()).visible());
        assert!(
            window
                .try_find(SharedString::from(format!("plugin-instance-{placeholder_id}")))
                .is_none()
        );
        window.click(loaded_cb.clone(), cx);
    })
    .unwrap();
    cx.run_until_parked();

    let disable_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action: PluginAction::Disable { instance_id },
                ..
            } if instance_id == live_id => Some(operation_id),
            _ => None,
        })
        .expect("unchecking Loaded must dispatch PluginAction::Disable");
    fixture
        .events
        .try_send(Event::PluginStopped {
            identity: loaded_info.identity,
            reason: "插件已禁用".into(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: disable_op,
            result: Ok(PluginOutcome::Disabled {
                identity: loaded_info.identity,
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.extensions.error_logs.is_empty(),
            "expected stop reason '插件已禁用' must not increment error logs: {:?}",
            view.extensions.error_logs
        );
        assert!(
            view.extensions
                .logs
                .iter()
                .any(|line| line.contains("插件已禁用"))
        );
        let inst = view
            .extensions
            .instance(live_id)
            .expect("disabled persisted plugin row remains visible");
        assert!(!inst.live);
        assert!(inst.persisted);
    });
    // Unexpected stop reasons must still surface in error_logs.
    fixture.view.update(cx, |view, cx| {
        view.extensions.instances[0].live = true;
        view.plugin_diagnostic(loaded_info.identity, "插件进程异常退出".into(), true, cx);
        assert_eq!(view.extensions.error_logs.len(), 1);
        view.extensions.error_logs.clear();
    });

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(loaded_cb, cx);
    })
    .unwrap();
    let enable_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action: PluginAction::EnableInstalled { plugin_id: pid },
                ..
            } if pid == plugin_id => Some(operation_id),
            _ => None,
        })
        .expect("re-checking Loaded on disabled persisted plugin must dispatch EnableInstalled");

    let mut reenabled_info = loaded_info;
    reenabled_info.identity.instance_id = Uuid::new_v4();
    let reenabled_id = reenabled_info.identity.instance_id;
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: enable_op,
            result: Ok(PluginOutcome::InstalledEnabled {
                info: reenabled_info.clone(),
                panel: Arc::new(PanelSnapshot {
                    revision: 1,
                    title: "持久化扩展面板".into(),
                    root: Some(Node::new(
                        1,
                        "状态",
                        node::Kind::Text(mio_core::panel::Text {
                            text: "enabled".into(),
                            tone: 0,
                        }),
                    )),
                }),
                auto_reload: false,
                order: vec![reenabled_id],
            }),
        })
        .unwrap();
    cx.run_until_parked();

    let reenabled_row = SharedString::from(format!("plugin-instance-{reenabled_id}"));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(reenabled_row.clone()).visible());
        window.click("plugin-uninstall", cx);
    })
    .unwrap();
    let uninstall_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::Uninstall {
                        plugin_id: pid,
                        instance_id,
                    },
                ..
            } if pid == plugin_id && instance_id == Some(reenabled_id) => Some(operation_id),
            _ => None,
        })
        .expect("clicking 移除 on persisted plugin must dispatch PluginAction::Uninstall");
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: uninstall_op,
            result: Ok(PluginOutcome::Uninstalled {
                plugin_id: plugin_id.into(),
                identity: Some(reenabled_info.identity),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(reenabled_row).is_none());
        assert!(window.try_find(top_tab).is_none());
    })
    .unwrap();
    cx.read(|cx| {
        assert!(fixture.view.read(cx).extensions.instances.is_empty());
    });
}

#[gpui_kit::test]
fn repeater_single_editor_per_plugin_and_extensions_hides_children(cx: &mut TestAppContext) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 64);
    let suite = install_suite_plugin(&fixture, "lab.signer", "native", &["suite_panel", "message_editor"], cx);
    let suite_id = suite.identity.instance_id;
    drain(&mut fixture.commands);

    let add_btn = SharedString::from(format!("plugin-add-editor-{suite_id}"));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert!(window.find(add_btn.clone()).visible());
        // Click once, and try clicking again while in-flight.
        window.click(add_btn.clone(), cx);
        window.render_frame(cx);
        assert_eq!(window.find(add_btn.clone()).disabled(), Some(true));
        window.click(add_btn.clone(), cx);
    })
    .unwrap();

    let create_ops: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .filter_map(|c| match c {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::CreateEditor {
                        source_instance,
                        tab_id,
                        ..
                    },
                ..
            } if source_instance == suite_id && tab_id == tab.id => Some(operation_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        create_ops.len(),
        1,
        "at most one in-flight CreateEditor is allowed per (tab, plugin)"
    );

    let child_id = Uuid::new_v4();
    let child_info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: child_id,
            project: fixture.project,
        },
        directory: suite.directory.clone(),
        manifest: suite.manifest.clone(),
        grants: suite.grants.clone(),
        tab_id: Some(tab.id),
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: create_ops[0],
            result: Ok(PluginOutcome::Loaded {
                info: child_info,
                panel: Arc::new(PanelSnapshot {
                    revision: 1,
                    title: "签名编辑页".into(),
                    root: Some(Node::new(
                        10,
                        "请求",
                        node::Kind::Editor(Editor {
                            document_id: 401,
                            revision: 1,
                            contents: tab.draft.raw.clone(),
                            format: EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    )),
                }),
            }),
        })
        .unwrap();
    cx.run_until_parked();

    let child_tab_btn = SharedString::from(format!("plugin-message-{child_id}"));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(child_tab_btn.clone()).visible());
        assert!(
            window.try_find(add_btn.clone()).is_none(),
            "add-editor button must disappear once plugin editor is open on this tab"
        );
        // Programmatic second trigger switches to existing child rather than spawning a second tab.
        window.click("plugin-message-native", cx);
        fixture.view.update(cx, |view, cx| {
            view.create_plugin_editor(suite_id, None, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "second create_plugin_editor call must switch to existing instance without spawning another"
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).extensions.active_messages.get(&tab.id).copied(),
            Some(child_id)
        );
    });

    // Extensions table shows only top-level suite instance, never internal Repeater child editor rows.
    let suite_row = SharedString::from(format!("plugin-instance-{suite_id}"));
    let child_row = SharedString::from(format!("plugin-instance-{child_id}"));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.render_frame(cx);
        assert!(window.find(suite_row).visible());
        assert!(
            window.try_find(child_row).is_none(),
            "Extensions table must not display internal Repeater child instances"
        );
    })
    .unwrap();
}
