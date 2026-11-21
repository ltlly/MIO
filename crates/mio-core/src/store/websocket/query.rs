use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, params, params_from_iter};
use tokio_util::sync::CancellationToken;

use super::super::{APPLICATION_ID, SCHEMA, metadata, schema_guard, websocket_notes};
use super::records::{
    MAX_EXCLUDED_IDS, SUMMARY_COLUMNS, direction_value, kind_value, read_summary,
};
use super::{CapturedWebSocket, ProjectStore};
use crate::{
    FlowId, Uuid, WebSocketHistoryFilter, WebSocketHistoryQuery, WebSocketHistoryResult,
    WebSocketSummary, http::MAX_BODY,
};

impl WebSocketHistoryQuery {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100_000).contains(&self.limit),
            "分页大小必须为 1–100000"
        );
        ensure!(
            self.filter.listener_port != Some(0),
            "监听端口必须为 1–65535"
        );
        ensure!(
            self.filter.term.len() <= 4096,
            "WebSocket 搜索文本超过 4096 字节限额"
        );
        if !self.filter.filter_script.trim().is_empty() {
            crate::http::validate_filter_script(&self.filter.filter_script)?;
        }
        let offset = i64::try_from(self.offset).context("WebSocket 分页偏移溢出")?;
        ensure!(
            offset.checked_add(self.limit as i64).is_some(),
            "WebSocket 分页范围溢出"
        );
        Ok(())
    }
}

impl ProjectStore {
    pub(crate) fn websocket_query_path(&self) -> PathBuf {
        self.actual_path()
    }

    pub(crate) fn websocket_query_snapshot(
        path: &Path,
        query: &WebSocketHistoryQuery,
        pending: &[(WebSocketSummary, CapturedWebSocket)],
        cancel: &CancellationToken,
    ) -> Result<WebSocketHistoryResult> {
        query.validate()?;
        ensure!(
            pending.len() <= MAX_EXCLUDED_IDS,
            "WebSocket 恢复消息超过 512 条限额"
        );
        ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(500))?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
        let interruption = cancel.clone();
        connection.progress_handler(1000, Some(move || interruption.is_cancelled()));
        let transaction = connection.unchecked_transaction()?;
        let app_id: i32 =
            transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            app_id == APPLICATION_ID && schema == SCHEMA,
            "WebSocket 查询源项目版本已变化；未返回其它项目数据"
        );
        schema_guard::validate(&transaction)?;
        ensure!(
            metadata::project_id(&transaction)? == query.project.id,
            "WebSocket 查询源项目 ID 已变化；未返回其它项目数据"
        );
        let summaries: Vec<_> = pending.iter().map(|(summary, _)| summary.clone()).collect();
        let (select, values) = history_sql(query.flow_id, &[], &summaries, &query.filter)?;
        let mut result = if query.filter.term.is_empty()
            && query.filter.filter_script.trim().is_empty()
        {
            let selected_match = if let Some(id) = query.selected_message_id {
                let mut selected_values = values.clone();
                selected_values.push(id.to_string().into());
                Some((
                    id,
                    transaction.query_row(
                        &format!("SELECT EXISTS(SELECT 1 FROM ({select}) WHERE id=?)"),
                        params_from_iter(selected_values.iter()),
                        |row| row.get(0),
                    )?,
                ))
            } else {
                None
            };
            let (total, rows) = metadata_page(
                &transaction,
                &select,
                values,
                query.offset as i64,
                query.limit as i64,
            )?;
            WebSocketHistoryResult {
                total,
                rows,
                selected_match,
            }
        } else {
            search_page(&transaction, query, pending, select, values, cancel)?
        };
        websocket_notes::fill_previews(&transaction, &mut result.rows)?;
        let mio_base = crate::service::websocket::ext_archive::snapshot_ext_ordinal_base(
            &transaction,
            &summaries,
        )?;
        ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
        transaction.commit()?;
        if let Some(base) = mio_base {
            let store = ProjectStore::from_read_only(connection, query.project.id);
            crate::service::websocket::ext_archive::merge_ext_websocket_query(
                &store,
                base,
                query,
                &mut result,
                cancel,
            )?;
        }
        Ok(result)
    }

    #[cfg(test)]
    pub fn websocket_page(
        &self,
        flow: Option<FlowId>,
        offset: usize,
        limit: usize,
        excluded: &[Uuid],
    ) -> Result<(usize, Vec<WebSocketSummary>)> {
        self.websocket_page_with_recovery(
            flow,
            offset,
            limit,
            excluded,
            &[],
            &WebSocketHistoryFilter::default(),
        )
    }

    #[cfg(test)]
    pub fn websocket_page_with_recovery(
        &self,
        flow: Option<FlowId>,
        offset: usize,
        limit: usize,
        excluded: &[Uuid],
        pending: &[WebSocketSummary],
        history_filter: &WebSocketHistoryFilter,
    ) -> Result<(usize, Vec<WebSocketSummary>)> {
        ensure!((1..=100_000).contains(&limit), "分页大小必须为 1–100000");
        ensure!(
            history_filter.listener_port != Some(0),
            "监听端口必须为 1–65535"
        );
        ensure!(
            excluded.len() <= MAX_EXCLUDED_IDS && pending.len() <= MAX_EXCLUDED_IDS,
            "WebSocket 恢复排除列表超过 512 条限额"
        );
        let offset = i64::try_from(offset).context("WebSocket 分页偏移溢出")?;
        let limit = i64::try_from(limit)?;
        ensure!(
            offset.checked_add(limit).is_some(),
            "WebSocket 分页范围溢出"
        );
        let (select, values) = history_sql(flow, excluded, pending, history_filter)?;
        // Count and rows share a read snapshot; no payload table is consulted by either query.
        let transaction = self.connection.unchecked_transaction()?;
        if !history_filter.filter_script.trim().is_empty()
            || !history_filter.term.is_empty()
        {
            let query = WebSocketHistoryQuery {
                project: crate::ProjectKey {
                    id: self.id,
                    generation: 1,
                },
                query_revision: 1,
                filter: history_filter.clone(),
                flow_id: flow,
                offset: offset as usize,
                limit: limit as usize,
                selected_message_id: None,
            };
            query.validate()?;
            let mut result = search_page(
                &transaction,
                &query,
                &[],
                select,
                values,
                &CancellationToken::new(),
            )?;
            websocket_notes::fill_previews(&transaction, &mut result.rows)?;
            transaction.commit()?;
            return Ok((result.total, result.rows));
        }
        let (total, mut rows) = metadata_page(&transaction, &select, values, offset, limit)?;
        websocket_notes::fill_previews(&transaction, &mut rows)?;
        let mio_base = crate::service::websocket::ext_archive::snapshot_ext_ordinal_base(
            &transaction,
            pending,
        )?;
        transaction.commit()?;
        if let Some(base) = mio_base {
            return crate::service::websocket::ext_archive::merge_ext_page(
                self,
                base,
                flow,
                excluded,
                history_filter,
                offset as usize,
                limit as usize,
                total,
                rows,
            );
        }
        Ok((total, rows))
    }
}

pub(super) fn metadata_page(
    connection: &Connection,
    select: &str,
    mut values: Vec<rusqlite::types::Value>,
    offset: i64,
    limit: i64,
) -> Result<(usize, Vec<WebSocketSummary>)> {
    let total: i64 = connection.query_row(
        &format!("SELECT count(*) FROM ({select})"),
        params_from_iter(values.iter()),
        |row| row.get(0),
    )?;
    values.push(limit.into());
    values.push(offset.into());
    let rows = {
        let mut query =
            connection.prepare(&format!("{select} ORDER BY ordinal ASC LIMIT ? OFFSET ?"))?;
        let mut query_rows = query.query(params_from_iter(values.iter()))?;
        let mut rows = Vec::with_capacity(usize::try_from(limit)?);
        while let Some(row) = query_rows.next()? {
            rows.push(read_summary(row)?);
        }
        rows
    };
    Ok((usize::try_from(total)?, rows))
}

pub(super) fn search_page(
    connection: &rusqlite::Transaction<'_>,
    query: &WebSocketHistoryQuery,
    pending: &[(WebSocketSummary, CapturedWebSocket)],
    select: String,
    values: Vec<rusqlite::types::Value>,
    cancel: &CancellationToken,
) -> Result<WebSocketHistoryResult> {
    let script = (!query.filter.filter_script.trim().is_empty())
        .then_some(query.filter.filter_script.as_str());
    if let Some(script) = script {
        crate::http::validate_filter_script(script)?;
    }
    let scope = script
        .is_some()
        .then(|| metadata::workspace(connection).map(|ws| ws.scope))
        .transpose()?;
    let matcher = if query.filter.term.is_empty() {
        None
    } else {
        let pattern = if query.filter.regex {
            query.filter.term.clone()
        } else {
            regex::escape(&query.filter.term)
        };
        Some(regex::bytes::RegexBuilder::new(&pattern)
            .unicode(false)
            .case_insensitive(!query.filter.case_sensitive)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .nest_limit(128)
            .build()
            .map_err(|error| anyhow::anyhow!("WebSocket 正则表达式无效或超出限制；当前不支持 Java lookaround / backreference 等语法：{error}"))?)
    };
    let pending_by_id: HashMap<_, _> = pending
        .iter()
        .map(|(row, capture)| (row.id, capture))
        .collect();
    // Keep bodies outside the ordered UNION: SQLite may otherwise sort all payloads in a temp B-tree.
    let mut statement = connection.prepare(&format!("{select} ORDER BY ordinal ASC"))?;
    let mut payload_statement = connection.prepare(&format!(
        "SELECT typeof(p.body),length(p.body),
            CASE WHEN typeof(p.body)='blob' AND length(p.body)<={MAX_BODY} THEN p.body END
         FROM websocket_messages w LEFT JOIN websocket_payloads p ON w.id=p.message
         WHERE w.id=?1"
    ))?;
    let mut original_statement = connection.prepare(&format!(
        "SELECT o.message,typeof(o.body),
            CASE WHEN typeof(o.body)='blob' THEN length(o.body) END,
            CASE WHEN ?2!=0 AND typeof(o.body)='blob' AND length(o.body)<={MAX_BODY}
                THEN o.body END
         FROM websocket_messages w LEFT JOIN websocket_originals o ON w.id=o.message
         WHERE w.id=?1"
    ))?;
    let mut result = WebSocketHistoryResult {
        total: 0,
        rows: Vec::with_capacity(query.limit),
        selected_match: query.selected_message_id.map(|id| (id, false)),
    };
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
        let summary = read_summary(row)?;
        let id = Uuid::parse_str(row.get_ref(0)?.as_str()?).context("WebSocket 消息 ID 损坏")?;
        let expected_length = row.get::<_, u64>(6)?;
        let (final_bytes, original_bytes) = if let Some(capture) = pending_by_id.get(&id) {
            if let Some(original) = &capture.original_payload {
                ensure!(
                    original.len() <= MAX_BODY,
                    "WebSocket 消息原始正文超过 16 MiB 限额"
                );
            }
            (
                capture.record.payload.clone(),
                capture.edited_original().map(|orig| orig.to_vec()),
            )
        } else {
            let mut payload_rows = payload_statement.query([id.to_string()])?;
            let final_bytes = {
                let payload_row = payload_rows.next()?.context("WebSocket 消息正文缺失")?;
                ensure!(
                    payload_row.get_ref(0)?.as_str()? == "blob",
                    "WebSocket 消息正文缺失或不是二进制"
                );
                let payload_len = payload_row.get::<_, u64>(1)?;
                ensure!(
                    payload_len <= MAX_BODY as u64,
                    "WebSocket 消息正文超过 16 MiB 限额"
                );
                let bytes = payload_row.get_ref(2)?.as_blob()?.to_vec();
                ensure!(
                    bytes.len() as u64 == expected_length,
                    "WebSocket 消息正文长度与摘要不一致"
                );
                ensure!(
                    payload_rows.next()?.is_none(),
                    "WebSocket 消息存在重复正文归属；未选择或读取其中一条"
                );
                bytes
            };
            let original_bytes = {
                let mut orig_rows = original_statement.query(params![id.to_string(), 1_i64])?;
                if let Some(orig_row) = orig_rows.next()?
                    && orig_row.get::<_, Option<String>>(0)?.is_some()
                {
                    ensure!(
                        orig_row.get_ref(1)?.as_str()? == "blob",
                        "WebSocket 消息原始正文不是二进制"
                    );
                    let orig_len = orig_row.get::<_, u64>(2)?;
                    ensure!(
                        orig_len <= MAX_BODY as u64,
                        "WebSocket 消息原始正文超过 16 MiB 限额"
                    );
                    let bytes = orig_row.get_ref(3)?.as_blob()?.to_vec();
                    ensure!(
                        bytes.len() as u64 == orig_len,
                        "WebSocket 消息正文长度与摘要不一致"
                    );
                    ensure!(
                        orig_rows.next()?.is_none(),
                        "WebSocket 消息存在重复原始正文归属；未选择或修改其中一条"
                    );
                    Some(bytes)
                } else {
                    None
                }
            };
            (final_bytes, original_bytes)
        };
        let term_matches = match &matcher {
            Some(matcher) => {
                (match_payload(&final_bytes, expected_length, matcher, cancel)?
                    || original_bytes.as_ref().is_some_and(|orig| {
                        match_payload(orig, orig.len() as u64, matcher, cancel).unwrap_or(false)
                    }))
                    != query.filter.negative
            }
            None => true,
        };
        let script_matches = if term_matches && let Some(script) = script {
            let notes: String = connection
                .query_row(
                    "SELECT COALESCE(text, '') FROM websocket_notes WHERE message=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            let in_scope = scope.as_ref().is_some_and(|s| s.is_url_in_scope(&summary.url));
            crate::http::eval_ws_filter_script_with_scope(
                script,
                &summary.url,
                summary.direction,
                &final_bytes,
                summary.edited,
                &notes,
                summary.listener_port,
                in_scope,
            )?
        } else {
            true
        };
        let matches = term_matches && script_matches;
        if query.selected_message_id == Some(id) {
            result.selected_match = Some((id, matches));
        }
        if matches {
            if result.total >= query.offset && result.rows.len() < query.limit {
                result.rows.push(summary);
            }
            result.total = result
                .total
                .checked_add(1)
                .context("WebSocket 查询总数溢出")?;
        }
    }
    Ok(result)
}

fn match_payload(
    payload: &[u8],
    expected_length: u64,
    matcher: &regex::bytes::Regex,
    cancel: &CancellationToken,
) -> Result<bool> {
    ensure!(
        payload.len() <= MAX_BODY,
        "WebSocket 消息正文超过 16 MiB 限额"
    );
    ensure!(
        payload.len() as u64 == expected_length,
        "WebSocket 消息正文长度与摘要不一致"
    );
    ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    let matches = matcher.is_match(payload);
    ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    Ok(matches)
}

#[cfg(test)]
pub(super) fn byte_character_view<'a>(
    payload: &'a [u8],
    cancel: &CancellationToken,
) -> Result<&'a [u8]> {
    ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    ensure!(
        payload.len() <= MAX_BODY,
        "WebSocket 消息正文超过 16 MiB 限额"
    );
    Ok(payload)
}

pub(super) fn history_sql(
    flow: Option<FlowId>,
    excluded: &[Uuid],
    pending: &[WebSocketSummary],
    history_filter: &WebSocketHistoryFilter,
) -> Result<(String, Vec<rusqlite::types::Value>)> {
    let mut values = Vec::<rusqlite::types::Value>::new();
    let mut prefix = String::new();
    if !pending.is_empty() {
        prefix.push_str("pending(id,revision,flow,sequence,direction,kind,payload_bytes,observed,url,ordinal,connection_id,listener_port,edited) AS (VALUES ");
        for (index, row) in pending.iter().enumerate() {
            if index != 0 {
                prefix.push(',');
            }
            prefix.push_str("(?,?,?,?,?,?,?,?,?,?,?,?,?)");
            values.extend([
                row.id.to_string().into(),
                i64::try_from(row.revision)?.into(),
                row.flow_id.to_string().into(),
                i64::try_from(row.sequence)?.into(),
                direction_value(row.direction).into(),
                kind_value(row.kind).into(),
                i64::try_from(row.payload_bytes)?.into(),
                row.observed_unix_ms.into(),
                row.url.clone().into(),
                i64::try_from(row.ordinal)?.into(),
                i64::try_from(row.connection_id)?.into(),
                row.listener_port.map(i64::from).into(),
                i64::from(row.edited).into(),
            ]);
        }
        prefix.push_str("),");
    }
    prefix.push_str(&format!("history AS (SELECT {SUMMARY_COLUMNS} FROM websocket_messages w
        LEFT JOIN flows f ON f.id=w.flow LEFT JOIN websocket_connections c ON f.id=c.flow WHERE 1=1"));
    if !excluded.is_empty() {
        prefix.push_str(" AND w.id NOT IN (");
        for (index, id) in excluded.iter().enumerate() {
            if index != 0 {
                prefix.push(',');
            }
            prefix.push('?');
            values.push(id.to_string().into());
        }
        prefix.push(')');
    }
    if !pending.is_empty() {
        prefix
            .push_str(" AND w.id NOT IN (SELECT id FROM pending) UNION ALL SELECT * FROM pending");
    }
    prefix.push(')');
    let mut filter = String::new();
    if let Some(flow) = flow {
        filter.push_str(" AND flow=?");
        values.push(flow.to_string().into());
    }
    if history_filter.hide_incoming {
        filter.push_str(" AND direction!=1");
    }
    if history_filter.hide_outgoing {
        filter.push_str(" AND direction!=0");
    }
    if let Some(port) = history_filter.listener_port {
        filter.push_str(" AND listener_port=?");
        values.push(i64::from(port).into());
    }
    if history_filter.only_with_notes {
        filter.push_str(" AND EXISTS(SELECT 1 FROM websocket_notes n WHERE history.id=n.message AND length(CAST(n.text AS BLOB))>0)");
    }
    Ok((
        format!("WITH {prefix} SELECT * FROM history WHERE 1=1{filter}"),
        values,
    ))
}
