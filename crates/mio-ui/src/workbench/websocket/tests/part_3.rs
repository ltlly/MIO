use super::*;

#[gpui_kit::test]
fn websocket_revision_refresh_and_project_change_reject_obsolete_details(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    let (mut summary, mut record) = message(Uuid::new_v4(), 1, b"before", WebSocketKind::Text);
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
            rows: vec![summary.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&fixture, 0, cx);
    let first_read = read(&mut fixture);
    fixture
        .events
        .try_send(Event::WebSocketHistoryChanged {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    let refresh = query(&mut fixture).0;
    summary.revision = 2;
    summary.payload_bytes = 5;
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: refresh,
            flow_id: None,
            offset: 0,
            total: 1,
            rows: vec![summary.clone()],
        })
        .unwrap();
    cx.run_until_parked();
    let new_read = read(&mut fixture);
    assert!(new_read.0 > first_read.0);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: new_read.0,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    record.revision = 2;
    record.payload = b"after".to_vec();
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: first_read.0,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(fixture.view.read(cx).websocket.detail.is_none());
        window.click("websocket-detail-retry", cx);
    })
    .unwrap();
    let retry = read(&mut fixture);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: retry.0,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).websocket.detail.as_deref(),
            Some(&record)
        )
    });
    let old_project = fixture.project;
    fixture.project.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    let new_query = query(&mut fixture).0;
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: old_project,
            query_revision: new_query,
            flow_id: None,
            offset: 0,
            total: 1,
            rows: vec![summary],
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketHistoryError {
            project: old_project,
            query_revision: new_query,
            message: "late old project".into(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: old_project,
            selection_revision: retry.0,
            record: Box::new(record),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let state = &view.websocket;
        assert!(state.table.read(cx).delegate().rows.is_empty());
        assert!(state.table.read(cx).delegate().error.is_none());
        assert!(state.selected.is_none());
        assert!(state.detail.is_none());
        assert!(state.payload.raw.read(cx).value().is_empty());
        assert!(!view.has_traffic);
    });
}

#[gpui_kit::test]
fn websocket_disconnect_finishes_pending_query(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    open(&mut fixture, cx);
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .websocket
                .table
                .read(cx)
                .delegate()
                .loading
        );
    });

    fixture.commands.close();
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let state = &view.websocket;
        let table = state.table.read(cx);
        assert!(view.backend_disconnected);
        assert!(
            !table.delegate().loading,
            "a closed service cannot finish a pending query"
        );
        assert!(table.delegate().error.as_ref().unwrap().contains("断开"));
        assert!(table.delegate().rows.is_empty());
        assert!(!state.dirty);
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("重试查询").is_some());
        window.click("websocket-refresh", cx);
        assert!(
            !fixture
                .view
                .read(cx)
                .websocket
                .table
                .read(cx)
                .delegate()
                .loading
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn websocket_disconnect_marks_pending_detail_but_keeps_loaded_payload(cx: &mut TestAppContext) {
    for detail_arrived in [false, true] {
        let mut fixture = fixture(cx, vec![], 64);
        let initial = open(&mut fixture, cx);
        let (summary, record) = message(Uuid::new_v4(), 1, b"keep payload", WebSocketKind::Text);
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
                rows: vec![summary.clone()],
            })
            .unwrap();
        cx.run_until_parked();
        click_row(&fixture, 0, cx);
        let selected = read(&mut fixture);
        if detail_arrived {
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
            cx.update_window(fixture.window, |_, window, cx| {
                window.render_frame(cx);
                let raw = fixture.view.read(cx).websocket.payload.raw.clone();
                window.click(("input", raw.entity_id()), cx);
                window.render_frame(cx);
                assert!(raw.focus_handle(cx).is_focused(window));
                window.press(crate::kbd!("a"), cx);
                window.input("不应写入只读正文", cx);
                assert_eq!(raw.read(cx).value().as_ref(), "keep payload");
            })
            .unwrap();
        }

        fixture.commands.close();
        let (replacement, _) = mpsc::channel(1);
        drop(std::mem::replace(&mut fixture.events, replacement));
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let state = &fixture.view.read(cx).websocket;
            assert_eq!(state.selected, Some(summary.id));
            assert_eq!(
                state.table.read(cx).delegate().rows.as_slice(),
                std::slice::from_ref(&summary)
            );
            if detail_arrived {
                assert_eq!(state.detail.as_deref(), Some(&record));
                assert!(state.detail_error.is_none());
                assert_eq!(state.payload.raw.read(cx).value().as_ref(), "keep payload");
                assert!(state.payload.raw.focus_handle(cx).is_focused(window));
            } else {
                assert!(state.detail.is_none());
                assert!(state.detail_error.as_ref().unwrap().contains("断开"));
                assert!(window.try_find_by_label("重试读取").is_some());
            }
            assert!(window.is_action_available(&Quit, cx));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn websocket_live_page_rollover_hands_focused_payload_back_to_workspace(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial = open(&mut fixture, cx);
    let (summary, record) = message(Uuid::new_v4(), 1, b"focused payload", WebSocketKind::Text);
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
            rows: vec![summary],
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
            record: Box::new(record),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let raw = fixture.view.read(cx).websocket.payload.raw.clone();
        window.click(("input", raw.entity_id()), cx);
        window.render_frame(cx);
        assert!(raw.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();

    fixture
        .events
        .try_send(Event::WebSocketHistoryChanged {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    let refreshed = query(&mut fixture).0;
    fixture
        .events
        .try_send(Event::WebSocketHistoryPage {
            selected_match: None,
            filter: WebSocketHistoryFilter::default(),
            project: fixture.project,
            query_revision: refreshed,
            flow_id: None,
            offset: 0,
            total: PAGE_SIZE + 1,
            rows: (0..PAGE_SIZE)
                .map(|index| {
                    message(
                        Uuid::new_v4(),
                        index as u64 + 1,
                        b"new",
                        WebSocketKind::Text,
                    )
                    .0
                })
                .collect(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.websocket.selected.is_none());
        assert!(view.websocket.detail.is_none());
        assert!(
            view.focus.is_focused(window),
            "the removed editor must hand off its focus"
        );
        assert!(window.is_action_available(&Quit, cx));
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "Cmd-Q must still reach the workspace"
    );
    cx.simulate_prompt_answer("取消");
    assert!(!cx.has_pending_prompt());
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
}
