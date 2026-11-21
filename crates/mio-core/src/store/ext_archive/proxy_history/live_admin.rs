use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, Row, params, types::ValueRef};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::*;

const MAX_LOGGER_PAGE_LIMIT: usize = 200;
const MAX_LOGGER_PAGE_SUMMARY_BYTES: usize = 1024 * 1024;
const LOGGER_QUERY_BUDGET: Duration = Duration::from_secs(5);

impl ProjectStore {
    pub fn logger_history_page(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
        source: Option<&TrafficSource>,
        cancel: &CancellationToken,
    ) -> Result<(usize, Vec<FlowSummary>)> {
        ensure!(
            (1..=MAX_LOGGER_PAGE_LIMIT).contains(&limit),
            "Logger 分页大小必须为 1–200"
        );
        ensure!(filter.len() <= 4096, "Logger 筛选文本过长");
        if let Some(TrafficSource::Plugin(name)) = source {
            ensure!(name.len() <= 4096, "Logger 插件来源筛选过长");
        }
        let database_offset = i64::try_from(offset).context("Logger 分页偏移溢出")?;
        database_offset
            .checked_add(limit as i64)
            .context("Logger 分页范围溢出")?;
        check_logger_cancel(cancel, Instant::now() + LOGGER_QUERY_BUDGET)?;
        let deadline = Instant::now() + LOGGER_QUERY_BUDGET;
        let interrupt = cancel.clone();
        self.connection.progress_handler(
            1000,
            Some(move || interrupt.is_cancelled() || Instant::now() >= deadline),
        );
        let result = (|| -> Result<(usize, Vec<FlowSummary>)> {
            let transaction = self.connection.unchecked_transaction()?;
            let invalid: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM flows WHERE typeof(summary)!='blob' OR length(summary)>?1
                    OR NOT json_valid(CAST(summary AS TEXT)) OR json_type(CAST(summary AS TEXT))!='object'
                    OR EXISTS(SELECT 1 FROM json_tree(CAST(summary AS TEXT))
                        WHERE typeof(key)='text' GROUP BY parent,key HAVING count(*)>1)
                    OR url != coalesce(json_extract(CAST(summary AS TEXT),'$.url'), '')
                    OR method != coalesce(json_extract(CAST(summary AS TEXT),'$.method'), '')
                    OR typeof(started)!='integer' OR started != json_extract(CAST(summary AS TEXT),'$.started_unix_ms')
                    OR state IS NOT CASE json_type(CAST(summary AS TEXT),'$.state')
                        WHEN 'text' THEN lower(json_extract(CAST(summary AS TEXT),'$.state'))
                        WHEN 'object' THEN CASE WHEN json_type(CAST(summary AS TEXT),'$.state.Failed')='text' THEN 'failed' END
                        END
                    OR CASE json_type(CAST(summary AS TEXT),'$.source')
                        WHEN 'text' THEN json_extract(CAST(summary AS TEXT),'$.source') NOT IN ('Proxy','Repeater','Intruder','Scanner')
                        WHEN 'object' THEN json_type(CAST(summary AS TEXT),'$.source.Plugin')!='text'
                            OR length(trim(json_extract(CAST(summary AS TEXT),'$.source.Plugin')))=0
                        ELSE 1 END
                    OR (json_type(CAST(summary AS TEXT),'$.status') NOT IN ('null','integer'))
                    OR (json_type(CAST(summary AS TEXT),'$.status')='integer'
                        AND json_extract(CAST(summary AS TEXT),'$.status') NOT BETWEEN 100 AND 999))",
                [MAX_SUMMARY_BYTES as i64],
                |row| row.get(0),
            )?;
            ensure!(
                !invalid,
                "Logger 流量摘要 JSON 损坏或包含重复字段；未省略无法分类的记录"
            );
            let needle = filter.to_lowercase();
            let mut query = if let Some(src) = source {
                let code = crate::store::messages::source_code(src);
                let mut stmt = transaction.prepare(
                    "SELECT id,revision,started,url,method,state,summary FROM flows WHERE source=?1 ORDER BY started DESC,id",
                )?;
                stmt.raw_bind_parameter(1, code)?;
                stmt
            } else {
                transaction.prepare(
                    "SELECT id,revision,started,url,method,state,summary FROM flows ORDER BY started DESC,id",
                )?
            };
            let mut rows = query.raw_query();
            let mut total = 0usize;
            let mut page = Vec::with_capacity(limit);
            let mut page_bytes = 0usize;
            let mut full = false;
            while let Some(row) = rows.next()? {
                check_logger_cancel(cancel, deadline)?;
                let (summary, bytes_len) = read_logger_flow(row)?;
                if !matches_logger_source(&summary.source, source)
                    || !matches_logger_filter(&summary, &needle)
                {
                    continue;
                }
                let index = total;
                total = total.checked_add(1).context("Logger 条目计数溢出")?;
                if index >= offset && !full && page.len() < limit {
                    let entry_bytes = bytes_len.saturating_add(32);
                    if page_bytes.saturating_add(entry_bytes) > MAX_LOGGER_PAGE_SUMMARY_BYTES {
                        full = true;
                    } else {
                        page_bytes += entry_bytes;
                        page.push(summary);
                    }
                }
            }
            ensure!(
                !page.is_empty() || offset >= total,
                "Logger 分页未取得完整摘要；未省略后续条目"
            );
            check_logger_cancel(cancel, deadline)?;
            drop(rows);
            drop(query);
            transaction.commit()?;
            Ok((total, page))
        })();
        self.connection.progress_handler(0, None::<fn() -> bool>);
        match result {
            Err(error)
                if cancel.is_cancelled()
                    || Instant::now() >= deadline
                    || matches!(
                        error.downcast_ref::<rusqlite::Error>(),
                        Some(rusqlite::Error::SqliteFailure(code, _))
                            if code.code == rusqlite::ErrorCode::OperationInterrupted
                    ) =>
            {
                check_logger_cancel(cancel, deadline)?;
                anyhow::bail!("Logger 历史查询已中断")
            }
            other => other,
        }
    }

    pub fn delete_live_proxy_flows(&mut self, flow_ids: &[FlowId]) -> Result<Vec<FlowId>> {
        ensure!(
            !flow_ids.is_empty() && flow_ids.len() <= 500,
            "单次删除 HTTP 历史记录数量须在 1..=500 之间"
        );
        let mut unique_ids = Vec::with_capacity(flow_ids.len());
        let mut seen = std::collections::HashSet::with_capacity(flow_ids.len());
        for &flow_id in flow_ids {
            ensure!(!flow_id.is_nil(), "HTTP 历史记录 ID 不能为空");
            self.reject_session_intruder_flow_write(flow_id)?;
            if seen.insert(flow_id) {
                unique_ids.push(flow_id);
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::store::schema_guard::validate(&tx)?;
        let workspace = crate::store::metadata::workspace(&tx)?;
        let pinned_flows: std::collections::HashSet<FlowId> = workspace
            .tabs
            .iter()
            .chain(&workspace.closed)
            .filter_map(|tab| tab.last_flow)
            .collect();
        {
            let mut check = tx.prepare(
                "SELECT
                    CASE WHEN typeof(f.summary) = 'blob' AND length(f.summary) <= ?2 THEN f.summary END,
                    f.state NOT IN ('pending', 'intercepted'),
                    EXISTS(SELECT 1 FROM main.intruder_results WHERE flow = ?1)
                        OR EXISTS(SELECT 1 FROM main.repeater_history WHERE flow = ?1)
                        OR EXISTS(SELECT 1 FROM main.websocket_messages WHERE flow = ?1)
                        OR EXISTS(SELECT 1 FROM main.websocket_connections WHERE flow = ?1)
                 FROM main.flows f
                 WHERE f.id = ?1",
            )?;
            for &flow_id in &unique_ids {
                let (summary_blob, is_terminal, is_referenced): (Option<Vec<u8>>, bool, bool) =
                    check
                        .query_row(
                            params![flow_id.to_string(), MAX_SUMMARY_BYTES as i64],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?
                        .with_context(|| format!("HTTP 历史记录 {flow_id} 不存在或属于只读归档"))?;
                let summary_bytes = summary_blob
                    .with_context(|| format!("HTTP 历史记录 {flow_id} 摘要损坏或超过 1 MiB"))?;
                let summary: FlowSummary =
                    serde_json::from_slice::<StrictFlowSummary>(&summary_bytes)
                        .with_context(|| format!("HTTP 历史记录 {flow_id} 摘要 JSON 损坏"))?
                        .into();
                ensure!(
                    summary.id == flow_id && summary.source == TrafficSource::Proxy,
                    "记录 {flow_id} 不是可删除的 Proxy 实时记录"
                );
                ensure!(
                    is_terminal
                        && !matches!(summary.state, FlowState::Pending | FlowState::Intercepted),
                    "HTTP 历史记录 {flow_id} 仍在传输或拦截中，不能删除"
                );
                ensure!(
                    !pinned_flows.contains(&flow_id) && !is_referenced,
                    "HTTP 历史记录 {flow_id} 仍被 Repeater、Intruder 或 WebSocket 引用，不能删除"
                );
            }
        }
        {
            let mut delete_messages = tx.prepare("DELETE FROM main.messages WHERE flow = ?1")?;
            let mut delete_flow = tx.prepare("DELETE FROM main.flows WHERE id = ?1")?;
            for &flow_id in &unique_ids {
                let key = flow_id.to_string();
                delete_messages.execute([&key])?;
                let deleted = delete_flow.execute([&key])?;
                ensure!(deleted == 1, "删除 HTTP 历史记录 {flow_id} 数量异常");
            }
        }
        crate::store::plugin_storage::remove_live_http_annotations(&tx, &unique_ids)?;
        crate::store::provenance::remove_flow_provenance(&tx, &unique_ids)?;
        {
            let mut delete_meta =
                tx.prepare("DELETE FROM main.metadata WHERE key = ?1 COLLATE BINARY")?;
            for &flow_id in &unique_ids {
                delete_meta.execute([format!(
                    "{}{flow_id}",
                    crate::store::messages::HTTP_HISTORY_META_PREFIX
                )])?;
            }
        }
        tx.commit()?;
        Ok(unique_ids)
    }

    pub fn clear_live_proxy_history(&mut self) -> Result<Vec<FlowId>> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        crate::store::schema_guard::validate(&tx)?;
        let workspace = crate::store::metadata::workspace(&tx)?;
        let pinned_flows: std::collections::HashSet<FlowId> = workspace
            .tabs
            .iter()
            .chain(&workspace.closed)
            .filter_map(|tab| tab.last_flow)
            .collect();
        let mut deleted_ids = Vec::new();
        {
            let mut query = tx.prepare(
                "SELECT f.id, f.summary
                 FROM main.flows f
                 WHERE f.source = 'Proxy'
                   AND typeof(f.id) = 'text'
                   AND length(CAST(f.id AS BLOB)) = 36
                   AND typeof(f.summary) = 'blob'
                   AND length(f.summary) <= ?1
                   AND json_valid(CAST(f.summary AS TEXT))
                   AND json_extract(CAST(f.summary AS TEXT), '$.source') = 'Proxy'
                   AND f.state NOT IN ('pending', 'intercepted')
                   AND NOT EXISTS(SELECT 1 FROM main.intruder_results WHERE flow = f.id)
                   AND NOT EXISTS(SELECT 1 FROM main.repeater_history WHERE flow = f.id)
                   AND NOT EXISTS(SELECT 1 FROM main.websocket_messages WHERE flow = f.id)
                   AND NOT EXISTS(SELECT 1 FROM main.websocket_connections WHERE flow = f.id)
                 ORDER BY f.started DESC, f.id",
            )?;
            let mut rows = query.query([MAX_SUMMARY_BYTES as i64])?;
            while let Some(row) = rows.next()? {
                let raw_id: String = row.get(0)?;
                let summary_bytes: Vec<u8> = row.get(1)?;
                let flow_id = Uuid::parse_str(&raw_id).context("HTTP 历史记录 ID 损坏")?;
                let summary: FlowSummary =
                    serde_json::from_slice::<StrictFlowSummary>(&summary_bytes)
                        .with_context(|| format!("HTTP 历史记录 {flow_id} 摘要 JSON 损坏"))?
                        .into();
                ensure!(
                    summary.id == flow_id
                        && summary.source == TrafficSource::Proxy
                        && !matches!(summary.state, FlowState::Pending | FlowState::Intercepted),
                    "HTTP 历史记录 {flow_id} 摘要状态或归属不一致"
                );
                if !pinned_flows.contains(&flow_id) {
                    self.reject_session_intruder_flow_write(flow_id)?;
                    deleted_ids.push(flow_id);
                }
            }
        }
        {
            let mut delete_messages = tx.prepare("DELETE FROM main.messages WHERE flow = ?1")?;
            let mut delete_flow = tx.prepare("DELETE FROM main.flows WHERE id = ?1")?;
            for &flow_id in &deleted_ids {
                let key = flow_id.to_string();
                delete_messages.execute([&key])?;
                let deleted = delete_flow.execute([&key])?;
                ensure!(deleted == 1, "清空 HTTP 历史记录 {flow_id} 数量异常");
            }
        }
        crate::store::plugin_storage::remove_live_http_annotations(&tx, &deleted_ids)?;
        crate::store::provenance::remove_flow_provenance(&tx, &deleted_ids)?;
        {
            let mut delete_meta =
                tx.prepare("DELETE FROM main.metadata WHERE key = ?1 COLLATE BINARY")?;
            for &flow_id in &deleted_ids {
                delete_meta.execute([format!(
                    "{}{flow_id}",
                    crate::store::messages::HTTP_HISTORY_META_PREFIX
                )])?;
            }
        }
        tx.commit()?;
        Ok(deleted_ids)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictFlowSummary {
    id: crate::FlowId,
    revision: u64,
    source: TrafficSource,
    method: String,
    url: String,
    status: Option<u16>,
    state: FlowState,
    request_bytes: u64,
    response_bytes: u64,
    started_unix_ms: i64,
    elapsed_ms: u64,
}

impl From<StrictFlowSummary> for FlowSummary {
    fn from(value: StrictFlowSummary) -> Self {
        Self {
            id: value.id,
            revision: value.revision,
            source: value.source,
            method: value.method,
            url: value.url,
            status: value.status,
            state: value.state,
            request_bytes: value.request_bytes,
            response_bytes: value.response_bytes,
            started_unix_ms: value.started_unix_ms,
            elapsed_ms: value.elapsed_ms,
        }
    }
}

fn check_logger_cancel(cancel: &CancellationToken, deadline: Instant) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "Logger 历史查询已取消");
    ensure!(Instant::now() < deadline, "Logger 历史查询超过 1s 预算");
    Ok(())
}

fn read_logger_flow(row: &Row<'_>) -> Result<(FlowSummary, usize)> {
    let ValueRef::Text(id) = row.get_ref(0)? else {
        anyhow::bail!("Logger 流量 ID 列类型损坏");
    };
    let id = std::str::from_utf8(id).context("Logger 流量 ID 编码损坏")?;
    ensure!(id.len() == 36, "Logger 流量 ID 长度损坏");
    let id = Uuid::parse_str(id).context("Logger 流量 ID 无效")?;
    let ValueRef::Integer(revision) = row.get_ref(1)? else {
        anyhow::bail!("Logger 流量 revision 列类型损坏");
    };
    let revision = u64::try_from(revision).context("Logger 流量 revision 范围无效")?;
    let ValueRef::Integer(started) = row.get_ref(2)? else {
        anyhow::bail!("Logger 流量 started 列类型损坏");
    };
    let ValueRef::Text(url) = row.get_ref(3)? else {
        anyhow::bail!("Logger 流量 url 列类型损坏");
    };
    let url = std::str::from_utf8(url).context("Logger 流量 url 编码损坏")?;
    let ValueRef::Text(method) = row.get_ref(4)? else {
        anyhow::bail!("Logger 流量 method 列类型损坏");
    };
    let method = std::str::from_utf8(method).context("Logger 流量 method 编码损坏")?;
    let ValueRef::Text(state) = row.get_ref(5)? else {
        anyhow::bail!("Logger 流量 state 列类型损坏");
    };
    let state = std::str::from_utf8(state).context("Logger 流量 state 编码损坏")?;
    let ValueRef::Blob(bytes) = row.get_ref(6)? else {
        anyhow::bail!("Logger 流量摘要列类型损坏");
    };
    ensure!(
        bytes.len() <= MAX_SUMMARY_BYTES,
        "Logger 流量摘要超过 1 MiB"
    );
    let raw: Value = serde_json::from_slice(bytes).context("Logger 流量摘要 JSON 损坏")?;
    validate_logger_source_json(&raw)?;
    let summary: FlowSummary = serde_json::from_value::<StrictFlowSummary>(raw)
        .context("Logger 流量摘要字段或来源损坏")?
        .into();
    ensure!(
        !summary.id.is_nil()
            && summary.id == id
            && summary.revision > 0
            && summary.revision == revision
            && summary.started_unix_ms == started
            && summary.url == url
            && !summary.url.is_empty()
            && summary.method == method
            && crate::http::token(summary.method.as_bytes())
            && logger_state_code(&summary.state) == state
            && summary
                .status
                .is_none_or(|status| (100..=999).contains(&status))
            && (summary.state != FlowState::Pending
                || (summary.status.is_none() && summary.response_bytes == 0)),
        "Logger 流量摘要与表列或归属不一致"
    );
    if let TrafficSource::Plugin(name) = &summary.source {
        ensure!(
            !name.trim().is_empty() && name.len() <= 4096,
            "Logger 插件流量来源名称为空或无效"
        );
    }
    Ok((summary, bytes.len()))
}

fn validate_logger_source_json(summary: &Value) -> Result<()> {
    let object = summary.as_object().context("Logger 流量摘要不是对象")?;
    let source = object.get("source").context("Logger 流量摘要缺少来源")?;
    let valid = match source {
        Value::String(name) => {
            matches!(name.as_str(), "Proxy" | "Repeater" | "Intruder" | "Scanner")
        }
        Value::Object(map) => {
            map.len() == 1
                && map
                    .get("Plugin")
                    .and_then(Value::as_str)
                    .is_some_and(|name| !name.trim().is_empty() && name.len() <= 4096)
        }
        _ => false,
    };
    ensure!(valid, "Logger 流量来源摘要损坏或未知；未省略无法分类的流量");
    Ok(())
}

fn logger_state_code(state: &FlowState) -> &'static str {
    match state {
        FlowState::Pending => "pending",
        FlowState::Intercepted => "intercepted",
        FlowState::Complete => "complete",
        FlowState::Dropped => "dropped",
        FlowState::Cancelled => "cancelled",
        FlowState::Failed(_) => "failed",
    }
}

fn matches_logger_source(actual: &TrafficSource, filter: Option<&TrafficSource>) -> bool {
    match filter {
        None => true,
        Some(TrafficSource::Plugin(expected)) if expected.is_empty() => {
            matches!(actual, TrafficSource::Plugin(_))
        }
        Some(expected) => actual == expected,
    }
}

fn matches_logger_filter(summary: &FlowSummary, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    summary.url.to_lowercase().contains(needle)
        || summary.method.to_lowercase().contains(needle)
        || summary
            .status
            .is_some_and(|status| status.to_string().contains(needle))
}
