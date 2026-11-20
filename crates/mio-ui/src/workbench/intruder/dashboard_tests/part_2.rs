use super::*;

#[gpui_kit::test]
fn only_the_last_observer_hands_off_to_a_run_probe_and_never_closes_main(cx: &mut TestAppContext) {
    let (mut fixture, mut run, _) = setup(cx);
    run.state = IntruderRunState::Running;
    run.dispatched = 0;
    run.completed = 0;
    show_tasks(&mut fixture, vec![run.clone()], cx);
    let (native, native_query) = native(&mut fixture, &run, cx);
    page(&fixture, &run, &native_query, &[], true, cx);
    let (embedded, embedded_query) = choose(&mut fixture, &run, cx);
    page(&fixture, &run, &embedded_query, &[], true, cx);
    let cancel = CancellationToken::new();
    update(&fixture, cx, |view, _, _| {
        view.intruder.runs.insert(
            run.id,
            IntruderRun {
                summary: run.clone(),
                cancel: cancel.clone(),
            },
        );
    });
    cx.update_window(native, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(!cancel.is_cancelled());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an embedded observer already owns progress reads"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder.results.views.contains_key(&run.id));
        assert!(view.intruder.results.views.contains_key(&embedded));
    });
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    click(fixture.window, "intruder-results-results", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !has_close_dialog(cx) && !cx.has_pending_prompt(),
        "embedded results must not install the native result close handler"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.results.views.contains_key(&embedded));
        assert!(view.intruder_result_close.is_none());
        assert_eq!(view.workspace, workspace);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    update(&fixture, cx, |view, window, cx| {
        view.close_intruder_results(embedded, window, cx)
    });
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            run_id,
            query_revision,
            limit: 1,
            after: None,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("only the final observer's removal may start one bounded actual-run probe");
    };
    assert_eq!(*run_id, run.id);
    assert!(!cancel.is_cancelled());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    run.revision += 1;
    run.state = IntruderRunState::Stopped;
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: run.id,
            query_revision: *query_revision,
            result: Ok(IntruderResultPage {
                run: run.clone(),
                rows: Vec::new(),
                next_after: None,
                persisted: true,
            }),
        },
        cx,
    );
    cx.read(|cx| assert!(!fixture.view.read(cx).intruder.runs.contains_key(&run.id)));
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn summary_and_discard_update_every_view_without_replaying_a_stale_run_page(
    cx: &mut TestAppContext,
) {
    let (mut fixture, run, _) = setup(cx);
    show_tasks(&mut fixture, vec![run.clone()], cx);
    let (native, native_query) = native(&mut fixture, &run, cx);
    let flow = flow("retained result");
    page(
        &fixture,
        &run,
        &native_query,
        std::slice::from_ref(&flow),
        true,
        cx,
    );
    let (embedded, embedded_query) = choose(&mut fixture, &run, cx);
    page(
        &fixture,
        &run,
        &embedded_query,
        std::slice::from_ref(&flow),
        true,
        cx,
    );
    update(&fixture, cx, |view, _, cx| {
        view.query_intruder_runs(None, cx)
    });
    let stale_list = run_query(&mut fixture, None);
    // TestPlatform records set_title but inherits the macOS-only get_title default.
    let mut main_window = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
    let mut result_window = gpui_kit::VisualTestContext::from_window(native, cx);
    let main_title = main_window.window_title();
    assert!(main_title.is_some());
    let mut newer = run.clone();
    newer.revision += 1;
    newer.title = "已确认任务名".into();
    newer.title_revision = 1;
    update(&fixture, cx, |view, _, cx| {
        view.update_intruder_run_summary(&newer);
        cx.notify();
    });
    runs(&fixture, stale_list, vec![run.clone()], None, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.intruder.results.runs.rows[0], newer);
        assert_eq!(
            view.intruder.results.views[&run.id].summary.as_ref(),
            Some(&newer)
        );
        assert_eq!(
            view.intruder.results.views[&embedded].summary.as_ref(),
            Some(&newer)
        );
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(SharedString::from(format!("dashboard-intruder-{}", run.id)))
                .label(),
            Some(newer.title.as_str())
        );
    })
    .unwrap();
    assert_eq!(
        main_window.window_title(),
        main_title,
        "embedded result updates must not rename the project window"
    );
    assert_eq!(
        result_window.window_title().as_deref(),
        Some(newer.title.as_str())
    );
    click(native, ("intruder-result-row", 0_usize), cx);
    let native_body = read(&mut fixture, flow.summary.id);
    detail(&fixture, native_body, &flow, cx);
    let input = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&run.id]
            .request
            .raw
            .clone()
    });
    cx.update_window(native, |_, window, cx| {
        window.click(("input", input.entity_id()), cx)
    })
    .unwrap();
    click(fixture.window, ("intruder-result-row", 0_usize), cx);
    let old_body = read(&mut fixture, flow.summary.id);
    update(&fixture, cx, |view, window, cx| {
        view.invalidate_discarded_intruder_results(run.id, cx);
        let (filter, sort) = view.intruder.results.views[&embedded].query.requested();
        assert!(
            view.query_intruder_result_view(embedded, 0, filter, sort, window, cx)
                .is_none()
        );
        view.query_intruder_results(embedded, None, window, cx);
        view.read_intruder_result_spec(embedded, cx);
        view.select_intruder_result(embedded, 0, window, cx);
    });
    detail(&fixture, old_body, &flow, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.results.runs.rows.is_empty());
        for id in [run.id, embedded] {
            let result = &view.intruder.results.views[&id];
            assert!(
                result.query.page.is_none()
                    && result.query.pending.is_none()
                    && result.rows.is_empty()
                    && result.detail.is_none()
                    && result.spec.is_none()
            );
            assert!(result.selected.is_none() && result.discarded);
            assert!(result.request.raw.read(cx).value().is_empty());
            assert!(result.error.as_ref().unwrap().contains("已丢弃"));
            assert!(result.spec_error.as_ref().unwrap().contains("已丢弃"));
        }
    });
    cx.update_window(native, |_, window, cx| window.press(crate::kbd!("w"), cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        !has_close_dialog(cx) && !cx.has_pending_prompt(),
        "an already discarded observer cannot save or discard the task again"
    );
    assert!(native.update(cx, |_, _, _| ()).is_err());
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    let commands = drain(&mut fixture.commands);
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::QueryIntruderRuns {
            limit: PAGE_SIZE,
            ..
        } | Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery {
                limit: crate::history::HTTP_HISTORY_BATCH_SIZE,
                ..
            }
        }
    )));
}

#[gpui_kit::test]
fn inline_settings_save_receipt_keeps_saved_result_checked_and_disabled(cx: &mut TestAppContext) {
    let (mut fixture, mut run, _) = setup(cx);
    run.retention = IntruderRetention::Session;
    let path = PathBuf::from("/tmp/intruder-dashboard-settings.mio");
    update(&fixture, cx, |view, _, _| {
        view.project_path = Some(path.clone())
    });
    show_tasks(&mut fixture, vec![run.clone()], cx);
    let (view_id, query) = choose(&mut fixture, &run, cx);
    page(&fixture, &run, &query, &[], false, cx);
    click(fixture.window, "intruder-results-settings", cx);
    click(fixture.window, "intruder-result-save-to-project", cx);
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
        panic!("the Settings checkbox must request exactly one retention transition")
    };
    assert_eq!(
        (*project, *run_id, *expected_revision, *retention),
        (
            fixture.project,
            run.id,
            run.revision,
            IntruderRetention::Project
        )
    );
    let mut saved_run = run.clone();
    saved_run.revision += 1;
    saved_run.retention = IntruderRetention::Project;
    emit(
        &fixture,
        Event::IntruderRetentionChanged {
            project: fixture.project,
            run_id: run.id,
            operation_id: *operation_id,
            result: Ok(mio_core::IntruderRetentionReceipt {
                run: saved_run.clone(),
                retention: IntruderRetention::Project,
                workspace: *workspace.clone(),
                session_logical_bytes: 0,
            }),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, Command::SaveProject { path: None, .. }))
            .count(),
        1
    );
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::SaveProject { .. }
            | Command::QueryHttpHistory { .. }
            | Command::QueryIntruderResults { .. }
            | Command::QueryIntruderResultView { .. }
    )));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder.session_runs.contains(&run.id));
        assert_eq!(
            view.intruder.results.views[&view_id].summary.as_ref(),
            Some(&saved_run)
        );
        assert!(view.intruder_result_close.is_some());
    });
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        },
        cx,
    );
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| assert!(fixture.view.read(cx).intruder_result_close.is_none()));
    assert!(drain(&mut fixture.commands).is_empty());
    let saved_workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let checkbox = window.find("intruder-result-save-to-project");
        assert_eq!(checkbox.checked(), Some(true));
        assert_eq!(checkbox.disabled(), Some(true));
        window.click("intruder-result-save-to-project", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder.session_runs.contains(&run.id));
        assert!(view.intruder_result_close.is_none());
        assert_eq!(view.workspace, saved_workspace);
        let result = &view.intruder.results.views[&view_id];
        assert_eq!(result.summary.as_ref(), Some(&saved_run));
        assert_eq!(result.persisted, Some(true));
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
}
