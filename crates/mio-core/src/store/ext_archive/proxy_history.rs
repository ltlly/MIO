use anyhow::{Context, Result, ensure};
use rusqlite::params;

use super::{
    MAX_PAGE_SUMMARY_BYTES, MAX_SUMMARY_BYTES, ProjectStore, read_item, read_message_summary,
    read_report, source_state,
};
use crate::{
    ExtHistoryRow, ExtItemSummary, ExtMessageSummary, FlowId, FlowState, FlowSummary,
    HistoryRow, TrafficSource, Uuid,
};

mod archive;
mod bindings;
mod live_admin;

use archive::{archive_query, expected_http_context};
pub(super) use archive::{
    canonical_item_ids, notes_owner, proxy_source, read_http_item,
    validate_http_source, validate_notes_item,
};
pub(in crate::store) use archive::validate_http_item;
#[cfg(test)]
use bindings::source_time;
pub(super) use bindings::{
    proxy_row, request_origin, request_origin_from_metadata,
};
pub(in crate::store) use bindings::validate_message_binding;

const PROXY_COLLECTIONS: [&str; 2] = ["root.f1.f0", "root.f1.f1"];
const FIELDS: &str = "$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.value.mapping.value";

// Classify before excluding rows: invalid source data must not disappear as another tool.
pub(super) const LIVE_HISTORY: &str = "WITH live AS (
    SELECT id,started,summary,
        CASE
            WHEN typeof(summary)!='blob' OR length(summary)>?2 THEN NULL
            WHEN NOT json_valid(CAST(summary AS TEXT)) THEN NULL
            WHEN json_type(CAST(summary AS TEXT))!='object' THEN NULL
            WHEN (SELECT count(*) FROM json_each(CAST(summary AS TEXT)) WHERE key='source')!=1 THEN NULL
            WHEN json_type(CAST(summary AS TEXT),'$.source')='text' THEN
                CASE json_extract(CAST(summary AS TEXT),'$.source')
                    WHEN 'Proxy' THEN 1 WHEN 'Repeater' THEN 0 WHEN 'Intruder' THEN 0
                    WHEN 'Scanner' THEN 0 END
            WHEN json_type(CAST(summary AS TEXT),'$.source')='object' THEN
                CASE WHEN (SELECT count(*) FROM json_each(CAST(summary AS TEXT),'$.source'))=1
                    THEN CASE WHEN json_type(CAST(summary AS TEXT),'$.source.Plugin')='text' THEN 0 END
                END
        END AS is_proxy
    FROM flows WHERE ?1='' OR instr(lower(url),lower(?1))>0 OR instr(lower(method),lower(?1))>0
)";

// The slot address can be a forwarding record; only the resolved object identifies aliases.
pub(super) const CANONICAL: &str = "WITH addresses AS (
    SELECT i.id,i.collection,i.item_index,
        CASE WHEN i.state='mapped' AND typeof(i.metadata)='blob' AND length(i.metadata)<=262144
            AND json_valid(CAST(i.metadata AS TEXT))
            AND json_extract(CAST(i.metadata AS TEXT),'$.context') IN ('http_history_item','http_history_229')
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.mapping.state')='value'
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.object_type')=1
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.subtype')=0
            AND json_type(CAST(i.metadata AS TEXT),'$.record.object.address')='integer'
            AND typeof(json_extract(CAST(i.metadata AS TEXT),'$.record.object.address'))='integer'
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.address')>=0
        THEN json_extract(CAST(i.metadata AS TEXT),'$.record.object.address') END AS address
    FROM ext_items i WHERE i.source=?1 AND i.collection IN ('root.f1.f0','root.f1.f1')
), ranked AS (
    SELECT id,collection,item_index,address,
        row_number() OVER (PARTITION BY address,collection ORDER BY item_index,id) AS alias_rank
    FROM addresses
), canonical AS (
    SELECT r.id,r.collection,r.item_index FROM ranked r
    WHERE r.address IS NULL OR r.collection='root.f1.f0' OR NOT EXISTS (
        SELECT 1 FROM ranked first WHERE first.collection='root.f1.f0'
            AND first.address=r.address AND first.alias_rank=r.alias_rank
    )
)";

impl ProjectStore {
    pub fn history_page(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
    ) -> Result<(usize, Vec<HistoryRow>)> {
        ensure!((1..=500).contains(&limit), "分页大小必须为 1–500");
        ensure!(filter.len() <= 4096, "筛选文本过长");
        let database_offset = i64::try_from(offset).context("History 分页偏移溢出")?;
        database_offset
            .checked_add(limit as i64)
            .context("History 分页范围溢出")?;
        let transaction = self.connection.unchecked_transaction()?;
        // Gate corrupted live_http_annotations metadata before returning any history page.
        let _ = crate::store::plugin_storage::read_live_http_annotations(&transaction)?;
        let source = proxy_source(&transaction)?;
        let (live_total, invalid_sources): (usize, usize) = transaction.query_row(
            &format!(
                "{LIVE_HISTORY} SELECT count(*),count(*) FILTER (WHERE is_proxy IS NULL) FROM live"
            ),
            params![filter, MAX_SUMMARY_BYTES as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            invalid_sources == 0,
            "HTTP 历史来源摘要损坏或未知；未省略无法分类的流量"
        );
        let query = archive_query();
        let archive_total: usize = if let Some(source) = source {
            transaction.query_row(
                &format!("{query} SELECT count(*) FROM filtered"),
                params![source.to_string(), filter],
                |row| row.get(0),
            )?
        } else {
            0
        };
        let total = live_total
            .checked_add(archive_total)
            .context("History 条目计数溢出")?;
        let mut result = Vec::with_capacity(limit);
        let mut page_bytes = 0;
        let mut full = false;
        if offset < live_total {
            let mut query = transaction.prepare(&format!(
                "{LIVE_HISTORY} SELECT CASE WHEN typeof(summary)='blob' AND length(summary)<=?2 THEN summary END
                FROM live ORDER BY started DESC,id LIMIT ?3 OFFSET ?4"
            ))?;
            let mut rows = query.query(params![
                filter,
                MAX_SUMMARY_BYTES as i64,
                limit as i64,
                database_offset
            ])?;
            while let Some(row) = rows.next()? {
                let bytes: Vec<u8> = row.get(0)?;
                let summary: FlowSummary = serde_json::from_slice(&bytes)?;
                if page_bytes + bytes.len() + 32 > MAX_PAGE_SUMMARY_BYTES {
                    full = true;
                    break;
                }
                page_bytes += bytes.len() + 32;
                result.push(HistoryRow::Live(summary));
            }
        }
        if let Some(source) = source
            && !full
            && result.len() < limit
        {
            let archive_offset = offset.saturating_sub(live_total);
            let mut query = transaction.prepare(&format!(
                "{query} SELECT id FROM filtered ORDER BY item_index DESC,collection,id LIMIT ?3 OFFSET ?4"
            ))?;
            let mut rows = query.query(params![
                source.to_string(),
                filter,
                (limit - result.len()) as i64,
                i64::try_from(archive_offset)?
            ])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                ensure!(id.len() == 36, "外部归档 HTTP 历史条目 ID 损坏");
                let value = proxy_row(&transaction, source, Uuid::parse_str(&id)?)?;
                let HistoryRow::ExtProxy(archive) = &value else {
                    unreachable!()
                };
                let bytes = serde_json::to_vec(archive.as_ref())?.len() + 128;
                if page_bytes + bytes > MAX_PAGE_SUMMARY_BYTES {
                    break;
                }
                page_bytes += bytes;
                result.push(value);
            }
        }
        ensure!(
            !result.is_empty() || offset >= total,
            "History 分页未取得完整摘要；未省略后续条目"
        );
        transaction.commit()?;
        Ok((total, result))
    }
}

#[cfg(test)]
pub(super) mod tests;
