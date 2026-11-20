use super::*;

fn v13_manifest(id: &str) -> PluginManifest {
    PluginManifest {
        id: id.into(),
        version: "1.3.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: PathBuf::from("worker"),
        permissions: vec![
            Permission::TrafficRead,
            Permission::DraftWrite,
            Permission::RepeaterOpen,
            Permission::Storage,
        ],
        contributions: vec![
            "suite_panel".into(),
            "message_editor".into(),
            "message_response_editor".into(),
            "context_menu".into(),
        ],
    }
}

fn install_v13_suite(fixture: &Fixture, id: &str, cx: &mut TestAppContext) -> LoadedPlugin {
    let info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: PathBuf::from(format!("/tmp/mio-v13-suite-{id}")),
        manifest: v13_manifest(id),
        grants: vec![
            Permission::TrafficRead,
            Permission::DraftWrite,
            Permission::RepeaterOpen,
            Permission::Storage,
        ],
        tab_id: None,
    };
    let snapshot = Arc::new(PanelSnapshot {
        revision: 1,
        title: format!("{id} v1.3"),
        root: Some(Node::new(
            1,
            "状态",
            node::Kind::Text(mio_core::panel::Text {
                text: format!("{id} 运行中"),
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
fn reload_button_retains_old_instance_mounts_new_panel_and_appends_log(cx: &mut TestAppContext) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 64);
    let old_info = install_v13_suite(&fixture, "lab.protocol", cx);
    drain(&mut fixture.commands);

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.plugin_response_editor_badge(tab.id).as_deref(),
            Some("插件响应视图可用（lab.protocol）")
        );
    });

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("plugin-reload", cx);
    })
    .unwrap();

    let reload_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Plugin {
                operation_id,
                action: PluginAction::Reload { instance_id },
                ..
            } if instance_id == old_info.identity.instance_id => Some(operation_id),
            _ => None,
        })
        .expect("clicking plugin-reload must dispatch PluginAction::Reload");

    let new_info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: old_info.directory.clone(),
        manifest: old_info.manifest.clone(),
        grants: old_info.grants.clone(),
        tab_id: None,
    };
    let new_panel = Arc::new(PanelSnapshot {
        revision: 2,
        title: "lab.protocol (reloaded)".into(),
        root: Some(Node::new(
            1,
            "新状态",
            node::Kind::Text(mio_core::panel::Text {
                text: "热重载后新面板就绪".into(),
                tone: 0,
            }),
        )),
    });
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: reload_op,
            result: Ok(PluginOutcome::Reloaded {
                old_instance: old_info.identity.instance_id,
                info: new_info.clone(),
                panel: new_panel,
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let old_inst = view
            .extensions
            .instance(old_info.identity.instance_id)
            .expect("old instance retained");
        assert!(!old_inst.live);
        assert!(old_inst.status.contains("已热重载"));

        let new_inst = view
            .extensions
            .instance(new_info.identity.instance_id)
            .expect("new instance installed");
        assert!(new_inst.live);
        assert_eq!(new_inst.panel.read(cx).title(), "lab.protocol (reloaded)");
        assert_eq!(
            view.extensions.selected,
            Some(new_info.identity.instance_id)
        );
        assert!(
            view.extensions
                .logs
                .iter()
                .any(|line| line.contains("已热重载插件：lab.protocol"))
        );
    });
}

#[gpui_kit::test]
fn invoke_context_menu_updates_logs_status_and_opens_repeater_tab(cx: &mut TestAppContext) {
    use mio_core::{Header, HttpRequest, HttpVersion};

    let initial = tab("1");
    let mut fixture = fixture(cx, vec![initial.clone()], 64);
    let info = install_v13_suite(&fixture, "lab.protocol", cx);
    drain(&mut fixture.commands);

    assert_eq!(
        cx.read(|cx| fixture.view.read(cx).active_context_menu_plugins()),
        vec![(info.identity.instance_id, "lab.protocol".into())]
    );
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            let req_state = view.tab_editors[&initial.id].request.state().clone();
            req_state.update(cx, |state, cx| {
                state.set_selected_range_with_direction(0..3, false, cx);
            });
        });
        window.render_frame(cx);
        window.click("plugin-context-menu", cx);
    })
    .unwrap();

    let menu_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::InvokeContextMenu {
                        instance_id,
                        action_id,
                        selection_start,
                        selection_end,
                        selected_bytes,
                        ..
                    },
                ..
            } if instance_id == info.identity.instance_id => {
                assert_eq!(action_id, "default");
                assert_eq!((selection_start, selection_end), (0, 3));
                assert_eq!(selected_bytes, b"GET".to_vec());
                Some(operation_id)
            }
            _ => None,
        })
        .expect("clicking plugin-context-menu must dispatch PluginAction::InvokeContextMenu");

    let proposed_req = HttpRequest {
        method: "POST".into(),
        url: "http://127.0.0.1:9099/ctx-menu".into(),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "X-Protocol-Lab-Context".into(),
            value: b"menu-v1".to_vec(),
        }],
        body: b"context-body".to_vec(),
        trailers: vec![],
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: menu_op,
            result: Ok(PluginOutcome::ContextMenuInvoked {
                identity: info.identity,
                status_message: "已由插件上下文菜单生成 Repeater 草稿".into(),
                log_line: "lab.protocol 上下文菜单：POST http://127.0.0.1:9099/ctx-menu".into(),
                open_repeater: Some(proposed_req),
                replace_request: None,
            }),
        })
        .unwrap();
    cx.run_until_parked();

    let saved_ws = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("opening Repeater tab from context menu must persist workspace");
    assert_eq!(saved_ws.tabs.len(), 2);
    assert_eq!(
        saved_ws.tabs[1].draft.target,
        "http://127.0.0.1:9099/ctx-menu"
    );

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.tool, Tool::Repeater);
        assert_eq!(view.status, "已由插件上下文菜单生成 Repeater 草稿");
        assert_eq!(view.workspace.tabs.len(), 2);
        let new_tab = &view.workspace.tabs[1];
        assert_eq!(view.workspace.active, Some(new_tab.id));
        assert!(
            String::from_utf8_lossy(&new_tab.draft.raw).contains("X-Protocol-Lab-Context: menu-v1")
        );
        assert!(
            view.extensions
                .logs
                .iter()
                .any(|line| line.contains("lab.protocol 上下文菜单"))
        );
    });
}

#[gpui_kit::test]
fn repeater_response_viewer_open_render_and_switch_back(cx: &mut TestAppContext) {
    let initial = tab("1");
    let mut fixture = fixture(cx, vec![initial.clone()], 64);
    let suite = install_v13_suite(&fixture, "lab.protocol", cx);
    drain(&mut fixture.commands);

    let raw_resp =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\n\r\naGVsbG8tdjEz";
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tool = Tool::Repeater;
            view.tab_editors
                .get_mut(&initial.id)
                .unwrap()
                .response
                .load(raw_resp, window, cx);
        });
        window.render_frame(cx);
        let btn = SharedString::from(format!(
            "plugin-response-tab-{}",
            suite.identity.instance_id
        ));
        window.click(btn, cx);
    })
    .unwrap();

    let (open_op, context_id, revision, opened_resp) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::OpenResponseViewer {
                        source_instance,
                        context_id,
                        revision,
                        read_only: true,
                        response,
                    },
                ..
            } if source_instance == suite.identity.instance_id => {
                Some((operation_id, context_id, revision, response))
            }
            _ => None,
        })
        .expect("clicking plugin-response-tab must dispatch OpenResponseViewer");
    assert_eq!(opened_resp.status, 200);
    assert_eq!(opened_resp.body, b"aGVsbG8tdjEz");

    let viewer_info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: suite.directory.clone(),
        manifest: suite.manifest.clone(),
        grants: suite.grants.clone(),
        tab_id: None,
    };
    let resp_panel = Arc::new(PanelSnapshot {
        revision: 1,
        title: "lab.protocol 响应视图".into(),
        root: Some(Node::new(
            201,
            "Decoded",
            node::Kind::Text(mio_core::panel::Text {
                text: "hello-v13".into(),
                tone: 0,
            }),
        )),
    });
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: open_op,
            result: Ok(PluginOutcome::ResponseOpened {
                info: viewer_info.clone(),
                context_id,
                revision,
                panel: resp_panel,
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.extensions.active_responses.get(&initial.id).copied(),
            Some(viewer_info.identity.instance_id)
        );
        assert_eq!(
            view.plugin_response_editor_badge(initial.id).as_deref(),
            Some("插件响应视图就绪（lab.protocol）")
        );
    });

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("plugin-response-native", cx);
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.extensions.active_responses.contains_key(&initial.id));
        assert_eq!(
            view.plugin_response_editor_badge(initial.id).as_deref(),
            Some("插件响应视图可用（lab.protocol）")
        );
    });
}

#[gpui_kit::test]
fn top_level_plugin_suite_tab_mounts_selects_with_orange_underline_and_cleans_up_on_unload(
    cx: &mut TestAppContext,
) {
    use gpui_kit::{Bounds, point, size};

    let initial = tab("1");
    let mut fixture = fixture(cx, vec![initial], 64);
    let suite = install_v13_suite(&fixture, "lab.protocol", cx);
    let instance_id = suite.identity.instance_id;
    let top_tab_id = SharedString::from(format!("tool-plugin-{instance_id}"));
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let ext_bounds = window.find("tool-extensions").bounds();
        let plugin_tab = window.find(top_tab_id.clone());
        let plugin_bounds = plugin_tab.bounds();
        assert!(plugin_tab.visible());
        assert_eq!(plugin_tab.role(), Some(gpui_kit::accesskit::Role::Tab));
        assert_eq!(plugin_tab.label(), Some("lab.protocol v1.3"));
        assert_eq!(plugin_bounds.origin.y, ext_bounds.origin.y);
        assert_eq!(plugin_bounds.size.height, px(42.));
        assert!(plugin_bounds.origin.x >= ext_bounds.right());
        assert_eq!(plugin_tab.selected(), Some(false));

        // Click top-level plugin suite tab: selects with one 3pt orange underline and mounts full panel view.
        window.click(top_tab_id.clone(), cx);
        assert_eq!(
            fixture.view.read(cx).extensions.active_top_suite_tab,
            Some(instance_id)
        );
        assert_eq!(window.find(top_tab_id.clone()).selected(), Some(true));
        assert_eq!(window.find("tool-extensions").selected(), Some(false));
        assert!(window.find("plugin-top-suite-view").visible());
        assert!(window.try_find("plugin-instance-list").is_none());

        let band_top = window.find("mio-workspace").bounds().origin.y;
        let scale = window.scale_factor();
        let selected = window.find(top_tab_id.clone()).bounds();
        let underlines: Vec<_> = window
            .painted_quads()
            .into_iter()
            .filter(|quad| quad.background == rgb(0xdb612f).into())
            .map(|quad| {
                quad.bounds
                    .intersect(&quad.content_mask.bounds)
                    .map(|value| px(value.0 / scale))
            })
            .filter(|bounds| bounds.origin.y >= band_top && bounds.bottom() <= band_top + px(42.))
            .collect();
        assert_eq!(
            underlines,
            vec![Bounds {
                origin: point(selected.origin.x, band_top + px(38.)),
                size: size(selected.size.width, px(3.)),
            }]
        );

        // Switching back to built-in Proxy or Extensions clears active_top_suite_tab.
        window.click("tool-proxy", cx);
        assert_eq!(fixture.view.read(cx).extensions.active_top_suite_tab, None);
        assert_eq!(fixture.view.read(cx).tool, Tool::Proxy);
        assert_eq!(window.find(top_tab_id.clone()).selected(), Some(false));
        assert_eq!(window.find("tool-proxy").selected(), Some(true));

        // Select top suite tab again, then switch to Extensions management tab and disable/unload.
        window.click(top_tab_id.clone(), cx);
        assert_eq!(
            fixture.view.read(cx).extensions.active_top_suite_tab,
            Some(instance_id)
        );
        window.click("tool-extensions", cx);
        assert_eq!(fixture.view.read(cx).extensions.active_top_suite_tab, None);
        assert!(window.find("plugin-instance-list").visible());

        // Re-select top suite tab, then disable plugin programmatically to verify automatic cleanup.
        window.click(top_tab_id.clone(), cx);
        assert!(window.find("plugin-top-suite-view").visible());
        fixture.view.update(cx, |view, cx| {
            view.disable_plugin(instance_id, cx);
        });
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).extensions.active_top_suite_tab, None);
        assert!(window.try_find(top_tab_id).is_none());
        assert!(window.find("plugin-instance-list").visible());
    })
    .unwrap();
}
