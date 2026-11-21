use super::*;

fn ensure_flows_binary_pk(transaction: &Transaction<'_>) -> Result<()> {
    let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let foreign_keys: bool =
        transaction.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    ensure!(
        schema == super::super::SCHEMA && foreign_keys,
        "不能确认 Intruder 丢弃的项目版本或外键保护"
    );
    let mut indexes = transaction.prepare("PRAGMA main.index_list('flows')")?;
    let mut indexes = indexes.query([])?;
    let mut primary_indexes = 0;
    while let Some(index) = indexes.next()? {
        if index.get::<_, String>(3)? != "pk" {
            continue;
        }
        primary_indexes += 1;
        let name = index.get::<_, String>(1)?.replace('"', "\"\"");
        let mut columns =
            transaction.prepare(&format!("PRAGMA main.index_xinfo(\"{name}\")"))?;
        let mut columns = columns.query([])?;
        while let Some(column) = columns.next()? {
            if column.get::<_, bool>(5)? {
                ensure!(
                    column.get::<_, Option<String>>(4)?.as_deref() == Some("BINARY"),
                    "项目主键比较规则未知；不能安全丢弃 Intruder"
                );
            }
        }
    }
    ensure!(
        primary_indexes == 1,
        "项目主键索引未知；不能安全丢弃 Intruder"
    );
    Ok(())
}

pub(in crate::store) fn discard_project_run(
    transaction: &Transaction<'_>,
    project_id: Uuid,
    id: Uuid,
    expected_revision: u64,
    mut workspace: Workspace,
    cancel: &CancellationToken,
) -> Result<IntruderDiscardReceipt> {
    ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
    ensure_flows_binary_pk(transaction)?;
    ensure!(
        super::super::metadata::project_id(transaction)? == project_id,
        "项目 ID 已改变；Intruder 未丢弃"
    );
    let run = read_run(transaction, id)?;
    ensure!(
        run.state.is_terminal(),
        "Intruder 尚未结束；此操作不会取消运行"
    );
    ensure!(
        run.revision == expected_revision,
        "Intruder run revision 已改变；未丢弃"
    );
    let revision = next_revision(run.revision)?;
    let previous = super::super::metadata::workspace(transaction)?;
    ensure!(
        previous.revision.checked_add(1) == Some(workspace.revision),
        "工作区已变更；保留当前草稿，请重新同步"
    );
    for tab in workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut workspace.intruder.closed)
    {
        if tab.last_run == Some(id) {
            tab.last_run = None;
        }
    }
    workspace.validate()?;
    // Existing and proposed owners both protect independently retained flows.
    let pinned: BTreeSet<_> = previous
        .tabs
        .iter()
        .chain(&previous.closed)
        .chain(&workspace.tabs)
        .chain(&workspace.closed)
        .filter_map(|tab| tab.last_flow)
        .chain(super::super::plugin_storage::audit_pinned_flows(
            transaction,
        )?)
        .collect();
    {
        // Validate ownership and terminal ordinals without loading templates, payloads or bodies.
        let mut query = transaction.prepare(&format!(
            "{} WHERE r.run=?1 ORDER BY r.ordinal",
            result_select()
        ))?;
        let mut rows = query.query([id.to_string()])?;
        let mut expected = 0;
        while let Some(row) = rows.next()? {
            ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
            let result = decode_result(row)?;
            ensure!(
                result.ordinal == expected
                    && expected < run.dispatched
                    && outcome_terminal(&result.outcome),
                "Intruder 结果序号、计数或终态无效；未丢弃"
            );
            if let IntruderResultOutcome::Flow(flow) = result.outcome {
                let flow_id = flow.id.to_string();
                ensure!(
                    row.get::<_, Option<String>>(2)?.as_deref() == Some(flow_id.as_str()),
                    "Intruder flow ID 编码无效；未丢弃"
                );
                shared_discard_flow(transaction, id, flow.id, &pinned)?;
            }
            expected += 1;
        }
        ensure!(expected == run.dispatched, "Intruder 结果缺失；未丢弃");
    }
    ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
    super::super::save::write_workspace(transaction, &workspace)?;
    let mut deleted_flows = 0;
    let mut retained_flows = 0;
    loop {
        ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
        let batch: Vec<(u64, Option<String>)> = {
            let mut query = transaction.prepare(
                "SELECT ordinal,flow FROM main.intruder_results WHERE run=?1 ORDER BY ordinal LIMIT ?2",
            )?;
            let rows = query.query_map(params![id.to_string(), DISCARD_BATCH], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        if batch.is_empty() {
            break;
        }
        for (ordinal, flow) in batch {
            let shared = flow
                .as_ref()
                .map(|flow| shared_discard_flow(transaction, id, Uuid::parse_str(flow)?, &pinned))
                .transpose()?;
            let deleted = transaction.execute(
                "DELETE FROM main.intruder_results WHERE run=?1 AND ordinal=?2",
                params![id.to_string(), ordinal],
            )?;
            ensure!(deleted == 1, "Intruder 结果删除数量不一致");
            if let Some(flow) = flow {
                if shared == Some(true) {
                    retained_flows += 1;
                } else {
                    transaction.execute("DELETE FROM main.messages WHERE flow=?1", [&flow])?;
                    let deleted =
                        transaction.execute("DELETE FROM main.flows WHERE id=?1", [&flow])?;
                    ensure!(deleted == 1, "Intruder 流量删除数量不一致");
                    if let Ok(fid) = Uuid::parse_str(&flow) {
                        super::super::provenance::remove_flow_provenance(transaction, &[fid])?;
                    }
                    deleted_flows += 1;
                }
            }
        }
    }
    loop {
        ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
        let deleted = transaction.execute(
            "DELETE FROM main.intruder_payloads WHERE run=?1 AND (set_index,ordinal) IN
             (SELECT set_index,ordinal FROM main.intruder_payloads WHERE run=?1 ORDER BY set_index,ordinal LIMIT ?2)",
            params![id.to_string(), DISCARD_BATCH],
        )?;
        if deleted == 0 {
            break;
        }
    }
    let remaining_payloads: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.intruder_payloads WHERE run=?1)",
        [id.to_string()],
        |row| row.get(0),
    )?;
    ensure!(
        !remaining_payloads,
        "Intruder payload 删除键含 NULL 或未完整清理；未丢弃"
    );
    let deleted = transaction.execute(
        "DELETE FROM main.intruder_runs WHERE id=?1 AND revision=?2",
        params![id.to_string(), expected_revision],
    )?;
    ensure!(deleted == 1, "Intruder run 删除数量不一致");
    // Only the retired revision and original cursor timestamp survive logical deletion.
    let mut retired = [0_u8; 16];
    retired[..8].copy_from_slice(&revision.to_le_bytes());
    retired[8..].copy_from_slice(&run.started_unix_ms.to_le_bytes());
    transaction.execute(
        "INSERT INTO main.metadata(key,value) VALUES(?1,?2)",
        params![retirement_key(id), retired.as_slice()],
    )?;
    ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
    Ok(IntruderDiscardReceipt {
        revision,
        workspace,
        deleted_flows,
        retained_flows,
    })
}

pub(in crate::store::intruder) fn retirement_key(id: Uuid) -> String {
    format!("intruder_discarded:{id}")
}

pub(in crate::store) fn validate_retirements(connection: &Connection) -> Result<()> {
    let has_non_text_key: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.metadata WHERE key < '')
         OR EXISTS(SELECT 1 FROM main.metadata WHERE key >= X'')",
        [],
        |row| row.get(0),
    )?;
    ensure!(!has_non_text_key, "Intruder 丢弃键长度或类型无效");
    let prefix = "intruder_discarded:";
    let mut statement = connection.prepare(
        "SELECT CASE WHEN typeof(key)='text' AND length(CAST(key AS BLOB))=?1 THEN key END,
                CASE WHEN typeof(value)='blob' AND length(value)=16 THEN value END
         FROM main.metadata WHERE key GLOB 'intruder_discarded:*'",
    )?;
    let mut rows = statement.query([prefix.len() + 36])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0).context("Intruder 丢弃键长度或类型无效")?;
        let suffix = key
            .strip_prefix(prefix)
            .context("Intruder 丢弃键前缀无效")?;
        let id = Uuid::parse_str(suffix).context("Intruder 丢弃键 ID 无效")?;
        ensure!(
            key == retirement_key(id),
            "Intruder 丢弃键必须使用规范 UUID"
        );
        let bytes = row.get_ref(1)?.as_blob().context("Intruder 丢弃记录损坏")?;
        let revision = u64::from_le_bytes(bytes[..8].try_into()?);
        ensure!(
            (1..=i64::MAX as u64).contains(&revision),
            "Intruder 丢弃 revision 无效"
        );
    }
    Ok(())
}

pub(in crate::store::intruder) fn retired_run(
    connection: &Connection,
    id: Uuid,
) -> Result<Option<(u64, i64)>> {
    let mut query = connection.prepare(
        "SELECT CASE WHEN typeof(value)='blob' AND length(value)=16 THEN value END
         FROM main.metadata WHERE key=?1",
    )?;
    let mut rows = query.query([retirement_key(id)])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let bytes = row.get_ref(0)?.as_blob().context("Intruder 丢弃记录损坏")?;
    let revision = u64::from_le_bytes(bytes[..8].try_into()?);
    let started = i64::from_le_bytes(bytes[8..].try_into()?);
    ensure!(
        (1..=i64::MAX as u64).contains(&revision),
        "Intruder 丢弃 revision 无效"
    );
    ensure!(rows.next()?.is_none(), "Intruder 丢弃记录重复");
    Ok(Some((revision, started)))
}

pub(in crate::store) fn ensure_not_discarded(connection: &Connection, id: Uuid) -> Result<()> {
    ensure!(
        retired_run(connection, id)?.is_none(),
        "Intruder run 已丢弃；不能重新开始、恢复或引用"
    );
    Ok(())
}

pub(in crate::store::intruder) fn run_cursor(
    connection: &Connection,
    id: Uuid,
) -> Result<(i64, Uuid)> {
    if let Some(run) = find_run(connection, id)? {
        ensure_not_discarded(connection, id)?;
        return Ok((run.started_unix_ms, id));
    }
    let (_, started) = retired_run(connection, id)?.context("Intruder 分页游标不存在")?;
    Ok((started, id))
}

pub(in crate::store::intruder) fn shared_discard_flow(
    transaction: &Transaction<'_>,
    run: Uuid,
    flow: Uuid,
    pinned: &BTreeSet<Uuid>,
) -> Result<bool> {
    let references: u64 = transaction.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM main.intruder_results WHERE run=?1 AND flow=?2 LIMIT 2)",
        params![run.to_string(), flow.to_string()],
        |row| row.get(0),
    )?;
    ensure!(references == 1, "Intruder 流量归属重复；未丢弃");
    let linked: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.intruder_results WHERE flow=?2 AND run!=?1)
         OR EXISTS(SELECT 1 FROM main.websocket_messages WHERE flow=?2)
         OR EXISTS(SELECT 1 FROM main.websocket_connections WHERE flow=?2)
         OR EXISTS(SELECT 1 FROM main.repeater_history WHERE flow=?2)",
        params![run.to_string(), flow.to_string()],
        |row| row.get(0),
    )?;
    Ok(pinned.contains(&flow) || linked)
}
