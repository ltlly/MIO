use super::*;

#[gpui_kit::test]
fn repeater_sse_streaming_progress_and_cancel_preserves_partial_response(cx: &mut TestAppContext) {
    let initial = tab("Repeater SSE 流式与取消保留测试");
    let tab_id = initial.id;
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let (run_id, draft_revision, cancel) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send {
                project,
                tab_id: sent_tab,
                run_id,
                draft,
                cancel,
                ..
            } if project == fixture.project && sent_tab == tab_id => {
                Some((run_id, draft.revision, cancel))
            }
            _ => None,
        })
        .expect("clicking repeater-send must emit Command::Send");
    assert!(!cancel.is_cancelled());

    fixture
        .events
        .try_send(Event::SendStarted {
            project: fixture.project,
            tab_id,
            run_id,
            draft_revision,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);

    let chunk1 = HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![mio_core::Header {
            name: "Content-Type".into(),
            value: "text/event-stream".into(),
        }],
        body: b"data: chunk-1\n\n".to_vec(),
        trailers: Vec::new(),
    };
    fixture
        .events
        .try_send(Event::SendProgress {
            project: fixture.project,
            tab_id,
            run_id,
            draft_revision,
            response: Box::new(chunk1.clone()),
            elapsed_ms: 42,
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let editors = &view.tab_editors[&tab_id];
        assert_eq!(editors.response_revision, Some(draft_revision));
        assert_eq!(
            editors.result.as_deref(),
            Some("正在接收… · 42 ms · 15 字节")
        );
        assert!(
            editors
                .response
                .raw
                .read(cx)
                .value()
                .contains("data: chunk-1\n\n")
        );
    });

    // Switch to Hex view and ensure subsequent SendProgress preserves Hex view.
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-response-hex", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let mut chunk2 = chunk1.clone();
    chunk2.body = b"data: chunk-1\n\ndata: chunk-2\n\n".to_vec();
    fixture
        .events
        .try_send(Event::SendProgress {
            project: fixture.project,
            tab_id,
            run_id,
            draft_revision,
            response: Box::new(chunk2.clone()),
            elapsed_ms: 85,
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let editors = &view.tab_editors[&tab_id];
        assert_eq!(editors.response.view(), MessageView::Hex);
        assert_eq!(
            editors.result.as_deref(),
            Some("正在接收… · 85 ms · 30 字节")
        );
    });

    // Switch back to Raw view, then cancel and deliver terminal SendFinished with partial response.
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-response-raw", cx);
        window.click("repeater-cancel", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).iter().any(|cmd| matches!(
        cmd,
        Command::CancelSend {
            project,
            tab_id: t,
            run_id: r,
            draft_revision: d,
        } if *project == fixture.project && *t == tab_id && *r == run_id && *d == draft_revision
    )));

    let mut terminal = record("http://127.0.0.1:9001/sse", "");
    terminal.summary.id = run_id;
    terminal.summary.source = TrafficSource::Repeater;
    terminal.summary.state = FlowState::Cancelled;
    terminal.summary.elapsed_ms = 90;
    terminal.summary.response_bytes = chunk2.body.len() as u64;
    terminal.response = Some(chunk2.clone());
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id,
            draft_revision,
            record: Box::new(terminal),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let editors = &view.tab_editors[&tab_id];
        assert!(
            editors
                .response
                .raw
                .read(cx)
                .value()
                .contains("data: chunk-1\n\ndata: chunk-2\n\n")
        );
        assert_eq!(editors.result.as_deref(), Some("已取消 · 90 ms · 30 字节"));
        assert_eq!(view.workspace.tabs[0].last_flow, Some(run_id));
    });
}

#[gpui_kit::test]
fn repeater_compressed_and_utf8_boundary_progress_do_not_lock_response_in_hex(
    cx: &mut TestAppContext,
) {
    let initial = tab("Compressed & Split UTF-8 Progress");
    let tab_id = initial.id;
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);

    // 1. Compressed partial SendProgress must not force Pretty response into Hex.
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (run1, rev1) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send {
                tab_id: t,
                run_id,
                draft,
                ..
            } if t == tab_id => Some((run_id, draft.revision)),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::SendStarted {
            project: fixture.project,
            tab_id,
            run_id: run1,
            draft_revision: rev1,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::SendProgress {
            project: fixture.project,
            tab_id,
            run_id: run1,
            draft_revision: rev1,
            response: Box::new(HttpResponse {
                version: HttpVersion::Http11,
                status: 200,
                reason_phrase: Some(b"OK".to_vec()),
                headers: vec![
                    mio_core::Header {
                        name: "Content-Type".into(),
                        value: b"application/json;charset=UTF-8".to_vec(),
                    },
                    mio_core::Header {
                        name: "Content-Encoding".into(),
                        value: b"gzip".to_vec(),
                    },
                ],
                body: vec![0x1f, 0x8b, 0x08, 0x00, 0xff],
                trailers: Vec::new(),
            }),
            elapsed_ms: 15,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let resp_ed = &fixture.view.read(cx).tab_editors[&tab_id].response;
        assert_eq!(resp_ed.view(), MessageView::Raw);
        assert!(!resp_ed.auto_hex_fallback);
    });

    let unpacked_json = HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: Some(b"OK".to_vec()),
        headers: vec![mio_core::Header {
            name: "Content-Type".into(),
            value: b"application/json;charset=UTF-8".to_vec(),
        }],
        body: br#"{"ok":true,"count":2}"#.to_vec(),
        trailers: Vec::new(),
    };
    let mut rec1 = record("https://api.example.test/v1/items", "");
    rec1.summary.id = run1;
    rec1.summary.source = TrafficSource::Repeater;
    rec1.summary.status = Some(200);
    rec1.summary.state = FlowState::Complete;
    rec1.summary.response_bytes = unpacked_json.body.len() as u64;
    rec1.response = Some(unpacked_json.clone());
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id,
            draft_revision: rev1,
            record: Box::new(rec1),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let resp_ed = &fixture.view.read(cx).tab_editors[&tab_id].response;
        assert_eq!(resp_ed.view(), MessageView::Raw);
        assert!(resp_ed.pretty_active());
        assert!(resp_ed.raw.read(cx).value().contains("\"ok\": true"));
    });
    for cmd in drain(&mut fixture.commands) {
        if let Command::SaveWorkspace { workspace, .. } = cmd {
            fixture
                .events
                .try_send(Event::WorkspaceChanged {
                    project: fixture.project,
                    workspace,
                })
                .unwrap();
        }
    }
    cx.run_until_parked();
    drain(&mut fixture.commands);

    // 2. Uncompressed chunk split mid-UTF-8 flips to auto Hex temporarily, then recovers Pretty/Raw on SendFinished.
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (run2, rev2) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Send {
                tab_id: t,
                run_id,
                draft,
                ..
            } if t == tab_id => Some((run_id, draft.revision)),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::SendProgress {
            project: fixture.project,
            tab_id,
            run_id: run2,
            draft_revision: rev2,
            response: Box::new(HttpResponse {
                version: HttpVersion::Http11,
                status: 200,
                reason_phrase: Some(b"OK".to_vec()),
                headers: vec![mio_core::Header {
                    name: "Content-Type".into(),
                    value: b"application/json;charset=UTF-8".to_vec(),
                }],
                body: b"{\"msg\":\"\xe4\xb8".to_vec(),
                trailers: Vec::new(),
            }),
            elapsed_ms: 20,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let resp_ed = &fixture.view.read(cx).tab_editors[&tab_id].response;
        assert_eq!(resp_ed.view(), MessageView::Hex);
        assert!(resp_ed.auto_hex_fallback);
    });

    let mut rec2 = record("https://api.example.test/v1/items", "");
    rec2.summary.id = run2;
    rec2.summary.source = TrafficSource::Repeater;
    rec2.summary.status = Some(200);
    rec2.summary.state = FlowState::Complete;
    rec2.summary.response_bytes = unpacked_json.body.len() as u64;
    rec2.response = Some(unpacked_json);
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id,
            draft_revision: rev2,
            record: Box::new(rec2),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let resp_ed = &fixture.view.read(cx).tab_editors[&tab_id].response;
        assert_eq!(resp_ed.view(), MessageView::Raw);
        assert!(!resp_ed.auto_hex_fallback);
        assert!(resp_ed.pretty_active());
    });
}

#[gpui_kit::test]
fn proxy_interception_ip_mime_page_title_and_was_annotated_save_with_ack(cx: &mut TestAppContext) {
    use mio_core::ProxyInterceptRelation::{DoesNotMatch, Matches, WasAnnotated};
    use mio_core::ProxyInterceptType::{IpAddress, MimeType, PageTitle, Request};

    let (mut fixture, settings) = proxy_rule_settings(cx);

    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_choose_type(dialog, "IP address", false, cx);
    proxy_rule_condition(dialog, "^127\\.0\\.0\\.1$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let saved_ip = saved(&mut fixture.commands);
    let ip_rule = saved_ip.proxy_interception.request.rules.last().unwrap();
    assert_eq!(ip_rule.match_type, IpAddress);
    assert_eq!(ip_rule.relationship, Matches);
    assert_eq!(ip_rule.condition, "^127\\.0\\.0\\.1$");
    acknowledge(&fixture, saved_ip, cx);

    for (label, expected_type, relation_down, expected_rel, cond) in [
        ("Mime type", MimeType, false, Matches, "^html$"),
        ("Page title", PageTitle, true, DoesNotMatch, "^Login$"),
    ] {
        proxy_rule_click(settings, "proxy-response-rule-add", cx);
        let dialog = proxy_rule_dialog(settings, &fixture, cx);
        proxy_rule_choose_type(dialog, label, true, cx);
        if relation_down {
            proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
            cx.simulate_keystrokes(dialog, "down enter");
            cx.run_until_parked();
        }
        proxy_rule_condition(dialog, cond, cx);
        proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
        let saved_resp = saved(&mut fixture.commands);
        let rule = saved_resp.proxy_interception.response.rules.last().unwrap();
        assert_eq!(rule.match_type, expected_type);
        assert_eq!(rule.relationship, expected_rel);
        assert_eq!(rule.condition, cond);
        acknowledge(&fixture, saved_resp, cx);
    }

    proxy_rule_click(settings, "proxy-response-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^cleared-when-boolean$", cx);
    proxy_rule_choose_type(dialog, "Request", true, cx);
    proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
    cx.simulate_keystrokes(dialog, "down down down down down down enter");
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-relationship").value(),
            Some("Was annotated")
        );
    })
    .unwrap();
    assert_proxy_rule_condition_disabled(dialog, cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let saved_annotated = saved(&mut fixture.commands);
    let annotated_rule = saved_annotated
        .proxy_interception
        .response
        .rules
        .last()
        .unwrap();
    assert_eq!(annotated_rule.match_type, Request);
    assert_eq!(annotated_rule.relationship, WasAnnotated);
    assert!(annotated_rule.condition.is_empty());
    acknowledge(&fixture, saved_annotated, cx);
}

#[gpui_kit::test]
fn multi_proxy_listeners_add_toggle_remove_and_legacy_input_sync(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-proxy", cx);
        window.render_frame(cx);
        assert!(window.find("proxy-listeners-heading").visible());
        assert_eq!(
            window.find(("settings-listener-toggle", 0usize)).checked(),
            Some(false)
        );
        window.click("proxy-listener-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("proxy-listener-tab-binding").visible());
        assert!(window.find("proxy-listener-tab-request-handling").visible());
        assert!(window.find("proxy-listener-tab-certificate").visible());
        assert!(window.find("proxy-listener-tab-tls").visible());
        window.click("proxy-listener-editor-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("127.0.0.1:18081", cx);
        window.click("proxy-listener-tab-request-handling", cx);
        window.render_frame(cx);
        window.click("proxy-listener-redirect-host", cx);
        window.input("backend.internal", cx);
        window.click("proxy-listener-redirect-port", cx);
        window.input("8443", cx);
        window.click("proxy-listener-force-tls", cx);
        window.click("proxy-listener-invisible", cx);
        window.click("proxy-listener-tab-certificate", cx);
        window.render_frame(cx);
        window.click("proxy-listener-cert-self-signed", cx);
        window.click("proxy-listener-tab-tls", cx);
        window.render_frame(cx);
        window.click("proxy-listener-tls13", cx);
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let added = saved(&mut fixture.commands);
    assert_eq!(added.proxy_listeners.len(), 2);
    assert_eq!(added.proxy_listeners[0].bind_address, "127.0.0.1:8080");
    assert_eq!(added.proxy_listeners[1].bind_address, "127.0.0.1:18081");
    assert!(added.proxy_listeners[1].invisible_proxying);
    assert!(added.proxy_listeners[1].force_use_of_tls);
    assert_eq!(added.proxy_listeners[1].redirect_to_host, "backend.internal");
    assert_eq!(added.proxy_listeners[1].redirect_to_port, Some(8443));
    assert_eq!(
        added.proxy_listeners[1].certificate_mode,
        mio_core::ProxyListenerCertMode::SelfSigned
    );
    assert_eq!(added.proxy_listeners[1].tls_protocols_label, "TLSv1.3 only");
    assert_eq!(added.proxy_listener_or_default(), "127.0.0.1:8080");
    acknowledge(&fixture, added, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dup_dialog = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(dup_dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-editor-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("127.0.0.1:18081", cx);
        window.click("proxy-listener-editor-ok", cx);
        window.render_frame(cx);
        assert!(window.find("proxy-listener-editor-error").visible());
        window.click("proxy-listener-editor-cancel", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(settings, |_, window, cx| {
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
    let disable_dialog = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(disable_dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-editor-enabled", cx);
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let toggled = saved(&mut fixture.commands);
    assert!(!toggled.proxy_listeners[0].enabled);
    assert_eq!(toggled.proxy_listener_or_default(), "127.0.0.1:18081");
    acknowledge(&fixture, toggled, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-listener-row", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-edit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let edit_dialog = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(edit_dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-editor-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("127.0.0.1:18082", cx);
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let updated = saved(&mut fixture.commands);
    assert_eq!(updated.proxy_listeners[1].bind_address, "127.0.0.1:18082");
    assert_eq!(updated.proxy_listener_or_default(), "127.0.0.1:18082");
    acknowledge(&fixture, updated, cx);

    for _ in 0..2 {
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-row", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-listener-remove", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut fixture.commands);
        acknowledge(&fixture, ws, cx);
    }
    cx.read(|cx| {
        let ws = &fixture.view.read(cx).workspace;
        assert!(ws.proxy_listeners.is_empty());
        assert_eq!(ws.proxy_listener_or_default(), "127.0.0.1:8080");
    });
}

#[gpui_kit::test]
fn client_tls_certificates_pem_add_toggle_remove_and_reject_invalid_host(cx: &mut TestAppContext) {
    const TEST_CLIENT_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIBbTCCARSgAwIBAgIUVIpxIWbRtuDLpYl1477vzV1ugh0wCgYIKoZIzj0EAwIw\n\
HTEbMBkGA1UEAwwSY2xpZW50LmV4YW1wbGUuY29tMB4XDTI2MTExMjA1MzA1NloX\n\
DTM2MTEwOTA1MzA1NlowHTEbMBkGA1UEAwwSY2xpZW50LmV4YW1wbGUuY29tMFkw\n\
EwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEIbSR+tNi17qXQFl59/hH+44RmlzWWDXc\n\
PEoCCPz1C/LzkFknUsbyvWR0EFyYoN31/z+spQ+LUJ8jj+u+DnVYUaMyMDAwHQYD\n\
VR0OBBYEFPiXZXvod4JsIMGiUYlRQAUcuszHMA8GA1UdEwEB/wQFMAMBAf8wCgYI\n\
KoZIzj0EAwIDRwAwRAIgaOz+x4MzyOceAJOUPHMeueynmih/2RJdlZ8u/MVFd5EC\n\
IF63cYtN914wygUdSOfm2bLCW6p5IoVVbzuVti6y/Qml\n\
-----END CERTIFICATE-----\n";
    const TEST_CLIENT_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg4XlmnemZ2BNPQoa8\n\
WzmlhPk1AkNyYLQDHfm6fDB64SChRANCAAQhtJH602LXupdAWXn3+Ef7jhGaXNZY\n\
Ndw8SgII/PUL8vOQWSdSxvK9ZHQQXJig3fX/P6ylD4tQnyOP674OdVhR\n\
-----END PRIVATE KEY-----\n";

    let cert_path = std::env::temp_dir().join(format!("mio-mtls-cert-{}.pem", Uuid::new_v4()));
    let key_path = std::env::temp_dir().join(format!("mio-mtls-key-{}.pem", Uuid::new_v4()));
    std::fs::write(&cert_path, TEST_CLIENT_CERT_PEM).unwrap();
    std::fs::write(&key_path, TEST_CLIENT_KEY_PEM).unwrap();

    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-tls", cx);
        window.render_frame(cx);
        if !window.find("settings-client-cert-add").visible() {
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-600.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-client-cert-add", cx);
        window.render_frame(cx);
        if !window.find("settings-client-cert-submit").visible() {
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-300.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-client-cert-host", cx);
        window.input("https://bad-host:443", cx);
        window.click("settings-client-cert-path", cx);
        window.input(&cert_path.display().to_string(), cx);
        window.click("settings-client-cert-key-path", cx);
        window.input(&key_path.display().to_string(), cx);
        window.click("settings-client-cert-submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        if !window.find("settings-client-cert-error").visible() {
            window.scroll(
                "settings-tls-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-120.))),
                cx,
            );
            window.render_frame(cx);
        }
        assert!(window.find("settings-client-cert-error").visible());
        assert_eq!(
            window.find("settings-client-cert-host").value(),
            Some("https://bad-host:443")
        );
        window.click("settings-client-cert-host", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("*.Internal.Local", cx);
        window.click("settings-client-cert-submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let _ = std::fs::remove_file(&cert_path);
    let _ = std::fs::remove_file(&key_path);

    let saved_ws = saved(&mut fixture.commands);
    assert_eq!(saved_ws.client_certificates.len(), 1);
    let rule = &saved_ws.client_certificates[0];
    assert!(rule.enabled);
    assert_eq!(rule.host_pattern, "*.internal.local");
    assert_eq!(rule.format, mio_core::ClientCertificateFormat::Pem);
    assert_eq!(rule.certificate_bytes, TEST_CLIENT_CERT_PEM.as_bytes());
    assert_eq!(rule.private_key_pem, TEST_CLIENT_KEY_PEM.as_bytes());
    acknowledge(&fixture, saved_ws, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-client-cert-remove-btn").disabled(),
            Some(true)
        );
        assert_eq!(
            window
                .find(("settings-client-cert-toggle", 0usize))
                .checked(),
            Some(true)
        );
        window.click(("settings-client-cert-toggle", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let toggled_ws = saved(&mut fixture.commands);
    assert!(!toggled_ws.client_certificates[0].enabled);
    acknowledge(&fixture, toggled_ws, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-client-cert-remove-btn").disabled(),
            None
        );
        window.click("settings-client-cert-edit", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-client-cert-remove-btn").disabled(),
            Some(true)
        );
        window.click("settings-client-cert-cancel", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-client-cert-remove-btn").disabled(),
            None
        );
        window.click("settings-client-cert-remove-btn", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let removed_ws = saved(&mut fixture.commands);
    assert!(removed_ws.client_certificates.is_empty());
    acknowledge(&fixture, removed_ws, cx);
}

#[gpui_kit::test]
fn proxy_listener_editor_supports_all_interfaces_and_specific_bind_addresses(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    // 1. Add listener bound to All interfaces (0.0.0.0)
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-proxy", cx);
        window.render_frame(cx);
        window.click("proxy-listener-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dlg_all = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(dlg_all, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("proxy-listener-non-loopback-warning").is_none());
        window.click("proxy-listener-bind-all", cx);
        window.render_frame(cx);
        assert!(window.find("proxy-listener-non-loopback-warning").visible());
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_all = saved(&mut fixture.commands);
    assert_eq!(ws_all.proxy_listeners.len(), 2);
    assert_eq!(ws_all.proxy_listeners[1].bind_address, "0.0.0.0:8081");
    acknowledge(&fixture, ws_all, cx);

    // 2. Add listener bound to a Specific IP address
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dlg_spec = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(dlg_spec, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-bind-specific", cx);
        window.render_frame(cx);
        window.click("proxy-listener-editor-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("8085", cx);
        window.click("proxy-listener-specific-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("192.0.2.10", cx);
        window.render_frame(cx);
        assert!(window.find("proxy-listener-non-loopback-warning").visible());
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_spec = saved(&mut fixture.commands);
    assert_eq!(ws_spec.proxy_listeners.len(), 3);
    assert_eq!(ws_spec.proxy_listeners[2].bind_address, "192.0.2.10:8085");
    acknowledge(&fixture, ws_spec, cx);

    // 3. Add listener by typing full 0.0.0.0:8088 directly into Bind to port
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dlg_full = proxy_rule_dialog(settings, &fixture, cx);
    cx.update_window(dlg_full, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-listener-editor-address", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("0.0.0.0:8088", cx);
        window.render_frame(cx);
        assert!(window.find("proxy-listener-non-loopback-warning").visible());
        window.click("proxy-listener-editor-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_full = saved(&mut fixture.commands);
    assert_eq!(ws_full.proxy_listeners.len(), 4);
    assert_eq!(ws_full.proxy_listeners[3].bind_address, "0.0.0.0:8088");
    acknowledge(&fixture, ws_full, cx);
}

#[gpui_kit::test]
fn toggling_second_listener_when_stopped_updates_enabled_mask_and_workspace_rejected_restores_listeners(
    cx: &mut TestAppContext,
) {
    let listener1 = mio_core::ProxyListenerConfig {
        id: mio_core::Uuid::new_v4(),
        enabled: true,
        bind_address: "127.0.0.1:8080".into(),
        ..mio_core::ProxyListenerConfig::default()
    };
    let listener2 = mio_core::ProxyListenerConfig {
        id: mio_core::Uuid::new_v4(),
        enabled: true,
        bind_address: "127.0.0.1:18081".into(),
        ..mio_core::ProxyListenerConfig::default()
    };
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);

    let initial_ws = cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            view.workspace.revision += 1;
            view.workspace.proxy_listeners = vec![listener1.clone(), listener2.clone()];
            cx.notify();
            (*view.workspace).clone()
        })
    });
    cx.run_until_parked();

    let settings = open_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-proxy", cx);
        window.render_frame(cx);
        // Proxy is stopped and row 0 + row 1 are both enabled; clicking row 1's Running
        // toggle must normalize enabled mask to row 1 only before StartProxy.
        window.click(("settings-listener-toggle", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();

    let commands = drain(&mut fixture.commands);
    let mut saw_save = None;
    let mut saw_start = false;
    for cmd in commands {
        match cmd {
            Command::SaveWorkspace { workspace, .. } => saw_save = Some(workspace),
            Command::StartProxy { listen, .. } => {
                assert_eq!(listen, "127.0.0.1:18081".parse::<SocketAddr>().unwrap());
                saw_start = true;
            }
            _ => {}
        }
    }
    assert!(saw_start, "expected StartProxy on row 1");
    let saved_ws = saw_save.expect("expected SaveWorkspace updating enabled mask");
    assert!(!saved_ws.proxy_listeners[0].enabled);
    assert!(saved_ws.proxy_listeners[1].enabled);

    // Rejecting that listener workspace edit must restore stored proxy_listeners + listener_input.
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: saved_ws.revision,
            stored: initial_ws.clone(),
            message: "synthetic listener conflict".into(),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.proxy_listeners, initial_ws.proxy_listeners);
        assert_eq!(view.listener_input.read(cx).value(), "127.0.0.1:8080");
    });
}
