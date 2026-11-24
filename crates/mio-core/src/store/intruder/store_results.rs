use super::*;

impl crate::store::IntruderRepo<&mut ProjectStore> {
    #[cfg(test)]
    pub fn put_result(
        &mut self,
        id: Uuid,
        ordinal: u64,
        result: std::result::Result<&FlowRecord, &str>,
    ) -> Result<IntruderRunSummary> {
        self.put_result_with_reason_phrase(id, ordinal, result, None)
    }

    pub(crate) fn put_result_with_reason_phrase(
        &mut self,
        id: Uuid,
        ordinal: u64,
        result: std::result::Result<&FlowRecord, &str>,
        reason_phrase: Option<&[u8]>,
    ) -> Result<IntruderRunSummary> {
        if self.0.intruder_sessions.contains(id) {
            return self.write_session_result_with_reason_phrase(
                id,
                ordinal,
                result,
                reason_phrase,
            );
        }
        if let Ok(record) = result {
            self.0
                .intruder()
                .reject_session_flow_write(record.summary.id)?;
        }
        let transaction = self.0.connection.transaction()?;
        let run =
            write_result_with_reason_phrase(&transaction, id, ordinal, result, reason_phrase)?;
        transaction.commit()?;
        Ok(run)
    }

    pub fn set_state(
        &mut self,
        id: Uuid,
        state: IntruderRunState,
    ) -> Result<IntruderRunSummary> {
        if self.0.intruder_sessions.contains(id) {
            return self.write_session_state(id, state);
        }
        let transaction = self.0.connection.transaction()?;
        let run = write_state(&transaction, id, state)?;
        transaction.commit()?;
        Ok(run)
    }

    pub fn set_result_notes(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        ordinal: u64,
        expected_notes_revision: u64,
        notes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(u64, Vec<u8>)> {
        self.0.intruder().check_session_owner(project, id)?;
        ensure!(
            expected_notes_revision <= i64::MAX as u64,
            "Intruder 结果备注版本越界"
        );
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果备注修改已取消；数据保留"
        );
        ensure!(
            notes.len() <= MAX_ERROR_BYTES,
            "Intruder 结果备注超过 64 KiB 字节限额"
        );
        if self.0.intruder_sessions.contains(id) {
            return self.write_session_result_notes(
                id,
                ordinal,
                expected_notes_revision,
                notes,
                cancel,
            );
        }
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let saved = write_result_notes(
            &transaction,
            id,
            ordinal,
            expected_notes_revision,
            notes,
            cancel,
        )?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果备注修改已取消；数据保留"
        );
        transaction.commit()?;
        Ok(saved)
    }

    pub fn set_result_highlight(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        entries: &[(u64, u64)],
        highlight: Option<u8>,
        cancel: &CancellationToken,
    ) -> Result<Vec<(u64, u64)>> {
        self.0.intruder().check_session_owner(project, id)?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果高亮修改已取消；数据保留"
        );
        ensure!(
            highlight.is_none_or(|color| color <= 9),
            "Intruder 结果高亮颜色不在已确认色板中"
        );
        ensure!(
            (1..=500).contains(&entries.len()),
            "Intruder 结果高亮批次数量无效"
        );
        ensure!(
            entries.iter().all(|&(ordinal, _)| ordinal < MAX_REQUESTS),
            "Intruder 结果高亮序号无效"
        );
        ensure!(
            entries
                .iter()
                .all(|&(_, expected)| expected <= i64::MAX as u64),
            "Intruder 结果高亮 revision 无效"
        );
        ensure!(
            entries
                .iter()
                .map(|&(ordinal, _)| ordinal)
                .collect::<BTreeSet<_>>()
                .len()
                == entries.len(),
            "Intruder 结果高亮序号重复"
        );
        if self.0.intruder_sessions.contains(id) {
            return self.write_session_result_highlight(id, entries, highlight, cancel);
        }
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let updated = write_result_highlights(&transaction, id, entries, highlight, cancel)?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果高亮修改已取消；数据保留"
        );
        transaction.commit()?;
        Ok(updated)
    }

    /// Crash recovery only, after old senders are gone. No requests are resumed or replayed.
    #[cfg(test)]
    pub fn interrupt_runs(&mut self) -> Result<usize> {
        self.recover_runs_on_open(false)
    }

    pub fn recover_runs_on_open(&mut self, pause_remaining: bool) -> Result<usize> {
        let transaction = self.0.connection.transaction()?;
        let mut changed = 0;
        let ids: Vec<Option<String>> = {
            let mut query = transaction.prepare(
                "SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END
                 FROM intruder_runs WHERE state IN (0,1) ORDER BY id",
            )?;
            query
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in ids {
            let id = Uuid::parse_str(&id.context("Intruder run ID 损坏")?)?;
            let mut run = read_run(&transaction, id)?;
            let mut seen = 0;
            let mut already_complete = 0;
            {
                // Stream summaries of this run; neither payloads nor message bodies are read.
                let mut query = transaction.prepare(&format!(
                    "{} WHERE r.run=?1 ORDER BY r.ordinal",
                    result_select()
                ))?;
                let mut rows = query.query([id.to_string()])?;
                while let Some(row) = rows.next()? {
                    let result = decode_result(row)?;
                    ensure!(
                        result.ordinal == seen && seen < run.dispatched,
                        "Intruder 中断恢复的结果序号或计数损坏"
                    );
                    seen += 1;
                    match result.outcome {
                        IntruderResultOutcome::Rejected(_) => already_complete += 1,
                        IntruderResultOutcome::Flow(mut summary) => {
                            if is_flow_terminal(&summary.state) {
                                already_complete += 1;
                                continue;
                            }
                            let references: i64 = transaction.query_row(
                                "SELECT count(*) FROM intruder_results WHERE flow=?1",
                                [summary.id.to_string()],
                                |row| row.get(0),
                            )?;
                            ensure!(
                                references == 1,
                                "Intruder 流量归属重复；未修改其它攻击的结果"
                            );
                            let revision = summary.revision;
                            summary.revision = next_revision(revision);
                            summary.state = FlowState::Failed(
                                "上次会话退出时仍在途；结果未知，未自动重发".into(),
                            );
                            let updated = transaction.execute(
                                "UPDATE flows SET revision=?1,state='failed',summary=?2 WHERE id=?3 AND revision=?4",
                                params![summary.revision, serde_json::to_vec(&summary)?, summary.id.to_string(), revision],
                            )?;
                            ensure!(updated == 1, "Intruder 中断恢复的 flow revision 冲突");
                        }
                    }
                }
            }
            ensure!(
                seen == run.dispatched && already_complete == run.completed,
                "Intruder 中断恢复计数不一致"
            );
            run.completed = run.dispatched;
            if let Some(baseline) = &mut run.baseline {
                baseline.completed = baseline.ordinal < run.dispatched;
            }
            let next_state = if pause_remaining
                && matches!(
                    run.state,
                    IntruderRunState::Running | IntruderRunState::Paused
                )
                && run.dispatched < run.total
            {
                IntruderRunState::Paused
            } else {
                IntruderRunState::Interrupted
            };
            if run.state != next_state || already_complete != seen {
                run.state = next_state;
                update_run(&transaction, &mut run)?;
                changed += 1;
            }
        }
        transaction.commit()?;
        Ok(changed)
    }
}

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::IntruderRepo<S> {
    pub fn results(
        &self,
        id: Uuid,
        after: Option<u64>,
        limit: usize,
    ) -> Result<IntruderResultPage> {
        ensure!((1..=500).contains(&limit), "Intruder 分页大小必须为 1–500");
        if let Some(dataset) = self.0.intruder().session_dataset(id)? {
            let (rows, next_after) = dataset.data.results_page(after, limit)?;
            self.0.intruder().validate_session_summaries(id, &rows)?;
            return Ok(IntruderResultPage {
                run: dataset.data.run.clone(),
                rows,
                next_after,
                persisted: false,
            });
        }
        let persisted = true;
        let transaction = self.0.connection.unchecked_transaction()?;
        let run = read_run(&transaction, id)?;
        ensure!(
            after.is_none_or(|value| value < run.total),
            "Intruder 分页游标超出计划"
        );
        let mut output = Vec::with_capacity(limit);
        let mut bytes = 0;
        let mut expected = after.map_or(0, |value| value + 1);
        let mut more = false;
        {
            let mut query = transaction.prepare(&format!(
                "{} WHERE r.run=?1 AND r.ordinal>?2 ORDER BY r.ordinal LIMIT ?3",
                result_select()
            ))?;
            let mut rows = query.query(params![
                id.to_string(),
                after.map(|v| v as i64).unwrap_or(-1),
                i64::try_from(limit + 1)?
            ])?;
            while let Some(row) = rows.next()? {
                if output.len() == limit {
                    more = true;
                    break;
                }
                let size = result_size(row)?;
                if !output.is_empty() && size > PAGE_BYTES - bytes {
                    more = true;
                    break;
                }
                let result = decode_result(row)?;
                ensure!(
                    !run.state.is_terminal() || outcome_terminal(&result.outcome),
                    "Intruder 终态 run 仍引用在途结果"
                );
                ensure!(
                    result.ordinal == expected && expected < run.dispatched,
                    "Intruder 结果序号缺失或与计数不符"
                );
                expected += 1;
                bytes += size;
                output.push(result);
            }
        }
        ensure!(more || expected >= run.dispatched, "Intruder 结果记录缺失");
        let next_after = more.then(|| output.last().expect("nonempty bounded page").ordinal);
        transaction.commit()?;
        Ok(IntruderResultPage {
            run,
            rows: output,
            next_after,
            persisted,
        })
    }
}
