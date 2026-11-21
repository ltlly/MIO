use super::*;

#[path = "session/storage.rs"]
mod storage;

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;

const STATE_RESERVE: u64 = (2 * MAX_ERROR_BYTES + MAX_TITLE_BYTES + 2048) as u64;
const COMPLETION_RESERVE: u64 =
    (MAX_BODY + MAX_MESSAGE_HEAD + MAX_SUMMARY_BYTES + 2 * MAX_ERROR_BYTES + 4096) as u64;

pub(super) struct Reservation {
    pub(super) bytes: u64,
    flow: FlowId,
}

impl<S: std::ops::Deref<Target = ProjectStore>> store::IntruderRepo<S> {
    #[cfg(test)]
    pub(crate) fn reject_session_writes(&self, id: Uuid, reject: bool) {
        self.0.intruder_sessions.runs[&id]
            .reject_writes
            .store(reject, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn set_session_write_hook(
        &self,
        id: Uuid,
        hook: Option<super::session_data::SessionWriteHook>,
    ) {
        *self.0.intruder_sessions.runs[&id]
            .write_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    pub fn is_session_run(&self, id: Uuid) -> bool {
        self.0.intruder_sessions.contains(id)
    }

    pub(in crate::store) fn session_cursor(&self, id: Uuid) -> Option<(i64, Uuid)> {
        self.0
            .intruder_sessions
            .runs
            .get(&id)
            .map(|dataset| (dataset.run.started_unix_ms, id))
    }
}

impl store::IntruderRepo<&mut ProjectStore> {
    #[cfg(test)]
    pub fn create_run_with_retention(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
    ) -> Result<IntruderRunSummary> {
        self.create_run_with_pool_catalog(
            project,
            id,
            tab_id,
            spec,
            started,
            &CancellationToken::new(),
        )
        .map(|(run, _)| run)
    }

    pub(crate) fn create_run_with_pool_catalog(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
        cancel: &CancellationToken,
    ) -> Result<(IntruderRunSummary, crate::IntruderResourcePools)> {
        self.0.intruder().check_session_owner(project, id)?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder Start 已取消；未创建资源池或运行"
        );
        if spec.save_to_project {
            return self.create_project_run_with_pool_catalog(id, tab_id, spec, started, cancel);
        }
        ensure!(
            !self.0.intruder_sessions.contains(id),
            "Intruder Session run ID 已存在；未开始"
        );
        ensure!(
            self.0.intruder_sessions.runs.len() < MAX_SESSION_DATASETS,
            "Intruder Session 数据集达到限额；未驱逐"
        );
        let total = spec.validate()?;
        let source = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            store::metadata::project_id(&source)? == project.id,
            "Intruder 项目归属已改变"
        );
        validate_retirements(&source)?;
        ensure_not_discarded(&source, id)?;
        ensure_no_run_rows(&source, id)?;
        let workspace = store::metadata::workspace(&source)?;
        let payload_configs = workspace
            .intruder
            .payload_configs
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        let has_pipeline = payload_configs
            .iter()
            .any(|cfg| cfg.processing_rules.iter().any(|r| r.enabled));
        let attack_settings = workspace
            .intruder
            .attack_settings
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        ensure!(
            workspace.intruder.tabs.iter().any(|tab| {
                tab.id == tab_id
                    && (tab.spec == *spec
                        || (has_pipeline
                            && tab.spec.revision == spec.revision
                            && tab.spec.draft == spec.draft
                            && tab.spec.attack_type == spec.attack_type
                            && tab.spec.positions == spec.positions
                            && tab.spec.max_concurrent_requests == spec.max_concurrent_requests
                            && tab.spec.request_timeout_ms == spec.request_timeout_ms
                            && tab.spec.update_host_header == spec.update_host_header
                            && tab.spec.save_to_project == spec.save_to_project
                            && tab.spec.baseline_request == spec.baseline_request
                            && tab.spec.resource_pool == spec.resource_pool))
            }),
            "Intruder 配置与已保存开放标签不一致；未开始"
        );
        let (pool, catalog) =
            store::resource_pools::resolve_start(&source, spec.resource_pool.as_ref(), cancel)?;
        let data = SessionData::from_spec(
            id,
            tab_id,
            spec,
            started,
            total,
            pool,
            attack_settings,
            payload_configs,
        )?;
        let available = self
            .0
            .intruder_sessions
            .quota
            .checked_sub(self.0.intruder_sessions.logical_bytes)
            .and_then(|bytes| bytes.checked_sub(self.0.intruder_sessions.reserved_bytes))
            .and_then(|bytes| bytes.checked_sub(STATE_RESERVE))
            .context("Intruder Session 容量不足；未开始")?;
        let logical_bytes = data.logical_bytes()?;
        ensure!(
            logical_bytes <= available,
            "Intruder Session 逻辑 RAM 数据配额不足（需要 {logical_bytes}，可用 {available} 字节）；未复制、驱逐或溢写到磁盘"
        );
        ensure!(
            !cancel.is_cancelled(),
            "Intruder Start 已取消；未创建资源池或运行"
        );
        source.commit()?;
        let run = data.run.clone();
        self.0.intruder_sessions.logical_bytes += logical_bytes;
        self.0.intruder_sessions.reserved_bytes += STATE_RESERVE;
        self.0.intruder_sessions.runs.insert(
            id,
            SessionDataset {
                data,
                owner: project,
                run: run.clone(),
                logical_bytes,
                disk_flows: BTreeSet::new(),
                disk_ticket: None,
                reservations: BTreeMap::new(),
                state_reserve: STATE_RESERVE,
                reactivated_baseline: None,
                #[cfg(test)]
                reject_writes: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                write_hook: std::sync::Mutex::new(None),
                #[cfg(test)]
                body_reads: std::sync::atomic::AtomicUsize::new(0),
            },
        );
        Ok((run, catalog))
    }

    pub(in crate::store) fn set_session_resource_pool(
        &mut self,
        id: Uuid,
        expected: u64,
        pool_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        let dataset = self
            .0
            .intruder_sessions
            .runs
            .get_mut(&id)
            .context("Intruder Session 不存在")?;
        let ticket = self.0.connection.unchecked_transaction()?;
        validate_ticket(&ticket, self.0.id, dataset)?;
        let catalog = store::resource_pools::read(&ticket)?;
        ensure!(
            catalog.find(pool_id).is_some(),
            "Intruder 资源池不存在；未绑定"
        );
        ensure!(
            dataset.data.run == dataset.run,
            "Intruder RAM 摘要已改变；未绑定资源池"
        );
        #[cfg(test)]
        ensure!(
            !dataset
                .reject_writes
                .load(std::sync::atomic::Ordering::SeqCst),
            "Intruder RAM 只读保护已启用"
        );
        let before = dataset.data.mutable_bytes(None)?;
        let mut staged = dataset.data.clone();
        let mut run = staged.bind_resource_pool(expected, pool_id, cancel)?;
        let after = staged.mutable_bytes(None)?;
        let growth = after.saturating_sub(before);
        ensure!(
            growth
                <= self.0.intruder_sessions.quota.saturating_sub(
                    self.0.intruder_sessions.logical_bytes
                        + self.0.intruder_sessions.reserved_bytes
                ),
            "Intruder Session 配额不足；资源池绑定未改变"
        );
        ensure!(!cancel.is_cancelled(), "Intruder 资源池绑定已取消");
        run.retention = IntruderRetention::Session;
        dataset.data = staged;
        dataset.run = run.clone();
        dataset.logical_bytes += growth;
        self.0.intruder_sessions.logical_bytes += growth;
        drop(ticket);
        Ok(run)
    }

    pub(in crate::store) fn request_session_baseline<T>(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        cancel: &CancellationToken,
        prepare: impl FnOnce(IntruderSpec) -> Result<T>,
    ) -> Result<(IntruderRunSummary, T)> {
        let dataset = self
            .0
            .intruder_sessions
            .runs
            .get_mut(&id)
            .context("Intruder Session 不存在")?;
        let ticket = self.0.connection.unchecked_transaction()?;
        validate_ticket(&ticket, self.0.id, dataset)?;
        validate_pool_binding(&ticket, &dataset.run)?;
        ensure!(
            dataset.data.run == dataset.run,
            "Intruder RAM 摘要已改变；未追加基准请求"
        );
        let state_reserve = if dataset.run.state == IntruderRunState::Completed {
            STATE_RESERVE.saturating_sub(dataset.state_reserve)
        } else {
            0
        };
        #[cfg(test)]
        ensure!(
            !dataset
                .reject_writes
                .load(std::sync::atomic::Ordering::SeqCst),
            "Intruder RAM 只读保护已启用"
        );
        let before = dataset.data.mutable_bytes(None)?;
        let mut staged = dataset.data.clone();
        let (mut run, prepared) = staged.request_baseline(expected_revision, cancel, prepare)?;
        let after = staged.mutable_bytes(None)?;
        let growth = after
            .checked_sub(before)
            .context("Intruder 基准请求字节计量回退")?;
        let available = self.0.intruder_sessions.quota.saturating_sub(
            self.0.intruder_sessions.logical_bytes + self.0.intruder_sessions.reserved_bytes,
        );
        ensure!(
            growth
                .checked_add(state_reserve)
                .is_some_and(|needed| needed <= available),
            "Intruder Session 状态余量不足；未追加基准请求"
        );
        ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
        run.retention = IntruderRetention::Session;
        dataset.data = staged;
        if dataset.disk_ticket.is_some() {
            dataset.reactivated_baseline = run.baseline.map(|baseline| baseline.ordinal);
        }
        dataset.run = run.clone();
        dataset.logical_bytes += growth;
        dataset.state_reserve += state_reserve;
        self.0.intruder_sessions.logical_bytes += growth;
        self.0.intruder_sessions.reserved_bytes += state_reserve;
        drop(ticket);
        Ok((run, prepared))
    }
}
