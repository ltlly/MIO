use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Row, params, types::Null};

use super::{
    MAX_COLLECTION_BYTES, MAX_METADATA_BYTES, MAX_PAGE_SUMMARY_BYTES, MAX_SUMMARY_BYTES,
    ProjectStore, blob_length, read_blob, read_summary, source_state, validate_collection,
    validate_item,
};
use crate::{ExtItem, ExtItemDetail, ExtItemPage, ExtItemSummary, ExtMessagePage, Uuid};

// Gate owner fields in SQL before rusqlite materializes strings from an untrusted project.
const ITEM_COLUMNS: &str = "
    CASE WHEN typeof(i.id)='text' AND length(CAST(i.id AS BLOB))=36 THEN i.id END,
    CASE WHEN typeof(i.source)='text' AND length(CAST(i.source AS BLOB))=36 THEN i.source END,
    CASE WHEN typeof(i.collection)='text' AND length(CAST(i.collection AS BLOB)) BETWEEN 1 AND ?5
        THEN i.collection END,
    CASE WHEN typeof(i.item_index)='integer' AND i.item_index>=0 THEN i.item_index END,
    typeof(i.object_address),
    CASE WHEN typeof(i.object_address)='integer' AND i.object_address>=0 THEN i.object_address END,
    CASE WHEN typeof(i.state)='text' AND length(CAST(i.state AS BLOB)) BETWEEN 1 AND ?5
        THEN i.state END,
    (SELECT count(*) FROM ext_messages m WHERE m.item=i.id)";

const ITEM_METADATA_COLUMNS: &str = "typeof(i.metadata),
    CASE WHEN typeof(i.metadata)='blob' THEN length(i.metadata) END,
    CASE WHEN typeof(i.metadata)='blob' THEN CASE WHEN length(i.metadata)<=?3 THEN i.metadata END END";

// Keep the parent's read_summary column layout and validation, with bounded owner fields.
const ITEM_MESSAGE_SELECT: &str = "SELECT
    CASE WHEN typeof(m.id)='text' AND length(CAST(m.id AS BLOB))=36 THEN m.id END,
    CASE WHEN typeof(m.item)='text' AND length(CAST(m.item AS BLOB))=36 THEN m.item END,
    CASE WHEN typeof(m.field_id)='integer' AND m.field_id BETWEEN 0 AND 255 THEN m.field_id END,
    CASE WHEN typeof(i.source)='text' AND length(CAST(i.source AS BLOB))=36 THEN i.source END,
    CASE WHEN typeof(i.collection)='text' AND length(CAST(i.collection AS BLOB)) BETWEEN 1 AND ?6
        THEN i.collection END,
    CASE WHEN typeof(i.item_index)='integer' AND i.item_index>=0 THEN i.item_index END,
    CASE WHEN typeof(s.source_bytes)='integer' AND s.source_bytes>=0 THEN s.source_bytes END,
    typeof(m.summary),CASE WHEN typeof(m.summary)='blob' THEN length(m.summary) END,
    CASE WHEN typeof(m.summary)='blob' THEN CASE WHEN length(m.summary)<=?5 THEN m.summary END END
    FROM ext_messages m JOIN ext_items i ON i.id=m.item JOIN ext_sources s ON s.id=i.source
    WHERE i.source=?1 AND m.item=?2 ORDER BY m.field_id LIMIT ?3 OFFSET ?4";

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::ExtArchiveRepo<S> {
    pub fn item_page(
        &self,
        source_id: Uuid,
        collection: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ExtItemPage> {
        let database_offset = checked_offset(offset, limit)?;
        if let Some(collection) = collection {
            validate_collection(collection)?;
        }
        if let Some(direct) = self.read_direct_source()?
            && direct.id == source_id
        {
            return Ok(ExtItemPage {
                total: 0,
                rows: Vec::new(),
                next_offset: None,
            });
        }
        let transaction = self.0.connection.unchecked_transaction()?;
        ensure!(
            source_state(&transaction, source_id)?.complete,
            "External 源文件导入尚未完成"
        );
        // Separate predicates retain the (source,collection,item_index) index prefix.
        let (total, predicate): (usize, &str) = if let Some(collection) = collection {
            (
                transaction.query_row(
                    "SELECT count(*) FROM ext_items WHERE source=?1 AND collection=?2",
                    params![source_id.to_string(), collection],
                    |row| row.get(0),
                )?,
                "i.source=?1 AND i.collection=?2",
            )
        } else {
            (
                transaction.query_row(
                    "SELECT count(*) FROM ext_items WHERE source=?1",
                    [source_id.to_string()],
                    |row| row.get(0),
                )?,
                "i.source=?1",
            )
        };
        let records = {
            let mut query = transaction.prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM ext_items i WHERE {predicate}
                ORDER BY i.collection,i.item_index LIMIT ?3 OFFSET ?4"
            ))?;
            let mut rows = query.query(params![
                source_id.to_string(),
                collection,
                limit as i64,
                database_offset,
                MAX_COLLECTION_BYTES as i64,
            ])?;
            let mut records = Vec::with_capacity(limit);
            let mut page_bytes = 0;
            while let Some(row) = rows.next()? {
                let record = read_item_summary(row)?;
                ensure!(record.source_id == source_id, "外部归档条目来源不一致");
                let next_bytes = page_bytes + serde_json::to_vec(&record)?.len();
                if next_bytes > MAX_PAGE_SUMMARY_BYTES {
                    break;
                }
                page_bytes = next_bytes;
                records.push(record);
            }
            records
        };
        let next_offset = continuation(offset, records.len(), total)?;
        transaction.commit()?;
        Ok(ExtItemPage {
            total,
            rows: records,
            next_offset,
        })
    }

    pub fn item_detail(
        &self,
        source_id: Uuid,
        item_id: Uuid,
        message_offset: usize,
        message_limit: usize,
    ) -> Result<ExtItemDetail> {
        let database_offset = checked_offset(message_offset, message_limit)?;
        if let Some(direct) = self.read_direct_source()?
            && direct.id == source_id
        {
            bail!("外部归档条目不存在或不属于当前来源");
        }
        let transaction = self.0.connection.unchecked_transaction()?;
        ensure!(
            source_state(&transaction, source_id)?.complete,
            "External 源文件导入尚未完成"
        );
        let (item, total) = {
            let mut query = transaction.prepare(&format!(
                "SELECT {ITEM_COLUMNS},{ITEM_METADATA_COLUMNS}
                FROM ext_items i WHERE i.source=?1 AND i.id=?2"
            ))?;
            let mut rows = query.query(params![
                source_id.to_string(),
                item_id.to_string(),
                MAX_METADATA_BYTES as i64,
                Null,
                MAX_COLLECTION_BYTES as i64,
            ])?;
            let row = rows.next()?.context("外部归档条目不存在或不属于当前来源")?;
            let summary = read_item_summary(row)?;
            ensure!(
                summary.id == item_id && summary.source_id == source_id,
                "外部归档条目身份或来源不一致"
            );
            let item = ExtItem {
                id: summary.id,
                source_id: summary.source_id,
                collection: summary.collection,
                index: summary.index,
                object_address: summary.object_address,
                state: summary.state,
                metadata_json: read_blob(row, 8, MAX_METADATA_BYTES, "外部归档条目元数据")?,
            };
            validate_item(&item)?;
            (item, summary.message_count)
        };
        let records = {
            let mut query = transaction.prepare(ITEM_MESSAGE_SELECT)?;
            let mut rows = query.query(params![
                source_id.to_string(),
                item_id.to_string(),
                message_limit as i64,
                database_offset,
                MAX_SUMMARY_BYTES as i64,
                MAX_COLLECTION_BYTES as i64,
            ])?;
            let mut records = Vec::with_capacity(message_limit);
            // Include metadata and every item field, not just the message summaries.
            let mut page_bytes = serde_json::to_vec(&item)?.len();
            ensure!(
                page_bytes <= MAX_PAGE_SUMMARY_BYTES,
                "外部归档条目详情超过读取限额"
            );
            while let Some(row) = rows.next()? {
                let next_bytes =
                    page_bytes + blob_length(row, 7, MAX_SUMMARY_BYTES, "外部归档消息摘要")?;
                if next_bytes > MAX_PAGE_SUMMARY_BYTES {
                    break;
                }
                page_bytes = next_bytes;
                records.push(read_summary(row)?);
            }
            records
        };
        let next_offset = continuation(message_offset, records.len(), total)?;
        transaction.commit()?;
        Ok(ExtItemDetail {
            item,
            messages: ExtMessagePage {
                total,
                rows: records,
                next_offset,
            },
        })
    }
}

fn checked_offset(offset: usize, limit: usize) -> Result<i64> {
    ensure!((1..=500).contains(&limit), "分页大小必须为 1–500");
    let offset = i64::try_from(offset).context("External 分页偏移溢出")?;
    ensure!(
        offset.checked_add(limit as i64).is_some(),
        "External 分页范围溢出"
    );
    Ok(offset)
}

fn continuation(offset: usize, count: usize, total: usize) -> Result<Option<usize>> {
    let next = offset.checked_add(count).context("External 续页偏移溢出")?;
    ensure!(
        count > 0 || next >= total,
        "External 分页没有取得完整记录；未省略后续条目"
    );
    Ok((next < total).then_some(next))
}

fn read_id(row: &Row<'_>, column: usize, label: &str) -> Result<Uuid> {
    let text: String = row.get(column)?;
    let id = Uuid::parse_str(&text).with_context(|| format!("{label}损坏"))?;
    ensure!(id.to_string() == text, "{label}格式损坏");
    Ok(id)
}

fn read_item_summary(row: &Row<'_>) -> Result<ExtItemSummary> {
    let object_address = match row.get::<_, String>(4)?.as_str() {
        "null" => None,
        "integer" => Some(row.get(5)?),
        _ => bail!("External 对象地址不是整数或 null"),
    };
    Ok(ExtItemSummary {
        id: read_id(row, 0, "外部归档条目 ID")?,
        source_id: read_id(row, 1, "外部来源 ID")?,
        collection: row.get(2)?,
        index: row.get(3)?,
        object_address,
        state: row.get(6)?,
        message_count: row.get(7)?,
    })
}

mod coverage;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ws_coverage;
mod websocket;

pub fn inspect_mio_ext_archive(
    path: &std::path::Path,
    ws_offset: usize,
    ws_limit: usize,
) -> Result<Option<(crate::ExtCoverageReport, crate::ExtWebSocketPage)>> {
    let store = ProjectStore::open(path)?;
    if let Some(direct) = store.ext_archive().read_direct_source()? {
        let coverage = direct.report.coverage_report();
        let ws_page = crate::ExtWebSocketPage {
            total: 0,
            rows: Vec::new(),
            next_offset: None,
        };
        return Ok(Some((coverage, ws_page)));
    }
    let mut sources = store.ext_archive().sources()?;
    let Some(source) = sources.pop() else {
        return Ok(None);
    };
    let coverage = store.ext_archive().coverage_report(source.id)?;
    let ws_page = store.ext_archive().websocket_page(source.id, ws_offset, ws_limit)?;
    Ok(Some((coverage, ws_page)))
}

pub fn read_mio_ext_websocket_fragment(
    path: &std::path::Path,
    source_id: Uuid,
    item_id: Uuid,
    edited: bool,
    offset: u64,
    length: usize,
) -> Result<crate::ExtWebSocketFragment> {
    let store = ProjectStore::open(path)?;
    store.ext_archive().websocket_fragment(source_id, item_id, edited, offset, length)
}
