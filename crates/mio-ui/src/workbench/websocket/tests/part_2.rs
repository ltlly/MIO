use super::*;

#[gpui_kit::test]
fn websocket_pending_keyboard_selection_finishes_after_scrolling_back(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    let flow = Uuid::new_v4();
    let records: Vec<_> = (0..PAGE_SIZE * 3)
        .map(|index| {
            message(
                flow,
                index as u64 + 1,
                b"selected last",
                WebSocketKind::Text,
            )
        })
        .collect();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: records.len(),
            rows: records[..PAGE_SIZE]
                .iter()
                .map(|(row, _)| row.clone())
                .collect(),
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&fixture, 0, cx);
    let first_read = read(&mut fixture);
    scroll(&fixture, -4500., cx);
    let middle = query(&mut fixture);
    assert!((PAGE_SIZE..PAGE_SIZE * 2).contains(&middle.2));
    cx.update_window(fixture.window, |_, window, cx| {
        window.press("end", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    scroll(&fixture, 20000., cx);
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.table.read(cx).selected_row(), Some(records.len() - 1));
        assert_eq!(state.pending_row, Some(records.len() - 1));
        assert_eq!(state.table.read(cx).delegate().visible.start, 0);
        assert!(state.detail.is_none());
    });
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "only one page query may be in flight"
    );
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: middle.0,
            flow_id: None,
            offset: middle.2,
            total: records.len(),
            rows: records[middle.2..middle.2 + PAGE_SIZE]
                .iter()
                .map(|(row, _)| row.clone())
                .collect(),
        })
        .unwrap();
    cx.run_until_parked();
    let selected_query = query(&mut fixture);
    assert_eq!(
        selected_query.2,
        records.len() - 1,
        "the selected uncached row must finish even after the viewport scrolls away"
    );
    let (last, last_record) = records.last().unwrap();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: selected_query.0,
            flow_id: None,
            offset: selected_query.2,
            total: records.len(),
            rows: vec![last.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadWebSocket {
            project: read_project,
            selection_revision,
            message_id,
        },
        Command::QueryWebSocketHistory {
            project: query_project,
            query_revision: visible_query,
            flow_id,
            offset,
            limit,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("selected payload must be read before exactly one current-viewport query");
    };
    assert_eq!(
        (*read_project, *query_project),
        (fixture.project, fixture.project)
    );
    assert_eq!(
        (*flow_id, *offset, *limit),
        (None, 0, WS_HISTORY_BATCH_SIZE)
    );
    assert!(*visible_query > selected_query.0);
    let selected_read = (*selection_revision, *message_id);
    assert_eq!(selected_read.1, last.id);
    assert!(selected_read.0 > first_read.0);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: selected_read.0,
            record: Box::new(last_record.clone()),
            original_payload: None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: first_read.0,
            record: Box::new(records[0].1.clone()),
            original_payload: None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: *visible_query,
            flow_id: None,
            offset: 0,
            total: records.len(),
            rows: records[..PAGE_SIZE]
                .iter()
                .map(|(row, _)| row.clone())
                .collect(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.selected, Some(last.id));
        assert!(state.pending_row.is_none());
        assert_eq!(state.detail.as_deref(), Some(last_record));
        assert_eq!(state.payload.raw.read(cx).value().as_ref(), "selected last");
        assert_eq!(state.table.read(cx).delegate().visible.start, 0);
        assert_eq!(state.table.read(cx).delegate().offset, 0);
        assert_eq!(state.table.read(cx).delegate().rows.len(), PAGE_SIZE);
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn websocket_failed_page_retry_keeps_visible_rows_and_payload(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    let (summary, record) = message(Uuid::new_v4(), 1, b"keep selected", WebSocketKind::Text);
    let mut rows = vec![summary.clone()];
    rows.extend((1..PAGE_SIZE).map(|index| {
        message(
            Uuid::new_v4(),
            index as u64 + 1,
            b"page",
            WebSocketKind::Text,
        )
        .0
    }));
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: initial,
            flow_id: None,
            offset: 0,
            total: PAGE_SIZE + 1,
            rows: rows.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&fixture, 0, cx);
    let selected = read(&mut fixture);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: selected.0,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    scroll(&fixture, -10000., cx);
    let failed = query(&mut fixture);
    assert_eq!((failed.1, failed.3), (None, WS_HISTORY_BATCH_SIZE));
    assert!((1..=PAGE_SIZE).contains(&failed.2));
    fixture
        .events
        .try_send(Event::WebSocketHistoryError {
            project: fixture.project,
            query_revision: failed.0,
            message: "next page read failed".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let state = &fixture.view.read(cx).websocket;
        let table = state.table.read(cx);
        assert_eq!(table.delegate().rows, rows);
        assert_eq!(table.delegate().offset, 0);
        assert_eq!(table.selected_row(), Some(0));
        assert!(!table.delegate().loading);
        assert_eq!(state.selected, Some(summary.id));
        assert_eq!(state.detail.as_deref(), Some(&record));
        assert_eq!(state.payload.raw.read(cx).value().as_ref(), "keep selected");
        assert!(window.try_find_by_label("重试查询").is_some());
        window.click("websocket-refresh", cx);
    })
    .unwrap();
    let retried = query(&mut fixture);
    assert!(retried.0 > failed.0);
    assert_eq!(
        (retried.1, retried.2, retried.3),
        (None, failed.2, WS_HISTORY_BATCH_SIZE),
        "retry must request the failed page, not silently return to the visible page"
    );
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.table.read(cx).delegate().rows, rows);
        assert_eq!(state.table.read(cx).delegate().offset, 0);
        assert_eq!(state.detail.as_deref(), Some(&record));
    });
    let last_rows: Vec<_> = (failed.2..=PAGE_SIZE)
        .map(|index| {
            message(
                Uuid::new_v4(),
                index as u64 + 1,
                b"last",
                WebSocketKind::Text,
            )
            .0
        })
        .collect();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: retried.0,
            flow_id: None,
            offset: failed.2,
            total: PAGE_SIZE + 1,
            rows: last_rows.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        let table = state.table.read(cx);
        assert_eq!(table.delegate().rows, last_rows);
        assert_eq!(table.delegate().offset, failed.2);
        assert!(table.delegate().error.is_none());
        assert!(!table.delegate().loading);
        assert_eq!(state.selected, Some(summary.id));
        assert_eq!(state.detail.as_deref(), Some(&record));
    });
}

#[gpui_kit::test]
fn websocket_queue_and_query_failures_have_real_retry_controls(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 1);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("proxy-websocket-history", cx)
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.websocket.table.read(cx).delegate().loading);
        assert!(view.error.as_ref().unwrap().contains("队列已满"));
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("websocket-refresh", cx)
    })
    .unwrap();
    let failed = query(&mut fixture).0;
    fixture
        .events
        .try_send(Event::WebSocketHistoryError {
            project: fixture.project,
            query_revision: failed,
            message: "disk unavailable".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("重试查询").is_some());
        window.click("websocket-refresh", cx);
    })
    .unwrap();
    let retried = query(&mut fixture).0;
    assert!(retried > failed);
    let (summary, record) = message(Uuid::new_v4(), 1, b"recovered", WebSocketKind::Text);
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: retried,
            flow_id: None,
            offset: 0,
            total: 1,
            rows: vec![summary.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    changed(&fixture, cx);
    click_row(&fixture, 0, cx);
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.selected, Some(summary.id));
        assert!(state.detail.is_none());
        assert!(state.detail_error.as_ref().unwrap().contains("队列已满"));
    });
    let pending_page = query(&mut fixture).0;
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("websocket-detail-retry", cx);
    })
    .unwrap();
    let pending_read = read(&mut fixture);
    fixture
        .events
        .try_send(Event::WebSocketDetailError {
            project: fixture.project,
            selection_revision: pending_read.0 - 1,
            message_id: summary.id,
            message: "late old failure".into(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: pending_read.0,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: pending_page,
            flow_id: None,
            offset: 0,
            total: 1,
            rows: vec![summary],
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &fixture.view.read(cx).websocket;
        assert_eq!(state.detail.as_deref(), Some(&record));
        assert!(state.detail_error.is_none());
        assert!(!state.table.read(cx).delegate().loading);
    });
}
