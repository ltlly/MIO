use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result, ensure};
use mio_proxy::CapturedFlow;
use rusqlite::{Transaction, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;

use crate::{
    AuditPhase, FlowId, FlowState, FlowSummary, SlimSaveOptions, SlimSaveReport, TrafficSource,
    Uuid, target_issues::IssueSeverity,
};

use super::{
    CapturedWebSocket, IntruderMutation, MAX_MESSAGE_HEAD_BYTES, OwnedResponseHead, ProjectStore,
    ext_archive::{
        encode_chunk,
        http_query::filters::{HistoryMime, infer_history_mime},
        read_chunk, source_state,
    },
    intruder, metadata, plugin_storage, provenance, schema_guard,
};

impl ProjectStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn slim_save_as(
        &mut self,
        destination: &Path,
        options: &SlimSaveOptions,
        records: &[CapturedFlow],
        websocket_records: &[CapturedWebSocket],
        intruder_mutations: &[IntruderMutation],
        pending_provenance: &[(FlowId, crate::FlowProvenance)],
        cancel: &CancellationToken,
    ) -> Result<SlimSaveReport> {
        ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
        ensure!(
            self.path.as_deref() != Some(destination),
            "瘦身另存为必须选择新的目标路径，不会原地覆盖当前项目"
        );
        for capture in records {
            self.intruder()
                .reject_session_flow_write(capture.record.summary.id)?;
        }
        for mutation in intruder_mutations {
            if let IntruderMutation::Result {
                result: Ok(record), ..
            } = mutation
            {
                self.intruder()
                    .reject_session_flow_write(record.summary.id)?;
            }
        }

        let old_bytes = self.total_bytes()?;
        let (mut verified, (removed_flows, compacted_ext_chunks)) = self
            .stage_backup_and_publish(
                destination,
                cancel,
                "瘦身保存已取消",
                "瘦身快照拷贝未完成",
                &format!("验证瘦身后的项目文件 {} 失败", destination.display()),
                |staged| {
                    staged.apply_pending_with_notes(
                        (None, &[], &[], &[]),
                        records,
                        websocket_records,
                        intruder_mutations,
                        cancel,
                    )?;
                    if !pending_provenance.is_empty() || !records.is_empty() || !websocket_records.is_empty() {
                        let tx = staged
                            .connection
                            .transaction_with_behavior(TransactionBehavior::Immediate)?;
                        for (flow_id, prov) in pending_provenance {
                            let exists: bool = tx.query_row(
                                "SELECT EXISTS(SELECT 1 FROM main.flows WHERE id=?1)",
                                [flow_id.to_string()],
                                |row| row.get(0),
                            )?;
                            if exists {
                                provenance::write_flow_provenance(&tx, *flow_id, prov)?;
                            }
                        }
                        super::finish_pending_records(&tx, false, false)?;
                        tx.commit()?;
                    }
                    let slim_stats = apply_slim_passes(staged, options, cancel)?;
                    ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
                    staged.connection.execute_batch("VACUUM;")?;
                    Ok(slim_stats)
                },
            )?;
        verified.checkpoint()?;
        verified.close()?;
        let new_bytes = crate::sys_metrics::measure_project_disk_bytes(destination)?;
        Ok(SlimSaveReport {
            old_bytes,
            new_bytes,
            removed_flows,
            compacted_ext_chunks,
        })
    }
}

fn apply_slim_passes(
    staged: &mut ProjectStore,
    options: &SlimSaveOptions,
    cancel: &CancellationToken,
) -> Result<(usize, usize)> {
    let mut removed_flows = 0usize;
    let tx = staged
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    schema_guard::validate(&tx)?;

    if options.drop_finished_intruder_results {
        let terminal_runs: Vec<(Uuid, u64)> = {
            let mut stmt = tx.prepare(&intruder::run_select())?;
            let mut rows = stmt.query([])?;
            let mut runs = Vec::new();
            while let Some(row) = rows.next()? {
                ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
                let run = intruder::decode_run(row)?;
                if run.state.is_terminal() {
                    runs.push((run.id, run.revision));
                }
            }
            runs
        };
        for (run_id, rev) in terminal_runs {
            ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
            let mut ws = metadata::workspace(&tx)?;
            ws.revision += 1;
            let receipt = intruder::discard_project_run(&tx, staged.id, run_id, rev, ws, cancel)?;
            removed_flows = removed_flows
                .saturating_add(usize::try_from(receipt.deleted_flows).unwrap_or(usize::MAX));
        }
    }

    let pinned = collect_pinned_flows(&tx)?;
    let candidate_deletes = select_unpinned_flows_to_drop(&tx, &pinned, options, cancel)?;
    if !candidate_deletes.is_empty() {
        {
            let mut del_msgs = tx.prepare("DELETE FROM main.messages WHERE flow=?1")?;
            let mut del_flow = tx.prepare("DELETE FROM main.flows WHERE id=?1")?;
            for &flow_id in &candidate_deletes {
                ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
                let key = flow_id.to_string();
                del_msgs.execute([&key])?;
                let count = del_flow.execute([&key])?;
                ensure!(count == 1, "瘦身删除流量 {flow_id} 数量异常");
            }
        }
        plugin_storage::remove_live_http_annotations(&tx, &candidate_deletes)?;
        provenance::remove_flow_provenance(&tx, &candidate_deletes)?;
        {
            let mut del_meta =
                tx.prepare("DELETE FROM main.metadata WHERE key=?1 COLLATE BINARY")?;
            for &flow_id in &candidate_deletes {
                del_meta.execute([format!(
                    "{}{flow_id}",
                    crate::store::messages::HTTP_HISTORY_META_PREFIX
                )])?;
            }
        }
        unlink_dropped_audit_flows(&tx, &candidate_deletes, cancel)?;
        removed_flows = removed_flows.saturating_add(candidate_deletes.len());
    }

    // Also prune orphan flow_provenance:* keys whose flows were previously discarded.
    tx.execute(
        "DELETE FROM main.metadata
         WHERE key GLOB 'flow_provenance:*'
           AND substr(key, 17) NOT IN (SELECT id FROM main.flows)",
        [],
    )?;
    tx.execute(
        "DELETE FROM main.metadata
         WHERE key GLOB 'http_history_meta:*'
           AND substr(key, 19) NOT IN (SELECT id FROM main.flows)",
        [],
    )?;

    let compacted_chunks = if options.compact_ext_source_chunks {
        compact_ext_chunks(&tx, cancel)?
    } else {
        0
    };

    ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
    tx.commit()?;
    Ok((removed_flows, compacted_chunks))
}

fn collect_pinned_flows(tx: &Transaction<'_>) -> Result<HashSet<FlowId>> {
    let mut pinned = HashSet::new();
    let workspace = metadata::workspace(tx)?;
    for tab in workspace.tabs.iter().chain(&workspace.closed) {
        if let Some(flow_id) = tab.last_flow {
            pinned.insert(flow_id);
        }
    }
    let audit_state = plugin_storage::read_audit_state(tx)?;
    for finding in &audit_state.findings {
        if (finding.phase == AuditPhase::Active || finding.severity() != IssueSeverity::Information)
            && let Some(id) = finding.flow_id
        {
            pinned.insert(id);
        }
    }
    let annotations = plugin_storage::read_live_http_annotations(tx)?;
    for (flow_id, entry) in annotations {
        if !entry.notes.is_empty() || entry.highlight > 0 {
            pinned.insert(flow_id);
        }
    }
    for query in [
        "SELECT flow FROM main.repeater_history WHERE flow IS NOT NULL",
        "SELECT flow FROM main.websocket_connections",
        "SELECT flow FROM main.websocket_messages",
        "SELECT flow FROM main.intruder_results WHERE flow IS NOT NULL",
        "SELECT id FROM main.site_map_entries",
        "SELECT substr(owner,6) FROM main.site_map_entries WHERE owner GLOB 'live:*'",
    ] {
        let mut stmt = tx.prepare(query)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let raw: String = row.get(0)?;
            if let Ok(id) = Uuid::parse_str(&raw) {
                pinned.insert(id);
            }
        }
    }
    Ok(pinned)
}

fn unlink_dropped_audit_flows(
    tx: &Transaction<'_>,
    dropped: &[FlowId],
    cancel: &CancellationToken,
) -> Result<()> {
    let dropped_set: HashSet<FlowId> = dropped.iter().copied().collect();
    let mut state = plugin_storage::read_audit_state(tx)?;
    let mut changed = false;
    for finding in &mut state.findings {
        if finding.flow_id.is_some_and(|id| dropped_set.contains(&id)) {
            finding.flow_id = None;
            changed = true;
        }
    }
    if changed {
        let _ = cancel;
        state.revision = state.revision.saturating_add(1);
        let encoded = serde_json::to_vec(&state)?;
        ensure!(
            encoded.len() <= crate::MAX_AUDIT_STATE_BYTES,
            "审计状态超过编码限额"
        );
        tx.execute(
            "INSERT OR REPLACE INTO main.metadata(key, value) VALUES('audit_state', ?1)",
            [encoded],
        )?;
    }
    Ok(())
}

fn select_unpinned_flows_to_drop(
    tx: &Transaction<'_>,
    pinned: &HashSet<FlowId>,
    options: &SlimSaveOptions,
    cancel: &CancellationToken,
) -> Result<Vec<FlowId>> {
    if !options.drop_scanner_and_unreferenced_tool_flows
        && !options.drop_static_asset_flows
        && options.max_unpinned_body_bytes.is_none()
    {
        return Ok(Vec::new());
    }
    let want_body_len = options.max_unpinned_body_bytes.is_some();
    let want_head = options.drop_static_asset_flows;
    let body_expr = if want_body_len {
        "(SELECT max(length(m.body)) FROM main.messages m WHERE m.flow=f.id AND m.side IN (1, 3))"
    } else {
        "NULL"
    };
    let (head_expr, prefix_expr, head_join) = if want_head {
        (
            "m1.head",
            "substr(m1.body, 1, 64)",
            "LEFT JOIN main.messages m1 ON m1.flow=f.id AND m1.side=(
                 SELECT min(ms.side) FROM main.messages ms
                 WHERE ms.flow=f.id AND ms.side IN (1, 3)
                   AND typeof(ms.head)='blob' AND length(ms.head)<=?1
             )",
        )
    } else {
        ("NULL", "NULL", "")
    };
    let sql = format!(
        "SELECT f.id, f.source, f.response_bytes,
                CASE WHEN f.source IS NULL OR f.response_bytes IS NULL THEN f.summary END,
                {body_expr}, {head_expr}, {prefix_expr}
         FROM main.flows f
         {head_join}
         WHERE f.state NOT IN ('pending', 'intercepted')
           AND (f.source IS NULL OR f.source NOT IN ('Repeater', 'Intruder'))"
    );
    let mut stmt = tx.prepare(&sql)?;
    let mut rows = if want_head {
        stmt.query([MAX_MESSAGE_HEAD_BYTES as i64])?
    } else {
        stmt.query([])?
    };
    let mut to_drop = Vec::new();
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
        let raw_id: String = row.get(0)?;
        let flow_id = Uuid::parse_str(&raw_id).context("流量 ID 损坏")?;
        if pinned.contains(&flow_id) {
            continue;
        }
        let source_col: Option<String> = row.get(1)?;
        let resp_bytes_col: Option<i64> = row.get(2)?;
        let (is_scanner, response_bytes) = match (source_col.as_deref(), resp_bytes_col) {
            (Some(src), Some(bytes)) => {
                (src == "Scanner", u64::try_from(bytes.max(0)).unwrap_or(0))
            }
            _ => {
                let summary_bytes: Vec<u8> =
                    row.get::<_, Option<Vec<u8>>>(3)?.context("流量摘要缺失")?;
                let summary: FlowSummary = serde_json::from_slice(&summary_bytes)?;
                if matches!(summary.state, FlowState::Pending | FlowState::Intercepted)
                    || matches!(
                        summary.source,
                        TrafficSource::Repeater | TrafficSource::Intruder
                    )
                {
                    continue;
                }
                (
                    matches!(summary.source, TrafficSource::Scanner),
                    summary.response_bytes,
                )
            }
        };
        if options.drop_scanner_and_unreferenced_tool_flows && is_scanner {
            to_drop.push(flow_id);
            continue;
        }
        if let Some(max_bytes) = options.max_unpinned_body_bytes {
            let max_body: Option<i64> = row.get(4)?;
            let max_i64 = i64::try_from(max_bytes).unwrap_or(i64::MAX);
            if max_body.is_some_and(|len| len > max_i64) || response_bytes > max_bytes as u64 {
                to_drop.push(flow_id);
                continue;
            }
        }
        if want_head && let Some(head_bytes) = row.get::<_, Option<Vec<u8>>>(5)? {
            let body_prefix: Vec<u8> = row.get::<_, Option<Vec<u8>>>(6)?.unwrap_or_default();
            let head: OwnedResponseHead = serde_json::from_slice(&head_bytes)?;
            let mime = infer_history_mime(&head.headers, &body_prefix);
            if matches!(
                mime,
                HistoryMime::Images | HistoryMime::Css | HistoryMime::Flash
            ) {
                to_drop.push(flow_id);
            }
        }
    }
    Ok(to_drop)
}

fn compact_ext_chunks(tx: &Transaction<'_>, cancel: &CancellationToken) -> Result<usize> {
    let sources: Vec<Uuid> = {
        let mut stmt = tx.prepare("SELECT id FROM main.ext_sources")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(Uuid::parse_str(&row?)?);
        }
        out
    };
    if sources.is_empty() {
        return Ok(0);
    }
    let mut compacted = 0usize;
    for source_id in sources {
        let state = source_state(tx, source_id)?;
        if state.source_bytes == 0 || !state.complete {
            continue;
        }
        let source_str = source_id.to_string();
        let uncompressed_ordinals: Vec<u64> = {
            let mut stmt = tx.prepare(
                "SELECT ordinal FROM main.ext_source_chunks WHERE source=?1 AND codec=0 ORDER BY ordinal ASC",
            )?;
            let rows = stmt.query_map([&source_str], |r| r.get::<_, u64>(0))?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row?);
            }
            list
        };
        let mut update = tx.prepare(
            "UPDATE main.ext_source_chunks SET codec=?1, data=?2 WHERE source=?3 AND ordinal=?4",
        )?;
        for ordinal in uncompressed_ordinals {
            ensure!(!cancel.is_cancelled(), "瘦身保存已取消");
            let raw = read_chunk(tx, source_id, state.source_bytes, ordinal)?;
            let (codec, data) = encode_chunk(&raw)?;
            if codec == 1 && data.len() < raw.len() {
                update.execute(params![codec, data, &source_str, i64::try_from(ordinal)?])?;
                compacted += 1;
            }
        }
    }
    Ok(compacted)
}
