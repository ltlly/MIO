use super::*;

#[gpui_kit::test]
fn proxy_interception_rules_stage_dropdowns_disable_condition_only_for_boolean_relationships(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let request = original.proxy_interception.request.rules[1].id;
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-row-{request}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-request-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    assert_proxy_rule_condition_disabled(dialog, cx);
    proxy_rule_click(dialog, "proxy-rule-editor-cancel", cx);
    let response = original.proxy_interception.response.rules[2].id;
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-response-rule-row-{response}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-response-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    assert_proxy_rule_condition_disabled(dialog, cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-relationship").value(),
            Some("Was intercepted")
        );
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-rule-editor-cancel", cx);
    assert!(drain(&mut fixture.commands).is_empty());

    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_click(dialog, "proxy-rule-editor-operator", cx);
    cx.simulate_keystrokes(dialog, "enter");
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-operator").expanded(),
            Some(true)
        );
    })
    .unwrap();
    cx.simulate_keystrokes(dialog, "down enter");
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-operator").value(),
            Some("Or")
        );
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-rule-editor-type", cx);
    let open = cx
        .update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            let select = window.find("proxy-rule-editor-type");
            assert_eq!(
                select.focused(),
                Some(true),
                "type keyboard action must target the combo focus"
            );
            select.expanded() == Some(true)
        })
        .unwrap();
    if !open {
        cx.simulate_keystrokes(dialog, "enter");
    }
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("proxy-rule-editor-type").expanded(), Some(true));
    })
    .unwrap();
    cx.simulate_keystrokes(dialog, "down down down enter");
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-type").value(),
            Some("HTTP method")
        );
    })
    .unwrap();
    proxy_rule_condition(dialog, "^POST$", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        let editor = window
            .root::<gpui_kit::base::Root>()
            .unwrap()
            .unwrap()
            .read(cx)
            .view()
            .clone()
            .downcast::<crate::workbench::settings::InterceptionEditor>()
            .unwrap();
        assert!(editor.read(cx).condition.read(cx).document_revision() > 0);
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("^POST$")
        );
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let changed = saved(&mut fixture.commands);
    let rule = changed.proxy_interception.request.rules.last().unwrap();
    assert_eq!(rule.operator, mio_core::ProxyInterceptOperator::Or);
    assert_eq!(rule.match_type, mio_core::ProxyInterceptType::HttpMethod);
    assert_eq!(rule.condition, "^POST$");
    acknowledge(&fixture, changed, cx);
}

#[gpui_kit::test]
fn save_as_picker_releases_its_token_after_an_auxiliary_reader_window_closes(
    cx: &mut TestAppContext,
) {
    struct AuxiliaryReader(Entity<Workbench>);
    impl Render for AuxiliaryReader {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().child(format!("project {:?}", self.0.read(cx).project))
        }
    }
    let mut fixture = fixture(cx, vec![tab("保存主窗草稿")], 64);
    drain(&mut fixture.commands);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let draft_id = original.active.unwrap();
    let latest = "POST /pending-save HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n中文草稿🙂";
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).tab_editors[&draft_id]
            .request
            .raw
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
        window.press(crate::kbd!("shift-s"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let (picker, expected) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.choosing_save_as && view.workspace_dirty);
        assert!(view.quit_attempt.is_none());
        assert_eq!(view.workspace.tabs[0].id, draft_id);
        assert_eq!(view.workspace.tabs[0].draft.raw, latest.as_bytes());
        (view.file_picker.unwrap(), (*view.workspace).clone())
    });
    assert!(drain(&mut fixture.commands).is_empty());
    // A freely closable auxiliary view models GPUI's shared-entity window mapping, not a product feature.
    let (auxiliary, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            window.on_window_should_close(cx, |_, _| true);
            cx.new(|_| AuxiliaryReader(fixture.view.clone()))
        })
        .unwrap()
    });
    cx.update_window(auxiliary, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(cx.update(|cx| cx.with_window(fixture.view.entity_id(), |window, _| window.window_handle())), Some(auxiliary));
    cx.skip_drawing();
    let mut native = gpui_kit::VisualTestContext::from_window(auxiliary, cx);
    assert!(native.simulate_close());
    // This helper only asks should_close; model the OS close notification after permission.
    auxiliary
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(auxiliary.update(cx, |_, _, _| ()).is_err());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert_eq!(cx.update(|cx| cx.with_window(fixture.view.entity_id(), |window, _| window.window_handle())), None);
    let destination = PathBuf::from("/tmp/mio-save-as-owner-fixture.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_ne!(view.file_picker, Some(picker), "the completed picker callback must release its own token even after the dynamic auxiliary window disappears");
        assert!(view.file_picker.is_none() && !view.choosing_save_as);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown.is_cancelled());
        assert_eq!(*view.workspace, expected);
        assert_eq!(view.workspace.repeater_settings, original.repeater_settings);
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: Some(path),
            workspace: Some(snapshot),
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must issue exactly one snapshot save and no Start, Send, or source write");
    };
    assert_eq!((*project, path), (fixture.project, &destination));
    let mut expected_snapshot = expected;
    expected_snapshot.revision += 1;
    assert_eq!(*snapshot, expected_snapshot);
    assert!(!fixture.events.is_closed());
}

fn close_auxiliary_reader(fixture: &Fixture, cx: &mut TestAppContext) {
    struct AuxiliaryReader(Entity<Workbench>);
    impl Render for AuxiliaryReader {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().child(format!("project {:?}", self.0.read(cx).project))
        }
    }
    let (auxiliary, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            window.on_window_should_close(cx, |_, _| true);
            cx.new(|_| AuxiliaryReader(fixture.view.clone()))
        })
        .unwrap()
    });
    cx.update_window(auxiliary, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(cx.update(|cx| cx.with_window(fixture.view.entity_id(), |window, _| window.window_handle())), Some(auxiliary));
    cx.skip_drawing();
    let mut native = gpui_kit::VisualTestContext::from_window(auxiliary, cx);
    assert!(native.simulate_close());
    // TestPlatform asks permission only; finish the allowed OS close notification.
    auxiliary
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(auxiliary.update(cx, |_, _, _| ()).is_err());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert_eq!(cx.update(|cx| cx.with_window(fixture.view.entity_id(), |window, _| window.window_handle())), None);
}

#[gpui_kit::test]
fn open_picker_cancel_releases_its_token_after_an_auxiliary_reader_window_closes(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("打开取消后保留草稿")], 64);
    drain(&mut fixture.commands);
    let expected = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    menu_action(fixture.window, "项目", "打开项目…", cx);
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
    assert!(drain(&mut fixture.commands).is_empty());
    close_auxiliary_reader(&fixture, cx);
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_ne!(view.file_picker, Some(picker));
        assert!(view.file_picker.is_none() && !view.choosing_save_as);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(*view.workspace, expected);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
    });
    assert!(!fixture.events.is_closed());
}

#[gpui_kit::test]
fn export_ca_picker_cancel_releases_its_token_after_an_auxiliary_reader_window_closes(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("CA导出取消后保留草稿")], 64);
    drain(&mut fixture.commands);
    let expected = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    menu_action(fixture.window, "项目", "导出 Proxy CA…", cx);
    assert!(cx.did_prompt_for_new_path());
    let picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
    assert!(drain(&mut fixture.commands).is_empty());
    close_auxiliary_reader(&fixture, cx);
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_ne!(view.file_picker, Some(picker));
        assert!(view.file_picker.is_none() && !view.choosing_save_as);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(*view.workspace, expected);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
    });
    assert!(!fixture.events.is_closed());
}

#[gpui_kit::test]
fn installed_settings_menu_zero_tabs_mouse_keyboard_close_reopen(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    checkbox_checked(settings, "repeater-update-content-length", true, cx);
    checked(settings, true, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for label in [
            "Tools",
            "Repeater",
            "Message modification",
            "项目设置",
            "更新 Content-Length",
            "解包压缩响应",
        ] {
            assert!(window.find_by_label(label).visible());
        }
        assert!(
            window.find("repeater-update-content-length").bounds().top()
                < window.find("repeater-unpack-responses").bounds().top()
        );
    })
    .unwrap();
    assert_eq!(open_settings(&fixture, cx), settings);
    menu_action(settings, "MIO", "设置", cx);
    assert_eq!(cx.windows().len(), 2);
    toggle_checkbox(settings, "repeater-update-content-length", false, cx);
    let first_cl = saved(&mut fixture.commands);
    assert!(first_cl.tabs.is_empty());
    assert!(!first_cl.repeater_settings.update_content_length);
    assert!(first_cl.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, first_cl, cx);
    toggle(settings, false, cx);
    let first = saved(&mut fixture.commands);
    assert!(first.tabs.is_empty());
    assert!(!first.repeater_settings.update_content_length);
    assert!(!first.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, first, cx);
    cx.simulate_keystrokes(settings, "escape");
    assert_eq!(cx.windows(), vec![fixture.window]);
    let reopened = open_settings(&fixture, cx);
    checkbox_checked(reopened, "repeater-update-content-length", false, cx);
    checked(reopened, false, cx);
    cx.update_window(reopened, |_, window, cx| {
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
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    checkbox_checked(reopened, "repeater-update-content-length", true, cx);
    let second_cl = saved(&mut fixture.commands);
    assert!(second_cl.repeater_settings.update_content_length);
    assert!(!second_cl.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, second_cl, cx);
    cx.update_window(reopened, |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(
            window.find("repeater-unpack-responses").focused(),
            Some(true)
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    checked(reopened, true, cx);
    let second = saved(&mut fixture.commands);
    assert!(second.repeater_settings.update_content_length);
    assert!(second.repeater_settings.unpack_compressed_responses);
    cx.simulate_keystrokes(reopened, crate::kbd!("w"));
    assert_eq!(cx.windows(), vec![fixture.window]);
    acknowledge(&fixture, second, cx);
    let trailing = drain(&mut fixture.commands);
    assert!(
        trailing.is_empty(),
        "unexpected workspace saves after Settings close and ACK: {:?}",
        trailing
            .iter()
            .filter_map(|command| match command {
                Command::SaveWorkspace { workspace, .. } => Some(workspace),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    let reopened_again = open_settings(&fixture, cx);
    checkbox_checked(reopened_again, "repeater-update-content-length", true, cx);
    checked(reopened_again, true, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace.tabs.is_empty());
        assert!(view.tab_editors.is_empty());
        assert!(!view.workspace_dirty);
        assert!(view.pending_workspace.is_none());
    });
}
