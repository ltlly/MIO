use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

use super::archive::{ARCHIVE_SERVICE, active_ext_source, build_ext_history_row, table_exists};
use super::tree::{EntryFilters, SiteMapTree, SnapshotBuilder, parse_site_map_url};
use super::{
    MAX_PENDING_ENTRIES, MAX_SITE_MAP_SUMMARY_BYTES, SiteMapEntryDetail, SiteMapItem, SiteMapQuery,
    SiteMapSnapshot,
};

#[derive(serde::Deserialize)]
struct HeadHeadersOnly {
    #[serde(default)]
    headers: Vec<crate::Header>,
}

fn extract_live_flow_referer_and_location(
    connection: &rusqlite::Connection,
    flow_id: crate::FlowId,
    request_url: &str,
    status: Option<u16>,
) -> (Option<String>, Option<String>) {
    let raw_id = flow_id.to_string();
    let mut referer = None;
    for side in [2u8, 0u8] {
        if let Ok(Some(bytes)) = connection
            .query_row(
                "SELECT CASE WHEN typeof(head)='blob' AND length(head)<=?3 THEN head END FROM main.messages WHERE flow=?1 AND side=?2",
                params![raw_id, side, crate::store::MAX_MESSAGE_HEAD_BYTES as i64],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .map(Option::flatten)
            && let Ok(parsed) = serde_json::from_slice::<HeadHeadersOnly>(&bytes)
        {
            referer = SiteMapItem::extract_referer_from_headers(request_url, &parsed.headers);
            break;
        }
    }
    let mut location = None;
    if status.is_some_and(|code| (300..=399).contains(&code)) {
        for side in [3u8, 1u8] {
            if let Ok(Some(bytes)) = connection
                .query_row(
                    "SELECT CASE WHEN typeof(head)='blob' AND length(head)<=?3 THEN head END FROM main.messages WHERE flow=?1 AND side=?2",
                    params![raw_id, side, crate::store::MAX_MESSAGE_HEAD_BYTES as i64],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()
                .map(Option::flatten)
                && let Ok(parsed) = serde_json::from_slice::<HeadHeadersOnly>(&bytes)
            {
                location = SiteMapItem::extract_redirect_location_from_headers(
                    request_url,
                    status,
                    &parsed.headers,
                );
                break;
            }
        }
    }
    (referer, location)
}

fn validate_pending(pending: &[crate::HttpHistoryPending]) -> Result<()> {
    ensure!(
        pending.len() <= MAX_PENDING_ENTRIES,
        "Site Map 待恢复摘要超过 512 条"
    );
    let mut ids = HashSet::new();
    for pending in pending {
        let summary = &pending.summary;
        ensure!(
            !summary.id.is_nil() && summary.revision > 0 && ids.insert(summary.id),
            "Site Map 待恢复摘要身份或 revision 无效"
        );
        ensure!(
            crate::http::token(summary.method.as_bytes())
                && !summary.url.is_empty()
                && summary.url.len() <= MAX_SITE_MAP_SUMMARY_BYTES,
            "Site Map 待恢复摘要方法或 URL 无效"
        );
        ensure!(
            summary
                .status
                .is_none_or(|status| (100..=999).contains(&status)),
            "Site Map 待恢复摘要状态码无效"
        );
    }
    Ok(())
}

fn live_order(started_unix_ms: i64, flow_id: crate::FlowId) -> (Reverse<i64>, crate::FlowId) {
    (Reverse(started_unix_ms), flow_id)
}

impl<S: std::ops::Deref<Target = crate::store::ProjectStore>> crate::store::SiteMapRepo<S> {
    pub fn snapshot(
        &self,
        scope: &crate::TargetScope,
        query: &SiteMapQuery,
        pending: &[crate::HttpHistoryPending],
    ) -> Result<SiteMapSnapshot> {
        query.validate()?;
        validate_pending(pending)?;
        let transaction = self.0.connection.unchecked_transaction()?;
        let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(schema == crate::store::SCHEMA, "Site Map 项目版本已变化");
        crate::store::schema_guard::validate(&transaction)?;
        ensure!(
            crate::store::metadata::project_id(&transaction)? == self.0.id,
            "Site Map 项目 ID 已变化"
        );
        let source_id = active_ext_source(&transaction)?;
        let has_notes_table = table_exists(&transaction, "ext_http_notes")?;
        let has_highlights_table = table_exists(&transaction, "ext_http_highlights")?;
        let filters = EntryFilters::new(scope, &query.filter);
        let selected_key = query.selected_raw().map(|key| {
            parse_site_map_url(key).map_or_else(|| key.to_owned(), |parsed| parsed.full_key)
        });
        let page_end = query
            .offset
            .checked_add(query.limit)
            .context("Site Map 分页范围溢出")?;
        let mut builder = SnapshotBuilder {
            filters,
            selected_key: selected_key.as_deref(),
            offset: query.offset,
            page_end,
            tree: SiteMapTree::default(),
            total_items: 0,
            page_rows: Vec::with_capacity(query.limit),
        };
        let mut pending_rows: Vec<crate::HttpHistoryPending> = pending.to_vec();
        let mut explicit_rows = HashMap::<crate::HistoryEntryId, (crate::Uuid, u64, crate::HistoryRow)>::new();
        let mut explicit_referers = HashMap::<crate::HistoryEntryId, String>::new();
        let mut redirect_sources = HashMap::<String, String>::new();
        if table_exists(&transaction, "site_map_entries")? {
            let mut stmt = transaction.prepare(
                "SELECT id,revision,CASE WHEN typeof(row)='blob' AND length(row)<=?1 THEN row END FROM main.site_map_entries ORDER BY id",
            )?;
            let mut rows = stmt.query([MAX_SITE_MAP_SUMMARY_BYTES as i64])?;
            while let Some(raw) = rows.next()? {
                let entry_id = crate::Uuid::parse_str(&raw.get::<_, String>(0)?).context("站点地图条目 ID 损坏")?;
                let revision = u64::try_from(raw.get::<_, i64>(1)?).context("站点地图条目 revision 损坏")?;
                let bytes: Vec<u8> = raw.get::<_, Option<Vec<u8>>>(2)?.context("站点地图条目摘要损坏或超限")?;
                let mut row: crate::HistoryRow = serde_json::from_slice(&bytes).context("站点地图条目摘要 JSON 损坏")?;
                match &mut row {
                    crate::HistoryRow::ExtProxy(archive) => {
                        if let crate::HistoryRow::ExtProxy(fresh) = build_ext_history_row(
                            &transaction, archive.item.source_id, archive.item.id, has_notes_table, has_highlights_table,
                        )? {
                            archive.notes_revision = fresh.notes_revision;
                            archive.notes_preview = fresh.notes_preview;
                            archive.notes_error = fresh.notes_error;
                            archive.highlight = fresh.highlight;
                            archive.highlight_error = fresh.highlight_error;
                        }
                    }
                    crate::HistoryRow::Live(summary) => {
                        ensure!(!summary.id.is_nil() && summary.revision > 0 && !summary.url.is_empty() && crate::http::token(summary.method.as_bytes()), "站点地图实时摘要损坏");
                    }
                }
                ensure!(explicit_rows.insert(row.id(), (entry_id, revision, row)).is_none(), "站点地图显式条目重复");
            }
            let mut d_stmt = transaction.prepare(
                "SELECT CASE WHEN typeof(detail)='blob' AND length(detail)<=?1 THEN detail END FROM main.site_map_entries ORDER BY id",
            )?;
            let mut d_rows = d_stmt.query([super::MAX_ENTRY_BYTES as i64])?;
            while let Some(d_raw) = d_rows.next()? {
                let Some(d_bytes) = d_raw.get::<_, Option<Vec<u8>>>(0)? else {
                    continue;
                };
                if let Ok(detail) = serde_json::from_slice::<SiteMapEntryDetail>(&d_bytes) {
                        if let Some(ref_url) = SiteMapItem::extract_referer_from_headers(
                            &detail.record.request.url,
                            &detail.record.request.headers,
                        ) {
                            explicit_referers.insert(detail.row_id, ref_url);
                        }
                        if let Some(resp) = &detail.record.response
                            && let Some(loc_url) =
                                SiteMapItem::extract_redirect_location_from_headers(
                                    &detail.record.request.url,
                                    Some(resp.status),
                                    &resp.headers,
                                )
                        {
                        if let Some(parsed_loc) = parse_site_map_url(&loc_url) {
                            redirect_sources
                                .entry(parsed_loc.full_key)
                                .or_insert_with(|| detail.record.request.url.clone());
                        }
                            redirect_sources
                                .entry(loc_url)
                                .or_insert_with(|| detail.record.request.url.clone());
                        }
                }
            }
        }
        let mut explicit_entries: Vec<(crate::HistoryEntryId, crate::Uuid, u64)> = explicit_rows
            .iter().map(|(&id, &(entry_id, revision, _))| (id, entry_id, revision)).collect();
        explicit_entries.sort_by_key(|(_, entry_id, _)| *entry_id);
        let mut emitted = HashSet::<crate::HistoryEntryId>::new();
        let mut explicit_live: Vec<crate::FlowSummary> = explicit_rows.values().filter_map(|(_, _, row)| match row {
            crate::HistoryRow::Live(summary) if !pending_rows.iter().any(|p| p.summary.id == summary.id) => Some(summary.clone()),
            _ => None,
        }).collect();
        explicit_live.sort_by_key(|l| live_order(l.started_unix_ms, l.id));
        let mut explicit_index = 0usize;
        pending_rows.sort_by(|left, right| {
            live_order(left.summary.started_unix_ms, left.summary.id)
                .cmp(&live_order(right.summary.started_unix_ms, right.summary.id))
        });
        let pending_ids: HashSet<crate::FlowId> =
            pending_rows.iter().map(|row| row.summary.id).collect();
        let mut pending_index = 0usize;
        let mut direct_referers = explicit_referers;
        let mut ordered_urls = Vec::<(crate::HistoryEntryId, String)>::new();
        let mut flush_pending =
            |builder: &mut SnapshotBuilder<'_>,
             emitted: &mut HashSet<crate::HistoryEntryId>,
             ordered_urls: &mut Vec<(crate::HistoryEntryId, String)>,
             order: (Reverse<i64>, crate::FlowId)| {
                while pending_index < pending_rows.len() {
                    let summary = explicit_rows.get(&crate::HistoryEntryId::Live(pending_rows[pending_index].summary.id))
                        .and_then(|(_, _, row)| row.live()).unwrap_or(&pending_rows[pending_index].summary);
                    if live_order(summary.started_unix_ms, summary.id) > order {
                        break;
                    }
                    emitted.insert( crate::HistoryEntryId::Live(summary.id) );
                    ordered_urls.push((
                        crate::HistoryEntryId::Live(summary.id),
                        summary.url.clone(),
                    ));
                    builder.push_live(summary);
                    pending_index += 1;
                }
                while explicit_index < explicit_live.len() {
                    let summary = &explicit_live[explicit_index];
                    if live_order(summary.started_unix_ms, summary.id) > order {
                        break;
                    }
                    explicit_index += 1;
                    if emitted.insert(crate::HistoryEntryId::Live(summary.id)) {
                        ordered_urls.push((
                            crate::HistoryEntryId::Live(summary.id),
                            summary.url.clone(),
                        ));
                        builder.push_live(summary);
                    }
                }
            };
        {
            let mut statement = transaction.prepare(
                "SELECT f.id,f.revision,f.started,f.url,f.method,
                    CASE WHEN typeof(f.summary)='blob' AND length(f.summary)<=?1 THEN f.summary END
                 FROM flows f
                 WHERE EXISTS (SELECT 1 FROM messages m WHERE m.flow=f.id AND m.side IN (0,2)
                     AND typeof(m.head)='blob' AND length(m.head)<=?2)
                 ORDER BY f.started DESC, f.id ASC",
            )?;
            let mut rows = statement.query(params![
                MAX_SITE_MAP_SUMMARY_BYTES as i64,
                crate::store::MAX_MESSAGE_HEAD_BYTES as i64,
            ])?;
            while let Some(row) = rows.next()? {
                let raw_id: String = row.get(0)?;
                let flow_id = crate::Uuid::parse_str(&raw_id).context("流量 ID 损坏")?;
                let revision =
                    u64::try_from(row.get::<_, i64>(1)?).context("流量 revision 溢出")?;
                let started_unix_ms: i64 = row.get(2)?;
                let url: String = row.get(3)?;
                let method: String = row.get(4)?;
                let summary_bytes: Option<Vec<u8>> = row.get(5)?;
                flush_pending(
                    &mut builder,
                    &mut emitted,
                    &mut ordered_urls,
                    live_order(started_unix_ms, flow_id),
                );
                if pending_ids.contains(&flow_id) || !emitted.insert(crate::HistoryEntryId::Live(flow_id)) {
                    continue;
                }
                let bytes =
                    summary_bytes.context("HTTP 流量摘要损坏或超过 1 MiB；未省略可筛选流量")?;
                let summary: crate::FlowSummary =
                    serde_json::from_slice(&bytes).context("HTTP 流量摘要 JSON 损坏")?;
                ensure!(
                    summary.id == flow_id
                        && summary.revision == revision
                        && summary.started_unix_ms == started_unix_ms
                        && summary.url == url
                        && summary.method == method
                        && !summary.url.is_empty()
                        && crate::http::token(summary.method.as_bytes()),
                    "HTTP 流量摘要与表列或归属不一致"
                );
                builder.push_live(&summary);
                let live_id = crate::HistoryEntryId::Live(flow_id);
                let (ref_opt, loc_opt) = extract_live_flow_referer_and_location(
                    &transaction,
                    flow_id,
                    &summary.url,
                    summary.status,
                );
                if let Some(r) = ref_opt {
                    direct_referers.entry(live_id).or_insert(r);
                }
                if let Some(loc) = loc_opt {
                    if let Some(parsed_loc) = parse_site_map_url(&loc) {
                        redirect_sources
                            .entry(parsed_loc.full_key)
                            .or_insert_with(|| summary.url.clone());
                    }
                    redirect_sources
                        .entry(loc)
                        .or_insert_with(|| summary.url.clone());
                }
                ordered_urls.push((live_id, summary.url.clone()));
            }
        }
        for summary in &pending_rows[pending_index..] {
            let summary = explicit_rows.get(&crate::HistoryEntryId::Live(summary.summary.id))
                .and_then(|(_, _, row)| row.live()).unwrap_or(&summary.summary);
            if emitted.insert(crate::HistoryEntryId::Live(summary.id)) {
                ordered_urls.push((
                    crate::HistoryEntryId::Live(summary.id),
                    summary.url.clone(),
                ));
                builder.push_live(summary);
            }
        }
        for summary in &explicit_live[explicit_index..] {
            if emitted.insert(crate::HistoryEntryId::Live(summary.id)) {
                ordered_urls.push((
                    crate::HistoryEntryId::Live(summary.id),
                    summary.url.clone(),
                ));
                builder.push_live(summary);
            }
        }
        if let Some((source_id, expected_context)) = source_id {
            let sql = r#"WITH addresses AS (
                    SELECT i.id,i.collection,i.item_index,
                        CASE WHEN i.state='mapped' AND typeof(i.metadata)='blob' AND length(i.metadata)<=?3
                            AND json_valid(CAST(i.metadata AS TEXT))
                            AND json_extract(CAST(i.metadata AS TEXT),'$.context')=?4
                            AND json_extract(CAST(i.metadata AS TEXT),'$.record.mapping.state')='value'
                            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.object_type')=1
                            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.subtype')=0
                            AND json_type(CAST(i.metadata AS TEXT),'$.record.object.address')='integer'
                            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.address')>=0
                        THEN json_extract(CAST(i.metadata AS TEXT),'$.record.object.address') END AS address,
                        CASE WHEN typeof(i.metadata)='blob' AND length(i.metadata)<=?3
                            AND json_valid(CAST(i.metadata AS TEXT))
                        THEN CAST(i.metadata AS TEXT) END AS metadata_text
                    FROM ext_items i WHERE i.source=?1 AND i.collection IN ('root.f1.f0','root.f1.f1')
                ), ranked AS (
                    SELECT id,collection,item_index,address,metadata_text,
                        row_number() OVER (PARTITION BY address,collection ORDER BY item_index,id) AS alias_rank
                    FROM addresses WHERE address IS NOT NULL
                ), canonical AS (
                    SELECT r.id,r.collection,r.item_index,r.metadata_text FROM ranked r
                    WHERE r.collection='root.f1.f0' OR NOT EXISTS (
                        SELECT 1 FROM ranked first WHERE first.collection='root.f1.f0'
                            AND first.address=r.address AND first.alias_rank=r.alias_rank
                    )
                )
                SELECT c.id,
                    coalesce(json_extract(CAST(req.summary AS TEXT),'$.method'),''),
                    coalesce(json_extract(CAST(req.summary AS TEXT),'$.request_target'),''),
                    CASE WHEN json_extract(c.metadata_text,'{SERVICE}.host.observation.state')='value'
                        AND json_extract(c.metadata_text,'{SERVICE}.host.observation.value.mapping.state')='value'
                        AND json_extract(c.metadata_text,'{SERVICE}.port.observation.state')='value'
                        AND json_extract(c.metadata_text,'{SERVICE}.tls.observation.state')='value'
                    THEN (CASE json_extract(c.metadata_text,'{SERVICE}.tls.observation.value') WHEN 1 THEN 'https://' ELSE 'http://' END)
                        || (CASE WHEN instr(json_extract(c.metadata_text,'{SERVICE}.host.observation.value.mapping.value'),':')>0
                            THEN '[' || json_extract(c.metadata_text,'{SERVICE}.host.observation.value.mapping.value') || ']'
                            ELSE json_extract(c.metadata_text,'{SERVICE}.host.observation.value.mapping.value') END)
                        || ':' || json_extract(c.metadata_text,'{SERVICE}.port.observation.value') END,
                    CASE WHEN resp.id IS NOT NULL AND typeof(resp.summary)='blob' AND length(resp.summary)<=?2
                        AND json_valid(CAST(resp.summary AS TEXT))
                    THEN json_extract(CAST(resp.summary AS TEXT),'$.status') END
                FROM canonical c
                JOIN ext_messages req ON req.item=c.id AND req.field_id=15
                    AND typeof(req.summary)='blob' AND length(req.summary)<=?2
                    AND json_valid(CAST(req.summary AS TEXT))
                LEFT JOIN ext_messages resp ON resp.item=c.id AND resp.field_id=18
                ORDER BY c.item_index DESC,c.collection ASC,c.id ASC"#
            .replace("{SERVICE}", ARCHIVE_SERVICE);
            let mut statement = transaction.prepare(&sql)?;
            let mut rows = statement.query(params![
                source_id.to_string(),
                MAX_SITE_MAP_SUMMARY_BYTES as i64,
                256 * 1024,
                expected_context,
            ])?;
            while let Some(row) = rows.next()? {
                let raw_item_id: String = row.get(0)?;
                let item_id =
                    crate::Uuid::parse_str(&raw_item_id).context("外部归档 HTTP 条目 ID 损坏")?;
                let method: String = row.get(1)?;
                let target: String = row.get(2)?;
                let origin: Option<String> = row.get(3)?;
                let status: Option<u16> = row.get(4)?;
                let raw_url = if target.starts_with('/') {
                    origin.as_ref().map(|origin| format!("{origin}{target}"))
                } else if target == "*" || (!target.contains("://") && !target.is_empty()) {
                    origin.clone()
                } else if target
                    .as_bytes()
                    .get(..7)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"http://"))
                    || target
                        .as_bytes()
                        .get(..8)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"https://"))
                {
                    Some(target.clone())
                } else {
                    None
                };
                let Some(raw_url) = raw_url else {
                    continue;
                };
                let entry_id = crate::HistoryEntryId::External { source_id, item_id };
                emitted.insert(entry_id);
                if let Some((_, _, crate::HistoryRow::ExtProxy(explicit))) = explicit_rows.get(&entry_id) {
                    let req = explicit.edited_request.as_ref().or(explicit.request.as_ref());
                    let resp = explicit.edited_response.as_ref().or(explicit.response.as_ref());
                    let method = req.and_then(|r| r.method.as_deref()).unwrap_or(&method);
                    let target = req.and_then(|r| r.request_target.as_deref()).unwrap_or(&target);
                    let url = if target.starts_with('/') { explicit.origin.as_ref().or(origin.as_ref()).map(|o| format!("{o}{target}")).unwrap_or(raw_url) } else { target.to_owned() };
                    builder.push_candidate(&url, method, resp.and_then(|r| r.status).or(status), || Ok(crate::HistoryRow::ExtProxy(explicit.clone())))?;
                    continue;
                }
                builder.push_candidate(&raw_url, &method, status, || {
                    build_ext_history_row(
                        &transaction,
                        source_id,
                        item_id,
                        has_notes_table,
                        has_highlights_table,
                    )
                })?;
            }
        }
        for (id, (_, _, row)) in &explicit_rows {
            if emitted.contains(id) { continue; }
            if let crate::HistoryRow::ExtProxy(archive) = row {
                let req = archive.edited_request.as_ref().or(archive.request.as_ref());
                let resp = archive.edited_response.as_ref().or(archive.response.as_ref());
                let method = req.and_then(|r| r.method.as_deref()).unwrap_or_default();
                let target = req.and_then(|r| r.request_target.as_deref()).unwrap_or_default();
                let url = if target.starts_with('/') { archive.origin.as_ref().map(|o| format!("{o}{target}")).unwrap_or_default() } else { target.to_owned() };
                builder.push_candidate(&url, method, resp.and_then(|r| r.status), || Ok(row.clone()))?;
            }
        }
        transaction.commit()?;
        let (nodes, truncated_nodes) = builder.tree.into_nodes(scope);
        let resolve_referer = |entry_id: crate::HistoryEntryId, url: &str| -> Option<String> {
            direct_referers
                .get(&entry_id)
                .cloned()
                .or_else(|| redirect_sources.get(url).cloned())
                .or_else(|| {
                    parse_site_map_url(url)
                        .and_then(|parsed| redirect_sources.get(&parsed.full_key).cloned())
                })
        };
        let site_map_items: Vec<SiteMapItem> = builder
            .page_rows
            .iter()
            .map(|row| {
                let full_ext_url;
                let url = match row {
                    crate::HistoryRow::Live(s) => s.url.as_str(),
                    crate::HistoryRow::ExtProxy(b) => {
                        let target = b
                            .edited_request
                            .as_ref()
                            .or(b.request.as_ref())
                            .and_then(|r| r.request_target.as_deref())
                            .unwrap_or("");
                        if target.starts_with('/')
                            && let Some(origin) = b.origin.as_deref()
                        {
                            full_ext_url = format!("{origin}{target}");
                            full_ext_url.as_str()
                        } else {
                            target
                        }
                    }
                };
                let referer_url = resolve_referer(row.id(), url);
                SiteMapItem {
                    row: row.clone(),
                    referer_url,
                }
            })
            .collect();
        let mut seen_edges = HashSet::<(Option<String>, String)>::new();
        let mut crawl_edges = Vec::new();
        for (entry_id, url) in ordered_urls {
            let referer_url = resolve_referer(entry_id, &url);
            let edge = (referer_url, url);
            if seen_edges.insert(edge.clone()) {
                crawl_edges.push(edge);
            }
        }
        Ok(SiteMapSnapshot {
            nodes,
            truncated_nodes,
            items: builder.page_rows,
            site_map_items,
            crawl_edges,
            total_items: builder.total_items,
            offset: query.offset,
            explicit_entries,
        })
    }
}
