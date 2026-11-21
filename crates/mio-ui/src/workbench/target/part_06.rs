use super::*;

impl Workbench {
    pub(in crate::workbench) fn add_advanced_scope_rule_from_prefix(
        &mut self,
        prefix: String,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        if prefix.contains("://") {
            let (protocol, host, port, file) = split_advanced_scope_url(&prefix);
            let rule = AdvancedScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                protocol,
                host,
                port,
                file,
            };
            let mut next = self.workspace.scope.clone();
            if include {
                next.advanced_include.push(rule);
            } else {
                next.advanced_exclude.push(rule);
            }
            if let Err(error) = next.validate() {
                self.fail(error.to_string(), cx);
                return;
            }
            self.workspace.scope = next;
            self.save_workspace(cx);
            return;
        }
        self.add_scope_rule(self.project, include, prefix, cx);
    }

    pub(in crate::workbench) fn modify_selected_target_scope(
        &mut self,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            return;
        };
        let url = match row {
            HistoryRow::Live(summary) => summary.url.clone(),
            HistoryRow::ExtProxy(archive) => {
                let Some(request) = archive.request.as_ref().or(archive.edited_request.as_ref())
                else {
                    self.fail("归档条目缺少请求 URL，未修改目标范围。", cx);
                    return;
                };
                let target = request.request_target.clone().unwrap_or_default();
                if target.starts_with('/') {
                    format!("{}{target}", archive.origin.as_deref().unwrap_or_default())
                } else {
                    target
                }
            }
        };
        let prefix = target_origin_from_url(&url);
        if self.workspace.scope.advanced_mode {
            self.add_advanced_scope_rule_from_prefix(prefix, include, cx);
        } else {
            self.add_scope_rule(self.project, include, prefix, cx);
        }
    }

    pub(in crate::workbench) fn send_target_history_transfer(
        &mut self,
        intruder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.selected_target_rows_in_order();
        if rows.is_empty() {
            return;
        }
        let entries: Vec<HistoryTransferEntry> = rows
            .iter()
            .filter_map(|row| self.target_transfer_entry(row))
            .collect();
        if entries.is_empty() {
            self.fail("所选条目没有可转送的请求；未创建标签。", cx);
            return;
        }
        self.begin_history_transfer(None, entries, intruder, window, cx);
    }

    pub(in crate::workbench) fn split_advanced_scope_url(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .target_pane
            .advanced_scope_inputs
            .include
            .first()
            .cloned()
        else {
            return;
        };
        let url = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let (protocol, host, port, file) = split_advanced_scope_url(&url);
        if protocol != ScopeProtocol::Any {
            self.target_pane.scope_protocol = protocol;
        }
        for (input, value) in self
            .target_pane
            .advanced_scope_inputs
            .include
            .iter()
            .zip([host, port, file])
        {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests_parity;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::tests::{Fixture, drain, fixture};
    use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
    use gpui_kit::{MouseButton, TestAppContext};

    fn open_target(fixture: &Fixture, cx: &mut TestAppContext) {
        fixture.view.update(cx, |view, cx| {
            view.tool = Tool::Target;
            cx.notify();
        });
    }

    #[gpui_kit::test]
    fn target_tree_renders_selects_and_adds_scope(cx: &mut TestAppContext) {
        let mut fixture = fixture(cx, Vec::new(), 64);
        drain(&mut fixture.commands);
        open_target(&fixture, cx);
        fixture.view.update(cx, |view, cx| {
            view.target_pane.tree_state.nodes = vec![SiteMapNode {
                key: "https://api.example.com:443".into(),
                parent_key: None,
                label: "api.example.com".into(),
                url_prefix: "https://api.example.com".into(),
                kind: SiteMapNodeKind::Origin,
                depth: 0,
                in_scope: false,
                direct_count: 1,
                subtree_count: 2,
            }];
            view.target_pane.tree_state.ensure_origins();
            cx.notify();
        });
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click(("target-sitemap-node", 0usize), cx);
            })
            .unwrap();
        let command = drain(&mut fixture.commands).pop();
        assert!(matches!(command, Some(Command::QuerySiteMap { .. })));
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.right_click(("target-sitemap-node", 0usize), cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let item = window.find_by_label("添加到范围").bounds().center();
                window.pointer_down(item, MouseButton::Left, cx);
                window.pointer_up(item, MouseButton::Left, cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture.view.update(cx, |view, _| {
            assert!(view.workspace.scope.include.iter().any(|rule| rule.enabled));
        });
        for command in drain(&mut fixture.commands) {
            if let Command::SaveWorkspace { project, workspace } = command {
                fixture
                    .events
                    .try_send(Event::WorkspaceChanged { project, workspace })
                    .unwrap();
            }
        }
        cx.run_until_parked();
        let mut live = crate::workbench::tests::record("https://api.example.com/v1/login", "ok=1");
        live.summary.status = Some(200);
        let flow_id = live.summary.id;
        fixture.view.update(cx, |view, cx| {
            view.target_pane.items = vec![HistoryRow::Live(live.summary.clone())];
            view.target_pane.total_items = 1;
            cx.notify();
        });
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.right_click(("target-sitemap-row", 0usize), cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let item = window.find_by_label("发送到 Repeater").bounds().center();
                window.pointer_down(item, MouseButton::Left, cx);
                window.pointer_up(item, MouseButton::Left, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let transfers = drain(&mut fixture.commands);
        assert!(transfers.iter().any(|command| matches!(
            command,
            Command::ReadHistoryTransfer { entries, .. }
                if matches!(entries.as_slice(), [HistoryTransferEntry::Live { flow_id: id, .. }] if *id == flow_id)
        )));
    }

    #[gpui_kit::test]
    fn advanced_scope_adds_valid_and_keeps_invalid_input(cx: &mut TestAppContext) {
        let mut fixture = fixture(cx, Vec::new(), 64);
        drain(&mut fixture.commands);
        open_target(&fixture, cx);
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-tab-scope", cx);
                window.render_frame(cx);
                window.click("target-scope-advanced-toggle", cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-scope-adv-include-add", cx);
                window.render_frame(cx);
                window.click("target-scope-advanced-host", cx);
                window.input("^api$", cx);
            })
            .unwrap();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-scope-adv-include-add", cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture.view.update(cx, |view, _| {
            assert_eq!(view.workspace.scope.advanced_include.len(), 1);
        });
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-scope-adv-include-add", cx);
                window.render_frame(cx);
                window.click("target-scope-advanced-host", cx);
                window.press(crate::kbd!("a"), cx);
                window.input("[", cx);
                window.click("target-scope-adv-include-add", cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(
                    window.find("target-scope-advanced-host").value().unwrap(),
                    "["
                );
            })
            .unwrap();
        fixture.view.update(cx, |view, _| {
            assert_eq!(view.workspace.scope.advanced_include.len(), 1);
        });
    }

    #[gpui_kit::test]
    fn issue_definitions_filter_and_show_details(cx: &mut TestAppContext) {
        let mut fixture = fixture(cx, Vec::new(), 64);
        drain(&mut fixture.commands);
        open_target(&fixture, cx);
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-tab-issues", cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("target-issues-search").is_none());
                assert_eq!(
                    window.find("target-issue-title").label(),
                    Some("操作系统命令注入 (OS command injection)")
                );
                fixture.view.update(cx, |view, cx| {
                    view.target_pane.issue_filter_open = true;
                    cx.notify();
                });
                window.render_frame(cx);
                window.click("target-issues-search", cx);
                window.input("sql", cx);
                window.click("target-issues-apply", cx);
                window.render_frame(cx);
                let row = window
                    .find_by_label("SQL 注入 (SQL injection) 0x00100200")
                    .bounds()
                    .center();
                window.pointer_down(row, MouseButton::Left, cx);
                window.pointer_up(row, MouseButton::Left, cx);
            })
            .unwrap();
        cx.run_until_parked();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(
                    window.find("target-issue-title").label(),
                    Some("SQL 注入 (SQL injection)")
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn sitemap_status_class_buttons_branch_real_headers_and_highlight_work(cx: &mut TestAppContext) {
        use mio_core::{AuditCommand, AuditTaskMode, Header};

        let mut fixture = fixture(cx, Vec::new(), 64);
        drain(&mut fixture.commands);
        open_target(&fixture, cx);

        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("target-sitemap-filter-bar", cx);
                window.render_frame(cx);
                window.click("target-sitemap-status-5xx", cx);
            })
            .unwrap();
        let cmds = drain(&mut fixture.commands);
        let query = cmds
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::QuerySiteMap { query, .. } => Some(query),
                _ => None,
            })
            .expect("QuerySiteMap emitted on status class toggle");
        assert_eq!(query.filter.status_classes, [true, true, false, false]);
        fixture.view.update(cx, |view, cx| {
            view.target_pane.site_map_loading = false;
            cx.notify();
        });

        let mut captured = crate::workbench::tests::record("https://api.example.com/v1/profile", "");
        captured.summary.status = Some(200);
        captured.request.headers.push(Header {
            name: "Cookie".into(),
            value: b"session=real_secret_token".to_vec(),
        });
        let flow_id = captured.summary.id;
        fixture.view.update(cx, |view, cx| {
            view.target_pane.tree_state.nodes = vec![SiteMapNode {
                key: "https://api.example.com:443".into(),
                parent_key: None,
                label: "api.example.com".into(),
                url_prefix: "https://api.example.com".into(),
                kind: SiteMapNodeKind::Origin,
                depth: 0,
                in_scope: true,
                direct_count: 1,
                subtree_count: 1,
            }];
            view.target_pane.tree_state.ensure_origins();
            view.target_pane.items = vec![HistoryRow::Live(captured.summary.clone())];
            view.target_pane.total_items = 1;
            cx.notify();
        });

        fixture
            .window
            .update(cx, |_, window, cx| {
                fixture.view.update(cx, |view, cx| {
                    view.run_target_branch_action(
                        "https://api.example.com:443".into(),
                        BranchItemAction::PassiveScan,
                        window,
                        cx,
                    );
                });
            })
            .unwrap();
        let read_rev = drain(&mut fixture.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::ReadFlow {
                    selection_revision,
                    flow_id: fid,
                    ..
                } if fid == flow_id => Some(selection_revision),
                _ => None,
            })
            .expect("ReadFlow dispatched before starting branch scan");

        fixture
            .events
            .try_send(Event::FlowDetail {
                project: fixture.project,
                selection_revision: read_rev,
                record: Box::new(captured.clone()),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();

        let scan_cmds = drain(&mut fixture.commands);
        let started_draft = scan_cmds
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::Audit(AuditCommand::StartScan {
                    draft,
                    mode: AuditTaskMode::PassiveOnly,
                    ..
                }) => Some(draft),
                _ => None,
            })
            .expect("Passive StartScan dispatched with real flow request");
        let raw_str = String::from_utf8_lossy(&started_draft.raw);
        assert!(
            raw_str.contains("Cookie: session=real_secret_token"),
            "branch scan must preserve real captured Cookie header, got: {raw_str}"
        );

        fixture.view.update(cx, |view, cx| {
            view.set_target_row_highlight(
                HistoryEntryId::Live(flow_id),
                Some(HistoryRow::Live(captured.summary.clone())),
                3,
                cx,
            );
        });
        let hl_cmds = drain(&mut fixture.commands);
        assert!(hl_cmds.iter().any(|cmd| matches!(
            cmd,
            Command::SetExtHttpHighlights { changes, .. }
                if changes.iter().any(|c| c.item_id == flow_id && c.color == 3)
        )));
    }

    #[gpui_kit::test]
    fn dashboard_task_card_filters_issues_and_saves_report_to_disk(cx: &mut TestAppContext) {
        use mio_core::{
            AuditEvent, AuditTaskMode, AuditTaskState, AuditTaskSummary,
            audit_engine::{AuditFinding, AuditPhase, IssueConfidence},
            target_issues::IssueSeverity,
        };

        let mut fixture = fixture(cx, Vec::new(), 64);
        drain(&mut fixture.commands);
        fixture.view.update(cx, |view, cx| {
            view.tool = Tool::Dashboard;
            view.activate_pro_dashboard(cx);
            cx.notify();
        });
        drain(&mut fixture.commands);

        let task_a = AuditTaskSummary {
            id: Uuid::new_v4(),
            title: "Task Alpha".into(),
            target_url: "https://alpha.example.com/api".into(),
            mode: AuditTaskMode::PassiveAndActive,
            state: AuditTaskState::Completed,
            requests_sent: 5,
            total_planned: 5,
            issues_found: 1,
            started_unix_ms: 10,
            error: None,
        };
        let finding_alpha = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00100200".into(),
            title: "Alpha SQL 注入".into(),
            severity_label: IssueSeverity::High.label().into(),
            severity_rank: IssueSeverity::High.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Active,
            host: "alpha.example.com".into(),
            url: "https://alpha.example.com/api/users".into(),
            parameter: Some("id".into()),
            flow_id: None,
            summary: "Alpha finding".into(),
            evidence: "SQL syntax".into(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "Use prepared statements".into(),
            detected_unix_ms: 11,
        };
        let finding_beta = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00200300".into(),
            title: "Beta XSS".into(),
            severity_label: IssueSeverity::Medium.label().into(),
            severity_rank: IssueSeverity::Medium.rank(),
            confidence: IssueConfidence::Firm,
            phase: AuditPhase::Passive,
            host: "beta.example.com".into(),
            url: "https://beta.example.com/home".into(),
            parameter: None,
            flow_id: None,
            summary: "Beta finding".into(),
            evidence: "<svg>".into(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "Encode output".into(),
            detected_unix_ms: 12,
        };
        let task_id = task_a.id;
        let alpha_id = finding_alpha.id;
        let beta_id = finding_beta.id;

        fixture
            .events
            .try_send(Event::Audit(AuditEvent::TaskUpdated {
                project: fixture.project,
                revision: 2,
                task: task_a,
                new_findings: vec![finding_alpha, finding_beta],
            }))
            .unwrap();
        cx.run_until_parked();

        fixture
            .window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click(
                    SharedString::from(format!("dashboard-audit-task-{task_id}")),
                    cx,
                );
                window.render_frame(cx);
                assert!(window.find("dashboard-task-filter-banner").visible());
                assert!(
                    window
                        .find(SharedString::from(format!(
                            "dashboard-issue-activity-row-{alpha_id}"
                        )))
                        .visible()
                );
                assert!(
                    window
                        .try_find(SharedString::from(format!(
                            "dashboard-issue-activity-row-{beta_id}"
                        )))
                        .is_none()
                );
                window.click("dashboard-show-all-task-issues", cx);
                window.render_frame(cx);
                assert!(
                    window
                        .find(SharedString::from(format!(
                            "dashboard-issue-activity-row-{beta_id}"
                        )))
                        .visible()
                );
            })
            .unwrap();

        let out_dir = std::env::current_dir()
            .unwrap()
            .join(".local")
            .join(format!("test-audit-export-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let html_path = out_dir.join("report.html");
        let single_json_path = out_dir.join("single.json");

        fixture
            .window
            .update(cx, |_, window, cx| {
                fixture.view.update(cx, |view, cx| {
                    view.save_audit_report_to_file(true, None, window, cx);
                });
            })
            .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(html_path.clone()));
        cx.run_until_parked();
        let saved_html = std::fs::read_to_string(&html_path).unwrap();
        assert!(saved_html.contains("<!DOCTYPE html>"));
        assert!(saved_html.contains("Alpha SQL 注入"));
        assert!(saved_html.contains("Beta XSS"));

        fixture
            .window
            .update(cx, |_, window, cx| {
                fixture.view.update(cx, |view, cx| {
                    view.save_audit_report_to_file(false, Some(alpha_id), window, cx);
                });
            })
            .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(single_json_path.clone()));
        cx.run_until_parked();
        let saved_json = std::fs::read_to_string(&single_json_path).unwrap();
        assert!(saved_json.contains("Alpha SQL 注入"));
        assert!(!saved_json.contains("Beta XSS"));

        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[gpui_kit::test]
    fn add_to_site_map_from_repeater_navigates_reads_detail_and_transfers(cx: &mut TestAppContext) {
        let mut initial = crate::workbench::tests::tab("1");
        initial.draft.target = "https://portal.example.com:443".into();
        initial.draft.raw = b"POST /api/checkout HTTP/1.1\r\nHost: portal.example.com\r\nContent-Length: 7\r\n\r\npayload".to_vec();
        let raw_req = initial.draft.raw.clone();
        let tab_id = initial.id;
        let mut fixture = fixture(cx, vec![initial], 64);
        drain(&mut fixture.commands);

        let raw_resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_vec();
        fixture
            .window
            .update(cx, |_, window, cx| {
                window.click("tool-repeater", cx);
                fixture.view.update(cx, |view, cx| {
                    view.tab_editors
                        .get_mut(&tab_id)
                        .unwrap()
                        .response
                        .load(&raw_resp, window, cx);
                });
                window.dispatch_action(Box::new(crate::AddToSiteMap), cx);
            })
            .unwrap();
        cx.run_until_parked();

        let cmds = drain(&mut fixture.commands);
        let (op_id, ws_rev) = cmds
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::AddSiteMapEntry {
                    operation_id,
                    workspace_revision,
                    entry:
                        SiteMapEntryInput::Draft {
                            owner_id,
                            source: mio_core::TrafficSource::Repeater,
                            draft,
                            response_raw,
                        },
                    ..
                } if owner_id == tab_id
                    && draft.raw == raw_req
                    && response_raw.as_deref() == Some(raw_resp.as_slice()) =>
                {
                    Some((operation_id, workspace_revision))
                }
                _ => None,
            })
            .expect("AddSiteMapEntry emitted from Repeater");

        let entry_id = Uuid::new_v4();
        let summary = mio_core::FlowSummary {
            id: entry_id,
            revision: 1,
            source: mio_core::TrafficSource::Repeater,
            method: "POST".into(),
            url: "https://portal.example.com:443/api/checkout".into(),
            status: Some(200),
            state: mio_core::FlowState::Pending,
            request_bytes: 7,
            response_bytes: 11,
            started_unix_ms: 0,
            elapsed_ms: 0,
        };
        let row = HistoryRow::Live(summary.clone());
        fixture
            .events
            .try_send(Event::SiteMapEntryAdded {
                project: fixture.project,
                operation_id: op_id,
                workspace_revision: ws_rev,
                result: Ok(SiteMapEntryReceipt {
                    entry_id,
                    revision: 1,
                    node_key: "https://portal.example.com:443/api/checkout".into(),
                    row: row.clone(),
                }),
            })
            .unwrap();
        cx.run_until_parked();

        fixture.view.update(cx, |view, _| {
            assert_eq!(view.tool, Tool::Target);
            assert_eq!(view.target_pane.page, TargetPage::SiteMap);
            assert_eq!(
                view.target_pane.selected_id,
                Some(HistoryEntryId::Live(entry_id))
            );
        });
        let after_add = drain(&mut fixture.commands);
        let sel_rev = after_add
            .iter()
            .find_map(|cmd| match cmd {
                Command::ReadSiteMapEntry {
                    selection_revision,
                    entry_id: id,
                    revision: 1,
                    ..
                } if *id == entry_id => Some(*selection_revision),
                _ => None,
            })
            .expect("ReadSiteMapEntry dispatched on selection");

        let parsed_req =
            mio_core::parse_repeater_request(&raw_req, "https://portal.example.com:443", HttpVersion::Http11, false)
                .unwrap();
        let parsed_resp = mio_core::parse_response(&raw_resp, HttpVersion::Http11).unwrap();
        fixture
            .events
            .try_send(Event::SiteMapEntryRead {
                project: fixture.project,
                selection_revision: sel_rev,
                entry_id,
                revision: 1,
                result: Ok(SiteMapEntryDetail {
                    row_id: HistoryEntryId::Live(entry_id),
                    record: FlowRecord {
                        summary,
                        request: parsed_req,
                        response: Some(parsed_resp),
                    },
                    original: OriginalMessages::default(),
                    request_raw: raw_req.clone(),
                    response_raw: Some(raw_resp.clone()),
                    original_request_raw: None,
                    original_response_raw: None,
                }),
            })
            .unwrap();
        cx.run_until_parked();

        fixture
            .window
            .update(cx, |_, window, cx| {
                let loaded_req = fixture
                    .view
                    .update(cx, |view, cx| view.target_pane.request.bytes(window, cx).unwrap());
                let loaded_resp = fixture
                    .view
                    .update(cx, |view, cx| view.target_pane.response.bytes(window, cx).unwrap());
                assert_eq!(loaded_req, raw_req);
                assert_eq!(loaded_resp, raw_resp);
                fixture
                    .view
                    .update(cx, |view, cx| view.send_target_history_transfer(false, window, cx));
            })
            .unwrap();
        cx.run_until_parked();
        let transfer_cmds = drain(&mut fixture.commands);
        assert!(transfer_cmds.iter().any(|cmd| matches!(
            cmd,
            Command::ReadHistoryTransfer { entries, .. }
                if matches!(
                    entries.as_slice(),
                    [HistoryTransferEntry::SiteMap {
                        row_id: HistoryEntryId::Live(r),
                        entry_id: e,
                        revision: 1
                    }] if *r == entry_id && *e == entry_id
                )
        )));
    }
}
