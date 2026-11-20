#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
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
    put_current_message, put_message, read_message, read_original, read_record, read_response_head,
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
// Version 27 denormalizes query/sort columns onto `flows` with indexed `source` filtering.
pub(in crate::store) const SCHEMA: i32 = 27;
const MAX_MESSAGE_HEAD_BYTES: usize = 4 * 1024 * 1024;
pub(super) const FLOW_LISTENER_PORT_COLUMN: &str = "listener_port INTEGER CHECK(listener_port IS NULL OR (typeof(listener_port)='integer' AND listener_port BETWEEN 1 AND 65535))";
pub(super) const FLOW_SOURCE_COLUMN: &str = "source TEXT CHECK(source IS NULL OR (typeof(source)='text' AND source IN ('Proxy','Repeater','Intruder','Scanner','Plugin')))";
pub(super) const FLOW_STATUS_COLUMN: &str = "status INTEGER CHECK(status IS NULL OR (typeof(status)='integer' AND status BETWEEN 100 AND 999))";
pub(super) const FLOW_RESPONSE_BYTES_COLUMN: &str = "response_bytes INTEGER CHECK(response_bytes IS NULL OR (typeof(response_bytes)='integer' AND response_bytes>=0))";
pub(super) const FLOW_ELAPSED_MS_COLUMN: &str = "elapsed_ms INTEGER CHECK(elapsed_ms IS NULL OR (typeof(elapsed_ms)='integer' AND elapsed_ms>=0))";
pub(super) const FLOW_CAPTURE_NUMBER_COLUMN: &str = "capture_number INTEGER CHECK(capture_number IS NULL OR (typeof(capture_number)='integer' AND capture_number>0))";
pub(super) const FLOW_RESPONSE_LENGTH_COLUMN: &str = "response_length INTEGER CHECK(response_length IS NULL OR (typeof(response_length)='integer' AND response_length>=0))";
pub(super) const FLOW_DENORM_COLUMN_NAMES: [&str; 6] = [
    "source",
    "status",
    "response_bytes",
    "elapsed_ms",
    "capture_number",
    "response_length",
];

pub(in crate::store) fn add_flow_denorm_columns(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "ALTER TABLE flows ADD COLUMN {FLOW_SOURCE_COLUMN};
         ALTER TABLE flows ADD COLUMN {FLOW_STATUS_COLUMN};
         ALTER TABLE flows ADD COLUMN {FLOW_RESPONSE_BYTES_COLUMN};
         ALTER TABLE flows ADD COLUMN {FLOW_ELAPSED_MS_COLUMN};
         ALTER TABLE flows ADD COLUMN {FLOW_CAPTURE_NUMBER_COLUMN};
         ALTER TABLE flows ADD COLUMN {FLOW_RESPONSE_LENGTH_COLUMN};
         CREATE INDEX flows_source_started ON flows(source, started DESC, id);
         CREATE INDEX flows_source_status ON flows(source, status);
         CREATE INDEX flows_source_capture ON flows(source, capture_number);"
    ))?;
    Ok(())
}

pub(super) fn backfill_flow_denorm_columns(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "UPDATE flows SET
            source = CASE
                WHEN json_type(CAST(summary AS TEXT),'$.source')='text'
                    AND json_extract(CAST(summary AS TEXT),'$.source') IN ('Proxy','Repeater','Intruder','Scanner')
                    THEN json_extract(CAST(summary AS TEXT),'$.source')
                WHEN json_type(CAST(summary AS TEXT),'$.source')='object'
                    AND json_type(CAST(summary AS TEXT),'$.source.Plugin')='text'
                    THEN 'Plugin'
                ELSE NULL
            END,
            status = CASE
                WHEN json_type(CAST(summary AS TEXT),'$.status')='integer'
                    AND typeof(json_extract(CAST(summary AS TEXT),'$.status'))='integer'
                    AND json_extract(CAST(summary AS TEXT),'$.status') BETWEEN 100 AND 999
                    THEN json_extract(CAST(summary AS TEXT),'$.status')
                ELSE NULL
            END,
            response_bytes = CASE
                WHEN json_type(CAST(summary AS TEXT),'$.response_bytes')='integer'
                    AND typeof(json_extract(CAST(summary AS TEXT),'$.response_bytes'))='integer'
                    AND json_extract(CAST(summary AS TEXT),'$.response_bytes')>=0
                    THEN json_extract(CAST(summary AS TEXT),'$.response_bytes')
                ELSE NULL
            END,
            elapsed_ms = CASE
                WHEN json_type(CAST(summary AS TEXT),'$.elapsed_ms')='integer'
                    AND typeof(json_extract(CAST(summary AS TEXT),'$.elapsed_ms'))='integer'
                    AND json_extract(CAST(summary AS TEXT),'$.elapsed_ms')>=0
                    THEN json_extract(CAST(summary AS TEXT),'$.elapsed_ms')
                ELSE NULL
            END,
            capture_number = COALESCE(
                (SELECT CASE
                    WHEN typeof(m.value)='blob' AND length(m.value)<=262144 AND json_valid(CAST(m.value AS TEXT))
                        AND json_type(CAST(m.value AS TEXT),'$.metadata.capture_number')='integer'
                        AND typeof(json_extract(CAST(m.value AS TEXT),'$.metadata.capture_number'))='integer'
                        AND json_extract(CAST(m.value AS TEXT),'$.metadata.capture_number')>0
                        THEN json_extract(CAST(m.value AS TEXT),'$.metadata.capture_number')
                 END FROM main.metadata m WHERE m.key=('http_history_meta:'||flows.id) COLLATE BINARY),
                (SELECT CASE
                    WHEN typeof(msg.head)='blob' AND length(msg.head)<=4194304 AND json_valid(CAST(msg.head AS TEXT))
                        AND json_type(CAST(msg.head AS TEXT),'$.capture_number')='integer'
                        AND typeof(json_extract(CAST(msg.head AS TEXT),'$.capture_number'))='integer'
                        AND json_extract(CAST(msg.head AS TEXT),'$.capture_number')>0
                        THEN json_extract(CAST(msg.head AS TEXT),'$.capture_number')
                 END FROM main.messages msg WHERE msg.flow=flows.id AND msg.side=2),
                (SELECT CASE
                    WHEN typeof(msg.head)='blob' AND length(msg.head)<=4194304 AND json_valid(CAST(msg.head AS TEXT))
                        AND json_type(CAST(msg.head AS TEXT),'$.capture_number')='integer'
                        AND typeof(json_extract(CAST(msg.head AS TEXT),'$.capture_number'))='integer'
                        AND json_extract(CAST(msg.head AS TEXT),'$.capture_number')>0
                        THEN json_extract(CAST(msg.head AS TEXT),'$.capture_number')
                 END FROM main.messages msg WHERE msg.flow=flows.id AND msg.side=0)
            ),
            response_length = (
                SELECT CASE
                    WHEN typeof(m.value)='blob' AND length(m.value)<=262144 AND json_valid(CAST(m.value AS TEXT))
                        AND json_type(CAST(m.value AS TEXT),'$.metadata.response_length')='integer'
                        AND typeof(json_extract(CAST(m.value AS TEXT),'$.metadata.response_length'))='integer'
                        AND json_extract(CAST(m.value AS TEXT),'$.metadata.response_length')>=0
                        THEN json_extract(CAST(m.value AS TEXT),'$.metadata.response_length')
                END FROM main.metadata m WHERE m.key=('http_history_meta:'||flows.id) COLLATE BINARY
            )
        WHERE typeof(summary)='blob' AND length(summary)<=1048576
          AND json_valid(CAST(summary AS TEXT))
          AND json_type(CAST(summary AS TEXT))='object';"
    )?;
    Ok(())
}

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
pub(in crate::store) struct ConnCacheState {
    pub(in crate::store) workspace: Option<WorkspaceCacheEntry>,
    pub(in crate::store) validated_schema: Option<(i32, i64, i64)>,
    pub(in crate::store) validated_metadata_value_deps_sv: Option<i64>,
    pub(in crate::store) validated_site_map_deps_sv: Option<i64>,
}

static CONN_CACHE_REGISTRY: LazyLock<Mutex<HashMap<usize, Arc<Mutex<ConnCacheState>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(in crate::store) struct CachedWorkspace {
    conn_key: usize,
    pub(in crate::store) state: Arc<Mutex<ConnCacheState>>,
}

impl Drop for CachedWorkspace {
    fn drop(&mut self) {
        if let Ok(mut reg) = CONN_CACHE_REGISTRY.lock()
            && let Some(existing) = reg.get(&self.conn_key)
            && Arc::ptr_eq(existing, &self.state)
        {
            reg.remove(&self.conn_key);
        }
    }
}

pub(in crate::store) fn query_total_changes(conn: &Connection) -> Result<i64> {
    let mut stmt = conn.prepare_cached("SELECT total_changes()")?;
    Ok(stmt.query_row([], |row| row.get(0))?)
}

pub(in crate::store) fn conn_cache_for(
    cache_mutex: &Mutex<Option<CachedWorkspace>>,
    conn: &Connection,
) -> Arc<Mutex<ConnCacheState>> {
    let mut guard = cache_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = guard.as_ref() {
        return Arc::clone(&existing.state);
    }
    // SAFETY: Only reads the opaque pointer address as a map key; never dereferences it.
    // `CachedWorkspace::drop` removes this entry before `Connection` is dropped/closed.
    let conn_key = unsafe { conn.handle() } as usize;
    let state = Arc::new(Mutex::new(ConnCacheState::default()));
    {
        let mut reg = CONN_CACHE_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reg.insert(conn_key, Arc::clone(&state));
    }
    *guard = Some(CachedWorkspace {
        conn_key,
        state: Arc::clone(&state),
    });
    state
}

pub(in crate::store) fn lookup_conn_cache(conn: &Connection) -> Option<Arc<Mutex<ConnCacheState>>> {
    // SAFETY: Only reads the opaque pointer address as a map lookup key.
    let conn_key = unsafe { conn.handle() } as usize;
    CONN_CACHE_REGISTRY
        .lock()
        .ok()
        .and_then(|reg| reg.get(&conn_key).cloned())
}

pub(in crate::store) fn ensure_schema_validated_in_tx(
    cache_mutex: &Mutex<Option<CachedWorkspace>>,
    tx: &rusqlite::Transaction<'_>,
    schema: i32,
) -> Result<()> {
    let actual_schema: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let sv: i64 = tx.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    let dv: i64 = tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
    let state = conn_cache_for(cache_mutex, tx);
    if actual_schema == schema
        && state
            .lock()
            .ok()
            .is_some_and(|g| g.validated_schema == Some((schema, sv, dv)))
    {
        return Ok(());
    }
    schema_guard::validate(tx, schema)?;
    if actual_schema == schema
        && let Ok(mut g) = state.lock()
    {
        g.validated_schema = Some((schema, sv, dv));
    }
    Ok(())
}

pub(in crate::store) fn validate_metadata_value_update_deps(
    tx: &rusqlite::Transaction<'_>,
    cancel: &CancellationToken,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "项目设置更新已取消");
    if let Some(state) = lookup_conn_cache(tx) {
        let sv: i64 = tx.pragma_query_value(None, "schema_version", |row| row.get(0))?;
        if state
            .lock()
            .ok()
            .is_some_and(|g| g.validated_metadata_value_deps_sv == Some(sv))
        {
            return Ok(());
        }
        schema_guard::validate_update_dependencies(tx, "metadata", &["value"], cancel)?;
        if let Ok(mut g) = state.lock() {
            g.validated_metadata_value_deps_sv = Some(sv);
        }
        return Ok(());
    }
    schema_guard::validate_update_dependencies(tx, "metadata", &["value"], cancel)
}

pub(in crate::store) fn validate_site_map_update_deps(
    cache_mutex: &Mutex<Option<CachedWorkspace>>,
    tx: &rusqlite::Transaction<'_>,
    cancel: &CancellationToken,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "项目设置更新已取消");
    let state = conn_cache_for(cache_mutex, tx);
    let sv: i64 = tx.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    if state
        .lock()
        .ok()
        .is_some_and(|g| g.validated_site_map_deps_sv == Some(sv))
    {
        return Ok(());
    }
    schema_guard::validate_update_dependencies(
        tx,
        "site_map_entries",
        &["revision", "row", "detail"],
        cancel,
    )?;
    if let Ok(mut g) = state.lock() {
        g.validated_site_map_deps_sv = Some(sv);
    }
    Ok(())
}

pub(in crate::store) fn advance_workspace_cache_changes(
    conn: &Connection,
    before_tc: i64,
) -> Result<()> {
    let Some(state) = lookup_conn_cache(conn) else {
        return Ok(());
    };
    let after_tc = query_total_changes(conn)?;
    if after_tc != before_tc
        && let Ok(mut g) = state.lock()
        && let Some(entry) = g.workspace.as_mut()
        && entry.total_changes == before_tc
    {
        entry.total_changes = after_tc;
    }
    Ok(())
}

pub(crate) struct ProjectStore {
    // Must be declared before `connection` so `CachedWorkspace::drop` removes the
    // connection pointer from `CONN_CACHE_REGISTRY` before `Connection::drop` frees it.
    cached_workspace: std::sync::Mutex<Option<CachedWorkspace>>,
    connection: Connection,
    pub id: Uuid,
    pub path: Option<PathBuf>,
    temporary: Option<TempDir>,
    intruder_sessions: intruder::IntruderSessions,
}
