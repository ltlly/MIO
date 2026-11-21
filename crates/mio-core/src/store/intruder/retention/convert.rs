use serde::de::DeserializeOwned;

use super::*;

pub(super) const DATA_SELECTORS: &[(&str, &str, &str)] = &[
    (
        "intruder_runs",
        "id,tab,revision,spec_revision,total,dispatched,completed,state,paused,error,started,spec_meta,template,title,title_revision,baseline_ordinal,pool_id,pool_revision",
        "id=?1",
    ),
    (
        "intruder_payloads",
        "run,set_index,ordinal,value",
        "run=?1 ORDER BY set_index,ordinal",
    ),
    (
        "flows",
        "id,revision,started,url,method,state,summary",
        "id IN (SELECT flow FROM main.intruder_results WHERE run=?1) ORDER BY id",
    ),
    (
        "messages",
        "flow,side,head,body",
        "flow IN (SELECT flow FROM main.intruder_results WHERE run=?1) ORDER BY flow,side",
    ),
    (
        "intruder_results",
        "run,ordinal,flow,error,notes,notes_revision,highlight,highlight_revision",
        "run=?1 ORDER BY ordinal",
    ),
];

pub(super) fn validate_workspace_links(
    connection: &Connection,
    sessions: &IntruderSessions,
    workspace: &Workspace,
) -> Result<()> {
    for id in workspace
        .intruder
        .tabs
        .iter()
        .chain(&workspace.intruder.closed)
        .filter_map(|tab| tab.last_run)
    {
        ensure!(
            !sessions.contains(id) || find_run(connection, id)?.is_some(),
            "Session-only Intruder 攻击不能成为已保存的 last_run；请显式保存攻击"
        );
    }
    for flow in workspace
        .tabs
        .iter()
        .chain(&workspace.closed)
        .filter_map(|tab| tab.last_flow)
    {
        for dataset in sessions.runs.values() {
            if dataset.data.has_flow(flow) {
                ensure!(
                    has_flow(connection, flow)?,
                    "Intruder Session-only 流量不能成为已保存的 Repeater 引用；请先显式保存攻击"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn retirement_bytes(run: &IntruderRunSummary) -> [u8; 16] {
    ticket_bytes(run.revision, run.started_unix_ms)
}

pub(super) fn ticket_bytes(revision: u64, started: i64) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&revision.to_le_bytes());
    bytes[8..].copy_from_slice(&started.to_le_bytes());
    bytes
}

pub(super) fn validate_ticket(
    connection: &Transaction<'_>,
    project: Uuid,
    dataset: &SessionDataset,
) -> Result<()> {
    ensure!(
        dataset.owner.id == project && store::metadata::project_id(connection)? == project,
        "Intruder Session 项目归属已改变；数据保留"
    );
    ensure!(
        retired_run(connection, dataset.run.id)? == dataset.disk_ticket,
        "Intruder Session retirement receipt 缺失或已改变；数据保留"
    );
    ensure_no_run_rows(connection, dataset.run.id)
}

pub(super) fn ensure_no_run_rows(connection: &Connection, id: Uuid) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.intruder_runs WHERE id=?1)
         OR EXISTS(SELECT 1 FROM main.intruder_payloads WHERE run=?1)
         OR EXISTS(SELECT 1 FROM main.intruder_results WHERE run=?1)",
        [id.to_string()],
        |row| row.get(0),
    )?;
    ensure!(!exists, "Intruder run ID 或子记录被占用；Session 数据保留");
    Ok(())
}

pub(super) fn select(table: &str, columns: &str, selector: &str) -> String {
    format!("SELECT {columns} FROM main.{table} WHERE {selector}")
}

pub(super) fn dataset_logical_bytes(
    connection: &Connection,
    id: Uuid,
    limit: u64,
    cancel: &CancellationToken,
) -> Result<u64> {
    let mut total = OWNERSHIP_BYTES;
    ensure!(
        total <= limit,
        "Intruder Session 逻辑 RAM 数据配额不足；未复制或驱逐"
    );
    for &(table, columns, selector) in DATA_SELECTORS {
        // Lengths are computed in SQLite before any template, payload or message reaches Rust.
        let sizes = columns.split(',').map(|column| format!(
            "CASE typeof({column}) WHEN 'null' THEN 0 WHEN 'integer' THEN 8 WHEN 'real' THEN 8 ELSE length(CAST({column} AS BLOB)) END"
        )).collect::<Vec<_>>().join("+");
        let mut query = connection.prepare(&select(table, &sizes, selector))?;
        let mut rows = query.query([id.to_string()])?;
        while let Some(row) = rows.next()? {
            ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
            let bytes: u64 = row.get(0)?;
            total = total
                .checked_add(bytes)
                .and_then(|total| total.checked_add(ROW_ACCOUNTING_BYTES))
                .context("Intruder Session 逻辑数据计量溢出")?;
            ensure!(
                total <= limit,
                "Intruder Session 逻辑 RAM 数据配额不足（至少 {total}，可用 {limit} 字节）；未复制、驱逐或溢写到磁盘"
            );
        }
    }
    Ok(total)
}

pub(super) fn same_rows(
    left: &Connection,
    right: &Connection,
    sql: &str,
    id: Uuid,
) -> Result<bool> {
    let mut left_query = left.prepare(sql)?;
    let mut right_query = right.prepare(sql)?;
    let count = left_query.column_count();
    let mut left_rows = left_query.query([id.to_string()])?;
    let mut right_rows = right_query.query([id.to_string()])?;
    loop {
        match (left_rows.next()?, right_rows.next()?) {
            (None, None) => return Ok(true),
            (Some(left), Some(right)) => {
                for index in 0..count {
                    if left.get_ref(index)? != right.get_ref(index)? {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
    }
}

pub(super) fn has_flow(connection: &Connection, id: FlowId) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.flows WHERE id=?1)",
        [id.to_string()],
        |row| row.get(0),
    )?)
}

pub(super) fn same_flow(
    left: &Connection,
    right: &Connection,
    id: FlowId,
    messages: bool,
) -> Result<bool> {
    if !same_rows(left, right, &select("flows", DATA_SELECTORS[2].1, "id=?1"), id)? {
        return Ok(false);
    }
    if messages {
        return same_rows(
            left,
            right,
            &select("messages", DATA_SELECTORS[3].1, "flow=?1 ORDER BY side"),
            id,
        );
    }
    Ok(true)
}

pub(in crate::store::intruder) fn strict_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut ignored = false;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = serde_ignored::deserialize(&mut decoder, |_| ignored = true)?;
    decoder.end()?;
    ensure!(!ignored, "Intruder 包含未知 JSON 字段；未转换或丢失源数据");
    Ok(value)
}

pub(super) fn validate_dataset(
    connection: &Connection,
    id: Uuid,
    cancel: &CancellationToken,
) -> Result<IntruderRunSummary> {
    validate_dataset_snapshot(connection, id, cancel, false)
}

pub(super) fn validate_dataset_snapshot(
    connection: &Connection,
    id: Uuid,
    cancel: &CancellationToken,
    allow_running: bool,
) -> Result<IntruderRunSummary> {
    let run = read_run(connection, id)?;
    ensure!(
        run.state.is_terminal()
            || (allow_running
                && matches!(
                    run.state,
                    IntruderRunState::Running | IntruderRunState::Paused
                )),
        "Intruder 状态不允许此保留转换；数据保留"
    );
    let tab: String = connection.query_row(
        "SELECT tab FROM main.intruder_runs WHERE id=?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    ensure!(tab == run.tab_id.to_string(), "Intruder tab ID 编码无效");
    let mut query = connection.prepare(SPEC_QUERY)?;
    let mut rows = query.query(params![id.to_string(), MAX_SPEC_META, MAX_REQUEST_BYTES])?;
    let row = rows.next()?.context("Intruder 配置缺失")?;
    let _: SpecMeta = strict_json(row.get_ref(0)?.as_blob()?)?;
    read_spec(connection, id)?;
    let mut query = connection.prepare(&format!(
        "{} WHERE r.run=?1 ORDER BY r.ordinal",
        result_select()
    ))?;
    let mut rows = query.query([id.to_string()])?;
    let mut expected = 0;
    let mut completed = 0;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
        let result = decode_result(row)?;
        let terminal = outcome_terminal(&result.outcome);
        ensure!(
            result.ordinal == expected
                && expected < run.dispatched
                && (terminal || (allow_running && !run.state.is_terminal())),
            "Intruder 结果序号、计数或终态无效；数据保留"
        );
        completed += u64::from(terminal);
        if let IntruderResultOutcome::Flow(summary) = result.outcome {
            ensure!(
                terminal || summary.state == FlowState::Pending,
                "Intruder 在途结果必须是已登记的 Pending 请求；数据保留"
            );
            ensure!(
                row.get::<_, String>(2)? == summary.id.to_string(),
                "Intruder flow ID 编码无效"
            );
            let strict: FlowSummary = strict_json(row.get_ref(10)?.as_blob()?)?;
            ensure!(strict == summary, "Intruder 流量摘要不一致");
            validate_flow(connection, &summary)?;
        }
        expected += 1;
    }
    ensure!(expected == run.dispatched, "Intruder 结果缺失；数据保留");
    ensure!(
        completed == run.completed,
        "Intruder 已完成结果数量与运行计数不一致；数据保留"
    );
    Ok(run)
}

fn validate_flow(connection: &Connection, summary: &FlowSummary) -> Result<()> {
    let (id, revision, started, url, method, state): (String, u64, i64, String, String, String) =
        connection.query_row(
            "SELECT id,revision,started,url,method,state FROM main.flows WHERE id=?1",
            [summary.id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;
    ensure!(
        id == summary.id.to_string()
            && revision == summary.revision
            && started == summary.started_unix_ms
            && url == summary.url
            && method == summary.method
            && state == store::state_code(&summary.state),
        "Intruder 流量索引与完整摘要不一致；数据保留"
    );
    let mut query = connection.prepare(
        "SELECT side,typeof(head),length(head),typeof(body),length(body),
                CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END
         FROM main.messages WHERE flow=?1 ORDER BY side",
    )?;
    let mut rows = query.query(params![id, MAX_MESSAGE_HEAD])?;
    while let Some(row) = rows.next()? {
        let side: u8 = row.get(0)?;
        ensure!(
            side <= 3
                && row.get::<_, String>(1)? == "blob"
                && row.get::<_, usize>(2)? <= MAX_MESSAGE_HEAD
                && row.get::<_, String>(3)? == "blob"
                && row.get::<_, usize>(4)? <= MAX_BODY,
            "Intruder 消息版本、类型或长度不受支持；数据保留"
        );
        let bytes = row.get_ref(5)?.as_blob()?;
        if side == 0 || side == 2 {
            let _: store::OwnedRequestHead = strict_json(bytes)?;
        } else {
            ensure!(
                is_flow_terminal(&summary.state),
                "Intruder 在途结果不能包含响应版本；数据保留"
            );
            let head: store::OwnedResponseHead = strict_json(bytes)?;
            ensure!(
                head.stream.is_none(),
                "Intruder 流式消息布局不支持保留切换；数据保留"
            );
        }
    }
    let record = store::read_record(connection, summary.id)?.context("Intruder 流量缺失")?;
    ensure!(record.summary == *summary, "Intruder 流量内容与摘要不一致");
    ensure!(
        is_flow_terminal(&summary.state) || record.response.is_none(),
        "Intruder 在途结果不能包含响应；数据保留"
    );
    validate_record(&record)
}
