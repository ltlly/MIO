use super::*;

#[gpui_kit::test]
fn committed_discard_invalidates_replacement_reads_but_closes_only_original_owner(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let flow = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&flow), cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    page(&fixture, &started, revision, vec![row(0, &flow)], None, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    cx.update_window(started.window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, cx| {
            view.open_intruder_results_window(started.summary.id, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let replacement =
        cx.read(|cx| fixture.view.read(cx).intruder.results.views[&started.summary.id].owner);
    assert_ne!(replacement, started.window);
    let (revision, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, revision, vec![row(0, &flow)], None, cx);
    started.window = replacement;
    let body = select(&mut fixture, &started, 0, &flow, cx);
    click(replacement, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        panic!("replacement frozen spec must be pending");
    };
    let spec = *selection_revision;
    let revision = poll(&mut fixture, &started, cx);
    let mut committed = receipt(&discard);
    committed.deleted_flows = 1;
    emit(&fixture, reply(&discard, Ok(committed)), cx);
    assert!(replacement.update(cx, |_, _, _| ()).is_ok());
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
    detail(&fixture, body, &flow, cx);
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: started.summary.id,
            selection_revision: spec,
            result: Ok(Box::new(started.spec.clone())),
        },
        cx,
    );
    page(&fixture, &started, revision, vec![row(0, &flow)], None, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(result.owner, replacement);
        assert!(
            result.pending.is_none()
                && result.query.pending.is_none()
                && result.spec_pending.is_none()
        );
        assert!(result.detail.is_none() && result.spec.is_none() && result.rows.is_empty());
        assert!(result.query.page.is_none());
        assert!(result.error.as_ref().unwrap().contains("已丢弃"));
        assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
        assert_eq!(view.workspace.revision, discard.workspace.revision);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn pending_result_close_keeps_native_quit_behind_existing_protection(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let probe = Probe::installed();
    ask_to_close(&started, true, cx);
    cx.simulate_keystrokes(started.window, crate::kbd!("q"));
    pump_native(cx);
    assert_eq!(probe.replies(), [false]);
    assert!(has_close_dialog(cx));
    assert!(!started.cancel.is_cancelled());
    answer_close_dialog("丢弃", cx);
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::StopIntruder { .. }]
    ));
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert_eq!(probe.replies(), [false, false]);
    assert!(!cx.has_pending_prompt());
    assert!(!has_close_dialog(cx));
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    cx.simulate_keystrokes(started.window, crate::kbd!("q"));
    pump_native(cx);
    assert_eq!(probe.replies(), [false, false, false]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.shutdown.is_cancelled() && !view.shutdown_requested && !view.quitting);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(view.intruder_close_owns_workspace());
    });
    emit(&fixture, reply(&discard, Err("discard refused".into())), cx);
    cx.simulate_keystrokes(started.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "a finished close failure must release, not bypass, normal Quit confirmation"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert_eq!(probe.replies(), [false, false, false, false]);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn an_existing_native_quit_prompt_blocks_a_second_running_close_prompt(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    let mut native = gpui_kit::VisualTestContext::from_window(started.window, cx);
    assert!(!native.simulate_close());
    cx.run_until_parked();
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("当前操作")
        );
    });
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    close(&started, cx);
    assert!(!started.cancel.is_cancelled());
}

#[gpui_kit::test]
fn completion_during_close_prompt_still_targets_that_run_and_requires_main_owner(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    ask_to_close(&started, false, cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    page(&fixture, &started, revision, vec![], None, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(has_close_dialog(cx));
    cx.update_window(fixture.window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(
        !has_close_dialog(cx),
        "owner destruction retires the close confirmation"
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.pending_workspace.is_none());
        assert!(view.intruder_result_close.is_none());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("主工作窗口")
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn terminal_completion_during_prompt_still_needs_its_actual_discard_receipt(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    ask_to_close(&started, true, cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    page(&fixture, &started, revision, vec![], None, cx);
    assert!(has_close_dialog(cx));
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    answer_close_dialog("丢弃", cx);
    cx.run_until_parked();
    let discard = take_discard(&mut fixture, &started, cx);
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
        assert_eq!(view.workspace.revision, discard.workspace.revision);
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
}

#[gpui_kit::test]
fn discard_receipt_with_late_main_ime_preserves_composition_and_blocks_old_pending_send(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press("cmd-enter", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let raw = cx
        .update_window(fixture.window, |_, window, cx| {
            let view = fixture.view.read(cx);
            let id = view.workspace.active.unwrap();
            assert_eq!(
                view.pending_sends[&id].required_workspace_revision,
                discard.workspace.revision
            );
            let raw = view.tab_editors[&id].request.raw.clone();
            raw.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
                assert!(state.marked_text_range(window, cx).is_some());
            });
            raw
        })
        .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(raw.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
        let view = fixture.view.read(cx);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
        assert_eq!(view.workspace.revision, discard.workspace.revision);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    // The discarded attack never sent the main Repeater draft.
    let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
    assert!(matches!(
        commands.as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
}

#[gpui_kit::test]
fn inactive_main_editor_composition_also_blocks_discard_before_any_workspace_write(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let inactive = cx.read(|cx| fixture.view.read(cx).workspace.active.unwrap());
    emit(
        &fixture,
        Event::RepeaterOpened {
            project: fixture.project,
            tab: crate::workbench::tests::tab("another draft"),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("the additional draft must use the existing workspace writer");
    };
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    // The new draft's navigation query is released only after this save ACK.
    assert!(crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx).is_empty());
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let target = cx
        .update_window(fixture.window, |_, window, cx| {
            let view = fixture.view.read(cx);
            assert_ne!(view.workspace.active, Some(inactive));
            assert!(!view.tab_editors[&inactive].dirty);
            let target = view.tab_editors[&inactive].target.clone();
            target.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
            target
        })
        .unwrap();
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(target.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.pending_workspace.is_none());
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn malformed_discard_receipt_keeps_ownership_until_a_valid_receipt_reconciles(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let mut wrong = receipt(&discard);
    wrong.workspace.tabs[0].draft.target = "http://127.0.0.1:99".into();
    emit(&fixture, reply(&discard, Ok(wrong)), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(view.pending_workspace, Some(discard.workspace.revision));
        assert!(view.intruder_close_owns_workspace());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("不一致")
        );
    });
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, receipt(&discard).workspace);
        assert!(view.pending_workspace.is_none());
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
}

#[gpui_kit::test]
fn terminal_read_failure_releases_close_gate_without_deleting_or_retrying(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: revision,
            result: Err("terminal read failed".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.intruder_result_close.is_none());
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("terminal read failed")
        );
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
}
