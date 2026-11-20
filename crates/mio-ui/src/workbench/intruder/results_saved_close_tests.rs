use mio_core::{FinishedAttackClose, RunningAttackClose, UserPreferences, UserPreferencesState};

use super::*;

fn named(fixture: &Fixture, cx: &mut TestAppContext) {
    // Only the UI project identity is supplied; no file is opened by this fixture.
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-saved-result-close.mio"));
        });
    });
}

fn delete_preferences(fixture: &mut Fixture, cx: &mut TestAppContext) -> UserPreferences {
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx));
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
        panic!("the fixture must read one correlated global preference snapshot");
    };
    let preferences = UserPreferences {
        version: 1,
        revision: 7,
        running: RunningAttackClose::Delete,
        finished: FinishedAttackClose::Delete,
        ..UserPreferences::default()
    };
    emit(
        fixture,
        Event::UserPreferences {
            operation_id: *operation_id,
            state: UserPreferencesState {
                preferences: preferences.clone(),
                load_error: None,
            },
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    preferences
}

fn close_cmd_w(started: &Started, cx: &mut TestAppContext) {
    cx.update_window(started.window, |_, window, cx| {
        assert!(window.is_action_available(&CloseTab, cx));
        window.press(crate::kbd!("w"), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn saved_running_keyboard_close_preserves_pending_save_latest_input_and_background_run(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    named(&fixture, cx);
    let preferences = delete_preferences(&mut fixture, cx);
    type_target(&mut fixture, true, "http://127.0.0.1:17", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("s"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: saving,
        },
    ] = commands.as_slice()
    else {
        panic!("explicit Save must first capture the latest main-window draft");
    };
    assert_eq!(*project, fixture.project);
    click(fixture.window, "intruder-sidebar-settings", cx);
    click(fixture.window, "intruder-config-save-to-project", cx);
    let target = type_target(&mut fixture, true, "http://127.0.0.1:18", cx);
    let (workspace, pending_save) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.workspace.intruder.tabs[0].spec.save_to_project);
        assert!(view.workspace_dirty);
        assert_eq!(view.pending_workspace, Some(saving.revision));
        assert_eq!(view.pending_project_save, Some(None));
        assert!(view.intruder.editors[&started.summary.tab_id].dirty);
        assert_eq!(
            view.intruder.results.views[&started.summary.id].persisted,
            Some(true)
        );
        (view.workspace.clone(), view.pending_project_save.clone())
    });
    assert!(drain(&mut fixture.commands).is_empty());
    close_cmd_w(&started, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(view.pending_workspace, Some(saving.revision));
        assert_eq!(view.pending_project_save, pending_save);
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.workspace_dirty);
        assert_eq!(target.read(cx).value().as_ref(), "http://127.0.0.1:18");
        assert!(view.intruder.editors[&started.summary.tab_id].dirty);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&preferences));
        assert!(view.intruder_result_close.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderResults { project, run_id, limit: 1, .. }]
        if *project == fixture.project && *run_id == started.summary.id),
        "detaching a running result may retain its progress probe, but must not save, stop, delete, or write preferences"
    );
    assert!(!fixture.events.is_closed());
}

#[gpui_kit::test]
fn saved_terminal_cmd_w_detaches_without_applying_global_delete(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let flows = [flow(0), flow(1)];
    bootstrap(&mut fixture, &mut started, &flows, cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    page(
        &fixture,
        &started,
        revision,
        vec![row(0, &flows[0]), row(1, &flows[1])],
        None,
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    named(&fixture, cx);
    let preferences = delete_preferences(&mut fixture, cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    close_cmd_w(&started, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(started.summary.id)
        );
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&preferences));
        assert!(view.intruder_result_close.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
}

fn unconfirmed_project_stays_open(kind: u8, cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    if kind != 1 {
        bootstrap(&mut fixture, &mut started, &[], cx);
    }
    named(&fixture, cx);
    let preferences = delete_preferences(&mut fixture, cx);
    if kind == 0 {
        let revision = poll(&mut fixture, &started, cx);
        started.summary.revision += 1;
        let event = result_event(
            &fixture,
            &started.summary,
            revision,
            vec![],
            None,
            false,
            cx,
        );
        assert!(matches!(&event, Event::IntruderResultView { query, .. }
            if query.project == fixture.project
                && query.run_id == started.summary.id
                && query.view_id == started.summary.id
                && query.query_revision == revision));
        emit(&fixture, event, cx);
        cx.read(|cx| {
            assert_eq!(
                fixture.view.read(cx).intruder.results.views[&started.summary.id].persisted,
                Some(false)
            )
        });
    } else if kind == 1 {
        cx.read(|cx| {
            assert_eq!(
                fixture.view.read(cx).intruder.results.views[&started.summary.id].persisted,
                None
            )
        });
    } else {
        let (replacement, _receiver) = mpsc::channel(1);
        drop(std::mem::replace(&mut fixture.events, replacement));
        cx.run_until_parked();
        cx.read(|cx| assert!(fixture.view.read(cx).backend_disconnected));
    }
    assert!(drain(&mut fixture.commands).is_empty());
    let cancelled = started.cancel.is_cancelled();
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    close_cmd_w(&started, cx);
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert_eq!(started.cancel.is_cancelled(), cancelled);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "unconfirmed Project storage must not fall through to automatic Delete or initiate another save"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .is_some()
        );
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&preferences));
        assert!(view.intruder_result_close.is_none());
    });
}

#[gpui_kit::test]
fn saved_project_recovery_page_blocks_close_without_auto_delete(cx: &mut TestAppContext) {
    unconfirmed_project_stays_open(0, cx);
}

#[gpui_kit::test]
fn saved_project_unread_persistence_blocks_close_without_auto_delete(cx: &mut TestAppContext) {
    unconfirmed_project_stays_open(1, cx);
}

#[gpui_kit::test]
fn saved_project_backend_disconnect_blocks_close_without_a_second_stop(cx: &mut TestAppContext) {
    unconfirmed_project_stays_open(2, cx);
}

#[gpui_kit::test]
fn saved_close_rechecks_storage_before_deferred_detach_and_preserves_pending_workspace(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    named(&fixture, cx);
    let preferences = delete_preferences(&mut fixture, cx);
    type_target(&mut fixture, true, "http://127.0.0.1:17", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("s"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            workspace: saving, ..
        },
    ] = commands.as_slice()
    else {
        panic!("the main draft must have a real outstanding workspace save");
    };
    let target = type_target(&mut fixture, true, "http://127.0.0.1:18", cx);
    let workspace = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(saving.revision));
        assert_eq!(view.pending_project_save, Some(None));
        assert_eq!(
            view.intruder.results.views[&started.summary.id].persisted,
            Some(true)
        );
        assert_eq!(view.pending_records, 0);
        assert!(view.storage_error.is_none());
        view.workspace.clone()
    });
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, main, cx| {
        started
            .window
            .update(cx, |_, window, cx| {
                assert!(window.is_action_available(&CloseTab, cx));
                window.press(crate::kbd!("w"), cx);
            })
            .unwrap();
        assert!(fixture.view.read(cx).intruder_result_close.is_some());
        // Deliver the backend fault in this same event cycle, before the queued detach runs.
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::StorageState {
                    project: fixture.project,
                    pending_records: 1,
                    message: Some("synthetic result write awaiting recovery".into()),
                },
                main,
                cx,
            );
            assert_eq!(view.pending_workspace, Some(saving.revision));
            assert_eq!(view.pending_project_save, Some(None));
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        started.window.update(cx, |_, _, _| ()).is_ok(),
        "storage becoming unconfirmed before deferred close must preserve the result window"
    );
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(!started.cancel.is_cancelled());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "a revoked detach must not save, stop, delete, or rewrite preferences"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(view.pending_workspace, Some(saving.revision));
        assert_eq!(view.pending_project_save, Some(None));
        assert_eq!(view.pending_records, 1);
        assert_eq!(
            view.storage_error.as_deref(),
            Some("synthetic result write awaiting recovery")
        );
        assert_eq!(target.read(cx).value().as_ref(), "http://127.0.0.1:18");
        assert!(view.intruder.editors[&started.summary.tab_id].dirty);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .is_some()
        );
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&preferences));
        assert!(view.intruder_result_close.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn saved_detach_target_loss_preserves_the_send_waiting_for_workspace_ack(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    named(&fixture, cx);
    let preferences = delete_preferences(&mut fixture, cx);
    type_target(&mut fixture, false, "http://127.0.0.1:19", cx);
    click(fixture.window, "repeater-send", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: saving,
        },
    ] = commands.as_slice()
    else {
        panic!("Send must first save its latest draft without opening a socket");
    };
    assert_eq!(*project, fixture.project);
    let (tab_id, draft, settings, workspace) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        let tab_id = view.workspace.active.unwrap();
        let pending = &view.pending_sends[&tab_id];
        assert_eq!(view.pending_sends.len(), 1);
        assert!(view.sending.is_empty());
        assert_eq!(view.pending_workspace, Some(saving.revision));
        assert_eq!(pending.required_workspace_revision, saving.revision);
        assert_eq!(pending.draft.target, "http://127.0.0.1:19");
        (
            tab_id,
            pending.draft.clone(),
            pending.settings.clone(),
            view.workspace.clone(),
        )
    });
    cx.update_window(fixture.window, |_, _, cx| {
        started
            .window
            .update(cx, |_, window, cx| {
                assert!(window.is_action_available(&CloseTab, cx));
                window.press(crate::kbd!("w"), cx);
                assert!(fixture.view.read(cx).intruder_result_close.is_some());
                // Model the OS destroying the auxiliary window before its queued detach callback.
                window.remove_window();
            })
            .unwrap();
    })
    .unwrap();
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, workspace);
        assert_eq!(view.pending_workspace, Some(saving.revision));
        assert_eq!(
            view.pending_sends.len(),
            1,
            "losing a pure-detach target must not cancel an unrelated pending Send"
        );
        let pending = &view.pending_sends[&tab_id];
        assert_eq!(pending.draft, draft);
        assert_eq!(pending.settings, settings);
        assert_eq!(pending.required_workspace_revision, saving.revision);
        assert!(view.sending.is_empty());
        assert!(view.intruder_result_close.is_none());
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert_eq!(view.user_preferences.confirmed.as_ref(), Some(&preferences));
        assert!(!view.shutdown.is_cancelled());
    });
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderResults { project, run_id, limit: 1, .. }]
        if *project == fixture.project && *run_id == started.summary.id),
        "the removed observer may leave only its background progress probe"
    );
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saving.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::Send {
            project,
            tab_id: sent_tab,
            draft: sent_draft,
            settings: sent_settings,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("the original workspace ACK must issue exactly one preserved Send");
    };
    assert_eq!((*project, *sent_tab), (fixture.project, tab_id));
    assert_eq!(sent_draft, &draft);
    assert_eq!(sent_settings, &settings);
    assert!(!cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_sends.is_empty());
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.sending.len(), 1);
        assert!(view.sending.contains_key(&tab_id));
        assert_eq!(view.workspace, *saving);
    });
    assert!(!fixture.events.is_closed());
}
