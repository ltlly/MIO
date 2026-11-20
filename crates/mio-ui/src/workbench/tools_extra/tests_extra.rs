use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};
use mio_core::{HistoryEntryId, HistoryRow, TrafficSource};

use super::*;
use crate::workbench::tests::{drain, fixture};

fn open_project(f: &mut crate::workbench::tests::Fixture, cx: &mut TestAppContext) {
    let workspace = cx.read(|cx| f.view.read(cx).workspace.clone());
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);
}

#[gpui_kit::test]
fn tool_message_bytes_reads_target_sequencer_and_dashboard_editors(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    open_project(&mut f, cx);

    let raw_request = b"GET /target HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let raw_response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec();
    let mut flow = crate::workbench::tests::record("http://127.0.0.1:9001/target", "ok");
    flow.summary.url = "http://127.0.0.1:9001/target".into();
    let flow_id = flow.summary.id;

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Target;
            view.target_pane.page = target::TargetPage::SiteMap;
            view.target_pane.selected_id = Some(HistoryEntryId::Live(flow_id));
            view.target_pane.selected_row = Some(HistoryRow::Live(flow.summary.clone()));
            view.target_pane.request.load(&raw_request, window, cx);
            view.target_pane.response.load(&raw_response, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            assert_eq!(
                view.tool_message_bytes(false, window, cx).unwrap(),
                raw_request
            );
            assert_eq!(
                view.tool_message_bytes(true, window, cx).unwrap(),
                raw_response
            );
        });
    })
    .unwrap();

    let sequencer_request = b"GET /login HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n".to_vec();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Sequencer;
            view.sequencer_pane.page = sequencer::SequencerPage::Live;
            view.sequencer_pane
                .request
                .load(&sequencer_request, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            assert_eq!(
                view.tool_message_bytes(false, window, cx).unwrap(),
                sequencer_request
            );
        });
    })
    .unwrap();

    let scan_seed = b"GET /seed HTTP/1.1\r\nHost: 127.0.0.1:9002\r\n\r\n".to_vec();
    let finding_id = Uuid::new_v4();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Dashboard;
            view.audit_dashboard.new_scan_open = true;
            view.audit_dashboard
                .scan_request_editor
                .load(&scan_seed, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            assert_eq!(
                view.tool_message_bytes(false, window, cx).unwrap(),
                scan_seed
            );
            assert!(view.tool_message_bytes(true, window, cx).is_err());
        });
    })
    .unwrap();

    let issue_request = b"GET /issue HTTP/1.1\r\nHost: 127.0.0.1:9003\r\n\r\n".to_vec();
    let issue_response = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.audit_dashboard.new_scan_open = false;
            view.audit_dashboard.selected_finding_id = Some(finding_id);
            view.audit_dashboard
                .issue_request_editor
                .load(&issue_request, window, cx);
            view.audit_dashboard
                .issue_response_editor
                .load(&issue_response, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            assert_eq!(
                view.tool_message_bytes(false, window, cx).unwrap(),
                issue_request
            );
            assert_eq!(
                view.tool_message_bytes(true, window, cx).unwrap(),
                issue_response
            );
        });
    })
    .unwrap();

    flow.summary.source = TrafficSource::Proxy;
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Logger;
            view.record_logger_finished(&flow, window, cx);
            view.select_logger_entry(flow_id, window, cx);
            view.extra_tools.logger_loaded_detail = Some((flow.clone(), Default::default()));
            view.extra_tools
                .logger_request
                .load(&raw_request, window, cx);
        });
        window.render_frame(cx);
        window.right_click(("logger-row", 0_usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("复制 URL").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("http://127.0.0.1:9001/target".into())
        );
    });
}

#[gpui_kit::test]
fn comparer_word_diff_side_by_side_and_remove_selected_work(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    open_project(&mut f, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-comparer", cx);
        window.render_frame(cx);

        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "GET /api/v1/user?id=100&role=guest HTTP/1.1\r\nHost: example.com\r\n".into(),
        ));
        window.click("comparer-paste-button", cx);

        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "GET /api/v1/user?id=200&role=admin HTTP/1.1\r\nHost: example.com\r\n".into(),
        ));
        window.click("comparer-paste-button", cx);
        window.render_frame(cx);

        let unified = f.view.read(cx).extra_tools.comparer_view.read(cx).value();
        assert!(
            unified.contains("Modified: 2  Added: 0  Deleted: 0"),
            "expected true word-level diff with 2 modified words, got:\n{unified}"
        );
        assert!(unified.contains("- 100\n+ 200"));
        assert!(unified.contains("- guest\n+ admin"));

        window.click("comparer-view-side-by-side", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).extra_tools.comparer_side_by_side);
        assert!(window.find("comparer-side-by-side").visible());
        let left_text = f
            .view
            .read(cx)
            .extra_tools
            .comparer_left_view
            .read(cx)
            .value();
        let right_text = f
            .view
            .read(cx)
            .extra_tools
            .comparer_right_view
            .read(cx)
            .value();
        assert!(
            left_text.contains("#A")
                && left_text.contains("[-100]")
                && left_text.contains("[-guest]")
        );
        assert!(
            right_text.contains("#B")
                && right_text.contains("[+200]")
                && right_text.contains("[+admin]")
        );

        window.click("comparer-view-unified", cx);
        window.render_frame(cx);
        assert!(!f.view.read(cx).extra_tools.comparer_side_by_side);

        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "third-extra-item".into(),
        ));
        window.click("comparer-paste-button", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).extra_tools.comparer_items.len(), 3);

        window.click(("comparer-item-row", 2_usize), cx);
        window.render_frame(cx);
        window.click("comparer-remove-selected", cx);
        window.render_frame(cx);
        let view = f.view.read(cx);
        assert_eq!(view.extra_tools.comparer_items.len(), 2);
        assert_eq!(
            (view.extra_tools.left_index, view.extra_tools.right_index),
            (0, 1)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn logger_capture_toggle_filter_refresh_sort_and_csv_export_work(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    open_project(&mut f, cx);

    let mut f1 = crate::workbench::tests::record("http://127.0.0.1:9001/b-path,comma", "ok");
    f1.summary.url = "http://127.0.0.1:9001/b-path,comma".into();
    f1.summary.method = "POST".into();
    f1.summary.source = TrafficSource::Proxy;
    f1.summary.status = Some(404);
    f1.summary.response_bytes = 50;

    let mut f2 = crate::workbench::tests::record("http://127.0.0.1:9001/a-path", "ok");
    f2.summary.url = "http://127.0.0.1:9001/a-path".into();
    f2.summary.method = "GET".into();
    f2.summary.source = TrafficSource::Repeater;
    f2.summary.status = Some(200);
    f2.summary.response_bytes = 300;

    let mut ignored = crate::workbench::tests::record("http://127.0.0.1:9001/paused", "ok");
    ignored.summary.url = "http://127.0.0.1:9001/paused".into();

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-logger", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).extra_tools.capture_enabled);
        assert!(window.find("logger-vertical-scrollbar").visible());
        assert!(window.find("logger-horizontal-scrollbar").visible());
        assert!(window.find("logger-empty-detail").visible());

        f.view.update(cx, |view, _| {
            view.record_logger_flow(&f1.summary);
            view.record_logger_flow(&f2.summary);
        });
        assert_eq!(f.view.read(cx).extra_tools.logger_entries.len(), 2);

        window.click("logger-capture-toggle", cx);
        window.render_frame(cx);
        assert!(!f.view.read(cx).extra_tools.capture_enabled);
        f.view.update(cx, |view, cx| {
            view.record_logger_flow(&ignored.summary);
            view.record_logger_finished(&ignored, window, cx);
        });
        assert_eq!(f.view.read(cx).extra_tools.logger_entries.len(), 2);

        window.click("logger-capture-toggle", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).extra_tools.capture_enabled);

        // Open view filter popover and select Repeater source -> triggers QueryLoggerHistory
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-source-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::QueryLoggerHistory {
            source: Some(TrafficSource::Repeater),
            ..
        }
    )));

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        if window.try_find("logger-source-all").is_none() {
            window.click("logger-view-filter-bar", cx);
            window.render_frame(cx);
        }
        window.click("logger-source-all", cx);
        window.render_frame(cx);
        // Close popover before clicking table header sort
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        // Sort by status ascending (200 first, 404 second)
        window.click("logger-sort-status", cx);
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).extra_tools.logger_sort,
            LoggerSort {
                column: LoggerSortColumn::Status,
                ascending: true,
            }
        );
        window.click("logger-options-menu", cx);
        window.render_frame(cx);
        let export_pos = window.find_by_label("导出 CSV").bounds().center();
        window.pointer_down(export_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(export_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let all_cmds = drain(&mut f.commands);
    assert!(
        all_cmds
            .iter()
            .any(|cmd| matches!(cmd, Command::QueryLoggerHistory { source: None, .. }))
    );
    cx.read(|cx| {
        let csv = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("CSV exported to clipboard");
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "编号,来源,方法,URL,状态码,长度,耗时");
        assert!(lines[1].starts_with("2,Repeater,GET,http://127.0.0.1:9001/a-path,200,300,"));
        assert!(
            lines[2].starts_with("1,Proxy,POST,\"http://127.0.0.1:9001/b-path,comma\",404,50,")
        );
    });
}

#[gpui_kit::test]
fn comparer_diff_navigation_and_logger_limit_har_export_work(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![crate::workbench::tests::tab("Extra增强")], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-comparer", cx);
        window.render_frame(cx);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("one two three".into()));
        window.click("comparer-paste-button", cx);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("one four five".into()));
        window.click("comparer-paste-button", cx);
        window.render_frame(cx);

        window.click("comparer-next-diff", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).extra_tools.comparer_diff_cursor, 1);
        assert!(!f.view.read(cx).extra_tools.comparer_view.read(cx).selected_range().is_empty());

        window.click("comparer-view-side-by-side", cx);
        window.render_frame(cx);
        window.click("comparer-next-diff", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).extra_tools.comparer_diff_cursor, 1);

        window.click("tool-logger", cx);
        window.render_frame(cx);
        window.click("logger-capture-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-max-2000", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).extra_tools.logger_max_entries, 2000);

        let mut flow = crate::workbench::tests::record("http://127.0.0.1:9001/har-log", "payload");
        flow.request.url = "http://127.0.0.1:9001/har-log".into();
        f.view.update(cx, |view, cx| {
            view.record_logger_finished(&flow, window, cx);
            view.extra_tools.logger_selected = Some(flow.summary.id);
            view.extra_tools.logger_loaded_detail = Some((flow, Default::default()));
            view.copy_selected_flow_har(window, cx);
        });
    })
    .unwrap();
    cx.read(|cx| {
        let har = cx.read_from_clipboard().and_then(|i| i.text()).expect("HAR JSON");
        assert!(har.contains("http://127.0.0.1:9001/har-log"));
    });
}

#[gpui_kit::test]
fn logger_status_classes_in_scope_and_script_expression_filtering_work(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    open_project(&mut f, cx);

    let mut post_200 = crate::workbench::tests::record("http://127.0.0.1:9001/api/login", "ok");
    post_200.summary.url = "http://127.0.0.1:9001/api/login".into();
    post_200.summary.method = "POST".into();
    post_200.summary.status = Some(200);

    let mut get_404 = crate::workbench::tests::record("http://127.0.0.1:9002/missing", "err");
    get_404.summary.url = "http://127.0.0.1:9002/missing".into();
    get_404.summary.method = "GET".into();
    get_404.summary.status = Some(404);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-logger", cx);
        f.view.update(cx, |view, cx| {
            view.workspace.scope.include.push(mio_core::ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix: "http://127.0.0.1:9001".into(),
                include_subdomains: false,
            });
            view.record_logger_flow(&post_200.summary);
            view.record_logger_flow(&get_404.summary);
            view.extra_tools
                .logger_search
                .update(cx, |input, cx| input.set_value("status == 200 && method == \"POST\"", window, cx));
        });
        window.render_frame(cx);
        window.click("logger-options-menu", cx);
        window.render_frame(cx);
        let export_pos = window.find_by_label("导出 CSV").bounds().center();
        window.pointer_down(export_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(export_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.read(|cx| {
        let csv = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("http://127.0.0.1:9001/api/login"));
    });

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.extra_tools
                .logger_search
                .update(cx, |input, cx| input.set_value("", window, cx));
        });
        window.render_frame(cx);
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-status-2xx", cx);
        window.render_frame(cx);
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-options-menu", cx);
        window.render_frame(cx);
        let export_pos = window.find_by_label("导出 CSV").bounds().center();
        window.pointer_down(export_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(export_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.read(|cx| {
        let csv = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("http://127.0.0.1:9002/missing"));
    });

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-status-2xx", cx);
        window.click("logger-in-scope-only", cx);
        window.render_frame(cx);
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-options-menu", cx);
        window.render_frame(cx);
        let export_pos = window.find_by_label("导出 CSV").bounds().center();
        window.pointer_down(export_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(export_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.read(|cx| {
        let csv = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("http://127.0.0.1:9001/api/login"));
    });
}
