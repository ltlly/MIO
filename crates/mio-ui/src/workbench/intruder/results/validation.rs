use mio_core::{
    IntruderResultOutcome, IntruderResultViewPage, IntruderResultViewQuery, IntruderResultViewRow,
};

use super::*;

pub(super) fn validate_result_view_page(
    query: &IntruderResultViewQuery,
    page: &IntruderResultViewPage,
) -> Result<(), String> {
    if page.run.id != query.run_id
        || page.offset != query.offset
        || page.rows.len() > query.limit
        || page.rows.len() > page.total.saturating_sub(page.offset)
        || page.next_offset
            != (page.offset + page.rows.len() < page.total).then_some(page.offset + page.rows.len())
        || page.rows.is_empty() && page.offset < page.total
        || result_page_bytes(page) > MAX_RESULT_VIEW_BYTES
    {
        return Err("Intruder 结果页的数量、偏移或运行身份不一致。".into());
    }
    let mut seen = HashSet::new();
    for row in &page.rows {
        if !seen.insert(row.ordinal) {
            return Err("Intruder 结果页含重复 ordinal。".into());
        }
        validate_result_view_row(&page.run, row)?;
    }
    match (
        &page.selected_row,
        page.selected_rank,
        query.selected_ordinal,
    ) {
        (None, None, _) => {}
        (None, Some(rank), Some(ordinal))
            if rank < page.total
                && (page.offset..page.offset + page.rows.len()).contains(&rank)
                && page.rows[rank - page.offset].ordinal == ordinal => {}
        (Some(row), Some(rank), Some(ordinal))
            if row.ordinal == ordinal
                && rank < page.total
                && !(page.offset..page.offset + page.rows.len()).contains(&rank) =>
        {
            validate_result_view_row(&page.run, row)?;
        }
        _ => return Err("Intruder 所选结果回执缺失或身份不一致。".into()),
    }
    Ok(())
}

pub(super) fn result_page_bytes(page: &IntruderResultViewPage) -> usize {
    page.rows
        .iter()
        .chain(page.selected_row.iter())
        .fold(0usize, |bytes, row| {
            bytes.saturating_add(result_row_bytes(row))
        })
}

pub(super) fn result_row_bytes(row: &IntruderResultViewRow) -> usize {
    let summary = match row.result.as_ref().map(|result| &result.outcome) {
        Some(IntruderResultOutcome::Flow(flow)) => {
            let state = match &flow.state {
                mio_core::FlowState::Failed(error) => error.len(),
                _ => 0,
            };
            let source = match &flow.source {
                mio_core::TrafficSource::Plugin(name) => name.len(),
                _ => 0,
            };
            512usize
                .saturating_add(flow.method.len())
                .saturating_add(flow.url.len())
                .saturating_add(state)
                .saturating_add(source)
        }
        Some(IntruderResultOutcome::Rejected(error)) => 64usize.saturating_add(error.len()),
        None => 64,
    }
    .saturating_add(row.notes_preview.len());
    let summary = row.grep_matches.iter().fold(summary, |bytes, matched| {
        bytes.saturating_add(matched.len())
    });
    let summary = row
        .grep_extracts
        .iter()
        .fold(summary, |bytes, (key, value)| {
            bytes.saturating_add(key.len()).saturating_add(value.len())
        });
    row.payloads.iter().fold(summary, |bytes, payload| {
        bytes
            .saturating_add(payload.prefix.len())
            .saturating_add(64)
    })
}

fn validate_result_view_row(
    run: &IntruderRunSummary,
    row: &IntruderResultViewRow,
) -> Result<(), String> {
    if run.request_number(row.ordinal).is_none()
        || row
            .result
            .as_ref()
            .is_some_and(|result| result.ordinal != row.ordinal)
        || row.result.is_none()
            && !run.baseline.is_some_and(|baseline| {
                baseline.ordinal == row.ordinal
                    && !baseline.completed
                    && row.ordinal == run.dispatched
            })
        || run
            .baseline
            .is_some_and(|baseline| baseline.ordinal == row.ordinal)
            && !row.payloads.is_empty()
        || row.payloads.len() > mio_core::intruder::MAX_PAYLOAD_SETS
        || row.highlight.is_some_and(|color| !(1..=9).contains(&color))
        || row.notes_revision > i64::MAX as u64
        || row.highlight_revision > i64::MAX as u64
        || row.notes_preview.len() > 768
    {
        return Err("Intruder 结果或基线身份不一致。".into());
    }
    let mut sets = HashSet::new();
    for payload in &row.payloads {
        if !sets.insert(payload.set_index)
            || payload.prefix.len() > 1024
            || payload.prefix.len() as u64 > payload.raw_bytes
        {
            return Err("Intruder payload 预览不完整或超过边界。".into());
        }
    }
    Ok(())
}

pub(in crate::workbench::intruder) fn same_run(
    before: &IntruderRunSummary,
    after: &IntruderRunSummary,
) -> bool {
    before.id == after.id
        && before.tab_id == after.tab_id
        && before.spec_revision == after.spec_revision
}

pub(in crate::workbench::intruder) fn merge_run_summary(
    before: &mut IntruderRunSummary,
    incoming: &IntruderRunSummary,
) {
    if !same_run(before, incoming) {
        return;
    }
    let title = if incoming.title_revision >= before.title_revision {
        (incoming.title.clone(), incoming.title_revision)
    } else {
        (before.title.clone(), before.title_revision)
    };
    let valid_pool = match incoming.resource_pool {
        Some(id) => {
            !id.is_nil() && (1..=i64::MAX as u64).contains(&incoming.resource_pool_revision)
        }
        None => incoming.resource_pool_revision == 0,
    };
    let pool = if valid_pool && incoming.resource_pool_revision > before.resource_pool_revision {
        (incoming.resource_pool, incoming.resource_pool_revision)
    } else {
        (before.resource_pool, before.resource_pool_revision)
    };
    if incoming.revision >= before.revision {
        *before = incoming.clone();
    }
    (before.title, before.title_revision) = title;
    (before.resource_pool, before.resource_pool_revision) = pool;
}
