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

impl ProjectStore {
    #[cfg(test)]
    pub(crate) fn reject_session_intruder_writes(&self, id: Uuid, reject: bool) {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        let connection = &self.intruder_sessions.runs[&id].connection;
        if reject {
            connection.authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Insert { .. }
                | AuthAction::Update { .. }
                | AuthAction::Delete { .. } => Authorization::Deny,
                _ => Authorization::Allow,
            }));
        } else {
            connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        }
    }

    pub fn is_session_intruder_run(&self, id: Uuid) -> bool {
        self.intruder_sessions.contains(id)
    }

    pub(in crate::store) fn session_intruder_cursor(&self, id: Uuid) -> Option<(i64, Uuid)> {
        self.intruder_sessions
            .runs
            .get(&id)
            .map(|dataset| (dataset.run.started_unix_ms, id))
    }

    #[cfg(test)]
    pub fn create_intruder_run_with_retention(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
    ) -> Result<IntruderRunSummary> {
        self.create_intruder_run_with_pool_catalog(
            project,
            id,
            tab_id,
            spec,
            started,
            &CancellationToken::new(),
        )
        .map(|(run, _)| run)
    }

    pub(crate) fn create_intruder_run_with_pool_catalog(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
        cancel: &CancellationToken,
    ) -> Result<(IntruderRunSummary, crate::IntruderResourcePools)> {
        self.check_intruder_session_owner(project, id)?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder Start 已取消；未创建资源池或运行"
        );
        if spec.save_to_project {
            return self
                .create_project_intruder_run_with_pool_catalog(id, tab_id, spec, started, cancel);
        }
        ensure!(
            !self.intruder_sessions.contains(id),
            "Intruder Session run ID 已存在；未开始"
        );
        ensure!(
            self.intruder_sessions.runs.len() < MAX_SESSION_DATASETS,
            "Intruder Session 数据集达到限额；未驱逐"
        );
        let total = spec.validate()?;
        let source = self
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
        let connection = session_connection()?;
        let transaction = connection.unchecked_transaction()?;
        let (pool, catalog) =
            store::resource_pools::resolve_start(&source, spec.resource_pool.as_ref(), cancel)?;
        let mut run = insert_intruder_run_with_settings(
            &transaction,
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
            .intruder_sessions
            .quota
            .checked_sub(self.intruder_sessions.logical_bytes)
            .and_then(|bytes| bytes.checked_sub(self.intruder_sessions.reserved_bytes))
            .and_then(|bytes| bytes.checked_sub(STATE_RESERVE))
            .context("Intruder Session 容量不足；未开始")?;
        let logical_bytes = dataset_logical_bytes(&transaction, id, available, cancel)?;
        transaction.pragma_update(None, "query_only", true)?;
        transaction.commit()?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder Start 已取消；未创建资源池或运行"
        );
        source.commit()?;
        run.retention = IntruderRetention::Session;
        self.intruder_sessions.logical_bytes += logical_bytes;
        self.intruder_sessions.reserved_bytes += STATE_RESERVE;
        self.intruder_sessions.runs.insert(
            id,
            SessionDataset {
                connection,
                owner: project,
                run: run.clone(),
                logical_bytes,
                disk_flows: BTreeSet::new(),
                disk_ticket: None,
                reservations: BTreeMap::new(),
                state_reserve: STATE_RESERVE,
                reactivated_baseline: None,
            },
        );
        Ok((run, catalog))
    }

    pub(in crate::store) fn set_session_intruder_resource_pool(
        &mut self,
        id: Uuid,
        expected: u64,
        pool_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        let dataset = self
            .intruder_sessions
            .runs
            .get_mut(&id)
            .context("Intruder Session 不存在")?;
        let ticket = self.connection.unchecked_transaction()?;
        validate_ticket(&ticket, self.id, dataset)?;
        let catalog = store::resource_pools::read(&ticket)?;
        ensure!(
            catalog.find(pool_id).is_some(),
            "Intruder 资源池不存在；未绑定"
        );
        let mut stored = read_run(&dataset.connection, id)?;
        stored.retention = IntruderRetention::Session;
        ensure!(
            stored == dataset.run,
            "Intruder RAM 摘要已改变；未绑定资源池"
        );
        let before = mutable_bytes(&dataset.connection, id, None)?;
        ensure!(
            dataset
                .connection
                .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))?,
            "Intruder RAM 只读保护已改变"
        );
        dataset
            .connection
            .pragma_update(None, "query_only", false)?;
        let operation = (|| -> Result<(IntruderRunSummary, u64)> {
            let transaction = dataset.connection.unchecked_transaction()?;
            let run = bind_resource_pool(&transaction, id, expected, pool_id, cancel)?;
            let after = mutable_bytes(&transaction, id, None)?;
            let growth = after.saturating_sub(before);
            ensure!(
                growth
                    <= self.intruder_sessions.quota.saturating_sub(
                        self.intruder_sessions.logical_bytes
                            + self.intruder_sessions.reserved_bytes
                    ),
                "Intruder Session 配额不足；资源池绑定未改变"
            );
            ensure!(!cancel.is_cancelled(), "Intruder 资源池绑定已取消");
            transaction.pragma_update(None, "query_only", true)?;
            transaction.commit()?;
            Ok((run, growth))
        })();
        if operation.is_err() {
            dataset.connection.pragma_update(None, "query_only", true)?;
        }
        let (mut run, growth) = operation?;
        run.retention = IntruderRetention::Session;
        dataset.run = run.clone();
        dataset.logical_bytes += growth;
        self.intruder_sessions.logical_bytes += growth;
        drop(ticket);
        Ok(run)
    }

    pub(in crate::store) fn request_session_intruder_baseline<T>(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        cancel: &CancellationToken,
        prepare: impl FnOnce(IntruderSpec) -> Result<T>,
    ) -> Result<(IntruderRunSummary, T)> {
        let dataset = self
            .intruder_sessions
            .runs
            .get_mut(&id)
            .context("Intruder Session 不存在")?;
        let ticket = self.connection.unchecked_transaction()?;
        validate_ticket(&ticket, self.id, dataset)?;
        validate_pool_binding(&ticket, &dataset.run)?;
        let mut stored = read_run(&dataset.connection, id)?;
        stored.retention = IntruderRetention::Session;
        ensure!(
            stored == dataset.run,
            "Intruder RAM 摘要已改变；未追加基准请求"
        );
        let state_reserve = if dataset.run.state == IntruderRunState::Completed {
            STATE_RESERVE.saturating_sub(dataset.state_reserve)
        } else {
            0
        };
        let before = mutable_bytes(&dataset.connection, id, None)?;
        ensure!(
            dataset
                .connection
                .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))?,
            "Intruder RAM 只读保护已改变"
        );
        dataset
            .connection
            .pragma_update(None, "query_only", false)?;
        let operation = (|| -> Result<(IntruderRunSummary, u64, T)> {
            let transaction = dataset.connection.unchecked_transaction()?;
            let (run, prepared) =
                request_baseline(&transaction, id, expected_revision, cancel, prepare)?;
            let after = mutable_bytes(&transaction, id, None)?;
            let growth = after
                .checked_sub(before)
                .context("Intruder 基准请求字节计量回退")?;
            let available = self.intruder_sessions.quota.saturating_sub(
                self.intruder_sessions.logical_bytes + self.intruder_sessions.reserved_bytes,
            );
            ensure!(
                growth
                    .checked_add(state_reserve)
                    .is_some_and(|needed| needed <= available),
                "Intruder Session 状态余量不足；未追加基准请求"
            );
            ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
            transaction.pragma_update(None, "query_only", true)?;
            transaction.commit()?;
            Ok((run, growth, prepared))
        })();
        if operation.is_err() {
            dataset.connection.pragma_update(None, "query_only", true)?;
        }
        let (mut run, growth, prepared) = operation?;
        run.retention = IntruderRetention::Session;
        if dataset.disk_ticket.is_some() {
            dataset.reactivated_baseline = run.baseline.map(|baseline| baseline.ordinal);
        }
        dataset.run = run.clone();
        dataset.logical_bytes += growth;
        dataset.state_reserve += state_reserve;
        self.intruder_sessions.logical_bytes += growth;
        self.intruder_sessions.reserved_bytes += state_reserve;
        // The read transaction only pins the existing ticket; no fallible work follows RAM commit.
        drop(ticket);
        Ok((run, prepared))
    }
}

pub(super) fn mutable_bytes(
    connection: &Connection,
    id: Uuid,
    ordinal: Option<u64>,
) -> Result<u64> {
    let run: u64 = connection.query_row(
        "SELECT length(CAST(title AS BLOB))+COALESCE(length(CAST(error AS BLOB)),0)+COALESCE(length(CAST(pool_id AS BLOB)),0)+CASE WHEN baseline_ordinal IS NULL THEN 0 ELSE 8 END
         + COALESCE((SELECT sum(CASE typeof(notes) WHEN 'null' THEN 0 ELSE length(CAST(notes AS BLOB)) END + 8 + CASE typeof(highlight) WHEN 'null' THEN 0 ELSE 8 END + 8) FROM intruder_results WHERE run=?1), 0)
         FROM intruder_runs WHERE id=?1", [id.to_string()], |row| row.get(0))?;
    let Some(ordinal) = ordinal else {
        return Ok(run);
    };
    let mut bytes = run;
    for (table, columns, selector) in [
        (
            "intruder_results",
            "run,ordinal,flow,error",
            "run=?1 AND ordinal=?2",
        ),
        (
            "flows",
            "id,revision,started,url,method,state,summary",
            "id=(SELECT flow FROM intruder_results WHERE run=?1 AND ordinal=?2)",
        ),
        (
            "messages",
            "flow,side,head,body",
            "flow=(SELECT flow FROM intruder_results WHERE run=?1 AND ordinal=?2)",
        ),
    ] {
        let sizes = columns.split(',').map(|column| format!("CASE typeof({column}) WHEN 'null' THEN 0 WHEN 'integer' THEN 8 WHEN 'real' THEN 8 ELSE length(CAST({column} AS BLOB)) END")).collect::<Vec<_>>().join("+");
        let size: u64 = connection.query_row(&format!("SELECT COALESCE(sum({sizes}+{ROW_ACCOUNTING_BYTES}),0) FROM {table} WHERE {selector}"), params![id.to_string(), ordinal], |row| row.get(0))?;
        bytes = bytes
            .checked_add(size)
            .context("Intruder RAM 字节计量溢出")?;
    }
    Ok(bytes)
}
