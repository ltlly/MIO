use super::*;
use crate::workbench::tests::{drain, fixture, tab};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestWindowExt};
use mio_core::{
    FlowProvenance, FlowState, FlowSummary, Header, HttpRequest, ProvenanceActor,
    ResponseProvenanceStep, TrafficSource, Uuid,
};

#[gpui_kit::test]
fn editing_repeater_nonprintable_render_view_and_pretty_rewrite_preserve_wire_bytes(
    cx: &mut TestAppContext,
) {
    let mut initial = tab("视图与改写");
    initial.draft.target = "http://127.0.0.1:9001".into();
    initial.draft.raw = concat!(
        "POST /api/login HTTP/1.1\r\n",
        "Host: 127.0.0.1:9001\r\n",
        "Content-Type: application/json\r\n",
        "Content-Length: 27\r\n\r\n",
        "{\"user\":\"alice\",\"pin\":\"09\"}"
    )
    .as_bytes()
    .to_vec();
    let orig_wire = initial.draft.raw.clone();
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        window.click("repeater-request-nonprintable", cx);
        window.render_frame(cx);
        let actual = f.view.update(cx, |view, cx| {
            let editor = &view.tab_editors[&tab_id].request;
            assert!(editor.show_non_printable());
            assert!(editor.state().read(cx).value().contains(" \\r   \\n "));
            editor.bytes(window, cx).unwrap()
        });
        assert_eq!(actual, orig_wire);

        window.click("repeater-request-nonprintable", cx);
        window.render_frame(cx);
        let restored = f.view.update(cx, |view, cx| {
            let editor = &view.tab_editors[&tab_id].request;
            assert!(!editor.show_non_printable());
            editor.bytes(window, cx).unwrap()
        });
        assert_eq!(restored, orig_wire);

        window.click("repeater-request-pretty", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).tab_editors[&tab_id].request.pretty_active());
        window.dispatch_action(Box::new(crate::ChangeRequestMethod), cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let rewritten = f.view.update(cx, |view, cx| {
            view.tab_editors[&tab_id].request.bytes(window, cx).unwrap()
        });
        let rewritten_str = std::str::from_utf8(&rewritten).unwrap();
        assert!(
            rewritten_str.starts_with("GET /api/login?user=alice&pin=09 HTTP/1.1\r\n"),
            "expected Pretty rewrite to flatten JSON into GET query: {rewritten_str}"
        );
    })
    .unwrap();
    let _ = drain(&mut f.commands);

    let flow_id = Uuid::new_v4();
    let req_obj = HttpRequest {
        method: "GET".into(),
        url: "http://127.0.0.1:9001/api/login".into(),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "Host".into(),
            value: b"127.0.0.1:9001".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    let orig_resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Encoding".into(),
            value: b"gzip".to_vec(),
        }],
        body: b"compressed-raw".to_vec(),
        trailers: Vec::new(),
    };
    let final_resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"text/html; charset=utf-8".to_vec(),
        }],
        body: b"<html><head><title>Portal</title></head><body><a href=\"/next\">Next</a></body></html>"
            .to_vec(),
        trailers: Vec::new(),
    };
    let record = FlowRecord {
        summary: FlowSummary {
            id: flow_id,
            revision: 1,
            source: TrafficSource::Repeater,
            method: "GET".into(),
            url: "http://127.0.0.1:9001/api/login".into(),
            status: Some(200),
            state: FlowState::Complete,
            request_bytes: 64,
            response_bytes: 96,
            started_unix_ms: 0,
            elapsed_ms: 12,
        },
        request: req_obj,
        response: Some(final_resp.clone()),
    };
    let original = OriginalMessages {
        request: None,
        response: Some(orig_resp.clone()),
        response_body_complete: Some(true),
        provenance: FlowProvenance {
            request_steps: Vec::new(),
            response_steps: vec![
                ResponseProvenanceStep {
                    actor: ProvenanceActor::Original,
                    label: "原始压缩响应".into(),
                    response: orig_resp,
                },
                ResponseProvenanceStep {
                    actor: ProvenanceActor::UnpackCompressed,
                    label: "自动解包后响应".into(),
                    response: final_resp,
                },
            ],
        },
    };

    cx.update_window(f.window, |_, _, cx| {
        f.view.update(cx, |view, _| {
            view.workspace.tabs[0].last_flow = Some(flow_id);
            view.repeater_reads.insert(99, tab_id);
        });
    })
    .unwrap();
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: 99,
            record: Box::new(record),
            original: Box::new(original),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("repeater-response-version").is_some(),
            "Repeater response provenance dropdown must appear when provenance steps > 1"
        );
        window.click("repeater-response-render", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-response-render-card").is_some());
        assert!(
            window
                .try_find("repeater-response-render-open-browser")
                .is_some()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn ext_message_editor_two_row_header_search_gear_menu_and_send_cancel_coplace(
    cx: &mut TestAppContext,
) {
    let mut initial = tab("1");
    initial.draft.target = "http://127.0.0.1:9001".into();
    initial.draft.raw = b"GET /?q=Alpha&x=alpha HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("inspector-toggle", cx);
        window.render_frame(cx);

        // 1. Send and Cancel co-placement: when idle, only repeater-send exists.
        let send_bounds = window.find("repeater-send").bounds();
        assert_eq!(send_bounds.size.width, px(65.));
        assert_eq!(send_bounds.size.height, px(24.));
        assert!(window.try_find("repeater-cancel").is_none());

        // 2. Two-row header geometry and right-aligned wrap / non-printable controls.
        let pretty_bounds = window.find("repeater-request-pretty").bounds();
        let raw_bounds = window.find("repeater-request-raw").bounds();
        let hex_bounds = window.find("repeater-request-hex").bounds();
        let wrap_bounds = window.find("repeater-request-wrap").bounds();
        let np_bounds = window.find("repeater-request-nonprintable").bounds();
        assert_eq!(pretty_bounds.size.width, px(64.));
        assert_eq!(raw_bounds.size.width, px(56.));
        assert_eq!(hex_bounds.size.width, px(53.));
        assert_eq!(pretty_bounds.size.height, px(37.));
        assert_eq!(wrap_bounds.size.width, px(19.));
        assert_eq!(np_bounds.size.width, px(19.));
        assert!(wrap_bounds.origin.x > hex_bounds.right() + px(40.));
        assert!(np_bounds.origin.x > wrap_bounds.right());

        // 3. Layout switcher resides in repeater-response first row, above Render tab.
        let side_bounds = window.find("repeater-layout-side").bounds();
        let stack_bounds = window.find("repeater-layout-stack").bounds();
        let combined_bounds = window.find("repeater-layout-combined").bounds();
        let render_bounds = window.find("repeater-response-render").bounds();
        assert_eq!(side_bounds.size.width, px(25.));
        assert_eq!(stack_bounds.size.width, px(25.));
        assert_eq!(combined_bounds.size.width, px(25.));
        assert_eq!(side_bounds.size.height, px(19.));
        assert_eq!(render_bounds.size.width, px(72.));
        assert!(side_bounds.bottom() <= render_bounds.origin.y);

        // 4. Bottom search bar: case/regex/replace are tucked inside the gear menu.
        let search_bar = window.find("repeater-request-search-bar").bounds();
        assert_eq!(search_bar.size.height, px(47.));
        assert!(window.try_find("repeater-request-search-case").is_none());
        assert!(window.try_find("repeater-request-search-regex").is_none());
        assert!(window.try_find("repeater-request-search-replace-toggle").is_none());
        let prev_bounds = window.find("repeater-request-search-prev").bounds();
        let next_bounds = window.find("repeater-request-search-next").bounds();
        assert_eq!(prev_bounds.size.width, px(24.));
        assert_eq!(next_bounds.size.width, px(24.));

        // Open gear menu to reveal case/regex/replace options.
        window.click("repeater-request-search-options", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-request-search-case").is_some());
        assert!(window.try_find("repeater-request-search-regex").is_some());
        assert!(window.try_find("repeater-request-search-replace-toggle").is_some());
        window.click("repeater-request-search-case", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).tab_editors[&tab_id].request.case_sensitive());

        // 5. Inspector sidebar width and 39px section headers.
        let inspector_bounds = window.find("inspector-sidebar").bounds();
        assert_eq!(inspector_bounds.size.width, px(343.));
        let expand_all = window.find("inspector-expand-all").bounds();
        assert_eq!(expand_all.size.width, px(18.));
        assert_eq!(expand_all.size.height, px(18.));

        // 6. Click Send -> co-placed button switches to repeater-cancel.
        window.click("repeater-send", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
        let cancel_bounds = window.find("repeater-cancel").bounds();
        assert_eq!(cancel_bounds.origin, send_bounds.origin);
        assert_eq!(cancel_bounds.size, send_bounds.size);
        window.click("repeater-cancel", cx);
    })
    .unwrap();
    let cancel_cmd = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::CancelSend {
                tab_id: t,
                run_id,
                draft_revision,
                ..
            } if t == tab_id => Some((run_id, draft_revision)),
            _ => None,
        })
        .expect("clicking Cancel must enqueue CancelSend");
    f.events
        .try_send(Event::SendRejected {
            project: f.project,
            tab_id,
            run_id: cancel_cmd.0,
            draft_revision: cancel_cmd.1,
            message: "用户已取消".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_some());
        assert!(window.try_find("repeater-cancel").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn history_message_actions_support_scan_launcher_selection_copy_save_and_browser_preview(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    let initial = drain(&mut f.commands);
    let query_revision = initial
        .iter()
        .find_map(|cmd| match cmd {
            Command::QueryHttpHistory { query } => Some(query.query_revision),
            _ => None,
        })
        .expect("initial history query");

    let mut flow = crate::workbench::tests::record(
        "https://portal.example.com/account?token=secret123",
        "{\"role\":\"admin\"}",
    );
    flow.request.url = "https://portal.example.com/account?token=secret123".into();
    flow.summary.status = Some(200);
    flow.response = Some(HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"text/html; charset=utf-8".to_vec(),
        }],
        body: b"<html><body><a href=\"/settings\">Settings</a></body></html>".to_vec(),
        trailers: Vec::new(),
    });
    let flow_id = flow.summary.id;

    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![HistoryRow::Live(flow.summary.clone())],
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.proxy_page = ProxyPage::History;
            view.select_flow(flow_id, window, cx);
        });
    })
    .unwrap();
    let sel_rev = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ReadFlow {
                selection_revision, ..
            } => Some(selection_revision),
            _ => None,
        })
        .expect("read flow command");
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: sel_rev,
            record: Box::new(flow),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();

    let out_dir = std::env::current_dir()
        .unwrap()
        .join(".local")
        .join(format!("test-msg-actions-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let sel_path = out_dir.join("selected-token.txt");

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-layout-combined", cx);
        window.render_frame(cx);
        assert!(window.find("history-combined-container").visible());
        let req_tab_bounds = window.find("history-combined-req").bounds();
        let resp_tab_bounds = window.find("history-combined-resp").bounds();
        let pretty_bounds = window.find("history-request-pretty").bounds();
        let layout_side_bounds = window.find("history-layout-side").bounds();
        assert_eq!(req_tab_bounds.size.width, px(90.));
        assert_eq!(req_tab_bounds.size.height, px(39.));
        assert_eq!(resp_tab_bounds.size.width, px(102.));
        assert_eq!(resp_tab_bounds.size.height, px(39.));
        assert_eq!(resp_tab_bounds.origin.x, req_tab_bounds.right());
        assert_eq!(pretty_bounds.origin.y, req_tab_bounds.bottom());
        assert!(layout_side_bounds.origin.x > resp_tab_bounds.right());
        window.click("history-combined-resp", cx);
        window.render_frame(cx);
        assert_eq!(window.find("history-combined-resp").checked(), Some(true));
        assert!(window.try_find("history-response-render").is_some());
        window.click("history-combined-req", cx);
        window.render_frame(cx);
        assert_eq!(window.find("history-combined-req").checked(), Some(true));
        window.click("history-layout-side", cx);
        window.render_frame(cx);
        f.view.update(cx, |view, cx| {
            view.copy_selected_editor_text(EditorSlot::HistoryRequest, window, cx);
        });
        assert!(
            f.view
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|e| e.contains("请先在消息编辑器中选中"))
        );

        f.view.update(cx, |view, cx| {
            let req_state = view.history_request.state();
            let text = req_state.read(cx).value().to_string();
            let start = text.find("secret123").expect("token in request");
            req_state.update(cx, |s, cx| {
                s.set_selected_range_with_direction(start..start + 9, false, cx);
            });
            view.copy_selected_editor_text(EditorSlot::HistoryRequest, window, cx);
        });
        assert!(f.view.read(cx).error.is_none());
    })
    .unwrap();
    assert_eq!(
        cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
        Some("secret123".into())
    );

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.save_selected_text_to_file(EditorSlot::HistoryRequest, window, cx);
        });
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(sel_path.clone()));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&sel_path).unwrap(), "secret123");
    let _ = std::fs::remove_dir_all(&out_dir);

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.open_current_response_in_browser(window, cx);
        });
        assert!(f.view.read(cx).error.is_none());
        assert!(f.view.read(cx).status.contains("已在系统浏览器中打开响应预览"));

        f.view.update(cx, |view, cx| {
            view.open_scan_launcher(window, cx);
        });
        assert_eq!(f.view.read(cx).tool, Tool::Dashboard);
        assert!(f.view.read(cx).audit_dashboard.new_scan_open);
        assert_eq!(
            f.view.read(cx).audit_dashboard.scan_url_input.read(cx).value(),
            "https://portal.example.com/account?token=secret123"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn message_actions_menu_follows_ext_group_order_height_cap_shortcuts_and_single_dispatch(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    let initial = drain(&mut f.commands);
    let query_revision = initial
        .iter()
        .find_map(|cmd| match cmd {
            Command::QueryHttpHistory { query } => Some(query.query_revision),
            _ => None,
        })
        .expect("initial history query");

    let mut flow = crate::workbench::tests::record(
        "https://portal.example.com/account?token=secret123",
        "{\"role\":\"admin\"}",
    );
    flow.request.url = "https://portal.example.com/account?token=secret123".into();
    flow.summary.status = Some(200);
    let flow_id = flow.summary.id;

    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision,
            offset: 0,
            total: 1,
            rows: vec![HistoryRow::Live(flow.summary.clone())],
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.proxy_page = ProxyPage::History;
            view.open_current_request_in_configured_browser(window, cx);
        });
        assert!(
            f.view
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|err| err.contains("已加载"))
        );
    })
    .unwrap();
    assert!(drain(&mut f.commands).is_empty());

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.select_flow(flow_id, window, cx);
        });
    })
    .unwrap();
    let sel_rev = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ReadFlow {
                selection_revision, ..
            } => Some(selection_revision),
            _ => None,
        })
        .expect("read flow command");
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: sel_rev,
            record: Box::new(flow),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-request-message-actions", cx);
        window.render_frame(cx);

        let popup_bounds = window.find("popup-menu").bounds();
        assert!(
            popup_bounds.size.height <= theme::scaled_px(420.),
            "message actions menu height {:?} must stay <= 420pt",
            popup_bounds.size.height
        );

        let menu = window.within("popup-menu");
        let ordered_labels = [
            "打开扫描启动器 (Scan)",
            "被动扫描当前消息 (Passive scan selected message)",
            "主动扫描当前消息 (Active scan selected message)",
            "发送到 Intruder",
            "发送到 Repeater",
            "发送到 Sequencer",
            "发送到 Comparer",
            "发送到 Decoder",
            "发送到 Organiser",
            "发送到 MioAT",
            "记录问题 (Record an issue)",
            "在浏览器中打开响应 (Open response in browser)",
            "在浏览器中发起请求 (Request in browser)",
            "辅助测试工具 (Engagement tools)",
            "复制 (Copy)",
            "复制美化报文 (Copy prettified)",
            "保存选中文本到文件… (Save selected text to file)",
            "复制 URL",
            "复制为 curl 命令",
            "提取页面链接 (Copy links in response)",
            "导出选中记录为 XML… (Save item)",
            "添加到站点地图 (Add to site map)",
        ];
        let mut prev_bottom = None;
        let separator_before = [
            "发送到 Intruder",
            "记录问题 (Record an issue)",
            "在浏览器中打开响应 (Open response in browser)",
            "复制 (Copy)",
            "复制 URL",
            "添加到站点地图 (Add to site map)",
        ];
        for label in ordered_labels {
            let item_bounds = if label == "发送到 Repeater" {
                menu.find("history-request-to-repeater").bounds()
            } else {
                menu.find_by_label(label).bounds()
            };
            assert_eq!(
                item_bounds.size.height,
                px(23.),
                "compact menu item '{label}' height must be 23px"
            );
            if let Some(prev) = prev_bottom {
                let expected_gap = if separator_before.contains(&label) {
                    px(1.)
                } else {
                    px(0.)
                };
                assert!(
                    (item_bounds.origin.y - (prev + expected_gap)).abs() <= px(0.5),
                    "menu item '{label}' at {:?} must follow {:?} with gap {:?}",
                    item_bounds.origin.y,
                    prev,
                    expected_gap
                );
            }
            prev_bottom = Some(item_bounds.bottom());
        }

        window.scroll(
            "popup-menu",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-240.))),
            cx,
        );
        window.render_frame(cx);
        let last_item_bounds = window
            .within("popup-menu")
            .find_by_label("添加到站点地图 (Add to site map)")
            .bounds();
        assert!(
            last_item_bounds.bottom() <= popup_bounds.bottom() + px(1.),
            "scrolling to the last menu item must bring it inside popup_bounds {:?}, got {:?}",
            popup_bounds,
            last_item_bounds
        );
        window.scroll(
            "popup-menu",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(240.))),
            cx,
        );
        window.render_frame(cx);

        let rendered_texts = gpui_kit::base::test_support::texts(window);
        let expected_repeater_kbd = gpui_kit::component::kbd::Kbd::format(
            &gpui_kit::Keystroke::parse("cmd-r").unwrap(),
        );
        let expected_intruder_kbd = gpui_kit::component::kbd::Kbd::format(
            &gpui_kit::Keystroke::parse("cmd-i").unwrap(),
        );
        let expected_copy_kbd = gpui_kit::component::kbd::Kbd::format(
            &gpui_kit::Keystroke::parse("cmd-c").unwrap(),
        );
        let expected_organiser_kbd = gpui_kit::component::kbd::Kbd::format(
            &gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                "cmd-o"
            } else {
                "ctrl-o"
            })
            .unwrap(),
        );
        let expected_ai_kbd = gpui_kit::component::kbd::Kbd::format(
            &gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                "cmd-g"
            } else {
                "ctrl-g"
            })
            .unwrap(),
        );
        assert!(rendered_texts.iter().any(|t| t.text() == expected_repeater_kbd));
        assert!(rendered_texts.iter().any(|t| t.text() == expected_intruder_kbd));
        assert!(rendered_texts.iter().any(|t| t.text() == expected_copy_kbd));
        assert!(rendered_texts.iter().any(|t| t.text() == expected_organiser_kbd));
        assert!(rendered_texts.iter().any(|t| t.text() == expected_ai_kbd));

        window.click("history-request-to-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let repeater_cmds = drain(&mut f.commands);
    assert_eq!(
        repeater_cmds.len(),
        1,
        "clicking Send to Repeater must dispatch exactly once"
    );
    assert!(matches!(
        repeater_cmds.as_slice(),
        [Command::OpenRepeater { project, flow_id: f_id, original: false }]
            if *project == f.project && *f_id == flow_id
    ));

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.render_frame(cx);
        window.click("history-request-message-actions", cx);
        window.render_frame(cx);
        let req_in_browser = window
            .within("popup-menu")
            .find_by_label("在浏览器中发起请求 (Request in browser)")
            .bounds()
            .center();
        window.pointer_down(req_in_browser, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(req_in_browser, gpui_kit::MouseButton::Left, cx);
        window.render_frame(cx);
        let orig_bounds = window
            .within("submenu")
            .find_by_label("在原始会话中 (In original session)")
            .bounds();
        let curr_bounds = window
            .within("submenu")
            .find_by_label("在当前浏览器会话中 (In current session)")
            .bounds();
        assert_eq!(orig_bounds.size.height, px(23.));
        assert_eq!(curr_bounds.size.height, px(23.));
        assert!((curr_bounds.origin.y - orig_bounds.bottom()).abs() <= px(0.5));
        let curr_session = curr_bounds.center();
        window.pointer_down(curr_session, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(curr_session, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let browser_cmds = drain(&mut f.commands);
    assert_eq!(
        browser_cmds.len(),
        1,
        "clicking Request in browser must dispatch exactly once"
    );
    assert!(matches!(
        browser_cmds.as_slice(),
        [Command::OpenConfiguredBrowser {
            project,
            url: Some(url),
            session: mio_core::BrowserRequestSession::CurrentSession,
        }] if *project == f.project && url == "https://portal.example.com/account?token=secret123"
    ));
}

#[gpui_kit::test]
fn hex_grid_in_place_edit_marks_repeater_tab_dirty(cx: &mut TestAppContext) {
    let mut initial = tab("Hex Dirty Check");
    initial.draft.raw = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_vec();
    let tab_id = initial.id;
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.render_frame(cx);
        assert!(!f.view.read(cx).tab_editors[&tab_id].dirty);
        window.click("repeater-request-hex", cx);
        window.render_frame(cx);
        let grid = f.view.read(cx).tab_editors[&tab_id]
            .request
            .hex_grid()
            .expect("switching to Hex must initialize hex_grid");
        grid.update(cx, |g, cx| {
            g.select_byte(0, false, false, window, cx);
            g.byte_input
                .update(cx, |inp, cx| inp.set_value("50", window, cx));
            g.edit_selected_byte(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        assert!(
            f.view.read(cx).tab_editors[&tab_id].dirty,
            "editing bytes in HexGridView must emit InputEvent::Change and mark Repeater tab dirty"
        );
        let bytes = f.view.update(cx, |wb, cx| {
            wb.tab_editors[&tab_id].request.bytes(window, cx).unwrap()
        });
        assert_eq!(bytes[0], b'P');
    })
    .unwrap();
}
