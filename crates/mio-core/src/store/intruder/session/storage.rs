use super::*;

impl ProjectStore {
    /// Reserves the request plus the maximum legal response before any socket is created.
    /// False means outstanding reservations may free enough space; the caller must wait for a completion.
    pub fn preflight_intruder_result(
        &mut self,
        id: Uuid,
        ordinal: u64,
        record: &FlowRecord,
    ) -> Result<bool> {
        let Some(dataset) = self.intruder_sessions.runs.get(&id) else {
            return Ok(true);
        };
        require_session_mutation(dataset, Some(ordinal))?;
        if dataset.disk_ticket.is_some() {
            let ticket = self.connection.unchecked_transaction()?;
            validate_ticket(&ticket, self.id, dataset)?;
        }
        validate_record(record)?;
        ensure!(
            record.summary.state == FlowState::Pending && record.response.is_none(),
            "Intruder 预留必须对应未发送请求"
        );
        ensure!(
            dataset.run.state == IntruderRunState::Running && ordinal == dataset.run.dispatched,
            "Intruder 预留序号或状态无效"
        );
        if let Some(reservation) = dataset.reservations.get(&ordinal) {
            ensure!(
                reservation.flow == record.summary.id,
                "Intruder 预留请求身份已改变"
            );
            return Ok(true);
        }
        ensure!(
            !has_flow(&self.connection, record.summary.id)?,
            "Intruder Session flow ID 与磁盘冲突"
        );
        for other in self.intruder_sessions.runs.values() {
            ensure!(
                !has_flow(&other.connection, record.summary.id)?,
                "Intruder Session flow ID 已存在"
            );
        }
        let summary = serde_json::to_vec(&record.summary)?.len();
        let head = serde_json::to_vec(&store::RequestHead::from(&record.request))?.len();
        let bytes = (summary
            + head
            + record.request.body.len()
            + record.summary.url.len()
            + record.summary.method.len()
            + 1024) as u64
            + COMPLETION_RESERVE;
        let used = self.intruder_sessions.logical_bytes + self.intruder_sessions.reserved_bytes;
        if bytes > self.intruder_sessions.quota.saturating_sub(used) {
            let releasable = self
                .intruder_sessions
                .runs
                .values()
                .flat_map(|dataset| dataset.reservations.values())
                .map(|reservation| reservation.bytes)
                .sum::<u64>();
            ensure!(
                releasable > 0
                    && bytes
                        <= self
                            .intruder_sessions
                            .quota
                            .saturating_sub(used - releasable),
                "Intruder Session 容量不足以保留完整响应；未发送请求，请保存或丢弃已有攻击"
            );
            return Ok(false);
        }
        self.intruder_sessions.reserved_bytes += bytes;
        self.intruder_sessions
            .runs
            .get_mut(&id)
            .unwrap()
            .reservations
            .insert(
                ordinal,
                Reservation {
                    bytes,
                    flow: record.summary.id,
                },
            );
        Ok(true)
    }

    pub(in crate::store) fn write_session_intruder_result_with_reason_phrase(
        &mut self,
        id: Uuid,
        ordinal: u64,
        result: std::result::Result<&FlowRecord, &str>,
        reason_phrase: Option<&[u8]>,
    ) -> Result<IntruderRunSummary> {
        let terminal = result
            .as_ref()
            .map_or(true, |record| is_flow_terminal(&record.summary.state));
        let flow = result.as_ref().ok().map(|record| record.summary.id);
        let (_, run) =
            self.mutate_session_intruder(id, Some((ordinal, terminal, flow)), |transaction| {
                let run = write_result_with_reason_phrase(
                    transaction,
                    id,
                    ordinal,
                    result,
                    reason_phrase,
                )?;
                Ok(((), run))
            })?;
        Ok(run)
    }

    pub(in crate::store) fn write_session_intruder_state(
        &mut self,
        id: Uuid,
        state: IntruderRunState,
    ) -> Result<IntruderRunSummary> {
        let (_, run) = self.mutate_session_intruder(id, None, |transaction| {
            let run = write_state(transaction, id, state)?;
            Ok(((), run))
        })?;
        Ok(run)
    }

    pub(in crate::store) fn write_session_intruder_result_notes(
        &mut self,
        id: Uuid,
        ordinal: u64,
        expected_notes_revision: u64,
        notes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(u64, Vec<u8>)> {
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果备注修改已取消；数据保留"
        );
        ensure!(
            notes.len() <= MAX_ERROR_BYTES,
            "Intruder 结果备注超过 64 KiB 字节限额"
        );
        let (saved, _run) = self.mutate_session_intruder(id, None, |transaction| {
            let saved = write_result_notes(
                transaction,
                id,
                ordinal,
                expected_notes_revision,
                notes,
                cancel,
            )?;
            let run = read_run(transaction, id)?;
            Ok((saved, run))
        })?;
        Ok(saved)
    }

    pub(in crate::store) fn write_session_intruder_result_highlight(
        &mut self,
        id: Uuid,
        entries: &[(u64, u64)],
        highlight: Option<u8>,
        cancel: &CancellationToken,
    ) -> Result<Vec<(u64, u64)>> {
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果高亮修改已取消；数据保留"
        );
        let (saved, _run) = self.mutate_session_intruder(id, None, |transaction| {
            let saved = write_result_highlights(transaction, id, entries, highlight, cancel)?;
            let run = read_run(transaction, id)?;
            Ok((saved, run))
        })?;
        Ok(saved)
    }

    fn mutate_session_intruder<T>(
        &mut self,
        id: Uuid,
        result: Option<(u64, bool, Option<FlowId>)>,
        change: impl FnOnce(&Transaction<'_>) -> Result<(T, IntruderRunSummary)>,
    ) -> Result<(T, IntruderRunSummary)> {
        let dataset = self
            .intruder_sessions
            .runs
            .get_mut(&id)
            .context("Intruder Session 不存在")?;
        require_session_mutation(dataset, result.map(|(ordinal, _, _)| ordinal))?;
        let ticket = if dataset.disk_ticket.is_some() {
            let ticket = self.connection.unchecked_transaction()?;
            validate_ticket(&ticket, self.id, dataset)?;
            Some(ticket)
        } else {
            None
        };
        let credit = result
            .and_then(|(ordinal, _, _)| dataset.reservations.get(&ordinal))
            .map_or(0, |reservation| reservation.bytes);
        if let Some((ordinal, _, Some(flow))) = result {
            let existing = read_result(&dataset.connection, id, ordinal)?;
            ensure!(dataset.reservations.get(&ordinal).is_some_and(|reservation| reservation.flow == flow)
                || existing.as_ref().is_some_and(|row| matches!(&row.outcome, IntruderResultOutcome::Flow(summary) if summary.id == flow && is_flow_terminal(&summary.state))), "Intruder Session 请求缺少完整结果预留");
        }
        let before = mutable_bytes(&dataset.connection, id, result.map(|value| value.0))?;
        ensure!(
            dataset
                .connection
                .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))?,
            "Intruder RAM 只读保护已改变"
        );
        dataset
            .connection
            .pragma_update(None, "query_only", false)?;
        let operation = (|| -> Result<(T, IntruderRunSummary)> {
            let transaction = dataset.connection.unchecked_transaction()?;
            let (saved, run) = change(&transaction)?;
            let after = mutable_bytes(&transaction, id, result.map(|value| value.0))?;
            let growth = after.saturating_sub(before);
            let available = self.intruder_sessions.quota.saturating_sub(
                self.intruder_sessions.logical_bytes + self.intruder_sessions.reserved_bytes,
            ) + credit
                + if result.is_none() {
                    dataset.state_reserve
                } else {
                    0
                };
            ensure!(
                growth <= available,
                "Intruder Session 配额不足；原记录保留，未写入磁盘"
            );
            if run.state.is_terminal() {
                ensure!(dataset.reservations.is_empty(), "Intruder 终态仍有结果预留");
            }
            transaction.pragma_update(None, "query_only", true)?;
            transaction.commit()?;
            Ok((saved, run))
        })();
        if operation.is_err() {
            dataset.connection.pragma_update(None, "query_only", true)?;
        }
        let (saved, mut run) = operation?;
        let after = mutable_bytes(&dataset.connection, id, result.map(|value| value.0))?;
        let growth = after.saturating_sub(before);
        let consumed_credit = credit.min(growth);
        if let Some((ordinal, terminal, _)) = result {
            if terminal {
                if let Some(reservation) = dataset.reservations.remove(&ordinal) {
                    self.intruder_sessions.reserved_bytes -= reservation.bytes;
                }
            } else if let Some(reservation) = dataset.reservations.get_mut(&ordinal) {
                reservation.bytes -= consumed_credit;
                self.intruder_sessions.reserved_bytes -= consumed_credit;
            }
        }
        let consumed_state = if result.is_none() {
            dataset.state_reserve.min(growth)
        } else {
            0
        };
        dataset.state_reserve -= consumed_state;
        self.intruder_sessions.reserved_bytes -= consumed_state;
        if run.state.is_terminal() {
            self.intruder_sessions.reserved_bytes -= dataset.state_reserve;
            dataset.state_reserve = 0;
        }
        self.intruder_sessions.logical_bytes =
            self.intruder_sessions.logical_bytes - before + after;
        dataset.logical_bytes = dataset.logical_bytes - before + after;
        run.retention = IntruderRetention::Session;
        dataset.run = run.clone();
        drop(ticket);
        Ok((saved, run))
    }

    /// Each batch belongs to one Session dataset and is committed only to its RAM connection.
    pub(crate) fn retry_session_intruder_mutations(
        &mut self,
        mutations: &[IntruderMutation],
        cancel: &CancellationToken,
    ) -> Result<()> {
        let Some(last) = mutations.last() else {
            return Ok(());
        };
        let expected = match last {
            IntruderMutation::Result { run, .. } | IntruderMutation::State { run } => run,
        };
        let id = expected.id;
        let dataset = self
            .intruder_sessions
            .runs
            .get_mut(&id)
            .context("RAM 恢复不能写入 Project 攻击")?;
        require_session_mutation(dataset, None)?;
        let ticket = if dataset.disk_ticket.is_some() {
            let ticket = self.connection.unchecked_transaction()?;
            validate_ticket(&ticket, self.id, dataset)?;
            Some(ticket)
        } else {
            None
        };
        let mut ordinals = BTreeMap::new();
        for mutation in mutations {
            let run = match mutation {
                IntruderMutation::Result {
                    run,
                    ordinal,
                    result,
                } => {
                    require_session_mutation(dataset, Some(*ordinal))?;
                    ordinals.insert(
                        *ordinal,
                        result
                            .as_ref()
                            .map_or(true, |record| is_flow_terminal(&record.summary.state)),
                    );
                    if let Ok(record) = result {
                        let existing = read_result(&dataset.connection, id, *ordinal)?;
                        ensure!(dataset.reservations.get(ordinal).is_some_and(|reservation| reservation.flow == record.summary.id)
                            || existing.as_ref().is_some_and(|row| matches!(&row.outcome, IntruderResultOutcome::Flow(summary) if summary.id == record.summary.id && is_flow_terminal(&summary.state))), "RAM 恢复预留身份不匹配");
                    }
                    run
                }
                IntruderMutation::State { run } => run,
            };
            ensure!(
                run.id == id && run.retention == IntruderRetention::Session,
                "RAM 恢复批次必须仅属于一个Session攻击"
            );
        }
        let bytes = |connection: &Connection| -> Result<u64> {
            let run_bytes = mutable_bytes(connection, id, None)?;
            ordinals.keys().try_fold(run_bytes, |bytes, ordinal| {
                Ok(bytes + mutable_bytes(connection, id, Some(*ordinal))? - run_bytes)
            })
        };
        let before = bytes(&dataset.connection)?;
        let credit = ordinals
            .keys()
            .filter_map(|ordinal| dataset.reservations.get(ordinal))
            .map(|reservation| reservation.bytes)
            .sum::<u64>();
        ensure!(
            dataset
                .connection
                .pragma_query_value::<bool, _>(None, "query_only", |row| row.get(0))?,
            "Intruder RAM 只读保护已改变"
        );
        dataset
            .connection
            .pragma_update(None, "query_only", false)?;
        let applied = (|| -> Result<u64> {
            let transaction = dataset.connection.unchecked_transaction()?;
            ensure!(!cancel.is_cancelled(), "Intruder RAM 恢复已取消");
            write_recovery_for_retention(&transaction, mutations, IntruderRetention::Session)?;
            let after = bytes(&transaction)?;
            let available = self.intruder_sessions.quota.saturating_sub(
                self.intruder_sessions.logical_bytes + self.intruder_sessions.reserved_bytes,
            ) + credit
                + dataset.state_reserve;
            ensure!(
                after.saturating_sub(before) <= available,
                "Intruder RAM 恢复容量不足；数据仍保留，未写盘"
            );
            if expected.state.is_terminal() {
                ensure!(
                    dataset
                        .reservations
                        .keys()
                        .all(|ordinal| ordinals.get(ordinal) == Some(&true)),
                    "Intruder 终态恢复仍有未确认请求"
                );
            }
            ensure!(!cancel.is_cancelled(), "Intruder RAM 恢复已取消");
            transaction.pragma_update(None, "query_only", true)?;
            transaction.commit()?;
            Ok(after)
        })();
        if applied.is_err() {
            dataset.connection.pragma_update(None, "query_only", true)?;
        }
        let after = applied?;
        let mut growth = after.saturating_sub(before);
        for (ordinal, terminal) in ordinals {
            if terminal {
                if let Some(reservation) = dataset.reservations.remove(&ordinal) {
                    self.intruder_sessions.reserved_bytes -= reservation.bytes;
                    growth = growth.saturating_sub(reservation.bytes);
                }
            } else if let Some(reservation) = dataset.reservations.get_mut(&ordinal) {
                let consumed = growth.min(reservation.bytes);
                reservation.bytes -= consumed;
                self.intruder_sessions.reserved_bytes -= consumed;
                growth -= consumed;
            }
        }
        let consumed_state = growth.min(dataset.state_reserve);
        dataset.state_reserve -= consumed_state;
        self.intruder_sessions.reserved_bytes -= consumed_state;
        if expected.state.is_terminal() {
            self.intruder_sessions.reserved_bytes -= dataset.state_reserve;
            dataset.state_reserve = 0;
        }
        self.intruder_sessions.logical_bytes =
            self.intruder_sessions.logical_bytes - before + after;
        dataset.logical_bytes = dataset.logical_bytes - before + after;
        dataset.run = expected.clone();
        drop(ticket);
        Ok(())
    }
}

fn require_session_mutation(dataset: &SessionDataset, ordinal: Option<u64>) -> Result<()> {
    if dataset.disk_ticket.is_some() {
        let slot = dataset
            .reactivated_baseline
            .context("终态 Project 转入的 Session 快照未显式追加基准请求，不能运行写入")?;
        ensure!(
            dataset
                .run
                .baseline
                .is_some_and(|baseline| baseline.ordinal == slot)
                && ordinal.is_none_or(|ordinal| ordinal == slot),
            "Project 转入的 Session 只允许写入显式追加的基准请求；原结果保留"
        );
    }
    Ok(())
}
