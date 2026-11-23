use gpui_kit::test::TestWindowExt;
use gpui_kit::TestAppContext;

use super::click;
use super::super::*;
use crate::workbench::tests::{drain, fixture};


#[gpui_kit::test]
fn toolbar_copies_json_and_escaped_html_audit_reports_to_clipboard(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);

    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let findings = vec![
        AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x01000400".into(),
            title: "HTTPS 页面混合内容".into(),
            severity_label: IssueSeverity::Medium.label().into(),
            severity_rank: IssueSeverity::Medium.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Passive,
            host: "portal.test".into(),
            url: "https://portal.test/login?next=<script>alert(1)</script>".into(),
            parameter: Some("next\" onclick=\"evil()".into()),
            flow_id: None,
            summary: "HTTPS HTML 引用明文 http:// 脚本或表单。".into(),
            evidence: "<script src=\"http://cdn.plain.test/app.js\">".into(),
            request_highlights: vec![],
            response_highlights: vec![EvidenceSpan {
                offset: 42,
                length: 43,
            }],
            remediation: "统一迁移至 https:// 并配置 CSP。".into(),
            detected_unix_ms: 500,
        },
        AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00500601".into(),
            title: "Cookie 缺少安全 SameSite 属性".into(),
            severity_label: IssueSeverity::Low.label().into(),
            severity_rank: IssueSeverity::Low.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Passive,
            host: "portal.test".into(),
            url: "https://portal.test/session".into(),
            parameter: None,
            flow_id: None,
            summary: "Set-Cookie 设置 SameSite=None 但缺少 Secure 标志。".into(),
            evidence: "sid=1; SameSite=None; HttpOnly".into(),
            request_highlights: vec![],
            response_highlights: vec![],
            remediation: "设置 SameSite=Lax/Strict，或 SameSite=None 配合 Secure。".into(),
            detected_unix_ms: 501,
        },
    ];

    f.events
        .try_send(Event::Audit(AuditEvent::PassiveFindings {
            project: f.project,
            revision: 2,
            findings,
        }))
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dashboard-summary-pane").visible());
    })
    .unwrap();
    click(f.window, "dashboard-summary-view-all", cx);
    click(f.window, "dashboard-copy-json-report", cx);
    let json_clip = cx
        .read(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .expect("JSON report copied to clipboard");
    assert!(json_clip.contains("\"format\": \"mio.audit_report.v1\""));
    assert!(json_clip.contains("\"medium\": 1"));
    assert!(json_clip.contains("\"low\": 1"));
    assert!(json_clip.contains("0x01000400"));
    assert!(json_clip.contains("0x00500601"));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.error.is_none());
        assert_eq!(view.status, "已复制 JSON 审计报告到剪贴板（共 2 条告警）。");
    });

    click(f.window, "dashboard-copy-html-report", cx);
    let html_clip = cx
        .read(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .expect("HTML report copied to clipboard");
    assert!(html_clip.contains("<!DOCTYPE html>"));
    assert!(html_clip.contains("<tr><td>Medium (中危)</td><td>1</td></tr>"));
    assert!(html_clip.contains("<tr><td>Low (低危)</td><td>1</td></tr>"));
    assert!(!html_clip.contains("<script>alert(1)</script>"));
    assert!(!html_clip.contains("<script src=\"http://cdn.plain.test/app.js\">"));
    assert!(html_clip.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html_clip.contains("next&quot; onclick=&quot;evil()"));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.error.is_none());
        assert_eq!(view.status, "已复制 HTML 审计报告到剪贴板（共 2 条告警）。");
    });
}

#[gpui_kit::test]
fn new_scan_wizard_toggles_crawl_same_origin_and_submits_crawl_flag(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);

    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    click(f.window, "dashboard-new-scan", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let cb = window.find("audit-crawl-same-origin");
        assert!(cb.visible());
        assert_eq!(cb.checked(), Some(false));
        assert!(!f.view.read(cx).audit_dashboard.crawl_same_origin);
    })
    .unwrap();

    click(f.window, "audit-crawl-same-origin", cx);
    let url_input = cx.read(|cx| f.view.read(cx).audit_dashboard.scan_url_input.clone());
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("audit-crawl-same-origin").checked(), Some(true));
        assert!(f.view.read(cx).audit_dashboard.crawl_same_origin);
        url_input.update(cx, |state, cx| {
            state.set_value("http://127.0.0.1:18090/", window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();

    click(f.window, "dashboard-start-scan", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::Audit(AuditCommand::StartScan {
            project,
            draft,
            mode,
            crawl_same_origin,
            ..
        }),
    ] = commands.as_slice()
    else {
        panic!(
            "expected single AuditCommand::StartScan, got {}",
            commands.len()
        );
    };
    assert_eq!(*project, f.project);
    assert_eq!(*mode, AuditTaskMode::PassiveAndActive);
    assert!(*crawl_same_origin);
    assert_eq!(draft.target, "http://127.0.0.1:18090/");

    click(f.window, "dashboard-new-scan", cx);
    click(f.window, "audit-crawl-same-origin", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("audit-crawl-same-origin").checked(),
            Some(false)
        );
    })
    .unwrap();

    click(f.window, "dashboard-start-scan", cx);
    let second_commands = drain(&mut f.commands);
    let [
        Command::Audit(AuditCommand::StartScan {
            crawl_same_origin: second_crawl,
            ..
        }),
    ] = second_commands.as_slice()
    else {
        panic!("expected single AuditCommand::StartScan after untoggling crawl");
    };
    assert!(!*second_crawl);
}

#[gpui_kit::test]
fn task_pause_resume_and_finding_triage_emit_audit_commands(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let task_id = Uuid::new_v4();
    let finding_id = Uuid::new_v4();
    f.events
        .try_send(Event::Audit(AuditEvent::TaskUpdated {
            project: f.project,
            revision: 1,
            task: AuditTaskSummary {
                id: task_id,
                title: "Triage Scan".into(),
                target_url: "http://127.0.0.1:18080/".into(),
                mode: AuditTaskMode::PassiveAndActive,
                state: AuditTaskState::Running,
                requests_sent: 2,
                total_planned: 10,
                issues_found: 1,
                started_unix_ms: 10,
                error: None,
            },
            new_findings: vec![AuditFinding {
                id: finding_id,
                issue_id: "0x00200602".into(),
                title: "CORS 允许 null 源".into(),
                severity_label: IssueSeverity::Medium.label().into(),
                severity_rank: IssueSeverity::Medium.rank(),
                confidence: IssueConfidence::Certain,
                phase: AuditPhase::Passive,
                host: "127.0.0.1".into(),
                url: "http://127.0.0.1:18080/".into(),
                parameter: None,
                flow_id: None,
                summary: "CORS null".into(),
                evidence: "Access-Control-Allow-Origin: null".into(),
                request_highlights: vec![],
                response_highlights: vec![],
                remediation: "禁用 null 源。".into(),
                detected_unix_ms: 10,
            }],
        }))
        .unwrap();
    cx.run_until_parked();

    click(
        f.window,
        SharedString::from(format!("dashboard-audit-pause-{task_id}")),
        cx,
    );
    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::Audit(AuditCommand::PauseScan { task_id: id, .. })] if *id == task_id
    ));

    click(
        f.window,
        SharedString::from(format!("dashboard-audit-resume-{task_id}")),
        cx,
    );
    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::Audit(AuditCommand::ResumeScan { task_id: id, .. })] if *id == task_id
    ));

    click(
        f.window,
        SharedString::from(format!("dashboard-issue-row-{finding_id}")),
        cx,
    );
    click(f.window, "dashboard-finding-sev-info", cx);
    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::Audit(AuditCommand::SetFindingSeverity {
            finding_id: id,
            severity: IssueSeverity::Information,
            confidence: IssueConfidence::Certain,
            ..
        })] if *id == finding_id
    ));

    click(f.window, "dashboard-delete-finding", cx);
    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::Audit(AuditCommand::DeleteFinding { finding_id: id, .. })] if *id == finding_id
    ));
}

#[gpui_kit::test]
fn ext_dashboard_two_pane_summary_layout_and_live_task_card(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);

    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("dashboard-pro-header").is_none());
        assert!(window.try_find("inspector-sidebar").is_none());
        assert!(window.find("dashboard-tasks-header").visible());
        assert!(window.find("dashboard-new-scan").visible());
        assert!(window.find("dashboard-live-passive-toggle").visible());
        assert!(window.find("dashboard-tasks-pause-all").visible());
        assert!(window.find("dashboard-task-settings").visible());
        assert!(window.find("dashboard-tasks-filter").visible());
        assert!(window.find("dashboard-tasks-search-box").visible());
        assert!(window.find("dashboard-live-audit-task").visible());
        assert_eq!(
            window.find("dashboard-live-audit-capturing-label").label(),
            Some("捕获中")
        );
        assert!(window.find("dashboard-live-audit-capturing").visible());
        assert!(window.find("dashboard-live-crawl-task").visible());
        assert_eq!(
            window.find("dashboard-live-crawl-capturing-label").label(),
            Some("捕获中")
        );
        assert!(window.find("dashboard-live-crawl-capturing").visible());
        assert_eq!(
            window.find("dashboard-summary-capturing-label").label(),
            Some("捕获中")
        );
        assert!(window.find("dashboard-task-workspace").visible());
        assert!(window.find("dashboard-summary-pane").visible());
        assert!(window.find("dashboard-summary-vuln-card").visible());
        assert!(window.find("dashboard-summary-table-scrollbar").visible());
        assert!(window.find("dashboard-summary-pane-scrollbar").visible());
        assert!(window.find("dashboard-summary-config-card").visible());
        assert!(window.find("dashboard-summary-log-card").visible());
        assert!(window.find("dashboard-summary-view-all").visible());
    })
    .unwrap();

    click(f.window, "dashboard-live-crawl-task", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).audit_dashboard.selected_builtin_task,
            DashboardBuiltinTask::LivePassiveCrawl
        );
    })
    .unwrap();

    click(f.window, "dashboard-live-audit-task", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).audit_dashboard.selected_builtin_task,
            DashboardBuiltinTask::LivePassiveAudit
        );
    })
    .unwrap();

    for (tab_id, pane_id) in [
        ("dashboard-tab-audit-items", "dashboard-audit-items-pane"),
        ("dashboard-tab-issues", "dashboard-issue-activity-pane"),
        ("dashboard-tab-event-log", "dashboard-event-log-pane"),
        ("dashboard-tab-logger", "dashboard-logger-pane"),
        ("dashboard-tab-audit-log", "dashboard-audit-log-pane"),
        ("dashboard-tab-summary", "dashboard-summary-pane"),
    ] {
        click(f.window, tab_id, cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(pane_id).visible());
        })
        .unwrap();
    }

    let _ = drain(&mut f.commands);
    click(f.window, "dashboard-live-audit-capturing", cx);
    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [Command::Audit(AuditCommand::SetLivePassive {
            enabled: false,
            ..
        })]
    ));

    click(f.window, "dashboard-live-crawl-capturing", cx);
    let crawl_cmds = drain(&mut f.commands);
    assert!(matches!(
        crawl_cmds.as_slice(),
        [Command::Audit(AuditCommand::SetLivePassiveCrawl {
            enabled: false,
            ..
        })]
    ));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            !f.view
                .read(cx)
                .audit_dashboard
                .snapshot
                .live_passive_crawl_enabled
        );
        assert_eq!(
            window.find("dashboard-live-audit-capturing-label").label(),
            Some("已暂停")
        );
        assert_eq!(
            window.find("dashboard-live-crawl-capturing-label").label(),
            Some("已暂停")
        );
        assert_eq!(
            window.find("dashboard-summary-capturing-label").label(),
            Some("已暂停")
        );
    })
    .unwrap();
}

fn sample_finding(
    issue_id: &str,
    title: &str,
    sev: IssueSeverity,
    conf: IssueConfidence,
    phase: AuditPhase,
    url: &str,
    detected_unix_ms: i64,
) -> AuditFinding {
    AuditFinding {
        id: Uuid::new_v4(),
        issue_id: issue_id.into(),
        title: title.into(),
        severity_label: sev.label().into(),
        severity_rank: sev.rank(),
        confidence: conf,
        phase,
        host: crate::workbench::wb_proxy::extract_url_host(url)
            .unwrap_or_else(|| "example.test".into()),
        url: url.into(),
        parameter: Some("q".into()),
        flow_id: None,
        summary: format!("Summary for {title}"),
        evidence: format!("Evidence <script>&\"' {url}"),
        request_highlights: vec![],
        response_highlights: vec![],
        remediation: "Encode & validate.".into(),
        detected_unix_ms,
    }
}

#[gpui_kit::test]
fn new_live_task_modal_configures_and_dispatches_real_commands(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    click(f.window, "dashboard-live-passive-toggle", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dashboard-new-live-task-panel").visible());
    })
    .unwrap();

    click(f.window, "dashboard-live-task-modal-crawl", cx);
    click(f.window, "dashboard-live-task-scope-suite", cx);
    let _ = drain(&mut f.commands);
    click(f.window, "dashboard-live-task-apply", cx);

    let cmds = drain(&mut f.commands);
    assert!(matches!(
        cmds.as_slice(),
        [
            Command::Audit(AuditCommand::SetLivePassiveCrawl { enabled: false, .. }),
            Command::Audit(AuditCommand::SetLivePassiveScope {
                suite_scope_only: true,
                ..
            }),
        ]
    ));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let view = f.view.read(cx);
        assert!(!view.audit_dashboard.new_live_task_open);
        assert!(!view.audit_dashboard.snapshot.live_passive_crawl_enabled);
        assert_eq!(
            view.audit_dashboard.live_task_scope,
            LiveTaskTrafficScope::SuiteScopeOnly
        );
        assert!(window.try_find("dashboard-new-live-task-panel").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn scan_task_report_scoping_and_xml_report_generation(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let task_a_id = Uuid::new_v4();
    let task_b_id = Uuid::new_v4();
    let f_task_a = sample_finding(
        "0x001",
        "Alpha XSS",
        IssueSeverity::High,
        IssueConfidence::Firm,
        AuditPhase::Active,
        "http://alpha.test/app/search?q=1",
        1500,
    );
    let f_pre_task_a = sample_finding(
        "0x002",
        "Old Finding Before Task A",
        IssueSeverity::Low,
        IssueConfidence::Certain,
        AuditPhase::Passive,
        "http://alpha.test/app/search?q=old",
        500,
    );
    let f_other_path = sample_finding(
        "0x003",
        "Admin Header",
        IssueSeverity::Low,
        IssueConfidence::Certain,
        AuditPhase::Passive,
        "http://alpha.test/admin/panel",
        1600,
    );
    let f_task_b = sample_finding(
        "0x004",
        "Beta CORS",
        IssueSeverity::Medium,
        IssueConfidence::Certain,
        AuditPhase::Passive,
        "http://beta.test/",
        2500,
    );

    let task_a = AuditTaskSummary {
        id: task_a_id,
        title: "Scan Alpha".into(),
        target_url: "http://alpha.test/app".into(),
        mode: AuditTaskMode::PassiveAndActive,
        state: AuditTaskState::Completed,
        requests_sent: 5,
        total_planned: 5,
        issues_found: 1,
        started_unix_ms: 1000,
        error: None,
    };
    let task_b = AuditTaskSummary {
        id: task_b_id,
        title: "Scan Beta".into(),
        target_url: "http://beta.test/".into(),
        mode: AuditTaskMode::PassiveOnly,
        state: AuditTaskState::Completed,
        requests_sent: 1,
        total_planned: 1,
        issues_found: 1,
        started_unix_ms: 2000,
        error: None,
    };

    f.events
        .try_send(Event::Audit(AuditEvent::PassiveFindings {
            project: f.project,
            revision: 1,
            findings: vec![f_pre_task_a.clone(), f_other_path.clone()],
        }))
        .unwrap();
    f.events
        .try_send(Event::Audit(AuditEvent::TaskUpdated {
            project: f.project,
            revision: 2,
            task: task_a,
            new_findings: vec![f_task_a.clone()],
        }))
        .unwrap();
    f.events
        .try_send(Event::Audit(AuditEvent::TaskUpdated {
            project: f.project,
            revision: 3,
            task: task_b,
            new_findings: vec![f_task_b.clone()],
        }))
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, _, cx| {
        f.view.update(cx, |view, cx| {
            let snap_a = view
                .scoped_audit_snapshot(AuditReportScope::Task(task_a_id))
                .expect("task A snapshot");
            assert_eq!(snap_a.findings.len(), 1);
            assert_eq!(snap_a.findings[0].id, f_task_a.id);
            assert!(snap_a.live_passive_crawl_enabled);

            view.copy_audit_report_json_for_scope(AuditReportScope::Task(task_a_id), cx);
        });
    })
    .unwrap();
    let clip = cx
        .read(|cx| cx.read_from_clipboard().and_then(|i| i.text()))
        .expect("scoped JSON copied");
    assert!(clip.contains("Alpha XSS"));
    assert!(!clip.contains("Beta CORS"));
    assert!(!clip.contains("Old Finding Before Task A"));
    assert!(!clip.contains("Admin Header"));

    cx.update_window(f.window, |_, _, cx| {
        f.view.update(cx, |view, cx| {
            view.mark_audit_finding_false_positive(f_task_a.id, cx);
            view.mark_audit_finding_verified(f_task_a.id, cx);
            view.audit_dashboard
                .finding_notes
                .insert(f_task_a.id, "Triaged <ok> & verified".into());
            let single = view
                .scoped_audit_snapshot(AuditReportScope::SingleFinding(f_task_a.id))
                .expect("single snapshot");
            assert_eq!(single.findings.len(), 1);
            let xml = String::from_utf8(
                view.render_audit_report_bytes(&single, AuditReportFormat::Xml)
                    .expect("xml bytes"),
            )
            .expect("utf8 xml");
            assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
            assert!(xml.contains("<name>Alpha XSS</name>"));
            assert!(xml.contains("<severity>False positive</severity>"));
            assert!(xml.contains("<confidence>Verified</confidence>"));
            assert!(xml.contains("&lt;script&gt;&amp;&quot;&apos;"));
            assert!(xml.contains("<notes>Triaged &lt;ok&gt; &amp; verified</notes>"));
            assert!(!xml.contains("Beta CORS"));
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn issue_context_actions_triage_restore_highlight_notes_comparer_mio_at_and_target(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let finding = sample_finding(
        "0x00200300",
        "Reflected XSS",
        IssueSeverity::High,
        IssueConfidence::Firm,
        AuditPhase::Active,
        "https://app.example.test/search?q=1",
        1000,
    );
    let fid = finding.id;
    f.events
        .try_send(Event::Audit(AuditEvent::PassiveFindings {
            project: f.project,
            revision: 1,
            findings: vec![finding],
        }))
        .unwrap();
    cx.run_until_parked();
    let _ = drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.mark_audit_finding_false_positive(fid, cx);
            assert!(view.audit_dashboard.false_positive_findings.contains(&fid));
            view.restore_audit_finding_severity(fid, cx);
            assert!(!view.audit_dashboard.false_positive_findings.contains(&fid));
            assert_eq!(
                view.audit_dashboard.snapshot.findings[0].severity(),
                IssueSeverity::High
            );

            view.mark_audit_finding_verified(fid, cx);
            assert!(view.audit_dashboard.verified_confidence_findings.contains(&fid));
            view.restore_audit_finding_confidence(fid, cx);
            assert!(!view.audit_dashboard.verified_confidence_findings.contains(&fid));
            assert_eq!(
                view.audit_dashboard.snapshot.findings[0].confidence,
                IssueConfidence::Firm
            );

            view.set_audit_finding_highlight(fid, 2, cx);
            assert_eq!(view.audit_dashboard.finding_highlights.get(&fid), Some(&2));
            view.set_audit_finding_highlight(fid, 0, cx);
            assert!(!view.audit_dashboard.finding_highlights.contains_key(&fid));

            view.open_audit_finding_note_editor(fid, window, cx);
            view.audit_dashboard
                .finding_note_input
                .update(cx, |state, cx| state.set_value("needs retest", window, cx));
            view.save_audit_finding_note(window, cx);
            assert_eq!(
                view.audit_dashboard.finding_notes.get(&fid).map(String::as_str),
                Some("needs retest")
            );

            view.send_audit_finding_to_mio_ai(fid, cx);
            assert!(view.mio_ai_panel_open);
            assert!(
                view.mio_ai_context
                    .as_deref()
                    .is_some_and(|ctx| ctx.contains("Reflected XSS") && ctx.contains("needs retest"))
            );

            view.send_audit_finding_to_comparer(fid, window, cx);
            assert_eq!(view.tool, Tool::Comparer);
            assert_eq!(view.extra_tools.comparer_items.len(), 1);

            view.go_to_audit_finding_in_target(fid, window, cx);
            assert_eq!(view.tool, Tool::Target);
            assert_eq!(view.target_pane.page, crate::workbench::target::TargetPage::SiteMap);
            assert_eq!(
                view.target_pane.site_map_subview,
                crate::workbench::target::SiteMapSubView::Issues
            );
            assert_eq!(view.target_pane.selected_site_map_finding_id, Some(fid));
            assert_eq!(
                view.target_pane.tree_state.selected_key.as_deref(),
                Some("https://app.example.test")
            );
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn issue_activity_honors_grouped_evidence_and_event_log_uses_footer_entries(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let f1 = sample_finding(
        "0x00100100",
        "Missing HSTS",
        IssueSeverity::Low,
        IssueConfidence::Certain,
        AuditPhase::Passive,
        "https://example.test/a",
        100,
    );
    let f2 = sample_finding(
        "0x00100100",
        "Missing HSTS",
        IssueSeverity::Low,
        IssueConfidence::Certain,
        AuditPhase::Passive,
        "https://example.test/b",
        101,
    );
    let f1_id = f1.id;
    let f2_id = f2.id;

    f.events
        .try_send(Event::Audit(AuditEvent::PassiveFindings {
            project: f.project,
            revision: 1,
            findings: vec![f1, f2],
        }))
        .unwrap();
    cx.run_until_parked();

    click(f.window, "dashboard-tab-issues", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // Default issues_grouped_evidence = true => grouped into 1 header row with count badge
        let lead_id = f.view.read(cx).audit_dashboard.snapshot.findings[0].id;
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-issue-group-count-{lead_id}"
                )))
                .visible()
        );
        f.view.update(cx, |view, cx| {
            view.user_preferences.display.issues_grouped_evidence = false;
            if let Some(conf) = view.user_preferences.confirmed.as_mut() {
                conf.display.issues_grouped_evidence = false;
            }
            cx.notify();
        });
        window.render_frame(cx);
        assert!(
            window
                .try_find(SharedString::from(format!(
                    "dashboard-issue-group-count-{lead_id}"
                )))
                .is_none()
        );
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-issue-activity-row-{f1_id}"
                )))
                .visible()
        );
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-issue-activity-row-{f2_id}"
                )))
                .visible()
        );
    })
    .unwrap();

    // Real Event log backed by footer_drawer.entries
    let event_id = cx.update_window(f.window, |_, _, cx| {
        f.view.update(cx, |view, cx| {
            view.record_footer_event(
                FooterLogSeverity::Warning,
                "Real footer log marker",
                "Real detail body",
            );
            cx.notify();
            view.footer_drawer
                .entries
                .iter()
                .last()
                .map(|e| e.id)
                .unwrap()
        })
    })
    .unwrap();
    click(f.window, "dashboard-tab-event-log", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-event-log-row-{event_id}"
                )))
                .visible()
        );
        f.view.update(cx, |view, cx| {
            view.footer_drawer.clear_events();
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.find("dashboard-event-log-empty").visible());
    })
    .unwrap();

    // Audit items context actions
    let _ = drain(&mut f.commands);
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.scan_audit_item(
                "GET",
                "https://example.test/item",
                AuditTaskMode::PassiveOnly,
                cx,
            );
            view.open_audit_item_in_repeater(
                None,
                "GET",
                "https://example.test/item",
                window,
                cx,
            );
            assert_eq!(view.tool, Tool::Repeater);
        });
    })
    .unwrap();
    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(AuditCommand::StartScan {
            mode: AuditTaskMode::PassiveOnly,
            ..
        })
    )));
}

#[gpui_kit::test]
fn dashboard_issues_multiselect_sort_columns_and_batch_actions_work(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);
    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let f1 = sample_finding(
        "0x00100100",
        "B Issue",
        IssueSeverity::Low,
        IssueConfidence::Certain,
        AuditPhase::Passive,
        "https://a.test/1",
        100,
    );
    let f2 = sample_finding(
        "0x00100200",
        "A Issue",
        IssueSeverity::High,
        IssueConfidence::Firm,
        AuditPhase::Active,
        "https://b.test/2",
        101,
    );
    let f3 = sample_finding(
        "0x00100300",
        "C Issue",
        IssueSeverity::Medium,
        IssueConfidence::Tentative,
        AuditPhase::Passive,
        "https://c.test/3",
        102,
    );
    let (id1, id2, id3) = (f1.id, f2.id, f3.id);

    f.events
        .try_send(Event::Audit(AuditEvent::PassiveFindings {
            project: f.project,
            revision: 1,
            findings: vec![f1, f2, f3],
        }))
        .unwrap();
    cx.run_until_parked();
    drain(&mut f.commands);

    click(f.window, "dashboard-tab-issues", cx);
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.user_preferences.display.issues_grouped_evidence = false;
            if let Some(conf) = view.user_preferences.confirmed.as_mut() {
                conf.display.issues_grouped_evidence = false;
            }
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.find("dashboard-issue-col-host").visible());

        let order = vec![id3, id2, id1];
        f.view.update(cx, |view, cx| {
            view.handle_audit_issue_row_click(&[id3], &order, false, false, true, window, cx);
            view.handle_audit_issue_row_click(&[id1], &order, true, false, true, window, cx);
            assert_eq!(view.audit_selected_target_ids(id3), vec![id3, id2, id1]);
            view.handle_audit_issue_row_click(&[id2], &order, false, true, true, window, cx);
            assert_eq!(view.audit_selected_target_ids(id3), vec![id3, id1]);
            view.handle_audit_issue_row_right_down(&[id3], window, cx);
            assert_eq!(view.audit_selected_target_ids(id3), vec![id3, id1]);

            view.toggle_audit_issue_sort(DashboardIssueSortColumn::Severity, cx);
            assert_eq!(
                view.audit_dashboard.issue_sort,
                Some((DashboardIssueSortColumn::Severity, true))
            );
            view.copy_audit_issue_column(3, &[id3, id1], cx);
            view.toggle_audit_issue_column_visibility(2, cx);
            view.fit_audit_issue_column_width(1, &order, cx);
        });
        window.render_frame(cx);
        assert!(window.try_find("dashboard-issue-col-host").is_none());
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("https://c.test/3\nhttps://a.test/1".into())
        );

        f.view.update(cx, |view, cx| {
            view.reset_audit_issue_column_layout(cx);
            view.set_audit_findings_highlight_batch(&[id1, id3], 3, cx);
            assert_eq!(view.audit_dashboard.finding_highlights.get(&id1), Some(&3));
            assert_eq!(view.audit_dashboard.finding_highlights.get(&id3), Some(&3));
            view.mark_audit_findings_false_positive_batch(&[id1, id3], cx);
            assert!(view.audit_dashboard.false_positive_findings.contains(&id1));
            assert!(view.audit_dashboard.false_positive_findings.contains(&id3));
            view.delete_audit_findings_batch(&[id1, id3], window, cx);
        });
        window.render_frame(cx);
        assert!(window.find("dashboard-issue-col-host").visible());
        assert_eq!(
            f.view
                .read(cx)
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![id2]
        );
    })
    .unwrap();

    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(AuditCommand::DeleteFinding { finding_id, .. }) if *finding_id == id1
    )));
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(AuditCommand::DeleteFinding { finding_id, .. }) if *finding_id == id3
    )));
}
