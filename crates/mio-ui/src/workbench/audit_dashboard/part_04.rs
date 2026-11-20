use super::*;

impl Workbench {
    pub(in crate::workbench) fn render_issue_message_editor(
        &self,
        finding: &AuditFinding,
        slot: AuditEditorSlot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if finding.flow_id.is_none() {
            return div()
                .id("dashboard-issue-no-flow")
                .test_support()
                .size_full()
                .p(px(16.))
                .text_color(rgb(if theme::is_dark() { 0x9da2a8 } else { 0x666666 }))
                .child("该告警未关联存储流量记录（FlowId）。")
                .into_any_element();
        }
        if let Some(err) = &self.audit_dashboard.flow_error {
            return div()
                .id("dashboard-issue-flow-error")
                .test_support()
                .role(Role::Alert)
                .size_full()
                .p(px(16.))
                .text_color(rgb(if theme::is_dark() { 0xf28b82 } else { 0x9b2727 }))
                .child(format!("读取关联流量失败：{err}"))
                .into_any_element();
        }
        if self.audit_dashboard.pending_flow_read.is_some()
            && self.audit_dashboard.loaded_flow.is_none()
        {
            return div()
                .id("dashboard-issue-flow-loading")
                .test_support()
                .size_full()
                .p(px(16.))
                .text_color(rgb(if theme::is_dark() { 0x9da2a8 } else { 0x666666 }))
                .child("正在读取关联请求 / 响应报文…")
                .into_any_element();
        }

        let (editor, prefix, title, aria) = match slot {
            AuditEditorSlot::IssueRequest => (
                &self.audit_dashboard.issue_request_editor,
                "dashboard-issue-request",
                "关联请求报文（只读）",
                "漏洞关联请求报文",
            ),
            AuditEditorSlot::IssueResponse | AuditEditorSlot::ScanRequest => (
                &self.audit_dashboard.issue_response_editor,
                "dashboard-issue-response",
                "关联响应报文（只读）",
                "漏洞关联响应报文",
            ),
        };
        let is_response = !matches!(slot, AuditEditorSlot::IssueRequest);

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.message_header(prefix, title, editor.view, cx))
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-editor")))
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::surface())
                    .child(if is_response {
                        self.render_response_editor_body(prefix, editor, cx)
                    } else {
                        Editor::new(editor.state())
                            .readonly(true)
                            .size_full()
                            .bordered(false)
                            .context_menu(self.request_context_menu(editor.state().clone(), true))
                            .aria_label(aria)
                            .into_any_element()
                    }),
            )
            .child(
                editor.render_search_bar(prefix, cx, move |this, action, window, cx| {
                    this.handle_audit_search_action(slot, action, window, cx);
                }),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn render_new_scan_request_editor(
        &self,
        pretty: bool,
        view: MessageView,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.audit_dashboard;
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child("种子 HTTP 请求")
                    .child(
                        button("dashboard-scan-request-pretty", "Pretty")
                            .selected(pretty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_audit_editor_pretty(
                                    AuditEditorSlot::ScanRequest,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("dashboard-scan-request-raw", "Raw")
                            .selected(!pretty && view == MessageView::Raw)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_audit_editor_view(
                                    AuditEditorSlot::ScanRequest,
                                    MessageView::Raw,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("dashboard-scan-request-hex", "Hex")
                            .selected(!pretty && view == MessageView::Hex)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_audit_editor_view(
                                    AuditEditorSlot::ScanRequest,
                                    MessageView::Hex,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .id("dashboard-scan-request-editor")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::surface())
                    .child(
                        Editor::new(pane.scan_request_editor.state())
                            .readonly(readonly || pane.scan_request_editor.is_readonly_preview())
                            .size_full()
                            .bordered(false)
                            .context_menu(self.request_context_menu(
                                pane.scan_request_editor.state().clone(),
                                readonly,
                            ))
                            .aria_label("扫描向导种子请求编辑器"),
                    ),
            )
            .child(pane.scan_request_editor.render_search_bar(
                "dashboard-scan-request",
                cx,
                |this, action, window, cx| {
                    this.handle_audit_search_action(
                        AuditEditorSlot::ScanRequest,
                        action,
                        window,
                        cx,
                    );
                },
            ))
            .into_any_element()
    }

    pub(in crate::workbench) fn on_audit_flow_detail(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        record: &FlowRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(project) {
            return false;
        }
        if let Some(batch) = self.audit_dashboard.pending_batch_transfer.as_mut()
            && batch.pending_revisions.remove(&selection_revision).is_some()
        {
            self.audit_dashboard
                .flow_cache
                .insert(record.summary.id, record.clone());
            if batch.pending_revisions.is_empty()
                && let Some(done) = self.audit_dashboard.pending_batch_transfer.take()
            {
                self.complete_audit_batch_transfer(done, window, cx);
            }
            return true;
        }
        if self.audit_dashboard.pending_flow_read
            != Some((project, selection_revision, record.summary.id))
        {
            return false;
        }
        self.audit_dashboard.pending_flow_read = None;
        self.audit_dashboard.flow_error = None;
        self.audit_dashboard
            .flow_cache
            .insert(record.summary.id, record.clone());
        let req_raw = request_to_raw(&record.request);
        let resp_raw = record
            .response
            .as_ref()
            .map(response_to_raw)
            .unwrap_or_default();
        self.audit_dashboard
            .issue_request_editor
            .load(&req_raw, window, cx);
        self.audit_dashboard
            .issue_response_editor
            .load(&resp_raw, window, cx);
        self.audit_dashboard.loaded_flow = Some(Box::new(record.clone()));
        self.apply_selected_finding_evidence_highlight(&req_raw, &resp_raw, window, cx);
        cx.notify();
        true
    }

    pub(in crate::workbench) fn on_audit_flow_detail_error(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        flow_id: FlowId,
        message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(project) {
            return false;
        }
        if let Some(batch) = self.audit_dashboard.pending_batch_transfer.as_mut()
            && batch.pending_revisions.remove(&selection_revision).is_some()
        {
            if batch.pending_revisions.is_empty()
                && let Some(done) = self.audit_dashboard.pending_batch_transfer.take()
            {
                let has_any = done
                    .flow_ids
                    .iter()
                    .any(|id| self.audit_dashboard.flow_cache.contains_key(id));
                if has_any {
                    self.complete_audit_batch_transfer(done, window, cx);
                } else {
                    self.fail(format!("批量读取漏洞关联流量失败：{message}"), cx);
                }
            }
            return true;
        }
        if self.audit_dashboard.pending_flow_read != Some((project, selection_revision, flow_id)) {
            return false;
        }
        self.audit_dashboard.pending_flow_read = None;
        self.audit_dashboard.flow_error = Some(message.to_owned());
        self.fail(format!("无法读取漏洞关联流量：{message}"), cx);
        cx.notify();
        true
    }

    pub(in crate::workbench) fn apply_selected_finding_evidence_highlight(
        &mut self,
        req_raw: &[u8],
        resp_raw: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_id) = self.audit_dashboard.selected_finding_id else {
            return;
        };
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == selected_id)
            .cloned()
        else {
            return;
        };
        if let Some(span) = finding.request_highlights.first() {
            let needle = extract_highlight_needle(req_raw, *span, &finding.evidence);
            if !needle.is_empty() {
                set_editor_plain_query(
                    &mut self.audit_dashboard.issue_request_editor,
                    needle,
                    window,
                    cx,
                );
            }
        }
        if let Some(span) = finding.response_highlights.first() {
            let needle = extract_highlight_needle(resp_raw, *span, &finding.evidence);
            if !needle.is_empty() {
                set_editor_plain_query(
                    &mut self.audit_dashboard.issue_response_editor,
                    needle,
                    window,
                    cx,
                );
            }
        }
    }

    pub(in crate::workbench) fn jump_to_finding_evidence(
        &mut self,
        in_request: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_id) = self.audit_dashboard.selected_finding_id else {
            return;
        };
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == selected_id)
            .cloned()
        else {
            return;
        };
        self.audit_dashboard.detail_tab = if in_request {
            IssueDetailTab::Request
        } else {
            IssueDetailTab::Response
        };
        let raw = if in_request {
            self.audit_dashboard
                .loaded_flow
                .as_ref()
                .map(|r| request_to_raw(&r.request))
                .unwrap_or_default()
        } else {
            self.audit_dashboard
                .loaded_flow
                .as_ref()
                .and_then(|r| r.response.as_ref().map(response_to_raw))
                .unwrap_or_default()
        };
        let spans = if in_request {
            &finding.request_highlights
        } else {
            &finding.response_highlights
        };
        let needle = spans
            .first()
            .map(|span| extract_highlight_needle(&raw, *span, &finding.evidence))
            .unwrap_or_else(|| finding.evidence.lines().next().unwrap_or("").to_owned());
        let editor = if in_request {
            &mut self.audit_dashboard.issue_request_editor
        } else {
            &mut self.audit_dashboard.issue_response_editor
        };
        if !needle.is_empty() {
            set_editor_plain_query(editor, needle, window, cx);
        }
        cx.notify();
    }
}

impl AuditDashboardPane {
    pub(in crate::workbench) fn validate_scan_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in [
            &self.scan_title_input,
            &self.scan_url_input,
            &self.scan_request_editor.search,
        ] {
            editing::input_snapshot(input, window, cx)?;
        }
        for editor in [
            &self.scan_request_editor.raw,
            &self.scan_request_editor.hex,
            &self.scan_request_editor.pretty,
        ] {
            editing::snapshot(editor, window, cx)?;
        }
        Ok(())
    }

    pub(in crate::workbench) fn has_non_editor_focus(&self, window: &Window, cx: &App) -> bool {
        [
            &self.scan_title_input,
            &self.scan_url_input,
            &self.task_search_input,
            &self.issue_filter_input,
            &self.finding_note_input,
        ]
        .into_iter()
        .any(|input| input.focus_handle(cx).is_focused(window))
    }
}

pub(in crate::workbench) fn severity_badge_style(
    severity: IssueSeverity,
) -> (&'static str, u32, u32) {
    let dark = theme::is_dark();
    match severity {
        IssueSeverity::High => (
            "High 高危",
            if dark { 0x4a2020 } else { 0xfce8e6 },
            if dark { 0xf28b82 } else { 0xc5221f },
        ),
        IssueSeverity::Medium => (
            "Medium 中危",
            if dark { 0x423018 } else { 0xfef7e0 },
            if dark { 0xfdd663 } else { 0xb06000 },
        ),
        IssueSeverity::Low => (
            "Low 低危",
            if dark { 0x1e3554 } else { 0xe8f0fe },
            if dark { 0x8ab4f8 } else { 0x1967d2 },
        ),
        IssueSeverity::Information => (
            "Info 信息",
            if dark { 0x35383d } else { 0xeeeeee },
            if dark { 0x9da2a8 } else { 0x555555 },
        ),
    }
}

pub(in crate::workbench) fn severity_display_style(
    severity: IssueSeverity,
    is_false_positive: bool,
) -> (&'static str, u32, u32) {
    if is_false_positive {
        let dark = theme::is_dark();
        (
            "误报 FP",
            if dark { 0x35383d } else { 0xececec },
            if dark { 0x9da2a8 } else { 0x6b7280 },
        )
    } else {
        severity_badge_style(severity)
    }
}

pub(in crate::workbench) fn confidence_label(confidence: IssueConfidence) -> &'static str {
    match confidence {
        IssueConfidence::Certain => "Certain 确定",
        IssueConfidence::Firm => "Firm 明确",
        IssueConfidence::Tentative => "Tentative 推测",
    }
}

pub(in crate::workbench) fn confidence_display_label(
    confidence: IssueConfidence,
    is_verified: bool,
) -> &'static str {
    if is_verified {
        "Verified 已验证"
    } else {
        confidence_label(confidence)
    }
}

pub(in crate::workbench) fn phase_label(phase: AuditPhase) -> &'static str {
    match phase {
        AuditPhase::Passive => "被动审计 Passive",
        AuditPhase::Active => "主动审计 Active",
    }
}

pub(in crate::workbench) fn format_evidence_spans(
    prefix: &str,
    spans: &[EvidenceSpan],
    evidence: &str,
) -> Option<String> {
    let first = spans.first()?;
    let end = first.offset.saturating_add(first.length);
    let extra = if spans.len() > 1 {
        format!(" (+{} 处)", spans.len() - 1)
    } else {
        String::new()
    };
    let preview: String = evidence.chars().take(96).collect();
    Some(format!(
        "{prefix}: [{}..{end}]{extra} {preview}",
        first.offset
    ))
}
