use super::*;

#[gpui_kit::test]
fn repeater_settings_normalize_line_endings_and_redirect_modes_persist_and_reset(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    checkbox_checked(
        settings,
        "settings-repeater-normalize-http1-line-endings",
        true,
        cx,
    );
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-repeater-redirect-never").selected(),
            Some(true)
        );
    })
    .unwrap();

    toggle_checkbox(
        settings,
        "settings-repeater-normalize-http1-line-endings",
        false,
        cx,
    );
    let first = saved(&mut fixture.commands);
    assert!(!first.repeater_settings.normalize_http1_line_endings);
    assert_eq!(
        first.repeater_settings.follow_redirects,
        RepeaterRedirectMode::Never
    );
    acknowledge(&fixture, first, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-repeater-redirect-on-site", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let second = saved(&mut fixture.commands);
    assert!(!second.repeater_settings.normalize_http1_line_endings);
    assert_eq!(
        second.repeater_settings.follow_redirects,
        RepeaterRedirectMode::OnSiteOnly
    );
    acknowledge(&fixture, second, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-repeater-redirect-on-site").selected(),
            Some(true)
        );
        window.click("settings-repeater-redirect-always", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let third = saved(&mut fixture.commands);
    assert_eq!(
        third.repeater_settings.follow_redirects,
        RepeaterRedirectMode::Always
    );
    acknowledge(&fixture, third, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-repeater-redirect-always").selected(),
            Some(true)
        );
        window.click("settings-repeater-redirects-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let fourth = saved(&mut fixture.commands);
    assert!(!fourth.repeater_settings.normalize_http1_line_endings);
    assert_eq!(
        fourth.repeater_settings.follow_redirects,
        RepeaterRedirectMode::Never
    );
    acknowledge(&fixture, fourth, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-repeater-redirect-never").selected(),
            Some(true)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn repeater_follow_redirection_button_and_copy_url_or_curl_actions_work(cx: &mut TestAppContext) {
    use mio_core::Header;

    let mut initial = tab("重定向与复制");
    initial.draft.target = "http://127.0.0.1:9001".into();
    initial.draft.raw =
        b"POST /login?step=1 HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 7\r\n\r\na=1&b=2".to_vec();
    let tab_id = initial.id;
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(CopyUrl), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let copied_url = cx
        .read(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text());
    assert_eq!(
        copied_url.as_deref(),
        Some("http://127.0.0.1:9001/login?step=1")
    );

    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(CopyCurlCommand), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let copied_curl = cx
        .read(|cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("clipboard should contain curl command");
    assert!(copied_curl.contains("curl"));
    assert!(copied_curl.contains("http://127.0.0.1:9001/login?step=1"));

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [Command::Send { run_id, draft, .. }] = commands.as_slice() else {
        panic!("expected initial Repeater Send command");
    };
    let mut redirect_record = record("http://127.0.0.1:9001/login?step=1", "a=1&b=2");
    redirect_record.summary.id = *run_id;
    redirect_record.summary.status = Some(302);
    redirect_record.summary.state = FlowState::Complete;
    redirect_record.response = Some(HttpResponse {
        status: 302,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Location".into(),
                value: b"http://127.0.0.1:9002/welcome?from=302".to_vec(),
            },
            Header {
                name: "Content-Length".into(),
                value: b"0".to_vec(),
            },
        ],
        body: Vec::new(),
        trailers: Vec::new(),
    });
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id,
            draft_revision: draft.revision,
            record: Box::new(redirect_record),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    let saved_after_302 = saved(&mut fixture.commands);
    acknowledge(&fixture, saved_after_302, cx);
    let commands = drain(&mut fixture.commands);
    for command in commands {
        if let Command::QueryRepeaterHistory {
            project,
            query_revision,
            tab_id,
            start,
            ..
        } = command
        {
            fixture
                .events
                .try_send(Event::RepeaterHistoryPage {
                    project,
                    query_revision,
                    tab_id,
                    result: Ok(mio_core::RepeaterHistoryPage {
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

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let btn = window.find("repeater-follow-redirection");
        assert!(btn.visible());
        assert_eq!(btn.disabled(), None);
        window.click("repeater-follow-redirection", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let redirected_workspace = saved(&mut fixture.commands);
    assert_eq!(
        redirected_workspace.tabs[0].draft.target,
        "http://127.0.0.1:9002"
    );
    let redirected_raw = String::from_utf8(redirected_workspace.tabs[0].draft.raw.clone()).unwrap();
    assert!(redirected_raw.starts_with("GET /welcome?from=302 HTTP/1.1\r\n"));
    assert!(redirected_raw.contains("Host: 127.0.0.1:9002\r\n"));

    acknowledge(&fixture, redirected_workspace, cx);
    let sent_commands = drain(&mut fixture.commands);
    let send_cmd = sent_commands
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send { draft, .. } => Some(draft),
            _ => None,
        })
        .expect(
            "clicking follow redirection must send the redirected request after workspace save ACK",
        );
    assert_eq!(send_cmd.target, "http://127.0.0.1:9002");
    assert!(
        String::from_utf8_lossy(&send_cmd.raw).starts_with("GET /welcome?from=302 HTTP/1.1\r\n")
    );
}

#[gpui_kit::test]
fn repeater_timeout_upstream_proxy_and_pretty_view_preserve_raw_bytes(cx: &mut TestAppContext) {
    let initial_tab = tab("Pretty与设置");
    let mut fixture = fixture(cx, vec![initial_tab.clone()], 64);
    let project = fixture.project;
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-repeater-timeout-input").visible() {
                break;
            }
            window.scroll(
                "settings-repeater-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-400.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-repeater-timeout-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("45", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_timeout = saved(&mut fixture.commands);
    assert_eq!(ws_timeout.repeater_settings.streaming_response_timeout_secs, 45);
    acknowledge(&fixture, ws_timeout, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-repeater-timeout-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_reset = saved(&mut fixture.commands);
    assert_eq!(ws_reset.repeater_settings.streaming_response_timeout_secs, 600);
    acknowledge(&fixture, ws_reset, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-connections", cx);
        window.render_frame(cx);
        window.click("settings-socks-enabled", cx);
        window.render_frame(cx);
        for (id, text) in [
            ("settings-upstream-host", "127.0.0.1"),
            ("settings-upstream-port", "1080"),
            ("settings-upstream-username", "alice"),
            ("settings-upstream-password", "secret"),
        ] {
            window.click(id, cx);
            window.press(crate::kbd!("a"), cx);
            window.input(text, cx);
        }
        window.render_frame(cx);
        window.click("settings-upstream-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_upstream = saved(&mut fixture.commands);
    assert_eq!(
        ws_upstream.upstream_proxy.protocol,
        mio_core::UpstreamProxyProtocol::Socks5
    );
    assert_eq!(ws_upstream.upstream_proxy.host, "127.0.0.1");
    assert_eq!(ws_upstream.upstream_proxy.port, 1080);
    assert_eq!(ws_upstream.upstream_proxy.username, "alice");
    assert_eq!(ws_upstream.upstream_proxy.password, "secret");
    assert!(ws_upstream.upstream_proxy.socks.use_proxy);
    acknowledge(&fixture, ws_upstream, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-upstream-reset", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_upstream_reset = saved(&mut fixture.commands);
    assert!(ws_upstream_reset.upstream_proxy.is_default());
    acknowledge(&fixture, ws_upstream_reset, cx);
    cx.simulate_keystrokes(settings, "escape");

    // Verify Pretty view in Repeater formats JSON visually while Send keeps exact raw compact bytes
    let compact_raw = b"POST /api HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Type: application/json\r\nContent-Length: 17\r\n\r\n{\"a\":1,\"b\":[2,3]}".to_vec();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tool = Tool::Repeater;
            let id = view.workspace.tabs[0].id;
            view.workspace.active = Some(id);
            view.workspace.tabs[0].draft.target = "http://127.0.0.1:9001".into();
            view.workspace.tabs[0].draft.raw = compact_raw.clone();
            view.workspace.tabs[0].draft.revision += 1;
            view.sync_tab_editors(window, cx);
            view.save_workspace(cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let synced = saved(&mut fixture.commands);
    acknowledge(&fixture, synced, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-request-pretty", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, _, cx| {
        let id = fixture.view.read(cx).workspace.tabs[0].id;
        let editor = &fixture.view.read(cx).tab_editors[&id].request;
        assert!(editor.pretty_active);
        let pretty_text = editor.state().read(cx).value().to_string();
        assert!(pretty_text.contains("{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}"));
    })
    .unwrap();

    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let sent = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send {
                project: p, draft, ..
            } if p == project => Some(draft),
            _ => None,
        })
        .expect("sending from Pretty view must emit Command::Send");
    assert_eq!(
        sent.raw, compact_raw,
        "Pretty view must never alter raw bytes sent on wire"
    );
}
