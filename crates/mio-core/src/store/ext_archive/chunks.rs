use std::io::Write;

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use rusqlite::{Connection, Row, params};

use super::storage::validate_message;
use super::*;

pub(in crate::store) fn blob_length(
    row: &Row<'_>,
    start: usize,
    limit: usize,
    label: &str,
) -> Result<usize> {
    ensure!(
        row.get::<_, String>(start)? == "blob",
        "{label}不是二进制或已缺失"
    );
    let length: u64 = row.get(start + 1)?;
    ensure!(length <= limit as u64, "{label}超过读取限额");
    Ok(length as usize)
}

pub(in crate::store) fn read_blob(
    row: &Row<'_>,
    start: usize,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let length = blob_length(row, start, limit, label)?;
    let bytes = row
        .get::<_, Option<Vec<u8>>>(start + 2)?
        .with_context(|| format!("{label}缺失"))?;
    ensure!(bytes.len() == length, "{label}长度不一致");
    Ok(bytes)
}

pub(in crate::store) fn read_report(connection: &Connection, id: Uuid) -> Result<ExtImportReport> {
    let state = source_state(connection, id)?;
    let mut query = connection.prepare(
        "SELECT typeof(report),CASE WHEN typeof(report)='blob' THEN length(report) END,
        CASE WHEN typeof(report)='blob' THEN CASE WHEN length(report)<=?2 THEN report END END
        FROM ext_sources WHERE id=?1",
    )?;
    let mut rows = query.query(params![id.to_string(), MAX_METADATA_BYTES as i64])?;
    let row = rows.next()?.context("外部源归档不存在")?;
    let report: ExtImportReport =
        serde_json::from_slice(&read_blob(row, 0, MAX_METADATA_BYTES, "外部导入报告")?)?;
    encode_report(&report)?;
    ensure!(
        report.source_bytes == state.source_bytes && report.complete_archive == state.complete,
        "外部导入报告与归档状态不一致"
    );
    Ok(report)
}

pub(in crate::store) fn read_item(connection: &Connection, id: Uuid) -> Result<ExtItem> {
    let mut query = connection.prepare(
        "SELECT id,source,collection,item_index,object_address,state,typeof(metadata),
        CASE WHEN typeof(metadata)='blob' THEN length(metadata) END,
        CASE WHEN typeof(metadata)='blob' THEN CASE WHEN length(metadata)<=?2 THEN metadata END END
        FROM ext_items WHERE id=?1",
    )?;
    let mut rows = query.query(params![id.to_string(), MAX_METADATA_BYTES as i64])?;
    let row = rows.next()?.context("外部归档条目不存在")?;
    let item = ExtItem {
        id: Uuid::parse_str(&row.get::<_, String>(0)?).context("外部归档条目 ID 损坏")?,
        source_id: Uuid::parse_str(&row.get::<_, String>(1)?).context("外部来源 ID 损坏")?,
        collection: row.get(2)?,
        index: row.get(3)?,
        object_address: row.get(4)?,
        state: row.get(5)?,
        metadata_json: read_blob(row, 6, MAX_METADATA_BYTES, "外部归档条目元数据")?,
    };
    validate_item(&item)?;
    Ok(item)
}

pub(super) const MESSAGE_SELECT: &str =
    "SELECT m.id,m.item,m.field_id,i.source,i.collection,i.item_index,s.source_bytes,
    typeof(m.summary),CASE WHEN typeof(m.summary)='blob' THEN length(m.summary) END,
    CASE WHEN typeof(m.summary)='blob' THEN CASE WHEN length(m.summary)<=?5 THEN m.summary END END
    FROM ext_messages m JOIN ext_items i ON i.id=m.item JOIN ext_sources s ON s.id=i.source";

pub(in crate::store) fn read_summary(row: &Row<'_>) -> Result<ExtMessageSummary> {
    let message: ExtMessageSummary =
        serde_json::from_slice(&read_blob(row, 7, MAX_SUMMARY_BYTES, "外部归档消息摘要")?)?;
    ensure!(
        message.id.to_string() == row.get::<_, String>(0)?
            && message.item_id.to_string() == row.get::<_, String>(1)?
            && message.field_id == row.get::<_, u8>(2)?
            && message.source_id.to_string() == row.get::<_, String>(3)?
            && message.collection == row.get::<_, String>(4)?
            && message.item_index == row.get::<_, u64>(5)?,
        "外部归档消息摘要与条目归属不一致"
    );
    validate_message(&message, row.get(6)?)?;
    Ok(message)
}

pub(in crate::store) fn read_message_summary(
    connection: &Connection,
    id: Uuid,
) -> Result<ExtMessageSummary> {
    let mut query = connection.prepare(&format!("{MESSAGE_SELECT} WHERE m.id=?1"))?;
    let mut rows = query.query(params![
        id.to_string(),
        rusqlite::types::Null,
        rusqlite::types::Null,
        rusqlite::types::Null,
        MAX_SUMMARY_BYTES as i64
    ])?;
    read_summary(rows.next()?.context("外部归档消息不存在")?)
}

pub(in crate::store) fn checked_range(offset: u64, length: u64, total: u64) -> Result<()> {
    ensure!(
        offset.checked_add(length).is_some_and(|end| end <= total),
        "External 读取范围超出原始数据"
    );
    Ok(())
}

pub(in crate::store) fn chunk_length(source_bytes: u64, ordinal: u64) -> Result<usize> {
    let start = ordinal
        .checked_mul(EXT_CHUNK_BYTES as u64)
        .context("外部归档块偏移溢出")?;
    ensure!(start < source_bytes, "外部归档块超出源文件范围");
    Ok((source_bytes - start).min(EXT_CHUNK_BYTES as u64) as usize)
}

pub(in crate::store) fn encode_chunk(raw: &[u8]) -> Result<(u8, Vec<u8>)> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(1));
    encoder.write_all(raw)?;
    let compressed = encoder.finish()?;
    Ok(if compressed.len() < raw.len() {
        (1, compressed)
    } else {
        (0, raw.to_vec())
    })
}

pub(in crate::store) fn read_chunk(
    connection: &Connection,
    source_id: Uuid,
    source_bytes: u64,
    ordinal: u64,
) -> Result<Vec<u8>> {
    let expected = chunk_length(source_bytes, ordinal)?;
    let mut query = connection.prepare(
        "SELECT raw_bytes,codec,typeof(data),CASE WHEN typeof(data)='blob' THEN length(data) END,
        CASE WHEN typeof(data)='blob' THEN CASE WHEN length(data)<=?3 THEN data END END
        FROM ext_source_chunks WHERE source=?1 AND ordinal=?2",
    )?;
    let mut rows = query.query(params![
        source_id.to_string(),
        i64::try_from(ordinal)?,
        EXT_CHUNK_BYTES as i64
    ])?;
    let row = rows.next()?.context("外部源归档缺块")?;
    ensure!(
        row.get::<_, u64>(0)? == expected as u64,
        "外部归档块原始长度不一致"
    );
    let codec: u8 = row.get(1)?;
    let data = read_blob(row, 2, EXT_CHUNK_BYTES, "外部归档块")?;
    match codec {
        0 => {
            ensure!(data.len() == expected, "External 原始归档块长度不一致");
            Ok(data)
        }
        1 => {
            ensure!(
                !data.is_empty() && data.len() < expected,
                "External 压缩归档块长度无效"
            );
            // One extra output byte detects expansion past the recorded bound without unbounded allocation.
            let mut raw = vec![0; expected + 1];
            let mut decoder = Decompress::new(true);
            let status = decoder
                .decompress(&data, &mut raw, FlushDecompress::Finish)
                .context("外部归档块解压失败")?;
            ensure!(
                status == Status::StreamEnd
                    && decoder.total_out() == expected as u64
                    && decoder.total_in() == data.len() as u64,
                "External 压缩归档块截断、超长或含多余尾数据"
            );
            raw.truncate(expected);
            Ok(raw)
        }
        _ => bail!("不支持的 外部归档块编码"),
    }
}

pub(in crate::store) fn read_range(
    connection: &Connection,
    source_id: Uuid,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    ensure!(
        length <= EXT_FRAGMENT_BYTES,
        "外部归档读取不得超过 64 KiB"
    );
    let state = source_state(connection, source_id)?;
    ensure!(state.complete, "External 源文件导入尚未完成");
    checked_range(offset, length as u64, state.source_bytes)?;
    let mut result = Vec::with_capacity(length);
    while result.len() < length {
        let position = offset + result.len() as u64;
        let ordinal = position / EXT_CHUNK_BYTES as u64;
        let within = (position % EXT_CHUNK_BYTES as u64) as usize;
        let raw = read_chunk(connection, source_id, state.source_bytes, ordinal)?;
        let count = (raw.len() - within).min(length - result.len());
        result.extend_from_slice(&raw[within..within + count]);
    }
    Ok(result)
}
