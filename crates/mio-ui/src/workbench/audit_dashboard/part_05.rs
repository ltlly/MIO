use super::*;

impl Workbench {
    pub(in crate::workbench) fn activate_pro_dashboard(&mut self, cx: &mut Context<Self>) {
        self.activate_intruder_dashboard(cx);
    }

    pub(in crate::workbench) fn query_audit_state(
        &mut self,
        project: ProjectKey,
        cx: &mut Context<Self>,
    ) {
        let Some(revision) = self.audit_dashboard.query_revision.checked_add(1) else {
            self.fail("审计状态查询 revision 已耗尽；请重新打开项目。", cx);
            return;
        };
        self.audit_dashboard.query_revision = revision;
        if self.enqueue(
            Command::Audit(AuditCommand::QueryState {
                project,
                query_revision: revision,
            }),
            cx,
        ) {
            self.audit_dashboard.loading = true;
            self.audit_dashboard.state_error = None;
        }
    }

    pub(in crate::workbench) fn reset_audit_dashboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = &mut self.audit_dashboard;
        for (_, (_, cancel)) in pane.running_cancels.drain() {
            cancel.cancel();
        }
        pane.loaded = false;
        pane.loading = false;
        pane.query_revision = pane.query_revision.saturating_add(1);
        pane.snapshot = AuditStateSnapshot::default();
        pane.state_error = None;
        pane.new_scan_open = false;
        pane.new_live_task_open = false;
        pane.live_task_draft_passive_audit = true;
        pane.live_task_draft_passive_crawl = true;
        pane.live_task_draft_scope = LiveTaskTrafficScope::AllTraffic;
        pane.live_task_scope = LiveTaskTrafficScope::AllTraffic;
        pane.scan_version = HttpVersion::Http11;
        pane.scan_mode = AuditTaskMode::PassiveAndActive;
        pane.crawl_same_origin = false;
        pane.scan_error = None;
        pane.selected_audit_task_id = None;
        pane.selected_builtin_task = DashboardBuiltinTask::LivePassiveAudit;
        pane.active_task_tab = DashboardTaskTab::Summary;
        pane.task_filter = DashboardTaskFilter::All;
        pane.summary_sort_asc = false;
        pane.severity_filter = None;
        pane.selected_finding_id = None;
        pane.selected_finding_ids.clear();
        pane.finding_selection_anchor = None;
        pane.issue_sort = None;
        pane.issue_col_resize_origin = None;
        pane.pending_batch_transfer = None;
        pane.original_triage.clear();
        pane.false_positive_findings.clear();
        pane.verified_confidence_findings.clear();
        pane.finding_highlights.clear();
        pane.finding_notes.clear();
        pane.editing_note_finding_id = None;
        pane.expanded_issue_groups.clear();
        pane.task_finding_ids.clear();
        pane.dismissed_audit_items.clear();
        pane.pending_flow_read = None;
        pane.loaded_flow = None;
        pane.flow_cache.clear();
        pane.flow_error = None;
        pane.detail_tab = IssueDetailTab::Advisory;
        pane.scan_title_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        pane.scan_url_input.update(cx, |state, cx| {
            state.set_value(DEFAULT_SCAN_URL, window, cx)
        });
        pane.task_search_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        pane.issue_filter_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        pane.finding_note_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        pane.scan_request_editor.load(DEFAULT_SCAN_RAW, window, cx);
        pane.issue_request_editor.load(&[], window, cx);
        pane.issue_response_editor.load(&[], window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn audit_backend_disconnected(&mut self, cx: &mut Context<Self>) {
        let pane = &mut self.audit_dashboard;
        for (_, (_, cancel)) in pane.running_cancels.drain() {
            cancel.cancel();
        }
        pane.loading = false;
        pane.pending_flow_read = None;
        pane.pending_batch_transfer = None;
        cx.notify();
    }

    pub(in crate::workbench) fn validate_audit_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        self.audit_dashboard.validate_scan_composition(window, cx)?;
        for input in [
            &self.audit_dashboard.task_search_input,
            &self.audit_dashboard.issue_filter_input,
            &self.audit_dashboard.finding_note_input,
            &self.audit_dashboard.issue_request_editor.search,
            &self.audit_dashboard.issue_response_editor.search,
        ] {
            editing::input_snapshot(input, window, cx)?;
        }
        Ok(())
    }

    pub(in crate::workbench) fn focus_active_audit_message_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Dashboard || self.audit_dashboard.has_non_editor_focus(window, cx) {
            return;
        }
        let result = if self.audit_dashboard.new_scan_open
            && self
                .audit_dashboard
                .scan_request_editor
                .has_focus(window, cx)
        {
            Some(
                self.audit_dashboard
                    .scan_request_editor
                    .focus_search(window, cx),
            )
        } else if !self.has_dashboard_intruder_selection()
            && self.audit_dashboard.selected_finding_id.is_some()
        {
            match self.audit_dashboard.detail_tab {
                IssueDetailTab::Request => Some(
                    self.audit_dashboard
                        .issue_request_editor
                        .focus_search(window, cx),
                ),
                IssueDetailTab::Response => Some(
                    self.audit_dashboard
                        .issue_response_editor
                        .focus_search(window, cx),
                ),
                IssueDetailTab::Advisory if self.audit_dashboard.new_scan_open => Some(
                    self.audit_dashboard
                        .scan_request_editor
                        .focus_search(window, cx),
                ),
                IssueDetailTab::Advisory => None,
            }
        } else if self.audit_dashboard.new_scan_open {
            Some(
                self.audit_dashboard
                    .scan_request_editor
                    .focus_search(window, cx),
            )
        } else {
            None
        };
        if let Some(Err(err)) = result {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn audit_can_operate(&self) -> bool {
        self.project.is_some() && self.backend_available() && self.transition.is_none()
    }

    pub(in crate::workbench) fn send_draft_to_scanner(
        &mut self,
        draft: Draft,
        mode: AuditTaskMode,
        start_immediately: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            self.fail("正在切换或保存项目；未转送请求到主动/被动扫描。", cx);
            return;
        }
        if let Err(err) = self.audit_dashboard.validate_scan_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let version = match draft.version {
            HttpVersion::Http2 => HttpVersion::Http2,
            HttpVersion::Http10 | HttpVersion::Http11 => HttpVersion::Http11,
        };
        let target = if draft.target.trim().is_empty() {
            DEFAULT_SCAN_URL.to_owned()
        } else {
            draft.target.clone()
        };
        self.audit_dashboard.scan_version = version;
        self.audit_dashboard.scan_mode = mode;
        self.audit_dashboard.crawl_same_origin = false;
        self.audit_dashboard.scan_error = None;
        self.audit_dashboard
            .scan_title_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.audit_dashboard
            .scan_url_input
            .update(cx, |state, cx| state.set_value(target.clone(), window, cx));
        self.audit_dashboard
            .scan_request_editor
            .load(&draft.raw, window, cx);
        self.cancel_plugin_interactions(cx);
        self.clear_dashboard_intruder_selection(window, cx);
        self.tool = Tool::Dashboard;
        self.activate_pro_dashboard(cx);
        if start_immediately {
            self.audit_dashboard.new_scan_open = false;
            let normalized = Draft {
                revision: draft.revision.max(1),
                target: target.clone(),
                raw: draft.raw,
                version,
            };
            let title = format!("扫描 · {target}");
            self.dispatch_audit_scan(title, normalized, mode, cx);
        } else {
            self.audit_dashboard.new_scan_open = true;
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn snapshot_new_scan_draft(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(String, Draft, AuditTaskMode), String> {
        let pane = &self.audit_dashboard;
        let title_raw = editing::input_snapshot(&pane.scan_title_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let target = editing::input_snapshot(&pane.scan_url_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let _ = editing::input_snapshot(&pane.scan_request_editor.search, window, cx)
            .map_err(str::to_owned)?;
        let raw = pane.scan_request_editor.bytes(window, cx)?;

        if target.is_empty() {
            return Err("请输入扫描目标 URL（http:// 或 https://）。".into());
        }
        if target.len() > MAX_SCAN_URL_BYTES {
            return Err("扫描目标 URL 不能超过 4096 字节；输入已保留。".into());
        }
        if title_raw.len() > MAX_SCAN_TITLE_BYTES {
            return Err("扫描标题不能超过 256 字节；输入已保留。".into());
        }
        let raw = if raw == DEFAULT_SCAN_RAW && target != DEFAULT_SCAN_URL {
            let without_frag = target.split('#').next().unwrap_or(&target);
            if let Some((_, rest)) = without_frag.split_once("://")
                && !rest.is_empty()
            {
                let (authority, path_and_query) = match rest.split_once('/') {
                    Some((auth, pq)) => (auth, format!("/{pq}")),
                    None => {
                        if let Some((auth, q)) = rest.split_once('?') {
                            (auth, format!("/?{q}"))
                        } else {
                            (rest, "/".to_owned())
                        }
                    }
                };
                format!("GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\n\r\n").into_bytes()
            } else {
                raw
            }
        } else {
            raw
        };
        parse_repeater_request(
            &raw,
            &target,
            pane.scan_version,
            self.workspace.repeater_settings.update_content_length,
        )
        .map_err(|err| format!("扫描种子 HTTP 请求无效：{err}"))?;

        let title = if title_raw.is_empty() {
            format!("{} · {target}", pane.scan_mode.label())
        } else {
            title_raw
        };
        Ok((
            title,
            Draft {
                revision: 1,
                target,
                raw,
                version: pane.scan_version,
            },
            pane.scan_mode,
        ))
    }

    pub(in crate::workbench) fn submit_new_audit_scan(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        match self.snapshot_new_scan_draft(window, cx) {
            Ok((title, draft, mode)) => {
                self.audit_dashboard.scan_error = None;
                if self.dispatch_audit_scan(title, draft, mode, cx) {
                    self.audit_dashboard.new_scan_open = false;
                    self.clear_dashboard_intruder_selection(window, cx);
                }
            }
            Err(err) => {
                self.audit_dashboard.scan_error = Some(err.clone());
                self.fail(err, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn dispatch_audit_scan(
        &mut self,
        title: String,
        draft: Draft,
        mode: AuditTaskMode,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.audit_can_operate() {
            return false;
        }
        let Some(project) = self.project else {
            return false;
        };
        let task_id = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        let target_url = draft.target.clone();
        if self.enqueue(
            Command::Audit(AuditCommand::StartScan {
                project,
                task_id,
                title: title.clone(),
                draft: Box::new(draft),
                mode,
                crawl_same_origin: self.audit_dashboard.crawl_same_origin,
                cancel: cancel.clone(),
            }),
            cx,
        ) {
            self.audit_dashboard
                .running_cancels
                .insert(task_id, (project, cancel));
            self.audit_dashboard.selected_audit_task_id = Some(task_id);
            if !self
                .audit_dashboard
                .snapshot
                .tasks
                .iter()
                .any(|t| t.id == task_id)
            {
                self.audit_dashboard.snapshot.tasks.insert(
                    0,
                    AuditTaskSummary {
                        id: task_id,
                        title: title.clone(),
                        target_url: target_url.clone(),
                        mode,
                        state: AuditTaskState::Running,
                        requests_sent: 0,
                        total_planned: 0,
                        issues_found: 0,
                        started_unix_ms: 0,
                        error: None,
                    },
                );
            }
            self.record_footer_event(
                FooterLogSeverity::Info,
                format!("启动扫描任务：{title}"),
                format!("模式：{} · 目标：{target_url}", mode.label()),
            );
            return true;
        }
        false
    }

    pub(in crate::workbench) fn set_audit_live_passive(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.audit_dashboard.snapshot.live_passive_enabled == enabled {
            return;
        }
        if self.enqueue(
            Command::Audit(AuditCommand::SetLivePassive { project, enabled }),
            cx,
        ) {
            self.audit_dashboard.snapshot.live_passive_enabled = enabled;
            self.record_footer_event(
                FooterLogSeverity::Info,
                if enabled {
                    "实时被动审计已恢复捕获"
                } else {
                    "实时被动审计已暂停"
                },
                format!(
                    "任务：2. 实时被动审计 ({})",
                    self.audit_dashboard.live_task_scope.card_suffix()
                ),
            );
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_audit_live_passive(&mut self, cx: &mut Context<Self>) {
        let next = !self.audit_dashboard.snapshot.live_passive_enabled;
        self.set_audit_live_passive(next, cx);
    }

    pub(in crate::workbench) fn set_audit_live_passive_crawl(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.audit_dashboard.snapshot.live_passive_crawl_enabled == enabled {
            return;
        }
        if self.enqueue(
            Command::Audit(AuditCommand::SetLivePassiveCrawl { project, enabled }),
            cx,
        ) {
            self.audit_dashboard.snapshot.live_passive_crawl_enabled = enabled;
            self.record_footer_event(
                FooterLogSeverity::Info,
                if enabled {
                    "实时被动爬行已恢复捕获"
                } else {
                    "实时被动爬行已暂停"
                },
                format!(
                    "任务：1. 实时被动爬行 ({})",
                    self.audit_dashboard.live_task_scope.card_suffix()
                ),
            );
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_audit_live_passive_crawl(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let next = !self.audit_dashboard.snapshot.live_passive_crawl_enabled;
        self.set_audit_live_passive_crawl(next, cx);
    }

    pub(in crate::workbench) fn open_new_live_task_modal(&mut self, cx: &mut Context<Self>) {
        if self.audit_dashboard.new_live_task_open {
            self.audit_dashboard.new_live_task_open = false;
        } else {
            self.audit_dashboard.new_scan_open = false;
            self.audit_dashboard.scan_error = None;
            self.audit_dashboard.live_task_draft_passive_audit =
                self.audit_dashboard.snapshot.live_passive_enabled;
            self.audit_dashboard.live_task_draft_passive_crawl =
                self.audit_dashboard.snapshot.live_passive_crawl_enabled;
            self.audit_dashboard.live_task_draft_scope = self.audit_dashboard.live_task_scope;
            self.audit_dashboard.new_live_task_open = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn apply_new_live_task_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let audit_on = self.audit_dashboard.live_task_draft_passive_audit;
        let crawl_on = self.audit_dashboard.live_task_draft_passive_crawl;
        let scope = self.audit_dashboard.live_task_draft_scope;

        self.set_audit_live_passive(audit_on, cx);
        self.set_audit_live_passive_crawl(crawl_on, cx);
        self.audit_dashboard.live_task_scope = scope;
        self.audit_dashboard.new_live_task_open = false;
        self.clear_dashboard_intruder_selection(window, cx);
        self.audit_dashboard.selected_audit_task_id = None;
        self.audit_dashboard.selected_builtin_task = if audit_on || !crawl_on {
            DashboardBuiltinTask::LivePassiveAudit
        } else {
            DashboardBuiltinTask::LivePassiveCrawl
        };
        let summary = format!(
            "已应用实时任务配置：被动审计={} · 被动爬行={} · 作用域={}",
            if audit_on { "开启" } else { "暂停" },
            if crawl_on { "开启" } else { "暂停" },
            scope.label()
        );
        self.record_footer_event(FooterLogSeverity::Info, "已更新实时任务配置", summary.clone());
        self.status = summary;
        cx.notify();
    }

    pub(in crate::workbench) fn stop_audit_scan(&mut self, task_id: Uuid, cx: &mut Context<Self>) {
        let Some(project) = self.project else {
            return;
        };
        let task_title = self
            .audit_dashboard
            .snapshot
            .tasks
            .iter()
            .find(|t| t.id == task_id)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| task_id.to_string());
        if let Some((_, cancel)) = self.audit_dashboard.running_cancels.remove(&task_id) {
            cancel.cancel();
        }
        if self.audit_can_operate() {
            let _ = self.enqueue(
                Command::Audit(AuditCommand::StopScan { project, task_id }),
                cx,
            );
            self.record_footer_event(
                FooterLogSeverity::Info,
                format!("已停止扫描任务：{task_title}"),
                format!("task_id={task_id}"),
            );
        }
        cx.notify();
    }

    pub(in crate::workbench) fn delete_audit_task(
        &mut self,
        task_id: Uuid,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let task_title = self
            .audit_dashboard
            .snapshot
            .tasks
            .iter()
            .find(|t| t.id == task_id)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| task_id.to_string());
        if let Some((_, cancel)) = self.audit_dashboard.running_cancels.remove(&task_id) {
            cancel.cancel();
        }
        if self.enqueue(
            Command::Audit(AuditCommand::DeleteTask { project, task_id }),
            cx,
        ) {
            self.audit_dashboard
                .snapshot
                .tasks
                .retain(|t| t.id != task_id);
            self.audit_dashboard.task_finding_ids.remove(&task_id);
            if self.audit_dashboard.selected_audit_task_id == Some(task_id) {
                self.audit_dashboard.selected_audit_task_id = None;
            }
            self.record_footer_event(
                FooterLogSeverity::Info,
                format!("已删除扫描任务：{task_title}"),
                format!("task_id={task_id}"),
            );
            cx.notify();
        }
    }
}
