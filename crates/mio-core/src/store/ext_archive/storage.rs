#[cfg(test)]
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension};
#[cfg(test)]
use rusqlite::params;

#[cfg(test)]
use super::chunks::{chunk_length, encode_chunk};
use super::*;

#[cfg(test)]
pub(in crate::store) fn write_chunk(
    connection: &Connection,
    source_id: Uuid,
    ordinal: u64,
    raw: &[u8],
) -> Result<()> {
    ensure!(
        !raw.is_empty() && raw.len() <= EXT_CHUNK_BYTES,
        "外部归档块必须为 1 字节至 1 MiB"
    );
    let state = source_state(connection, source_id)?;
    ensure!(!state.complete, "已完成的 外部源归档不可修改");
    ensure!(ordinal <= state.next_chunk, "外部归档块不连续");
    ensure!(
        raw.len() == chunk_length(state.source_bytes, ordinal)?,
        "外部归档块长度与源文件范围不一致"
    );
    if ordinal < state.next_chunk {
        ensure!(
            read_chunk(connection, source_id, state.source_bytes, ordinal)? == raw,
            "外部归档块内容冲突；未替换原数据"
        );
        return Ok(());
    }
    let (codec, data) = encode_chunk(raw)?;
    connection.execute(
        "INSERT INTO ext_source_chunks(source,ordinal,raw_bytes,codec,data) VALUES(?1,?2,?3,?4,?5)",
        params![source_id.to_string(), i64::try_from(ordinal)?, raw.len() as i64, codec, data],
    )?;
    connection.execute(
        "UPDATE ext_sources SET written_bytes=written_bytes+?2,next_chunk=next_chunk+1 WHERE id=?1",
        params![source_id.to_string(), raw.len() as i64],
    )?;
    Ok(())
}

#[cfg(test)]
pub(in crate::store) fn finish_source_in(
    transaction: &rusqlite::Transaction<'_>,
    source_id: Uuid,
    report: &ExtImportReport,
) -> Result<()> {
    ensure!(
        report.complete_archive,
        "完成 外部导入时必须明确源文件归档完整"
    );
    let bytes = encode_report(report)?;
    let state = source_state(transaction, source_id)?;
    let previous = read_report(transaction, source_id)?;
    if state.complete {
        ensure!(previous == *report, "已完成的 外部导入报告不可修改");
        return Ok(());
    }
    ensure!(
        report.source_bytes == state.source_bytes
            && report.schema_floor == previous.schema_floor
            && report.schema_revision == previous.schema_revision,
        "外部导入报告与源文件不一致"
    );
    ensure!(
        state.written_bytes == state.source_bytes,
        "External 源文件归档尚未完整写入"
    );
    let (count, size, last): (u64, u64, Option<u64>) = transaction.query_row(
        "SELECT count(*),coalesce(sum(raw_bytes),0),max(ordinal) FROM ext_source_chunks WHERE source=?1",
        [source_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    ensure!(
        count == state.next_chunk && size == state.source_bytes && last == count.checked_sub(1),
        "外部源归档缺块或长度损坏"
    );
    let mut actual = BTreeMap::<String, (u64, u64, u64)>::new();
    {
        let mut query = transaction.prepare(
            "SELECT collection,count(*),max(item_index) FROM ext_items WHERE source=?1 GROUP BY collection",
        )?;
        for row in query.query_map([source_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })? {
            let (collection, count, last) = row?;
            actual.insert(collection, (count, 0, last + 1));
        }
        let mut query = transaction.prepare(
            "SELECT i.collection,count(*) FROM ext_messages m JOIN ext_items i ON i.id=m.item
            WHERE i.source=?1 GROUP BY i.collection",
        )?;
        for row in query.query_map([source_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })? {
            let (collection, count) = row?;
            actual.entry(collection).or_default().1 = count;
        }
    }
    for collection in &report.collections {
        let (items, messages, extent) = actual.remove(&collection.path).unwrap_or_default();
        ensure!(
            (items, messages) == (collection.indexed_items, collection.message_count),
            "External 集合报告的条目或消息数量与索引不一致"
        );
        ensure!(
            collection
                .logical_items
                .is_none_or(|logical| extent <= logical),
            "外部归档条目序号超出集合的逻辑范围"
        );
    }
    ensure!(actual.is_empty(), "外部导入报告遗漏了已索引的集合");
    transaction.execute(
        "UPDATE ext_sources SET complete=1,report=?2 WHERE id=?1",
        params![source_id.to_string(), bytes],
    )?;
    Ok(())
}

#[cfg(test)]
pub(in crate::store) fn write_item(connection: &Connection, item: &ExtItem) -> Result<()> {
    validate_item(item)?;
    let state = source_state(connection, item.source_id)?;
    ensure!(!state.complete, "已完成的 External 索引不可修改");
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM ext_items WHERE id=?1)",
        [item.id.to_string()],
        |row| row.get(0),
    )?;
    if exists {
        ensure!(
            read_item(connection, item.id)? == *item,
            "外部归档项目条目 ID 或内容冲突；未替换原数据"
        );
        return Ok(());
    }
    connection.execute(
        "INSERT INTO ext_items(id,source,collection,item_index,object_address,state,metadata)
        VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            item.id.to_string(),
            item.source_id.to_string(),
            item.collection,
            i64::try_from(item.index)?,
            item.object_address.map(i64::try_from).transpose()?,
            item.state,
            item.metadata_json
        ],
    )?;
    Ok(())
}

#[cfg(test)]
pub(in crate::store) fn write_message(
    connection: &Connection,
    message: &ExtMessageSummary,
) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    ensure!(
        bytes.len() <= MAX_SUMMARY_BYTES,
        "外部归档消息摘要超过 1 MiB 限额"
    );
    let state = source_state(connection, message.source_id)?;
    ensure!(!state.complete, "已完成的 外部归档消息索引不可修改");
    validate_message(message, state.source_bytes)?;
    let owner: Option<(String, String, u64)> = connection
        .query_row(
            "SELECT source,collection,item_index FROM ext_items WHERE id=?1",
            [message.item_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    ensure!(
        owner
            == Some((
                message.source_id.to_string(),
                message.collection.clone(),
                message.item_index
            )),
        "外部归档消息的来源、集合或条目归属不一致"
    );
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM ext_messages WHERE id=?1)",
        [message.id.to_string()],
        |row| row.get(0),
    )?;
    if exists {
        ensure!(
            read_message_summary(connection, message.id)? == *message,
            "外部归档消息 ID 或内容冲突；未替换原数据"
        );
        return Ok(());
    }
    connection.execute(
        "INSERT INTO ext_messages(id,item,field_id,summary) VALUES(?1,?2,?3,?4)",
        params![
            message.id.to_string(),
            message.item_id.to_string(),
            message.field_id,
            bytes
        ],
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(in crate::store) struct SourceState {
    pub(in crate::store) source_bytes: u64,
    #[cfg(test)]
    pub(in crate::store) written_bytes: u64,
    #[cfg(test)]
    pub(in crate::store) next_chunk: u64,
    pub(in crate::store) complete: bool,
}

pub(in crate::store) fn source_state(connection: &Connection, id: Uuid) -> Result<SourceState> {
    let (source_bytes, written_bytes, next_chunk, complete): (u64, u64, u64, u8) = connection
        .query_row(
            "SELECT source_bytes,written_bytes,next_chunk,complete FROM ext_sources WHERE id=?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?
        .context("外部源归档不存在")?;
    ensure!(
        complete <= 1
            && written_bytes <= source_bytes
            && next_chunk <= source_bytes.div_ceil(EXT_CHUNK_BYTES as u64)
            && next_chunk
                .checked_mul(EXT_CHUNK_BYTES as u64)
                .map(|bytes| bytes.min(source_bytes))
                == Some(written_bytes)
            && (complete == 0 || written_bytes == source_bytes),
        "外部源归档状态损坏"
    );
    Ok(SourceState {
        source_bytes,
        #[cfg(test)]
        written_bytes,
        #[cfg(test)]
        next_chunk,
        complete: complete == 1,
    })
}

pub(in crate::store) fn validate_collection(collection: &str) -> Result<()> {
    ensure!(
        !collection.is_empty() && collection.len() <= MAX_COLLECTION_BYTES,
        "External 集合路径为空或超过 4 KiB"
    );
    Ok(())
}

pub(in crate::store) fn encode_report(report: &ExtImportReport) -> Result<Vec<u8>> {
    i64::try_from(report.source_bytes).context("External 源文件大小溢出")?;
    let mut counts = (0u64, 0u64);
    let mut paths = std::collections::BTreeSet::new();
    for collection in &report.collections {
        validate_collection(&collection.path)?;
        ensure!(paths.insert(&collection.path), "外部导入报告含重复集合");
        i64::try_from(collection.indexed_items)?;
        i64::try_from(collection.message_count)?;
        if let Some(logical) = collection.logical_items {
            ensure!(
                collection.indexed_items <= logical,
                "External 已索引条目数超过集合逻辑长度"
            );
        }
        counts.0 = counts
            .0
            .checked_add(collection.indexed_items)
            .context("外部归档条目计数溢出")?;
        counts.1 = counts
            .1
            .checked_add(collection.message_count)
            .context("外部归档消息计数溢出")?;
    }
    ensure!(
        counts == (report.indexed_items, report.indexed_messages),
        "外部导入报告总数与集合计数不一致"
    );
    if let Some(active) = report.repeater_active_tab {
        ensure!(
            (report.schema_floor, report.schema_revision) == (229, 229),
            "External 活动 Repeater 标签只支持已验证的 schema 229"
        );
        ensure!(
            report.collections.iter().any(|collection| {
                collection.path == "root.f2.f1"
                    && collection
                        .logical_items
                        .is_some_and(|length| active < length)
            }),
            "External 活动 Repeater 标签缺少已知目录或超出源槽位范围"
        );
    }
    let bytes = serde_json::to_vec(report)?;
    ensure!(
        bytes.len() <= MAX_METADATA_BYTES,
        "外部导入报告超过 256 KiB 限额"
    );
    Ok(bytes)
}

pub(in crate::store) fn validate_item(item: &ExtItem) -> Result<()> {
    validate_collection(&item.collection)?;
    i64::try_from(item.index).context("外部归档条目序号溢出")?;
    item.object_address
        .map(i64::try_from)
        .transpose()
        .context("External 对象地址溢出")?;
    ensure!(
        !item.state.is_empty() && item.state.len() <= MAX_COLLECTION_BYTES,
        "外部归档条目状态为空或过长"
    );
    ensure!(
        item.metadata_json.len() <= MAX_METADATA_BYTES,
        "外部归档条目元数据超过 256 KiB 限额"
    );
    serde_json::from_slice::<serde_json::Value>(&item.metadata_json)
        .context("外部归档条目元数据不是有效 JSON")?;
    Ok(())
}

pub(in crate::store) fn validate_message(
    message: &ExtMessageSummary,
    source_bytes: u64,
) -> Result<()> {
    validate_collection(&message.collection)?;
    i64::try_from(message.item_index).context("外部归档条目序号溢出")?;
    ensure!(
        !message.role.is_empty() && message.role.len() <= MAX_COLLECTION_BYTES,
        "外部归档消息角色为空或过长"
    );
    checked_range(message.raw_offset, message.raw_bytes, source_bytes)?;
    ensure!(
        message
            .body_offset
            .is_none_or(|offset| offset <= message.raw_bytes),
        "External 正文偏移超出原始消息"
    );
    ensure!(
        message
            .status
            .is_none_or(|status| (100..=999).contains(&status)),
        "外部归档响应状态码损坏"
    );
    Ok(())
}
