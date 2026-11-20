use super::*;

#[gpui_kit::test]
fn result_sort_queries_all_pages_and_rejects_cancelled_late_ack(cx: &mut TestAppContext) {
    let summary = run(PAGE_SIZE + 1, None);
    let rows = (0..=PAGE_SIZE)
        .map(|ordinal| {
            row(
                ordinal as u64,
                &flow(ordinal, if ordinal == PAGE_SIZE { 200 } else { 500 }),
                Some(b"x"),
            )
        })
        .collect::<Vec<_>>();
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(
        &f,
        initial.clone(),
        page(&summary, &initial, rows[..PAGE_SIZE].to_vec(), rows.len()),
        cx,
    );
    click(window, ("icon-sort", 2usize), cx);
    let (descending, descending_cancel) = query(&mut f);
    cells(window, &[("table-cell-0:0", "1")], cx);
    click(window, ("icon-sort", 2usize), cx);
    let (ascending, _) = query(&mut f);
    assert!(descending_cancel.is_cancelled());
    assert!(ascending.sort.ascending);
    assert!(!descending.sort.ascending);
    assert!(ascending.query_revision > descending.query_revision);
    assert_eq!((ascending.offset, descending.offset), (0, 0));
    let mut sorted = vec![rows[PAGE_SIZE].clone()];
    sorted.extend_from_slice(&rows[..PAGE_SIZE - 1]);
    answer(
        &f,
        ascending.clone(),
        page(&summary, &ascending, sorted, rows.len()),
        cx,
    );
    answer(
        &f,
        descending.clone(),
        page(
            &summary,
            &descending,
            rows[..PAGE_SIZE].to_vec(),
            rows.len(),
        ),
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.query.sort, ascending.sort);
        assert_eq!(
            view.query.page.as_ref().unwrap().rows[0].ordinal,
            PAGE_SIZE as u64
        );
        assert!(view.query.pending.is_none());
    });
    let request_number = (PAGE_SIZE + 1).to_string();
    cells(
        window,
        &[
            ("table-cell-0:0", request_number.as_str()),
            ("table-cell-0:2", "200"),
        ],
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).intruder.results.views[&summary.id]
                .query
                .page
                .as_ref()
                .unwrap()
                .rows[0]
                .ordinal,
            PAGE_SIZE as u64
        )
    });
    click(window, ("icon-sort", 1usize), cx);
    let (payload_desc, _) = query(&mut f);
    assert_eq!(
        payload_desc.sort,
        IntruderResultSort {
            column: IntruderResultSortColumn::Payload,
            ascending: false,
        }
    );
    click(window, ("icon-sort", 1usize), cx);
    let (payload_asc, _) = query(&mut f);
    assert_eq!(
        payload_asc.sort,
        IntruderResultSort {
            column: IntruderResultSortColumn::Payload,
            ascending: true,
        }
    );
    answer(
        &f,
        payload_asc.clone(),
        page(
            &summary,
            &payload_asc,
            rows[..PAGE_SIZE].to_vec(),
            rows.len(),
        ),
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn result_short_budget_page_fills_visible_tail_and_selects_queued_rank(cx: &mut TestAppContext) {
    let summary = run(4, None);
    let flows = [flow(0, 200), flow(1, 201), flow(2, 202), flow(3, 203)];
    let rows = flows
        .iter()
        .enumerate()
        .map(|(ordinal, flow)| row(ordinal as u64, flow, Some(b"bounded payload")))
        .collect::<Vec<_>>();
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(
        &f,
        initial.clone(),
        page(&summary, &initial, rows[..2].to_vec(), 4),
        cx,
    );
    cells(
        window,
        &[("table-cell-0:0", "1"), ("table-cell-1:0", "2")],
        cx,
    );
    cx.run_until_parked();
    let (missing, missing_cancel) = query(&mut f);
    assert_eq!(
        missing.offset, 2,
        "a short budget page must automatically continue at its first missing visible rank"
    );
    assert!(missing.selected_ordinal.is_none());

    click(window, "table-cell-0:0", cx);
    let mut selected_query = None;
    let mut first_detail = None;
    for command in drain(&mut f.commands) {
        match command {
            Command::QueryIntruderResultView { query, .. } => {
                assert!(selected_query.is_none());
                assert_eq!(
                    (query.project, query.run_id, query.view_id),
                    (f.project, summary.id, summary.id)
                );
                assert_eq!(
                    (query.offset, query.limit, query.selected_ordinal),
                    (2, PAGE_SIZE, Some(0))
                );
                assert_eq!(
                    (query.filter.clone(), query.sort),
                    (missing.filter.clone(), missing.sort)
                );
                assert!(query.query_revision > missing.query_revision);
                selected_query = Some(query);
            }
            Command::ReadFlow {
                project,
                flow_id,
                selection_revision,
            } => {
                assert!(first_detail.is_none());
                assert_eq!((project, flow_id), (f.project, flows[0].summary.id));
                first_detail = Some(selection_revision);
            }
            _ => panic!(
                "selecting a cached row may only read its flow and update the pending selection identity"
            ),
        }
    }
    assert!(missing_cancel.is_cancelled());
    let missing = selected_query.expect("tail query must now track the selected cached row");
    emit(
        &f,
        Event::FlowDetail {
            project: f.project,
            selection_revision: first_detail.expect("cached row selection reads its detail"),
            record: Box::new(flows[0].clone()),
            original: Box::default(),
        },
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());

    click(window, "table-cell-3:0", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "the in-flight tail query already covers this unloaded rank"
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.selected, Some(0));
        assert_eq!(view.detail.as_deref(), Some(&flows[0]));
        assert_eq!(view.query.row_at(0), Some(&rows[0]));
        assert_eq!(view.query.row_at(1), Some(&rows[1]));
    });
    let mut tail = page(&summary, &missing, rows[2..].to_vec(), 4);
    tail.selected_rank = Some(0);
    tail.selected_row = Some(rows[0].clone());
    answer(&f, missing.clone(), tail, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            project,
            flow_id,
            selection_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("the original unloaded-row click must read its exact flow once the rank arrives")
    };
    assert_eq!((*project, *flow_id), (f.project, flows[3].summary.id));
    emit(
        &f,
        Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(flows[3].clone()),
            original: Box::default(),
        },
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.selected, Some(3));
        assert_eq!(view.detail.as_deref(), Some(&flows[3]));
        for (rank, row) in rows.iter().enumerate() {
            assert_eq!(view.query.row_at(rank), Some(row));
        }
    });
    cells(
        window,
        &[
            ("table-cell-0:0", "1"),
            ("table-cell-1:0", "2"),
            ("table-cell-2:0", "3"),
            ("table-cell-3:0", "4"),
        ],
        cx,
    );
    cx.run_until_parked();
    assert!(
        drain(&mut f.commands).is_empty(),
        "complete visible rows must not oscillate back to offset zero"
    );
}

#[gpui_kit::test]
fn closing_result_window_cancels_view_query_but_preserves_attack_and_spec_boundary(
    cx: &mut TestAppContext,
) {
    let mut summary = run(5, None);
    summary.dispatched = 4;
    summary.completed = 4;
    summary.state = IntruderRunState::Running;
    let (mut f, window, pending, query_cancel) = open(&summary, cx);
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
    click(window, "intruder-results-positions", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadIntruderSpec {
            project, run_id, ..
        },
    ] = commands.as_slice()
    else {
        panic!("the Positions tab alone requests the frozen spec independently of result filtering")
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    assert!(!query_cancel.is_cancelled());
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(query_cancel.is_cancelled());
    assert!(!attack_cancel.is_cancelled());
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.intruder.results.views.contains_key(&summary.id));
        assert!(view.intruder.runs.contains_key(&summary.id));
    });
    let commands = drain(&mut f.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            after: None,
            limit: 1,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("the attack must retain only its unfiltered status probe after its last view closes")
    };
    assert_eq!((*project, *run_id), (f.project, summary.id));
    answer(&f, pending.clone(), page(&summary, &pending, vec![], 0), cx);
    assert!(drain(&mut f.commands).is_empty());
    assert!(!attack_cancel.is_cancelled());
}

#[gpui_kit::test]
fn result_filter_pending_row_click_requeries_selection_and_ignores_old_ack(
    cx: &mut TestAppContext,
) {
    let summary = run(3, None);
    let flows = [flow(0, 200), flow(1, 201), flow(2, 403)];
    let rows = flows
        .iter()
        .enumerate()
        .map(|(ordinal, flow)| row(ordinal as u64, flow, Some(b"payload")))
        .collect::<Vec<_>>();
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(
        &f,
        initial.clone(),
        page(&summary, &initial, rows.clone(), 3),
        cx,
    );
    select(&mut f, window, 0, &flows[0], cx);
    click(window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .query
            .filter_window
            .unwrap()
    });
    click(filter_window, "intruder-result-filter-2xx", cx);
    click(filter_window, "intruder-result-filter-apply", cx);
    let (old, old_cancel) = query(&mut f);
    assert_eq!(old.selected_ordinal, Some(0));

    click(window, "table-cell-1:0", cx);
    assert!(
        old_cancel.is_cancelled(),
        "the old selected identity cannot validate the newly clicked row"
    );
    let mut current = None;
    let mut detail_revision = None;
    for command in drain(&mut f.commands) {
        match command {
            Command::QueryIntruderResultView { query, cancel } => {
                assert!(
                    current.is_none(),
                    "one replacement view query is sufficient"
                );
                assert!(!cancel.is_cancelled());
                assert_eq!(
                    (query.project, query.run_id, query.view_id),
                    (f.project, summary.id, summary.id)
                );
                assert_eq!((query.offset, query.limit), (0, PAGE_SIZE));
                assert_eq!(query.filter, old.filter);
                assert_eq!(query.sort, old.sort);
                assert_eq!(query.selected_ordinal, Some(1));
                assert!(query.query_revision > old.query_revision);
                current = Some(query);
            }
            Command::ReadFlow {
                project,
                flow_id,
                selection_revision,
            } => {
                assert!(detail_revision.is_none());
                assert_eq!((project, flow_id), (f.project, flows[1].summary.id));
                detail_revision = Some(selection_revision);
            }
            _ => panic!(
                "a pending-filter row click may only read that flow and replace the view query"
            ),
        }
    }
    let current =
        current.expect("selection changed during the pending filter must replace its query");
    emit(
        &f,
        Event::FlowDetail {
            project: f.project,
            selection_revision: detail_revision.expect("the clicked row still loads its detail"),
            record: Box::new(flows[1].clone()),
            original: Box::default(),
        },
        cx,
    );
    emit(
        &f,
        Event::IntruderResultView {
            query: old.clone(),
            result: Err("late failure for the old selected identity".into()),
        },
        cx,
    );
    answer(
        &f,
        old.clone(),
        page(&summary, &old, vec![rows[2].clone()], 1),
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.selected, Some(1));
        assert_eq!(view.detail.as_deref(), Some(&flows[1]));
        assert_eq!(view.query.applied, IntruderResultFilter::default());
        assert_eq!(view.query.pending.as_ref().unwrap().query, current);
        assert!(view.query.error.is_none());
    });
    answer(
        &f,
        current.clone(),
        page(&summary, &current, vec![rows[2].clone()], 1),
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.query.applied.status_classes, [false, true, true, true]);
        assert_eq!(
            view.query.page.as_ref().unwrap().rows,
            vec![rows[2].clone()]
        );
        assert!(view.selected.is_none());
        assert!(view.detail.is_none());
    });
    assert!(drain(&mut f.commands).is_empty());
}
