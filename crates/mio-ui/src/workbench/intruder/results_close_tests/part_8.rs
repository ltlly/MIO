use super::*;

#[gpui_kit::test]
fn session_save_as_cancel_keeps_session_and_never_promotes_before_a_destination(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    started.summary.retention = mio_core::IntruderRetention::Session;
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    save_started_from_dashboard(&mut fixture, &started, cx);
    assert!(cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref()
                .unwrap()
                .retention,
            mio_core::IntruderRetention::Session
        );
        assert!(view.project_path.is_none());
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert!(view.error.as_ref().unwrap().contains("保存未完成"));
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn expired_save_as_answer_cannot_create_a_project_or_promote_session(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    started.summary.retention = mio_core::IntruderRetention::Session;
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    save_started_from_dashboard(&mut fixture, &started, cx);
    assert!(cx.did_prompt_for_new_path());
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(31));
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_new_path_selection(|_| Some(PathBuf::from("/tmp/expired-intruder-save.mio")));
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none());
        assert!(!view.choosing_save_as && view.file_picker.is_none());
        assert!(view.intruder_result_close.is_none());
        assert!(view.intruder.session_runs.contains(&started.summary.id));
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn stopped_notification_retains_unconfirmed_discard_receipt_and_expires_only_close_intent(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    emit(&fixture, Event::Stopped, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.intruder_close_owns_workspace(),
            "a notification does not determine the outcome of an already submitted deletion"
        );
        assert_eq!(view.pending_workspace, Some(discard.workspace.revision));
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(view.workspace.revision, discard.workspace.revision);
        assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn overdue_receipt_retires_close_intent_before_the_timer_callback_runs(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    let discard = take_discard(&mut fixture, &started, cx);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_intruder_discarded(
                discard.project,
                discard.run_id,
                discard.operation_id,
                Ok(receipt(&discard)),
                window,
                cx,
            );
            assert!(
                view.intruder_result_close.is_none(),
                "an overdue receipt must retire the close intent before any timer callback can run"
            );
            assert_eq!(view.workspace.intruder.tabs[0].last_run, None);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}
