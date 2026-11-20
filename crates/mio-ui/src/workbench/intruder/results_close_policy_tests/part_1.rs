use super::*;

#[gpui_kit::test]
fn close_policy_running_continue_closes_only_view_and_keeps_background_run(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Continue,
            FinishedAttackClose::Ask,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    close_without_prompt(&started, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!started.cancel.is_cancelled());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::QueryIntruderResults { project, run_id, after: None, limit: 1, .. }] if *project == fixture.project && *run_id == started.summary.id)
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert!(view.intruder_result_close.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn close_policy_running_delete_is_frozen_while_preferences_change(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::KeepMemory,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    close_without_prompt(&started, cx);
    assert!(started.cancel.is_cancelled());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::StopIntruder { project, run_id }] if *project == fixture.project && *run_id == started.summary.id)
    );
    let prior_query_revision = cx.read(|cx| fixture.view.read(cx).intruder.results.query_revision);
    let commands = preferences(
        &mut fixture,
        RunningAttackClose::Continue,
        FinishedAttackClose::SaveProject,
        2,
        None,
        cx,
    );
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after,
            limit,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "the preferences ACK may only flush one unfiltered status probe for the stopping attack"
        );
    };
    assert_eq!(
        (*project, *run_id, *query_revision, *after, *limit),
        (
            fixture.project,
            started.summary.id,
            prior_query_revision + 1,
            None,
            1
        )
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(result.pending, Some(*query_revision));
        assert!(result.rows.is_empty());
        assert!(result.query.page.as_ref().unwrap().rows.is_empty());
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert!(started.cancel.is_cancelled());
    });
    let revision = *query_revision;
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).iter().all(|command| matches!(
        command,
        Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. }
    )));
}

#[gpui_kit::test]
fn close_policy_running_ask_keeps_its_prompt_when_preferences_change(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Ask,
            FinishedAttackClose::Ask,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    ask_to_close(&started, true, cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::Delete,
            2,
            None,
            cx,
        )
        .is_empty()
    );
    assert!(has_close_dialog(cx));
    assert!(!started.cancel.is_cancelled());
    answer_close_dialog("在后台继续运行", cx);
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!started.cancel.is_cancelled());
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::QueryIntruderResults { project, run_id, after: None, limit: 1, .. }]
            if *project == fixture.project && *run_id == started.summary.id));
}

#[gpui_kit::test]
fn close_policy_finished_keep_waits_for_retention_receipt(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    completed(&mut fixture, &mut started, cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::KeepMemory,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    close_without_prompt(&started, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    let [
        command @ Command::SetIntruderRetention {
            retention: mio_core::IntruderRetention::Session,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("KeepMemory must await its Session receipt");
    };
    emit(&fixture, retain_reply(command, &started.summary), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .session_runs
                .contains(&started.summary.id)
        )
    });
}

#[gpui_kit::test]
fn explicit_save_waits_for_named_project_ack_when_close_preferences_change(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    completed(&mut fixture, &mut started, cx);
    let path = PathBuf::from("/tmp/mio-policy-save-fixture.mio");
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, _| view.project_path = Some(path.clone()))
    });
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Continue,
            FinishedAttackClose::SaveProject,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    save_started_from_dashboard(&mut fixture, &started, cx);
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }] if *project == fixture.project)
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Delete,
            FinishedAttackClose::Delete,
            2,
            None,
            cx,
        )
        .is_empty()
    );
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        },
        cx,
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    close_without_prompt(&started, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
}

#[gpui_kit::test]
fn close_policy_finished_save_keeps_temporary_results_without_changing_preferences(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    completed(&mut fixture, &mut started, cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Continue,
            FinishedAttackClose::SaveProject,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    let (mut workspace, configured) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none());
        (
            view.workspace.clone(),
            view.user_preferences.confirmed.clone(),
        )
    });
    close_without_prompt(&started, cx);
    assert!(!cx.did_prompt_for_new_path());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    let [
        command @ Command::SetIntruderRetention {
            project,
            run_id,
            expected_revision,
            retention: mio_core::IntruderRetention::Session,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!(
            "temporary Save policy must only submit Session retention, not a picker, project save, or preference write"
        );
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (
            fixture.project,
            started.summary.id,
            started.summary.revision
        )
    );
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, workspace));
    emit(&fixture, retain_reply(command, &started.summary), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!cx.did_prompt_for_new_path());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(!started.cancel.is_cancelled());
    workspace.revision += 1;
    workspace.intruder.tabs[0].last_run = None;
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert!(view.project_path.is_none());
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(view.user_preferences.confirmed, configured);
        assert_eq!(
            view.intruder_close_preferences(),
            (
                RunningAttackClose::Continue,
                FinishedAttackClose::SaveProject
            )
        );
        assert!(view.intruder_result_close.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| match command {
                Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. } => true,
                Command::QueryIntruderResults {
                    project,
                    run_id,
                    after: None,
                    limit: 1,
                    ..
                } => {
                    *project == fixture.project && *run_id == started.summary.id
                }
                _ => false,
            })
    );
}

#[gpui_kit::test]
fn close_policy_finished_delete_failure_preserves_window_and_requires_retry(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    completed(&mut fixture, &mut started, cx);
    assert!(
        preferences(
            &mut fixture,
            RunningAttackClose::Continue,
            FinishedAttackClose::Delete,
            1,
            None,
            cx,
        )
        .is_empty()
    );
    close_without_prompt(&started, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    emit(
        &fixture,
        reply(&discard, Err("synthetic discard denied".into())),
        cx,
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("synthetic discard denied")
        );
    });
}
