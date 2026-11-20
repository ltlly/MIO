use std::fmt::Write as _;

use anyhow::{Context, Result};
use serde::Serialize;

use super::{AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence};
use crate::target_issues::IssueSeverity;
use crate::{AuditStateSnapshot, AuditTaskSummary};

const REPORT_CSS: &str = "\
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;margin:24px;color:#1f2328;background:#ffffff;line-height:1.5}\
h1,h2,h3{margin:0 0 12px}\
.meta{color:#57606a;margin-bottom:20px;font-size:14px}\
table{border-collapse:collapse;width:100%;max-width:960px;margin-bottom:24px;font-size:14px}\
th,td{border:1px solid #d0d7de;padding:8px 12px;text-align:left;vertical-align:top}\
th{background:#f6f8fa;font-weight:600}\
.finding{border:1px solid #d0d7de;border-radius:6px;padding:16px;margin-bottom:16px;max-width:960px}\
.badge{display:inline-block;padding:2px 8px;border-radius:4px;font-weight:600;font-size:12px;margin-right:8px}\
.sev-high{background:#fce8e6;color:#c5221f}\
.sev-medium{background:#fef7e0;color:#b06000}\
.sev-low{background:#e8f0fe;color:#1967d2}\
.sev-info{background:#eeeeee;color:#555555}\
code,pre{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px}\
pre.evidence{background:#fff8f7;border:1px solid #f1d2cf;border-radius:4px;padding:10px;overflow-x:auto;white-space:pre-wrap;word-break:break-all}\
.remediation{background:#f6f8fa;border:1px solid #d0d7de;border-radius:4px;padding:10px}\
.empty{color:#57606a;font-style:italic}";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AuditSeverityCounts {
    pub total: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub information: usize,
    pub passive: usize,
    pub active: usize,
    pub tasks_total: usize,
}

#[derive(Serialize)]
struct JsonFindingEntry<'a> {
    severity: &'static str,
    #[serde(flatten)]
    finding: &'a AuditFinding,
}

#[derive(Serialize)]
struct AuditJsonReport<'a> {
    format: &'static str,
    revision: u64,
    live_passive_enabled: bool,
    summary: AuditSeverityCounts,
    tasks: &'a [AuditTaskSummary],
    findings: Vec<JsonFindingEntry<'a>>,
}

pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn summarize_counts(snapshot: &AuditStateSnapshot) -> AuditSeverityCounts {
    let mut counts = AuditSeverityCounts {
        total: snapshot.findings.len(),
        tasks_total: snapshot.tasks.len(),
        ..AuditSeverityCounts::default()
    };
    for finding in &snapshot.findings {
        match finding.severity() {
            IssueSeverity::High => counts.high += 1,
            IssueSeverity::Medium => counts.medium += 1,
            IssueSeverity::Low => counts.low += 1,
            IssueSeverity::Information => counts.information += 1,
        }
        match finding.phase {
            AuditPhase::Passive => counts.passive += 1,
            AuditPhase::Active => counts.active += 1,
        }
    }
    counts
}

fn sorted_findings(snapshot: &AuditStateSnapshot) -> Vec<&AuditFinding> {
    let mut list: Vec<&AuditFinding> = snapshot.findings.iter().collect();
    list.sort_by(|a, b| {
        b.severity()
            .rank()
            .cmp(&a.severity().rank())
            .then_with(|| a.issue_id.cmp(&b.issue_id))
            .then_with(|| a.url.cmp(&b.url))
    });
    list
}

fn severity_css_class(sev: IssueSeverity) -> &'static str {
    match sev {
        IssueSeverity::High => "sev-high",
        IssueSeverity::Medium => "sev-medium",
        IssueSeverity::Low => "sev-low",
        IssueSeverity::Information => "sev-info",
    }
}

fn confidence_text(confidence: IssueConfidence) -> &'static str {
    match confidence {
        IssueConfidence::Certain => "Certain (确定)",
        IssueConfidence::Firm => "Firm (可靠)",
        IssueConfidence::Tentative => "Tentative (疑似)",
    }
}

fn phase_text(phase: AuditPhase) -> &'static str {
    match phase {
        AuditPhase::Passive => "Passive (被动合规检查)",
        AuditPhase::Active => "Active (主动审计探测)",
    }
}

fn format_spans(spans: &[EvidenceSpan]) -> String {
    if spans.is_empty() {
        return "-".to_owned();
    }
    spans
        .iter()
        .map(|s| format!("[{}..{}]", s.offset, s.offset.saturating_add(s.length)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn render_audit_report_json(snapshot: &AuditStateSnapshot) -> Result<Vec<u8>> {
    let summary = summarize_counts(snapshot);
    let findings = sorted_findings(snapshot)
        .into_iter()
        .map(|finding| JsonFindingEntry {
            severity: finding.severity().en_label(),
            finding,
        })
        .collect();
    let report = AuditJsonReport {
        format: "mio.audit_report.v1",
        revision: snapshot.revision,
        live_passive_enabled: snapshot.live_passive_enabled,
        summary,
        tasks: &snapshot.tasks,
        findings,
    };
    serde_json::to_vec_pretty(&report).context("序列化 JSON 审计报告失败")
}

pub fn render_audit_report_html(snapshot: &AuditStateSnapshot) -> String {
    let counts = summarize_counts(snapshot);
    let findings = sorted_findings(snapshot);
    let mut out = String::with_capacity(4096 + findings.len() * 768);
    out.push_str("<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">");
    out.push_str(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline';\">",
    );
    out.push_str("<title>MIO 审计合规报告</title><style>");
    out.push_str(REPORT_CSS);
    out.push_str("</style></head><body>");
    out.push_str("<h1>MIO 静态与合规审计报告</h1>");
    let _ = write!(
        out,
        "<div class=\"meta\">快照版本 Revision: {} · 实时被动审计: {} · 扫描任务: {} · 告警总数: {}（被动 {} / 主动 {}）</div>",
        snapshot.revision,
        if snapshot.live_passive_enabled {
            "开启"
        } else {
            "暂停"
        },
        counts.tasks_total,
        counts.total,
        counts.passive,
        counts.active,
    );

    out.push_str("<h2>严重度汇总统计 (Severity Summary)</h2>");
    out.push_str(
        "<table><thead><tr><th>Severity (严重度)</th><th>Count (数量)</th></tr></thead><tbody>",
    );
    for (label, count) in [
        ("High (高危)", counts.high),
        ("Medium (中危)", counts.medium),
        ("Low (低危)", counts.low),
        ("Information (信息)", counts.information),
        ("Total (总计)", counts.total),
    ] {
        let _ = write!(out, "<tr><td>{label}</td><td>{count}</td></tr>");
    }
    out.push_str("</tbody></table>");

    if !snapshot.tasks.is_empty() {
        out.push_str("<h2>审计任务概览 (Scan Tasks)</h2>");
        out.push_str("<table><thead><tr><th>任务名称</th><th>模式</th><th>状态</th><th>目标 URL</th><th>请求进度</th><th>发现告警</th><th>错误详情</th></tr></thead><tbody>");
        for task in &snapshot.tasks {
            let _ = write!(
                out,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}/{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&task.title),
                escape_html(task.mode.label()),
                escape_html(task.state.label()),
                escape_html(&task.target_url),
                task.requests_sent,
                task.total_planned,
                task.issues_found,
                escape_html(task.error.as_deref().unwrap_or("-")),
            );
        }
        out.push_str("</tbody></table>");
    }

    out.push_str("<h2>告警详情与修复建议 (Findings &amp; Remediation)</h2>");
    if findings.is_empty() {
        out.push_str("<p class=\"empty\">当前项目尚无审计告警记录。</p>");
    } else {
        for (idx, finding) in findings.into_iter().enumerate() {
            let sev = finding.severity();
            let cls = severity_css_class(sev);
            let param = finding.parameter.as_deref().unwrap_or("-");
            let flow_text = finding
                .flow_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_owned());
            let _ = write!(
                out,
                "<section class=\"finding\" data-severity=\"{}\"><h3>#{} <span class=\"badge {cls}\">{}</span>{} (<code>{}</code>)</h3>",
                sev.en_label(),
                idx + 1,
                escape_html(sev.bilingual_label()),
                escape_html(&finding.title),
                escape_html(&finding.issue_id),
            );
            let _ = write!(
                out,
                "<table><tbody>\
                <tr><th>Host</th><td><code>{}</code></td><th>Phase</th><td>{}</td></tr>\
                <tr><th>URL</th><td colspan=\"3\"><code>{}</code></td></tr>\
                <tr><th>Parameter</th><td><code>{}</code></td><th>Confidence</th><td>{}</td></tr>\
                <tr><th>Highlights</th><td>请求 {} · 响应 {}</td><th>Flow ID</th><td><code>{}</code></td></tr>\
                </tbody></table>",
                escape_html(&finding.host),
                phase_text(finding.phase),
                escape_html(&finding.url),
                escape_html(param),
                confidence_text(finding.confidence),
                format_spans(&finding.request_highlights),
                format_spans(&finding.response_highlights),
                escape_html(&flow_text),
            );
            let _ = write!(
                out,
                "<p><strong>问题摘要：</strong>{}</p>",
                escape_html(&finding.summary),
            );
            let _ = write!(
                out,
                "<div><strong>证据片段 (Evidence)：</strong><pre class=\"evidence\">{}</pre></div>",
                escape_html(&finding.evidence),
            );
            let _ = write!(
                out,
                "<div class=\"remediation\"><strong>修复建议 (Remediation)：</strong>{}</div></section>",
                escape_html(&finding.remediation),
            );
        }
    }
    out.push_str("</body></html>");
    out
}
