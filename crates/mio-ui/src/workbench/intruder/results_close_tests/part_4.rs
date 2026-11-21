use super::*;

#[gpui_kit::test]
fn discard_receipt_preserves_a_newer_admission_and_its_last_run(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    click(fixture.window, "intruder-start", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("the second Start must save before admission");
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
    let [
        Command::StartIntruder {
            run_id,
            tab_id,
            spec,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("second admission must still be awaiting its own core result");
    };
    let newer = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: *run_id,
        tab_id: *tab_id,
        title: String::new(),
        title_revision: 0,
        revision: 1,
        spec_revision: spec.revision,
        total: spec.validate().unwrap(),
        dispatched: 0,
        completed: 0,
        state: IntruderRunState::Running,
        started_unix_ms: 2,
        retention: mio_core::IntruderRetention::Project,
    };
    let newer_cancel = cancel.clone();
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    assert_eq!(
        discard.workspace.intruder.tabs[0].last_run,
        Some(started.summary.id)
    );
    emit(
        &fixture,
        Event::IntruderStarted {
            project: fixture.project,
            run_id: newer.id,
            tab_id: newer.tab_id,
            spec_revision: newer.spec_revision,
            result: Ok(newer.clone()),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderResults { run_id, limit: 1, .. }] if *run_id == newer.id)
    );
    let newer_window = cx.read(|cx| fixture.view.read(cx).intruder.results.views[&newer.id].owner);
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(newer_window.update(cx, |_, _, _| ()).is_ok());
    assert!(!newer_cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.intruder.tabs[0].last_run, Some(newer.id));
        assert_eq!(view.intruder.runs[&newer.id].summary, newer);
        assert_eq!(view.intruder.results.views[&newer.id].owner, newer_window);
    });
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 2);
    let saved = commands
        .iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("the newer last_run must be persisted by the normal workspace ACK hooks");
    assert_eq!(saved.intruder.tabs[0].last_run, Some(newer.id));
    assert_eq!(saved.revision, discard.workspace.revision + 1);
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::SaveWorkspace { .. } | Command::QueryHttpHistory { .. }
    )));
}

#[gpui_kit::test]
fn discard_close_main_ime_blocks_proposal_without_partial_harvest_or_retry(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let (target, raw) = cx
        .update_window(fixture.window, |_, window, cx| {
            let view = fixture.view.read(cx);
            let target = view.intruder.editors[&started.summary.tab_id]
                .target
                .clone();
            let raw = view.tab_editors[&view.workspace.active.unwrap()]
                .request
                .raw
                .clone();
            raw.update(cx, |state, cx| {
                state.set_value("POST /latest HTTP/1.1\r\n\r\n", window, cx)
            });
            target.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
                assert!(state.marked_text_range(window, cx).is_some());
            });
            (target, raw)
        })
        .unwrap();
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.update_window(fixture.window, |_, window, cx| {
        assert!(target.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, workspace);
        assert!(view.pending_workspace.is_none());
        assert!(view.intruder_result_close.is_none());
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
        assert_eq!(
            raw.read(cx).value().as_str(),
            "POST /latest HTTP/1.1\r\n\r\n"
        );
    })
    .unwrap();
    let revision = poll(&mut fixture, &started, cx);
    page(&fixture, &started, revision, vec![], None, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "terminal refresh cannot retry a failed deletion"
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn discard_failure_preserves_results_inputs_and_does_not_hijack_later_save(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let flow = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&flow), cx);
    let selected = select(&mut fixture, &started, 0, &flow, cx);
    detail(&fixture, selected, &flow, cx);
    click(started.window, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        panic!("the selected frozen spec must have its own read identity");
    };
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: started.summary.id,
            selection_revision: *selection_revision,
            result: Ok(Box::new(started.spec.clone())),
        },
        cx,
    );
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    page(&fixture, &started, revision, vec![row(0, &flow)], None, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let target = type_target(&mut fixture, true, "http://127.0.0.1:11", cx);
    emit(
        &fixture,
        reply(&discard, Err("disk write denied".into())),
        cx,
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(*view.workspace, workspace);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
        assert_eq!(
            result
                .query
                .page
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .filter_map(|row| row.result.clone())
                .collect::<Vec<_>>(),
            vec![row(0, &flow)]
        );
        assert_eq!(result.detail.as_deref(), Some(&flow));
        assert_eq!(result.spec.as_deref(), Some(&started.spec));
        assert!(
            result
                .close_error
                .as_ref()
                .unwrap()
                .contains("disk write denied")
        );
        assert_eq!(target.read(cx).value().as_str(), "http://127.0.0.1:11");
    });
    assert!(drain(&mut fixture.commands).is_empty());
    let revision = poll(&mut fixture, &started, cx);
    page(&fixture, &started, revision, vec![row(0, &flow)], None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        fixture
            .view
            .update(cx, |view, cx| assert!(view.flush_workspace(window, cx)));
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("a later explicit save must be independent of the failed discard");
    };
    assert_eq!(workspace.revision, discard.workspace.revision);
    assert_eq!(
        workspace.intruder.tabs[0].spec.draft.target,
        "http://127.0.0.1:11"
    );
    let saved = workspace.clone();
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(saved.revision));
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft.target,
            "http://127.0.0.1:11"
        );
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved,
        },
        cx,
    );
    cx.read(|cx| assert!(fixture.view.read(cx).pending_workspace.is_none()));
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn discard_queue_failure_after_terminal_never_closes_or_retries(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 4);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let capacity = fill_commands(&fixture, cx);
    stopped(&fixture, &mut started, revision, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), capacity);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryIntruderRuns { limit: 1, .. }))
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, workspace);
        assert!(view.pending_workspace.is_none());
        assert!(view.intruder_result_close.is_none());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("队列已满")
        );
    });
    let revision = poll(&mut fixture, &started, cx);
    page(&fixture, &started, revision, vec![], None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn closed_command_channel_still_cancels_selected_run_and_retains_window(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    fixture.commands.close();
    ask_to_close(&started, false, cx);
    answer_close_dialog("丢弃", cx);
    cx.run_until_parked();
    assert!(started.cancel.is_cancelled());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("断开")
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn lost_event_channel_never_treats_pending_discard_as_acknowledged(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let (replacement, _receiver) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        assert_eq!(*view.workspace, workspace);
        assert!(view.workspace.revision < discard.workspace.revision);
        assert!(view.workspace_dirty);
        assert_eq!(view.pending_workspace, Some(discard.workspace.revision));
        assert!(view.intruder_close_owns_workspace());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("未确认")
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn late_close_answers_do_not_target_new_project_or_replacement_window(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut old = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut old, &[], cx);
    ask_to_close(&old, false, cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    fixture.project.generation += 1;
    emit(
        &fixture,
        Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace,
        },
        cx,
    );
    assert!(old.window.update(cx, |_, _, _| ()).is_err());
    drain(&mut fixture.commands);
    let mut current = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut current, &[], cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    assert!(
        !has_close_dialog(cx),
        "project replacement retires the old close dialog"
    );
    assert!(old.cancel.is_cancelled());
    assert!(!current.cancel.is_cancelled());
    assert!(current.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    ask_to_close(&current, false, cx);
    cx.update_window(current.window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, cx| {
            view.open_intruder_results_window(current.summary.id, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let replacement =
        cx.read(|cx| fixture.view.read(cx).intruder.results.views[&current.summary.id].owner);
    assert_ne!(replacement, current.window);
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderResults { run_id, limit: 1, .. }] if *run_id == current.summary.id)
    );
    assert!(
        !has_close_dialog(cx),
        "result replacement retires its old close dialog"
    );
    assert!(replacement.update(cx, |_, _, _| ()).is_ok());
    assert!(!current.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, workspace);
        assert_eq!(
            view.intruder.results.views[&current.summary.id].owner,
            replacement
        );
        assert!(view.intruder_result_close.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
