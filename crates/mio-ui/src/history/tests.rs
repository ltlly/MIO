use super::*;
use gpui_kit::component::table::DataTable;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Render, ScrollDelta, TestAppContext, point, px};
use mio_core::{ExtItemSummary, Uuid};

#[test]
fn time_format_uses_local_calendar_and_floors_negative_milliseconds() {
    #[cfg(unix)]
    {
        let mut local = libc::tm {
            tm_year: 126,
            tm_mon: 10,
            tm_mday: 8,
            tm_hour: 16,
            tm_min: 25,
            tm_sec: 17,
            tm_isdst: -1,
            // The remaining fields are outputs of mktime, not a manually set timezone.
            ..unsafe { std::mem::zeroed() }
        };
        let seconds = unsafe { libc::mktime(&mut local) };
        assert_ne!(seconds, -1);
        assert_eq!(local_time(seconds * 1000 + 491), "16:25:17 8 11月 2026");
    }
    assert_eq!(local_time(-1), local_time(-1000));
    assert_eq!(local_time(0), local_time(999));
    assert_ne!(local_time(-1), local_time(0));
    for extreme in [i64::MIN, i64::MAX] {
        let displayed = local_time(extreme);
        assert!(!displayed.is_empty());
        assert_ne!(displayed, local_time(0));
    }
}

fn archived(time_unix_ms: Option<i64>, time_error: Option<&str>) -> HistoryRow {
    HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        item: ExtItemSummary {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            collection: "root.f1.f0".into(),
            index: 0,
            object_address: None,
            state: "mapped".into(),
            message_count: 0,
        },
        request: None,
        edited_request: None,
        response: None,
        edited_response: None,
        origin: None,
        time_unix_ms,
        time_error: time_error.map(str::to_owned),
        notes_revision: 0,
        notes_preview: "note remains".into(),
        notes_error: None,
        highlight: None,
        highlight_error: None,
    }))
}

struct HistoryTable(Entity<TableState<HistoryDelegate>>);

impl Render for HistoryTable {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1920.)).h(px(360.)).child(DataTable::new(&self.0))
    }
}

#[gpui_kit::test]
fn time_column_renders_values_and_explicit_errors_without_fake_epoch(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let timestamp = 1794126317491;
    let rows = vec![
        archived(Some(timestamp), None),
        archived(Some(-1), None),
        archived(Some(0), Some("源时间字段损坏")),
        archived(None, None),
        HistoryRow::Live(mio_core::FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "GET".into(),
            url: "http://127.0.0.1:9001/".into(),
            status: Some(200),
            state: mio_core::FlowState::Complete,
            request_bytes: 32,
            response_bytes: 64,
            started_unix_ms: timestamp,
            elapsed_ms: 1234,
        }),
    ];
    let (_, visual) = cx.add_window_view(|window, cx| {
        HistoryTable(cx.new(|cx| {
            TableState::new(
                HistoryDelegate {
                    rows,
                    ..Default::default()
                },
                window,
                cx,
            )
        }))
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.scroll("table", ScrollDelta::Pixels(point(px(-3000.), px(0.))), cx);
        window.render_frame(cx);
        for (index, expected) in [
            local_time(timestamp),
            local_time(-1),
            "时间读取失败".into(),
            String::new(),
            local_time(timestamp),
        ]
        .iter()
        .enumerate()
        {
            let cell = window.find(format!("table-cell-{index}:15"));
            assert!(cell.visible());
            assert_eq!(cell.label(), Some(expected.as_str()));
        }
        assert_eq!(window.find("table-cell-0:11").label(), Some("note remains"));
    });
}

#[test]
fn response_links_and_ext_xml_export_helpers_preserve_structure() {
    let raw_response = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><a href=\"/login\">Login</a><a href=\"https://cdn.example.com/app.js\">JS</a></body></html>";
    let links = Workbench::extract_response_links_for_history(
        "https://app.example.com/index.html",
        raw_response,
    );
    assert!(links.iter().any(|u| u == "https://app.example.com/login"));
    assert!(links.iter().any(|u| u == "https://cdn.example.com/app.js"));

    let xml = Workbench::build_ext_xml_for_history(
        "https://app.example.com:8443/api/items.json?page=1",
        "POST",
        Some(201),
        "12:00:00 1 1月 2026",
        b"POST /api/items.json?page=1 HTTP/1.1\r\nHost: app.example.com:8443\r\n\r\n{\"a\":1}",
        b"HTTP/1.1 201 Created\r\n\r\n{\"ok\":true}",
        "note <escaped>",
    );
    assert!(xml.contains("<items "));
    assert!(xml.contains("<item>"));
    assert!(xml.contains("<host>app.example.com</host>"));
    assert!(xml.contains("<port>8443</port>"));
    assert!(xml.contains("<protocol>https</protocol>"));
    assert!(xml.contains("<extension>json</extension>"));
    assert!(xml.contains("<comment>note &lt;escaped&gt;</comment>"));
}

#[gpui_kit::test]
fn url_column_renders_raw_path_and_query_while_preserving_host_and_full_stored_url(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let make_archived = |origin: Option<&str>, target: Option<&str>| {
        HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
            item: ExtItemSummary {
                id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                collection: "root.f1.f0".into(),
                index: 0,
                object_address: None,
                state: "mapped".into(),
                message_count: usize::from(target.is_some()),
            },
            request: target.map(|request_target| mio_core::ExtMessageSummary {
                id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                item_id: Uuid::new_v4(),
                collection: "root.f1.f0".into(),
                item_index: 0,
                role: "request".into(),
                field_id: 15,
                raw_offset: 0,
                raw_bytes: 32,
                body_offset: Some(32),
                declared_version: Some(mio_core::HttpVersion::Http11),
                method: Some("GET".into()),
                request_target: Some(request_target.into()),
                status: None,
                diagnostic: None,
            }),
            edited_request: None,
            response: None,
            edited_response: None,
            origin: origin.map(str::to_owned),
            time_unix_ms: None,
            time_error: None,
            notes_revision: 0,
            notes_preview: String::new(),
            notes_error: None,
            highlight: None,
            highlight_error: None,
        }))
    };
    let make_live = |url: &str| {
        HistoryRow::Live(mio_core::FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: url.into(),
            status: Some(200),
            state: mio_core::FlowState::Complete,
            request_bytes: 32,
            response_bytes: 64,
            started_unix_ms: 1000,
            elapsed_ms: 12,
        })
    };
    let rows = vec![
        make_live(
            "https://gateway.example.test:443/gw/demo.search.upload/1.0/?type=originaljson",
        ),
        make_live("http://127.0.0.1:9001/a/../b?first=%2F&x=%2e%2E#frag"),
        make_live("http://[::1]:8080/a//b///c?"),
        make_live("http://example.com?only_query=1"),
        make_live("https://example.com"),
        make_archived(
            Some("https://cdn.example.test:443"),
            Some("/config/10000001/1.0.0/0~1700000000000/~?v=1"),
        ),
        make_archived(
            Some("http://127.0.0.1:18765"),
            Some("http://127.0.0.1:18765/a/%2e%2E/b?x=1#section"),
        ),
        make_archived(Some("http://127.0.0.1:18765"), None),
    ];
    let (view, visual) = cx.add_window_view(|window, cx| {
        HistoryTable(cx.new(|cx| {
            TableState::new(
                HistoryDelegate {
                    rows,
                    ..Default::default()
                },
                window,
                cx,
            )
        }))
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        let expected = [
            (
                "https://gateway.example.test",
                "/gw/demo.search.upload/1.0/?type=originaljson",
            ),
            ("http://127.0.0.1:9001", "/a/../b?first=%2F&x=%2e%2E"),
            ("http://[::1]:8080", "/a//b///c?"),
            ("http://example.com", "/?only_query=1"),
            ("https://example.com", "/"),
            (
                "https://cdn.example.test",
                "/config/10000001/1.0.0/0~1700000000000/~?v=1",
            ),
            ("http://127.0.0.1:18765", "/a/%2e%2E/b?x=1"),
            ("http://127.0.0.1:18765", ""),
        ];
        for (index, (expected_host, expected_url)) in expected.iter().enumerate() {
            assert_eq!(
                window.find(format!("table-cell-{index}:1")).label(),
                Some(*expected_host)
            );
            assert_eq!(
                window.find(format!("table-cell-{index}:3")).label(),
                Some(*expected_url)
            );
        }
        assert_eq!(window.find("table-cell-0:4").label(), Some("✓"));
        assert_eq!(window.find("table-cell-0:12").label(), Some("✓"));
        assert_eq!(window.find("table-cell-1:12").label(), Some(""));
        let delegate = view.read(cx).0.read(cx).delegate();
        assert_eq!(
            delegate.rows[0].live().unwrap().url,
            "https://gateway.example.test:443/gw/demo.search.upload/1.0/?type=originaljson"
        );
    });
}

#[gpui_kit::test]
fn live_number_column_renders_fixed_capture_numbers_across_row_reordering(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let make_live = |url: &str, status: u16, started: i64| mio_core::FlowSummary {
        id: Uuid::new_v4(),
        revision: 1,
        source: TrafficSource::Proxy,
        method: "GET".into(),
        url: url.into(),
        status: Some(status),
        state: mio_core::FlowState::Complete,
        request_bytes: 16,
        response_bytes: 32,
        started_unix_ms: started,
        elapsed_ms: 10,
    };
    let a = make_live("http://127.0.0.1:9001/a", 500, 100);
    let b = make_live("http://127.0.0.1:9001/b", 200, 200);
    let c = make_live("http://127.0.0.1:9001/c", 404, 300);
    let live_numbers = HashMap::from([(a.id, 1), (b.id, 2), (c.id, 3)]);

    let (view, visual) = cx.add_window_view(|window, cx| {
        HistoryTable(cx.new(|cx| {
            TableState::new(
                HistoryDelegate {
                    rows: vec![
                        HistoryRow::Live(c.clone()),
                        HistoryRow::Live(b.clone()),
                        HistoryRow::Live(a.clone()),
                    ],
                    live_numbers,
                    ..Default::default()
                },
                window,
                cx,
            )
        }))
    });
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("table-cell-0:0").label(), Some("3"));
        assert_eq!(window.find("table-cell-1:0").label(), Some("2"));
        assert_eq!(window.find("table-cell-2:0").label(), Some("1"));

        let table = view.read(cx).0.clone();
        table.update(cx, |table, cx| {
            table.delegate_mut().rows = vec![
                HistoryRow::Live(b.clone()),
                HistoryRow::Live(c.clone()),
                HistoryRow::Live(a.clone()),
            ];
            table.refresh(cx);
        });
        window.render_frame(cx);
        assert_eq!(window.find("table-cell-0:0").label(), Some("2"));
        assert_eq!(window.find("table-cell-1:0").label(), Some("3"));
        assert_eq!(window.find("table-cell-2:0").label(), Some("1"));
    });
}

#[gpui_kit::test]
fn number_column_fits_four_digit_lead_row_with_highlight_combo_and_supports_five_digit_resize(
    cx: &mut TestAppContext,
) {
    use crate::workbench::tests::{drain, fixture};

    let mut f = fixture(cx, vec![], 64);
    let initial = drain(&mut f.commands);
    let [
        mio_core::Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = initial.as_slice()
    else {
        panic!("initial history query");
    };

    let source_id = Uuid::new_v4();
    let make_ext_row = |index: u64| {
        HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
            item: ExtItemSummary {
                id: Uuid::new_v4(),
                source_id,
                collection: "root.f1.f0".into(),
                index,
                object_address: None,
                state: "mapped".into(),
                message_count: 0,
            },
            request: None,
            edited_request: None,
            response: None,
            edited_response: None,
            origin: Some("http://203.0.113.10".into()),
            time_unix_ms: None,
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

    let row_1402 = make_ext_row(1401);
    let row_9999 = make_ext_row(9998);
    let row_10000 = make_ext_row(9999);
    f.events
        .try_send(mio_core::Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 3,
            rows: vec![row_1402, row_9999, row_10000],
        })
        .unwrap();
    cx.run_until_parked();

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-history", cx);
            window.render_frame(cx);
            window.click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);

            let cell_0 = window.find("table-cell-0:0");
            assert_eq!(cell_0.label(), Some("1402"));
            let cell_bounds = cell_0.bounds();
            assert_eq!(cell_bounds.size.width, px(45.));

            let text_0 = window.find(("history-cell-text", 0usize));
            let text_0_bounds = text_0.bounds();
            let combo = window.find(("history-row-highlight-combo", 0usize));
            assert!(combo.visible());
            let combo_bounds = combo.bounds();
            assert_eq!(combo_bounds.size.width, px(6.));
            assert_eq!(text_0_bounds.origin.x - cell_bounds.origin.x, px(5.));
            assert!(combo_bounds.right() <= cell_bounds.right());
            let text_2_unselected = window.find(("history-cell-text", 2usize << 8)).bounds();
            assert_eq!(
                text_0_bounds.origin.x, text_2_unselected.origin.x,
                "selected and unselected row numbers must start at the exact same x column"
            );
            let left_border = window.find(("history-row-lead-left-border", 0usize)).bounds();
            assert_eq!(left_border.origin.x, cell_bounds.origin.x);
            assert_eq!(left_border.size.width, px(1.));

            let font_size = crate::theme::ui_font_size();
            let run_1402 = gpui_kit::TextRun {
                len: "1402".len(),
                font: window.text_style().font(),
                color: crate::theme::foreground(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped_1402 = window
                .text_system()
                .shape_line("1402".into(), font_size, &[run_1402.clone()], None);
            let shaped_9999 = window
                .text_system()
                .shape_line("9999".into(), font_size, &[run_1402.clone()], None);
            let mut helv_font = window.text_style().font();
            helv_font.family = crate::theme::ui_font_family().into();
            let helv_run_1402 = gpui_kit::TextRun {
                font: helv_font,
                ..run_1402.clone()
            };
            let helv_1402 = window
                .text_system()
                .shape_line("1402".into(), font_size, &[helv_run_1402], None);
            let usable_before_combo = combo_bounds.left() - text_0_bounds.left();
            assert_eq!(usable_before_combo, px(34.));
            assert!(
                shaped_1402.width <= usable_before_combo
                    && shaped_9999.width <= usable_before_combo
                    && helv_1402.width <= usable_before_combo
                    && text_0_bounds.left() + helv_1402.width <= combo_bounds.left(),
                "four-digit row numbers (test_font 1402={:?}, 9999={:?}, Helvetica={:?}) at font_size {:?} must not overlap combo {:?} (usable={:?}, text_0={:?})",
                shaped_1402.width,
                shaped_9999.width,
                helv_1402.width,
                font_size,
                combo_bounds,
                usable_before_combo,
                text_0_bounds,
            );

            // Unselected 5-digit row #10000 has a 40px text container at default 45px width (5px left padding).
            let cell_2 = window.find("table-cell-2:0");
            assert_eq!(cell_2.label(), Some("10000"));
            let text_2_bounds = window.find(("history-cell-text", 2usize << 8)).bounds();
            assert_eq!(text_2_bounds.size.width, px(40.));
            let run_10000 = gpui_kit::TextRun {
                len: "10000".len(),
                ..run_1402.clone()
            };
            let shaped_10000 = window
                .text_system()
                .shape_line("10000".into(), font_size, &[run_10000], None);
            assert!(
                shaped_10000.width <= text_2_bounds.size.width,
                "five-digit unselected row number {:?} must fit inside unselected text container {:?}",
                shaped_10000.width,
                text_2_bounds
            );

            // Opening the highlight combo renders all 10 color swatches with the row number label.
            window.click(("history-row-highlight-combo", 0usize), cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let cell_bounds = window.find("table-cell-0:0").bounds();
            let menu_bounds = window.find("popup-menu").bounds();
            assert_eq!(menu_bounds.size, gpui_kit::size(px(52.), px(212.)));
            assert_eq!(menu_bounds.left(), cell_bounds.left());
            assert_eq!(
                menu_bounds.top(),
                cell_bounds.bottom(),
                "annotations swatch popup top must sit flush with the cell bottom"
            );
            assert_eq!(
                f.view.read(cx).history.read(cx).delegate().open_highlight_combo_row,
                Some(0),
                "opening the highlight combo must track the open row via on_open_change"
            );
            for color in 0..=9usize {
                let item = window.find(("http-highlight-color", color));
                assert!(item.visible());
                let bounds = item.bounds();
                assert_eq!(bounds.size, gpui_kit::size(px(50.), px(21.)));
                assert_eq!(bounds.left(), menu_bounds.left() + px(1.));
                assert_eq!(bounds.top(), menu_bounds.top() + px(1. + 21. * color as f32));
            }
            window.click(("http-highlight-color", 4usize), cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                f.view.read(cx).history.read(cx).delegate().open_highlight_combo_row,
                None,
                "closing the highlight combo must restore the cell state via on_open_change"
            );
            assert_eq!(window.find("table-cell-0:0").label(), Some("1402"));
        })
        .unwrap();

    // Select 5-digit row #10000, then drag the # column header resize handle to widen column 0 from 45px to 60px.
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("table-cell-2:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let header_0 = window.find(("col-header", 0usize)).bounds();
            let drag_start = point(header_0.right() - px(1.), header_0.center().y);
            let drag_end = point(drag_start.x + px(15.), drag_start.y);
            window.drag(drag_start, drag_end, cx);
            window.render_frame(cx);
            let widened_cell = window.find("table-cell-2:0").bounds();
            let widened_text = window.find(("history-cell-text", 2usize << 8)).bounds();
            let widened_combo = window.find(("history-row-highlight-combo", 2usize)).bounds();
            assert!(widened_cell.size.width >= px(58.));
            let widened_usable = widened_combo.left() - widened_text.left();
            let font_size = crate::theme::ui_font_size();
            let run_10000 = gpui_kit::TextRun {
                len: "10000".len(),
                font: window.text_style().font(),
                color: crate::theme::foreground(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped_10000 = window
                .text_system()
                .shape_line("10000".into(), font_size, &[run_10000], None);
            assert!(
                shaped_10000.width <= widened_usable,
                "five-digit selected row number {:?} must fit before combo (usable={:?}, widened_text={:?})",
                shaped_10000.width,
                widened_usable,
                widened_text
            );
            // Clicking another row refreshes the table; verify the widened column width persists via ColumnWidthsChanged.
            window.click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.find("table-cell-0:0").bounds().size.width >= px(58.),
                "resized # column width must persist across row selection refresh"
            );
        })
        .unwrap();
}

#[test]
fn highlight_foreground_and_sort_column_mapping_are_consistent() {
    assert_eq!(highlight_foreground(8), gpui_kit::rgb(0xffffff).into());
    assert_eq!(highlight_foreground(1), gpui_kit::rgb(0xffffff).into());
    assert_eq!(highlight_foreground(6), gpui_kit::rgb(0xffffff).into());
    for color in [0, 2, 3, 4, 5, 7, 9] {
        assert_eq!(highlight_foreground(color), gpui_kit::rgb(0x111111).into());
    }

    for (key, sort_col) in HISTORY_SORT_COLUMNS {
        assert_eq!(sort_column_key(sort_col), key);
        assert_eq!(sort_column_from_key(key), Some(sort_col));
    }
    assert_eq!(sort_column_from_key("unknown"), None);
}

#[test]
fn history_cell_handles_ext_proxy_and_live_uniformly() {
    let source_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    let ext = HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        item: ExtItemSummary {
            id: item_id,
            source_id,
            collection: "c".into(),
            index: 4,
            object_address: None,
            state: "mapped".into(),
            message_count: 2,
        },
        request: Some(mio_core::ExtMessageSummary {
            id: Uuid::new_v4(),
            source_id,
            item_id,
            collection: "c".into(),
            item_index: 4,
            role: "request".into(),
            field_id: 0,
            raw_offset: 0,
            raw_bytes: 64,
            body_offset: None,
            declared_version: None,
            method: Some("POST".into()),
            request_target: Some("/api/items?a=1".into()),
            status: None,
            diagnostic: None,
        }),
        edited_request: None,
        response: Some(mio_core::ExtMessageSummary {
            id: Uuid::new_v4(),
            source_id,
            item_id,
            collection: "c".into(),
            item_index: 4,
            role: "response".into(),
            field_id: 1,
            raw_offset: 64,
            raw_bytes: 128,
            body_offset: None,
            declared_version: None,
            method: None,
            request_target: None,
            status: Some(201),
            diagnostic: None,
        }),
        edited_response: None,
        origin: Some("https://example.invalid:443".into()),
        time_unix_ms: Some(0),
        time_error: None,
        notes_revision: 1,
        notes_preview: "line1\r\nline2".into(),
        notes_error: None,
        highlight: None,
        highlight_error: None,
    }));
    let live_id = Uuid::new_v4();
    let live = HistoryRow::Live(mio_core::FlowSummary {
        id: live_id,
        revision: 1,
        source: TrafficSource::Proxy,
        method: "POST".into(),
        url: "https://example.invalid:443/api/items?a=1".into(),
        status: Some(201),
        state: mio_core::FlowState::Complete,
        request_bytes: 64,
        response_bytes: 128,
        started_unix_ms: 0,
        elapsed_ms: 10,
    });
    let mut delegate = HistoryDelegate {
        rows: vec![ext, live],
        ..Default::default()
    };
    delegate.live_numbers.insert(live_id, 5);
    delegate
        .live_notes
        .borrow_mut()
        .insert(live_id, (1, "line1\r\nline2".into()));

    for col in 0..delegate.columns.len() {
        assert_eq!(
            delegate.cell(0, col),
            delegate.cell(1, col),
            "column {} ({}) should match across ExtProxy and Live",
            col,
            delegate.columns[col].key
        );
    }
}
