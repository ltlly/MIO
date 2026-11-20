use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::{WebSocketDirection, WebSocketKind};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::{MAX_PAGE_SUMMARY_BYTES, ProjectStore, checked_offset, continuation, source_state};
use crate::store::ext_archive::{
    EXT_FRAGMENT_BYTES, checked_range, chunks::read_range, read_item, read_message_summary,
};
use crate::{
    ExtItem, ExtItemSummary, ExtMessageSummary, ExtWebSocketFragment, ExtWebSocketPage,
    ExtWebSocketRow, Uuid,
};

const WEBSOCKET_COLLECTIONS: [&str; 2] = ["root.f1.f2", "root.f1.f3"];

const CANONICAL_WEBSOCKET_SQL: &str = "WITH addresses AS (
    SELECT i.id,i.collection,i.item_index,
        CASE WHEN i.state='mapped' AND typeof(i.metadata)='blob' AND length(i.metadata)<=262144
            AND json_valid(CAST(i.metadata AS TEXT))
            AND json_extract(CAST(i.metadata AS TEXT),'$.context')='websocket_history'
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.mapping.state')='value'
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.object_type')=1
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.subtype')=1
            AND json_type(CAST(i.metadata AS TEXT),'$.record.object.address')='integer'
            AND typeof(json_extract(CAST(i.metadata AS TEXT),'$.record.object.address'))='integer'
            AND json_extract(CAST(i.metadata AS TEXT),'$.record.object.address')>=0
        THEN json_extract(CAST(i.metadata AS TEXT),'$.record.object.address') END AS address
    FROM ext_items i WHERE i.source=?1 AND i.collection IN ('root.f1.f2','root.f1.f3')
), ranked AS (
    SELECT id,collection,item_index,address,
        row_number() OVER (PARTITION BY address,collection ORDER BY item_index,id) AS alias_rank
    FROM addresses
), canonical AS (
    SELECT r.id,r.collection,r.item_index FROM ranked r
    WHERE r.address IS NULL OR r.collection='root.f1.f2' OR NOT EXISTS (
        SELECT 1 FROM ranked first WHERE first.collection='root.f1.f2'
            AND first.address=r.address AND first.alias_rank=r.alias_rank
    )
)";

impl ProjectStore {
    pub fn ext_websocket_page(
        &self,
        source_id: Uuid,
        offset: usize,
        limit: usize,
    ) -> Result<ExtWebSocketPage> {
        let database_offset = checked_offset(offset, limit)?;
        if let Some(direct) = self.read_direct_ext_source()?
            && direct.id == source_id
        {
            return Ok(ExtWebSocketPage {
                total: 0,
                rows: Vec::new(),
                next_offset: None,
            });
        }
        let transaction = self.connection.unchecked_transaction()?;
        ensure!(
            source_state(&transaction, source_id)?.complete,
            "External 源文件导入尚未完成"
        );
        let total: usize = transaction.query_row(
            &format!("{CANONICAL_WEBSOCKET_SQL} SELECT count(*) FROM canonical"),
            [source_id.to_string()],
            |row| row.get(0),
        )?;
        let rows = {
            let mut query = transaction.prepare(&format!(
                "{CANONICAL_WEBSOCKET_SQL}
                SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END
                FROM canonical ORDER BY item_index ASC,collection,id LIMIT ?2 OFFSET ?3"
            ))?;
            let mut cursor = query.query(params![
                source_id.to_string(),
                limit as i64,
                database_offset
            ])?;
            let mut rows = Vec::with_capacity(limit);
            let mut page_bytes = 0usize;
            while let Some(row) = cursor.next()? {
                let raw_id: Option<String> = row.get(0)?;
                let item_id = Uuid::parse_str(&raw_id.context("外部归档 WebSocket 条目 ID 损坏")?)?;
                let entry = build_websocket_row(&transaction, source_id, item_id)?;
                let row_bytes = serde_json::to_vec(&entry)?.len() + 64;
                if page_bytes + row_bytes > MAX_PAGE_SUMMARY_BYTES {
                    break;
                }
                page_bytes += row_bytes;
                rows.push(entry);
            }
            rows
        };
        let next_offset = continuation(offset, rows.len(), total)?;
        transaction.commit()?;
        Ok(ExtWebSocketPage {
            total,
            rows,
            next_offset,
        })
    }

    pub fn ext_websocket_fragment(
        &self,
        source_id: Uuid,
        item_id: Uuid,
        edited: bool,
        offset: u64,
        length: usize,
    ) -> Result<ExtWebSocketFragment> {
        ensure!(
            length <= EXT_FRAGMENT_BYTES,
            "外部归档 WebSocket 载荷单次读取不得超过 64 KiB"
        );
        let transaction = self.connection.unchecked_transaction()?;
        ensure!(
            source_state(&transaction, source_id)?.complete,
            "External 源文件导入尚未完成"
        );
        let item = read_item(&transaction, item_id)?;
        ensure!(
            item.source_id == source_id
                && WEBSOCKET_COLLECTIONS.contains(&item.collection.as_str()),
            "外部归档 WebSocket 条目来源或集合不匹配"
        );
        let row = build_websocket_row(&transaction, source_id, item_id)?;
        let (field_id, role, key, summary) = if edited {
            (
                6_u8,
                "edited_websocket_payload",
                "edited_payload",
                row.edited_payload
                    .clone()
                    .context("所选 外部归档 WebSocket 条目没有已映射的编辑后载荷")?,
            )
        } else {
            (
                5_u8,
                "websocket_payload",
                "payload",
                row.payload
                    .clone()
                    .context("所选 外部归档 WebSocket 条目没有已映射的原始载荷")?,
            )
        };
        let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
        validate_payload_binding(&item, &metadata, key, field_id, role, &summary)?;
        checked_range(offset, length as u64, summary.raw_bytes)?;
        let absolute = summary
            .raw_offset
            .checked_add(offset)
            .context("外部归档 WebSocket 载荷偏移溢出")?;
        let bytes = read_range(&transaction, source_id, absolute, length)?;
        transaction.commit()?;
        Ok(ExtWebSocketFragment {
            row,
            summary,
            edited,
            offset,
            bytes,
        })
    }
}

fn build_websocket_row(
    connection: &Connection,
    source_id: Uuid,
    item_id: Uuid,
) -> Result<ExtWebSocketRow> {
    let item = read_item(connection, item_id)?;
    ensure!(
        item.source_id == source_id && WEBSOCKET_COLLECTIONS.contains(&item.collection.as_str()),
        "外部归档 WebSocket 条目来源或集合不一致"
    );
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    let mut payload = None;
    let mut edited_payload = None;
    let mut message_count = 0usize;
    {
        let mut query = connection
            .prepare("SELECT id,field_id FROM ext_messages WHERE item=?1 ORDER BY field_id")?;
        let mut cursor = query.query([item_id.to_string()])?;
        while let Some(row) = cursor.next()? {
            message_count += 1;
            let message_id = Uuid::parse_str(&row.get::<_, String>(0)?)?;
            let field_id: u8 = row.get(1)?;
            let summary = read_message_summary(connection, message_id)?;
            match field_id {
                5 => payload = Some(summary),
                6 => edited_payload = Some(summary),
                _ => bail!("外部归档 WebSocket 条目包含未知消息字段 f{field_id}"),
            }
        }
    }
    let item_summary = ExtItemSummary {
        id: item.id,
        source_id: item.source_id,
        collection: item.collection.clone(),
        index: item.index,
        object_address: item.object_address,
        state: item.state.clone(),
        message_count,
    };
    if item.state != "mapped" || metadata["context"] != "websocket_history" {
        let diagnostic = match item.state.as_str() {
            "null" | "missing" => None,
            "unmapped" => Some(format!(
                "未映射紧凑对象（type={}, subtype={}）",
                metadata["record"]["object"]["object_type"],
                metadata["record"]["object"]["subtype"]
            )),
            _ => metadata["record"]["error"]["message"]
                .as_str()
                .or_else(|| metadata["record"]["mapping"]["error"]["message"].as_str())
                .map(str::to_owned)
                .or_else(|| Some(format!("WebSocket 槽位状态：{}", item.state))),
        };
        return Ok(ExtWebSocketRow {
            item: item_summary,
            payload,
            edited_payload,
            connection_id: None,
            direction: None,
            kind: None,
            url: None,
            observed_unix_ms: None,
            notes: None,
            highlight: None,
            diagnostic,
        });
    }
    if let Some(summary) = &payload {
        validate_payload_binding(&item, &metadata, "payload", 5, "websocket_payload", summary)?;
    }
    if let Some(summary) = &edited_payload {
        validate_payload_binding(
            &item,
            &metadata,
            "edited_payload",
            6,
            "edited_websocket_payload",
            summary,
        )?;
    }
    let fields = &metadata["record"]["mapping"]["value"];
    let connection_id = fields["connection_id"]["observation"]["value"].as_u64();
    let direction = serde_json::from_value::<WebSocketDirection>(
        fields["direction"]["observation"]["value"].clone(),
    )
    .ok();
    let kind =
        serde_json::from_value::<WebSocketKind>(fields["kind"]["observation"]["value"].clone())
            .ok();
    let url = fields["upgrade"]["observation"]["value"]["mapping"]["value"]["url"]["value"]
        .as_str()
        .map(str::to_owned);
    let observed_unix_ms = fields["time"]["observation"]["value"].as_i64();
    let highlight = fields["highlight"]["observation"]["value"]
        .as_u64()
        .filter(|color| *color <= 9)
        .map(|color| color as u8);
    let notes = fields["annotations"]["observation"]["value"]["mapping"]["value"]["note"]
        ["observation"]["value"]["mapping"]["value"]["text"]["value"]
        .as_str()
        .map(str::to_owned);
    let mut diagnostics = Vec::new();
    if direction.is_none() {
        diagnostics.push("方向未确认");
    }
    if kind.is_none() {
        diagnostics.push("帧类型/opcode 未确认");
    }
    Ok(ExtWebSocketRow {
        item: item_summary,
        payload,
        edited_payload,
        connection_id,
        direction,
        kind,
        url,
        observed_unix_ms,
        notes,
        highlight,
        diagnostic: (!diagnostics.is_empty()).then(|| diagnostics.join("；")),
    })
}

fn validate_payload_binding(
    item: &ExtItem,
    metadata: &Value,
    key: &str,
    field_id: u8,
    role: &str,
    summary: &ExtMessageSummary,
) -> Result<()> {
    let record = &metadata["record"];
    let field = &record["mapping"]["value"][key];
    let raw = &field["observation"]["value"];
    let binding = &field["binding"];
    ensure!(
        item.state == "mapped"
            && item.object_address.is_some()
            && metadata["version"] == 1
            && metadata["context"] == "websocket_history"
            && metadata["slot"]["state"] == "value"
            && metadata["slot"]["value"].as_u64() == item.object_address
            && record["object"]["object_type"] == 1
            && record["object"]["subtype"] == 1
            && record["mapping"]["state"] == "value"
            && field["id"] == field_id
            && field["observation"]["state"] == "value"
            && binding["source_id"] == summary.source_id.to_string()
            && binding["item_id"] == summary.item_id.to_string()
            && binding["message_id"] == summary.id.to_string()
            && summary.source_id == item.source_id
            && summary.item_id == item.id
            && summary.collection == item.collection
            && summary.item_index == item.index
            && summary.field_id == field_id
            && summary.role == role
            && raw["kind"] == "bytes"
            && raw["address"]
                .as_u64()
                .and_then(|address| address.checked_add(8))
                == Some(summary.raw_offset)
            && raw["payload_bytes"].as_u64() == Some(summary.raw_bytes)
            && raw["element_count"].as_u64() == Some(summary.raw_bytes)
            && raw["total_size"].as_u64() == summary.raw_bytes.max(1).checked_add(8),
        "外部归档 WebSocket 载荷与条目元数据绑定不一致"
    );
    Ok(())
}
