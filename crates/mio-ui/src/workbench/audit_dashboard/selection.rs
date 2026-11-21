use super::*;

impl Workbench {
    pub(in crate::workbench) fn reconcile_audit_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(flow) = self.audit_dashboard.loaded_flow.as_deref() {
            self.audit_dashboard
                .flow_cache
                .insert(flow.summary.id, flow.clone());
        }
        let valid_ids: HashSet<Uuid> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| f.id)
            .collect();
        self.audit_dashboard
            .selected_finding_ids
            .retain(|id| valid_ids.contains(id));
        if self
            .audit_dashboard
            .finding_selection_anchor
            .is_some_and(|anchor| !valid_ids.contains(&anchor))
        {
            self.audit_dashboard.finding_selection_anchor = None;
        }
        if let Some(selected_id) = self.audit_dashboard.selected_finding_id
            && !valid_ids.contains(&selected_id)
        {
            self.audit_dashboard.selected_finding_id = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .find(|f| self.audit_dashboard.selected_finding_ids.contains(&f.id))
                .map(|f| f.id);
            self.audit_dashboard.pending_flow_read = None;
            self.audit_dashboard.loaded_flow = None;
            self.audit_dashboard.flow_error = None;
            self.audit_dashboard
                .issue_request_editor
                .load(&[], window, cx);
            self.audit_dashboard
                .issue_response_editor
                .load(&[], window, cx);
            if let Some(next_id) = self.audit_dashboard.selected_finding_id {
                self.load_audit_finding_detail_only(next_id, false, window, cx);
            }
        }
        if let Some(editing_id) = self.audit_dashboard.editing_note_finding_id
            && !valid_ids.contains(&editing_id)
        {
            self.audit_dashboard.editing_note_finding_id = None;
        }
    }

    pub(in crate::workbench) fn load_audit_finding_detail_only(
        &mut self,
        finding_id: Uuid,
        switch_to_issues_tab: bool,
        window: &mut Window,
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
        self.clear_dashboard_intruder_selection(window, cx);
        if self.audit_dashboard.selected_audit_task_id.is_none() {
            self.audit_dashboard.selected_builtin_task = DashboardBuiltinTask::LivePassiveAudit;
        }
        if switch_to_issues_tab {
            self.audit_dashboard.active_task_tab = DashboardTaskTab::Issues;
        }
        if let Some(prev_flow) = self.audit_dashboard.loaded_flow.take() {
            self.audit_dashboard
                .flow_cache
                .insert(prev_flow.summary.id, *prev_flow);
        }
        self.audit_dashboard.selected_finding_id = Some(finding_id);
        self.audit_dashboard.pending_flow_read = None;
        self.audit_dashboard.flow_error = None;
        self.audit_dashboard
            .issue_request_editor
            .load(&[], window, cx);
        self.audit_dashboard
            .issue_response_editor
            .load(&[], window, cx);
        if let Some(flow_id) = finding.flow_id
            && let Some(project) = self.project
            && self.backend_available()
            && self.transition.is_none()
        {
            if let Some(cached) = self.audit_dashboard.flow_cache.get(&flow_id).cloned() {
                let req_raw = request_to_raw(&cached.request);
                let resp_raw = cached
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
                self.audit_dashboard.loaded_flow = Some(Box::new(cached));
                self.apply_selected_finding_evidence_highlight(&req_raw, &resp_raw, window, cx);
                cx.notify();
                return;
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
                self.audit_dashboard.pending_flow_read = Some((project, revision, flow_id));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_audit_finding_in_place(
        &mut self,
        finding_id: Uuid,
        switch_to_issues_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .any(|f| f.id == finding_id)
        {
            return;
        }
        self.audit_dashboard.selected_finding_ids.clear();
        self.audit_dashboard.selected_finding_ids.insert(finding_id);
        self.audit_dashboard.finding_selection_anchor = Some(finding_id);
        self.load_audit_finding_detail_only(finding_id, switch_to_issues_tab, window, cx);
    }

    pub(in crate::workbench) fn handle_audit_issue_row_click(
        &mut self,
        row_members: &[Uuid],
        visible_order: &[Uuid],
        shift: bool,
        toggle: bool,
        switch_to_issues_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&lead_id) = row_members.first() else {
            return;
        };
        if self.audit_dashboard.selected_finding_id.is_none() {
            self.audit_dashboard.selected_finding_ids.clear();
            self.audit_dashboard.finding_selection_anchor = None;
        }
        let valid_ids: HashSet<Uuid> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| f.id)
            .collect();
        self.audit_dashboard
            .selected_finding_ids
            .retain(|id| valid_ids.contains(id));
        if self.audit_dashboard.selected_finding_ids.is_empty()
            && let Some(cur) = self
                .audit_dashboard
                .selected_finding_id
                .filter(|id| valid_ids.contains(id))
        {
            self.audit_dashboard.selected_finding_ids.insert(cur);
        }

        if shift {
            let anchor = self
                .audit_dashboard
                .finding_selection_anchor
                .filter(|id| valid_ids.contains(id))
                .or_else(|| {
                    self.audit_dashboard
                        .selected_finding_id
                        .filter(|id| valid_ids.contains(id))
                })
                .unwrap_or(lead_id);
            let start_pos = visible_order.iter().position(|id| *id == anchor);
            let end_pos = row_members
                .iter()
                .filter_map(|id| visible_order.iter().position(|v| v == id))
                .next()
                .or_else(|| visible_order.iter().position(|id| *id == lead_id));
            if let (Some(s), Some(e)) = (start_pos, end_pos) {
                let lo = s.min(e);
                let hi = row_members
                    .iter()
                    .filter_map(|id| visible_order.iter().position(|v| v == id))
                    .max()
                    .unwrap_or(e)
                    .max(s.max(e));
                self.audit_dashboard.selected_finding_ids =
                    visible_order[lo..=hi].iter().copied().collect();
            } else {
                self.audit_dashboard.selected_finding_ids =
                    row_members.iter().copied().collect();
            }
            if self.audit_dashboard.finding_selection_anchor.is_none() {
                self.audit_dashboard.finding_selection_anchor = Some(anchor);
            }
            self.load_audit_finding_detail_only(lead_id, switch_to_issues_tab, window, cx);
            return;
        }

        if toggle {
            let all_selected = row_members
                .iter()
                .all(|id| self.audit_dashboard.selected_finding_ids.contains(id));
            if all_selected {
                for id in row_members {
                    self.audit_dashboard.selected_finding_ids.remove(id);
                }
                if self.audit_dashboard.selected_finding_ids.is_empty() {
                    self.audit_dashboard.selected_finding_id = None;
                    self.audit_dashboard.finding_selection_anchor = None;
                    self.audit_dashboard.pending_flow_read = None;
                    self.audit_dashboard.loaded_flow = None;
                    self.audit_dashboard.flow_error = None;
                    self.audit_dashboard
                        .issue_request_editor
                        .load(&[], window, cx);
                    self.audit_dashboard
                        .issue_response_editor
                        .load(&[], window, cx);
                    cx.notify();
                } else if self
                    .audit_dashboard
                    .selected_finding_id
                    .is_none_or(|cur| !self.audit_dashboard.selected_finding_ids.contains(&cur))
                {
                    let next_lead = visible_order
                        .iter()
                        .copied()
                        .find(|id| self.audit_dashboard.selected_finding_ids.contains(id))
                        .or_else(|| self.audit_dashboard.selected_finding_ids.iter().copied().next());
                    if let Some(next_id) = next_lead {
                        self.load_audit_finding_detail_only(
                            next_id,
                            switch_to_issues_tab,
                            window,
                            cx,
                        );
                    } else {
                        cx.notify();
                    }
                } else {
                    cx.notify();
                }
            } else {
                for &id in row_members {
                    self.audit_dashboard.selected_finding_ids.insert(id);
                }
                self.audit_dashboard.finding_selection_anchor = Some(lead_id);
                self.load_audit_finding_detail_only(lead_id, switch_to_issues_tab, window, cx);
            }
            return;
        }

        self.audit_dashboard.selected_finding_ids = row_members.iter().copied().collect();
        self.audit_dashboard.finding_selection_anchor = Some(lead_id);
        self.load_audit_finding_detail_only(lead_id, switch_to_issues_tab, window, cx);
    }

    pub(in crate::workbench) fn handle_audit_issue_row_right_down(
        &mut self,
        row_members: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&lead_id) = row_members.first() else {
            return;
        };
        if self.audit_dashboard.selected_finding_id.is_none() {
            self.audit_dashboard.selected_finding_ids.clear();
            self.audit_dashboard.finding_selection_anchor = None;
        }
        let any_in_selection = row_members
            .iter()
            .any(|id| self.audit_dashboard.selected_finding_ids.contains(id))
            || self.audit_dashboard.selected_finding_id == Some(lead_id);
        if any_in_selection && self.audit_dashboard.selected_finding_ids.len() > 1 {
            for &id in row_members {
                self.audit_dashboard.selected_finding_ids.insert(id);
            }
            cx.notify();
            return;
        }
        self.audit_dashboard.selected_finding_ids = row_members.iter().copied().collect();
        self.audit_dashboard.finding_selection_anchor = Some(lead_id);
        if self.audit_dashboard.selected_finding_id != Some(lead_id) {
            self.load_audit_finding_detail_only(lead_id, false, window, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::workbench) fn select_audit_finding(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_audit_finding_in_place(finding_id, true, window, cx);
    }

    pub(in crate::workbench) fn audit_selected_target_ids(&self, fallback_id: Uuid) -> Vec<Uuid> {
        if self
            .audit_dashboard
            .selected_finding_ids
            .contains(&fallback_id)
            && self.audit_dashboard.selected_finding_ids.len() > 1
        {
            let ordered: Vec<Uuid> = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .map(|f| f.id)
                .filter(|id| self.audit_dashboard.selected_finding_ids.contains(id))
                .collect();
            if !ordered.is_empty() {
                return ordered;
            }
        }
        vec![fallback_id]
    }

    pub(in crate::workbench) fn toggle_audit_issue_sort(
        &mut self,
        col: DashboardIssueSortColumn,
        cx: &mut Context<Self>,
    ) {
        self.audit_dashboard.issue_sort = match self.audit_dashboard.issue_sort {
            Some((cur, asc)) if cur == col => Some((col, !asc)),
            _ => Some((col, true)),
        };
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_issue_sort(
        &mut self,
        col: DashboardIssueSortColumn,
        asc: bool,
        cx: &mut Context<Self>,
    ) {
        self.audit_dashboard.issue_sort = Some((col, asc));
        cx.notify();
    }

    pub(in crate::workbench) fn audit_issue_cell_text(
        &self,
        col_ix: usize,
        finding: &AuditFinding,
    ) -> String {
        let is_fp = self
            .audit_dashboard
            .false_positive_findings
            .contains(&finding.id);
        let is_verified = self
            .audit_dashboard
            .verified_confidence_findings
            .contains(&finding.id);
        match col_ix {
            0 => severity_display_style(finding.severity(), is_fp).0.to_owned(),
            1 => finding.title.clone(),
            2 => match &finding.parameter {
                Some(param) if !param.is_empty() => format!("{} ({param})", finding.host),
                _ => finding.host.clone(),
            },
            3 => finding.url.clone(),
            4 => confidence_display_label(finding.confidence, is_verified).to_owned(),
            _ => String::new(),
        }
    }

    pub(in crate::workbench) fn copy_audit_issue_column(
        &mut self,
        col_ix: usize,
        finding_ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        let by_id: HashMap<Uuid, &AuditFinding> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| (f.id, f))
            .collect();
        let mut lines = Vec::with_capacity(finding_ids.len());
        for &id in finding_ids {
            if let Some(finding) = by_id.get(&id) {
                lines.push(self.audit_issue_cell_text(col_ix, finding));
            }
        }
        let count = lines.len();
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!(
            "已复制「{}」列（{count} 行）到剪贴板。",
            DASHBOARD_ISSUE_COL_NAMES.get(col_ix).copied().unwrap_or("Issues")
        );
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_audit_issue_column_visibility(
        &mut self,
        col_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let visible_count = self
            .audit_dashboard
            .issue_col_visible
            .iter()
            .filter(|&&v| v)
            .count();
        if self.audit_dashboard.issue_col_visible[col_ix] {
            if visible_count > 1 {
                self.audit_dashboard.issue_col_visible[col_ix] = false;
            }
        } else {
            self.audit_dashboard.issue_col_visible[col_ix] = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn fit_audit_issue_column_width(
        &mut self,
        col_ix: usize,
        finding_ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest = measure(DASHBOARD_ISSUE_COL_NAMES[col_ix]) + 22.0;
        let by_id: HashMap<Uuid, &AuditFinding> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| (f.id, f))
            .collect();
        for &id in finding_ids.iter().take(250) {
            if let Some(finding) = by_id.get(&id) {
                let text = self.audit_issue_cell_text(col_ix, finding);
                let preview = theme::truncate_cell_preview(&text);
                widest = widest.max(measure(&preview));
            }
        }
        let fitted = (widest + 22.0).round().clamp(56.0, 560.0);
        self.audit_dashboard.issue_col_widths[col_ix] = fitted;
        cx.notify();
    }

    pub(in crate::workbench) fn reset_audit_issue_column_layout(&mut self, cx: &mut Context<Self>) {
        self.audit_dashboard.issue_col_widths = DASHBOARD_ISSUE_DEFAULT_WIDTHS;
        self.audit_dashboard.issue_col_visible = [true; DASHBOARD_ISSUE_COL_COUNT];
        self.audit_dashboard.issue_sort = None;
        self.audit_dashboard_view
            .update(cx, |view, cx| view.clear_issue_column_resize(cx));
        cx.notify();
    }

    pub(in crate::workbench) fn switch_audit_editor_pretty(
        &mut self,
        which: AuditEditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = match which {
            AuditEditorSlot::ScanRequest => &mut self.audit_dashboard.scan_request_editor,
            AuditEditorSlot::IssueRequest => &mut self.audit_dashboard.issue_request_editor,
            AuditEditorSlot::IssueResponse => &mut self.audit_dashboard.issue_response_editor,
        };
        if let Err(err) = editor.switch_pretty(window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_audit_editor_view(
        &mut self,
        which: AuditEditorSlot,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = match which {
            AuditEditorSlot::ScanRequest => &mut self.audit_dashboard.scan_request_editor,
            AuditEditorSlot::IssueRequest => &mut self.audit_dashboard.issue_request_editor,
            AuditEditorSlot::IssueResponse => &mut self.audit_dashboard.issue_response_editor,
        };
        if let Err(err) = editor.switch(view, window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn handle_audit_search_action(
        &mut self,
        which: AuditEditorSlot,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = match which {
            AuditEditorSlot::ScanRequest => &mut self.audit_dashboard.scan_request_editor,
            AuditEditorSlot::IssueRequest => &mut self.audit_dashboard.issue_request_editor,
            AuditEditorSlot::IssueResponse => &mut self.audit_dashboard.issue_response_editor,
        };
        if let Err(err) = editor.handle_search_bar_action(action, window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

}
