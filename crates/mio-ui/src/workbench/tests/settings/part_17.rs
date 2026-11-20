use super::*;

#[gpui_kit::test]
fn proxy_listener_persistence_copy_response_and_delete_live_history_work(cx: &mut TestAppContext) {
    let initial = tab("Proxy监听与复制响应与删除历史测试");
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);

    let settings = open_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-proxy", cx);
        window.render_frame(cx);
        window.click(("settings-listener-row", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-edit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-editor-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("127.0.0.1:18080", cx);
        window.render_frame(cx);
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let saved_ws = saved(&mut fixture.commands);
    assert_eq!(saved_ws.proxy_listener.as_deref(), Some("127.0.0.1:18080"));
    acknowledge(&fixture, saved_ws, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .listener_input
                .read(cx)
                .value()
                .as_ref(),
            "127.0.0.1:18080"
        );
    });

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-proxy-listener-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let reset_ws = saved(&mut fixture.commands);
    assert_eq!(reset_ws.proxy_listener, None);
    acknowledge(&fixture, reset_ws, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-http", cx);
        window.render_frame(cx);
        assert!(window.find("settings-network-http1-heading").visible());
        assert!(window.find("settings-network-http2-heading").visible());
        window.click("settings-http1-keep-alive", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let http1_ws = saved(&mut fixture.commands);
    assert!(http1_ws.repeater_settings.enforce_http1_connection_close);
    acknowledge(&fixture, http1_ws, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..6 {
            if window.find("settings-100-remove-headers").visible() {
                break;
            }
            window.scroll(
                "settings-network-http-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-450.))),
                cx,
            );
            window.render_frame(cx);
        }
        assert!(window.find("settings-network-100-continue-heading").visible());
        window.click("settings-100-remove-headers", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let http_100_ws = saved(&mut fixture.commands);
    assert!(http_100_ws.network_http.remove_100_continue_headers);
    acknowledge(&fixture, http_100_ws, cx);

    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .listener_input
                .read(cx)
                .value()
                .as_ref(),
            "127.0.0.1:8080"
        );
    });
    cx.simulate_keystrokes(settings, "escape");

    activate_empty_repeater(&mut fixture, cx);
    let tab_id = cx.read(|cx| fixture.view.read(cx).workspace.active.unwrap());
    let response_raw = "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello-world";
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tab_editors.get_mut(&tab_id).unwrap().response.load(
                response_raw.as_bytes(),
                window,
                cx,
            );
        });
        window.dispatch_action(Box::new(crate::CopyResponse), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(response_raw.into())
        );
    });

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tab_editors.get_mut(&tab_id).unwrap().response.load(
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n\xff\xfe\xfd\x00",
                window,
                cx,
            );
        });
        window.dispatch_action(Box::new(crate::CopyResponse), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|err| err.contains("非 UTF-8 二进制字节"))
        );
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(response_raw.into())
        );
    });

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.render_frame(cx);
        window.click("proxy-history", cx);
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    let live_flow = record("http://127.0.0.1:9001/live-delete", "live-resp");
    let live_id = live_flow.summary.id;
    let ext_row = HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
        item: mio_core::ExtItemSummary {
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
        origin: Some("http://127.0.0.1:9001".into()),
        notes_revision: 0,
        notes_preview: String::new(),
        notes_error: None,
        highlight: None,
        highlight_error: None,
        time_unix_ms: None,
        time_error: None,
    }));
    let ext_entry_id = ext_row.id();
    let query_rev = cx.read(|cx| fixture.view.read(cx).query_revision);
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: query_rev,
            offset: 0,
            total: 2,
            rows: vec![ext_row, HistoryRow::Live(live_flow.summary.clone())],
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.history_pointer_selection(
                ext_entry_id,
                MouseButton::Left,
                Modifiers::default(),
                window,
                cx,
            );
        });
        window.dispatch_action(Box::new(crate::DeleteSelectedHttpHistory), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .into_iter()
            .all(|cmd| !matches!(cmd, Command::DeleteHttpHistory { .. })),
        "readonly External rows must not emit DeleteHttpHistory"
    );
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|err| err.contains("外部源归档记录为只读"))
        );
    });

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.history_pointer_selection(
                HistoryEntryId::Live(live_id),
                MouseButton::Left,
                Modifiers::default(),
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::DeleteSelectedHttpHistory), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (delete_op, deleted_ids) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::DeleteHttpHistory {
                project,
                operation_id,
                flow_ids,
            } if project == fixture.project => Some((operation_id, flow_ids)),
            _ => None,
        })
        .expect("selected Live history row must emit Command::DeleteHttpHistory");
    assert_eq!(deleted_ids, vec![live_id]);

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::DeleteSelectedHttpHistory), cx);
        window.dispatch_action(Box::new(crate::ClearLiveProxyHistory), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .into_iter()
            .all(|command| !matches!(
                command,
                Command::DeleteHttpHistory { .. } | Command::ClearLiveProxyHistory { .. }
            )),
        "pending deletion must prevent duplicate Delete and Clear commands"
    );
    let awaiting_delete = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.pending_history_delete,
            Some((fixture.project, delete_op))
        );
        assert_eq!(view.selected_history, Some(HistoryEntryId::Live(live_id)));
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("仍在处理中"))
        );
        (
            view.status.clone(),
            view.error.clone(),
            view.query_revision,
            view.selection_revision,
        )
    });
    for (project, operation_id, result) in [
        (
            ProjectKey {
                generation: fixture.project.generation + 1,
                ..fixture.project
            },
            delete_op,
            Ok(1),
        ),
        (
            fixture.project,
            Uuid::new_v4(),
            Err("错误操作的删除回执".to_string()),
        ),
    ] {
        fixture
            .events
            .try_send(Event::HttpHistoryDeleted {
                project,
                operation_id,
                deleted_ids: vec![live_id],
                result,
            })
            .unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(
                view.pending_history_delete,
                Some((fixture.project, delete_op))
            );
            assert_eq!(view.selected_history, Some(HistoryEntryId::Live(live_id)));
            assert_eq!(view.history.read(cx).delegate().selected_ids.len(), 1);
            assert!(
                view.history
                    .read(cx)
                    .delegate()
                    .selected_ids
                    .contains(&HistoryEntryId::Live(live_id))
            );
            assert_eq!(view.history.read(cx).delegate().rows.len(), 2);
            assert_eq!(
                (
                    view.status.clone(),
                    view.error.clone(),
                    view.query_revision,
                    view.selection_revision,
                ),
                awaiting_delete,
                "a receipt from another project generation or operation must be ignored"
            );
        });
        assert!(drain(&mut fixture.commands).is_empty());
    }

    let delete_failure = "HTTP 历史记录仍在传输中，不能删除";
    fixture
        .events
        .try_send(Event::HttpHistoryDeleted {
            project: fixture.project,
            operation_id: delete_op,
            deleted_ids: Vec::new(),
            result: Err(delete_failure.to_string()),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_history_delete.is_none());
        assert_eq!(view.error.as_deref(), Some(delete_failure));
        assert_eq!(view.selected_history, Some(HistoryEntryId::Live(live_id)));
        assert_eq!(view.history.read(cx).delegate().rows.len(), 2);
        assert_eq!(view.query_revision, awaiting_delete.2);
        assert_eq!(view.selection_revision, awaiting_delete.3);
    });
    assert!(drain(&mut fixture.commands).is_empty());

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::DeleteSelectedHttpHistory), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let retry_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::DeleteHttpHistory {
                project,
                operation_id,
                flow_ids,
            } if project == fixture.project => {
                assert_eq!(flow_ids, vec![live_id]);
                Some(operation_id)
            }
            _ => None,
        })
        .expect("a failed deletion must release its operation so the selection can be retried");
    assert_ne!(retry_op, delete_op);
    fixture
        .events
        .try_send(Event::HttpHistoryDeleted {
            project: fixture.project,
            operation_id: delete_op,
            deleted_ids: vec![live_id],
            result: Ok(1),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.pending_history_delete,
            Some((fixture.project, retry_op))
        );
        assert_eq!(view.selected_history, Some(HistoryEntryId::Live(live_id)));
        assert!(
            view.error.is_none(),
            "a retry clears the previous failure before its receipt arrives"
        );
        assert_eq!(view.query_revision, awaiting_delete.2);
    });
    assert!(drain(&mut fixture.commands).is_empty());

    fixture
        .events
        .try_send(Event::HttpHistoryDeleted {
            project: fixture.project,
            operation_id: retry_op,
            deleted_ids: vec![live_id],
            result: Ok(1),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_history_delete.is_none());
        assert_eq!(view.selected_history, None);
        assert!(view.status.contains("已删除 1 条实时 Proxy 历史记录"));
        assert!(
            !view
                .extra_tools
                .logger_entries
                .iter()
                .any(|e| e.id == live_id)
        );
    });
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .any(|cmd| matches!(cmd, Command::QueryHttpHistory { .. }))
    );

    let completed_delete = cx.read(|cx| {
        let view = fixture.view.read(cx);
        (
            view.status.clone(),
            view.error.clone(),
            view.query_revision,
            view.selection_revision,
        )
    });
    for (operation_id, result) in [
        (delete_op, Ok(1)),
        (retry_op, Err("迟到的重复删除失败".to_string())),
    ] {
        fixture
            .events
            .try_send(Event::HttpHistoryDeleted {
                project: fixture.project,
                operation_id,
                deleted_ids: vec![live_id],
                result,
            })
            .unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.pending_history_delete.is_none());
            assert_eq!(view.selected_history, None);
            assert_eq!(
                (
                    view.status.clone(),
                    view.error.clone(),
                    view.query_revision,
                    view.selection_revision,
                ),
                completed_delete,
                "completed deletion must ignore duplicate success and failure receipts"
            );
        });
        assert!(drain(&mut fixture.commands).is_empty());
    }

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::ClearLiveProxyHistory), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let clear_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ClearLiveProxyHistory {
                project,
                operation_id,
            } if project == fixture.project => Some(operation_id),
            _ => None,
        })
        .expect("ClearLiveProxyHistory must be enqueued");
    fixture
        .events
        .try_send(Event::HttpHistoryDeleted {
            project: fixture.project,
            operation_id: clear_op,
            deleted_ids: Vec::new(),
            result: Ok(0),
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    super::part_18::verify_http_filter_and_user_tls_certificates(fixture, cx);
}
