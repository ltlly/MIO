use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use super::{EXT_PROXY_COLLECTIONS, MAX_NOTES_BYTES, NOTES_PREVIEW_CHARACTERS};

pub(super) fn table_exists(connection: &Connection, name: &str) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub(super) fn active_ext_source(
    connection: &Connection,
) -> Result<Option<(crate::Uuid, &'static str)>> {
    for name in ["ext_sources", "ext_items", "ext_messages"] {
        if !table_exists(connection, name)? {
            return Ok(None);
        }
    }
    let mut query = connection.prepare(
        "SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END
         FROM ext_sources LIMIT 2",
    )?;
    let mut rows = query.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let raw_id: Option<String> = row.get(0)?;
    let source_id = crate::Uuid::parse_str(&raw_id.context("外部来源 ID 损坏")?)
        .context("外部来源 ID 无效")?;
    ensure!(
        rows.next()?.is_none(),
        "项目包含多个 外部源归档；未合并历史"
    );
    let report = crate::store::ext_archive::read_report(connection, source_id)?;
    if !report.complete_archive {
        return Ok(None);
    }
    Ok(match (report.schema_floor, report.schema_revision) {
        (228, 228) => Some((source_id, "http_history_item")),
        (229, 229) => Some((source_id, "http_history_229")),
        _ => None,
    })
}

pub(super) const ARCHIVE_SERVICE: &str = "$.record.mapping.value.request.parsed_request.observation.value.mapping.value.service.observation.value.mapping.value";

pub(in crate::store) fn archive_origin(metadata: &Value) -> Option<String> {
    let request = &metadata["record"]["mapping"]["value"]["request"]["parsed_request"];
    let decoded = &request["observation"]["value"]["mapping"]["value"];
    let service = &decoded["service"]["observation"]["value"]["mapping"]["value"];
    if request["observation"]["state"] != "value"
        || request["observation"]["value"]["mapping"]["state"] != "value"
        || decoded["service"]["observation"]["state"] != "value"
        || decoded["service"]["observation"]["value"]["mapping"]["state"] != "value"
        || service["host"]["observation"]["state"] != "value"
        || service["host"]["observation"]["value"]["mapping"]["state"] != "value"
        || service["port"]["observation"]["state"] != "value"
        || service["tls"]["observation"]["state"] != "value"
    {
        return None;
    }
    let host = service["host"]["observation"]["value"]["mapping"]["value"].as_str()?;
    let port = service["port"]["observation"]["value"]
        .as_u64()
        .filter(|port| (1..=65535).contains(port))?;
    let tls = service["tls"]["observation"]["value"].as_bool()?;
    let scheme = if tls { "https" } else { "http" };
    let origin = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("{scheme}://[{host}]:{port}")
    } else {
        format!("{scheme}://{host}:{port}")
    };
    crate::validate_target_origin(&origin).ok()?;
    Some(origin)
}

pub(in crate::store) fn archive_time(item: &crate::ExtItem, metadata: &Value) -> (Option<i64>, Option<String>) {
    if metadata["context"] == "http_history_item" {
        return (None, None);
    }
    let result = (|| -> Result<i64> {
        let field = &metadata["record"]["mapping"]["value"]["time"];
        ensure!(!field.is_null(), "外部归档 HTTP Time 尚未映射");
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

fn archive_source_notes(item: &crate::ExtItem, metadata: &Value) -> Result<String> {
    let annotations = &metadata["record"]["mapping"]["value"]["annotations"];
    ensure!(
        annotations["id"] == 27
            && annotations["binding"]["source_id"] == item.source_id.to_string()
            && annotations["binding"]["item_id"] == item.id.to_string(),
        "外部归档 HTTP 源备注未映射或来源绑定不一致"
    );
    let parent_state = annotations["observation"]["state"].as_str();
    if parent_state == Some("null") {
        return Ok(String::new());
    }
    ensure!(
        parent_state == Some("value"),
        "外部归档 HTTP 源备注父字段不可解释；未推断为空"
    );
    let note = &annotations["observation"]["value"]["mapping"]["value"]["note"];
    ensure!(note["id"] == 2, "外部归档 HTTP 源备注字段不是 f2");
    let node_state = note["observation"]["state"].as_str();
    if node_state == Some("null") {
        return Ok(String::new());
    }
    ensure!(
        node_state == Some("value"),
        "外部归档 HTTP 源备注字段不可解释；未推断为空"
    );
    let text = note["observation"]["value"]["mapping"]["value"]["text"]["value"]
        .as_str()
        .context("外部归档 HTTP 源备注不是文本")?;
    ensure!(text.len() <= MAX_NOTES_BYTES, "外部归档 HTTP 源备注超过 64 KiB");
    Ok(text.to_owned())
}

fn archive_notes(
    connection: &Connection,
    has_notes_table: bool,
    item: &crate::ExtItem,
    metadata: &Value,
) -> (u64, String, Option<String>) {
    if has_notes_table {
        match connection
            .query_row(
                "SELECT revision, substr(text,1,256) FROM ext_http_notes WHERE item=?1",
                [item.id.to_string()],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
        {
            Ok(Some((revision, text))) if revision > 0 => return (revision, text, None),
            Ok(Some(_)) => {
                return (
                    0,
                    String::new(),
                    Some("外部归档 HTTP 备注 revision 损坏".to_owned()),
                );
            }
            Ok(None) => {}
            Err(error) => return (0, String::new(), Some(error.to_string())),
        }
    }
    match archive_source_notes(item, metadata) {
        Ok(text) => (
            0,
            text.chars().take(NOTES_PREVIEW_CHARACTERS).collect(),
            None,
        ),
        Err(error) => (0, String::new(), Some(error.to_string())),
    }
}

fn archive_source_highlight(item: &crate::ExtItem, metadata: &Value) -> Result<u8> {
    let field = &metadata["record"]["mapping"]["value"]["highlight"];
    ensure!(!field.is_null(), "外部归档 HTTP 源高亮未映射；未推断为无色");
    ensure!(
        field["id"] == 13 && field["observation"]["state"] == "value",
        "外部归档 HTTP 源高亮未映射；未推断为无色"
    );
    if !field["binding"].is_null() {
        ensure!(
            field["binding"]["source_id"] == item.source_id.to_string()
                && field["binding"]["item_id"] == item.id.to_string(),
            "外部归档 HTTP 源高亮身份或字段绑定不一致"
        );
    }
    field["observation"]["value"]
        .as_u64()
        .filter(|color| *color <= 9)
        .map(|color| color as u8)
        .context("外部归档 HTTP 源高亮不在已确认色板中；原始值保留")
}

fn archive_highlight(
    connection: &Connection,
    has_highlights_table: bool,
    item: &crate::ExtItem,
    metadata: &Value,
) -> (Option<crate::ExtHttpHighlight>, Option<String>) {
    if has_highlights_table {
        match connection
            .query_row(
                "SELECT revision,color FROM ext_http_highlights WHERE item=?1",
                [item.id.to_string()],
                |row| {
                    Ok(crate::ExtHttpHighlight {
                        revision: row.get(0)?,
                        color: row.get(1)?,
                    })
                },
            )
            .optional()
        {
            Ok(Some(highlight)) => return (Some(highlight), None),
            Ok(None) => {}
            Err(error) => return (None, Some(error.to_string())),
        }
    }
    match archive_source_highlight(item, metadata) {
        Ok(color) => (Some(crate::ExtHttpHighlight { revision: 0, color }), None),
        Err(error) => (None, Some(error.to_string())),
    }
}

pub(in crate::store) fn build_ext_history_row(
    connection: &Connection,
    source_id: crate::Uuid,
    item_id: crate::Uuid,
    has_notes_table: bool,
    has_highlights_table: bool,
) -> Result<crate::HistoryRow> {
    let item = crate::store::ext_archive::read_item(connection, item_id)?;
    ensure!(
        item.source_id == source_id && EXT_PROXY_COLLECTIONS.contains(&item.collection.as_str()),
        "外部归档 HTTP 条目来源或集合不一致"
    );
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    crate::store::ext_archive::proxy_history::validate_http_item(&item, &metadata)?;
    let mut request = None;
    let mut edited_request = None;
    let mut response = None;
    let mut edited_response = None;
    let mut message_count = 0usize;
    {
        let mut query = connection
            .prepare("SELECT id,field_id FROM ext_messages WHERE item=?1 ORDER BY field_id")?;
        let mut rows = query.query([item_id.to_string()])?;
        while let Some(row) = rows.next()? {
            message_count += 1;
            let field_id: u8 = row.get(1)?;
            let slot = match field_id {
                15 => Some(0),
                17 => Some(1),
                18 => Some(2),
                20 => Some(3),
                _ => None,
            };
            let Some(slot) = slot else {
                continue;
            };
            let message_id = crate::Uuid::parse_str(&row.get::<_, String>(0)?)
                .context("外部归档 HTTP 消息 ID 损坏")?;
            let message = crate::store::ext_archive::read_message_summary(connection, message_id)?;
            crate::store::ext_archive::proxy_history::validate_message_binding(&item, &metadata, &message)?;
            match slot {
                0 => request = Some(message),
                1 => edited_request = Some(message),
                2 => response = Some(message),
                _ => edited_response = Some(message),
            }
        }
    }
    let origin = if request.is_some() {
        archive_origin(&metadata)
    } else {
        None
    };
    let (time_unix_ms, time_error) = archive_time(&item, &metadata);
    let (notes_revision, notes_preview, notes_error) =
        archive_notes(connection, has_notes_table, &item, &metadata);
    let (highlight, highlight_error) =
        archive_highlight(connection, has_highlights_table, &item, &metadata);
    Ok(crate::HistoryRow::ExtProxy(Box::new(
        crate::ExtHistoryRow {
            item: crate::ExtItemSummary {
                id: item.id,
                source_id: item.source_id,
                collection: item.collection,
                index: item.index,
                object_address: item.object_address,
                state: item.state,
                message_count,
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
        },
    )))
}

pub(in crate::store) fn read_ext_message_bytes(
    connection: &Connection,
    message: &crate::ExtMessageSummary,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<u8>> {
    crate::ext_archive::import::check_cancel(cancel)?;
    let state = crate::store::ext_archive::source_state(connection, message.source_id)?;
    ensure!(state.complete, "External 源文件导入尚未完成");
    let length = usize::try_from(message.raw_bytes).context("外部归档消息大小溢出")?;
    ensure!(length <= crate::http::MAX_BODY + 256 * 1024, "外部归档消息超过完整读取限额");
    crate::store::ext_archive::checked_range(message.raw_offset, message.raw_bytes, state.source_bytes)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(length).context("无法分配 外部归档消息缓冲区")?;
    let end = message.raw_offset + message.raw_bytes;
    let first = message.raw_offset / crate::store::ext_archive::EXT_CHUNK_BYTES as u64;
    for ordinal in first..end.div_ceil(crate::store::ext_archive::EXT_CHUNK_BYTES as u64) {
        crate::ext_archive::import::check_cancel(cancel)?;
        let block = crate::store::ext_archive::read_chunk(connection, message.source_id, state.source_bytes, ordinal)?;
        let block_start = ordinal * crate::store::ext_archive::EXT_CHUNK_BYTES as u64;
        let start = message.raw_offset.saturating_sub(block_start) as usize;
        let stop = (end - block_start).min(block.len() as u64) as usize;
        raw.extend_from_slice(&block[start..stop]);
    }
    ensure!(raw.len() == length, "外部归档消息读取不完整");
    Ok(raw)
}

impl<S: std::ops::Deref<Target = crate::store::ProjectStore>> crate::store::SiteMapRepo<S> {
    pub(crate) fn ext_detail(
        &self,
        source_id: crate::Uuid,
        item_id: crate::Uuid,
        message_id: crate::Uuid,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(crate::HistoryRow, super::SiteMapEntryDetail)> {
        let transaction = self.0.connection.unchecked_transaction()?;
        let row = build_ext_history_row(&transaction, source_id, item_id, true, true)?;
        let crate::HistoryRow::ExtProxy(archive) = &row else { unreachable!() };
        let selected = archive.request.as_ref().filter(|message| message.id == message_id)
            .or_else(|| archive.edited_request.as_ref().filter(|message| message.id == message_id))
            .context("所选站点地图归档请求身份不一致")?;
        let target = archive.origin.as_deref().context("归档请求缺少连接目标")?;
        let version = selected.declared_version.context("归档请求声明版本不可用")?;
        let request_raw = read_ext_message_bytes(&transaction, selected, cancel)?;
        let request = crate::parse_repeater_request(&request_raw, target, version, false)?;
        let edited = archive.edited_request.as_ref().is_some_and(|message| message.id == message_id);
        let response_message = if edited { archive.edited_response.as_ref().or(archive.response.as_ref()) } else { archive.response.as_ref() };
        let read_response = |message: &crate::ExtMessageSummary| -> Result<(Vec<u8>, crate::HttpResponse)> {
            let raw = read_ext_message_bytes(&transaction, message, cancel)?;
            let response = crate::parse_response(&raw, message.declared_version.context("归档响应声明版本不可用")?)?;
            Ok((raw, response))
        };
        let response_pair = response_message.map(read_response).transpose()?;
        let mut original = crate::OriginalMessages::default();
        let original_request_raw = if edited {
            let message = archive.request.as_ref().context("编辑请求缺少原始请求")?;
            let raw = read_ext_message_bytes(&transaction, message, cancel)?;
            original.request = Some(crate::parse_repeater_request(&raw, target, message.declared_version.context("原始请求声明版本不可用")?, false)?);
            Some(raw)
        } else { None };
        let original_response_raw = if archive.edited_response.as_ref().is_some_and(|message| response_message.is_some_and(|selected| selected.id == message.id)) {
            archive.response.as_ref().map(read_response).transpose()?.map(|(raw, response)| { original.response = Some(response); raw })
        } else { None };
        original.synthesize_provenance(Some(&request), response_pair.as_ref().map(|(_, response)| response));
        let summary = crate::FlowSummary {
            id: item_id, revision: 1, source: crate::TrafficSource::Proxy,
            method: request.method.clone(), url: request.url.clone(), status: response_pair.as_ref().map(|(_, response)| response.status),
            state: crate::FlowState::Pending, request_bytes: request.body.len() as u64,
            response_bytes: response_pair.as_ref().map_or(0, |(_, response)| response.body.len() as u64),
            started_unix_ms: archive.time_unix_ms.unwrap_or(0), elapsed_ms: 0,
        };
        let (response_raw, response) = response_pair.map_or((None, None), |(raw, response)| (Some(raw), Some(response)));
        let mut selected_row = row.clone();
        if !edited && let crate::HistoryRow::ExtProxy(row) = &mut selected_row {
            row.edited_request = None;
            row.edited_response = None;
        }
        let detail = super::SiteMapEntryDetail {
            row_id: row.id(), record: crate::FlowRecord { summary, request, response }, original,
            request_raw, response_raw, original_request_raw, original_response_raw,
        };
        transaction.commit()?;
        Ok((selected_row, detail))
    }
}
