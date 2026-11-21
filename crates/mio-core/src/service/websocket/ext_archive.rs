use anyhow::{Context, Result, bail, ensure};
use tokio_util::sync::CancellationToken;

use crate::http::MAX_BODY;
use crate::store::{ProjectStore, ext_archive::EXT_FRAGMENT_BYTES};
use crate::{
    ExtWebSocketRow, FlowId, Uuid, WebSocketDirection, WebSocketHistoryFilter,
    WebSocketHistoryQuery, WebSocketHistoryResult, WebSocketKind, WebSocketMessage, WebSocketNotes,
    WebSocketSummary,
};

const MAX_NOTES_BYTES: usize = 64 * 1024;
const PREVIEW_CHARACTERS: usize = 256;
const ARCHIVE_PAGE_LIMIT: usize = 500;

pub(crate) fn snapshot_ext_ordinal_base(
    connection: &rusqlite::Connection,
    pending: &[WebSocketSummary],
) -> Result<Option<u64>> {
    let has_sources: bool =
        connection.query_row("SELECT EXISTS(SELECT 1 FROM ext_sources)", [], |row| {
            row.get(0)
        })?;
    if !has_sources {
        return Ok(None);
    }
    let max_live: u64 = connection.query_row(
        "SELECT COALESCE(MAX(ordinal), 0) FROM websocket_messages",
        [],
        |row| row.get(0),
    )?;
    Ok(Some(
        pending
            .iter()
            .map(|row| row.ordinal)
            .fold(max_live, u64::max),
    ))
}

pub(crate) fn merge_ext_websocket_query(
    store: &ProjectStore,
    ordinal_base: u64,
    query: &WebSocketHistoryQuery,
    result: &mut WebSocketHistoryResult,
    cancel: &CancellationToken,
) -> Result<()> {
    merge_ext_websocket_history(
        store,
        ordinal_base,
        query.flow_id,
        &[],
        &query.filter,
        query.offset,
        query.limit,
        query.selected_message_id,
        result,
        cancel,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_ext_page(
    store: &ProjectStore,
    ordinal_base: u64,
    flow_id: Option<FlowId>,
    excluded: &[Uuid],
    filter: &WebSocketHistoryFilter,
    offset: usize,
    limit: usize,
    total: usize,
    rows: Vec<WebSocketSummary>,
) -> Result<(usize, Vec<WebSocketSummary>)> {
    let mut result = WebSocketHistoryResult {
        total,
        rows,
        selected_match: None,
    };
    merge_ext_websocket_history(
        store,
        ordinal_base,
        flow_id,
        excluded,
        filter,
        offset,
        limit,
        None,
        &mut result,
        &CancellationToken::new(),
    )?;
    Ok((result.total, result.rows))
}

struct ArchiveSummaryEntry {
    summary: WebSocketSummary,
    raw_bytes: Option<u64>,
    edited_bytes: Option<u64>,
    has_notes: bool,
}

pub(crate) fn completed_source_id(store: &ProjectStore) -> Result<Option<Uuid>> {
    if store.ext_archive().read_direct_source()?.is_some() {
        return Ok(None);
    }
    let mut sources = store.ext_archive().sources()?;
    let Some(source) = sources.pop() else {
        return Ok(None);
    };
    if !source.report.complete_archive
        || !matches!(
            (source.report.schema_floor, source.report.schema_revision),
            (228, 228) | (229, 229)
        )
    {
        return Ok(None);
    }
    Ok(Some(source.id))
}

fn load_mapped_ws_item(
    store: &ProjectStore,
    source_id: Uuid,
    id: Uuid,
) -> Result<Option<crate::ExtItemDetail>> {
    let detail = match store.ext_archive().item_detail(source_id, id, 0, 8) {
        Ok(detail) => detail,
        Err(error)
            if error
                .to_string()
                .contains("外部归档条目不存在或不属于当前来源") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if detail.item.state != "mapped"
        || !matches!(detail.item.collection.as_str(), "root.f1.f2" | "root.f1.f3")
    {
        return Ok(None);
    }
    Ok(Some(detail))
}

pub(crate) fn read_ext_websocket_detail(
    store: &ProjectStore,
    id: Uuid,
) -> Result<(WebSocketMessage, Option<Vec<u8>>)> {
    let source_id = completed_source_id(store)?.context("WebSocket 消息不存在")?;
    let detail = load_mapped_ws_item(store, source_id, id)?.context("WebSocket 消息不存在")?;
    let raw_msg = detail.messages.rows.iter().find(|msg| msg.field_id == 5);
    let edited_msg = detail.messages.rows.iter().find(|msg| msg.field_id == 6);
    let (row, final_payload, original_payload) = match (raw_msg, edited_msg) {
        (_, Some(edited)) => {
            let (row, final_bytes) =
                read_full_payload(store, source_id, id, true, edited.raw_bytes, None)?;
            let original = match raw_msg {
                Some(raw) => {
                    Some(read_full_payload(store, source_id, id, false, raw.raw_bytes, None)?.1)
                }
                None => None,
            };
            (row, final_bytes, original)
        }
        (Some(raw), None) => {
            let (row, final_bytes) =
                read_full_payload(store, source_id, id, false, raw.raw_bytes, None)?;
            (row, final_bytes, None)
        }
        (None, None) => bail!("WebSocket 消息不存在"),
    };
    let sequence = row
        .item
        .index
        .checked_add(1)
        .context("外部归档 WebSocket 消息序号溢出")?;
    Ok((
        WebSocketMessage {
            id: row.item.id,
            revision: 1,
            flow_id: Uuid::nil(),
            sequence,
            direction: row.direction.unwrap_or(WebSocketDirection::ClientToServer),
            kind: row.kind.unwrap_or(WebSocketKind::Binary),
            payload: final_payload,
            observed_unix_ms: row.observed_unix_ms.unwrap_or(0),
        },
        original_payload,
    ))
}

pub(crate) fn read_ext_websocket_notes(
    store: &ProjectStore,
    id: Uuid,
) -> Result<Option<WebSocketNotes>> {
    let Some(source_id) = completed_source_id(store)? else {
        return Ok(None);
    };
    let Some(detail) = load_mapped_ws_item(store, source_id, id)? else {
        return Ok(None);
    };
    let has_edited = detail.messages.rows.iter().any(|msg| msg.field_id == 6);
    let has_raw = detail.messages.rows.iter().any(|msg| msg.field_id == 5);
    if !has_edited && !has_raw {
        return Ok(None);
    }
    let fragment = store
        .ext_archive()
        .websocket_fragment(source_id, id, !has_raw && has_edited, 0, 0)?;
    let text = fragment.row.notes.unwrap_or_default();
    ensure!(
        text.len() <= MAX_NOTES_BYTES,
        "WebSocket 备注超过 64 KiB 资源限额"
    );
    Ok(Some(WebSocketNotes { revision: 0, text }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_ext_websocket_history(
    store: &ProjectStore,
    ordinal_base: u64,
    flow_id: Option<FlowId>,
    excluded: &[Uuid],
    filter: &WebSocketHistoryFilter,
    offset: usize,
    limit: usize,
    selected_message_id: Option<Uuid>,
    result: &mut WebSocketHistoryResult,
    cancel: &CancellationToken,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    let Some(source_id) = completed_source_id(store)? else {
        return Ok(());
    };
    let matcher = if filter.term.is_empty() {
        None
    } else {
        let pattern = if filter.regex {
            filter.term.clone()
        } else {
            regex::escape(&filter.term)
        };
        Some(
            regex::bytes::RegexBuilder::new(&pattern)
                .unicode(false)
                .case_insensitive(!filter.case_sensitive)
                .size_limit(1024 * 1024)
                .dfa_size_limit(1024 * 1024)
                .nest_limit(128)
                .build()
                .map_err(|error| {
                    anyhow::anyhow!(
                        "WebSocket 正则表达式无效或超出限制；当前不支持 Java lookaround / backreference 等语法：{error}"
                    )
                })?,
        )
    };
    let mut page_offset = 0usize;
    let mut mapped_ordinal = 0u64;
    loop {
        ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
        let page = store
            .ext_archive()
            .websocket_page(source_id, page_offset, ARCHIVE_PAGE_LIMIT)?;
        for row in &page.rows {
            ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
            if row.item.state != "mapped" || (row.payload.is_none() && row.edited_payload.is_none())
            {
                continue;
            }
            mapped_ordinal += 1;
            let ordinal = ordinal_base + mapped_ordinal;
            let entry = row_to_archive_summary(row, ordinal)?;
            if excluded.contains(&entry.summary.id) {
                continue;
            }
            let meta_matches = flow_id.is_none_or(|target| target == entry.summary.flow_id)
                && (!filter.hide_incoming
                    || entry.summary.direction != WebSocketDirection::ServerToClient)
                && (!filter.hide_outgoing
                    || entry.summary.direction != WebSocketDirection::ClientToServer)
                && filter
                    .listener_port
                    .is_none_or(|port| entry.summary.listener_port == Some(port))
                && (!filter.only_with_notes || entry.has_notes);
            if !meta_matches {
                if selected_message_id == Some(entry.summary.id) {
                    result.selected_match = Some((entry.summary.id, false));
                }
                continue;
            }
            let matches = match &matcher {
                None => true,
                Some(regex) => {
                    let hit = match (entry.raw_bytes, entry.edited_bytes) {
                        (raw_opt, Some(edited_len)) => {
                            let (_, final_bytes) = read_full_payload(
                                store,
                                source_id,
                                entry.summary.id,
                                true,
                                edited_len,
                                Some(cancel),
                            )?;
                            if match_bytes(&final_bytes, regex, cancel)? {
                                true
                            } else if let Some(raw_len) = raw_opt {
                                let (_, orig_bytes) = read_full_payload(
                                    store,
                                    source_id,
                                    entry.summary.id,
                                    false,
                                    raw_len,
                                    Some(cancel),
                                )?;
                                match_bytes(&orig_bytes, regex, cancel)?
                            } else {
                                false
                            }
                        }
                        (Some(raw_len), None) => {
                            let (_, final_bytes) = read_full_payload(
                                store,
                                source_id,
                                entry.summary.id,
                                false,
                                raw_len,
                                Some(cancel),
                            )?;
                            match_bytes(&final_bytes, regex, cancel)?
                        }
                        (None, None) => false,
                    };
                    hit != filter.negative
                }
            };
            if selected_message_id == Some(entry.summary.id) {
                result.selected_match = Some((entry.summary.id, matches));
            }
            if matches {
                if result.total >= offset && result.rows.len() < limit {
                    result.rows.push(entry.summary);
                }
                result.total = result
                    .total
                    .checked_add(1)
                    .context("WebSocket 查询总数溢出")?;
            }
        }
        match page.next_offset {
            Some(next) => page_offset = next,
            None => break,
        }
    }
    Ok(())
}

fn row_to_archive_summary(row: &ExtWebSocketRow, ordinal: u64) -> Result<ArchiveSummaryEntry> {
    let raw_bytes = row.payload.as_ref().map(|summary| summary.raw_bytes);
    let edited_bytes = row.edited_payload.as_ref().map(|summary| summary.raw_bytes);
    if let Some(len) = raw_bytes {
        ensure!(
            len <= MAX_BODY as u64,
            "WebSocket 消息原始正文超过 16 MiB 限额"
        );
    }
    if let Some(len) = edited_bytes {
        ensure!(len <= MAX_BODY as u64, "WebSocket 消息正文超过 16 MiB 限额");
    }
    let (payload_bytes, edited) = match (raw_bytes, edited_bytes) {
        (_, Some(edited_len)) => (edited_len, true),
        (Some(raw_len), None) => (raw_len, false),
        (None, None) => bail!("外部归档 WebSocket 条目缺少已映射载荷"),
    };
    let notes_text = row.notes.as_deref().unwrap_or("");
    ensure!(
        notes_text.len() <= MAX_NOTES_BYTES,
        "WebSocket 备注超过 64 KiB 资源限额"
    );
    let has_notes = !notes_text.is_empty();
    let notes_preview: String = notes_text.chars().take(PREVIEW_CHARACTERS).collect();
    let sequence = row
        .item
        .index
        .checked_add(1)
        .context("外部归档 WebSocket 消息序号溢出")?;
    Ok(ArchiveSummaryEntry {
        summary: WebSocketSummary {
            id: row.item.id,
            ordinal,
            connection_id: row.connection_id.filter(|&id| id > 0).unwrap_or(1),
            listener_port: None,
            revision: 1,
            flow_id: Uuid::nil(),
            sequence,
            direction: row.direction.unwrap_or(WebSocketDirection::ClientToServer),
            kind: row.kind.unwrap_or(WebSocketKind::Binary),
            payload_bytes,
            observed_unix_ms: row.observed_unix_ms.unwrap_or(0),
            url: row.url.clone().unwrap_or_default(),
            edited,
            notes_revision: 0,
            notes_preview,
        },
        raw_bytes,
        edited_bytes,
        has_notes,
    })
}

fn read_full_payload(
    store: &ProjectStore,
    source_id: Uuid,
    item_id: Uuid,
    edited: bool,
    expected_bytes: u64,
    cancel: Option<&CancellationToken>,
) -> Result<(ExtWebSocketRow, Vec<u8>)> {
    ensure!(
        expected_bytes <= MAX_BODY as u64,
        "WebSocket 消息超过 16 MiB 限额"
    );
    let total = expected_bytes as usize;
    if let Some(cancel) = cancel {
        ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    }
    if total == 0 {
        let fragment = store
            .ext_archive()
            .websocket_fragment(source_id, item_id, edited, 0, 0)?;
        ensure!(
            fragment.summary.raw_bytes == 0 && fragment.bytes.is_empty(),
            "外部归档 WebSocket 空消息长度与摘要不一致"
        );
        return Ok((fragment.row, Vec::new()));
    }
    let mut out = Vec::with_capacity(total);
    let mut row_out = None;
    while out.len() < total {
        if let Some(cancel) = cancel {
            ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
        }
        let chunk_len = (total - out.len()).min(EXT_FRAGMENT_BYTES);
        let pos = out.len() as u64;
        let fragment = store
            .ext_archive()
            .websocket_fragment(source_id, item_id, edited, pos, chunk_len)?;
        ensure!(
            fragment.summary.raw_bytes == expected_bytes && fragment.bytes.len() == chunk_len,
            "外部归档 WebSocket 载荷分片长度与摘要不一致"
        );
        out.extend_from_slice(&fragment.bytes);
        row_out.get_or_insert(fragment.row);
    }
    Ok((row_out.expect("non-empty payload"), out))
}

fn match_bytes(
    payload: &[u8],
    matcher: &regex::bytes::Regex,
    cancel: &CancellationToken,
) -> Result<bool> {
    ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    let matched = matcher.is_match(payload);
    ensure!(!cancel.is_cancelled(), "WebSocket 查询已取消");
    Ok(matched)
}
