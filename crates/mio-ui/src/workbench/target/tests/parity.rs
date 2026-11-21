use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};
use mio_core::{ExtHistoryRow, ExtItemSummary, ExtMessageSummary};

use super::open_target;
use super::super::*;
use crate::workbench::tests::{drain, fixture};

#[test]
fn mime_label_resolves_extensions_headers_magic_and_ext_archive() {
    let live_row = |method: &str, url: &str, status: Option<u16>, bytes: u64| {
        let mut rec = crate::workbench::tests::record(url, "");
        rec.summary.method = method.into();
        rec.summary.status = status;
        rec.summary.response_bytes = bytes;
        HistoryRow::Live(rec.summary)
    };
    assert_eq!(
        response_mime_label(&live_row("GET", "https://ex.com/a.html", None, 0), None),
        ""
    );
    for (url, expected) in [
        ("https://ex.com/a.html?x=1", "HTML"),
        ("https://ex.com/data.json", "JSON"),
        ("https://ex.com/app.mjs", "Script"),
        ("https://ex.com/icon.svg", "XML"),
        ("https://ex.com/main.css", "CSS"),
        ("https://ex.com/logo.webp", "Image"),
        ("https://ex.com/list.csv", "Text"),
    ] {
        assert_eq!(
            response_mime_label(&live_row("GET", url, Some(200), 10), None),
            expected
        );
    }
    assert_eq!(
        response_mime_label(
            &live_row("POST", "https://ex.com/v1/rpc", Some(200), 10),
            None
        ),
        "Data"
    );
    let endpoint = live_row("GET", "https://ex.com/v1/users", Some(200), 42);
    assert_eq!(response_mime_label(&endpoint, None), "HTML");
    assert_eq!(
        response_mime_label(
            &endpoint,
            Some(b"HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\n\r\n{}")
        ),
        "JSON"
    );
    assert_eq!(
        response_mime_label(
            &endpoint,
            Some(b"HTTP/1.1 200 OK\r\n\r\n\x89PNG\r\n\x1a\n\x00")
        ),
        "Image"
    );

    let source_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    let ext_archive = HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        item: ExtItemSummary {
            id: item_id,
            source_id,
            collection: "root.f1.f0".into(),
            index: 0,
            object_address: Some(1),
            state: "mapped".into(),
            message_count: 2,
        },
        request: Some(ExtMessageSummary {
            id: Uuid::new_v4(),
            source_id,
            item_id,
            collection: "root.f1.f0".into(),
            item_index: 0,
            role: "request".into(),
            field_id: 15,
            raw_offset: 0,
            raw_bytes: 32,
            body_offset: Some(32),
            declared_version: Some(HttpVersion::Http11),
            method: Some("GET".into()),
            request_target: Some("/feed.xml".into()),
            status: None,
            diagnostic: None,
        }),
        edited_request: None,
        response: None,
        edited_response: Some(ExtMessageSummary {
            id: Uuid::new_v4(),
            source_id,
            item_id,
            collection: "root.f1.f0".into(),
            item_index: 0,
            role: "response".into(),
            field_id: 20,
            raw_offset: 32,
            raw_bytes: 64,
            body_offset: Some(48),
            declared_version: Some(HttpVersion::Http11),
            method: None,
            request_target: None,
            status: Some(200),
            diagnostic: None,
        }),
        origin: Some("https://archive.example.com:443".into()),
        time_unix_ms: None,
        time_error: None,
        notes_revision: 0,
        notes_preview: String::new(),
        notes_error: None,
        highlight: None,
        highlight_error: None,
    }));
    let summary = target_row_summary(&ext_archive);
    assert_eq!(summary.mime, "XML");
    assert_eq!(summary.host, "https://archive.example.com");
    assert_eq!(summary.url, "/feed.xml");
}

#[gpui_kit::test]
fn sitemap_sorting_branch_copy_hide_scan_and_issues_subview_work(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);
    let mut r1 = crate::workbench::tests::record("https://api.example.com/b.json", "");
    r1.summary.method = "POST".into();
    r1.summary.status = Some(404);
    r1.summary.response_bytes = 250;
    let mut r2 = crate::workbench::tests::record("https://api.example.com/a.html", "");
    r2.summary.method = "GET".into();
    r2.summary.status = Some(200);
    r2.summary.response_bytes = 50;
    let mut r3 = crate::workbench::tests::record("https://other.example.com/c.css", "");
    r3.summary.method = "GET".into();
    r3.summary.status = Some(200);
    r3.summary.response_bytes = 120;
    let first_api_id = r1.summary.id;
    fixture.view.update(cx, |view, cx| {
        view.target_pane.tree_state.nodes = vec![
            SiteMapNode {
                key: "https://api.example.com:443".into(),
                parent_key: None,
                label: "https://api.example.com".into(),
                url_prefix: "https://api.example.com".into(),
                kind: SiteMapNodeKind::Origin,
                depth: 0,
                in_scope: true,
                direct_count: 2,
                subtree_count: 2,
            },
            SiteMapNode {
                key: "https://other.example.com:443".into(),
                parent_key: None,
                label: "https://other.example.com".into(),
                url_prefix: "https://other.example.com".into(),
                kind: SiteMapNodeKind::Origin,
                depth: 0,
                in_scope: false,
                direct_count: 1,
                subtree_count: 1,
            },
        ];
        view.target_pane.tree_state.ensure_origins();
        view.target_pane.items = vec![
            HistoryRow::Live(r1.summary.clone()),
            HistoryRow::Live(r2.summary.clone()),
            HistoryRow::Live(r3.summary.clone()),
        ];
        view.target_pane.total_items = 3;
        view.audit_dashboard.snapshot.findings = vec![mio_core::audit_engine::AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00100200".into(),
            title: "SQL 注入".into(),
            severity_label: "高危 High".into(),
            severity_rank: 4,
            confidence: mio_core::audit_engine::IssueConfidence::Certain,
            phase: mio_core::audit_engine::AuditPhase::Active,
            host: "api.example.com".into(),
            url: "https://api.example.com/b.json".into(),
            parameter: Some("id".into()),
            flow_id: Some(first_api_id),
            summary: "SQL error".into(),
            evidence: "syntax error".into(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "use parameterized query".into(),
            detected_unix_ms: 1,
        }];
        cx.notify();
    });
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("target-sitemap-sort-length", cx);
        })
        .unwrap();
    fixture.view.update(cx, |view, cx| {
        let idx = sort_site_map_indices(
            &view.target_pane.items,
            None,
            None,
            &view.target_pane.tree_state.hidden_prefixes,
            view.target_pane.item_sort,
        );
        assert_eq!(idx, vec![1, 2, 0]);
        view.copy_target_branch_urls("https://api.example.com:443".into(), cx);
    });
    assert_eq!(
        cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
        Some("https://api.example.com/b.json\nhttps://api.example.com/a.html".into())
    );
    fixture
        .window
        .update(cx, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.hide_target_branch("https://other.example.com:443".into(), window, cx);
            });
            window.render_frame(cx);
            assert_eq!(
                fixture
                    .view
                    .read(cx)
                    .target_pane
                    .tree_state
                    .visible_nodes()
                    .len(),
                1
            );
            window.click("target-sitemap-show-hidden", cx);
            window.render_frame(cx);
            assert_eq!(
                fixture
                    .view
                    .read(cx)
                    .target_pane
                    .tree_state
                    .visible_nodes()
                    .len(),
                2
            );
            window.click("target-sitemap-subtab-issues", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("target-sitemap-issue-title").label(),
                Some("SQL 注入")
            );
            fixture.view.update(cx, |view, cx| {
                view.scan_target_branch("https://api.example.com:443".into(), window, cx);
            });
            assert_eq!(
                fixture.view.read(cx).target_pane.selected_id,
                Some(HistoryEntryId::Live(first_api_id))
            );
            assert_eq!(fixture.view.read(cx).tool, Tool::Dashboard);
        })
        .unwrap();
}

#[gpui_kit::test]
fn scope_rule_inplace_edit_and_json_clipboard_guard_work(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("target-tab-scope", cx);
            window.render_frame(cx);
            window.click("target-scope-include-add", cx);
            window.render_frame(cx);
            window.click("target-scope-include-input", cx);
            window.input("https://old.example.com", cx);
            window.click("target-scope-include-add", cx);
            window.render_frame(cx);
            window.click(("target-scope-include-row", 0usize), cx);
            window.render_frame(cx);
            window.click("target-scope-include-edit-btn", cx);
            window.render_frame(cx);
            window.click("target-scope-include-input", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("https://new.example.com", cx);
            window.click("target-scope-include-add", cx);
            window.render_frame(cx);
            window.click("target-scope-settings-menu", cx);
            window.render_frame(cx);
            let copy_item = window
                .within("popup-menu")
                .find_by_label("复制 Scope (JSON)")
                .bounds()
                .center();
            window.pointer_down(copy_item, MouseButton::Left, cx);
            window.pointer_up(copy_item, MouseButton::Left, cx);
        })
        .unwrap();
    let exported = cx
        .read(|cx| cx.read_from_clipboard().and_then(|i| i.text()))
        .unwrap();
    assert!(exported.contains("https://new.example.com"));
    assert!(parse_target_scope_json("{}").is_err());
    assert!(parse_target_scope_json(r#"{"unknown":1}"#).is_err());
    fixture
        .window
        .update(cx, |_, window, cx| {
            let input = fixture
                .view
                .read(cx)
                .target_pane
                .simple_scope_inputs
                .include
                .clone();
            input.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            });
            window.click("target-scope-settings-menu", cx);
            window.render_frame(cx);
            let paste_item = window
                .within("popup-menu")
                .find_by_label("粘贴 Scope (JSON)")
                .bounds()
                .center();
            window.pointer_down(paste_item, MouseButton::Left, cx);
            window.pointer_up(paste_item, MouseButton::Left, cx);
        })
        .unwrap();
    fixture.view.update(cx, |view, _| {
        assert!(view.error.is_some());
        assert_eq!(view.workspace.scope.include.len(), 1);
        assert_eq!(
            view.workspace.scope.include[0].prefix,
            "https://new.example.com"
        );
    });
    fixture
        .window
        .update(cx, |_, window, cx| {
            let input = fixture
                .view
                .read(cx)
                .target_pane
                .simple_scope_inputs
                .include
                .clone();
            input.update(cx, |state, cx| {
                state.set_value("", window, cx);
            });
            fixture.view.update(cx, |view, _| {
                view.workspace.scope = TargetScope::default();
            });
            window.click("target-scope-settings-menu", cx);
            window.render_frame(cx);
            let paste_item = window
                .within("popup-menu")
                .find_by_label("粘贴 Scope (JSON)")
                .bounds()
                .center();
            window.pointer_down(paste_item, MouseButton::Left, cx);
            window.pointer_up(paste_item, MouseButton::Left, cx);
        })
        .unwrap();
    fixture.view.update(cx, |view, _| {
        assert_eq!(view.workspace.scope.include.len(), 1);
        assert_eq!(
            view.workspace.scope.include[0].prefix,
            "https://new.example.com"
        );
    });
}

#[gpui_kit::test]
fn scope_include_subdomains_checkbox_in_editor_and_table_persists(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("target-tab-scope", cx);
            window.render_frame(cx);
            window.click("target-scope-include-add", cx);
            window.render_frame(cx);
            window.click("target-scope-include-input", cx);
            window.input("https://example.com", cx);
            window.click("target-scope-include-subdomains-editor", cx);
            window.click("target-scope-include-save", cx);
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();
    fixture.view.update(cx, |view, _| {
        assert_eq!(view.workspace.scope.include.len(), 1);
        assert!(view.workspace.scope.include[0].include_subdomains);
    });
    let save_cmds = drain(&mut fixture.commands);
    let saved_ws = save_cmds
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::SaveWorkspace { workspace, .. }
                if workspace.scope.include.first().is_some_and(|r| r.include_subdomains) =>
            {
                Some(workspace)
            }
            _ => None,
        })
        .expect("SaveWorkspace emitted with include_subdomains=true");
    fixture
        .events
        .try_send(mio_core::Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved_ws,
        })
        .unwrap();
    cx.run_until_parked();
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("target-scope-include-subdomains-toggle", 0usize), cx);
            window.render_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();
    fixture.view.update(cx, |view, _| {
        assert!(!view.workspace.scope.include[0].include_subdomains);
    });
    let toggle_cmds = drain(&mut fixture.commands);
    assert!(toggle_cmds.iter().any(|cmd| matches!(
        cmd,
        Command::SaveWorkspace { workspace, .. }
            if workspace.scope.include.first().is_some_and(|r| !r.include_subdomains)
    )));
}

#[gpui_kit::test]
fn target_tree_background_restore_hidden_branches_triggers_refresh(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);
    fixture.view.update(cx, |view, cx| {
        view.target_pane.tree_state.nodes = vec![SiteMapNode {
            key: "https://hidden.example.com:443".into(),
            parent_key: None,
            label: "hidden.example.com".into(),
            url_prefix: "https://hidden.example.com".into(),
            kind: SiteMapNodeKind::Origin,
            depth: 0,
            in_scope: false,
            direct_count: 1,
            subtree_count: 1,
        }];
        cx.notify();
    });
    fixture
        .window
        .update(cx, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.hide_target_branch("https://hidden.example.com:443".into(), window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    drain(&mut fixture.commands);
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click("target-sitemap-tree", cx);
        })
        .unwrap();
    cx.run_until_parked();
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let item = window.find_by_label("恢复已隐藏分支").bounds().center();
            window.pointer_down(item, MouseButton::Left, cx);
            window.pointer_up(item, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    fixture.view.update(cx, |view, _| {
        assert!(view.target_pane.tree_state.hidden_prefixes.is_empty());
    });
    let cmds = drain(&mut fixture.commands);
    assert!(
        cmds.iter().any(|cmd| matches!(cmd, Command::QuerySiteMap { .. })),
        "tree background restore hidden branches must call show_hidden_target_branches and emit QuerySiteMap"
    );
}

#[gpui_kit::test]
fn target_branch_expand_engagement_tools_and_compare_work(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);
    let mut r1 = crate::workbench::tests::record("https://alpha.example.com/api/users?id=1", "");
    r1.summary.status = Some(200);
    let mut r2 = crate::workbench::tests::record("https://beta.example.com/api/orders", "");
    r2.summary.status = Some(200);
    let raw_resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
        <html><body><!-- internal debug token -->\
        <script src=\"/static/app.js\"></script>\
        <a href=\"/admin/panel\">Admin</a>\
        <form action=\"/submit\"><input name=\"csrf_token\" value=\"abc\"></form>\
        </body></html>";
    fixture
        .window
        .update(cx, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.target_pane.tree_state.nodes = vec![
                    SiteMapNode {
                        key: "https://alpha.example.com:443".into(),
                        parent_key: None,
                        label: "alpha.example.com".into(),
                        url_prefix: "https://alpha.example.com".into(),
                        kind: SiteMapNodeKind::Origin,
                        depth: 0,
                        in_scope: true,
                        direct_count: 0,
                        subtree_count: 2,
                    },
                    SiteMapNode {
                        key: "https://alpha.example.com:443/api".into(),
                        parent_key: Some("https://alpha.example.com:443".into()),
                        label: "api".into(),
                        url_prefix: "https://alpha.example.com/api".into(),
                        kind: SiteMapNodeKind::Folder,
                        depth: 1,
                        in_scope: true,
                        direct_count: 0,
                        subtree_count: 1,
                    },
                    SiteMapNode {
                        key: "https://alpha.example.com:443/api/users".into(),
                        parent_key: Some("https://alpha.example.com:443/api".into()),
                        label: "users".into(),
                        url_prefix: "https://alpha.example.com/api/users".into(),
                        kind: SiteMapNodeKind::File,
                        depth: 2,
                        in_scope: true,
                        direct_count: 1,
                        subtree_count: 1,
                    },
                    SiteMapNode {
                        key: "https://alpha.example.com:443/unrequested".into(),
                        parent_key: Some("https://alpha.example.com:443".into()),
                        label: "unrequested".into(),
                        url_prefix: "https://alpha.example.com/unrequested".into(),
                        kind: SiteMapNodeKind::Folder,
                        depth: 1,
                        in_scope: true,
                        direct_count: 0,
                        subtree_count: 0,
                    },
                    SiteMapNode {
                        key: "https://beta.example.com:443".into(),
                        parent_key: None,
                        label: "beta.example.com".into(),
                        url_prefix: "https://beta.example.com".into(),
                        kind: SiteMapNodeKind::Origin,
                        depth: 0,
                        in_scope: false,
                        direct_count: 1,
                        subtree_count: 1,
                    },
                ];
                view.target_pane.items = vec![
                    HistoryRow::Live(r1.summary.clone()),
                    HistoryRow::Live(r2.summary.clone()),
                ];
                view.target_pane.total_items = 2;
                view.target_pane.selected_id = Some(HistoryEntryId::Live(r1.summary.id));
                view.target_pane.selected_row = Some(HistoryRow::Live(r1.summary.clone()));
                view.target_pane.selected_response_bytes = raw_resp.to_vec();
                view.target_pane.response.load(raw_resp, window, cx);

                view.expand_target_branch("https://alpha.example.com:443".into(), cx);
                assert!(view
                    .target_pane
                    .tree_state
                    .expanded
                    .contains("https://alpha.example.com:443/unrequested"));
                view.collapse_target_branch("https://alpha.example.com:443".into(), cx);
                assert!(view.target_pane.tree_state.expanded.is_empty());
                view.expand_target_requested_items("https://alpha.example.com:443".into(), cx);
                assert!(view
                    .target_pane
                    .tree_state
                    .expanded
                    .contains("https://alpha.example.com:443/api"));
                assert!(!view
                    .target_pane
                    .tree_state
                    .expanded
                    .contains("https://alpha.example.com:443/unrequested"));

                view.run_target_find_comments(
                    Some("https://alpha.example.com:443".into()),
                    window,
                    cx,
                );
                let comments = view.target_pane.engagement_report.as_ref().unwrap();
                assert!(comments
                    .entries
                    .iter()
                    .any(|e| e.detail.contains("internal debug token")));

                view.run_target_find_scripts(
                    Some("https://alpha.example.com:443".into()),
                    window,
                    cx,
                );
                let scripts = view.target_pane.engagement_report.as_ref().unwrap();
                assert!(scripts
                    .entries
                    .iter()
                    .any(|e| e.detail.contains("/static/app.js")));

                view.run_target_find_references(
                    Some("https://alpha.example.com:443".into()),
                    window,
                    cx,
                );
                let refs = view.target_pane.engagement_report.as_ref().unwrap();
                assert!(refs
                    .entries
                    .iter()
                    .any(|e| e.detail.contains("/admin/panel")));

                view.run_target_analyze(
                    Some("https://alpha.example.com:443".into()),
                    window,
                    cx,
                );
                let analysis = view.target_pane.engagement_report.as_ref().unwrap();
                assert!(analysis
                    .entries
                    .iter()
                    .any(|e| e.detail.contains("csrf_token")));

                view.open_compare_site_maps(
                    Some("https://alpha.example.com:443".into()),
                    window,
                    cx,
                );
                let cmp = view.target_pane.compare_modal.as_ref().unwrap();
                assert_eq!(cmp.only_left, 1);
                assert_eq!(cmp.only_right, 1);
            });
        })
        .unwrap();
}

#[gpui_kit::test]
fn target_contents_mio_at_csrf_record_issue_browser_and_delete_work(cx: &mut TestAppContext) {
    use mio_core::{AuditCommand, BrowserRequestSession};

    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);
    let mut row = crate::workbench::tests::record("https://shop.example.com/cart/checkout", "item=42");
    row.summary.method = "POST".into();
    row.summary.status = Some(200);
    let flow_id = row.summary.id;
    let raw_req = b"POST /cart/checkout HTTP/1.1\r\nHost: shop.example.com\r\nContent-Type: application/x-www-form-urlencoded\r\nCookie: sid=orig\r\n\r\nitem=42";
    let existing_finding_id = Uuid::new_v4();

    fixture
        .window
        .update(cx, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.target_pane.items = vec![HistoryRow::Live(row.summary.clone())];
                view.target_pane.total_items = 1;
                view.target_pane.selected_id = Some(HistoryEntryId::Live(flow_id));
                view.target_pane.selected_row = Some(HistoryRow::Live(row.summary.clone()));
                view.target_pane.selected_live_record = Some(Box::new(row.clone()));
                view.target_pane.request.load(raw_req, window, cx);

                view.send_selected_target_to_mio_at(window, cx);
                assert!(view.mio_ai_panel_open);
                assert!(view
                    .mio_ai_context
                    .as_deref()
                    .is_some_and(|c| c.contains("https://shop.example.com/cart/checkout")));

                view.open_target_csrf_poc_modal(window, cx);
                let (_, html) = view.csrf_poc_modal.as_ref().expect("CSRF PoC modal opened");
                assert!(html.contains("<form"));
                assert!(html.contains("item"));

                view.record_manual_issue_from_target(cx);
                view.attach_target_row_to_issue(existing_finding_id, cx);
                view.open_current_request_in_browser_with_session(true, window, cx);
                view.open_current_request_in_browser_with_session(false, window, cx);
                view.delete_selected_target_item(window, cx);
                assert!(view.target_pane.items.is_empty());
                assert!(view
                    .target_pane
                    .hidden_item_ids
                    .contains(&HistoryEntryId::Live(flow_id)));
            });
        })
        .unwrap();

    let cmds = drain(&mut fixture.commands);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Audit(AuditCommand::RecordManualFinding { finding, .. })
            if finding.url == "https://shop.example.com/cart/checkout"
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Audit(AuditCommand::AttachFlowToFinding { finding_id, .. })
            if *finding_id == existing_finding_id
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::OpenConfiguredBrowser {
            session: BrowserRequestSession::OriginalSession { .. },
            ..
        }
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::OpenConfiguredBrowser {
            session: BrowserRequestSession::CurrentSession,
            ..
        }
    )));
}

#[gpui_kit::test]
fn target_sitemap_issues_row_context_menu_triage_comparer_mio_at_and_delete(
    cx: &mut TestAppContext,
) {
    use mio_core::{
        AuditCommand,
        audit_engine::{AuditFinding, AuditPhase, IssueConfidence},
        target_issues::IssueSeverity,
    };

    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);

    let finding_id = Uuid::new_v4();
    fixture.view.update(cx, |view, cx| {
        view.target_pane.site_map_subview = SiteMapSubView::Issues;
        view.audit_dashboard.snapshot.findings = vec![AuditFinding {
            id: finding_id,
            issue_id: "0x00100200".into(),
            title: "SQL 注入".into(),
            severity_label: IssueSeverity::High.label().into(),
            severity_rank: IssueSeverity::High.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Active,
            host: "api.example.com".into(),
            url: "https://api.example.com/v1/items".into(),
            parameter: Some("id".into()),
            flow_id: None,
            summary: "SQL error detected".into(),
            evidence: "syntax error near '".into(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "Use parameterized queries".into(),
            detected_unix_ms: 100,
        }];
        cx.notify();
    });

    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(("target-sitemap-issue-row", 0usize), cx);
        })
        .unwrap();
    cx.run_until_parked();
    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find_by_label("报告问题").visible());
            assert!(window.find_by_label("删除问题").visible());
            assert!(window.find_by_label("设置严重性").visible());
            assert!(window.find_by_label("设置置信度").visible());
            assert!(window.find_by_label("发送到 Comparer").visible());
            assert!(window.find_by_label("发送到 MioAT").visible());
        })
        .unwrap();

    fixture
        .window
        .update(cx, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.set_target_finding_severity(finding_id, IssueSeverity::Low, cx);
                assert_eq!(
                    view.audit_dashboard.snapshot.findings[0].severity(),
                    IssueSeverity::Low
                );
                view.restore_target_finding_severity(finding_id, cx);
                assert_eq!(
                    view.audit_dashboard.snapshot.findings[0].severity(),
                    IssueSeverity::High
                );

                view.set_target_finding_confidence(finding_id, IssueConfidence::Tentative, false, cx);
                assert_eq!(
                    view.audit_dashboard.snapshot.findings[0].confidence,
                    IssueConfidence::Tentative
                );
                view.restore_target_finding_confidence(finding_id, cx);
                assert_eq!(
                    view.audit_dashboard.snapshot.findings[0].confidence,
                    IssueConfidence::Certain
                );

                view.send_target_issue_to_mio_at(finding_id, cx);
                assert!(view.mio_ai_panel_open);

                view.send_target_issue_to_comparer(finding_id, window, cx);
                assert_eq!(view.tool, Tool::Comparer);
                assert_eq!(view.extra_tools.comparer_items.len(), 1);

                view.delete_target_site_map_finding(finding_id, window, cx);
                assert!(view.audit_dashboard.snapshot.findings.is_empty());
            });
        })
        .unwrap();

    let cmds = drain(&mut fixture.commands);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Audit(AuditCommand::SetFindingSeverity {
            finding_id: id,
            severity: IssueSeverity::Low,
            ..
        }) if *id == finding_id
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Audit(AuditCommand::DeleteFinding { finding_id: id, .. }) if *id == finding_id
    )));
}

#[gpui_kit::test]
fn target_sitemap_multiselect_column_layout_copy_and_batch_delete_work(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    open_target(&fixture, cx);

    let mut r1 = crate::workbench::tests::record("https://one.example.com/a", "a");
    r1.summary.url = "https://one.example.com/a".into();
    let mut r2 = crate::workbench::tests::record("https://two.example.com/b", "b");
    r2.summary.url = "https://two.example.com/b".into();
    let mut r3 = crate::workbench::tests::record("https://three.example.com/c", "c");
    r3.summary.url = "https://three.example.com/c".into();
    let rows = vec![
        HistoryRow::Live(r1.summary.clone()),
        HistoryRow::Live(r2.summary.clone()),
        HistoryRow::Live(r3.summary.clone()),
    ];
    let (id1, id2, id3) = (rows[0].id(), rows[1].id(), rows[2].id());

    fixture.view.update(cx, |view, cx| {
        view.target_pane.items = rows.clone();
        view.target_pane.total_items = 3;
        cx.notify();
    });

    fixture
        .window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("target-sitemap-sort-method").visible());

            let visible_order = vec![id1, id2, id3];
            fixture.view.update(cx, |view, cx| {
                view.handle_target_row_click(&rows[0], &visible_order, false, false, window, cx);
                view.handle_target_row_click(&rows[2], &visible_order, true, false, window, cx);
                assert_eq!(view.selected_target_rows_in_order().len(), 3);
                view.handle_target_row_click(&rows[1], &visible_order, false, true, window, cx);
                assert_eq!(
                    view.selected_target_rows_in_order()
                        .iter()
                        .map(HistoryRow::id)
                        .collect::<Vec<_>>(),
                    vec![id1, id3]
                );
                view.handle_target_row_right_down(&rows[0], window, cx);
                assert_eq!(view.selected_target_rows_in_order().len(), 2);

                view.copy_target_url(cx);
            });
        })
        .unwrap();
    assert_eq!(
        cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
        Some("https://one.example.com/a\nhttps://three.example.com/c".into())
    );

    fixture
        .window
        .update(cx, |_, window, cx| {
            let summaries: Vec<(usize, super::super::mime::TargetRowSummary)> = rows
                .iter()
                .enumerate()
                .map(|(i, row)| (i, target_row_summary(row)))
                .collect();
            fixture.view.update(cx, |view, cx| {
                view.copy_target_column(0, &summaries, cx);
                view.toggle_target_column_visibility(1, cx);
                view.fit_target_column_width(2, &summaries, cx);
            });
            window.render_frame(cx);
            assert!(window.try_find("target-sitemap-sort-method").is_none());
            assert_eq!(
                cx.read_from_clipboard().and_then(|i| i.text()),
                Some("https://one.example.com\nhttps://two.example.com\nhttps://three.example.com".into())
            );

            fixture.view.update(cx, |view, cx| {
                view.reset_target_column_layout(cx);
                view.delete_selected_target_item(window, cx);
            });
            window.render_frame(cx);
            assert!(window.find("target-sitemap-sort-method").visible());
            assert_eq!(
                fixture
                    .view
                    .read(cx)
                    .target_pane
                    .items
                    .iter()
                    .map(HistoryRow::id)
                    .collect::<Vec<_>>(),
                vec![id2]
            );
        })
        .unwrap();
}
