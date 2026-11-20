use super::*;

/// Replays the whole recovery batch inside the caller's transaction, which must roll back on error.
pub(in crate::store) fn write_recovery(
    transaction: &Transaction<'_>,
    mutations: &[IntruderMutation],
) -> Result<()> {
    write_recovery_for_retention(transaction, mutations, IntruderRetention::Project)
}

pub(in crate::store::intruder) fn write_recovery_for_retention(
    transaction: &Transaction<'_>,
    mutations: &[IntruderMutation],
    retention: IntruderRetention,
) -> Result<()> {
    // Only references are indexed: retaining the batch never copies message bodies.
    let mut final_runs = BTreeMap::new();
    let mut final_results = BTreeMap::new();
    for mutation in mutations {
        let run = match mutation {
            IntruderMutation::Result {
                run,
                ordinal,
                result,
            } => {
                final_results.insert((run.id, *ordinal), result);
                run
            }
            IntruderMutation::State { run } => run,
        };
        ensure!(
            run.retention == retention,
            "Intruder 恢复批次不能改变存储归属"
        );
        final_runs.insert(run.id, run);
    }
    let mut already_applied = BTreeSet::new();
    for (id, expected) in &final_runs {
        let mut actual = read_run(transaction, *id)?;
        actual.retention = retention;
        if actual == **expected {
            already_applied.insert(*id);
        }
    }
    for ((id, ordinal), result) in final_results {
        if !already_applied.contains(&id) {
            continue;
        }
        ensure!(
            ordinal < final_runs[&id].dispatched,
            "Intruder 恢复批次最终结果超过已登记计数"
        );
        let stored =
            read_result(transaction, id, ordinal)?.context("Intruder 恢复批次最终结果缺失")?;
        match (&stored.outcome, result.as_deref().map_err(String::as_str)) {
            (IntruderResultOutcome::Flow(before), Ok(after)) => {
                validate_record(after)?;
                ensure!(
                    before == &after.summary && record_matches(transaction, after)?,
                    "Intruder 恢复批次最终流量与磁盘记录冲突"
                );
            }
            (IntruderResultOutcome::Rejected(before), Err(after)) => ensure!(
                before == after,
                "Intruder 恢复批次最终拒绝原因与磁盘记录冲突"
            ),
            _ => bail!("Intruder 恢复批次最终结果类型与磁盘记录冲突"),
        }
    }
    for id in &already_applied {
        let expected = final_runs[id];
        if expected.state.is_terminal() {
            // Also check joined terminal results when the batch itself is an exact replay.
            let mut actual = write_state(transaction, *id, expected.state.clone())?;
            actual.retention = retention;
            ensure!(actual == *expected, "Intruder 恢复批次最终状态冲突");
        }
    }
    for mutation in mutations {
        let (expected, mut actual) = match mutation {
            IntruderMutation::Result {
                run,
                ordinal,
                result,
            } => {
                if already_applied.contains(&run.id) {
                    continue;
                }
                (
                    run,
                    write_result(
                        transaction,
                        run.id,
                        *ordinal,
                        result.as_deref().map_err(String::as_str),
                    )?,
                )
            }
            IntruderMutation::State { run } => {
                if already_applied.contains(&run.id) {
                    continue;
                }
                (run, write_state(transaction, run.id, run.state.clone())?)
            }
        };
        actual.retention = retention;
        ensure!(
            actual == *expected,
            "Intruder 恢复步骤的完整摘要或 revision 与预期不符"
        );
    }
    Ok(())
}

pub(in crate::store::intruder) fn request_baseline<T>(
    transaction: &Transaction<'_>,
    id: Uuid,
    expected_revision: u64,
    cancel: &CancellationToken,
    prepare: impl FnOnce(IntruderSpec) -> Result<T>,
) -> Result<(IntruderRunSummary, T)> {
    ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
    let run = read_run(transaction, id)?;
    ensure!(
        run.revision == expected_revision,
        "Intruder run revision 已改变；未追加基准请求"
    );
    ensure!(
        run.baseline.is_none(),
        "Intruder 已有基准请求；不会再次发送"
    );
    ensure!(
        matches!(
            run.state,
            IntruderRunState::Running | IntruderRunState::Paused | IntruderRunState::Completed
        ),
        "Intruder 状态不允许追加基准请求；不会恢复原攻击"
    );
    let total = run
        .total
        .checked_add(1)
        .filter(|total| *total <= MAX_REQUESTS)
        .context("Intruder 基准请求超过总请求限额；未追加")?;
    let revision = next_revision(run.revision)?;
    let metadata: Vec<u8> = transaction
        .query_row(
            "SELECT CASE WHEN typeof(spec_meta)='blob' AND length(spec_meta)<=?2 THEN spec_meta END
         FROM intruder_runs WHERE id=?1",
            params![id.to_string(), MAX_SPEC_META],
            |row| row.get(0),
        )
        .context("Intruder 配置元数据类型或长度无效；未追加基准请求")?;
    let _: SpecMeta = retention::strict_json(&metadata)?;
    super::super::schema_guard::validate_update_dependencies(
        transaction,
        "intruder_runs",
        &[
            "baseline_ordinal",
            "total",
            "revision",
            "state",
            "error",
            "paused",
        ],
        cancel,
    )?;
    let spec = read_spec(transaction, id)?;
    let mut expected = 0;
    let mut completed = 0;
    let mut query = transaction.prepare(&format!(
        "{} WHERE r.run=?1 ORDER BY r.ordinal",
        result_select()
    ))?;
    let mut rows = query.query([id.to_string()])?;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
        let result = decode_result(row)?;
        ensure!(
            result.ordinal == expected && expected < run.dispatched,
            "Intruder 结果序号或计数损坏；未追加基准请求"
        );
        completed += u64::from(outcome_terminal(&result.outcome));
        expected += 1;
    }
    ensure!(
        expected == run.dispatched && completed == run.completed,
        "Intruder 结果数量与计数不符；未追加基准请求"
    );
    drop(rows);
    drop(query);
    let prepared = prepare(spec)?;
    ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
    let changed = transaction.execute(
        "UPDATE intruder_runs SET baseline_ordinal=?1,total=?2,revision=?3,state=0,error=NULL,paused=?4
         WHERE id=?5 AND revision=?6 AND baseline_ordinal IS NULL",
        params![run.dispatched, total, revision, run.state == IntruderRunState::Paused, id.to_string(), expected_revision],
    )?;
    ensure!(changed == 1, "Intruder 基准请求准入冲突；未追加");
    ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
    Ok((read_run(transaction, id)?, prepared))
}

pub(in crate::store::intruder) fn validate_pool_binding(
    connection: &Connection,
    run: &IntruderRunSummary,
) -> Result<()> {
    let catalog = super::super::resource_pools::read(connection)?;
    ensure!(
        run.resource_pool
            .is_none_or(|id| catalog.find(id).is_some()),
        "Intruder 运行绑定的资源池不存在；不会使用默认池"
    );
    Ok(())
}

pub(in crate::store::intruder) fn bind_resource_pool(
    transaction: &Transaction<'_>,
    id: Uuid,
    expected: u64,
    pool_id: Uuid,
    cancel: &CancellationToken,
) -> Result<IntruderRunSummary> {
    ensure!(
        !pool_id.is_nil() && !cancel.is_cancelled(),
        "Intruder 资源池绑定无效或已取消"
    );
    let mut run = read_run(transaction, id)?;
    ensure!(
        run.resource_pool_revision == expected,
        "Intruder 资源池绑定 revision 已改变；未覆盖"
    );
    ensure!(
        run.resource_pool != Some(pool_id),
        "Intruder 已绑定此资源池；未重复修改"
    );
    let revision = next_revision(expected)?;
    super::super::schema_guard::validate_update_dependencies(
        transaction,
        "intruder_runs",
        &["pool_id", "pool_revision"],
        cancel,
    )?;
    let changed = transaction.execute(
        "UPDATE intruder_runs SET pool_id=?1,pool_revision=?2 WHERE id=?3 AND pool_revision=?4",
        params![pool_id.to_string(), revision, id.to_string(), expected],
    )?;
    ensure!(changed == 1, "Intruder 资源池绑定冲突");
    ensure!(!cancel.is_cancelled(), "Intruder 资源池绑定已取消");
    run.resource_pool = Some(pool_id);
    run.resource_pool_revision = revision;
    Ok(run)
}

pub(in crate::store::intruder) fn ensure_no_pool_references(
    connection: &Connection,
    id: Option<Uuid>,
    removed: &BTreeSet<Uuid>,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut query = connection.prepare("SELECT CASE WHEN pool_id IS NULL THEN '' WHEN typeof(pool_id)='text' AND length(CAST(pool_id AS BLOB))=36 THEN pool_id END,
        CASE WHEN typeof(spec_meta)='blob' AND length(spec_meta)<=?1 THEN spec_meta END FROM intruder_runs WHERE ?2 IS NULL OR id=?2")?;
    let mut rows = query.query(params![MAX_SPEC_META, id.map(|id| id.to_string())])?;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
        let current: String = row
            .get::<_, Option<String>>(0)?
            .context("Intruder 资源池引用类型无效；未删除")?;
        if !current.is_empty() {
            let pool = Uuid::parse_str(&current)?;
            ensure!(
                pool.to_string() == current && !pool.is_nil(),
                "Intruder 资源池引用编码无效；未删除"
            );
            ensure!(
                !removed.contains(&pool),
                "Intruder 资源池仍被运行引用；未删除"
            );
        }
        let meta: SpecMeta = retention::strict_json(
            row.get_ref(1)?
                .as_blob()
                .context("Intruder 冻结配置类型或长度无效；未删除资源池")?,
        )?;
        ensure!(
            meta.resource_pool
                .as_ref()
                .is_none_or(|selection| !removed.contains(&selection.id())),
            "Intruder 资源池仍被冻结配置引用；未删除"
        );
    }
    Ok(())
}
