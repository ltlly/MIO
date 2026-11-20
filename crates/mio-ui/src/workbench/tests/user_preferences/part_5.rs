use super::*;

#[gpui_kit::test]
fn user_preference_old_query_unknown_file_and_timeout_keep_safe_policy(cx: &mut TestAppContext) {
    let (mut fixture, settings) = setup(cx);
    click(settings, "intruder-running-delete", cx);
    let (operation, saved) = take_save(&mut fixture);
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id: old }] = commands.as_slice() else {
        panic!("query expected")
    };
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
        },
        cx,
    );
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *old,
            state: UserPreferencesState {
                preferences: UserPreferences::default(),
                load_error: None,
            },
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Delete, Finished::Ask)
        )
    });
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryUserPreferences {
            operation_id: query,
        },
    ] = commands.as_slice()
    else {
        panic!("query expected")
    };
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *query,
            state: UserPreferencesState {
                preferences: UserPreferences::default(),
                load_error: Some("未知字段，原文件未修改".into()),
            },
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Ask, Finished::Ask)
        )
    });
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("user-preferences-load-error")
                .label()
                .unwrap()
                .contains("未知字段")
        );
    })
    .unwrap();
    click(settings, "user-preferences-retry", cx);
    drain(&mut fixture.commands);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_secs(31));
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(!fixture.view.read(cx).user_preferences.busy());
        assert!(
            fixture
                .view
                .read(cx)
                .user_preferences
                .load_error
                .as_ref()
                .unwrap()
                .contains("超时")
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

fn assert_late_user_preference_ack(save: bool, cx: &mut TestAppContext) {
    let (mut fixture, settings) = setup(cx);
    let event = if save {
        click(settings, "intruder-running-delete", cx);
        let (operation_id, saved) = take_save(&mut fixture);
        Event::UserPreferencesSaved {
            operation_id,
            result: Ok(saved),
        }
    } else {
        cx.update(|cx| {
            fixture
                .view
                .update(cx, |view, cx| view.query_user_preferences(cx))
        });
        let commands = drain(&mut fixture.commands);
        let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
            panic!("one query expected")
        };
        let preferences = UserPreferences {
            revision: 1,
            running: Running::Delete,
            ..UserPreferences::default()
        };
        Event::UserPreferences {
            operation_id: *operation_id,
            state: UserPreferencesState {
                preferences,
                load_error: None,
            },
        }
    };
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_secs(31));
    // Do not run the timer: the matching event must enforce the absolute deadline itself.
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(event, window, cx);
            assert_eq!(
                view.intruder_close_preferences(),
                (Running::Ask, Finished::Ask),
                "late acknowledgement must not enable Delete, save={save}"
            );
            assert!(
                view.user_preferences
                    .load_error
                    .as_ref()
                    .is_some_and(|error| error.contains("超时") || error.contains("未确认")),
                "late acknowledgement needs a visible safety state"
            );
        });
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryUserPreferences { .. })),
        "deadline handling must never retry a write"
    );
    if save {
        assert_eq!(
            commands.len(),
            1,
            "an unknown save result must be re-read once"
        );
    }
}

#[gpui_kit::test]
fn user_preference_late_query_ack_is_rejected_before_its_deadline_timer_runs(
    cx: &mut TestAppContext,
) {
    assert_late_user_preference_ack(false, cx);
}

#[gpui_kit::test]
fn user_preference_late_save_ack_is_rejected_before_its_deadline_timer_runs(
    cx: &mut TestAppContext,
) {
    assert_late_user_preference_ack(true, cx);
}

#[gpui_kit::test]
fn user_preference_expired_pending_save_uses_ask_before_the_timer_runs(cx: &mut TestAppContext) {
    let (mut fixture, settings) = setup(cx);
    click(settings, "intruder-running-delete", cx);
    let (operation_id, saved) = take_save(&mut fixture);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id,
            result: Ok(saved),
        },
        cx,
    );
    click(settings, "intruder-running-ask", cx);
    take_save(&mut fixture);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Delete, Finished::Ask),
            "before deadline the confirmed policy remains effective"
        )
    });
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_secs(31));
    cx.read(|cx| assert_eq!(fixture.view.read(cx).intruder_close_preferences(), (Running::Ask, Finished::Ask), "an overdue change to Ask must not leave automatic Delete active while the timer is queued"));
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "reading policy does not send or retry commands"
    );
}

#[gpui_kit::test]
fn user_preference_same_revision_conflicting_query_never_enables_delete(cx: &mut TestAppContext) {
    let (mut fixture, settings) = setup(cx);
    click(settings, "intruder-closing-defaults", cx);
    let (operation_id, confirmed) = take_save(&mut fixture);
    assert_eq!(
        (confirmed.revision, confirmed.running, confirmed.finished),
        (1, Running::Ask, Finished::Ask)
    );
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id,
            result: Ok(confirmed.clone()),
        },
        cx,
    );
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
        panic!("one query expected")
    };
    let mut conflicting = confirmed.clone();
    conflicting.running = Running::Delete;
    conflicting.finished = Finished::Delete;
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *operation_id,
            state: UserPreferencesState {
                preferences: conflicting,
                load_error: None,
            },
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&confirmed));
        assert!(
            view.user_preferences.load_error.is_some(),
            "same revision with different values is a visible invalid receipt"
        );
        assert_eq!(
            view.intruder_close_preferences(),
            (Running::Ask, Finished::Ask)
        );
    });
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("user-preferences-load-error").visible());
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "conflicting query cannot trigger an automatic save"
    );
}
