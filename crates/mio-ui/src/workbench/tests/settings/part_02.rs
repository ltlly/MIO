use super::*;

#[gpui_kit::test]
fn proxy_message_unpack_rejection_preserves_values_retries_and_backend_disconnect_disables_input(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_message_show_controls(settings, cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-proxy-unpack-retry.mio"));
        })
    });
    let stored = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    proxy_rule_click(settings, "proxy-unpack-requests", cx);
    let rejected = saved(&mut fixture.commands);
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: rejected.revision,
            stored,
            message: "Proxy 解压设置测试拒绝".into(),
        })
        .unwrap();
    cx.run_until_parked();
    checkbox_checked(settings, "proxy-unpack-requests", true, cx);
    checkbox_checked(settings, "proxy-unpack-responses", true, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty);
        assert!(view.pending_workspace.is_none());
    });
    menu_action(settings, "项目", "保存项目", cx);
    let retry = saved(&mut fixture.commands);
    assert_eq!(retry.proxy_messages, rejected.proxy_messages);
    acknowledge(&fixture, retry, cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::SaveProject { project, path: None, workspace: None, .. }] if *project == fixture.project)
    );
    fixture
        .events
        .try_send(Event::ProjectSaved {
            project: fixture.project,
            path: PathBuf::from("/tmp/mio-proxy-unpack-retry.mio"),
            total_bytes: 4096,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    checkbox_checked(settings, "proxy-unpack-requests", true, cx);
    checkbox_checked(settings, "proxy-unpack-responses", true, cx);
    let events = std::mem::replace(&mut fixture.events, mpsc::channel(1).0);
    drop(events);
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for id in ["proxy-unpack-requests", "proxy-unpack-responses"] {
            assert_eq!(window.find(id).disabled(), Some(true));
            window.click(id, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    checkbox_checked(settings, "proxy-unpack-requests", true, cx);
    checkbox_checked(settings, "proxy-unpack-responses", true, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn proxy_message_unpack_old_project_mouse_press_cannot_toggle_new_project(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for id in ["proxy-unpack-requests", "proxy-unpack-responses"] {
        proxy_message_show_controls(settings, cx);
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
        let commands = drain(&mut fixture.commands);
        for command in commands {
            let Command::QueryHttpHistory { query } = command else {
                panic!(
                    "opening an empty project may query its history but must not save any settings"
                );
            };
            assert_eq!(query.project, fixture.project);
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
        checkbox_checked(settings, "proxy-unpack-requests", false, cx);
        checkbox_checked(settings, "proxy-unpack-responses", true, cx);
        let commands = drain(&mut fixture.commands);
        let kinds = commands
            .iter()
            .map(|command| match command {
                Command::SaveWorkspace { .. } => "SaveWorkspace",
                Command::QueryHttpHistory { .. } => "QueryHttpHistory",
                Command::QueryWebSocketHistory { .. } => "QueryWebSocketHistory",
                Command::QueryLoggerHistory { .. } => "QueryLoggerHistory",
                _ => "Other",
            })
            .collect::<Vec<_>>();
        assert!(
            commands.is_empty(),
            "late mouse-up after project switch must not dispatch new commands: {kinds:?}"
        );
    }
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            view.project = None;
            cx.notify();
        })
    });
    cx.run_until_parked();
    proxy_message_show_controls(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for id in ["proxy-unpack-requests", "proxy-unpack-responses"] {
            assert_eq!(window.find(id).disabled(), Some(true));
            window.click(id, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn proxy_interception_rules_response_request_parameters_and_url_relations_follow_observed_choices(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let response = original.proxy_interception.response.rules[2].id;
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-response-rule-row-{response}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-response-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        for label in [
            "Contains parameters",
            "Does not contain parameters",
            "Was intercepted",
            "Was not intercepted",
            "Was modified",
            "Was not modified",
            "Was annotated",
            "Was not annotated",
        ] {
            assert!(
                !window.find_all_by_label(label).is_empty(),
                "implemented observed choice {label} must be available"
            );
        }
    })
    .unwrap();
    cx.simulate_keystrokes(dialog, "up up enter");
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-relationship").value(),
            Some("Contains parameters")
        );
    })
    .unwrap();
    assert_proxy_rule_condition_disabled(dialog, cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let parameters = saved(&mut fixture.commands);
    let rule = parameters
        .proxy_interception
        .response
        .rules
        .iter()
        .find(|rule| rule.id == response)
        .unwrap();
    assert_eq!(
        rule.relationship,
        mio_core::ProxyInterceptRelation::ContainsParameters
    );
    assert!(rule.condition.is_empty());
    assert_eq!(
        parameters.proxy_interception.request,
        original.proxy_interception.request
    );
    assert!(cx.windows().contains(&dialog));
    acknowledge(&fixture, parameters.clone(), cx);
    assert!(!cx.windows().contains(&dialog));

    let request_url = parameters.proxy_interception.request.rules[3].id;
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-row-{request_url}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-request-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        for label in [
            "Matches",
            "Does not match",
            "Is in target scope",
            "Is not in target scope",
        ] {
            assert!(!window.find_all_by_label(label).is_empty());
        }
    })
    .unwrap();
    cx.simulate_keystrokes(dialog, "up enter");
    cx.run_until_parked();
    proxy_rule_condition(dialog, "^/kept-condition$", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-relationship").value(),
            Some("Does not match")
        );
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("^/kept-condition$")
        );
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let matching = saved(&mut fixture.commands);
    let matching_rule = matching
        .proxy_interception
        .request
        .rules
        .iter()
        .find(|rule| rule.id == request_url)
        .unwrap();
    assert_eq!(
        matching_rule.relationship,
        mio_core::ProxyInterceptRelation::DoesNotMatch
    );
    assert_eq!(matching_rule.condition, "^/kept-condition$");
    acknowledge(&fixture, matching, cx);
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-row-{request_url}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-request-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
    cx.simulate_keystrokes(dialog, "down enter");
    cx.run_until_parked();
    assert_proxy_rule_condition_disabled(dialog, cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let scope = saved(&mut fixture.commands);
    let rule = scope
        .proxy_interception
        .request
        .rules
        .iter()
        .find(|rule| rule.id == request_url)
        .unwrap();
    assert_eq!(rule.relationship, mio_core::ProxyInterceptRelation::InScope);
    assert!(rule.condition.is_empty());
    acknowledge(&fixture, scope, cx);
}

#[gpui_kit::test]
fn proxy_interception_rules_add_edit_cancel_and_confirmed_project_save(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^cancelled\\.invalid$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-cancel", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(*fixture.view.read(cx).workspace, original));

    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^example\\.test$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let added = saved(&mut fixture.commands);
    assert_eq!(added.revision, original.revision + 1);
    assert_eq!(added.proxy_interception.request.rules.len(), 5);
    let new_rule = added
        .proxy_interception
        .request
        .rules
        .last()
        .unwrap()
        .clone();
    assert_eq!(new_rule.condition, "^example\\.test$");
    assert!(new_rule.enabled);
    assert!(
        cx.windows().contains(&dialog),
        "editor must wait for project CAS acknowledgment"
    );
    acknowledge(&fixture, added.clone(), cx);
    assert!(!cx.windows().contains(&dialog));

    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-row-{}", new_rule.id)),
        cx,
    );
    proxy_rule_click(settings, "proxy-request-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^edited\\.test$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let edited = saved(&mut fixture.commands);
    assert_eq!(
        edited.proxy_interception.request.rules.last().unwrap().id,
        new_rule.id
    );
    assert_eq!(
        edited
            .proxy_interception
            .request
            .rules
            .last()
            .unwrap()
            .condition,
        "^edited\\.test$"
    );
    assert_eq!(
        edited.proxy_interception.response,
        original.proxy_interception.response
    );
    acknowledge(&fixture, edited, cx);
}

#[gpui_kit::test]
fn proxy_interception_rules_cas_rejection_preserves_input_and_retries_same_rule_without_duplicates(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let stored = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^retry-kept$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let first = saved(&mut fixture.commands);
    let id = first.proxy_interception.request.rules.last().unwrap().id;
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: first.revision,
            stored,
            message: "synthetic read-only project".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("^retry-kept$")
        );
        assert!(
            window
                .find("proxy-rule-editor-error")
                .label()
                .unwrap()
                .contains("read-only project")
        );
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let retry = saved(&mut fixture.commands);
    assert_eq!(
        retry.proxy_interception.request.rules.len(),
        first.proxy_interception.request.rules.len()
    );
    assert_eq!(
        retry.proxy_interception.request.rules.last().unwrap().id,
        id
    );
    assert_eq!(
        retry
            .proxy_interception
            .request
            .rules
            .last()
            .unwrap()
            .condition,
        "^retry-kept$"
    );
    acknowledge(&fixture, retry, cx);
    assert!(!cx.windows().contains(&dialog));
}
