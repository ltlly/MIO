use super::*;

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn session_data_blocks_new_project_and_native_quit_before_quiescing(cx: &mut TestAppContext) {
    let (mut fixture, mut run, _) = setup(cx);
    run.retention = IntruderRetention::Session;
    show_tasks(&mut fixture, vec![run.clone()], cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("n"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    let probe = crate::native_terminate::tests::Probe::installed();
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    crate::workbench::tests::pump_native(cx);
    assert_eq!(probe.replies(), [false]);
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert!(view.intruder.session_runs.contains(&run.id));
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.shutdown.is_cancelled());
        assert!(view.tool == Tool::Dashboard);
        assert!(view.error.as_ref().unwrap().contains("仅保留在内存"));
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn keeping_the_last_rendered_result_window_preserves_the_main_event_receiver(
    cx: &mut TestAppContext,
) {
    let (mut fixture, run, _) = setup(cx);
    show_tasks(&mut fixture, vec![run.clone()], cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(
            SharedString::from(format!("dashboard-intruder-options-{}", run.id)),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("在新窗口中打开").bounds().center();
        window.pointer_down(item, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(item, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let result_window = cx.read(|cx| fixture.view.read(cx).intruder.results.views[&run.id].owner);
    assert_ne!(result_window, fixture.window);
    let revision = result_query(&mut fixture, run.id);
    page(
        &fixture,
        &run,
        &revision,
        &[flow("first"), flow("second")],
        true,
        cx,
    );
    cx.update_window(result_window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(result_window, cx);
    assert!(!native.simulate_close());
    cx.run_until_parked();
    assert!(has_close_dialog(cx));
    answer_close_dialog("保留在内存", cx);
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderRetention {
            project,
            run_id,
            operation_id,
            expected_revision,
            retention,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("Keep must submit only the selected run's retention transition");
    };
    assert_eq!(
        (*project, *run_id, *expected_revision, *retention),
        (
            fixture.project,
            run.id,
            run.revision,
            IntruderRetention::Session
        )
    );
    let mut kept = run.clone();
    kept.revision += 1;
    kept.retention = IntruderRetention::Session;
    let mut stored_workspace = *workspace.clone();
    for tab in stored_workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut stored_workspace.intruder.closed)
    {
        if tab.last_run == Some(run.id) {
            tab.last_run = None;
        }
    }
    // A real detached render references Workbench. Do not redraw the main window to repair its
    // dynamic GPUI current-window mapping before the result closes and the next event arrives.
    cx.update_window(result_window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(cx.update(|cx| cx.with_window(fixture.view.entity_id(), |window, _| window.window_handle())), Some(result_window));
    // Covered main windows may not repaint; event delivery and close callbacks must still work.
    cx.skip_drawing();
    fixture
        .events
        .try_send(Event::IntruderRetentionChanged {
            project: fixture.project,
            run_id: run.id,
            operation_id: *operation_id,
            result: Ok(mio_core::IntruderRetentionReceipt {
                run: kept,
                retention: IntruderRetention::Session,
                workspace: stored_workspace,
                session_logical_bytes: 128,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(result_window.update(cx, |_, _, _| ()).is_err());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!fixture.events.is_closed());
    let path = PathBuf::from("/tmp/mio-after-result-owner.mio");
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: path.clone(),
            total_bytes: 128,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        !fixture.events.is_closed(),
        "closing the most recently rendered result must not drop the Workbench event receiver"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project_path.as_ref(), Some(&path));
        assert!(view.intruder.session_runs.contains(&run.id));
        assert!(!view.backend_disconnected && !view.shutdown.is_cancelled());
        assert!(view.intruder_result_close.is_none());
    });
    let mut main = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
    assert_eq!(
        main.window_title().as_deref(),
        Some("MIO — mio-after-result-owner.mio")
    );
    assert!(drain(&mut fixture.commands).iter().all(|command| matches!(
        command,
        Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. }
    )));
}
