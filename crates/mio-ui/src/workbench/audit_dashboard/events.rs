use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_audit_event(
        &mut self,
        event: AuditEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(event.project()) {
            return;
        }
        match event {
            AuditEvent::State {
                query_revision,
                result,
                ..
            } => {
                if query_revision != 0 && query_revision != self.audit_dashboard.query_revision {
                    return;
                }
                self.audit_dashboard.loading = false;
                match result {
                    Ok(snapshot) => {
                        if !self.audit_dashboard.loaded
                            || snapshot.revision >= self.audit_dashboard.snapshot.revision
                        {
                            self.audit_dashboard.snapshot = *snapshot;
                        }
                        self.audit_dashboard.loaded = true;
                        self.audit_dashboard.state_error = None;
                        self.reconcile_audit_selection(window, cx);
                    }
                    Err(err) => {
                        self.audit_dashboard.state_error = Some(err.clone());
                        self.fail(err, cx);
                    }
                }
            }
            AuditEvent::TaskUpdated {
                revision,
                task,
                new_findings,
                ..
            } => {
                if revision < self.audit_dashboard.snapshot.revision {
                    return;
                }
                self.audit_dashboard.snapshot.revision = revision;
                self.audit_dashboard.loaded = true;
                let task_id = task.id;
                if task.state.is_terminal() {
                    self.audit_dashboard.running_cancels.remove(&task_id);
                }
                if let Some(existing) = self
                    .audit_dashboard
                    .snapshot
                    .tasks
                    .iter_mut()
                    .find(|item| item.id == task_id)
                {
                    *existing = task;
                } else {
                    self.audit_dashboard.snapshot.tasks.insert(0, task);
                }
                if !new_findings.is_empty() {
                    let entry = self
                        .audit_dashboard
                        .task_finding_ids
                        .entry(task_id)
                        .or_default();
                    for f in &new_findings {
                        entry.insert(f.id);
                    }
                }
                self.merge_audit_findings(new_findings);
                self.reconcile_audit_selection(window, cx);
            }
            AuditEvent::PassiveFindings {
                revision, findings, ..
            } => {
                if revision < self.audit_dashboard.snapshot.revision {
                    return;
                }
                self.audit_dashboard.snapshot.revision = revision;
                self.audit_dashboard.loaded = true;
                self.merge_audit_findings(findings);
                self.reconcile_audit_selection(window, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn merge_audit_findings(&mut self, incoming: Vec<AuditFinding>) {
        for finding in incoming {
            if let Some(existing) = self
                .audit_dashboard
                .snapshot
                .findings
                .iter_mut()
                .find(|f| f.id == finding.id)
            {
                *existing = finding;
            } else {
                self.audit_dashboard.snapshot.findings.insert(0, finding);
            }
        }
        if self.audit_dashboard.snapshot.findings.len() > mio_core::MAX_AUDIT_FINDINGS {
            self.audit_dashboard
                .snapshot
                .findings
                .truncate(mio_core::MAX_AUDIT_FINDINGS);
        }
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

    pub(in crate::workbench) fn pause_audit_scan(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.enqueue(
            Command::Audit(AuditCommand::PauseScan { project, task_id }),
            cx,
        ) {
            let mut paused_title = None;
            if let Some(task) = self
                .audit_dashboard
                .snapshot
                .tasks
                .iter_mut()
                .find(|t| t.id == task_id && t.state == AuditTaskState::Running)
            {
                task.state = AuditTaskState::Paused;
                paused_title = Some(task.title.clone());
            }
            if let Some(title) = paused_title {
                self.record_footer_event(
                    FooterLogSeverity::Info,
                    format!("已暂停扫描任务：{title}"),
                    format!("task_id={task_id}"),
                );
            }
            cx.notify();
        }
    }

    pub(in crate::workbench) fn resume_audit_scan(
        &mut self,
        task_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.enqueue(
            Command::Audit(AuditCommand::ResumeScan { project, task_id }),
            cx,
        ) {
            let mut resumed_title = None;
            if let Some(task) = self
                .audit_dashboard
                .snapshot
                .tasks
                .iter_mut()
                .find(|t| t.id == task_id && t.state == AuditTaskState::Paused)
            {
                task.state = AuditTaskState::Running;
                resumed_title = Some(task.title.clone());
            }
            if let Some(title) = resumed_title {
                self.record_footer_event(
                    FooterLogSeverity::Info,
                    format!("已恢复扫描任务：{title}"),
                    format!("task_id={task_id}"),
                );
            }
            cx.notify();
        }
    }

    pub(in crate::workbench) fn set_audit_finding_triage(
        &mut self,
        finding_id: Uuid,
        severity: IssueSeverity,
        confidence: IssueConfidence,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        self.ensure_original_triage_recorded(finding_id);
        if self.enqueue(
            Command::Audit(AuditCommand::SetFindingSeverity {
                project,
                finding_id,
                severity,
                confidence,
            }),
            cx,
        ) {
            if let Some(finding) = self
                .audit_dashboard
                .snapshot
                .findings
                .iter_mut()
                .find(|f| f.id == finding_id)
            {
                finding.severity_label = severity.label().to_owned();
                finding.severity_rank = severity.rank();
                finding.confidence = confidence;
            }
            cx.notify();
        }
    }

    pub(in crate::workbench) fn delete_audit_finding(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let removed_title = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.title.clone());
        if self.enqueue(
            Command::Audit(AuditCommand::DeleteFinding {
                project,
                finding_id,
            }),
            cx,
        ) {
            self.audit_dashboard
                .snapshot
                .findings
                .retain(|f| f.id != finding_id);
            self.audit_dashboard.original_triage.remove(&finding_id);
            self.audit_dashboard
                .false_positive_findings
                .remove(&finding_id);
            self.audit_dashboard
                .verified_confidence_findings
                .remove(&finding_id);
            self.audit_dashboard.finding_highlights.remove(&finding_id);
            self.audit_dashboard.finding_notes.remove(&finding_id);
            for ids in self.audit_dashboard.task_finding_ids.values_mut() {
                ids.remove(&finding_id);
            }
            if let Some(title) = removed_title {
                self.record_footer_event(
                    FooterLogSeverity::Info,
                    format!("已删除问题：{title}"),
                    format!("finding_id={finding_id}"),
                );
            }
            self.reconcile_audit_selection(window, cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn clear_audit_findings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.enqueue(Command::Audit(AuditCommand::ClearFindings { project }), cx) {
            self.audit_dashboard.snapshot.findings.clear();
            self.audit_dashboard.selected_finding_id = None;
            self.audit_dashboard.selected_finding_ids.clear();
            self.audit_dashboard.finding_selection_anchor = None;
            self.audit_dashboard.pending_batch_transfer = None;
            self.audit_dashboard.original_triage.clear();
            self.audit_dashboard.false_positive_findings.clear();
            self.audit_dashboard.verified_confidence_findings.clear();
            self.audit_dashboard.finding_highlights.clear();
            self.audit_dashboard.finding_notes.clear();
            self.audit_dashboard.editing_note_finding_id = None;
            self.audit_dashboard.task_finding_ids.clear();
            self.audit_dashboard.pending_flow_read = None;
            self.audit_dashboard.loaded_flow = None;
            self.audit_dashboard.flow_error = None;
            self.audit_dashboard
                .issue_request_editor
                .load(&[], window, cx);
            self.audit_dashboard
                .issue_response_editor
                .load(&[], window, cx);
            self.record_footer_event(
                FooterLogSeverity::Info,
                "已清空全部漏洞告警",
                "Dashboard 漏洞活动已清空",
            );
            cx.notify();
        }
    }
}
