use super::*;

pub(in crate::store::intruder) fn sort_by_length(
    rows: &mut [(u64, u64, Option<u64>)],
    ascending: bool,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let mut comparisons = 0usize;
    let mut aborted = None;
    rows.sort_by(|left, right| {
        if aborted.is_some() {
            return std::cmp::Ordering::Equal;
        }
        comparisons = comparisons.wrapping_add(1);
        if comparisons.is_multiple_of(1024)
            && let Err(error) = check()
        {
            aborted = Some(error);
            return std::cmp::Ordering::Equal;
        }
        let ordering = left.2.cmp(&right.2);
        let ordering = if ascending {
            ordering
        } else {
            ordering.reverse()
        };
        ordering.then_with(|| left.1.cmp(&right.1))
    });
    if let Some(error) = aborted {
        return Err(error);
    }
    Ok(())
}

fn read_response_head(
    statement: &mut rusqlite::Statement<'_>,
    flow: &str,
    expected_status: u16,
    check: &dyn Fn() -> Result<()>,
) -> Result<Option<crate::store::OwnedResponseHead>> {
    check()?;
    let Some(row) = statement
        .query_row(params![flow, MAX_SEARCH_HEAD_BYTES], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, usize>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .optional()?
    else {
        return Ok(None);
    };
    ensure!(
        row.0 == "blob" && row.1 <= MAX_SEARCH_HEAD_BYTES,
        "Intruder 结果响应消息类型或长度无效"
    );
    let head_bytes = row.2.context("Intruder 结果响应头超过 128 KiB 限额")?;
    ensure!(
        head_bytes.len() == row.1,
        "Intruder 结果响应消息长度与摘要不一致"
    );
    let head: crate::store::OwnedResponseHead = retention::strict_json(&head_bytes)?;
    ensure!(
        head.stream.is_none() && head.status == expected_status,
        "Intruder 结果响应状态或流式元数据无效"
    );
    check()?;
    Ok(Some(head))
}

pub(super) fn response_length(
    recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
    ordinal: u64,
    status: Option<u16>,
    response_bytes: u64,
    flow: Option<&str>,
    message_query: &mut rusqlite::Statement<'_>,
    check: &dyn Fn() -> Result<()>,
) -> Result<Option<u64>> {
    let Some(status) = status else {
        return Ok(None);
    };
    if let Some(&response) = recovery_responses.get(&ordinal) {
        ensure!(
            response.status == status && response.body.len() as u64 == response_bytes,
            "Intruder 恢复响应状态或长度不一致"
        );
        return Ok(Some(crate::http::intruder_response_len(
            response.version,
            response.status,
            response.reason_phrase.as_deref(),
            &response.headers,
            response.body.len() as u64,
        )));
    }
    let flow = flow.context("Intruder 已完成响应缺少关联 flow")?;
    let Some(head) = read_response_head(message_query, flow, status, check)? else {
        bail!("Intruder 结果响应消息缺失");
    };
    Ok(Some(crate::http::intruder_response_len(
        head.version,
        head.status,
        head.reason_phrase.as_deref(),
        &head.headers,
        response_bytes,
    )))
}

pub(in crate::store::intruder) fn sort_filtered_rows(
    rows: &mut [MatchedResult],
    sort: crate::IntruderResultSort,
    grep_config: &crate::IntruderGrepConfig,
    payload_order: Option<&result_view::PayloadOrder>,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    check()?;
    match sort.column {
        IntruderResultSortColumn::Request => {
            if !sort.ascending {
                rows.reverse();
            }
        }
        IntruderResultSortColumn::Status => {
            let mut comparisons = 0usize;
            let mut aborted = None;
            rows.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                let ordering = left.status.cmp(&right.status);
                let ordering = if sort.ascending {
                    ordering
                } else {
                    ordering.reverse()
                };
                ordering.then_with(|| left.number.cmp(&right.number))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
        }
        IntruderResultSortColumn::Payload => {
            let order = payload_order.expect("payload order loaded");
            let mut comparisons = 0usize;
            let mut aborted = None;
            rows.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                order
                    .compare_indices(left.payload_index, right.payload_index, sort.ascending)
                    .then_with(|| left.number.cmp(&right.number))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
        }
        IntruderResultSortColumn::ResponseReceived => {
            let mut comparisons = 0usize;
            let mut aborted = None;
            rows.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                let ordering = left.response_received.cmp(&right.response_received);
                let ordering = if sort.ascending {
                    ordering
                } else {
                    ordering.reverse()
                };
                ordering.then_with(|| left.number.cmp(&right.number))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
        }
        IntruderResultSortColumn::GrepMatch(idx) => {
            let target = grep_config.match_expressions.get(idx).map(String::as_str);
            let mut comparisons = 0usize;
            let mut aborted = None;
            rows.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                let left_hit =
                    target.is_some_and(|expr| left.grep_matches.iter().any(|m| m == expr));
                let right_hit =
                    target.is_some_and(|expr| right.grep_matches.iter().any(|m| m == expr));
                let ordering = left_hit.cmp(&right_hit);
                let ordering = if sort.ascending {
                    ordering
                } else {
                    ordering.reverse()
                };
                ordering.then_with(|| left.number.cmp(&right.number))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
        }
        IntruderResultSortColumn::GrepExtract(idx) => {
            let key_label = grep_config
                .extract_rules
                .get(idx)
                .map(crate::IntruderGrepExtractRule::key_label);
            let mut comparisons = 0usize;
            let mut aborted = None;
            rows.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                let left_val = key_label.as_deref().and_then(|key| {
                    left.grep_extracts
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.as_str())
                });
                let right_val = key_label.as_deref().and_then(|key| {
                    right
                        .grep_extracts
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.as_str())
                });
                let ordering = left_val.cmp(&right_val);
                let ordering = if sort.ascending {
                    ordering
                } else {
                    ordering.reverse()
                };
                ordering.then_with(|| left.number.cmp(&right.number))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
        }
        IntruderResultSortColumn::GrepPayload(idx) => {
            let mut comparisons = 0usize;
            let mut aborted = None;
            rows.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                let left_hit = left.grep_payload_hits.get(idx).copied().unwrap_or(false);
                let right_hit = right.grep_payload_hits.get(idx).copied().unwrap_or(false);
                let ordering = left_hit.cmp(&right_hit);
                let ordering = if sort.ascending {
                    ordering
                } else {
                    ordering.reverse()
                };
                ordering.then_with(|| left.number.cmp(&right.number))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
        }
        IntruderResultSortColumn::Length => unreachable!("handled by length sort"),
    }
    check()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn view_row(
    connection: &Connection,
    run: &IntruderRunSummary,
    (recovery, recovery_responses): (
        &BTreeMap<u64, &IntruderResultSummary>,
        &BTreeMap<u64, &crate::HttpResponse>,
    ),
    ordinal: u64,
    payloads: &mut BTreeMap<u64, Vec<result_view::PayloadSummary>>,
    grep_config: &crate::IntruderGrepConfig,
    message_query: &mut rusqlite::Statement<'_>,
    full_message_query: Option<&mut rusqlite::Statement<'_>>,
    check: &dyn Fn() -> Result<()>,
) -> Result<Option<IntruderResultViewRow>> {
    let Some(payloads) = payloads.remove(&ordinal) else {
        return Ok(None);
    };
    let result = if let Some(result) = recovery.get(&ordinal) {
        Some((*result).clone())
    } else if ordinal == run.dispatched
        && run
            .baseline
            .is_some_and(|baseline| baseline.ordinal == ordinal)
    {
        None
    } else {
        Some(read_result(connection, run.id, ordinal)?.context("Intruder 页面结果缺失")?)
    };
    let (response_length, grep_matches, grep_extracts, grep_payload_hits) = match result.as_ref() {
        Some(IntruderResultSummary {
            outcome: IntruderResultOutcome::Flow(summary),
            ..
        }) if summary.status.is_some() => {
            let flow = summary.id.to_string();
            let length = response_length(
                recovery_responses,
                ordinal,
                summary.status,
                summary.response_bytes,
                Some(&flow),
                message_query,
                check,
            )?;
            if grep_config.is_empty() {
                (length, Vec::new(), Vec::new(), Vec::new())
            } else {
                let expected_status = summary.status.expect("status checked");
                let raw = if let Some(&response) = recovery_responses.get(&ordinal) {
                    ensure!(
                        response.status == expected_status
                            && response.body.len() as u64 == summary.response_bytes,
                        "Intruder 恢复响应状态或长度不一致"
                    );
                    crate::response_to_raw(response)
                } else {
                    let full_stmt = full_message_query
                        .context("Intruder grep 查询缺少完整响应语句")?;
                    super::filters::read_search_response(
                        full_stmt,
                        &flow,
                        expected_status,
                        summary.response_bytes,
                        check,
                    )?
                };
                let payload_slices: Vec<&[u8]> = payloads
                    .iter()
                    .map(|payload| payload.preview_bytes.as_slice())
                    .collect();
                let (matches, extracts, hits) =
                    crate::intruder::evaluate_grep_row(&raw, grep_config, &payload_slices);
                (length, matches, extracts, hits)
            }
        }
        _ => (None, Vec::new(), Vec::new(), Vec::new()),
    };
    let (notes, notes_revision, highlight, highlight_revision): (
        Option<Vec<u8>>,
        u64,
        Option<u8>,
        u64,
    ) = if result.is_none() || recovery.contains_key(&ordinal) {
        (None, 0, None, 0)
    } else {
        connection
            .query_row(
                "SELECT CASE WHEN typeof(notes)='blob' AND length(notes)<=?1 THEN substr(notes,1,259) END,
                        notes_revision,highlight,highlight_revision
                 FROM intruder_results WHERE run=?2 AND ordinal=?3",
                params![MAX_ERROR_BYTES, run.id.to_string(), ordinal],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                    ))
                },
            )
            .optional()?
            .context("Intruder 结果注解行缺失")?
    };
    let notes_preview = notes
        .as_deref()
        .map(|bytes| {
            let text = String::from_utf8_lossy(bytes);
            let cut = text.floor_char_boundary(256.min(text.len()));
            text[..cut].to_owned()
        })
        .unwrap_or_default();
    Ok(Some(IntruderResultViewRow {
        ordinal,
        result,
        payloads: payloads
            .into_iter()
            .map(|payload| IntruderPayloadPreview {
                set_index: payload.selected.set_index,
                payload_index: payload.selected.payload_index,
                raw_bytes: payload.raw_bytes,
                prefix: payload.preview_bytes,
            })
            .collect(),
        response_length,
        notes_revision,
        notes_preview,
        highlight,
        highlight_revision,
        grep_matches,
        grep_extracts,
        grep_payload_hits,
    }))
}

pub(in crate::store::intruder) fn view_row_bytes(row: &IntruderResultViewRow) -> Result<usize> {
    let result = match row.result.as_ref().map(|result| &result.outcome) {
        Some(IntruderResultOutcome::Flow(summary)) => serde_json::to_vec(summary)?.len(),
        Some(IntruderResultOutcome::Rejected(error)) => error.len(),
        None => 0,
    };
    let bytes = row
        .payloads
        .iter()
        .try_fold(result + 64, |bytes, payload| {
            bytes
                .checked_add(payload.prefix.len() + 64)
                .context("Intruder 页面 payload 长度溢出")
        })?;
    let bytes = row.grep_matches.iter().try_fold(bytes, |bytes, matched| {
        bytes
            .checked_add(matched.len())
            .context("Intruder 页面 grep match 长度溢出")
    })?;
    let bytes = row
        .grep_extracts
        .iter()
        .try_fold(bytes, |bytes, (key, value)| {
            bytes
                .checked_add(key.len() + value.len())
                .context("Intruder 页面 grep extract 长度溢出")
        })?;
    bytes
        .checked_add(row.grep_payload_hits.len())
        .context("Intruder 页面 grep payload 长度溢出")
}
