use super::*;

#[gpui_kit::test]
fn late_baseline_beyond_one_page_keeps_first_payload_visible_without_page_ping_pong(
    cx: &mut TestAppContext,
) {
    let baseline_ordinal = (PAGE_SIZE + 1) as u64;
    let mut config = attack();
    config.spec.payload_sets[0] = (0..baseline_ordinal)
        .map(|ordinal| ordinal.to_string().into_bytes())
        .collect();
    let mut fixture = intruder_fixture(cx, vec![config], 64);
    let mut started = start(&mut fixture, cx);
    // Only summaries cross the fixture channel; no body is delivered or prefetched.
    let first_rows: Vec<_> = (0..32)
        .map(|ordinal| row(ordinal, &flow(ordinal as usize)))
        .collect();
    let baseline_row = row(baseline_ordinal, &flow(baseline_ordinal as usize));
    started.summary.revision += 1;
    started.summary.dispatched = baseline_ordinal;
    started.summary.completed = baseline_ordinal;
    started.summary.state = IntruderRunState::Completed;
    page(
        &fixture,
        &started,
        started.probe,
        vec![first_rows[0].clone()],
        Some(0),
        cx,
    );
    let (initial, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    page(
        &fixture,
        &started,
        initial,
        first_rows.clone(),
        Some(31),
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    click(started.window, "intruder-results-settings", cx);
    let (operation, _) = request(&mut fixture, &started, cx);
    let accepted = receipt(&started);
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result: Ok(accepted.clone()),
        },
        cx,
    );
    let (refresh, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    started.summary = accepted.run;
    started.summary.revision += 1;
    started.summary.dispatched += 1;
    started.summary.completed += 1;
    started.summary.state = IntruderRunState::Completed;
    started.summary.baseline.as_mut().unwrap().completed = true;
    let refresh_query = view_query(&fixture, started.summary.id, refresh, cx);
    assert_eq!(refresh_query.offset, 0);
    let mut visible_rows = vec![baseline_row.clone()];
    visible_rows.extend(first_rows.iter().cloned());
    let refreshed = build_view_page(
        &refresh_query,
        &started.summary,
        visible_rows.clone(),
        Some(31),
        true,
    );
    assert_eq!(refreshed.total, baseline_ordinal as usize + 1);
    assert_eq!(refreshed.rows[0].ordinal, baseline_ordinal);
    assert_eq!(refreshed.rows[1].ordinal, 0);
    assert_eq!(refreshed.next_offset, Some(visible_rows.len()));
    emit(
        &fixture,
        Event::IntruderResultView {
            query: refresh_query,
            result: Ok(refreshed),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "the ordered view page already contains baseline and payloads; it must not ping-pong through legacy ordinal pages"
    );
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("0").visible());
        assert!(window.find_by_label("1").visible());
        assert!(window.try_find_by_label("0 · 未读取").is_none());
        assert!(window.try_find_by_label("1 · 未读取").is_none());
        assert!(
            window
                .find(("intruder-result-row", baseline_ordinal))
                .bounds()
                .origin
                .y
                < window
                    .find(("intruder-result-row", 0_usize))
                    .bounds()
                    .origin
                    .y
        );
    })
    .unwrap();
    for selected in [&baseline_row, &first_rows[0]] {
        cx.update_window(started.window, |_, window, cx| {
            window.click(("intruder-result-row", selected.ordinal), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut fixture.commands);
        let [
            Command::ReadFlow {
                project, flow_id, ..
            },
        ] = commands.as_slice()
        else {
            panic!(
                "a loaded baseline or payload click must read exactly its body, without changing summary pages"
            )
        };
        let IntruderResultOutcome::Flow(summary) = &selected.outcome else {
            panic!("flow summary")
        };
        assert_eq!((*project, *flow_id), (fixture.project, summary.id));
        cx.read(|cx| {
            let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
            assert_eq!(result.selected, Some(selected.ordinal));
            assert!(result.detail.is_none());
            assert!(
                result.query.page.as_ref().unwrap().rows.len() <= visible_rows.len(),
                "the UI must keep only the bounded query page, not collect the whole attack to pin one baseline"
            );
        });
    }
    for delta in [-100., 100.] {
        cx.update_window(started.window, |_, window, cx| {
            window.scroll(
                "intruder-results-table",
                ScrollDelta::Pixels(point(px(0.), px(delta))),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "scrolling within the loaded viewport must not ping-pong baseline and payload pages"
        );
    }
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("0").visible());
        assert!(window.find_by_label("1").visible());
    })
    .unwrap();
    assert!(!accepted.cancel.is_cancelled());
}

#[gpui_kit::test]
fn completed_baseline_refreshes_cached_summary_and_rejects_old_body_and_page(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, payloads) = setup(cx);
    let (operation, _) = request(&mut fixture, &started, cx);
    let accepted = receipt(&started);
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result: Ok(accepted.clone()),
        },
        cx,
    );
    let (pending_revision, _) = query(&mut fixture, started.summary.id);
    let pending_query = view_query(&fixture, started.summary.id, pending_revision, cx);
    started.summary = accepted.run;
    started.summary.revision += 1;
    started.summary.dispatched += 1;
    let mut pending_flow = flow(2);
    pending_flow.summary.state = mio_core::FlowState::Pending;
    let mut rows = payloads.clone();
    rows.push(row(2, &pending_flow));
    let pending_page = build_view_page(&pending_query, &started.summary, rows, None, true);
    assert_eq!(pending_page.rows[0].ordinal, 2);
    let stale_page = Event::IntruderResultView {
        query: pending_query.clone(),
        result: Ok(pending_page.clone()),
    };
    emit(&fixture, stale_page.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let old_body = select(&mut fixture, &started, 2, &pending_flow, cx);
    detail(&fixture, old_body, &pending_flow, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail
                .as_deref(),
            Some(&pending_flow)
        )
    });
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (completed_revision, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    let completed_query = view_query(&fixture, started.summary.id, completed_revision, cx);
    assert_eq!(completed_query.selected_ordinal, Some(2));
    started.summary.revision += 1;
    started.summary.completed += 1;
    started.summary.state = IntruderRunState::Completed;
    started.summary.baseline.as_mut().unwrap().completed = true;
    let mut completed_flow = pending_flow.clone();
    completed_flow.summary.revision += 1;
    completed_flow.summary.state = mio_core::FlowState::Complete;
    completed_flow.summary.status = Some(204);
    completed_flow.summary.elapsed_ms = 12;
    completed_flow.summary.response_bytes = 64;
    let mut completed_rows = payloads.clone();
    completed_rows.push(row(2, &completed_flow));
    let completed_page = build_view_page(
        &completed_query,
        &started.summary,
        completed_rows,
        None,
        true,
    );
    assert_eq!(completed_page.rows[0].ordinal, 2);
    assert_eq!(completed_page.selected_rank, Some(0));
    emit(
        &fixture,
        Event::IntruderResultView {
            query: completed_query,
            result: Ok(completed_page),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision: new_body,
            flow_id,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "the complete ordered page needs one selected-body refresh and no baseline/payload-page ping-pong"
        )
    };
    assert_eq!(
        (*project, *flow_id),
        (fixture.project, completed_flow.summary.id)
    );
    let new_body = *new_body;
    assert!(new_body > old_body);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.row(2), Some(&row(2, &completed_flow)));
        assert!(
            result.detail.is_none(),
            "pending baseline body must clear before the completed body arrives"
        );
        assert_eq!(result.selected, Some(2));
    });
    detail(&fixture, old_body, &pending_flow, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail
                .is_none()
        )
    });
    detail(&fixture, new_body, &completed_flow, cx);
    detail(&fixture, old_body, &pending_flow, cx);
    emit(&fixture, stale_page, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "old typed view callbacks cannot refresh or overwrite the completed baseline"
    );
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.row(2), Some(&row(2, &completed_flow)));
        assert_eq!(result.detail.as_deref(), Some(&completed_flow));
        assert_eq!(result.selected, Some(2));
        assert_eq!(result.summary, Some(started.summary.clone()));
        let page = result.query.page.as_ref().unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(
            page.rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
            [2, 0, 1]
        );
    });
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("204").visible());
        assert!(window.find_by_label("0").visible());
        assert!(window.find_by_label("1").visible());
    })
    .unwrap();
}
