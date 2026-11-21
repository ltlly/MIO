use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

use super::{
    EXT_CHUNK_BYTES, ProjectStore, checked_range, read_chunk, read_item, read_message_summary,
    read_report, source_state,
};
use crate::ext_archive::import::check_cancel;
use crate::{ExtMessageSummary, Draft, Uuid, WorkTab, Workspace, http::MAX_BODY};

#[cfg(test)]
pub(super) mod groups;
pub(super) mod response;
#[cfg(test)]
pub(super) mod restore;

impl ProjectStore {
    pub(crate) fn ext_repeater_tab(
        &self,
        source_id: Uuid,
        item_id: Uuid,
        message_id: Uuid,
        workspace: &Workspace,
        cancel: &CancellationToken,
    ) -> Result<WorkTab> {
        check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
        let transaction = self.connection.unchecked_transaction()?;
        let tab = load_tab(
            &transaction,
            source_id,
            item_id,
            message_id,
            workspace,
            cancel,
        )?;
        transaction.commit()?;
        Ok(tab)
    }
}

pub(super) fn load_tab(
    connection: &rusqlite::Connection,
    source_id: Uuid,
    item_id: Uuid,
    message_id: Uuid,
    workspace: &Workspace,
    cancel: &CancellationToken,
) -> Result<WorkTab> {
    check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
    let message = read_message_summary(connection, message_id)?;
    ensure!(
        message.source_id == source_id && message.item_id == item_id,
        "外部归档消息不属于所选来源或条目"
    );
    load_request(connection, &message, workspace, None, cancel)
}

pub(in crate::store) fn load_request(
    connection: &rusqlite::Connection,
    message: &ExtMessageSummary,
    workspace: &Workspace,
    replacing: Option<&WorkTab>,
    cancel: &CancellationToken,
) -> Result<WorkTab> {
    check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
    let source_id = message.source_id;
    let state = source_state(connection, source_id)?;
    ensure!(state.complete, "External 源文件导入尚未完成");
    ensure!(
        matches!(message.role.as_str(), "request" | "current_request")
            || (message.role == "edited_request"
                && message.field_id == 17
                && matches!(message.collection.as_str(), "root.f1.f0" | "root.f1.f1")),
        "仅支持将明确映射的 External 原始、当前或编辑请求载入 Repeater"
    );
    ensure!(message.raw_bytes > 0, "归档请求为空，未创建 Repeater 草稿");
    let version = message
        .declared_version
        .context("归档请求未识别受支持的 HTTP 声明版本，未创建 Repeater 草稿")?;
    workspace.validate()?;
    ensure!(
        replacing.is_some() || workspace.tab_count() < 512,
        "工作标签已达到 512 个限额，未载入归档请求"
    );
    let length = usize::try_from(message.raw_bytes).context("归档请求大小溢出")?;
    ensure!(
        length <= MAX_BODY + 256 * 1024,
        "归档请求超过可编辑大小限额，原始归档仍可分片查看"
    );
    let total = workspace
        .draft_bytes()?
        .checked_sub(replacing.map_or(0, |tab| tab.draft.raw.len()))
        .context("替换的 Repeater 工作稿大小无效")?
        .checked_add(length)
        .context("工作区大小溢出")?;
    ensure!(
        total <= 64 * 1024 * 1024,
        "工作区草稿总量将超过 64 MiB 限额，未载入归档请求"
    );
    checked_range(message.raw_offset, message.raw_bytes, state.source_bytes)?;

    // Capacity and ownership checks precede raw allocation; each archive block is decoded once.
    check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
    let title = current_title_229(connection, message)?.unwrap_or_else(|| "外部归档请求".into());
    let target = match repeater_target_229(connection, message)? {
        Some(target) => target,
        None => super::proxy_history::request_origin(connection, message)?.unwrap_or_default(),
    };
    let archive_response = response::for_request(connection, message)?;
    check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(length)
        .context("无法为所选归档请求分配编辑缓冲区")?;
    let end = message.raw_offset + message.raw_bytes;
    let first = message.raw_offset / EXT_CHUNK_BYTES as u64;
    for ordinal in first..end.div_ceil(EXT_CHUNK_BYTES as u64) {
        check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
        let block = read_chunk(connection, source_id, state.source_bytes, ordinal)?;
        check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
        let block_start = ordinal * EXT_CHUNK_BYTES as u64;
        let start = message.raw_offset.saturating_sub(block_start) as usize;
        let stop = (end - block_start).min(block.len() as u64) as usize;
        raw.extend_from_slice(&block[start..stop]);
    }
    ensure!(
        raw.len() == length,
        "归档请求读取不完整，未创建 Repeater 草稿"
    );
    check_cancel(cancel).context("归档请求载入已取消，未创建 Repeater 草稿")?;
    Ok(WorkTab {
        id: Uuid::new_v4(),
        title,
        group: None,
        draft: Draft {
            revision: 1,
            target,
            raw,
            version,
        },
        last_flow: None,
        archive_response,
        history: None,
    })
}

pub(in crate::store) fn repeater_target_229(
    connection: &rusqlite::Connection,
    message: &ExtMessageSummary,
) -> Result<Option<String>> {
    let (context, tag, role, request_field, service_field) = if message.collection == "root.f2.f1"
        && message.role == "current_request"
        && message.field_id == 4
    {
        ("repeater_current_229", (0, 2), "current_request", 4, 3)
    } else if message.role == "request"
        && message.field_id == 2
        && message
            .collection
            .strip_prefix("root.f2.f1[")
            .and_then(|path| path.strip_suffix("].f2"))
            .is_some_and(|index| {
                !index.is_empty()
                    && index.bytes().all(|c| c.is_ascii_digit())
                    && index.parse::<u64>().is_ok()
            })
    {
        ("repeater_pair_229", (1, 0), "request", 2, 0)
    } else {
        return Ok(None);
    };
    let report = read_report(connection, message.source_id)?;
    if (report.schema_floor, report.schema_revision) != (229, 229) {
        return Ok(None);
    }
    let item = read_item(connection, message.item_id)?;
    let metadata: serde_json::Value = serde_json::from_slice(&item.metadata_json)?;
    let record = &metadata["record"];
    let request = &record["mapping"]["value"][role];
    let service = &request["service"];
    let decoded = &service["observation"]["value"];
    if service["observation"]["state"] != "value" || decoded["mapping"]["state"] != "value" {
        return Ok(None);
    }
    let fields = &decoded["mapping"]["value"];
    let host = &fields["host"];
    let host_value = &host["observation"]["value"];
    let port = &fields["port"];
    let tls = &fields["tls"];
    if host["observation"]["state"] != "value"
        || host_value["mapping"]["state"] != "value"
        || port["observation"]["state"] != "value"
        || tls["observation"]["state"] != "value"
    {
        return Ok(None);
    }
    let binding = &request["binding"];
    let raw = &request["observation"]["value"];
    ensure!(
        item.source_id == message.source_id
            && item.id == message.item_id
            && item.collection == message.collection
            && item.index == message.item_index
            && item.state == "mapped"
            && item.object_address.is_some()
            && metadata["version"] == 1
            && metadata["context"] == context
            && metadata["slot"]["state"] == "value"
            && metadata["slot"]["value"].as_u64() == item.object_address
            && record["object"]["object_type"] == tag.0
            && record["object"]["subtype"] == tag.1
            && record["mapping"]["state"] == "value"
            && request["id"] == request_field
            && request["observation"]["state"] == "value"
            && binding["source_id"] == message.source_id.to_string()
            && binding["item_id"] == message.item_id.to_string()
            && binding["message_id"] == message.id.to_string()
            && raw["kind"] == "bytes"
            && raw["address"]
                .as_u64()
                .and_then(|address| address.checked_add(8))
                == Some(message.raw_offset)
            && raw["payload_bytes"].as_u64() == Some(message.raw_bytes)
            && raw["element_count"].as_u64() == Some(message.raw_bytes)
            && raw["total_size"].as_u64() == message.raw_bytes.max(1).checked_add(8)
            && service["id"] == service_field
            && decoded["object"]["object_type"] == 0
            && decoded["object"]["subtype"] == 0
            && host["id"] == 0
            && host_value["object"]["object_type"] == 0
            && host_value["object"]["subtype"] == 0
            && port["id"] == 1
            && tls["id"] == 2,
        "归档请求的目标服务与消息绑定不一致；未创建草稿"
    );
    let host = host_value["mapping"]["value"]
        .as_str()
        .context("归档目标主机不是文本")?;
    let port = port["observation"]["value"]
        .as_u64()
        .filter(|port| (1..=65535).contains(port))
        .context("归档目标端口无效")?;
    let tls = tls["observation"]["value"]
        .as_bool()
        .context("归档目标 TLS 值无效")?;
    let scheme = if tls { "https" } else { "http" };
    let target = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("{scheme}://[{host}]:{port}")
    } else {
        ensure!(!host.contains([':', '[', ']']), "归档目标主机无效");
        format!("{scheme}://{host}:{port}")
    };
    crate::validate_target_origin(&target)?;
    Ok(Some(target))
}

fn current_title_229(
    connection: &rusqlite::Connection,
    message: &ExtMessageSummary,
) -> Result<Option<String>> {
    if message.collection != "root.f2.f1"
        || message.role != "current_request"
        || message.field_id != 4
    {
        return Ok(None);
    }
    let report = read_report(connection, message.source_id)?;
    if (report.schema_floor, report.schema_revision) != (229, 229) {
        return Ok(None);
    }
    let item = read_item(connection, message.item_id)?;
    ensure!(
        item.id == message.item_id
            && item.source_id == message.source_id
            && item.collection == message.collection
            && item.index == message.item_index,
        "External 标题的来源或条目归属不一致"
    );
    let metadata: serde_json::Value = serde_json::from_slice(&item.metadata_json)?;
    let record = &metadata["record"];
    let current = &record["mapping"]["value"]["current_request"];
    let binding = &current["binding"];
    let raw = &current["observation"]["value"];
    let title = &current["title"];
    let decoded_title = &title["observation"]["value"];
    if item.state != "mapped"
        || item.object_address.is_none()
        || metadata["version"] != 1
        || metadata["context"] != "repeater_current_229"
        || metadata["slot"]["state"] != "value"
        || metadata["slot"]["value"].as_u64() != item.object_address
        || record["object"]["object_type"] != 0
        || record["object"]["subtype"] != 2
        || record["mapping"]["state"] != "value"
        || current["id"] != 4
        || current["observation"]["state"] != "value"
        || binding["source_id"] != message.source_id.to_string()
        || binding["item_id"] != message.item_id.to_string()
        || binding["message_id"] != message.id.to_string()
        || raw["kind"] != "bytes"
        || raw["address"]
            .as_u64()
            .and_then(|address| address.checked_add(8))
            != Some(message.raw_offset)
        || raw["payload_bytes"].as_u64() != Some(message.raw_bytes)
        || raw["element_count"].as_u64() != Some(message.raw_bytes)
        || raw["total_size"].as_u64() != message.raw_bytes.max(1).checked_add(8)
        || title["id"] != 0
        || title["observation"]["state"] != "value"
        || decoded_title["object"]["object_type"] != 0
        || decoded_title["object"]["subtype"] != 0
        || decoded_title["mapping"]["state"] != "value"
    {
        return Ok(None);
    }
    Ok(decoded_title["mapping"]["value"]
        .as_str()
        .filter(|title| {
            !title.is_empty()
                && title.len() <= 4096
                && title.encode_utf16().count() <= crate::ext_archive::MAX_REPEATER_TITLE_UNITS
        })
        .map(str::to_owned))
}

#[cfg(test)]
mod tests;
