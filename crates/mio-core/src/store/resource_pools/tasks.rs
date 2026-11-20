use std::collections::BTreeMap;

use super::*;
use crate::{IntruderPoolDeleteError, IntruderPoolDeleteReceipt, IntruderPoolUsage, Workspace};

fn usage(
    transaction: &Transaction<'_>,
    sessions: &intruder::IntruderSessions,
    cancel: &CancellationToken,
) -> Result<IntruderPoolUsage> {
    let catalog = read(transaction)?;
    let mut usage = IntruderPoolUsage {
        catalog_revision: catalog.revision,
        task_counts: catalog
            .pools
            .iter()
            .map(|pool| (pool.id, 0))
            .collect::<BTreeMap<_, _>>(),
        requires_confirmation: BTreeSet::new(),
    };
    let mut statement = transaction.prepare(
        "SELECT CASE WHEN typeof(pool_id)='text' AND length(CAST(pool_id AS BLOB))=36 THEN pool_id END,count(*) FROM intruder_runs WHERE pool_id IS NOT NULL GROUP BY pool_id LIMIT 65",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "Intruder 资源池查询已取消");
        let value: String = row.get(0)?;
        let id = Uuid::parse_str(&value)?;
        ensure!(
            !id.is_nil() && id.to_string() == value,
            "Intruder 资源池引用编码无效"
        );
        let count: u64 = row.get(1)?;
        *usage
            .task_counts
            .get_mut(&id)
            .context("Intruder 当前资源池缺失")? = count;
        usage.requires_confirmation.insert(id);
    }
    sessions.add_pool_usage(transaction, &mut usage, cancel)?;
    let workspace = metadata::workspace_for_update(transaction)?;
    for tab in workspace
        .intruder
        .tabs
        .iter()
        .chain(&workspace.intruder.closed)
    {
        ensure!(!cancel.is_cancelled(), "Intruder 资源池查询已取消");
        if let Some(selection) = &tab.spec.resource_pool
            && usage.task_counts.contains_key(&selection.id())
        {
            usage.requires_confirmation.insert(selection.id());
        }
    }
    Ok(usage)
}

fn reassigned_workspace(
    mut workspace: Workspace,
    from: Uuid,
    to: Option<Uuid>,
) -> Result<(Workspace, u64)> {
    let mut count = 0;
    for tab in workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut workspace.intruder.closed)
    {
        if tab
            .spec
            .resource_pool
            .as_ref()
            .is_some_and(|pool| pool.id() == from)
        {
            let to = to.context("Intruder 默认资源池缺失；未改变配置")?;
            tab.spec.resource_pool = Some(IntruderPoolSelection::Existing(to));
            tab.spec.revision = tab
                .spec
                .revision
                .checked_add(1)
                .filter(|n| *n <= i64::MAX as u64)
                .context("Intruder 配置 revision 已耗尽")?;
            count += 1;
        }
    }
    if count != 0 {
        workspace.revision = workspace
            .revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .context("工作区 revision 已耗尽")?;
    }
    workspace.validate()?;
    Ok((workspace, count))
}

fn apply_delete(
    transaction: &Transaction<'_>,
    catalog: &IntruderResourcePools,
    pool_id: Uuid,
    workspace: &Workspace,
    cancel: &CancellationToken,
) -> Result<IntruderPoolDeleteReceipt> {
    let (workspace, reassigned_tabs) =
        reassigned_workspace(workspace.clone(), pool_id, catalog.default_pool_id)?;
    let reassigned_runs = match catalog.default_pool_id {
        Some(default) => intruder::reassign_project_pool(transaction, pool_id, default, cancel)?,
        None => 0,
    };
    if reassigned_tabs != 0 {
        super::super::schema_guard::validate_update_dependencies(
            transaction,
            "metadata",
            &["value"],
            cancel,
        )?;
        super::super::write_workspace(transaction, &workspace)?;
    }
    let pools = catalog
        .pools
        .iter()
        .filter(|pool| pool.id != pool_id)
        .cloned()
        .collect();
    let catalog = write(transaction, catalog, pools, catalog.default_pool_id, cancel)?;
    Ok(IntruderPoolDeleteReceipt {
        catalog,
        workspace,
        reassigned_runs,
        reassigned_tabs,
    })
}

impl ProjectStore {
    pub(crate) fn intruder_pool_usage(
        &self,
        cancel: &CancellationToken,
    ) -> Result<IntruderPoolUsage> {
        let transaction = self.connection.unchecked_transaction()?;
        ensure!(
            metadata::project_id(&transaction)? == self.id,
            "Intruder 项目归属已改变"
        );
        usage(&transaction, &self.intruder_sessions, cancel)
    }

    pub(crate) fn delete_intruder_resource_pool(
        &mut self,
        project: ProjectKey,
        pool_id: Uuid,
        expected_revisions: (u64, u64),
        allow_reassign: bool,
        has_in_flight: bool,
        cancel: &CancellationToken,
    ) -> std::result::Result<IntruderPoolDeleteReceipt, IntruderPoolDeleteError> {
        let (expected_catalog_revision, expected_workspace_revision) = expected_revisions;
        let mut changed_sessions = 0;
        let mut default_created = false;
        let mut confirmed_catalog = None;
        let mut confirmed_workspace = None;
        let result = (|| -> Result<IntruderPoolDeleteReceipt> {
            ensure!(
                project.id == self.id && !pool_id.is_nil(),
                "Intruder 资源池项目或身份无效"
            );
            ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
            self.intruder_sessions.check_pool_delete_owner(project)?;
            let preview = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            ensure!(
                metadata::project_id(&preview)? == project.id,
                "Intruder 项目归属已改变"
            );
            let mut catalog = read(&preview)?;
            confirmed_catalog = Some(catalog.clone());
            ensure!(
                catalog.revision == expected_catalog_revision,
                "Intruder 资源池目录 revision 已改变；未删除"
            );
            ensure!(catalog.find(pool_id).is_some(), "Intruder 资源池不存在");
            ensure!(
                catalog.default_pool_id != Some(pool_id),
                "Intruder 默认资源池不能删除"
            );
            let workspace = metadata::workspace_for_update(&preview)?;
            confirmed_workspace = Some(workspace.clone());
            ensure!(
                workspace.revision == expected_workspace_revision,
                "工作区 revision 已改变；未删除资源池"
            );
            let used = usage(&preview, &self.intruder_sessions, cancel)?
                .requires_confirmation
                .contains(&pool_id)
                || has_in_flight;
            ensure!(
                !used || allow_reassign,
                "资源池已有任务或配置引用；请确认改绑默认池后删除"
            );
            let initialize = used && catalog.default_pool_id.is_none();
            if initialize {
                ensure!(
                    catalog.pools.len() < MAX_INTRUDER_RESOURCE_POOLS,
                    "资源池已达上限；无法初始化默认池，未删除"
                );
                let id = Uuid::new_v4();
                let mut pools = catalog.pools.clone();
                pools.insert(0, default_pool(id));
                catalog = write(&preview, &catalog, pools, Some(id), cancel)?;
            }
            // Exercise every main-database constraint before the first separate Session commit.
            apply_delete(&preview, &catalog, pool_id, &workspace, cancel)?;
            if let Some(default) = catalog.default_pool_id {
                self.intruder_sessions
                    .preflight_pool_delete(&preview, pool_id, default, cancel)?;
            }
            preview.rollback()?;
            if initialize {
                let transaction = self
                    .connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                ensure!(
                    metadata::project_id(&transaction)? == project.id,
                    "Intruder 项目归属已改变；未创建默认池"
                );
                let current = read(&transaction)?;
                ensure!(
                    current.revision == expected_catalog_revision,
                    "Intruder 资源池目录已改变；未创建默认池"
                );
                write(
                    &transaction,
                    &current,
                    catalog.pools.clone(),
                    catalog.default_pool_id,
                    cancel,
                )?;
                ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
                transaction.commit()?;
                default_created = true;
                confirmed_catalog = Some(catalog.clone());
            }
            let transaction = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            ensure!(
                metadata::project_id(&transaction)? == project.id && read(&transaction)? == catalog,
                "Intruder 资源池目录已改变；未删除"
            );
            ensure!(
                metadata::workspace_for_update(&transaction)? == workspace,
                "工作区已改变；未删除资源池"
            );
            let mut receipt = apply_delete(&transaction, &catalog, pool_id, &workspace, cancel)?;
            if let Some(default) = catalog.default_pool_id {
                self.intruder_sessions.reassign_deleted_pool(
                    &transaction,
                    pool_id,
                    default,
                    cancel,
                    &mut changed_sessions,
                )?;
            }
            ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
            transaction.commit()?;
            receipt.reassigned_runs += changed_sessions;
            Ok(receipt)
        })();
        result.map_err(|error| IntruderPoolDeleteError {
            message: if changed_sessions > 0 || default_created {
                format!("资源池删除未完成：{error}；已改绑 {changed_sessions} 个 Session，默认池初始化：{default_created}；请核对实际状态后重试。")
            } else {
                format!("资源池未删除：{error}")
            },
            reassigned_sessions: changed_sessions,
            default_pool_created: default_created,
            // Global shutdown can interrupt fresh SQL reads after a committed Session/default.
            catalog: self.intruder_resource_pools().ok().or(confirmed_catalog),
            workspace: self.workspace().ok().or(confirmed_workspace).map(Box::new),
        })
    }
}
