use super::*;

#[gpui_kit::test]
fn named_saved_result_setting_is_checked_disabled_and_cannot_revert_to_session(
    cx: &mut TestAppContext,
) {
    let (mut fixture, started) =
        result_settings_fixture(true, mio_core::IntruderRetention::Project, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let checkbox = window.find("intruder-result-save-to-project");
        assert_eq!(checkbox.checked(), Some(true));
        assert_eq!(checkbox.disabled(), Some(true));
        window.click("intruder-result-save-to-project", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
        assert!(!view.intruder.session_runs.contains(&started.summary.id));
    });
}

#[gpui_kit::test]
fn named_terminal_session_result_setting_requests_explicit_project_promotion(
    cx: &mut TestAppContext,
) {
    let (mut fixture, started) =
        result_settings_fixture(true, mio_core::IntruderRetention::Session, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let checkbox = window.find("intruder-result-save-to-project");
        assert_eq!(checkbox.checked(), Some(false));
        assert_ne!(checkbox.disabled(), Some(true));
        window.click("intruder-result-save-to-project", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::SetIntruderRetention {
        project, run_id, expected_revision, retention: mio_core::IntruderRetention::Project, ..
    }] if *project == fixture.project && *run_id == started.summary.id && *expected_revision == started.summary.revision)
    );
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .session_runs
                .contains(&started.summary.id),
            "click does not claim promotion before the correlated receipt"
        )
    });
    assert!(!cx.did_prompt_for_new_path());
}

#[gpui_kit::test]
fn temporary_result_save_setting_is_disabled_without_opening_a_picker(cx: &mut TestAppContext) {
    for retention in [
        mio_core::IntruderRetention::Session,
        mio_core::IntruderRetention::Project,
    ] {
        let (mut fixture, started) = result_settings_fixture(false, retention, cx);
        let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
        cx.update_window(started.window, |_, window, cx| {
            window.render_frame(cx);
            let checkbox = window.find("intruder-result-save-to-project");
            assert_eq!(checkbox.checked(), Some(false));
            assert_eq!(checkbox.disabled(), Some(true));
            window.click("intruder-result-save-to-project", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!cx.did_prompt_for_new_path());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            assert_eq!(fixture.view.read(cx).workspace, workspace);
            assert_eq!(
                fixture.view.read(cx).intruder.results.views[&started.summary.id]
                    .summary
                    .as_ref(),
                Some(&started.summary)
            );
        });
    }
}

#[gpui_kit::test]
fn start_click_opens_each_latest_typed_run_and_positions_reads_its_frozen_spec(
    cx: &mut TestAppContext,
) {
    let original = attack();
    let tab_id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original], 64);
    click(fixture.window, "tool-intruder", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.editors[&tab_id]
            .target
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("http://127.0.0.1:9", cx);
    })
    .unwrap();
    cx.run_until_parked();
    for _ in 0..3 {
        let commands = drain(&mut fixture.commands);
        if commands.is_empty() {
            break;
        }
        let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
            panic!("typing may only save the draft")
        };
        emit(
            &fixture,
            Event::WorkspaceChanged {
                project: fixture.project,
                workspace: workspace.clone(),
            },
            cx,
        );
    }
    assert!(drain(&mut fixture.commands).is_empty());
    let first = start(&mut fixture, cx);
    assert_eq!(first.spec.draft.target, "http://127.0.0.1:9");
    let second = start(&mut fixture, cx);
    assert_ne!(first.window, second.window);
    assert_ne!(first.summary.id, second.summary.id);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    assert_eq!(workspace.intruder.tabs[0].last_run, Some(second.summary.id));
    click(first.window, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            project,
            run_id,
            selection_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("Positions reads only the chosen frozen run")
    };
    assert_eq!((*project, *run_id), (fixture.project, first.summary.id));
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: first.summary.id,
            selection_revision: *selection_revision,
            result: Ok(Box::new(first.spec.clone())),
        },
        cx,
    );
    cx.update_window(first.window, |_, window, cx| {
        window.render_frame(cx);
        let snapshot = window.find_by_label("Intruder 冻结 Positions");
        let before = snapshot
            .value()
            .expect("frozen editor exposes its text")
            .to_owned();
        let editor = snapshot.bounds().center();
        use gpui_kit::test::TestEventExt;
        window.pointer_down(editor, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(editor, gpui_kit::MouseButton::Left, cx);
        window.input("must remain readonly", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find_by_label("Intruder 冻结 Positions").value(),
            Some(before.as_str())
        );
    })
    .unwrap();
    click(first.window, "intruder-frozen-hex", cx);
    click(first.window, "intruder-frozen-raw", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(
            view.intruder.results.views[&first.summary.id]
                .spec
                .as_deref(),
            Some(&first.spec)
        );
        assert_eq!(view.intruder.results.views.len(), 2);
    });
    click(first.window, "intruder-results-results", cx);
    cx.update_window(first.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("intruder-results-table").visible());
        assert_eq!(
            window.find("intruder-results-results").selected(),
            Some(true)
        );
        assert_eq!(
            window.find("intruder-results-positions").selected(),
            Some(false)
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    close(&first, cx);
    assert!(!first.cancel.is_cancelled());
    assert!(!second.cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderResults { run_id, limit: 1, .. }] if *run_id == first.summary.id)
    );
    emit(
        &fixture,
        Event::IntruderStarted {
            project: fixture.project,
            run_id: first.summary.id,
            tab_id: first.summary.tab_id,
            spec_revision: first.summary.spec_revision,
            result: Ok(first.summary.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.intruder.results.views.len(), 1);
        assert_eq!(
            view.intruder.results.views[&second.summary.id].owner,
            second.window
        );
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(second.summary.id)
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn clicking_an_unloaded_result_retains_selection_until_its_bounded_page_arrives(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let first = flow(0);
    let selected = flow(1);
    started.summary.revision += 1;
    started.summary.dispatched = 2;
    page(
        &fixture,
        &started,
        started.probe,
        vec![row(0, &first)],
        Some(0),
        cx,
    );
    let (first_page, _) = query(&mut fixture, started.summary.id);
    // The byte limit can end a page before the viewport is full.
    page(
        &fixture,
        &started,
        first_page,
        vec![row(0, &first)],
        Some(0),
        cx,
    );
    cx.update_window(started.window, |_, window, cx| {
        window.click("table-cell-1:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (next_page, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, Some(0));
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(
            result.selected, None,
            "the stable ordinal is resolved only when the requested rank arrives"
        );
        assert_eq!(result.query.pending.as_ref().unwrap().query.offset, 1);
        assert!(result.detail.is_none());
    });
    page(
        &fixture,
        &started,
        next_page,
        vec![row(1, &selected)],
        None,
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision,
            flow_id,
        },
    ] = commands.as_slice()
    else {
        panic!("the original click must read its selected body after paging");
    };
    assert_eq!((*project, *flow_id), (fixture.project, selected.summary.id));
    detail(&fixture, *selection_revision, &selected, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail
                .as_deref(),
            Some(&selected)
        );
    });
}

#[gpui_kit::test]
fn production_row_clicks_lazy_read_latest_body_and_readonly_type_preserves_binary(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let first = flow(0);
    let mut binary = flow(1);
    binary.request.body = vec![0, 255, 7];
    binary.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\n\x00\xff", HttpVersion::Http11).unwrap());
    bootstrap(
        &mut fixture,
        &mut started,
        &[first.clone(), binary.clone()],
        cx,
    );
    let old = select(&mut fixture, &started, 0, &first, cx);
    let latest = select(&mut fixture, &started, 1, &binary, cx);
    assert!(latest > old);
    detail(&fixture, old, &first, cx);
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail
                .is_none()
        )
    });
    let raw_focus = cx
        .update_window(started.window, |_, window, cx| {
            let input = fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .raw
                .clone();
            window.click(("input", input.entity_id()), cx);
            let focus = input.focus_handle(cx);
            assert!(focus.is_focused(window));
            focus
        })
        .unwrap();
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    let main_focus = cx
        .update_window(fixture.window, |_, window, cx| {
            let view = fixture.view.read(cx);
            let input = view.tab_editors[&view.workspace.active.unwrap()]
                .request
                .state()
                .clone();
            window.click(("input", input.entity_id()), cx);
            let focus = input.focus_handle(cx);
            assert!(focus.is_focused(window));
            focus
        })
        .unwrap();
    detail(&fixture, latest, &binary, cx);
    cx.update_window(fixture.window, |_, window, _| {
        assert!(main_focus.is_focused(window))
    })
    .unwrap();
    cx.update_window(started.window, |_, window, cx| {
        assert!(!raw_focus.is_focused(window));
        assert!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .request
                .hex
                .focus_handle(cx)
                .is_focused(window)
        );
        let input = fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .request
            .state()
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("cannot edit captured request", cx);
        fixture.view.update(cx, |view, cx| {
            let result = &view.intruder.results.views[&started.summary.id];
            assert_eq!(result.request.view, MessageView::Hex);
            assert_eq!(
                result.request.bytes(window, cx).unwrap(),
                request_to_raw(&binary.request)
            );
        });
    })
    .unwrap();
    click(started.window, "intruder-result-response", cx);
    cx.update_window(started.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.results.views[&started.summary.id]
            .response
            .state()
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.input("cannot edit captured response", cx);
        fixture.view.update(cx, |view, cx| {
            let result = &view.intruder.results.views[&started.summary.id];
            assert_eq!(
                result.response.bytes(window, cx).unwrap(),
                response_to_raw(binary.response.as_ref().unwrap())
            );
        });
    })
    .unwrap();
    click(started.window, "intruder-result-raw", cx);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.response.view, MessageView::Hex);
        assert!(result.detail_error.as_ref().unwrap().contains("非 UTF-8"));
        assert_eq!(result.detail.as_deref(), Some(&binary));
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
