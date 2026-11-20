use super::*;

#[gpui_kit::test]
fn remember_closing_preserves_the_other_visible_unsaved_choice_and_uses_confirmed_cas(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = setup(cx);
    cx.update(|cx| {
        let view = fixture.view.read(cx);
        while view
            .commands
            .try_send(Command::QueryUserPreferences {
                operation_id: Uuid::new_v4(),
            })
            .is_ok()
        {}
    });
    click(settings, "intruder-finished-keep", cx);
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::QueryUserPreferences { .. }))
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.user_preferences.finished, Finished::KeepMemory);
        assert_eq!(
            view.user_preferences.confirmed.as_ref().unwrap().finished,
            Finished::Ask
        );
    });
    let operation = remember(
        &fixture,
        RememberCloseChoice::Running(Running::Continue),
        cx,
    )
    .unwrap();
    let (sent, saved) = take_save(&mut fixture);
    assert_eq!(sent, operation);
    assert_eq!(
        (saved.revision, saved.running, saved.finished),
        (1, Running::Continue, Finished::Ask)
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Ask, Finished::Ask)
        )
    });
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        let prefs = &fixture.view.read(cx).user_preferences;
        assert_eq!(prefs.confirmed.as_ref(), Some(&saved));
        assert_eq!(
            (prefs.running, prefs.finished),
            (Running::Continue, Finished::KeepMemory)
        );
    });
    // A later read must not overwrite the other category's retained Settings draft.
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
        panic!("one explicit read expected");
    };
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *operation_id,
            state: UserPreferencesState {
                preferences: saved,
                load_error: None,
            },
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).user_preferences.finished,
            Finished::KeepMemory
        )
    });
    let operation = remember(
        &fixture,
        RememberCloseChoice::Finished(Finished::Delete),
        cx,
    )
    .unwrap();
    let (sent, saved) = take_save(&mut fixture);
    assert_eq!(sent, operation);
    assert_eq!(
        (saved.revision, saved.running, saved.finished),
        (2, Running::Continue, Finished::Delete)
    );
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

#[gpui_kit::test]
fn remember_closing_rejects_busy_queue_load_error_and_backend_without_action_commands(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("原草稿")], 64);
    drain(&mut fixture.commands);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let first = remember(
        &fixture,
        RememberCloseChoice::Running(Running::Continue),
        cx,
    )
    .unwrap();
    let (operation, saved) = take_save(&mut fixture);
    assert_eq!(first, operation);
    assert!(
        remember(
            &fixture,
            RememberCloseChoice::Finished(Finished::Delete),
            cx
        )
        .unwrap_err()
        .contains("尚未结束")
    );
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
        },
        cx,
    );
    cx.update(|cx| {
        let view = fixture.view.read(cx);
        while view
            .commands
            .try_send(Command::QueryUserPreferences {
                operation_id: Uuid::new_v4(),
            })
            .is_ok()
        {}
    });
    assert!(
        remember(
            &fixture,
            RememberCloseChoice::Finished(Finished::Delete),
            cx
        )
        .unwrap_err()
        .contains("队列已满")
    );
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::QueryUserPreferences { .. }))
    );
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
        panic!("one read expected");
    };
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *operation_id,
            state: UserPreferencesState {
                preferences: UserPreferences::default(),
                load_error: Some("corrupt fixture".into()),
            },
        },
        cx,
    );
    assert!(
        remember(
            &fixture,
            RememberCloseChoice::Finished(Finished::Delete),
            cx
        )
        .unwrap_err()
        .contains("正确载入")
    );
    cx.update(|cx| fixture.view.read(cx).shutdown.cancel());
    assert!(
        remember(
            &fixture,
            RememberCloseChoice::Finished(Finished::Delete),
            cx
        )
        .unwrap_err()
        .contains("后台不可用")
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, original));
}

#[gpui_kit::test]
fn remember_closing_failure_is_visible_without_settings_and_never_replays_action(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("记忆失败保留操作")], 64);
    drain(&mut fixture.commands);
    let operation = remember(
        &fixture,
        RememberCloseChoice::Finished(Finished::KeepMemory),
        cx,
    )
    .unwrap();
    let (sent, saved) = take_save(&mut fixture);
    assert_eq!(operation, sent);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: Uuid::new_v4(),
            result: Err("unrelated".into()),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Err("published but directory sync failed".into()),
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
        panic!("failure may only reread preferences, not repeat an action");
    };
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let error = fixture.view.read(cx).error.as_ref().unwrap();
        assert!(error.contains("下次关闭的选择未确认") && error.contains("本次操作按原选择处理"));
        assert!(window.find_by_label(error).visible());
    })
    .unwrap();
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
            result: Ok(saved),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("下次关闭的选择未确认")
        )
    });
}

#[gpui_kit::test]
fn remember_closing_failure_remains_visible_after_dashboard_query_enqueue(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("偏好错误持续可见")], 64);
    drain(&mut fixture.commands);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let operation = remember(
        &fixture,
        RememberCloseChoice::Running(Running::Continue),
        cx,
    )
    .unwrap();
    let (sent, _) = take_save(&mut fixture);
    assert_eq!(sent, operation);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Err("synthetic preference write denied".into()),
        },
        cx,
    );
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryUserPreferences { .. }]
    ));
    let message = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.settings_window.is_none());
        let message = view.error.clone().unwrap();
        assert!(message.contains("下次关闭的选择未确认"));
        message
    });
    click(fixture.window, "tool-dashboard", cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryIntruderRuns { project, after: None, limit, .. }]
            if *project == fixture.project && *limit == PAGE_SIZE
    ));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Dashboard);
        assert_eq!(view.workspace, original);
        assert!(
            view.user_preferences
                .error
                .as_ref()
                .unwrap()
                .contains("synthetic preference write denied")
        );
        assert!(!view.shutdown.is_cancelled());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let alert = window.find_by_label(&message);
        assert!(
            alert.visible(),
            "a read-only task query must not dismiss the independent Remember failure"
        );
        assert_eq!(alert.role(), Some(gpui_kit::accesskit::Role::Alert));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}
