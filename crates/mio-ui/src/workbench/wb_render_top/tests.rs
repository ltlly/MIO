use gpui_kit::test::TestWindowExt;
use gpui_kit::{EntityInputHandler, TestAppContext};
use mio_core::{
    AuditCommand, AuditEvent, AuditStateSnapshot, AuditTaskMode, AuditTaskState,
    AuditTaskSummary, Command, Event, FlowState, ProjectKey, Uuid,
    audit_engine::{AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence},
    target_issues::IssueSeverity,
};

use super::*;
use crate::workbench::tests::{drain, fixture, record, tab};

#[test]
fn footer_memory_ratio_handles_empty_fractional_and_over_total_metrics() {
    assert_eq!(footer_memory_ratio(0, 0), 0.0);
    assert_eq!(footer_memory_ratio(1, 0), 0.0);
    assert_eq!(footer_memory_ratio(1, 4), 0.25);
    assert_eq!(footer_memory_ratio(4, 4), 1.0);
    assert_eq!(footer_memory_ratio(5, 4), 1.0);
    assert_eq!(footer_memory_ratio(u64::MAX, 1), 1.0);
}

fn assert_footer_dot_paints(window: &Window, id: &'static str, color: u32) {
    let scale = window.scale_factor();
    let bounds = window.find(id).bounds().scale(scale);
    let diameter = gpui_kit::ScaledPixels(6. * scale);
    let radius = gpui_kit::ScaledPixels(3. * scale);
    assert!(
        window.painted_quads().iter().any(|quad| {
            quad.bounds.contains(&bounds.center())
                && quad.bounds.size == gpui_kit::size(diameter, diameter)
                && quad.background == gpui_kit::rgb(color).into()
                && quad.corner_radii == gpui_kit::Corners::all(radius)
                && quad.bounds.intersect(&quad.content_mask.bounds).size == quad.bounds.size
        }),
        "{id} 需要完整绘制指定颜色的 6×6 实心圆点"
    );
}

#[gpui_kit::test]
fn footer_geometry_and_real_physical_memory_metrics(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1: /api")], 32);
    drain(&mut f.commands);

    let total_mem = mio_core::total_physical_memory_bytes();
    let process_mem = mio_core::current_process_memory_bytes();
    assert!(total_mem >= process_mem && total_mem > 0);
    assert_eq!(footer_memory_ratio(0, total_mem), 0.0);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let bar = window.find("mio-status-bar").bounds();
        assert_eq!(bar.origin.y + bar.size.height, window.viewport_size().height);
        assert_eq!(bar.size.height, px(40.));

        let event_text = window.find("footer-event-log-text").bounds();
        assert_eq!(event_text.origin.x - bar.origin.x, px(15.));
        assert_eq!(event_text.origin.y - bar.origin.y, px(14.));
        assert!(event_text.size.width >= px(71.));
        assert_eq!(event_text.size.height, px(17.));
        let event_dot = window.find("footer-event-log-dot").bounds();
        assert_eq!(event_dot.size, gpui_kit::size(px(6.), px(6.)));
        assert!(event_dot.origin.x >= event_text.origin.x + event_text.size.width);
        assert!(event_dot.origin.y <= event_text.origin.y);
        assert_footer_dot_paints(window, "footer-event-log-dot", 0x26649d);

        let all_issues = window.find("footer-all-issues").bounds();
        assert!(all_issues.origin.x - bar.origin.x >= px(116.));
        assert!(window.try_find("footer-all-issues-dot").is_none());

        let resource = window.find("footer-resource-usage");
        let res_bounds = resource.bounds();
        assert_eq!(res_bounds.size, gpui_kit::size(px(262.), px(22.)));
        let icon = window.find("footer-memory-info-icon").bounds();
        assert_eq!(icon.origin.x - res_bounds.origin.x, px(0.));
        assert_eq!(icon.size.width, px(15.));
        let text = window.find("footer-memory-text").bounds();
        assert_eq!(text.origin.x - res_bounds.origin.x, px(25.));
        assert_eq!(text.size.width, px(180.));
        let progress = window.find("footer-memory-progress").bounds();
        assert_eq!(progress.origin.x - res_bounds.origin.x, px(210.));
        assert_eq!(progress.origin.y - res_bounds.origin.y, px(7.));
        assert_eq!(progress.size, gpui_kit::size(px(52.), px(8.)));
        let fill = window.find("footer-memory-progress-fill").bounds();
        assert_eq!(fill.size.height, px(8.));
        assert!(fill.size.width > px(0.) && fill.size.width <= px(52.));
        let scale = window.scale_factor();
        let progress_scaled = progress.scale(scale);
        let fill_scaled = fill.scale(scale);
        let painted_quads = window.painted_quads();
        assert!(
            painted_quads.iter().any(|quad| {
                quad.bounds.contains(&progress_scaled.center())
                    && quad.bounds.size == progress_scaled.size
                    && quad.background == gpui_kit::rgb(0xeeeeee).into()
                    && quad.bounds.intersect(&quad.content_mask.bounds).size == quad.bounds.size
            }),
            "内存进度条需要完整绘制 52×8 浅灰背景"
        );
        assert!(
            painted_quads.iter().any(|quad| {
                quad.bounds.contains(&fill_scaled.center())
                    && quad.background == gpui_kit::rgb(0xc9c9c9).into()
                    && quad.bounds.size.height == fill_scaled.size.height
                    && (quad.bounds.size.width.0 - fill_scaled.size.width.0).abs() <= 1.
                    && quad.bounds.origin.x >= progress_scaled.origin.x.floor()
                    && quad.bounds.right() <= progress_scaled.right().ceil()
            }),
            "内存进度条需要按真实占用比例绘制灰色填充"
        );

        let capsule = window.find("footer-settings-activity-capsule").bounds();
        assert_eq!(capsule.size, gpui_kit::size(px(58.), px(22.)));
        assert_eq!(bar.origin.x + bar.size.width - (capsule.origin.x + capsule.size.width), px(15.));

        let label = window.find("footer-memory-text").label().unwrap().to_owned();
        assert!(label.starts_with("Memory: ") && label.contains(&format!(" of {}", format_resource_bytes(total_mem))));

        window.click("footer-resource-usage", cx);
        window.render_frame(cx);
        assert!(window.find("footer-info-popup").visible());
        assert_eq!(window.find("footer-project-name").label(), Some("临时项目"));
        assert!(window.find("project-status").label().unwrap().contains("项目已打开"));
        window.click("footer-slim-save-as", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
}

#[gpui_kit::test]
fn footer_event_log_captures_real_events_and_bounds_retention(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1: /api")], 64);
    drain(&mut f.commands);
    let tab_id = cx.read(|cx| f.view.read(cx).workspace.tabs[0].id);

    let addr: std::net::SocketAddr = "127.0.0.1:18080".parse().unwrap();
    f.events.try_send(Event::ProxyState {
        project: f.project,
        address: Some(addr),
        cancel: Some(CancellationToken::new()),
        intercept_requests: false,
        intercept_responses: false,
    }).unwrap();
    f.events.try_send(Event::Error {
        project: Some(f.project),
        operation: "save_project",
        message: "磁盘空间不足".into(),
    }).unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        f.view.update(cx, |view, cx| view.send(window, cx));
    }).unwrap();
    let cmds = drain(&mut f.commands);
    let run_id = cmds.into_iter().find_map(|cmd| match cmd {
        Command::Send { run_id, .. } => Some(run_id),
        _ => None,
    }).expect("send command issued");
    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            assert!(view.cancel_send(tab_id, cx));
        });
        window.render_frame(cx);
    }).unwrap();
    let mut cancelled = record("http://127.0.0.1:9001/", "");
    cancelled.summary.id = run_id;
    cancelled.summary.state = FlowState::Cancelled;
    f.events.try_send(Event::SendFinished {
        project: f.project,
        tab_id,
        draft_revision: 1,
        record: Box::new(cancelled),
        persisted: false,
    }).unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("footer-event-log", cx);
        window.render_frame(cx);
        assert!(window.find("footer-event-log-drawer").visible());
        assert!(f.view.read(cx).tool != Tool::Dashboard);

        let entries: Vec<_> = f.view.read(cx).footer_drawer.entries.iter().cloned().collect();
        assert!(entries.iter().any(|e| e.message.contains("项目已打开")));
        assert!(entries.iter().any(|e| e.message.contains("Proxy 正在监听 127.0.0.1:18080")));
        assert!(entries.iter().any(|e| e.severity == FooterLogSeverity::Error && e.message.contains("磁盘空间不足")));
        assert!(entries.iter().any(|e| e.message.contains("Repeater 请求取消已提交")));
        assert!(entries.iter().any(|e| e.severity == FooterLogSeverity::Warning && e.message.contains("Repeater 请求已取消")));

        let err_id = entries.iter().find(|e| e.severity == FooterLogSeverity::Error).unwrap().id;
        window.click("footer-event-filter-error", cx);
        window.render_frame(cx);
        window.click(SharedString::from(format!("footer-event-row-{err_id}")), cx);
        window.render_frame(cx);
        window.click("footer-event-copy-selected", cx);
        let clip = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
        assert!(clip.contains("Error") && clip.contains("磁盘空间不足"));

        f.view.update(cx, |view, _| {
            for idx in 0..800 {
                view.record_footer_event(FooterLogSeverity::Info, format!("event-{idx}"), "x".repeat(3000));
            }
            assert!(view.footer_drawer.retained_bytes <= FOOTER_LOG_MAX_BYTES);
            assert!(view.footer_drawer.entries.len() <= FOOTER_LOG_MAX_ENTRIES);
        });
        window.render_frame(cx);
        window.click("footer-event-clear", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).footer_drawer.entries.is_empty());
        assert!(window.try_find("footer-event-log-dot").is_none());
    }).unwrap();

    let next_project = ProjectKey { id: Uuid::new_v4(), generation: 2 };
    f.events.try_send(Event::ProjectOpened {
        project: next_project,
        path: None,
        workspace: mio_core::Workspace::default(),
    }).unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let drawer = &f.view.read(cx).footer_drawer;
        assert!(drawer.active.is_none());
        assert_eq!(drawer.entries.len(), 1);
        assert!(drawer.entries[0].message.contains("项目已打开"));
    });
}

#[gpui_kit::test]
fn footer_all_issues_drawer_queries_filters_and_selects_real_finding(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![tab("1: /api")], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("footer-all-issues", cx);
        window.render_frame(cx);
        assert!(window.find("footer-all-issues-drawer").visible());
        assert!(f.view.read(cx).tool != Tool::Dashboard);
    }).unwrap();

    let cmds = drain(&mut f.commands);
    let [Command::Audit(AuditCommand::QueryState { project, query_revision })] = cmds.as_slice() else {
        panic!("opening All issues drawer must query backend audit state");
    };
    assert_eq!(*project, f.project);

    let mut flow = record("http://127.0.0.1:9001/vuln?id=1", "");
    flow.response = Some(mio_core::HttpResponse {
        status: 200,
        version: mio_core::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: b"SQL syntax error".to_vec(),
        trailers: vec![],
    });
    let high = AuditFinding {
        id: Uuid::new_v4(),
        issue_id: "0x00100200".into(),
        title: "SQL 注入".into(),
        severity_label: "高危".into(),
        severity_rank: IssueSeverity::High.rank(),
        confidence: IssueConfidence::Certain,
        phase: AuditPhase::Active,
        host: "127.0.0.1".into(),
        url: flow.request.url.clone(),
        parameter: Some("id".into()),
        flow_id: Some(flow.summary.id),
        summary: "响应包含数据库语法错误。".into(),
        evidence: "SQL syntax error".into(),
        request_highlights: vec![],
        response_highlights: vec![EvidenceSpan { offset: 0, length: 16 }],
        remediation: "使用参数化查询。".into(),
        detected_unix_ms: 100,
    };
    let low = AuditFinding {
        id: Uuid::new_v4(),
        severity_label: "低危".into(),
        severity_rank: IssueSeverity::Low.rank(),
        title: "缺少安全响应头".into(),
        flow_id: None,
        ..high.clone()
    };
    f.events.try_send(Event::Audit(AuditEvent::State {
        project: f.project,
        query_revision: *query_revision,
        result: Ok(Box::new(AuditStateSnapshot {
            revision: 1,
            live_passive_enabled: true,
            live_passive_crawl_enabled: true,
            live_passive_suite_scope_only: false,
            tasks: vec![AuditTaskSummary {
                id: Uuid::new_v4(),
                title: "扫描".into(),
                target_url: flow.request.url.clone(),
                mode: AuditTaskMode::PassiveAndActive,
                state: AuditTaskState::Completed,
                requests_sent: 2,
                total_planned: 2,
                issues_found: 2,
                started_unix_ms: 90,
                error: None,
            }],
            findings: vec![high.clone(), low.clone()],
        })),
    })).unwrap();
    cx.run_until_parked();

    let search = cx.read(|cx| f.view.read(cx).footer_drawer.issue_search.clone());
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("footer-all-issues").label(), Some("All issues (2)"));
        let issues_text = window.find("footer-all-issues-text").bounds();
        let issues_dot = window.find("footer-all-issues-dot").bounds();
        assert_eq!(issues_dot.size, gpui_kit::size(px(6.), px(6.)));
        assert!(issues_dot.origin.x >= issues_text.origin.x + issues_text.size.width);
        assert!(issues_dot.origin.y <= issues_text.origin.y);
        assert_footer_dot_paints(window, "footer-all-issues-dot", 0xd4380d);
        window.click("footer-issues-filter-high", cx);
        window.render_frame(cx);
        assert!(window.try_find(SharedString::from(format!("footer-issue-row-{}", low.id))).is_none());
        search.update(cx, |state, cx| state.replace_text_in_range(None, "SQL", window, cx));
        window.render_frame(cx);
        window.click(SharedString::from(format!("footer-issue-row-{}", high.id)), cx);
        window.render_frame(cx);
    }).unwrap();

    let cmds = drain(&mut f.commands);
    let [Command::ReadFlow { project, selection_revision, flow_id }] = cmds.as_slice() else {
        panic!("selecting finding in All issues drawer must read associated flow");
    };
    assert_eq!((*project, *flow_id), (f.project, flow.summary.id));
    f.events.try_send(Event::FlowDetail {
        project: f.project,
        selection_revision: *selection_revision,
        record: Box::new(flow),
        original: Box::default(),
    }).unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dashboard-issue-remediation").label().unwrap().contains("参数化查询"));
        window.click("dashboard-issue-tab-response", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).audit_dashboard.issue_response_editor.raw.read(cx).value().contains("SQL syntax error"));

        window.click("footer-open-activity", cx);
        window.render_frame(cx);
        assert!(window.find("footer-activity-drawer").visible());
    }).unwrap();

    let running_task_id = Uuid::new_v4();
    f.events.try_send(Event::Audit(AuditEvent::State {
        project: f.project,
        query_revision: 0,
        result: Ok(Box::new(AuditStateSnapshot {
            revision: 2,
            live_passive_enabled: true,
            live_passive_crawl_enabled: true,
            live_passive_suite_scope_only: false,
            tasks: vec![AuditTaskSummary {
                id: running_task_id,
                title: "主站扫描".into(),
                target_url: "http://127.0.0.1:9001/".into(),
                mode: AuditTaskMode::PassiveAndActive,
                state: AuditTaskState::Running,
                requests_sent: 1,
                total_planned: 5,
                issues_found: 0,
                started_unix_ms: 100,
                error: None,
            }],
            findings: vec![],
        })),
    })).unwrap();
    cx.run_until_parked();
    let _ = drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("footer-activity-summary")
                .label()
                .unwrap()
                .contains("扫描任务：运行中 1")
        );
        window.click("footer-activity-pause-all-scans", cx);
        window.click("footer-activity-live-passive", cx);
    }).unwrap();
    let cmds = drain(&mut f.commands);
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(AuditCommand::PauseScan { project, task_id })
            if *project == f.project && *task_id == running_task_id
    )));
    assert!(cmds.iter().any(|cmd| matches!(
        cmd,
        Command::Audit(AuditCommand::SetLivePassive { project, enabled: false })
            if *project == f.project
    )));
}

#[gpui_kit::test]
fn discover_tab_and_suite_ai_button_navigate_and_copy_real_ipc_command(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![tab("1: /api")], 32);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let discover_tab = window.find("tool-discover");
        assert!(discover_tab.visible());
        assert_eq!(discover_tab.bounds().size.width, px(74.));

        let ai_btn = window.find("suiteMioAiButton");
        assert!(ai_btn.visible());
        assert_eq!(ai_btn.bounds().size, gpui_kit::size(px(120.), px(25.)));
        assert_eq!(ai_btn.label(), Some("MioAT"));

        window.click("suiteMioAiButton", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).tool, Tool::Discover);
        assert!(window.find("discover-main-pane").visible());
        assert!(window.find("discover-ipc-status").visible());
        assert!(window.find("discover-ipc-socket-path").visible());

        window.click("discover-copy-ipc-cmd", cx);
        window.render_frame(cx);
        let clip = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        assert!(clip.starts_with("mio-cli --socket \""));
        assert!(clip.ends_with("\" status"));

        window.click("discover-card-command-palette", cx);
        window.render_frame(cx);
        assert!(f.view.read(cx).command_palette_open);
        f.view.update(cx, |view, _| view.command_palette_open = false);
        window.render_frame(cx);

        window.click("discover-card-custom-logic", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).tool, Tool::Proxy);
        assert_eq!(f.view.read(cx).proxy_page, ProxyPage::History);

        window.click("tool-discover", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).tool, Tool::Discover);

        window.click("discover-card-scan-checks", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).tool, Tool::Dashboard);
        assert!(f.view.read(cx).audit_dashboard.new_scan_open);

        window.click("tool-discover", cx);
        window.render_frame(cx);
        window.click("discover-card-organiser", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).tool, Tool::Organiser);
    })
    .unwrap();
}
