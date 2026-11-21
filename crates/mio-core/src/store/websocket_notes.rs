use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Statement, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;

use super::ProjectStore;
use crate::{
    ProjectKey, Uuid, WebSocketNotes, WebSocketNotesChange, WebSocketNotesError, WebSocketSummary,
};

const MAX_NOTES_BYTES: usize = 64 * 1024;
const PREVIEW_CHARACTERS: usize = 256;
const SELECT_NOTES: &str =
    "SELECT w.id,n.message,n.revision,typeof(n.text),length(CAST(n.text AS BLOB)),
    CASE WHEN typeof(n.text)='text' AND length(CAST(n.text AS BLOB))<=65536
        THEN CASE WHEN ?2 IS NULL THEN n.text
            WHEN length(CAST(n.text AS BLOB))=0 THEN CAST(n.text AS BLOB)
            ELSE substr(CAST(n.text AS BLOB),1,?2) END END
    FROM websocket_messages w LEFT JOIN websocket_notes n ON w.id=n.message WHERE w.id=?1";

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE websocket_notes (
            message TEXT PRIMARY KEY NOT NULL REFERENCES websocket_messages(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
            text TEXT NOT NULL CHECK(typeof(text)='text' AND length(CAST(text AS BLOB))<=65536)
        ) WITHOUT ROWID;",
    )?;
    Ok(())
}

fn read(
    statement: &mut Statement<'_>,
    id: Uuid,
    preview: Option<i64>,
) -> Result<Option<(String, WebSocketNotes)>> {
    let mut rows = statement.query(params![id.to_string(), preview])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let mut canonical: String = row.get(0)?;
    let notes = if let Some(key) = row.get::<_, Option<String>>(1)? {
        canonical = key;
        let revision: u64 = row.get(2)?;
        ensure!(revision > 0, "WebSocket 备注 revision 损坏");
        ensure!(
            row.get_ref(3)?.as_str()? == "text",
            "WebSocket 备注不是文本"
        );
        let length: u64 = row.get(4)?;
        ensure!(
            length <= MAX_NOTES_BYTES as u64,
            "WebSocket 备注超过 64 KiB 资源限额"
        );
        let text = if preview.is_some() {
            let bytes = row.get_ref(5)?.as_blob()?;
            let prefix = match std::str::from_utf8(bytes) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() && bytes.len() < length as usize => {
                    std::str::from_utf8(&bytes[..error.valid_up_to()])?
                }
                Err(error) => return Err(error).context("WebSocket 备注不是有效 UTF-8"),
            };
            prefix.chars().take(PREVIEW_CHARACTERS).collect()
        } else {
            row.get::<_, Option<String>>(5)?
                .context("WebSocket 备注文本损坏")?
        };
        WebSocketNotes { revision, text }
    } else {
        WebSocketNotes::default()
    };
    ensure!(
        rows.next()?.is_none(),
        "WebSocket 消息存在重复备注归属；未选择或修改其中一条"
    );
    Ok(Some((canonical, notes)))
}

pub(super) fn fill_previews(connection: &Connection, rows: &mut [WebSocketSummary]) -> Result<()> {
    let mut statement = connection.prepare(SELECT_NOTES)?;
    for row in rows {
        // Recovery rows may not exist in this snapshot yet; only persisted parents own notes.
        let Some((_, notes)) = read(
            &mut statement,
            row.id,
            Some((PREVIEW_CHARACTERS * 4) as i64),
        )?
        else {
            row.notes_revision = 0;
            row.notes_preview.clear();
            continue;
        };
        row.notes_revision = notes.revision;
        row.notes_preview = notes.text;
    }
    Ok(())
}

impl ProjectStore {
    pub(crate) fn websocket_notes(&self, project: ProjectKey, id: Uuid) -> Result<WebSocketNotes> {
        ensure!(!id.is_nil(), "WebSocket 消息 ID 不能为空");
        ensure!(
            !project.id.is_nil() && project.generation > 0 && project.id == self.id,
            "WebSocket 备注项目归属无效"
        );
        let transaction = self.connection.unchecked_transaction()?;
        let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(schema == super::SCHEMA, "WebSocket 备注项目版本已改变");
        super::schema_guard::validate(&transaction)?;
        ensure!(
            super::metadata::project_id(&transaction)? == project.id,
            "WebSocket 备注项目 ID 已改变"
        );
        let notes = read(&mut transaction.prepare(SELECT_NOTES)?, id, None)?
            .context("WebSocket 消息不存在；备注未读取")?
            .1;
        transaction.commit()?;
        Ok(notes)
    }

    pub(crate) fn set_websocket_notes(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected: u64,
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<WebSocketNotes, WebSocketNotesError> {
        let saved = (|| -> Result<WebSocketNotes> {
            ensure!(!id.is_nil(), "WebSocket 消息 ID 不能为空；备注未修改");
            ensure!(
                !project.id.is_nil() && project.generation > 0 && project.id == self.id,
                "WebSocket 备注项目归属无效；备注未修改"
            );
            ensure!(
                text.len() <= MAX_NOTES_BYTES,
                "WebSocket 备注超过 64 KiB UTF-8 资源限额；备注未修改"
            );
            ensure!(!cancel.is_cancelled(), "WebSocket 备注修改已取消");
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let schema: i32 =
                transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
            ensure!(
                schema == super::SCHEMA,
                "WebSocket 备注项目版本已改变；备注未修改"
            );
            super::schema_guard::validate(&transaction)?;
            ensure!(
                super::metadata::project_id(&transaction)? == project.id,
                "WebSocket 备注项目 ID 已改变；备注未修改"
            );
            let notes = write(&transaction, id, expected, text, cancel)?;
            ensure!(!cancel.is_cancelled(), "WebSocket 备注修改已取消");
            transaction.commit()?;
            Ok(notes)
        })();
        saved.map_err(|error| WebSocketNotesError {
            message: error.to_string(),
            stored: self.websocket_notes(project, id).ok(),
        })
    }
}

pub(super) fn validate_changes(changes: &[WebSocketNotesChange]) -> Result<()> {
    ensure!(changes.len() <= 64, "待保存 WebSocket 备注超过 64 条限额");
    let mut ids = std::collections::HashSet::new();
    let mut total = 0usize;
    for change in changes {
        ensure!(
            !change.message_id.is_nil() && ids.insert(change.message_id),
            "待保存 WebSocket 备注消息 ID 为空或重复"
        );
        ensure!(
            change.text.len() <= MAX_NOTES_BYTES,
            "WebSocket 备注超过 64 KiB UTF-8 资源限额；未保存"
        );
        total = total
            .checked_add(change.text.len())
            .context("WebSocket 备注字节总数溢出")?;
    }
    ensure!(
        total <= 4 * 1024 * 1024,
        "待保存 WebSocket 备注超过 4 MiB 总限额"
    );
    Ok(())
}

pub(super) fn write(
    transaction: &rusqlite::Transaction<'_>,
    id: Uuid,
    expected: u64,
    text: &str,
    cancel: &CancellationToken,
) -> Result<WebSocketNotes> {
    ensure!(!id.is_nil(), "WebSocket 备注消息 ID 不能为空");
    ensure!(
        text.len() <= MAX_NOTES_BYTES,
        "WebSocket 备注超过 64 KiB UTF-8 资源限额"
    );
    ensure!(!cancel.is_cancelled(), "WebSocket 备注修改已取消");
    let (canonical, stored) = read(&mut transaction.prepare(SELECT_NOTES)?, id, None)?
        .context("WebSocket 消息尚未写盘；请保存或另存为后重试备注")?;
    ensure!(
        stored.revision == expected,
        "WebSocket 备注已改变；请确认当前备注后重试"
    );
    if stored.text == text {
        return Ok(stored);
    }
    let revision = expected
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .context("WebSocket 备注 revision 已耗尽；备注未修改")?;
    if expected == 0 {
        transaction.execute(
            "INSERT INTO websocket_notes(message,revision,text) VALUES(?1,?2,?3)",
            params![canonical, revision, text],
        )?;
    } else {
        let changed = transaction.execute(
            "UPDATE websocket_notes SET revision=?1,text=?2 WHERE message=?3 AND revision=?4",
            params![revision, text, canonical, expected],
        )?;
        ensure!(
            changed == 1,
            "WebSocket 备注归属或 revision 已改变；备注未修改"
        );
    }
    ensure!(!cancel.is_cancelled(), "WebSocket 备注修改已取消");
    Ok(WebSocketNotes {
        revision,
        text: text.into(),
    })
}

#[cfg(test)]
mod tests;
