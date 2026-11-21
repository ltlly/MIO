use super::*;

impl ProjectStore {
    pub(crate) fn begin_repeater_send(
        &mut self,
        tab_id: TabId,
        draft: &Draft,
        pending: &FlowRecord,
        cancel: &CancellationToken,
    ) -> Result<()> {
        check_cancel(cancel)?;
        self.reject_session_intruder_flow_write(pending.summary.id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let workspace = metadata::workspace(&transaction)?;
        let tab = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .context("Repeater 标签已关闭，未发送")?;
        ensure!(
            tab.draft == *draft && draft.revision > 0,
            "Repeater 发送稿与已保存 revision 不一致"
        );
        ensure!(
            pending.summary.source == TrafficSource::Repeater
                && pending.summary.state == FlowState::Pending
                && pending.summary.revision == 1
                && pending.response.is_none(),
            "Repeater 初始流量不是 Pending 请求"
        );
        ensure!(
            pending.summary.method == pending.request.method
                && pending.summary.url == pending.request.url
                && pending.summary.status.is_none()
                && pending.summary.response_bytes == 0
                && pending.summary.request_bytes == pending.request.body.len() as u64,
            "Repeater Pending 摘要与请求不一致"
        );
        ensure!(
            pending.request
                == crate::http::parse_repeater_request(
                    &draft.raw,
                    &draft.target,
                    draft.version,
                    false
                )?
                || pending.request
                    == crate::http::parse_repeater_request(
                        &draft.raw,
                        &draft.target,
                        draft.version,
                        true
                    )?,
            "Repeater 流量与原始发送稿不一致"
        );
        ensure!(!pending.summary.id.is_nil(), "Repeater flow ID 无效");
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.flows WHERE id=?1)",
            [pending.summary.id.to_string()],
            |row| row.get(0),
        )?;
        ensure!(!exists, "Repeater flow ID 已存在，未重新登记或发送");
        let archive = archive_for_tab(&transaction, tab)?;
        let archive_len = archive.as_ref().map_or(0, |archive| archive.length);
        let (boundary, ordinal) = match tab
            .history
            .as_ref()
            .and_then(|history| history.cursor.as_ref())
        {
            Some(RepeaterHistoryCursor {
                entry: RepeaterHistoryEntryId::Archive(index),
                ..
            }) => {
                ensure!(*index < archive_len, "Repeater 源历史位置越界");
                (index.checked_add(1).context("Repeater 历史边界溢出")?, 0)
            }
            Some(RepeaterHistoryCursor {
                entry: RepeaterHistoryEntryId::Live(flow),
                ..
            }) => {
                let live = live_meta(&transaction, tab_id, *flow)?;
                (
                    live.boundary,
                    live.ordinal
                        .checked_add(1)
                        .context("Repeater 历史顺序溢出")?,
                )
            }
            None => (0, 0),
        };
        ensure!(boundary <= archive_len, "Repeater live 历史边界越界");
        let count = boundary_count(&transaction, tab_id, boundary)?;
        ensure!(ordinal <= count, "Repeater 插入位置越界");
        let meta = serde_json::to_vec(&DraftMeta {
            revision: draft.revision,
            target: draft.target.clone(),
            version: draft.version,
            send_cursor: tab
                .history
                .as_ref()
                .and_then(|history| history.cursor.clone()),
        })?;
        ensure!(
            meta.len() <= MAX_META && !draft.raw.is_empty() && draft.raw.len() <= MAX_RAW,
            "Repeater 原始稿超过保存限额"
        );
        check_cancel(cancel)?;
        write_record(&transaction, pending)?;
        if ordinal < count {
            // Move the affected suffix above its old range, then settle at old ordinal + 1.
            let shift = count.checked_add(1).context("Repeater 历史顺序溢出")?;
            i64::try_from(count.checked_add(shift).context("Repeater 历史顺序溢出")?)?;
            let changed = transaction.execute("UPDATE main.repeater_history SET ordinal=ordinal+?4 WHERE tab=?1 AND archive_boundary=?2 AND ordinal>=?3", params![tab_id.to_string(), i64::try_from(boundary)?, i64::try_from(ordinal)?, i64::try_from(shift)?])?;
            ensure!(
                changed as u64 == count - ordinal,
                "Repeater 历史后继范围已改变"
            );
            check_cancel(cancel)?;
            transaction.execute("UPDATE main.repeater_history SET ordinal=ordinal-?4+1 WHERE tab=?1 AND archive_boundary=?2 AND ordinal>=?3", params![tab_id.to_string(), i64::try_from(boundary)?, i64::try_from(ordinal + shift)?, i64::try_from(shift)?])?;
        }
        transaction.execute("INSERT INTO main.repeater_history(tab,archive_boundary,ordinal,flow,draft_meta,raw) VALUES(?1,?2,?3,?4,?5,?6)", params![tab_id.to_string(), i64::try_from(boundary)?, i64::try_from(ordinal)?, pending.summary.id.to_string(), meta, &draft.raw])?;
        check_cancel(cancel)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn repeater_history_page(
        &self,
        tab_id: TabId,
        start: u64,
        limit: usize,
    ) -> Result<RepeaterHistoryPage> {
        ensure!((1..=64).contains(&limit), "Repeater 历史页大小必须为 1–64");
        let transaction = self.connection.unchecked_transaction()?;
        let workspace = metadata::workspace(&transaction)?;
        let tab = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .context("Repeater 标签已关闭")?;
        let archive = archive_for_tab(&transaction, tab)?;
        let archive_len = archive.as_ref().map_or(0, |archive| archive.length);
        let (live_count, last_boundary): (u64, Option<u64>) = transaction.query_row(
            "SELECT count(*),max(archive_boundary) FROM main.repeater_history WHERE tab=?1",
            [tab_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            last_boundary.is_none_or(|boundary| boundary <= archive_len),
            "Repeater live 历史越过源目录末尾"
        );
        let total = archive_len
            .checked_add(live_count)
            .context("Repeater 历史长度溢出")?;
        ensure!(start <= total, "Repeater 历史页起点越界");
        let current_position = tab
            .history
            .as_ref()
            .and_then(|history| history.cursor.as_ref())
            .map(|cursor| entry_position(&transaction, tab_id, cursor.entry, archive_len))
            .transpose()?;
        let mut rows = Vec::with_capacity(limit);
        let mut position = start;
        let mut bytes = 0;
        let (mut boundary, mut at_archive, mut ordinal) =
            locate(&transaction, tab_id, start, archive_len)?;
        let mut counted_boundary = None;
        while position < total && rows.len() < limit {
            let entry = if at_archive {
                let archive = archive.as_ref().context("Repeater 源目录缺失")?;
                let entry = archive_entry(&transaction, archive, boundary, position)?;
                boundary += 1;
                ordinal = 0;
                at_archive = false;
                entry
            } else {
                let count = if let Some((_, count)) =
                    counted_boundary.filter(|(cached, _)| *cached == boundary)
                {
                    count
                } else {
                    let count = boundary_count(&transaction, tab_id, boundary)?;
                    counted_boundary = Some((boundary, count));
                    count
                };
                if ordinal == count {
                    ensure!(boundary < archive_len, "Repeater 历史页长度与索引不一致");
                    at_archive = true;
                    continue;
                }
                ensure!(ordinal < count, "Repeater live 历史页顺序越界");
                let flow: String = transaction.query_row("SELECT flow FROM main.repeater_history WHERE tab=?1 AND archive_boundary=?2 AND ordinal=?3", params![tab_id.to_string(), i64::try_from(boundary)?, i64::try_from(ordinal)?], |row| row.get(0))?;
                let flow = Uuid::parse_str(&flow)?;
                let summary = flow_summary(&transaction, flow)?;
                ordinal += 1;
                RepeaterHistoryEntry {
                    id: RepeaterHistoryEntryId::Live(flow),
                    position,
                    method: Some(summary.method.clone()),
                    target: Some(summary.url.clone()),
                    status: summary.status,
                    diagnostic: flow_diagnostic(&summary),
                }
            };
            let size = entry.method.as_ref().map_or(0, String::len)
                + entry.target.as_ref().map_or(0, String::len)
                + entry.diagnostic.as_ref().map_or(0, String::len)
                + 128;
            if bytes + size > PAGE_BYTES {
                break;
            }
            bytes += size;
            rows.push(entry);
            position += 1;
        }
        ensure!(
            start == total || !rows.is_empty(),
            "Repeater 历史条目超过分页限额"
        );
        transaction.commit()?;
        Ok(RepeaterHistoryPage {
            total,
            start,
            current_position,
            rows,
            next_offset: (position < total).then_some(position),
        })
    }

    pub(crate) fn load_repeater_history(
        &self,
        tab_id: TabId,
        entry: RepeaterHistoryEntryId,
        workspace_revision: u64,
        draft_revision: u64,
        cancel: &CancellationToken,
    ) -> Result<WorkTab> {
        check_cancel(cancel)?;
        let transaction = self.connection.unchecked_transaction()?;
        let workspace = metadata::workspace(&transaction)?;
        ensure!(
            workspace.revision == workspace_revision,
            "工作区 revision 已变化，未载入旧历史选择"
        );
        let old = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .context("Repeater 标签已关闭")?;
        ensure!(
            old.draft.revision == draft_revision,
            "草稿 revision 已变化，未覆盖当前编辑"
        );
        let next = draft_revision
            .checked_add(1)
            .context("草稿 revision 已耗尽")?;
        let (mut draft, mut response, last_flow) =
            entry_content(&transaction, old, entry, &workspace, cancel)?;
        draft.revision = next;
        if let Some(response) = &mut response {
            response.draft_revision = next;
        }
        let tab = WorkTab {
            id: old.id,
            title: old.title.clone(),
            group: old.group,
            draft,
            last_flow,
            archive_response: response,
            history: Some(RepeaterHistory {
                source: old
                    .history
                    .as_ref()
                    .and_then(|history| history.source.clone()),
                cursor: Some(RepeaterHistoryCursor {
                    entry,
                    draft_revision: next,
                }),
            }),
        };
        check_cancel(cancel)?;
        transaction.commit()?;
        Ok(tab)
    }
}
