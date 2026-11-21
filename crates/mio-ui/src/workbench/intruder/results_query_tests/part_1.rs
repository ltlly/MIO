use super::*;

#[gpui_kit::test]
fn result_response_received_and_length_columns_render_and_sort(cx: &mut TestAppContext) {
    let summary = run(3, None);
    let mut first = flow(0, 200);
    first.summary.response_bytes = 321;
    first.summary.elapsed_ms = 12;
    let mut second = flow(1, 404);
    second.summary.response_bytes = 54;
    second.summary.elapsed_ms = 7;
    let rows = vec![
        row(0, &first, Some(b"a")),
        row(1, &second, Some(b"b")),
        IntruderResultViewRow {
            ordinal: 2,
            result: Some(IntruderResultSummary {
                ordinal: 2,
                outcome: IntruderResultOutcome::Rejected("目标连接被拒绝".into()),
            }),
            payloads: Vec::new(),
            response_length: None,
            notes_revision: 0,
            notes_preview: String::new(),
            highlight: None,
            highlight_revision: 0,
            grep_matches: Vec::new(),
            grep_extracts: Vec::new(),
            grep_payload_hits: Vec::new(),
        },
    ];
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(&f, initial.clone(), page(&summary, &initial, rows, 3), cx);
    cells(
        window,
        &[
            ("table-cell-0:3", "12"),
            ("table-cell-0:4", "321"),
            ("table-cell-1:3", "7"),
            ("table-cell-1:4", "54"),
            ("table-cell-2:3", ""),
            ("table-cell-2:4", ""),
        ],
        cx,
    );

    click(window, ("icon-sort", 3usize), cx);
    let (received_desc, _) = query(&mut f);
    assert_eq!(
        received_desc.sort,
        IntruderResultSort {
            column: IntruderResultSortColumn::ResponseReceived,
            ascending: false,
        }
    );
    click(window, ("icon-sort", 3usize), cx);
    let (received_asc, _) = query(&mut f);
    assert_eq!(
        received_asc.sort,
        IntruderResultSort {
            column: IntruderResultSortColumn::ResponseReceived,
            ascending: true,
        }
    );

    click(window, ("icon-sort", 4usize), cx);
    let (length_desc, _) = query(&mut f);
    assert_eq!(
        length_desc.sort,
        IntruderResultSort {
            column: IntruderResultSortColumn::Length,
            ascending: false,
        }
    );
    click(window, ("icon-sort", 4usize), cx);
    let (length_asc, _) = query(&mut f);
    assert_eq!(
        length_asc.sort,
        IntruderResultSort {
            column: IntruderResultSortColumn::Length,
            ascending: true,
        }
    );
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn result_filter_search_term_hide_all_revert_show_all_and_ime_guard(cx: &mut TestAppContext) {
    let summary = run(2, None);
    let flows = [flow(0, 200), flow(1, 404)];
    let rows = [
        row(0, &flows[0], Some(b"alpha")),
        row(1, &flows[1], Some(b"beta")),
    ];
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(
        &f,
        initial.clone(),
        page(&summary, &initial, rows.to_vec(), 2),
        cx,
    );
    click(window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .query
            .filter_window
            .expect("filter window must open")
    });

    // Type search term, check Regex / Case sensitive / Negative search, then click Apply
    cx.update_window(filter_window, |_, window, cx| {
        window.render_frame(cx);
        window.click("input-state", cx);
        window.input("baseline-setting", cx);
        window.click("intruder-result-filter-regex", cx);
        window.click("intruder-result-filter-case-sensitive", cx);
        window.click("intruder-result-filter-negative", cx);
        window.click("intruder-result-filter-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (applied_search, _) = query(&mut f);
    assert_eq!(
        applied_search.filter,
        IntruderResultFilter {
            status_classes: [true; 4],
            search_term: "baseline-setting".into(),
            regex: true,
            case_sensitive: true,
            negative_search: true,
            only_with_notes: false,
            only_highlighted: false,
            only_grep_matched: false,
        }
    );
    answer(
        &f,
        applied_search.clone(),
        page(&summary, &applied_search, vec![rows[1].clone()], 1),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-view-filter").label(),
            Some("查看筛选：排除表达式 baseline-setting")
        );
    })
    .unwrap();

    // Reopen filter window, test Hide all preserves search_term & checkboxes, Revert restores applied, Show all clears all, and IME guard blocks Apply/Revert/Cancel
    click(window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .query
            .filter_window
            .expect("filter window must reopen")
    });
    cx.update_window(filter_window, |_, window, cx| {
        window.render_frame(cx);
        let dialog = window
            .root::<gpui_kit::base::Root>()
            .unwrap()
            .unwrap()
            .read(cx)
            .view()
            .clone()
            .downcast::<super::results_filter::ResultFilterWindow>()
            .unwrap();
        let term = dialog.read(cx).term.clone();
        assert_eq!(term.read(cx).value().as_ref(), "baseline-setting");
        window.click("intruder-result-filter-hide-all", cx);
        window.render_frame(cx);
        assert_eq!(dialog.read(cx).draft.status_classes, [false; 4]);
        assert_eq!(term.read(cx).value().as_ref(), "baseline-setting");
        assert!(
            dialog.read(cx).draft.regex
                && dialog.read(cx).draft.case_sensitive
                && dialog.read(cx).draft.negative_search
        );

        window.click("intruder-result-filter-all", cx);
        window.render_frame(cx);
        assert_eq!(dialog.read(cx).draft, IntruderResultFilter::default());
        assert_eq!(term.read(cx).value().as_ref(), "");

        window.click("intruder-result-filter-revert", cx);
        window.render_frame(cx);
        assert_eq!(dialog.read(cx).draft, applied_search.filter);
        assert_eq!(term.read(cx).value().as_ref(), "baseline-setting");

        // Start IME composition and verify Apply, Revert, All, and Cancel refuse to discard preedit
        window.click(("input", term.entity_id()), cx);
        term.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        for button_id in [
            "intruder-result-filter-apply",
            "intruder-result-filter-revert",
            "intruder-result-filter-all",
            "intruder-result-filter-cancel",
        ] {
            window.click(button_id, cx);
            window.render_frame(cx);
            assert!(
                term.update(cx, |input, cx| input
                    .marked_text_range(window, cx)
                    .is_some()),
                "{button_id} must preserve active IME composition"
            );
        }
        term.update(cx, |input, cx| {
            input.unmark_text(window, cx);
            input.set_value("中文命中", window, cx);
        });
        window.click("intruder-result-filter-negative", cx);
        window.click("intruder-result-filter-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (committed, _) = query(&mut f);
    assert_eq!(committed.filter.search_term, "中文命中");
    assert!(!committed.filter.negative_search);
    answer(
        &f,
        committed.clone(),
        page(&summary, &committed, vec![rows[0].clone()], 1),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-view-filter").label(),
            Some("查看筛选：匹配表达式 中文命中")
        );
    })
    .unwrap();
    assert!(drain(&mut f.commands).is_empty());
}

#[gpui_kit::test]
fn result_payloads_and_late_baseline_follow_sorted_rank_without_reading_spec(
    cx: &mut TestAppContext,
) {
    let summary = run(4, Some(2));
    let flows = [flow(0, 500), flow(1, 201), flow(2, 200), flow(3, 403)];
    let rows = [
        row(0, &flows[0], Some(b"alpha")),
        row(1, &flows[1], Some(b"")),
        row(2, &flows[2], None),
        row(3, &flows[3], Some(&[0, 255, 7])),
    ];
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(
        &f,
        initial.clone(),
        page(
            &summary,
            &initial,
            vec![
                rows[2].clone(),
                rows[0].clone(),
                rows[1].clone(),
                rows[3].clone(),
            ],
            4,
        ),
        cx,
    );
    cells(
        window,
        &[
            ("table-cell-0:0", "0"),
            ("table-cell-0:1", ""),
            ("table-cell-1:1", "alpha"),
            ("table-cell-2:1", "（空 payload）"),
            ("table-cell-3:1", "Hex: 00 ff 07"),
        ],
        cx,
    );
    assert!(
        drain(&mut f.commands).is_empty(),
        "Payload summaries must not read frozen specs"
    );

    click(window, ("icon-sort", 2usize), cx);
    let (descending, _) = query(&mut f);
    assert_eq!(descending.sort.column, IntruderResultSortColumn::Status);
    assert!(!descending.sort.ascending);
    answer(
        &f,
        descending.clone(),
        page(
            &summary,
            &descending,
            vec![
                rows[0].clone(),
                rows[3].clone(),
                rows[1].clone(),
                rows[2].clone(),
            ],
            4,
        ),
        cx,
    );
    cells(
        window,
        &[
            ("table-cell-0:0", "1"),
            ("table-cell-1:0", "3"),
            ("table-cell-3:0", "0"),
            ("table-cell-3:1", ""),
        ],
        cx,
    );
    select(&mut f, window, 1, &flows[3], cx);
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.selected, Some(3));
        assert_eq!(view.detail.as_deref(), Some(&flows[3]));
        assert!(view.spec.is_none());
    });
}

#[gpui_kit::test]
fn result_filter_failure_preserves_baseline_hex_and_success_excludes_it(cx: &mut TestAppContext) {
    let summary = run(4, Some(2));
    let flows = [flow(0, 500), flow(1, 201), flow(2, 200), flow(3, 403)];
    let rows = [
        row(2, &flows[2], None),
        row(0, &flows[0], Some(b"a")),
        row(1, &flows[1], Some(b"b")),
        row(3, &flows[3], Some(b"c")),
    ];
    let (mut f, window, initial, _) = open(&summary, cx);
    answer(
        &f,
        initial.clone(),
        page(&summary, &initial, rows.to_vec(), 4),
        cx,
    );
    select(&mut f, window, 0, &flows[2], cx);
    click(window, "intruder-result-hex", cx);
    let before_hex = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .request
            .hex
            .read(cx)
            .value()
            .to_string()
    });
    assert!(!before_hex.is_empty());
    click(window, "intruder-result-view-filter", cx);
    let filter_window = cx.read(|cx| {
        f.view.read(cx).intruder.results.views[&summary.id]
            .query
            .filter_window
            .expect("filter trigger opens its native form")
    });
    click(filter_window, "intruder-result-filter-2xx", cx);
    click(filter_window, "intruder-result-filter-apply", cx);
    let (failed, _) = query(&mut f);
    assert_eq!(failed.filter.status_classes, [false, true, true, true]);
    assert_eq!(failed.selected_ordinal, Some(2));
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.query.applied, IntruderResultFilter::default());
        assert_eq!(view.query.page.as_ref().unwrap().rows, rows);
        assert_eq!(view.detail.as_deref(), Some(&flows[2]));
    });
    emit(
        &f,
        Event::IntruderResultView {
            query: failed,
            result: Err("controlled result read failure".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert_eq!(view.query.filter_window, Some(filter_window));
        assert_eq!(view.query.applied, IntruderResultFilter::default());
        assert_eq!(view.query.page.as_ref().unwrap().rows, rows);
        assert_eq!(view.selected, Some(2));
        assert_eq!(view.detail.as_deref(), Some(&flows[2]));
        assert_eq!(view.request.view(), MessageView::Hex);
        assert_eq!(view.request.hex.read(cx).value().to_string(), before_hex);
    });
    cx.update_window(filter_window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-filter-2xx").checked(),
            Some(false)
        );
    })
    .unwrap();
    click(filter_window, "intruder-result-filter-apply", cx);
    let (retry, _) = query(&mut f);
    answer(
        &f,
        retry.clone(),
        page(&summary, &retry, vec![rows[1].clone(), rows[3].clone()], 2),
        cx,
    );
    cx.read(|cx| {
        let view = &f.view.read(cx).intruder.results.views[&summary.id];
        assert!(
            view.query.filter_window.is_none(),
            "Apply closes only after successful query"
        );
        assert_eq!(view.query.applied.status_classes, [false, true, true, true]);
        assert_eq!(view.query.page.as_ref().unwrap().total, 2);
        assert!(view.selected.is_none());
        assert!(view.detail.is_none());
        assert!(view.request.hex.read(cx).value().is_empty());
    });
    assert!(drain(&mut f.commands).is_empty());
}
