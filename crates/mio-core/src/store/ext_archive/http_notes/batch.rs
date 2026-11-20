use std::collections::HashMap;

use super::*;
use crate::store::ext_archive::proxy_history::{canonical_item_ids, read_http_item, validate_http_source};
use crate::{ExtHttpNotesBatch, ExtHttpNotesBatchError};

type NoteReceipts = Vec<(Uuid, Uuid, u64)>;

fn owners(
    transaction: &Transaction<'_>,
    ids: &[(Uuid, Uuid)],
    cancel: &CancellationToken,
) -> Result<HashMap<(Uuid, Uuid), Uuid>> {
    ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注已取消");
    ensure!(
        !ids.is_empty() && ids.len() <= 500,
        "外部归档 HTTP 批量备注需要 1–500 个条目"
    );
    let mut seen = HashSet::new();
    let mut sources = HashMap::<_, Vec<_>>::new();
    let mut owners = HashMap::new();
    let mut unique = HashSet::new();
    for &(source, item) in ids {
        ensure!(
            !item.is_nil() && seen.insert((source, item)),
            "外部归档 HTTP 批量备注来源或条目 ID 为空或重复"
        );
        if source.is_nil() {
            validate_live_proxy_flow(transaction, item)?;
            ensure!(
                unique.insert((Uuid::nil(), item)),
                "同一批 外部归档 HTTP 备注包含同一条目的多个目录别名"
            );
            owners.insert((source, item), item);
            continue;
        }
        sources.entry(source).or_default().push(item);
    }
    for (source, ids) in sources {
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注已取消");
        validate_http_source(transaction, source)?;
        let found = canonical_item_ids(transaction, source, &ids)?;
        for item in ids {
            let owner = *found.get(&item).context("外部归档 HTTP 批量备注条目未找到")?;
            ensure!(
                unique.insert((source, owner)),
                "同一批 外部归档 HTTP 备注包含同一条目的多个目录别名"
            );
            owners.insert((source, item), owner);
        }
    }
    Ok(owners)
}

fn read_alias(
    transaction: &Transaction<'_>,
    source: Uuid,
    item: Uuid,
    owner: Uuid,
) -> Result<(ExtItem, Value)> {
    let (input, metadata) = read_http_item(transaction, source, item)?;
    validate_notes_item(&input, &metadata)?;
    if item == owner {
        return Ok((input, metadata));
    }
    let (canonical, canonical_metadata) = read_http_item(transaction, source, owner)?;
    validate_notes_item(&canonical, &canonical_metadata)?;
    ensure!(
        metadata["record"]["object"]["address"]
            == canonical_metadata["record"]["object"]["address"]
            && metadata["record"]["mapping"]["value"]["annotations"]["observation"]
                == canonical_metadata["record"]["mapping"]["value"]["annotations"]["observation"],
        "外部归档 HTTP 备注别名不是同一源对象"
    );
    Ok((canonical, canonical_metadata))
}

impl ProjectStore {
    pub(crate) fn ext_http_notes_batch(
        &self,
        project: ProjectKey,
        ids: &[(Uuid, Uuid)],
        cancel: &CancellationToken,
    ) -> Result<ExtHttpNotesBatch> {
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注读取已取消");
        let transaction = self.connection.unchecked_transaction()?;
        project_guard(&transaction, project, self.id)?;
        let owners = owners(&transaction, ids, cancel)?;
        let live_map = if ids.iter().any(|(source, _)| source.is_nil()) {
            read_live_http_annotations(&transaction)?
        } else {
            std::collections::BTreeMap::new()
        };
        let mut states = Vec::with_capacity(ids.len());
        let mut common_text: Option<String> = None;
        for &(source, item) in ids {
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注读取已取消");
            let note = if source.is_nil() {
                live_map
                    .get(&item)
                    .map_or_else(ExtHttpNotes::default, |entry| ExtHttpNotes {
                        revision: entry.notes_revision,
                        text: entry.notes.clone(),
                    })
            } else {
                let (owner, metadata) =
                    read_alias(&transaction, source, item, owners[&(source, item)])?;
                // Reuse strict UTF-8/source validation without retaining text for the whole selection.
                read_owned(&transaction, &owner, &metadata, false)?
            };
            let has_notes = !note.text.is_empty();
            if states.is_empty() {
                common_text = Some(note.text);
            } else if common_text.as_ref().is_some_and(|text| *text != note.text) {
                common_text = None;
            }
            states.push((source, item, note.revision, has_notes));
        }
        ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注读取已取消");
        transaction.commit()?;
        Ok(ExtHttpNotesBatch {
            states,
            common_text,
        })
    }

    pub(crate) fn set_ext_http_notes_batch(
        &mut self,
        project: ProjectKey,
        changes: &[(Uuid, Uuid, u64)],
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<NoteReceipts, ExtHttpNotesBatchError> {
        let ids: Vec<_> = changes
            .iter()
            .take(501)
            .map(|&(source, item, _)| (source, item))
            .collect();
        let result = (|| {
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注修改已取消");
            ensure!(
                !changes.is_empty() && changes.len() <= 500,
                "外部归档 HTTP 批量备注需要 1–500 个条目"
            );
            ensure!(
                text.len() <= MAX_NOTES_BYTES,
                "外部归档 HTTP 备注超过 64 KiB UTF-8 资源限额"
            );
            ensure!(
                changes
                    .iter()
                    .all(|&(_, _, revision)| revision <= i64::MAX as u64),
                "外部归档 HTTP 备注 revision 无效"
            );
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            project_guard(&transaction, project, self.id)?;
            let owners = owners(&transaction, &ids, cancel)?;
            let mut receipts = Vec::with_capacity(changes.len());
            for &(source, item, revision) in changes {
                ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注修改已取消");
                let note = if source.is_nil() {
                    write_live(&transaction, item, revision, text, cancel)?
                } else {
                    let (owner, metadata) =
                        read_alias(&transaction, source, item, owners[&(source, item)])?;
                    write(&transaction, &owner, &metadata, revision, text, cancel)?
                };
                receipts.push((source, item, note.revision));
            }
            ensure!(!cancel.is_cancelled(), "外部归档 HTTP 批量备注修改已取消");
            transaction.commit()?;
            Ok(receipts)
        })();
        result.map_err(|error: anyhow::Error| ExtHttpNotesBatchError {
            message: error.to_string(),
            stored: if cancel.is_cancelled() {
                Vec::new()
            } else {
                self.ext_http_notes_batch(project, &ids, cancel)
                    .map(|batch| batch.states)
                    .unwrap_or_default()
            },
        })
    }
}

#[cfg(test)]
mod tests;
