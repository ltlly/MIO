use super::*;

fn ensure_fallback_live_numbers(
    live_numbers: &mut HashMap<FlowId, u64>,
    rows: &[HistoryRow],
    selected_rows: &[HistoryRow],
) {
    let mut max_known = live_numbers.values().copied().max().unwrap_or(0);
    for row in rows.iter().chain(selected_rows) {
        if let HistoryRow::ExtProxy(archive) = row {
            max_known = max_known.max(archive.item.index.saturating_add(1));
        }
    }
    let mut unnumbered: Vec<&mio_core::FlowSummary> = Vec::new();
    for row in rows.iter().chain(selected_rows) {
        if let HistoryRow::Live(flow) = row
            && !live_numbers.contains_key(&flow.id)
            && !unnumbered.iter().any(|existing| existing.id == flow.id)
        {
            unnumbered.push(flow);
        }
    }
    unnumbered.sort_by(|a, b| {
        a.started_unix_ms
            .cmp(&b.started_unix_ms)
            .then_with(|| b.id.as_hyphenated().cmp(a.id.as_hyphenated()))
    });
    for flow in unnumbered {
        max_known = max_known.saturating_add(1);
        live_numbers.insert(flow.id, max_known);
    }
}

impl Workbench {
    pub(in crate::workbench) fn apply_http_history_page(
        &mut self,
        offset: usize,
        total: usize,
        mut rows: Vec<HistoryRow>,
        selection: Option<HistorySelectionPage<'_>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.refresh_http_notes_rows(&mut rows, cx);
        self.refresh_http_highlight_rows(&mut rows, cx);
        let legacy_selection = selection.is_none();
        let mut fallback_selected = Vec::new();
        if let Some(((matches, ranks), mut selected_rows)) = selection {
            self.refresh_http_notes_rows(&mut selected_rows, cx);
            self.refresh_http_highlight_rows(&mut selected_rows, cx);
            self.retain_history_query_selection(
                &rows,
                &selected_rows,
                (matches, ranks),
                window,
                cx,
            )?;
            fallback_selected = selected_rows;
        }
        self.history_loading = false;
        self.has_traffic |= total > 0;
        let selected = self
            .selected_history
            .and_then(|id| rows.iter().position(|row| row.id() == id));
        let reread = selected.and_then(|index| {
            let next = rows[index].live()?;
            let revision = self
                .detail
                .as_ref()
                .map(|record| record.summary.revision)
                .or_else(|| {
                    self.history
                        .read(cx)
                        .delegate()
                        .rows
                        .iter()
                        .filter_map(HistoryRow::live)
                        .find(|previous| previous.id == next.id)
                        .map(|previous| previous.revision)
                });
            revision
                .is_none_or(|revision| revision < next.revision)
                .then_some(next.id)
        });
        let archive = selected.and_then(|index| {
            matches!(rows[index], HistoryRow::ExtProxy(_)).then(|| rows[index].clone())
        });
        let auto_scroll = self.http_filter.pending_auto_scroll && self.http_filter.continuous_scroll;
        self.http_filter.pending_auto_scroll = false;
        let requested_sort = self
            .http_filter
            .pending
            .as_ref()
            .map_or(self.http_filter.sort, |pending| pending.query.sort);
        let sort_changed = requested_sort != self.http_filter.sort;
        if selected.is_none() && legacy_selection {
            if !self.leave_http_notes(window, cx) {
                self.history.update(cx, |table, cx| {
                    table.delegate_mut().loading = false;
                    table.refresh(cx);
                });
                return Err("当前备注输入尚未提交；列表保留。".into());
            }
            self.selected_flow = None;
            self.selected_history = None;
            self.reset_history_version_view();
            self.clear_history_detail(window, cx);
            self.reset_imported_history(window, cx);
        }
        self.history.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            ensure_fallback_live_numbers(&mut delegate.live_numbers, &rows, &fallback_selected);
            if offset == 0 {
                delegate.previous_offsets.clear();
            } else if offset > delegate.offset {
                delegate.previous_offsets.push(delegate.offset);
            } else if offset < delegate.offset {
                while delegate
                    .previous_offsets
                    .last()
                    .is_some_and(|previous| *previous >= offset)
                {
                    delegate.previous_offsets.pop();
                }
            }
            if legacy_selection {
                delegate
                    .selected_ids
                    .retain(|id| rows.iter().any(|row| row.id() == *id));
                if delegate
                    .selection_anchor
                    .is_some_and(|id| !rows.iter().any(|row| row.id() == id))
                {
                    delegate.selection_anchor = None;
                }
                if delegate
                    .selection_lead
                    .is_some_and(|id| !rows.iter().any(|row| row.id() == id))
                {
                    delegate.selection_lead = None;
                }
            }
            if legacy_selection {
                delegate
                    .selected_rows
                    .retain(|id, _| delegate.selected_ids.contains(id));
                delegate
                    .selected_order
                    .retain(|id| delegate.selected_ids.contains(id));
                delegate
                    .selected_ranks
                    .retain(|id, _| delegate.selected_ids.contains(id));
                for (index, row) in rows.iter().enumerate() {
                    if delegate.selected_ids.contains(&row.id()) {
                        delegate.selected_rows.insert(row.id(), row.clone());
                        delegate.selected_ranks.insert(row.id(), offset + index);
                    }
                }
            }
            let offset_changed = delegate.offset != offset;
            let has_rows = !rows.is_empty();
            let row_count = rows.len();
            delegate.rows = rows;
            delegate.offset = offset;
            delegate.total = total;
            delegate.loading = false;
            if let Some(row) = selected {
                if state.selected_row() != Some(row) {
                    state.set_selected_row_without_scroll(row, cx);
                }
            } else {
                state.clear_selection(cx);
            }
            if auto_scroll && has_rows {
                let target = if requested_sort.column == mio_core::HttpHistorySortColumn::Number
                    && requested_sort.ascending
                {
                    row_count - 1
                } else {
                    0
                };
                state.scroll_to_row(target, cx);
            } else if (sort_changed && offset == 0 && has_rows)
                || (offset_changed && has_rows && selected.is_none())
            {
                state.scroll_to_row(0, cx);
            }
            state.refresh(cx);
        });
        if let Some(id) = reread {
            let error = self.error.take();
            self.select_flow(id, window, cx);
            self.error = self.error.take().or(error);
        } else if let Some(row) = archive {
            self.refresh_imported_history(&row, window, cx);
        }
        Ok(())
    }

    pub(in crate::workbench) fn footer_status(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> String {
        if self.shutdown_requested {
            return self.status.clone();
        }
        if self.backend_disconnected {
            return if self.proxy_address.is_some() || self.proxy_busy {
                "后台已断开；Proxy 停止结果未确认；保存结果未确认"
            } else {
                "后台已断开；保存结果未确认"
            }
            .into();
        }
        let unsaved = self.workspace_dirty
            || self.websocket_notes_unsaved(cx)
            || self.http_notes_unsaved(cx)
            || self.http_highlights_unsaved()
            || self.pending_records > 0
            || self.intruder_unsaved(window, cx)
            || self.websocket_repeater_unsaved(window, cx)
            || self.repeater_target_dialog_unsaved(cx)
            || self.tab_editors.values().any(|editors| {
                editors.dirty
                    || [&editors.request.raw, &editors.request.hex]
                        .into_iter()
                        .any(|input| {
                            input.update(cx, |state, cx| {
                                state.marked_text_range(window, cx).is_some()
                            })
                        })
                    || editors.target.update(cx, |state, cx| {
                        state.marked_text_range(window, cx).is_some()
                    })
            });
        let saving = self.pending_workspace.is_some()
            || self.websocket_notes_pending()
            || self.http_notes_pending()
            || self.http_highlights_pending()
            || self.notes_save_wait.is_some()
            || self.pending_project_save.is_some()
            || self.project_saves_in_flight > 0;
        let save_status = match (unsaved, saving) {
            (true, true) => "正在保存；仍有未保存的更改",
            (true, false) => "有未保存的更改",
            (false, true) => "正在保存…",
            (false, false) => self.project_save_status.as_str(),
        };
        if save_status.is_empty() {
            self.status.clone()
        } else {
            format!("{save_status} · {}", self.status)
        }
    }

    pub(in crate::workbench) fn update_title(&self, window: &mut Window) {
        if self.startup_wizard_active() {
            window.set_window_title("MIO");
            return;
        }
        let name = self
            .workspace
            .project_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.project_path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "临时项目".into());
        window.set_window_title(&format!("MIO — {name}"));
    }

    pub(crate) fn sort_http_history(
        &mut self,
        sort: mio_core::HttpHistorySort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (filter, _, enabled) = self.http_filter.requested();
        if self
            .query_http_history_filtered(filter, sort, enabled, 0, window, cx)
            .is_none()
        {
            let applied = self.http_filter.sort;
            self.history.update(cx, |table, cx| {
                table.delegate_mut().apply_sort(applied);
                table.refresh(cx);
            });
        }
    }

    pub(in crate::workbench) fn query_history(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (filter, sort, enabled) = self.http_filter.requested();
        self.query_http_history_filtered(filter, sort, enabled, offset, window, cx);
    }

    pub(in crate::workbench) fn clear_history_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_request.has_focus(window, cx) || self.history_response.has_focus(window, cx)
        {
            self.focus.focus(window, cx);
        }
        self.detail = None;
        self.detail_error = None;
        *self.detail_original = OriginalMessages::default();
        self.history_request.load(&[], window, cx);
        self.history_response.load(&[], window, cx);
        self.clear_context_viewer("history", cx);
    }

    pub(in crate::workbench) fn select_history_entry(
        &mut self,
        id: HistoryEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_history != Some(id) && !self.leave_http_notes(window, cx) {
            return;
        }
        match id {
            HistoryEntryId::Live(flow) => self.select_flow(flow, window, cx),
            HistoryEntryId::External { .. } => {
                let row = self.history.read(cx).delegate().row_by_id(id).cloned();
                let Some(row) = row else { return };
                if self.selected_history != Some(id) {
                    self.clear_history_detail(window, cx);
                    self.reset_imported_history(window, cx);
                    self.selection_revision = self.selection_revision.saturating_add(1);
                    self.history_detail_revision = self.selection_revision;
                }
                self.selected_flow = None;
                self.selected_history = Some(id);
                self.reset_history_version_view();
                self.refresh_imported_history(&row, window, cx);
                self.select_http_notes(window, cx);
            }
        }
    }

    pub(in crate::workbench) fn select_flow(
        &mut self,
        flow_id: FlowId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let same_flow = self.selected_flow == Some(flow_id);
        if !same_flow && !self.leave_http_notes(window, cx) {
            return;
        }
        if same_flow
            && self.detail.as_ref().is_some_and(|record| {
                self.history
                    .read(cx)
                    .delegate()
                    .row_by_id(HistoryEntryId::Live(flow_id))
                    .and_then(HistoryRow::live)
                    .is_some_and(|row| row.revision <= record.summary.revision)
            })
        {
            return;
        }
        self.selection_revision += 1;
        self.history_detail_revision = self.selection_revision;
        self.clear_history_detail(window, cx);
        self.selected_flow = Some(flow_id);
        self.selected_history = Some(HistoryEntryId::Live(flow_id));
        self.reset_imported_history(window, cx);
        if !same_flow {
            self.reset_history_version_view();
        }
        if !self.enqueue(
            Command::ReadFlow {
                project,
                selection_revision: self.selection_revision,
                flow_id,
            },
            cx,
        ) {
            self.detail_error = self.error.clone();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn reset_history_version_view(&mut self) {
        self.history_original_view = false;
        self.history_request_original_view = false;
        self.history_response_original_view = false;
    }

    pub(in crate::workbench) fn sync_history_original_view_flag(&mut self) {
        self.history_original_view =
            self.history_request_original_view || self.history_response_original_view;
    }

    pub(in crate::workbench) fn reload_history_detail_versions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.detail.as_ref() else {
            return;
        };
        let request = if self.history_request_original_view {
            self.detail_original
                .request
                .as_ref()
                .unwrap_or(&record.request)
        } else {
            &record.request
        };
        let response = if self.history_response_original_view {
            self.detail_original
                .response
                .as_ref()
                .or(record.response.as_ref())
        } else {
            record.response.as_ref()
        };
        self.history_request
            .load(&request_to_raw(request), window, cx);
        self.history_response.load(
            &response.map(response_to_raw).unwrap_or_default(),
            window,
            cx,
        );
        if crate::theme::auto_expand_inspector() {
            self.inspector.open = true;
        }
        self.sync_history_original_view_flag();
        cx.notify();
    }

    pub(in crate::workbench) fn show_history_request_version(
        &mut self,
        original: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.detail.as_ref() else {
            return;
        };
        let request = if original {
            self.detail_original
                .request
                .as_ref()
                .unwrap_or(&record.request)
        } else {
            &record.request
        };
        self.history_request
            .load(&request_to_raw(request), window, cx);
        if self.history_request_original_view != original {
            self.clear_context_viewer("history", cx);
        }
        if crate::theme::auto_expand_inspector() {
            self.inspector.open = true;
        }
        self.history_request_original_view = original;
        self.sync_history_original_view_flag();
        cx.notify();
    }

    pub(in crate::workbench) fn show_history_response_version(
        &mut self,
        original: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.detail.as_ref() else {
            return;
        };
        let response = if original {
            self.detail_original
                .response
                .as_ref()
                .or(record.response.as_ref())
        } else {
            record.response.as_ref()
        };
        self.history_response.load(
            &response.map(response_to_raw).unwrap_or_default(),
            window,
            cx,
        );
        if self.history_response_original_view != original {
            self.clear_context_viewer("history", cx);
        }
        if crate::theme::auto_expand_inspector() {
            self.inspector.open = true;
        }
        self.history_response_original_view = original;
        self.sync_history_original_view_flag();
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::workbench) fn show_history_version(
        &mut self,
        original: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.history_request_original_view != original
            || self.history_response_original_view != original
            || self.history_original_view != original;
        self.history_request_original_view = original;
        self.history_response_original_view = original;
        self.history_original_view = original;
        if changed {
            self.clear_context_viewer("history", cx);
        }
        self.reload_history_detail_versions(window, cx);
    }
}
