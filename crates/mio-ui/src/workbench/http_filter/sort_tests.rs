use super::*;
use gpui_kit::ClipboardItem;
use mio_core::{FlowState, HttpHistorySortColumn};

fn sort(column: HttpHistorySortColumn, ascending: bool) -> HttpHistorySort {
    HttpHistorySort { column, ascending }
}

fn page(
    f: &Fixture,
    query: &HttpHistoryQuery,
    rows: Vec<HistoryRow>,
    total: usize,
    selected_ranks: Vec<(HistoryEntryId, usize)>,
    selected_rows: Vec<HistoryRow>,
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::HttpHistoryPage {
            query: query.clone(),
            result: Ok(HttpHistoryResult {
                selected_rows,
                total,
                rows,
                selected_matches: query
                    .selected_ids
                    .iter()
                    .map(|id| {
                        (
                            *id,
                            selected_ranks.iter().any(|(selected, _)| selected == id),
                        )
                    })
                    .collect(),
                selected_ranks,
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn header_sort_waits_for_latest_query_and_keeps_selected_detail_when_the_row_moves_offpage(
    cx: &mut TestAppContext,
) {
    let (mut f, _) = boot(cx);
    let mut first = record("http://127.0.0.1:9001/selected", "selected request");
    first.summary.status = Some(500);
    first.summary.state = FlowState::Complete;
    let mut second = record("http://127.0.0.1:9002/other", "other request");
    second.summary.status = Some(200);
    second.summary.state = FlowState::Complete;
    let initial_rows = vec![
        HistoryRow::Live(first.summary.clone()),
        HistoryRow::Live(second.summary.clone()),
    ];
    click(f.window, "history-filter", cx);
    let refreshed = query(&mut f);
    page(&f, &refreshed, initial_rows.clone(), 3, vec![], vec![], cx);

    click(f.window, "table-cell-0:0", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            flow_id,
            selection_revision,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("selection must read exactly the selected request")
    };
    assert_eq!(*flow_id, first.summary.id);
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(first.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    let selected = HistoryEntryId::Live(first.summary.id);
    let (request, editor_revision, detail_revision) = cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.selected_history, Some(selected));
        assert!(view.detail.is_some());
        (
            view.history_request.raw.read(cx).value().to_string(),
            view.history_request.raw.read(cx).document_revision(),
            view.history_detail_revision,
        )
    });

    click(f.window, ("col-header", 6usize), cx);
    let descending = query(&mut f);
    assert_eq!(descending.sort, sort(HttpHistorySortColumn::Status, false));
    assert_eq!(descending.offset, 0);
    assert_eq!(descending.selected_ids, [selected]);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, HttpHistorySort::default());
        assert_eq!(view.history.read(cx).delegate().rows, initial_rows);
    });
    click(f.window, ("col-header", 6usize), cx);
    let ascending = query(&mut f);
    assert!(ascending.query_revision > descending.query_revision);
    assert_eq!(ascending.sort, sort(HttpHistorySortColumn::Status, true));
    page(
        &f,
        &descending,
        initial_rows.clone(),
        3,
        vec![(selected, 0)],
        vec![],
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, HttpHistorySort::default());
        assert_eq!(view.history.read(cx).delegate().rows, initial_rows);
        assert!(view.history_loading);
    });

    // A bounded short page can omit the selected row while its global match remains true.
    let ascending_page = vec![HistoryRow::Live(second.summary.clone())];
    page(
        &f,
        &ascending,
        ascending_page.clone(),
        3,
        vec![(selected, 2)],
        vec![HistoryRow::Live(first.summary.clone())],
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, ascending.sort);
        assert_eq!(view.history.read(cx).delegate().rows, ascending_page);
        assert_eq!(view.selected_history, Some(selected));
        assert_eq!(view.history.read(cx).selected_row(), None);
        assert_eq!(
            view.history
                .read(cx)
                .delegate()
                .selected_ranks
                .get(&selected),
            Some(&2)
        );
        assert!(
            view.history
                .read(cx)
                .delegate()
                .row_by_id(selected)
                .is_some()
        );
        assert_eq!(view.history_request.raw.read(cx).value().as_ref(), request);
        assert_eq!(
            view.history_request.raw.read(cx).document_revision(),
            editor_revision
        );
        assert_eq!(view.history_detail_revision, detail_revision);
        assert_eq!(view.detail.as_ref().unwrap().summary.id, first.summary.id);
        assert!(!view.history_loading);
    });
    assert!(
        drain(&mut f.commands).is_empty(),
        "sorting must not reread or resend the selected request"
    );

    click(f.window, ("col-header", 0usize), cx);
    let num_desc = query(&mut f);
    assert_eq!(num_desc.sort, sort(HttpHistorySortColumn::Number, false));
    let mut many_rows: Vec<HistoryRow> = (0..39)
        .map(|i| HistoryRow::Live(record(&format!("http://127.0.0.1:9002/item-{i}"), "").summary))
        .collect();
    many_rows.push(HistoryRow::Live(first.summary.clone()));
    page(
        &f,
        &num_desc,
        many_rows,
        40,
        vec![(selected, 39)],
        vec![],
        cx,
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).selected_history, Some(selected));
            assert_eq!(f.view.read(cx).history.read(cx).selected_row(), Some(39));
            assert!(window.find("table-cell-0:0").visible());
        })
        .unwrap();
}

#[gpui_kit::test]
fn rejected_sort_query_restores_the_applied_header_before_the_next_click(cx: &mut TestAppContext) {
    let (mut f, rows) = boot(cx);
    click(f.window, ("col-header", 6usize), cx);
    let status = query(&mut f);
    assert_eq!(status.sort, sort(HttpHistorySortColumn::Status, false));
    reply(&f, &status, rows.clone(), cx);

    click(f.window, ("col-header", 1usize), cx);
    let host = query(&mut f);
    assert_eq!(host.sort, sort(HttpHistorySortColumn::Host, false));
    f.events
        .try_send(Event::HttpHistoryPage {
            query: host,
            result: Err("controlled snapshot read failure".into()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, status.sort);
        assert_eq!(view.history.read(cx).delegate().rows, rows);
        assert!(!view.history_loading);
    });
    assert!(drain(&mut f.commands).is_empty());
    click(f.window, ("col-header", 1usize), cx);
    let retry = query(&mut f);
    assert_eq!(
        retry.sort,
        sort(HttpHistorySortColumn::Host, false),
        "the failed column must return to its unsorted state, not advance to ascending"
    );
    reply(&f, &retry, rows, cx);
}

#[gpui_kit::test]
fn invalid_url_input_rejects_sort_without_leaving_the_header_on_an_unapplied_direction(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = boot(cx);
    let input = cx.read(|cx| f.view.read(cx).history_filter.clone());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", input.entity_id()), cx);
            assert!(input.focus_handle(cx).is_focused(window));
            let before = input.read(cx).document_revision();
            cx.write_to_clipboard(ClipboardItem::new_string("x".repeat(4097)));
            window.press(crate::kbd!("v"), cx);
            assert_eq!(input.read(cx).value().len(), 4097);
            assert!(input.read(cx).document_revision() > before);
        })
        .unwrap();
    cx.run_until_parked();
    click(f.window, ("col-header", 6usize), cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "invalid input must reject the query synchronously"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, HttpHistorySort::default());
        assert_eq!(view.history.read(cx).delegate().rows, rows);
        assert!(view.http_filter.pending_query().is_none());
        assert!(!view.history_loading);
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("4096"))
        );
    });
    f.window
        .update(cx, |_, window, cx| {
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.press("backspace", cx);
            assert!(input.read(cx).value().is_empty());
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    click(f.window, ("col-header", 6usize), cx);
    let first_valid = query(&mut f);
    assert_eq!(
        first_valid.sort,
        sort(HttpHistorySortColumn::Status, false),
        "an unsubmitted sort must restore its header before accepting the next click"
    );
    reply(&f, &first_valid, rows, cx);
}

#[gpui_kit::test]
fn length_header_sort_toggles_descending_and_ascending_queries(cx: &mut TestAppContext) {
    let (mut f, _) = boot(cx);
    let mut short = record("http://127.0.0.1:9001/short", "short");
    short.summary.response_bytes = 64;
    short.summary.state = FlowState::Complete;
    let mut long = record("http://127.0.0.1:9002/long", "long");
    long.summary.response_bytes = 2048;
    long.summary.state = FlowState::Complete;
    let initial_rows = vec![
        HistoryRow::Live(short.summary.clone()),
        HistoryRow::Live(long.summary.clone()),
    ];
    click(f.window, "history-filter", cx);
    let refreshed = query(&mut f);
    page(&f, &refreshed, initial_rows.clone(), 2, vec![], vec![], cx);

    click(f.window, ("col-header", 7usize), cx);
    let descending = query(&mut f);
    assert_eq!(descending.sort, sort(HttpHistorySortColumn::Length, false));
    assert_eq!(descending.offset, 0);
    let descending_rows = vec![
        HistoryRow::Live(long.summary.clone()),
        HistoryRow::Live(short.summary.clone()),
    ];
    page(
        &f,
        &descending,
        descending_rows.clone(),
        2,
        vec![],
        vec![],
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, descending.sort);
        assert_eq!(view.history.read(cx).delegate().rows, descending_rows);
    });

    click(f.window, ("col-header", 7usize), cx);
    let ascending = query(&mut f);
    assert!(ascending.query_revision > descending.query_revision);
    assert_eq!(ascending.sort, sort(HttpHistorySortColumn::Length, true));
    page(&f, &ascending, initial_rows.clone(), 2, vec![], vec![], cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.http_filter.sort, ascending.sort);
        assert_eq!(view.history.read(cx).delegate().rows, initial_rows);
    });
}

#[gpui_kit::test]
fn capture_number_column_stays_bound_to_each_record_across_number_status_and_url_sorts(
    cx: &mut TestAppContext,
) {
    let (mut f, _) = boot(cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("history-previous").is_none());
            assert!(window.try_find("history-next").is_none());
        })
        .unwrap();

    let mut flow_a = record("http://127.0.0.1:9001/a", "a");
    flow_a.summary.source = TrafficSource::Proxy;
    flow_a.summary.status = Some(500);
    flow_a.summary.started_unix_ms = 100;
    let mut flow_b = record("http://127.0.0.1:9001/b", "b");
    flow_b.summary.source = TrafficSource::Proxy;
    flow_b.summary.status = Some(200);
    flow_b.summary.started_unix_ms = 200;
    let mut flow_c = record("http://127.0.0.1:9001/c", "c");
    flow_c.summary.source = TrafficSource::Proxy;
    flow_c.summary.status = Some(404);
    flow_c.summary.started_unix_ms = 300;

    let live_numbers = vec![
        (flow_a.summary.id, 1),
        (flow_b.summary.id, 2),
        (flow_c.summary.id, 3),
    ];

    // Default # descending: #3 (/c), #2 (/b), #1 (/a)
    click(f.window, "history-filter", cx);
    let initial_desc = query(&mut f);
    assert_eq!(initial_desc.sort, sort(HttpHistorySortColumn::Number, false));
    f.events
        .try_send(Event::HttpHistoryPage {
            query: initial_desc,
            result: Ok(HttpHistoryResult {
                total: 3,
                rows: vec![
                    HistoryRow::Live(flow_c.summary.clone()),
                    HistoryRow::Live(flow_b.summary.clone()),
                    HistoryRow::Live(flow_a.summary.clone()),
                ],
                selected_rows: Vec::new(),
                selected_matches: Vec::new(),
                selected_ranks: Vec::new(),
                live_annotations: Vec::new(),
                live_numbers: live_numbers.clone(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("table-cell-0:0").label(), Some("3"));
            assert_eq!(window.find("table-cell-1:0").label(), Some("2"));
            assert_eq!(window.find("table-cell-2:0").label(), Some("1"));
        })
        .unwrap();

    // Click # header to toggle ascending: #1 (/a), #2 (/b), #3 (/c)
    click(f.window, ("icon-sort", 0usize), cx);
    let num_asc = query(&mut f);
    assert_eq!(num_asc.sort, sort(HttpHistorySortColumn::Number, true));
    f.events
        .try_send(Event::HttpHistoryPage {
            query: num_asc,
            result: Ok(HttpHistoryResult {
                total: 3,
                rows: vec![
                    HistoryRow::Live(flow_a.summary.clone()),
                    HistoryRow::Live(flow_b.summary.clone()),
                    HistoryRow::Live(flow_c.summary.clone()),
                ],
                selected_rows: Vec::new(),
                selected_matches: Vec::new(),
                selected_ranks: Vec::new(),
                live_annotations: Vec::new(),
                live_numbers: live_numbers.clone(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("table-cell-0:0").label(), Some("1"));
            assert_eq!(window.find("table-cell-1:0").label(), Some("2"));
            assert_eq!(window.find("table-cell-2:0").label(), Some("3"));
        })
        .unwrap();

    // Click Status header twice to sort ascending: #2 (/b, 200), #3 (/c, 404), #1 (/a, 500)
    click(f.window, ("col-header", 6usize), cx);
    let status_desc = query(&mut f);
    click(f.window, ("col-header", 6usize), cx);
    let status_asc = query(&mut f);
    assert!(status_asc.query_revision > status_desc.query_revision);
    assert_eq!(status_asc.sort, sort(HttpHistorySortColumn::Status, true));
    f.events
        .try_send(Event::HttpHistoryPage {
            query: status_asc,
            result: Ok(HttpHistoryResult {
                total: 3,
                rows: vec![
                    HistoryRow::Live(flow_b.summary.clone()),
                    HistoryRow::Live(flow_c.summary.clone()),
                    HistoryRow::Live(flow_a.summary.clone()),
                ],
                selected_rows: Vec::new(),
                selected_matches: Vec::new(),
                selected_ranks: Vec::new(),
                live_annotations: Vec::new(),
                live_numbers: live_numbers.clone(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("table-cell-0:0").label(), Some("2"));
            assert_eq!(window.find("table-cell-0:6").label(), Some("200"));
            assert_eq!(window.find("table-cell-1:0").label(), Some("3"));
            assert_eq!(window.find("table-cell-1:6").label(), Some("404"));
            assert_eq!(window.find("table-cell-2:0").label(), Some("1"));
            assert_eq!(window.find("table-cell-2:6").label(), Some("500"));
        })
        .unwrap();

    // Click URL header descending: #3 (/c), #2 (/b), #1 (/a)
    click(f.window, ("col-header", 3usize), cx);
    let url_desc = query(&mut f);
    assert_eq!(url_desc.sort, sort(HttpHistorySortColumn::Url, false));
    f.events
        .try_send(Event::HttpHistoryPage {
            query: url_desc,
            result: Ok(HttpHistoryResult {
                total: 3,
                rows: vec![
                    HistoryRow::Live(flow_c.summary.clone()),
                    HistoryRow::Live(flow_b.summary.clone()),
                    HistoryRow::Live(flow_a.summary.clone()),
                ],
                selected_rows: Vec::new(),
                selected_matches: Vec::new(),
                selected_ranks: Vec::new(),
                live_annotations: Vec::new(),
                live_numbers,
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("table-cell-0:0").label(), Some("3"));
            assert_eq!(window.find("table-cell-0:1").label(), Some("http://127.0.0.1:9001"));
            assert_eq!(window.find("table-cell-0:3").label(), Some("/c"));
            assert_eq!(window.find("table-cell-1:0").label(), Some("2"));
            assert_eq!(window.find("table-cell-1:3").label(), Some("/b"));
            assert_eq!(window.find("table-cell-2:0").label(), Some("1"));
            assert_eq!(window.find("table-cell-2:3").label(), Some("/a"));
        })
        .unwrap();
}

#[gpui_kit::test]
fn history_headers_show_only_active_sort_icon_and_lead_number_cell_dropdown_sets_highlight(
    cx: &mut TestAppContext,
) {
    let (mut f, _) = boot(cx);
    let source = mio_core::Uuid::new_v4();
    let item_a = mio_core::Uuid::new_v4();
    let item_b = mio_core::Uuid::new_v4();
    let make_row = |id, index, url: &str| {
        HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
            item: mio_core::ExtItemSummary {
                id,
                source_id: source,
                collection: "root.f1.f0".into(),
                index,
                object_address: None,
                state: "mapped".into(),
                message_count: 1,
            },
            request: Some(mio_core::ExtMessageSummary {
                id: mio_core::Uuid::new_v4(),
                source_id: source,
                item_id: id,
                collection: "root.f1.f0".into(),
                item_index: index,
                role: "request".into(),
                field_id: 15,
                raw_offset: 0,
                raw_bytes: 32,
                body_offset: Some(32),
                declared_version: Some(mio_core::HttpVersion::Http11),
                method: Some("POST".into()),
                request_target: Some(url.into()),
                status: None,
                diagnostic: None,
            }),
            edited_request: None,
            response: Some(mio_core::ExtMessageSummary {
                id: mio_core::Uuid::new_v4(),
                source_id: source,
                item_id: id,
                collection: "root.f1.f0".into(),
                item_index: index,
                role: "response".into(),
                field_id: 16,
                raw_offset: 32,
                raw_bytes: 1611,
                body_offset: Some(64),
                declared_version: Some(mio_core::HttpVersion::Http11),
                method: None,
                request_target: None,
                status: Some(200),
                diagnostic: None,
            }),
            edited_response: None,
            origin: Some("http://59.82.113.35".into()),
            time_unix_ms: Some(1_794_905_401_128),
            time_error: None,
            notes_revision: 0,
            notes_preview: String::new(),
            notes_error: None,
            highlight: Some(mio_core::ExtHttpHighlight {
                revision: 0,
                color: 0,
            }),
            highlight_error: None,
        }))
    };
    let rows = vec![
        make_row(item_a, 47, "/api/dispatch?key=1"),
        make_row(item_b, 46, "/api/dispatch?key=2"),
    ];
    click(f.window, "history-filter", cx);
    let refreshed = query(&mut f);
    page(&f, &refreshed, rows.clone(), 2, vec![], vec![], cx);

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("icon-sort", 0usize)).is_some());
            for col_ix in 1..18usize {
                assert!(
                    window.try_find(("icon-sort", col_ix)).is_none(),
                    "unsorted column {col_ix} must not render a placeholder sort icon"
                );
            }
            assert!(window.try_find(("history-row-highlight-combo", 0usize)).is_none());
        })
        .unwrap();

    // Click Method header directly via col-header to sort descending; sort icon moves to Method.
    click(f.window, ("col-header", 2usize), cx);
    let method_desc = query(&mut f);
    assert_eq!(method_desc.sort, sort(HttpHistorySortColumn::Method, false));
    page(&f, &method_desc, rows, 2, vec![], vec![], cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("icon-sort", 0usize)).is_none());
            assert!(window.try_find(("icon-sort", 2usize)).is_some());
            assert!(window.try_find(("icon-sort", 7usize)).is_none());
        })
        .unwrap();

    // Clicking the active sort icon directly must dispatch a single toggle to ascending without double-firing.
    click(f.window, ("icon-sort", 2usize), cx);
    let method_asc = query(&mut f);
    assert_eq!(method_asc.sort, sort(HttpHistorySortColumn::Method, true));
    page(
        &f,
        &method_asc,
        vec![
            make_row(item_a, 47, "/api/dispatch?key=1"),
            make_row(item_b, 46, "/api/dispatch?key=2"),
        ],
        2,
        vec![],
        vec![],
        cx,
    );

    // Selecting row 0 makes it the lead row and exposes the highlight dropdown in the # cell.
    click(f.window, "table-cell-0:0", cx);
    drain(&mut f.commands);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("history-row-highlight-combo", 0usize)).is_some());
            assert!(window.try_find(("history-row-highlight-combo", 1usize)).is_none());
            window.click(("history-row-highlight-combo", 0usize), cx);
        })
        .unwrap();
    cx.run_until_parked();
    click(f.window, ("http-highlight-color", 4usize), cx);
    let commands = drain(&mut f.commands);
    assert!(commands.iter().any(|cmd| matches!(
        cmd,
        Command::SetExtHttpHighlights { changes, .. }
            if changes.len() == 1
                && changes[0].source_id == source
                && changes[0].item_id == item_a
                && changes[0].color == 4
    )));
}

#[gpui_kit::test]
fn all_eighteen_headers_toggle_descending_and_ascending_sorts_keep_capture_numbers_bound_and_dispatch_by_key_after_column_changes(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::table::ColumnSort;
    use mio_core::{ExtHttpHighlight, HttpHistoryRowMetadata};

    let (mut f, _) = boot(cx);

    let mut flow_a = record("https://alpha.example:8443/app/report.html?id=1", "a");
    flow_a.summary.source = TrafficSource::Proxy;
    flow_a.summary.method = "POST".into();
    flow_a.summary.status = Some(200);
    flow_a.summary.response_bytes = 512;
    flow_a.summary.started_unix_ms = 1_700_000_001_000;
    flow_a.summary.state = FlowState::Complete;

    let mut flow_b = record("http://beta.example/api/items.json", "b");
    flow_b.summary.source = TrafficSource::Proxy;
    flow_b.summary.method = "GET".into();
    flow_b.summary.status = Some(404);
    flow_b.summary.response_bytes = 128;
    flow_b.summary.started_unix_ms = 1_700_000_002_000;
    flow_b.summary.state = FlowState::Complete;

    let id_a = HistoryEntryId::Live(flow_a.summary.id);
    let id_b = HistoryEntryId::Live(flow_b.summary.id);
    let live_numbers = vec![(flow_a.summary.id, 10), (flow_b.summary.id, 20)];
    let live_annotations = vec![
        (
            flow_a.summary.id,
            1,
            "alpha note".to_string(),
            ExtHttpHighlight {
                revision: 0,
                color: 0,
            },
        ),
        (
            flow_b.summary.id,
            1,
            "beta note".to_string(),
            ExtHttpHighlight {
                revision: 0,
                color: 0,
            },
        ),
    ];
    let row_metadata = vec![
        (
            id_a,
            HttpHistoryRowMetadata {
                has_params: true,
                params_known: true,
                params_error: None,
                edited: true,
                has_original_request: true,
                has_original_response: false,
                response_length: Some(512),
                mime_type: "html".into(),
                title: "Alpha Title".into(),
                title_error: None,
                remote_ip: "10.0.0.1".into(),
                cookies: "sid".into(),
                cookies_error: None,
                listener_port: Some(8080),
                response_received_ms: Some(35),
                time_known: true,
                capture_number: Some(10),
            },
        ),
        (
            id_b,
            HttpHistoryRowMetadata {
                has_params: false,
                params_known: true,
                params_error: None,
                edited: false,
                has_original_request: false,
                has_original_response: false,
                response_length: Some(128),
                mime_type: "JSON".into(),
                title: String::new(),
                title_error: None,
                remote_ip: "10.0.0.2".into(),
                cookies: "token".into(),
                cookies_error: None,
                listener_port: Some(9090),
                response_received_ms: Some(120),
                time_known: true,
                capture_number: Some(20),
            },
        ),
    ];

    let send_sorted_page =
        |f: &Fixture, query: &HttpHistoryQuery, rows: Vec<HistoryRow>, cx: &mut TestAppContext| {
            f.events
                .try_send(Event::HttpHistoryPage {
                    query: query.clone(),
                    result: Ok(HttpHistoryResult {
                        total: rows.len(),
                        rows,
                        selected_rows: Vec::new(),
                        selected_matches: Vec::new(),
                        selected_ranks: Vec::new(),
                        live_annotations: live_annotations.clone(),
                        live_numbers: live_numbers.clone(),
                        row_metadata: row_metadata.clone(),
                    }),
                })
                .unwrap();
            cx.run_until_parked();
        };

    let rows_ba = vec![
        HistoryRow::Live(flow_b.summary.clone()),
        HistoryRow::Live(flow_a.summary.clone()),
    ];
    let rows_ab = vec![
        HistoryRow::Live(flow_a.summary.clone()),
        HistoryRow::Live(flow_b.summary.clone()),
    ];

    click(f.window, "history-filter", cx);
    let refreshed = query(&mut f);
    send_sorted_page(&f, &refreshed, rows_ba.clone(), cx);

    let expected_columns: [(usize, &str, HttpHistorySortColumn); 18] = [
        (0, "number", HttpHistorySortColumn::Number),
        (1, "host", HttpHistorySortColumn::Host),
        (2, "method", HttpHistorySortColumn::Method),
        (3, "url", HttpHistorySortColumn::Url),
        (4, "params", HttpHistorySortColumn::Params),
        (5, "edited", HttpHistorySortColumn::Edited),
        (6, "status", HttpHistorySortColumn::Status),
        (7, "length", HttpHistorySortColumn::Length),
        (8, "mime", HttpHistorySortColumn::MimeType),
        (9, "extension", HttpHistorySortColumn::Extension),
        (10, "title", HttpHistorySortColumn::Title),
        (11, "notes", HttpHistorySortColumn::Notes),
        (12, "tls", HttpHistorySortColumn::Tls),
        (13, "ip", HttpHistorySortColumn::Ip),
        (14, "cookies", HttpHistorySortColumn::Cookies),
        (15, "time", HttpHistorySortColumn::Time),
        (16, "listener_port", HttpHistorySortColumn::ListenerPort),
        (
            17,
            "start_response_timer",
            HttpHistorySortColumn::StartResponseTimer,
        ),
    ];

    cx.read(|cx| {
        let delegate = f.view.read(cx).history.read(cx).delegate();
        assert_eq!(delegate.columns.len(), 18);
        for (col_ix, key, _) in expected_columns {
            assert_eq!(delegate.columns[col_ix].key.as_ref(), key);
            assert!(
                delegate.columns[col_ix].sort.is_some(),
                "column {key} must be sortable"
            );
        }
    });

    for (col_ix, key, sort_col) in expected_columns {
        let current_sort = cx.read(|cx| f.view.read(cx).http_filter.sort);
        let first_ascending =
            current_sort.column == sort_col && !current_sort.ascending;
        f.window
            .update(cx, |_, window, cx| {
                let history = f.view.read(cx).history.clone();
                history.update(cx, |table, cx| {
                    table.scroll_to_col(col_ix, cx);
                });
                window.render_frame(cx);
            })
            .unwrap();
        click(f.window, ("col-header", col_ix), cx);
        let first_q = query(&mut f);
        assert_eq!(
            first_q.sort,
            sort(sort_col, first_ascending),
            "first click on column {key} ({col_ix})"
        );
        send_sorted_page(
            &f,
            &first_q,
            if first_ascending {
                rows_ab.clone()
            } else {
                rows_ba.clone()
            },
            cx,
        );
        f.window
            .update(cx, |_, window, cx| {
                let history = f.view.read(cx).history.clone();
                history.update(cx, |table, cx| {
                    table.scroll_to_col(col_ix, cx);
                });
                window.render_frame(cx);
                let delegate = f.view.read(cx).history.read(cx).delegate();
                for (idx, col) in delegate.columns.iter().enumerate() {
                    let expected = if idx != col_ix {
                        ColumnSort::Default
                    } else if first_ascending {
                        ColumnSort::Ascending
                    } else {
                        ColumnSort::Descending
                    };
                    assert_eq!(col.sort, Some(expected), "column {idx} sort state after first click on {key}");
                }
                assert!(window.try_find(("icon-sort", col_ix)).is_some());
                history.update(cx, |table, cx| {
                    table.scroll_to_col(0, cx);
                });
                window.render_frame(cx);
                assert_eq!(
                    window.find("table-cell-0:0").label(),
                    Some(if first_ascending { "10" } else { "20" }),
                    "capture number must stay bound to record after first click on {key}"
                );
                assert_eq!(
                    window.find("table-cell-1:0").label(),
                    Some(if first_ascending { "20" } else { "10" }),
                    "capture number must stay bound to record after first click on {key}"
                );
                history.update(cx, |table, cx| {
                    table.scroll_to_col(col_ix, cx);
                });
                window.render_frame(cx);
            })
            .unwrap();

        let second_ascending = !first_ascending;
        click(f.window, ("col-header", col_ix), cx);
        let second_q = query(&mut f);
        assert_eq!(
            second_q.sort,
            sort(sort_col, second_ascending),
            "second click on column {key} ({col_ix})"
        );
        send_sorted_page(
            &f,
            &second_q,
            if second_ascending {
                rows_ab.clone()
            } else {
                rows_ba.clone()
            },
            cx,
        );
        f.window
            .update(cx, |_, window, cx| {
                let history = f.view.read(cx).history.clone();
                history.update(cx, |table, cx| {
                    table.scroll_to_col(col_ix, cx);
                });
                window.render_frame(cx);
                let delegate = f.view.read(cx).history.read(cx).delegate();
                assert_eq!(
                    delegate.columns[col_ix].sort,
                    Some(if second_ascending {
                        ColumnSort::Ascending
                    } else {
                        ColumnSort::Descending
                    })
                );
                assert!(window.try_find(("icon-sort", col_ix)).is_some());
                history.update(cx, |table, cx| {
                    table.scroll_to_col(0, cx);
                });
                window.render_frame(cx);
                assert_eq!(
                    window.find("table-cell-0:0").label(),
                    Some(if second_ascending { "10" } else { "20" })
                );
                assert_eq!(
                    window.find("table-cell-1:0").label(),
                    Some(if second_ascending { "20" } else { "10" })
                );
            })
            .unwrap();
    }

    // Reorder and hide columns to verify key-based sort dispatch and cell rendering.
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                let sort = view.http_filter.requested().1;
                view.history.update(cx, |table, cx| {
                    table
                        .delegate_mut()
                        .columns
                        .retain(|col| col.key.as_ref() != "host");
                    let pos = table
                        .delegate()
                        .columns
                        .iter()
                        .position(|col| col.key.as_ref() == "cookies")
                        .unwrap();
                    let cookies_col = table.delegate_mut().columns.remove(pos);
                    table.delegate_mut().columns.insert(1, cookies_col);
                    table.delegate_mut().apply_sort(sort);
                    table.refresh(cx);
                });
            });
            window.render_frame(cx);
        })
        .unwrap();

    click(f.window, ("col-header", 1usize), cx);
    let cookies_desc = query(&mut f);
    assert_eq!(
        cookies_desc.sort,
        sort(HttpHistorySortColumn::Cookies, false)
    );
    send_sorted_page(&f, &cookies_desc, rows_ba, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("icon-sort", 1usize)).is_some());
            assert_eq!(window.find("table-cell-0:0").label(), Some("20"));
            assert_eq!(window.find("table-cell-0:1").label(), Some("token"));
            assert_eq!(window.find("table-cell-1:0").label(), Some("10"));
            assert_eq!(window.find("table-cell-1:1").label(), Some("sid"));
        })
        .unwrap();
}
