use super::*;

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn native_close_and_close_action_share_one_paused_choice(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let operation = pause_command(&mut fixture, &started, true, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    emit(&fixture, control_event(&fixture, &started, operation), cx);
    let (revision, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, revision, vec![], None, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let probe = Probe::installed();
    ask_to_close(&started, true, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx);
        window.dispatch_action(Box::new(CloseTab), cx);
        window.render_frame(cx);
        assert_eq!(window.find("intruder-result-pause").disabled(), Some(true));
        assert!(window.try_find("intruder-result-stop").is_none());
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(probe.terminations().is_empty());
    answer_close_dialog("在后台继续运行", cx);
    cx.run_until_parked();
    assert!(!has_close_dialog(cx));
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert!(
            !view
                .intruder
                .results
                .views
                .contains_key(&started.summary.id)
        );
        assert!(view.intruder_result_close.is_none());
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            run_id,
            query_revision,
            limit: 1,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Continue must retain a background status probe without Resume or Start");
    };
    assert_eq!(*run_id, started.summary.id);
    page(&fixture, &started, *query_revision, vec![], None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn discard_close_cancels_only_selected_token_before_full_queue_and_waits_for_receipt(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 4);
    let mut first = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut first, &[], cx);
    let mut second = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut second, &[], cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let capacity = fill_commands(&fixture, cx);
    assert_eq!(capacity, 4);
    ask_to_close(&first, true, cx);
    assert!(!first.cancel.is_cancelled() && !second.cancel.is_cancelled());
    answer_close_dialog("丢弃", cx);
    cx.run_until_parked();
    assert!(first.cancel.is_cancelled());
    assert!(!second.cancel.is_cancelled());
    let full = drain(&mut fixture.commands);
    assert_eq!(full.len(), capacity);
    assert!(
        full.iter()
            .all(|command| matches!(command, Command::QueryIntruderRuns { limit: 1, .. }))
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.intruder.runs.contains_key(&first.summary.id));
        let result = &view.intruder.results.views[&first.summary.id];
        assert!(result.stop_requested);
        assert!(result.control_error.as_ref().unwrap().contains("未入队"));
        assert!(result.close_status.unwrap().contains("等待终态"));
    });
    cx.update_window(first.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(first.window, cx);
    assert!(!native.simulate_close());
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert!(!has_close_dialog(cx));
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let polls = drain(&mut fixture.commands);
    assert_eq!(polls.len(), 2);
    let revisions: HashMap<_, _> = polls
        .iter()
        .map(|command| match command {
            Command::QueryIntruderResults {
                run_id,
                query_revision,
                limit: 1,
                ..
            } if *run_id == first.summary.id => (*run_id, *query_revision),
            Command::QueryIntruderResultView { query, cancel } if query.run_id == second.summary.id => {
                assert_eq!((query.project, query.offset, query.limit), (fixture.project, 0, PAGE_SIZE));
                assert!(!cancel.is_cancelled());
                (query.run_id, query.query_revision)
            }
            _ => panic!("only a Stop probe for the first run and a display query for the second may follow backpressure"),
        })
        .collect();
    first.summary.revision += 1;
    first.summary.state = IntruderRunState::Stopped;
    page(&fixture, &first, first.probe, vec![], None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .runs
                .contains_key(&first.summary.id)
        )
    });
    page(
        &fixture,
        &second,
        revisions[&second.summary.id],
        vec![],
        None,
        cx,
    );
    page(
        &fixture,
        &first,
        revisions[&first.summary.id],
        vec![],
        None,
        cx,
    );
    let discard = take_discard(&mut fixture, &first, cx);
    assert_eq!(
        discard.workspace.intruder.tabs[0].last_run,
        Some(second.summary.id)
    );
    let committed = receipt(&discard);
    let mut wrong_project = fixture.project;
    wrong_project.generation += 1;
    for event in [
        Event::IntruderDiscarded {
            project: wrong_project,
            run_id: discard.run_id,
            operation_id: discard.operation_id,
            result: Ok(committed.clone()),
        },
        Event::IntruderDiscarded {
            project: discard.project,
            run_id: second.summary.id,
            operation_id: discard.operation_id,
            result: Ok(committed.clone()),
        },
        Event::IntruderDiscarded {
            project: discard.project,
            run_id: discard.run_id,
            operation_id: Uuid::new_v4(),
            result: Ok(committed.clone()),
        },
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: committed.workspace.clone(),
        },
        Event::WorkspaceRejected {
            project: fixture.project,
            revision: discard.workspace.revision,
            stored: workspace.clone(),
            message: "unrelated stale save rejection".into(),
        },
    ] {
        emit(&fixture, event, cx);
        assert!(first.window.update(cx, |_, _, _| ()).is_ok());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.workspace, workspace);
            assert_eq!(view.pending_workspace, Some(discard.workspace.revision));
        });
        assert!(drain(&mut fixture.commands).is_empty());
    }
    emit(&fixture, reply(&discard, Ok(committed)), cx);
    assert!(first.window.update(cx, |_, _, _| ()).is_err());
    assert!(second.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!second.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(second.summary.id)
        );
        assert_eq!(view.workspace.revision, discard.workspace.revision);
        assert_eq!(view.intruder.results.views.len(), 1);
        assert_eq!(
            view.intruder.results.views[&second.summary.id].owner,
            second.window
        );
        assert_eq!(
            view.intruder.runs[&second.summary.id].summary,
            second.summary
        );
        assert!(view.pending_workspace.is_none());
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
}

#[gpui_kit::test]
fn discard_close_rejects_overtaken_terminal_page_and_late_resume(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let pause = pause_command(&mut fixture, &started, true, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    emit(&fixture, control_event(&fixture, &started, pause), cx);
    let (revision, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, revision, vec![], None, cx);
    let resume = pause_command(&mut fixture, &started, false, cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    let mut resumed = started.summary.clone();
    resumed.revision += 1;
    resumed.state = IntruderRunState::Running;
    emit(
        &fixture,
        Event::IntruderRunControl {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: resume,
            result: Ok(resumed),
        },
        cx,
    );
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert!(result.control_pending.is_none());
        assert!(result.stop_requested);
        assert_eq!(
            result.summary.as_ref().unwrap().state,
            IntruderRunState::Paused
        );
    });
    cx.update_window(fixture.window, |_, _, cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_intruder_runs(None, cx));
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderRuns { query_revision, .. }] = commands.as_slice() else {
        panic!("the run list is an independent source of current summaries");
    };
    started.summary.revision += 2;
    started.summary.state = IntruderRunState::Stopping;
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(IntruderRunPage {
                rows: vec![started.summary.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    let mut old = started.summary.clone();
    old.revision -= 1;
    old.state = IntruderRunState::Stopped;
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: revision,
            result: Ok(IntruderResultPage {
                run: old,
                rows: vec![],
                next_after: None,
                persisted: true,
            }),
        },
        cx,
    );
    let (latest, _) = query(&mut fixture, started.summary.id);
    assert!(latest > revision);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert!(view.pending_workspace.is_none());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .error
                .is_none()
        );
    });
    stopped(&fixture, &mut started, latest, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    assert_eq!(discard.expected_revision, started.summary.revision);
    assert!(started.cancel.is_cancelled());
    emit(
        &fixture,
        reply(&discard, Err("selected-run deletion failed".into())),
        cx,
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn discard_close_waits_for_existing_save_then_captures_latest_main_window_drafts(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    cx.update_window(fixture.window, |_, _, cx| {
        fixture
            .view
            .update(cx, |view, cx| assert!(view.save_workspace(cx)));
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("an existing workspace writer must be pending");
    };
    let saved = workspace.clone();
    let intruder = type_target(&mut fixture, true, "http://127.0.0.1:9", cx);
    let repeater = type_target(&mut fixture, false, "http://127.0.0.1:8", cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(saved.revision));
        assert_ne!(
            view.workspace.intruder.tabs[0].spec.draft.target,
            intruder.read(cx).value().as_str()
        );
        assert_ne!(
            view.workspace.tabs[0].draft.target,
            repeater.read(cx).value().as_str()
        );
    });
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved.clone(),
        },
        cx,
    );
    let discard = take_discard(&mut fixture, &started, cx);
    assert_eq!(discard.workspace.revision, saved.revision + 1);
    assert_eq!(
        discard.workspace.intruder.tabs[0].spec.draft.target,
        "http://127.0.0.1:9"
    );
    assert_eq!(discard.workspace.tabs[0].draft.target, "http://127.0.0.1:8");
    assert_eq!(
        discard.workspace.intruder.tabs[0].last_run,
        Some(started.summary.id)
    );
    type_target(&mut fixture, true, "http://127.0.0.1:7", cx);
    type_target(&mut fixture, false, "http://127.0.0.1:6", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 2);
    let next = commands
        .iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        })
        .expect("the real workspace ACK must save newer edits, not replace them with its proposal");
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::SaveWorkspace { .. } | Command::QueryHttpHistory { .. }
    )));
    assert_eq!(next.revision, discard.workspace.revision + 1);
    assert_eq!(next.intruder.tabs[0].last_run, None);
    assert_eq!(
        next.intruder.tabs[0].spec.draft.target,
        "http://127.0.0.1:7"
    );
    assert_eq!(next.tabs[0].draft.target, "http://127.0.0.1:6");
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: next,
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none() && !view.workspace_dirty);
        assert_eq!(intruder.read(cx).value().as_str(), "http://127.0.0.1:7");
        assert_eq!(repeater.read(cx).value().as_str(), "http://127.0.0.1:6");
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
