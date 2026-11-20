use super::*;

fn archive_rows(source: Uuid, count: usize) -> Vec<HistoryRow> {
    (0..count)
        .map(|index| {
            HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
                item: ExtItemSummary {
                    id: Uuid::new_v4(),
                    source_id: source,
                    collection: "root.f1.f0".into(),
                    index: index as u64,
                    object_address: None,
                    state: "mapped".into(),
                    message_count: 0,
                },
                request: None,
                edited_request: None,
                response: None,
                edited_response: None,
                origin: Some("http://127.0.0.1:9001".into()),
                notes_revision: 0,
                notes_preview: String::new(),
                notes_error: None,
                highlight: Some(ExtHttpHighlight {
                    revision: 0,
                    color: 0,
                }),
                highlight_error: None,
                time_unix_ms: None,
                time_error: None,
            }))
        })
        .collect()
}

fn reply_snapshot(
    f: &Fixture,
    query: &HttpHistoryQuery,
    all_rows: &[HistoryRow],
    cx: &mut TestAppContext,
) {
    let end = (query.offset + query.limit).min(all_rows.len());
    let page_rows = all_rows[query.offset.min(all_rows.len())..end].to_vec();
    let mut selected_rows = Vec::new();
    let mut selected_ranks = Vec::new();
    let mut selected_matches = Vec::new();
    for id in &query.selected_ids {
        if let Some(rank) = all_rows.iter().position(|row| row.id() == *id) {
            selected_matches.push((*id, true));
            selected_ranks.push((*id, rank));
            if !page_rows.iter().any(|row| row.id() == *id) {
                selected_rows.push(all_rows[rank].clone());
            }
        } else {
            selected_matches.push((*id, false));
        }
    }
    f.events
        .try_send(Event::HttpHistoryPage {
            query: query.clone(),
            result: Ok(HttpHistoryResult {
                total: all_rows.len(),
                rows: page_rows,
                selected_rows,
                selected_ranks,
                selected_matches,
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn cross_page_shift_selection_collects_exact_range_keeps_current_page_and_guards_500_limit(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let all_rows = archive_rows(Uuid::new_v4(), 510);
    let initial = query(&mut f);
    reply_snapshot(&f, &initial, &all_rows[..PAGE_SIZE], cx);
    click(f.window, "proxy-history", cx);
    if let Ok(Command::QueryHttpHistory { query }) = f.commands.try_recv() {
        reply_snapshot(&f, &query, &all_rows[..PAGE_SIZE], cx);
    }
    assert!(drain(&mut f.commands).is_empty());

    // Apply an active sort and URL filter so Shift queries must preserve current query conditions.
    let url_input = cx.read(|cx| f.view.read(cx).history_filter.clone());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("history-filter").label(), Some("筛选"));
        })
        .unwrap();
    type_in(f.window, &url_input, "GET", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("history-filter").label(),
                Some("筛选"),
                "unacknowledged search input must remain in Search mode rather than Clear mode"
            );
        })
        .unwrap();
    click(f.window, ("col-header", 6usize), cx);
    let sorted = query(&mut f);
    assert_eq!(
        sorted.sort,
        HttpHistorySort {
            column: HttpHistorySortColumn::Status,
            ascending: false,
        }
    );
    assert_eq!(sorted.url_filter, "GET");
    f.events
        .try_send(Event::HttpHistoryPage {
            query: sorted.clone(),
            result: Ok(HttpHistoryResult {
                total: all_rows.len(),
                rows: all_rows[..PAGE_SIZE].to_vec(),
                selected_rows: Vec::new(),
                selected_ranks: Vec::new(),
                selected_matches: Vec::new(),
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("history-filter").label(),
                Some("清除筛选"),
                "acknowledged non-empty search must switch suffix button to Clear mode"
            );
        })
        .unwrap();
    let active_filter = cx.read(|cx| f.view.read(cx).http_filter.applied.clone());

    // Select row 198 on page 0 as anchor.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.history
                    .update(cx, |table, cx| table.scroll_to_row(198, cx));
            });
            window.render_frame(cx);
            window.click("table-cell-198:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.selected_history, Some(all_rows[198].id()));
        assert_eq!(
            view.history.read(cx).delegate().selection_anchor,
            Some(all_rows[198].id())
        );
    });

    // Advance to offset PAGE_SIZE.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(PAGE_SIZE, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let page_one = query(&mut f);
    assert_eq!(page_one.offset, PAGE_SIZE);
    assert_eq!(page_one.selected_ids, vec![all_rows[198].id()]);
    f.events
        .try_send(Event::HttpHistoryPage {
            query: page_one.clone(),
            result: Ok(HttpHistoryResult {
                total: all_rows.len(),
                rows: all_rows[PAGE_SIZE..PAGE_SIZE * 2].to_vec(),
                selected_rows: vec![all_rows[198].clone()],
                selected_ranks: vec![(all_rows[198].id(), 198)],
                selected_matches: vec![(all_rows[198].id(), true)],
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).history.read(cx).delegate().offset,
            PAGE_SIZE
        );
    });

    // Shift-left-click row 203 (visible index 3 on page 1) via real pointer event.
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let pos = window.find("table-cell-3:0").bounds().center();
            window.change_modifiers(
                gpui_kit::Modifiers {
                    shift: true,
                    ..Default::default()
                },
                cx,
            );
            window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
            window.change_modifiers(gpui_kit::Modifiers::default(), cx);
        })
        .unwrap();
    cx.run_until_parked();

    // Discover query must keep current offset 200, active filter/sort/url_filter, and [anchor, lead].
    let discover = query(&mut f);
    assert_eq!(discover.offset, PAGE_SIZE);
    assert_eq!(discover.url_filter, "GET");
    assert_eq!(discover.filter, active_filter);
    assert_eq!(discover.sort, sorted.sort);
    assert_eq!(
        discover.selected_ids,
        vec![all_rows[198].id(), all_rows[203].id()]
    );
    cx.read(|cx| assert!(f.view.read(cx).http_filter.pending_query().is_none()));
    reply_snapshot(&f, &discover, &all_rows, cx);

    // Missing page 0 (offset 0) is fetched next to collect rows 198..=203.
    let missing_page = query(&mut f);
    assert_eq!(missing_page.offset, 0);
    assert_eq!(missing_page.url_filter, "GET");
    assert_eq!(missing_page.filter, active_filter);
    assert_eq!(missing_page.sort, sorted.sort);
    assert!(missing_page.selected_ids.is_empty());
    reply_snapshot(&f, &missing_page, &all_rows, cx);
    assert!(drain(&mut f.commands).is_empty());

    let expected_order: Vec<_> = (198..=203).map(|idx| all_rows[idx].id()).collect();
    let expected_set: HashSet<_> = expected_order.iter().copied().collect();
    cx.read(|cx| {
        let view = f.view.read(cx);
        let delegate = view.history.read(cx).delegate();
        assert!(!view.history_loading);
        assert!(!delegate.loading);
        assert!(view.http_filter.history_shift.is_none());
        assert_eq!(delegate.offset, PAGE_SIZE);
        assert_eq!(delegate.rows, all_rows[PAGE_SIZE..PAGE_SIZE * 2]);
        assert_eq!(delegate.selected_ids, expected_set);
        assert_eq!(delegate.selected_order, expected_order);
        assert_eq!(view.selected_history, Some(all_rows[203].id()));
        assert_eq!(view.history.read(cx).selected_row(), Some(3));
        assert_eq!(delegate.selection_anchor, Some(all_rows[198].id()));
        assert_eq!(delegate.selection_lead, Some(all_rows[203].id()));
        for (rank, row) in all_rows.iter().enumerate().take(204).skip(198) {
            let id = row.id();
            assert_eq!(delegate.selected_ranks.get(&id), Some(&rank));
            assert!(delegate.row_by_id(id).is_some());
        }
    });

    // Return to page 0, select rank 0 as anchor, jump to page 2 (offset 400), Shift-click rank 505 (> 500 span).
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(0, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let back_zero = query(&mut f);
    assert_eq!(back_zero.offset, 0);
    reply_snapshot(&f, &back_zero, &all_rows, cx);
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.history
                    .update(cx, |table, cx| table.scroll_to_row(0, cx));
            });
            window.render_frame(cx);
            window.click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(PAGE_SIZE, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let to_one = query(&mut f);
    reply_snapshot(&f, &to_one, &all_rows, cx);
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(PAGE_SIZE * 2, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let to_two = query(&mut f);
    assert_eq!(to_two.offset, PAGE_SIZE * 2);
    reply_snapshot(&f, &to_two, &all_rows, cx);

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.history
                    .update(cx, |table, cx| table.scroll_to_row(105, cx));
            });
            window.render_frame(cx);
            let pos = window.find("table-cell-105:0").bounds().center();
            window.change_modifiers(
                gpui_kit::Modifiers {
                    shift: true,
                    ..Default::default()
                },
                cx,
            );
            window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
            window.change_modifiers(gpui_kit::Modifiers::default(), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let over_limit = query(&mut f);
    assert_eq!(over_limit.offset, PAGE_SIZE * 2);
    assert_eq!(
        over_limit.selected_ids,
        vec![all_rows[0].id(), all_rows[505].id()]
    );
    reply_snapshot(&f, &over_limit, &all_rows, cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "exceeding 500 items must reject immediately without fetching extra pages"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        let delegate = view.history.read(cx).delegate();
        assert!(!view.history_loading);
        assert!(!delegate.loading);
        assert!(view.http_filter.history_shift.is_none());
        assert_eq!(delegate.offset, PAGE_SIZE * 2);
        assert_eq!(delegate.selected_ids, HashSet::from([all_rows[0].id()]));
        assert_eq!(view.selected_history, Some(all_rows[0].id()));
        assert_eq!(delegate.selection_anchor, Some(all_rows[0].id()));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("500"))
        );
    });
}
