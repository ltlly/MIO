use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Transaction, params};
use tokio_util::sync::CancellationToken;

use super::{ProjectStore, intruder, metadata};
use crate::{
    IntruderAutoThrottle, IntruderPoolSelection, IntruderResourcePool, IntruderResourcePools,
    MAX_INTRUDER_RESOURCE_POOLS, MAX_INTRUDER_RESOURCE_POOLS_BYTES, ProjectKey, Uuid,
};

pub(super) const KEY: &str = "intruder_resource_pools";

mod tasks;

pub(in crate::store) fn default_pool(id: Uuid) -> IntruderResourcePool {
    IntruderResourcePool {
        retained_inputs: None,
        id,
        name: "默认资源池".into(),
        max_concurrent_requests: Some(10),
        delay: None,
        automatic_throttling: IntruderAutoThrottle {
            enabled: true,
            status_429: true,
            status_503: false,
            other_enabled: false,
            other_codes: String::new(),
        },
    }
}

pub(super) fn read(connection: &Connection) -> Result<IntruderResourcePools> {
    let mut query = connection.prepare(
        "SELECT key,CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
         FROM main.metadata WHERE key=?1",
    )?;
    let mut rows = query.query(params![KEY, MAX_INTRUDER_RESOURCE_POOLS_BYTES])?;
    let Some(row) = rows.next()? else {
        return Ok(IntruderResourcePools::default());
    };
    ensure!(
        row.get::<_, String>(0)? == KEY,
        "Intruder 资源池目录键编码无效"
    );
    let bytes = row
        .get_ref(1)?
        .as_blob()
        .context("Intruder 资源池目录类型或长度无效")?;
    let catalog: IntruderResourcePools =
        serde_json::from_slice(bytes).context("Intruder 资源池目录损坏或含未知配置")?;
    catalog.validate()?;
    ensure!(rows.next()?.is_none(), "Intruder 资源池目录重复");
    Ok(catalog)
}

pub(super) fn write(
    transaction: &Transaction<'_>,
    current: &IntruderResourcePools,
    pools: Vec<IntruderResourcePool>,
    default_pool_id: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<IntruderResourcePools> {
    ensure!(
        current.default_pool_id.is_none() || current.default_pool_id == default_pool_id,
        "Intruder 默认资源池身份不能移除或替换"
    );
    ensure!(
        current
            .default_pool_id
            .is_none_or(|id| pools.iter().any(|pool| pool.id == id)),
        "Intruder 默认资源池不能删除或用同名资源池替换"
    );
    let next = IntruderResourcePools {
        revision: current
            .revision
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .context("Intruder 资源池目录 revision 已耗尽")?,
        pools,
        default_pool_id,
    };
    next.validate()?;
    let encoded = serde_json::to_vec(&next)?;
    ensure!(
        encoded.len() <= MAX_INTRUDER_RESOURCE_POOLS_BYTES,
        "Intruder 资源池目录超过编码限额"
    );
    ensure!(!cancel.is_cancelled(), "Intruder 资源池变更已取消");
    let present: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.metadata WHERE key=?1)",
        [KEY],
        |row| row.get(0),
    )?;
    let changed = if present {
        transaction.execute(
            "UPDATE main.metadata SET value=?1 WHERE key=?2",
            params![encoded, KEY],
        )?
    } else {
        transaction.execute(
            "INSERT INTO main.metadata(key,value) VALUES(?1,?2)",
            params![KEY, encoded],
        )?
    };
    ensure!(changed == 1, "Intruder 资源池目录写入冲突");
    ensure!(!cancel.is_cancelled(), "Intruder 资源池变更已取消");
    Ok(next)
}

pub(super) fn resolve_start(
    transaction: &Transaction<'_>,
    selection: Option<&IntruderPoolSelection>,
    cancel: &CancellationToken,
) -> Result<(Option<Uuid>, IntruderResourcePools)> {
    let catalog = read(transaction)?;
    let Some(selection) = selection else {
        return Ok((None, catalog));
    };
    selection.validate()?;
    match selection {
        IntruderPoolSelection::Existing(id) => {
            ensure!(
                catalog.find(*id).is_some(),
                "Intruder 资源池不存在；未开始请求"
            );
            Ok((Some(*id), catalog))
        }
        IntruderPoolSelection::Create(pool) => {
            ensure!(
                catalog.find(pool.id).is_none(),
                "Intruder 新资源池 ID 已存在；未复用或重复创建"
            );
            let mut pools = catalog.pools.clone();
            pools.push(pool.clone());
            let catalog = write(
                transaction,
                &catalog,
                pools,
                catalog.default_pool_id,
                cancel,
            )?;
            Ok((Some(pool.id), catalog))
        }
    }
}

impl ProjectStore {
    pub fn ensure_intruder_default_pool(
        &mut self,
        project: ProjectKey,
        cancel: &CancellationToken,
    ) -> Result<IntruderResourcePools> {
        ensure!(project.id == self.id, "Intruder 默认资源池项目归属不匹配");
        ensure!(!cancel.is_cancelled(), "Intruder 默认资源池建立已取消");
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            metadata::project_id(&transaction)? == project.id,
            "Intruder 默认资源池项目归属已改变"
        );
        let current = read(&transaction)?;
        if current.default_pool_id.is_some() {
            ensure!(!cancel.is_cancelled(), "Intruder 默认资源池确认已取消");
            transaction.commit()?;
            return Ok(current);
        }
        ensure!(
            current.pools.len() < MAX_INTRUDER_RESOURCE_POOLS,
            "Intruder 资源池已达 64 个；默认资源池未建立，原目录保留"
        );
        let id = Uuid::new_v4();
        ensure!(
            current.find(id).is_none(),
            "Intruder 默认资源池 ID 冲突；原目录保留"
        );
        let mut pools = current.pools.clone();
        pools.insert(0, default_pool(id));
        let next = write(&transaction, &current, pools, Some(id), cancel)?;
        transaction.commit()?;
        Ok(next)
    }

    pub fn intruder_resource_pools(&self) -> Result<IntruderResourcePools> {
        let transaction = self.connection.unchecked_transaction()?;
        ensure!(
            metadata::project_id(&transaction)? == self.id,
            "Intruder 资源池项目归属已改变"
        );
        read(&transaction)
    }

    pub fn set_intruder_resource_pools(
        &mut self,
        project: ProjectKey,
        expected_revision: u64,
        pools: Vec<IntruderResourcePool>,
        cancel: &CancellationToken,
    ) -> Result<IntruderResourcePools> {
        ensure!(project.id == self.id, "Intruder 资源池项目归属不匹配");
        ensure!(!cancel.is_cancelled(), "Intruder 资源池变更已取消");
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            metadata::project_id(&transaction)? == project.id,
            "Intruder 资源池项目归属已改变"
        );
        let current = read(&transaction)?;
        ensure!(
            current.revision == expected_revision,
            "Intruder 资源池目录 revision 已改变；未覆盖"
        );
        let removed: BTreeSet<_> = current
            .pools
            .iter()
            .map(|pool| pool.id)
            .filter(|id| !pools.iter().any(|pool| pool.id == *id))
            .collect();
        if !removed.is_empty() {
            intruder::ensure_resource_pools_unreferenced(
                &transaction,
                &self.intruder_sessions,
                &removed,
                cancel,
            )?;
        }
        let next = write(
            &transaction,
            &current,
            pools,
            current.default_pool_id,
            cancel,
        )?;
        transaction.commit()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests;
