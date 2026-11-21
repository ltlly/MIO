use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{IntruderMutation, ProjectStore};
use crate::{
    Draft, FlowRecord, FlowState, FlowSummary, HttpVersion, IntruderBaseline,
    IntruderDiscardReceipt, IntruderResultOutcome, IntruderResultPage, IntruderResultSummary,
    IntruderRetention, IntruderRunPage, IntruderRunState, IntruderRunSummary, IntruderSpec,
    ProjectKey, TabId, TrafficSource, Uuid, Workspace,
    http::MAX_BODY,
    intruder::{
        AttackType, MAX_PAYLOAD_BYTES, MAX_PAYLOAD_SETS, MAX_PAYLOADS, MAX_REQUEST_BYTES,
        MAX_REQUESTS, Position,
    },
};

mod discard_ops;
mod mutations;
mod query;
mod recovery_ops;
mod result_rows;
pub(crate) mod result_view;
mod retention;
mod run_codec;
mod schema;
mod store_results;
mod store_runs;
pub(in crate::store) use discard_ops::discard_project_run;
pub(super) use discard_ops::{ensure_not_discarded, validate_retirements};
pub(in crate::store::intruder) use discard_ops::{
    retired_run, retirement_key, run_cursor, shared_discard_flow,
};
pub(in crate::store::intruder) use mutations::{next_revision, rename_run, update_run};
pub(super) use mutations::{write_result, write_result_with_reason_phrase, write_state};
pub(super) use recovery_ops::write_recovery;
pub(in crate::store::intruder) use recovery_ops::{
    bind_resource_pool, ensure_no_pool_references, request_baseline, validate_pool_binding,
};
pub(in crate::store::intruder) use result_rows::*;
pub(super) use retention::{IntruderSessions, ensure_resource_pools_unreferenced};
#[cfg(test)]
pub(super) use retention::SessionWriteHook;
pub(in crate::store::intruder) use run_codec::*;
pub(in crate::store) use run_codec::{decode_run, run_select};
pub(super) use schema::create_schema;
pub(in crate::store::intruder) use schema::insert_intruder_run_with_settings;

pub(in crate::store) fn reassign_project_pool(
    transaction: &Transaction<'_>,
    from: Uuid,
    to: Uuid,
    cancel: &CancellationToken,
) -> Result<u64> {
    let mut query = transaction.prepare(&format!("{} WHERE pool_id=?1", run_select()))?;
    let mut rows = query.query([from.to_string()])?;
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
        let run = decode_run(row)?;
        ensure_not_discarded(transaction, run.id)?;
        count = count.checked_add(1).context("Intruder 任务计数溢出")?;
    }
    drop(rows);
    drop(query);
    if count != 0 {
        let changed = transaction.execute(
            "UPDATE intruder_runs SET pool_id=?1,pool_revision=pool_revision+1 WHERE pool_id=?2",
            params![to.to_string(), from.to_string()],
        )?;
        ensure!(changed as u64 == count, "Intruder 资源池改绑数量已改变");
    }
    Ok(count)
}

// Encoded metadata limits are separate from the smaller decoded input limits in Spec::validate.
const MAX_SPEC_META: usize = 512 * 1024;
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_TITLE_BYTES: usize = 4096;
const MAX_SUMMARY_BYTES: usize = 1024 * 1024;
const MAX_MESSAGE_HEAD: usize = 4 * 1024 * 1024;
const PAGE_BYTES: usize = MAX_SUMMARY_BYTES;
const DISCARD_BATCH: usize = 128;
// Keep the original lifecycle CHECK and table intact when migrating existing projects.
// A separate flag avoids rebuilding payload/result foreign keys or discarding extension columns.
const PAUSED_COLUMN: &str = "paused INTEGER NOT NULL DEFAULT 0
    CHECK(typeof(paused)='integer' AND paused IN (0,1) AND (paused=0 OR state=0))";
const TITLE_COLUMN: &str = "title TEXT NOT NULL DEFAULT ''
    CHECK(typeof(title)='text' AND length(CAST(title AS BLOB))<=4096)";
const TITLE_REVISION_COLUMN: &str = "title_revision INTEGER NOT NULL DEFAULT 0
    CHECK(typeof(title_revision)='integer' AND title_revision>=0)";
const BASELINE_COLUMN: &str = "baseline_ordinal INTEGER
    CHECK(baseline_ordinal IS NULL OR (typeof(baseline_ordinal)='integer' AND baseline_ordinal>=0 AND baseline_ordinal<total AND baseline_ordinal<=dispatched))";
const POOL_COLUMN: &str = "pool_id TEXT CHECK(pool_id IS NULL OR (typeof(pool_id)='text' AND length(CAST(pool_id AS BLOB))=36))";
const POOL_REVISION_COLUMN: &str = "pool_revision INTEGER NOT NULL DEFAULT 0 CHECK(typeof(pool_revision)='integer' AND pool_revision>=0 AND ((pool_id IS NULL AND pool_revision=0) OR (pool_id IS NOT NULL AND pool_revision>0)))";
const RESULT_NOTES_COLUMN: &str =
    "notes BLOB CHECK(notes IS NULL OR (typeof(notes)='blob' AND length(notes)<=65536))";
const RESULT_NOTES_REVISION_COLUMN: &str =
    "notes_revision INTEGER NOT NULL DEFAULT 0 CHECK(notes_revision>=0)";
const RESULT_HIGHLIGHT_COLUMN: &str = "highlight INTEGER CHECK(highlight IS NULL OR (typeof(highlight)='integer' AND highlight BETWEEN 0 AND 9))";
const RESULT_HIGHLIGHT_REVISION_COLUMN: &str =
    "highlight_revision INTEGER NOT NULL DEFAULT 0 CHECK(highlight_revision>=0)";

const SPEC_QUERY: &str =
    "SELECT CASE WHEN typeof(spec_meta)='blob' AND length(spec_meta)<=?2 THEN spec_meta END,
        CASE WHEN typeof(template)='blob' AND length(template)<=?3 THEN template END
     FROM intruder_runs WHERE id=?1";
const PAYLOAD_QUERY: &str = "SELECT set_index,ordinal,typeof(value),length(value),
        CASE WHEN typeof(value)='blob' AND length(value)<=?3 THEN value END
     FROM intruder_payloads WHERE run=?1 ORDER BY set_index,ordinal LIMIT ?2";

#[derive(Serialize, Deserialize)]
struct SpecMeta {
    draft_revision: u64,
    target: String,
    version: HttpVersion,
    attack_type: AttackType,
    positions: Vec<Position>,
    max_concurrent_requests: u16,
    request_timeout_ms: u64,
    update_host_header: bool,
    set_count: usize,
    #[serde(default = "legacy_save_to_project")]
    save_to_project: bool,
    #[serde(default)]
    baseline_request: bool,
    #[serde(default)]
    resource_pool: Option<crate::IntruderPoolSelection>,
    #[serde(default)]
    attack_settings: crate::IntruderAttackSettings,
    #[serde(default)]
    payload_configs: Vec<crate::IntruderPayloadSetConfig>,
}

fn legacy_save_to_project() -> bool {
    true
}

#[cfg(test)]
mod tests;
