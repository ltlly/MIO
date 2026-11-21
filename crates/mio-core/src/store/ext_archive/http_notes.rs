use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{
    ProjectStore,
    proxy_history::{notes_owner, validate_notes_item},
};
use crate::store::plugin_storage::{
    read_live_http_annotations, validate_live_proxy_flow, write_live_http_annotations,
};
use crate::{ExtHttpNotes, ExtHttpNotesChange, ExtHttpNotesError, ExtItem, ProjectKey, Uuid};

mod batch;

const MAX_NOTES_BYTES: usize = 64 * 1024;
const PREVIEW_CHARACTERS: usize = 256;

pub(in crate::store) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE ext_http_notes (
            item TEXT PRIMARY KEY NOT NULL REFERENCES ext_items(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
            text TEXT NOT NULL CHECK(typeof(text)='text' AND length(CAST(text AS BLOB))<=65536)
        ) WITHOUT ROWID;",
    )?;
    Ok(())
}

fn project_guard(transaction: &Transaction<'_>, project: ProjectKey, expected: Uuid) -> Result<()> {
    ensure!(
        !project.id.is_nil() && project.generation > 0 && project.id == expected,
        "外部归档 HTTP 备注项目归属无效"
    );
    let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        schema == crate::store::SCHEMA,
        "外部归档 HTTP 备注项目版本已改变"
    );
    crate::store::schema_guard::validate(transaction)?;
    ensure!(
        crate::store::metadata::project_id(transaction)? == project.id,
        "外部归档 HTTP 备注项目 ID 已改变"
    );
    Ok(())
}

fn source_text<'a>(item: &ExtItem, metadata: &'a Value) -> Result<&'a str> {
    ensure!(item.state == "mapped", "外部归档 HTTP 备注条目尚未映射");
    let annotations = &metadata["record"]["mapping"]["value"]["annotations"];
    ensure!(
        annotations["id"] == 27
            && annotations["binding"]["source_id"] == item.source_id.to_string()
            && annotations["binding"]["item_id"] == item.id.to_string(),
        "外部归档 HTTP 源备注未映射或来源绑定不一致"
    );
    let parent = match annotations["observation"]["state"].as_str() {
        Some("null") => return Ok(""),
        Some("value") => &annotations["observation"]["value"],
        _ => anyhow::bail!("外部归档 HTTP 源备注父字段不可解释；未推断为空"),
    };
    ensure!(
        parent["object"]["object_type"] == 0
            && parent["object"]["subtype"] == 0
            && parent["object"]["address"].as_u64().is_some()
            && parent["mapping"]["state"] == "value",
        "外部归档 HTTP 源备注父对象布局未确认"
    );
    let note = &parent["mapping"]["value"]["note"];
    ensure!(note["id"] == 2, "外部归档 HTTP 源备注字段不是 f2");
    let node = match note["observation"]["state"].as_str() {
        Some("null") => return Ok(""),
        Some("value") => &note["observation"]["value"],
        _ => anyhow::bail!("外部归档 HTTP 源备注字段不可解释；未推断为空"),
    };
    ensure!(
        node["object"]["object_type"] == 0
            && node["object"]["subtype"] == 0
            && node["object"]["address"].as_u64().is_some()
            && node["mapping"]["state"] == "value",
        "外部归档 HTTP 源备注文本对象布局未确认"
    );
    let value = &node["mapping"]["value"];
    let backing = &value["backing"];
    let array = &backing["observation"]["value"];
    let length = &value["length"];
    let count = length["observation"]["value"]
        .as_u64()
        .filter(|length| *length <= 32768)
        .context("外部归档 HTTP 源备注声明长度无效")?;
    let capacity = array["element_count"]
        .as_u64()
        .filter(|capacity| *capacity > 0)
        .context("外部归档 HTTP 源备注容量无效")?;
    let payload_bytes = capacity
        .checked_mul(2)
        .context("外部归档 HTTP 源备注容量溢出")?;
    let total_size = payload_bytes
        .max(1)
        .checked_add(8)
        .context("外部归档 HTTP 源备注数组大小溢出")?;
    ensure!(
        backing["id"] == 1
            && backing["observation"]["state"] == "value"
            && array["kind"] == "utf16"
            && array["address"].as_u64().is_some()
            && Some(payload_bytes) == array["payload_bytes"].as_u64()
            && Some(total_size) == array["total_size"].as_u64()
            && length["id"] == 2
            && length["observation"]["state"] == "value"
            && count <= capacity
            && value["text"]["state"] == "value",
        "外部归档 HTTP 源备注数组或文本状态未确认"
    );
    let text = value["text"]["value"]
        .as_str()
        .context("外部归档 HTTP 源备注不是文本")?;
    ensure!(
        text.len() <= MAX_NOTES_BYTES && text.encode_utf16().count() as u64 == count,
        "外部归档 HTTP 源备注文本长度与已验证声明不一致"
    );
    Ok(text)
}

pub(super) fn read_owned(
    connection: &Connection,
    item: &ExtItem,
    metadata: &Value,
    preview: bool,
) -> Result<ExtHttpNotes> {
    validate_notes_item(item, metadata)?;
    let mut query = connection.prepare(
        "SELECT typeof(revision),revision,typeof(text),length(CAST(text AS BLOB)),
            CASE WHEN typeof(text)='text' AND length(CAST(text AS BLOB))<=65536
                THEN CASE WHEN ?2 AND length(CAST(text AS BLOB))>0
                    THEN substr(CAST(text AS BLOB),1,1024) ELSE CAST(text AS BLOB) END END
         FROM main.ext_http_notes WHERE item=?1",
    )?;
    let mut rows = query.query(params![item.id.to_string(), preview])?;
    let Some(row) = rows.next()? else {
        let text = source_text(item, metadata)?;
        return Ok(ExtHttpNotes {
            revision: 0,
            text: if preview {
                text.chars().take(PREVIEW_CHARACTERS).collect()
            } else {
                text.into()
            },
        });
    };
    let revision: u64 = row.get(1)?;
    ensure!(
        row.get::<_, String>(0)? == "integer" && revision > 0 && revision <= i64::MAX as u64,
        "外部归档 HTTP 备注 revision 损坏"
    );
    let length: u64 = row.get(3)?;
    ensure!(
        row.get::<_, String>(2)? == "text" && length <= MAX_NOTES_BYTES as u64,
        "外部归档 HTTP 备注文本类型或长度无效"
    );
    let bytes: Vec<u8> = row
        .get::<_, Option<Vec<u8>>>(4)?
        .context("外部归档 HTTP 备注文本损坏")?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if preview && error.error_len().is_none() && bytes.len() < length as usize => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])?
        }
        Err(error) => return Err(error).context("外部归档 HTTP 备注不是有效 UTF-8"),
    };
    let text = if preview {
        text.chars().take(PREVIEW_CHARACTERS).collect()
    } else {
        text.into()
    };
    ensure!(rows.next()?.is_none(), "外部归档 HTTP 备注归属重复");
    Ok(ExtHttpNotes { revision, text })
}

pub(super) fn preview(
    connection: &Connection,
    item: &ExtItem,
    metadata: &Value,
) -> (u64, String, Option<String>) {
    // history_page already selected canonical rows. Do not rank the whole source again per row.
    match read_owned(connection, item, metadata, true) {
        Ok(notes) => (notes.revision, notes.text, None),
        Err(error) => (
            0,
            String::new(),
            Some(error.to_string().chars().take(512).collect()),
        ),
    }
}

impl ProjectStore {
    pub(crate) fn ext_http_notes(
        &self,
        project: ProjectKey,
        source: Uuid,
        item: Uuid,
    ) -> Result<ExtHttpNotes> {
        let transaction = self.connection.unchecked_transaction()?;
        project_guard(&transaction, project, self.id)?;
        if source.is_nil() {
            validate_live_proxy_flow(&transaction, item)?;
            let map = read_live_http_annotations(&transaction)?;
            let notes = map
                .get(&item)
                .map_or_else(ExtHttpNotes::default, |entry| ExtHttpNotes {
                    revision: entry.notes_revision,
                    text: entry.notes.clone(),
                });
            transaction.commit()?;
            return Ok(notes);
        }
        let (owner, metadata) = notes_owner(&transaction, source, item)?;
        let notes = read_owned(&transaction, &owner, &metadata, false)?;
        transaction.commit()?;
        Ok(notes)
    }

    pub(crate) fn set_ext_http_notes(
        &mut self,
        project: ProjectKey,
        source: Uuid,
        item: Uuid,
        expected: u64,
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<ExtHttpNotes, ExtHttpNotesError> {
        let result = (|| {
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            project_guard(&transaction, project, self.id)?;
            let change = ExtHttpNotesChange {
                source_id: source,
                item_id: item,
                expected_notes_revision: expected,
                text: text.into(),
            };
            validate_changes(std::slice::from_ref(&change))?;
            if source.is_nil() {
                let saved = write_live(&transaction, item, expected, text, cancel)?;
                ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
                transaction.commit()?;
                return Ok(saved);
            }
            let (owner, metadata) = notes_owner(&transaction, source, item)?;
            let saved = write(
                &transaction,
                &owner,
                &metadata,
                change.expected_notes_revision,
                &change.text,
                cancel,
            )?;
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
            transaction.commit()?;
            Ok(saved)
        })();
        result.map_err(|error: anyhow::Error| ExtHttpNotesError {
            message: error.to_string(),
            stored: self.ext_http_notes(project, source, item).ok(),
        })
    }
}

pub(in crate::store) fn validate_changes(changes: &[ExtHttpNotesChange]) -> Result<()> {
    ensure!(changes.len() <= 64, "待保存 外部归档 HTTP 备注超过 64 条限额");
    let mut seen = HashSet::new();
    let mut bytes = 0usize;
    for change in changes {
        ensure!(
            !change.item_id.is_nil() && seen.insert((change.source_id, change.item_id)),
            "外部归档 HTTP 备注来源或条目 ID 为空或重复"
        );
        ensure!(
            change.text.len() <= MAX_NOTES_BYTES,
            "外部归档 HTTP 备注超过 64 KiB UTF-8 资源限额"
        );
        ensure!(
            change.expected_notes_revision <= i64::MAX as u64,
            "外部归档 HTTP 备注 revision 无效"
        );
        bytes = bytes
            .checked_add(change.text.len())
            .context("外部归档 HTTP 备注总长度溢出")?;
    }
    ensure!(
        bytes <= 4 * 1024 * 1024,
        "待保存 外部归档 HTTP 备注超过 4 MiB 总限额"
    );
    Ok(())
}

pub(in crate::store) fn write_changes(
    transaction: &Transaction<'_>,
    changes: &[ExtHttpNotesChange],
    cancel: &CancellationToken,
) -> Result<Vec<(Uuid, Uuid, u64)>> {
    validate_changes(changes)?;
    let mut owners = Vec::with_capacity(changes.len());
    let mut unique = HashSet::new();
    for change in changes {
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
        if change.source_id.is_nil() {
            validate_live_proxy_flow(transaction, change.item_id)?;
            ensure!(
                unique.insert((Uuid::nil(), change.item_id)),
                "同一批 外部归档 HTTP 备注包含同一条目的重复记录"
            );
            owners.push(None);
            continue;
        }
        let owner = notes_owner(transaction, change.source_id, change.item_id)?;
        ensure!(
            unique.insert((change.source_id, owner.0.id)),
            "同一批 外部归档 HTTP 备注包含同一条目的多个目录别名"
        );
        owners.push(Some(owner));
    }
    changes
        .iter()
        .zip(&owners)
        .map(|(change, owner)| {
            let saved = match owner {
                Some((owner, metadata)) => write(
                    transaction,
                    owner,
                    metadata,
                    change.expected_notes_revision,
                    &change.text,
                    cancel,
                )?,
                None => write_live(
                    transaction,
                    change.item_id,
                    change.expected_notes_revision,
                    &change.text,
                    cancel,
                )?,
            };
            Ok((change.source_id, change.item_id, saved.revision))
        })
        .collect()
}

fn write_live(
    transaction: &Transaction<'_>,
    flow_id: Uuid,
    expected: u64,
    text: &str,
    cancel: &CancellationToken,
) -> Result<ExtHttpNotes> {
    ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
    validate_live_proxy_flow(transaction, flow_id)?;
    let mut map = read_live_http_annotations(transaction)?;
    let current = map.get(&flow_id).cloned().unwrap_or_default();
    ensure!(
        current.notes_revision == expected,
        "外部归档 HTTP 备注已改变；请确认当前备注后重试"
    );
    if current.notes == text {
        return Ok(ExtHttpNotes {
            revision: current.notes_revision,
            text: current.notes,
        });
    }
    let revision = current
        .notes_revision
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .context("外部归档 HTTP 备注 revision 已耗尽")?;
    let entry = map.entry(flow_id).or_default();
    entry.notes_revision = revision;
    entry.notes = text.into();
    write_live_http_annotations(transaction, &mut map, cancel)?;
    ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
    Ok(ExtHttpNotes {
        revision,
        text: text.into(),
    })
}

fn write(
    transaction: &Transaction<'_>,
    owner: &ExtItem,
    metadata: &Value,
    expected: u64,
    text: &str,
    cancel: &CancellationToken,
) -> Result<ExtHttpNotes> {
    ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
    let stored = read_owned(transaction, owner, metadata, false)?;
    ensure!(
        stored.revision == expected,
        "外部归档 HTTP 备注已改变；请确认当前备注后重试"
    );
    if stored.text == text {
        return Ok(stored);
    }
    let revision = stored
        .revision
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .context("外部归档 HTTP 备注 revision 已耗尽")?;
    if stored.revision == 0 {
        transaction.execute(
            "INSERT INTO main.ext_http_notes(item,revision,text) VALUES(?1,?2,?3)",
            params![owner.id.to_string(), revision, text],
        )?;
    } else {
        let changed = transaction.execute(
            "UPDATE main.ext_http_notes SET revision=?1,text=?2 WHERE item=?3 AND revision=?4",
            params![revision, text, owner.id.to_string(), stored.revision],
        )?;
        ensure!(changed == 1, "外部归档 HTTP 备注归属或 revision 已改变；未修改");
    }
    ensure!(!cancel.is_cancelled(), "外部归档 HTTP 备注修改已取消");
    Ok(ExtHttpNotes {
        revision,
        text: text.into(),
    })
}

#[cfg(test)]
mod tests;
