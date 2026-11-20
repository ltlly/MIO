use super::*;

pub(in crate::store::intruder) fn is_flow_terminal(state: &FlowState) -> bool {
    !matches!(state, FlowState::Pending | FlowState::Intercepted)
}

pub(in crate::store::intruder) fn outcome_terminal(outcome: &IntruderResultOutcome) -> bool {
    match outcome {
        IntruderResultOutcome::Flow(summary) => is_flow_terminal(&summary.state),
        IntruderResultOutcome::Rejected(_) => true,
    }
}

pub(in crate::store::intruder) fn validate_summary(summary: &FlowSummary) -> Result<()> {
    ensure!(
        summary.source == TrafficSource::Intruder,
        "Intruder 结果引用了其它工具的流量"
    );
    ensure!(
        (1..=i64::MAX as u64).contains(&summary.revision),
        "Intruder flow revision 无效"
    );
    ensure!(
        summary.request_bytes <= MAX_BODY as u64 && summary.response_bytes <= MAX_BODY as u64,
        "Intruder 正文字节数超过限额"
    );
    ensure!(
        summary
            .status
            .is_none_or(|status| (100..=599).contains(&status))
            && (!matches!(summary.state, FlowState::Complete) || summary.status.is_some())
            && (is_flow_terminal(&summary.state)
                || (summary.status.is_none() && summary.response_bytes == 0)),
        "Intruder 响应状态与结果终态不一致"
    );
    Ok(())
}

pub(in crate::store::intruder) fn validate_record(record: &FlowRecord) -> Result<()> {
    validate_record_with_reason_phrase(record, None)
}

pub(in crate::store::intruder) fn validate_record_with_reason_phrase(
    record: &FlowRecord,
    reason_phrase: Option<&[u8]>,
) -> Result<()> {
    validate_summary(&record.summary)?;
    ensure!(
        record.summary.method == record.request.method
            && record.summary.url == record.request.url
            && record.summary.request_bytes == record.request.body.len() as u64,
        "Intruder 请求与摘要不一致"
    );
    ensure!(
        record.summary.status == record.response.as_ref().map(|response| response.status)
            && record.summary.response_bytes
                == record
                    .response
                    .as_ref()
                    .map_or(0, |response| response.body.len() as u64),
        "Intruder 响应与摘要不一致"
    );
    ensure!(
        !matches!(record.summary.state, FlowState::Complete) || record.response.is_some(),
        "Intruder 完成结果缺少响应"
    );
    ensure!(
        is_flow_terminal(&record.summary.state) || record.response.is_none(),
        "Intruder 在途结果不能包含响应"
    );
    ensure!(
        reason_phrase.is_none() || record.response.is_some(),
        "Intruder 无响应结果不能附带 reason phrase"
    );
    ensure!(
        serde_json::to_vec(&record.summary)?.len() <= MAX_SUMMARY_BYTES,
        "Intruder 结果摘要超过编码限额"
    );
    ensure!(
        serde_json::to_vec(&super::super::heads::RequestHead::from(&record.request))?.len()
            <= MAX_MESSAGE_HEAD,
        "Intruder 请求元数据超过编码限额"
    );
    if let Some(response) = &record.response {
        let mut head = super::super::heads::ResponseHead::from(response);
        head.reason_phrase = reason_phrase.or(response.reason_phrase.as_deref());
        ensure!(
            serde_json::to_vec(&head)?.len() <= MAX_MESSAGE_HEAD,
            "Intruder 响应元数据超过编码限额"
        );
    }
    Ok(())
}

pub(in crate::store::intruder) fn request_matches(
    connection: &Connection,
    record: &FlowRecord,
) -> Result<bool> {
    message_matches(
        connection,
        record.summary.id,
        0,
        &serde_json::to_vec(&super::super::heads::RequestHead::from(&record.request))?,
        &record.request.body,
    )
}

pub(in crate::store::intruder) fn response_absent(
    connection: &Connection,
    id: Uuid,
) -> Result<bool> {
    Ok(!connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE flow=?1 AND side=1)",
        [id.to_string()],
        |row| row.get::<_, bool>(0),
    )?)
}

pub(in crate::store::intruder) fn record_matches(
    connection: &Connection,
    record: &FlowRecord,
) -> Result<bool> {
    record_matches_with_reason_phrase(connection, record, None)
}

pub(in crate::store::intruder) fn record_matches_with_reason_phrase(
    connection: &Connection,
    record: &FlowRecord,
    reason_phrase: Option<&[u8]>,
) -> Result<bool> {
    if !request_matches(connection, record)? {
        return Ok(false);
    }
    match &record.response {
        Some(response) => {
            let mut head = super::super::heads::ResponseHead::from(response);
            let effective_reason = reason_phrase.or(response.reason_phrase.as_deref());
            head.reason_phrase = effective_reason;
            if message_matches(
                connection,
                record.summary.id,
                1,
                &serde_json::to_vec(&head)?,
                &response.body,
            )? {
                return Ok(true);
            }
            if effective_reason.is_none()
                && let Some(stored_head) = super::super::flows::read_response_head(
                    connection,
                    &record.summary.id.to_string(),
                    1,
                )?
                && stored_head.reason_phrase.is_some()
            {
                head.reason_phrase = stored_head.reason_phrase.as_deref();
                return message_matches(
                    connection,
                    record.summary.id,
                    1,
                    &serde_json::to_vec(&head)?,
                    &response.body,
                );
            }
            Ok(false)
        }
        None => response_absent(connection, record.summary.id),
    }
}

pub(in crate::store::intruder) fn message_matches(
    connection: &Connection,
    id: Uuid,
    side: u8,
    head: &[u8],
    body: &[u8],
) -> Result<bool> {
    let equal: Option<Option<bool>> = connection.query_row(
        "SELECT CASE WHEN typeof(head)='blob' AND length(head)<=?3 AND typeof(body)='blob' AND length(body)<=?4
            THEN head=?5 AND body=?6 END FROM messages WHERE flow=?1 AND side=?2",
        params![id.to_string(), side, MAX_MESSAGE_HEAD as i64, MAX_BODY as i64, head, body], |row| row.get(0),
    ).optional()?;
    Ok(equal.flatten().unwrap_or(false))
}

pub(in crate::store::intruder) fn result_select() -> String {
    format!(
        "SELECT r.ordinal,typeof(r.flow),
            CASE WHEN typeof(r.flow)='text' AND length(CAST(r.flow AS BLOB))=36 THEN r.flow END,
            typeof(r.error),CASE WHEN typeof(r.error)='text' THEN length(CAST(r.error AS BLOB)) END,
            CASE WHEN typeof(r.error)='text' AND length(CAST(r.error AS BLOB))<={MAX_ERROR_BYTES} THEN r.error END,
            f.revision,CASE WHEN typeof(f.state)='text' AND length(CAST(f.state AS BLOB))<=32 THEN f.state END,
            typeof(f.summary),CASE WHEN typeof(f.summary)='blob' THEN length(f.summary) END,
            CASE WHEN typeof(f.summary)='blob' AND length(f.summary)<={MAX_SUMMARY_BYTES} THEN f.summary END
         FROM intruder_results r LEFT JOIN flows f ON f.id=r.flow"
    )
}

pub(in crate::store::intruder) fn result_size(row: &Row<'_>) -> Result<usize> {
    let flow_type: String = row.get(1)?;
    let error_type: String = row.get(3)?;
    if flow_type == "null" && error_type == "text" {
        let length: usize = row.get(4)?;
        ensure!(length <= MAX_ERROR_BYTES, "Intruder 拒绝原因超过读取限额");
        Ok(length)
    } else {
        ensure!(
            flow_type == "text" && error_type == "null" && row.get::<_, String>(8)? == "blob",
            "Intruder 结果类型或关联流量无效"
        );
        let length: usize = row.get(9)?;
        ensure!(length <= MAX_SUMMARY_BYTES, "Intruder 结果摘要超过读取限额");
        Ok(length)
    }
}

pub(in crate::store::intruder) fn decode_result(row: &Row<'_>) -> Result<IntruderResultSummary> {
    result_size(row)?;
    let ordinal: u64 = row.get(0)?;
    ensure!(ordinal < MAX_REQUESTS, "Intruder 结果序号无效");
    let outcome = if row.get::<_, String>(1)? == "null" {
        IntruderResultOutcome::Rejected(
            row.get::<_, Option<String>>(5)?
                .context("Intruder 拒绝原因类型或长度无效")?,
        )
    } else {
        let flow = Uuid::parse_str(
            &row.get::<_, Option<String>>(2)?
                .context("Intruder flow ID 无效")?,
        )?;
        let summary: FlowSummary = serde_json::from_slice(
            row.get_ref(10)?
                .as_blob()
                .context("Intruder 结果摘要类型或长度无效")?,
        )?;
        validate_summary(&summary)?;
        ensure!(
            summary.id == flow
                && summary.revision == row.get::<_, u64>(6)?
                && row.get::<_, Option<String>>(7)?.as_deref()
                    == Some(super::super::flows::state_code(&summary.state)),
            "Intruder flow 摘要与索引不一致"
        );
        IntruderResultOutcome::Flow(summary)
    };
    Ok(IntruderResultSummary { ordinal, outcome })
}

pub(in crate::store::intruder) fn read_result(
    connection: &Connection,
    id: Uuid,
    ordinal: u64,
) -> Result<Option<IntruderResultSummary>> {
    let mut query = connection.prepare(&format!(
        "{} WHERE r.run=?1 AND r.ordinal=?2",
        result_select()
    ))?;
    let mut rows = query.query(params![id.to_string(), ordinal])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let result = decode_result(row)?;
    ensure!(rows.next()?.is_none(), "Intruder 结果序号重复");
    Ok(Some(result))
}

pub(in crate::store::intruder) fn write_result_notes(
    transaction: &Transaction<'_>,
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
    let run = read_run(transaction, id)?;
    let (current_revision, _current) = result_annotations(transaction, id, ordinal, &run)?;
    ensure!(
        current_revision == expected_notes_revision,
        "Intruder 结果备注 revision 已改变；未覆盖（当前 {current_revision}）"
    );
    let revision = next_revision(expected_notes_revision)?;
    let changed = transaction.execute(
        "UPDATE intruder_results SET notes=?1,notes_revision=?2
         WHERE run=?3 AND ordinal=?4 AND notes_revision=?5",
        params![
            notes,
            revision,
            id.to_string(),
            ordinal,
            expected_notes_revision
        ],
    )?;
    ensure!(changed == 1, "Intruder 结果备注修改冲突");
    ensure!(
        !cancel.is_cancelled(),
        "Intruder 结果备注修改已取消；数据保留"
    );
    Ok((revision, notes.to_vec()))
}

pub(in crate::store::intruder) fn result_annotations(
    transaction: &Transaction<'_>,
    id: Uuid,
    ordinal: u64,
    run: &IntruderRunSummary,
) -> Result<(u64, Option<IntruderResultSummary>)> {
    ensure!(ordinal < run.total, "Intruder 结果序号超过冻结计划的数量");
    let (notes_revision, highlight, highlight_revision): (u64, Option<u8>, u64) = transaction
        .query_row(
            "SELECT notes_revision,highlight,highlight_revision FROM intruder_results
             WHERE run=?1 AND ordinal=?2",
            params![id.to_string(), ordinal],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .context("Intruder 结果不存在；不能修改注解")?;
    ensure!(
        ordinal < run.dispatched,
        "Intruder 基准请求尚未派发；不能修改注解"
    );
    let current =
        read_result(transaction, id, ordinal)?.context("Intruder 结果不存在；不能修改注解")?;
    ensure!(
        matches!(current.outcome, IntruderResultOutcome::Flow(_)),
        "未发送的 Intruder 请求不能设置注解"
    );
    let _ = (highlight, highlight_revision);
    Ok((notes_revision, Some(current)))
}

pub(in crate::store::intruder) fn write_result_highlights(
    transaction: &Transaction<'_>,
    id: Uuid,
    entries: &[(u64, u64)],
    highlight: Option<u8>,
    cancel: &CancellationToken,
) -> Result<Vec<(u64, u64)>> {
    ensure!(
        !cancel.is_cancelled(),
        "Intruder 结果高亮修改已取消；数据保留"
    );
    let run = read_run(transaction, id)?;
    let mut updated = Vec::with_capacity(entries.len());
    for &(ordinal, expected_highlight_revision) in entries {
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果高亮修改已取消；数据保留"
        );
        let (current_notes_revision, current) = result_annotations(transaction, id, ordinal, &run)?;
        let _ = current_notes_revision;
        let _ = current;
        ensure!(current.is_some(), "Intruder 基准请求尚未派发；不能设置高亮");
        let revision = next_revision(expected_highlight_revision)?;
        let changed = transaction.execute(
            "UPDATE intruder_results SET highlight=?1,highlight_revision=?2
             WHERE run=?3 AND ordinal=?4 AND highlight_revision=?5",
            params![
                highlight,
                revision,
                id.to_string(),
                ordinal,
                expected_highlight_revision
            ],
        )?;
        ensure!(changed == 1, "Intruder 结果高亮修改冲突");
        updated.push((ordinal, revision));
    }
    ensure!(
        !cancel.is_cancelled(),
        "Intruder 结果高亮修改已取消；数据保留"
    );
    Ok(updated)
}
