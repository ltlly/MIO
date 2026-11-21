use super::*;

#[gpui_kit::test]
fn old_workspace_rejection_does_not_clear_a_newer_save_before_transition(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("迟到的源保存错误")], 64);
    drain(&mut fixture.commands);
    let stored = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    let rejected = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { .. }]
    ));
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: rejected.revision,
            stored,
            message: "先前的源保存失败".into(),
        })
        .unwrap();
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert!(fixture.view.read(cx).after_save.is_some()));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    pump_native(cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Resume { .. }]
    ));
}

#[gpui_kit::test]
fn terminal_send_replaces_the_waiting_for_save_status(cx: &mut TestAppContext) {
    let first = tab("最终发送状态");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    let saved = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(matches!(commands.as_slice(), [Command::Send { .. }]));
    let mut terminal = record("http://127.0.0.1:9001/", "完成");
    terminal.summary.state = FlowState::Complete;
    terminal.summary.id = commands
        .iter()
        .find_map(|command| match command {
            Command::Send { run_id, .. } => Some(*run_id),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id: id,
            draft_revision: 1,
            record: Box::new(terminal),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.status.contains("完成"));
        assert!(!view.status.contains("保存成功后发送"));
    });
}

#[gpui_kit::test]
fn storage_error_is_persistent_and_explicit_discard_quit_ignores_queue_backpressure(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 1);
    let shutdown = cx.read(|cx| fixture.view.read(cx).shutdown.clone());
    fixture
        .events
        .try_send(Event::StorageState {
            project: fixture.project,
            pending_records: 1,
            message: Some("测试磁盘不可写".into()),
        })
        .unwrap();
    pump_native(cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.click("history-filter", cx);
        assert_eq!(
            fixture.view.read(cx).storage_error.as_deref(),
            Some("测试磁盘不可写")
        );
        assert!(window.try_find_by_label("保存重试").is_some());
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "quit remains reachable while the normal command queue is full"
    );
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    assert!(shutdown.is_cancelled());
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::QueryHttpHistory { .. }
    ));
    assert!(fixture.commands.try_recv().is_err());
}

#[gpui_kit::test]
fn group_menu_creation_cross_group_drop_and_collapse_use_real_pointer_events(
    cx: &mut TestAppContext,
) {
    let first = tab("1");
    let first_id = first.id;
    let second = tab("2");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first, second], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.right_click(SharedString::from(format!("work-tab-{first_id}")), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(acknowledge_history_query(&fixture, commands, cx).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let submenu = window.find_by_label("添加到组").bounds().center();
        window.pointer_move(submenu, None, cx);
        let create = window.find_by_label("新建组").bounds().center();
        window.pointer_down(create, MouseButton::Left, cx);
        window.pointer_up(create, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = cx.read(|cx| fixture.view.read(cx).tab_group_dialog.unwrap());
    dialog
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("a"), cx);
            window.input("中文分组", cx);
            window.click("group-submit", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("group dialog must save exactly one workspace")
    };
    assert_eq!(*project, fixture.project);
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let group_id = fixture.view.read(cx).workspace.groups[0].id;
        window.drag_to(
            SharedString::from(format!("work-tab-{second_id}")),
            SharedString::from(format!("group-{group_id}")),
            cx,
        );
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.groups[0].title, "中文分组");
        assert!(
            view.workspace
                .tabs
                .iter()
                .all(|tab| tab.group == Some(group_id))
        );
        window.click(SharedString::from(format!("group-{group_id}")), cx);
        assert!(fixture.view.read(cx).workspace.groups[0].collapsed);
        window.click(SharedString::from(format!("group-{group_id}")), cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).workspace.orientation,
            Orientation::Vertical
        );
        assert!(!fixture.view.read(cx).workspace.groups[0].collapsed);
    });
}

#[gpui_kit::test]
fn escape_cancels_pointer_drag_before_reordering(cx: &mut TestAppContext) {
    let first = tab("1");
    let first_id = first.id;
    let second = tab("2");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first, second], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        let start = window
            .find(SharedString::from(format!("work-tab-{second_id}")))
            .bounds()
            .center();
        let target = window
            .find(SharedString::from(format!("work-tab-{first_id}")))
            .bounds()
            .center();
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseMoveEvent {
                position: start + point(px(12.), px(0.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseMoveEvent {
                position: target,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        assert!(cx.has_active_drag());
        assert_eq!(fixture.view.read(cx).drop_target, Some(first_id));
        window.press("escape", cx);
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: target,
                modifiers: Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        assert!(!cx.has_active_drag());
        assert_eq!(fixture.view.read(cx).workspace.tabs[0].id, first_id);
    })
    .unwrap();
}

fn assert_cmd_r_does_not_export_hidden_http_selection(
    destination: &'static str,
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("保留的 Repeater 草稿")], 64);
    let original_workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let query_revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    let selected = record("http://127.0.0.1:9001/hidden-http", "隐藏的 HTTP 请求");
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![selected.summary.clone().into()],
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.click_at("table", point(px(90.), px(theme::ROW_HEIGHT * 1.5)), cx);
    })
    .unwrap();
    let selection_revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if flow_id == selected.summary.id => Some(selection_revision),
            _ => None,
        })
        .expect("the actual HTTP row click must select the old request");
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision,
            record: Box::new(selected.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).selected_flow,
            Some(selected.summary.id)
        );
        window.click(destination, cx);
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.press(crate::kbd!("r"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "Cmd-R in {destination} must not export the hidden HTTP selection or write a workspace"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, original_workspace);
        assert!(view.pending_workspace.is_none());
        if destination == "tool-dashboard" {
            assert!(view.tool == Tool::Dashboard);
        } else {
            assert!(view.tool == Tool::Proxy);
            assert!(view.proxy_page == ProxyPage::WebSocketHistory);
        }
    });
}

#[gpui_kit::test]
fn dashboard_cmd_r_does_not_export_hidden_http_selection(cx: &mut TestAppContext) {
    assert_cmd_r_does_not_export_hidden_http_selection("tool-dashboard", cx);
}

#[gpui_kit::test]
fn websocket_cmd_r_does_not_export_hidden_http_selection(cx: &mut TestAppContext) {
    assert_cmd_r_does_not_export_hidden_http_selection("proxy-websocket-history", cx);
}
