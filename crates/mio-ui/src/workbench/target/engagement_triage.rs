use mio_core::{
    AuditCommand, AuditStateSnapshot,
    audit_engine::{AuditPhase, IssueConfidence, render_audit_report_html, render_audit_report_json},
};

use super::engagement::*;
use super::*;

impl Workbench {
    // --- Tree branch expansion & collapse ---

    pub(in crate::workbench) fn expand_target_branch(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.target_pane.tree_state.expand_branch(&key);
        self.error = None;
        self.status = if expanded > 0 {
            format!("已展开分支及 {expanded} 个子节点。")
        } else {
            "该分支已全部展开。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn expand_target_requested_items(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let mut requested_keys = HashSet::new();
        let history_rows = self.history.read(cx).delegate().rows.clone();
        for row in self.target_pane.items.iter().chain(&history_rows) {
            if self.target_pane.hidden_item_ids.contains(&row.id()) {
                continue;
            }
            let url = row_url(row);
            if row_status_code(row).is_some()
                && !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                && url_matches_node_prefix(&url, &key)
                && let Some(canon) = canonical_site_map_url_key(&url)
            {
                requested_keys.insert(canon);
            }
        }
        let candidate_node_keys: Vec<String> = self
            .target_pane
            .tree_state
            .nodes
            .iter()
            .filter(|node| {
                !self.target_pane.tree_state.is_hidden(node)
                    && self.target_pane.tree_state.is_descendant_or_self(node, &key)
            })
            .filter(|node| {
                if !requested_keys.is_empty() {
                    let node_canon =
                        canonical_site_map_url_key(&node.key).unwrap_or_else(|| node.key.clone());
                    requested_keys.contains(&node_canon)
                } else {
                    node.direct_count > 0
                }
            })
            .map(|node| node.key.clone())
            .collect();

        let mut expanded = 0usize;
        for target_node in &candidate_node_keys {
            expanded += self
                .target_pane
                .tree_state
                .expand_ancestors_within(target_node, &key);
        }
        self.error = None;
        self.status = if candidate_node_keys.is_empty() {
            "该分支下暂无已请求项可展开。".into()
        } else if expanded > 0 {
            format!("已展开 {} 个包含已请求项的节点。", expanded)
        } else {
            "该分支下的已请求项均已处于展开状态。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn collapse_target_branch(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let collapsed = self.target_pane.tree_state.collapse_branch(&key);
        self.error = None;
        self.status = if collapsed > 0 {
            format!("已折叠分支及 {collapsed} 个子节点。")
        } else {
            "该分支已处于折叠状态。".into()
        };
        cx.notify();
    }

    // --- Branch issues report & delete ---

    pub(in crate::workbench) fn save_branch_audit_report_to_file(
        &mut self,
        key: String,
        as_html: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() || self.file_picker_pending() {
            return;
        }
        let findings: Vec<AuditFinding> = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            Some(&key),
            &self.target_pane.tree_state.hidden_prefixes,
        )
        .into_iter()
        .cloned()
        .collect();
        if findings.is_empty() {
            self.fail("当前选中分支下暂无可导出的漏洞告警。", cx);
            return;
        }
        let snapshot = AuditStateSnapshot {
            revision: self.audit_dashboard.snapshot.revision,
            live_passive_enabled: self.audit_dashboard.snapshot.live_passive_enabled,
            live_passive_crawl_enabled: self.audit_dashboard.snapshot.live_passive_crawl_enabled,
            live_passive_suite_scope_only: self
                .audit_dashboard
                .snapshot
                .live_passive_suite_scope_only,
            tasks: Vec::new(),
            findings,
        };
        let payload = if as_html {
            render_audit_report_html(&snapshot).into_bytes()
        } else {
            match render_audit_report_json(&snapshot) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(format!("生成分支 JSON 报告失败：{err}"), cx);
                    return;
                }
            }
        };
        let finding_count = snapshot.findings.len();
        let suggested = if as_html {
            "site-map-branch-issues.html"
        } else {
            "site-map-branch-issues.json"
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，分支报告未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension(if as_html { "html" } else { "json" });
                            }
                            match mio_core::ui_files::write_export_file(&path, &payload) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存分支漏洞报告到文件：{}（共 {} 条告警）",
                                        path.display(),
                                        finding_count
                                    );
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入分支漏洞报告失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择报告保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn delete_branch_audit_findings(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let ids: Vec<Uuid> = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            Some(&key),
            &self.target_pane.tree_state.hidden_prefixes,
        )
        .into_iter()
        .map(|f| f.id)
        .collect();
        if ids.is_empty() {
            self.fail("当前选中分支下暂无可删除的漏洞告警。", cx);
            return;
        }
        let mut deleted = 0usize;
        for finding_id in &ids {
            if self.enqueue(
                Command::Audit(AuditCommand::DeleteFinding {
                    project,
                    finding_id: *finding_id,
                }),
                cx,
            ) {
                deleted += 1;
            }
        }
        let id_set: HashSet<Uuid> = ids.into_iter().collect();
        self.audit_dashboard
            .snapshot
            .findings
            .retain(|f| !id_set.contains(&f.id));
        if self
            .target_pane
            .selected_site_map_finding_id
            .is_some_and(|sel| id_set.contains(&sel))
        {
            self.target_pane.selected_site_map_finding_id = None;
        }
        self.reconcile_audit_selection(window, cx);
        self.error = None;
        self.status = format!("已删除该分支下的 {deleted} 条漏洞告警。");
        cx.notify();
    }

    // --- Single Site Map Issue row actions ---

    fn remember_finding_original(&mut self, finding: &AuditFinding) {
        self.target_pane
            .finding_originals
            .entry(finding.id)
            .or_insert((finding.severity(), finding.confidence));
    }

    pub(in crate::workbench) fn set_target_finding_severity(
        &mut self,
        finding_id: Uuid,
        severity: MioIssueSeverity,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        self.remember_finding_original(&finding);
        self.audit_dashboard
            .false_positive_findings
            .remove(&finding_id);
        self.set_audit_finding_triage(finding_id, severity, finding.confidence, cx);
        self.status = format!("已将问题严重性设置为 {}。", severity.label());
        cx.notify();
    }

    pub(in crate::workbench) fn restore_target_finding_severity(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        let orig_sev = self
            .target_pane
            .finding_originals
            .get(&finding_id)
            .map(|(sev, _)| *sev)
            .or_else(|| {
                issue_definitions()
                    .iter()
                    .find(|def| def.id == finding.issue_id)
                    .map(|def| def.typical_severity)
            })
            .unwrap_or_else(|| finding.severity());
        self.audit_dashboard
            .false_positive_findings
            .remove(&finding_id);
        self.set_audit_finding_triage(finding_id, orig_sev, finding.confidence, cx);
        self.status = format!("已恢复问题原始严重性（{}）。", orig_sev.label());
        cx.notify();
    }

    pub(in crate::workbench) fn set_target_finding_confidence(
        &mut self,
        finding_id: Uuid,
        confidence: IssueConfidence,
        verified: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        self.remember_finding_original(&finding);
        if verified {
            self.target_pane.verified_findings.insert(finding_id);
        } else {
            self.target_pane.verified_findings.remove(&finding_id);
        }
        self.set_audit_finding_triage(finding_id, finding.severity(), confidence, cx);
        self.status = if verified {
            "已将问题置信度设置为已验证 (Certain)。".into()
        } else {
            format!(
                "已将问题置信度设置为 {}。",
                crate::workbench::audit_dashboard::confidence_label(confidence)
            )
        };
        cx.notify();
    }

    pub(in crate::workbench) fn restore_target_finding_confidence(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        let orig_conf = self
            .target_pane
            .finding_originals
            .get(&finding_id)
            .map(|(_, conf)| *conf)
            .unwrap_or(finding.confidence);
        self.target_pane.verified_findings.remove(&finding_id);
        self.set_audit_finding_triage(finding_id, finding.severity(), orig_conf, cx);
        self.status = format!(
            "已恢复问题原始置信度（{}）。",
            crate::workbench::audit_dashboard::confidence_label(orig_conf)
        );
        cx.notify();
    }

    pub(in crate::workbench) fn delete_target_site_map_finding(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.verified_findings.remove(&finding_id);
        self.target_pane.finding_originals.remove(&finding_id);
        if self.target_pane.selected_site_map_finding_id == Some(finding_id) {
            self.target_pane.selected_site_map_finding_id = None;
        }
        self.delete_audit_finding(finding_id, window, cx);
        self.status = "已删除所选问题。".into();
        cx.notify();
    }

    pub(super) fn push_text_to_comparer(
        &mut self,
        label: &str,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if bytes.len() > MAX_COMPARER_ITEM_BYTES {
            self.fail(
                format!(
                    "Comparer 单项超过 {} KiB 上限，未添加。",
                    MAX_COMPARER_ITEM_BYTES / 1024
                ),
                cx,
            );
            return false;
        }
        if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
            self.fail(
                format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                cx,
            );
            return false;
        }
        let id = self
            .extra_tools
            .comparer_items
            .last()
            .map_or(1, |item| item.id + 1);
        self.extra_tools
            .comparer_items
            .push(tools_extra::ComparerItem {
                id,
                label: format!("{id}: {label} ({} bytes)", bytes.len()),
                bytes,
            });
        let count = self.extra_tools.comparer_items.len();
        self.extra_tools.comparer_selected = count - 1;
        if count >= 2 {
            self.extra_tools.left_index = count - 2;
            self.extra_tools.right_index = count - 1;
        }
        self.refresh_comparer(window, cx);
        true
    }

    pub(in crate::workbench) fn send_target_issue_to_comparer(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            self.fail("所选漏洞告警已不存在。", cx);
            return;
        };
        let mut text = format!(
            "Issue: {}\nIssue ID: {}\nSeverity: {}\nConfidence: {}\nHost: {}\nURL: {}\n",
            finding.title,
            finding.issue_id,
            finding.severity_label,
            crate::workbench::audit_dashboard::confidence_label(finding.confidence),
            finding.host,
            finding.url
        );
        if let Some(param) = &finding.parameter {
            text.push_str(&format!("Parameter: {param}\n"));
        }
        text.push_str(&format!("\nSummary:\n{}\n", finding.summary));
        if !finding.evidence.is_empty() {
            text.push_str(&format!("\nEvidence:\n{}\n", finding.evidence));
        }
        if !finding.remediation.is_empty() {
            text.push_str(&format!("\nRemediation:\n{}\n", finding.remediation));
        }
        if self.push_text_to_comparer(&finding.title, text.into_bytes(), window, cx) {
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.focus.focus(window, cx);
            self.error = None;
            self.status = format!("已将问题「{}」发送到 Comparer。", finding.title);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn send_target_issue_to_mio_at(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            self.fail("所选漏洞告警已不存在。", cx);
            return;
        };
        let mut context = format!(
            "Target Issue Analysis\nTitle: {}\nIssue ID: {}\nSeverity: {}\nConfidence: {}\nURL: {}\n",
            finding.title,
            finding.issue_id,
            finding.severity_label,
            crate::workbench::audit_dashboard::confidence_label(finding.confidence),
            finding.url,
        );
        if let Some(param) = &finding.parameter {
            context.push_str(&format!("Parameter: {param}\n"));
        }
        context.push_str(&format!("Summary: {}\n", finding.summary));
        if !finding.evidence.is_empty() {
            context.push_str(&format!("Evidence: {}\n", finding.evidence));
        }
        if !finding.remediation.is_empty() {
            context.push_str(&format!("Remediation: {}\n", finding.remediation));
        }
        self.open_mio_ai_with_context(finding.title, context, cx);
    }

    // --- Contents row MioAT / CSRF PoC / Record Issue / Delete / Save ---

    pub(in crate::workbench) fn send_selected_target_to_mio_at(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        let req_bytes = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        let resp_bytes = self
            .selected_target_response_bytes(cx)
            .unwrap_or_default();
        let mut ctx = format!("Target Site map URL: {url}\n");
        if !req_bytes.is_empty() {
            ctx.push_str("\n--- Request ---\n");
            let req_text = String::from_utf8_lossy(&req_bytes);
            ctx.push_str(&req_text.chars().take(8000).collect::<String>());
        }
        if !resp_bytes.is_empty() {
            ctx.push_str("\n\n--- Response ---\n");
            let resp_text = String::from_utf8_lossy(&resp_bytes);
            ctx.push_str(&resp_text.chars().take(8000).collect::<String>());
        }
        let title = if url.is_empty() {
            "Site map 条目".to_owned()
        } else {
            url
        };
        self.open_mio_ai_with_context(title, ctx, cx);
    }

    pub(in crate::workbench) fn open_target_csrf_poc_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        let mut raw_request = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        if raw_request.is_empty() {
            let method = match row {
                HistoryRow::Live(s) => s.method.as_str(),
                HistoryRow::ExtProxy(a) => a
                    .edited_request
                    .as_ref()
                    .or(a.request.as_ref())
                    .and_then(|r| r.method.as_deref())
                    .unwrap_or("POST"),
            };
            raw_request = super::site_map_entries::synthetic_scan_draft(method, &url).raw;
        }
        self.open_csrf_poc_modal(&url, &raw_request, cx);
    }

    pub(in crate::workbench) fn record_manual_issue_from_target(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        if url.trim().is_empty() {
            self.fail("所选站点地图记录缺少有效的请求 URL。", cx);
            return;
        }
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_default();
        let row_id = row.id();
        let flow_id = match row_id {
            HistoryEntryId::Live(fid) if !self.target_is_draft_entry(row_id) => Some(fid),
            _ => None,
        };
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let finding = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00800100".into(),
            title: format!("自定义问题 · {url}"),
            severity_label: IssueSeverity::Information.label().into(),
            severity_rank: IssueSeverity::Information.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Passive,
            host,
            url: url.clone(),
            parameter: None,
            flow_id,
            summary: format!("从 Site map 创建的自定义问题（{url}）。"),
            evidence: url.clone(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "核查关联的 HTTP 请求与响应报文并补充修复建议。".into(),
            detected_unix_ms: now_unix_ms,
        };
        if self.enqueue(
            Command::Audit(AuditCommand::RecordManualFinding {
                project,
                finding: Box::new(finding),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已记录新问题：{url}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn attach_target_row_to_issue(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        if url.trim().is_empty() {
            self.fail("所选站点地图记录缺少有效的请求 URL。", cx);
            return;
        }
        let row_id = row.id();
        let flow_id = match row_id {
            HistoryEntryId::Live(fid) if !self.target_is_draft_entry(row_id) => Some(fid),
            _ => None,
        };
        if self.enqueue(
            Command::Audit(AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id,
                url: url.clone(),
                evidence: format!("Site map 关联流量：{url}"),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已将所选站点地图记录附加到已有问题：{url}");
            cx.notify();
        }
    }
}
