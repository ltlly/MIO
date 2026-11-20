use super::*;

#[gpui_kit::test]
fn websocket_return_to_proxy_refreshes_hidden_changes_once(cx: &mut TestAppContext) {
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
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-repeater", cx);
    })
    .unwrap();
    drain(&mut fixture.commands);
    for _ in 0..3 {
        fixture
            .events
            .try_send(Event::WebSocketHistoryChanged {
                project: fixture.project,
            })
            .unwrap();
    }
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-proxy", cx);
        window.click("tool-proxy", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryWebSocketHistory {
            query_revision,
            flow_id,
            offset,
            limit,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("returning to Proxy must coalesce hidden changes into exactly one query");
    };
    assert!(*query_revision > initial);
    assert_eq!(
        (*flow_id, *offset, *limit),
        (None, 0, WS_HISTORY_BATCH_SIZE)
    );
}

#[gpui_kit::test]
fn websocket_row_click_reads_only_latest_binary_payload_and_keeps_it_readonly(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let revision = open(&mut fixture, cx);
    let (first, first_record) = message(Uuid::new_v4(), 1, b"first", WebSocketKind::Text);
    let (second, second_record) = message(Uuid::new_v4(), 1, &[0, 255, 1], WebSocketKind::Binary);
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: revision,
            flow_id: None,
            offset: 0,
            total: 2,
            rows: vec![first.clone(), second.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "listing summaries must not read payloads"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let table = fixture.view.read(cx).websocket.table.read(cx);
        let delegate = table.delegate();
        assert_eq!(delegate.columns_count(cx), 10);
        assert_eq!(delegate.cell(0, 0), first.ordinal.to_string());
        assert_eq!(delegate.cell(0, 3), "");
        assert_eq!(delegate.cell(0, 5), "");
        assert_eq!(delegate.cell(0, 6), "");
        assert_eq!(delegate.cell(0, 8), "9001");
        assert_eq!(delegate.cell(0, 9), first.connection_id.to_string());
        let row_height = window.find(("websocket-row", 0_usize)).bounds().size.height;
        let padding = delegate.column(0, cx).paddings.unwrap();
        assert_eq!(row_height, px(19.));
        assert!(
            row_height - padding.top - padding.bottom - px(1.) >= px(18.),
            "compact rows must leave room for the full text line after the row border"
        );
        assert!(window.try_find("websocket-next").is_none());
        assert!(window.try_find("websocket-connection").is_none());
        assert!(window.try_find("websocket-refresh").is_none());
    })
    .unwrap();
    click_row(&fixture, 0, cx);
    let first_read = read(&mut fixture);
    assert_eq!(first_read.1, first.id);
    click_row(&fixture, 1, cx);
    let second_read = read(&mut fixture);
    assert_eq!(second_read.1, second.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: second_read.0,
            record: Box::new(second_record.clone()),
            original_payload: None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: first_read.0,
            record: Box::new(first_record),
            original_payload: None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketDetailError {
            project: fixture.project,
            selection_revision: first_read.0,
            message_id: first.id,
            message: "late first failure".into(),
        })
        .unwrap();
    let mut old_project_record = second_record.clone();
    old_project_record.payload = b"another project".to_vec();
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: ProjectKey {
                generation: 0,
                ..fixture.project
            },
            selection_revision: second_read.0,
            record: Box::new(old_project_record),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.detail.as_deref(), Some(&second_record));
        assert!(state.detail_error.is_none());
        assert_eq!(state.payload.view, MessageView::Hex);
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let editor = fixture.view.read(cx).websocket.payload.hex.clone();
        assert_eq!(editor.read(cx).value().as_ref(), "00 ff 01");
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("00", cx);
        assert_eq!(editor.read(cx).value().as_ref(), "00 ff 01");
        window.click("websocket-payload-raw", cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.websocket.payload.view, MessageView::Hex);
        assert!(view.error.as_ref().unwrap().contains("非 UTF-8"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn websocket_refresh_filter_and_paging_keep_stable_message_identity(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    let flow = Uuid::new_v4();
    let (selected, selected_record) = message(flow, 1, "选中🙂".as_bytes(), WebSocketKind::Text);
    let (inserted, _) = message(Uuid::new_v4(), 1, b"new", WebSocketKind::Text);
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: 1,
            rows: vec![selected.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&fixture, 0, cx);
    let selected_read = read(&mut fixture);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: selected_read.0,
            record: Box::new(selected_record.clone()),
            original_payload: None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketHistoryChanged {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    let refresh = query(&mut fixture);
    assert!(refresh.0 > initial);
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: refresh.0,
            flow_id: None,
            offset: 0,
            total: 2,
            rows: vec![inserted.clone(), selected.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.table.read(cx).selected_row(), Some(1));
        assert_eq!(state.selected, Some(selected.id));
        assert_eq!(state.detail.as_deref(), Some(&selected_record));
        assert_eq!(state.payload.raw.read(cx).value().as_ref(), "选中🙂");
    });
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        fixture.view.update(cx, |view, cx| {
            view.query_websocket_history(Some(flow), 0, window, cx);
        });
    })
    .unwrap();
    let filtered = query(&mut fixture);
    assert_eq!(
        (filtered.1, filtered.2, filtered.3),
        (Some(flow), 0, WS_HISTORY_BATCH_SIZE)
    );
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert!(state.selected.is_none());
        assert!(state.detail.is_none());
        assert!(state.table.read(cx).delegate().rows.is_empty());
    });
    fixture
        .events
        .try_send(Event::WebSocketHistoryError {
            project: fixture.project,
            query_revision: refresh.0,
            message: "old global query".into(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: refresh.0,
            flow_id: None,
            offset: 0,
            total: 1,
            rows: vec![inserted],
        })
        .unwrap();
    let rows: Vec<_> = (0..PAGE_SIZE)
        .map(|index| message(flow, index as u64 + 1, b"page", WebSocketKind::Text).0)
        .collect();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: filtered.0,
            flow_id: Some(flow),
            offset: 0,
            total: PAGE_SIZE + 1,
            rows,
        })
        .unwrap();
    cx.run_until_parked();
    scroll(&fixture, -10000., cx);
    scroll(&fixture, -10000., cx);
    let next_commands = drain(&mut fixture.commands);
    let [
        Command::QueryWebSocketHistory {
            query_revision: next,
            flow_id,
            offset,
            limit,
            ..
        },
    ] = next_commands.as_slice()
    else {
        panic!("repeated scrolling while loading must issue exactly one page query");
    };
    assert_eq!((*flow_id, *limit), (Some(flow), WS_HISTORY_BATCH_SIZE));
    assert!((1..=PAGE_SIZE).contains(offset));
    let last_rows: Vec<_> = (*offset..=PAGE_SIZE)
        .map(|index| message(flow, index as u64 + 1, b"last", WebSocketKind::Text).0)
        .collect();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: *next,
            flow_id: Some(flow),
            offset: *offset,
            total: PAGE_SIZE + 1,
            rows: last_rows.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let table = fixture.view.read(cx).websocket.table.read(cx);
        assert_eq!(table.delegate().rows, last_rows);
        assert!(table.delegate().error.is_none());
    });
    scroll(&fixture, 10000., cx);
    let previous = query(&mut fixture);
    assert_eq!(
        (previous.1, previous.2, previous.3),
        (Some(flow), 0, WS_HISTORY_BATCH_SIZE)
    );
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.query_websocket_history(None, 0, window, cx);
        })
    })
    .unwrap();
    let all = query(&mut fixture);
    assert!(all.0 > previous.0);
    assert_eq!((all.1, all.2), (None, 0));
}
