use super::page::{response_length, sort_by_length, sort_filtered_rows};
use super::*;

pub(super) fn matching_ordinals(
    connection: &Connection,
    query: &IntruderResultViewQuery,
    run: &IntruderRunSummary,
    recovery: &BTreeMap<u64, &IntruderResultSummary>,
    recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
    check: &dyn Fn() -> Result<()>,
) -> Result<(usize, Option<usize>, Vec<u64>)> {
    check()?;
    let matcher = search_matcher(&query.filter, check)?;
    let need_grep_filter = query.filter.only_grep_matched && !query.grep_config.is_empty();
    let is_grep_sort = matches!(
        query.sort.column,
        IntruderResultSortColumn::GrepMatch(_)
            | IntruderResultSortColumn::GrepExtract(_)
            | IntruderResultSortColumn::GrepPayload(_)
    );
    let need_grep = need_grep_filter || (is_grep_sort && !query.grep_config.is_empty());
    let pending: Vec<_> = recovery
        .values()
        .map(|row| {
            let (status, flow, response_bytes, elapsed_ms) = match &row.outcome {
                IntruderResultOutcome::Flow(summary) => (
                    summary.status,
                    Some(summary.id.to_string()),
                    summary.response_bytes,
                    Some(summary.elapsed_ms),
                ),
                IntruderResultOutcome::Rejected(_) => (None, None, 0, None),
            };
            (row.ordinal, status, flow, response_bytes, elapsed_ms)
        })
        .collect();
    let pending = serde_json::to_string(&pending)?;
    check()?;
    let baseline = run.baseline.map(|baseline| baseline.ordinal);
    let admitted = baseline.filter(|ordinal| *ordinal == run.dispatched);
    let enabled = query.filter.status_classes;
    let cte = "WITH recovery AS (SELECT json_extract(value,'$[0]') ordinal,json_extract(value,'$[1]') status,json_extract(value,'$[2]') flow,json_extract(value,'$[3]') response_bytes,json_extract(value,'$[4]') elapsed_ms FROM json_each(?2)),
        results AS (
            SELECT r.ordinal,json_extract(CAST(f.summary AS TEXT),'$.status') status,r.flow flow,COALESCE(json_extract(CAST(f.summary AS TEXT),'$.response_bytes'),0) response_bytes,
                CASE WHEN json_extract(CAST(f.summary AS TEXT),'$.status') IS NOT NULL THEN json_extract(CAST(f.summary AS TEXT),'$.elapsed_ms') END response_received,
                (r.notes IS NOT NULL AND length(r.notes)>0) has_notes,
                (r.highlight IS NOT NULL AND r.highlight>0) has_highlight
            FROM intruder_results r
            LEFT JOIN flows f ON f.id=r.flow WHERE r.run=?1 AND NOT EXISTS(SELECT 1 FROM recovery p WHERE p.ordinal=r.ordinal)
            UNION ALL SELECT ordinal,status,flow,response_bytes,CASE WHEN status IS NOT NULL THEN elapsed_ms END,0,0 FROM recovery
            UNION ALL SELECT ?4,NULL,NULL,0,NULL,0,0 WHERE ?4 IS NOT NULL),
        matching AS (SELECT ordinal,status,flow,response_bytes,response_received,CASE WHEN ordinal=?3 THEN 0 ELSE ordinal+1-(?3 IS NOT NULL AND ordinal>?3) END number FROM results
            WHERE (status IS NULL OR (?5 AND ?6 AND ?7 AND ?8)
                OR (status BETWEEN 200 AND 299 AND ?5) OR (status BETWEEN 300 AND 399 AND ?6)
                OR (status BETWEEN 400 AND 499 AND ?7) OR (status BETWEEN 500 AND 599 AND ?8))
                AND (?9=0 OR has_notes) AND (?10=0 OR has_highlight))";
    let parameters = params![
        run.id.to_string(),
        pending,
        baseline,
        admitted,
        enabled[0],
        enabled[1],
        enabled[2],
        enabled[3],
        query.filter.only_with_notes,
        query.filter.only_highlighted
    ];
    if matcher.is_some() || need_grep {
        let payload_order = matches!(query.sort.column, IntruderResultSortColumn::Payload)
            .then(|| result_view::payload_order(connection, run, check))
            .transpose()?;
        let mut statement = connection.prepare(&format!(
            "{cte} SELECT ordinal,status,number,flow,response_bytes,response_received FROM matching ORDER BY number ASC"
        ))?;
        let mut result = statement.query(parameters)?;
        let mut candidates = Vec::new();
        while let Some(row) = result.next()? {
            check()?;
            candidates.push(MatchedResult {
                ordinal: row.get(0)?,
                status: row.get(1)?,
                number: row.get(2)?,
                payload_index: None,
                flow: row.get(3)?,
                response_bytes: row.get(4)?,
                response_received: row.get(5)?,
                grep_matches: Vec::new(),
                grep_extracts: Vec::new(),
                grep_payload_hits: Vec::new(),
            });
        }
        drop(result);
        drop(statement);
        let mut payload_map: BTreeMap<u64, Vec<result_view::PayloadSummary>> = BTreeMap::new();
        if need_grep && query.grep_config.grep_payloads && !candidates.is_empty() {
            let all_ordinals: Vec<u64> = candidates.iter().map(|c| c.ordinal).collect();
            let mut idx = 0usize;
            while idx < all_ordinals.len() {
                let chunk = &all_ordinals[idx..(idx + 500).min(all_ordinals.len())];
                let batch = result_view::payload_summaries(connection, run, chunk, check)?;
                ensure!(!batch.is_empty(), "Intruder payload 单行超过分页限额");
                idx += batch.len();
                for row in batch {
                    payload_map.insert(row.ordinal, row.payloads);
                }
            }
        }
        let mut message_query = connection.prepare(FULL_MESSAGE_QUERY)?;
        let mut filtered = Vec::new();
        for mut candidate in candidates {
            check()?;
            if let Some(expected_status) = candidate.status {
                let raw = if let Some(&response) = recovery_responses.get(&candidate.ordinal) {
                    ensure!(
                        response.status == expected_status
                            && response.body.len() as u64 == candidate.response_bytes,
                        "Intruder 恢复响应状态或长度不一致"
                    );
                    crate::response_to_raw(response)
                } else {
                    let flow = candidate
                        .flow
                        .as_deref()
                        .context("Intruder 已完成响应缺少关联 flow")?;
                    read_search_response(
                        &mut message_query,
                        flow,
                        expected_status,
                        candidate.response_bytes,
                        check,
                    )?
                };
                check()?;
                if let Some(matcher) = &matcher {
                    let matched = matcher.is_match(&raw);
                    check()?;
                    if matched == query.filter.negative_search {
                        continue;
                    }
                }
                if need_grep {
                    let payload_slices: Vec<&[u8]> = payload_map
                        .get(&candidate.ordinal)
                        .map(|list| {
                            list.iter()
                                .map(|item| item.preview_bytes.as_slice())
                                .collect()
                        })
                        .unwrap_or_default();
                    let (grep_matches, grep_extracts, grep_payload_hits) =
                        crate::intruder::evaluate_grep_row(
                            &raw,
                            &query.grep_config,
                            &payload_slices,
                        );
                    if need_grep_filter
                        && grep_matches.is_empty()
                        && grep_extracts.is_empty()
                        && !grep_payload_hits.iter().any(|&hit| hit)
                    {
                        continue;
                    }
                    candidate.grep_matches = grep_matches;
                    candidate.grep_extracts = grep_extracts;
                    candidate.grep_payload_hits = grep_payload_hits;
                }
                check()?;
            } else {
                if matcher.is_some() && !query.filter.negative_search {
                    continue;
                }
                if need_grep_filter {
                    continue;
                }
            }
            if let Some(order) = &payload_order {
                candidate.payload_index = order.request_index(candidate.ordinal)?;
            }
            filtered.push(candidate);
        }
        drop(message_query);
        if query.sort.column == IntruderResultSortColumn::Length {
            let mut message_query = connection.prepare(HEAD_MESSAGE_QUERY)?;
            let mut lengths = Vec::with_capacity(filtered.len());
            for row in &filtered {
                check()?;
                let length = response_length(
                    recovery_responses,
                    row.ordinal,
                    row.status,
                    row.response_bytes,
                    row.flow.as_deref(),
                    &mut message_query,
                    check,
                )?;
                lengths.push((row.ordinal, row.number, length));
            }
            drop(message_query);
            sort_by_length(&mut lengths, query.sort.ascending, check)?;
            let ordinals = lengths.into_iter().map(|(ordinal, _, _)| ordinal).collect();
            return Ok(page_from_ordinals(query, ordinals));
        }
        sort_filtered_rows(
            &mut filtered,
            query.sort,
            &query.grep_config,
            payload_order.as_ref(),
            check,
        )?;
        let ordinals = filtered.into_iter().map(|row| row.ordinal).collect();
        return Ok(page_from_ordinals(query, ordinals));
    }
    if matches!(query.sort.column, IntruderResultSortColumn::Payload) {
        let order = result_view::payload_order(connection, run, check)?;
        let mut statement = connection.prepare(&format!(
            "{cte} SELECT ordinal,number FROM matching ORDER BY number ASC"
        ))?;
        let mut result = statement.query(parameters)?;
        let mut matching = Vec::new();
        while let Some(row) = result.next()? {
            check()?;
            let ordinal = row.get::<_, u64>(0)?;
            let number = row.get::<_, u64>(1)?;
            matching.push((ordinal, number, order.request_index(ordinal)?));
        }
        drop(result);
        drop(statement);
        let mut comparisons = 0usize;
        let mut aborted = None;
        matching.sort_by(|left, right| {
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
                .compare_indices(left.2, right.2, query.sort.ascending)
                .then_with(|| left.1.cmp(&right.1))
        });
        if let Some(error) = aborted {
            return Err(error);
        }
        check()?;
        let total = matching.len();
        let selected_rank = query.selected_ordinal.and_then(|selected| {
            matching
                .iter()
                .position(|(ordinal, _, _)| *ordinal == selected)
        });
        let ordinals = matching
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .map(|(ordinal, _, _)| ordinal)
            .collect();
        return Ok((total, selected_rank, ordinals));
    }
    if matches!(query.sort.column, IntruderResultSortColumn::Length) {
        let mut statement = connection.prepare(&format!(
            "{cte} SELECT ordinal,status,number,flow,response_bytes FROM matching ORDER BY number ASC"
        ))?;
        let mut result = statement.query(parameters)?;
        let mut matching = Vec::new();
        while let Some(row) = result.next()? {
            check()?;
            matching.push((
                row.get::<_, u64>(0)?,
                row.get::<_, Option<u16>>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, u64>(4)?,
            ));
        }
        drop(result);
        drop(statement);
        let mut message_query = connection.prepare(HEAD_MESSAGE_QUERY)?;
        let mut lengths = Vec::with_capacity(matching.len());
        for (ordinal, status, number, flow, response_bytes) in matching {
            check()?;
            let length = response_length(
                recovery_responses,
                ordinal,
                status,
                response_bytes,
                flow.as_deref(),
                &mut message_query,
                check,
            )?;
            lengths.push((ordinal, number, length));
        }
        drop(message_query);
        sort_by_length(&mut lengths, query.sort.ascending, check)?;
        let ordinals = lengths.into_iter().map(|(ordinal, _, _)| ordinal).collect();
        return Ok(page_from_ordinals(query, ordinals));
    }
    let total: usize = connection.query_row(
        &format!("{cte} SELECT count(*) FROM matching"),
        parameters,
        |row| row.get(0),
    )?;
    let direction = if query.sort.ascending { "ASC" } else { "DESC" };
    let order = match query.sort.column {
        IntruderResultSortColumn::Request => format!("number {direction}"),
        IntruderResultSortColumn::Status => format!("status {direction},number ASC"),
        IntruderResultSortColumn::ResponseReceived => {
            format!("response_received {direction},number ASC")
        }
        IntruderResultSortColumn::GrepMatch(_)
        | IntruderResultSortColumn::GrepExtract(_)
        | IntruderResultSortColumn::GrepPayload(_) => "number ASC".to_string(),
        IntruderResultSortColumn::Payload | IntruderResultSortColumn::Length => {
            unreachable!("handled above")
        }
    };
    let end = query
        .offset
        .checked_add(query.limit)
        .context("Intruder 分页位置溢出")?;
    let sql = format!(
        "{cte},ranked AS (SELECT ordinal,row_number() OVER(ORDER BY {order})-1 rank FROM matching)
        SELECT ordinal,rank FROM ranked WHERE (rank>=?11 AND rank<?12) OR ordinal=?13 ORDER BY rank"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut result = statement.query(params![
        run.id.to_string(),
        pending,
        baseline,
        admitted,
        enabled[0],
        enabled[1],
        enabled[2],
        enabled[3],
        query.filter.only_with_notes,
        query.filter.only_highlighted,
        query.offset,
        end,
        query.selected_ordinal
    ])?;
    let mut ordinals = Vec::with_capacity(query.limit);
    let mut selected_rank = None;
    while let Some(row) = result.next()? {
        check()?;
        let ordinal = row.get::<_, u64>(0)?;
        let rank = row.get::<_, usize>(1)?;
        if Some(ordinal) == query.selected_ordinal {
            selected_rank = Some(rank);
        }
        if (query.offset..end).contains(&rank) {
            ordinals.push(ordinal);
        }
    }
    Ok((total, selected_rank, ordinals))
}

fn search_matcher(
    filter: &crate::IntruderResultFilter,
    check: &dyn Fn() -> Result<()>,
) -> Result<Option<regex::bytes::Regex>> {
    check()?;
    if filter.search_term.is_empty() {
        return Ok(None);
    }
    ensure!(
        filter.search_term.len() <= MAX_SEARCH_TERM_BYTES,
        "Intruder 搜索词超过 4096 字节限额"
    );
    let pattern = if filter.regex {
        filter.search_term.clone()
    } else {
        regex::escape(&filter.search_term)
    };
    let matcher = regex::bytes::RegexBuilder::new(&pattern)
        .unicode(false)
        .case_insensitive(!filter.case_sensitive)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .nest_limit(128)
        .build()
        .map_err(|error| {
            anyhow::anyhow!(
                "Intruder 正则表达式无效或超出限制；当前不支持 Java lookaround / backreference 等语法：{error}"
            )
        })?;
    check()?;
    Ok(Some(matcher))
}

pub(super) fn read_search_response(
    statement: &mut rusqlite::Statement<'_>,
    flow: &str,
    expected_status: u16,
    expected_bytes: u64,
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    check()?;
    let row = statement
        .query_row(params![flow, MAX_SEARCH_HEAD_BYTES, MAX_BODY], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, usize>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
            ))
        })
        .optional()?
        .context("Intruder 结果响应消息缺失")?;
    ensure!(
        row.0 == "blob"
            && row.1 <= MAX_SEARCH_HEAD_BYTES
            && row.3 == "blob"
            && row.4 <= MAX_BODY as u64
            && row.4 == expected_bytes,
        "Intruder 结果响应消息类型或长度无效"
    );
    let head_bytes = row.2.context("Intruder 结果响应头超过 128 KiB 限额")?;
    let body = row.5.context("Intruder 结果响应正文超过 16 MiB 限额")?;
    ensure!(
        head_bytes.len() == row.1 && body.len() as u64 == expected_bytes,
        "Intruder 结果响应消息长度与摘要不一致"
    );
    let head: crate::store::OwnedResponseHead = retention::strict_json(&head_bytes)?;
    ensure!(
        head.stream.is_none() && head.status == expected_status,
        "Intruder 结果响应状态或流式元数据无效"
    );
    check()?;
    Ok(crate::response_to_raw(&crate::HttpResponse {
        status: head.status,
        version: head.version,
        reason_phrase: head.reason_phrase,
        headers: head.headers,
        trailers: head.trailers,
        body,
    }))
}

fn page_from_ordinals(
    query: &IntruderResultViewQuery,
    ordinals: Vec<u64>,
) -> (usize, Option<usize>, Vec<u64>) {
    let total = ordinals.len();
    let selected_rank = query
        .selected_ordinal
        .and_then(|selected| ordinals.iter().position(|ordinal| *ordinal == selected));
    let page = ordinals
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .collect();
    (total, selected_rank, page)
}
