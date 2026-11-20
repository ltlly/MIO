use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext, point};

use crate::workbench::tests::settings::{acknowledge, saved};
use crate::workbench::tests::{drain, fixture, record, tab};
use crate::workbench::*;

#[gpui_kit::test]
fn proxy_history_context_menu_disables_har_until_detail_ready_and_copies_har_json(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let query_revision = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .expect("initial history query");

    let mut live = record("http://127.0.0.1:9001/api?k=1&k=2", "{\"ok\":true}");
    live.summary.started_unix_ms = 1_700_000_000_250;
    live.summary.elapsed_ms = 42;
    live.response = Some(
        parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\n{\"done\":1}",
            HttpVersion::Http11,
        )
        .unwrap(),
    );

    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![live.summary.clone().into()],
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        window.click_at("table", point(px(90.), px(theme::ROW_HEIGHT * 1.5)), cx);
    })
    .unwrap();
    cx.run_until_parked();

    let read_revision = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if flow_id == live.summary.id => Some(selection_revision),
            _ => None,
        })
        .expect("selecting row triggers ReadFlow");

    // Before FlowDetail arrives, right-click menu item must be disabled.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-0:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let engage_center = window
            .within("popup-menu")
            .find_by_label("辅助测试工具 (Engagement tools)")
            .bounds()
            .center();
        window.pointer_down(engage_center, MouseButton::Left, cx);
        window.pointer_up(engage_center, MouseButton::Left, cx);
        window.render_frame(cx);
        let item = window.find_by_label("复制为 HAR (JSON)");
        assert!(item.visible());
        window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
        window.render_frame(cx);
        assert!(
            window
                .find_by_label("复制为 HAR (JSON)")
                .visible(),
            "disabled menu item must not dismiss popup menu or copy"
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();

    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: read_revision,
            record: Box::new(live.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();

    // Once FlowDetail is loaded, clicking `复制为 HAR (JSON)` writes HAR 1.2 JSON to clipboard.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-0:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let engage_center = window
            .within("popup-menu")
            .find_by_label("辅助测试工具 (Engagement tools)")
            .bounds()
            .center();
        window.pointer_down(engage_center, MouseButton::Left, cx);
        window.pointer_up(engage_center, MouseButton::Left, cx);
        window.render_frame(cx);
        let center = window
            .find_by_label("复制为 HAR (JSON)")
            .bounds()
            .center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let copied = cx
        .read(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("HAR JSON written to clipboard");
    assert!(copied.contains("\"version\": \"1.2\""));
    assert!(copied.contains("\"name\": \"MIO\""));
    assert!(copied.contains("\"time\": 42"));
    assert!(copied.contains("\"startedDateTime\": \"2023-11-14T22:13:20.250Z\""));
    assert!(copied.contains("\"text\": \"{\\\"done\\\":1}\""));
}

#[gpui_kit::test]
fn repeater_copy_selected_flow_har_exports_loaded_last_flow(cx: &mut TestAppContext) {
    let initial = tab("HAR-Repeater");
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.copy_selected_flow_har(window, cx);
        });
    })
    .unwrap();
    cx.read(|cx| {
        assert!(
            f.view
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|err| err.contains("尚无已完成"))
        );
    });

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (run_id, draft_revision) = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send { run_id, draft, .. } => Some((run_id, draft.revision)),
            _ => None,
        })
        .expect("Repeater Send command");

    let mut sent = record("http://127.0.0.1:9001/repeater?x=1", "");
    sent.summary.id = run_id;
    sent.summary.started_unix_ms = 1_700_000_001_000;
    sent.summary.elapsed_ms = 19;
    sent.response = Some(mio_core::HttpResponse {
        status: 200,
        version: HttpVersion::Http2,
        reason_phrase: None,
        headers: Vec::new(),
        body: vec![0xff, 0x00, 0xfe],
        trailers: Vec::new(),
    });
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id,
            draft_revision,
            record: Box::new(sent),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    acknowledge(&f, ws, cx);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.copy_selected_flow_har(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    let copied = cx
        .read(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("Repeater HAR JSON written to clipboard");
    assert!(copied.contains("\"httpVersion\": \"HTTP/2.0\""));
    assert!(copied.contains("\"encoding\": \"base64\""));
    assert!(copied.contains("\"text\": \"/wD+\""));
}
