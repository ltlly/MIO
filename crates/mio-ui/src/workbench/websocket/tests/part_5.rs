use super::*;

#[gpui_kit::test]
fn websocket_repeater_transfer_utf8_guard_send_receive_and_replay_history(cx: &mut TestAppContext) {
    use mio_core::{
        WebSocketRepeaterCommand, WebSocketRepeaterConnectionMode, WebSocketRepeaterEvent,
        WebSocketRepeaterHistoryItem, WebSocketRepeaterTab,
    };

    let http_tab = tab("1");
    let http_tab_id = http_tab.id;
    let mut fixture = fixture(cx, vec![http_tab], 64);
    let initial = open(&mut fixture, cx);
    let flow_id = Uuid::new_v4();
    let (mut summary, record) = message(flow_id, 1, b"edited-client-msg", WebSocketKind::Text);
    summary.edited = true;
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

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("websocket-to-repeater").disabled(), Some(true));
    })
    .unwrap();

    click_row(&fixture, 0, cx);
    let (sel_rev, msg_id) = read(&mut fixture);
    assert_eq!(msg_id, record.id);
    fixture
        .events
        .try_send(Event::WebSocketDetail {
            project: fixture.project,
            selection_revision: sel_rev,
            record: Box::new(record.clone()),
            original_payload: Some(b"orig-client-msg".to_vec()),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("websocket-to-repeater").disabled(), None);
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
        .expect("clicking #websocket-to-repeater must emit OpenFromHistory");
    let operation_id = match open_cmd {
        WebSocketRepeaterCommand::OpenFromHistory {
            project,
            operation_id,
            message_id,
            use_original,
            payload_override,
        } => {
            assert_eq!(project, fixture.project);
            assert_eq!(message_id, record.id);
            assert!(use_original);
            assert!(payload_override.is_none());
            operation_id
        }
        other => panic!("unexpected command: {other:?}"),
    };

    let ws_tab_id = Uuid::new_v4();
    let ws_tab = WebSocketRepeaterTab {
        id: ws_tab_id,
        title: "1".into(),
        group: None,
        url: "ws://127.0.0.1:9001/socket".into(),
        handshake_headers: Vec::new(),
        source_flow_id: Some(flow_id),
        source_message_id: Some(record.id),
        direction: WebSocketDirection::ServerToClient,
        draft_revision: 1,
        draft_payload: b"orig-client-msg".to_vec(),
        draft_kind: WebSocketKind::Text,
        history: Vec::new(),
        closed: false,
    };
    fixture
        .events
        .try_send(Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
            project: fixture.project,
            operation_id,
            result: Ok(ws_tab),
            mode: WebSocketRepeaterConnectionMode::ProxyShared { flow_id },
        }))
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    // Verify co-existence inside #work-tabs without breaking 47pt strip or + button.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.tool == Tool::Repeater);
        assert_eq!(view.workspace.active, None);
        assert_eq!(view.workspace.active_websocket_tab, Some(ws_tab_id));

        let strip = window.find("work-tabs").bounds();
        let http_bounds = window
            .find(SharedString::from(format!("work-tab-{http_tab_id}")))
            .bounds();
        let ws_bounds = window
            .find(SharedString::from(format!("ws-work-tab-{ws_tab_id}")))
            .bounds();
        let plus_bounds = window.find("repeater-new").bounds();
        assert_eq!(strip.size.height, px(47.));
        assert!(ws_bounds.origin.x >= http_bounds.right());
        assert!(plus_bounds.origin.x >= ws_bounds.right());
        assert!(window.try_find("ws-repeater-send").is_some());
        assert_eq!(
            window.find("ws-repeater-status").label(),
            Some("复用代理连接")
        );
        assert_eq!(window.find("ws-repeater-dir-s2c").disabled(), None);

        // Invalid UTF-8 in Hex view must be rejected when Text frame kind is selected.
        window.click("ws-repeater-draft-hex", cx);
        window.render_frame(cx);
        let hex_editor = fixture.view.read(cx).websocket_repeater.editors[&ws_tab_id]
            .draft
            .hex
            .clone();
        hex_editor.update(cx, |state, cx| state.set_value("ff fe", window, cx));
        window.click("ws-repeater-send", cx);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("UTF-8")
        );
    })
    .unwrap();
    assert!(
        !drain(&mut fixture.commands)
            .iter()
            .any(|cmd| matches!(cmd, Command::WebSocketRepeater(_)))
    );

    // Valid UTF-8 draft advances draft_revision and dispatches Send.
    cx.update_window(fixture.window, |_, window, cx| {
        let hex_editor = fixture.view.read(cx).websocket_repeater.editors[&ws_tab_id]
            .draft
            .hex
            .clone();
        hex_editor.update(cx, |state, cx| state.set_value("68 69", window, cx));
        window.click("ws-repeater-draft-raw", cx);
        window.render_frame(cx);
        let raw_editor = fixture.view.read(cx).websocket_repeater.editors[&ws_tab_id]
            .draft
            .raw
            .clone();
        window.click(("input", raw_editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("client-ping", cx);
        window.click("ws-repeater-dir-c2s", cx);
        window.click("ws-repeater-send", cx);
    })
    .unwrap();

    let send_cmd = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::WebSocketRepeater(cmd) => Some(cmd),
            _ => None,
        })
        .expect("clicking #ws-repeater-send must emit WebSocketRepeaterCommand::Send");
    let (send_op, sent_rev) = match send_cmd {
        WebSocketRepeaterCommand::Send {
            project,
            operation_id,
            tab_id,
            direction,
            kind,
            payload,
            draft_revision,
            ..
        } => {
            assert_eq!(project, fixture.project);
            assert_eq!(tab_id, ws_tab_id);
            assert_eq!(direction, WebSocketDirection::ClientToServer);
            assert_eq!(kind, WebSocketKind::Text);
            assert_eq!(payload, b"client-ping");
            assert!(draft_revision > 1);
            (operation_id, draft_revision)
        }
        other => panic!("unexpected command: {other:?}"),
    };

    let sent_item = WebSocketRepeaterHistoryItem {
        id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        flow_id: Some(flow_id),
        sequence: 2,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Text,
        payload: b"client-ping".to_vec(),
        observed_unix_ms: 2000,
    };
    let received_item = WebSocketRepeaterHistoryItem {
        id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        flow_id: Some(flow_id),
        sequence: 3,
        direction: WebSocketDirection::ServerToClient,
        kind: WebSocketKind::Text,
        payload: b"server-pong-payload".to_vec(),
        observed_unix_ms: 2005,
    };
    fixture
        .events
        .try_send(Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
            project: fixture.project,
            operation_id: send_op,
            tab_id: ws_tab_id,
            draft_revision: sent_rev,
            mode: WebSocketRepeaterConnectionMode::Standalone { flow_id },
            result: Ok(sent_item),
        }))
        .unwrap();
    fixture
        .events
        .try_send(Event::WebSocketRepeater(WebSocketRepeaterEvent::Received {
            project: fixture.project,
            tab_id: ws_tab_id,
            flow_id,
            item: received_item,
        }))
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            fixture.view.read(cx).workspace.websocket_tabs[0]
                .history
                .len(),
            2
        );
        assert!(
            window
                .try_find(("ws-repeater-history-row", 0_usize))
                .is_some()
        );
        assert!(
            window
                .try_find(("ws-repeater-history-row", 1_usize))
                .is_some()
        );
        assert_eq!(
            window.find("ws-repeater-status").label(),
            Some("独立会话已连接")
        );
        assert_eq!(window.find("ws-repeater-dir-s2c").disabled(), Some(true));

        window.click(("ws-repeater-history-row", 1_usize), cx);
        window.render_frame(cx);
        window.click("ws-repeater-use-history", cx);
        window.render_frame(cx);

        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.websocket_tabs[0].draft_payload,
            b"server-pong-payload"
        );
        assert_eq!(
            view.workspace.websocket_tabs[0].direction,
            WebSocketDirection::ClientToServer,
            "Standalone mode must keep direction locked to ClientToServer on history replay"
        );
        assert_eq!(
            view.websocket_repeater.editors[&ws_tab_id]
                .draft
                .raw
                .read(cx)
                .value()
                .as_ref(),
            "server-pong-payload"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn intercept_websocket_to_repeater_sends_edited_payload_override_and_guards_ime(
    cx: &mut TestAppContext,
) {
    use mio_core::WebSocketRepeaterCommand;

    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let (_, record) = message(Uuid::new_v4(), 1, b"intercept-orig", WebSocketKind::Text);
    let message_id = record.id;
    fixture
        .events
        .try_send(Event::WebSocketInterceptPending {
            project: fixture.project,
            url: "ws://127.0.0.1:9001/socket".into(),
            record: Box::new(record),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("intercept-ws-to-repeater").is_some());
        let raw = fixture.view.read(cx).intercepts[0].editor.raw.clone();
        raw.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        window.click("intercept-ws-to-repeater", cx);
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );

        raw.update(cx, |state, cx| {
            state.unmark_text(window, cx);
            state.set_value("intercept-edited", window, cx);
        });
        window.click("intercept-ws-to-repeater", cx);
    })
    .unwrap();

    let cmd = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::WebSocketRepeater(cmd) => Some(cmd),
            _ => None,
        })
        .expect("clicking #intercept-ws-to-repeater must emit OpenFromHistory");
    match cmd {
        WebSocketRepeaterCommand::OpenFromHistory {
            project,
            message_id: sent_id,
            use_original,
            payload_override,
            ..
        } => {
            assert_eq!(project, fixture.project);
            assert_eq!(sent_id, message_id);
            assert!(!use_original);
            assert_eq!(
                payload_override,
                Some((WebSocketKind::Text, b"intercept-edited".to_vec()))
            );
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
