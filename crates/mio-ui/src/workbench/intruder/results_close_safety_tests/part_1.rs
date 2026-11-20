use super::*;

#[gpui_kit::test]
fn dashboard_task_delete_after_continue_waits_for_terminal_without_creating_a_view(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    ask_to_close(&started, false, cx);
    answer_close_dialog("在后台继续运行", cx);
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!started.cancel.is_cancelled());
    drain(&mut fixture.commands);
    click(fixture.window, "tool-dashboard", cx);
    let list_revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryIntruderRuns { query_revision, .. } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: list_revision,
            result: Ok(IntruderRunPage {
                rows: vec![started.summary.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    drain(&mut fixture.commands);
    dashboard_task_menu(&fixture, started.summary.id, "删除", cx);
    cx.simulate_prompt_answer("否");
    cx.run_until_parked();
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    dashboard_task_menu(&fixture, started.summary.id, "删除", cx);
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    assert!(started.cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    assert!(commands.iter().any(|command| matches!(command, Command::StopIntruder { run_id, .. } if *run_id == started.summary.id)));
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::StopIntruder { project, run_id }
            | Command::QueryIntruderResults { project, run_id, after: None, limit: 1, .. }
            if *project == fixture.project && *run_id == started.summary.id
    )));
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.results.views.is_empty()));
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    // A completed background probe must reach the task even after its Dashboard page disappeared.
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.query_intruder_runs(None, cx);
            view.intruder.results.runs.rows.clear();
            view.refresh_intruder_results(window, cx);
        });
    })
    .unwrap();
    let revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryIntruderRuns { query_revision, .. } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: revision,
            result: Ok(IntruderRunPage {
                rows: vec![started.summary.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let discard = commands
        .iter()
        .find_map(|command| match command {
            Command::DiscardIntruder {
                project,
                run_id,
                operation_id,
                expected_revision,
                workspace,
            } => Some(Discard {
                project: *project,
                run_id: *run_id,
                operation_id: *operation_id,
                expected_revision: *expected_revision,
                workspace: workspace.clone(),
            }),
            _ => None,
        })
        .expect("only an actual terminal receipt permits discard");
    assert_eq!(discard.expected_revision, started.summary.revision);
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.results.views.is_empty());
        assert!(view.intruder_result_close.is_none());
        assert!(view.tool == Tool::Repeater);
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn dashboard_task_delete_late_ack_keeps_main_and_other_task_after_tool_change(
    cx: &mut TestAppContext,
) {
    let (mut fixture, first, second) = dashboard_tasks(cx);
    dashboard_task_menu(&fixture, first.id, "删除", cx);
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::DiscardIntruder {
            project,
            run_id,
            operation_id,
            expected_revision,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("delete must await its own discard receipt");
    };
    let discard = Discard {
        project: *project,
        run_id: *run_id,
        operation_id: *operation_id,
        expected_revision: *expected_revision,
        workspace: workspace.clone(),
    };
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(reply(&discard, Ok(receipt(&discard))), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Repeater);
        assert!(view.intruder_result_close.is_none());
        assert!(!view.intruder.session_runs.contains(&first.id));
        assert!(view.intruder.session_runs.contains(&second.id));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("先前等待已结束"))
        );
        assert!(
            view.intruder
                .results
                .views
                .values()
                .all(|result| result.run_id == second.id)
        );
        assert!(view.pending_workspace.is_none());
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    let commands = drain(&mut fixture.commands);
    // Tool switching releases a read-only page for the unsent main draft.
    let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. }
    )));
}

#[gpui_kit::test]
fn dashboard_task_delete_prompt_cannot_apply_yes_to_a_replaced_project(cx: &mut TestAppContext) {
    let (mut fixture, first, _) = dashboard_tasks(cx);
    dashboard_task_menu(&fixture, first.id, "删除", cx);
    assert!(cx.has_pending_prompt());
    let replacement = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    emit(
        &fixture,
        Event::ProjectOpened {
            project: replacement,
            path: None,
            workspace: Workspace::default(),
        },
        cx,
    );
    drain(&mut fixture.commands);
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(replacement));
        assert_eq!(view.workspace, Workspace::default());
        assert!(view.intruder_result_close.is_none());
        assert!(view.intruder.session_runs.is_empty());
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn discard_rejection_cancels_waiting_repeater_send_before_later_workspace_acks(
    cx: &mut TestAppContext,
) {
    let (mut fixture, selected, other, discard) = discarding_with_another_live_run(cx);
    crate::workbench::tests::settings::activate_empty_repeater(&mut fixture, cx);
    click(fixture.window, "repeater-send", cx);
    let draft = cx.read(|cx| {
        let view = fixture.view.read(cx);
        let tab_id = view.workspace.active.unwrap();
        assert_eq!(view.pending_sends.len(), 1);
        assert_eq!(
            view.pending_sends[&tab_id].required_workspace_revision,
            discard.workspace.revision
        );
        assert!(view.sending.is_empty());
        view.pending_sends[&tab_id].draft.clone()
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        reply(&discard, Err("discard write denied".into())),
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.pending_sends.is_empty(),
            "a rejected discard CAS must cancel the Send waiting for its workspace ACK"
        );
        assert!(view.sending.is_empty() && view.intruder.starts.is_empty());
        assert!(view.pending_workspace.is_none() && view.workspace_dirty);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(view.workspace.tabs[0].draft, draft);
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(other.summary.id)
        );
        assert_eq!(view.intruder.runs[&other.summary.id].summary, other.summary);
    });
    assert!(selected.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    for revision in discard.workspace.revision..=discard.workspace.revision + 1 {
        let saved = save_workspace_without_reviving_requests(&mut fixture, revision, cx);
        assert_eq!(saved.tabs[0].draft, draft);
        assert_eq!(saved.intruder.tabs[0].last_run, Some(other.summary.id));
    }
    assert!(!other.cancel.is_cancelled());
    assert!(other.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn discard_rejection_cancels_waiting_intruder_start_before_later_workspace_acks(
    cx: &mut TestAppContext,
) {
    let (mut fixture, selected, other, discard) = discarding_with_another_live_run(cx);
    click(fixture.window, "tool-intruder", cx);
    click(fixture.window, "intruder-start", cx);
    let (pending_id, pending_cancel) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.intruder.starts.len(), 1);
        let pending = &view.intruder.starts[&selected.summary.tab_id];
        assert!(pending.phase == StartPhase::Saving);
        assert_eq!(pending.after_revision, discard.workspace.revision);
        assert_eq!(pending.spec, selected.spec);
        assert!(!pending.cancel.is_cancelled());
        (pending.run_id, pending.cancel.clone())
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        reply(&discard, Err("discard write denied".into())),
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.intruder.starts.is_empty(),
            "a rejected discard CAS must remove the Start waiting for a later workspace ACK"
        );
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.pending_workspace.is_none() && view.workspace_dirty);
        assert!(view.intruder_result_close.is_none());
        assert!(!view.intruder.runs.contains_key(&pending_id));
        assert_eq!(view.intruder.runs[&other.summary.id].summary, other.summary);
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(other.summary.id)
        );
    });
    assert!(pending_cancel.is_cancelled());
    assert!(selected.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    for revision in discard.workspace.revision..=discard.workspace.revision + 1 {
        let saved = save_workspace_without_reviving_requests(&mut fixture, revision, cx);
        assert_eq!(saved.intruder.tabs[0].spec, selected.spec);
        assert_eq!(saved.intruder.tabs[0].last_run, Some(other.summary.id));
    }
    assert!(!other.cancel.is_cancelled());
    assert!(other.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn session_keep_rejection_preserves_latest_inputs_without_refresh_or_save_retry(
    cx: &mut TestAppContext,
) {
    let mut attack = attack();
    attack.spec.payload_sets[0].truncate(1);
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let mut started = start(&mut fixture, cx);
    let flow = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&flow), cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    page(&fixture, &started, revision, vec![row(0, &flow)], None, cx);
    let selected = select(&mut fixture, &started, 0, &flow, cx);
    detail(&fixture, selected, &flow, cx);
    type_target(&mut fixture, true, "http://127.0.0.1:17", cx);
    ask_to_close(&started, false, cx);
    answer_close_dialog("保留在内存", cx);
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderRetention {
            project,
            run_id,
            operation_id,
            expected_revision,
            retention,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("Keep must submit one correlated retention CAS, not close before its receipt");
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    assert_eq!(*expected_revision, started.summary.revision);
    assert_eq!(*retention, mio_core::IntruderRetention::Session);
    assert_eq!(
        workspace.intruder.tabs[0].spec.draft.target,
        "http://127.0.0.1:17"
    );
    let operation_id = *operation_id;
    let revision = workspace.revision;
    let before = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let target = type_target(&mut fixture, true, "http://127.0.0.1:18", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::IntruderRetentionChanged {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id,
            result: Err("session copy denied".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(view.workspace, before);
        assert!(view.pending_workspace.is_none() && view.workspace_dirty);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(
            result.summary.as_ref().unwrap().retention,
            mio_core::IntruderRetention::Project
        );
        let displayed = result
            .query
            .page
            .as_ref()
            .expect("the displayed result page remains");
        assert_eq!(displayed.total, 1);
        assert_eq!(displayed.rows.len(), 1);
        assert_eq!(displayed.rows[0].ordinal, 0);
        assert_eq!(displayed.rows[0].result, Some(row(0, &flow)));
        assert_eq!(result.detail.as_deref(), Some(&flow));
        assert!(
            result
                .close_error
                .as_ref()
                .unwrap()
                .contains("session copy denied")
        );
        assert_eq!(target.read(cx).value().as_str(), "http://127.0.0.1:18");
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
    });
    assert!(!started.cancel.is_cancelled());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    let refresh = poll(&mut fixture, &started, cx);
    page(&fixture, &started, refresh, vec![row(0, &flow)], None, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "refresh must not repeat a failed Keep"
    );
    let saved = save_workspace_without_reviving_requests(&mut fixture, revision, cx);
    assert_eq!(
        saved.intruder.tabs[0].spec.draft.target,
        "http://127.0.0.1:18"
    );
    assert_eq!(saved.intruder.tabs[0].last_run, Some(started.summary.id));
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
}
