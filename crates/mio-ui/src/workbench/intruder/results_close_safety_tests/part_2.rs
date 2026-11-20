use super::*;

#[gpui_kit::test]
fn another_result_close_cannot_replace_a_discard_after_backend_disconnect(cx: &mut TestAppContext) {
    let (mut fixture, selected, other, discard) = discarding_with_another_live_run(cx);
    let (replacement, _receiver) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    cx.read(|cx| assert!(fixture.view.read(cx).backend_disconnected));
    assert!(other.cancel.is_cancelled());

    cx.update_window(other.window, |_, window, cx| {
        window.dispatch_action(Box::new(CloseTab), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_close_owns_workspace());
        assert_eq!(view.pending_workspace, Some(discard.workspace.revision));
        assert!(
            view.intruder.results.views[&other.summary.id]
                .close_error
                .is_some()
        );
    });

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
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.workspace.revision, discard.workspace.revision);
        assert!(view.intruder.results.views[&selected.summary.id].discarded);
        assert!(!view.intruder.results.views[&other.summary.id].discarded);
    });
    assert!(selected.window.update(cx, |_, _, _| ()).is_ok());
    assert!(other.window.update(cx, |_, _, _| ()).is_ok());
    assert!(other.cancel.is_cancelled());
}

#[gpui_kit::test]
fn settings_are_disabled_during_pending_discard_for_the_same_result(cx: &mut TestAppContext) {
    let (mut fixture, selected, other, discard) = discarding_with_another_live_run(cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            // Keep the pending-discard gate as the reason this otherwise savable control is disabled.
            view.project_path = Some(PathBuf::from("/tmp/mio-pending-discard-settings.mio"));
            let result = view
                .intruder
                .results
                .views
                .get_mut(&selected.summary.id)
                .unwrap();
            let summary = result.summary.as_mut().unwrap();
            assert!(summary.state.is_terminal());
            summary.retention = mio_core::IntruderRetention::Session;
            result.persisted = Some(false);
            cx.notify();
        });
    });
    click(selected.window, "intruder-results-settings", cx);
    cx.update_window(selected.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-save-to-project").checked(),
            Some(false)
        );
        assert_eq!(
            window.find("intruder-result-save-to-project").disabled(),
            Some(true)
        );
    })
    .unwrap();
    click(selected.window, "intruder-result-save-to-project", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_close_owns_workspace());
        assert_eq!(view.pending_workspace, Some(discard.workspace.revision));
    });

    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.workspace.revision, discard.workspace.revision);
        assert!(!view.intruder.results.views[&other.summary.id].discarded);
    });
    assert!(selected.window.update(cx, |_, _, _| ()).is_err());
    assert!(other.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!other.cancel.is_cancelled());
}

#[gpui_kit::test]
fn overdue_terminal_page_cannot_submit_discard_before_timer_callback(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    choose_discard(&mut fixture, &started, cx);
    let revision = poll(&mut fixture, &started, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Stopped;
    let event = Event::IntruderResults {
        project: fixture.project,
        run_id: started.summary.id,
        query_revision: revision,
        result: Ok(IntruderResultPage {
            run: started.summary.clone(),
            rows: vec![],
            next_after: None,
            persisted: true,
        }),
    };
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(started.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            assert!(view.on_intruder_result_event(&event, window, cx));
            assert!(view.intruder_result_close.is_none());
            assert!(view.pending_workspace.is_none());
            assert_eq!(
                view.workspace.intruder.tabs[0].last_run,
                Some(started.summary.id)
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn overdue_retention_receipt_cannot_start_project_save_before_timer_callback(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    started.summary.retention = mio_core::IntruderRetention::Session;
    cx.update_window(fixture.window, |_, _, cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/intruder-close-deadline.mio"));
        });
    })
    .unwrap();
    choose_terminal(&mut fixture, &mut started, "保存到项目文件", cx);
    let commands = drain(&mut fixture.commands);
    let [command @ Command::SetIntruderRetention { .. }] = commands.as_slice() else {
        panic!("saving a Session run must first submit its retention transition");
    };
    let Event::IntruderRetentionChanged {
        project,
        run_id,
        operation_id,
        result,
    } = retain_reply(command, &started.summary)
    else {
        unreachable!();
    };
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_intruder_retention_changed(project, run_id, operation_id, result, window, cx);
            assert!(view.intruder_result_close.is_none());
            assert!(view.pending_workspace.is_none());
            assert!(!view.intruder.session_runs.contains(&run_id));
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    for command in drain(&mut fixture.commands) {
        match command {
            Command::QueryHttpHistory { query } => assert_eq!(query.project, fixture.project),
            Command::QueryIntruderResults {
                project,
                run_id,
                after: None,
                limit: 1,
                ..
            } => assert_eq!((project, run_id), (fixture.project, started.summary.id)),
            Command::QueryIntruderResultView { query, cancel } => {
                assert_eq!(query.project, fixture.project);
                assert_eq!(query.run_id, started.summary.id);
                assert_eq!(query.view_id, started.summary.id);
                assert_eq!(query.offset, 0);
                assert_eq!(query.limit, PAGE_SIZE);
                assert!(!cancel.is_cancelled());
            }
            _ => panic!("an expired retention reply must not trigger save, send or another close"),
        }
    }
}
