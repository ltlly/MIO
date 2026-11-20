use super::*;

/// Shares the caller's save/recovery transaction; the caller must roll it back on any error.
pub(in crate::store) fn write_result(
    transaction: &Transaction<'_>,
    id: Uuid,
    ordinal: u64,
    result: std::result::Result<&FlowRecord, &str>,
) -> Result<IntruderRunSummary> {
    write_result_with_reason_phrase(transaction, id, ordinal, result, None)
}

/// Shares the caller's save/recovery transaction; the caller must roll it back on any error.
pub(in crate::store) fn write_result_with_reason_phrase(
    transaction: &Transaction<'_>,
    id: Uuid,
    ordinal: u64,
    result: std::result::Result<&FlowRecord, &str>,
    reason_phrase: Option<&[u8]>,
) -> Result<IntruderRunSummary> {
    let mut run = read_run(transaction, id)?;
    ensure!(ordinal < run.total, "Intruder 结果序号超过冻结计划的数量");
    let previous = read_result(transaction, id, ordinal)?;
    ensure!(
        previous.is_none() || ordinal < run.dispatched,
        "Intruder 已有结果超过已派发计数"
    );
    let terminal = match result {
        Ok(record) => {
            validate_record_with_reason_phrase(record, reason_phrase)?;
            is_flow_terminal(&record.summary.state)
        }
        Err(message) => {
            ensure!(
                reason_phrase.is_none(),
                "Intruder 拒绝结果不能附带响应 reason phrase"
            );
            ensure!(
                message.len() <= MAX_ERROR_BYTES,
                "Intruder 拒绝原因超过 64 KiB"
            );
            true
        }
    };
    if let Some(previous) = &previous {
        ensure!(
            !run.state.is_terminal() || outcome_terminal(&previous.outcome),
            "Intruder 终态 run 仍引用在途结果"
        );
        match (&previous.outcome, result) {
            (IntruderResultOutcome::Rejected(before), Err(after)) if before == after => {
                return Ok(run);
            }
            (IntruderResultOutcome::Flow(before), Ok(after)) => {
                ensure!(
                    before.id == after.summary.id,
                    "Intruder 序号不能改为另一 flow ID"
                );
                if before == &after.summary
                    && record_matches_with_reason_phrase(transaction, after, reason_phrase)?
                {
                    return Ok(run);
                }
                ensure!(
                    !run.state.is_terminal() && !is_flow_terminal(&before.state),
                    "Intruder 终态结果不能被覆盖"
                );
                ensure!(
                    terminal && after.summary.revision == next_revision(before.revision)?,
                    "Intruder 仅允许在途结果进入下一 revision 的终态"
                );
                ensure!(
                    before.started_unix_ms == after.summary.started_unix_ms
                        && before.method == after.summary.method
                        && before.url == after.summary.url
                        && before.request_bytes == after.summary.request_bytes
                        && request_matches(transaction, after)?
                        && response_absent(transaction, before.id)?,
                    "Intruder 在途请求不能被替换"
                );
            }
            _ => bail!("Intruder 结果类型或拒绝原因冲突；未覆盖已有结果"),
        }
    } else {
        ensure!(
            run.state == IntruderRunState::Running,
            "Intruder 已停止派发新请求"
        );
        ensure!(
            ordinal == run.dispatched,
            "Intruder 新结果必须按连续序号登记"
        );
        if let Ok(record) = result {
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM flows WHERE id=?1)",
                [record.summary.id.to_string()],
                |row| row.get(0),
            )?;
            ensure!(!exists, "Intruder 新序号不能使用已经存在的 flow ID");
        }
        run.dispatched += 1;
    }
    match result {
        Ok(record) => {
            super::super::messages::write_record_with_reason_phrase(
                transaction,
                record,
                reason_phrase,
            )?;
            if previous.is_none() {
                transaction.execute(
                    "INSERT INTO intruder_results(run,ordinal,flow,error) VALUES(?1,?2,?3,NULL)",
                    params![id.to_string(), ordinal, record.summary.id.to_string()],
                )?;
            }
        }
        Err(message) => {
            transaction.execute(
                "INSERT INTO intruder_results(run,ordinal,flow,error) VALUES(?1,?2,NULL,?3)",
                params![id.to_string(), ordinal, message],
            )?;
        }
    }
    if terminal {
        run.completed += 1;
        if let Some(baseline) = &mut run.baseline
            && baseline.ordinal == ordinal
        {
            baseline.completed = true;
        }
    }
    update_run(transaction, &mut run)?;
    Ok(run)
}

/// Shares the caller's save/recovery transaction; the caller must roll it back on any error.
pub(in crate::store) fn write_state(
    transaction: &Transaction<'_>,
    id: Uuid,
    state: IntruderRunState,
) -> Result<IntruderRunSummary> {
    let mut run = read_run(transaction, id)?;
    if state.is_terminal() {
        ensure!(
            run.completed == run.dispatched,
            "Intruder 尚有在途结果，不能确认停止"
        );
        let mut query = transaction.prepare(&format!(
            "{} WHERE r.run=?1 ORDER BY r.ordinal",
            result_select()
        ))?;
        let mut rows = query.query([id.to_string()])?;
        let mut expected = 0;
        while let Some(row) = rows.next()? {
            let result = decode_result(row)?;
            ensure!(
                result.ordinal == expected && expected < run.dispatched,
                "Intruder 终态结果序号缺失或与计数不符"
            );
            ensure!(
                outcome_terminal(&result.outcome),
                "Intruder 尚有在途结果，不能确认停止"
            );
            expected += 1;
        }
        ensure!(expected == run.dispatched, "Intruder 终态结果记录缺失");
    }
    if run.state == state {
        return Ok(run);
    }
    ensure!(
        !run.state.is_terminal(),
        "Intruder 终态不能重新开始或被覆盖"
    );
    match &state {
        IntruderRunState::Running => ensure!(
            run.state == IntruderRunState::Paused && run.completed < run.total,
            "Intruder 只能恢复尚未结束的暂停攻击，不能重发已派发请求"
        ),
        IntruderRunState::Paused => ensure!(
            run.state == IntruderRunState::Running && run.completed < run.total,
            "Intruder 只能暂停正在运行的攻击"
        ),
        IntruderRunState::Interrupted => {
            bail!("Intruder 不能跳过中断恢复")
        }
        IntruderRunState::Stopping => (),
        IntruderRunState::Completed => ensure!(
            run.completed == run.dispatched && run.dispatched == run.total,
            "Intruder 尚未收齐所有请求终态"
        ),
        IntruderRunState::Stopped | IntruderRunState::Failed(_) => ensure!(
            run.completed == run.dispatched,
            "Intruder 尚有在途结果，不能确认停止"
        ),
    }
    if let IntruderRunState::Failed(message) = &state {
        ensure!(message.len() <= MAX_ERROR_BYTES, "Intruder 错误超过 64 KiB");
    }
    run.state = state;
    update_run(transaction, &mut run)?;
    Ok(run)
}

pub(in crate::store::intruder) fn next_revision(revision: u64) -> Result<u64> {
    revision
        .checked_add(1)
        .filter(|next| *next <= i64::MAX as u64)
        .context("Intruder revision 已耗尽")
}

pub(in crate::store::intruder) fn rename_run(
    transaction: &Transaction<'_>,
    id: Uuid,
    expected_title_revision: u64,
    title: &str,
    cancel: &CancellationToken,
) -> Result<IntruderRunSummary> {
    ensure!(
        title.len() <= MAX_TITLE_BYTES,
        "Intruder 任务名称超过 4096 字节；名称未改变"
    );
    let mut run = read_run(transaction, id)?;
    ensure!(
        run.title_revision == expected_title_revision,
        "Intruder 任务名称 revision 已改变；名称未覆盖"
    );
    let revision = next_revision(expected_title_revision)?;
    ensure!(!cancel.is_cancelled(), "Intruder 重命名已取消；名称未改变");
    let changed = transaction.execute(
        "UPDATE intruder_runs SET title=?1,title_revision=?2 WHERE id=?3 AND title_revision=?4",
        params![title, revision, id.to_string(), expected_title_revision],
    )?;
    ensure!(changed == 1, "Intruder 任务名称 revision 冲突；名称未覆盖");
    run.title = title.to_owned();
    run.title_revision = revision;
    ensure!(!cancel.is_cancelled(), "Intruder 重命名已取消；名称未改变");
    Ok(run)
}

pub(in crate::store::intruder) fn state_parts(state: &IntruderRunState) -> (i64, Option<&str>) {
    match state {
        IntruderRunState::Running | IntruderRunState::Paused => (0, None),
        IntruderRunState::Stopping => (1, None),
        IntruderRunState::Stopped => (2, None),
        IntruderRunState::Completed => (3, None),
        IntruderRunState::Interrupted => (4, None),
        IntruderRunState::Failed(message) => (5, Some(message)),
    }
}

pub(in crate::store::intruder) fn update_run(
    transaction: &Transaction<'_>,
    run: &mut IntruderRunSummary,
) -> Result<()> {
    let previous = run.revision;
    run.revision = next_revision(previous)?;
    ensure!(
        run.completed <= run.dispatched && run.dispatched <= run.total,
        "Intruder 计数不一致"
    );
    let (state, error) = state_parts(&run.state);
    let changed = transaction.execute(
        "UPDATE intruder_runs SET revision=?1,dispatched=?2,completed=?3,state=?4,error=?5,paused=?8 WHERE id=?6 AND revision=?7",
        params![run.revision, run.dispatched, run.completed, state, error, run.id.to_string(), previous, run.state == IntruderRunState::Paused],
    )?;
    ensure!(changed == 1, "Intruder run revision 冲突");
    Ok(())
}
