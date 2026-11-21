#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use mio_proxy::{CapturedFlow, StreamedResponse};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use tempfile::{NamedTempFile, TempDir};
use tokio_util::sync::CancellationToken;

use crate::{
    ExtHttpHighlightChange, ExtHttpNotesChange, FlowId, FlowRecord, FlowSummary, HttpRequest,
    HttpResponse, IntruderRunSummary, OriginalMessages, Uuid, WebSocketMessage,
    WebSocketNotesChange, Workspace,
};

pub(crate) mod audit;
pub(crate) mod ext_archive;
mod ext_direct;
pub use ext_archive::{inspect_mio_ext_archive, read_mio_ext_websocket_fragment};
mod flows;
mod heads;
mod intruder;
mod lifecycle;
mod messages;
mod metadata;
mod plugin_storage;
pub(crate) mod provenance;
mod repeater_history;
mod resource_pools;
mod save;
mod schema_guard;
mod slim_save;
#[cfg(test)]
mod slim_save_tests;
pub(crate) mod startup_config;
#[cfg(test)]
mod streaming_tests;
pub(crate) mod target_site_map;
#[cfg(test)]
mod tests;
mod websocket;
mod websocket_notes;
#[cfg(test)]
mod workspace_tests;

use flows::{
    put_current_message, put_message, read_message, read_record, read_response_head,
    request_from_parts, response_from_parts, response_with_stream, state_code,
};
use heads::{OwnedRequestHead, OwnedResponseHead, RequestHead, ResponseHead};
pub(crate) use messages::validate_repeater_completion;
#[cfg(test)]
use messages::write_capture_with_port;
use messages::{
    finish_pending_records, write_record, write_repeater_completion,
};
use save::write_workspace;

const APPLICATION_ID: i32 = 0x4d494f31;
pub(in crate::store) const SCHEMA: i32 = 1;
const MAX_MESSAGE_HEAD_BYTES: usize = 4 * 1024 * 1024;
pub(super) const FLOW_LISTENER_PORT_COLUMN: &str = "listener_port INTEGER CHECK(listener_port IS NULL OR (typeof(listener_port)='integer' AND listener_port BETWEEN 1 AND 65535))";
pub(super) const FLOW_SOURCE_COLUMN: &str = "source TEXT CHECK(source IS NULL OR (typeof(source)='text' AND source IN ('Proxy','Repeater','Intruder','Scanner','Plugin')))";
pub(super) const FLOW_STATUS_COLUMN: &str = "status INTEGER CHECK(status IS NULL OR (typeof(status)='integer' AND status BETWEEN 100 AND 999))";
pub(super) const FLOW_RESPONSE_BYTES_COLUMN: &str = "response_bytes INTEGER CHECK(response_bytes IS NULL OR (typeof(response_bytes)='integer' AND response_bytes>=0))";
pub(super) const FLOW_ELAPSED_MS_COLUMN: &str = "elapsed_ms INTEGER CHECK(elapsed_ms IS NULL OR (typeof(elapsed_ms)='integer' AND elapsed_ms>=0))";
pub(super) const FLOW_CAPTURE_NUMBER_COLUMN: &str = "capture_number INTEGER CHECK(capture_number IS NULL OR (typeof(capture_number)='integer' AND capture_number>0))";
pub(super) const FLOW_RESPONSE_LENGTH_COLUMN: &str = "response_length INTEGER CHECK(response_length IS NULL OR (typeof(response_length)='integer' AND response_length>=0))";

type NotesSaveReceipt = (
    Vec<(Uuid, u64)>,
    Vec<(Uuid, Uuid, u64)>,
    Vec<(Uuid, Uuid, u64)>,
);
type ProjectSaveReceipt = (PathBuf, u64, NotesSaveReceipt);
type ProjectEdits<'a> = (
    Option<&'a Workspace>,
    &'a [WebSocketNotesChange],
    &'a [ExtHttpNotesChange],
    &'a [ExtHttpHighlightChange],
);

#[derive(Clone, Debug)]
pub(crate) struct CapturedWebSocket {
    pub record: Arc<WebSocketMessage>,
    pub listener_port: Option<u16>,
    pub original_payload: Option<Arc<Vec<u8>>>,
}

impl CapturedWebSocket {
    pub(crate) fn edited_original(&self) -> Option<&[u8]> {
        self.original_payload
            .as_deref()
            .filter(|original| original.as_slice() != self.record.payload.as_slice())
            .map(|original| original.as_slice())
    }
}

#[cfg(test)]
impl From<WebSocketMessage> for CapturedWebSocket {
    fn from(record: WebSocketMessage) -> Self {
        Self {
            record: record.into(),
            listener_port: None,
            original_payload: None,
        }
    }
}

pub(crate) enum IntruderMutation {
    Result {
        run: IntruderRunSummary,
        ordinal: u64,
        result: std::result::Result<Box<FlowRecord>, String>,
    },
    State {
        run: IntruderRunSummary,
    },
}

pub(crate) fn check_new_project_path(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut candidate = path.as_os_str().to_os_string();
        candidate.push(suffix);
        let candidate = PathBuf::from(candidate);
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => bail!(
                "保存目标或其 SQLite 关联文件已存在：{}；不会覆盖已有数据",
                candidate.display()
            ),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "不能确认新项目路径可用：{}；未发布项目",
                        candidate.display()
                    )
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn check_outside_owned_directory(path: &Path, owned: &Path) -> Result<()> {
    let owned = owned.canonicalize().context("不能确认待清理的临时目录")?;
    let input = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let input = match fs::symlink_metadata(&input) {
        Ok(_) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            input.parent().context("项目目标缺少父目录")?.to_owned()
        }
        Err(error) => return Err(error).context("不能确认项目目标归属"),
    };
    let mut pending = vec![input];
    let mut links = std::collections::HashSet::new();
    while let Some(path) = pending.pop() {
        // Resolving only the final target hides aliases that enter owned storage and leave it.
        for ancestor in path.ancestors() {
            let metadata = fs::symlink_metadata(ancestor).context("不能检查项目路径祖先")?;
            let resolved = ancestor.canonicalize().context("不能解析项目路径归属")?;
            ensure!(
                !resolved.starts_with(&owned),
                "项目目标经过待清理的临时目录；请选择其它目录"
            );
            if metadata.file_type().is_symlink() {
                let parent = ancestor
                    .parent()
                    .context("符号链接缺少父目录")?
                    .canonicalize()?;
                let link = parent.join(ancestor.file_name().context("符号链接缺少名称")?);
                if links.insert(link.clone()) {
                    ensure!(links.len() <= 64, "项目路径符号链接过多；未修改项目");
                    let target = fs::read_link(&link).context("不能读取项目路径符号链接")?;
                    pending.push(if target.is_absolute() {
                        target
                    } else {
                        parent.join(target)
                    });
                }
            }
        }
    }
    Ok(())
}

pub(in crate::store) struct WorkspaceCacheEntry {
    pub(in crate::store) data_version: i64,
    pub(in crate::store) schema_version: i64,
    pub(in crate::store) total_changes: i64,
    pub(in crate::store) workspace: Arc<Workspace>,
}

#[derive(Default)]
pub(in crate::store) struct StoreCache {
    pub(in crate::store) workspace: Option<WorkspaceCacheEntry>,
    pub(in crate::store) validated_schema: Option<(i64, i64)>,
}

pub(in crate::store) fn query_total_changes(conn: &Connection) -> Result<i64> {
    let mut stmt = conn.prepare_cached("SELECT total_changes()")?;
    Ok(stmt.query_row([], |row| row.get(0))?)
}

pub(in crate::store) fn ensure_schema_validated_in_tx(
    cache: &Mutex<StoreCache>,
    tx: &rusqlite::Transaction<'_>,
) -> Result<()> {
    let actual_schema: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let sv: i64 = tx.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    let dv: i64 = tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
    if actual_schema == SCHEMA
        && cache
            .lock()
            .ok()
            .is_some_and(|g| g.validated_schema == Some((sv, dv)))
    {
        return Ok(());
    }
    schema_guard::validate(tx)?;
    if actual_schema == SCHEMA
        && let Ok(mut g) = cache.lock()
    {
        g.validated_schema = Some((sv, dv));
    }
    Ok(())
}

impl ProjectStore {
    pub(in crate::store) fn advance_workspace_cache_changes(&self, before_tc: i64) -> Result<()> {
        let after_tc = query_total_changes(&self.connection)?;
        if after_tc != before_tc
            && let Ok(mut g) = self.cache.lock()
            && let Some(entry) = g.workspace.as_mut()
            && entry.total_changes == before_tc
        {
            entry.total_changes = after_tc;
        }
        Ok(())
    }
}

pub(crate) struct ProjectStore {
    cache: Mutex<StoreCache>,
    connection: Connection,
    pub id: Uuid,
    pub path: Option<PathBuf>,
    temporary: Option<TempDir>,
    intruder_sessions: intruder::IntruderSessions,
}
