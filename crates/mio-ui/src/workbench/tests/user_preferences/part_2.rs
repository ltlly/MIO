use super::*;

#[gpui_kit::test]
fn remember_closing_error_survives_query_and_rejection_until_dismiss_or_new_admission(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    for from_settings in [false, true] {
        let operation = remember(
            &fixture,
            RememberCloseChoice::Running(Running::Continue),
            cx,
        )
        .unwrap();
        let (sent, saved) = take_save(&mut fixture);
        assert_eq!(sent, operation);
        emit(
            &fixture,
            Event::UserPreferencesSaved {
                operation_id: operation,
                result: Err("synthetic Remember rejection".into()),
            },
            cx,
        );
        let commands = drain(&mut fixture.commands);
        let [
            Command::QueryUserPreferences {
                operation_id: query,
            },
        ] = commands.as_slice()
        else {
            panic!("failed preference write must issue only one read");
        };
        let message = cx.read(|cx| {
            fixture
                .view
                .read(cx)
                .user_preferences
                .remember_error
                .clone()
                .unwrap()
        });
        assert!(
            remember(
                &fixture,
                RememberCloseChoice::Finished(Finished::Delete),
                cx
            )
            .is_err()
        );
        assert!(drain(&mut fixture.commands).is_empty());
        emit(
            &fixture,
            Event::UserPreferences {
                operation_id: *query,
                state: UserPreferencesState {
                    preferences: saved,
                    load_error: None,
                },
            },
            cx,
        );
        cx.read(|cx| {
            assert_eq!(
                fixture
                    .view
                    .read(cx)
                    .user_preferences
                    .remember_error
                    .as_ref(),
                Some(&message)
            )
        });
        if from_settings {
            cx.update(|cx| {
                fixture.view.update(cx, |view, cx| {
                    view.set_user_closing_preferences(Running::Ask, Finished::KeepMemory, cx);
                })
            });
        } else {
            remember(
                &fixture,
                RememberCloseChoice::Finished(Finished::KeepMemory),
                cx,
            )
            .unwrap();
        }
        let (operation, saved) = take_save(&mut fixture);
        cx.read(|cx| {
            assert!(
                fixture
                    .view
                    .read(cx)
                    .user_preferences
                    .remember_error
                    .is_none()
            )
        });
        emit(
            &fixture,
            Event::UserPreferencesSaved {
                operation_id: operation,
                result: Ok(saved),
            },
            cx,
        );
        assert!(drain(&mut fixture.commands).is_empty());
    }
    let operation = remember(&fixture, RememberCloseChoice::Running(Running::Delete), cx).unwrap();
    take_save(&mut fixture);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Err("dismiss this Remember error".into()),
        },
        cx,
    );
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryUserPreferences { .. }]
    ));
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.fail("independent project failure", cx))
    });
    click(fixture.window, "dismiss-error", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.user_preferences.remember_error.is_none());
        assert!(view.error.is_none());
        assert!(
            view.user_preferences
                .error
                .as_ref()
                .unwrap()
                .contains("dismiss this Remember error")
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn remember_closing_timeout_requeries_and_late_ack_cannot_hide_failure(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let operation = remember(&fixture, RememberCloseChoice::Running(Running::Delete), cx).unwrap();
    let (_, saved) = take_save(&mut fixture);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_secs(31));
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
        },
        cx,
    );
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryUserPreferences { .. }]
    ));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("下次关闭的选择未确认")
        );
        assert_eq!(
            view.intruder_close_preferences(),
            (Running::Ask, Finished::Ask)
        );
    });
}

#[gpui_kit::test]
fn remember_closing_disconnect_keeps_the_preference_failure_on_the_main_window(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    remember(
        &fixture,
        RememberCloseChoice::Running(Running::Continue),
        cx,
    )
    .unwrap();
    take_save(&mut fixture);
    drop(fixture.events);
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("下次关闭的选择未确认")
        );
        assert_eq!(view.user_preferences.running, Running::Continue);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
