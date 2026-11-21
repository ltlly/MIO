use super::*;

impl State {
    pub(in crate::service) fn request_intruder_baseline(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        operation_id: Uuid,
        expected_revision: u64,
        admission_cancel: &CancellationToken,
    ) -> Result<IntruderBaselineReceipt> {
        self.check_project(project)?;
        self.require_saved()?;
        self.store.intruder().check_session_owner(project, id)?;
        ensure!(
            !operation_id.is_nil()
                && !self.quiesced
                && !self.cancel.is_cancelled()
                && !admission_cancel.is_cancelled(),
            "Intruder 基准请求准入已取消或不可用；未增加请求"
        );
        let previous = self.intruder_run(id)?;
        ensure!(
            previous.revision == expected_revision,
            "Intruder run revision 已改变；未增加基准请求"
        );
        ensure!(
            previous.baseline.is_none(),
            "Intruder 已有基准请求；未重复发送"
        );
        ensure!(
            previous
                .resource_pool
                .is_none_or(|id| self.intruder.resource_pools.find(id).is_some()),
            "Intruder 当前资源池不存在；未追加基准请求"
        );
        let needs_preparation = if let Some(active) = self.intruder.runs.get(&id) {
            ensure!(
                matches!(
                    active.summary.state,
                    IntruderRunState::Running | IntruderRunState::Paused
                ) && !active.cancel.is_cancelled()
                    && !active.preparing
                    && active.failure.is_none()
                    && active.summary == previous
                    && active.plan.request_count() == previous.total,
                "Intruder 正在停止或运行计划不一致；未增加基准请求"
            );
            ensure!(
                active.active
                    == self
                        .intruder
                        .requests
                        .values()
                        .filter(|request| request.run_id == id)
                        .count()
                    && previous.dispatched.checked_sub(previous.completed)
                        == Some(active.active as u64)
                    && self.intruder.requests.len() <= MAX_REQUESTS_IN_FLIGHT
                    && self.intruder.jobs.len() == self.intruder.requests.len(),
                "Intruder 在途请求归属不一致；未增加基准请求"
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
                    request.ordinal < previous.dispatched
                        && ordinals.insert(request.ordinal)
                        && flows.insert(request.flow_id),
                    "Intruder 在途请求重复；未增加基准请求"
                );
                let page = self
                    .store
                    .intruder()
                    .results(id, request.ordinal.checked_sub(1), 1)?;
                ensure!(
                    page.run == previous
                        && matches!(page.rows.as_slice(),
                    [IntruderResultSummary { ordinal, outcome: IntruderResultOutcome::Flow(flow) }]
                        if *ordinal == request.ordinal && flow.id == request.flow_id
                            && flow.state == FlowState::Pending),
                    "Intruder 在途注册与 Pending 结果不匹配；未增加基准请求"
                );
            }
            false
        } else {
            ensure!(
                previous.state == IntruderRunState::Completed,
                "仅正常完成的攻击可显式补发基准请求"
            );
            ensure!(
                self.intruder.runs.len() < MAX_RUNS
                    && !self
                        .intruder
                        .requests
                        .values()
                        .any(|request| request.run_id == id)
                    && self.intruder.jobs.len() == self.intruder.requests.len(),
                "Intruder 活动任务已达上限或原发送尚未回收；未增加基准请求"
            );
            true
        };
        let (run, prepared) = if needs_preparation {
            let (attack_settings, _) = self.store.intruder().read_run_settings_only(id)?;
            self.store.intruder_mut().request_baseline_with(
                project,
                id,
                expected_revision,
                admission_cancel,
                |spec| {
                    let plan = AttackPlan::new(
                        spec.attack_type,
                        spec.draft.raw,
                        spec.positions,
                        spec.payload_sets,
                    )?;
                    ensure!(
                        plan.request_count() > 0 && plan.request_count() == previous.total,
                        "Intruder 冻结计划与已完成结果不一致；未增加基准请求"
                    );
                    Ok(Some(ActiveRun {
                        summary: previous.clone(),
                        plan,
                        target: spec.draft.target,
                        version: spec.draft.version,
                        max_concurrent_requests: spec.max_concurrent_requests,
                        timeout: Duration::from_millis(spec.request_timeout_ms),
                        update_host_header: spec.update_host_header,
                        attack_settings,
                        cancel: CancellationToken::new(),
                        active: 0,
                        preparing: false,
                        waiting_for_capacity: false,
                        failure: None,
                    }))
                },
            )?
        } else {
            (
                self.store.intruder_mut().request_baseline(
                    project,
                    id,
                    expected_revision,
                    admission_cancel,
                )?,
                None,
            )
        };
        let cancel = if let Some(mut active) = prepared {
            active.summary = run.clone();
            let cancel = active.cancel.clone();
            self.intruder.runs.insert(id, active);
            cancel
        } else {
            let active = self
                .intruder
                .runs
                .get_mut(&id)
                .expect("validated active Intruder remains registered");
            active.summary = run.clone();
            active.cancel.clone()
        };
        self.intruder.dirty = true;
        self.intruder.notified = false;
        Ok(IntruderBaselineReceipt { run, cancel })
    }

    pub(in crate::service) fn set_intruder_result_notes(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        ordinal: u64,
        expected_notes_revision: u64,
        notes: Vec<u8>,
    ) -> Result<(u64, Vec<u8>)> {
        self.check_project(project)?;
        self.store.intruder_mut().set_result_notes(
            project,
            run_id,
            ordinal,
            expected_notes_revision,
            &notes,
            &self.cancel,
        )
    }

    pub(in crate::service) fn set_intruder_result_highlight(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        entries: Vec<(u64, u64)>,
        highlight: Option<u8>,
    ) -> Result<Vec<(u64, u64)>> {
        self.check_project(project)?;
        self.store
            .intruder_mut()
            .set_result_highlight(project, run_id, &entries, highlight, &self.cancel)
    }

    pub(in crate::service) fn rename_intruder(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        operation_id: Uuid,
        expected_title_revision: u64,
        title: &str,
    ) -> Result<IntruderRunSummary> {
        self.check_project(project)?;
        ensure!(
            !operation_id.is_nil(),
            "Intruder 重命名 operation ID 无效；名称未改变"
        );
        self.require_saved()?;
        ensure!(
            !self.quiesced && !self.cancel.is_cancelled(),
            "正在停止或切换项目；名称未改变"
        );
        let run = self.store.intruder_mut().rename_run(
            project,
            id,
            expected_title_revision,
            title,
            &self.cancel,
        )?;
        if let Some(active) = self.intruder.runs.get_mut(&id) {
            active.summary.title = run.title.clone();
            active.summary.title_revision = run.title_revision;
        }
        self.intruder.dirty = true;
        self.intruder.notified = false;
        Ok(run)
    }

    pub(in crate::service) fn set_intruder_paused(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        paused: bool,
    ) -> Result<IntruderRunSummary> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(
            !self.quiesced && !self.cancel.is_cancelled(),
            "正在停止或切换项目；Intruder 控制未应用"
        );
        let summary = self.intruder_run(id)?;
        ensure!(
            !summary.state.is_terminal() && summary.completed < summary.total,
            "Intruder 攻击已结束；不能暂停或恢复，也不会重发请求"
        );
        if !self.intruder.runs.contains_key(&id)
            && summary.state == IntruderRunState::Paused
            && summary.completed == summary.dispatched
        {
            if paused {
                return Ok(summary);
            }
            ensure!(
                self.intruder.runs.len() < MAX_RUNS,
                "活动 Intruder 达到 {MAX_RUNS} 个 MIO 资源限额"
            );
            ensure!(
                summary.resource_pool.is_none_or(|pool| self
                    .intruder
                    .resource_pools
                    .find(pool)
                    .is_some()),
                "Intruder 当前资源池不存在；不能恢复"
            );
            let (spec, attack_settings, _) = self.store.intruder().read_spec_with_settings(id)?;
            let plan = AttackPlan::new(
                spec.attack_type,
                spec.draft.raw,
                spec.positions,
                spec.payload_sets,
            )?;
            let resumed = self
                .store
                .intruder_mut()
                .set_state(id, IntruderRunState::Running)?;
            self.intruder.runs.insert(
                id,
                ActiveRun {
                    summary: resumed.clone(),
                    plan,
                    target: spec.draft.target,
                    version: spec.draft.version,
                    max_concurrent_requests: spec.max_concurrent_requests,
                    timeout: Duration::from_millis(spec.request_timeout_ms),
                    update_host_header: spec.update_host_header,
                    attack_settings,
                    cancel: CancellationToken::new(),
                    active: 0,
                    preparing: false,
                    waiting_for_capacity: false,
                    failure: None,
                },
            );
            self.intruder.dirty = true;
            return Ok(resumed);
        }
        let run = self
            .intruder
            .runs
            .get(&id)
            .context("Intruder 运行任务不在当前会话中；不能恢复")?;
        ensure!(
            !run.cancel.is_cancelled()
                && matches!(
                    run.summary.state,
                    IntruderRunState::Running | IntruderRunState::Paused
                ),
            "Intruder 正在停止；不能暂停或恢复"
        );
        let state = if paused {
            IntruderRunState::Paused
        } else {
            IntruderRunState::Running
        };
        // Unlike stop/recovery transitions, a failed control must not change phase in memory
        // or queue a future Resume. Saving retries only the stop and result recovery records.
        let saved = self.store.intruder_mut().set_state(id, state);
        let run = self.intruder.runs.get_mut(&id).unwrap();
        match saved {
            Ok(summary) => {
                run.summary = summary.clone();
                self.intruder.dirty = true;
                Ok(summary)
            }
            Err(error) => {
                run.failure.get_or_insert_with(|| {
                    "暂停或恢复状态写盘失败；已停止派发，保存不会恢复攻击".into()
                });
                self.intruder.dirty = true;
                self.storage_failed(&error);
                Err(error.context("Intruder 暂停或恢复未确认，已取消后续派发"))
            }
        }
    }
}
