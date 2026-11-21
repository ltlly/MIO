use std::io::{Read, Write};

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{FlowId, FlowProvenance};

use super::ProjectStore;

pub(in crate::store) const PREFIX: &str = "flow_provenance:";
const ZLIB_HEADER: &[u8; 2] = b"Z\x01";
const MAX_COMPRESSED_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;

pub(in crate::store) fn provenance_key(flow_id: FlowId) -> String {
    format!("{PREFIX}{flow_id}")
}

fn encode_provenance(provenance: &FlowProvenance) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(provenance)?;
    ensure!(
        json.len() <= MAX_DECOMPRESSED_BYTES,
        "修改溯源链 JSON 超过 64 MiB 限额"
    );
    let mut out = Vec::with_capacity(2 + json.len().min(4096));
    out.extend_from_slice(ZLIB_HEADER);
    {
        let mut encoder = ZlibEncoder::new(&mut out, Compression::new(1));
        encoder.write_all(&json)?;
        encoder.finish()?;
    }
    ensure!(
        out.len() <= MAX_COMPRESSED_BYTES,
        "修改溯源链编码超过 16 MiB 限额"
    );
    Ok(out)
}

fn decode_provenance(bytes: &[u8]) -> Result<FlowProvenance> {
    ensure!(
        bytes.len() <= MAX_COMPRESSED_BYTES,
        "修改溯源链数据超过 16 MiB 限额"
    );
    let json_bytes = if let Some(compressed) = bytes.strip_prefix(ZLIB_HEADER) {
        let mut decoded = Vec::new();
        ZlibDecoder::new(compressed)
            .take((MAX_DECOMPRESSED_BYTES + 1) as u64)
            .read_to_end(&mut decoded)
            .context("修改溯源链解压失败")?;
        ensure!(
            decoded.len() <= MAX_DECOMPRESSED_BYTES,
            "修改溯源链解压后超过 64 MiB 限额"
        );
        decoded
    } else if bytes.first() == Some(&b'{') {
        bytes.to_vec()
    } else {
        bail!("未知的修改溯源链编码格式");
    };
    let provenance: FlowProvenance =
        serde_json::from_slice(&json_bytes).context("修改溯源链 JSON 损坏")?;
    Ok(provenance)
}

pub(in crate::store) fn read_flow_provenance(
    connection: &Connection,
    flow_id: FlowId,
) -> Result<Option<FlowProvenance>> {
    let key = provenance_key(flow_id);
    let row: Option<(String, Option<Vec<u8>>)> = connection
        .query_row(
            "SELECT typeof(value),
                    CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
             FROM metadata WHERE key=?1 COLLATE BINARY",
            params![&key, MAX_COMPRESSED_BYTES as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((kind, blob)) = row else {
        return Ok(None);
    };
    ensure!(kind == "blob", "修改溯源链存储必须为 BLOB");
    let bytes = blob.context("修改溯源链记录超过 16 MiB 限额")?;
    Ok(Some(decode_provenance(&bytes)?))
}

pub(in crate::store) fn write_flow_provenance(
    transaction: &Transaction<'_>,
    flow_id: FlowId,
    provenance: &FlowProvenance,
) -> Result<()> {
    ensure!(!flow_id.is_nil(), "修改溯源链 Flow ID 不能为空");
    let key = provenance_key(flow_id);
    if provenance.request_steps.len() < 2 && provenance.response_steps.len() < 2 {
        transaction.execute(
            "DELETE FROM main.metadata WHERE key=?1 COLLATE BINARY",
            [&key],
        )?;
        return Ok(());
    }
    let encoded = encode_provenance(provenance)?;
    transaction.execute(
        "INSERT INTO main.metadata(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![&key, encoded.as_slice()],
    )?;
    Ok(())
}

pub(in crate::store) fn remove_flow_provenance(
    transaction: &Transaction<'_>,
    flow_ids: &[FlowId],
) -> Result<()> {
    if flow_ids.is_empty() {
        return Ok(());
    }
    let mut stmt = transaction.prepare("DELETE FROM main.metadata WHERE key=?1 COLLATE BINARY")?;
    for &id in flow_ids {
        stmt.execute([provenance_key(id)])?;
    }
    Ok(())
}

impl super::FlowsRepo<&mut ProjectStore> {
    pub(crate) fn put_provenance(
        &mut self,
        flow_id: FlowId,
        provenance: &FlowProvenance,
    ) -> Result<()> {
        self.0.intruder().reject_session_flow_write(flow_id)?;
        let tx = self.0.connection.transaction()?;
        write_flow_provenance(&tx, flow_id, provenance)?;
        tx.commit()?;
        Ok(())
    }
}
