use super::*;

#[gpui_kit::test]
fn inspector_sidebar_edits_request_and_intercept_forward_drop_all_work(cx: &mut TestAppContext) {
    let initial_tab = tab("Inspector与Intercept批量操作");
    let mut fixture = fixture(cx, vec![initial_tab], 64);
    drain(&mut fixture.commands);

    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("inspector-toggle", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert!(window.find("inspector-section-attributes").visible());
        assert!(window.find("inspector-section-query").visible());
        assert!(window.find("inspector-section-request-headers").visible());
        assert!(window.try_find("inspector-target-query").is_none());
        window.click("inspector-section-query", cx);
        window.render_frame(cx);
        window.click("inspector-target-query", cx);
        window.render_frame(cx);
        window.click("inspector-name-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("token", cx);
        window.click("inspector-value-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("abc", cx);
        window.render_frame(cx);
        window.click("inspector-add-button", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let saved_ws = saved(&mut fixture.commands);
    let updated_raw = String::from_utf8(saved_ws.tabs[0].draft.raw.clone()).unwrap();
    assert!(
        updated_raw
            .lines()
            .next()
            .is_some_and(|line| line.contains("token=abc")),
        "Inspector query parameter addition must update request line: {updated_raw}"
    );
    acknowledge(&fixture, saved_ws, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-proxy", cx);
        window.render_frame(cx);
        window.click("proxy-intercept", cx);
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    let req1 = record("http://127.0.0.1:9001/first", "one");
    let req1_id = req1.summary.id;
    let req2 = record("http://127.0.0.1:9001/second", "two");
    let req2_id = req2.summary.id;
    for rec in [req1, req2] {
        fixture
            .events
            .try_send(Event::InterceptPending {
                project: fixture.project,
                stage: InterceptStage::Request,
                response_head_only: false,
                record: Box::new(rec),
            })
            .unwrap();
    }
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intercept-forward-all", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let forwarded_ids: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Command::Decide {
                flow_id,
                decision: Decision::ForwardRequest(_),
                ..
            } => Some(flow_id),
            _ => None,
        })
        .collect();
    assert_eq!(forwarded_ids, vec![req1_id, req2_id]);

    for flow_id in [req1_id, req2_id] {
        fixture
            .events
            .try_send(Event::InterceptResolved {
                project: fixture.project,
                flow_id,
            })
            .unwrap();
    }
    cx.run_until_parked();

    let drop1 = record("http://127.0.0.1:9001/drop-1", "d1");
    let drop1_id = drop1.summary.id;
    let drop2 = record("http://127.0.0.1:9001/drop-2", "d2");
    let drop2_id = drop2.summary.id;
    for rec in [drop1, drop2] {
        fixture
            .events
            .try_send(Event::InterceptPending {
                project: fixture.project,
                stage: InterceptStage::Request,
                response_head_only: false,
                record: Box::new(rec),
            })
            .unwrap();
    }
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intercept-drop-all", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let dropped_ids: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Command::Decide {
                flow_id,
                decision: Decision::Drop,
                ..
            } => Some(flow_id),
            _ => None,
        })
        .collect();
    assert_eq!(dropped_ids, vec![drop1_id, drop2_id]);
}

#[gpui_kit::test]
fn target_scope_settings_context_actions_and_http_filter_in_scope_work(cx: &mut TestAppContext) {
    let mut initial = tab("TargetScope测试");
    initial.draft.target = "http://127.0.0.1:9001".into();
    initial.draft.raw = b"GET /profile HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::AddToScope), cx);
    })
    .unwrap();
    cx.run_until_parked();

    let ws_include = saved(&mut fixture.commands);
    assert_eq!(ws_include.scope.include.len(), 1);
    assert!(ws_include.scope.include[0].enabled);
    assert_eq!(ws_include.scope.include[0].prefix, "http://127.0.0.1:9001");
    acknowledge(&fixture, ws_include, cx);

    let settings = open_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-project-scope-nav", cx);
        window.render_frame(cx);
        window.click("settings-scope-exclude-add", cx);
        window.render_frame(cx);
        window.click("settings-scope-exclude-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("http://127.0.0.1:9001/logout", cx);
        window.render_frame(cx);
        window.click("settings-scope-exclude-add", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let ws_exclude = saved(&mut fixture.commands);
    assert_eq!(ws_exclude.scope.include.len(), 1);
    assert_eq!(ws_exclude.scope.include[0].prefix, "http://127.0.0.1:9001");
    assert_eq!(ws_exclude.scope.exclude.len(), 1);
    assert!(ws_exclude.scope.exclude[0].enabled);
    assert_eq!(
        ws_exclude.scope.exclude[0].prefix,
        "http://127.0.0.1:9001/logout"
    );
    acknowledge(&fixture, ws_exclude, cx);
    cx.simulate_keystrokes(settings, "escape");

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-proxy", cx);
        window.render_frame(cx);
        window.click("proxy-history", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let initial_history_cmds = drain(&mut fixture.commands);
    let _ = acknowledge_history_query(&fixture, initial_history_cmds, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("http-history-filter", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let filter_window = cx.read(|cx| fixture.view.read(cx).http_filter.window.unwrap());
    cx.update_window(filter_window, |_, window, cx| {
        window.render_frame(cx);
        window.click("http-filter-in-scope-only", cx);
        window.render_frame(cx);
        window.click("http-filter-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let filter_query = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::QueryHttpHistory { query } => Some(query),
            _ => None,
        })
        .expect("applying HTTP history filter must emit Command::QueryHttpHistory");
    assert!(filter_query.filter.in_scope_only);
}

#[gpui_kit::test]
fn match_and_replace_decoder_and_comparer_workflows_work(cx: &mut TestAppContext) {
    let initial = tab("Decoder与Comparer");
    let original = String::from_utf8(initial.draft.raw.clone()).unwrap();
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.click("proxy-match-replace", cx);
        window.render_frame(cx);
        assert!(window.try_find("mr-target-select").is_none());
        window.click("mr-side-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let add_dialog = *cx.windows().iter().find(|&&w| w != fixture.window).unwrap();
    cx.update_window(add_dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("mr-type-select").visible());
        window.click("mr-match-input", cx);
        window.input("User-Agent: old", cx);
        window.click("mr-replace-input", cx);
        window.input("User-Agent: mio", cx);
        window.click("mr-submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let workspace = saved(&mut fixture.commands);
    assert_eq!(workspace.match_and_replace_rules.len(), 1);
    assert_eq!(
        workspace.match_and_replace_rules[0].match_pattern,
        "User-Agent: old"
    );
    assert_eq!(
        workspace.match_and_replace_rules[0].replace_with,
        "User-Agent: mio"
    );
    acknowledge(&fixture, workspace, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("mr-row", 0usize)).visible());
        assert!(window.find("mr-edit").disabled() == Some(true));
        window.click(("mr-row", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("mr-edit").disabled(), None);
        window.click("mr-side-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let mr_dialog = *cx.windows().iter().find(|&&w| w != fixture.window).unwrap();
    cx.update_window(mr_dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("mr-mode-script", cx);
        window.render_frame(cx);
        window.click("mr-load-template", cx);
        window.click("mr-script-name-input", cx);
        window.input("Workbench script", cx);
        window.click("mr-test-btn", cx);
        window.render_frame(cx);
        window.click("mr-submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_script = saved(&mut fixture.commands);
    assert_eq!(ws_script.match_and_replace_rules.len(), 2);
    assert_eq!(
        ws_script.match_and_replace_rules[1].item_type,
        mio_core::MatchReplaceTarget::RequestScript
    );
    assert_eq!(
        ws_script.match_and_replace_rules[1].script_name,
        "Workbench script"
    );
    acknowledge(&fixture, ws_script, cx);
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::SendToDecoder), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Decoder);
        assert_eq!(
            view.extra_tools.decoder_input.read(cx).value().as_ref(),
            original
        );
        window.render_frame(cx);
        window.click("decoder-encode-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("decoder-base64-encode", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let output = fixture
            .view
            .read(cx)
            .extra_tools
            .decoder_output
            .read(cx)
            .value();
        assert_eq!(
            output.as_ref(),
            editing::convert_selection(editing::ConvertKind::Base64Encode, &original).unwrap()
        );
        window.click("decoder-continue-button", cx);
        window.render_frame(cx);
        window.click("decoder-decode-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("decoder-base64-decode", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .extra_tools
                .decoder_output
                .read(cx)
                .value()
                .as_ref(),
            original
        )
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        let focus = fixture.view.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(crate::SendToComparerRequest), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        fixture.view.update(cx, |view, cx| {
            let id = view.workspace.active.unwrap();
            let editor = view.tab_editors.get_mut(&id).unwrap();
            editor.request.load(
                b"GET /changed HTTP/1.1\r\nHost: localhost\r\n\r\n",
                window,
                cx,
            );
            editor.dirty = true;
            view.focus.focus(window, cx);
        });
        window.dispatch_action(Box::new(crate::SendToComparerRequest), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Comparer);
        assert_eq!(view.extra_tools.comparer_items.len(), 2);
        assert!(
            view.extra_tools
                .comparer_view
                .read(cx)
                .value()
                .contains("/changed")
        );
        window.click("comparer-mode-bytes", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.extra_tools.byte_mode);
        assert!(
            view.extra_tools
                .comparer_view
                .read(cx)
                .value()
                .contains("Hex bytes")
        );
    });

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("comparer-clear-button", cx);
        window.click("comparer-mode-words", cx);
        fixture.view.update(cx, |view, cx| {
            let id = view.workspace.active.unwrap();
            let editor = view.tab_editors.get_mut(&id).unwrap();
            editor.request.load(b"A\nB\nC\n", window, cx);
            editor.dirty = true;
            view.tool = Tool::Repeater;
            view.focus.focus(window, cx);
        });
        window.dispatch_action(Box::new(crate::SendToComparerRequest), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            let id = view.workspace.active.unwrap();
            let editor = view.tab_editors.get_mut(&id).unwrap();
            editor.request.load(b"A\nX\nY\nB\nC\n", window, cx);
            editor.dirty = true;
            view.tool = Tool::Repeater;
            view.focus.focus(window, cx);
        });
        window.dispatch_action(Box::new(crate::SendToComparerRequest), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let diff = fixture
            .view
            .read(cx)
            .extra_tools
            .comparer_view
            .read(cx)
            .value();
        assert!(
            diff.contains("Modified: 0  Added: 2  Deleted: 0"),
            "unexpected diff: {diff}"
        );
    });

    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: fixture.project.generation + 1,
            },
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.extra_tools.comparer_items.is_empty());
        assert!(view.extra_tools.decoder_input.read(cx).value().is_empty());
    });
}
