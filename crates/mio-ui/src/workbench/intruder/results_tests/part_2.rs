use super::*;

#[gpui_kit::test]
fn background_binary_detail_keeps_focus_in_its_result_window(cx: &mut TestAppContext) {
    let mut fixture = setup(cx, 64);
    let main_focus = focus_main_request(&mut fixture, cx);
    let mut a = run();
    let b = run();
    let mut binary = flow();
    binary.request.body = vec![0, 255, 7];
    let other = flow();
    let (a_window, a_query, _) = result_window(&mut fixture, a.id, cx);
    let (b_window, b_query, _) = result_window(&mut fixture, b.id, cx);
    page(
        &fixture,
        &a,
        &a_query,
        vec![row(0, &binary)],
        None,
        true,
        cx,
    );
    page(&fixture, &b, &b_query, vec![row(0, &other)], None, true, cx);
    let b_read = click_result(&mut fixture, b_window, 0, other.summary.id, cx);
    detail(&fixture, b_read, &other, cx);
    let b_focus = focus_result(&fixture, b_window, b.id, cx);
    let a_read = click_result(&mut fixture, a_window, 0, binary.summary.id, cx);
    let old_raw_focus = focus_result(&fixture, a_window, a.id, cx);
    emit_from_main(
        &fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: a_read,
            record: Box::new(binary.clone()),
            original: Box::default(),
        },
        cx,
    );
    cx.update_window(a_window, |_, window, cx| {
        window.render_frame(cx);
        fixture.view.update(cx, |view, cx| {
            let result = &view.intruder.results.views[&a.id];
            assert!(result.request.view == MessageView::Hex);
            assert_eq!(result.detail.as_deref(), Some(&binary));
            assert!(!old_raw_focus.is_focused(window));
            assert!(result.request.state().focus_handle(cx).is_focused(window));
            assert_eq!(
                result.request.bytes(window, cx).unwrap(),
                request_to_raw(&binary.request)
            );
        });
    })
    .unwrap();
    assert_focused(fixture.window, &main_focus, cx);
    assert_focused(b_window, &b_focus, cx);
    cx.update_window(a_window, |_, window, cx| {
        fixture
            .view
            .update(cx, |view, cx| request_page(view, a.id, 0, window, cx));
    })
    .unwrap();
    let refresh = next_query(&mut fixture, a.id, 0);
    a.revision += 1;
    binary.summary.revision += 1;
    binary.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\n\0\xff", HttpVersion::Http11).unwrap());
    page(
        &fixture,
        &a,
        &refresh,
        vec![row(0, &binary)],
        None,
        true,
        cx,
    );
    let response_read = detail_query(&mut fixture, binary.summary.id);
    let response = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&a.id]
            .response
            .state()
            .clone()
    });
    cx.update_window(a_window, |_, window, cx| {
        window.click(("input", response.entity_id()), cx);
        assert!(response.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    emit_from_main(
        &fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: response_read,
            record: Box::new(binary.clone()),
            original: Box::default(),
        },
        cx,
    );
    cx.update_window(a_window, |_, window, cx| {
        window.render_frame(cx);
        let result = &fixture.view.read(cx).intruder.results.views[&a.id];
        assert!(result.response.view == MessageView::Hex);
        assert!(!response.focus_handle(cx).is_focused(window));
        assert!(result.response.state().focus_handle(cx).is_focused(window));
    })
    .unwrap();
    assert_focused(fixture.window, &main_focus, cx);
    assert_focused(b_window, &b_focus, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn background_page_clear_does_not_leave_hidden_focus_or_touch_other_windows(
    cx: &mut TestAppContext,
) {
    let mut fixture = setup(cx, 64);
    let main_focus = focus_main_request(&mut fixture, cx);
    let a = run();
    let b = run();
    let first = flow();
    let second = flow();
    let (a_window, a_query, _) = result_window(&mut fixture, a.id, cx);
    let (b_window, b_query, _) = result_window(&mut fixture, b.id, cx);
    page(&fixture, &a, &a_query, vec![row(0, &first)], None, true, cx);
    page(
        &fixture,
        &b,
        &b_query,
        vec![row(0, &second)],
        None,
        true,
        cx,
    );
    let a_read = click_result(&mut fixture, a_window, 0, first.summary.id, cx);
    let b_read = click_result(&mut fixture, b_window, 0, second.summary.id, cx);
    detail(&fixture, a_read, &first, cx);
    detail(&fixture, b_read, &second, cx);
    let a_focus = focus_result(&fixture, a_window, a.id, cx);
    let b_focus = focus_result(&fixture, b_window, b.id, cx);
    for _ in 0..12 {
        fixture
            .events
            .try_send(Event::IntruderChanged {
                project: fixture.project,
            })
            .unwrap();
    }
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 2);
    let queries: HashMap<_, _> = commands
        .into_iter()
        .map(|command| {
            let Command::QueryIntruderResultView { query, cancel } = command else {
                panic!("one bounded refresh per results window");
            };
            assert_eq!(query.project, fixture.project);
            assert_eq!(query.view_id, query.run_id);
            assert_eq!((query.offset, query.limit), (0, PAGE_SIZE));
            assert_eq!(query.selected_ordinal, Some(0));
            assert!(!cancel.is_cancelled());
            (query.run_id, query)
        })
        .collect();
    emit_from_main(
        &fixture,
        Event::IntruderResultView {
            query: queries[&a.id].clone(),
            result: Ok(view_page(&a, &queries[&a.id], vec![], None, true)),
        },
        cx,
    );
    page(
        &fixture,
        &b,
        &queries[&b.id],
        vec![row(0, &second)],
        None,
        true,
        cx,
    );
    cx.update_window(a_window, |_, window, cx| {
        window.render_frame(cx);
        let result = &fixture.view.read(cx).intruder.results.views[&a.id];
        assert!(result.selected.is_none() && result.detail.is_none());
        assert!(!a_focus.is_focused(window));
        assert!(result.focus.is_focused(window));
        assert!(!fixture.view.read(cx).focus.is_focused(window));
    })
    .unwrap();
    assert_focused(fixture.window, &main_focus, cx);
    assert_focused(b_window, &b_focus, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn closing_a_result_window_drops_queued_detail_without_stopping_its_run(cx: &mut TestAppContext) {
    let mut fixture = setup(cx, 64);
    let run = run();
    let flow = flow();
    let cancel = CancellationToken::new();
    let (owner, query, spec) = result_window(&mut fixture, run.id, cx);
    page(&fixture, &run, &query, vec![row(0, &flow)], None, true, cx);
    let read = click_result(&mut fixture, owner, 0, flow.summary.id, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.intruder.runs.insert(
                run.id,
                IntruderRun {
                    summary: run.clone(),
                    cancel: cancel.clone(),
                },
            );
            view.query_intruder_results(run.id, Some(37), window, cx);
            view.on_intruder_result_event(
                &Event::FlowDetail {
                    project: fixture.project,
                    selection_revision: read,
                    record: Box::new(flow.clone()),
                    original: Box::default(),
                },
                window,
                cx,
            );
        });
        owner
            .update(cx, |_, window, cx| window.click("result-window-close", cx))
            .unwrap();
    })
    .unwrap();
    cx.run_until_parked();
    assert!(owner.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder.results.views.contains_key(&run.id));
        assert!(view.intruder.runs.contains_key(&run.id));
    });
    assert!(!cancel.is_cancelled());
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            run_id,
            after: None,
            limit: 1,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("closing the window only changes progress observation to a bounded status probe");
    };
    assert_eq!(*run_id, run.id);
    detail(&fixture, read, &flow, cx);
    page(&fixture, &run, &query, vec![row(0, &flow)], None, true, cx);
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: run.id,
            selection_revision: spec,
            result: Ok(Box::new(super::tests::attack().spec)),
        },
        cx,
    );
    cx.read(|cx| {
        assert!(
            !fixture
                .view
                .read(cx)
                .intruder
                .results
                .views
                .contains_key(&run.id)
        )
    });
    assert!(!cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn replacement_window_queries_cancel_old_cursors_and_keep_focus_until_selection_is_excluded(
    cx: &mut TestAppContext,
) {
    let mut fixture = setup(cx, 64);
    let main_focus = focus_main_request(&mut fixture, cx);
    let run = run();
    let flow = flow();
    let (owner, initial, _) = result_window(&mut fixture, run.id, cx);
    page(
        &fixture,
        &run,
        &initial,
        vec![row(0, &flow)],
        Some(40),
        true,
        cx,
    );
    let read = click_result(&mut fixture, owner, 0, flow.summary.id, cx);
    detail(&fixture, read, &flow, cx);
    let editor_focus = focus_result(&fixture, owner, run.id, cx);
    update(&fixture, cx, |view, window, cx| {
        for _ in 0..2 {
            view.on_intruder_result_event(
                &Event::IntruderChanged {
                    project: fixture.project,
                },
                window,
                cx,
            );
            view.refresh_intruder_results(window, cx);
        }
    });
    cx.run_until_parked();
    let refresh = next_query(&mut fixture, run.id, 0);
    assert!(refresh.query_revision > initial.query_revision);
    update(&fixture, cx, |view, window, cx| {
        request_page(view, run.id, 18, window, cx);
        request_page(view, run.id, 38, window, cx);
    });
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResultView {
            query: superseded,
            cancel: superseded_cancel,
        },
        Command::QueryIntruderResultView {
            query: latest,
            cancel: latest_cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("both cursor requests retain their own cancellation identity")
    };
    assert_eq!(
        (superseded.project, latest.project),
        (fixture.project, fixture.project)
    );
    assert_eq!((superseded.view_id, latest.view_id), (run.id, run.id));
    assert_eq!((superseded.run_id, latest.run_id), (run.id, run.id));
    assert_eq!((superseded.offset, latest.offset), (18, 38));
    assert_eq!((superseded.limit, latest.limit), (PAGE_SIZE, PAGE_SIZE));
    assert_eq!(
        (superseded.selected_ordinal, latest.selected_ordinal),
        (Some(0), Some(0))
    );
    assert!(superseded_cancel.is_cancelled());
    assert!(!latest_cancel.is_cancelled());
    assert!(
        latest.query_revision > superseded.query_revision
            && superseded.query_revision > refresh.query_revision
    );
    page(
        &fixture,
        &run,
        &refresh,
        vec![row(0, &flow)],
        Some(40),
        true,
        cx,
    );
    cx.update_window(owner, |_, window, cx| {
        window.render_frame(cx);
        let result = &fixture.view.read(cx).intruder.results.views[&run.id];
        assert_eq!(
            result.query.pending.as_ref().map(|pending| &pending.query),
            Some(latest)
        );
        assert_eq!(result.query.page.as_ref().unwrap().offset, 0);
        assert_eq!(displayed_rows(result), vec![row(0, &flow)]);
        assert_eq!(result.selected, Some(0));
        assert_eq!(result.detail.as_deref(), Some(&flow));
        assert!(editor_focus.is_focused(window));
    })
    .unwrap();
    assert_focused(fixture.window, &main_focus, cx);
    page(&fixture, &run, latest, vec![], Some(0), true, cx);
    cx.update_window(owner, |_, window, cx| {
        window.render_frame(cx);
        let result = &fixture.view.read(cx).intruder.results.views[&run.id];
        assert!(result.query.pending.is_none());
        assert!(displayed_rows(result).is_empty() && result.detail.is_none());
        assert!(result.selected.is_none());
        assert!(!editor_focus.is_focused(window));
        assert!(result.focus.is_focused(window));
    })
    .unwrap();
    assert_focused(fixture.window, &main_focus, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}
