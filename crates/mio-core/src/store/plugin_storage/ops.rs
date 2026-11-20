use anyhow::{Context, Result, ensure};
use rusqlite::{Transaction, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;

use crate::{
    AuditStateSnapshot, FlowId, MAX_AUDIT_FINDINGS, MAX_AUDIT_STATE_BYTES, MAX_AUDIT_TASKS,
    ProjectKey,
    store::{ProjectStore, SCHEMA, audit, metadata, schema_guard},
};

use super::{
    MAX_ENTRIES, MAX_NAMESPACES, MAX_PLUGIN_STORAGE_BYTES, PREFIX, decode_map, namespace,
    namespace_count, read_map, validate_key, validate_plugin_id, validate_value,
};

pub(in crate::store) fn read_audit_state(
    transaction: &Transaction<'_>,
) -> Result<AuditStateSnapshot> {
    let mut query = transaction.prepare(
        "SELECT key, typeof(value),
                CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
         FROM main.metadata WHERE key=?1 COLLATE NOCASE",
    )?;
    let mut rows = query.query(params![audit::KEY, i64::try_from(MAX_AUDIT_STATE_BYTES)?])?;
    let Some(row) = rows.next()? else {
        return Ok(AuditStateSnapshot::default());
    };
    ensure!(
        row.get::<_, String>(0)? == audit::KEY,
        "审计状态目录键编码无效"
    );
    ensure!(
        row.get::<_, String>(1)? == "blob",
        "审计状态必须为二进制 JSON"
    );
    let bytes = row
        .get_ref(2)?
        .as_blob()
        .context("审计状态类型无效或超过编码读取限额")?;
    let state: AuditStateSnapshot =
        serde_json::from_slice(bytes).context("审计状态 JSON 损坏或包含无效字段")?;
    ensure!(rows.next()?.is_none(), "审计状态记录重复");
    ensure!(
        state.tasks.len() <= MAX_AUDIT_TASKS,
        "审计任务数量超过 {MAX_AUDIT_TASKS} 个上限"
    );
    ensure!(
        state.findings.len() <= MAX_AUDIT_FINDINGS,
        "审计告警数量超过 {MAX_AUDIT_FINDINGS} 条上限"
    );
    Ok(state)
}

pub(in crate::store) fn audit_pinned_flows(transaction: &Transaction<'_>) -> Result<Vec<FlowId>> {
    let state = read_audit_state(transaction)?;
    Ok(state
        .findings
        .into_iter()
        .filter_map(|finding| finding.flow_id)
        .collect())
}

pub(in crate::store) fn validate_all(transaction: &Transaction<'_>) -> Result<()> {
    let count = namespace_count(transaction)?;
    ensure!(
        count <= MAX_NAMESPACES,
        "插件存储命名空间数量超过 {MAX_NAMESPACES} 个上限"
    );
    if count == 0 {
        return Ok(());
    }
    let mut statement = transaction.prepare(
        "SELECT key,typeof(value),
                CASE WHEN typeof(value)='blob' AND length(value)<=?1 THEN value END
         FROM main.metadata WHERE key GLOB 'plugin_storage:*' ORDER BY key LIMIT ?2",
    )?;
    let mut rows = statement.query(params![
        i64::try_from(MAX_PLUGIN_STORAGE_BYTES)?,
        i64::try_from(MAX_NAMESPACES)?
    ])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let suffix = key.strip_prefix(PREFIX).context("插件存储键前缀无效")?;
        ensure!(
            row.get::<_, String>(1)? == "blob",
            "插件存储必须为二进制 JSON"
        );
        let bytes = row
            .get_ref(2)?
            .as_blob()
            .context("插件存储类型或长度无效")?;
        decode_map(suffix, bytes)?;
    }
    Ok(())
}

impl ProjectStore {
    pub fn plugin_storage_get(
        &self,
        project: ProjectKey,
        plugin_id: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>> {
        ensure!(project.id == self.id, "插件存储项目归属不匹配");
        validate_plugin_id(plugin_id)?;
        validate_key(key)?;
        let transaction = self.connection.unchecked_transaction()?;
        ensure!(
            metadata::project_id(&transaction)? == project.id && self.id == project.id,
            "插件存储项目归属已改变"
        );
        let Some(map) = read_map(&transaction, plugin_id)? else {
            return Ok(None);
        };
        transaction.commit()?;
        Ok(map.entries.get(key).cloned())
    }

    pub fn plugin_storage_set(
        &mut self,
        project: ProjectKey,
        plugin_id: &str,
        key: &str,
        value: &[u8],
        cancel: &CancellationToken,
    ) -> Result<()> {
        ensure!(project.id == self.id, "插件存储项目归属不匹配");
        validate_plugin_id(plugin_id)?;
        validate_key(key)?;
        validate_value(value)?;
        ensure!(!cancel.is_cancelled(), "插件存储写入已取消");
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(schema == SCHEMA, "项目版本已改变；未写入插件存储");
        ensure!(
            metadata::project_id(&transaction)? == project.id && self.id == project.id,
            "插件存储项目归属已改变"
        );
        schema_guard::validate(&transaction, SCHEMA)?;
        schema_guard::validate_update_dependencies(&transaction, "metadata", &["value"], cancel)?;
        ensure!(!cancel.is_cancelled(), "插件存储写入已取消");
        let mut map = read_map(&transaction, plugin_id)?.unwrap_or_default();
        if !map.entries.contains_key(key) {
            if map.entries.is_empty() {
                ensure!(
                    namespace_count(&transaction)? < MAX_NAMESPACES,
                    "插件存储命名空间已达 {MAX_NAMESPACES} 个上限"
                );
            } else {
                ensure!(
                    map.entries.len() < MAX_ENTRIES,
                    "插件存储键数量已达 {MAX_ENTRIES} 个上限"
                );
            }
        }
        map.entries.insert(key.to_owned(), value.to_vec());
        let encoded = serde_json::to_vec(&map)?;
        ensure!(
            encoded.len() <= MAX_PLUGIN_STORAGE_BYTES,
            "插件存储超过编码限额"
        );
        transaction.execute(
            "INSERT INTO main.metadata(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![namespace(plugin_id)?, encoded.as_slice()],
        )?;
        ensure!(!cancel.is_cancelled(), "插件存储写入已取消");
        transaction.commit()?;
        Ok(())
    }
}
