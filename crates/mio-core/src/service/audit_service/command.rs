use super::*;

impl State {
    pub(in crate::service) async fn audit_command(&mut self, command: AuditCommand) -> Result<()> {
        match command {
            AuditCommand::QueryState {
                project,
                query_revision,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.store.audit().read_state().map(Box::new))
                    .map_err(|error| error.to_string());
                self.emit(Event::Audit(AuditEvent::State {
                    project,
                    query_revision,
                    result,
                }))
                .await;
            }
            AuditCommand::SetLivePassive { project, enabled } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未更改审计设置");
                let mut state = self.store.audit().read_state()?;
                if state.live_passive_enabled != enabled {
                    state.live_passive_enabled = enabled;
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::SetLivePassiveCrawl { project, enabled } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未更改被动爬行设置");
                let mut state = self.store.audit().read_state()?;
                if state.live_passive_crawl_enabled != enabled {
                    state.live_passive_crawl_enabled = enabled;
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                if let Ok(mut workspace) = self.store.metadata().workspace()
                    && workspace.live_passive_crawl_enabled != enabled
                {
                    workspace.live_passive_crawl_enabled = enabled;
                    workspace.revision = workspace.revision.saturating_add(1);
                    if self.store.metadata_mut().save_workspace(&workspace).is_ok() {
                        self.emit(Event::WorkspaceChanged { project, workspace })
                            .await;
                    }
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::SetLivePassiveScope {
                project,
                suite_scope_only,
            } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未更改实时任务作用域");
                let mut state = self.store.audit().read_state()?;
                if state.live_passive_suite_scope_only != suite_scope_only {
                    state.live_passive_suite_scope_only = suite_scope_only;
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::StartScan {
                project,
                task_id,
                title,
                draft,
                mode,
                crawl_same_origin,
                cancel,
            } => {
                self.start_audit_scan(
                    project,
                    task_id,
                    title,
                    *draft,
                    (mode, crawl_same_origin),
                    cancel,
                )
                .await?;
            }
            AuditCommand::StopScan { project, task_id } => {
                self.check_project(project)?;
                let stopping_in_flight = self.audit.stop_task(project, task_id);
                if !stopping_in_flight && self.unsaved_count() == 0 && !self.quiesced {
                    let mut state = self.store.audit().read_state()?;
                    if let Some(idx) = state.tasks.iter().position(|t| {
                        t.id == task_id
                            && matches!(t.state, AuditTaskState::Running | AuditTaskState::Paused)
                    }) {
                        state.tasks[idx].state = AuditTaskState::Cancelled;
                        state.revision = next_audit_revision(state.revision);
                        let task = state.tasks[idx].clone();
                        self.write_audit_snapshot(&state)?;
                        self.emit(Event::Audit(AuditEvent::TaskUpdated {
                            project,
                            revision: state.revision,
                            task,
                            new_findings: Vec::new(),
                        }))
                        .await;
                    }
                }
            }
            AuditCommand::PauseScan { project, task_id } => {
                self.set_scan_pause_state(project, task_id, true).await?;
            }
            AuditCommand::ResumeScan { project, task_id } => {
                self.set_scan_pause_state(project, task_id, false).await?;
            }
            AuditCommand::DeleteTask { project, task_id } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未删除审计任务");
                ensure!(
                    !self.audit.active.contains_key(&task_id),
                    "审计任务仍在运行；请先停止扫描再删除"
                );
                let mut state = self.store.audit().read_state()?;
                let before = state.tasks.len();
                state.tasks.retain(|t| t.id != task_id);
                if state.tasks.len() != before {
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::SetFindingSeverity {
                project,
                finding_id,
                severity,
                confidence,
            } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未更改审计告警等级");
                let mut state = self.store.audit().read_state()?;
                if let Some(finding) = state.findings.iter_mut().find(|f| f.id == finding_id) {
                    finding.severity_label = severity.label().to_owned();
                    finding.severity_rank = severity.rank();
                    finding.confidence = confidence;
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::DeleteFinding {
                project,
                finding_id,
            } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未删除审计告警");
                let mut state = self.store.audit().read_state()?;
                let before = state.findings.len();
                state.findings.retain(|f| f.id != finding_id);
                if state.findings.len() != before {
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::ClearFindings { project } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未清空审计告警");
                let mut state = self.store.audit().read_state()?;
                if !state.findings.is_empty() {
                    state.findings.clear();
                    state.revision = next_audit_revision(state.revision);
                    self.write_audit_snapshot(&state)?;
                }
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::RecordManualFinding { project, finding } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未记录手动审计告警");
                ensure!(!finding.url.trim().is_empty(), "审计告警 URL 不能为空");
                if let Some(fid) = finding.flow_id {
                    self.store.websocket().flow_url(fid).context("关联的实时流量记录不存在")?;
                }
                let mut state = self.store.audit().read_state()?;
                ensure!(
                    !state.findings.iter().any(|f| f.id == finding.id),
                    "审计告警 ID 已存在"
                );
                ensure!(
                    state.findings.len() < crate::MAX_AUDIT_FINDINGS,
                    "审计告警数量已达 {} 条上限",
                    crate::MAX_AUDIT_FINDINGS
                );
                state.findings.insert(0, *finding);
                state.revision = next_audit_revision(state.revision);
                self.write_audit_snapshot(&state)?;
                self.emit_audit_state(project, state).await;
            }
            AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id,
                url,
                evidence,
            } => {
                self.check_project(project)?;
                self.require_saved()?;
                ensure!(!self.quiesced, "正在保存或切换项目；未更新审计告警关联报文");
                if let Some(fid) = flow_id {
                    self.store
                        .websocket()
                        .flow_url(fid)
                        .context("关联的实时流量记录不存在")?;
                }
                let mut state = self.store.audit().read_state()?;
                let finding = state
                    .findings
                    .iter_mut()
                    .find(|f| f.id == finding_id)
                    .context("所选审计告警已不存在")?;
                if let Some(fid) = flow_id {
                    finding.flow_id = Some(fid);
                }
                if !url.is_empty() && finding.url.is_empty() {
                    finding.url = url.clone();
                }
                let append_line = if !url.is_empty() && !evidence.contains(&url) {
                    format!("[{url}] {evidence}")
                } else {
                    evidence
                };
                if !append_line.trim().is_empty() && !finding.evidence.contains(&append_line) {
                    if finding.evidence.is_empty() {
                        finding.evidence = append_line;
                    } else {
                        finding.evidence = format!("{}\n{}", finding.evidence, append_line);
                    }
                }
                state.revision = next_audit_revision(state.revision);
                self.write_audit_snapshot(&state)?;
                self.emit_audit_state(project, state).await;
            }
        }
        Ok(())
    }

    async fn set_scan_pause_state(
        &mut self,
        project: ProjectKey,
        task_id: Uuid,
        paused: bool,
    ) -> Result<()> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(!self.quiesced, "正在保存或切换项目；未更改扫描任务暂停状态");
        ensure!(
            self.audit.set_task_paused(project, task_id, paused),
            if paused {
                "扫描任务未在运行中，无法暂停"
            } else {
                "扫描任务未在暂停中或进程已结束，无法继续"
            }
        );
        let mut state = self.store.audit().read_state()?;
        let target_state = if paused {
            AuditTaskState::Paused
        } else {
            AuditTaskState::Running
        };
        if let Some(idx) = state.tasks.iter().position(|t| t.id == task_id) {
            state.tasks[idx].state = target_state;
            state.revision = next_audit_revision(state.revision);
            let task = state.tasks[idx].clone();
            self.write_audit_snapshot(&state)?;
            self.emit(Event::Audit(AuditEvent::TaskUpdated {
                project,
                revision: state.revision,
                task,
                new_findings: Vec::new(),
            }))
            .await;
        }
        Ok(())
    }

    pub(in crate::service) async fn flush_audit_signals(&mut self) {
        for signal in self.audit.drain_pending_signals(self.project) {
            self.on_audit_signal(signal).await;
        }
    }

    pub(in crate::service) async fn on_audit_signal(&mut self, signal: AuditSignal) {
        if signal.project != self.project {
            return;
        }
        let mut history_changed = false;
        if let AuditSignalKind::Step {
            flow: Some(flow), ..
        } = &signal.kind
        {
            if self.persist(flow).is_ok() {
                self.audit
                    .bind_probe_seed_flow(signal.project, signal.task_id, flow.summary.id);
                history_changed = true;
            } else {
                self.audit.cancel_all();
            }
        }
        let is_finished = matches!(signal.kind, AuditSignalKind::Finished { .. });
        let Ok(mut state) = self.store.audit().read_state() else {
            return;
        };
        let Some(task_idx) = state.tasks.iter().position(|t| t.id == signal.task_id) else {
            if history_changed {
                self.history_changed().await;
            }
            return;
        };
        let new_findings = match signal.kind {
            AuditSignalKind::Step {
                requests_sent,
                findings,
                ..
            } => {
                let added = merge_audit_findings(&mut state.findings, findings);
                let task = &mut state.tasks[task_idx];
                task.requests_sent = task.requests_sent.max(requests_sent);
                task.total_planned = task.total_planned.max(task.requests_sent);
                task.issues_found = task.issues_found.saturating_add(added.len() as u64);
                added
            }
            AuditSignalKind::Finished {
                requests_sent,
                state: terminal,
                error,
            } => {
                let task = &mut state.tasks[task_idx];
                task.requests_sent = task.requests_sent.max(requests_sent);
                task.total_planned = task.total_planned.max(task.requests_sent);
                task.state = terminal;
                task.error = error;
                Vec::new()
            }
        };
        state.revision = next_audit_revision(state.revision);
        let updated_task = state.tasks[task_idx].clone();
        if self.unsaved_count() != 0 {
            if history_changed {
                self.history_changed().await;
            }
            return;
        }
        if let Err(error) = self.write_audit_snapshot(&state) {
            self.error("audit", error).await;
            return;
        }
        if history_changed {
            self.history_changed().await;
        }
        self.emit(Event::Audit(AuditEvent::TaskUpdated {
            project: signal.project,
            revision: state.revision,
            task: updated_task,
            new_findings,
        }))
        .await;
        if is_finished {
            self.finish_task_oast_check(signal.project, signal.task_id)
                .await;
        }
    }

    pub(in crate::service) fn audit_inspect_live_flow(
        &mut self,
        record: &FlowRecord,
    ) -> Option<AuditEvent> {
        if record.summary.state != FlowState::Complete || self.unsaved_count() != 0 {
            return None;
        }
        let resp = record.response.as_ref()?;
        let mut state = self.store.audit().read_state().ok()?;
        let workspace = self.store.metadata().workspace_arc().ok();
        if state.live_passive_suite_scope_only
            && workspace
                .as_ref()
                .is_some_and(|ws| !ws.scope.is_url_in_scope(&record.summary.url))
        {
            return None;
        }
        if state.live_passive_crawl_enabled
            && let Some(ws) = workspace.as_ref()
        {
            let discovered =
                discover_same_origin_links_and_forms(&record.request.url, resp, &ws.scope);
            let now_ms = now_unix_ms();
            for ep in discovered.into_iter().take(16) {
                let owner = format!("passive-crawl:{}:{}", ep.method, ep.url);
                let raw_req = crate::request_to_raw(&ep.request);
                let id = Uuid::new_v4();
                let summary = FlowSummary {
                    id,
                    revision: 1,
                    source: TrafficSource::Scanner,
                    method: ep.request.method.clone(),
                    url: ep.request.url.clone(),
                    status: None,
                    state: FlowState::Pending,
                    request_bytes: ep.request.body.len() as u64,
                    response_bytes: 0,
                    started_unix_ms: now_ms,
                    elapsed_ms: 0,
                };
                let detail = crate::SiteMapEntryDetail {
                    row_id: crate::HistoryEntryId::Live(id),
                    request_raw: raw_req,
                    response_raw: None,
                    original_request_raw: None,
                    original_response_raw: None,
                    record: FlowRecord {
                        summary: summary.clone(),
                        request: ep.request,
                        response: None,
                    },
                    original: crate::OriginalMessages::default(),
                };
                let _ = self.store.site_map_mut().add_entry(
                    ws.revision,
                    &owner,
                    crate::HistoryRow::Live(summary),
                    detail,
                    true,
                    &self.cancel,
                );
            }
        }
        if !state.live_passive_enabled {
            return None;
        }
        let now_ms = now_unix_ms();
        let mut detected = inspect_passive(
            Some(record.summary.id),
            &record.request,
            resp,
            now_ms,
        );
        if let Some(ws) = workspace.as_ref()
            && !ws.custom_scan_checks.is_empty()
        {
            detected.extend(evaluate_custom_scan_checks(
                Some(record.summary.id),
                &record.request,
                resp,
                &ws.custom_scan_checks,
                crate::AuditPhase::Passive,
                now_ms,
            ));
        }
        if detected.is_empty() {
            return None;
        }
        let added = merge_audit_findings(&mut state.findings, detected);
        if added.is_empty() {
            return None;
        }
        state.revision = next_audit_revision(state.revision);
        self.write_audit_snapshot(&state).ok()?;
        Some(AuditEvent::PassiveFindings {
            project: self.project,
            revision: state.revision,
            findings: added,
        })
    }

    async fn start_audit_scan(
        &mut self,
        project: ProjectKey,
        task_id: Uuid,
        title: String,
        draft: Draft,
        (mode, crawl_same_origin): (AuditTaskMode, bool),
        cancel: CancellationToken,
    ) -> Result<()> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(!self.quiesced, "正在保存或切换项目；未启动审计扫描");
        ensure!(!task_id.is_nil(), "审计任务缺少有效 ID");
        ensure!(!cancel.is_cancelled(), "审计任务已取消；未发送请求");
        let mut state = self.store.audit().read_state()?;
        ensure!(
            !state.tasks.iter().any(|t| t.id == task_id),
            "审计任务 ID 已存在；未重复创建扫描"
        );
        ensure!(
            state.tasks.len() < MAX_AUDIT_TASKS,
            "审计任务数量已达 {MAX_AUDIT_TASKS} 个上限；请清理旧任务后重试"
        );
        let mut plan =
            self.prepare_audit_scan(project, task_id, &draft, mode, crawl_same_origin)?;
        if mode == AuditTaskMode::PassiveAndActive
            && (!plan.insertion_points.is_empty() || plan.crawl_same_origin)
        {
            let _ = self.collaborator.ensure_default_started(project).await;
            if let Some(issued) = self.collaborator.issue_internal_payload("scanner-ssrf") {
                self.audit.register_oast_probe(OastProbeRecord {
                    project,
                    token: issued.token,
                    callback_url: issued.http_callback_url.clone(),
                    task_id,
                    seed_flow_id: None,
                    target_url: plan.request.url.clone(),
                });
                plan.oast_callback_url = Some(issued.http_callback_url);
            }
        }
        let probes_per_point = 8 + u64::from(plan.oast_callback_url.is_some());
        let target_level_probes = 1 + u64::from(build_authz_strip_probe(&plan.request).is_some());
        let crawl_budget = if plan.crawl_same_origin {
            CrawlConfig::default().max_requests as u64
        } else {
            0
        };
        let total_planned = match mode {
            AuditTaskMode::PassiveOnly => 1 + crawl_budget,
            AuditTaskMode::PassiveAndActive => {
                let active_extra = if !plan.insertion_points.is_empty() || plan.crawl_same_origin {
                    target_level_probes
                        + (plan.insertion_points.len() as u64).saturating_mul(probes_per_point)
                } else {
                    0
                };
                1 + crawl_budget + active_extra
            }
        };
        let clean_title = if title.trim().is_empty() {
            format!("{} {}", plan.request.method, plan.request.url)
        } else {
            title.trim().to_owned()
        };
        let summary = AuditTaskSummary {
            id: task_id,
            title: clean_title,
            target_url: plan.request.url.clone(),
            mode,
            state: AuditTaskState::Running,
            requests_sent: 0,
            total_planned,
            issues_found: 0,
            started_unix_ms: now_unix_ms(),
            error: None,
        };
        state.tasks.insert(0, summary.clone());
        state.revision = next_audit_revision(state.revision);
        self.write_audit_snapshot(&state)?;
        let combined_cancel = self.cancel.child_token();
        let shutdown = self.cancel.clone();
        let caller_cancel = cancel;
        let forward = combined_cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = caller_cancel.cancelled() => {}
                _ = shutdown.cancelled() => {}
                _ = forward.cancelled() => return,
            }
            forward.cancel();
        });
        if let Err(err) = self.audit.start_scan(plan, combined_cancel.clone()) {
            combined_cancel.cancel();
            if let Some(t) = state.tasks.iter_mut().find(|t| t.id == task_id) {
                t.state = AuditTaskState::Failed;
                t.error = Some(err.to_string());
            }
            state.revision = next_audit_revision(state.revision);
            let _ = self.write_audit_snapshot(&state);
            return Err(err);
        }
        self.emit(Event::Audit(AuditEvent::TaskUpdated {
            project,
            revision: state.revision,
            task: summary,
            new_findings: Vec::new(),
        }))
        .await;
        Ok(())
    }
    fn prepare_audit_scan(
        &self,
        project: ProjectKey,
        task_id: Uuid,
        draft: &Draft,
        mode: AuditTaskMode,
        crawl_same_origin: bool,
    ) -> Result<ScanPlan> {
        let workspace = self.store.metadata().workspace()?;
        let repeater_settings = &workspace.repeater_settings;
        let request = http::parse_repeater_request(
            &draft.raw,
            &draft.target,
            draft.version,
            repeater_settings.update_content_length,
        )?;
        let mut send_config = self.build_tool_send_config_for_url(&workspace, &request.url)?;
        send_config.http.version = Some(request.version);
        send_config.http.update_content_length = repeater_settings.update_content_length;
        send_config.http.normalize_http1_line_endings =
            repeater_settings.normalize_http1_line_endings;
        send_config.http.raw_http1_override = (!repeater_settings.normalize_http1_line_endings
            && draft.version != mio_plugin_api::HttpVersion::Http2)
            .then(|| Arc::<[u8]>::from(draft.raw.as_slice()));
        let _ = prepare_send(&request, send_config.clone())?;
        let insertion_points = if mode == AuditTaskMode::PassiveAndActive {
            extract_insertion_points(&request)
        } else {
            Vec::new()
        };
        Ok(ScanPlan {
            project,
            task_id,
            request,
            config: send_config,
            mode,
            crawl_same_origin,
            scope: workspace.scope,
            insertion_points,
            unpack_compressed_responses: repeater_settings.unpack_compressed_responses,
            oast_callback_url: None,
            plugin_active_hooks: if mode == AuditTaskMode::PassiveAndActive {
                self.active_check_hooks()
            } else {
                Vec::new()
            },
            custom_scan_checks: workspace.custom_scan_checks,
            sessions: self.sessions.runtime.clone(),
        })
    }

    pub(super) fn write_audit_snapshot(&mut self, state: &AuditStateSnapshot) -> Result<()> {
        self.store
            .audit_mut()
            .write_state(state, &CancellationToken::new())
    }

    pub(super) async fn emit_audit_state(
        &mut self,
        project: ProjectKey,
        state: AuditStateSnapshot,
    ) {
        self.emit(Event::Audit(AuditEvent::State {
            project,
            query_revision: 0,
            result: Ok(Box::new(state)),
        }))
        .await;
    }
}

pub(super) fn next_audit_revision(current: u64) -> u64 {
    current + 1
}
