use anyhow::{Context, Result, ensure};
use rusqlite::{
    Connection,
    functions::FunctionFlags,
    params, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::*;

const TIME: &str = "$.record.mapping.value.time";
const ANNOTATIONS: &str = "$.record.mapping.value.annotations";
const HIGHLIGHT: &str = "$.record.mapping.value.highlight.observation.value";
const SERVICE: &str = "$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.value.mapping.value";

pub(crate) fn http_history_host_origin(raw: &str) -> String {
    if raw.is_empty()
        || raw.contains('\\')
        || raw.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return String::new();
    }
    let Ok(parsed) = url::Url::parse(raw) else {
        return String::new();
    };
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return String::new();
    }
    let Some(host) = parsed.host_str() else {
        return String::new();
    };
    match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

pub(crate) fn http_history_request_path(raw: &str) -> String {
    let path_and_query = crate::http_history_path_and_query(raw);
    path_and_query
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_owned()
}

fn compute_live_response_length(
    head_raw: Option<&[u8]>,
    stored_length: Option<i64>,
    fallback_body_bytes: Option<i64>,
    has_response: bool,
) -> rusqlite::Result<Option<i64>> {
    if let Some(bytes) = head_raw {
        let head: crate::store::OwnedResponseHead = serde_json::from_slice(bytes)
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        let fallback = fallback_body_bytes
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);
        let body_bytes = head.body_bytes.unwrap_or(fallback);
        let len = crate::http::intruder_response_len(
            head.version,
            head.status,
            head.reason_phrase.as_deref(),
            &head.headers,
            body_bytes,
        );
        let len_i64 =
            i64::try_from(len).map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        return Ok(Some(len_i64));
    }
    if !has_response {
        return Ok(None);
    }
    if let Some(len) = stored_length.filter(|&v| v >= 0) {
        return Ok(Some(len));
    }
    Ok(fallback_body_bytes.filter(|&v| v >= 0))
}

fn extract_path_extension_sql(raw_url: &str) -> Option<String> {
    let path_and_query = crate::http_history_path_and_query(raw_url);
    let path = path_and_query.split(['?', '#']).next().unwrap_or("");
    let segment = path.rsplit('/').next().filter(|s| !s.is_empty())?;
    let (_, ext) = segment.rsplit_once('.')?;
    if !ext.is_empty()
        && ext.len() <= 16
        && ext
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        Some(ext.to_owned())
    } else {
        None
    }
}

fn normalize_note_sql(raw: Option<&str>) -> Option<String> {
    let text = raw?;
    let normalized = text
        .chars()
        .take(256)
        .collect::<String>()
        .replace("\r\n", " ")
        .replace(['\r', '\n', '\t'], " ");
    (!normalized.is_empty()).then_some(normalized)
}

fn optional_blob_or_text<'a>(ctx: &'a rusqlite::functions::Context<'_>, idx: usize) -> rusqlite::Result<Option<&'a [u8]>> {
    match ctx.get_raw(idx) {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(Some(bytes)),
        ValueRef::Null => Ok(None),
        _ => Err(rusqlite::Error::UserFunctionError(
            "HTTP 历史标量函数二进制参数类型无效".into(),
        )),
    }
}

fn decode_stored_meta_sql(raw: Option<&[u8]>) -> Option<crate::HttpHistoryRowMetadata> {
    let bytes = raw.filter(|b| b.len() <= crate::store::messages::MAX_HTTP_HISTORY_META_BYTES)?;
    let stored: crate::store::messages::StoredHttpHistoryMetadata =
        serde_json::from_slice(bytes).ok()?;
    Some(stored.metadata)
}

fn decode_request_head_sql(raw: Option<&[u8]>) -> rusqlite::Result<Option<crate::store::OwnedRequestHead>> {
    let Some(bytes) = raw else {
        return Ok(None);
    };
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))
}

fn decode_response_head_sql(raw: Option<&[u8]>) -> rusqlite::Result<Option<crate::store::OwnedResponseHead>> {
    let Some(bytes) = raw else {
        return Ok(None);
    };
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))
}

pub(super) fn register_scalar_functions(connection: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    connection.create_scalar_function("http_history_path_and_query", 1, flags, |ctx| {
        let raw = ctx.get_raw(0).as_str().unwrap_or("");
        Ok(crate::http_history_path_and_query(raw))
    })?;
    connection.create_scalar_function("http_history_host_origin", 1, flags, |ctx| {
        let raw = ctx.get_raw(0).as_str().unwrap_or("");
        Ok(http_history_host_origin(raw))
    })?;
    connection.create_scalar_function("http_history_request_path", 1, flags, |ctx| {
        let raw = ctx.get_raw(0).as_str().unwrap_or("");
        Ok(http_history_request_path(raw))
    })?;
    connection.create_scalar_function("http_history_response_length", 4, flags, |ctx| {
        let head_raw = match ctx.get_raw(0) {
            ValueRef::Blob(bytes) => Some(bytes),
            ValueRef::Text(bytes) => Some(bytes),
            ValueRef::Null => None,
            _ => {
                return Err(rusqlite::Error::UserFunctionError(
                    "HTTP 历史原始响应头类型无效".into(),
                ));
            }
        };
        let stored_length = ctx.get::<Option<i64>>(1)?;
        let fallback_body_bytes = ctx.get::<Option<i64>>(2)?;
        let has_response = ctx.get::<Option<i64>>(3)?.unwrap_or(0) != 0;
        compute_live_response_length(head_raw, stored_length, fallback_body_bytes, has_response)
    })?;
    connection.create_scalar_function("http_history_extension", 1, flags, |ctx| {
        let raw = ctx.get_raw(0).as_str().unwrap_or("");
        Ok(extract_path_extension_sql(raw))
    })?;
    connection.create_scalar_function("http_history_ip_literal", 1, flags, |ctx| {
        let raw = ctx.get_raw(0).as_str().unwrap_or("");
        let ip = super::extract_url_ip_literal(raw);
        Ok((!ip.is_empty()).then_some(ip))
    })?;
    connection.create_scalar_function("http_history_normalize_note", 1, flags, |ctx| {
        let raw = ctx.get::<Option<String>>(0)?;
        Ok(normalize_note_sql(raw.as_deref()))
    })?;
    connection.create_scalar_function("http_history_live_params", 4, flags, |ctx| {
        let req = decode_request_head_sql(optional_blob_or_text(ctx, 0)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 1)?);
        let pending_params = ctx.get::<Option<String>>(2)?;
        let fallback_url = ctx.get_raw(3).as_str().unwrap_or("");
        let req_url = req.as_ref().map_or(fallback_url, |r| r.url.as_str());
        let text = if let Some(req) = &req
            && (req.params_known.is_some() || req.has_params.is_some() || req.params_error.is_some())
        {
            let known = req.params_known.unwrap_or(req.has_params.is_some());
            if !(req.params_error.is_none() || !known) {
                return Err(rusqlite::Error::UserFunctionError(
                    "HTTP 参数分析状态与诊断冲突".into(),
                ));
            }
            if !(!known || req.has_params.is_some()) {
                return Err(rusqlite::Error::UserFunctionError(
                    "HTTP 参数分析缺少已知结果".into(),
                ));
            }
            if !known {
                "?"
            } else if req.has_params.unwrap_or(false) {
                "✓"
            } else {
                ""
            }
        } else if req.is_none()
            && let Some(p) = pending_params.as_deref()
        {
            p
        } else if let Some(stored) = &stored {
            stored.params_cell_text()
        } else {
            match url_has_parameters(req_url, &CancellationToken::new()) {
                Ok(true) => "✓",
                Ok(false) | Err(_) => "?",
            }
        };
        Ok((!text.is_empty()).then(|| text.to_owned()))
    })?;
    connection.create_scalar_function("http_history_live_edited", 3, flags, |ctx| {
        let db_edited = ctx.get::<Option<i64>>(0)?.unwrap_or(0) != 0;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 1)?);
        let pending_edited = ctx.get::<Option<i64>>(2)?.unwrap_or(0) != 0;
        let edited = db_edited
            || pending_edited
            || stored.is_some_and(|m| m.edited || m.has_original_request || m.has_original_response);
        Ok(edited.then(|| "✓".to_owned()))
    })?;
    connection.create_scalar_function("http_history_live_mime", 3, flags, |ctx| {
        let resp = decode_response_head_sql(optional_blob_or_text(ctx, 0)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 1)?);
        let pending_mime = ctx.get::<Option<String>>(2)?;
        let mime = if let Some(resp) = &resp {
            resp.mime_type
                .clone()
                .filter(|m| !m.is_empty())
                .or_else(|| stored.as_ref().map(|s| s.mime_type.clone()).filter(|m| !m.is_empty()))
                .unwrap_or_else(|| {
                    crate::http::infer_history_display_mime_type(&resp.headers, &[]).to_owned()
                })
        } else {
            stored
                .as_ref()
                .map(|s| s.mime_type.clone())
                .filter(|m| !m.is_empty())
                .or_else(|| pending_mime.filter(|m| !m.is_empty()))
                .unwrap_or_default()
        };
        Ok((!mime.is_empty()).then_some(mime))
    })?;
    connection.create_scalar_function("http_history_live_title", 3, flags, |ctx| {
        let resp = decode_response_head_sql(optional_blob_or_text(ctx, 0)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 1)?);
        let pending_title = ctx.get::<Option<String>>(2)?;
        let (title, title_error) = if let Some(resp) = &resp {
            let mime = resp
                .mime_type
                .clone()
                .filter(|m| !m.is_empty())
                .or_else(|| stored.as_ref().map(|s| s.mime_type.clone()).filter(|m| !m.is_empty()))
                .unwrap_or_else(|| {
                    crate::http::infer_history_display_mime_type(&resp.headers, &[]).to_owned()
                });
            if resp.title_error.is_some() {
                if !resp.title.as_ref().is_none_or(String::is_empty) {
                    return Err(rusqlite::Error::UserFunctionError(
                        "HTTP 标题分析状态与诊断冲突".into(),
                    ));
                }
                (String::new(), resp.title_error.clone())
            } else if let Some(t) = &resp.title {
                (t.clone(), None)
            } else if let Some(stored) = &stored {
                (stored.title.clone(), stored.title_error.clone())
            } else {
                (String::new(), (mime == "html").then(|| "HTTP 原始响应未保存标题分析结果".into()))
            }
        } else if let Some(stored) = &stored {
            (stored.title.clone(), stored.title_error.clone())
        } else if let Some(p) = pending_title {
            (p, None)
        } else {
            (String::new(), None)
        };
        let cell = if !title.is_empty() || title_error.is_none() {
            title
        } else {
            "标题读取失败".into()
        };
        Ok((!cell.is_empty()).then_some(cell))
    })?;
    connection.create_scalar_function("http_history_live_cookies", 4, flags, |ctx| {
        let req = decode_request_head_sql(optional_blob_or_text(ctx, 0)?)?;
        let resp = decode_response_head_sql(optional_blob_or_text(ctx, 1)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 2)?);
        let pending_cookies = ctx.get::<Option<String>>(3)?;
        if req.is_none() && resp.is_none() {
            if let Some(p) = pending_cookies.filter(|s| !s.is_empty()) {
                return Ok(Some(p));
            }
            if let Some(stored) = &stored {
                let cell = if !stored.cookies.is_empty() || stored.cookies_error.is_none() {
                    stored.cookies.clone()
                } else {
                    "Cookie 解析失败".into()
                };
                return Ok((!cell.is_empty()).then_some(cell));
            }
        }
        let (cookies, cookies_error) = super::analysis_text(super::history_cookies(
            req.as_ref().map(|r| r.headers.as_slice()),
            resp.as_ref().map(|r| r.headers.as_slice()),
        ));
        let cell = if !cookies.is_empty() || cookies_error.is_none() {
            cookies
        } else {
            "Cookie 解析失败".into()
        };
        Ok((!cell.is_empty()).then_some(cell))
    })?;
    connection.create_scalar_function("http_history_live_ip", 4, flags, |ctx| {
        let req = decode_request_head_sql(optional_blob_or_text(ctx, 0)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 1)?);
        let pending_ip = ctx.get::<Option<String>>(2)?;
        let fallback_url = ctx.get_raw(3).as_str().unwrap_or("");
        let req_url = req.as_ref().map_or(fallback_url, |r| r.url.as_str());
        let ip = pending_ip
            .filter(|ip| !ip.is_empty())
            .or_else(|| req.as_ref().and_then(|r| r.remote_ip.clone()).filter(|ip| !ip.is_empty()))
            .or_else(|| stored.as_ref().map(|s| s.remote_ip.clone()).filter(|ip| !ip.is_empty()))
            .unwrap_or_else(|| super::extract_url_ip_literal(req_url));
        Ok((!ip.is_empty()).then_some(ip))
    })?;
    connection.create_scalar_function("http_history_live_response_timer", 3, flags, |ctx| {
        let resp = decode_response_head_sql(optional_blob_or_text(ctx, 0)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 1)?);
        let pending_ms = ctx.get::<Option<i64>>(2)?;
        let timer = pending_ms
            .filter(|&v| v >= 0)
            .or_else(|| {
                resp.as_ref()
                    .and_then(|r| r.response_received_ms)
                    .and_then(|v| i64::try_from(v).ok())
            })
            .or_else(|| {
                stored
                    .as_ref()
                    .and_then(|s| s.response_received_ms)
                    .and_then(|v| i64::try_from(v).ok())
            });
        Ok(timer)
    })?;
    connection.create_scalar_function("http_history_live_time", 3, flags, |ctx| {
        let started = ctx.get::<Option<i64>>(0)?;
        let req = decode_request_head_sql(optional_blob_or_text(ctx, 1)?)?;
        let stored = decode_stored_meta_sql(optional_blob_or_text(ctx, 2)?);
        let time_known = req
            .as_ref()
            .and_then(|r| r.time_known)
            .or_else(|| stored.as_ref().map(|s| s.time_known))
            .unwrap_or(true);
        Ok(time_known.then_some(started).flatten())
    })?;
    Ok(())
}

fn unfiltered_live() -> String {
    "live AS (
        SELECT id, started, summary,
               listener_port AS denorm_listener_port,
               capture_number AS denorm_capture_number,
               response_length AS denorm_response_length,
               1 AS is_proxy
        FROM flows WHERE source='Proxy'
    )"
    .to_owned()
}

fn unique_object(value: &str, limit: usize) -> String {
    format!(
        "CASE WHEN typeof({value})!='blob' OR length({value})>{limit} THEN 0
          WHEN NOT json_valid(CAST({value} AS TEXT)) THEN 0
          WHEN json_type(CAST({value} AS TEXT))!='object' THEN 0
          ELSE NOT EXISTS(SELECT 1 FROM json_tree(CAST({value} AS TEXT))
              WHERE typeof(key)='text' GROUP BY parent,key HAVING count(*)>1) END"
    )
}

pub(super) fn validate_unique_json(connection: &Connection, source: Option<Uuid>) -> Result<()> {
    let live = unique_object("summary", MAX_SUMMARY_BYTES);
    let invalid: bool = connection.query_row(
        &format!(
            "{LIVE_HISTORY} SELECT EXISTS(SELECT 1 FROM live WHERE is_proxy=1 AND NOT ({live}))"
        ),
        params!["", MAX_SUMMARY_BYTES as i64],
        |row| row.get(0),
    )?;
    ensure!(
        !invalid,
        "HTTP 历史摘要 JSON 损坏或包含重复字段；未按有歧义的值筛选"
    );
    if let Some(source) = source {
        let metadata = unique_object("i.metadata", super::super::MAX_METADATA_BYTES);
        let summary = unique_object("m.summary", MAX_SUMMARY_BYTES);
        let invalid: bool = connection.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM ext_items i
                WHERE i.source=?1 AND i.collection IN ('root.f1.f0','root.f1.f1') AND NOT ({metadata}))
                OR EXISTS(SELECT 1 FROM ext_messages m JOIN ext_items i ON i.id=m.item
                WHERE i.source=?1 AND i.collection IN ('root.f1.f0','root.f1.f1')
                    AND m.field_id IN (15,18) AND NOT ({summary}))"),
            [source.to_string()],
            |row| row.get(0),
        )?;
        ensure!(
            !invalid,
            "HTTP 归档摘要或注解 JSON 损坏或包含重复字段；未按有歧义的值筛选"
        );
    }
    Ok(())
}

pub(super) fn validate_archive_bindings(
    connection: &Connection,
    source: Uuid,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut statement = connection.prepare(&format!(
        "{CANONICAL} SELECT c.id,m.id FROM canonical c JOIN ext_messages m ON m.item=c.id
            WHERE m.field_id IN (15,18) ORDER BY c.id,m.field_id"
    ))?;
    let mut rows = statement.query([source.to_string()])?;
    let mut owner: Option<(crate::ExtItem, Value)> = None;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let item_id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
        if owner.as_ref().is_none_or(|(item, _)| item.id != item_id) {
            let item = read_item(connection, item_id)?;
            ensure!(item.source_id == source, "HTTP 归档原始消息来源不一致");
            let metadata = serde_json::from_slice(&item.metadata_json)?;
            owner = Some((item, metadata));
        }
        let message =
            read_message_summary(connection, Uuid::parse_str(row.get_ref(1)?.as_str()?)?)?;
        let (item, metadata) = owner.as_ref().expect("current message retains its item");
        if message.field_id == 15 {
            request_origin_from_metadata(item, metadata, &message)?;
        } else {
            validate_message_binding(item, metadata, &message)?;
        }
    }
    Ok(())
}

pub(super) fn validate_original_heads(
    connection: &Connection,
    sql: &str,
    values: &[SqlValue],
    cancel: &CancellationToken,
) -> Result<()> {
    let request = unique_object("request_head", crate::store::MAX_MESSAGE_HEAD_BYTES);
    let response = unique_object("response_head", crate::store::MAX_MESSAGE_HEAD_BYTES);
    let invalid: bool = connection.query_row(
        &format!(
            "{sql} SELECT EXISTS(SELECT 1 FROM live_headers WHERE
            (request_present AND NOT ({request})) OR (response_present AND NOT ({response}))
            OR (NOT request_present AND (first_method IS NULL OR first_url IS NULL)))"
        ),
        params_from_iter(values.iter()),
        |row| row.get(0),
    )?;
    ensure!(!invalid, "HTTP 历史原始消息头缺失、损坏或包含重复字段");
    let mut statement = connection.prepare(&format!(
        "{sql} SELECT request_present,request_head,response_present,response_head FROM live_headers"
    ))?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        if row.get::<_, bool>(0)? {
            let head: crate::store::OwnedRequestHead =
                serde_json::from_slice(row.get_ref(1)?.as_blob()?)
                    .map_err(|_| anyhow::anyhow!("HTTP 历史原始请求头结构无效"))?;
            ensure!(
                crate::http::token(head.method.as_bytes()) && !head.url.is_empty(),
                "HTTP 历史原始请求方法或 URL 无效"
            );
        }
        if row.get::<_, bool>(2)? {
            let head: crate::store::OwnedResponseHead =
                serde_json::from_slice(row.get_ref(3)?.as_blob()?)
                    .map_err(|_| anyhow::anyhow!("HTTP 历史原始响应头结构无效"))?;
            ensure!(
                (100..=999).contains(&head.status),
                "HTTP 历史原始响应状态码无效"
            );
            if let Some(stream) = head.stream {
                stream.validate()?;
            }
        }
    }
    Ok(())
}

pub(super) fn history_sql(
    connection: &Connection,
    query: &HttpHistoryQuery,
    source: Option<Uuid>,
    pending: &[HttpHistoryPending],
    cancel: &CancellationToken,
) -> Result<(String, Vec<SqlValue>)> {
    let canonical = CANONICAL.trim_start_matches("WITH ").replace("?1", "?3");
    let live = unfiltered_live();
    let host = format!("json_extract(metadata,'{SERVICE}.host.observation.value.mapping.value')");
    let port = format!("json_extract(metadata,'{SERVICE}.port.observation.value')");
    let tls = format!("json_extract(metadata,'{SERVICE}.tls.observation.value')");
    let (extension_mode, extensions) = extension_values(&query.filter.extensions)?;
    let status_mask = query
        .filter
        .status_classes
        .iter()
        .enumerate()
        .fold(0_i64, |mask, (index, enabled)| {
            mask | (i64::from(*enabled) << index)
        });
    let pending_by_id: HashMap<Uuid, &HttpHistoryPending> = pending
        .iter()
        .map(|pending| (pending.summary.id, pending))
        .collect();
    let deep_archive_sort = source.is_some()
        && matches!(
            query.sort.column,
            HttpHistorySortColumn::Params
                | HttpHistorySortColumn::MimeType
                | HttpHistorySortColumn::Title
                | HttpHistorySortColumn::Notes
                | HttpHistorySortColumn::Cookies
        );
    let mut archive_sort_metadata = serde_json::Map::new();
    if deep_archive_sort {
        let source_id = source.context("HTTP 归档来源缺失")?;
        let mut chunk_cache = search::ChunkCache::default();
        let mut statement = connection.prepare(&format!(
            "{CANONICAL} SELECT c.id FROM canonical c ORDER BY c.item_index,c.id"
        ))?;
        let mut rows = statement.query([source_id.to_string()])?;
        while let Some(row) = rows.next()? {
            ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
            let id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
            let history = proxy_row(connection, source_id, id)?;
            if query.sort.column == HttpHistorySortColumn::Notes {
                let HistoryRow::ExtProxy(archive) = &history else {
                    unreachable!()
                };
                let cell = if archive.notes_error.is_some() {
                    "备注读取失败".to_owned()
                } else {
                    normalize_note_sql(Some(&archive.notes_preview)).unwrap_or_default()
                };
                archive_sort_metadata.insert(id.to_string(), cell.into());
                continue;
            }
            let metadata = row_metadata_for_entry_cached(
                connection,
                &history,
                &pending_by_id,
                &mut chunk_cache,
            )?;
            let value = match query.sort.column {
                HttpHistorySortColumn::Params => {
                    metadata.params_cell_text().to_owned().into()
                }
                HttpHistorySortColumn::MimeType => metadata.mime_type.clone().into(),
                HttpHistorySortColumn::Title => {
                    if !metadata.title.is_empty() || metadata.title_error.is_none() {
                        metadata.title.clone().into()
                    } else {
                        "标题读取失败".into()
                    }
                }
                HttpHistorySortColumn::Cookies => {
                    if !metadata.cookies.is_empty() || metadata.cookies_error.is_none() {
                        metadata.cookies.clone().into()
                    } else {
                        "Cookie 解析失败".into()
                    }
                }
                _ => unreachable!("deep archive sort is restricted to four metadata columns"),
            };
            archive_sort_metadata.insert(id.to_string(), value);
        }
    }
    let request_flags = i64::from(query.filter.hide_without_response)
        | (i64::from(query.filter.hide_connect) << 1);
    let values: Vec<SqlValue> = vec![
        query.url_filter.clone().into(),
        (MAX_SUMMARY_BYTES as i64).into(),
        source.map(|id| id.to_string()).into(),
        serde_json::to_string(
            &pending
                .iter()
                .map(|pending| {
                    let pending_response_length = pending
                        .first_response
                        .as_ref()
                        .or(pending.response.as_ref())
                        .map(|resp| {
                            crate::http::intruder_response_len(
                                resp.version,
                                pending.first_response_status.unwrap_or(resp.status),
                                pending.capture_metrics.reason_phrase.as_deref(),
                                &resp.headers,
                                resp.body.len() as u64,
                            )
                        })
                        .or_else(|| {
                            pending
                                .first_response_status
                                .map(|_| pending.summary.response_bytes)
                        });
                    let first_resp = pending.first_response.as_ref().or(pending.response.as_ref());
                    let pending_params = pending.first_request.as_ref().map(|first_req| {
                        match crate::store::messages::history_has_parameters(first_req) {
                            Ok(true) => "✓",
                            Ok(false) => "",
                            Err(_) => "?",
                        }
                    });
                    let pending_edited = i64::from(
                        pending
                            .first_request
                            .as_ref()
                            .zip(pending.request.as_ref())
                            .is_some_and(|(first, cur)| cur != first)
                            || first_resp
                                .zip(pending.response.as_ref())
                                .is_some_and(|(first, cur)| cur != first),
                    );
                    let (pending_mime, pending_title) = first_resp
                        .map(|resp| {
                            let (mime, title_res) = crate::store::messages::response_history_values(
                                resp,
                                &pending.first_request_method,
                            );
                            let (title, title_err) = super::analysis_text(title_res);
                            let title_cell = if !title.is_empty() || title_err.is_none() {
                                title
                            } else {
                                "标题读取失败".into()
                            };
                            (
                                (!mime.is_empty()).then_some(mime),
                                (!title_cell.is_empty()).then_some(title_cell),
                            )
                        })
                        .unwrap_or((None, None));
                    let pending_cookies = pending.first_request.as_ref().and_then(|first_req| {
                        let (cookies, cookies_err) = super::analysis_text(super::history_cookies(
                            Some(&first_req.headers),
                            first_resp.map(|r| r.headers.as_slice()),
                        ));
                        let cell = if !cookies.is_empty() || cookies_err.is_none() {
                            cookies
                        } else {
                            "Cookie 解析失败".into()
                        };
                        (!cell.is_empty()).then_some(cell)
                    });
                    let pending_remote_ip = pending.capture_metrics.remote_ip.map(|ip| ip.to_string());
                    let pending_response_received_ms = pending
                        .capture_metrics
                        .response_received_ms
                        .and_then(|v| i64::try_from(v).ok());
                    serde_json::json!({
                        "summary": pending.summary,
                        "first_request_method": pending.first_request_method,
                        "first_request_url": pending.first_request_url,
                        "first_response_status": pending.first_response_status,
                        "listener_port": pending.listener_port,
                        "response_length": pending_response_length,
                        "params": pending_params,
                        "edited": pending_edited,
                        "mime": pending_mime,
                        "title": pending_title,
                        "cookies": pending_cookies,
                        "remote_ip": pending_remote_ip,
                        "response_received_ms": pending_response_received_ms,
                    })
                })
                .collect::<Vec<_>>(),
        )?
        .into(),
        request_flags.into(),
        status_mask.into(),
        i64::from(query.filter.only_with_notes).into(),
        i64::from(query.filter.only_highlighted).into(),
        serde_json::to_string(&(extension_mode, extensions, archive_sort_metadata))?.into(),
        query
            .filter
            .listener_port
            .map_or(SqlValue::Null, |port| (port as i64).into()),
    ];
    let active_sort = query.sort.column;
    let live_notes_sql = if active_sort == HttpHistorySortColumn::Notes {
        "http_history_normalize_note((SELECT json_extract(data,'$.\"'||live_headers.id||'\".notes') FROM live_annotations))"
    } else {
        "NULL"
    };
    let live_params_sql = if active_sort == HttpHistorySortColumn::Params {
        "http_history_live_params(request_head,stored_meta_blob,pending_params,CASE WHEN request_present THEN json_extract(CAST(request_head AS TEXT),'$.url') ELSE first_url END)"
    } else {
        "NULL"
    };
    let live_edited_sql = if active_sort == HttpHistorySortColumn::Edited {
        "http_history_live_edited(db_edited,stored_meta_blob,pending_edited)"
    } else {
        "NULL"
    };
    let live_mime_sql = if active_sort == HttpHistorySortColumn::MimeType {
        "http_history_live_mime(response_head,stored_meta_blob,pending_mime)"
    } else {
        "NULL"
    };
    let live_title_sql = if active_sort == HttpHistorySortColumn::Title {
        "http_history_live_title(response_head,stored_meta_blob,pending_title)"
    } else {
        "NULL"
    };
    let live_cookies_sql = if active_sort == HttpHistorySortColumn::Cookies {
        "http_history_live_cookies(request_head,response_head,stored_meta_blob,pending_cookies)"
    } else {
        "NULL"
    };
    let live_ip_sql = if active_sort == HttpHistorySortColumn::Ip {
        "http_history_live_ip(request_head,stored_meta_blob,pending_remote_ip,CASE WHEN request_present THEN json_extract(CAST(request_head AS TEXT),'$.url') ELSE first_url END)"
    } else {
        "NULL"
    };
    let live_response_timer_sql = if active_sort == HttpHistorySortColumn::StartResponseTimer {
        "http_history_live_response_timer(response_head,stored_meta_blob,pending_response_received_ms)"
    } else {
        "NULL"
    };
    let live_time_sql = if active_sort == HttpHistorySortColumn::Time {
        "http_history_live_time(started,request_head,stored_meta_blob)"
    } else {
        "NULL"
    };
    let live_has_notes_sql = if query.filter.only_with_notes {
        "coalesce((SELECT length(CAST(coalesce(json_extract(data,'$.\"'||live_headers.id||'\".notes'),'') AS BLOB))>0 FROM live_annotations),0)"
    } else {
        "0"
    };
    let live_highlighted_sql = if query.filter.only_highlighted {
        "coalesce((SELECT coalesce(json_extract(data,'$.\"'||live_headers.id||'\".highlight'),0)>0 FROM live_annotations),0)"
    } else {
        "0"
    };
    let live_extension_sql = if active_sort == HttpHistorySortColumn::Extension {
        "http_history_extension(original_url)"
    } else {
        "NULL"
    };
    let live_tls_sql = if active_sort == HttpHistorySortColumn::Tls {
        "CASE WHEN lower(substr(original_url,1,8))='https://' THEN '✓' END"
    } else {
        "NULL"
    };
    let archive_extension_sql = if active_sort == HttpHistorySortColumn::Extension {
        "http_history_extension(CASE WHEN substr(target,1,1)='/' THEN coalesce(origin,'')||target ELSE target END)"
    } else {
        "NULL"
    };
    let archive_tls_sql = if active_sort == HttpHistorySortColumn::Tls {
        "CASE WHEN lower(substr(CASE WHEN substr(target,1,1)='/' THEN coalesce(origin,'')||target ELSE target END,1,8))='https://' THEN '✓' END"
    } else {
        "NULL"
    };
    let request_path_sql = if extension_mode != 0 {
        "http_history_request_path(url)"
    } else {
        "''"
    };
    let sql = format!("WITH {live}, {canonical}, archive_base AS (
        SELECT coalesce(max(item_index) + 1, 0) AS base FROM canonical
    ), pending AS (
        SELECT CAST(json_extract(value,'$.summary.id') AS TEXT) AS id,CAST(json_extract(value,'$.summary') AS BLOB) AS summary,json_extract(value,'$.summary.started_unix_ms') AS started,
          json_extract(value,'$.first_request_method') AS first_method,json_extract(value,'$.first_request_url') AS first_url,json_extract(value,'$.first_response_status') AS first_status,
          json_extract(value,'$.listener_port') AS pending_listener_port,
          json_extract(value,'$.response_length') AS pending_response_length,
          json_extract(value,'$.params') AS pending_params,
          json_extract(value,'$.edited') AS pending_edited,
          json_extract(value,'$.mime') AS pending_mime,
          json_extract(value,'$.title') AS pending_title,
          json_extract(value,'$.cookies') AS pending_cookies,
          json_extract(value,'$.remote_ip') AS pending_remote_ip,
          json_extract(value,'$.response_received_ms') AS pending_response_received_ms,
          NULL AS denorm_listener_port,
          NULL AS denorm_capture_number,
          NULL AS denorm_response_length
        FROM json_each(?4)
    ), live_rows AS (
        SELECT id,started,summary,NULL AS first_method,NULL AS first_url,NULL AS first_status,NULL AS pending_listener_port,NULL AS pending_response_length,NULL AS pending_params,0 AS pending_edited,NULL AS pending_mime,NULL AS pending_title,NULL AS pending_cookies,NULL AS pending_remote_ip,NULL AS pending_response_received_ms,denorm_listener_port,denorm_capture_number,denorm_response_length FROM live WHERE is_proxy=1 AND id NOT IN (SELECT id FROM pending)
        UNION ALL SELECT id,started,summary,first_method,first_url,first_status,pending_listener_port,pending_response_length,pending_params,coalesce(pending_edited,0),pending_mime,pending_title,pending_cookies,pending_remote_ip,pending_response_received_ms,denorm_listener_port,denorm_capture_number,denorm_response_length FROM pending
    ), live_with_stored_numbers AS (
        SELECT l.*,
          CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END AS request_head,
          CASE WHEN so.flow IS NOT NULL THEN so.head ELSE sc.head END AS response_head,
          ro.flow IS NOT NULL OR rc.flow IS NOT NULL AS request_present,
          so.flow IS NOT NULL OR sc.flow IS NOT NULL AS response_present,
          ro.flow IS NOT NULL OR so.flow IS NOT NULL AS db_edited,
          hm.value AS stored_meta_blob,
          COALESCE(
            l.denorm_response_length,
            CASE WHEN hm.value IS NOT NULL AND typeof(hm.value)='blob' AND length(hm.value)<=262144 AND json_valid(CAST(hm.value AS TEXT)) AND json_type(CAST(hm.value AS TEXT))='object'
              AND json_type(CAST(hm.value AS TEXT),'$.metadata.response_length')='integer'
              AND json_extract(CAST(hm.value AS TEXT),'$.metadata.response_length')>=0
            THEN json_extract(CAST(hm.value AS TEXT),'$.metadata.response_length') END
          ) AS stored_response_length,
          COALESCE(
            l.denorm_listener_port,
            CASE WHEN hm.value IS NOT NULL AND typeof(hm.value)='blob' AND length(hm.value)<=262144 AND json_valid(CAST(hm.value AS TEXT)) AND json_type(CAST(hm.value AS TEXT))='object'
              AND json_type(CAST(hm.value AS TEXT),'$.metadata.listener_port')='integer'
              AND json_extract(CAST(hm.value AS TEXT),'$.metadata.listener_port')>0
            THEN json_extract(CAST(hm.value AS TEXT),'$.metadata.listener_port') END
          ) AS stored_listener_port,
          COALESCE(
            l.denorm_capture_number,
            CASE WHEN hm.value IS NOT NULL AND typeof(hm.value)='blob' AND length(hm.value)<=262144 AND json_valid(CAST(hm.value AS TEXT)) AND json_type(CAST(hm.value AS TEXT))='object'
              AND json_type(CAST(hm.value AS TEXT),'$.metadata.capture_number')='integer'
              AND json_extract(CAST(hm.value AS TEXT),'$.metadata.capture_number')>0
            THEN json_extract(CAST(hm.value AS TEXT),'$.metadata.capture_number') END,
            CASE WHEN (ro.flow IS NOT NULL OR rc.flow IS NOT NULL)
              AND typeof(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END)='blob'
              AND length(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END)<=4194304
              AND json_valid(CAST(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END AS TEXT))
              AND json_type(CAST(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END AS TEXT))='object'
              AND json_type(CAST(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END AS TEXT),'$.capture_number')='integer'
              AND json_extract(CAST(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END AS TEXT),'$.capture_number')>0
            THEN json_extract(CAST(CASE WHEN ro.flow IS NOT NULL THEN ro.head ELSE rc.head END AS TEXT),'$.capture_number') END
          ) AS stored_capture_number
        FROM live_rows l
        LEFT JOIN messages ro ON ro.flow=l.id AND ro.side=2
        LEFT JOIN messages rc ON rc.flow=l.id AND rc.side=0
        LEFT JOIN messages so ON so.flow=l.id AND so.side=3
        LEFT JOIN messages sc ON sc.flow=l.id AND sc.side=1
        LEFT JOIN main.metadata hm ON hm.key=('http_history_meta:'||l.id) COLLATE BINARY
    ), live_number_base AS (
        SELECT max((SELECT base FROM archive_base), coalesce(max(stored_capture_number), 0)) AS base FROM live_with_stored_numbers
    ), numbered_live AS (
        SELECT *,
          COALESCE(
            stored_capture_number,
            (SELECT base FROM live_number_base) + row_number() OVER (PARTITION BY stored_capture_number IS NULL ORDER BY started ASC, id DESC)
          ) AS capture_number
        FROM live_with_stored_numbers
    ), live_headers AS (
        SELECT * FROM numbered_live
    ), live_annotations AS (
        SELECT CASE WHEN typeof(value)='blob' AND length(value)<=8388608 AND json_valid(CAST(value AS TEXT)) AND json_type(CAST(value AS TEXT))='object'
          THEN CAST(value AS TEXT) END AS data
        FROM main.metadata WHERE key='live_http_annotations' COLLATE BINARY
    ), live_values AS (
        SELECT *,CASE WHEN request_present THEN json_extract(CAST(request_head AS TEXT),'$.method') ELSE first_method END AS original_method,
          CASE WHEN request_present THEN json_extract(CAST(request_head AS TEXT),'$.url') ELSE first_url END AS original_url,
          CASE WHEN response_present THEN json_extract(CAST(response_head AS TEXT),'$.status') ELSE first_status END AS original_status,
          CASE WHEN response_present THEN
              CASE WHEN pending_response_length IS NULL AND denorm_response_length IS NOT NULL
                THEN denorm_response_length
              ELSE http_history_response_length(
                  response_head,
                  coalesce(pending_response_length, stored_response_length),
                  CASE WHEN json_type(CAST(summary AS TEXT),'$.response_bytes')='integer' AND json_extract(CAST(summary AS TEXT),'$.response_bytes')>=0
                    THEN json_extract(CAST(summary AS TEXT),'$.response_bytes') END,
                  1
              ) END
            WHEN first_status IS NOT NULL THEN coalesce(
              pending_response_length,
              stored_response_length,
              CASE WHEN json_type(CAST(summary AS TEXT),'$.response_bytes')='integer' AND json_extract(CAST(summary AS TEXT),'$.response_bytes')>=0
                THEN json_extract(CAST(summary AS TEXT),'$.response_bytes') END
            )
          END AS original_length,
          {live_has_notes_sql} AS live_has_notes,
          {live_highlighted_sql} AS live_highlighted,
          {live_notes_sql} AS live_notes_sort,
          {live_params_sql} AS live_params_sort,
          {live_edited_sql} AS live_edited_sort,
          {live_mime_sql} AS live_mime_sort,
          {live_title_sql} AS live_title_sort,
          {live_cookies_sql} AS live_cookies_sort,
          {live_ip_sql} AS live_ip_sort,
          {live_response_timer_sql} AS live_response_timer_sort,
          {live_time_sql} AS live_time_sort
        FROM live_headers
    ), archive_input AS (
        SELECT c.id,c.collection,c.item_index,
          CASE WHEN typeof(i.metadata)='blob' AND length(i.metadata)<=262144 AND json_valid(CAST(i.metadata AS TEXT)) THEN CAST(i.metadata AS TEXT) END AS metadata,
          CASE WHEN typeof(r.summary)='blob' AND length(r.summary)<=?2 AND json_valid(CAST(r.summary AS TEXT)) THEN CAST(r.summary AS TEXT) END AS request,
          CASE WHEN typeof(s.summary)='blob' AND length(s.summary)<=?2 AND json_valid(CAST(s.summary AS TEXT)) THEN CAST(s.summary AS TEXT) END AS response,
          r.id AS request_id,s.id AS response_id,n.item AS notes_item,n.revision AS notes_revision,n.text AS note,h.item AS highlight_item,h.revision AS highlight_revision,h.color AS color,
          EXISTS(SELECT 1 FROM ext_messages em WHERE em.item=c.id AND em.field_id IN (17,20)) AS archive_edited
        FROM canonical c JOIN ext_items i ON i.id=c.id
        LEFT JOIN ext_messages r ON r.item=c.id AND r.field_id=15
        LEFT JOIN ext_messages s ON s.item=c.id AND s.field_id=18
        LEFT JOIN ext_http_notes n ON n.item=c.id LEFT JOIN ext_http_highlights h ON h.item=c.id
    ), archive_values AS (
        SELECT *,
          coalesce(json_extract(request,'$.method'),'') AS method,
          coalesce(json_extract(request,'$.request_target'),'') AS target,
          CASE WHEN json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.state')='value'
            AND json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.value.mapping.state')='value'
            AND json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.state')='value'
            AND json_extract(metadata,'$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.value.mapping.state')='value'
            AND json_extract(metadata,'{SERVICE}.host.observation.state')='value'
            AND json_extract(metadata,'{SERVICE}.host.observation.value.mapping.state')='value'
            AND json_extract(metadata,'{SERVICE}.port.observation.state')='value'
            AND json_extract(metadata,'{SERVICE}.tls.observation.state')='value'
          THEN (CASE {tls} WHEN 1 THEN 'https://' ELSE 'http://' END)||
               (CASE WHEN instr({host},':')>0 THEN '['||{host}||']' ELSE {host} END)||':'||{port} END AS origin,
          json_extract(response,'$.status') AS status,
          CASE WHEN json_extract(metadata,'{TIME}.id')=11 AND json_extract(metadata,'{TIME}.binding.source_id')=?3
              AND json_extract(metadata,'{TIME}.binding.item_id')=id AND json_extract(metadata,'{TIME}.observation.state')='value'
              AND json_type(metadata,'{TIME}.observation.value')='integer' AND typeof(json_extract(metadata,'{TIME}.observation.value'))='integer'
          THEN json_extract(metadata,'{TIME}.observation.value') END AS time,
          CASE WHEN notes_item IS NOT NULL THEN length(CAST(note AS BLOB))>0
               WHEN json_extract(metadata,'{ANNOTATIONS}.observation.state')='null' THEN 0
               WHEN json_extract(metadata,'{ANNOTATIONS}.observation.value.mapping.value.note.observation.state')='null' THEN 0
               ELSE length(CAST(json_extract(metadata,'{ANNOTATIONS}.observation.value.mapping.value.note.observation.value.mapping.value.text.value') AS BLOB))>0 END AS has_notes,
          CASE WHEN highlight_item IS NOT NULL THEN color>0 ELSE json_extract(metadata,'{HIGHLIGHT}')>0 END AS highlighted,
          http_history_normalize_note(CASE WHEN notes_item IS NOT NULL THEN note
               WHEN json_extract(metadata,'{ANNOTATIONS}.observation.state')='null' THEN NULL
               WHEN json_extract(metadata,'{ANNOTATIONS}.observation.value.mapping.value.note.observation.state')='null' THEN NULL
               ELSE json_extract(metadata,'{ANNOTATIONS}.observation.value.mapping.value.note.observation.value.mapping.value.text.value') END) AS archive_notes_sort
        FROM archive_input
    ), candidates AS (
        SELECT 1 AS kind,id,'1:'||id AS identity,summary,original_method AS method,
          original_url AS url,original_status AS status,original_length AS length,live_time_sort AS time,capture_number AS number,
          live_has_notes AS has_notes,live_highlighted AS highlighted,original_status IS NOT NULL AS has_response,
          COALESCE(pending_listener_port, denorm_listener_port, stored_listener_port, -1) AS listener_port,
          live_params_sort AS params_sort,
          live_edited_sort AS edited_sort,
          live_mime_sort AS mime_sort,
          {live_extension_sql} AS extension_sort,
          live_title_sort AS title_sort,
          live_notes_sort AS notes_sort,
          {live_tls_sql} AS tls_sort,
          live_ip_sort AS ip_sort,
          live_cookies_sort AS cookies_sort,
          live_response_timer_sort AS start_response_timer_sort,
          coalesce(json_type(CAST(summary AS TEXT),'$.id')!='text' OR json_extract(CAST(summary AS TEXT),'$.id')!=id
          OR json_type(CAST(summary AS TEXT),'$.method')!='text' OR json_type(CAST(summary AS TEXT),'$.url')!='text'
          OR json_type(CAST(summary AS TEXT),'$.started_unix_ms')!='integer' OR json_extract(CAST(summary AS TEXT),'$.started_unix_ms')!=started
          OR (json_type(CAST(summary AS TEXT),'$.status') NOT IN ('null','integer'))
          OR (json_type(CAST(summary AS TEXT),'$.status')='integer' AND json_extract(CAST(summary AS TEXT),'$.status') NOT BETWEEN 100 AND 999)
          OR json_type(CAST(summary AS TEXT),'$.response_bytes')!='integer' OR json_extract(CAST(summary AS TEXT),'$.response_bytes')<0,1) AS invalid
        FROM live_values WHERE ?1='' OR instr(lower(original_method),lower(?1))>0 OR instr(lower(original_url),lower(?1))>0
        UNION ALL
        SELECT 0,id,'0:'||?3||':'||id,NULL,method,
          CASE WHEN substr(target,1,1)='/' THEN coalesce(origin,'')||target ELSE target END,status,json_extract(response,'$.raw_bytes') AS length,time,(item_index + 1) AS number,has_notes,highlighted,response_id IS NOT NULL,
          -1,
          nullif(json_extract(?9,'$[2].\"'||id||'\"'),''),
          CASE WHEN archive_edited THEN '✓' END,
          nullif(json_extract(?9,'$[2].\"'||id||'\"'),''),
          {archive_extension_sql},
          nullif(json_extract(?9,'$[2].\"'||id||'\"'),''),
          coalesce(nullif(json_extract(?9,'$[2].\"'||id||'\"'),''),archive_notes_sort),
          {archive_tls_sql},
          http_history_ip_literal(coalesce(origin,CASE WHEN substr(target,1,1)='/' THEN coalesce(origin,'')||target ELSE target END)),
          nullif(json_extract(?9,'$[2].\"'||id||'\"'),''),
          NULL,
          metadata IS NULL OR coalesce((request_id IS NOT NULL AND (request IS NULL OR json_type(request,'$.method') NOT IN ('null','text') OR json_type(request,'$.request_target') NOT IN ('null','text')))
          OR (response_id IS NOT NULL AND (response IS NULL OR json_type(response,'$.status') NOT IN ('null','integer') OR (status IS NOT NULL AND status NOT BETWEEN 100 AND 999)
              OR json_type(response,'$.raw_bytes')!='integer' OR json_extract(response,'$.raw_bytes')<0)),1)
        FROM archive_values WHERE ?1='' OR instr(lower(method),lower(?1))>0 OR instr(lower(target),lower(?1))>0 OR instr(lower(coalesce(origin,'')||target),lower(?1))>0
    ), urls AS (
        SELECT *,CASE WHEN instr(url,'?')>0 THEN substr(url,1,instr(url,'?')-1) ELSE url END AS without_query FROM candidates
    ), paths AS (
        SELECT *,{request_path_sql} AS request_path FROM urls
    ), filtered AS (
        SELECT * FROM paths WHERE ((?5 & 1)=0 OR has_response)
        AND ((?5 & 2)=0 OR coalesce(upper(method),'')!='CONNECT')
        AND (NOT has_response OR ?6=15 OR CASE WHEN status BETWEEN 200 AND 599 THEN (?6 & (1 << (status/100-2)))!=0 ELSE 0 END)
        AND (?7=0 OR has_notes) AND (?8=0 OR highlighted)
        AND (json_extract(?9,'$[0]')=0 OR
            EXISTS(SELECT 1 FROM json_each(?9,'$[1]') ext WHERE lower(CASE WHEN instr(request_path,'#')>0 THEN substr(request_path,1,instr(request_path,'#')-1) ELSE request_path END) GLOB '*.'||ext.value) = (json_extract(?9,'$[0]')=1))
        AND (?10 IS NULL OR listener_port = ?10)
    )");
    Ok((sql, values))
}

pub(super) fn order_sql(column: HttpHistorySortColumn, ascending: bool) -> String {
    let direction = if ascending { "ASC" } else { "DESC" };
    let key = match column {
        HttpHistorySortColumn::Number => {
            return format!("number {direction},kind {direction},identity ASC");
        }
        HttpHistorySortColumn::Host => {
            "nullif(http_history_host_origin(url),'') COLLATE NOCASE"
        }
        HttpHistorySortColumn::Method => "nullif(method,'') COLLATE NOCASE",
        HttpHistorySortColumn::Url => "nullif(http_history_path_and_query(url),'') COLLATE NOCASE",
        HttpHistorySortColumn::Params => "params_sort COLLATE NOCASE",
        HttpHistorySortColumn::Edited => "edited_sort COLLATE NOCASE",
        HttpHistorySortColumn::Status => "status",
        HttpHistorySortColumn::Length => "length",
        HttpHistorySortColumn::MimeType => "mime_sort COLLATE NOCASE",
        HttpHistorySortColumn::Extension => "extension_sort COLLATE NOCASE",
        HttpHistorySortColumn::Title => "title_sort COLLATE NOCASE",
        HttpHistorySortColumn::Notes => "notes_sort COLLATE NOCASE",
        HttpHistorySortColumn::Tls => "tls_sort COLLATE NOCASE",
        HttpHistorySortColumn::Ip => "ip_sort COLLATE NOCASE",
        HttpHistorySortColumn::Cookies => "cookies_sort COLLATE NOCASE",
        HttpHistorySortColumn::Time => "time",
        HttpHistorySortColumn::ListenerPort => {
            "CASE WHEN listener_port > 0 THEN listener_port END"
        }
        HttpHistorySortColumn::StartResponseTimer => "start_response_timer_sort",
    };
    format!("({key}) IS NULL ASC,{key} {direction},identity ASC")
}

pub(super) fn identity_id(identity: &str) -> Result<HistoryEntryId> {
    if let Some(id) = identity.strip_prefix("1:") {
        Ok(HistoryEntryId::Live(Uuid::parse_str(id)?))
    } else {
        let (source, item) = identity
            .strip_prefix("0:")
            .and_then(|value| value.split_once(':'))
            .context("HTTP 历史选择身份损坏")?;
        Ok(HistoryEntryId::External {
            source_id: Uuid::parse_str(source)?,
            item_id: Uuid::parse_str(item)?,
        })
    }
}
