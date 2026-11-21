use super::*;

impl Workbench {
    pub(in crate::workbench) fn delete_audit_findings_batch(
        &mut self,
        ids: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.delete_audit_finding(id, window, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let mut remove_set = HashSet::new();
        for &finding_id in ids {
            if self.enqueue(
                Command::Audit(AuditCommand::DeleteFinding {
                    project,
                    finding_id,
                }),
                cx,
            ) {
                remove_set.insert(finding_id);
            } else {
                break;
            }
        }
        if remove_set.is_empty() {
            return;
        }
        let removed_count = remove_set.len();
        self.audit_dashboard
            .snapshot
            .findings
            .retain(|f| !remove_set.contains(&f.id));
        for finding_id in &remove_set {
            self.audit_dashboard.original_triage.remove(finding_id);
            self.audit_dashboard
                .false_positive_findings
                .remove(finding_id);
            self.audit_dashboard
                .verified_confidence_findings
                .remove(finding_id);
            self.audit_dashboard.finding_highlights.remove(finding_id);
            self.audit_dashboard.finding_notes.remove(finding_id);
            self.audit_dashboard.selected_finding_ids.remove(finding_id);
            for task_ids in self.audit_dashboard.task_finding_ids.values_mut() {
                task_ids.remove(finding_id);
            }
        }
        self.record_footer_event(
            FooterLogSeverity::Info,
            format!("已批量删除 {removed_count} 条问题"),
            "Dashboard 漏洞活动批量移除",
        );
        self.reconcile_audit_selection(window, cx);
        self.error = None;
        self.status = format!("已批量删除 {removed_count} 条问题。");
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_findings_severity_batch(
        &mut self,
        ids: &[Uuid],
        severity: IssueSeverity,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.set_audit_finding_severity_override(id, severity, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        for &id in ids {
            self.set_audit_finding_severity_override(id, severity, cx);
        }
        self.status = format!(
            "已批量将 {} 条问题严重性设为 {}。",
            ids.len(),
            severity.bilingual_label()
        );
        cx.notify();
    }

    pub(in crate::workbench) fn mark_audit_findings_false_positive_batch(
        &mut self,
        ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.mark_audit_finding_false_positive(id, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        for &id in ids {
            self.mark_audit_finding_false_positive(id, cx);
        }
        self.status = format!("已批量将 {} 条问题标记为误报 (False positive)。", ids.len());
        cx.notify();
    }

    pub(in crate::workbench) fn restore_audit_findings_severity_batch(
        &mut self,
        ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.restore_audit_finding_severity(id, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        for &id in ids {
            self.restore_audit_finding_severity(id, cx);
        }
        self.status = format!("已批量恢复 {} 条问题的原始严重性。", ids.len());
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_findings_confidence_batch(
        &mut self,
        ids: &[Uuid],
        confidence: IssueConfidence,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.set_audit_finding_confidence_override(id, confidence, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        for &id in ids {
            self.set_audit_finding_confidence_override(id, confidence, cx);
        }
        self.status = format!(
            "已批量将 {} 条问题置信度设为 {}。",
            ids.len(),
            confidence_label(confidence)
        );
        cx.notify();
    }

    pub(in crate::workbench) fn mark_audit_findings_verified_batch(
        &mut self,
        ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.mark_audit_finding_verified(id, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        for &id in ids {
            self.mark_audit_finding_verified(id, cx);
        }
        self.status = format!("已批量将 {} 条问题置信度设为已验证 (Verified)。", ids.len());
        cx.notify();
    }

    pub(in crate::workbench) fn restore_audit_findings_confidence_batch(
        &mut self,
        ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.restore_audit_finding_confidence(id, cx);
            }
            return;
        }
        if !self.audit_can_operate() {
            return;
        }
        for &id in ids {
            self.restore_audit_finding_confidence(id, cx);
        }
        self.status = format!("已批量恢复 {} 条问题的原始置信度。", ids.len());
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_findings_highlight_batch(
        &mut self,
        ids: &[Uuid],
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.set_audit_finding_highlight(id, color, cx);
            }
            return;
        }
        if color > 9 {
            return;
        }
        for &id in ids {
            if color == 0 {
                self.audit_dashboard.finding_highlights.remove(&id);
            } else {
                self.audit_dashboard.finding_highlights.insert(id, color);
            }
        }
        self.status = if color == 0 {
            format!("已批量清除 {} 条问题高亮。", ids.len())
        } else {
            format!(
                "已批量设置 {} 条问题高亮：{}。",
                ids.len(),
                ISSUE_HIGHLIGHT_LABELS[usize::from(color)]
            )
        };
        cx.notify();
    }

    pub(in crate::workbench) fn copy_audit_findings_urls_batch(
        &mut self,
        ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
        let urls: Vec<String> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .filter(|f| id_set.contains(&f.id) && !f.url.is_empty())
            .map(|f| f.url.clone())
            .collect();
        if urls.is_empty() {
            self.fail("所选问题没有可复制的 URL。", cx);
            return;
        }
        let count = urls.len();
        cx.write_to_clipboard(ClipboardItem::new_string(urls.join("\n")));
        self.error = None;
        self.status = format!("已复制 {count} 条问题 URL 到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn send_audit_findings_to_comparer_batch(
        &mut self,
        ids: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            if let Some(&id) = ids.first() {
                self.send_audit_finding_to_comparer(id, window, cx);
            }
            return;
        }
        if self.transition.is_some() {
            return;
        }
        const MAX_COMPARER_ITEMS: usize = 32;
        const MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;
        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
        let findings: Vec<AuditFinding> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .filter(|f| id_set.contains(&f.id))
            .cloned()
            .collect();
        let mut added = 0usize;
        for finding in findings {
            let flow = finding.flow_id.and_then(|fid| {
                self.audit_dashboard
                    .loaded_flow
                    .as_deref()
                    .filter(|r| r.summary.id == fid)
                    .or_else(|| self.audit_dashboard.flow_cache.get(&fid))
            });
            let mut payloads: Vec<(String, Vec<u8>)> = Vec::new();
            if let Some(flow) = flow {
                let req_raw = request_to_raw(&flow.request);
                if !req_raw.is_empty() {
                    payloads.push((format!("{} (请求)", finding.title), req_raw));
                }
                if let Some(resp) = &flow.response {
                    let resp_raw = response_to_raw(resp);
                    if !resp_raw.is_empty() {
                        payloads.push((format!("{} (响应)", finding.title), resp_raw));
                    }
                }
            }
            if payloads.is_empty() {
                let text = format_finding_context_text(
                    &finding,
                    self.audit_dashboard
                        .false_positive_findings
                        .contains(&finding.id),
                    self.audit_dashboard
                        .verified_confidence_findings
                        .contains(&finding.id),
                    self.audit_dashboard
                        .finding_notes
                        .get(&finding.id)
                        .map(String::as_str),
                    None,
                );
                payloads.push((finding.title.clone(), text.into_bytes()));
            }
            for (label, bytes) in payloads {
                if bytes.len() > MAX_COMPARER_ITEM_BYTES {
                    continue;
                }
                if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                    break;
                }
                let id = self
                    .extra_tools
                    .comparer_items
                    .last()
                    .map_or(1, |item| item.id + 1);
                self.extra_tools
                    .comparer_items
                    .push(crate::workbench::tools_extra::ComparerItem {
                        id,
                        label: format!("{id}: {label} ({} bytes)", bytes.len()),
                        bytes,
                    });
                added += 1;
            }
            if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                break;
            }
        }
        if added > 0 {
            let count = self.extra_tools.comparer_items.len();
            self.extra_tools.comparer_selected = count - 1;
            if count >= 2 {
                self.extra_tools.left_index = count - 2;
                self.extra_tools.right_index = count - 1;
            }
            self.refresh_comparer(window, cx);
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.status = format!("已批量发送 {added} 项问题报文到 Comparer。");
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn start_audit_findings_batch_transfer(
        &mut self,
        ids: &[Uuid],
        kind: DashboardBatchTransferKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        if self.audit_dashboard.pending_batch_transfer.is_some() {
            self.fail("正在读取漏洞关联流量；请稍后重试。", cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if let Some(loaded) = self.audit_dashboard.loaded_flow.as_deref() {
            self.audit_dashboard
                .flow_cache
                .insert(loaded.summary.id, loaded.clone());
        }
        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
        let mut seen_flows = HashSet::new();
        let mut flow_ids = Vec::new();
        for finding in &self.audit_dashboard.snapshot.findings {
            if id_set.contains(&finding.id)
                && let Some(fid) = finding.flow_id
                && seen_flows.insert(fid)
            {
                flow_ids.push(fid);
            }
        }
        if flow_ids.is_empty() {
            self.fail("所选问题没有关联可转送的 HTTP 流量记录。", cx);
            return;
        }
        let mut pending_revisions = HashMap::new();
        for &flow_id in &flow_ids {
            if self.audit_dashboard.flow_cache.contains_key(&flow_id) {
                continue;
            }
            self.selection_revision += 1;
            let revision = self.selection_revision;
            if self.enqueue(
                Command::ReadFlow {
                    project,
                    selection_revision: revision,
                    flow_id,
                },
                cx,
            ) {
                pending_revisions.insert(revision, flow_id);
            }
        }
        let batch = PendingIssueBatchTransfer {
            kind,
            flow_ids,
            pending_revisions,
        };
        if batch.pending_revisions.is_empty() {
            self.complete_audit_batch_transfer(batch, window, cx);
        } else {
            self.audit_dashboard.pending_batch_transfer = Some(batch);
            self.status = "正在读取问题关联流量以便转送…".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn complete_audit_batch_transfer(
        &mut self,
        batch: PendingIssueBatchTransfer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let records: Vec<FlowRecord> = batch
            .flow_ids
            .iter()
            .filter_map(|id| self.audit_dashboard.flow_cache.get(id).cloned())
            .collect();
        if records.is_empty() {
            self.fail("未获取到可转送的问题关联流量。", cx);
            return;
        }
        match batch.kind {
            DashboardBatchTransferKind::Repeater | DashboardBatchTransferKind::Intruder => {
                let entries: Vec<mio_core::HistoryTransferEntry> = records
                    .into_iter()
                    .filter(|r| r.summary.revision > 0)
                    .map(|r| mio_core::HistoryTransferEntry::Live {
                        flow_id: r.summary.id,
                        revision: r.summary.revision,
                        original: false,
                    })
                    .collect();
                if entries.is_empty() {
                    self.fail("所选问题关联的流量尚未落盘，无法转送。", cx);
                    return;
                }
                self.begin_history_transfer(
                    None,
                    entries,
                    batch.kind == DashboardBatchTransferKind::Intruder,
                    window,
                    cx,
                );
            }
            DashboardBatchTransferKind::Organiser => {
                if !self.organiser_editable() {
                    return;
                }
                if let Err(err) = self.validate_organiser_composition(window, cx) {
                    self.fail(err, cx);
                    return;
                }
                if !self.flush_workspace(window, cx) {
                    return;
                }
                let mut added = 0usize;
                let mut first_id = None;
                for record in &records {
                    let number = mio_core::next_organiser_number(&self.workspace.organiser_items);
                    let Ok(item) =
                        mio_core::OrganiserItem::from_flow_record(number, "Scanner", record)
                    else {
                        continue;
                    };
                    let item_id = item.id;
                    self.workspace.organiser_items.push(item);
                    if mio_core::validate_organiser_items(&self.workspace.organiser_items)
                        .and_then(|()| self.workspace.validate_structure())
                        .is_err()
                    {
                        self.workspace.organiser_items.pop();
                        break;
                    }
                    first_id.get_or_insert(item_id);
                    added += 1;
                }
                if added == 0 {
                    self.fail("未能将所选问题关联流量添加到 Organiser。", cx);
                    return;
                }
                let had_sel = self.organiser.selected_id.is_some();
                if !had_sel {
                    self.organiser.selected_id = first_id;
                }
                self.sync_organiser_table(window, cx);
                if !had_sel {
                    self.reload_selected_organiser_detail(window, cx);
                }
                self.save_workspace(cx);
                self.error = None;
                self.status = format!("已批量添加 {added} 条问题关联流量到 Organiser。");
                cx.notify();
            }
        }
    }
}
