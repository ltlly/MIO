use super::*;
use gpui_kit::component::input::EditorState;
use mio_core::{WebSocketNotes, WebSocketNotesChange, WebSocketNotesError};

const MAX_DRAFTS: usize = 64;
const MAX_NOTE_BYTES: usize = 64 * 1024;

mod editor;

pub(in crate::workbench) struct Notes {
    pub(in crate::workbench) open: bool,
    pub(in crate::workbench) editor: Entity<EditorState>,
    pub(super) active: Option<Uuid>,
    pub(super) stored: Option<WebSocketNotes>,
    pub(super) read_revision: u64,
    pub(super) reading: bool,
    pub(super) error: Option<String>,
    pub(super) drafts: HashMap<Uuid, Draft>,
    pub(super) _subscription: Subscription,
}

pub(super) struct Draft {
    pub(super) text: String,
    pub(super) stored: WebSocketNotes,
    pub(super) pending: Option<Pending>,
    pub(super) error: Option<String>,
}
pub(super) struct Pending {
    pub(super) operation: Uuid,
    pub(super) text: String,
    pub(super) cancel: CancellationToken,
}

impl Notes {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .line_number(false)
                .soft_wrap(true)
                .auto_close(false)
                .autocorrect(false)
        });
        let subscription = cx.subscribe_in(&editor, window, |view, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                match view.capture_websocket_notes(window, cx) {
                    Ok(()) => view.send_websocket_notes(false, cx),
                    Err(error) => {
                        view.websocket.notes.error = Some(error);
                        cx.notify();
                    }
                }
            }
        });
        Self {
            open: false,
            editor,
            active: None,
            stored: None,
            read_revision: 0,
            reading: false,
            error: None,
            drafts: HashMap::new(),
            _subscription: subscription,
        }
    }

    pub(in crate::workbench) fn disconnect(&mut self) {
        self.reading = false;
        for draft in self.drafts.values_mut() {
            if let Some(pending) = &draft.pending {
                pending.cancel.cancel();
            }
            draft.error = Some("后台服务已断开；备注保存结果未确认，草稿仍保留。".into());
        }
        if self.active.is_some() {
            self.error = Some("后台服务已断开；备注草稿仍保留。".into());
        }
    }

    pub(in crate::workbench) fn reset(&mut self, window: &mut Window, cx: &mut App) {
        for draft in self.drafts.values() {
            if let Some(pending) = &draft.pending {
                pending.cancel.cancel();
            }
        }
        self.drafts.clear();
        self.active = None;
        self.stored = None;
        self.reading = false;
        self.error = None;
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
    }
}

impl Workbench {
    pub(in crate::workbench) fn validate_websocket_notes_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        editing::snapshot(&self.websocket.notes.editor, window, cx)
            .map(|_| ())
            .map_err(str::to_owned)
    }

    pub(in crate::workbench) fn websocket_notes_pending(&self) -> bool {
        self.websocket
            .notes
            .drafts
            .values()
            .any(|draft| draft.pending.is_some())
    }

    pub(in crate::workbench) fn snapshot_websocket_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<WebSocketNotesChange>, String> {
        self.capture_websocket_notes(window, cx)?;
        if self.websocket_notes_pending() {
            return Err("备注保存仍在确认；请等待当前回执后保存项目。".into());
        }
        Ok(self
            .websocket
            .notes
            .drafts
            .iter()
            .map(|(id, draft)| WebSocketNotesChange {
                message_id: *id,
                expected_notes_revision: draft.stored.revision,
                text: draft.text.clone(),
            })
            .collect())
    }

    pub(in crate::workbench) fn accept_saved_websocket_notes(
        &mut self,
        submitted: &[WebSocketNotesChange],
        receipts: &[(Uuid, u64)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if submitted.len() != receipts.len()
            || submitted.iter().any(|change| {
                receipts
                    .iter()
                    .filter(|(id, revision)| {
                        *id == change.message_id
                            && *revision >= change.expected_notes_revision
                            && *revision <= change.expected_notes_revision.saturating_add(1)
                    })
                    .count()
                    != 1
            })
        {
            return Err("项目保存的备注回执不匹配；本地草稿仍保留。".into());
        }
        for change in submitted {
            let revision = receipts
                .iter()
                .find(|(id, _)| *id == change.message_id)
                .unwrap()
                .1;
            let stored = WebSocketNotes {
                revision,
                text: change.text.clone(),
            };
            if let Some(draft) = self.websocket.notes.drafts.get_mut(&change.message_id) {
                draft.stored = stored.clone();
                draft.error = None;
                if draft.pending.is_none() && draft.text == change.text {
                    self.websocket.notes.drafts.remove(&change.message_id);
                }
            }
            if self.websocket.notes.active == Some(change.message_id) {
                self.websocket.notes.stored = Some(stored.clone());
                self.websocket.notes.error = None;
            }
            self.update_websocket_notes_summary(change.message_id, &stored, cx);
        }
        let _ = self.capture_websocket_notes(window, cx);
        self.websocket.dirty = true;
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn update_websocket_notes_summary(
        &mut self,
        id: Uuid,
        stored: &WebSocketNotes,
        cx: &mut Context<Self>,
    ) {
        let preview = stored.text.chars().take(256).collect::<String>();
        self.websocket.table.update(cx, |table, cx| {
            for row in &mut table.delegate_mut().rows {
                if row.id == id && row.notes_revision <= stored.revision {
                    row.notes_revision = stored.revision;
                    row.notes_preview = preview.clone();
                }
            }
            table.refresh(cx);
        });
        if let Some(summary) = self
            .websocket
            .selected_summary
            .as_mut()
            .filter(|row| row.id == id && row.notes_revision <= stored.revision)
        {
            summary.notes_revision = stored.revision;
            summary.notes_preview = preview;
        }
    }

    pub(in crate::workbench) fn websocket_notes_unsaved(&self, cx: &App) -> bool {
        let notes = &self.websocket.notes;
        !notes.drafts.is_empty()
            || notes.active.is_some()
                && notes
                    .stored
                    .as_ref()
                    .is_some_and(|stored| notes.editor.read(cx).value().as_ref() != stored.text)
    }

    #[cfg(test)]
    pub(in crate::workbench) fn flush_websocket_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        self.capture_websocket_notes(window, cx)?;
        self.send_websocket_notes(false, cx);
        if let Some(error) = self
            .websocket
            .notes
            .drafts
            .values()
            .find_map(|draft| draft.error.clone())
        {
            return Err(error);
        }
        Ok(!self.websocket_notes_unsaved(cx))
    }

    pub(in crate::workbench) fn capture_websocket_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let notes = &mut self.websocket.notes;
        let (Some(id), Some(stored)) = (notes.active, notes.stored.clone()) else {
            return Ok(());
        };
        let text = editing::snapshot(&notes.editor, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        if text.len() > MAX_NOTE_BYTES {
            return Err("备注超过 64 KiB；当前输入仍保留，请缩短后重试。".into());
        }
        if let Some(draft) = notes.drafts.get_mut(&id) {
            if draft.text != text {
                draft.text = text;
                draft.error = None;
            }
            let owned_by_project_save = self.project_saves_in_flight > 0
                && self
                    .project_notes_save
                    .as_ref()
                    .is_some_and(|batch| batch.iter().any(|change| change.message_id == id));
            if draft.pending.is_none() && !owned_by_project_save && draft.text == draft.stored.text
            {
                notes.drafts.remove(&id);
            }
        } else if text != stored.text {
            if notes.drafts.len() >= MAX_DRAFTS {
                return Err("已有 64 条备注待保存；当前输入仍保留，请先重试保存。".into());
            }
            notes.drafts.insert(
                id,
                Draft {
                    text,
                    stored,
                    pending: None,
                    error: None,
                },
            );
        }
        Ok(())
    }

    pub(in crate::workbench) fn send_websocket_notes(
        &mut self,
        retry: bool,
        cx: &mut Context<Self>,
    ) {
        if self.notes_save_wait.is_some()
            || self.pending_project_save.is_some()
            || self.choosing_save_as
            || self.project_saves_in_flight > 0
            || self.transition.is_some()
        {
            return;
        }
        let Some(project) = self.project else { return };
        let ids: Vec<_> = self
            .websocket
            .notes
            .drafts
            .iter()
            .filter(|(_, draft)| draft.pending.is_none() && (retry || draft.error.is_none()))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let draft = &self.websocket.notes.drafts[&id];
            let operation = Uuid::new_v4();
            let text = draft.text.clone();
            let cancel = CancellationToken::new();
            let sent = self.enqueue(
                Command::SetWebSocketNotes {
                    project,
                    operation_id: operation,
                    message_id: id,
                    expected_notes_revision: draft.stored.revision,
                    text: text.clone(),
                    cancel: cancel.clone(),
                },
                cx,
            );
            let draft = self.websocket.notes.drafts.get_mut(&id).unwrap();
            if sent {
                draft.error = None;
                draft.pending = Some(Pending {
                    operation,
                    text,
                    cancel: cancel.clone(),
                });
                let owner = cx.weak_entity();
                cx.spawn(async move |_, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(30))
                        .await;
                    let _ = owner.update(cx, |view, cx| {
                        if view.project != Some(project) {
                            return;
                        }
                        if let Some(draft) = view.websocket.notes.drafts.get_mut(&id)
                            && draft
                                .pending
                                .as_ref()
                                .is_some_and(|pending| pending.operation == operation)
                        {
                            cancel.cancel();
                            draft.error = Some(
                                "备注保存确认超时；已请求取消，草稿保留，等待实际结果。".into(),
                            );
                            cx.notify();
                        }
                    });
                })
                .detach();
            } else {
                draft.error = self.error.clone();
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
