use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::Value;

use super::super::{
    EXT_FRAGMENT_BYTES, ProjectStore, checked_range, read_item, read_message_summary, read_range,
    read_report, source_state,
};
use crate::{ExtItem, ExtMessageFragment, ExtMessageSummary, ExtResponseRef, Uuid, Workspace};

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::ExtArchiveRepo<S> {
    pub fn response_fragment(
        &self,
        reference: &ExtResponseRef,
        offset: u64,
        length: usize,
    ) -> Result<ExtMessageFragment> {
        ensure!(
            length <= EXT_FRAGMENT_BYTES,
            "外部归档响应读取不得超过 64 KiB"
        );
        let transaction = self.0.connection.unchecked_transaction()?;
        let summary = validate_reference(&transaction, reference)?;
        checked_range(offset, length as u64, summary.raw_bytes)?;
        let absolute = summary
            .raw_offset
            .checked_add(offset)
            .context("外部归档响应偏移溢出")?;
        let bytes = read_range(&transaction, summary.source_id, absolute, length)?;
        transaction.commit()?;
        Ok(ExtMessageFragment {
            summary,
            offset,
            bytes,
        })
    }
}

pub(in crate::store) fn validate_workspace_responses(
    connection: &Connection,
    workspace: &Workspace,
) -> Result<()> {
    for reference in workspace
        .tabs
        .iter()
        .chain(&workspace.closed)
        .filter_map(|tab| tab.archive_response.as_ref())
    {
        validate_reference(connection, reference)?;
    }
    Ok(())
}

pub(in crate::store) fn for_request(
    connection: &Connection,
    request: &ExtMessageSummary,
) -> Result<Option<ExtResponseRef>> {
    if request.role != "request" || request.field_id != 2 || !history_path(&request.collection) {
        return Ok(None);
    }
    let report = read_report(connection, request.source_id)?;
    let expected_context = match (report.schema_floor, report.schema_revision) {
        (228, 228) => "repeater_pair",
        (229, 229) => "repeater_pair_229",
        _ => return Ok(None),
    };
    let item = read_item(connection, request.item_id)?;
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    if metadata["record"]["mapping"]["value"]["response"]["observation"]["state"] != "value" {
        return Ok(None);
    }
    ensure!(
        metadata["context"] == expected_context,
        "归档历史条目上下文与源 schema 不一致"
    );
    validate_pair_message(&item, &metadata, request, 2)?;
    ensure!(
        message_id(connection, item.id, 2)? == request.id,
        "归档请求索引不唯一或已改变"
    );
    let response = read_message_summary(connection, message_id(connection, item.id, 3)?)?;
    validate_pair_message(&item, &metadata, &response, 3)?;
    ensure!(report.complete_archive, "External 源文件导入尚未完成");
    validate_record_range(request, report.source_bytes)?;
    validate_record_range(&response, report.source_bytes)?;
    Ok(Some(ExtResponseRef {
        source_id: item.source_id,
        item_id: item.id,
        message_id: response.id,
        draft_revision: 1,
    }))
}

fn validate_reference(
    connection: &Connection,
    reference: &ExtResponseRef,
) -> Result<ExtMessageSummary> {
    ensure!(
        !reference.source_id.is_nil()
            && !reference.item_id.is_nil()
            && !reference.message_id.is_nil()
            && reference.draft_revision > 0,
        "归档响应引用无效"
    );
    let state = source_state(connection, reference.source_id)?;
    ensure!(state.complete, "External 源文件导入尚未完成");
    let report = read_report(connection, reference.source_id)?;
    let expected_context = match (report.schema_floor, report.schema_revision) {
        (228, 228) => "repeater_pair",
        (229, 229) => "repeater_pair_229",
        _ => anyhow::bail!("归档响应仅支持已映射的 schema 228/229 Repeater 历史"),
    };
    let item = read_item(connection, reference.item_id)?;
    ensure!(
        item.source_id == reference.source_id,
        "归档响应条目不属于所选来源"
    );
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    ensure!(
        metadata["context"] == expected_context,
        "归档响应条目上下文与源 schema 不一致"
    );
    let request = read_message_summary(connection, message_id(connection, item.id, 2)?)?;
    validate_pair_message(&item, &metadata, &request, 2)?;
    ensure!(
        message_id(connection, item.id, 3)? == reference.message_id,
        "归档响应不属于所选历史条目"
    );
    let response = read_message_summary(connection, reference.message_id)?;
    validate_pair_message(&item, &metadata, &response, 3)?;
    validate_record_range(&request, state.source_bytes)?;
    validate_record_range(&response, state.source_bytes)?;
    Ok(response)
}

pub(in crate::store) fn validate_record_range(
    message: &ExtMessageSummary,
    source_bytes: u64,
) -> Result<()> {
    let address = message
        .raw_offset
        .checked_sub(8)
        .context("归档消息缺少数组头范围")?;
    let size = message
        .raw_bytes
        .max(1)
        .checked_add(8)
        .context("归档消息数组范围溢出")?;
    checked_range(address, size, source_bytes)
}

fn message_id(connection: &Connection, item_id: Uuid, field_id: u8) -> Result<Uuid> {
    let mut query = connection
        .prepare("SELECT id FROM main.ext_messages WHERE item=?1 AND field_id=?2 LIMIT 2")?;
    let mut rows = query.query(params![item_id.to_string(), field_id])?;
    let id: String = rows
        .next()?
        .context("归档历史声明了消息字节，但对应消息索引缺失")?
        .get(0)?;
    ensure!(rows.next()?.is_none(), "归档历史消息索引重复");
    Uuid::parse_str(&id).context("归档历史消息 ID 损坏")
}

fn history_path(path: &str) -> bool {
    path.strip_prefix("root.f2.f1[")
        .and_then(|path| path.strip_suffix("].f2"))
        .is_some_and(|index| {
            !index.is_empty()
                && index.bytes().all(|c| c.is_ascii_digit())
                && index.parse::<u64>().is_ok()
        })
}

pub(in crate::store) fn validate_pair_message(
    item: &ExtItem,
    metadata: &Value,
    message: &ExtMessageSummary,
    field_id: u8,
) -> Result<()> {
    let record = &metadata["record"];
    let role = if field_id == 2 { "request" } else { "response" };
    let field = &record["mapping"]["value"][role];
    let raw = &field["observation"]["value"];
    ensure!(
        history_path(&item.collection)
            && item.source_id == message.source_id
            && item.id == message.item_id
            && item.collection == message.collection
            && item.index == message.item_index
            && item.state == "mapped"
            && item.object_address.is_some()
            && message.field_id == field_id
            && message.role == role
            && metadata["version"] == 1
            && matches!(
                metadata["context"].as_str(),
                Some("repeater_pair" | "repeater_pair_229")
            )
            && metadata["slot"]["state"] == "value"
            && metadata["slot"]["value"].as_u64() == item.object_address
            && record["object"]["object_type"] == 1
            && record["object"]["subtype"] == 0
            && record["mapping"]["state"] == "value"
            && field["id"] == field_id
            && field["observation"]["state"] == "value"
            && raw["kind"] == "bytes"
            && raw["address"]
                .as_u64()
                .and_then(|address| address.checked_add(8))
                == Some(message.raw_offset)
            && raw["payload_bytes"].as_u64() == Some(message.raw_bytes)
            && raw["element_count"].as_u64() == Some(message.raw_bytes)
            && raw["total_size"].as_u64() == message.raw_bytes.max(1).checked_add(8),
        "归档历史请求或响应与条目元数据绑定不一致"
    );
    if field_id == 2 {
        let binding = &field["binding"];
        ensure!(
            binding["source_id"] == message.source_id.to_string()
                && binding["item_id"] == message.item_id.to_string()
                && binding["message_id"] == message.id.to_string(),
            "归档历史请求绑定已改变"
        );
    }
    Ok(())
}
