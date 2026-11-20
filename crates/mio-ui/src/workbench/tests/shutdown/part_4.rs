use super::*;

#[gpui_kit::test]
fn cancelled_backend_disables_network_and_tab_settings(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("保留此编辑器")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-settings", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let settings = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .settings_window
            .expect("Proxy settings window opened")
    });
    cx.update_window(fixture.window, |_, _window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.shutdown.cancel();
            cx.notify();
        });
    })
    .unwrap();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("settings-listener-toggle", 0usize)).disabled(),
            Some(true)
        );
    })
    .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intercept-toggle").disabled(), Some(true));
        assert!(window.try_find("intercept-responses").is_none());
        window.click("intercept-toggle", cx);
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert_eq!(window.find("repeater-send").disabled(), Some(true));
        assert_eq!(window.find("repeater-new").disabled(), Some(true));
        let active = fixture.view.read(cx).workspace.active.unwrap();
        window.right_click(SharedString::from(format!("work-tab-{active}")), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let submenu = window.find_by_label("标签视图设置").bounds().center();
        window.pointer_move(submenu, None, cx);
        window.render_frame(cx);
        let option = window.find_by_label("垂直标签");
        let center = option.bounds().center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
        // Kit menus expose disabled behavior without disabled snapshot metadata.
        assert!(window.find_by_label("垂直标签").visible());
        window.press("right", cx);
        window.press("enter", cx);
        assert!(window.find_by_label("垂直标签").visible());
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.orientation, Orientation::Horizontal);
        assert_eq!(view.workspace.tabs[0].title, "保留此编辑器");
        assert!(!view.quitting);
    });
}

#[gpui_kit::test]
fn cancelled_backend_still_accepts_explicit_quit_and_waits_for_join(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("断连时仍可明确退出")], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.shutdown.cancel();
            view.backend_disconnected = true;
            cx.notify();
        });
        window.render_frame(cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "Quit must stay reachable while cleanup is pending"
    );
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested);
        assert!(!view.quitting);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
}

#[gpui_kit::test]
fn unexpected_join_keeps_live_inputs_and_other_tabs_readable(cx: &mut TestAppContext) {
    let first = tab("实时输入");
    let first_id = first.id;
    let initial_target = first.draft.target.clone();
    let second = tab("另一份草稿");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first, second], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    let (raw, base_target) = cx.read(|cx| {
        let editors = &fixture.view.read(cx).tab_editors[&first_id];
        (editors.request.raw.clone(), editors.target.clone())
    });
    let latest = "POST /保留 HTTP/1.1\r\nHost: localhost\r\n\r\n最新输入🙂";
    let latest_target = "http://localhost:32123";
    settings::activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
        window.click("repeater-edit-target", cx);
        assert_ne!(
            fixture.view.read(cx).workspace.tabs[0].draft.raw,
            latest.as_bytes()
        );
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = cx.read(|cx| {
        *cx.windows()
            .iter()
            .find(|&&w| w != fixture.window)
            .expect("Repeater target dialog window")
    });
    let dialog_target = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .open_repeater_target_dialog_for_tab(first_id, cx)
            .expect("Repeater target dialog entity")
            .read(cx)
            .target
            .clone()
    });
    assert_ne!(dialog_target.entity_id(), base_target.entity_id());
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-target-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest_target, cx);
    })
    .unwrap();
    cx.run_until_parked();
    finished
        .send(Err(anyhow::anyhow!("保留实时输入的后台错误")))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(dialog_target.read(cx).value().as_str(), latest_target);
        assert_eq!(
            window.find("repeater-target-ok").disabled(),
            Some(true)
        );
        window.click("repeater-target-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.press(crate::kbd!("c"), cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some(latest_target)
        );
    })
    .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(SharedString::from(format!("work-tab-{second_id}")), cx);
        assert_eq!(fixture.view.read(cx).workspace.active, Some(second_id));
        window.click(SharedString::from(format!("work-tab-{first_id}")), cx);
        assert_eq!(fixture.view.read(cx).workspace.active, Some(first_id));
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert_eq!(base_target.read(cx).value().as_str(), initial_target);
        assert_eq!(dialog_target.read(cx).value().as_str(), latest_target);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("保留实时输入的后台错误")
        );
        assert_eq!(window.find("repeater-send").disabled(), Some(true));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn unexpected_runtime_completion_preserves_window_and_drafts(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("异常退出仍可核对")], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.quitting);
        assert!(view.backend_disconnected);
        assert_eq!(view.workspace.tabs[0].title, "异常退出仍可核对");
    });
    assert!(!cx.has_pending_prompt());
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("退出");
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn native_busy_open_picker_replies_no_without_a_second_prompt(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("synthetic open picker")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(OpenProject), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths());
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "an active Open picker must not acquire a second Quit confirmation"
    );
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(cx.did_prompt_for_paths());
    cx.read(|cx| assert!(!fixture.view.read(cx).shutdown.is_cancelled()));
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn native_busy_export_picker_replies_no_without_a_second_prompt(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("synthetic export picker")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(ExportCa), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "an active CA export picker must not acquire a second Quit confirmation"
    );
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(cx.did_prompt_for_new_path());
    cx.read(|cx| assert!(!fixture.view.read(cx).shutdown.is_cancelled()));
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn native_busy_plugin_picker_rejects_quit_until_picker_cancel(cx: &mut TestAppContext) {
    let first = tab("插件目录选择期间保留输入");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /plugin-picker HTTP/1.1\r\nHost: localhost\r\n\r\n未保存的输入🙂";
    drain(&mut fixture.commands);
    settings::activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
        window.click("tool-extensions", cx);
        window.render_frame(cx);
        window.click("plugin-choose", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(cx.did_prompt_for_paths());
    assert!(!cx.has_pending_prompt());
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "the clicked plugin directory picker must reject native Quit without a second confirmation"
    );
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
        assert_eq!(raw.read(cx).value().as_str(), latest);
    });
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "cancelling the plugin picker must release the busy gate for a fresh native Quit"
    );
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, false]);
    assert!(!cx.has_pending_prompt());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "cancelling directory selection must not inspect, load or execute a plugin"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace.active, Some(id));
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
    });
}

#[gpui_kit::test]
fn plugin_picker_success_releases_busy_gate_without_loading(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("目录选择保留项目")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.click("plugin-choose", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("plugin-choose").disabled(), Some(true));
        assert_eq!(window.find("plugin-inspect").disabled(), Some(true));
        let center = window.find("plugin-choose").bounds().center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(fixture.view.read(cx).file_picker, Some(picker)));
    assert!(drain(&mut fixture.commands).is_empty());
    let selected = std::env::temp_dir().join("mio-plugin-picker-success-synthetic");
    cx.simulate_path_prompt_response(|_| Some(vec![selected.clone()]));
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    cx.read(|cx| assert!(fixture.view.read(cx).file_picker.is_none()));
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::Plugin {
            project,
            action: mio_core::PluginAction::Inspect { directory },
            ..
        }] if *project == fixture.project && *directory == selected),
        "a directory selection may only inspect the package, never load or execute it"
    );
    project_menu(&fixture, "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.file_picker.is_none() && !view.choosing_save_as);
        assert_eq!(view.project, Some(fixture.project));
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn plugin_picker_disconnect_releases_busy_gate_without_loading(cx: &mut TestAppContext) {
    struct PickerDisconnected;

    let mut fixture = fixture(cx, vec![tab("目录选择出错仍保留项目")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.click("plugin-choose", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths());
    // Unwinding drops the test platform's picker sender without a response.
    let disconnected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.simulate_path_prompt_response(|_| {
            std::panic::resume_unwind(Box::new(PickerDisconnected))
        });
    }))
    .expect_err("drop the picker sender to exercise the receiver-error callback");
    assert!(disconnected.is::<PickerDisconnected>());
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.file_picker.is_none());
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("插件目录选择器已关闭")
        );
        assert_eq!(view.project, Some(fixture.project));
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-choose", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert!(fixture.view.read(cx).file_picker.is_none()));
}
