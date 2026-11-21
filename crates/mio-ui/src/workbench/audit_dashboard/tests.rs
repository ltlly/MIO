mod actions_and_reports;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, EntityInputHandler, TestAppContext};
use mio_core::{Header, HttpResponse};

use super::*;
use crate::editing::COMPOSITION_PENDING;
use crate::workbench::tests::{drain, fixture, record};

pub(super) fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn new_scan_wizard_guards_ime_and_starts_active_and_passive_scan(cx: &mut TestAppContext) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);

    let raw = b"GET /search?q=seed HTTP/1.1\r\nHost: 127.0.0.1:18088\r\n\r\n".to_vec();
    let draft = Draft {
        revision: 2,
        target: "http://127.0.0.1:18088/search?q=seed".into(),
        raw: raw.clone(),
        version: HttpVersion::Http11,
    };

    cx.update_window(f.window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.send_draft_to_scanner(
                draft.clone(),
                AuditTaskMode::PassiveAndActive,
                false,
                window,
                cx,
            );
        });
        window.render_frame(cx);
        assert!(f.view.read(cx).tool == Tool::Dashboard);
        assert!(f.view.read(cx).audit_dashboard.new_scan_open);
    })
    .unwrap();
    let _ = drain(&mut f.commands);

    let url_input = cx.read(|cx| f.view.read(cx).audit_dashboard.scan_url_input.clone());
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        url_input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        });
        window.render_frame(cx);
        window.click("dashboard-start-scan", cx);
        window.render_frame(cx);
        assert_eq!(f.view.read(cx).error.as_deref(), Some(COMPOSITION_PENDING));
    })
    .unwrap();
    assert!(
        drain(&mut f.commands).is_empty(),
        "IME composition must block AuditCommand::StartScan"
    );

    cx.update_window(f.window, |_, window, cx| {
        url_input.update(cx, |state, cx| {
            state.replace_text_in_range(None, "", window, cx);
            state.set_value("http://127.0.0.1:18088/search?q=seed", window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();

    click(f.window, "dashboard-scan-mode-active", cx);
    click(f.window, "dashboard-start-scan", cx);

    let commands = drain(&mut f.commands);
    let [
        Command::Audit(AuditCommand::StartScan {
            project,
            task_id,
            draft: sent_draft,
            mode,
            crawl_same_origin,
            cancel,
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
    assert!(!*crawl_same_origin, "Crawl defaults to disabled");
    assert_eq!(sent_draft.target, "http://127.0.0.1:18088/search?q=seed");
    assert_eq!(sent_draft.raw, raw);
    assert!(!cancel.is_cancelled());
    let task_id = *task_id;

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(!f.view.read(cx).audit_dashboard.new_scan_open);
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-audit-task-{task_id}"
                )))
                .visible()
        );
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-audit-stop-{task_id}"
                )))
                .visible()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn task_updated_finding_selection_reads_flow_and_renders_evidence_and_remediation(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, Vec::new(), 64);
    drain(&mut f.commands);

    click(f.window, "tool-dashboard", cx);
    let _ = drain(&mut f.commands);

    let mut flow = record("http://127.0.0.1:18089/item?id=mio7f9c2a", "");
    let resp_body = b"<html><body>hit: mio7f9c2a</body></html>".to_vec();
    flow.response = Some(HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"text/html".to_vec(),
        }],
        body: resp_body,
        trailers: Vec::new(),
    });
    let resp_raw = response_to_raw(flow.response.as_ref().unwrap());
    let offset = resp_raw
        .windows(b"mio7f9c2a".len())
        .position(|w| w == b"mio7f9c2a")
        .unwrap();

    let finding = AuditFinding {
        id: Uuid::new_v4(),
        issue_id: "0x00200300".into(),
        title: "跨站脚本（反射型）".into(),
        severity_label: "高危".into(),
        severity_rank: IssueSeverity::High.rank(),
        confidence: IssueConfidence::Firm,
        phase: AuditPhase::Active,
        host: "127.0.0.1".into(),
        url: flow.request.url.clone(),
        parameter: Some("id".into()),
        flow_id: Some(flow.summary.id),
        summary: "无害 canary 原样反射。".into(),
        evidence: "mio7f9c2a".into(),
        request_highlights: vec![],
        response_highlights: vec![EvidenceSpan {
            offset,
            length: b"mio7f9c2a".len(),
        }],
        remediation: "对输出上下文执行严格 HTML 实体编码。".into(),
        detected_unix_ms: 100,
    };
    let task = AuditTaskSummary {
        id: Uuid::new_v4(),
        title: "主动扫描 · /item".into(),
        target_url: flow.request.url.clone(),
        mode: AuditTaskMode::PassiveAndActive,
        state: AuditTaskState::Completed,
        requests_sent: 4,
        total_planned: 4,
        issues_found: 1,
        started_unix_ms: 90,
        error: None,
    };

    f.events
        .try_send(Event::Audit(AuditEvent::TaskUpdated {
            project: f.project,
            revision: 1,
            task,
            new_findings: vec![finding.clone()],
        }))
        .unwrap();
    cx.run_until_parked();

    click(
        f.window,
        SharedString::from(format!("dashboard-issue-row-{}", finding.id)),
        cx,
    );

    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision,
            flow_id,
        },
    ] = commands.as_slice()
    else {
        panic!("selecting a finding with flow_id must emit Command::ReadFlow");
    };
    assert_eq!((*project, *flow_id), (f.project, flow.summary.id));

    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(flow.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let banner = window
            .find("dashboard-issue-evidence-banner")
            .label()
            .unwrap()
            .to_owned();
        assert!(banner.contains("mio7f9c2a"));
        assert!(banner.contains(&format!("[{offset}..{}]", offset + 9)));
        let remediation = window
            .find("dashboard-issue-remediation")
            .label()
            .unwrap()
            .to_owned();
        assert!(remediation.contains("HTML 实体编码"));
    })
    .unwrap();

    click(f.window, "dashboard-jump-response-evidence", cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            f.view.read(cx).audit_dashboard.detail_tab,
            IssueDetailTab::Response
        );
        assert!(
            f.view
                .read(cx)
                .audit_dashboard
                .issue_response_editor
                .raw
                .read(cx)
                .value()
                .contains("mio7f9c2a")
        );
        assert_eq!(
            f.view
                .read(cx)
                .audit_dashboard
                .issue_response_editor
                .search_match_count(cx),
            1
        );
    })
    .unwrap();
}
