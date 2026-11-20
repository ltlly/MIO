use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use serde_json::Value;

use super::*;

pub(super) fn source_time(
    item: &crate::ExtItem,
    metadata: &Value,
) -> (Option<i64>, Option<String>) {
    if metadata["context"] == "http_history_item" {
        return (None, None);
    }
    let result = (|| {
        let field = &metadata["record"]["mapping"]["value"]["time"];
        ensure!(!field.is_null(), "外部归档 HTTP Time 尚未映射");
        validate_http_item(item, metadata)?;
        ensure!(
            field["id"] == 11
                && field["binding"]["source_id"] == item.source_id.to_string()
                && field["binding"]["item_id"] == item.id.to_string(),
            "外部归档 HTTP Time 来源或字段绑定不一致"
        );
        ensure!(
            field["observation"]["state"] == "value",
            "外部归档 HTTP Time 缺失、不可用或读取失败；未推断时间"
        );
        field["observation"]["value"]
            .as_i64()
            .context("外部归档 HTTP Time 不是有效的有符号毫秒值")
    })();
    match result {
        Ok(time) => (Some(time), None),
        Err(error) => (None, Some(error.to_string().chars().take(512).collect())),
    }
}

pub(in crate::store::ext_archive) fn proxy_row(
    connection: &Connection,
    source: Uuid,
    id: Uuid,
) -> Result<HistoryRow> {
    let report = read_report(connection, source)?;
    let expected_context = expected_http_context(&report)?;
    let item = read_item(connection, id)?;
    ensure!(
        item.source_id == source && PROXY_COLLECTIONS.contains(&item.collection.as_str()),
        "外部归档 HTTP 历史条目来源或集合不一致"
    );
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    ensure!(
        metadata["version"] == 1 && metadata["context"] == expected_context,
        "外部归档 HTTP 历史上下文未确认"
    );
    let mut request = None;
    let mut edited_request = None;
    let mut response = None;
    let mut edited_response = None;
    let mut query = connection
        .prepare("SELECT id,field_id FROM ext_messages WHERE item=?1 ORDER BY field_id")?;
    let mut rows = query.query([id.to_string()])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        let field: u8 = row.get(1)?;
        if ![15, 17, 18, 20].contains(&field) {
            continue;
        }
        let message =
            read_message_summary(connection, Uuid::parse_str(&row.get::<_, String>(0)?)?)?;
        validate_message_binding(&item, &metadata, &message)?;
        match field {
            15 => request = Some(message),
            17 => edited_request = Some(message),
            18 => response = Some(message),
            20 => edited_response = Some(message),
            _ => unreachable!(),
        }
    }
    let origin = request
        .as_ref()
        .map(|message| request_origin(connection, message))
        .transpose()?
        .flatten();
    let (time_unix_ms, time_error) = source_time(&item, &metadata);
    let (notes_revision, notes_preview, notes_error) =
        super::super::http_notes::preview(connection, &item, &metadata);
    let (highlight, highlight_error) =
        super::super::http_highlights::preview(connection, &item, &metadata);
    Ok(HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        item: ExtItemSummary {
            id: item.id,
            source_id: item.source_id,
            collection: item.collection,
            index: item.index,
            object_address: item.object_address,
            state: item.state,
            message_count: count,
        },
        request,
        edited_request,
        response,
        edited_response,
        origin,
        time_unix_ms,
        time_error,
        notes_revision,
        notes_preview,
        notes_error,
        highlight,
        highlight_error,
    })))
}

pub(in crate::store) fn validate_message_binding(
    item: &crate::ExtItem,
    metadata: &Value,
    message: &ExtMessageSummary,
) -> Result<()> {
    let role = match message.field_id {
        15 => "request",
        17 => "edited_request",
        18 => "response",
        20 => "edited_response",
        _ => anyhow::bail!("未确认的 HTTP 历史消息字段"),
    };
    let record = &metadata["record"];
    let field = &record["mapping"]["value"][role];
    let raw = &field["observation"]["value"];
    let is_228 = metadata["context"] == "http_history_item";
    ensure!(
        item.id == message.item_id
            && item.source_id == message.source_id
            && item.collection == message.collection
            && item.index == message.item_index
            && PROXY_COLLECTIONS.contains(&item.collection.as_str())
            && item.state == "mapped"
            && item.object_address.is_some()
            && metadata["version"] == 1
            && (is_228 || metadata["context"] == "http_history_229")
            && (!is_228 || matches!(message.field_id, 15 | 18))
            && metadata["slot"]["state"] == "value"
            && metadata["slot"]["value"].as_u64() == item.object_address
            && record["object"]["object_type"] == 1
            && record["object"]["subtype"] == 0
            && record["object"]["address"].as_u64().is_some()
            && record["mapping"]["state"] == "value"
            && message.role == role
            && field["id"] == message.field_id
            && field["observation"]["state"] == "value"
            && raw["kind"] == "bytes"
            && raw["address"]
                .as_u64()
                .and_then(|value| value.checked_add(8))
                == Some(message.raw_offset)
            && raw["payload_bytes"].as_u64() == Some(message.raw_bytes)
            && raw["element_count"].as_u64() == Some(message.raw_bytes)
            && raw["total_size"].as_u64() == message.raw_bytes.max(1).checked_add(8),
        "外部归档 HTTP 历史原始消息与条目绑定不一致"
    );
    if matches!(message.field_id, 15 | 17 | 20) {
        let binding = &field["binding"];
        ensure!(
            binding["source_id"] == message.source_id.to_string()
                && binding["item_id"] == message.item_id.to_string()
                && binding["message_id"] == message.id.to_string(),
            "外部归档 HTTP 历史消息身份绑定不一致"
        );
    }
    if message.field_id == 17 {
        ensure!(
            field["parsed_request"] == record["mapping"]["value"]["request"]["parsed_request"],
            "HTTP 编辑请求不能更换同一条目的原始连接目标"
        );
    }
    Ok(())
}

pub(in crate::store::ext_archive) fn request_origin(
    connection: &Connection,
    message: &ExtMessageSummary,
) -> Result<Option<String>> {
    let edited = message.field_id == 17 || message.role == "edited_request";
    if !PROXY_COLLECTIONS.contains(&message.collection.as_str()) {
        ensure!(!edited, "未确认此集合的 HTTP 编辑请求");
        return Ok(None);
    }
    let report = read_report(connection, message.source_id)?;
    let schema = (report.schema_floor, report.schema_revision);
    if !matches!(schema, (228, 228) | (229, 229)) {
        ensure!(!edited, "HTTP 编辑请求只支持已确认的 schema 229");
        return Ok(None);
    }
    if schema != (229, 229) {
        ensure!(!edited, "HTTP 编辑请求只支持已确认的 schema 229");
        return Ok(None);
    }
    ensure!(
        report.complete_archive && source_state(connection, message.source_id)?.complete,
        "外部源归档尚未完成"
    );
    let item = read_item(connection, message.item_id)?;
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    let origin = request_origin_from_metadata(&item, &metadata, message)?;
    ensure!(
        metadata["context"] == expected_http_context(&report)?,
        "外部归档 HTTP 历史上下文与源 schema 不一致"
    );
    Ok(origin)
}

pub(in crate::store::ext_archive) fn request_origin_from_metadata(
    item: &crate::ExtItem,
    metadata: &Value,
    message: &ExtMessageSummary,
) -> Result<Option<String>> {
    validate_message_binding(item, metadata, message)?;
    ensure!(
        (message.field_id == 15 && message.role == "request")
            || (message.field_id == 17 && message.role == "edited_request"),
        "仅请求可提供 HTTP 历史连接目标"
    );
    if metadata["context"] == "http_history_item" {
        return Ok(None);
    }
    let parsed = &metadata["record"]["mapping"]["value"][&message.role]["parsed_request"];
    if parsed["observation"]["state"] != "value" {
        return Ok(None);
    }
    let decoded = &parsed["observation"]["value"];
    if decoded["mapping"]["state"] != "value" {
        return Ok(None);
    }
    ensure!(
        parsed["id"] == 2
            && decoded["object"]["object_type"] == 0
            && decoded["object"]["subtype"] == 0,
        "HTTP 历史解析请求对象与原始字段不一致"
    );
    let service = &decoded["mapping"]["value"]["service"];
    if service["observation"]["state"] != "value" {
        return Ok(None);
    }
    let decoded = &service["observation"]["value"];
    if decoded["mapping"]["state"] != "value" {
        return Ok(None);
    }
    ensure!(
        service["id"] == 0
            && decoded["object"]["object_type"] == 0
            && decoded["object"]["subtype"] == 0,
        "HTTP 历史服务对象与解析请求不一致"
    );
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
    ensure!(
        host["id"] == 0
            && host_value["object"]["object_type"] == 0
            && host_value["object"]["subtype"] == 0
            && port["id"] == 1
            && tls["id"] == 2,
        "HTTP 历史目标字段布局不一致"
    );
    let host = host_value["mapping"]["value"]
        .as_str()
        .context("HTTP 历史目标主机不是文本")?;
    let port = port["observation"]["value"]
        .as_u64()
        .filter(|port| (1..=65535).contains(port))
        .context("HTTP 历史目标端口无效")?;
    let tls = tls["observation"]["value"]
        .as_bool()
        .context("HTTP 历史目标 TLS 值无效")?;
    let scheme = if tls { "https" } else { "http" };
    let target = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("{scheme}://[{host}]:{port}")
    } else {
        ensure!(!host.contains([':', '[', ']']), "HTTP 历史目标主机无效");
        format!("{scheme}://{host}:{port}")
    };
    crate::validate_target_origin(&target)?;
    Ok(Some(target))
}
