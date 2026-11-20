use super::*;

fn rejected_remember(load_error: bool, cx: &mut TestAppContext) {
    let (mut fixture, started, confirmed) = setup(false, cx);
    ask_to_close(&started, true, cx);
    let dialog = remember(cx);
    let query = query_preferences(&mut fixture, cx);
    if load_error {
        emit(
            &fixture,
            Event::UserPreferences {
                operation_id: query,
                state: UserPreferencesState {
                    preferences: confirmed.clone(),
                    load_error: Some("synthetic invalid preferences".into()),
                },
            },
            cx,
        );
    }
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    answer_close_dialog("丢弃", cx);
    assert_eq!(close_dialog(cx), Some(dialog));
    cx.update_window(dialog, |_, window, _| {
        assert_eq!(window.find("intruder-close-remember").checked(), Some(true));
        assert!(window.find("intruder-close-dialog-error").visible());
        // base::Button omits the AX disabled flag when the button is enabled.
        assert!(
            !window
                .find(("intruder-close-choice", 0usize))
                .disabled()
                .unwrap_or(false)
        );
    })
    .unwrap();
    assert!(!started.cancel.is_cancelled());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "failed Remember admission must not save, stop, or delete"
    );
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, workspace));

    let recovery_query = if load_error {
        query_preferences(&mut fixture, cx)
    } else {
        query
    };
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: recovery_query,
            state: UserPreferencesState {
                preferences: confirmed.clone(),
                load_error: None,
            },
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(close_dialog(cx), Some(dialog));
    cx.update_window(dialog, |_, window, cx| {
        assert_eq!(window.find("intruder-close-remember").checked(), Some(true));
        window.click(("intruder-close-choice", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (_, _, actions) = take_remember(
        &mut fixture,
        &confirmed,
        Running::Delete,
        confirmed.finished,
    );
    assert!(
        matches!(actions.as_slice(), [Command::StopIntruder { project, run_id }]
        if *project == fixture.project && *run_id == started.summary.id),
        "the original button must submit exactly one remembered choice and one Stop after recovery"
    );
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&confirmed));
        assert!(!view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn remember_busy_preferences_preserve_checked_dialog_without_stopping(cx: &mut TestAppContext) {
    rejected_remember(false, cx);
}

#[gpui_kit::test]
fn remember_load_error_preserves_checked_dialog_without_stopping(cx: &mut TestAppContext) {
    rejected_remember(true, cx);
}

fn preference_outcome_does_not_replay_close(timeout: bool, cx: &mut TestAppContext) {
    let (mut fixture, started, confirmed) = setup(false, cx);
    ask_to_close(&started, true, cx);
    remember(cx);
    answer_close_dialog("在后台继续运行", cx);
    let (operation, saved, actions) = take_remember(
        &mut fixture,
        &confirmed,
        Running::Continue,
        confirmed.finished,
    );
    assert!(
        matches!(actions.as_slice(), [Command::QueryIntruderResults { project, run_id, after: None, limit: 1, .. }] if *project == fixture.project && *run_id == started.summary.id)
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    if timeout {
        cx.dispatcher
            .scheduler()
            .clock()
            .advance(Duration::from_secs(31));
    }
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::UserPreferencesSaved {
                    operation_id: operation,
                    result: if timeout {
                        Ok(saved.clone())
                    } else {
                        Err("synthetic preference write denied".into())
                    },
                },
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryUserPreferences {
            operation_id: query,
        },
    ] = commands.as_slice()
    else {
        panic!("an unknown or failed preference write must only re-read once");
    };
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *query,
            state: UserPreferencesState {
                preferences: saved.clone(),
                load_error: None,
            },
        },
        cx,
    );
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved.clone()),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "preference reconciliation cannot repeat the close action"
    );
    assert!(!has_close_dialog(cx));
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&saved));
        assert!(view.user_preferences.error.is_some());
        assert!(!view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn remember_save_error_and_late_ack_never_repeat_continue(cx: &mut TestAppContext) {
    preference_outcome_does_not_replay_close(false, cx);
}

#[gpui_kit::test]
fn remember_expired_save_ack_only_requeries_and_never_repeats_continue(cx: &mut TestAppContext) {
    preference_outcome_does_not_replay_close(true, cx);
}

#[gpui_kit::test]
fn remember_running_prompt_keeps_its_category_after_terminal_event(cx: &mut TestAppContext) {
    let (mut fixture, mut started, confirmed) = setup(false, cx);
    ask_to_close(&started, true, cx);
    remember(cx);
    let revision = poll(&mut fixture, &started, cx);
    stopped(&fixture, &mut started, revision, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    answer_close_dialog("在后台继续运行", cx);
    let (_, _, actions) = take_remember(
        &mut fixture,
        &confirmed,
        Running::Continue,
        confirmed.finished,
    );
    assert!(
        actions.is_empty(),
        "finished state cannot remap a running Continue choice to retention or deletion"
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
        assert!(view.intruder_result_close.is_none());
    });
}
