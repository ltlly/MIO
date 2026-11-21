use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};

use super::ProjectStore;
use crate::{
    ExtImportReport, ExtItem, ExtMessageFragment, ExtMessagePage, ExtMessageSummary,
    ExtSourceInfo, Uuid,
};

mod chunks;
pub(in crate::store) mod http_highlights;
pub(in crate::store) mod http_notes;
pub(crate) mod http_query;
mod items;
pub use items::{inspect_mio_ext_archive, read_mio_ext_websocket_fragment};
pub(in crate::store) mod proxy_history;
mod repeater;
mod storage;
use chunks::{MESSAGE_SELECT, read_range};
pub(in crate::store) use chunks::{
    blob_length, checked_range, encode_chunk, read_blob, read_chunk, read_item,
    read_message_summary, read_report, read_summary,
};
pub(super) use repeater::load_request as load_repeater_request;
pub(super) use repeater::repeater_target_229 as repeater_request_origin;
pub(super) use repeater::response::validate_workspace_responses;
pub(super) use repeater::response::{
    for_request as archived_response_for_request,
    validate_pair_message as validate_repeater_pair_message,
    validate_record_range as validate_repeater_record_range,
};
pub(in crate::store) use storage::{
    encode_report, source_state, validate_collection, validate_item,
};
#[cfg(test)]
use storage::{finish_source_in, write_chunk, write_item, write_message};

pub(crate) const EXT_CHUNK_BYTES: usize = 1024 * 1024;
pub(crate) const EXT_FRAGMENT_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 256 * 1024;
pub(in crate::store) const MAX_SUMMARY_BYTES: usize = 1024 * 1024;
const MAX_PAGE_SUMMARY_BYTES: usize = 4 * 1024 * 1024;
const MAX_COLLECTION_BYTES: usize = 4096;

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "CREATE TABLE ext_sources (
            id TEXT PRIMARY KEY,
            source_bytes INTEGER NOT NULL CHECK(source_bytes>=0),
            written_bytes INTEGER NOT NULL CHECK(written_bytes BETWEEN 0 AND source_bytes),
            next_chunk INTEGER NOT NULL CHECK(next_chunk>=0),
            complete INTEGER NOT NULL CHECK(complete IN (0,1)),
            report BLOB NOT NULL CHECK(typeof(report)='blob' AND length(report)<={MAX_METADATA_BYTES})
        ) WITHOUT ROWID;
        CREATE TABLE ext_source_chunks (
            source TEXT NOT NULL REFERENCES ext_sources(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal>=0),
            raw_bytes INTEGER NOT NULL CHECK(raw_bytes BETWEEN 1 AND {EXT_CHUNK_BYTES}),
            codec INTEGER NOT NULL CHECK(codec IN (0,1)),
            data BLOB NOT NULL CHECK(typeof(data)='blob' AND length(data) BETWEEN 1 AND {EXT_CHUNK_BYTES}),
            PRIMARY KEY(source,ordinal)
        ) WITHOUT ROWID;
        CREATE TABLE ext_items (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL REFERENCES ext_sources(id) ON DELETE CASCADE,
            collection TEXT NOT NULL CHECK(length(CAST(collection AS BLOB)) BETWEEN 1 AND {MAX_COLLECTION_BYTES}),
            item_index INTEGER NOT NULL CHECK(item_index>=0),
            object_address INTEGER CHECK(object_address>=0),
            state TEXT NOT NULL,
            metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<={MAX_METADATA_BYTES}),
            UNIQUE(source,collection,item_index)
        );
        CREATE TABLE ext_messages (
            id TEXT PRIMARY KEY,
            item TEXT NOT NULL REFERENCES ext_items(id) ON DELETE CASCADE,
            field_id INTEGER NOT NULL CHECK(field_id BETWEEN 0 AND 255),
            summary BLOB NOT NULL CHECK(typeof(summary)='blob' AND length(summary)<={MAX_SUMMARY_BYTES}),
            UNIQUE(item,field_id)
        );"
    ))?;
    Ok(())
}

impl crate::store::ExtArchiveRepo<&mut ProjectStore> {
    #[cfg(test)]
    pub fn begin_source(&mut self, source_id: Uuid, report: &ExtImportReport) -> Result<()> {
        ensure!(
            !report.complete_archive,
            "尚未写入的 外部源归档不能标记为完整"
        );
        let bytes = encode_report(report)?;
        let transaction = self.0.connection.transaction()?;
        let exists: bool =
            transaction.query_row("SELECT EXISTS(SELECT 1 FROM ext_sources)", [], |row| {
                row.get(0)
            })?;
        ensure!(!exists, "项目已包含 外部源归档；当前不支持合并多个来源");
        transaction.execute(
            "INSERT INTO ext_sources(id,source_bytes,written_bytes,next_chunk,complete,report)
            VALUES(?1,?2,0,0,0,?3)",
            params![
                source_id.to_string(),
                i64::try_from(report.source_bytes)?,
                bytes
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn append_source_chunk(&mut self, source_id: Uuid, ordinal: u64, raw: &[u8]) -> Result<()> {
        let transaction = self.0.connection.transaction()?;
        write_chunk(&transaction, source_id, ordinal, raw)?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn append_source_chunks(
        &mut self,
        source_id: Uuid,
        first: u64,
        chunks: &[Vec<u8>],
    ) -> Result<()> {
        ensure!(chunks.len() <= 8, "外部归档批次不得超过 8 块/8 MiB");
        let transaction = self.0.connection.transaction()?;
        for (index, chunk) in chunks.iter().enumerate() {
            let ordinal = first
                .checked_add(index as u64)
                .context("外部归档块序号溢出")?;
            write_chunk(&transaction, source_id, ordinal, chunk)?;
        }
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn put_item(&mut self, item: &ExtItem) -> Result<()> {
        self.put_batch(std::slice::from_ref(item), &[])
    }

    #[cfg(test)]
    pub fn put_message(&mut self, message: &ExtMessageSummary) -> Result<()> {
        self.put_batch(&[], std::slice::from_ref(message))
    }

    #[cfg(test)]
    pub fn put_batch(
        &mut self,
        items: &[ExtItem],
        messages: &[ExtMessageSummary],
    ) -> Result<()> {
        ensure!(
            items.len() <= 256 && messages.len() <= 512,
            "External 索引批次超过 256 条目或 512 消息限额"
        );
        let transaction = self.0.connection.transaction()?;
        for item in items {
            write_item(&transaction, item)?;
        }
        for message in messages {
            write_message(&transaction, message)?;
        }
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn finish_source(&mut self, source_id: Uuid, report: &ExtImportReport) -> Result<()> {
        let transaction = self.0.connection.transaction()?;
        finish_source_in(&transaction, source_id, report)?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn finish_source_with_repeater_workspace(
        &mut self,
        source_id: Uuid,
        report: &ExtImportReport,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<ExtImportReport> {
        crate::ext_archive::import::check_cancel(cancel)?;
        ensure!(
            matches!(
                (report.schema_floor, report.schema_revision),
                (228, 228) | (229, 229)
            ),
            "未确认的 外部归档标签恢复版本"
        );
        let transaction = self.0.connection.transaction()?;
        ensure!(
            !source_state(&transaction, source_id)?.complete,
            "已完成的 外部来源不能重复恢复或修改报告"
        );
        finish_source_in(&transaction, source_id, report)?;
        let (workspace, restored) =
            repeater::restore::build_workspace(&transaction, source_id, cancel)?;
        let mut final_report = report.clone();
        final_report.gaps.extend(restored.gaps);
        final_report.gaps.push(format!("已恢复 {} 个 Repeater 标签的已保存选择；{} 个源槽位未恢复。已索引历史和原始归档保留，历史导航使用已映射原稿。", restored.restored_tabs, restored.skipped_tabs));
        let encoded = encode_report(&final_report)?;
        transaction.execute(
            "UPDATE main.ext_sources SET report=?2 WHERE id=?1",
            params![source_id.to_string(), encoded],
        )?;
        crate::ext_archive::import::check_cancel(cancel)?;
        if restored.restored_tabs > 0 || restored.restored_groups > 0 {
            super::write_workspace(&transaction, &workspace)?;
        }
        crate::ext_archive::import::check_cancel(cancel)?;
        transaction.commit()?;
        Ok(final_report)
    }
}

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::ExtArchiveRepo<S> {
    pub fn sources(&self) -> Result<Vec<ExtSourceInfo>> {
        if let Some(direct) = self.read_direct_source()? {
            return Ok(vec![direct]);
        }
        let transaction = self.0.connection.unchecked_transaction()?;
        let ids = {
            let mut query = transaction.prepare(
                "SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END
                FROM ext_sources LIMIT 2",
            )?;
            query
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        ensure!(
            ids.len() <= 1,
            "项目包含多个 外部源归档；当前不支持合并，未省略任何来源"
        );
        let mut sources = Vec::with_capacity(ids.len());
        for id in ids {
            let id = Uuid::parse_str(&id).context("外部来源 ID 损坏")?;
            sources.push(ExtSourceInfo {
                id,
                report: read_report(&transaction, id)?,
            });
        }
        transaction.commit()?;
        Ok(sources)
    }

    #[cfg(test)]
    pub fn report(&self, source_id: Uuid) -> Result<ExtImportReport> {
        read_report(&self.0.connection, source_id)
    }

    #[cfg(test)]
    pub fn item(&self, id: Uuid) -> Result<ExtItem> {
        read_item(&self.0.connection, id)
    }

    pub fn message_page(
        &self,
        source_id: Uuid,
        collection: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ExtMessagePage> {
        ensure!((1..=500).contains(&limit), "分页大小必须为 1–500");
        if let Some(collection) = collection {
            validate_collection(collection)?;
        }
        let database_offset = i64::try_from(offset).context("External 分页偏移溢出")?;
        ensure!(
            database_offset.checked_add(limit as i64).is_some(),
            "External 分页范围溢出"
        );
        if let Some(direct) = self.read_direct_source()?
            && direct.id == source_id
        {
            return Ok(ExtMessagePage {
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
        // Keep parameter slots stable while allowing a direct collection-index seek.
        let collection_filter = if collection.is_some() {
            "i.collection=?2"
        } else {
            "?2 IS NULL"
        };
        let total: usize = transaction.query_row(
            &format!(
                "SELECT count(*) FROM ext_messages m JOIN ext_items i ON i.id=m.item
                WHERE i.source=?1 AND {collection_filter}"
            ),
            params![source_id.to_string(), collection],
            |row| row.get(0),
        )?;
        let records = {
            let mut query = transaction.prepare(&format!(
                "{MESSAGE_SELECT} WHERE i.source=?1 AND {collection_filter}
                ORDER BY i.collection,i.item_index,m.field_id LIMIT ?3 OFFSET ?4"
            ))?;
            let mut rows = query.query(params![
                source_id.to_string(),
                collection,
                limit as i64,
                database_offset,
                MAX_SUMMARY_BYTES as i64
            ])?;
            let mut records = Vec::with_capacity(limit);
            let mut summary_bytes = 0;
            while let Some(row) = rows.next()? {
                let next_bytes =
                    summary_bytes + blob_length(row, 7, MAX_SUMMARY_BYTES, "外部归档消息摘要")?;
                if next_bytes > MAX_PAGE_SUMMARY_BYTES {
                    break;
                }
                summary_bytes = next_bytes;
                records.push(read_summary(row)?);
            }
            records
        };
        let next = offset
            .checked_add(records.len())
            .context("External 续页偏移溢出")?;
        ensure!(
            !records.is_empty() || next >= total,
            "External 分页没有取得完整消息；未省略后续条目"
        );
        let next_offset = (next < total).then_some(next);
        transaction.commit()?;
        Ok(ExtMessagePage {
            total,
            rows: records,
            next_offset,
        })
    }

    #[cfg(test)]
    pub fn read_source_range(
        &self,
        source_id: Uuid,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let transaction = self.0.connection.unchecked_transaction()?;
        let bytes = read_range(&transaction, source_id, offset, length)?;
        transaction.commit()?;
        Ok(bytes)
    }

    pub fn message_fragment(
        &self,
        id: Uuid,
        offset: u64,
        length: usize,
    ) -> Result<ExtMessageFragment> {
        ensure!(
            length <= EXT_FRAGMENT_BYTES,
            "外部归档消息读取不得超过 64 KiB"
        );
        let transaction = self.0.connection.unchecked_transaction()?;
        let message = read_message_summary(&transaction, id)?;
        checked_range(offset, length as u64, message.raw_bytes)?;
        let absolute = message
            .raw_offset
            .checked_add(offset)
            .context("外部归档消息偏移溢出")?;
        let bytes = read_range(&transaction, message.source_id, absolute, length)?;
        transaction.commit()?;
        Ok(ExtMessageFragment {
            summary: message,
            offset,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests;
