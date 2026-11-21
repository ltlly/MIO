use super::*;

fn live_before(connection: &Connection, tab: TabId, boundary: u64, inclusive: bool) -> Result<u64> {
    let sql = if inclusive {
        "SELECT count(*) FROM main.repeater_history WHERE tab=?1 AND archive_boundary<=?2"
    } else {
        "SELECT count(*) FROM main.repeater_history WHERE tab=?1 AND archive_boundary<?2"
    };
    Ok(connection.query_row(
        sql,
        params![tab.to_string(), i64::try_from(boundary)?],
        |row| row.get(0),
    )?)
}

pub(super) fn boundary_count(connection: &Connection, tab: TabId, boundary: u64) -> Result<u64> {
    let (count, first, last): (u64, Option<u64>, Option<u64>) = connection.query_row("SELECT count(*),min(ordinal),max(ordinal) FROM main.repeater_history WHERE tab=?1 AND archive_boundary=?2", params![tab.to_string(), i64::try_from(boundary)?], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    ensure!(
        (count == 0 && first.is_none() && last.is_none())
            || (first == Some(0) && last == count.checked_sub(1)),
        "Repeater live 历史顺序存在缺口"
    );
    Ok(count)
}

pub(super) fn locate(
    connection: &Connection,
    tab: TabId,
    position: u64,
    archive_len: u64,
) -> Result<(u64, bool, u64)> {
    let (mut low, mut high) = (0, archive_len);
    while low < high {
        let middle = low + (high - low) / 2;
        let rank = middle
            .checked_add(live_before(connection, tab, middle, true)?)
            .context("Repeater 历史位置溢出")?;
        if rank < position {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low < archive_len && low + live_before(connection, tab, low, true)? == position {
        return Ok((low, true, 0));
    }
    let base = low
        .checked_add(live_before(connection, tab, low, false)?)
        .context("Repeater 历史位置溢出")?;
    Ok((
        low,
        false,
        position
            .checked_sub(base)
            .context("Repeater 历史位置无效")?,
    ))
}

pub(super) fn entry_position(
    connection: &Connection,
    tab: TabId,
    entry: RepeaterHistoryEntryId,
    archive_len: u64,
) -> Result<u64> {
    match entry {
        RepeaterHistoryEntryId::Archive(index) => {
            ensure!(index < archive_len, "Repeater 源历史位置越界");
            index
                .checked_add(live_before(connection, tab, index, true)?)
                .context("Repeater 历史位置溢出")
        }
        RepeaterHistoryEntryId::Live(flow) => {
            let live = live_meta(connection, tab, flow)?;
            ensure!(live.boundary <= archive_len, "Repeater live 历史边界越界");
            ensure!(
                live.ordinal < boundary_count(connection, tab, live.boundary)?,
                "Repeater live 历史位置不存在"
            );
            live.boundary
                .checked_add(live_before(connection, tab, live.boundary, false)?)
                .and_then(|value| value.checked_add(live.ordinal))
                .context("Repeater 历史位置溢出")
        }
    }
}

pub(super) fn flow_diagnostic(summary: &FlowSummary) -> Option<String> {
    match &summary.state {
        FlowState::Complete => None,
        FlowState::Failed(reason) => Some(reason.clone()),
        state => Some(state.label().into()),
    }
}

pub(super) fn flow_summary(connection: &Connection, flow: FlowId) -> Result<FlowSummary> {
    let (revision, bytes): (u64, Vec<u8>) = connection.query_row("SELECT revision,CASE WHEN typeof(summary)='blob' AND length(summary)<=1048576 THEN summary END FROM main.flows WHERE id=?1", [flow.to_string()], |row| Ok((row.get(0)?, row.get(1)?))).context("Repeater 历史 flow 摘要缺失或过大")?;
    let summary: FlowSummary = serde_json::from_slice(&bytes)?;
    ensure!(
        summary.id == flow
            && summary.revision == revision
            && summary.source == TrafficSource::Repeater,
        "Repeater 历史 flow 归属或 revision 不一致"
    );
    Ok(summary)
}

pub(super) fn live_meta(connection: &Connection, tab: TabId, flow: FlowId) -> Result<Live> {
    let mut query = connection.prepare("SELECT archive_boundary,ordinal,CASE WHEN typeof(draft_meta)='blob' AND length(draft_meta)<=?3 THEN draft_meta END,CASE WHEN typeof(raw)='blob' AND length(raw)<=?4 THEN length(raw) END FROM main.repeater_history WHERE tab=?1 AND flow=?2 LIMIT 2")?;
    let mut rows = query.query(params![
        tab.to_string(),
        flow.to_string(),
        MAX_META as i64,
        MAX_RAW as i64
    ])?;
    let row = rows
        .next()?
        .context("该 live 历史不属于所选 Repeater 标签")?;
    let boundary: u64 = row.get(0)?;
    let ordinal: u64 = row.get(1)?;
    let bytes: Vec<u8> = row.get(2)?;
    let draft: DraftMeta = serde_json::from_slice(&bytes)?;
    let raw_bytes: usize = row.get(3)?;
    ensure!(rows.next()?.is_none(), "Repeater 历史 flow 关联重复");
    ensure!(
        draft.revision > 0
            && draft.target.len() <= 64 * 1024
            && raw_bytes > 0
            && draft.send_cursor.as_ref().is_none_or(|cursor| {
                cursor.draft_revision > 0 && cursor.draft_revision <= draft.revision
            }),
        "Repeater 历史原始稿无效"
    );
    flow_summary(connection, flow)?;
    Ok(Live {
        boundary,
        ordinal,
        draft,
        raw_bytes,
    })
}

pub(in crate::store) fn validate_workspace(
    connection: &Connection,
    workspace: &Workspace,
) -> Result<()> {
    for tab in workspace.tabs.iter().chain(&workspace.closed) {
        let Some(history) = &tab.history else {
            continue;
        };
        let schema: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(schema == crate::store::SCHEMA, "旧版本项目包含未确认的 Repeater 导航状态");
        let archive = archive_for_tab(connection, tab)?;
        let Some(cursor) = &history.cursor else {
            continue;
        };
        match cursor.entry {
            RepeaterHistoryEntryId::Archive(index) => {
                let archive = archive.as_ref().context("归档导航缺少来源标签")?;
                let request = archive_request(connection, archive, index)?;
                let mut expected = ext_archive::archived_response_for_request(connection, &request)?;
                if let Some(response) = &mut expected {
                    response.draft_revision = cursor.draft_revision;
                }
                ensure!(
                    tab.last_flow.is_none() && tab.archive_response == expected,
                    "归档导航的响应绑定不一致"
                );
            }
            RepeaterHistoryEntryId::Live(flow) => {
                let live = live_meta(connection, tab.id, flow)?;
                ensure!(
                    live.boundary <= archive.as_ref().map_or(0, |archive| archive.length)
                        && tab.last_flow == Some(flow)
                        && tab.archive_response.is_none(),
                    "live 导航的 flow 或边界绑定不一致"
                );
            }
        }
    }
    Ok(())
}

pub(in crate::store) fn validate_change(
    connection: &Connection,
    workspace: &Workspace,
    old: &WorkTab,
    new: &WorkTab,
) -> Result<()> {
    let old_source = old
        .history
        .as_ref()
        .and_then(|history| history.source.as_ref());
    let new_source = new
        .history
        .as_ref()
        .and_then(|history| history.source.as_ref());
    ensure!(
        old_source == new_source,
        "已有 Repeater 标签不能更换历史来源"
    );
    ensure!(
        old.history.is_none() || new.history.is_some(),
        "已有 Repeater 历史不能被清除"
    );
    let old_cursor = old
        .history
        .as_ref()
        .and_then(|history| history.cursor.as_ref());
    let new_cursor = new
        .history
        .as_ref()
        .and_then(|history| history.cursor.as_ref());
    if old_cursor == new_cursor {
        ensure!(
            new.archive_response.is_none() || new.archive_response == old.archive_response,
            "已保存标签的归档响应只能保留或通过历史导航重新绑定"
        );
        return Ok(());
    }
    let cursor = new_cursor.context("历史导航不能清除已选条目")?;
    if let RepeaterHistoryEntryId::Live(flow) = cursor.entry
        && cursor.draft_revision <= old.draft.revision
    {
        let live = live_meta(connection, old.id, flow)?;
        if cursor.draft_revision == live.draft.revision
            && old.draft.revision >= live.draft.revision
            && old_cursor == live.draft.send_cursor.as_ref()
        {
            if old.draft.revision == live.draft.revision {
                ensure!(
                    live.draft.target == old.draft.target
                        && live.draft.version == old.draft.version
                        && live.raw_bytes == old.draft.raw.len(),
                    "发送完成绑定与已登记原始稿不一致"
                );
                let same_raw: bool = connection.query_row(
                    "SELECT raw=?3 FROM main.repeater_history WHERE tab=?1 AND flow=?2",
                    params![old.id.to_string(), flow.to_string(), &old.draft.raw],
                    |row| row.get(0),
                )?;
                ensure!(same_raw, "发送完成绑定与已登记原始稿不一致");
            }
            return Ok(());
        }
    }
    ensure!(
        old.draft.revision.checked_add(1) == Some(new.draft.revision)
            && cursor.draft_revision == new.draft.revision,
        "历史导航必须绑定下一草稿 revision"
    );
    let (expected, _, _) = entry_content(
        connection,
        new,
        cursor.entry,
        workspace,
        &CancellationToken::new(),
    )?;
    ensure!(
        new.draft.raw == expected.raw
            && new.draft.target == expected.target
            && new.draft.version == expected.version,
        "历史导航稿与所选原始稿不一致"
    );
    Ok(())
}
