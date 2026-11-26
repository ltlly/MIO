use super::*;

#[gpui_kit::test]
fn websocket_interception_rules_defaults_and_persistence_clicks(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_ws_show_controls(settings, cx);
    checkbox_checked(settings, "proxy-ws-intercept-client-to-server", true, cx);
    checkbox_checked(settings, "proxy-ws-intercept-server-to-client", true, cx);
    checkbox_checked(settings, "proxy-ws-intercept-in-scope-only", false, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let heading = window.find("proxy-ws-rules-heading");
        assert_eq!(heading.role(), Some(gpui_kit::accesskit::Role::Heading));
        assert_eq!(heading.label(), Some("WebSocket 拦截规则"));
        let c2s = window.find("proxy-ws-intercept-client-to-server");
        let s2c = window.find("proxy-ws-intercept-server-to-client");
        let scope = window.find("proxy-ws-intercept-in-scope-only");
        assert!(heading.bounds().bottom() <= c2s.bounds().top());
        assert!(c2s.bounds().bottom() <= s2c.bounds().top());
        assert!(s2c.bounds().bottom() <= scope.bounds().top());
    })
    .unwrap();

    proxy_rule_click(settings, "proxy-ws-intercept-client-to-server", cx);
    let first = saved(&mut fixture.commands);
    assert!(!first.proxy_websocket_interception.client_to_server_messages);
    assert!(first.proxy_websocket_interception.server_to_client_messages);
    assert!(!first.proxy_websocket_interception.intercept_in_scope_only);
    let mut wrong_ws = first.clone();
    wrong_ws
        .proxy_websocket_interception
        .client_to_server_messages = true;
    acknowledge(&fixture, wrong_ws, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(first.revision));
        assert_eq!(
            view.pending_proxy_interception,
            Some((
                first.revision,
                first.proxy_interception.clone(),
                first.proxy_messages,
                first.proxy_websocket_interception,
            ))
        );
        assert_eq!(view.workspace.revision + 1, first.revision);
        assert!(
            view.error
                .as_deref()
                .is_some_and(|msg| msg.contains("消息设置或规则保存回执与实际提交快照不匹配"))
        );
    });
    let mut stale_ack = first.clone();
    stale_ack.revision = first.revision + 99;
    acknowledge(&fixture, stale_ack, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(first.revision));
        assert!(
            view.error
                .as_deref()
                .is_some_and(|msg| msg.contains("消息设置或规则保存回执与实际提交快照不匹配"))
        );
    });
    acknowledge(&fixture, first.clone(), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, first);
        assert!(view.pending_workspace.is_none());
        assert!(view.pending_proxy_interception.is_none());
        assert!(view.error.is_none());
    });

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-ws-intercept-client-to-server").focused(),
            Some(true)
        );
        window.press("tab", cx);
        assert_eq!(
            window.find("proxy-ws-intercept-server-to-client").focused(),
            Some(true)
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let s2c_off = saved(&mut fixture.commands);
    assert!(
        !s2c_off
            .proxy_websocket_interception
            .client_to_server_messages
    );
    assert!(
        !s2c_off
            .proxy_websocket_interception
            .server_to_client_messages
    );
    assert!(!s2c_off.proxy_websocket_interception.intercept_in_scope_only);
    acknowledge(&fixture, s2c_off, cx);

    proxy_rule_click(settings, "proxy-ws-intercept-in-scope-only", cx);
    let second = saved(&mut fixture.commands);
    assert!(
        !second
            .proxy_websocket_interception
            .client_to_server_messages
    );
    assert!(
        !second
            .proxy_websocket_interception
            .server_to_client_messages
    );
    assert!(second.proxy_websocket_interception.intercept_in_scope_only);
    acknowledge(&fixture, second.clone(), cx);

    cx.simulate_keystrokes(settings, crate::kbd!("w"));
    cx.run_until_parked();
    let reopened = open_settings(&fixture, cx);
    proxy_rule_click(reopened, "settings-proxy", cx);
    proxy_ws_show_controls(reopened, cx);
    checkbox_checked(reopened, "proxy-ws-intercept-client-to-server", false, cx);
    checkbox_checked(reopened, "proxy-ws-intercept-server-to-client", false, cx);
    checkbox_checked(reopened, "proxy-ws-intercept-in-scope-only", true, cx);
    cx.read(|cx| assert_eq!(*fixture.view.read(cx).workspace, second));
}

#[gpui_kit::test]
fn websocket_interception_rules_old_project_mouse_press_cannot_toggle_new_project(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for id in [
        "proxy-ws-intercept-client-to-server",
        "proxy-ws-intercept-server-to-client",
        "proxy-ws-intercept-in-scope-only",
    ] {
        proxy_ws_show_controls(settings, cx);
        let position = cx
            .update_window(settings, |_, window, cx| {
                window.render_frame(cx);
                let position = window.find(id).bounds().center();
                window.pointer_down(position, MouseButton::Left, cx);
                position
            })
            .unwrap();
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
        for command in drain(&mut fixture.commands) {
            let Command::QueryHttpHistory { query } = command else {
                panic!("opening an empty project must not save settings");
            };
            fixture
                .events
                .try_send(Event::HttpHistoryPage {
                    query,
                    result: Ok(mio_core::HttpHistoryResult {
                        total: 0,
                        rows: Vec::new(),
                        selected_rows: Vec::new(),
                        selected_matches: Vec::new(),
                        selected_ranks: Vec::new(),
                        live_annotations: Vec::new(),
                        live_numbers: Vec::new(),
                        row_metadata: Vec::new(),
                    }),
                })
                .unwrap();
        }
        cx.run_until_parked();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.pointer_up(position, MouseButton::Left, cx);
        })
        .unwrap();
        cx.run_until_parked();
        checkbox_checked(settings, "proxy-ws-intercept-client-to-server", true, cx);
        checkbox_checked(settings, "proxy-ws-intercept-server-to-client", true, cx);
        checkbox_checked(settings, "proxy-ws-intercept-in-scope-only", false, cx);
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "late mouse-up after project switch must not dispatch new commands"
        );
    }
}

#[gpui_kit::test]
fn proxy_message_unpack_defaults_clicks_and_exact_workspace_acknowledgment(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_message_show_controls(settings, cx);
    checkbox_checked(settings, "proxy-unpack-requests", false, cx);
    checkbox_checked(settings, "proxy-unpack-responses", true, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let heading = window.find("proxy-miscellaneous-heading");
        assert_eq!(heading.role(), Some(gpui_kit::accesskit::Role::Heading));
        assert_eq!(heading.label(), Some("杂项"));
        let requests = window.find("proxy-unpack-requests");
        let responses = window.find("proxy-unpack-responses");
        assert!(heading.bounds().bottom() <= requests.bounds().top());
        assert!(requests.bounds().bottom() <= responses.bounds().top());
        assert!(responses.bounds().bottom() <= window.viewport_size().height);
    })
    .unwrap();
    for (response, id, checked) in [
        (false, "proxy-unpack-requests", true),
        (true, "proxy-unpack-responses", false),
        (false, "proxy-unpack-requests", false),
        (true, "proxy-unpack-responses", true),
    ] {
        let original = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
        proxy_rule_click(settings, id, cx);
        checkbox_checked(settings, id, checked, cx);
        let candidate = saved(&mut fixture.commands);
        let mut expected = original;
        expected.revision += 1;
        expected.tab_sidebar_width = cx.read(|cx| fixture.view.read(cx).tab_width as u16);
        if response {
            expected.proxy_messages.unpack_compressed_responses = checked;
        } else {
            expected.proxy_messages.unpack_compressed_requests = checked;
        }
        assert_eq!(candidate, expected);
        let mut wrong = candidate.clone();
        if response {
            wrong.proxy_messages.unpack_compressed_responses = !checked;
        } else {
            wrong.proxy_messages.unpack_compressed_requests = !checked;
        }
        acknowledge(&fixture, wrong, cx);
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.pending_workspace, Some(candidate.revision));
            assert_eq!(view.workspace.revision, candidate.revision - 1);
        });
        checkbox_checked(settings, id, checked, cx);
        acknowledge(&fixture, candidate.clone(), cx);
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(*view.workspace, candidate);
            assert!(view.pending_workspace.is_none());
            assert!(!view.workspace_dirty);
        });
        checkbox_checked(settings, id, checked, cx);
    }
}

#[gpui_kit::test]
fn proxy_message_unpack_keyboard_close_reopen_keeps_pending_and_confirmed_values(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_message_show_controls(settings, cx);
    proxy_rule_click(settings, "proxy-unpack-requests", cx);
    let first = saved(&mut fixture.commands);
    assert!(first.proxy_messages.unpack_compressed_requests);
    assert!(first.proxy_messages.unpack_compressed_responses);
    acknowledge(&fixture, first.clone(), cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("proxy-unpack-requests").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("proxy-unpack-responses").focused(), Some(true));
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    checkbox_checked(settings, "proxy-unpack-requests", true, cx);
    checkbox_checked(settings, "proxy-unpack-responses", false, cx);
    let pending = saved(&mut fixture.commands);
    let mut expected = first;
    expected.revision += 1;
    expected.proxy_messages.unpack_compressed_responses = false;
    assert_eq!(pending, expected);
    cx.simulate_keystrokes(settings, crate::kbd!("w"));
    cx.run_until_parked();
    assert_eq!(cx.windows(), vec![fixture.window]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(pending.revision));
        assert_eq!(view.workspace.proxy_messages, pending.proxy_messages);
    });
    acknowledge(&fixture, pending.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let reopened = open_settings(&fixture, cx);
    proxy_rule_click(reopened, "settings-proxy", cx);
    proxy_message_show_controls(reopened, cx);
    checkbox_checked(reopened, "proxy-unpack-requests", true, cx);
    checkbox_checked(reopened, "proxy-unpack-responses", false, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, pending);
        assert!(view.pending_workspace.is_none());
        assert!(view.pending_proxy_interception.is_none());
        assert!(!view.workspace_dirty);
    });
    cx.simulate_keystrokes(reopened, "escape");
    cx.run_until_parked();
    assert_eq!(cx.windows(), vec![fixture.window]);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn proxy_message_unpack_pending_rule_ack_recovers_only_its_snapshot_error(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for unrelated in [false, true] {
        proxy_rule_click(settings, "proxy-request-rule-add", cx);
        let dialog = proxy_rule_dialog(settings, &fixture, cx);
        proxy_rule_choose_type(dialog, "正文", false, cx);
        let condition = format!("^preserved-pending-body-{unrelated}$");
        proxy_rule_condition(dialog, &condition, cx);
        proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
        let submitted = saved(&mut fixture.commands);
        let mut wrong = submitted.clone();
        wrong.proxy_messages.unpack_compressed_responses =
            !submitted.proxy_messages.unpack_compressed_responses;
        acknowledge(&fixture, wrong, cx);
        assert!(cx.windows().contains(&dialog));
        cx.update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("proxy-rule-editor-condition").value(),
                Some(condition.as_str())
            );
            let error = window.find("proxy-rule-editor-error");
            assert!(error.visible());
            assert!(error.label().unwrap().contains("消息设置或规则"));
        })
        .unwrap();
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.pending_workspace, Some(submitted.revision));
            assert_eq!(view.workspace.revision + 1, submitted.revision);
        });
        assert!(drain(&mut fixture.commands).is_empty());
        if unrelated {
            fixture
                .events
                .try_send(Event::Error {
                    project: Some(fixture.project),
                    operation: "start_proxy",
                    message: "distinct later Proxy failure".into(),
                })
                .unwrap();
            cx.run_until_parked();
        }
        acknowledge(&fixture, submitted.clone(), cx);
        assert!(!cx.windows().contains(&dialog));
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(*view.workspace, submitted);
            assert!(view.pending_workspace.is_none());
            assert!(view.pending_proxy_interception.is_none());
            assert!(!view.workspace_dirty);
            assert_eq!(
                view.error.as_deref(),
                unrelated.then_some("start_proxy：distinct later Proxy failure")
            );
        });
    }
}

#[gpui_kit::test]
fn proxy_message_unpack_coalesced_old_snapshot_ack_preserves_new_checkbox_and_editor_draft(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_message_show_controls(settings, cx);
    proxy_rule_click(settings, "proxy-unpack-requests", cx);
    let first = saved(&mut fixture.commands);
    assert!(first.proxy_messages.unpack_compressed_requests);
    assert!(first.proxy_messages.unpack_compressed_responses);
    proxy_rule_click(settings, "proxy-unpack-responses", cx);
    checkbox_checked(settings, "proxy-unpack-responses", false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, first, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.pending_workspace.is_some(),
            "coalesced second checkbox must enqueue its own save after first acknowledgment"
        );
    });
    let second = saved(&mut fixture.commands);
    assert!(second.proxy_messages.unpack_compressed_requests);
    assert!(!second.proxy_messages.unpack_compressed_responses);
    checkbox_checked(settings, "proxy-unpack-requests", true, cx);
    checkbox_checked(settings, "proxy-unpack-responses", false, cx);
    proxy_rule_click(settings, "settings-repeater", cx);
    proxy_rule_click(settings, "settings-proxy", cx);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^kept-with-unpack-save$", cx);
    assert!(cx.windows().contains(&dialog));
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("proxy-rule-editor-error").is_none(),
            "valid coalesced editor base must not reject the saved message settings"
        );
    })
    .unwrap();
    acknowledge(&fixture, second, cx);
    let candidate = saved(&mut fixture.commands);
    assert!(candidate.proxy_messages.unpack_compressed_requests);
    assert!(!candidate.proxy_messages.unpack_compressed_responses);
    assert_eq!(
        candidate
            .proxy_interception
            .request
            .rules
            .last()
            .unwrap()
            .condition,
        "^kept-with-unpack-save$"
    );
    assert!(cx.windows().contains(&dialog));
    acknowledge(&fixture, candidate, cx);
    assert!(!cx.windows().contains(&dialog));
}
