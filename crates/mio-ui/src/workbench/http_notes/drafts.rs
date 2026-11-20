use super::*;

impl Workbench {
    pub(in crate::workbench) fn http_notes_unsaved(&self, cx: &App) -> bool {
        let notes = &self.http_notes;
        notes.batch.is_some()
            || !notes.drafts.is_empty()
            || notes.active.is_some()
                && notes
                    .stored
                    .as_ref()
                    .is_some_and(|stored| notes.editor.read(cx).value().as_ref() != stored.text)
    }

    pub(in crate::workbench) fn send_http_notes(&mut self, retry: bool, cx: &mut Context<Self>) {
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
            .http_notes
            .drafts
            .iter()
            .filter(|(id, draft)| {
                draft.pending.is_none()
                    && (retry || draft.error.is_none())
                    && self
                        .http_notes
                        .batch
                        .as_ref()
                        .is_none_or(|batch| !batch.ids.contains(id))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let draft = &self.http_notes.drafts[&id];
            let operation = Uuid::new_v4();
            let text = draft.text.clone();
            let cancel = CancellationToken::new();
            let sent = self.enqueue(
                Command::SetExtHttpNotes {
                    project,
                    operation_id: operation,
                    change: ExtHttpNotesChange {
                        source_id: id.0,
                        item_id: id.1,
                        expected_notes_revision: draft.stored.revision,
                        text: text.clone(),
                    },
                    cancel: cancel.clone(),
                },
                cx,
            );
            let draft = self.http_notes.drafts.get_mut(&id).unwrap();
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
                        if let Some(draft) = view.http_notes.drafts.get_mut(&id)
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

    pub(in crate::workbench) fn leave_http_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Err(error) = self.capture_http_notes(window, cx) {
            self.http_notes.error = Some(error.clone());
            self.fail(error, cx);
            return false;
        }
        self.send_http_notes(false, cx);
        if self.http_notes.open {
            let owner = cx.weak_entity();
            let handle = window.window_handle();
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    owner.update(cx, |view, cx| {
                        if view.http_notes.open
                            && let Some(id) = view.selected_http_notes()
                            && view.http_notes.active != Some(id)
                        {
                            view.select_http_notes(window, cx);
                        }
                    })
                });
            });
        }
        true
    }

    pub(in crate::workbench) fn select_http_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.http_notes.open {
            return;
        }
        let Some(id) = self.selected_http_notes() else {
            return;
        };
        if self.http_notes.active == Some(id)
            && (self.http_notes.stored.is_some() || self.http_notes.reading)
        {
            return;
        }
        if self.http_notes.editor.focus_handle(cx).is_focused(window) {
            self.focus.focus(window, cx);
        }
        let Some(project) = self.project else { return };
        let notes = &mut self.http_notes;
        notes.active = Some(id);
        notes.stored = None;
        notes.error = None;
        notes.read_revision = match notes.read_revision.checked_add(1) {
            Some(next) => next,
            None => {
                notes.error = Some("备注读取 revision 已耗尽，请重新打开项目。".into());
                return;
            }
        };
        if let Some(draft) = notes.drafts.get(&id) {
            notes.stored = Some(draft.stored.clone());
            notes.error = draft.error.clone();
            let text = draft.text.clone();
            notes
                .editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
            notes.reading = false;
        } else {
            let revision = notes.read_revision;
            notes.reading = true;
            notes
                .editor
                .update(cx, |editor, cx| editor.set_value("", window, cx));
            if !self.enqueue(
                Command::ReadExtHttpNotes {
                    project,
                    selection_revision: revision,
                    source_id: id.0,
                    item_id: id.1,
                },
                cx,
            ) {
                self.http_notes.reading = false;
                self.http_notes.error = self.error.clone();
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_http_notes(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::ExtHttpNotes {
                project,
                selection_revision,
                source_id,
                item_id,
                result,
            } if self.project == Some(project)
                && self.http_notes.active == Some((source_id, item_id))
                && self.http_notes.read_revision == selection_revision =>
            {
                let notes = &mut self.http_notes;
                notes.reading = false;
                match result {
                    Ok(stored) => {
                        let text = stored.text.clone();
                        notes.stored = Some(stored);
                        notes.error = None;
                        notes
                            .editor
                            .update(cx, |editor, cx| editor.set_value(text, window, cx));
                    }
                    Err(error) => notes.error = Some(error),
                }
            }
            Event::ExtHttpNotesChanged {
                project,
                operation_id,
                source_id,
                item_id,
                result,
            } if self.project == Some(project) => {
                self.receive_http_notes((source_id, item_id), operation_id, result, window, cx)
            }
            _ => {}
        }
        cx.notify();
    }

    fn receive_http_notes(
        &mut self,
        id: (Uuid, Uuid),
        operation: Uuid,
        result: Result<ExtHttpNotes, ExtHttpNotesError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .http_notes
            .drafts
            .get(&id)
            .and_then(|draft| draft.pending.as_ref())
            .is_none_or(|pending| pending.operation != operation)
        {
            return;
        }
        let _ = self.capture_http_notes(window, cx);
        let draft = self.http_notes.drafts.get_mut(&id).unwrap();
        let pending = draft.pending.take().unwrap();
        match result {
            Ok(stored)
                if (stored.revision == draft.stored.revision
                    || Some(stored.revision) == draft.stored.revision.checked_add(1))
                    && stored.text == pending.text =>
            {
                draft.stored = stored.clone();
                draft.error = None;
                if self.http_notes.active == Some(id) {
                    self.http_notes.stored = Some(stored.clone());
                    self.http_notes.error = None;
                }
                if draft.text == stored.text {
                    self.http_notes.drafts.remove(&id);
                }
                self.update_http_notes_summary(id, &stored, cx);
                self.history_dirty = true;
                self.send_http_notes(false, cx);
            }
            Ok(_) => {
                draft.error =
                    Some("备注保存回执内容或 revision 不匹配；草稿保留，请重新读取后重试。".into())
            }
            Err(error) => {
                if let Some(stored) = error.stored {
                    if self.http_notes.active == Some(id) {
                        self.http_notes.stored = Some(stored.clone());
                    }
                    draft.stored = stored;
                }
                draft.error = Some(error.message);
            }
        }
    }
}
