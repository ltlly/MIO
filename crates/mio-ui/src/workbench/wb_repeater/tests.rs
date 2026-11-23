use super::*;
use crate::workbench::tests::{acknowledge_history_query, drain, fixture, tab};
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};
use mio_core::{
    FlowState, Header, HttpResponse, RepeaterHistory, RepeaterHistoryCursor,
    RepeaterHistoryEntry, RepeaterHistoryEntryId, RepeaterHistoryPage, RepeaterHistorySource,
    TabGroup, Uuid,
    WebSocketDirection, WebSocketKind, WebSocketRepeaterCommand, WebSocketRepeaterConnectionMode,
    WebSocketRepeaterEvent, WebSocketRepeaterHistoryItem, WebSocketRepeaterTab,
};

#[gpui_kit::test]
fn repeater_protocol_toggle_and_request_line_edit_sync_http_version(cx: &mut TestAppContext) {
    let initial = tab("Proto");
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-protocol-toggle").label(),
            Some("HTTP/1.1")
        );
        window.click("repeater-protocol-toggle", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-protocol-toggle").label(),
            Some("HTTP/2")
        );
    })
    .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut f.commands);
    let commands = acknowledge_history_query(&f, commands, cx);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("protocol toggle must save workspace once");
    };
    assert_eq!(workspace.tabs[0].draft.version, HttpVersion::Http2);
    assert!(
        std::str::from_utf8(&workspace.tabs[0].draft.raw)
            .unwrap()
            .starts_with("GET / HTTP/2\r\n")
    );
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();

    // Active IME composition blocks protocol rewrite without losing marked text.
    cx.update_window(f.window, |_, window, cx| {
        let req = f.view.read(cx).tab_editors[&tab_id].request.raw.clone();
        req.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
        });
        window.click("repeater-protocol-toggle", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-protocol-toggle").label(),
            Some("HTTP/2")
        );
        req.update(cx, |editor, cx| {
            assert!(editor.marked_text_range(window, cx).is_some());
            editor.unmark_text(window, cx);
            editor.set_value(
                "GET /manual HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n",
                window,
                cx,
            );
        });
        window.click("repeater-send", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-protocol-toggle").label(),
            Some("HTTP/1.1")
        );
    })
    .unwrap();
    cx.run_until_parked();

    let saved_ws = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("manual request-line edit before send must save synced Http11 version");
    assert_eq!(saved_ws.tabs[0].draft.version, HttpVersion::Http11);
}

#[gpui_kit::test]
fn repeater_group_context_menu_sends_all_tabs_in_group(cx: &mut TestAppContext) {
    let group = TabGroup {
        id: Uuid::new_v4(),
        title: "BatchGroup".into(),
        collapsed: false,
        color: Some(7),
    };
    let mut first = tab("G1");
    let mut second = tab("G2");
    let outsider = tab("Out");
    first.group = Some(group.id);
    second.group = Some(group.id);
    let first_id = first.id;
    let second_id = second.id;
    let outsider_id = outsider.id;

    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace: Workspace {
                revision: 1,
                groups: vec![group.clone()],
                tabs: vec![first, second, outsider],
                active: Some(first_id),
                ..Default::default()
            },
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    assert!(acknowledge_history_query(&f, cmds, cx).is_empty());

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(format!("group-{}", group.id), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("发送本组请求").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let sent: Vec<TabId> = drain(&mut f.commands)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Command::Send { tab_id, .. } => Some(tab_id),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![first_id, second_id]);
    assert!(!sent.contains(&outsider_id));
}

#[gpui_kit::test]
fn repeater_quick_settings_dropdown_and_inline_follow_redirection_work(cx: &mut TestAppContext) {
    let mut initial = tab("快捷设置");
    initial.draft.target = "http://127.0.0.1:9001".into();
    initial.draft.raw =
        b"POST /step1 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Length: 3\r\n\r\nabc".to_vec();
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert!(window.find("repeater-settings-menu").visible());
        assert!(window.try_find("repeater-follow-redirection").is_none());
        window.click("repeater-settings-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    assert!(acknowledge_history_query(&f, cmds, cx).is_empty());

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("HTTP/1 强制 Connection: close")
            .bounds()
            .center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let saved_h1 = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("clicking quick setting item must persist workspace");
    assert!(saved_h1.repeater_settings.enforce_http1_connection_close);
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: saved_h1,
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-settings-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("仅同站跟随重定向").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let saved_redirect = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("redirect mode selection must save workspace");
    assert_eq!(
        saved_redirect.repeater_settings.follow_redirects,
        RepeaterRedirectMode::OnSiteOnly
    );
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: saved_redirect,
        })
        .unwrap();
    cx.run_until_parked();

    // Trigger 302 response and verify inline follow redirection button sits in main toolbar.
    let send_bounds = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("repeater-cancel").is_none());
            let send_bounds = window.find("repeater-send").bounds();
            window.click("repeater-send", cx);
            window.render_frame(cx);
            assert!(window.try_find("repeater-send").is_none());
            let cancel_bounds = window.find("repeater-cancel").bounds();
            assert_eq!(cancel_bounds.origin, send_bounds.origin);
            assert_eq!(cancel_bounds.size, send_bounds.size);
            send_bounds
        })
        .unwrap();
    cx.run_until_parked();
    let (run_id, sent_rev) = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send { run_id, draft, .. } => Some((run_id, draft.revision)),
            _ => None,
        })
        .expect("Repeater Send command");
    let mut redirect_rec = crate::workbench::tests::record("http://127.0.0.1:9001/step1", "abc");
    redirect_rec.summary.id = run_id;
    redirect_rec.summary.status = Some(302);
    redirect_rec.summary.state = FlowState::Complete;
    redirect_rec.response = Some(HttpResponse {
        status: 302,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Location".into(),
            value: b"http://127.0.0.1:9002/landed".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    });
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id,
            draft_revision: sent_rev,
            record: Box::new(redirect_rec),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    for cmd in drain(&mut f.commands) {
        if let Command::SaveWorkspace { workspace, .. } = cmd {
            f.events
                .try_send(Event::WorkspaceChanged {
                    project: f.project,
                    workspace,
                })
                .unwrap();
        }
    }
    cx.run_until_parked();
    for cmd in drain(&mut f.commands) {
        if let Command::QueryRepeaterHistory {
            project,
            query_revision,
            tab_id,
            start,
            ..
        } = cmd
        {
            f.events
                .try_send(Event::RepeaterHistoryPage {
                    project,
                    query_revision,
                    tab_id,
                    result: Ok(RepeaterHistoryPage {
                        total: 1,
                        start,
                        current_position: Some(0),
                        rows: Vec::new(),
                        next_offset: None,
                    }),
                })
                .unwrap();
        }
    }
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let tool_bounds = window.find("tool-repeater").bounds();
        let strip_bounds = window.find("work-tabs").bounds();
        assert!(window.try_find("repeater-cancel").is_none());
        let idle_send_bounds = window.find("repeater-send").bounds();
        assert_eq!(idle_send_bounds, send_bounds);
        let follow_bounds = window.find("repeater-follow-redirection").bounds();
        assert_eq!(
            strip_bounds.origin.y,
            tool_bounds.bottom(),
            "inline follow redirection must not push work-tabs down"
        );
        assert_eq!(follow_bounds.center().y, idle_send_bounds.center().y);
        assert!(follow_bounds.origin.x >= idle_send_bounds.right());
    })
    .unwrap();
}

#[gpui_kit::test]
fn repeater_sequential_group_send_advances_one_by_one_and_stops_on_cancel(
    cx: &mut TestAppContext,
) {
    let group = TabGroup {
        id: Uuid::new_v4(),
        title: "SeqGroup".into(),
        collapsed: false,
        color: Some(3),
    };
    let mut t1 = tab("S1");
    let mut t2 = tab("S2");
    let mut t3 = tab("S3");
    t1.group = Some(group.id);
    t2.group = Some(group.id);
    t3.group = Some(group.id);
    let id1 = t1.id;
    let id2 = t2.id;
    let id3 = t3.id;

    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    f.events
        .try_send(Event::ProjectOpened {
            project: f.project,
            path: None,
            workspace: Workspace {
                revision: 1,
                groups: vec![group.clone()],
                tabs: vec![t1, t2, t3],
                active: Some(id1),
                ..Default::default()
            },
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    let _ = acknowledge_history_query(&f, cmds, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("repeater-send-mode-menu").visible());
        window.click("repeater-send-mode-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("顺序发送本组全部请求 (Sequential)")
            .bounds()
            .center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    // Only the first tab in the group should be dispatched initially.
    let first_batch: Vec<(TabId, Uuid, u64)> = drain(&mut f.commands)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Command::Send {
                tab_id,
                run_id,
                draft,
                ..
            } => Some((tab_id, run_id, draft.revision)),
            _ => None,
        })
        .collect();
    assert_eq!(first_batch.len(), 1);
    assert_eq!(first_batch[0].0, id1);

    // Finish tab 1 -> tab 2 must automatically dispatch next!
    let mut rec1 = crate::workbench::tests::record("http://127.0.0.1:9001/", "");
    rec1.summary.id = first_batch[0].1;
    rec1.summary.status = Some(200);
    rec1.summary.state = FlowState::Complete;
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id: id1,
            draft_revision: first_batch[0].2,
            record: Box::new(rec1),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();

    // Acknowledge workspace save from tab 1 completion so pending send for tab 2 releases.
    for cmd in drain(&mut f.commands) {
        if let Command::SaveWorkspace { workspace, .. } = cmd {
            f.events
                .try_send(Event::WorkspaceChanged {
                    project: f.project,
                    workspace,
                })
                .unwrap();
        }
    }
    cx.run_until_parked();

    let second_batch: Vec<(TabId, Uuid, u64)> = drain(&mut f.commands)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Command::Send {
                tab_id,
                run_id,
                draft,
                ..
            } => Some((tab_id, run_id, draft.revision)),
            _ => None,
        })
        .collect();
    assert_eq!(second_batch.len(), 1);
    assert_eq!(second_batch[0].0, id2);

    // Cancel while tab 2 is in-flight -> tab 3 must NOT be sent after rejection!
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-cancel", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cancel_cmds = drain(&mut f.commands);
    assert!(
        cancel_cmds
            .iter()
            .any(|cmd| matches!(cmd, Command::CancelSend { tab_id, .. } if *tab_id == id2))
    );
    f.events
        .try_send(Event::SendRejected {
            project: f.project,
            tab_id: id2,
            run_id: second_batch[0].1,
            draft_revision: second_batch[0].2,
            message: "用户已取消".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        !drain(&mut f.commands)
            .iter()
            .any(|cmd| matches!(cmd, Command::Send { tab_id, .. } if *tab_id == id3)),
        "cancelling sequential queue must abort remaining queued tabs"
    );
}

#[gpui_kit::test]
fn repeater_target_helper_syncs_from_host_and_toggles_scheme_with_host_header(
    cx: &mut TestAppContext,
) {
    let mut initial = tab("TargetHelper");
    initial.draft.target = String::new();
    initial.draft.raw = b"GET /v1/ping HTTP/1.1\r\nHost: api.internal.local:8080\r\n\r\n".to_vec();
    initial.history = Some(RepeaterHistory {
        source: Some(RepeaterHistorySource {
            source_id: Uuid::new_v4(),
            parent_item_id: Uuid::new_v4(),
        }),
        cursor: Some(RepeaterHistoryCursor {
            entry: RepeaterHistoryEntryId::Archive(2),
            draft_revision: 1,
        }),
    });
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    for cmd in drain(&mut f.commands) {
        if let Command::QueryRepeaterHistory {
            project,
            query_revision,
            tab_id,
            start,
            ..
        } = cmd
        {
            f.events
                .try_send(Event::RepeaterHistoryPage {
                    project,
                    query_revision,
                    tab_id,
                    result: Ok(RepeaterHistoryPage {
                        total: 3,
                        start,
                        current_position: Some(2),
                        rows: vec![
                            RepeaterHistoryEntry {
                                id: RepeaterHistoryEntryId::Archive(0),
                                position: 0,
                                method: Some("POST".into()),
                                target: Some("https://api.example.com/login".into()),
                                status: Some(200),
                                diagnostic: None,
                            },
                            RepeaterHistoryEntry {
                                id: RepeaterHistoryEntryId::Archive(1),
                                position: 1,
                                method: Some("GET".into()),
                                target: Some("https://api.example.com/timeout".into()),
                                status: None,
                                diagnostic: Some("连接超时".into()),
                            },
                            RepeaterHistoryEntry {
                                id: RepeaterHistoryEntryId::Archive(2),
                                position: 2,
                                method: Some("GET".into()),
                                target: Some("http://api.internal.local:8080/v1/ping".into()),
                                status: Some(200),
                                diagnostic: None,
                            },
                        ],
                        next_offset: None,
                    }),
                })
                .unwrap();
        }
    }
    cx.run_until_parked();

    // Verify history dropdown menu renders full Method, Target, Status and Diagnostic.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-history-previous-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find_by_label("2. GET https://api.example.com/timeout [连接超时]")
                .visible()
        );
        assert!(
            window
                .find_by_label("1. POST https://api.example.com/login [200]")
                .visible()
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-target-label").label(),
            Some("Target: ")
        );
        let edit_btn = window.find("repeater-edit-target");
        assert_eq!(edit_btn.bounds().size.width, px(22.));
        assert_eq!(edit_btn.bounds().size.height, px(24.));
        let help_btn = window.find("repeater-help");
        assert_eq!(help_btn.bounds().size.width, px(22.));
        assert_eq!(help_btn.bounds().size.height, px(24.));
        window.click("repeater-edit-target", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let dialog = cx.read(|cx| {
        *cx.windows()
            .iter()
            .find(|&&w| w != f.window)
            .expect("Repeater target dialog window")
    });
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("repeater-target-dialog").visible());
        window.click("repeater-target-sync-host", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-target-input").value(),
            Some("http://api.internal.local:8080")
        );
        window.click("repeater-target-cancel", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| cx.windows().contains(&dialog)));
    assert!(
        drain(&mut f.commands).is_empty(),
        "cancelling target dialog after Host fill must not save workspace"
    );
    cx.read(|cx| {
        assert!(f.view.read(cx).workspace.tabs[0].draft.target.is_empty());
    });

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-edit-target", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = cx.read(|cx| {
        *cx.windows()
            .iter()
            .find(|&&w| w != f.window)
            .expect("Repeater target dialog window")
    });
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-target-sync-host", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-target-input").value(),
            Some("http://api.internal.local:8080")
        );
        window.click("repeater-target-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("http://example.org:80", cx);
        window.click("repeater-target-toggle-scheme", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-target-input").value(),
            Some("https://example.org:443")
        );
        assert!(
            f.view.read(cx).workspace.tabs[0].draft.target.is_empty(),
            "scheme toggle in dialog must stay local until OK"
        );
        window.click("repeater-target-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let saved_target = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("submitting target dialog must save workspace");
    assert_eq!(saved_target.tabs[0].draft.target, "https://example.org:443");
    let raw_text = String::from_utf8_lossy(&saved_target.tabs[0].draft.raw);
    assert!(raw_text.contains("Host: example.org\r\n"));
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: saved_target,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| cx.windows().contains(&dialog)));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-target-label").label(),
            Some("Target: https://example.org")
        );
        window.click("repeater-edit-target", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let dialog = cx.read(|cx| {
        *cx.windows()
            .iter()
            .find(|&&w| w != f.window)
            .expect("Repeater target dialog window")
    });
    // IPv6 scheme toggle + IME cancel block + WorkspaceRejected retention.
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-target-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("http://[::1]:80", cx);
        window.click("repeater-target-toggle-scheme", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-target-input").value(),
            Some("https://[::1]:443")
        );
        window.click("repeater-target-update-host", cx);
        window.click("repeater-target-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let rejected_ws = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("SaveWorkspace for IPv6 target");
    assert_eq!(rejected_ws.tabs[0].draft.target, "https://[::1]:443");
    let raw_before_reject = String::from_utf8_lossy(&rejected_ws.tabs[0].draft.raw);
    assert!(
        raw_before_reject.contains("Host: example.org\r\n"),
        "disabling repeater-target-update-host must preserve existing Host header"
    );
    f.events
        .try_send(Event::WorkspaceRejected {
            project: f.project,
            revision: rejected_ws.revision,
            stored: Workspace {
                revision: rejected_ws.revision - 1,
                tabs: rejected_ws.tabs.clone(),
                active: rejected_ws.active,
                ..Default::default()
            },
            message: "磁盘空间不足".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        cx.read(|cx| cx.windows().contains(&dialog)),
        "WorkspaceRejected must keep target dialog open with error"
    );
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("repeater-target-status")
                .label()
                .unwrap_or_default()
                .contains("磁盘空间不足")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn new_ws_tab_menu_control_frames_handshake_headers_history_actions_and_unsaved_close_confirm(
    cx: &mut TestAppContext,
) {
    let initial = tab("DirtyClose");
    let http_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let cmds = drain(&mut f.commands);
    let _ = acknowledge_history_query(&f, cmds, cx);

    // Verify unsaved draft close confirmation when enabled.
    crate::workbench::tabs::actions::set_force_close_unsaved_prompt_in_test(true);
    cx.update_window(f.window, |_, window, cx| {
        let req = f.view.read(cx).tab_editors[&http_id].request.raw.clone();
        window.click(("input", req.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("unsaved-body", cx);
        window.click(format!("close-tab-{http_id}"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "closing tab with unsaved live edit must prompt when confirmation is active"
    );
    cx.simulate_prompt_answer("保留标签");
    cx.run_until_parked();
    crate::workbench::tabs::actions::set_force_close_unsaved_prompt_in_test(false);
    cx.read(|cx| {
        assert_eq!(f.view.read(cx).workspace.tabs.len(), 1);
    });
    for cmd in drain(&mut f.commands) {
        if let Command::SaveWorkspace { workspace, .. } = cmd {
            f.events
                .try_send(Event::WorkspaceChanged {
                    project: f.project,
                    workspace,
                })
                .unwrap();
        }
    }
    cx.run_until_parked();

    // Create standalone WebSocket Repeater tab via right-click context menu on #repeater-new.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-new-kind").is_none());
        assert!(window.find("repeater-new").visible());
        window.right_click("repeater-new", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window
            .find_by_label("新建 WebSocket 会话标签页")
            .bounds()
            .center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let open_ws = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::WebSocketRepeater(WebSocketRepeaterCommand::Open {
                operation_id,
                url,
                title,
                ..
            }) => Some((operation_id, url, title)),
            _ => None,
        })
        .expect("selecting new WebSocket tab must emit WebSocketRepeaterCommand::Open");
    assert_eq!(open_ws.1, "wss://localhost:8443/");
    let ws_id = Uuid::new_v4();
    let history_item = WebSocketRepeaterHistoryItem {
        id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        flow_id: None,
        sequence: 1,
        direction: WebSocketDirection::ServerToClient,
        kind: WebSocketKind::Text,
        payload: b"hello-ws".to_vec(),
        observed_unix_ms: 1000,
    };
    let hist_id = history_item.id;
    f.events
        .try_send(Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
            project: f.project,
            operation_id: open_ws.0,
            result: Ok(WebSocketRepeaterTab {
                id: ws_id,
                title: open_ws.2.unwrap_or_else(|| "WS 1".into()),
                group: None,
                url: open_ws.1,
                handshake_headers: Vec::new(),
                source_flow_id: None,
                source_message_id: None,
                direction: WebSocketDirection::ClientToServer,
                draft_revision: 1,
                draft_payload: Vec::new(),
                draft_kind: WebSocketKind::Text,
                history: vec![history_item],
                closed: false,
            }),
            mode: WebSocketRepeaterConnectionMode::Disconnected,
        }))
        .unwrap();
    cx.run_until_parked();
    for cmd in drain(&mut f.commands) {
        if let Command::SaveWorkspace { workspace, .. } = cmd {
            f.events
                .try_send(Event::WorkspaceChanged {
                    project: f.project,
                    workspace,
                })
                .unwrap();
        }
    }
    cx.run_until_parked();

    // Verify Ping/Pong/Close frame buttons, handshake headers panel, auto-follow toggle, and history actions.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("ws-repeater-kind-ping").visible());
        assert!(window.find("ws-repeater-kind-pong").visible());
        assert!(window.find("ws-repeater-kind-close").visible());
        window.click("ws-repeater-kind-ping", cx);
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).workspace.websocket_tabs[0].draft_kind,
            WebSocketKind::Ping
        );

        window.click("ws-repeater-headers-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("ws-repeater-headers-panel").visible());
        f.view.update(cx, |view, cx| {
            let ed = &view.websocket_repeater.editors[&ws_id];
            ed.header_name
                .update(cx, |s, cx| s.set_value("Authorization", window, cx));
            ed.header_value
                .update(cx, |s, cx| s.set_value("Bearer secret-ws", window, cx));
        });
        window.click("ws-repeater-header-add", cx);
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).workspace.websocket_tabs[0]
                .handshake_headers
                .len(),
            1
        );

        // Disable auto-follow and receive a second frame; selection must stay on first frame.
        window.click("ws-repeater-auto-follow", cx);
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);

    let incoming = WebSocketRepeaterHistoryItem {
        id: Uuid::new_v4(),
        message_id: Uuid::new_v4(),
        flow_id: None,
        sequence: 2,
        direction: WebSocketDirection::ServerToClient,
        kind: WebSocketKind::Pong,
        payload: b"pong-2".to_vec(),
        observed_unix_ms: 2000,
    };
    f.events
        .try_send(Event::WebSocketRepeater(WebSocketRepeaterEvent::Received {
            project: f.project,
            tab_id: ws_id,
            flow_id: Uuid::new_v4(),
            item: incoming,
        }))
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).websocket_repeater.editors[&ws_id].selected_history,
            Some(hist_id),
            "disabling auto-follow must keep existing selected history frame"
        );
    });

    // Copy Hex and send history payload to Decoder & Comparer.
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.copy_ws_repeater_history_payload(ws_id, hist_id, true, cx);
            view.send_ws_repeater_history_to_comparer(ws_id, hist_id, window, cx);
            view.send_ws_repeater_history_to_decoder(ws_id, hist_id, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let hex_clip = cx
        .read(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text());
    assert_eq!(hex_clip.as_deref(), Some("68 65 6c 6c 6f 2d 77 73"));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.tool, Tool::Decoder);
        assert_eq!(
            view.extra_tools.decoder_input.read(cx).value().as_ref(),
            "hello-ws"
        );
        assert_eq!(view.extra_tools.comparer_items.len(), 1);
    });
}

#[test]
fn repeater_raw_response_byte_len_matches_response_to_raw_across_variants() {
    let field = |name: &str, value: &[u8]| Header {
        name: name.into(),
        value: value.to_vec(),
    };
    let ext_sample = HttpResponse {
        status: 200,
        version: HttpVersion::Http2,
        reason_phrase: Some(b"OK".to_vec()),
        headers: vec![
            field("Date", b"Tue, 17 Nov 2026 12:17:54 GMT"),
            field("Content-Type", b"application/json"),
            field("Content-Length", b"49"),
            field("Vary", b"Accept-Encoding"),
            field("X-Retcode", b"SUCCESS"),
            field(
                "Access-Control-Allow-Headers",
                b"Authorization,x-appkey,x-app-ver,x-zone,x-uid,Content-Type,Content-Length,x-trace,x-real-ip,x-ap-v,x-ap-act,X-Sample-TraceId",
            ),
            field("Access-Control-Allow-Credentials", b"true"),
            field("Access-Control-Allow-Origin", b"*"),
            field(
                "Access-Control-Allow-Methods",
                b"GET, POST, PUT, DELETE, OPTIONS, PATCH",
            ),
            field("Server", b"MIO-Mock/Server"),
            field(
                "X-Pc-Lowpower-Mark",
                b"default_ratio_eq_zero_app_false_mark_0",
            ),
            field("X-Sample-Power-Policy", b"0"),
            field(
                "X-Sample-Requestd",
                b"demo-gateway-node-0001^^00000000000000000000000000000000|1700000000000",
            ),
            field("X-Node-RequestId", b"000000000000000000000000000000"),
            field("Strict-Transport-Security", b"max-age=0"),
        ],
        body: br#"{"api":"","data":{},"ret":["SUCCESS::ok"],"v":""}"#.to_vec(),
        trailers: Vec::new(),
    };
    assert_eq!(ext_sample.body.len(), 49);
    assert_eq!(raw_response_byte_len(&ext_sample), 786);
    assert_eq!(
        raw_response_byte_len(&ext_sample),
        response_to_raw(&ext_sample).len()
    );

    let variants = [
        HttpResponse {
            status: 204,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![field("Connection", b"close")],
            body: Vec::new(),
            trailers: Vec::new(),
        },
        HttpResponse {
            status: 201,
            version: HttpVersion::Http11,
            reason_phrase: Some(b"Created".to_vec()),
            headers: vec![
                field("Content-Type", b"text/plain"),
                field("Content-Length", b"16"),
            ],
            body: b"0123456789abcdef".to_vec(),
            trailers: vec![field("X-Checksum", b"abc123")],
        },
        HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: Some(b"".to_vec()),
            headers: vec![field("Transfer-Encoding", b"chunked")],
            body: Vec::new(),
            trailers: vec![field("X-Empty-Trailer", b"1")],
        },
    ];
    for resp in &variants {
        assert_eq!(raw_response_byte_len(resp), response_to_raw(resp).len());
    }
}

#[gpui_kit::test]
fn repeater_footer_stats_reports_full_raw_response_bytes_not_body_only(cx: &mut TestAppContext) {
    let initial = tab("FooterBytes");
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-send").bounds().size,
            gpui_kit::size(px(65.), px(24.))
        );
        assert_eq!(
            window.find("repeater-settings-menu").bounds().size,
            gpui_kit::size(px(24.), px(24.))
        );
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let cmds = drain(&mut f.commands);
    let cmds = acknowledge_history_query(&f, cmds, cx);
    let (run_id, sent_rev) = cmds
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send { run_id, draft, .. } => Some((run_id, draft.revision)),
            _ => None,
        })
        .expect("Repeater Send command");

    let field = |name: &str, value: &[u8]| Header {
        name: name.into(),
        value: value.to_vec(),
    };
    let response = HttpResponse {
        status: 200,
        version: HttpVersion::Http2,
        reason_phrase: Some(b"OK".to_vec()),
        headers: vec![
            field("Date", b"Tue, 17 Nov 2026 12:17:54 GMT"),
            field("Content-Type", b"application/json"),
            field("Content-Length", b"49"),
            field("Vary", b"Accept-Encoding"),
            field("X-Retcode", b"SUCCESS"),
            field(
                "Access-Control-Allow-Headers",
                b"Authorization,x-appkey,x-app-ver,x-zone,x-uid,Content-Type,Content-Length,x-trace,x-real-ip,x-ap-v,x-ap-act,X-Sample-TraceId",
            ),
            field("Access-Control-Allow-Credentials", b"true"),
            field("Access-Control-Allow-Origin", b"*"),
            field(
                "Access-Control-Allow-Methods",
                b"GET, POST, PUT, DELETE, OPTIONS, PATCH",
            ),
            field("Server", b"MIO-Mock/Server"),
            field(
                "X-Pc-Lowpower-Mark",
                b"default_ratio_eq_zero_app_false_mark_0",
            ),
            field("X-Sample-Power-Policy", b"0"),
            field(
                "X-Sample-Requestd",
                b"demo-gateway-node-0001^^00000000000000000000000000000000|1700000000000",
            ),
            field("X-Node-RequestId", b"000000000000000000000000000000"),
            field("Strict-Transport-Security", b"max-age=0"),
        ],
        body: br#"{"api":"","data":{},"ret":["SUCCESS::ok"],"v":""}"#.to_vec(),
        trailers: Vec::new(),
    };
    let mut rec = crate::workbench::tests::record("https://gateway.example.test/", "");
    rec.summary.id = run_id;
    rec.summary.status = Some(200);
    rec.summary.state = FlowState::Complete;
    rec.summary.response_bytes = response.body.len() as u64;
    rec.summary.elapsed_ms = 104;
    rec.response = Some(response);

    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id,
            draft_revision: sent_rev,
            record: Box::new(rec),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    for cmd in drain(&mut f.commands) {
        if let Command::SaveWorkspace { workspace, .. } = cmd {
            f.events
                .try_send(Event::WorkspaceChanged {
                    project: f.project,
                    workspace,
                })
                .unwrap();
        }
    }
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-footer-stats").label(),
            Some("786 bytes | 104 millis")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn repeater_toolbar_mio_ai_button_and_split_history_geometry(cx: &mut TestAppContext) {
    let initial = tab("ToolbarParity");
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-new-kind").is_none());
        let prev_bounds = window.find("repeater-history-previous").bounds();
        let prev_menu_bounds = window.find("repeater-history-previous-menu").bounds();
        let next_bounds = window.find("repeater-history-next").bounds();
        let next_menu_bounds = window.find("repeater-history-next-menu").bounds();
        assert_eq!(prev_bounds.size, gpui_kit::size(px(30.), px(24.)));
        assert_eq!(prev_menu_bounds.size, gpui_kit::size(px(17.), px(24.)));
        assert_eq!(next_bounds.size, gpui_kit::size(px(30.), px(24.)));
        assert_eq!(next_menu_bounds.size, gpui_kit::size(px(17.), px(24.)));

        let ai_btn = window.find("repeater-mio-ai");
        let ai_bounds = ai_btn.bounds();
        assert_eq!(ai_btn.label(), Some("MioAI"));
        assert_eq!(ai_bounds.size, gpui_kit::size(px(104.), px(24.)));
        assert!(ai_bounds.origin.x > next_menu_bounds.right());
        assert_eq!(ai_bounds.center().y, next_menu_bounds.center().y);

        window.click("repeater-mio-ai", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(f.view.read(cx).mio_ai_panel_open);
        assert!(f.view.read(cx).mio_ai_context.is_some());
    });
}

#[test]
fn bare_repeater_target_normalization_adds_scheme_and_respects_https_context() {
    use super::normalize_repeater_target_input;

    assert_eq!(
        normalize_repeater_target_input("127.0.0.1:8080", "http://127.0.0.1:9001").unwrap(),
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        normalize_repeater_target_input("example.invalid:443", "http://127.0.0.1:8080").unwrap(),
        "https://example.invalid:443"
    );
    assert_eq!(
        normalize_repeater_target_input("[::1]:8080", "http://127.0.0.1:8080").unwrap(),
        "http://[::1]:8080"
    );
    assert_eq!(
        normalize_repeater_target_input("example.invalid:8443", "https://example.invalid:443")
            .unwrap(),
        "https://example.invalid:8443"
    );
    assert_eq!(
        normalize_repeater_target_input("http://example.invalid:8080", "https://example.invalid:443")
            .unwrap(),
        "http://example.invalid:8080"
    );
}
