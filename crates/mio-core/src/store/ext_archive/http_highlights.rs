use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{
    ProjectStore,
    proxy_history::{canonical_item_ids, read_http_item, validate_http_item, validate_http_source},
};
use crate::store::plugin_storage::{
    read_live_http_annotations, validate_live_proxy_flow, write_live_http_annotations,
};
use crate::{
    ExtHttpHighlight, ExtHttpHighlightChange, ExtHttpHighlightError, ExtItem, ProjectKey, Uuid,
};

type SavedHighlights = Vec<(Uuid, Uuid, ExtHttpHighlight)>;

pub(in crate::store) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE ext_http_highlights (
            item TEXT PRIMARY KEY NOT NULL REFERENCES ext_items(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
            color INTEGER NOT NULL CHECK(typeof(color)='integer' AND color BETWEEN 0 AND 9)
        ) WITHOUT ROWID;",
    )?;
    Ok(())
}

fn project_guard(transaction: &Transaction<'_>, project: ProjectKey, expected: Uuid) -> Result<()> {
    ensure!(
        !project.id.is_nil() && project.generation > 0 && project.id == expected,
        "外部归档 HTTP 高亮项目归属无效"
    );
    let schema: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        schema == crate::store::SCHEMA,
        "外部归档 HTTP 高亮项目版本已改变"
    );
    crate::store::schema_guard::validate(transaction)?;
    ensure!(
        crate::store::metadata::project_id(transaction)? == project.id,
        "外部归档 HTTP 高亮项目 ID 已改变"
    );
    Ok(())
}

fn validate_highlight_item(item: &ExtItem, metadata: &Value) -> Result<()> {
    validate_http_item(item, metadata)?;
    let field = &metadata["record"]["mapping"]["value"]["highlight"];
    if !field.is_null() {
        ensure!(
            field["id"] == 13
                && field["binding"]["source_id"] == item.source_id.to_string()
                && field["binding"]["item_id"] == item.id.to_string(),
            "外部归档 HTTP 源高亮身份或字段绑定不一致"
        );
    }
    Ok(())
}

fn read_owned(
    connection: &Connection,
    item: &ExtItem,
    metadata: &Value,
) -> Result<ExtHttpHighlight> {
    validate_highlight_item(item, metadata)?;
    let mut query = connection.prepare(
        "SELECT CASE WHEN typeof(revision)='integer' AND revision>0 THEN revision END,
            CASE WHEN typeof(color)='integer' AND color BETWEEN 0 AND 9 THEN color END
         FROM main.ext_http_highlights WHERE item=?1",
    )?;
    let mut rows = query.query([item.id.to_string()])?;
    if let Some(row) = rows.next()? {
        let revision: u64 = row
            .get::<_, Option<u64>>(0)?
            .context("外部归档 HTTP 高亮 revision 损坏")?;
        let color: u8 = row
            .get::<_, Option<u8>>(1)?
            .context("外部归档 HTTP 高亮颜色损坏")?;
        ensure!(rows.next()?.is_none(), "外部归档 HTTP 高亮归属重复");
        return Ok(ExtHttpHighlight { revision, color });
    }
    let field = &metadata["record"]["mapping"]["value"]["highlight"];
    ensure!(
        field["id"] == 13 && field["observation"]["state"] == "value",
        "外部归档 HTTP 源高亮未映射；未推断为无色"
    );
    let color = field["observation"]["value"]
        .as_u64()
        .filter(|color| *color <= 9)
        .context("外部归档 HTTP 源高亮不在已确认色板中；原始值保留")? as u8;
    Ok(ExtHttpHighlight { revision: 0, color })
}

fn read_alias(
    connection: &Connection,
    source: Uuid,
    input: Uuid,
    owner: Uuid,
) -> Result<ExtHttpHighlight> {
    let (item, metadata) = read_http_item(connection, source, input)?;
    validate_highlight_item(&item, &metadata)?;
    if input == owner {
        return read_owned(connection, &item, &metadata);
    }
    let (canonical, canonical_metadata) = read_http_item(connection, source, owner)?;
    validate_highlight_item(&canonical, &canonical_metadata)?;
    ensure!(
        metadata["record"]["object"]["address"]
            == canonical_metadata["record"]["object"]["address"]
            && metadata["record"]["mapping"]["value"]["highlight"]["observation"]
                == canonical_metadata["record"]["mapping"]["value"]["highlight"]["observation"],
        "外部归档 HTTP 高亮别名不是同一源字段"
    );
    read_owned(connection, &canonical, &canonical_metadata)
}

pub(super) fn preview(
    connection: &Connection,
    item: &ExtItem,
    metadata: &Value,
) -> (Option<ExtHttpHighlight>, Option<String>) {
    // The history query has already selected the canonical item; only read the scalar overlay.
    match read_owned(connection, item, metadata) {
        Ok(highlight) => (Some(highlight), None),
        Err(error) => (None, Some(error.to_string().chars().take(512).collect())),
    }
}

pub(in crate::store) fn validate_changes(changes: &[ExtHttpHighlightChange]) -> Result<()> {
    ensure!(changes.len() <= 500, "外部归档 HTTP 高亮批次超过 500 条限额");
    let mut ids = HashSet::new();
    for change in changes {
        ensure!(
            !change.item_id.is_nil() && ids.insert((change.source_id, change.item_id)),
            "外部归档 HTTP 高亮来源或条目 ID 为空或重复"
        );
        ensure!(change.color <= 9, "外部归档 HTTP 高亮颜色不在已确认色板中");
        ensure!(
            change.expected_highlight_revision <= i64::MAX as u64,
            "外部归档 HTTP 高亮 revision 无效"
        );
    }
    Ok(())
}

fn source_ids(changes: &[ExtHttpHighlightChange]) -> HashMap<Uuid, Vec<Uuid>> {
    let mut sources = HashMap::<_, Vec<_>>::new();
    for change in changes {
        if change.source_id.is_nil() {
            continue;
        }
        sources
            .entry(change.source_id)
            .or_default()
            .push(change.item_id);
    }
    sources
}

fn write_batch(
    transaction: &Transaction<'_>,
    changes: &[ExtHttpHighlightChange],
    cancel: &CancellationToken,
) -> Result<SavedHighlights> {
    validate_changes(changes)?;
    let mut owners = HashMap::new();
    for (source, ids) in source_ids(changes) {
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 高亮修改已取消");
        validate_http_source(transaction, source)?;
        owners.extend(
            canonical_item_ids(transaction, source, &ids)?
                .into_iter()
                .map(|(id, owner)| ((source, id), owner)),
        );
    }
    let has_live = changes.iter().any(|change| change.source_id.is_nil());
    let mut live_map = if has_live {
        read_live_http_annotations(transaction)?
    } else {
        std::collections::BTreeMap::new()
    };
    let mut unique = HashSet::new();
    let mut ready = Vec::with_capacity(changes.len());
    for change in changes {
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 高亮修改已取消");
        if change.source_id.is_nil() {
            validate_live_proxy_flow(transaction, change.item_id)?;
            ensure!(
                unique.insert((Uuid::nil(), change.item_id)),
                "外部归档 HTTP 高亮批次包含同一条目的重复记录"
            );
            let stored = live_map.get(&change.item_id).map_or(
                ExtHttpHighlight {
                    revision: 0,
                    color: 0,
                },
                |entry| ExtHttpHighlight {
                    revision: entry.highlight_revision,
                    color: entry.highlight,
                },
            );
            ensure!(
                stored.revision == change.expected_highlight_revision,
                "外部归档 HTTP 高亮已改变；请确认当前颜色后重试"
            );
            ready.push((change.item_id, stored));
            continue;
        }
        let owner = *owners
            .get(&(change.source_id, change.item_id))
            .context("外部归档 HTTP 高亮条目归属不存在")?;
        ensure!(
            unique.insert((change.source_id, owner)),
            "外部归档 HTTP 高亮批次包含同一条目的多个目录别名"
        );
        let stored = read_alias(transaction, change.source_id, change.item_id, owner)?;
        ensure!(
            stored.revision == change.expected_highlight_revision,
            "外部归档 HTTP 高亮已改变；请确认当前颜色后重试"
        );
        ready.push((owner, stored));
    }
    let mut live_dirty = false;
    let mut saved = Vec::with_capacity(changes.len());
    for (change, (owner, stored)) in changes.iter().zip(ready) {
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 高亮修改已取消");
        let value = if stored.color == change.color {
            stored
        } else if change.source_id.is_nil() {
            let revision = stored
                .revision
                .checked_add(1)
                .filter(|revision| *revision <= i64::MAX as u64)
                .context("外部归档 HTTP 高亮 revision 已耗尽")?;
            let entry = live_map.entry(owner).or_default();
            entry.highlight_revision = revision;
            entry.highlight = change.color;
            live_dirty = true;
            ExtHttpHighlight {
                revision,
                color: change.color,
            }
        } else {
            let revision = stored
                .revision
                .checked_add(1)
                .filter(|revision| *revision <= i64::MAX as u64)
                .context("外部归档 HTTP 高亮 revision 已耗尽")?;
            if stored.revision == 0 {
                transaction.execute(
                    "INSERT INTO main.ext_http_highlights(item,revision,color) VALUES(?1,?2,?3)",
                    params![owner.to_string(), revision, change.color],
                )?;
            } else {
                let count = transaction.execute("UPDATE main.ext_http_highlights SET revision=?1,color=?2 WHERE item=?3 AND revision=?4",params![revision,change.color,owner.to_string(),stored.revision])?;
                ensure!(count == 1, "外部归档 HTTP 高亮归属或 revision 已改变；未修改");
            }
            ExtHttpHighlight {
                revision,
                color: change.color,
            }
        };
        saved.push((change.source_id, change.item_id, value));
    }
    if live_dirty {
        write_live_http_annotations(transaction, &mut live_map, cancel)?;
    }
    ensure!(!cancel.is_cancelled(), "外部归档 HTTP 高亮修改已取消");
    Ok(saved)
}

pub(in crate::store) fn write_changes(
    transaction: &Transaction<'_>,
    changes: &[ExtHttpHighlightChange],
    cancel: &CancellationToken,
) -> Result<Vec<(Uuid, Uuid, u64)>> {
    Ok(write_batch(transaction, changes, cancel)?
        .into_iter()
        .map(|(source, item, value)| (source, item, value.revision))
        .collect())
}

impl ProjectStore {
    pub(crate) fn ext_http_highlights(
        &self,
        project: ProjectKey,
        changes: &[ExtHttpHighlightChange],
    ) -> Result<SavedHighlights> {
        ensure!(changes.len() <= 500, "外部归档 HTTP 高亮批次超过 500 条限额");
        let transaction = self.connection.unchecked_transaction()?;
        project_guard(&transaction, project, self.id)?;
        let mut owners = HashMap::new();
        for (source, ids) in source_ids(changes) {
            if validate_http_source(&transaction, source).is_ok()
                && let Ok(found) = canonical_item_ids(
                    &transaction,
                    source,
                    &ids.into_iter()
                        .filter(|id| !id.is_nil())
                        .collect::<Vec<_>>(),
                )
            {
                owners.extend(found.into_iter().map(|(id, owner)| ((source, id), owner)));
            }
        }
        let mut seen = HashSet::new();
        let mut saved = Vec::new();
        let live_map = if changes.iter().any(|change| change.source_id.is_nil()) {
            read_live_http_annotations(&transaction).ok()
        } else {
            None
        };
        for change in changes {
            if !seen.insert((change.source_id, change.item_id)) {
                continue;
            }
            if change.source_id.is_nil() {
                if !change.item_id.is_nil()
                    && validate_live_proxy_flow(&transaction, change.item_id).is_ok()
                    && let Some(map) = &live_map
                {
                    let value = map.get(&change.item_id).map_or(
                        ExtHttpHighlight {
                            revision: 0,
                            color: 0,
                        },
                        |entry| ExtHttpHighlight {
                            revision: entry.highlight_revision,
                            color: entry.highlight,
                        },
                    );
                    saved.push((change.source_id, change.item_id, value));
                }
            } else if let Some(owner) = owners.get(&(change.source_id, change.item_id))
                && let Ok(value) =
                    read_alias(&transaction, change.source_id, change.item_id, *owner)
            {
                saved.push((change.source_id, change.item_id, value));
            }
        }
        transaction.commit()?;
        Ok(saved)
    }

    pub(crate) fn set_ext_http_highlights(
        &mut self,
        project: ProjectKey,
        changes: &[ExtHttpHighlightChange],
        cancel: &CancellationToken,
    ) -> std::result::Result<SavedHighlights, ExtHttpHighlightError> {
        let result = (|| {
            ensure!(!changes.is_empty(), "外部归档 HTTP 高亮修改批次为空");
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 高亮修改已取消");
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            project_guard(&transaction, project, self.id)?;
            let saved = write_batch(&transaction, changes, cancel)?;
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 高亮修改已取消");
            transaction.commit()?;
            Ok(saved)
        })();
        result.map_err(|error: anyhow::Error| ExtHttpHighlightError {
            message: error.to_string(),
            stored: if cancel.is_cancelled() {
                Vec::new()
            } else {
                self.ext_http_highlights(project, changes)
                    .unwrap_or_default()
            },
        })
    }
}

#[cfg(test)]
mod tests;
