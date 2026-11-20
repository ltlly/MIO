use super::*;

#[gpui_kit::test]
fn admission_probe_error_is_visible_and_explicit_retry_keeps_query_roles(cx: &mut TestAppContext) {
    let mut f = super::tests::intruder_fixture(cx, vec![super::tests::attack()], 64);
    click(f.window, "tool-intruder", cx);
    click(f.window, "intruder-start", cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("Start must first save its actual editor snapshot");
    };
    assert_eq!(*project, f.project);
    emit(
        &f,
        Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut f.commands);
    let [
        Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("only the acknowledged snapshot may start the attack");
    };
    assert_eq!(*project, f.project);
    let attack_cancel = cancel.clone();
    let mut summary = run(spec.validate().unwrap() as usize, None);
    summary.id = *run_id;
    summary.tab_id = *tab_id;
    summary.revision = 1;
    summary.spec_revision = spec.revision;
    summary.dispatched = 0;
    summary.completed = 0;
    summary.state = IntruderRunState::Running;
    summary.title.clear();
    summary.baseline = spec.baseline_request.then_some(mio_core::IntruderBaseline {
        ordinal: 0,
        completed: false,
    });
    summary.retention = if spec.save_to_project {
        mio_core::IntruderRetention::Project
    } else {
        mio_core::IntruderRetention::Session
    };
    emit(
        &f,
        Event::IntruderStarted {
            project: f.project,
            run_id: summary.id,
            tab_id: summary.tab_id,
            spec_revision: summary.spec_revision,
            result: Ok(summary.clone()),
        },
        cx,
    );
    let commands = drain(&mut f.commands);
    let [
        Command::SaveWorkspace { workspace, .. },
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("the new result window must reuse the admission status probe");
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    let admission = *query_revision;
    emit(
        &f,
        Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    let window = cx.read(|cx| f.view.read(cx).intruder.results.views[&summary.id].owner);
    let error = "admission metadata unavailable";
    emit(
        &f,
        Event::IntruderResults {
            project: f.project,
            run_id: summary.id,
            query_revision: admission,
            result: Err(error.into()),
        },
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.find_by_label(error).visible(),
            "a failed admission probe must be shown as an error, never an empty result set"
        );
    })
    .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    emit(&f, Event::IntruderChanged { project: f.project }, cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "an admission failure requires an explicit retry"
    );
    assert!(!attack_cancel.is_cancelled());

    click(window, "intruder-results-results", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "the Results click must explicitly retry the failed status probe without starting or sending again"
        );
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    let retry = *query_revision;
    assert!(retry > admission);
    emit(
        &f,
        Event::IntruderResults {
            project: f.project,
            run_id: summary.id,
            query_revision: admission,
            result: Err("late original error".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.pending, Some(retry));
        assert!(view.error.is_none());
    });
    assert!(drain(&mut f.commands).is_empty());
    emit(
        &f,
        Event::IntruderResults {
            project: f.project,
            run_id: summary.id,
            query_revision: retry,
            result: Ok(mio_core::IntruderResultPage {
                run: summary.clone(),
                rows: vec![],
                next_after: None,
                persisted: true,
            }),
        },
        cx,
    );
    let (display, display_cancel) = query(&mut f);
    assert_eq!(
        (display.run_id, display.view_id, display.offset),
        (summary.id, summary.id, 0)
    );
    assert!(display.query_revision > retry);
    assert!(!display_cancel.is_cancelled());
    let rows = summary
        .baseline
        .map(|baseline| mio_core::IntruderResultViewRow {
            ordinal: baseline.ordinal,
            result: None,
            payloads: vec![],
            response_length: None,
            notes_revision: 0,
            notes_preview: String::new(),
            highlight: None,
            highlight_revision: 0,
            grep_matches: Vec::new(),
            grep_extracts: Vec::new(),
            grep_payload_hits: Vec::new(),
        })
        .into_iter()
        .collect::<Vec<_>>();
    let total = rows.len();
    answer(
        &f,
        display.clone(),
        page(&summary, &display, rows, total),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label(error).is_none());
    })
    .unwrap();
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert!(view.error.is_none() && view.query.error.is_none());
        assert!(view.pending.is_none() && view.query.pending.is_none());
        assert!(view.query.page.is_some());
    });
    assert!(drain(&mut f.commands).is_empty());
    assert!(!attack_cancel.is_cancelled());
}

#[gpui_kit::test]
fn stop_uses_unfiltered_status_probe_while_display_filter_is_pending(cx: &mut TestAppContext) {
    let mut summary = run(3, None);
    summary.dispatched = 2;
    summary.completed = 2;
    summary.state = IntruderRunState::Running;
    let rows = vec![
        row(0, &flow(0, 200), Some(b"a")),
        row(1, &flow(1, 403), Some(b"b")),
    ];
    let (mut f, window, initial, _) = open(&summary, cx);
    let attack_cancel = CancellationToken::new();
    f.view.update(cx, |view, _| {
        view.intruder.runs.insert(
            summary.id,
            IntruderRun {
                summary: summary.clone(),
                cancel: attack_cancel.clone(),
            },
        );
    });
    answer(&f, initial.clone(), page(&summary, &initial, rows, 2), cx);
    click(window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .query
            .filter_window
            .unwrap()
    });
    click(filter_window, "intruder-result-filter-2xx", cx);
    click(filter_window, "intruder-result-filter-apply", cx);
    let (display, display_cancel) = query(&mut f);
    assert_eq!(display.filter.status_classes, [false, true, true, true]);
    assert!(!attack_cancel.is_cancelled());

    click(window, "intruder-results-results", cx);
    cx.update_window(window, |_, window, cx| {
        assert!(window.is_action_available(&CloseTab, cx));
        window.press(crate::kbd!("w"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(has_close_dialog(cx));
    assert!(drain(&mut f.commands).is_empty());
    answer_close_dialog("丢弃", cx);
    let commands = drain(&mut f.commands);
    let [Command::StopIntruder { project, run_id }] = commands.as_slice() else {
        panic!(
            "the real close/discard choice must stop immediately without waiting for the display query"
        )
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    assert!(attack_cancel.is_cancelled());
    assert!(
        !display_cancel.is_cancelled(),
        "display cancellation is independent of the running attack"
    );

    emit(&f, Event::IntruderChanged { project: f.project }, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "Stop progress must use one unfiltered status probe while the display query is still pending"
        )
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    assert!(*query_revision > display.query_revision);
    assert!(!display_cancel.is_cancelled());
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert!(view.stop_requested);
        assert_eq!(view.pending, Some(*query_revision));
        assert_eq!(view.query.pending.as_ref().unwrap().query, display);
        assert!(view.query.page.is_some());
    });
}

#[gpui_kit::test]
fn newer_control_summary_requeries_old_display_without_rolling_back_page_or_persistence(
    cx: &mut TestAppContext,
) {
    let mut summary = run(2, None);
    summary.dispatched = 1;
    summary.completed = 1;
    summary.state = IntruderRunState::Running;
    let (mut f, window, initial, _) = open(&summary, cx);
    let attack_cancel = CancellationToken::new();
    f.view.update(cx, |view, _| {
        view.intruder.runs.insert(
            summary.id,
            IntruderRun {
                summary: summary.clone(),
                cancel: attack_cancel.clone(),
            },
        );
    });
    let original = page(
        &summary,
        &initial,
        vec![row(0, &flow(0, 200), Some(b"a"))],
        1,
    );
    answer(&f, initial, original.clone(), cx);
    click(window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .query
            .filter_window
            .unwrap()
    });
    click(filter_window, "intruder-result-filter-2xx", cx);
    click(filter_window, "intruder-result-filter-apply", cx);
    let (display, display_cancel) = query(&mut f);

    click(window, "intruder-result-pause", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::SetIntruderPaused {
            project,
            run_id,
            operation_id,
            paused: true,
        },
    ] = commands.as_slice()
    else {
        panic!("the actual Pause icon must issue a correlated control command while filtering")
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    let mut terminal = summary.clone();
    terminal.revision += 2;
    terminal.state = IntruderRunState::Stopped;
    emit(
        &f,
        Event::IntruderRunControl {
            project: f.project,
            run_id: summary.id,
            operation_id: *operation_id,
            result: Ok(terminal.clone()),
        },
        cx,
    );
    assert!(
        drain(&mut f.commands).is_empty(),
        "pending display work must not be duplicated before its reply"
    );
    cx.read(|cx| {
        let workbench = f.view.read(cx);
        let view = &workbench.intruder.results.views[&summary.id];
        assert_eq!(view.summary.as_ref(), Some(&terminal));
        assert_eq!(view.query.page.as_ref(), Some(&original));
        assert_eq!(view.persisted, Some(true));
        assert!(!workbench.intruder.runs.contains_key(&summary.id));
    });

    let mut stale = page(&summary, &display, vec![], 0);
    stale.persisted = false;
    answer(&f, display.clone(), stale, cx);
    let (replacement, replacement_cancel) = query(&mut f);
    assert!(display_cancel.is_cancelled());
    assert!(!replacement_cancel.is_cancelled());
    assert!(replacement.query_revision > display.query_revision);
    assert_eq!(
        (replacement.project, replacement.run_id, replacement.view_id),
        (display.project, display.run_id, display.view_id)
    );
    assert_eq!(
        (
            replacement.offset,
            replacement.limit,
            replacement.selected_ordinal
        ),
        (display.offset, display.limit, display.selected_ordinal)
    );
    assert_eq!(
        (replacement.filter.clone(), replacement.sort),
        (display.filter, display.sort)
    );
    cx.read(|cx| {
        let workbench = f.view.read(cx);
        let view = &workbench.intruder.results.views[&summary.id];
        assert_eq!(view.summary.as_ref(), Some(&terminal));
        assert_eq!(view.query.page.as_ref(), Some(&original));
        assert_eq!(view.persisted, Some(true));
        assert!(view.query.error.is_none());
        assert!(!workbench.intruder.runs.contains_key(&summary.id));
    });
    answer(
        &f,
        replacement.clone(),
        page(&terminal, &replacement, vec![], 0),
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.summary.as_ref(), Some(&terminal));
        assert_eq!(view.persisted, Some(true));
        assert!(view.query.page.as_ref().unwrap().rows.is_empty());
        assert!(view.query.pending.is_none());
    });
    assert!(drain(&mut f.commands).is_empty());
    assert!(
        !attack_cancel.is_cancelled(),
        "a terminal control receipt must not be confused with local cancellation"
    );
}
