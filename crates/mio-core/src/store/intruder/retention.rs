use super::super as store;
use super::*;
use crate::{FlowId, IntruderRetention, IntruderRetentionReceipt, OriginalMessages, ProjectKey};

mod convert;
#[path = "pool_delete.rs"]
mod pool_delete;
mod project;
#[path = "session.rs"]
mod session;
mod session_views;

#[cfg(test)]
mod tests;

pub(in crate::store::intruder) use convert::strict_json;
use convert::{
    copy_rows, dataset_logical_bytes, ensure_no_run_rows, has_flow, retirement_bytes, same_flow,
    same_rows, select, session_connection, ticket_bytes, validate_dataset,
    validate_dataset_snapshot, validate_ticket, validate_workspace_links, visit_flows,
};

// MIO logical SQL-data limits, not External defaults or a process RSS guarantee.
const SESSION_LOGICAL_QUOTA: u64 = 64 * 1024 * 1024;
const MAX_SESSION_DATASETS: usize = 64;
const OWNERSHIP_BYTES: u64 = 256;
const ROW_ACCOUNTING_BYTES: u64 = 64;

// The selectors include all message versions, not only the current request and response.
const DATA: &[(&str, &str, &str)] = &[
    (
        "intruder_runs",
        "id,tab,revision,spec_revision,total,dispatched,completed,state,paused,error,started,spec_meta,template,title,title_revision,baseline_ordinal,pool_id,pool_revision",
        "id=?1",
    ),
    (
        "intruder_payloads",
        "run,set_index,ordinal,value",
        "run=?1 ORDER BY set_index,ordinal",
    ),
    (
        "flows",
        "id,revision,started,url,method,state,summary",
        "id IN (SELECT flow FROM main.intruder_results WHERE run=?1) ORDER BY id",
    ),
    (
        "messages",
        "flow,side,head,body",
        "flow IN (SELECT flow FROM main.intruder_results WHERE run=?1) ORDER BY flow,side",
    ),
    (
        "intruder_results",
        "run,ordinal,flow,error,notes,notes_revision,highlight,highlight_revision",
        "run=?1 ORDER BY ordinal",
    ),
];

struct SessionDataset {
    connection: Connection,
    owner: ProjectKey,
    run: IntruderRunSummary,
    logical_bytes: u64,
    // Only these exact pre-existing disk identities may be reused during promotion.
    disk_flows: BTreeSet<FlowId>,
    disk_ticket: Option<(u64, i64)>,
    reservations: BTreeMap<u64, session::Reservation>,
    state_reserve: u64,
    // A ticketed snapshot can only be reactivated by its explicit one-time baseline admission.
    reactivated_baseline: Option<u64>,
}

pub(in crate::store) struct IntruderSessions {
    runs: BTreeMap<Uuid, SessionDataset>,
    logical_bytes: u64,
    quota: u64,
    reserved_bytes: u64,
    #[cfg(test)]
    fail_pool_delete_commit: Option<Uuid>,
}

impl Default for IntruderSessions {
    fn default() -> Self {
        Self {
            runs: BTreeMap::new(),
            logical_bytes: 0,
            quota: SESSION_LOGICAL_QUOTA,
            reserved_bytes: 0,
            #[cfg(test)]
            fail_pool_delete_commit: None,
        }
    }
}

impl IntruderSessions {
    pub(in crate::store) fn contains(&self, id: Uuid) -> bool {
        self.runs.contains_key(&id)
    }

    fn remove(&mut self, id: Uuid) {
        let removed = self.runs.remove(&id).expect("Session dataset is owned");
        self.logical_bytes -= removed.logical_bytes;
        self.reserved_bytes -= removed.state_reserve
            + removed
                .reservations
                .values()
                .map(|reservation| reservation.bytes)
                .sum::<u64>();
    }
}

pub(in crate::store) fn ensure_resource_pools_unreferenced(
    connection: &Transaction<'_>,
    sessions: &IntruderSessions,
    removed: &BTreeSet<Uuid>,
    cancel: &CancellationToken,
) -> Result<()> {
    let workspace = store::metadata::workspace_for_update(connection)?;
    for tab in workspace
        .intruder
        .tabs
        .iter()
        .chain(&workspace.intruder.closed)
    {
        ensure!(
            tab.spec
                .resource_pool
                .as_ref()
                .is_none_or(|selection| !removed.contains(&selection.id())),
            "Intruder 资源池仍被工作标签引用；未删除"
        );
    }
    ensure_no_pool_references(connection, None, removed, cancel)?;
    for dataset in sessions.runs.values() {
        validate_ticket(connection, dataset.owner.id, dataset)?;
        ensure_no_pool_references(&dataset.connection, Some(dataset.run.id), removed, cancel)?;
    }
    Ok(())
}

impl ProjectStore {
    pub fn rename_intruder_run(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_title_revision: u64,
        title: &str,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.check_intruder_session_owner(project, id)?;
        ensure!(
            title.len() <= MAX_TITLE_BYTES,
            "Intruder 任务名称超过 4096 字节；名称未改变"
        );
        ensure!(!cancel.is_cancelled(), "Intruder 重命名已取消；名称未改变");
        let Some(dataset) = self.intruder_sessions.runs.get_mut(&id) else {
            let transaction = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            ensure!(
                store::metadata::project_id(&transaction)? == project.id,
                "Intruder 项目归属已改变；名称未改变"
            );
            let run = rename_run(&transaction, id, expected_title_revision, title, cancel)?;
            transaction.commit()?;
            return Ok(run);
        };
        let ticket = self.connection.unchecked_transaction()?;
        validate_ticket(&ticket, self.id, dataset)?;
        ticket.commit()?;
        let mut stored = read_run(&dataset.connection, id)?;
        stored.retention = IntruderRetention::Session;
        ensure!(stored == dataset.run, "Intruder RAM 摘要已改变；名称未改变");
        let previous_bytes = u64::try_from(dataset.run.title.len())?;
        let new_bytes = u64::try_from(title.len())?;
        let logical_bytes = dataset
            .logical_bytes
            .checked_sub(previous_bytes)
            .and_then(|bytes| bytes.checked_add(new_bytes))
            .context("Intruder Session 名称配额计量溢出")?;
        let total_bytes = self
            .intruder_sessions
            .logical_bytes
            .checked_sub(dataset.logical_bytes)
            .and_then(|bytes| bytes.checked_add(logical_bytes))
            .context("Intruder Session 总配额计量溢出")?;
        ensure!(
            total_bytes
                .checked_add(self.intruder_sessions.reserved_bytes)
                .is_some_and(|bytes| bytes <= self.intruder_sessions.quota),
            "Intruder Session 逻辑 RAM 数据配额不足；名称未改变"
        );
        let query_only: bool =
            dataset
                .connection
                .pragma_query_value(None, "query_only", |row| row.get(0))?;
        ensure!(query_only, "Intruder Session 只读保护已改变；名称未改变");
        dataset
            .connection
            .pragma_update(None, "query_only", false)?;
        let renamed = (|| -> Result<IntruderRunSummary> {
            let transaction = dataset.connection.unchecked_transaction()?;
            let run = rename_run(&transaction, id, expected_title_revision, title, cancel)?;
            // Restore read-only protection before publishing; COMMIT is allowed in query-only mode.
            transaction.pragma_update(None, "query_only", true)?;
            transaction.commit()?;
            Ok(run)
        })();
        if renamed.is_err() {
            dataset
                .connection
                .pragma_update(None, "query_only", true)
                .context("Intruder 重命名失败，且 Session 只读保护未恢复")?;
        }
        let mut run = renamed?;
        run.retention = IntruderRetention::Session;
        dataset.run = run.clone();
        dataset.logical_bytes = logical_bytes;
        self.intruder_sessions.logical_bytes = total_bytes;
        Ok(run)
    }

    pub(in crate::store) fn validate_session_snapshot(&self, reopened: &Self) -> Result<()> {
        if self.intruder_sessions.runs.is_empty() {
            return Ok(());
        }
        let source = self.connection.unchecked_transaction()?;
        let destination = reopened.connection.unchecked_transaction()?;
        ensure!(
            store::resource_pools::read(&source)? == store::resource_pools::read(&destination)?,
            "另存为的 Intruder 资源池目录已改变；Session 数据保留"
        );
        for dataset in self.intruder_sessions.runs.values() {
            validate_pool_binding(&source, &dataset.run)?;
            validate_ticket(&source, self.id, dataset)?;
            validate_ticket(&destination, self.id, dataset)?;
            visit_flows(&dataset.connection, dataset.run.id, |flow| {
                ensure!(
                    same_flow(&source, &destination, flow, true)?,
                    "另存为的 Intruder 共享流量或 ID 归属已改变；原项目与 Session 数据保留"
                );
                Ok(())
            })?;
        }
        destination.commit()?;
        source.commit()?;
        Ok(())
    }

    /// New/Open must refuse before replacing a store that still owns Session datasets.
    pub fn require_intruder_sessions_resolved(&self) -> Result<()> {
        ensure!(
            self.intruder_sessions.runs.is_empty(),
            "仍有 Intruder Session 数据；请显式保存到项目或丢弃后再新建/打开项目（普通保存不会转换）"
        );
        Ok(())
    }

    #[cfg(test)]
    pub fn set_intruder_session_quota(&mut self, bytes: u64) {
        assert!(self.intruder_sessions.runs.is_empty());
        self.intruder_sessions.quota = bytes;
    }

    pub fn check_intruder_session_owner(&self, project: ProjectKey, id: Uuid) -> Result<()> {
        ensure!(project.id == self.id, "Intruder 项目归属不匹配；数据保留");
        if let Some(dataset) = self.intruder_sessions.runs.get(&id) {
            ensure!(
                dataset.owner == project,
                "Intruder Session generation 或项目归属已改变"
            );
        }
        Ok(())
    }

    /// Promotion permits a healthy Running/Paused Session snapshot; reverse transfer remains terminal.
    /// The caller excludes pending recovery and validates active request ownership and ProjectKey.
    pub fn set_intruder_retention(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        retention: IntruderRetention,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderRetentionReceipt> {
        self.check_intruder_session_owner(project, id)?;
        ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
        match retention {
            IntruderRetention::Session => {
                ensure!(
                    !self.intruder_sessions.contains(id),
                    "Intruder 已在 Session；未重复应用工作区修改"
                );
                self.move_intruder_to_session(project, id, expected_revision, workspace, cancel)
            }
            IntruderRetention::Project => {
                ensure!(
                    self.intruder_sessions.contains(id),
                    "Intruder 不在 Session；未重复应用工作区修改"
                );
                self.promote_session_intruder(id, expected_revision, workspace, cancel)
            }
        }
    }
}
