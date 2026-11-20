use super::*;

#[gpui_kit::test]
fn already_session_result_close_keeps_session_without_rewriting(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    started.summary.retention = mio_core::IntruderRetention::Session;
    page(&fixture, &started, revision, vec![], None, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    ask_to_close(&started, false, cx);
    answer_close_dialog("保留在内存", cx);
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!has_close_dialog(cx));
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, workspace));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn terminal_close_asks_before_preserving_or_discarding_results(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    ask_to_close(&started, true, cx);
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    answer_close_dialog("保留在内存", cx);
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::SetIntruderRetention {
        retention: mio_core::IntruderRetention::Session, run_id, ..
    }] if *run_id == started.summary.id)
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn stopping_close_deadline_preserves_results_and_requires_a_new_explicit_choice(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(31));
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.intruder_result_close.is_none(),
            "a stalled terminal wait must end with a visible failure"
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("超时")
        );
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderResultView { query, cancel }] = commands.as_slice() else {
        panic!(
            "late terminal state may refresh its display only; it must not authorize deletion, saving, Stop, or replay after timeout"
        );
    };
    assert_eq!(
        (
            query.project,
            query.run_id,
            query.view_id,
            query.offset,
            query.limit
        ),
        (
            fixture.project,
            started.summary.id,
            started.summary.id,
            0,
            PAGE_SIZE
        )
    );
    assert_eq!(query.filter, mio_core::IntruderResultFilter::default());
    assert_eq!(query.sort, mio_core::IntruderResultSort::default());
    assert!(query.query_revision > revision);
    assert!(!cancel.is_cancelled());
    assert_eq!(
        *query,
        view_query(&fixture, started.summary.id, query.query_revision, cx)
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("超时")
        );
    });
    emit(
        &fixture,
        Event::IntruderResultView {
            query: query.clone(),
            result: Ok(build_view_page(query, &started.summary, vec![], None, true)),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "display completion must not replay the expired deletion or issue another query"
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    ask_to_close(&started, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn in_flight_discard_timeout_keeps_receipt_ownership_and_late_success_keeps_window(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(31));
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_close_owns_workspace());
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("未确认")
        );
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    assert!(
        started.window.update(cx, |_, _, _| ()).is_ok(),
        "expired close intent must not close on a late receipt"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
        assert_eq!(view.workspace.revision, discard.workspace.revision);
    });
}

#[gpui_kit::test]
fn keep_receipt_preserves_newer_drafts_and_tracks_session_after_window_closes(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_terminal(&mut fixture, &mut started, "保留在内存", cx);
    let commands = drain(&mut fixture.commands);
    let [
        command @ Command::SetIntruderRetention {
            project,
            run_id,
            retention,
            workspace,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Keep must submit one complete, correlated retention proposal")
    };
    assert_eq!(
        (*project, *run_id, *retention),
        (
            fixture.project,
            started.summary.id,
            mio_core::IntruderRetention::Session
        )
    );
    assert_eq!(
        workspace.intruder.tabs[0].last_run,
        Some(started.summary.id)
    );
    let committed_revision = workspace.revision;
    type_target(&mut fixture, true, "http://127.0.0.1:19", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let event = retain_reply(command, &started.summary);
    emit(&fixture, event.clone(), cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft.target,
            "http://127.0.0.1:19"
        );
        assert_eq!(view.workspace.revision, committed_revision);
    });
    let commands = drain(&mut fixture.commands);
    let next = commands
        .iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        })
        .expect("newer edits must use the usual workspace writer");
    assert_eq!(next.revision, committed_revision + 1);
    emit(&fixture, event, cx);
    cx.read(|cx| assert_eq!(fixture.view.read(cx).pending_workspace, Some(next.revision)));
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: next,
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
}
