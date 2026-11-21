use super::*;
use crate::{
    IntruderPayloadPreview, IntruderResultSortColumn, IntruderResultViewPage,
    IntruderResultViewQuery, IntruderResultViewRow,
};
use std::time::{Duration, Instant};

mod filters;
mod page;
mod validation;

pub(in crate::store::intruder) use filters::{search_matcher, page_from_ordinals};
use filters::matching_ordinals;
pub(in crate::store::intruder) use page::{sort_by_length, sort_filtered_rows, view_row_bytes};
use page::view_row;
pub(in crate::store::intruder) use validation::{
    check_query, validate_recovery_responses, validate_run_overlay,
};
use validation::{QueryScope, validate_recovery, validate_results};

pub(in crate::store::intruder) const QUERY_TIME: Duration = Duration::from_secs(1);
const MAX_SEARCH_TERM_BYTES: usize = 4096;
pub(in crate::store::intruder) const MAX_SEARCH_HEAD_BYTES: usize = 128 * 1024;
const HEAD_MESSAGE_QUERY: &str = "SELECT typeof(head),length(head),
    CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END
 FROM messages WHERE flow=?1 AND side=1";
const FULL_MESSAGE_QUERY: &str = "SELECT typeof(head),length(head),
    CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END,
    typeof(body),length(body),
    CASE WHEN typeof(body)='blob' AND length(body)<=?3 THEN body END
 FROM messages WHERE flow=?1 AND side=1";
type ValidateSession<'a> = Option<&'a dyn Fn(&IntruderResultSummary) -> Result<()>>;

pub(in crate::store::intruder) struct MatchedResult {
    pub ordinal: u64,
    pub status: Option<u16>,
    pub number: u64,
    pub payload_index: Option<usize>,
    pub flow: Option<String>,
    pub response_bytes: u64,
    pub response_received: Option<u64>,
    pub grep_matches: Vec<String>,
    pub grep_extracts: Vec<(String, String)>,
    pub grep_payload_hits: Vec<bool>,
}

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::IntruderRepo<S> {
    #[cfg(test)]
    pub(crate) fn query_result_view(
        &self,
        query: &IntruderResultViewQuery,
        latest: Option<&IntruderRunSummary>,
        recovery: &[IntruderResultSummary],
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
    ) -> Result<IntruderResultViewPage> {
        self.query_result_view_with_responses(
            query,
            latest,
            recovery,
            &BTreeMap::new(),
            cancel,
            shutdown,
        )
    }

    pub(crate) fn query_result_view_with_responses(
        &self,
        query: &IntruderResultViewQuery,
        latest: Option<&IntruderRunSummary>,
        recovery: &[IntruderResultSummary],
        recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
    ) -> Result<IntruderResultViewPage> {
        let deadline = Instant::now() + QUERY_TIME;
        let check = || check_query(deadline, cancel, shutdown);
        check()?;
        ensure!(
            !query.project.id.is_nil()
                && query.project.generation > 0
                && !query.run_id.is_nil()
                && !query.view_id.is_nil()
                && query.query_revision > 0
                && (1..=500).contains(&query.limit)
                && query.offset <= MAX_REQUESTS as usize,
            "Intruder 结果查询身份或分页无效"
        );
        self.check_session_owner(query.project, query.run_id)?;
        if let Some(dataset) = self.0.intruder().session_dataset(query.run_id)? {
            return self.with_session_query(query.run_id, |validate| {
                dataset.data.query_result_view(
                    query,
                    latest,
                    recovery,
                    recovery_responses,
                    validate,
                    cancel,
                    shutdown,
                    #[cfg(test)]
                    &dataset.body_reads,
                )
            });
        }
        let _main_scope = QueryScope::new(
            &self.0.connection,
            deadline,
            cancel,
            shutdown,
            Some(shutdown.clone()),
        )?;
        let read = |validate_session: ValidateSession<'_>| {
            check()?;
            let transaction = self.0.connection.unchecked_transaction()?;
            ensure!(
                crate::store::metadata::project_id(&transaction)? == query.project.id,
                "Intruder 项目归属已改变"
            );
            let stored = read_run(&transaction, query.run_id)?;
            let run = latest.unwrap_or(&stored);
            validate_run_overlay(&stored, run)?;
            ensure!(
                query
                    .selected_ordinal
                    .is_none_or(|ordinal| ordinal < run.total),
                "Intruder 选中结果序号超出计划"
            );
            let recovery = validate_recovery(&transaction, run, recovery, &check)?;
            validate_recovery_responses(&recovery, recovery_responses, &check)?;
            if let Some(validate_session) = validate_session {
                for result in recovery.values() {
                    check()?;
                    validate_session(result)?;
                }
            }
            validate_results(
                &transaction,
                &stored,
                run,
                &recovery,
                validate_session,
                &check,
            )?;
            let (total, selected_rank, ordinals) = matching_ordinals(
                &transaction,
                query,
                run,
                &recovery,
                recovery_responses,
                &check,
            )?;
            check()?;
            let mut requested = Vec::with_capacity(ordinals.len() + 1);
            if selected_rank.is_some() {
                requested.push(query.selected_ordinal.expect("matching selection"));
            }
            requested.extend(ordinals.iter().copied().filter(|ordinal| {
                selected_rank.is_none() || Some(*ordinal) != query.selected_ordinal
            }));
            let mut payloads = if requested.is_empty() {
                BTreeMap::new()
            } else {
                result_view::payload_summaries(&transaction, run, &requested, &check)?
                    .into_iter()
                    .map(|row| (row.ordinal, row.payloads))
                    .collect()
            };
            let mut bytes = serde_json::to_vec(run)?.len() + 128;
            ensure!(bytes < PAGE_BYTES, "Intruder 运行摘要超过分页限额");
            let mut head_query = transaction.prepare(HEAD_MESSAGE_QUERY)?;
            let mut full_query = (!query.grep_config.is_empty())
                .then(|| transaction.prepare(FULL_MESSAGE_QUERY))
                .transpose()?;
            let mut selected_row = None;
            if selected_rank.is_some() {
                let ordinal = query.selected_ordinal.expect("matching selection");
                let row = view_row(
                    &transaction,
                    run,
                    (&recovery, recovery_responses),
                    ordinal,
                    &mut payloads,
                    &query.grep_config,
                    &mut head_query,
                    full_query.as_mut(),
                    &check,
                )?
                .context("Intruder 选中结果 payload 超过分页限额")?;
                bytes = bytes
                    .checked_add(view_row_bytes(&row)?)
                    .context("Intruder 分页长度溢出")?;
                ensure!(bytes <= PAGE_BYTES, "Intruder 选中结果超过 1 MiB 分页限额");
                selected_row = Some(row);
            }
            let mut rows = Vec::with_capacity(ordinals.len());
            for ordinal in ordinals {
                check()?;
                if selected_row
                    .as_ref()
                    .is_some_and(|row| row.ordinal == ordinal)
                {
                    rows.push(selected_row.take().expect("selected row"));
                    continue;
                }
                let Some(row) = view_row(
                    &transaction,
                    run,
                    (&recovery, recovery_responses),
                    ordinal,
                    &mut payloads,
                    &query.grep_config,
                    &mut head_query,
                    full_query.as_mut(),
                    &check,
                )?
                else {
                    break;
                };
                let row_bytes = view_row_bytes(&row)?;
                ensure!(
                    row_bytes <= PAGE_BYTES,
                    "Intruder 单条结果超过 1 MiB 分页限额"
                );
                if row_bytes > PAGE_BYTES - bytes {
                    break;
                }
                bytes += row_bytes;
                rows.push(row);
            }
            ensure!(
                !rows.is_empty() || query.offset >= total,
                "Intruder 当前页与选中结果合计超过 1 MiB；未返回不完整结果"
            );
            let next = query
                .offset
                .checked_add(rows.len())
                .context("Intruder 分页位置溢出")?;
            drop(full_query);
            drop(head_query);
            check()?;
            transaction.commit()?;
            Ok(IntruderResultViewPage {
                run: run.clone(),
                offset: query.offset,
                total,
                rows,
                next_offset: (next < total).then_some(next),
                selected_rank,
                selected_row,
                persisted: recovery.is_empty() && run == &stored,
            })
        };
        read(None)
    }
}

#[cfg(test)]
mod tests;
