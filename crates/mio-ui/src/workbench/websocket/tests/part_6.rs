use crate::gpui::test::TestEventExt;
use gpui_kit::MouseButton;
use mio_core::WebSocketRepeaterCommand;

use super::*;

#[gpui_kit::test]
fn archived_ext_websocket_row_switches_versions_and_transfers_to_repeater_decoder_and_comparer(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);

    // Archived 外部归档 WebSocket row uses Uuid::nil() flow_id.
    let (mut archived_summary, mut archived_record) = message(
        Uuid::nil(),
        1,
        b"edited-ext-ws-payload",
        WebSocketKind::Text,
    );
    archived_summary.url = "wss://ws.archive.example/feed".into();
    archived_summary.edited = true;
    archived_summary.notes_preview = "ext-ws-note".into();
    archived_record.direction = WebSocketDirection::ServerToClient;
    archived_summary.direction = WebSocketDirection::ServerToClient;

    let (second_summary, second_record) =
        message(Uuid::nil(), 2, b"second-ext-ws-frame", WebSocketKind::Text);

    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: 2,
            rows: vec![archived_summary.clone(), second_summary.clone()],
        })
        .unwrap();
    cx.run_until_parked();

    click_row(&fixture, 0, cx);
    let (sel_rev, msg_id) = read(&mut fixture);
    assert_eq!(msg_id, archived_record.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: sel_rev,
            record: Box::new(archived_record.clone()),
            original_payload: Some(b"orig-ext-ws-payload".to_vec()),
        })
        .unwrap();
    cx.run_until_parked();

    // Switch to original version and send archived 外部归档 WebSocket frame to Repeater.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("websocket-original", cx);
        window.click("websocket-to-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let open_cmd = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::WebSocketRepeater(cmd) => Some(cmd),
            _ => None,
        })
        .expect("sending archived 外部归档 WebSocket row to Repeater must emit Open");
    match open_cmd {
        WebSocketRepeaterCommand::Open {
            project,
            url,
            source_flow_id,
            source_message_id,
            source_direction,
            source_kind,
            source_payload,
            ..
        } => {
            assert_eq!(project, fixture.project);
            assert_eq!(url, "wss://ws.archive.example/feed");
            assert!(source_flow_id.is_none());
            assert_eq!(source_message_id, Some(archived_record.id));
            assert_eq!(source_direction, WebSocketDirection::ServerToClient);
            assert_eq!(source_kind, WebSocketKind::Text);
            assert_eq!(source_payload, b"orig-ext-ws-payload");
        }
        other => panic!("unexpected command for archived 外部归档 WebSocket: {other:?}"),
    }

    // Right-click table row 0 -> send to Decoder.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-0:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let menu = window.within("popup-menu");
        let _ = menu.find_by_label("发送到 Repeater");
        let _ = menu.find_by_label("发送到 Comparer");
        let center = menu.find_by_label("发送到 Decoder").bounds().center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.tool, Tool::Decoder);
        assert_eq!(
            view.extra_tools.decoder_input.read(cx).value().as_ref(),
            "orig-ext-ws-payload"
        );
        window.click("tool-proxy", cx);
    })
    .unwrap();
    cx.run_until_parked();

    // Right-click row 0 -> send to Comparer, then right-click unselected row 1 -> send to Comparer.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-0:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let center = window
            .within("popup-menu")
            .find_by_label("发送到 Comparer")
            .bounds()
            .center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(fixture.view.read(cx).tool, Tool::Comparer);
        assert_eq!(fixture.view.read(cx).extra_tools.comparer_items.len(), 1);
        window.click("tool-proxy", cx);
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-1:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let center = window
            .within("popup-menu")
            .find_by_label("发送到 Comparer")
            .bounds()
            .center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let (sel_rev_2, msg_id_2) = read(&mut fixture);
    assert_eq!(msg_id_2, second_record.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: sel_rev_2,
            record: Box::new(second_record),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.tool, Tool::Comparer);
        assert_eq!(view.extra_tools.comparer_items.len(), 2);
        assert!(
            view.extra_tools
                .comparer_view
                .read(cx)
                .value()
                .contains("Modified: 1")
        );
    });
}

#[gpui_kit::test]
fn websocket_column_sorting_menus_footer_and_request_context_actions_work_end_to_end(
    cx: &mut TestAppContext,
) {
    let initial_tab = tab("1");
    let tab_id = initial_tab.id;
    let mut fixture = fixture(cx, vec![initial_tab], 64);
    let initial = open(&mut fixture, cx);

    let (mut row_a, record_a) = message(
        Uuid::nil(),
        1,
        b"{\"z\":1,\"a\":[1,2]}",
        WebSocketKind::Text,
    );
    row_a.direction = WebSocketDirection::ClientToServer;
    row_a.observed_unix_ms = 3_000;
    row_a.listener_port = Some(9002);

    let (mut row_b, _) = message(
        Uuid::nil(),
        2,
        b"very-long-websocket-payload-frame",
        WebSocketKind::Text,
    );
    row_b.direction = WebSocketDirection::ServerToClient;
    row_b.observed_unix_ms = 1_000;
    row_b.listener_port = Some(9001);

    let (mut row_c, _) = message(
        Uuid::nil(),
        3,
        b"{\"z\":1,\"a\":[1,2]}",
        WebSocketKind::Text,
    );
    row_c.direction = WebSocketDirection::ClientToServer;
    row_c.observed_unix_ms = 2_000;
    row_c.listener_port = Some(9003);

    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: 3,
            rows: vec![row_a.clone(), row_b.clone(), row_c.clone()],
        })
        .unwrap();
    cx.run_until_parked();

    // Select row 0 (ordinal 1) and load JSON WebSocket detail.
    click_row(&fixture, 0, cx);
    let (sel_rev, msg_id) = read(&mut fixture);
    assert_eq!(msg_id, record_a.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: sel_rev,
            record: Box::new(record_a),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();

    // Sort by Length (col 4): first click -> Descending (row_b first, tie-break ordinal 1 before 3).
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("footer-resource-usage", cx);
        window.render_frame(cx);
        assert_eq!(window.find("footer-project-name").label(), Some("临时项目"));
        let menu_names: Vec<_> = cx
            .get_menus()
            .expect("menus installed")
            .into_iter()
            .map(|m| m.name.to_string())
            .collect();
        assert_eq!(
            menu_names,
            vec![
                "MIO", "项目", "编辑", "视图", "Target", "Proxy", "Intruder", "Repeater",
                "窗口", "帮助",
            ]
        );
        assert!(window.try_find(("icon-sort", 0_usize)).is_some());
        assert!(window.try_find(("icon-sort", 4_usize)).is_none());
        assert!(window.try_find(("icon-sort", 2_usize)).is_none());
        window.click(("col-header", 4_usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let table = fixture.view.read(cx).websocket.table.read(cx);
        let ordinals: Vec<u64> = table.delegate().rows.iter().map(|r| r.ordinal).collect();
        assert_eq!(ordinals, vec![2, 1, 3]);
        assert_eq!(table.selected_row(), Some(1));
        assert!(window.try_find(("icon-sort", 0_usize)).is_none());
        assert!(window.try_find(("icon-sort", 4_usize)).is_some());
        // Second click on Length -> Ascending (tie-break ordinal 1 before 3, then 2).
        window.click(("icon-sort", 4_usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let ordinals: Vec<u64> = fixture
            .view
            .read(cx)
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|r| r.ordinal)
            .collect();
        assert_eq!(ordinals, vec![1, 3, 2]);
        // Sort by Direction (col 2): first click -> Descending ("← 客户端" > "→ 服务器" in UTF-8? Check exact ordering via second click Ascending).
        window.click(("col-header", 2_usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("icon-sort", 2_usize)).is_some());
        window.click(("icon-sort", 2_usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let directions: Vec<WebSocketDirection> = fixture
            .view
            .read(cx)
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|r| r.direction)
            .collect();
        // Ascending: '←' (U+2190) < '→' (U+2192), so ServerToClient comes before ClientToServer (with tie-break 1 before 3).
        assert_eq!(
            directions,
            vec![
                WebSocketDirection::ServerToClient,
                WebSocketDirection::ClientToServer,
                WebSocketDirection::ClientToServer,
            ]
        );
        let payload_editor = fixture.view.read(cx).websocket.payload.state().clone();
        payload_editor.update(cx, |state, cx| state.focus(window, cx));
        assert!(payload_editor.focus_handle(cx).is_focused(window));
        window.press(crate::kbd!("alt-c"), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("prettified JSON copied");
        assert!(copied.contains("\n  \"z\": 1"));
    });

    // Switch to Repeater via View menu action, toggle orientation, paste URL as request, and toggle GET/POST.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(SelectToolRepeater), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let req_editor = fixture.view.read(cx).tab_editors[&tab_id]
            .request
            .raw
            .clone();
        req_editor.update(cx, |state, cx| state.focus(window, cx));
        window.dispatch_action(Box::new(ToggleTabOrientation), cx);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "https://api.example.com:8443/v1/search?q=mio&page=2#frag".into(),
        ));
        window.dispatch_action(Box::new(PasteUrlAsRequest), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.tool, Tool::Repeater);
        assert_eq!(view.workspace.orientation, Orientation::Vertical);
        let tab = view.workspace.tabs.iter().find(|t| t.id == tab_id).unwrap();
        assert_eq!(tab.draft.target, "https://api.example.com:8443");
        assert_eq!(
            String::from_utf8_lossy(&tab.draft.raw),
            "GET /v1/search?q=mio&page=2 HTTP/1.1\r\nHost: api.example.com:8443\r\n\r\n"
        );
        window.dispatch_action(Box::new(ChangeRequestMethod), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        let tab = view.workspace.tabs.iter().find(|t| t.id == tab_id).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&tab.draft.raw),
            "POST /v1/search HTTP/1.1\r\nHost: api.example.com:8443\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 12\r\n\r\nq=mio&page=2"
        );
        window.dispatch_action(Box::new(ChangeRequestMethod), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let tab = view.workspace.tabs.iter().find(|t| t.id == tab_id).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&tab.draft.raw),
            "GET /v1/search?q=mio&page=2 HTTP/1.1\r\nHost: api.example.com:8443\r\n\r\n"
        );
    });
}

#[gpui_kit::test]
fn websocket_continuous_batch_sorts_across_over_two_hundred_frames_and_preserves_fixed_ordinals(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    let flow = Uuid::new_v4();
    let count = 260_usize;
    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let ordinal = (index + 1) as u64;
        let payload_len = match ordinal {
            1 => 300,
            120 => 10,
            260 => 900,
            _ => 50 + (index % 20),
        };
        let url = match ordinal {
            1 => "wss://m.example.com/ws".to_owned(),
            120 => "wss://z.example.com/ws".to_owned(),
            260 => "wss://a.example.com/ws".to_owned(),
            _ => format!("wss://h{:03}.example.com/ws", index),
        };
        let payload = vec![b'x'; payload_len];
        let (mut summary, _) = message(flow, ordinal, &payload, WebSocketKind::Text);
        summary.ordinal = ordinal;
        summary.url = url;
        rows.push(summary);
    }
    let selected_target = rows[259].clone();

    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: count,
            rows: rows.clone(),
        })
        .unwrap();
    cx.run_until_parked();

    let win = fixture.window;
    let view = fixture.view.clone();
    let click_sort = |col: usize, cx: &mut TestAppContext| {
        cx.update_window(win, |_, window, cx| {
            window.render_frame(cx);
            window.click(("col-header", col), cx);
        })
        .unwrap();
        cx.run_until_parked();
    };

    let assert_number_cells_match_ordinals = |cx: &mut TestAppContext| {
        cx.update_window(win, |_, window, cx| {
            window.render_frame(cx);
            let table = view.read(cx).websocket.table.read(cx);
            let delegate = table.delegate();
            assert_eq!(delegate.rows.len(), count);
            for idx in 0..count {
                assert_eq!(delegate.cell(idx, 0), delegate.rows[idx].ordinal.to_string());
            }
            let visible_row = table.selected_row().unwrap_or(0);
            let expected_label = delegate.rows[visible_row].ordinal.to_string();
            assert_eq!(
                window
                    .find(SharedString::from(format!("table-cell-{visible_row}:0")))
                    .label(),
                Some(expected_label.as_str())
            );
        })
        .unwrap();
    };

    // 初始按 `#` 升序：第 0 行为 1，最后一行为 260
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let delegate = fixture.view.read(cx).websocket.table.read(cx).delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 1);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 260);
    });

    // 按长度（第 4 列）降序：260（长度 900）排在首位，120（长度 10）排在末尾，# 列保持各自原始编号
    click_sort(4, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let delegate = fixture.view.read(cx).websocket.table.read(cx).delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 260);
        assert_eq!(delegate.rows[1].ordinal, 1);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 120);
    });

    // 选中排在第 0 行的第 260 号消息
    click_row(&fixture, 0, cx);
    let (_, read_id) = read(&mut fixture);
    assert_eq!(read_id, selected_target.id);

    // 按长度（第 4 列）升序：120（长度 10）排在首位，260（长度 900）排在末尾，选中行跟随移至末尾
    click_sort(4, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let table = fixture.view.read(cx).websocket.table.read(cx);
        let delegate = table.delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 120);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 260);
        assert_eq!(table.selected_row(), Some(count - 1));
    });

    // 按 URL（第 1 列）降序：120（wss://z...）排在首位，260（wss://a...）排在末尾
    click_sort(1, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let delegate = fixture.view.read(cx).websocket.table.read(cx).delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 120);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 260);
    });

    // 按 URL（第 1 列）升序：260（wss://a...）排在首位，120（wss://z...）排在末尾
    click_sort(1, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let table = fixture.view.read(cx).websocket.table.read(cx);
        let delegate = table.delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 260);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 120);
        assert_eq!(table.selected_row(), Some(0));
    });

    // 按 `#`（第 0 列）降序：260 排在首位，1 排在末尾
    click_sort(0, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let delegate = fixture.view.read(cx).websocket.table.read(cx).delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 260);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 1);
    });

    // 按 `#`（第 0 列）升序：1 排在首位，260 排在末尾；再次点击 `#` 切换回降序
    click_sort(0, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let table = fixture.view.read(cx).websocket.table.read(cx);
        let delegate = table.delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 1);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 260);
        assert_eq!(table.selected_row(), Some(count - 1));
    });

    click_sort(0, cx);
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let delegate = fixture.view.read(cx).websocket.table.read(cx).delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 260);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 1);
    });

    // 后台刷新到达时保持当前激活的 `#` 降序排序与固定 ordinal 显示
    changed(&fixture, cx);
    let refreshed = query(&mut fixture);
    assert_eq!((refreshed.1, refreshed.2, refreshed.3), (None, 0, WS_HISTORY_BATCH_SIZE));
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: refreshed.0,
            flow_id: None,
            offset: 0,
            total: count,
            rows,
        })
        .unwrap();
    cx.run_until_parked();
    assert_number_cells_match_ordinals(cx);
    cx.read(|cx| {
        let table = fixture.view.read(cx).websocket.table.read(cx);
        let delegate = table.delegate();
        assert_eq!(delegate.rows.first().unwrap().ordinal, 260);
        assert_eq!(delegate.rows.last().unwrap().ordinal, 1);
        assert_eq!(table.selected_row(), Some(0));
    });
}

#[gpui_kit::test]
fn websocket_empty_state_ext_filter_bar_quick_search_and_header_parity(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: 0,
            rows: vec![],
        })
        .unwrap();
    cx.run_until_parked();

    // Truly empty default state shows MIO's full-width empty state without table/editor/inspector.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("websocket-empty-state").is_some());
        assert!(window.try_find("websocket-empty-heading").is_some());
        assert_eq!(
            window.find("websocket-empty-learn-more").label(),
            Some("了解更多")
        );
        assert_eq!(
            window.find("websocket-empty-open-browser").label(),
            Some("打开浏览器")
        );
        assert!(window.try_find("websocket-filter").is_none());
        assert!(window.try_find("websocket-history-table").is_none());
        assert!(window.try_find("inspector-toggle").is_none());
        window.click("websocket-empty-learn-more", cx);
        window.render_frame(cx);
        assert!(window.try_find("websocket-empty-learn-more-content").is_some());
    })
    .unwrap();

    // Once messages arrive, switch cleanly to the populated External layout.
    changed(&fixture, cx);
    let rev = query(&mut fixture).0;
    let flow = Uuid::new_v4();
    let (mut first, _) = message(flow, 1, b"{\"hello\":\"world\"}", WebSocketKind::Text);
    first.url = "wss://chat.example.com/live".into();
    let (mut second, _) = message(flow, 2, b"ping", WebSocketKind::Text);
    second.url = "wss://metrics.example.com/stream".into();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: rev,
            flow_id: None,
            offset: 0,
            total: 2,
            rows: vec![first.clone(), second.clone()],
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("websocket-empty-state").is_none());
        assert!(window.try_find("websocket-filter").is_some());
        assert!(window.try_find("websocket-search").is_some());
        assert!(window.try_find("websocket-filter-enabled").is_some());
        assert!(window.try_find("websocket-help").is_some());
        assert!(window.try_find("websocket-options-menu").is_some());
        assert!(window.try_find("websocket-history-table").is_some());
        assert!(window.try_find("websocket-payload-pretty").is_some());
        assert!(window.try_find("websocket-payload-raw").is_some());
        assert!(window.try_find("websocket-payload-hex").is_some());
        assert!(window.try_find("websocket-payload-wrap").is_some());
        assert!(window.try_find("websocket-payload-nonprintable").is_some());
        assert!(window.try_find("websocket-payload-message-actions").is_some());
        assert!(window.try_find("websocket-to-repeater").is_some());
        assert!(window.try_find("inspector-toggle").is_some());
        assert!(window.try_find("websocket-notes-toggle").is_some());
        assert!(window.try_find("websocket-edit-notes-btn").is_none());
        assert!(window.try_find("websocket-copy-text").is_none());
        assert!(window.try_find(("icon-sort", 0_usize)).is_some());
        for col_ix in 1..10_usize {
            assert!(
                window.try_find(("icon-sort", col_ix)).is_none(),
                "unsorted column {col_ix} must not render an unsorted sort icon"
            );
        }

        // Quick search filtering keeps the table/filter bar visible even when 0 rows match.
        let search = fixture.view.read(cx).websocket.search_input.clone();
        window.click(("input", search.entity_id()), cx);
        window.input("nomatch", cx);
        window.click("websocket-search", cx);
    })
    .unwrap();
    let search_rev = query(&mut fixture).0;
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: search_rev,
            flow_id: None,
            offset: 0,
            total: 2,
            rows: vec![first.clone(), second.clone()],
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .table
                .read(cx)
                .delegate()
                .rows
                .len(),
            0
        );
        assert!(window.try_find("websocket-empty-state").is_none());
        assert!(window.try_find("websocket-filter").is_some());
        assert_eq!(window.find("websocket-search").label(), Some("清除筛选"));
        window.click("websocket-search", cx);
    })
    .unwrap();
    let cleared_rev = query(&mut fixture).0;
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: cleared_rev,
            flow_id: None,
            offset: 0,
            total: 2,
            rows: vec![first, second],
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .websocket
                .table
                .read(cx)
                .delegate()
                .rows
                .len(),
            2
        );
        assert!(fixture.view.read(cx).websocket.applied_search.is_empty());
    });
}
