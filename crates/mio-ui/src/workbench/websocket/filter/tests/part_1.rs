use super::*;

#[gpui_kit::test]
fn body_filter_preserves_regex_error_draft_and_ime_then_restores_applied_search(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    boot_populated(&mut f, cx);
    let (window, dialog) = open(&mut f, cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let term = dialog.read(cx).term.clone();
            window.click(("input", term.entity_id()), cx);
            window.input("(", cx);
        })
        .unwrap();
    for id in [
        "websocket-filter-regex",
        "websocket-filter-case",
        "websocket-filter-negative",
    ] {
        click(window, id, cx);
    }
    click(window, "websocket-filter-apply", cx);
    let invalid = query(&mut f);
    assert_eq!(invalid.1.term, "(");
    assert!(invalid.1.regex && invalid.1.case_sensitive && invalid.1.negative);
    let regex_error = "WebSocket 正则表达式无效或超出限制；当前不支持 Java lookaround / backreference 等语法：regex parse error:\n    (\n    ^\nerror: unclosed group";
    f.events
        .try_send(Event::WebSocketHistoryError {
            project: f.project,
            query_revision: invalid.0,
            message: regex_error.into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).websocket.applied_filter,
            WebSocketHistoryFilter::default()
        );
        assert_eq!(dialog.read(cx).term.read(cx).value().as_ref(), "(");
        assert_eq!(dialog.read(cx).error.as_deref(), Some(regex_error));
    });
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let body = window.find("websocket-filter-dialog").bounds();
            for id in [
                "websocket-filter-cancel",
                "websocket-filter-apply",
                "websocket-filter-apply-close",
            ] {
                let button = window.find(id);
                let bounds = button.bounds();
                assert!(button.visible());
                assert!(bounds.origin.x >= body.origin.x && bounds.right() <= body.right());
                assert!(
                    bounds.origin.y >= body.origin.y && bounds.bottom() <= body.bottom(),
                    "multiline Regex errors must not push {id} outside the window"
                );
            }
            let error = window.find("websocket-filter-error");
            assert!(error.visible());
            assert_eq!(error.role(), Some(gpui_kit::accesskit::Role::Alert));
            assert_eq!(error.label(), Some(regex_error));
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            let term = dialog.read(cx).term.clone();
            window.click(("input", term.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("WS", cx);
            term.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
            });
            window.click("websocket-filter-apply", cx);
            assert!(term.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }));
            term.update(cx, |input, cx| {
                input.unmark_text(window, cx);
                input.set_value("WS.*合成", window, cx);
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    click(window, "websocket-filter-apply", cx);
    let applied = query(&mut f);
    assert_eq!(applied.1.term, "WS.*合成");
    assert!(applied.1.regex && applied.1.case_sensitive && applied.1.negative);
    page(&f, &applied, vec![], cx);
    click(window, "websocket-filter-all", cx);
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).draft, WebSocketHistoryFilter::default());
        assert!(dialog.read(cx).term.read(cx).value().is_empty());
    });
    click(window, "websocket-filter-revert", cx);
    cx.read(|cx| {
        assert_eq!(dialog.read(cx).draft, applied.1);
        assert_eq!(dialog.read(cx).term.read(cx).value().as_ref(), "WS.*合成");
    });
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let body = window.find("websocket-filter-dialog").bounds();
            for id in [
                "websocket-filter-regex",
                "websocket-filter-case",
                "websocket-filter-negative",
                "websocket-filter-apply-close",
            ] {
                let bounds = window.find(id).bounds();
                assert!(bounds.origin.x >= body.origin.x && bounds.right() <= body.right());
                assert!(bounds.origin.y >= body.origin.y && bounds.bottom() <= body.bottom());
            }
        })
        .unwrap();
    click(window, "websocket-filter-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| assert_eq!(f.view.read(cx).websocket.applied_filter, applied.1));
}

#[gpui_kit::test]
fn body_filter_membership_keeps_offscreen_selection_and_checks_newer_selection(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    boot(&mut f, cx);
    let (first, record) = message(Uuid::new_v4(), 1, b"match", WebSocketKind::Text);
    let (second, _) = message(Uuid::new_v4(), 2, b"hidden", WebSocketKind::Text);
    f.events
        .try_send(Event::WebSocketHistoryChanged { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let initial = query(&mut f);
    page(&f, &initial, vec![first.clone(), second.clone()], cx);
    click_row(&f, 0, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadWebSocket {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        unreachable!()
    };
    f.events
        .try_send(Event::WebSocketDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(record.clone()),
            original_payload: None,
        })
        .unwrap();
    cx.run_until_parked();
    let (window, dialog) = open(&mut f, cx);
    window
        .update(cx, |_, window, cx| {
            let term = dialog.read(cx).term.clone();
            window.click(("input", term.entity_id()), cx);
            window.input("match", cx);
        })
        .unwrap();
    click(window, "websocket-filter-apply", cx);
    let applied = query(&mut f);
    assert_eq!(applied.2, Some(first.id));
    let other_rows: Vec<_> = (0..WS_HISTORY_BATCH_SIZE)
        .map(|i| message(Uuid::new_v4(), i as u64 + 3, b"match", WebSocketKind::Text).0)
        .collect();
    f.events
        .try_send(Event::WebSocketHistoryPage {
            project: f.project,
            query_revision: applied.0,
            flow_id: None,
            filter: applied.1.clone(),
            selected_match: Some((first.id, true)),
            offset: 0,
            total: WS_HISTORY_BATCH_SIZE + 1,
            rows: other_rows,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &f.view.read(cx).websocket;
        assert_eq!(state.selected, Some(first.id));
        assert_eq!(state.detail.as_deref(), Some(&record));
    });
    assert!(drain(&mut f.commands).is_empty());
    click(window, "websocket-filter-all", cx);
    click(window, "websocket-filter-apply", cx);
    let all = query(&mut f);
    page(&f, &all, vec![first.clone(), second.clone()], cx);
    window
        .update(cx, |_, window, cx| {
            let term = dialog.read(cx).term.clone();
            window.click(("input", term.entity_id()), cx);
            window.input("match", cx);
        })
        .unwrap();
    click(window, "websocket-filter-apply", cx);
    let requested = query(&mut f);
    click_row(&f, 1, cx);
    let commands = drain(&mut f.commands);
    assert!(
        matches!(commands.as_slice(), [Command::ReadWebSocket { message_id, .. }] if *message_id == second.id)
    );
    f.events
        .try_send(Event::WebSocketHistoryPage {
            project: f.project,
            query_revision: requested.0,
            flow_id: None,
            filter: requested.1.clone(),
            selected_match: Some((first.id, true)),
            offset: 0,
            total: 1,
            rows: vec![first],
        })
        .unwrap();
    cx.run_until_parked();
    let membership = query(&mut f);
    assert_eq!(membership.2, Some(second.id));
    assert_eq!(membership.1, requested.1);
    f.events
        .try_send(Event::WebSocketHistoryPage {
            project: f.project,
            query_revision: membership.0,
            flow_id: None,
            filter: membership.1,
            selected_match: Some((second.id, false)),
            offset: 0,
            total: 0,
            rows: vec![],
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let state = &f.view.read(cx).websocket;
        assert!(state.selected.is_none());
        assert!(state.detail.is_none());
    });
    click(window, "websocket-filter-cancel", cx);
}

#[gpui_kit::test]
fn notes_filter_checkbox_applies_the_real_query_condition(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![], 64);
    boot_populated(&mut f, cx);
    let (window, _dialog) = open(&mut f, cx);
    click(window, "websocket-filter-notes", cx);
    click(window, "websocket-filter-apply", cx);
    let applied = query(&mut f);
    assert!(applied.1.only_with_notes);
    assert!(applied.1.term.is_empty());
    assert!(!applied.1.hide_incoming && !applied.1.hide_outgoing);
    assert_eq!(applied.1.listener_port, None);
    page(&f, &applied, vec![], cx);
    cx.read(|cx| assert!(f.view.read(cx).websocket.applied_filter.only_with_notes));
    click(window, "websocket-filter-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
}
