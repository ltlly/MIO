use super::*;

#[gpui_kit::test]
fn intruder_numbers_payload_processing_and_logger_tool_work(cx: &mut TestAppContext) {
    use mio_core::{
        IntruderSpec, IntruderTab, IntruderWorkspace,
        intruder::{AttackType, Position},
    };

    let initial = tab("Logger与Numbers测试");
    let repeater_tab_id = initial.id;
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);

    let raw = b"GET /items?id=x HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let intruder_tab = IntruderTab {
        id: Uuid::new_v4(),
        title: "1".into(),
        group: None,
        last_run: None,
        spec: IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft {
                revision: 1,
                target: "http://127.0.0.1:9001".into(),
                version: HttpVersion::Http11,
                raw: raw.clone(),
            },
            attack_type: AttackType::Sniper,
            positions: vec![Position { start: 14, end: 15 }],
            payload_sets: vec![Vec::new()],
            max_concurrent_requests: 1,
            request_timeout_ms: 500,
            update_host_header: true,
            save_to_project: true,
        },
    };
    let intruder_id = intruder_tab.id;
    let mut workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    workspace.intruder = IntruderWorkspace {
        active: Some(intruder_id),
        tabs: vec![intruder_tab],
        ..IntruderWorkspace::default()
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-numbers-from", cx);
        window.input("1", cx);
        window.click("intruder-numbers-to", cx);
        window.input("3", cx);
        window.click("intruder-numbers-step", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("1", cx);
        window.click("intruder-numbers-min-digits", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("2", cx);
        window.click("intruder-numbers-generate", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let saved_ws = saved(&mut fixture.commands);
    assert_eq!(
        saved_ws.intruder.tabs[0].spec.payload_sets[0],
        vec![b"01".to_vec(), b"02".to_vec(), b"03".to_vec()]
    );
    acknowledge(&fixture, saved_ws, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.editors[&intruder_id]
            .payload_input
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.input("id=", cx);
        window.click("intruder-proc-prefix", cx);
        window.click("intruder-proc-base64-encode", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.payload_sets[0],
            vec![
                b"aWQ9MDE=".to_vec(),
                b"aWQ9MDI=".to_vec(),
                b"aWQ9MDM=".to_vec(),
            ]
        );
    });
    drain(&mut fixture.commands);

    let mut flow = record("http://127.0.0.1:9001/logger-check", "logger-request-body");
    flow.request.url = "http://127.0.0.1:9001/logger-check".into();
    flow.summary.url = flow.request.url.clone();
    flow.summary.source = TrafficSource::Repeater;
    flow.summary.status = Some(200);
    flow.response = Some(mio_core::HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: Vec::new(),
        body: b"logger-response-body".to_vec(),
        trailers: Vec::new(),
    });
    let flow_id = flow.summary.id;
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id: repeater_tab_id,
            draft_revision: 1,
            record: Box::new(flow.clone()),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-logger", cx);
        assert!(fixture.view.read(cx).tool == Tool::Logger);
        assert_eq!(fixture.view.read(cx).extra_tools.logger_entries.len(), 1);
        window.render_frame(cx);
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click("logger-source-proxy", cx);
        assert_eq!(
            fixture.view.read(cx).extra_tools.logger_source_filter,
            Some(TrafficSource::Proxy)
        );
        window.click("logger-source-repeater", cx);
        window.render_frame(cx);
        window.click("logger-view-filter-bar", cx);
        window.render_frame(cx);
        window.click(("logger-row", 0_usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let read_rev = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ReadFlow {
                selection_revision,
                flow_id: actual,
                ..
            } if actual == flow_id => Some(selection_revision),
            _ => None,
        })
        .expect("clicking a logger row must issue Command::ReadFlow");
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).extra_tools.logger_pending_read,
            Some((fixture.project, read_rev, flow_id))
        );
    });
    let mut stale_flow = flow.clone();
    stale_flow.request.url = "http://127.0.0.1:9001/stale-logger".into();
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: read_rev.saturating_sub(1),
            record: Box::new(stale_flow),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.extra_tools.logger_pending_read,
            Some((fixture.project, read_rev, flow_id))
        );
        assert!(
            !view
                .extra_tools
                .logger_request
                .state()
                .read(cx)
                .value()
                .contains("/stale-logger")
        );
    });
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: read_rev,
            record: Box::new(flow),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.extra_tools.logger_selected, Some(flow_id));
        assert!(view.extra_tools.logger_pending_read.is_none());
        assert!(view.extra_tools.logger_loaded_detail.is_some());
        assert!(
            String::from_utf8_lossy(
                view.extra_tools
                    .logger_request
                    .state()
                    .read(cx)
                    .value()
                    .as_bytes()
            )
            .contains("/logger-check")
        );
        assert!(
            String::from_utf8_lossy(
                view.extra_tools
                    .logger_response
                    .state()
                    .read(cx)
                    .value()
                    .as_bytes()
            )
            .contains("logger-response-body")
        );
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::CopyUrl), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("http://127.0.0.1:9001/logger-check".into())
        );
    });
}

#[gpui_kit::test]
fn intruder_grep_match_extract_and_logger_history_query_work(cx: &mut TestAppContext) {
    use mio_core::{
        IntruderGrepExtractRule, IntruderSpec, IntruderTab, IntruderWorkspace,
        intruder::{AttackType, Position, evaluate_grep_config},
    };

    let initial = tab("Grep与Logger历史测试");
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);

    let raw = b"GET /items?id=x HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let intruder_tab = IntruderTab {
        id: Uuid::new_v4(),
        title: "1".into(),
        group: None,
        last_run: None,
        spec: IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft {
                revision: 1,
                target: "http://127.0.0.1:9001".into(),
                version: HttpVersion::Http11,
                raw,
            },
            attack_type: AttackType::Sniper,
            positions: vec![Position { start: 14, end: 15 }],
            payload_sets: vec![Vec::new()],
            max_concurrent_requests: 1,
            request_timeout_ms: 500,
            update_host_header: true,
            save_to_project: true,
        },
    };
    let intruder_id = intruder_tab.id;
    let mut workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    workspace.intruder = IntruderWorkspace {
        active: Some(intruder_id),
        tabs: vec![intruder_tab],
        ..IntruderWorkspace::default()
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.render_frame(cx);
        window.click("intruder-sidebar-settings", cx);
        window.render_frame(cx);
        assert!(window.find("intruder-config-settings").visible());
        window.click("intruder-grep-match-input", cx);
        window.input(" admin ", cx);
        window.press(crate::kbd!("f"), cx);
        assert!(
            fixture.view.read(cx).intruder.editors[&intruder_id]
                .grep_match_input
                .focus_handle(cx)
                .is_focused(window)
        );
        window.click("intruder-grep-match-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let saved_ws = saved(&mut fixture.commands);
    assert_eq!(
        saved_ws.intruder.grep_configs[&intruder_id].match_expressions,
        vec![" admin ".to_string()]
    );
    acknowledge(&fixture, saved_ws, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-grep-match-input", cx);
        window.input("flag{", cx);
        window.click("intruder-grep-match-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let saved_ws = saved(&mut fixture.commands);
    assert_eq!(
        saved_ws.intruder.grep_configs[&intruder_id].match_expressions,
        vec![" admin ".to_string(), "flag{".to_string()]
    );
    acknowledge(&fixture, saved_ws, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-grep-extract-start", cx);
        window.input("token=", cx);
        window.click("intruder-grep-extract-end", cx);
        window.input("&", cx);
        window.click("intruder-grep-extract-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let saved_ws = saved(&mut fixture.commands);
    let grep_cfg = &saved_ws.intruder.grep_configs[&intruder_id];
    assert_eq!(
        grep_cfg.extract_rules,
        vec![IntruderGrepExtractRule {
            start_delimiter: "token=".into(),
            end_delimiter: "&".into(),
        }]
    );
    let (matched, extracted) = evaluate_grep_config(
        b"HTTP/1.1 200 OK\r\n\r\nuser= ADMIN &token=secret123&flag{ok}",
        grep_cfg,
    );
    assert_eq!(matched, vec![" admin ".to_string(), "flag{".to_string()]);
    assert_eq!(extracted, vec!["token=..&: secret123".to_string()]);
    acknowledge(&fixture, saved_ws, cx);

    let mut older_live = record("http://127.0.0.1:9001/logger-history-older", "body");
    older_live.summary.url = "http://127.0.0.1:9001/logger-history-older".into();
    older_live.summary.source = TrafficSource::Proxy;
    older_live.summary.started_unix_ms = 5;
    let mut unmatched_live = record("http://127.0.0.1:9001/unmatched-entry", "body");
    unmatched_live.summary.url = "http://127.0.0.1:9001/unmatched-entry".into();
    unmatched_live.summary.source = TrafficSource::Proxy;
    unmatched_live.summary.started_unix_ms = 15;

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.record_logger_flow(&older_live.summary);
            view.record_logger_flow(&unmatched_live.summary);
            view.extra_tools.logger_search.update(cx, |input, cx| {
                input.set_value("logger-history", window, cx)
            });
        });
        window.click("tool-logger", cx);
        window.render_frame(cx);
        window.click("logger-options-menu", cx);
        window.render_frame(cx);
        let refresh_pos = window.find_by_label("刷新历史").bounds().center();
        window.pointer_down(refresh_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(refresh_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let (query_project, query_rev) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::QueryLoggerHistory {
                project,
                query_revision,
                offset: 0,
                limit: 200,
                ..
            } => Some((project, query_revision)),
            _ => None,
        })
        .expect("clicking logger-refresh-button must issue Command::QueryLoggerHistory");
    assert_eq!(query_project, fixture.project);

    let mut queried_flow = record("http://127.0.0.1:9001/logger-history-queried", "body");
    queried_flow.summary.url = "http://127.0.0.1:9001/logger-history-queried".into();
    queried_flow.summary.source = TrafficSource::Proxy;
    queried_flow.summary.status = Some(201);
    queried_flow.summary.started_unix_ms = 20;
    fixture
        .events
        .try_send(Event::LoggerHistoryPage {
            project: fixture.project,
            query_revision: query_rev,
            offset: 0,
            result: Ok((1, vec![queried_flow.summary.clone()])),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.extra_tools.logger_query_cancel.is_none());
        assert_eq!(view.extra_tools.logger_entries.len(), 2);
        assert_eq!(
            view.extra_tools.logger_entries[0].url,
            "http://127.0.0.1:9001/logger-history-older"
        );
        assert_eq!(
            view.extra_tools.logger_entries[1].url,
            "http://127.0.0.1:9001/logger-history-queried"
        );
        assert!(window.find(("logger-row", 0_usize)).visible());
    })
    .unwrap();
}
