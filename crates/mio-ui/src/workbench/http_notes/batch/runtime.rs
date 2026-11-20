use super::*;

impl Workbench {
    pub(in crate::workbench) fn http_notes_batch_runtime_joined(&mut self, cx: &mut Context<Self>) {
        if self.runtime_result.is_none() {
            return;
        }
        let Some(batch) = self
            .http_notes
            .batch
            .as_mut()
            .filter(|batch| batch.pending())
        else {
            return;
        };
        if let Some(read) = batch.read.take() {
            read.cancel.cancel();
        }
        if let Some(write) = batch.write.take() {
            write.cancel.cancel();
        }
        batch.closing = false;
        batch.saved = None;
        batch.error =
            Some("后台已回收；批量备注结果未确认，输入仍保留，请核对项目后再决定是否取消。".into());
        cx.notify();
    }

    pub(in crate::workbench) fn submit_http_notes_batch(
        &mut self,
        id: Uuid,
        text: String,
        editor_revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.backend_available()
            || self.transition.is_some()
            || self.project_saves_in_flight > 0
            || self.file_picker_pending()
        {
            return Err("项目状态尚不可保存批量备注；当前输入保留。".into());
        }
        self.capture_http_notes(window, cx)?;
        let batch = self
            .http_notes
            .batch
            .as_ref()
            .filter(|batch| batch.id == id)
            .ok_or("Add notes 窗口已失效")?;
        if batch.pending() || batch.closing {
            return Err("批量备注仍在确认；输入保留。".into());
        }
        if batch.states.is_empty() {
            let ids = batch.ids.clone();
            self.read_http_notes_batch(ids, cx);
            return Ok(());
        }
        if text.len() > MAX_NOTE_BYTES {
            return Err("备注超过 64 KiB；请缩短后保存，输入仍保留。".into());
        }
        let project = batch.project;
        if self.project != Some(project) {
            return Err("项目已改变；未写入批量备注。".into());
        }
        let changes: Vec<_> = batch
            .states
            .iter()
            .map(|&(source, item, revision, _)| {
                let revision = self
                    .http_notes
                    .drafts
                    .get(&(source, item))
                    .map_or(revision, |draft| revision.max(draft.stored.revision));
                (source, item, revision)
            })
            .collect();
        let local = self
            .http_notes
            .drafts
            .iter()
            .filter(|(key, _)| batch.ids.contains(key))
            .map(|(key, draft)| (*key, draft.text.clone()))
            .collect();
        let active = self
            .http_notes
            .active
            .filter(|active| batch.ids.contains(active))
            .map(|active| (active, self.http_notes.editor.read(cx).document_revision()));
        let operation = Uuid::new_v4();
        let cancel = CancellationToken::new();
        if !self.enqueue(
            Command::SetExtHttpNotesBatch {
                project,
                operation_id: operation,
                changes: changes.clone(),
                text: text.clone(),
                cancel: cancel.clone(),
            },
            cx,
        ) {
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "批量备注未入队；输入仍保留。".into()));
        }
        let batch = self.http_notes.batch.as_mut().unwrap();
        batch.error = None;
        batch.saved = None;
        batch.write = Some(BatchWrite {
            operation,
            changes,
            text,
            editor_revision,
            local,
            active,
            cancel,
        });
        self.http_notes_batch_deadline(id, operation, cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn on_http_notes_batch(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::ExtHttpNotesBatch {
                project,
                operation_id,
                result,
            } => {
                let drafts = &self.http_notes.drafts;
                let Some(batch) = self.http_notes.batch.as_mut().filter(|batch| {
                    batch.project == project
                        && self.project == Some(project)
                        && batch
                            .read
                            .as_ref()
                            .is_some_and(|read| read.operation == operation_id)
                }) else {
                    return;
                };
                let read = batch.read.take().unwrap();
                let mut next_read = None;
                match result {
                    Ok(result) if valid_states(&read.ids, &result.states) => {
                        let subset = read.ids != batch.ids;
                        if subset {
                            for state in result.states {
                                if let Some(previous) = batch
                                    .states
                                    .iter_mut()
                                    .find(|previous| (previous.0, previous.1) == (state.0, state.1))
                                {
                                    *previous = state;
                                }
                            }
                        } else {
                            batch.states = result.states;
                        }
                        batch.common = result.common_text.map(|text| (read.ids.clone(), text));
                        batch.error = None;
                        let uncovered: Vec<_> = batch
                            .ids
                            .iter()
                            .copied()
                            .filter(|id| !drafts.contains_key(id))
                            .collect();
                        if !batch.closing
                            && !uncovered.is_empty()
                            && (subset && uncovered != read.ids
                                || !subset
                                    && batch.common.is_none()
                                    && uncovered.len() < batch.ids.len())
                        {
                            if uncovered.iter().all(|id| {
                                batch
                                    .states
                                    .iter()
                                    .any(|state| (state.0, state.1) == *id && !state.3)
                            }) {
                                batch.common = Some((uncovered, String::new()));
                            } else {
                                next_read = Some(uncovered);
                            }
                        }
                    }
                    Ok(_) => {
                        batch.states.clear();
                        batch.common = None;
                        batch.error =
                            Some("批量备注读取结果不完整或身份不匹配；未允许覆盖。".into())
                    }
                    Err(error) => {
                        batch.states.clear();
                        batch.common = None;
                        batch.error = Some(error);
                    }
                }
                if let Some(ids) = next_read {
                    self.read_http_notes_batch(ids, cx);
                }
            }
            Event::ExtHttpNotesBatchChanged {
                project,
                operation_id,
                result,
            } => {
                let Some(batch) = self.http_notes.batch.as_mut().filter(|batch| {
                    batch.project == project
                        && self.project == Some(project)
                        && batch
                            .write
                            .as_ref()
                            .is_some_and(|write| write.operation == operation_id)
                }) else {
                    return;
                };
                let write = batch.write.take().unwrap();
                match result {
                    Ok(receipts) if valid_receipts(&write.changes, &receipts) => {
                        batch.states = receipts
                            .iter()
                            .map(|&(source, item, revision)| {
                                (source, item, revision, !write.text.is_empty())
                            })
                            .collect();
                        batch.saved = Some((write.editor_revision, write.text.clone()));
                        batch.common = Some((batch.ids.clone(), write.text.clone()));
                        batch.error = None;
                        self.accept_http_notes_batch(&write, &receipts, window, cx);
                    }
                    Ok(_) => {
                        batch.error = Some("批量备注保存回执不匹配；输入和原草稿仍保留。".into())
                    }
                    Err(error) => {
                        if valid_states(&batch.ids, &error.stored) {
                            batch.states = error.stored;
                            batch.common = None;
                        }
                        batch.error = Some(error.message);
                    }
                }
            }
            _ => return,
        }
        cx.notify();
    }

    pub(in crate::workbench) fn http_notes_batch_project_saved(&mut self, cx: &mut Context<Self>) {
        if let Some(batch) = &mut self.http_notes.batch {
            if batch.pending() || batch.closing {
                return;
            }
            batch.states.clear();
            batch.common = None;
            let ids = batch.ids.clone();
            self.read_http_notes_batch(ids, cx);
        }
    }

    fn accept_http_notes_batch(
        &mut self,
        write: &BatchWrite,
        receipts: &[(Uuid, Uuid, u64)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let captured = self.capture_http_notes(window, cx).is_ok();
        for &(source, item, revision) in receipts {
            let id = (source, item);
            let stored = ExtHttpNotes {
                revision,
                text: write.text.clone(),
            };
            if let Some(draft) = self.http_notes.drafts.get_mut(&id) {
                if write.local.get(&id) == Some(&draft.text) {
                    self.http_notes.drafts.remove(&id);
                } else {
                    draft.stored = stored.clone();
                    draft.error = None;
                }
            }
            if self.http_notes.active == Some(id) {
                let unchanged = captured
                    && write.active
                        == Some((id, self.http_notes.editor.read(cx).document_revision()));
                self.http_notes.stored = Some(stored.clone());
                self.http_notes.error = None;
                if unchanged {
                    self.http_notes.editor.update(cx, |editor, cx| {
                        editor.set_value(write.text.clone(), window, cx)
                    });
                }
            }
            self.update_http_notes_summary(id, &stored, cx);
        }
        self.history_dirty = true;
    }
}

pub(super) fn valid_states(ids: &[NoteId], states: &[NoteState]) -> bool {
    ids.len() == states.len()
        && ids.iter().all(|id| {
            states
                .iter()
                .filter(|(source, item, revision, _)| {
                    (*source, *item) == *id && *revision <= i64::MAX as u64
                })
                .count()
                == 1
        })
}

fn valid_receipts(changes: &[(Uuid, Uuid, u64)], receipts: &[(Uuid, Uuid, u64)]) -> bool {
    changes.len() == receipts.len()
        && changes.iter().all(|(source, item, revision)| {
            receipts
                .iter()
                .filter(|(actual_source, actual_item, actual_revision)| {
                    (*source, *item) == (*actual_source, *actual_item)
                        && (*actual_revision == *revision
                            || Some(*actual_revision) == revision.checked_add(1))
                })
                .count()
                == 1
        })
}

pub(in crate::workbench::http_notes) fn effective_common(
    notes: &HttpNotes,
    batch: &BatchNotes,
) -> Option<String> {
    let mut values = batch.ids.iter().map(|id| {
        notes
            .drafts
            .get(id)
            .map(|draft| draft.text.as_str())
            .or_else(|| {
                batch
                    .common
                    .as_ref()
                    .filter(|(ids, _)| ids.contains(id))
                    .map(|(_, text)| text.as_str())
            })
    });
    let first = values.next()??;
    values
        .all(|value| value == Some(first))
        .then(|| first.to_owned())
}
