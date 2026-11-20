use super::*;

pub(super) fn check_query(
    deadline: Instant,
    cancel: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<()> {
    ensure!(
        !cancel.is_cancelled() && !shutdown.is_cancelled(),
        "Intruder 结果查询已取消"
    );
    ensure!(Instant::now() < deadline, "Intruder 结果查询超过 1 秒限额");
    Ok(())
}

pub(super) struct QueryScope<'a> {
    connection: &'a Connection,
    busy: Option<Duration>,
    restore: Option<CancellationToken>,
}

impl<'a> QueryScope<'a> {
    pub(super) fn new(
        connection: &'a Connection,
        deadline: Instant,
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
        restore: Option<CancellationToken>,
    ) -> Result<Self> {
        let cancel = cancel.clone();
        let shutdown = shutdown.clone();
        connection.progress_handler(
            1000,
            Some(move || {
                cancel.is_cancelled() || shutdown.is_cancelled() || Instant::now() >= deadline
            }),
        );
        let mut scope = Self {
            connection,
            busy: None,
            restore,
        };
        let millis =
            connection.pragma_query_value(None, "busy_timeout", |row| row.get::<_, u64>(0))?;
        let busy = Duration::from_millis(millis);
        scope.busy = Some(busy);
        connection.busy_timeout(busy.min(Duration::from_millis(250)))?;
        Ok(scope)
    }
}

impl Drop for QueryScope<'_> {
    fn drop(&mut self) {
        // Service owns the persistent shutdown handler; Session connections have no handler.
        if let Some(shutdown) = self.restore.take() {
            self.connection
                .progress_handler(1000, Some(move || shutdown.is_cancelled()));
        } else {
            self.connection.progress_handler(0, None::<fn() -> bool>);
        }
        if let Some(busy) = self.busy {
            let _ = self.connection.busy_timeout(busy);
        }
    }
}

pub(super) fn validate_run_overlay(
    stored: &IntruderRunSummary,
    run: &IntruderRunSummary,
) -> Result<()> {
    ensure!(
        run.id == stored.id
            && run.tab_id == stored.tab_id
            && run.spec_revision == stored.spec_revision
            && run.total == stored.total
            && run.started_unix_ms == stored.started_unix_ms
            && run.retention == stored.retention
            && run.title == stored.title
            && run.title_revision == stored.title_revision
            && run.resource_pool == stored.resource_pool
            && run.resource_pool_revision == stored.resource_pool_revision
            && run.baseline.map(|baseline| baseline.ordinal)
                == stored.baseline.map(|baseline| baseline.ordinal),
        "Intruder 恢复运行与原运行身份或配置不一致"
    );
    ensure!(
        run.revision >= stored.revision
            && run.revision <= i64::MAX as u64
            && run.dispatched >= stored.dispatched
            && run.completed >= stored.completed
            && run.payload_progress().is_some()
            && (!run.state.is_terminal() || run.completed == run.dispatched)
            && (run.state != IntruderRunState::Completed || run.dispatched == run.total)
            && (!stored.state.is_terminal() || run == stored)
            && (run.revision > stored.revision || run == stored),
        "Intruder 恢复运行 revision、状态或计数无效"
    );
    if let IntruderRunState::Failed(error) = &run.state {
        ensure!(
            error.len() <= MAX_ERROR_BYTES,
            "Intruder 恢复错误文本超过限额"
        );
    }
    Ok(())
}

pub(super) fn validate_recovery<'a>(
    connection: &Connection,
    run: &IntruderRunSummary,
    recovery: &'a [IntruderResultSummary],
    check: &dyn Fn() -> Result<()>,
) -> Result<BTreeMap<u64, &'a IntruderResultSummary>> {
    ensure!(
        recovery.len() <= MAX_REQUESTS as usize,
        "Intruder 恢复结果数量超过限额"
    );
    let mut by_ordinal = BTreeMap::new();
    let mut flows = BTreeSet::new();
    let mut query = connection.prepare(
        "SELECT r.run,r.ordinal FROM flows f LEFT JOIN intruder_results r ON r.flow=f.id WHERE f.id=?1",
    )?;
    for result in recovery {
        check()?;
        ensure!(
            result.ordinal < run.dispatched,
            "Intruder 恢复结果序号超出已派发范围"
        );
        ensure!(
            by_ordinal.insert(result.ordinal, result).is_none(),
            "Intruder 恢复结果序号重复"
        );
        match &result.outcome {
            IntruderResultOutcome::Rejected(error) => ensure!(
                error.len() <= MAX_ERROR_BYTES,
                "Intruder 恢复拒绝原因超过限额"
            ),
            IntruderResultOutcome::Flow(summary) => {
                validate_summary(summary)?;
                ensure!(
                    !summary.id.is_nil() && flows.insert(summary.id),
                    "Intruder 恢复 flow 身份重复或无效"
                );
                ensure!(
                    serde_json::to_vec(summary)?.len() <= MAX_SUMMARY_BYTES,
                    "Intruder 恢复摘要超过读取限额"
                );
                let owner: Option<(Option<String>, Option<u64>)> = query
                    .query_row([summary.id.to_string()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .optional()?;
                ensure!(
                    owner.is_none_or(|(id, ordinal)| id.as_deref() == Some(&run.id.to_string())
                        && ordinal == Some(result.ordinal)),
                    "Intruder 恢复 flow 归属冲突"
                );
            }
        }
    }
    Ok(by_ordinal)
}

pub(super) fn validate_recovery_responses(
    recovery: &BTreeMap<u64, &IntruderResultSummary>,
    recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    for (&ordinal, &response) in recovery_responses {
        check()?;
        let Some(result) = recovery.get(&ordinal) else {
            bail!("Intruder 恢复响应缺少对应的恢复结果摘要");
        };
        let IntruderResultOutcome::Flow(summary) = &result.outcome else {
            bail!("Intruder 拒绝结果不能附带恢复响应");
        };
        ensure!(
            matches!(summary.state, FlowState::Complete)
                && summary.status == Some(response.status)
                && summary.response_bytes == response.body.len() as u64
                && response.body.len() <= MAX_BODY
                && serde_json::to_vec(&crate::store::ResponseHead::from(response))?.len()
                    <= MAX_SEARCH_HEAD_BYTES,
            "Intruder 恢复响应与结果摘要不一致或超过读取限额"
        );
    }
    Ok(())
}

pub(super) fn validate_results(
    connection: &Connection,
    stored: &IntruderRunSummary,
    run: &IntruderRunSummary,
    recovery: &BTreeMap<u64, &IntruderResultSummary>,
    validate_session: ValidateSession<'_>,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let mut statement = connection.prepare(&format!(
        "{} WHERE r.run=?1 ORDER BY r.ordinal",
        result_select()
    ))?;
    let mut rows = statement.query([run.id.to_string()])?;
    let mut expected = 0;
    let mut stored_completed = 0;
    let mut completed = 0;
    let mut baseline_completed = false;
    while let Some(row) = rows.next()? {
        check()?;
        let result = decode_result(row)?;
        if let IntruderResultOutcome::Flow(summary) = &result.outcome {
            let _: FlowSummary = retention::strict_json(row.get_ref(10)?.as_blob()?)?;
            ensure!(
                !summary.id.is_nil() && row.get::<_, String>(2)? == summary.id.to_string(),
                "Intruder 原始 flow 身份编码无效"
            );
        }
        ensure!(
            result.ordinal == expected && expected < stored.dispatched,
            "Intruder 原始结果序号缺失或与计数不符"
        );
        ensure!(
            !stored.state.is_terminal() || outcome_terminal(&result.outcome),
            "Intruder 原始终态运行仍引用在途结果"
        );
        stored_completed += u64::from(outcome_terminal(&result.outcome));
        if let Some(validate_session) = validate_session {
            validate_session(&result)?;
        }
        let latest = recovery.get(&expected).copied().unwrap_or(&result);
        if latest != &result {
            ensure!(
                !outcome_terminal(&result.outcome),
                "Intruder 恢复覆盖已完成结果"
            );
            match (&result.outcome, &latest.outcome) {
                (IntruderResultOutcome::Flow(previous), IntruderResultOutcome::Flow(next)) => {
                    ensure!(
                        previous.id == next.id && next.revision > previous.revision,
                        "Intruder 恢复 flow 身份或 revision 不一致"
                    )
                }
                _ => bail!("Intruder 恢复结果类型不一致"),
            }
        }
        count_result(run, latest, &mut completed, &mut baseline_completed)?;
        expected += 1;
    }
    ensure!(
        expected == stored.dispatched && stored_completed == stored.completed,
        "Intruder 原始结果缺失或完成计数不符"
    );
    while expected < run.dispatched {
        check()?;
        let result = recovery
            .get(&expected)
            .context("Intruder 恢复结果序号缺失")?;
        count_result(run, result, &mut completed, &mut baseline_completed)?;
        expected += 1;
    }
    ensure!(
        completed == run.completed
            && run
                .baseline
                .is_none_or(|baseline| baseline.completed == baseline_completed),
        "Intruder 恢复结果完成计数或基准状态不符"
    );
    Ok(())
}

fn count_result(
    run: &IntruderRunSummary,
    result: &IntruderResultSummary,
    completed: &mut u64,
    baseline_completed: &mut bool,
) -> Result<()> {
    let terminal = outcome_terminal(&result.outcome);
    ensure!(
        !run.state.is_terminal() || terminal,
        "Intruder 终态运行仍引用在途结果"
    );
    *completed += u64::from(terminal);
    if run
        .baseline
        .is_some_and(|baseline| baseline.ordinal == result.ordinal)
    {
        *baseline_completed = terminal;
    }
    Ok(())
}
