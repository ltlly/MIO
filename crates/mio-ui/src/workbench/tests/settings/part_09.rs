use super::*;

#[gpui_kit::test]
fn settings_only_temporary_project_requires_confirmation_for_quit_close_and_new(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let workspace = saved(&mut fixture.commands);
    acknowledge(&fixture, workspace.clone(), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none() && view.workspace.tabs.is_empty());
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.workspace.repeater_settings.unpack_compressed_responses);
    });
    acknowledged_temporary_settings_require_confirmation(&mut fixture, settings, workspace, cx);
}

#[gpui_kit::test]
fn settings_only_temporary_proxy_websocket_requires_confirmation_for_quit_close_and_new(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    proxy_ws_show_controls(settings, cx);
    proxy_rule_click(settings, "proxy-ws-intercept-in-scope-only", cx);
    let workspace = saved(&mut fixture.commands);
    let mut expected = original;
    expected.revision += 1;
    expected.tab_sidebar_width = cx.read(|cx| fixture.view.read(cx).tab_width as u16);
    expected
        .proxy_websocket_interception
        .intercept_in_scope_only = true;
    assert_eq!(workspace, expected);
    assert!(!workspace.proxy_websocket_interception.is_default());
    acknowledge(&fixture, workspace.clone(), cx);
    acknowledged_temporary_settings_require_confirmation(&mut fixture, settings, workspace, cx);
}

#[gpui_kit::test]
fn settings_only_temporary_proxy_interception_requires_confirmation_for_quit_close_and_new(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    proxy_rule_click(settings, "proxy-request-rules-enabled", cx);
    let workspace = saved(&mut fixture.commands);
    let mut expected = original;
    expected.revision += 1;
    expected.tab_sidebar_width = cx.read(|cx| fixture.view.read(cx).tab_width as u16);
    expected.proxy_interception.request.enabled = false;
    assert_eq!(workspace, expected);
    assert!(!workspace.proxy_interception.is_default());
    assert!(workspace.proxy_messages.is_default());
    assert_eq!(workspace.repeater_settings, RepeaterSettings::default());
    acknowledge(&fixture, workspace.clone(), cx);
    acknowledged_temporary_settings_require_confirmation(&mut fixture, settings, workspace, cx);
}

#[gpui_kit::test]
fn settings_only_temporary_proxy_messages_requires_confirmation_for_quit_close_and_new(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    proxy_message_show_controls(settings, cx);
    proxy_rule_click(settings, "proxy-unpack-requests", cx);
    let workspace = saved(&mut fixture.commands);
    let mut expected = original;
    expected.revision += 1;
    expected.tab_sidebar_width = cx.read(|cx| fixture.view.read(cx).tab_width as u16);
    expected.proxy_messages.unpack_compressed_requests = true;
    assert_eq!(workspace, expected);
    assert!(workspace.proxy_interception.is_default());
    assert!(!workspace.proxy_messages.is_default());
    assert_eq!(workspace.repeater_settings, RepeaterSettings::default());
    acknowledge(&fixture, workspace.clone(), cx);
    acknowledged_temporary_settings_require_confirmation(&mut fixture, settings, workspace, cx);
}

fn acknowledged_temporary_settings_require_confirmation(
    fixture: &mut Fixture,
    settings: AnyWindowHandle,
    workspace: Workspace,
    cx: &mut TestAppContext,
) {
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none());
        assert!(view.workspace.tabs.is_empty() && view.workspace.closed.is_empty());
        assert_eq!(view.workspace, workspace);
        assert!(!view.has_traffic && view.pending_records == 0);
        assert!(!view.workspace_dirty);
        assert!(view.pending_workspace.is_none() && view.pending_proxy_interception.is_none());
        assert!(view.transition.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
    menu_action(settings, "MIO", "退出 MIO", cx);
    assert!(
        cx.has_pending_prompt(),
        "acknowledged settings still belong to a temporary project that would be deleted on exit"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    cx.read(|cx| assert!(!fixture.view.read(cx).shutdown.is_cancelled()));
    cx.simulate_keystrokes(settings, crate::kbd!("w"));
    let mut native = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
    assert!(!native.simulate_close());
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "window close must protect settings too"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    menu_action(fixture.window, "项目", "新建项目", cx);
    assert!(
        cx.has_pending_prompt(),
        "project replacement must protect settings too"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.transition.is_none());
        assert!(!view.shutdown.is_cancelled() && !view.quitting);
    });
}

#[gpui_kit::test]
fn settings_only_temporary_project_save_transition_preserves_settings(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let workspace = saved(&mut fixture.commands);
    acknowledge(&fixture, workspace.clone(), cx);
    menu_action(settings, "MIO", "退出 MIO", cx);
    assert!(cx.has_pending_prompt(), "settings require a save decision");
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { project }] if *project == fixture.project));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    pump_native(cx);
    assert!(cx.did_prompt_for_new_path());
    let destination =
        std::env::temp_dir().join(format!("mio-settings-only-{}.mio", Uuid::new_v4()));
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    pump_native(cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: Some(path),
            workspace: None,
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must back up the already acknowledged project without another CAS");
    };
    assert_eq!((*project, path), (fixture.project, &destination));
    assert!(workspace.tabs.is_empty() && workspace.closed.is_empty());
    assert!(!workspace.repeater_settings.unpack_compressed_responses);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.pending_workspace.is_none() && !view.workspace_dirty);
        assert!(!view.shutdown.is_cancelled());
    });
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: destination,
            total_bytes: 4096,
        })
        .unwrap();
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested && view.shutdown.is_cancelled());
        assert!(!view.quitting, "exit still awaits the real Runtime join");
    });
}

#[gpui_kit::test]
fn settings_keyboard_notifications_update_the_existing_checkbox(cx: &mut TestAppContext) {
    use gpui_kit::{KeyDownEvent, KeyUpEvent, Keystroke};

    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    checkbox_checked(settings, "repeater-update-content-length", true, cx);
    checked(settings, true, cx);
    for (key, expected, focused) in [
        ("tab", None, "settings-proxy"),
        ("tab", None, "settings-intruder"),
        ("tab", None, "settings-repeater"),
        ("tab", None, "settings-project-tasks"),
        ("tab", None, "repeater-update-content-length"),
        ("space", Some(false), "repeater-update-content-length"),
        ("space", Some(true), "repeater-update-content-length"),
        ("tab", None, "repeater-unpack-responses"),
        ("space", Some(false), "repeater-unpack-responses"),
        ("space", Some(true), "repeater-unpack-responses"),
    ] {
        cx.update_window(settings, |_, window, cx| {
            let keystroke = Keystroke::parse(key).unwrap();
            window.dispatch_event(
                KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(settings, |_, window, _| {
            assert_eq!(window.find(focused).focused(), Some(true));
        })
        .unwrap();
        let Some(expected) = expected else {
            assert!(drain(&mut fixture.commands).is_empty());
            continue;
        };
        let workspace = saved(&mut fixture.commands);
        let actual = if focused == "repeater-update-content-length" {
            workspace.repeater_settings.update_content_length
        } else {
            workspace.repeater_settings.unpack_compressed_responses
        };
        assert_eq!(actual, expected);
        cx.update_window(settings, |_, window, _| {
            // Observe the completed frame without asking the test helper to redraw it.
            assert_eq!(window.find(focused).checked(), Some(expected));
        })
        .unwrap();
        acknowledge(&fixture, workspace, cx);
    }
}

#[gpui_kit::test]
fn settings_navigation_and_radio_keyboard_keep_accessible_selection_and_ack_semantics(
    cx: &mut TestAppContext,
) {
    use mio_core::{FinishedAttackClose, RunningAttackClose, UserPreferences};

    let mut fixture = fixture(cx, vec![tab("导航保留草稿")], 64);
    drain(&mut fixture.commands);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let settings = open_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-intruder").selected(), Some(false));
        assert_eq!(window.find("settings-repeater").selected(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-proxy").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-intruder").focused(), Some(true));
        window.press("enter", cx);
        assert_eq!(window.find("settings-intruder").selected(), Some(true));
        assert_eq!(window.find("settings-repeater").selected(), Some(false));
        window.press("tab", cx);
        assert_eq!(window.find("settings-repeater").focused(), Some(true));
        window.press("space", cx);
        assert_eq!(window.find("settings-repeater").selected(), Some(true));
        assert!(window.find("repeater-unpack-responses").visible());
        window.click("settings-intruder", cx);
        assert_eq!(window.find("settings-intruder").selected(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-repeater").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-project-tasks").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(
            window.find("intruder-closing-defaults").focused(),
            Some(true)
        );
        window.press("tab", cx);
        assert_eq!(
            window.find("intruder-running-continue").focused(),
            Some(true)
        );
        let radio = window.find("intruder-running-continue");
        assert_eq!(radio.role(), Some(gpui_kit::accesskit::Role::RadioButton));
        assert_eq!(radio.checked(), Some(false));
        assert_eq!(radio.selected(), Some(false));
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserPreferences {
            operation_id,
            expected_revision,
            running,
            finished,
        },
    ] = commands.as_slice()
    else {
        panic!("keyboard radio activation emits one user setting change")
    };
    assert_eq!(
        (*expected_revision, *running, *finished),
        (0, RunningAttackClose::Continue, FinishedAttackClose::Ask)
    );
    cx.read(|cx| {
        assert_eq!(fixture.view.read(cx).workspace, original);
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (RunningAttackClose::Ask, FinishedAttackClose::Ask)
        );
    });
    fixture
        .events
        .try_send(Event::UserPreferencesSaved {
            operation_id: *operation_id,
            result: Ok(UserPreferences {
                version: 1,
                revision: 1,
                running: *running,
                finished: *finished,
                ..UserPreferences::default()
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for (id, checked) in [
            ("intruder-running-continue", true),
            ("intruder-running-delete", false),
            ("intruder-running-ask", false),
        ] {
            let radio = window.find(id);
            assert_eq!(radio.role(), Some(gpui_kit::accesskit::Role::RadioButton));
            assert_eq!(radio.checked(), Some(checked));
            assert_eq!(radio.selected(), Some(checked));
        }
        assert_eq!(window.find("intruder-finished-ask").checked(), Some(true));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}
