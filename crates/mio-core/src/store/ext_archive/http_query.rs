//! HTTP history filtering and global ordering over a read-only project snapshot.
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, params,
    params_from_iter,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{
    EXT_CHUNK_BYTES, MAX_PAGE_SUMMARY_BYTES, MAX_SUMMARY_BYTES, ProjectStore, checked_range,
    proxy_history::{
        CANONICAL, LIVE_HISTORY, proxy_row, proxy_source, request_origin_from_metadata,
        validate_message_binding,
    },
    read_chunk, read_item, read_message_summary, source_state,
};
use crate::{
    ExtHistoryRow, ExtMessageSummary, FlowSummary, Header, HistoryEntryId, HistoryRow,
    HttpExtensionFilter, HttpHistoryFilter, HttpHistoryPending, HttpHistoryQuery,
    HttpHistoryResult, HttpHistoryRowMetadata, HttpHistorySortColumn, TrafficSource, Uuid,
    http::MAX_BODY,
};

pub(crate) mod filters;
pub(crate) mod search;
mod sql;

#[cfg(test)]
use filters::parsed_parameters;
use filters::{
    extension_values, history_mime_from_archive, history_mime_from_live,
    parameterized_archive_request_cached, parameterized_live_request, search_matcher,
    url_has_parameters,
};
use search::search_page;
use sql::{
    history_sql, identity_id, order_sql, register_scalar_functions, validate_archive_bindings, validate_original_heads,
    validate_unique_json,
};

const MAX_PENDING: usize = 512;
pub(super) const MAX_HTTP_HISTORY_SUMMARY_BYTES: usize = 64 * 1024 * 1024;

fn extract_url_ip_literal(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| match parsed.host()? {
            url::Host::Ipv4(v4) => Some(v4.to_string()),
            url::Host::Ipv6(v6) => Some(match v6.to_ipv4_mapped() {
                Some(mapped) => mapped.to_string(),
                None => v6.to_string(),
            }),
            url::Host::Domain(domain) => domain.parse::<std::net::IpAddr>().ok().map(|ip| match ip {
                std::net::IpAddr::V4(v4) => v4.to_string(),
                std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                    Some(mapped) => mapped.to_string(),
                    None => v6.to_string(),
                },
            }),
        })
        .unwrap_or_default()
}

pub(in crate::store) fn live_row_metadata(
    connection: &Connection,
    summary: &FlowSummary,
    fallback_listener_port: Option<u16>,
) -> Result<HttpHistoryRowMetadata> {
    let id = summary.id.to_string();
    let schema: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let stored_port: Option<u16> = if schema >= 25 {
        connection
            .query_row(
                "SELECT listener_port FROM flows WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
    } else {
        None
    };
    let listener_port = stored_port.or(fallback_listener_port);
    let stored_meta = crate::store::messages::read_http_history_metadata(connection, summary.id)?;

    let mut stmt = connection.prepare(
        "SELECT side,
            CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END
         FROM messages WHERE flow=?1 AND side IN (0, 1, 2, 3) ORDER BY side",
    )?;
    let mut rows = stmt.query(params![
        &id,
        crate::store::MAX_MESSAGE_HEAD_BYTES as i64
    ])?;
    let mut req_cur: Option<crate::store::OwnedRequestHead> = None;
    let mut req_orig: Option<crate::store::OwnedRequestHead> = None;
    let mut resp_cur: Option<crate::store::OwnedResponseHead> = None;
    let mut resp_orig: Option<crate::store::OwnedResponseHead> = None;
    while let Some(row) = rows.next()? {
        let side: u8 = row.get(0)?;
        let head_bytes: Vec<u8> = row
            .get::<_, Option<Vec<u8>>>(1)?
            .context("HTTP 历史消息头缺失或超过 4 MiB 限额")?;
        match side {
            0 => req_cur = Some(serde_json::from_slice(&head_bytes)?),
            1 => resp_cur = Some(serde_json::from_slice(&head_bytes)?),
            2 => req_orig = Some(serde_json::from_slice(&head_bytes)?),
            3 => resp_orig = Some(serde_json::from_slice(&head_bytes)?),
            _ => {}
        }
    }
    let has_original_request = req_orig.is_some();
    let has_original_response = resp_orig.is_some();
    let edited = has_original_request || has_original_response;
    let first_req = req_orig.as_ref().or(req_cur.as_ref());
    let first_resp = resp_orig.as_ref().or(resp_cur.as_ref());

    let cancel = CancellationToken::new();
    let req_url = first_req.map_or(summary.url.as_str(), |r| r.url.as_str());
    let (has_params, params_known, params_error) = if let Some(req) = first_req
        && (req.params_known.is_some() || req.has_params.is_some() || req.params_error.is_some())
    {
        let known = req.params_known.unwrap_or(req.has_params.is_some());
        ensure!(req.params_error.is_none() || !known, "HTTP 参数分析状态与诊断冲突");
        ensure!(!known || req.has_params.is_some(), "HTTP 参数分析缺少已知结果");
        (req.has_params.unwrap_or(false), known, req.params_error.clone())
    } else if let Some(stored) = &stored_meta {
        (stored.metadata.has_params, stored.metadata.params_known, stored.metadata.params_error.clone())
    } else {
        match url_has_parameters(req_url, &cancel) {
            Ok(true) => (true, true, None),
            Ok(false) => (false, false, Some("HTTP 原始请求未保存完整参数分析结果".into())),
            Err(error) => (false, false, Some(crate::store::messages::history_analysis_diagnostic(&error))),
        }
    };

    let (cookies_str, cookies_error) = analysis_text(history_cookies(
        first_req.map(|request| request.headers.as_slice()),
        first_resp.map(|response| response.headers.as_slice()),
    ));

    let (response_length, mime_type, title, title_error, response_received_ms) = if let Some(resp) = first_resp {
        let body_bytes = resp.body_bytes.unwrap_or(summary.response_bytes);
        let len = crate::http::intruder_response_len(
            resp.version,
            resp.status,
            resp.reason_phrase.as_deref(),
            &resp.headers,
            body_bytes,
        );
        let mime = resp
            .mime_type
            .clone()
            .filter(|m| !m.is_empty())
            .or_else(|| {
                stored_meta
                    .as_ref()
                    .map(|s| s.metadata.mime_type.clone())
                    .filter(|m| !m.is_empty())
            })
            .unwrap_or_else(|| {
                crate::http::infer_history_display_mime_type(&resp.headers, &[]).to_owned()
            });
        let (page_title, title_error) = if resp.title_error.is_some() {
            ensure!(resp.title.as_ref().is_none_or(String::is_empty), "HTTP 标题分析状态与诊断冲突");
            (String::new(), resp.title_error.clone())
        } else if let Some(title) = &resp.title {
            (title.clone(), None)
        } else if let Some(stored) = &stored_meta {
            (stored.metadata.title.clone(), stored.metadata.title_error.clone())
        } else {
            (String::new(), (mime == "html").then(|| "HTTP 原始响应未保存标题分析结果".into()))
        };
        let recv_ms = resp
            .response_received_ms
            .or_else(|| stored_meta.as_ref().and_then(|s| s.metadata.response_received_ms));
        (Some(len), mime, page_title, title_error, recv_ms)
    } else {
        (
            stored_meta.as_ref().and_then(|s| s.metadata.response_length),
            stored_meta
                .as_ref()
                .map(|s| s.metadata.mime_type.clone())
                .unwrap_or_default(),
            stored_meta
                .as_ref()
                .map(|s| s.metadata.title.clone())
                .unwrap_or_default(),
            stored_meta.as_ref().and_then(|s| s.metadata.title_error.clone()),
            stored_meta
                .as_ref()
                .and_then(|s| s.metadata.response_received_ms),
        )
    };

    let remote_ip = first_req
        .and_then(|r| r.remote_ip.clone())
        .filter(|ip| !ip.is_empty())
        .or_else(|| {
            stored_meta
                .as_ref()
                .map(|s| s.metadata.remote_ip.clone())
                .filter(|ip| !ip.is_empty())
        })
        .unwrap_or_else(|| extract_url_ip_literal(req_url));

    let time_known = first_req
        .and_then(|r| r.time_known)
        .or_else(|| stored_meta.as_ref().map(|s| s.metadata.time_known))
        .unwrap_or(true);

    let capture_number = first_req
        .and_then(|r| r.capture_number)
        .filter(|&n| n > 0)
        .or_else(|| {
            stored_meta
                .as_ref()
                .and_then(|s| s.metadata.capture_number)
                .filter(|&n| n > 0)
        });

    Ok(HttpHistoryRowMetadata {
        has_params,
        params_known,
        params_error,
        edited,
        has_original_request,
        has_original_response,
        response_length,
        mime_type,
        title,
        title_error,
        remote_ip,
        cookies: cookies_str,
        cookies_error,
        listener_port: listener_port
            .or_else(|| stored_meta.as_ref().and_then(|s| s.metadata.listener_port)),
        response_received_ms,
        time_known,
        capture_number,
    })
}

pub(in crate::store::ext_archive::http_query) fn row_metadata_for_entry(
    connection: &Connection,
    row: &HistoryRow,
    pending_by_id: &HashMap<Uuid, &HttpHistoryPending>,
) -> Result<HttpHistoryRowMetadata> {
    let mut chunk_cache = search::ChunkCache::default();
    row_metadata_for_entry_cached(connection, row, pending_by_id, &mut chunk_cache)
}

pub(in crate::store::ext_archive::http_query) fn row_metadata_for_entry_cached(
    connection: &Connection,
    row: &HistoryRow,
    pending_by_id: &HashMap<Uuid, &HttpHistoryPending>,
    chunk_cache: &mut search::ChunkCache,
) -> Result<HttpHistoryRowMetadata> {
    match row {
        HistoryRow::Live(summary) => {
            let pending = pending_by_id.get(&summary.id).copied();
            let mut meta = live_row_metadata(
                connection,
                summary,
                pending.and_then(|p| p.listener_port),
            )?;
            if let Some(pending) = pending {
                let request_present: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE flow=?1 AND side IN (0,2))", [summary.id.to_string()], |row| row.get(0))?;
                let first_resp = pending.first_response.as_ref().or(pending.response.as_ref());
                if let Some(first_req) = &pending.first_request
                    && let Some(cur_req) = &pending.request
                    && cur_req != first_req
                {
                    meta.has_original_request = true;
                    meta.edited = true;
                }
                if !request_present && let Some(first_req) = &pending.first_request {
                    apply_params_analysis(&mut meta, crate::store::messages::history_has_parameters(first_req));
                    (meta.cookies, meta.cookies_error) = analysis_text(history_cookies(Some(&first_req.headers), first_resp.map(|response| response.headers.as_slice())));
                }
                let response_present: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE flow=?1 AND side IN (1,3))", [summary.id.to_string()], |row| row.get(0))?;
                if let Some(resp) = first_resp
                    && let Some(cur_resp) = &pending.response
                    && cur_resp != resp
                {
                    meta.has_original_response = true;
                    meta.edited = true;
                }
                if !response_present && let Some(resp) = first_resp {
                    if meta.response_length.is_none() {
                        meta.response_length = Some(crate::http::intruder_response_len(
                            resp.version,
                            resp.status,
                            pending.capture_metrics.reason_phrase.as_deref(),
                            &resp.headers,
                            resp.body.len() as u64,
                        ));
                    }
                    if meta.mime_type.is_empty() {
                        let (mime, title) = crate::store::messages::response_history_values(
                            resp,
                            &pending.first_request_method,
                        );
                        meta.mime_type = mime;
                        (meta.title, meta.title_error) = analysis_text(title);
                    }
                }
                if let Some(ip) = pending.capture_metrics.remote_ip {
                    meta.remote_ip = ip.to_string();
                }
                if pending.capture_metrics.response_received_ms.is_some() {
                    meta.response_received_ms = pending.capture_metrics.response_received_ms;
                }
            }
            Ok(meta)
        }
        HistoryRow::ExtProxy(archive) => {
            let cancel = CancellationToken::new();
            let url = archive
                .request
                .as_ref()
                .and_then(|r| r.request_target.as_deref())
                .unwrap_or("");
            let origin_str = archive.origin.as_deref().unwrap_or(url);
            let has_original_request = archive.edited_request.is_some();
            let has_original_response = archive.edited_response.is_some();
            let edited = has_original_request || has_original_response;
            let mut metadata = HttpHistoryRowMetadata {
                edited,
                has_original_request,
                has_original_response,
                response_length: archive.response.as_ref().map(|r| r.raw_bytes),
                remote_ip: extract_url_ip_literal(origin_str),
                time_known: archive.time_unix_ms.is_some() || archive.time_error.is_some(),
                ..Default::default()
            };
            let params = (|| -> Result<bool> {
                let request = archive.request.as_ref().context("HTTP 归档缺少原始请求，参数状态未知")?;
                if url_has_parameters(url, &cancel)? {
                    return Ok(true);
                }
                parameterized_archive_request_cached(
                        connection,
                        request,
                        chunk_cache,
                        &cancel,
                )
            })();
            apply_params_analysis(&mut metadata, params);
            let cookies = (|| -> Result<String> {
                let mut names = Vec::new();
                for (message, response) in [(archive.request.as_ref(), false), (archive.response.as_ref(), true)] {
                    let Some(message) = message else { continue; };
                    let limit = message.body_offset.unwrap_or(message.raw_bytes);
                    ensure!(limit <= (crate::http::MAX_HEAD + 4) as u64, "HTTP Cookie 分析消息头超过 128 KiB");
                    let raw = search::read_archive_message_prefix_cached(connection, message, limit, chunk_cache, &cancel)?;
                    let (_, headers, _) = crate::http::head(&raw)?;
                    for name in crate::store::messages::history_cookie_names(&headers, response)? {
                        if !names.contains(&name) { names.push(name); }
                    }
                }
                Ok(names.join(", "))
            })();
            (metadata.cookies, metadata.cookies_error) = analysis_text(cookies);
            if let Some(message) = &archive.response {
                let title = (|| -> Result<String> {
                    ensure!(message.raw_bytes <= (crate::http::MAX_HEAD + crate::http::MAX_BODY + 4) as u64, "HTTP 标题分析归档响应超过 16 MiB");
                    let raw = search::read_archive_message_prefix_cached(connection, message, message.raw_bytes, chunk_cache, &cancel)?;
                    let version = message.declared_version.context("HTTP 标题分析原始响应协议未知")?;
                    let response = crate::parse_response(&raw, version)?;
                    ensure!(Some(response.status) == message.status, "HTTP 标题分析响应状态与摘要不一致");
                    let method = archive.request.as_ref().and_then(|request| request.method.as_deref()).context("HTTP 标题分析原始请求方法未知")?;
                    let (mime, title) = crate::store::messages::response_history_values(&response, method);
                    metadata.mime_type = mime;
                    title
                })();
                (metadata.title, metadata.title_error) = analysis_text(title);
            }
            Ok(metadata)
        }
    }
}

fn analysis_text(result: Result<String>) -> (String, Option<String>) {
    match result {
        Ok(text) => (text, None),
        Err(error) => (String::new(), Some(crate::store::messages::history_analysis_diagnostic(&error))),
    }
}

fn apply_params_analysis(metadata: &mut HttpHistoryRowMetadata, result: Result<bool>) {
    match result {
        Ok(has_params) => {
            metadata.has_params = has_params;
            metadata.params_known = true;
            metadata.params_error = None;
        }
        Err(error) => {
            metadata.has_params = false;
            metadata.params_known = false;
            metadata.params_error = Some(crate::store::messages::history_analysis_diagnostic(&error));
        }
    }
}

fn history_cookies(request: Option<&[Header]>, response: Option<&[Header]>) -> Result<String> {
    let mut names = Vec::new();
    for (headers, response) in [(request, false), (response, true)] {
        let Some(headers) = headers else { continue; };
        for name in crate::store::messages::history_cookie_names(headers, response)? {
            if !names.contains(&name) { names.push(name); }
        }
    }
    Ok(names.join(", "))
}
impl HttpHistoryQuery {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.project.id.is_nil() && self.project.generation > 0,
            "HTTP 历史项目归属无效"
        );
        ensure!(
            (1..=100_000).contains(&self.limit),
            "查询上限必须为 1–100000"
        );
        ensure!(self.url_filter.len() <= 4096, "URL／方法筛选超过 4096 字节");
        ensure!(
            self.filter.search_term.len() <= 4096,
            "HTTP 搜索词超过 4096 字节"
        );
        ensure!(
            self.filter.listener_port != Some(0),
            "HTTP 监听端口不能为 0"
        );
        search_matcher(&self.filter)?;
        if !self.filter.filter_script.trim().is_empty() {
            crate::http::validate_filter_script(&self.filter.filter_script)?;
        }
        let offset = i64::try_from(self.offset).context("HTTP 历史分页偏移溢出")?;
        ensure!(
            offset.checked_add(self.limit as i64).is_some(),
            "HTTP 历史分页范围溢出"
        );
        ensure!(self.selected_ids.len() <= 500, "HTTP 历史选择超过 500 条");
        let mut selected = HashSet::new();
        for id in &self.selected_ids {
            let valid = match id {
                HistoryEntryId::Live(id) => !id.is_nil(),
                HistoryEntryId::External { source_id, item_id } => {
                    !source_id.is_nil() && !item_id.is_nil()
                }
            };
            ensure!(valid && selected.insert(*id), "HTTP 历史选择身份为空或重复");
        }
        extension_values(&self.filter.extensions)?;
        Ok(())
    }
}

impl ProjectStore {
    pub(crate) fn http_history_snapshot(
        path: &Path,
        query: &HttpHistoryQuery,
        pending: &[HttpHistoryPending],
        cancel: &CancellationToken,
    ) -> Result<HttpHistoryResult> {
        query.validate()?;
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(500))?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
        let interrupt = cancel.clone();
        connection.progress_handler(1000, Some(move || interrupt.is_cancelled()));
        query_connection(&connection, query, pending, cancel)
    }

    #[cfg(test)]
    pub(crate) fn query_http_history(
        &self,
        query: &HttpHistoryQuery,
        cancel: &CancellationToken,
    ) -> Result<HttpHistoryResult> {
        query_connection(&self.connection, query, &[], cancel)
    }
}

fn query_connection(
    connection: &Connection,
    query: &HttpHistoryQuery,
    pending: &[HttpHistoryPending],
    cancel: &CancellationToken,
) -> Result<HttpHistoryResult> {
    query.validate()?;
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    ensure!(pending.len() <= MAX_PENDING, "HTTP 恢复摘要超过 512 条");
    register_scalar_functions(connection)?;
    let mut pending_ids = HashSet::new();
    for pending in pending {
        let row = &pending.summary;
        ensure!(
            row.source == TrafficSource::Proxy
                && !row.id.is_nil()
                && row.revision > 0
                && pending_ids.insert(row.id),
            "HTTP 恢复摘要归属或 revision 无效"
        );
        ensure!(
            crate::http::token(pending.first_request_method.as_bytes())
                && !pending.first_request_url.is_empty()
                && pending.first_request_url.len() <= MAX_SUMMARY_BYTES
                && pending
                    .first_response_status
                    .is_none_or(|status| (100..=999).contains(&status)),
            "HTTP 恢复原始消息摘要无效"
        );
    }
    let transaction = connection.unchecked_transaction()?;
    let app: i32 = transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        app == crate::store::APPLICATION_ID && schema == crate::store::SCHEMA,
        "HTTP 历史查询项目版本已变化"
    );
    crate::store::schema_guard::validate(&transaction, schema)?;
    ensure!(
        crate::store::metadata::project_id(&transaction)? == query.project.id,
        "HTTP 历史查询项目 ID 已变化"
    );
    let live_annotation_store =
        crate::store::plugin_storage::read_live_http_annotations(&transaction)?;
    let live_annotations: Vec<crate::LiveHttpAnnotationEntry> =
        live_annotation_store
            .iter()
            .map(|(flow_id, entry)| {
                (
                    *flow_id,
                    entry.notes_revision,
                    entry.notes.chars().take(256).collect(),
                    crate::ExtHttpHighlight {
                        revision: entry.highlight_revision,
                        color: entry.highlight,
                    },
                )
            })
            .collect();
    let source = proxy_source(&transaction)?;
    let invalid_sources: usize = transaction.query_row(
        &format!("{LIVE_HISTORY} SELECT count(*) FROM live WHERE is_proxy IS NULL"),
        params!["", MAX_SUMMARY_BYTES as i64],
        |row| row.get(0),
    )?;
    ensure!(
        invalid_sources == 0,
        "HTTP 历史来源摘要损坏或未知；未省略无法分类的流量"
    );
    validate_unique_json(&transaction, source)?;
    if let Some(source) = source {
        validate_archive_bindings(&transaction, source, cancel)?;
    }
    let (sql, values) = history_sql(&transaction, query, source, pending, cancel)?;
    validate_original_heads(&transaction, &sql, &values, cancel)?;
    if query.filter.only_with_notes || query.filter.only_highlighted {
        let mut statement =
            transaction.prepare(&format!("{sql} SELECT id FROM candidates WHERE kind=0"))?;
        let mut rows = statement.query(params_from_iter(values.iter()))?;
        while let Some(row) = rows.next()? {
            ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
            let id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
            let item = read_item(&transaction, id)?;
            let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
            if query.filter.only_with_notes {
                let (_, _, error) = super::http_notes::preview(&transaction, &item, &metadata);
                ensure!(
                    error.is_none(),
                    "HTTP 备注筛选不能判断未映射或损坏的源备注：{}",
                    error.unwrap_or_default()
                );
            }
            if query.filter.only_highlighted {
                let (_, error) = super::http_highlights::preview(&transaction, &item, &metadata);
                ensure!(
                    error.is_none(),
                    "HTTP 高亮筛选不能判断未映射或损坏的源颜色：{}",
                    error.unwrap_or_default()
                );
            }
        }
    }
    let (total, invalid): (usize, usize) = transaction.query_row(
        &format!("{sql} SELECT (SELECT count(*) FROM filtered),(SELECT count(*) FROM candidates WHERE invalid)"),
        params_from_iter(values.iter()),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(invalid == 0, "HTTP 历史摘要字段损坏；未省略无法筛选的记录");
    if !query.filter.search_term.is_empty()
        || !query.filter.filter_script.trim().is_empty()
        || query.filter.in_scope_only
        || query.filter.parameterized_only
        || query.filter.mime_types != [true; 8]
    {
        let result = search_page(
            &transaction,
            source,
            query,
            pending,
            &sql,
            &values,
            &live_annotation_store,
            live_annotations,
            cancel,
        )?;
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        transaction.commit()?;
        return Ok(result);
    }
    let order = order_sql(query.sort.column, query.sort.ascending);
    let mut bytes = 0usize;
    let mut live_numbers = Vec::new();
    let mut live_number_ids = HashSet::new();
    let pending_by_id: HashMap<Uuid, &HttpHistoryPending> = pending
        .iter()
        .map(|pending| (pending.summary.id, pending))
        .collect();
    let mut row_metadata = Vec::new();
    let mut metadata_ids = HashSet::new();
    let mut chunk_cache = search::ChunkCache::default();
    let (selected_matches, selected_ranks, mut selected_rows) = if query.selected_ids.is_empty() {
        (Vec::new(), Vec::new(), Vec::new())
    } else {
        let selected_json = serde_json::to_string(
            &query
                .selected_ids
                .iter()
                .map(|id| match id {
                    HistoryEntryId::Live(id) => format!("1:{id}"),
                    HistoryEntryId::External { source_id, item_id } => {
                        format!("0:{source_id}:{item_id}")
                    }
                })
                .collect::<Vec<_>>(),
        )?;
        let mut selected_values = values.clone();
        selected_values.push(selected_json.into());
        let (selected_matches, selected_ranks) = {
            let mut statement = transaction.prepare(&format!(
                "{sql}, ranked_results AS (
                SELECT identity,row_number() OVER (ORDER BY {order})-1 AS global_rank FROM filtered
            ) SELECT selected.value,f.global_rank FROM json_each(?11) selected
              LEFT JOIN ranked_results f ON f.identity=selected.value
              ORDER BY f.global_rank IS NULL,f.global_rank,selected.key"
            ))?;
            let rows = statement.query_map(params_from_iter(selected_values.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<usize>>(1)?))
            })?;
            let mut matches = Vec::with_capacity(query.selected_ids.len());
            let mut ranks = Vec::with_capacity(query.selected_ids.len());
            for row in rows {
                let (identity, rank) = row?;
                let id = identity_id(&identity)?;
                matches.push((id, rank.is_some()));
                if let Some(rank) = rank {
                    ranks.push((id, rank));
                }
            }
            (matches, ranks)
        };
        let mut selected_rows = Vec::with_capacity(selected_ranks.len());
        {
            let mut statement = transaction.prepare(&format!(
                "{sql} SELECT kind,id,summary,method,url,status,number FROM filtered
                    WHERE identity IN (SELECT value FROM json_each(?11)) ORDER BY {order}"
            ))?;
            let mut rows = statement.query(params_from_iter(selected_values.iter()))?;
            while let Some(row) = rows.next()? {
                ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
                let kind: u8 = row.get(0)?;
                let id = Uuid::parse_str(row.get_ref(1)?.as_str()?)?;
                if kind == 1 && live_number_ids.insert(id) {
                    let capture_number = u64::try_from(row.get::<_, i64>(6)?)
                        .context("HTTP 实时历史捕获序号无效")?;
                    live_numbers.push((id, capture_number));
                }
                let value = read_history_row(&transaction, source, row)?;
                bytes = bytes
                    .checked_add(summary_bytes(&value)?)
                    .context("HTTP 所选摘要大小溢出")?;
                ensure!(
                    bytes <= MAX_PAGE_SUMMARY_BYTES,
                    "HTTP 所选记录摘要超过 4 MiB；未省略所选记录"
                );
                let entry_id = value.id();
                if metadata_ids.insert(entry_id) {
                    let meta = row_metadata_for_entry_cached(
                        &transaction,
                        &value,
                        &pending_by_id,
                        &mut chunk_cache,
                    )?;
                    row_metadata.push((entry_id, meta));
                }
                selected_rows.push(value);
            }
        }
        (selected_matches, selected_ranks, selected_rows)
    };
    let mut page_values = values;
    page_values.push((query.limit as i64).into());
    page_values.push((query.offset as i64).into());
    let mut statement = transaction.prepare(&format!(
        "{sql} SELECT kind,id,summary,method,url,status,number FROM filtered ORDER BY {order} LIMIT ?11 OFFSET ?12"
    ))?;
    let mut rows = statement.query(params_from_iter(page_values.iter()))?;
    let mut page = Vec::with_capacity(query.limit);
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let id = Uuid::parse_str(row.get_ref(1)?.as_str()?)?;
        let kind: u8 = row.get(0)?;
        let live_capture_number = if kind == 1 {
            Some(
                u64::try_from(row.get::<_, i64>(6)?)
                    .context("HTTP 实时历史捕获序号无效")?,
            )
        } else {
            None
        };
        let selected_index = selected_rows
            .iter()
            .position(|selected| match selected.id() {
                HistoryEntryId::Live(selected) => kind == 1 && selected == id,
                HistoryEntryId::External { item_id, .. } => kind == 0 && item_id == id,
            });
        let value = if let Some(index) = selected_index {
            selected_rows.remove(index)
        } else {
            read_history_row(&transaction, source, row)?
        };
        let next_bytes = if selected_index.is_some() {
            0
        } else {
            summary_bytes(&value)?
        };
        if bytes + next_bytes > MAX_HTTP_HISTORY_SUMMARY_BYTES {
            break;
        }
        bytes += next_bytes;
        if let Some(capture_number) = live_capture_number
            && live_number_ids.insert(id)
        {
            live_numbers.push((id, capture_number));
        }
        let entry_id = value.id();
        if metadata_ids.insert(entry_id) {
            let meta = row_metadata_for_entry_cached(
                &transaction,
                &value,
                &pending_by_id,
                &mut chunk_cache,
            )?;
            row_metadata.push((entry_id, meta));
        }
        page.push(value);
    }
    ensure!(
        !page.is_empty() || query.offset >= total,
        "HTTP 历史分页未取得完整摘要；未省略后续记录"
    );
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    drop(rows);
    drop(statement);
    transaction.commit()?;
    Ok(HttpHistoryResult {
        total,
        rows: page,
        selected_rows,
        selected_matches,
        selected_ranks,
        live_annotations,
        live_numbers,
        row_metadata,
    })
}

fn read_history_row(
    connection: &Connection,
    source: Option<Uuid>,
    row: &Row<'_>,
) -> Result<HistoryRow> {
    let kind: u8 = row.get(0)?;
    let id = Uuid::parse_str(row.get_ref(1)?.as_str()?)?;
    if kind == 1 {
        let mut summary: FlowSummary = serde_json::from_slice(row.get_ref(2)?.as_blob()?)?;
        ensure!(
            summary.id == id && summary.source == TrafficSource::Proxy,
            "HTTP 历史摘要身份不一致"
        );
        summary.method = row.get(3)?;
        summary.url = row.get(4)?;
        summary.status = row.get(5)?;
        Ok(HistoryRow::Live(summary))
    } else {
        proxy_row(connection, source.context("HTTP 归档来源缺失")?, id)
    }
}

fn summary_bytes(row: &HistoryRow) -> Result<usize> {
    Ok(match row {
        HistoryRow::Live(row) => serde_json::to_vec(row)?.len() + 32,
        HistoryRow::ExtProxy(row) => serde_json::to_vec(row)?.len() + 128,
    })
}

#[cfg(test)]
mod tests;
