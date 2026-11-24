use super::*;

impl State {
    pub(in crate::service) fn queue_intruder_event(&mut self, event: Event) {
        let event = if self.intruder.notifications.is_empty() && !self.awaiting_event_slot {
            match self.events.try_send(event) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.cancel.cancel();
                    return;
                }
                Err(mpsc::error::TrySendError::Full(event)) => event,
            }
        } else {
            event
        };
        assert!(
            self.intruder.notifications.len() < MAX_NOTIFICATIONS,
            "Intruder notifications exceeded their admitted producer budget"
        );
        self.intruder.notifications.push_back(event);
    }

    pub(super) fn intruder_history_changed(&mut self) {
        if !self.storage.history_dirty {
            self.storage.history_dirty = true;
            self.queue_intruder_event(Event::HistoryChanged {
                project: self.project,
            });
        }
    }

    pub(in crate::service) async fn start_intruder(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        tab_id: TabId,
        spec: IntruderSpec,
        cancel: CancellationToken,
    ) {
        let spec_revision = spec.revision;
        let result = self
            .admit_intruder(project, run_id, tab_id, spec, cancel)
            .map_err(|error| error.to_string());
        self.queue_intruder_event(Event::IntruderStarted {
            project,
            run_id,
            tab_id,
            spec_revision,
            result,
        });
    }

    pub(super) fn admit_intruder(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        tab_id: TabId,
        spec: IntruderSpec,
        cancel: CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(!self.quiesced, "正在保存并切换项目；Intruder 未开始");
        ensure!(!cancel.is_cancelled(), "Intruder 已取消；未开始");
        ensure!(
            self.intruder.runs.len() < MAX_RUNS,
            "活动 Intruder 达到 {MAX_RUNS} 个 MIO 资源限额"
        );
        ensure!(
            self.store.intruder().find_run(run_id)?.is_none(),
            "Intruder run ID 已存在；未重新开始或重发请求"
        );
        ensure!(
            spec.validate()?
                .checked_sub(u64::from(spec.baseline_request))
                .is_some_and(|payloads| payloads > 0),
            "Intruder payload 列表为空，无法生成请求；未开始"
        );
        let workspace = self.store.metadata().workspace_arc()?;
        let attack_settings = workspace
            .intruder
            .attack_settings
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        let payload_configs = workspace
            .intruder
            .payload_configs
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        let mut effective_spec = spec;
        for (set_idx, set) in effective_spec.payload_sets.iter_mut().enumerate() {
            if let Some(cfg) = payload_configs.get(set_idx) {
                let enabled_rules: Vec<_> = cfg
                    .processing_rules
                    .iter()
                    .filter(|entry| entry.enabled)
                    .map(|entry| entry.rule.clone())
                    .collect();
                if !enabled_rules.is_empty() {
                    let processed =
                        crate::intruder::apply_payload_processing_rules(set, &enabled_rules)?;
                    ensure!(
                        !processed.is_empty(),
                        "Intruder payload 经处理管线执行后为空；未开始"
                    );
                    *set = processed;
                }
            }
        }
        ensure!(
            effective_spec
                .validate()?
                .checked_sub(u64::from(effective_spec.baseline_request))
                .is_some_and(|payloads| payloads > 0),
            "Intruder payload 经处理管线过滤后无法生成请求；未开始"
        );
        let (summary, resource_pools) = self.store.intruder_mut().create_run_with_pool_catalog(
            project,
            run_id,
            tab_id,
            &effective_spec,
            crate::codec_util::now_unix_ms(),
            &cancel,
        )?;
        // Start can add a fresh pool; existing shared timing is never reset by another run.
        self.intruder
            .pool_timings
            .retain(|id, _| resource_pools.find(*id).is_some());
        for pool in &resource_pools.pools {
            self.intruder.pool_timings.entry(pool.id).or_default();
        }
        self.intruder.resource_pools = resource_pools;
        let plan = AttackPlan::new(
            effective_spec.attack_type,
            effective_spec.draft.raw,
            effective_spec.positions,
            effective_spec.payload_sets,
        )?;
        self.intruder.runs.insert(
            run_id,
            ActiveRun {
                summary: summary.clone(),
                plan,
                target: effective_spec.draft.target,
                version: effective_spec.draft.version,
                max_concurrent_requests: effective_spec.max_concurrent_requests,
                timeout: Duration::from_millis(effective_spec.request_timeout_ms),
                update_host_header: effective_spec.update_host_header,
                attack_settings,
                cancel,
                active: 0,
                preparing: false,
                waiting_for_capacity: false,
                failure: None,
            },
        );
        self.intruder.dirty = true;
        Ok(summary)
    }

    pub(in crate::service) fn intruder_discarded(&mut self) {
        self.intruder.dirty = true;
        self.intruder.notified = false;
        self.intruder_history_changed();
    }

    pub(in crate::service) fn discard_intruder(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
    ) -> Result<IntruderDiscardReceipt> {
        self.require_terminal_intruder(project, id, expected_revision)?;
        let receipt = self
            .store
            .intruder_mut()
            .discard_run(id, expected_revision, workspace, &self.cancel)?;
        self.intruder.capacity_released();
        Ok(receipt)
    }

    pub(in crate::service) fn set_intruder_retention(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        retention: IntruderRetention,
        workspace: Workspace,
    ) -> Result<IntruderRetentionReceipt> {
        if retention == IntruderRetention::Project
            && let Some(active) = self.intruder.runs.get(&id)
        {
            self.check_project(project)?;
            self.require_saved()?;
            self.store.intruder().check_session_owner(project, id)?;
            ensure!(
                !self.quiesced && !self.cancel.is_cancelled(),
                "正在停止或切换项目；Intruder 数据保留"
            );
            ensure!(
                active.summary.retention == IntruderRetention::Session
                    && self.store.intruder().is_session_run(id)
                    && matches!(
                        active.summary.state,
                        IntruderRunState::Running | IntruderRunState::Paused
                    )
                    && !active.cancel.is_cancelled()
                    && !active.preparing
                    && active.failure.is_none(),
            );
            ensure!(
                active.summary.revision == expected_revision,
                "Intruder run revision 已改变；数据保留"
            );
            ensure!(
                active.active
                    == self
                        .intruder
                        .requests
                        .values()
                        .filter(|request| request.run_id == id)
                        .count()
                    && active
                        .summary
                        .dispatched
                        .checked_sub(active.summary.completed)
                        == Some(active.active as u64)
                    && self.intruder.requests.len() <= MAX_REQUESTS_IN_FLIGHT
                    && self.intruder.jobs.len() == self.intruder.requests.len()
                    && self.intruder.prep_jobs.len() == self.intruder.prep_requests.len(),
            );
            let mut ordinals = BTreeSet::new();
            let mut flows = BTreeSet::new();
            for request in self
                .intruder
                .requests
                .values()
                .filter(|request| request.run_id == id)
            {
                ensure!(
                    request.ordinal < active.summary.dispatched
                        && ordinals.insert(request.ordinal)
                        && flows.insert(request.flow_id),
                    "Intruder 在途请求序号或流量重复、越界；未改变保存方式"
                );
                let page = self
                    .store
                    .intruder()
                    .results(id, request.ordinal.checked_sub(1), 1)?;
                ensure!(
                    page.run == active.summary,
                    "Intruder 在途摘要已改变；未改变保存方式"
                );
                ensure!(
                    matches!(page.rows.as_slice(), [IntruderResultSummary {
                        ordinal,
                        outcome: IntruderResultOutcome::Flow(flow),
                    }] if *ordinal == request.ordinal
                        && flow.id == request.flow_id
                        && flow.state == FlowState::Pending),
                    "Intruder 在途注册与 Pending 结果不匹配；未改变保存方式"
                );
            }
        } else {
            self.require_terminal_intruder(project, id, expected_revision)?;
        }
        let receipt = self.store.intruder_mut().set_retention(
            project,
            id,
            expected_revision,
            retention,
            workspace,
            &self.cancel,
        )?;
        if let Some(active) = self.intruder.runs.get_mut(&id) {
            active.summary = receipt.run.clone();
        }
        self.intruder.capacity_released();
        self.intruder.dirty = true;
        self.intruder.notified = false;
        Ok(receipt)
    }

    pub(super) fn require_terminal_intruder(
        &self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
    ) -> Result<()> {
        self.check_project(project)?;
        self.require_saved()?;
        self.store.intruder().check_session_owner(project, id)?;
        ensure!(
            !self.quiesced && !self.cancel.is_cancelled(),
            "正在停止或切换项目；Intruder 数据保留"
        );
        // Request identities remain registered until their JoinSet handles are joined.
        ensure!(
            !self.intruder.runs.contains_key(&id)
                && !self
                    .intruder
                    .requests
                    .values()
                    .any(|request| request.run_id == id)
                && self.intruder.jobs.len() == self.intruder.requests.len()
                && self.intruder.prep_jobs.len() == self.intruder.prep_requests.len(),
        );
        let summary = self.intruder_run(id)?;
        ensure!(
            summary.state.is_terminal(),
            "Intruder 尚未结束；此操作不会取消运行"
        );
        ensure!(
            summary.revision == expected_revision,
            "Intruder run revision 已改变；数据保留"
        );
        Ok(())
    }

    pub(in crate::service) fn request_intruder_stop(
        &mut self,
        project: ProjectKey,
        id: Uuid,
    ) -> Result<()> {
        self.check_project(project)?;
        if let Some(run) = self.intruder.runs.get(&id) {
            run.cancel.cancel();
        } else {
            let summary = self.intruder_run(id)?;
            if summary.state == IntruderRunState::Paused && summary.completed == summary.dispatched
            {
                self.require_saved()?;
                ensure!(
                    !self.quiesced && !self.cancel.is_cancelled(),
                    "正在停止或切换项目；Intruder 控制未应用"
                );
                self.store
                    .intruder_mut()
                    .set_state(id, IntruderRunState::Stopped)?;
                self.intruder.dirty = true;
                return Ok(());
            }
            ensure!(
                summary.state.is_terminal(),
                "Intruder 运行任务不在当前会话中"
            );
        }
        Ok(())
    }
}
