use super::*;
use gpui_kit::assets::IconName;

impl Workbench {
    pub(in crate::workbench) fn render_issue_finding_detail(
        &self,
        finding: &AuditFinding,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.audit_dashboard.detail_tab;
        let has_flow = finding.flow_id.is_some();
        let finding_id = finding.id;
        let can_operate = self.audit_can_operate();
        let grouped_evidence = theme::issues_grouped_evidence();
        let sibling_findings: Vec<(usize, Uuid, String)> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .filter(|f| {
                f.issue_id == finding.issue_id
                    && f.url == finding.url
                    && (!self.audit_dashboard.false_positive_findings.contains(&f.id)
                        || f.id == finding_id)
            })
            .enumerate()
            .map(|(idx, f)| {
                let param_lbl = f.parameter.as_deref().unwrap_or("Evidence");
                (idx + 1, f.id, format!("#{} {param_lbl}", idx + 1))
            })
            .collect();
        let req_ev = format_evidence_spans(
            "请求证据高亮",
            &finding.request_highlights,
            &finding.evidence,
        );
        let resp_ev = format_evidence_spans(
            "响应证据高亮",
            &finding.response_highlights,
            &finding.evidence,
        );
        let evidence_summary = match (&req_ev, &resp_ev) {
            (Some(req), Some(resp)) => format!("{req} · {resp}"),
            (Some(req), None) => req.clone(),
            (None, Some(resp)) => resp.clone(),
            (None, None) => {
                let preview: String = finding.evidence.chars().take(120).collect();
                format!("证据摘要: {preview}")
            }
        };

        let req_btn = button("dashboard-issue-tab-request", "请求")
            .icon(
                gpui_kit::component::Icon::new(IconName::SendHorizontal)
                    .size(theme::scaled_px(13.)),
            )
            .tooltip("查看触发该漏洞的关联 HTTP 请求报文")
            .selected(tab == IssueDetailTab::Request)
            .on_click(cx.listener(|this, _, _, cx| {
                this.audit_dashboard.detail_tab = IssueDetailTab::Request;
                cx.notify();
            }));
        let resp_btn = button("dashboard-issue-tab-response", "响应")
            .icon(
                gpui_kit::component::Icon::new(IconName::FileText)
                    .size(theme::scaled_px(13.)),
            )
            .tooltip("查看触发该漏洞的关联 HTTP 响应报文")
            .selected(tab == IssueDetailTab::Response)
            .on_click(cx.listener(|this, _, _, cx| {
                this.audit_dashboard.detail_tab = IssueDetailTab::Response;
                cx.notify();
            }));

        let mut tab_bar = toolbar()
            .child(
                button("dashboard-issue-tab-advisory", "问题详情")
                    .icon(
                        gpui_kit::component::Icon::new(IconName::Info).size(theme::scaled_px(13.)),
                    )
                    .tooltip("查看漏洞背景、风险描述与修复建议 (Advisory)")
                    .selected(tab == IssueDetailTab::Advisory)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.audit_dashboard.detail_tab = IssueDetailTab::Advisory;
                        cx.notify();
                    })),
            );
        if grouped_evidence {
            tab_bar = tab_bar.child(
                    div()
                        .id("dashboard-issue-evidence-grouped-tabs")
                        .test_support()
                        .px(px(5.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(theme::scaled_px(11.5))
                                .text_color(theme::muted_foreground())
                                .child(format!("证据组 ({})", sibling_findings.len().max(1))),
                        )
                        .child(req_btn)
                        .child(resp_btn),
                );
        } else {
            let mut bar = tab_bar.child(req_btn).child(resp_btn);
                if sibling_findings.len() > 1 {
                    for (ordinal, sib_id, sib_label) in &sibling_findings {
                        let target_id = *sib_id;
                        let active = target_id == finding_id;
                        bar = bar.child(
                            button(
                                ("dashboard-issue-ungrouped-evidence-tab", *ordinal),
                                sib_label.clone(),
                            )
                            .selected(active)
                            .tooltip("切换查看独立拆分证据标签页")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_audit_finding(target_id, window, cx);
                            })),
                        );
                    }
                }
            tab_bar = bar;
        }
        let tab_bar = tab_bar
            .when(has_flow && !finding.request_highlights.is_empty(), |bar| {
                bar.child(
                    button("dashboard-jump-request-evidence", "定位请求证据")
                        .icon(
                            gpui_kit::component::Icon::new(IconName::Crosshair)
                                .size(theme::scaled_px(13.)),
                        )
                        .tooltip("跳转到请求报文中高亮的漏洞证据偏移位置")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.jump_to_finding_evidence(true, window, cx);
                        })),
                )
            })
            .when(has_flow && !finding.response_highlights.is_empty(), |bar| {
                bar.child(
                    button("dashboard-jump-response-evidence", "定位响应证据")
                        .icon(
                            gpui_kit::component::Icon::new(IconName::Crosshair)
                                .size(theme::scaled_px(13.)),
                        )
                        .tooltip("跳转到响应报文中高亮的漏洞证据偏移位置")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.jump_to_finding_evidence(false, window, cx);
                        })),
                )
            })
            .child(
                button("dashboard-delete-finding", "标记误报并移除")
                    .icon(
                        gpui_kit::component::Icon::new(IconName::Trash).size(theme::scaled_px(13.)),
                    )
                    .tooltip("从当前项目中移除此条漏洞告警记录")
                    .disabled(!can_operate)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.delete_audit_finding(finding_id, window, cx);
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("dashboard-issue-evidence-banner")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(evidence_summary.clone())
                    .max_w(px(460.))
                    .truncate()
                    .text_color(theme::danger_fg())
                    .child(evidence_summary),
            );

        let body = match tab {
            IssueDetailTab::Advisory => self.render_issue_advisory_body(finding, cx),
            IssueDetailTab::Request => {
                self.render_issue_message_editor(finding, AuditEditorSlot::IssueRequest, cx)
            }
            IssueDetailTab::Response => {
                self.render_issue_message_editor(finding, AuditEditorSlot::IssueResponse, cx)
            }
        };

        div()
            .id("dashboard-issue-detail")
            .test_support()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(tab_bar)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_issue_advisory_body(
        &self,
        finding: &AuditFinding,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let finding_id = finding.id;
        let current_sev = finding.severity();
        let current_conf = finding.confidence;
        let can_operate = self.audit_can_operate();
        let is_fp = self
            .audit_dashboard
            .false_positive_findings
            .contains(&finding_id);
        let is_verified = self
            .audit_dashboard
            .verified_confidence_findings
            .contains(&finding_id);
        let grouped_evidence = theme::issues_grouped_evidence();
        let grouped_siblings: Vec<&AuditFinding> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .filter(|f| f.issue_id == finding.issue_id && f.url == finding.url)
            .collect();
        let note_text = self.audit_dashboard.finding_notes.get(&finding_id).cloned();
        let (sev_text, badge_bg, badge_fg) = severity_display_style(current_sev, is_fp);
        let param_text = finding.parameter.as_deref().unwrap_or("（无特定参数）");
        let req_spans_text = if finding.request_highlights.is_empty() {
            "无".to_owned()
        } else {
            finding
                .request_highlights
                .iter()
                .map(|s| format!("[{}..{}]", s.offset, s.offset.saturating_add(s.length)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let resp_spans_text = if finding.response_highlights.is_empty() {
            "无".to_owned()
        } else {
            finding
                .response_highlights
                .iter()
                .map(|s| format!("[{}..{}]", s.offset, s.offset.saturating_add(s.length)))
                .collect::<Vec<_>>()
                .join(", ")
        };

        div()
            .id("dashboard-issue-advisory")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(2.))
                            .bg(rgb(badge_bg))
                            .text_color(rgb(badge_fg))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(sev_text),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} ({})", finding.title, finding.issue_id)),
                    )
                    .child(format!(
                        "置信度：{}",
                        confidence_display_label(finding.confidence, is_verified)
                    ))
                    .child(format!("来源：{}", phase_label(finding.phase))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child("严重度研判：")
                    .children(
                        [
                            ("dashboard-finding-sev-high", "High", IssueSeverity::High),
                            (
                                "dashboard-finding-sev-medium",
                                "Medium",
                                IssueSeverity::Medium,
                            ),
                            ("dashboard-finding-sev-low", "Low", IssueSeverity::Low),
                            (
                                "dashboard-finding-sev-info",
                                "Information",
                                IssueSeverity::Information,
                            ),
                        ]
                        .into_iter()
                        .map(|(id, label, target_sev)| {
                            button(id, label)
                                .tooltip(format!("将此漏洞告警人工研判严重度设为 {label}"))
                                .selected(!is_fp && current_sev == target_sev)
                                .disabled(!can_operate)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_audit_finding_severity_override(
                                        finding_id,
                                        target_sev,
                                        cx,
                                    );
                                }))
                        }),
                    )
                    .child("置信度：")
                    .children(
                        [
                            (
                                "dashboard-finding-conf-certain",
                                "Certain",
                                IssueConfidence::Certain,
                            ),
                            ("dashboard-finding-conf-firm", "Firm", IssueConfidence::Firm),
                            (
                                "dashboard-finding-conf-tentative",
                                "Tentative",
                                IssueConfidence::Tentative,
                            ),
                        ]
                        .into_iter()
                        .map(|(id, label, target_conf)| {
                            button(id, label)
                                .tooltip(format!("将此漏洞告警人工研判置信度设为 {label}"))
                                .selected(!is_verified && current_conf == target_conf)
                                .disabled(!can_operate)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_audit_finding_confidence_override(
                                        finding_id,
                                        target_conf,
                                        cx,
                                    );
                                }))
                        }),
                    )
                    .child(
                        button(
                            "dashboard-finding-edit-note",
                            if note_text.is_some() {
                                "编辑备注"
                            } else {
                                "添加备注"
                            },
                        )
                        .icon(
                            gpui_kit::component::Icon::new(IconName::StickyNote)
                                .size(theme::scaled_px(12.5)),
                        )
                        .tooltip("为此漏洞添加或修改备注")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_audit_finding_note_editor(finding_id, window, cx);
                        })),
                    ),
            )
            .when_some(note_text, |col, note| {
                col.child(
                    div()
                        .id("dashboard-issue-note-box")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(format!("问题备注：{note}"))
                        .p(px(10.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::toolbar_surface())
                        .child(format!("备注（Notes）：{note}")),
                )
            })
            .child(format!("目标 URL：{} · 参数：{param_text}", finding.url))
            .child(
                div()
                    .id("dashboard-issue-summary-text")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(finding.summary.clone())
                    .child(format!("问题摘要：{}", finding.summary)),
            )
            .child(
                div()
                    .id("dashboard-issue-evidence-box")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(format!(
                        "证据高亮: 请求 {req_spans_text} · 响应 {resp_spans_text} · {}",
                        finding.evidence
                    ))
                    .p(px(10.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(if theme::is_dark() { 0x6b2c2a } else { 0xf1d2cf }))
                    .bg(rgb(if theme::is_dark() { 0x3d2322 } else { 0xfff8f7 }))
                    .child(format!(
                        "证据高亮偏移 — 请求：{req_spans_text} · 响应：{resp_spans_text}\n证据片段：{}",
                        finding.evidence
                    ))
                    .when(grouped_evidence && grouped_siblings.len() > 1, |box_el| {
                        let merged_lines: Vec<String> = grouped_siblings
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                format!(
                                    "#{}: 参数 {} — {}",
                                    i + 1,
                                    f.parameter.as_deref().unwrap_or("无"),
                                    f.evidence
                                )
                            })
                            .collect();
                        box_el.child(
                            div()
                                .id("dashboard-issue-grouped-evidence-merged")
                                .test_support()
                                .mt(px(6.))
                                .pt(px(6.))
                                .border_t_1()
                                .border_color(theme::border())
                                .child(format!(
                                    "同漏洞合并分组证据（共 {} 条）：\n{}",
                                    grouped_siblings.len(),
                                    merged_lines.join("\n")
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .id("dashboard-issue-remediation")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(finding.remediation.clone())
                    .p(px(10.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(format!("修复建议（Remediation）：{}", finding.remediation)),
            )
            .into_any_element()
    }


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

        let (editor, editor_slot, title, aria) = match slot {
            AuditEditorSlot::IssueRequest => (
                &self.audit_dashboard.issue_request_editor,
                super::wb_editors::EditorSlot::DashboardIssueRequest,
                "关联请求报文（只读）",
                "漏洞关联请求报文",
            ),
            AuditEditorSlot::IssueResponse | AuditEditorSlot::ScanRequest => (
                &self.audit_dashboard.issue_response_editor,
                super::wb_editors::EditorSlot::DashboardIssueResponse,
                "关联响应报文（只读）",
                "漏洞关联响应报文",
            ),
        };
        let prefix = editor_slot.id();
        let is_response = !matches!(slot, AuditEditorSlot::IssueRequest);

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.message_header(editor_slot, title, editor.view(), cx))
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-editor")))
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::surface())
                    .child(if is_response {
                        self.render_response_editor_body(editor_slot, editor, cx)
                    } else {
                        editing::message_editor(editor)
                            .readonly(true)
                            .size_full()
                            .context_menu(self.request_context_menu(editor.state(), true))
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

}
