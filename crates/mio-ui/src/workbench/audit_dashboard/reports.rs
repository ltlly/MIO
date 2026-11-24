use super::*;

fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            other => out.push(other),
        }
    }
    out
}

pub(in crate::workbench) fn render_audit_report_xml(
    snapshot: &AuditStateSnapshot,
    false_positives: &HashSet<Uuid>,
    verified: &HashSet<Uuid>,
    notes: &HashMap<Uuid, String>,
) -> String {
    use std::fmt::Write as _;
    let mut findings: Vec<&AuditFinding> = snapshot.findings.iter().collect();
    findings.sort_by(|a, b| {
        b.severity()
            .rank()
            .cmp(&a.severity().rank())
            .then_with(|| a.issue_id.cmp(&b.issue_id))
            .then_with(|| a.url.cmp(&b.url))
    });
    let mut out = String::with_capacity(1024 + findings.len() * 512);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<issues format=\"mio.audit_report.v1\" revision=\"{}\" livePassiveEnabled=\"{}\" livePassiveCrawlEnabled=\"{}\">",
        snapshot.revision, snapshot.live_passive_enabled, snapshot.live_passive_crawl_enabled
    );
    for (idx, finding) in findings.into_iter().enumerate() {
        let is_fp = false_positives.contains(&finding.id);
        let is_verified = verified.contains(&finding.id);
        let severity_str = if is_fp {
            "False positive"
        } else {
            finding.severity().en_label()
        };
        let confidence_str = if is_verified {
            "Verified"
        } else {
            match finding.confidence {
                IssueConfidence::Certain => "Certain",
                IssueConfidence::Firm => "Firm",
                IssueConfidence::Tentative => "Tentative",
            }
        };
        let phase_str = match finding.phase {
            AuditPhase::Passive => "Passive",
            AuditPhase::Active => "Active",
        };
        let path_part = finding
            .url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("/");
        let location = match &finding.parameter {
            Some(param) if !param.is_empty() => format!("{path_part} [{param}]"),
            _ => path_part.to_owned(),
        };
        out.push_str("  <issue>\n");
        let _ = writeln!(out, "    <serialNumber>{}</serialNumber>", idx + 1);
        let _ = writeln!(out, "    <id>{}</id>", escape_xml(&finding.id.to_string()));
        let _ = writeln!(out, "    <type>{}</type>", escape_xml(&finding.issue_id));
        let _ = writeln!(out, "    <name>{}</name>", escape_xml(&finding.title));
        let _ = writeln!(out, "    <host>{}</host>", escape_xml(&finding.host));
        let _ = writeln!(out, "    <url>{}</url>", escape_xml(&finding.url));
        let _ = writeln!(out, "    <path>{}</path>", escape_xml(path_part));
        let _ = writeln!(out, "    <location>{}</location>", escape_xml(&location));
        let _ = writeln!(out, "    <severity>{}</severity>", escape_xml(severity_str));
        let _ = writeln!(
            out,
            "    <confidence>{}</confidence>",
            escape_xml(confidence_str)
        );
        let _ = writeln!(out, "    <phase>{}</phase>", escape_xml(phase_str));
        if let Some(param) = &finding.parameter {
            let _ = writeln!(out, "    <parameter>{}</parameter>", escape_xml(param));
        }
        let _ = writeln!(
            out,
            "    <issueBackground>{}</issueBackground>",
            escape_xml(&finding.summary)
        );
        let _ = writeln!(
            out,
            "    <remediationBackground>{}</remediationBackground>",
            escape_xml(&finding.remediation)
        );
        let _ = writeln!(
            out,
            "    <evidence>{}</evidence>",
            escape_xml(&finding.evidence)
        );
        if let Some(note) = notes.get(&finding.id).filter(|n| !n.trim().is_empty()) {
            let _ = writeln!(out, "    <notes>{}</notes>", escape_xml(note));
        }
        let _ = writeln!(
            out,
            "    <detectedUnixMs>{}</detectedUnixMs>",
            finding.detected_unix_ms
        );
        out.push_str("  </issue>\n");
    }
    out.push_str("</issues>\n");
    out
}


impl Workbench {
    pub(in crate::workbench) fn scoped_audit_snapshot(
        &self,
        scope: AuditReportScope,
    ) -> Result<AuditStateSnapshot, &'static str> {
        let base = &self.audit_dashboard.snapshot;
        match scope {
            AuditReportScope::All => Ok(base.clone()),
            AuditReportScope::SingleFinding(finding_id) => {
                let Some(finding) = base.findings.iter().find(|f| f.id == finding_id).cloned()
                else {
                    return Err("未找到要导出的漏洞告警记录。");
                };
                Ok(AuditStateSnapshot {
                    revision: base.revision,
                    live_passive_enabled: base.live_passive_enabled,
                    live_passive_crawl_enabled: base.live_passive_crawl_enabled,
                    live_passive_suite_scope_only: base.live_passive_suite_scope_only,
                    tasks: Vec::new(),
                    findings: vec![finding],
                })
            }
            AuditReportScope::Task(task_id) => {
                let Some(task) = base.tasks.iter().find(|t| t.id == task_id).cloned() else {
                    return Err("未找到对应的扫描任务记录。");
                };
                let findings = base
                    .findings
                    .iter()
                    .filter(|f| self.finding_belongs_to_audit_task(f, &task))
                    .cloned()
                    .collect();
                Ok(AuditStateSnapshot {
                    revision: base.revision,
                    live_passive_enabled: base.live_passive_enabled,
                    live_passive_crawl_enabled: base.live_passive_crawl_enabled,
                    live_passive_suite_scope_only: base.live_passive_suite_scope_only,
                    tasks: vec![task],
                    findings,
                })
            }
        }
    }

    pub(in crate::workbench) fn render_audit_report_bytes(
        &self,
        snapshot: &AuditStateSnapshot,
        format: AuditReportFormat,
    ) -> Result<Vec<u8>, String> {
        match format {
            AuditReportFormat::Html => Ok(render_audit_report_html(snapshot).into_bytes()),
            AuditReportFormat::Json => render_audit_report_json(snapshot)
                .map_err(|err| format!("生成 JSON 审计报告失败：{err}")),
            AuditReportFormat::Xml => Ok(render_audit_report_xml(
                snapshot,
                &self.audit_dashboard.false_positive_findings,
                &self.audit_dashboard.verified_confidence_findings,
                &self.audit_dashboard.finding_notes,
            )
            .into_bytes()),
        }
    }

    pub(in crate::workbench) fn save_audit_report_with_format(
        &mut self,
        format: AuditReportFormat,
        scope: AuditReportScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() || self.file_picker_pending() {
            return;
        }
        let snapshot = match self.scoped_audit_snapshot(scope) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let payload = match self.render_audit_report_bytes(&snapshot, format) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let finding_count = snapshot.findings.len();
        let suggested_name = match (scope, format) {
            (AuditReportScope::SingleFinding(_), AuditReportFormat::Html) => {
                "mio-issue-report.html"
            }
            (AuditReportScope::SingleFinding(_), AuditReportFormat::Json) => {
                "mio-issue-report.json"
            }
            (AuditReportScope::SingleFinding(_), AuditReportFormat::Xml) => "mio-issue-report.xml",
            (AuditReportScope::Task(_), AuditReportFormat::Html) => "mio-task-report.html",
            (AuditReportScope::Task(_), AuditReportFormat::Json) => "mio-task-report.json",
            (AuditReportScope::Task(_), AuditReportFormat::Xml) => "mio-task-report.xml",
            (AuditReportScope::All, AuditReportFormat::Html) => "mio-audit-report.html",
            (AuditReportScope::All, AuditReportFormat::Json) => "mio-audit-report.json",
            (AuditReportScope::All, AuditReportFormat::Xml) => "mio-audit-report.xml",
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested_name),
        );
        let owner = self.owner_window;
        let fp_snapshot = self.audit_dashboard.false_positive_findings.clone();
        let verified_snapshot = self.audit_dashboard.verified_confidence_findings.clone();
        let notes_snapshot = self.audit_dashboard.finding_notes.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_write = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != project {
                            view.fail("项目已切换，本次审计报告未写入磁盘。", cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(mut path))) => {
                                if path.extension().is_none() {
                                    path.set_extension(format.extension());
                                }
                                let ext = path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .map(str::to_ascii_lowercase);
                                let final_bytes = match ext.as_deref() {
                                    Some("json") if format != AuditReportFormat::Json => {
                                        match render_audit_report_json(&snapshot) {
                                            Ok(b) => b,
                                            Err(err) => {
                                                view.fail(format!("生成 JSON 报告失败：{err}"), cx);
                                                return None;
                                            }
                                        }
                                    }
                                    Some("html" | "htm") if format != AuditReportFormat::Html => {
                                        render_audit_report_html(&snapshot).into_bytes()
                                    }
                                    Some("xml") if format != AuditReportFormat::Xml => {
                                        render_audit_report_xml(
                                            &snapshot,
                                            &fp_snapshot,
                                            &verified_snapshot,
                                            &notes_snapshot,
                                        )
                                        .into_bytes()
                                    }
                                    _ => payload,
                                };
                                Some((path, final_bytes))
                            }
                            Ok(Ok(None)) => None,
                            Ok(Err(err)) => {
                                view.fail(format!("无法选择报告保存路径：{err}"), cx);
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已意外关闭。", cx);
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some((path, final_bytes)) = pending_write else {
                return;
            };
            let write_path = path.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_path, &final_bytes)
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| match write_result {
                    Ok(()) => {
                        view.error = None;
                        view.status = format!(
                            "已保存审计报告到文件：{}（共 {} 条告警）",
                            path.display(),
                            finding_count
                        );
                        cx.notify();
                    }
                    Err(err) => {
                        view.fail(format!("写入审计报告文件失败：{err}"), cx);
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn save_audit_report_to_file(
        &mut self,
        as_html: bool,
        single_finding_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let format = if as_html {
            AuditReportFormat::Html
        } else {
            AuditReportFormat::Json
        };
        let scope = match single_finding_id {
            Some(id) => AuditReportScope::SingleFinding(id),
            None => AuditReportScope::All,
        };
        self.save_audit_report_with_format(format, scope, window, cx);
    }

    pub(in crate::workbench) fn copy_audit_report_json_for_scope(
        &mut self,
        scope: AuditReportScope,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let snapshot = match self.scoped_audit_snapshot(scope) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        match render_audit_report_json(&snapshot) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(json) => {
                    let count = snapshot.findings.len();
                    cx.write_to_clipboard(ClipboardItem::new_string(json));
                    self.error = None;
                    self.status = format!("已复制 JSON 审计报告到剪贴板（共 {count} 条告警）。");
                    cx.notify();
                }
                Err(err) => self.fail(format!("导出 JSON 审计报告失败：{err}"), cx),
            },
            Err(err) => self.fail(format!("导出 JSON 审计报告失败：{err}"), cx),
        }
    }

    pub(in crate::workbench) fn copy_audit_report_html_for_scope(
        &mut self,
        scope: AuditReportScope,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let snapshot = match self.scoped_audit_snapshot(scope) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let html = render_audit_report_html(&snapshot);
        let count = snapshot.findings.len();
        cx.write_to_clipboard(ClipboardItem::new_string(html));
        self.error = None;
        self.status = format!("已复制 HTML 审计报告到剪贴板（共 {count} 条告警）。");
        cx.notify();
    }

    pub(in crate::workbench) fn copy_audit_report_json(&mut self, cx: &mut Context<Self>) {
        self.copy_audit_report_json_for_scope(AuditReportScope::All, cx);
    }

    pub(in crate::workbench) fn copy_audit_report_html(&mut self, cx: &mut Context<Self>) {
        self.copy_audit_report_html_for_scope(AuditReportScope::All, cx);
    }

}
