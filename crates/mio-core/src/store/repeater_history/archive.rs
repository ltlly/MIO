use super::*;

fn message(connection: &Connection, item: Uuid, field: u8) -> Result<Option<ExtMessageSummary>> {
    let mut query = connection
        .prepare("SELECT id FROM main.ext_messages WHERE item=?1 AND field_id=?2 LIMIT 2")?;
    let mut rows = query.query(params![item.to_string(), field])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let id: String = row.get(0)?;
    ensure!(rows.next()?.is_none(), "归档历史消息重复");
    Ok(Some(ext_archive::read_message_summary(
        connection,
        Uuid::parse_str(&id)?,
    )?))
}

fn archive_item(connection: &Connection, archive: &Archive, index: u64) -> Result<Option<Uuid>> {
    ensure!(index < archive.length, "归档历史位置越界");
    let mut query = connection.prepare("SELECT id FROM main.ext_items WHERE source=?1 AND collection=?2 AND item_index=?3 LIMIT 2")?;
    let mut rows = query.query(params![
        archive.source_id.to_string(),
        archive.collection,
        i64::try_from(index)?
    ])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let id: String = row.get(0)?;
    ensure!(rows.next()?.is_none(), "归档历史槽位重复");
    Ok(Some(Uuid::parse_str(&id)?))
}

pub(super) fn archive_request(
    connection: &Connection,
    archive: &Archive,
    index: u64,
) -> Result<ExtMessageSummary> {
    let item = ext_archive::read_item(
        connection,
        archive_item(connection, archive, index)?.context("归档历史槽位未索引")?,
    )?;
    let request = message(connection, item.id, 2)?.context("该归档历史槽位没有可载入请求")?;
    let metadata: Value = serde_json::from_slice(&item.metadata_json)?;
    ensure!(
        metadata["context"] == archive.pair_context,
        "归档历史条目上下文与源 schema 不一致"
    );
    ext_archive::validate_repeater_pair_message(&item, &metadata, &request, 2)?;
    ext_archive::validate_repeater_record_range(&request, archive.source_bytes)?;
    ensure!(
        request.source_id == archive.source_id
            && request.collection == archive.collection
            && request.item_index == index,
        "归档历史请求不属于所选目录"
    );
    Ok(request)
}

pub(super) fn archive_entry(
    connection: &Connection,
    archive: &Archive,
    index: u64,
    position: u64,
) -> Result<RepeaterHistoryEntry> {
    let mut entry = RepeaterHistoryEntry {
        id: RepeaterHistoryEntryId::Archive(index),
        position,
        method: None,
        target: None,
        status: None,
        diagnostic: None,
    };
    let Some(item) = archive_item(connection, archive, index)? else {
        entry.diagnostic = Some("该源历史槽位未建立索引".into());
        return Ok(entry);
    };
    let request = message(connection, item, 2)?;
    let response = message(connection, item, 3)?;
    if let Some(request) = request {
        entry.target = match (
            ext_archive::repeater_request_origin(connection, &request)?,
            request.request_target.as_deref(),
        ) {
            (Some(origin), Some(target)) => full_target(&origin, target),
            _ => None,
        };
        entry.method = request.method;
        entry.diagnostic = request.diagnostic;
        if entry.target.is_none() && entry.diagnostic.is_none() {
            entry.diagnostic = Some("归档完整请求 URL 未映射".into());
        }
    } else {
        entry.diagnostic = Some("该源历史槽位没有可载入请求".into());
    }
    if let Some(response) = response {
        entry.status = response.status;
        if entry.diagnostic.is_none() {
            entry.diagnostic = response.diagnostic;
        }
    }
    Ok(entry)
}

fn full_target(origin: &str, target: &str) -> Option<String> {
    let base = url::Url::parse(origin).ok()?;
    let full = if target.starts_with('/') && !target.starts_with("//") {
        format!("{origin}{target}")
    } else if target.starts_with("http://") || target.starts_with("https://") {
        target.to_owned()
    } else {
        return None;
    };
    let parsed = url::Url::parse(&full).ok()?;
    (parsed.origin() == base.origin()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none())
    .then_some(full)
}

fn archive_info(connection: &Connection, source: &RepeaterHistorySource) -> Result<Archive> {
    let report = ext_archive::read_report(connection, source.source_id)?;
    ensure!(
        report.complete_archive,
        "Repeater 历史来源未完整归档或 schema 未支持"
    );
    let (parent_context, pair_context) = match (report.schema_floor, report.schema_revision) {
        (228, 228) => ("repeater_tab", "repeater_pair"),
        (229, 229) => ("repeater_current_229", "repeater_pair_229"),
        _ => anyhow::bail!("Repeater 历史来源未完整归档或 schema 未支持"),
    };
    let parent = ext_archive::read_item(connection, source.parent_item_id)?;
    let metadata: Value = serde_json::from_slice(&parent.metadata_json)?;
    ensure!(
        parent.source_id == source.source_id
            && parent.collection == "root.f2.f1"
            && parent.state == "mapped"
            && parent.object_address.is_some()
            && metadata["version"] == 1
            && metadata["context"] == parent_context
            && metadata["slot"]["state"] == "value"
            && metadata["slot"]["value"].as_u64() == parent.object_address
            && metadata["record"]["object"]["object_type"] == 0
            && metadata["record"]["object"]["subtype"] == 2
            && metadata["record"]["mapping"]["state"] == "value",
        "Repeater 历史来源与原标签不一致"
    );
    let history = &metadata["record"]["mapping"]["value"]["history"];
    let decoded = &history["observation"]["value"];
    ensure!(
        history["id"] == 2
            && history["observation"]["state"] == "value"
            && decoded["object"]["object_type"] == 0
            && decoded["object"]["subtype"] == 0
            && decoded["mapping"]["state"] == "value",
        "源 Repeater 历史目录未映射"
    );
    let length = decoded["mapping"]["value"]["logical_len"]
        .as_u64()
        .filter(|length| *length <= i32::MAX as u64)
        .context("源 Repeater 历史长度无效")?;
    let collection = format!("root.f2.f1[{}].f2", parent.index);
    ensure!(
        report
            .collections
            .iter()
            .any(|entry| entry.path == collection && entry.logical_items == Some(length)),
        "源 Repeater 历史目录与报告不一致"
    );
    Ok(Archive {
        source_id: source.source_id,
        collection,
        length,
        source_bytes: report.source_bytes,
        pair_context,
    })
}

pub(super) fn archive_for_tab(connection: &Connection, tab: &WorkTab) -> Result<Option<Archive>> {
    tab.history
        .as_ref()
        .and_then(|history| history.source.as_ref())
        .map(|source| archive_info(connection, source))
        .transpose()
}

#[cfg(test)]
pub(in crate::store) fn source_for_parent(
    connection: &Connection,
    source_id: Uuid,
    parent_item_id: Uuid,
) -> Result<Option<RepeaterHistorySource>> {
    let parent = ext_archive::read_item(connection, parent_item_id)?;
    let metadata: Value = serde_json::from_slice(&parent.metadata_json)?;
    let history = &metadata["record"]["mapping"]["value"]["history"];
    if history["observation"]["state"] != "value"
        || history["observation"]["value"]["mapping"]["state"] != "value"
    {
        return Ok(None);
    }
    let source = RepeaterHistorySource {
        source_id,
        parent_item_id,
    };
    archive_info(connection, &source)?;
    Ok(Some(source))
}

pub(super) fn entry_content(
    connection: &Connection,
    tab: &WorkTab,
    entry: RepeaterHistoryEntryId,
    workspace: &Workspace,
    cancel: &CancellationToken,
) -> Result<(Draft, Option<ExtResponseRef>, Option<FlowId>)> {
    match entry {
        RepeaterHistoryEntryId::Archive(index) => {
            let archive = archive_for_tab(connection, tab)?.context("此标签没有源归档历史")?;
            let request = archive_request(connection, &archive, index)?;
            let loaded =
                ext_archive::load_repeater_request(connection, &request, workspace, Some(tab), cancel)?;
            Ok((loaded.draft, loaded.archive_response, None))
        }
        RepeaterHistoryEntryId::Live(flow) => {
            let live = live_meta(connection, tab.id, flow)?;
            let length = archive_for_tab(connection, tab)?
                .as_ref()
                .map_or(0, |archive| archive.length);
            ensure!(live.boundary <= length, "Repeater live 历史边界越界");
            let total = workspace
                .draft_bytes()?
                .checked_sub(tab.draft.raw.len())
                .and_then(|bytes| bytes.checked_add(live.raw_bytes))
                .context("Repeater 工作稿大小溢出")?;
            ensure!(total <= 64 * 1024 * 1024, "所选历史将使工作稿超过 64 MiB");
            check_cancel(cancel)?;
            let raw: Vec<u8> = connection.query_row("SELECT CASE WHEN typeof(raw)='blob' AND length(raw)=?3 THEN raw END FROM main.repeater_history WHERE tab=?1 AND flow=?2", params![tab.id.to_string(), flow.to_string(), live.raw_bytes as i64], |row| row.get(0))?;
            check_cancel(cancel)?;
            Ok((
                Draft {
                    revision: live.draft.revision,
                    target: live.draft.target,
                    raw,
                    version: live.draft.version,
                },
                None,
                Some(flow),
            ))
        }
    }
}
