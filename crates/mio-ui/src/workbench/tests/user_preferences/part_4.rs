use super::*;

#[gpui_kit::test]
fn user_tls_preferences_snapshot_refresh_reuses_unchanged_certificate_storage(
    cx: &mut TestAppContext,
) {
    let (mut fixture, _) = setup(cx);
    let certificate = settings::tls_test_certificate();
    let operation = cx
        .update(|cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_user_tls_certificates(vec![certificate], 0, cx)
            })
        })
        .unwrap();
    let (_, saved) = take_tls_save(&mut fixture, cx);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
        },
        cx,
    );
    cx.update(|cx| {
        let latest = &fixture.view.read(cx).user_preferences;
        let mut snapshot = latest.clone();
        let certificate_ptr = snapshot.confirmed.as_ref().unwrap().custom_ca_ders[0].as_ptr();
        let mut changed_status = latest.clone();
        changed_status.status = "SSE progress does not change preferences".into();
        changed_status.finished = Finished::KeepMemory;
        assert!(!snapshot.sync_from(&changed_status));
        assert_eq!(
            snapshot.confirmed.as_ref().unwrap().custom_ca_ders[0].as_ptr(),
            certificate_ptr
        );
        assert_eq!(snapshot.status, changed_status.status);
        assert_eq!(snapshot.finished, Finished::KeepMemory);
        assert!(!snapshot.sync_from(&changed_status));
        assert_eq!(
            snapshot.confirmed.as_ref().unwrap().custom_ca_ders[0].as_ptr(),
            certificate_ptr
        );
    });
}

#[gpui_kit::test]
fn user_preference_mouse_release_preserves_other_group_from_an_ack_before_repaint(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = setup(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let keep = window.find("intruder-finished-keep").bounds().center();
        let delete = window.find("intruder-running-delete").bounds().center();
        // Input and a backend ACK can arrive before the next rendered frame replaces these handlers.
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: keep,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: keep,
                modifiers: Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        let (operation, saved) = take_save(&mut fixture);
        assert_eq!(
            (saved.revision, saved.running, saved.finished),
            (1, Running::Ask, Finished::KeepMemory)
        );
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: delete,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        assert!(drain(&mut fixture.commands).is_empty());
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::UserPreferencesSaved {
                    operation_id: operation,
                    result: Ok(saved),
                },
                window,
                cx,
            );
        });
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Ask, Finished::KeepMemory)
        );
        assert!(drain(&mut fixture.commands).is_empty());
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: delete,
                modifiers: Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserPreferences {
            operation_id,
            expected_revision,
            running,
            finished,
        },
    ] = commands.as_slice()
    else {
        panic!("releasing the second radio must submit exactly one current-revision choice");
    };
    assert_eq!(
        (*expected_revision, *running, *finished),
        (1, Running::Delete, Finished::KeepMemory),
        "the old radio handler must not restore the other group's pre-ACK value"
    );
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: *operation_id,
            result: Ok(UserPreferences {
                version: 1,
                revision: 2,
                running: *running,
                finished: *finished,
                ..UserPreferences::default()
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder_close_preferences(),
            (Running::Delete, Finished::KeepMemory)
        );
        assert_eq!(view.workspace, original);
    });
}

#[gpui_kit::test]
fn user_preference_radios_apply_only_after_ack_and_survive_project_switch(cx: &mut TestAppContext) {
    let (mut fixture, settings) = setup(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    click(settings, "intruder-running-continue", cx);
    let (operation, saved) = take_save(&mut fixture);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Ask, Finished::Ask)
        )
    });
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-running-continue").checked(),
            Some(true)
        );
        assert!(window.try_find_by_label("Apply").is_none());
    })
    .unwrap();
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: Uuid::new_v4(),
            result: Ok(saved.clone()),
        },
        cx,
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
            result: Ok(saved),
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Continue, Finished::Ask)
        );
        assert_eq!(fixture.view.read(cx).workspace, original);
    });
    emit(
        &fixture,
        Event::ProjectOpened {
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 2,
            },
            path: None,
            workspace: Workspace::default(),
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Continue, Finished::Ask)
        )
    });
    drain(&mut fixture.commands);
    click(settings, "intruder-closing-defaults", cx);
    let (operation, saved) = take_save(&mut fixture);
    assert_eq!(
        (saved.running, saved.finished),
        (Running::Ask, Finished::Ask)
    );
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder_close_preferences(),
            (Running::Ask, Finished::Ask)
        )
    });
}

#[gpui_kit::test]
fn user_preference_save_error_requeries_disk_without_losing_selection_or_error(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = setup(cx);
    click(settings, "intruder-finished-keep", cx);
    let (operation, saved) = take_save(&mut fixture);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Err("已发布但目录同步失败".into()),
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
        panic!("failure rereads without retrying save")
    };
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
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-finished-keep").checked(), Some(true));
        assert!(
            window
                .find("user-preferences-save-error")
                .label()
                .unwrap()
                .contains("目录同步失败")
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    click(settings, "user-preferences-retry", cx);
    let (retry, saved) = take_save(&mut fixture);
    assert_ne!(operation, retry);
    assert_eq!(saved.revision, 2);
}
