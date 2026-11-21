use super::*;

impl State {
    pub(super) async fn command_notes_and_repeater(&mut self, command: Command) -> Result<()> {
        match command {
            Command::SetWebSocketNotes {
                project,
                operation_id,
                message_id,
                expected_notes_revision,
                text,
                cancel,
            } => {
                let result = self.set_websocket_notes(
                    project,
                    operation_id,
                    message_id,
                    expected_notes_revision,
                    &text,
                    &cancel,
                );
                let changed = result
                    .as_ref()
                    .is_ok_and(|notes| notes.revision != expected_notes_revision);
                self.emit(Event::WebSocketNotesChanged {
                    project,
                    operation_id,
                    message_id,
                    result,
                })
                .await;
                if changed {
                    self.websocket_history_changed().await;
                }
            }
            Command::ReadExtHttpNotes {
                project,
                selection_revision,
                source_id,
                item_id,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.store.ext_http_notes(project, source_id, item_id))
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtHttpNotes {
                    project,
                    selection_revision,
                    source_id,
                    item_id,
                    result,
                })
                .await;
            }
            Command::SetExtHttpNotes {
                project,
                operation_id,
                change,
                cancel,
            } => {
                let ready = self.check_project(project).and_then(|()| {
                    ensure!(!operation_id.is_nil(), "HTTP 备注操作 ID 不能为空");
                    ensure!(!cancel.is_cancelled(), "HTTP 备注修改已取消");
                    self.require_saved()?;
                    self.require_ext_released()
                });
                let result = match ready {
                    Ok(()) => self.store.set_ext_http_notes(
                        project,
                        change.source_id,
                        change.item_id,
                        change.expected_notes_revision,
                        &change.text,
                        &cancel,
                    ),
                    Err(error) => Err(ExtHttpNotesError {
                        message: error.to_string(),
                        stored: if project == self.project {
                            self.store
                                .ext_http_notes(project, change.source_id, change.item_id)
                                .ok()
                        } else {
                            None
                        },
                    }),
                };
                let changed = result
                    .as_ref()
                    .is_ok_and(|notes| notes.revision != change.expected_notes_revision);
                self.emit(Event::ExtHttpNotesChanged {
                    project,
                    operation_id,
                    source_id: change.source_id,
                    item_id: change.item_id,
                    result,
                })
                .await;
                if changed {
                    self.history_changed().await;
                }
            }
            Command::ReadExtHttpNotesBatch {
                project,
                operation_id,
                ids,
                cancel,
            } => {
                self.store
                    .cancel_queries_with_operation(self.cancel.clone(), cancel.clone());
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        ensure!(!operation_id.is_nil(), "HTTP 批量备注操作 ID 不能为空");
                        self.store.ext_http_notes_batch(project, &ids, &cancel)
                    })
                    .map_err(|error| error.to_string());
                self.store.cancel_queries_with(self.cancel.clone());
                self.emit(Event::ExtHttpNotesBatch {
                    project,
                    operation_id,
                    result,
                })
                .await;
            }
            Command::SetExtHttpNotesBatch {
                project,
                operation_id,
                changes,
                text,
                cancel,
            } => {
                let ready = self.check_project(project).and_then(|()| {
                    ensure!(!operation_id.is_nil(), "HTTP 批量备注操作 ID 不能为空");
                    ensure!(!cancel.is_cancelled(), "HTTP 批量备注修改已取消");
                    self.require_saved()?;
                    self.require_ext_released()
                });
                self.store
                    .cancel_queries_with_operation(self.cancel.clone(), cancel.clone());
                let result = match ready {
                    Ok(()) => self
                        .store
                        .set_ext_http_notes_batch(project, &changes, &text, &cancel),
                    Err(error) => Err(ExtHttpNotesBatchError {
                        message: error.to_string(),
                        stored: if project == self.project
                            && !cancel.is_cancelled()
                            && changes.len() <= 500
                        {
                            let ids: Vec<_> = changes
                                .iter()
                                .map(|&(source, item, _)| (source, item))
                                .collect();
                            self.store
                                .ext_http_notes_batch(project, &ids, &cancel)
                                .map(|batch| batch.states)
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        },
                    }),
                };
                self.store.cancel_queries_with(self.cancel.clone());
                let changed = result.as_ref().is_ok_and(|saved| {
                    saved
                        .iter()
                        .zip(&changes)
                        .any(|((_, _, revision), (_, _, expected))| revision != expected)
                });
                self.emit(Event::ExtHttpNotesBatchChanged {
                    project,
                    operation_id,
                    result,
                })
                .await;
                if changed {
                    self.history_changed().await;
                }
            }
            Command::SetExtHttpHighlights {
                project,
                operation_id,
                changes,
                cancel,
            } => {
                let ready = self.check_project(project).and_then(|()| {
                    ensure!(!operation_id.is_nil(), "HTTP 高亮操作 ID 不能为空");
                    ensure!(!cancel.is_cancelled(), "HTTP 高亮修改已取消");
                    self.require_saved()?;
                    self.require_ext_released()
                });
                self.store
                    .cancel_queries_with_operation(self.cancel.clone(), cancel.clone());
                let result = match ready {
                    Ok(()) => self
                        .store
                        .set_ext_http_highlights(project, &changes, &cancel),
                    Err(error) => Err(ExtHttpHighlightError {
                        message: error.to_string(),
                        stored: if project == self.project && !cancel.is_cancelled() {
                            self.store
                                .ext_http_highlights(project, &changes)
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        },
                    }),
                };
                self.store.cancel_queries_with(self.cancel.clone());
                let changed = result.as_ref().is_ok_and(|saved| {
                    saved.iter().zip(&changes).any(|((_, _, value), change)| {
                        value.revision != change.expected_highlight_revision
                    })
                });
                self.emit(Event::ExtHttpHighlightsChanged {
                    project,
                    operation_id,
                    result,
                })
                .await;
                if changed {
                    self.history_changed().await;
                }
            }
            Command::NewRepeater { project } => {
                self.check_project(project)?;
                self.open_repeater(None).await?;
            }
            Command::OpenRepeater {
                project,
                flow_id,
                original,
            } => {
                self.check_project(project)?;
                let record = self.read_flow(flow_id)?;
                let original = if original {
                    self.original_flow(flow_id)?.request
                } else {
                    None
                };
                self.open_repeater(Some(original.as_ref().unwrap_or(&record.request)))
                    .await?;
            }
            Command::Send {
                project,
                tab_id,
                run_id,
                draft,
                settings,
                group_send,
                cancel,
            } => {
                let revision = draft.revision;
                if let Err(error) = self
                    .begin_send(
                        project,
                        tab_id,
                        run_id,
                        draft,
                        settings,
                        group_send,
                        cancel,
                    )
                    .await
                {
                    self.emit(Event::SendRejected {
                        project,
                        tab_id,
                        run_id,
                        draft_revision: revision,
                        message: error.to_string(),
                    })
                    .await;
                }
            }
            Command::CancelSend {
                project,
                tab_id,
                run_id,
                draft_revision,
            } => {
                self.check_project(project)?;
                if let Some(active) = self.sends.get_mut(&tab_id)
                    && active.flow_id == run_id
                    && active.draft_revision == draft_revision
                {
                    active.cancel.cancel();
                    active.progress_rx = None;
                }
            }
            _ => unreachable!("routed by State::command"),
        }
        Ok(())
    }
}
