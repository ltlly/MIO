use super::*;
use crate::{
    IntruderPayloadPreview, IntruderResultSortColumn, IntruderResultViewPage,
    IntruderResultViewQuery, IntruderResultViewRow,
};
use std::time::{Duration, Instant};

mod filters;
mod page;
mod validation;

use filters::matching_ordinals;
use page::{view_row, view_row_bytes};
use validation::{
    QueryScope, check_query, validate_recovery, validate_recovery_responses, validate_results,
    validate_run_overlay,
};

const QUERY_TIME: Duration = Duration::from_secs(1);
const MAX_SEARCH_TERM_BYTES: usize = 4096;
const MAX_SEARCH_HEAD_BYTES: usize = 128 * 1024;
const HEAD_MESSAGE_QUERY: &str = "SELECT typeof(head),length(head),
    CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END
 FROM messages WHERE flow=?1 AND side=1";
const FULL_MESSAGE_QUERY: &str = "SELECT typeof(head),length(head),
    CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END,
    typeof(body),length(body),
    CASE WHEN typeof(body)='blob' AND length(body)<=?3 THEN body END
 FROM messages WHERE flow=?1 AND side=1";
type ValidateSession<'a> = Option<&'a dyn Fn(&IntruderResultSummary) -> Result<()>>;

struct MatchedResult {
    ordinal: u64,
    status: Option<u16>,
    number: u64,
    payload_index: Option<usize>,
    flow: Option<String>,
    response_bytes: u64,
    response_received: Option<u64>,
    grep_matches: Vec<String>,
    grep_extracts: Vec<(String, String)>,
    grep_payload_hits: Vec<bool>,
}

impl ProjectStore {
    #[cfg(test)]
    pub(crate) fn query_intruder_result_view(
        &self,
        query: &IntruderResultViewQuery,
        latest: Option<&IntruderRunSummary>,
        recovery: &[IntruderResultSummary],
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
    ) -> Result<IntruderResultViewPage> {
        self.query_intruder_result_view_with_responses(
            query,
            latest,
            recovery,
            &BTreeMap::new(),
            cancel,
            shutdown,
        )
    }

    pub(crate) fn query_intruder_result_view_with_responses(
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
        self.check_intruder_session_owner(query.project, query.run_id)?;
        let _main_scope = QueryScope::new(
            &self.connection,
            deadline,
            cancel,
            shutdown,
            Some(shutdown.clone()),
        )?;
        let (connection, retention) = self.intruder_connection(query.run_id)?;
        let _session_scope = (retention == IntruderRetention::Session)
            .then(|| QueryScope::new(connection, deadline, cancel, shutdown, None))
            .transpose()?;
        let read = |validate_session: ValidateSession<'_>| {
            check()?;
            let transaction = connection.unchecked_transaction()?;
            if retention == IntruderRetention::Project {
                ensure!(
                    crate::store::metadata::project_id(&transaction)? == query.project.id,
                    "Intruder 项目归属已改变"
                );
            }
            let mut stored = read_run(&transaction, query.run_id)?;
            stored.retention = retention;
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
                persisted: retention == IntruderRetention::Project
                    && recovery.is_empty()
                    && run == &stored,
            })
        };
        if retention == IntruderRetention::Session {
            self.with_session_intruder_query(query.run_id, |validate| read(Some(validate)))
        } else {
            read(None)
        }
    }
}

#[cfg(test)]
mod tests;
