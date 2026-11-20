use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_http_highlights(
        &mut self,
        event: Event,
        cx: &mut Context<Self>,
    ) {
        let Event::ExtHttpHighlightsChanged {
            project,
            operation_id,
            result,
        } = event
        else {
            return;
        };
        if self.project != Some(project)
            || self
                .http_highlights
                .pending
                .as_ref()
                .is_none_or(|pending| pending.operation != operation_id)
        {
            return;
        }
        let pending = self.http_highlights.pending.take().unwrap();
        match result {
            Ok(stored) if valid_receipts(&pending.changes, &stored) => {
                self.accept_http_highlight_receipts(&stored, cx);
                self.history_dirty = true;
                self.send_http_highlights(false, cx);
            }
            Ok(_) => self.reject_http_highlights(
                &pending.changes,
                ExtHttpHighlightError {
                    message: "Highlight 保存回执内容或 revision 不匹配；当前选择保留。".into(),
                    stored: Vec::new(),
                },
                cx,
            ),
            Err(error) => self.reject_http_highlights(&pending.changes, error, cx),
        }
    }

    fn reject_http_highlights(
        &mut self,
        changes: &[ExtHttpHighlightChange],
        error: ExtHttpHighlightError,
        cx: &mut Context<Self>,
    ) {
        let mut seen = HashSet::new();
        let valid_stored = error.stored.iter().all(|(source, item, stored)| {
            stored.color <= 9
                && seen.insert((*source, *item))
                && changes.iter().any(|change| {
                    (change.source_id, change.item_id) == (*source, *item)
                        && stored.revision >= change.expected_highlight_revision
                })
        });
        for change in changes {
            let id = (change.source_id, change.item_id);
            if let Some(draft) = self.http_highlights.drafts.get_mut(&id) {
                if valid_stored
                    && let Some((_, _, stored)) = error
                        .stored
                        .iter()
                        .find(|(source, item, _)| (*source, *item) == id)
                {
                    draft.stored = stored.clone();
                }
                draft.error = Some(error.message.clone());
            }
        }
        self.update_http_highlight_rows(cx);
        self.fail(error.message, cx);
        self.send_http_highlights(false, cx);
    }

    pub(in crate::workbench) fn accept_saved_http_highlights(
        &mut self,
        submitted: &[ExtHttpHighlightChange],
        receipts: &[(Uuid, Uuid, u64)],
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let stored: Vec<_> = receipts
            .iter()
            .filter_map(|(source, item, revision)| {
                submitted
                    .iter()
                    .find(|change| (change.source_id, change.item_id) == (*source, *item))
                    .map(|change| {
                        (
                            *source,
                            *item,
                            ExtHttpHighlight {
                                revision: *revision,
                                color: change.color,
                            },
                        )
                    })
            })
            .collect();
        if stored.len() != receipts.len() || !valid_receipts(submitted, &stored) {
            return Err("项目保存的 Highlight 回执不匹配；当前选择仍保留。".into());
        }
        self.accept_http_highlight_receipts(&stored, cx);
        self.history_dirty = true;
        Ok(())
    }

    fn accept_http_highlight_receipts(
        &mut self,
        receipts: &[(Uuid, Uuid, ExtHttpHighlight)],
        cx: &mut Context<Self>,
    ) {
        for (source, item, stored) in receipts {
            let id = (*source, *item);
            if let Some(draft) = self.http_highlights.drafts.get_mut(&id) {
                draft.stored = stored.clone();
                draft.error = None;
                if draft.color == stored.color {
                    self.http_highlights.drafts.remove(&id);
                }
            }
        }
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            for (source, item, stored) in receipts {
                if source.is_nil()
                    && delegate
                        .live_highlights
                        .borrow()
                        .get(item)
                        .is_none_or(|previous| previous.revision <= stored.revision)
                {
                    delegate
                        .live_highlights
                        .borrow_mut()
                        .insert(*item, stored.clone());
                    delegate.live_highlight_errors.borrow_mut().remove(item);
                }
            }
            for row in delegate.cached_rows_mut() {
                if let HistoryRow::ExtProxy(row) = row
                    && let Some((_, _, stored)) = receipts.iter().find(|(source, item, _)| {
                        (*source, *item) == (row.item.source_id, row.item.id)
                    })
                    && row
                        .highlight
                        .as_ref()
                        .is_none_or(|previous| previous.revision <= stored.revision)
                {
                    row.highlight = Some(stored.clone());
                    row.highlight_error = None;
                }
            }
            table.refresh(cx);
        });
        self.update_http_highlight_rows(cx);
    }

    pub(in crate::workbench) fn update_http_highlight_rows(&mut self, cx: &mut Context<Self>) {
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            for ((source, item), draft) in &self.http_highlights.drafts {
                if source.is_nil() {
                    delegate.live_highlights.borrow_mut().insert(
                        *item,
                        ExtHttpHighlight {
                            revision: draft.stored.revision,
                            color: draft.color,
                        },
                    );
                    if let Some(error) = &draft.error {
                        delegate
                            .live_highlight_errors
                            .borrow_mut()
                            .insert(*item, error.clone());
                    } else {
                        delegate.live_highlight_errors.borrow_mut().remove(item);
                    }
                }
            }
            for row in delegate.cached_rows_mut() {
                if let HistoryRow::ExtProxy(row) = row
                    && let Some(draft) = self
                        .http_highlights
                        .drafts
                        .get(&(row.item.source_id, row.item.id))
                {
                    row.highlight = Some(ExtHttpHighlight {
                        revision: draft.stored.revision,
                        color: draft.color,
                    });
                    row.highlight_error = draft.error.clone();
                }
            }
            table.refresh(cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn refresh_http_highlight_rows(
        &self,
        rows: &mut [HistoryRow],
        cx: &App,
    ) {
        if self.project.is_some() {
            let delegate = self.history.read(cx).delegate();
            for (flow_id, _, _, highlight) in &self.live_http_annotations {
                let flow_id = *flow_id;
                let highlight = highlight.clone();
                let key = (Uuid::nil(), flow_id);
                if !self.http_highlights.drafts.contains_key(&key)
                    && delegate
                        .live_highlights
                        .borrow()
                        .get(&flow_id)
                        .is_none_or(|prev| prev.revision <= highlight.revision)
                {
                    delegate
                        .live_highlights
                        .borrow_mut()
                        .insert(flow_id, highlight);
                }
            }
        }
        for row in rows {
            let HistoryRow::ExtProxy(row) = row else {
                continue;
            };
            let id = (row.item.source_id, row.item.id);
            if let Some(draft) = self.http_highlights.drafts.get(&id) {
                row.highlight = Some(ExtHttpHighlight {
                    revision: draft.stored.revision,
                    color: draft.color,
                });
                row.highlight_error = draft.error.clone();
            } else if let Some(HistoryRow::ExtProxy(previous)) = self
                .history
                .read(cx)
                .delegate()
                .row_by_id(HistoryEntryId::External {
                    source_id: id.0,
                    item_id: id.1,
                })
                && let Some(stored) = &previous.highlight
                && row
                    .highlight
                    .as_ref()
                    .is_some_and(|next| next.revision < stored.revision)
            {
                row.highlight = Some(stored.clone());
                row.highlight_error = previous.highlight_error.clone();
            }
        }
    }
}
