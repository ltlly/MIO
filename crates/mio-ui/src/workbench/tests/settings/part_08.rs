use super::*;

#[gpui_kit::test]
fn quit_from_installed_settings_menu_waits_for_main_runtime_join(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("主项目仍由主窗管理")], 64);
    drain(&mut fixture.commands);
    let (finished, joined) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
    cx.update(|cx| {
        let joined = cx.background_spawn(async move { joined.await? });
        Workbench::observe_runtime(&fixture.view, fixture.window, joined, cx);
    });
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    menu_action(settings, "MIO", "退出 MIO", cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    checked(settings, false, cx);
    cx.simulate_keystrokes(settings, crate::kbd!("q"));
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested && view.shutdown.is_cancelled());
        assert!(!view.quitting);
        assert_eq!(view.workspace.tabs[0].title, "主项目仍由主窗管理");
    });
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
}

#[gpui_kit::test]
fn checkbox_mouse_press_from_old_project_cannot_toggle_new_project_on_release(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    let position = cx
        .update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            let position = window.find("repeater-unpack-responses").bounds().center();
            window.pointer_down(position, MouseButton::Left, cx);
            position
        })
        .unwrap();
    let next = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.pointer_up(position, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    checked(settings, true, cx);
    assert!(
        !drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::SaveWorkspace { .. }))
    );
    cx.update_window(settings, |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(window.find("settings-proxy").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-intruder").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-repeater").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-project-tasks").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(
            window.find("repeater-update-content-length").focused(),
            Some(true)
        );
        window.press("tab", cx);
        assert_eq!(
            window.find("repeater-unpack-responses").focused(),
            Some(true)
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    checked(settings, false, cx);
    let commands = drain(&mut fixture.commands);
    assert!(commands.iter().any(|command| matches!(command, Command::SaveWorkspace { project, workspace } if *project == next && !workspace.repeater_settings.unpack_compressed_responses)));
}

#[gpui_kit::test]
fn send_keeps_activation_setting_when_later_checkbox_changes_coalesce_the_save(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("合并保存")], 64);
    drain(&mut fixture.commands);
    let settings_window = open_settings(&fixture, cx);
    toggle(settings_window, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings_window, true, cx);
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    toggle(settings_window, false, cx);
    acknowledge(&fixture, old, cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. }))
    );
    let latest = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    assert!(!latest.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, latest, cx);
    let commands = drain(&mut fixture.commands);
    // Before this first Send the fixture has no stored history.
    let commands = acknowledge_history_query(&fixture, commands, cx);
    let [Command::Send { settings, .. }] = commands.as_slice() else {
        panic!("exactly one captured Send follows the later ACK");
    };
    assert!(settings.unpack_compressed_responses);
    toggle(settings_window, true, cx);
    let latest = saved(&mut fixture.commands);
    acknowledge(&fixture, latest, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "later checkbox changes never repeat an active Send"
    );
    assert!(settings.unpack_compressed_responses);
}

fn ordinary_settings_save_as_abandons_send(cx: &mut TestAppContext, picker_disconnect: bool) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let expected = &stored.tabs[0].draft;
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings, true, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(&old.tabs[0].draft, expected);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            &view.pending_sends[&stored.tabs[0].id].draft, expected,
            "the settings revision, not changed request bytes, must block the old ACK"
        );
    });
    save_as_ack_while_choosing(&mut fixture, settings, old, cx);
    if picker_disconnect {
        disconnect_save_picker(cx);
        visible_error(settings, "文件选择器已意外关闭", cx);
    } else {
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
    }
    abandoned_settings_send_stays_unsent(&mut fixture, settings, expected, cx);
    let editor = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&stored.tabs[0].id]
            .request
            .raw
            .clone()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("仍可编辑🙂", cx);
        assert!(editor.read(cx).value().ends_with("仍可编辑🙂"));
        assert!(fixture.view.read(cx).pending_sends.is_empty());
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn settings_deferred_send_cancel_accepts_click_without_disabled_flag(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings, true, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-cancel").is_none());
        let send_bounds = window.find("repeater-send").bounds();
        window.click("repeater-send", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
        let cancel = window.find("repeater-cancel");
        assert!(cancel.visible());
        assert_eq!(cancel.role(), Some(gpui_kit::accesskit::Role::Button));
        assert_eq!(cancel.disabled(), None);
        assert_eq!(cancel.bounds().origin, send_bounds.origin);
        assert_eq!(cancel.bounds().size, send_bounds.size);
        assert_eq!(fixture.view.read(cx).pending_sends.len(), 1);
        window.click("repeater-cancel", cx);
        window.render_frame(cx);
        assert!(fixture.view.read(cx).pending_sends.is_empty());
        assert!(window.try_find("repeater-cancel").is_none());
        let send = window.find("repeater-send");
        assert_eq!(send.disabled(), None);
        assert_eq!(send.bounds().origin, send_bounds.origin);
        assert_eq!(send.bounds().size, send_bounds.size);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, old, cx);
    let latest = saved(&mut fixture.commands);
    assert!(latest.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, latest, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs, stored.tabs);
        assert!(view.sending.is_empty() && view.pending_sends.is_empty());
        window.click("repeater-send", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [
        Command::Send {
            draft, settings, ..
        },
    ] = commands.as_slice()
    else {
        panic!("the enabled Send control must accept a new explicit click after cancellation");
    };
    assert_eq!(draft, &stored.tabs[0].draft);
    assert!(settings.unpack_compressed_responses);
}

#[gpui_kit::test]
fn ordinary_settings_save_as_cancel_clears_deferred_send(cx: &mut TestAppContext) {
    ordinary_settings_save_as_abandons_send(cx, false);
}

#[gpui_kit::test]
fn ordinary_settings_save_as_picker_disconnect_clears_deferred_send(cx: &mut TestAppContext) {
    ordinary_settings_save_as_abandons_send(cx, true);
}

fn settings_save_as_capture_failure_preserves_composition(
    cx: &mut TestAppContext,
    picker_open: bool,
) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.tabs[0].id;
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings, true, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    if picker_open {
        save_as_ack_while_choosing(&mut fixture, settings, old, cx);
    }
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let (mut handler, marked, text) = cx
        .update_window(fixture.window, |_, window, cx| {
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("end"), cx);
            let bounds = window.find(("input", editor.entity_id())).bounds();
            // Input protocol delivery covers late composition, not native modal/IME routing.
            let mut handler: Box<dyn InputHandler> =
                Box::new(ElementInputHandler::new(bounds, editor.clone()));
            handler.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            let marked = handler.marked_text_range(window, cx).unwrap();
            let text = editor.read(cx).value();
            assert!(text.ends_with("zhong"));
            (handler, marked, text)
        })
        .unwrap();
    if picker_open {
        cx.simulate_new_path_selection(|_| Some(PathBuf::from("unused-capture-failure.mio")));
        cx.run_until_parked();
    } else {
        menu_action(settings, "项目", "另存为…", cx);
    }
    assert!(!cx.did_prompt_for_new_path());
    visible_error(settings, editing::COMPOSITION_PENDING, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(!view.choosing_save_as);
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(view.pending_project_save.is_none() && view.workspace_dirty);
        assert!(view.workspace.repeater_settings.unpack_compressed_responses);
        assert_eq!(view.workspace.tabs, stored.tabs);
        assert!(view.sending.is_empty());
        assert!(
            view.pending_sends.is_empty(),
            "a failed Save As draft capture must discard its deferred unsent intent"
        );
        assert_eq!(handler.marked_text_range(window, cx), Some(marked));
        assert_eq!(editor.read(cx).value(), text);
        window.render_frame(cx);
        assert!(rendered_status(window).contains("尚未建立网络连接"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_save_as_capture_failure_before_picker_discards_unsent_intent(cx: &mut TestAppContext) {
    settings_save_as_capture_failure_preserves_composition(cx, false);
}

#[gpui_kit::test]
fn settings_save_as_capture_failure_after_picker_discards_unsent_intent(cx: &mut TestAppContext) {
    settings_save_as_capture_failure_preserves_composition(cx, true);
}

#[gpui_kit::test]
fn settings_transition_save_as_picker_disconnect_resumes_without_sending(cx: &mut TestAppContext) {
    let first = tab("保留退出前的草稿");
    let mut fixture = fixture(cx, vec![first.clone()], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    menu_action(settings, "MIO", "退出 MIO", cx);
    assert!(cx.has_pending_prompt());
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
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.choosing_save_as && view.after_save.is_some());
        assert!(view.transition == Some(TransitionPhase::Ready));
    });
    disconnect_save_picker(cx);
    visible_error(settings, "文件选择器已意外关闭", cx);
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::Resume { project }] if *project == fixture.project));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(!view.choosing_save_as && !view.quitting && !view.shutdown.is_cancelled());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(view.workspace.tabs, vec![first]);
    });
}
