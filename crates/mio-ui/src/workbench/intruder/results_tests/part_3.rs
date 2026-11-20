use super::*;

#[gpui_kit::test]
fn a_deferred_old_project_page_cannot_fill_a_reopened_result_view(cx: &mut TestAppContext) {
    let mut fixture = setup(cx, 64);
    let run = run();
    let old_flow = flow();
    let latest_flow = flow();
    let cancel = CancellationToken::new();
    let old_project = fixture.project;
    let next_project = ProjectKey {
        id: old_project.id,
        generation: old_project.generation + 1,
    };
    let (owner, old_query, _) = result_window(&mut fixture, run.id, cx);
    let old_editor = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&run.id]
            .request
            .raw
            .entity_id()
    });
    assert_eq!(old_query.project, old_project);
    let old_event = Event::IntruderResultView {
        query: old_query.clone(),
        result: Ok(view_page(
            &run,
            &old_query,
            vec![row(0, &old_flow)],
            None,
            true,
        )),
    };
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_intruder_result_event(&old_event, window, cx);
            view.on_event(
                Event::ProjectOpened {
                    project: next_project,
                    path: None,
                    workspace: Workspace::default(),
                },
                window,
                cx,
            );
        });
        owner
            .update(cx, |_, window, cx| {
                fixture.view.update(cx, |view, cx| {
                    view.select_intruder_run(run.id, window, cx);
                    view.intruder.runs.insert(
                        run.id,
                        IntruderRun {
                            summary: run.clone(),
                            cancel: cancel.clone(),
                        },
                    );
                });
            })
            .unwrap();
    })
    .unwrap();
    fixture.project = next_project;
    cx.run_until_parked();
    let (query, _) = opening_queries(&mut fixture, run.id);
    assert_eq!(
        query.query_revision, old_query.query_revision,
        "query numbers may repeat after a project reset"
    );
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&run.id];
        assert_ne!(result.request.raw.entity_id(), old_editor);
        assert_eq!(
            result.query.pending.as_ref().map(|pending| &pending.query),
            Some(&query)
        );
        assert!(result.summary.is_none() && result.query.page.is_none());
    });
    page(
        &fixture,
        &run,
        &query,
        vec![row(0, &latest_flow)],
        None,
        true,
        cx,
    );
    emit_from_main(&fixture, old_event, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            displayed_rows(&view.intruder.results.views[&run.id]),
            vec![row(0, &latest_flow)]
        );
        assert!(view.intruder.runs.contains_key(&run.id));
    });
    assert!(!cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryHttpHistory {
            query:
                mio_core::HttpHistoryQuery {
                    project,
                    offset: 0,
                    limit,
                    url_filter: filter,
                    ..
                },
        },
    ] = commands.as_slice()
    else {
        panic!("the new project only starts its normal first history-page read");
    };
    assert_eq!(*project, next_project);
    assert_eq!(*limit, crate::history::HTTP_HISTORY_BATCH_SIZE);
    assert!(filter.is_empty());
}

#[gpui_kit::test]
fn changed_events_coalesce_and_query_failures_need_explicit_retry(cx: &mut TestAppContext) {
    let mut fixture = setup(cx, 64);
    let run = run();
    let flow = flow();
    let history_revision = cx.read(|cx| fixture.view.read(cx).query_revision);
    let (initial, _) = open(&mut fixture, run.id, cx);
    for _ in 0..12 {
        fixture
            .events
            .try_send(Event::IntruderChanged {
                project: fixture.project,
            })
            .unwrap();
    }
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "do not duplicate in-flight reads"
    );
    page(
        &fixture,
        &run,
        &initial,
        vec![row(0, &flow)],
        None,
        true,
        cx,
    );
    let refreshed = next_query(&mut fixture, run.id, 0);
    assert!(refreshed.query_revision > initial.query_revision);
    page(
        &fixture,
        &run,
        &refreshed,
        vec![row(0, &flow)],
        None,
        true,
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).query_revision, history_revision));

    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let failing = next_query(&mut fixture, run.id, 0);
    emit(
        &fixture,
        Event::IntruderResultView {
            query: failing.clone(),
            result: Err("disk read failed".into()),
        },
        cx,
    );
    for _ in 0..8 {
        emit(
            &fixture,
            Event::IntruderChanged {
                project: fixture.project,
            },
            cx,
        );
    }
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "errors must not busy-retry"
    );
    cx.read(|cx| {
        let view = &fixture.view.read(cx).intruder.results.views[&run.id];
        assert_eq!(displayed_rows(view), vec![row(0, &flow)]);
        assert_eq!(view.query.error.as_deref(), Some("disk read failed"));
        assert!(view.query.pending.is_none());
    });
    update(&fixture, cx, |view, window, cx| {
        request_page(view, run.id, 0, window, cx)
    });
    let retry = next_query(&mut fixture, run.id, 0);
    emit(
        &fixture,
        Event::IntruderResultView {
            query: failing,
            result: Err("late error".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = &fixture.view.read(cx).intruder.results.views[&run.id];
        assert_eq!(
            view.query.pending.as_ref().map(|pending| &pending.query),
            Some(&retry)
        );
        assert!(view.query.error.is_none());
    });
    page(&fixture, &run, &retry, vec![], None, true, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn run_and_result_cursors_keep_windows_independent_and_reject_late_pages(cx: &mut TestAppContext) {
    let mut fixture = setup(cx, 64);
    let mut a = run();
    a.dispatched = 39;
    let b = run();
    let first = flow();
    let second = flow();
    let third = flow();
    let mut first_page = vec![row(0, &first)];
    first_page.extend((1..38).map(|ordinal| row(ordinal, &flow())));
    let (a_query, _) = open(&mut fixture, a.id, cx);
    let (b_query, _) = open(&mut fixture, b.id, cx);
    page(
        &fixture,
        &b,
        &b_query,
        vec![row(0, &second)],
        None,
        true,
        cx,
    );
    page(
        &fixture,
        &a,
        &a_query,
        first_page.clone(),
        Some(39),
        true,
        cx,
    );
    let old_detail = select(&mut fixture, a.id, 0, first.summary.id, cx);
    update(&fixture, cx, |view, window, cx| {
        let offset = view.intruder.results.views[&a.id]
            .query
            .page
            .as_ref()
            .unwrap()
            .next_offset
            .unwrap();
        assert_eq!(offset, 38);
        request_page(view, a.id, offset, window, cx);
    });
    let next = next_query(&mut fixture, a.id, 38);
    detail(&fixture, old_detail, &first, cx);
    emit(
        &fixture,
        Event::IntruderResultView {
            query: IntruderResultViewQuery {
                project: ProjectKey {
                    generation: fixture.project.generation + 1,
                    ..fixture.project
                },
                ..next.clone()
            },
            result: Err("wrong generation".into()),
        },
        cx,
    );
    emit(
        &fixture,
        Event::IntruderResultView {
            query: IntruderResultViewQuery {
                run_id: b.id,
                ..next.clone()
            },
            result: Err("wrong run".into()),
        },
        cx,
    );
    page(&fixture, &a, &a_query, vec![row(0, &first)], None, true, cx);
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert_eq!(
            views[&a.id]
                .query
                .pending
                .as_ref()
                .map(|pending| &pending.query),
            Some(&next)
        );
        assert_eq!(views[&a.id].selected, Some(0));
        assert_eq!(views[&a.id].detail.as_deref(), Some(&first));
        assert_eq!(displayed_rows(&views[&a.id]), first_page);
        assert_eq!(displayed_rows(&views[&b.id]), vec![row(0, &second)]);
    });
    let mut next_page = view_page(&a, &next, vec![row(38, &third)], Some(39), true);
    next_page.selected_rank = Some(0);
    let selected = row(0, &first);
    next_page.selected_row = Some(IntruderResultViewRow {
        ordinal: 0,
        result: Some(selected.clone()),
        payloads: vec![],
        response_length: match &selected.outcome {
            IntruderResultOutcome::Flow(flow) if flow.status.is_some() => Some(flow.response_bytes),
            _ => None,
        },
        notes_revision: 0,
        notes_preview: String::new(),
        highlight: None,
        highlight_revision: 0,
        grep_matches: Vec::new(),
        grep_extracts: Vec::new(),
        grep_payload_hits: Vec::new(),
    });
    emit(
        &fixture,
        Event::IntruderResultView {
            query: next,
            result: Ok(next_page),
        },
        cx,
    );
    update(&fixture, cx, |view, _, cx| {
        view.query_intruder_runs(None, cx)
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderRuns {
            after: None,
            query_revision,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("first run-list page expected")
    };
    let cursor = Uuid::from_u128(17);
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(IntruderRunPage {
                rows: vec![a.clone()],
                next_after: Some(cursor),
            }),
        },
        cx,
    );
    update(&fixture, cx, |view, _, cx| {
        view.query_intruder_runs(view.intruder.results.runs.next_after, cx);
    });
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::QueryIntruderRuns { after: Some(id), .. }] if *id == cursor)
    );
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert_eq!(displayed_rows(&views[&a.id]), vec![row(38, &third)]);
        assert_eq!(views[&a.id].selected, Some(0));
        assert_eq!(views[&a.id].detail.as_deref(), Some(&first));
        assert_eq!(
            views[&a.id].query.page.as_ref().unwrap().selected_rank,
            Some(0)
        );
        assert_eq!(
            displayed_rows(&views[&b.id])[0].outcome,
            row(0, &second).outcome
        );
    });
}
