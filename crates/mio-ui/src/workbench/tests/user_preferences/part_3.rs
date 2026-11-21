use super::*;

fn assert_user_preferences_ack_preserves_newer_snapshot(
    tls: bool,
    same_revision: bool,
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings_window) = setup(cx);
    let certificate = settings::tls_test_certificate();
    let seeded_operation = cx
        .update(|cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_user_tls_certificates(vec![certificate.clone()], 0, cx)
            })
        })
        .unwrap();
    let (_, seeded) = take_tls_save(&mut fixture, cx);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: seeded_operation,
            result: Ok(seeded.clone()),
        },
        cx,
    );
    let (operation, stale_saved) = if tls {
        cx.update(|cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_user_tls_certificates(Vec::new(), seeded.revision, cx)
            })
        })
        .unwrap();
        take_tls_save(&mut fixture, cx)
    } else {
        click(settings_window, "intruder-running-continue", cx);
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
            panic!("one closing preference save expected");
        };
        assert_eq!(*expected_revision, seeded.revision);
        (
            *operation_id,
            UserPreferences {
                revision: expected_revision + 1,
                running: *running,
                finished: *finished,
                ..seeded.clone()
            },
        )
    };
    let mut latest = seeded;
    latest.revision = stale_saved.revision + u64::from(!same_revision);
    latest.startup.disable_extensions = true;
    latest.recent_projects.push(mio_core::RecentProject {
        name: "另一个项目".into(),
        path: PathBuf::from("/tmp/user-preferences-newer-snapshot.mio"),
        last_accessed_unix_ms: 1,
    });
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: Uuid::nil(),
            state: UserPreferencesState {
                preferences: latest.clone(),
                load_error: None,
            },
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).user_preferences.confirmed.as_ref(),
            Some(&latest),
            "the actual unsolicited startup/MRU snapshot path must update confirmed first"
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(stale_saved.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        let preferences = &fixture.view.read(cx).user_preferences;
        assert_eq!(
            preferences.confirmed.as_ref(),
            Some(&latest),
            "old or conflicting save acknowledgment must not roll back confirmed settings"
        );
        assert_eq!(
            preferences.confirmed.as_ref().unwrap().custom_ca_ders,
            vec![certificate.clone()],
            "the complete confirmed certificate list must remain intact"
        );
        assert!(preferences.error.is_some() && preferences.load_error.is_some());
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryUserPreferences {
            operation_id: query,
        },
    ] = commands.as_slice()
    else {
        panic!("a rejected acknowledgment must only requery, never replay a save");
    };
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(stale_saved),
        },
        cx,
    );
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *query,
            state: UserPreferencesState {
                preferences: latest.clone(),
                load_error: None,
            },
        },
        cx,
    );
    cx.read(|cx| {
        let preferences = &fixture.view.read(cx).user_preferences;
        assert_eq!(preferences.confirmed.as_ref(), Some(&latest));
        assert!(preferences.error.is_some());
        assert!(preferences.load_error.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn user_tls_ack_preserves_newer_snapshot(cx: &mut TestAppContext) {
    assert_user_preferences_ack_preserves_newer_snapshot(true, false, cx);
}

#[gpui_kit::test]
fn user_tls_ack_rejects_same_revision_conflicting_snapshot(cx: &mut TestAppContext) {
    assert_user_preferences_ack_preserves_newer_snapshot(true, true, cx);
}

#[gpui_kit::test]
fn user_closing_ack_preserves_newer_snapshot(cx: &mut TestAppContext) {
    assert_user_preferences_ack_preserves_newer_snapshot(false, false, cx);
}

#[gpui_kit::test]
fn user_closing_ack_rejects_same_revision_conflicting_snapshot(cx: &mut TestAppContext) {
    assert_user_preferences_ack_preserves_newer_snapshot(false, true, cx);
}

#[gpui_kit::test]
fn user_tls_preferences_preserve_closing_draft_and_project_override_across_global_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings_window) = setup(cx);
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
    click(settings_window, "intruder-finished-keep", cx);
    drain(&mut fixture.commands);
    let certificate = settings::tls_test_certificate();
    let original = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let operation = cx
        .update(|cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_user_tls_certificates(vec![certificate.clone()], 0, cx)
            })
        })
        .unwrap();
    let (sent, saved) = take_tls_save(&mut fixture, cx);
    assert_eq!(sent, operation);
    assert_eq!(
        (saved.running, saved.finished),
        (Running::Ask, Finished::Ask)
    );
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: Uuid::new_v4(),
            result: Ok(saved.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.user_preferences
                .confirmed
                .as_ref()
                .unwrap()
                .custom_ca_ders
                .is_empty()
        );
        assert_eq!(view.user_preferences.finished, Finished::KeepMemory);
    });
    let next_project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 5,
    };
    emit(
        &fixture,
        Event::ProjectOpened {
            project: next_project,
            path: None,
            workspace: original.clone(),
        },
        cx,
    );
    drain(&mut fixture.commands);
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next_project));
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&saved));
        assert_eq!(view.user_preferences.finished, Finished::KeepMemory);
        assert_eq!(*view.workspace, original);
    });
    click(settings_window, "intruder-running-continue", cx);
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
        panic!("closing choice remains separately savable after TLS acknowledgment");
    };
    assert_eq!(
        (*expected_revision, *running, *finished),
        (1, Running::Continue, Finished::KeepMemory)
    );
    let closing_saved = UserPreferences {
        revision: 2,
        running: *running,
        finished: *finished,
        ..saved
    };
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: *operation_id,
            result: Ok(closing_saved.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).user_preferences.confirmed.as_ref(),
            Some(&closing_saved)
        );
        assert_eq!(closing_saved.custom_ca_ders, vec![certificate]);
    });
}

#[gpui_kit::test]
fn user_tls_preferences_mismatched_roots_timeout_and_stale_revision_never_apply_or_replay(
    cx: &mut TestAppContext,
) {
    let (mut fixture, _) = setup(cx);
    let certificate = settings::tls_test_certificate();
    assert!(
        cx.update(|cx| fixture.view.update(cx, |view, cx| {
            view.set_user_tls_certificates(vec![certificate.clone()], 9, cx)
        }))
        .is_err()
    );
    assert!(drain(&mut fixture.commands).is_empty());
    let operation = cx
        .update(|cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_user_tls_certificates(vec![certificate.clone()], 0, cx)
            })
        })
        .unwrap();
    let (_, mut saved) = take_tls_save(&mut fixture, cx);
    saved.custom_ca_ders.clear();
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
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
        panic!("invalid TLS ACK must requery without sending another save");
    };
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.user_preferences
                .confirmed
                .as_ref()
                .unwrap()
                .custom_ca_ders
                .is_empty()
        );
        assert!(view.user_preferences.error.is_some());
    });
    emit(
        &fixture,
        Event::UserPreferences {
            operation_id: *query,
            state: UserPreferencesState {
                preferences: UserPreferences::default(),
                load_error: None,
            },
        },
        cx,
    );
    let operation = cx
        .update(|cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_user_tls_certificates(vec![certificate], 0, cx)
            })
        })
        .unwrap();
    let (_, saved) = take_tls_save(&mut fixture, cx);
    cx.background_executor
        .advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(matches!(
        commands.as_slice(),
        [Command::QueryUserPreferences { .. }]
    ));
    emit(
        &fixture,
        Event::UserPreferencesSaved {
            operation_id: operation,
            result: Ok(saved),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.user_preferences
                .confirmed
                .as_ref()
                .unwrap()
                .custom_ca_ders
                .is_empty()
        );
        assert!(view.user_preferences.error.is_some());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
