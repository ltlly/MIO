use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

use super::decode_ext::{MAX_CYBERCHEF_INPUT_BYTES, build_cyberchef_url, build_selection_preview};
use super::*;
use crate::workbench::tests::{drain, fixture, record, tab};
use mio_core::{FlowProvenance, ProvenanceActor};

#[test]
fn cyberchef_url_encodes_standard_base64_and_escapes_plus_slash_equals() {
    assert_eq!(
        build_cyberchef_url(b"hello").unwrap(),
        "https://gchq.github.io/CyberChef/#input=aGVsbG8%3D"
    );
    assert_eq!(
        build_cyberchef_url(&[0xfb, 0xff, 0xbf]).unwrap(),
        "https://gchq.github.io/CyberChef/#input=%2B%2F%2B%2F"
    );
    assert_eq!(
        build_cyberchef_url("中文".as_bytes()).unwrap(),
        "https://gchq.github.io/CyberChef/#input=5Lit5paH"
    );
    assert!(build_cyberchef_url(&[]).is_err());
    assert!(build_cyberchef_url(&vec![b'a'; MAX_CYBERCHEF_INPUT_BYTES + 1]).is_err());
}

#[test]
fn selection_preview_auto_decodes_jwt_html_entities_and_ascii_hex() {
    let jwt =
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NSIsInJvbGUiOiJhZG1pbiJ9.sig_abc";
    let preview = build_selection_preview(jwt.as_bytes().to_vec(), true, true);
    assert_eq!(preview.chain_label, "JWT");
    assert!(!preview.can_write_back);
    assert!(preview.decoded.contains("\"alg\": \"HS256\""));
    assert!(preview.decoded.contains("\"role\": \"admin\""));

    let html = "&lt;script&gt;alert(&quot;ok&quot;)&lt;/script&gt;";
    let html_prev = build_selection_preview(html.as_bytes().to_vec(), true, true);
    assert_eq!(html_prev.chain_label, "HTML");
    assert_eq!(html_prev.decoded, "<script>alert(\"ok\")</script>");

    let hex = "7b226b6579223a2276616c227d";
    let hex_prev = build_selection_preview(hex.as_bytes().to_vec(), true, false);
    assert_eq!(hex_prev.chain_label, "Hex");
    assert_eq!(hex_prev.decoded, "{\"key\":\"val\"}");
}

#[gpui_kit::test]
fn inspector_auto_syncs_selection_across_editors_and_opens_cyberchef(cx: &mut TestAppContext) {
    let mut initial = tab("CyberChef测试");
    initial.draft.raw =
        b"GET /hello?x=aGVsbG8%3D HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let mut f = fixture(cx, vec![initial], 64);
    drain(&mut f.commands);
    let tab_id = cx.read(|cx| f.view.read(cx).workspace.tabs[0].id);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("inspector-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());

        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get_mut(&tab_id).unwrap();
            editors.response.load(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n\xfb\xff\xbf",
                window,
                cx,
            );
            editors
                .request
                .raw
                .update(cx, |state, cx| state.set_selected_range(13..23, cx));
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let auto_decoded = f
            .view
            .read(cx)
            .inspector
            .selection_editor
            .read(cx)
            .value()
            .to_string();
        assert_eq!(auto_decoded, "hello");
        assert!(window.find("inspector-section-selection").visible());
        assert!(window.try_find("inspector-open-cyberchef").is_none());
        window.click("inspector-section-selection", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-open-cyberchef").is_none());
        window.dispatch_action(Box::new(crate::editing::OpenSelectionInCyberChef), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert_eq!(
        cx.opened_url(),
        Some("https://gchq.github.io/CyberChef/#input=YUdWc2JHOCUzRA%3D%3D".into())
    );

    // Newer selection in Response Hex view overrides prior Request selection and opens raw bytes in CyberChef
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            let editors = view.tab_editors.get(&tab_id).unwrap();
            let hex_len = editors.response.hex.read(cx).value().len();
            let tail_start = hex_len.saturating_sub(8);
            editors.response.hex.update(cx, |state, cx| {
                state.set_selected_range(tail_start..hex_len, cx)
            });
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("inspector-selection-editor").visible());
        assert!(window.try_find("inspector-selection-apply").is_none());
        assert!(window.try_find("inspector-open-cyberchef").is_none());
        assert_eq!(
            window.find("inspector-selection-item").label(),
            Some("Hex 字节")
        );
        window.dispatch_action(Box::new(crate::editing::OpenSelectionInCyberChef), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert_eq!(
        cx.opened_url(),
        Some("https://gchq.github.io/CyberChef/#input=%2B%2F%2B%2F".into())
    );
}

#[gpui_kit::test]
fn inspector_selection_works_across_tools_and_provenance_dropdown_renders(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1")], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        let req_raw = b"GET /?q=%7B%22ok%22%3Atrue%7D HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n";
        f.view.update(cx, |view, cx| {
            view.tool = Tool::Logger;
            view.extra_tools.logger_request.load(req_raw, window, cx);
            view.extra_tools
                .logger_request
                .raw
                .update(cx, |s, cx| s.set_selected_range(8..29, cx));
            assert_eq!(
                view.current_selection_preview(cx).map(|p| p.decoded),
                Some("{\"ok\":true}".into())
            );

            view.tool = Tool::Target;
            view.target_pane.page = target::TargetPage::SiteMap;
            view.target_pane.response.load(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"tok\":\"aGVsbG8=\"}",
                window,
                cx,
            );
            let _ = view.target_pane.response.switch_pretty(window, cx);
            let pretty_val = view
                .target_pane
                .response
                .pretty
                .read(cx)
                .value()
                .to_string();
            let pos = pretty_val.find("aGVsbG8=").unwrap();
            view.target_pane
                .response
                .pretty
                .update(cx, |s, cx| s.set_selected_range(pos..pos + 8, cx));
            let prev = view.current_selection_preview(cx).unwrap();
            assert_eq!(prev.decoded, "hello");
            assert!(!prev.can_write_back);

            view.tool = Tool::Organiser;
            view.organiser.request.load(req_raw, window, cx);
            view.organiser
                .request
                .raw
                .update(cx, |s, cx| s.set_selected_range(8..29, cx));
            assert_eq!(
                view.current_selection_preview(cx).map(|p| p.decoded),
                Some("{\"ok\":true}".into())
            );

            view.tool = Tool::Dashboard;
            view.audit_dashboard.new_scan_open = false;
            view.audit_dashboard
                .issue_request_editor
                .load(req_raw, window, cx);
            view.audit_dashboard
                .issue_request_editor
                .raw
                .update(cx, |s, cx| s.set_selected_range(8..29, cx));
            assert_eq!(
                view.current_selection_preview(cx).map(|p| p.decoded),
                None,
                "unmounted issue_request_editor while on Advisory tab must not leak selection"
            );
            let finding_id = mio_core::Uuid::new_v4();
            view.audit_dashboard
                .snapshot
                .findings
                .push(mio_core::AuditFinding {
                    id: finding_id,
                    issue_id: "TEST-1".into(),
                    title: "Test finding".into(),
                    severity_label: "High".into(),
                    severity_rank: 3,
                    confidence: mio_core::IssueConfidence::Certain,
                    phase: mio_core::AuditPhase::Passive,
                    host: "127.0.0.1".into(),
                    url: "http://127.0.0.1:9001/".into(),
                    parameter: None,
                    flow_id: Some(mio_core::Uuid::new_v4()),
                    summary: String::new(),
                    evidence: String::new(),
                    request_highlights: Vec::new(),
                    response_highlights: Vec::new(),
                    remediation: String::new(),
                    detected_unix_ms: 0,
                });
            view.audit_dashboard.selected_finding_id = Some(finding_id);
            view.audit_dashboard.detail_tab =
                crate::workbench::audit_dashboard::IssueDetailTab::Request;
            assert_eq!(
                view.current_selection_preview(cx).map(|p| p.decoded),
                Some("{\"ok\":true}".into())
            );

            view.tool = Tool::Sequencer;
            view.sequencer_pane.page = crate::workbench::sequencer::SequencerPage::Manual;
            view.sequencer_pane.manual_editor.update(cx, |s, cx| {
                s.set_value("aGVsbG8=\n", window, cx);
                s.set_selected_range(0..8, cx);
            });
            assert_eq!(
                view.current_selection_preview(cx).map(|p| p.decoded),
                Some("hello".into())
            );
        });
    })
    .unwrap();

    let orig_rec = record("http://127.0.0.1:9001/orig", "v1");
    let mut final_rec = orig_rec.clone();
    final_rec.request.url = "http://127.0.0.1:9001/rewritten".into();
    let mut prov = FlowProvenance::default();
    prov.push_request_step(
        &orig_rec.request,
        ProvenanceActor::MatchAndReplace {
            rule_id: mio_core::Uuid::nil(),
            comment: "rewrite path".into(),
        },
        &final_rec.request,
    );
    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        f.view.update(cx, |view, cx| {
            view.proxy_page = ProxyPage::History;
            view.detail = Some(Box::new(final_rec.clone()));
            *view.detail_original = OriginalMessages {
                request: Some(orig_rec.request.clone()),
                response: None,
                response_body_complete: None,
                provenance: prov,
            };
            view.show_history_version(false, window, cx);
        });
        window.render_frame(cx);
        assert!(window.find("history-request-version").visible());
        assert!(
            window
                .find("history-request-version")
                .label()
                .unwrap_or_default()
                .contains("Match & Replace: rewrite path")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn footer_resource_usage_and_slim_save_roundtrip(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1: /api")], 32);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let label = window
            .find("footer-resource-usage")
            .label()
            .unwrap_or_default()
            .to_owned();
        assert!(
            label.contains("Memory:") && label.contains(" of "),
            "expected resource label, got {label}"
        );
        window.click("footer-resource-usage", cx);
        window.render_frame(cx);
        window.click("footer-slim-save-as", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    let slim_path = std::path::PathBuf::from("/tmp/mio-ui-slim-test.mio");
    cx.simulate_new_path_selection(|_| Some(slim_path.clone()));
    cx.run_until_parked();

    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        mio_core::Command::SlimSaveProjectAs { path, .. } if path == &slim_path
    )));

    f.events
        .try_send(mio_core::Event::ProjectSlimSaved {
            project: f.project,
            path: slim_path.clone(),
            result: Ok(mio_core::SlimSaveReport {
                old_bytes: 10 * 1_048_576,
                new_bytes: 2 * 1_048_576,
                removed_flows: 42,
                compacted_ext_chunks: 3,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let status = &f.view.read(cx).status;
        assert!(status.contains("瘦身另存为已完成") && status.contains("清理 42 条冗余流量"));
    });

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let menus = cx.get_menus().expect("top menus installed");
        for (menu_name, required_item) in [
            ("MIO", "用户设置…"),
            ("项目", "项目设置…"),
            ("项目", "瘦身另存为…"),
            ("视图", "恢复默认标签布局"),
            ("Target", "目标范围设置…"),
            ("Proxy", "切换 Intercept 开关"),
            ("Intruder", "开始攻击"),
            ("Intruder", "打开已保存的攻击"),
            ("Repeater", "更新 Content-Length"),
            ("Repeater", "复制标签页"),
        ] {
            let m = menus
                .iter()
                .find(|m| m.name == menu_name)
                .unwrap_or_else(|| panic!("missing menu {menu_name}"));
            let act = m
                .items
                .iter()
                .find_map(|it| match it {
                    gpui_kit::OwnedMenuItem::Action { name, action, .. }
                        if name == required_item =>
                    {
                        Some(action.boxed_clone())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing item {required_item} in {menu_name}"));
            assert!(
                window.is_action_available(act.as_ref(), cx),
                "action {required_item} in {menu_name} must be wired and available"
            );
        }
        window.dispatch_action(Box::new(crate::ToggleRepeaterUpdateContentLength), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(
            !f.view
                .read(cx)
                .workspace
                .repeater_settings
                .update_content_length
        );
    });
}

#[gpui_kit::test]
fn proxy_history_selection_auto_opens_inspector_and_unifies_notes_rail(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1")], 64);
    let initial_cmds = drain(&mut f.commands);
    let query_rev = initial_cmds
        .into_iter()
        .find_map(|c| match c {
            Command::QueryHttpHistory { query } => Some(query.query_revision),
            _ => None,
        })
        .expect("initial history query");

    let mut rec = record("http://127.0.0.1:9001/echo?tok=aGVsbG8%3D", "");
    rec.request.url = "http://127.0.0.1:9001/echo?tok=aGVsbG8%3D".into();
    let flow_id = rec.summary.id;
    rec.response = Some(mio_core::HttpResponse {
        version: mio_core::HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![mio_core::Header {
            name: "Set-Cookie".into(),
            value: b"sid=eyJ1c2VyIjoiYWxpY2UifQ%3D%3D".to_vec(),
        }],
        body: b"ok".to_vec(),
        trailers: Vec::new(),
    });
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: query_rev,
            offset: 0,
            total: 1,
            rows: vec![mio_core::HistoryRow::Live(rec.summary.clone())],
        })
        .unwrap();
    cx.run_until_parked();

    // Before selecting any row in HTTP history, right rail still exposes Inspector toggle and disabled Notes toggle.
    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-toggle").visible());
        assert!(window.find("http-notes-toggle").visible());
        assert!(window.try_find("inspector-sidebar").is_none());
        window.click("table-cell-0:0", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let sel_rev = drain(&mut f.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::ReadFlow {
                selection_revision,
                flow_id: id,
                ..
            } if id == flow_id => Some(selection_revision),
            _ => None,
        })
        .expect("ReadFlow emitted");
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: sel_rev,
            record: Box::new(rec),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();

    // Selecting text in HTTP history response automatically opens Inspector without clicking toggle!
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());
        f.view.update(cx, |view, cx| {
            let text = view.history_response.raw.read(cx).value().to_string();
            let needle = "eyJ1c2VyIjoiYWxpY2UifQ%3D%3D";
            let pos = text.find(needle).unwrap();
            view.history_response.raw.update(cx, |s, cx| {
                s.set_selected_range(pos..pos + needle.len(), cx)
            });
        });
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert!(window.find("inspector-section-selection").visible());
        assert!(window.try_find("inspector-selection-editor").is_none());
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let decoded = f
            .view
            .read(cx)
            .inspector
            .selection_editor
            .read(cx)
            .value()
            .to_string();
        assert_eq!(decoded, "{\"user\":\"alice\"}");

        // Clearing selection when Inspector was auto-opened collapses Inspector cleanly.
        f.view.update(cx, |view, cx| {
            view.history_response
                .raw
                .update(cx, |s, cx| s.set_selected_range(0..0, cx));
        });
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());

        // Re-selecting auto-opens Inspector; clicking Notes rail tab switches cleanly to Notes (mutually exclusive).
        f.view.update(cx, |view, cx| {
            view.history_request
                .raw
                .update(cx, |s, cx| s.set_selected_range(14..24, cx));
        });
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert!(window.find("inspector-section-selection").visible());
        assert!(window.try_find("inspector-selection-editor").is_none());
        window.click("inspector-section-selection", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-selection-editor").visible());
        let right_origin_x = window.find("inspector-sidebar").bounds().origin.x;
        window.click("inspector-dock-left", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").bounds().origin.x < right_origin_x);
        window.click("inspector-dock-right", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inspector-sidebar").bounds().origin.x, right_origin_x);

        window.click("http-notes-toggle", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());
        assert!(window.find("http-notes-close").visible());

        // Clicking Inspector rail tab switches cleanly back from Notes to Inspector and pins it open.
        window.click("inspector-toggle", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert!(window.try_find("http-notes-close").is_none());

        // Clicking a read-only row opens detail/copy drawer without edit mutation buttons.
        assert!(window.try_find("inspector-item-query-0").is_none());
        window.click("inspector-section-query", cx);
        window.render_frame(cx);
        window.scroll(
            "inspector-sections",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-240.))),
            cx,
        );
        window.render_frame(cx);
        window.click("inspector-item-query-0", cx);
        window.render_frame(cx);
        assert!(window.find("inspector-copy-value").visible());
        assert!(window.try_find("inspector-add-button").is_none());
        assert!(window.try_find("inspector-apply-button").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn dual_pane_dismiss_stays_closed_same_bytes_new_range_reopens_and_whitespace_ignored(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![tab("1")], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-decoder", cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());

        // Pure whitespace selection in UTF-8 editor must never auto-open Inspector.
        f.view.update(cx, |view, cx| {
            view.extra_tools.decoder_input.update(cx, |s, cx| {
                s.set_value("   \n\t  aGVsbG8=  aGVsbG8=", window, cx);
                s.set_selected_range(0..6, cx);
            });
        });
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());

        // Select first occurrence in input -> auto-opens Inspector ("hello").
        f.view.update(cx, |view, cx| {
            view.extra_tools
                .decoder_input
                .update(cx, |s, cx| s.set_selected_range(7..15, cx));
        });
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .selection_editor
                .read(cx)
                .value()
                .as_ref(),
            "hello"
        );

        // Select a newer range in output ("d29ybGQ=" -> "world"); both panes now hold selections.
        f.view.update(cx, |view, cx| {
            view.extra_tools.decoder_output.update(cx, |s, cx| {
                s.set_value("d29ybGQ=", window, cx);
                s.focus(window, cx);
                s.set_selected_range(0..8, cx);
            });
        });
        window.render_frame(cx);
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .selection_editor
                .read(cx)
                .value()
                .as_ref(),
            "world"
        );

        // Dismissing via close button moves focus off decoder_output, yet Inspector must stay closed
        // on subsequent frames instead of flipping back to decoder_input's selection.
        window.click("inspector-close", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find("inspector-sidebar").is_none());

        // Selecting the exact same bytes ("aGVsbG8=") at a different byte range in decoder_input
        // produces a distinct SelectionIdentity and must auto-open Inspector again.
        f.view.update(cx, |view, cx| {
            view.extra_tools.decoder_input.update(cx, |s, cx| {
                s.focus(window, cx);
                s.set_selected_range(17..25, cx);
            });
        });
        window.render_frame(cx);
        assert!(window.find("inspector-sidebar").visible());
        assert_eq!(
            f.view
                .read(cx)
                .inspector
                .selection_editor
                .read(cx)
                .value()
                .as_ref(),
            "hello"
        );
    })
    .unwrap();
}
