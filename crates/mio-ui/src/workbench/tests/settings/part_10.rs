use super::*;

#[gpui_kit::test]
fn pending_radio_save_keeps_settings_keyboard_route_and_survives_escape(cx: &mut TestAppContext) {
    use mio_core::{FinishedAttackClose, RunningAttackClose, UserPreferences};

    let mut fixture = fixture(cx, vec![tab("等待偏好保存时保留项目")], 64);
    drain(&mut fixture.commands);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let settings = open_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-intruder", cx);
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
        panic!("Space must enqueue exactly one preference save")
    };
    assert_eq!(
        (*expected_revision, *running, *finished),
        (0, RunningAttackClose::Continue, FinishedAttackClose::Ask)
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.user_preferences.busy());
        assert_eq!(view.user_preferences.running, RunningAttackClose::Continue);
        assert_eq!(
            view.intruder_close_preferences(),
            (RunningAttackClose::Ask, FinishedAttackClose::Ask)
        );
        assert_eq!(view.workspace, original);
    });
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.is_action_available(&crate::CloseSettings, cx),
            "disabling the focused radio while awaiting ACK must retain the Settings Escape route"
        );
        window.press("tab", cx);
        assert!(
            window.find("settings-proxy").focused() == Some(true),
            "Tab must leave the disabled option for an enabled navigation item"
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(settings.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.user_preferences.busy(),
            "closing Settings does not cancel the pending disk operation"
        );
        assert_eq!(view.workspace, original);
    });
    assert!(drain(&mut fixture.commands).is_empty());
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
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.user_preferences.busy());
        assert_eq!(
            view.intruder_close_preferences(),
            (RunningAttackClose::Continue, FinishedAttackClose::Ask)
        );
        assert_eq!(view.workspace, original);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn settings_only_temporary_project_restored_to_defaults_stays_empty(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    for expected in [false, true] {
        toggle(settings, expected, cx);
        let workspace = saved(&mut fixture.commands);
        acknowledge(&fixture, workspace, cx);
    }
    menu_action(settings, "MIO", "退出 MIO", cx);
    assert!(
        !cx.has_pending_prompt(),
        "default settings leave an empty temporary project"
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.repeater_settings,
            RepeaterSettings::default()
        );
        assert!(view.shutdown_requested && view.shutdown.is_cancelled());
        assert!(!view.quitting);
    });
}

#[gpui_kit::test]
fn settings_only_temporary_proxy_defaults_do_not_prompt_when_replacing_or_closing_project(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for _ in 0..2 {
        proxy_rule_click(settings, "proxy-request-rules-enabled", cx);
        let workspace = saved(&mut fixture.commands);
        acknowledge(&fixture, workspace, cx);
    }
    proxy_message_show_controls(settings, cx);
    for _ in 0..2 {
        proxy_rule_click(settings, "proxy-unpack-requests", cx);
        let workspace = saved(&mut fixture.commands);
        acknowledge(&fixture, workspace, cx);
    }
    for _ in 0..2 {
        proxy_rule_click(settings, "proxy-ws-intercept-in-scope-only", cx);
        let workspace = saved(&mut fixture.commands);
        acknowledge(&fixture, workspace, cx);
    }
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none() && !view.has_traffic);
        assert!(view.workspace.proxy_interception.is_default());
        assert!(view.workspace.proxy_messages.is_default());
        assert!(view.workspace.proxy_websocket_interception.is_default());
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
    });
    cx.simulate_keystrokes(settings, crate::kbd!("w"));
    cx.run_until_parked();
    menu_action(fixture.window, "项目", "新建项目", cx);
    assert!(
        !cx.has_pending_prompt(),
        "restored Proxy defaults must not require a save decision for project replacement"
    );
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::NewProject { path: None }]
    ));
    fixture.project = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryHttpHistory { .. }))
    );
    let mut native = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
    assert!(!native.simulate_close());
    pump_native(cx);
    assert!(
        !cx.has_pending_prompt(),
        "empty defaults must not require a save decision for window close"
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested && view.shutdown.is_cancelled());
        assert!(!view.quitting, "window close still waits for Runtime join");
    });
}

#[gpui_kit::test]
fn settings_only_clean_ack_ordinary_save_save_as_and_reopen(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let stored = saved(&mut fixture.commands);
    acknowledge(&fixture, stored.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let assert_clean = |fixture: &Fixture, path: Option<&PathBuf>, cx: &mut TestAppContext| {
        checked(settings, false, cx);
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.project, Some(fixture.project));
            assert_eq!(view.workspace, stored);
            assert!(view.workspace.tabs.is_empty() && view.workspace.closed.is_empty());
            assert!(view.tab_editors.is_empty());
            assert_eq!(view.project_path.as_ref(), path);
            assert!(!view.workspace_dirty && view.pending_workspace.is_none());
            assert!(!view.saving_project_snapshot && !view.choosing_save_as);
            assert!(view.pending_project_save.is_none());
            assert_eq!(view.project_saves_in_flight, 0);
            assert!(view.transition.is_none() && view.after_save.is_none());
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
            assert!(!view.shutdown.is_cancelled());
        });
    };
    assert_clean(&fixture, None, cx);
    let source = std::env::temp_dir().join(format!("mio-settings-clean-{}.mio", Uuid::new_v4()));
    let copy = source.with_extension("copy.mio");

    menu_action(settings, "项目", "保存项目", cx);
    assert!(cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_new_path_selection(|_| Some(source.clone()));
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: Some(path), workspace: None }]
            if *project == fixture.project && path == &source));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none());
        assert_eq!(view.project_saves_in_flight, 1);
    });
    acknowledge_project_saved(&fixture, source.clone(), cx);
    assert_clean(&fixture, Some(&source), cx);

    menu_action(settings, "项目", "保存项目", cx);
    assert!(!cx.did_prompt_for_new_path());
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
            if *project == fixture.project));
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "save_project",
            message: "设置 checkpoint 失败".into(),
        })
        .unwrap();
    cx.run_until_parked();
    visible_error(settings, "设置 checkpoint 失败", cx);
    assert_clean(&fixture, Some(&source), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(rendered_status(window).contains("项目保存失败"));
        window.click("dismiss-error", cx);
        window.render_frame(cx);
        assert!(rendered_status(window).contains("项目保存失败"));
    })
    .unwrap();
    cx.simulate_keystrokes(settings, crate::kbd!("s"));
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
            if *project == fixture.project));
    acknowledge_project_saved(&fixture, source.clone(), cx);
    assert_clean(&fixture, Some(&source), cx);

    for disconnect in [false, true] {
        menu_action(settings, "项目", "另存为…", cx);
        assert!(cx.did_prompt_for_new_path());
        assert!(drain(&mut fixture.commands).is_empty());
        if disconnect {
            disconnect_save_picker(cx);
            visible_error(settings, "文件选择器已意外关闭", cx);
        } else {
            cx.simulate_new_path_selection(|_| None);
            cx.run_until_parked();
        }
        assert!(!cx.did_prompt_for_new_path());
        assert_clean(&fixture, Some(&source), cx);
        assert!(drain(&mut fixture.commands).is_empty());
    }
    menu_action(settings, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(copy.clone()));
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: Some(path), workspace: None }]
            if *project == fixture.project && path == &copy));
    acknowledge_project_saved(&fixture, copy.clone(), cx);
    assert_clean(&fixture, Some(&copy), cx);

    menu_action(fixture.window, "项目", "打开项目…", cx);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![copy.clone()]));
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { project }] if *project == fixture.project));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
            if *project == fixture.project));
    acknowledge_project_saved(&fixture, copy.clone(), cx);
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::OpenProject { path }] if path == &copy));
    let reopened = ProjectKey {
        generation: fixture.project.generation + 1,
        ..fixture.project
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: reopened,
            path: Some(copy.clone()),
            workspace: stored.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    fixture.project = reopened;
    assert_clean(&fixture, Some(&copy), cx);
    assert_eq!(open_settings(&fixture, cx), settings);
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::Send { .. }
            | Command::SaveWorkspace { .. }
            | Command::SaveProject { .. }
            | Command::Plugin { .. }
    )));
}
