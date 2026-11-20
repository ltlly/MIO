use super::*;

pub(in crate::store) fn run_select() -> String {
    format!(
        "SELECT CASE WHEN typeof(id)='text' AND length(CAST(id AS BLOB))=36 THEN id END,
            CASE WHEN typeof(tab)='text' AND length(CAST(tab AS BLOB))=36 THEN tab END,
            revision,spec_revision,total,dispatched,completed,state,typeof(error),
            CASE WHEN typeof(error)='text' AND length(CAST(error AS BLOB))<={MAX_ERROR_BYTES} THEN error END,started,paused
            ,CASE WHEN typeof(title)='text' AND length(CAST(title AS BLOB))<={MAX_TITLE_BYTES} THEN title END,
            title_revision,baseline_ordinal,
            EXISTS(SELECT 1 FROM intruder_results r LEFT JOIN flows f ON f.id=r.flow
                WHERE r.run=intruder_runs.id AND r.ordinal=intruder_runs.baseline_ordinal
                  AND ((r.flow IS NULL AND typeof(r.error)='text')
                    OR (r.error IS NULL AND f.state IN ('complete','dropped','cancelled','failed')))),
            CASE WHEN pool_id IS NULL THEN '' WHEN typeof(pool_id)='text' AND length(CAST(pool_id AS BLOB))=36 THEN pool_id END,pool_revision
         FROM intruder_runs"
    )
}

pub(in crate::store::intruder) fn run_page_query(has_cursor: bool) -> String {
    let filter = if has_cursor {
        "WHERE (started,id)>(?2,?3)"
    } else {
        ""
    };
    format!("{} {filter} ORDER BY started,id LIMIT ?1", run_select())
}

pub(in crate::store) fn decode_run(row: &Row<'_>) -> Result<IntruderRunSummary> {
    let raw_id: String = row
        .get::<_, Option<String>>(0)?
        .context("Intruder run ID 类型或长度无效")?;
    let id = Uuid::parse_str(&raw_id).context("Intruder run ID 损坏")?;
    ensure!(id.to_string() == raw_id, "Intruder run ID 编码无效");
    let error_type: String = row.get(8)?;
    let error: Option<String> = row.get(9)?;
    let base_state: i64 = row.get(7)?;
    let paused: i64 = row.get(11)?;
    ensure!(
        (paused == 0 || paused == 1) && (paused == 0 || base_state == 0),
        "Intruder 暂停标记与运行状态不一致"
    );
    let state = match base_state {
        0 if paused == 1 => IntruderRunState::Paused,
        0 => IntruderRunState::Running,
        1 => IntruderRunState::Stopping,
        2 => IntruderRunState::Stopped,
        3 => IntruderRunState::Completed,
        4 => IntruderRunState::Interrupted,
        5 => IntruderRunState::Failed(error.context("Intruder run 错误文本类型或长度无效")?),
        _ => bail!("Intruder run 状态无效"),
    };
    ensure!(
        (matches!(state, IntruderRunState::Failed(_)) && error_type == "text")
            || (!matches!(state, IntruderRunState::Failed(_)) && error_type == "null"),
        "Intruder run 状态与错误文本不一致"
    );
    let baseline_completed: bool = row.get(15)?;
    let pool_id: String = row
        .get::<_, Option<String>>(16)?
        .context("Intruder 资源池绑定类型或长度无效")?;
    let resource_pool = if pool_id.is_empty() {
        None
    } else {
        let id = Uuid::parse_str(&pool_id)?;
        ensure!(
            !id.is_nil() && id.to_string() == pool_id,
            "Intruder 资源池绑定编码无效"
        );
        Some(id)
    };
    let run = IntruderRunSummary {
        resource_pool,
        resource_pool_revision: row.get(17)?,
        baseline: row
            .get::<_, Option<u64>>(14)?
            .map(|ordinal| IntruderBaseline {
                ordinal,
                completed: baseline_completed,
            }),
        id,
        tab_id: Uuid::parse_str(
            &row.get::<_, Option<String>>(1)?
                .context("Intruder tab ID 损坏")?,
        )?,
        revision: row.get(2)?,
        spec_revision: row.get(3)?,
        total: row.get(4)?,
        dispatched: row.get(5)?,
        completed: row.get(6)?,
        state,
        started_unix_ms: row.get(10)?,
        retention: IntruderRetention::Project,
        title: row
            .get::<_, Option<String>>(12)?
            .context("Intruder 任务名称类型或长度无效")?,
        title_revision: row.get(13)?,
    };
    ensure!(
        run.revision > 0 && run.spec_revision > 0 && run.title_revision <= i64::MAX as u64,
        "Intruder revision 无效"
    );
    ensure!(
        run.resource_pool_revision <= i64::MAX as u64
            && (run.resource_pool.is_some() == (run.resource_pool_revision > 0)),
        "Intruder 资源池绑定 revision 无效"
    );
    ensure!(
        run.completed <= run.dispatched && run.dispatched <= run.total && run.total <= MAX_REQUESTS,
        "Intruder run 计数无效"
    );
    ensure!(
        !run.state.is_terminal() || run.completed == run.dispatched,
        "Intruder 终态仍有在途记录"
    );
    ensure!(
        run.state != IntruderRunState::Completed || run.dispatched == run.total,
        "Intruder 完成状态与总数不符"
    );
    ensure!(
        run.payload_progress().is_some(),
        "Intruder 基准请求序号或计数无效"
    );
    ensure!(
        run.baseline.is_none_or(|baseline| {
            !run.state.is_terminal() || baseline.ordinal == run.dispatched || baseline.completed
        }),
        "Intruder 终态仍有未完成的基准请求"
    );
    Ok(run)
}

pub(in crate::store::intruder) fn find_run(
    connection: &Connection,
    id: Uuid,
) -> Result<Option<IntruderRunSummary>> {
    let mut query = connection.prepare(&format!("{} WHERE id=?1", run_select()))?;
    let mut rows = query.query([id.to_string()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let run = decode_run(row)?;
    ensure!(rows.next()?.is_none(), "Intruder run ID 重复");
    Ok(Some(run))
}

pub(in crate::store::intruder) fn read_run(
    connection: &Connection,
    id: Uuid,
) -> Result<IntruderRunSummary> {
    ensure_not_discarded(connection, id)?;
    find_run(connection, id)?.context("Intruder run 不存在")
}

pub(in crate::store::intruder) fn read_spec(
    connection: &Connection,
    id: Uuid,
) -> Result<IntruderSpec> {
    read_spec_with_settings(connection, id).map(|(spec, _, _)| spec)
}

pub(in crate::store::intruder) fn read_run_settings_only(
    connection: &Connection,
    id: Uuid,
) -> Result<(
    crate::IntruderAttackSettings,
    Vec<crate::IntruderPayloadSetConfig>,
)> {
    let raw_meta: Vec<u8> = connection
        .query_row(
            "SELECT CASE WHEN typeof(spec_meta)='blob' AND length(spec_meta)<=?2 THEN spec_meta END
             FROM intruder_runs WHERE id=?1",
            params![id.to_string(), MAX_SPEC_META as i64],
            |row| row.get(0),
        )
        .context("Intruder 配置元数据类型或长度无效")?;
    let meta: SpecMeta = serde_json::from_slice(&raw_meta)?;
    Ok((meta.attack_settings, meta.payload_configs))
}

pub(in crate::store::intruder) fn read_spec_with_settings(
    connection: &Connection,
    id: Uuid,
) -> Result<(
    IntruderSpec,
    crate::IntruderAttackSettings,
    Vec<crate::IntruderPayloadSetConfig>,
)> {
    let run = read_run(connection, id)?;
    let mut query = connection.prepare(SPEC_QUERY)?;
    let mut rows = query.query(params![
        id.to_string(),
        MAX_SPEC_META as i64,
        MAX_REQUEST_BYTES as i64
    ])?;
    let row = rows.next()?.context("Intruder 配置不存在")?;
    let meta: SpecMeta = serde_json::from_slice(
        row.get_ref(0)?
            .as_blob()
            .context("Intruder 配置元数据类型或长度无效")?,
    )?;
    ensure!(
        (1..=MAX_PAYLOAD_SETS).contains(&meta.set_count),
        "Intruder payload set 数量无效"
    );
    let raw = row
        .get_ref(1)?
        .as_blob()
        .context("Intruder 模板类型或长度无效")?
        .to_vec();
    ensure!(rows.next()?.is_none(), "Intruder 配置重复");
    let mut payload_sets = vec![Vec::new(); meta.set_count];
    let mut count = 0;
    let mut bytes = 0_usize;
    let mut query = connection.prepare(PAYLOAD_QUERY)?;
    let mut rows = query.query(params![
        id.to_string(),
        i64::try_from(MAX_PAYLOADS + 1)?,
        MAX_PAYLOAD_BYTES as i64
    ])?;
    while let Some(row) = rows.next()? {
        count += 1;
        ensure!(count <= MAX_PAYLOADS, "Intruder payload 总数量超过限额");
        let set: usize = row.get(0)?;
        let ordinal: usize = row.get(1)?;
        ensure!(
            set < payload_sets.len() && ordinal == payload_sets[set].len(),
            "Intruder payload 位置缺失、重复或越界"
        );
        ensure!(
            row.get::<_, String>(2)? == "blob",
            "Intruder payload 不是二进制"
        );
        let length: usize = row.get(3)?;
        bytes = bytes
            .checked_add(length)
            .context("Intruder payload 字节数溢出")?;
        ensure!(
            bytes <= MAX_PAYLOAD_BYTES,
            "Intruder payload 总字节数超过限额"
        );
        let value = row
            .get_ref(4)?
            .as_blob()
            .context("Intruder payload 类型或长度无效")?;
        ensure!(value.len() == length, "Intruder payload 长度不一致");
        payload_sets[set].push(value.to_vec());
    }
    let spec = IntruderSpec {
        resource_pool: meta.resource_pool,
        baseline_request: meta.baseline_request,
        revision: run.spec_revision,
        draft: Draft {
            revision: meta.draft_revision,
            target: meta.target,
            raw,
            version: meta.version,
        },
        attack_type: meta.attack_type,
        positions: meta.positions,
        payload_sets,
        max_concurrent_requests: meta.max_concurrent_requests,
        request_timeout_ms: meta.request_timeout_ms,
        update_host_header: meta.update_host_header,
        save_to_project: meta.save_to_project,
    };
    ensure!(
        spec.validate()?
            .checked_sub(u64::from(spec.baseline_request))
            .and_then(|payloads| payloads.checked_add(u64::from(run.baseline.is_some())))
            == Some(run.total)
            && (!spec.baseline_request
                || run.baseline.is_some_and(|baseline| baseline.ordinal == 0)),
        "Intruder 冻结配置与请求总数不一致"
    );
    ensure!(
        spec.resource_pool.is_none() || run.resource_pool.is_some(),
        "Intruder 冻结资源池配置丢失运行绑定"
    );
    Ok((spec, meta.attack_settings, meta.payload_configs))
}
