use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{ProjectStore, ext_archive, metadata, schema_guard, write_record};
use crate::{
    ExtMessageSummary, ExtResponseRef, Draft, FlowId, FlowRecord, FlowState, FlowSummary,
    HttpVersion, RepeaterHistory, RepeaterHistoryCursor, RepeaterHistoryEntry,
    RepeaterHistoryEntryId, RepeaterHistoryPage, RepeaterHistorySource, TabId, TrafficSource, Uuid,
    WorkTab, Workspace, http::MAX_BODY,
};

const MAX_META: usize = 512 * 1024;
const MAX_RAW: usize = MAX_BODY + 256 * 1024;
const PAGE_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch("CREATE TABLE repeater_history (tab TEXT NOT NULL, archive_boundary INTEGER NOT NULL CHECK(archive_boundary>=0), ordinal INTEGER NOT NULL CHECK(ordinal>=0), flow TEXT NOT NULL UNIQUE REFERENCES flows(id) ON DELETE RESTRICT, draft_meta BLOB NOT NULL CHECK(typeof(draft_meta)='blob' AND length(draft_meta)<=524288), raw BLOB NOT NULL CHECK(typeof(raw)='blob' AND length(raw)<=17039360), PRIMARY KEY(tab,archive_boundary,ordinal)) WITHOUT ROWID;")?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftMeta {
    revision: u64,
    target: String,
    version: HttpVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    send_cursor: Option<RepeaterHistoryCursor>,
}

struct Live {
    boundary: u64,
    ordinal: u64,
    draft: DraftMeta,
    raw_bytes: usize,
}

struct Archive {
    source_id: Uuid,
    collection: String,
    length: u64,
    source_bytes: u64,
    pair_context: &'static str,
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "Repeater 历史操作已取消");
    Ok(())
}

mod archive;
mod ops;
mod validate;

#[cfg(test)]
pub(in crate::store) use archive::source_for_parent;
pub(super) use validate::{validate_change, validate_workspace};

pub(in crate::store) fn insert_imported_history_entry(
    tx: &rusqlite::Transaction<'_>,
    tab_id: TabId,
    ordinal: u64,
    flow_id: FlowId,
    target: &str,
    version: HttpVersion,
    raw: &[u8],
) -> Result<()> {
    ensure!(
        !raw.is_empty() && raw.len() <= MAX_RAW,
        "Repeater 历史原始请求大小超出范围"
    );
    let meta = serde_json::to_vec(&DraftMeta {
        revision: 1,
        target: target.to_owned(),
        version,
        send_cursor: None,
    })?;
    ensure!(meta.len() <= MAX_META, "Repeater 历史元数据超过限额");
    tx.execute(
        "INSERT INTO main.repeater_history(tab,archive_boundary,ordinal,flow,draft_meta,raw) VALUES(?1,0,?2,?3,?4,?5)",
        params![
            tab_id.to_string(),
            i64::try_from(ordinal)?,
            flow_id.to_string(),
            meta,
            raw,
        ],
    )?;
    Ok(())
}

use archive::{archive_entry, archive_for_tab, archive_request, entry_content};
use validate::{boundary_count, entry_position, flow_diagnostic, flow_summary, live_meta, locate};

#[cfg(test)]
mod tests;
