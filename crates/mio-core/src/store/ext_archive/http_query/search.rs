use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    rc::Rc,
};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::store::ext_archive::storage::SourceState;
use crate::store::plugin_storage::LiveHttpAnnotation;

const MAX_MESSAGE_BYTES: u64 = MAX_BODY as u64 + 128 * 1024;
const MAX_CACHED_ARCHIVE_CHUNKS: usize = 4;

struct GlobalSearchCollector<'a> {
    query: &'a crate::GlobalSearchQuery,
    matcher: regex::bytes::Regex,
    scope: &'a crate::TargetScope,
    result: crate::GlobalSearchResult,
}

impl GlobalSearchCollector<'_> {
    fn add(&mut self, mut hit: crate::GlobalSearchHit, fields: &[(&str, &[u8])]) -> Result<()> {
        if self.query.in_scope_only && !self.scope.is_url_in_scope(&hit.url) {
            return Ok(());
        }
        let Some((label, bytes, found)) = fields.iter().find_map(|(label, bytes)| {
            self.matcher.find(bytes).map(|found| (*label, *bytes, found))
        }) else { return Ok(()); };
        self.result.total = self.result.total.checked_add(1).context("全局搜索计数溢出")?;
        if self.result.hits.len() < self.query.limit {
            hit.matched_in = label.into();
            let start = found.start().saturating_sub(48);
            let end = found.end().saturating_add(96).min(bytes.len());
            hit.snippet = String::from_utf8_lossy(&bytes[start..end]).chars()
                .map(|character| if character.is_control() { ' ' } else { character }).take(180).collect();
            self.result.hits.push(hit);
        }
        Ok(())
    }
}

fn global_hit(target: crate::GlobalSearchTarget, tool: &str, location: String, method: String,
    url: String, status: Option<u16>, notes: &str) -> crate::GlobalSearchHit {
    crate::GlobalSearchHit { target, tool_label: tool.into(), location_label: location,
        method_or_direction: method, url, status, matched_in: String::new(), snippet: String::new(),
        notes_preview: notes.chars().take(256).collect(), history_row: None,
        websocket_summary: None, site_map_entry: None }
}

pub(crate) fn global_search_snapshot(
    path: &std::path::Path,
    query: &crate::GlobalSearchQuery,
    pending: &[(crate::FlowRecord, crate::OriginalMessages)],
    websocket_pending: &[(crate::WebSocketSummary, crate::store::CapturedWebSocket)],
    shutdown: &CancellationToken,
    cancel: &CancellationToken,
) -> Result<crate::GlobalSearchResult> {
        let is_cancelled = || shutdown.is_cancelled() || cancel.is_cancelled();
        ensure!(!query.term.trim().is_empty() && query.term.len() <= 4096, "搜索关键词须为 1–4096 字节");
        ensure!((1..=10_000).contains(&query.limit), "全局搜索显示上限须为 1–10000");
        ensure!(!is_cancelled(), "全局搜索已取消");
        let pattern = if query.regex { query.term.clone() } else { regex::escape(&query.term) };
        let matcher = regex::bytes::RegexBuilder::new(&pattern).unicode(false)
            .case_insensitive(!query.case_sensitive).size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024).nest_limit(128).build()?;
        let connection = Connection::open_with_flags(path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        connection.busy_timeout(std::time::Duration::from_millis(500))?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
        let interrupt_shutdown = shutdown.clone();
        let interrupt_op = cancel.clone();
        connection.progress_handler(
            1000,
            Some(move || interrupt_shutdown.is_cancelled() || interrupt_op.is_cancelled()),
        );
        let transaction = connection.unchecked_transaction()?;
        let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let app: i32 = transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
        ensure!(schema == crate::store::SCHEMA && app == crate::store::APPLICATION_ID
            && crate::store::metadata::project_id(&transaction)? == query.project.id,
            "全局搜索项目身份或版本已变化");
        crate::store::schema_guard::validate(&transaction)?;
        let scope = &query.workspace.scope;
        let mut collector = GlobalSearchCollector { query, matcher, scope, result: Default::default() };
        let annotations = crate::store::plugin_storage::read_live_http_annotations(&transaction)?;
        let mut live = transaction.prepare("SELECT id,revision,summary FROM flows ORDER BY started DESC,id")?;
        let mut live_rows = live.query([])?;
        let mut seen = HashSet::new();
        let mut scan_flow = |record: &crate::FlowRecord, original: &crate::OriginalMessages| -> Result<()> {
            let summary = &record.summary;
            let notes = annotations.get(&summary.id).map_or("", |entry| entry.notes.as_str());
            let target = if summary.source == TrafficSource::Proxy {
                crate::GlobalSearchTarget::ProxyHttp(HistoryEntryId::Live(summary.id))
            } else { crate::GlobalSearchTarget::LoggerFlow(summary.id) };
            let tool = if summary.source == TrafficSource::Proxy { "Proxy · HTTP" } else { "Logger · HTTP" };
            let mut hit = global_hit(target, tool, summary.id.to_string(), summary.method.clone(),
                summary.url.clone(), summary.status, notes);
            hit.history_row = Some(HistoryRow::Live(summary.clone()));
            let request = crate::request_to_raw(&record.request);
            let response = record.response.as_ref().map(crate::response_to_raw).unwrap_or_default();
            let original_request = original.request.as_ref().map(crate::request_to_raw).unwrap_or_default();
            let original_response = original.response.as_ref().map(crate::response_to_raw).unwrap_or_default();
            collector.add(hit, &[("URL", summary.url.as_bytes()), ("请求", &request),
                ("响应", &response), ("原始请求", &original_request), ("原始响应", &original_response), ("备注", notes.as_bytes())])
        };
        for (record, original) in pending {
            ensure!(seen.insert(record.summary.id), "全局搜索待保存记录重复");
            scan_flow(record, original)?;
        }
        while let Some(row) = live_rows.next()? {
            ensure!(!is_cancelled(), "全局搜索已取消");
            let id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
            if !seen.insert(id) { continue; }
            let summary: FlowSummary = serde_json::from_slice(row.get_ref(2)?.as_blob()?)?;
            ensure!(summary.id == id && summary.revision == row.get::<_, u64>(1)?, "全局搜索流量摘要身份不一致");
            let record = crate::store::flows::read_record(&transaction, id)?.context("搜索流量消息缺失")?;
            let original = crate::store::flows::read_original(&transaction, id)?;
            scan_flow(&record, &original)?;
        }
        drop(scan_flow);
        drop(live_rows);
        drop(live);
        let mut cache = ChunkCache::default();
        if let Some(source) = proxy_source(&transaction)? {
            let mut statement = transaction.prepare(&format!("{CANONICAL} SELECT id FROM canonical ORDER BY item_index DESC,id"))?;
            let mut rows = statement.query([source.to_string()])?;
            while let Some(row) = rows.next()? {
                ensure!(!is_cancelled(), "全局搜索已取消");
                let id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
                let history = proxy_row(&transaction, source, id)?;
                let HistoryRow::ExtProxy(archive) = &history else { unreachable!() };
                let notes = read_archive_full_notes(&transaction, archive)?;
                let request = archive.request.as_ref().map(|message| read_archive_message_bytes_cached(&transaction, message, &mut cache, cancel)).transpose()?.unwrap_or_default();
                let edited_request = archive.edited_request.as_ref().map(|message| read_archive_message_bytes_cached(&transaction, message, &mut cache, cancel)).transpose()?.unwrap_or_default();
                let response = archive.response.as_ref().map(|message| read_archive_message_bytes_cached(&transaction, message, &mut cache, cancel)).transpose()?.unwrap_or_default();
                let edited_response = archive.edited_response.as_ref().map(|message| read_archive_message_bytes_cached(&transaction, message, &mut cache, cancel)).transpose()?.unwrap_or_default();
                let message = archive.request.as_ref().or(archive.edited_request.as_ref());
                let target = message.and_then(|message| message.request_target.as_deref()).unwrap_or("");
                let url = if target.starts_with('/') { format!("{}{target}", archive.origin.as_deref().unwrap_or("")) } else { target.into() };
                let method = message.and_then(|message| message.method.clone()).unwrap_or_default();
                let status = archive.response.as_ref().and_then(|message| message.status);
                let mut hit = global_hit(crate::GlobalSearchTarget::ProxyHttp(history.id()), "Proxy · HTTP 归档",
                    archive.item.index.saturating_add(1).to_string(), method, url.clone(), status, &notes);
                hit.history_row = Some(history);
                collector.add(hit, &[("URL", url.as_bytes()), ("原始请求", &request), ("修改后请求", &edited_request),
                    ("原始响应", &response), ("修改后响应", &edited_response), ("备注", notes.as_bytes())])?;
            }
        }
        for tab in &query.workspace.tabs {
            let notes = query.workspace.tab_notes.get(&tab.id).map_or("", String::as_str);
            collector.add(global_hit(crate::GlobalSearchTarget::RepeaterHttp(tab.id), "Repeater", tab.title.clone(),
                String::new(), tab.draft.target.clone(), None, notes), &[("请求", &tab.draft.raw), ("目标", tab.draft.target.as_bytes()), ("标签", tab.title.as_bytes()), ("备注", notes.as_bytes())])?;
        }
        for tab in &query.workspace.intruder.tabs {
            collector.add(global_hit(crate::GlobalSearchTarget::IntruderTab(tab.id), "Intruder", tab.title.clone(),
                String::new(), tab.spec.draft.target.clone(), None, ""), &[("请求", &tab.spec.draft.raw), ("目标", tab.spec.draft.target.as_bytes()), ("标签", tab.title.as_bytes())])?;
        }
        for tab in &query.workspace.websocket_tabs {
            let notes = query.workspace.tab_notes.get(&tab.id).map_or("", String::as_str);
            collector.add(global_hit(crate::GlobalSearchTarget::RepeaterWebSocket(tab.id), "Repeater · WebSocket", tab.title.clone(),
                "WebSocket".into(), tab.url.clone(), None, notes), &[("消息", &tab.draft_payload), ("URL", tab.url.as_bytes()), ("备注", notes.as_bytes())])?;
        }
        for item in &query.workspace.organiser_items {
            collector.add(global_hit(crate::GlobalSearchTarget::OrganiserItem(item.id), "Organiser", item.number.to_string(),
                item.method.clone(), item.url.clone(), item.status_code, &item.notes), &[("请求", &item.request_raw),
                ("响应", item.response_raw.as_deref().unwrap_or_default()), ("URL", item.url.as_bytes()), ("备注", item.notes.as_bytes())])?;
        }
        let mut explicit = transaction.prepare("SELECT id,revision,row,detail FROM site_map_entries ORDER BY id")?;
        let mut explicit_rows = explicit.query([])?;
        while let Some(row) = explicit_rows.next()? {
            ensure!(!is_cancelled(), "全局搜索已取消");
            let id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
            let revision = row.get::<_, u64>(1)?;
            let history: HistoryRow = serde_json::from_slice(row.get_ref(2)?.as_blob()?)?;
            let detail: crate::SiteMapEntryDetail = serde_json::from_slice(row.get_ref(3)?.as_blob()?)?;
            let url = detail.record.request.url.clone();
            let mut hit = global_hit(crate::GlobalSearchTarget::TargetSiteMap { node_key: url.clone(), entry_id: history.id() },
                "Target · Site map", id.to_string(), detail.record.request.method.clone(), url.clone(), detail.record.summary.status, "");
            hit.history_row = Some(history);
            hit.site_map_entry = Some((id, revision));
            let request = detail.original_request_raw.clone().or_else(|| detail.original.request.as_ref().map(crate::request_to_raw)).unwrap_or_default();
            let response = detail.original_response_raw.clone().or_else(|| detail.original.response.as_ref().map(crate::response_to_raw)).unwrap_or_default();
            collector.add(hit, &[("URL", url.as_bytes()), ("请求", &detail.request_raw),
                ("响应", detail.response_raw.as_deref().unwrap_or_default()), ("原始请求", &request), ("原始响应", &response)])?;
        }
        drop(explicit_rows);
        drop(explicit);
        let mut seen_ws = HashSet::new();
        for (summary, capture) in websocket_pending {
            ensure!(seen_ws.insert(summary.id), "全局搜索待保存 WebSocket 记录重复");
            let notes: String = transaction.query_row("SELECT COALESCE(text,'') FROM websocket_notes WHERE message=?1",
                [summary.id.to_string()], |row| row.get(0)).unwrap_or_default();
            let mut hit = global_hit(crate::GlobalSearchTarget::ProxyWebSocket(summary.id), "Proxy · WebSocket",
                summary.ordinal.to_string(), format!("{:?}", summary.direction), summary.url.clone(), None, &notes);
            let mut row = summary.clone();
            row.notes_preview = hit.notes_preview.clone();
            hit.websocket_summary = Some(row);
            let orig = capture.edited_original().unwrap_or_default();
            collector.add(hit, &[("URL", summary.url.as_bytes()), ("WebSocket 消息", &capture.record.payload),
                ("WebSocket 原始消息", orig), ("备注", notes.as_bytes())])?;
        }
        let mut ws_stmt = transaction.prepare("SELECT w.id,w.revision,w.flow,w.sequence,w.direction,w.kind,w.payload_bytes,
            w.observed,COALESCE(f.url,''),w.ordinal,COALESCE(c.id,1),c.listener_port,p.body,o.body,COALESCE(n.text,''),COALESCE(n.revision,0)
            FROM websocket_messages w LEFT JOIN flows f ON f.id=w.flow LEFT JOIN websocket_connections c ON f.id=c.flow
            LEFT JOIN websocket_payloads p ON w.id=p.message LEFT JOIN websocket_originals o ON w.id=o.message
            LEFT JOIN websocket_notes n ON w.id=n.message ORDER BY w.ordinal ASC")?;
        let mut ws_rows = ws_stmt.query([])?;
        while let Some(row) = ws_rows.next()? {
            ensure!(!is_cancelled(), "全局搜索已取消");
            let id = Uuid::parse_str(row.get_ref(0)?.as_str()?)?;
            if !seen_ws.insert(id) { continue; }
            let direction = if row.get::<_, i64>(4)? == 0 { crate::WebSocketDirection::ClientToServer } else { crate::WebSocketDirection::ServerToClient };
            let kind = match row.get::<_, i64>(5)? {
                1 => crate::WebSocketKind::Text, 2 => crate::WebSocketKind::Binary,
                8 => crate::WebSocketKind::Close, 9 => crate::WebSocketKind::Ping, _ => crate::WebSocketKind::Pong,
            };
            let url: String = row.get(8)?;
            let ordinal: u64 = row.get(9)?;
            let payload: Vec<u8> = row.get::<_, Option<Vec<u8>>>(12)?.unwrap_or_default();
            let original: Vec<u8> = row.get::<_, Option<Vec<u8>>>(13)?.unwrap_or_default();
            let notes: String = row.get(14)?;
            let mut hit = global_hit(crate::GlobalSearchTarget::ProxyWebSocket(id), "Proxy · WebSocket",
                ordinal.to_string(), format!("{direction:?}"), url.clone(), None, &notes);
            hit.websocket_summary = Some(crate::WebSocketSummary { id, ordinal, connection_id: row.get(10)?,
                listener_port: row.get(11)?, revision: row.get(1)?, flow_id: Uuid::parse_str(row.get_ref(2)?.as_str()?)?,
                sequence: row.get(3)?, direction, kind, payload_bytes: row.get(6)?, observed_unix_ms: row.get(7)?,
                url: url.clone(), edited: !original.is_empty(), notes_revision: row.get(15)?, notes_preview: hit.notes_preview.clone() });
            collector.add(hit, &[("URL", url.as_bytes()), ("WebSocket 消息", &payload),
                ("WebSocket 原始消息", &original), ("备注", notes.as_bytes())])?;
        }
        drop(ws_rows);
        drop(ws_stmt);
        if let Some(bytes) = transaction.query_row("SELECT value FROM metadata WHERE key='audit_state'",
            [], |row| row.get::<_, Vec<u8>>(0)).optional()? {
            let audit: crate::AuditStateSnapshot = serde_json::from_slice(&bytes)?;
            for finding in &audit.findings {
                collector.add(global_hit(crate::GlobalSearchTarget::AuditIssue(finding.id), "Dashboard · Issue",
                    finding.title.clone(), finding.severity().label().into(), finding.url.clone(), None, ""),
                    &[("标题", finding.title.as_bytes()), ("URL", finding.url.as_bytes()),
                      ("参数", finding.parameter.as_deref().unwrap_or_default().as_bytes()), ("证据", finding.evidence.as_bytes())])?;
            }
        }
        transaction.commit()?;
        let websocket_query = crate::WebSocketHistoryQuery { project: query.project, query_revision: query.query_revision,
            filter: crate::WebSocketHistoryFilter { term: query.term.clone(), regex: query.regex, case_sensitive: query.case_sensitive,
                ..Default::default() }, flow_id: None, offset: 0, limit: query.limit, selected_message_id: None };
        ensure!(!is_cancelled(), "全局搜索已取消");
        let websocket = if shutdown.is_cancelled() {
            bail!("全局搜索已取消");
        } else {
            crate::store::websocket_query_snapshot(path, &websocket_query, websocket_pending, cancel)?
        };
        for row in websocket.rows {
            if !seen_ws.insert(row.id) { continue; }
            if query.in_scope_only && !scope.is_url_in_scope(&row.url) { continue; }
            collector.result.total = collector.result.total.checked_add(1).context("全局搜索计数溢出")?;
            if collector.result.hits.len() < query.limit {
                let mut hit = global_hit(crate::GlobalSearchTarget::ProxyWebSocket(row.id), "Proxy · WebSocket 归档",
                    row.ordinal.to_string(), format!("{:?}", row.direction), row.url.clone(), None, &row.notes_preview);
                hit.matched_in = "WebSocket 归档消息 / 修改后消息 / 备注".into();
                hit.snippet = if row.notes_preview.is_empty() { row.url.clone() } else { row.notes_preview.clone() };
                hit.websocket_summary = Some(row);
                collector.result.hits.push(hit);
            }
        }
        ensure!(!is_cancelled(), "全局搜索已取消");
        Ok(collector.result)
}

#[derive(Default)]
pub(super) struct ChunkCache {
    states: HashMap<Uuid, SourceState>,
    chunks: VecDeque<((Uuid, u64), Rc<[u8]>)>,
}

impl ChunkCache {
    fn source_state(&mut self, connection: &Connection, source_id: Uuid) -> Result<SourceState> {
        if let Some(state) = self.states.get(&source_id) {
            return Ok(*state);
        }
        let state = source_state(connection, source_id)?;
        self.states.insert(source_id, state);
        Ok(state)
    }

    fn chunk(
        &mut self,
        connection: &Connection,
        source_id: Uuid,
        source_bytes: u64,
        ordinal: u64,
    ) -> Result<Rc<[u8]>> {
        let key = (source_id, ordinal);
        if let Some(pos) = self.chunks.iter().position(|(k, _)| *k == key) {
            let entry = self.chunks.remove(pos).unwrap();
            let block = entry.1.clone();
            self.chunks.push_back(entry);
            return Ok(block);
        }
        let block: Rc<[u8]> = read_chunk(connection, source_id, source_bytes, ordinal)?.into();
        if self.chunks.len() >= MAX_CACHED_ARCHIVE_CHUNKS {
            self.chunks.pop_front();
        }
        self.chunks.push_back((key, block.clone()));
        Ok(block)
    }
}

pub(super) fn search_page(
    connection: &rusqlite::Transaction<'_>,
    source: Option<Uuid>,
    query: &HttpHistoryQuery,
    pending: &[HttpHistoryPending],
    sql: &str,
    values: &[SqlValue],
    live_annotation_store: &BTreeMap<Uuid, LiveHttpAnnotation>,
    live_annotations: Vec<crate::LiveHttpAnnotationEntry>,
    cancel: &CancellationToken,
) -> Result<HttpHistoryResult> {
    struct MatchedCandidate {
        entry_id: HistoryEntryId,
        rank: usize,
        row: HistoryRow,
        row_bytes: usize,
        live_number: Option<(Uuid, u64)>,
    }

    let matcher = search_matcher(&query.filter)?;
    let script = (!query.filter.filter_script.trim().is_empty())
        .then_some(query.filter.filter_script.as_str());
    if let Some(script) = script {
        crate::http::validate_filter_script(script)?;
    }
    let mut chunk_cache = ChunkCache::default();
    let scope = if query.filter.in_scope_only || script.is_some() {
        Some(super::super::super::metadata::workspace(connection)?.scope)
    } else {
        None
    };
    let pending_by_id: HashMap<Uuid, &HttpHistoryPending> = pending
        .iter()
        .map(|pending| (pending.summary.id, pending))
        .collect();
    let selected_set: HashSet<HistoryEntryId> = query.selected_ids.iter().copied().collect();
    let order = order_sql(query.sort.column, query.sort.ascending);
    let mime_enabled = query.filter.mime_types != [true; 8];
    let mut live_message_stmt = (matcher.is_some() || script.is_some())
        .then(|| {
            connection.prepare(
                "SELECT side,
                    CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END,
                    CASE WHEN typeof(body)='blob' AND length(body)<=?3 THEN body END
                 FROM messages
                 WHERE flow=?1 AND side IN (0, 1, 2, 3)
                 ORDER BY side",
            )
        })
        .transpose()?;
    let mut statement = connection.prepare(&format!(
        "{sql} SELECT kind,id,summary,method,url,status,number,length,listener_port FROM filtered ORDER BY {order}"
    ))?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    let mut total = 0usize;
    let mut bytes = 0usize;
    let page_end = query
        .offset
        .checked_add(query.limit)
        .context("HTTP 历史分页范围溢出")?;
    let mut page_candidates = Vec::with_capacity(query.limit);
    let mut selected_ranks_by_id = HashMap::with_capacity(query.selected_ids.len());
    let mut selected_rows = Vec::with_capacity(query.selected_ids.len());
    let mut live_numbers = Vec::new();
    let mut live_number_ids = HashSet::new();
    let mut row_metadata = Vec::new();
    let mut metadata_ids = HashSet::new();
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let url: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
        if query.filter.in_scope_only
            && let Some(scope) = &scope
            && !scope.is_url_in_scope(&url)
        {
            continue;
        }
        let kind: u8 = row.get(0)?;
        let id = Uuid::parse_str(row.get_ref(1)?.as_str()?)?;
        let mut cached_row = None;
        if query.filter.parameterized_only {
            let is_parameterized = if url_has_parameters(&url, cancel)? {
                true
            } else if kind == 1 {
                parameterized_live_request(connection, id, pending_by_id.get(&id).copied(), cancel)?
            } else {
                let source_id = source.context("HTTP 归档来源缺失")?;
                let message: Option<String> = connection
                    .query_row(
                        "SELECT id FROM ext_messages WHERE item=?1 AND field_id=15",
                        [id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(message) = message {
                    let message = read_message_summary(connection, Uuid::parse_str(&message)?)?;
                    ensure!(
                        message.source_id == source_id,
                        "HTTP 归档原始请求来源不一致"
                    );
                    parameterized_archive_request_cached(
                        connection,
                        &message,
                        &mut chunk_cache,
                        cancel,
                    )?
                } else {
                    false
                }
            };
            if !is_parameterized {
                continue;
            }
        }
        if let Some(matcher) = &matcher {
            let raw_match = if kind == 1 {
                match_live_row(
                    live_message_stmt
                        .as_mut()
                        .expect("live message statement prepared when matcher is active"),
                    id,
                    pending_by_id.get(&id).copied(),
                    matcher,
                    cancel,
                )?
            } else {
                if cached_row.is_none() {
                    let source_id = source.context("HTTP 归档来源缺失")?;
                    cached_row = Some(proxy_row(connection, source_id, id)?);
                }
                let Some(HistoryRow::ExtProxy(archive)) = &cached_row else {
                    unreachable!()
                };
                match_archive_row(connection, archive, matcher, &mut chunk_cache, cancel)?
            };
            if raw_match == query.filter.negative_search {
                continue;
            }
        };
        if mime_enabled {
            let mime = if kind == 1 {
                history_mime_from_live(connection, &id, pending_by_id.get(&id).copied(), cancel)?
            } else {
                let source_id = source.context("HTTP 归档来源缺失")?;
                if cached_row.is_none() {
                    cached_row = Some(proxy_row(connection, source_id, id)?);
                }
                let Some(HistoryRow::ExtProxy(archive)) = &cached_row else {
                    unreachable!()
                };
                history_mime_from_archive(connection, archive, &mut chunk_cache, cancel)?
            };
            if !query.filter.mime_types[mime.index()] {
                continue;
            }
        }
        if let Some(script) = script {
            let method: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
            let status: Option<u16> = row.get(5)?;
            let length = u64::try_from(row.get::<_, Option<i64>>(7)?.unwrap_or(0).max(0))?;
            let listener_port = u16::try_from(row.get::<_, i64>(8)?).ok().filter(|p| *p > 0);
            let in_scope = scope.as_ref().is_some_and(|s| s.is_url_in_scope(&url));
            let matched = if kind == 1 {
                let (req_raw, resp_raw) = load_live_raw_messages(
                    live_message_stmt.as_mut().expect("live message statement prepared"),
                    id,
                    pending_by_id.get(&id).copied(),
                    cancel,
                )?;
                let (notes, highlight_color) = live_annotation_store
                    .get(&id)
                    .map_or(("", 0), |entry| {
                        (entry.notes.as_str(), entry.highlight)
                    });
                crate::http::eval_http_filter_script_with_highlight_color_and_cancel(
                    script,
                    &method,
                    &url,
                    status,
                    length,
                    req_raw.as_deref(),
                    resp_raw.as_deref(),
                    notes,
                    highlight_color,
                    listener_port,
                    in_scope,
                    cancel,
                )?
            } else {
                let source_id = source.context("HTTP 归档来源缺失")?;
                if cached_row.is_none() {
                    cached_row = Some(proxy_row(connection, source_id, id)?);
                }
                let Some(HistoryRow::ExtProxy(archive)) = &cached_row else {
                    unreachable!()
                };
                let req_raw = archive
                    .request
                    .as_ref()
                    .map(|m| read_archive_message_bytes_cached(connection, m, &mut chunk_cache, cancel))
                    .transpose()?;
                let resp_raw = archive
                    .response
                    .as_ref()
                    .map(|m| read_archive_message_bytes_cached(connection, m, &mut chunk_cache, cancel))
                    .transpose()?;
                let notes = read_archive_full_notes(connection, archive)?;
                let highlight_color = archive.highlight.as_ref().map_or(0, |h| h.color);
                crate::http::eval_http_filter_script_with_highlight_color_and_cancel(
                    script,
                    &method,
                    &url,
                    status,
                    length,
                    req_raw.as_deref(),
                    resp_raw.as_deref(),
                    &notes,
                    highlight_color,
                    listener_port,
                    in_scope,
                    cancel,
                )?
            };
            if !matched {
                continue;
            }
        }
        let rank = total;
        total = total.checked_add(1).context("HTTP 历史搜索匹配计数溢出")?;
        let entry_id = if kind == 1 {
            HistoryEntryId::Live(id)
        } else {
            HistoryEntryId::External {
                source_id: source.context("HTTP 归档来源缺失")?,
                item_id: id,
            }
        };
        let is_selected = selected_set.contains(&entry_id);
        if is_selected {
            selected_ranks_by_id.insert(entry_id, rank);
        }
        let in_page_window = (query.offset..page_end).contains(&rank);
        if !is_selected && !in_page_window {
            continue;
        }
        let live_number = if kind == 1 {
            Some((
                id,
                u64::try_from(row.get::<_, i64>(6)?)
                    .context("HTTP 实时历史捕获序号无效")?,
            ))
        } else {
            None
        };
        let value = match cached_row {
            Some(value) => value,
            None => read_history_row(connection, source, row)?,
        };
        let row_bytes = summary_bytes(&value)?;
        if is_selected {
            bytes = bytes
                .checked_add(row_bytes)
                .context("HTTP 所选摘要大小溢出")?;
            ensure!(
                bytes <= MAX_PAGE_SUMMARY_BYTES,
                "HTTP 所选记录摘要超过 4 MiB；未省略所选记录"
            );
            if let Some((flow_id, capture_number)) = live_number
                && live_number_ids.insert(flow_id)
            {
                live_numbers.push((flow_id, capture_number));
            }
            if metadata_ids.insert(entry_id) {
                let meta = super::row_metadata_for_entry(connection, &value, &pending_by_id)?;
                row_metadata.push((entry_id, meta));
            }
            selected_rows.push(value.clone());
        }
        if in_page_window {
            page_candidates.push(MatchedCandidate {
                entry_id,
                rank,
                row: value,
                row_bytes,
                live_number,
            });
        }
    }
    let mut page = Vec::with_capacity(page_candidates.len());
    for candidate in page_candidates {
        debug_assert!((query.offset..page_end).contains(&candidate.rank));
        let selected_index = selected_rows
            .iter()
            .position(|selected| selected.id() == candidate.entry_id);
        let (value, next_bytes) = if let Some(index) = selected_index {
            (selected_rows.remove(index), 0)
        } else {
            (candidate.row, candidate.row_bytes)
        };
        if bytes
            .checked_add(next_bytes)
            .is_none_or(|next| next > MAX_HTTP_HISTORY_SUMMARY_BYTES)
        {
            break;
        }
        bytes += next_bytes;
        if let Some((flow_id, capture_number)) = candidate.live_number
            && live_number_ids.insert(flow_id)
        {
            live_numbers.push((flow_id, capture_number));
        }
        if metadata_ids.insert(candidate.entry_id) {
            let meta = super::row_metadata_for_entry(connection, &value, &pending_by_id)?;
            row_metadata.push((candidate.entry_id, meta));
        }
        page.push(value);
    }
    ensure!(
        !page.is_empty() || query.offset >= total,
        "HTTP 历史分页未取得完整摘要；未省略后续记录"
    );
    let mut selected_ranks: Vec<_> = selected_ranks_by_id.into_iter().collect();
    selected_ranks.sort_by_key(|(_, rank)| *rank);
    let mut selected_matches: Vec<_> = selected_ranks.iter().map(|(id, _)| (*id, true)).collect();
    for id in &query.selected_ids {
        if !selected_ranks.iter().any(|(matched, _)| matched == id) {
            selected_matches.push((*id, false));
        }
    }
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

fn match_live_row(
    statement: &mut rusqlite::Statement<'_>,
    id: Uuid,
    pending: Option<&HttpHistoryPending>,
    matcher: &regex::bytes::Regex,
    cancel: &CancellationToken,
) -> Result<bool> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    if let Some(pending) = pending
        && (pending.first_request.is_some() || pending.request.is_some())
    {
        for request in [pending.first_request.as_ref(), pending.request.as_ref()]
            .into_iter()
            .flatten()
        {
            ensure!(
                request.body.len() <= MAX_BODY,
                "HTTP 恢复请求正文超过 16 MiB 限额"
            );
            ensure!(
                serde_json::to_vec(&crate::store::RequestHead::from(request))?.len()
                    <= crate::store::MAX_MESSAGE_HEAD_BYTES,
                "HTTP 恢复请求头超过 4 MiB 限额"
            );
            if match_bytes(&crate::request_to_raw(request), matcher, cancel)? {
                return Ok(true);
            }
        }
        for response in [pending.first_response.as_ref(), pending.response.as_ref()]
            .into_iter()
            .flatten()
        {
            ensure!(
                response.body.len() <= MAX_BODY,
                "HTTP 恢复响应正文超过 16 MiB 限额"
            );
            ensure!(
                serde_json::to_vec(&crate::store::ResponseHead::from(response))?.len()
                    <= crate::store::MAX_MESSAGE_HEAD_BYTES,
                "HTTP 恢复响应头超过 4 MiB 限额"
            );
            if match_bytes(&crate::response_to_raw(response), matcher, cancel)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let flow_id = id.to_string();
    let mut rows = statement.query(params![
        &flow_id,
        crate::store::MAX_MESSAGE_HEAD_BYTES as i64,
        MAX_BODY as i64,
    ])?;
    let mut has_request = false;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let side: u8 = row.get(0)?;
        let head: Option<Vec<u8>> = row.get(1)?;
        let body: Option<Vec<u8>> = row.get(2)?;
        let head = head.context("HTTP 历史消息头损坏或超过 4 MiB 限额")?;
        let body = body.context("HTTP 历史消息正文损坏或超过 16 MiB 限额")?;
        let raw = match side {
            0 | 2 => {
                if side == 0 {
                    has_request = true;
                }
                let request = super::super::super::request_from_parts((head, body))?;
                crate::request_to_raw(&request)
            }
            1 | 3 => {
                let response = super::super::super::response_from_parts((head, body))?;
                crate::response_to_raw(&response)
            }
            _ => unreachable!(),
        };
        if match_bytes(&raw, matcher, cancel)? {
            return Ok(true);
        }
    }
    ensure!(has_request, "HTTP 历史请求消息缺失");
    Ok(false)
}

fn load_live_raw_messages(
    statement: &mut rusqlite::Statement<'_>,
    id: Uuid,
    pending: Option<&HttpHistoryPending>,
    cancel: &CancellationToken,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    let mut rows = statement.query(params![
        id.to_string(),
        crate::store::MAX_MESSAGE_HEAD_BYTES as i64,
        MAX_BODY as i64,
    ])?;
    let mut req_cur = None;
    let mut req_orig = None;
    let mut resp_cur = None;
    let mut resp_orig = None;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let side: u8 = row.get(0)?;
        let head = row
            .get::<_, Option<Vec<u8>>>(1)?
            .context("HTTP 历史消息头损坏或超过 4 MiB 限额")?;
        let body = row
            .get::<_, Option<Vec<u8>>>(2)?
            .context("HTTP 历史消息正文损坏或超过 16 MiB 限额")?;
        match side {
            0 => {
                let req = super::super::super::request_from_parts((head, body))?;
                req_cur = Some(crate::request_to_raw(&req));
            }
            1 => {
                let resp = super::super::super::response_from_parts((head, body))?;
                resp_cur = Some(crate::response_to_raw(&resp));
            }
            2 => {
                let req = super::super::super::request_from_parts((head, body))?;
                req_orig = Some(crate::request_to_raw(&req));
            }
            3 => {
                let resp = super::super::super::response_from_parts((head, body))?;
                resp_orig = Some(crate::response_to_raw(&resp));
            }
            _ => {}
        }
    }
    let req_raw = if let Some(raw) = req_orig.or(req_cur) {
        Some(raw)
    } else if let Some(pending) = pending {
        let request = pending
            .first_request
            .as_ref()
            .context("HTTP 脚本筛选缺少原始请求；未使用编辑版替代")?;
        ensure!(
            request.method == pending.first_request_method
                && request.url == pending.first_request_url,
            "HTTP 脚本筛选原始请求与摘要不一致"
        );
        Some(crate::request_to_raw(request))
    } else {
        None
    };
    let resp_raw = if let Some(raw) = resp_orig.or(resp_cur) {
        Some(raw)
    } else if let Some(pending) = pending {
        if let Some(expected_status) = pending.first_response_status {
            let response = pending
                .first_response
                .as_ref()
                .or(pending.response.as_ref())
                .context("HTTP 脚本筛选缺少原始响应；未使用编辑版替代")?;
            ensure!(
                response.status == expected_status,
                "HTTP 脚本筛选原始响应与摘要不一致"
            );
            Some(crate::response_to_raw(response))
        } else {
            None
        }
    } else {
        None
    };
    Ok((req_raw, resp_raw))
}

fn read_archive_full_notes(connection: &Connection, archive: &ExtHistoryRow) -> Result<String> {
    let item = read_item(connection, archive.item.id)?;
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    Ok(super::super::http_notes::read_owned(connection, &item, &metadata, false)?.text)
}

fn match_archive_row(
    connection: &Connection,
    row: &ExtHistoryRow,
    matcher: &regex::bytes::Regex,
    chunk_cache: &mut ChunkCache,
    cancel: &CancellationToken,
) -> Result<bool> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    for (message, base) in [
        (row.request.as_ref(), None),
        (row.edited_request.as_ref(), row.request.as_ref()),
        (row.response.as_ref(), None),
        (row.edited_response.as_ref(), row.response.as_ref()),
    ] {
        let Some(message) = message else {
            continue;
        };
        if base.is_some_and(|base| {
            (base.raw_offset, base.raw_bytes) == (message.raw_offset, message.raw_bytes)
        }) {
            continue;
        }
        let bytes = read_archive_message_bytes_cached(connection, message, chunk_cache, cancel)?;
        if match_bytes(&bytes, matcher, cancel)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn read_archive_message_bytes_cached(
    connection: &Connection,
    message: &ExtMessageSummary,
    chunk_cache: &mut ChunkCache,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    ensure!(
        message.raw_bytes <= MAX_MESSAGE_BYTES,
        "外部归档消息超过可搜索大小限额"
    );
    read_archive_message_prefix_cached(connection, message, message.raw_bytes, chunk_cache, cancel)
}

pub(super) fn read_archive_message_prefix_cached(
    connection: &Connection,
    message: &ExtMessageSummary,
    prefix_bytes: u64,
    chunk_cache: &mut ChunkCache,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    ensure!(
        prefix_bytes <= message.raw_bytes,
        "外部归档读取超过消息范围"
    );
    let state = chunk_cache.source_state(connection, message.source_id)?;
    ensure!(state.complete, "External 源文件导入尚未完成");
    checked_range(message.raw_offset, message.raw_bytes, state.source_bytes)?;
    let length = usize::try_from(prefix_bytes).context("外部归档消息大小溢出")?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(length)
        .context("无法分配归档搜索缓冲区")?;
    if length == 0 {
        return Ok(raw);
    }
    let end = message.raw_offset + prefix_bytes;
    let first = message.raw_offset / EXT_CHUNK_BYTES as u64;
    for ordinal in first..end.div_ceil(EXT_CHUNK_BYTES as u64) {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let block =
            chunk_cache.chunk(connection, message.source_id, state.source_bytes, ordinal)?;
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let block_start = ordinal * EXT_CHUNK_BYTES as u64;
        let start = message.raw_offset.saturating_sub(block_start) as usize;
        let stop = (end - block_start).min(block.len() as u64) as usize;
        raw.extend_from_slice(&block[start..stop]);
    }
    ensure!(raw.len() == length, "外部归档消息读取不完整");
    Ok(raw)
}

fn match_bytes(
    payload: &[u8],
    matcher: &regex::bytes::Regex,
    cancel: &CancellationToken,
) -> Result<bool> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    let matched = matcher.is_match(payload);
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    Ok(matched)
}
