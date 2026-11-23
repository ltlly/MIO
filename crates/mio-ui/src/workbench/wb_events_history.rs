use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_tool_and_history_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ControlFlow<()> {
        match event {
            Event::SiteMapEntryAdded { project, operation_id, workspace_revision, result } => {
                self.on_site_map_entry_added(project, operation_id, workspace_revision, result, window, cx);
            }
            Event::SiteMapEntryRead { project, selection_revision, entry_id, revision, result } => {
                self.on_site_map_entry_read(project, selection_revision, entry_id, revision, result, window, cx);
            }
            event @ Event::ExtResponse { .. } => {
                if let Event::ExtResponse {
                    project,
                    selection_revision,
                    ref reference,
                    offset,
                    ref result,
                } = event
                    && self.on_target_ext_response(
                        project,
                        selection_revision,
                        reference,
                        offset,
                        result.clone(),
                        window,
                        cx,
                    )
                {
                    return ControlFlow::Break(());
                }
                self.on_archive_response(event, window, cx);
            }
            event @ Event::ExtMessage { .. } => {
                if let Event::ExtMessage {
                    project,
                    selection_revision,
                    message_id,
                    offset,
                    ref result,
                } = event
                    && self.on_target_ext_message(
                        project,
                        selection_revision,
                        message_id,
                        offset,
                        result.clone(),
                        window,
                        cx,
                    )
                {
                    return ControlFlow::Break(());
                }
                if !self.on_imported_history(event.clone(), window, cx) {
                    self.on_ext_event(event, window, cx);
                }
            }
            Event::SiteMapPage {
                project,
                query_revision,
                query,
                result,
            } => {
                self.on_site_map_page(project, query_revision, query, result, window, cx);
            }
            Event::GlobalSearchPage { query, result } => {
                self.on_global_search_page(query, result, window, cx);
            }
            Event::Sequencer(event) => {
                self.on_sequencer_event(event, window, cx);
            }
            Event::Audit(event) => {
                self.on_audit_event(event, window, cx);
            }
            Event::Collaborator(event) => {
                self.on_collaborator_event(event, window, cx);
            }
            event @ (Event::RepeaterHistoryPage { .. } | Event::RepeaterHistorySelected { .. }) => {
                self.on_repeater_history(event, window, cx);
            }
            event @ (Event::ExtJob { .. }
            | Event::ExtProgress { .. }
            | Event::ExtReport { .. }
            | Event::ExtItems { .. }
            | Event::ExtItem { .. }
            | Event::ExtRepeaterOpened { .. }
            | Event::ExtMessages { .. }) => self.on_ext_event(event, window, cx),
            event @ (Event::WebSocketHistoryChanged { .. }
            | Event::WebSocketHistoryPage { .. }
            | Event::WebSocketHistoryError { .. }
            | Event::WebSocketDetail { .. }
            | Event::WebSocketDetailError { .. }) => {
                self.on_websocket_event(event, window, cx);
            }
            event @ (Event::WebSocketNotes { .. } | Event::WebSocketNotesChanged { .. }) => {
                self.on_websocket_notes_event(event, window, cx);
            }
            Event::WebSocketRepeater(event) => {
                self.on_websocket_repeater_event(event, window, cx);
            }
            event @ Event::IntruderPositions { .. } => {
                self.on_intruder_positions_event(event, window, cx);
            }
            event @ Event::IntruderStarted { .. } => {
                let changed = matches!(&event, Event::IntruderStarted { project, result: Ok(_), .. } if self.project == Some(*project));
                self.on_intruder_event(event, window, cx);
                if changed {
                    self.settings_pool_usage_changed(cx);
                }
            }
            Event::IntruderDefaultPoolReady {
                project,
                operation_id,
                result,
            } => self.on_intruder_default_pool_ready(project, operation_id, result, window, cx),
            Event::IntruderResourcePools {
                project,
                query_revision,
                result,
            } => {
                self.on_intruder_resource_pools(project, query_revision, result, cx);
            }
            Event::IntruderResourcePoolChanged {
                project,
                run_id,
                operation_id,
                result,
            } => {
                let changed = self.project == Some(project) && result.is_ok();
                self.on_intruder_resource_pool_changed(project, run_id, operation_id, result, cx);
                if changed {
                    self.settings_pool_usage_changed(cx);
                }
            }
            event @ (Event::IntruderPoolUsage { .. }
            | Event::IntruderResourcePoolsUpdated { .. }
            | Event::IntruderResourcePoolDeleted { .. }) => {
                self.on_settings_pool_event(event, window, cx)
            }
            Event::IntruderRenamed {
                project,
                run_id,
                operation_id,
                result,
            } => {
                self.on_intruder_renamed(project, run_id, operation_id, result, window, cx);
            }
            Event::IntruderBaselineRequested {
                project,
                run_id,
                operation_id,
                result,
            } => {
                self.on_intruder_baseline_requested(
                    project,
                    run_id,
                    operation_id,
                    result,
                    window,
                    cx,
                );
            }
            Event::IntruderDiscarded {
                project,
                run_id,
                operation_id,
                result,
            } => {
                let changed = self.project == Some(project) && result.is_ok();
                self.on_intruder_discarded(project, run_id, operation_id, result, window, cx);
                if changed {
                    self.settings_pool_usage_changed(cx);
                }
            }
            Event::IntruderRetentionChanged {
                project,
                run_id,
                operation_id,
                result,
            } => {
                self.on_intruder_retention_changed(
                    project,
                    run_id,
                    operation_id,
                    result,
                    window,
                    cx,
                );
            }
            event @ (Event::IntruderChanged { .. }
            | Event::IntruderResults { .. }
            | Event::IntruderResultView { .. }
            | Event::IntruderResultNotes { .. }
            | Event::IntruderResultHighlight { .. }
            | Event::IntruderRuns { .. }
            | Event::IntruderRunControl { .. }
            | Event::IntruderSpec { .. }) => {
                self.on_intruder_result_event(&event, window, cx);
            }
            Event::HistoryChanged { project } if self.project == Some(project) => {
                self.history_dirty = true;
                self.has_traffic = true;
                if self.http_filter.continuous_scroll {
                    self.http_filter.pending_auto_scroll = true;
                }
            }
            Event::HttpHistoryDeleted {
                project,
                operation_id,
                deleted_ids,
                result,
            } if self.pending_history_delete == Some((project, operation_id)) => {
                self.pending_history_delete = None;
                match result {
                    Ok(count) => self.http_history_deleted(&deleted_ids, count, window, cx),
                    Err(error) => self.fail(error, cx),
                }
            }
            Event::HistoryPage {
                project,
                query_revision,
                offset,
                total,
                rows,
            } if self.project == Some(project) && query_revision == self.query_revision => {
                for row in &rows {
                    if let Some(summary) = row.live() {
                        self.record_logger_flow(summary);
                    }
                }
                let pending = self.http_filter.pending_query();
                let applied = self.apply_http_history_page(offset, total, rows, None, window, cx);
                if let Some(query) = pending.filter(|query| {
                    query.project == project && query.query_revision == query_revision
                }) {
                    self.complete_http_filter_query(&query, applied, cx);
                }
            }
            Event::LoggerHistoryPage {
                project,
                query_revision,
                offset,
                result,
            } if self.project == Some(project)
                && self.extra_tools.logger_query_revision == query_revision =>
            {
                self.extra_tools.logger_query_cancel = None;
                match result {
                    Ok((total, mut rows)) => {
                        let page_len = rows.len();
                        rows.reverse();
                        if offset == 0 {
                            let plain_filter =
                                self.extra_tools.logger_query_filter.trim().to_lowercase();
                            for live in std::mem::take(&mut self.extra_tools.logger_entries) {
                                if let Some(existing) =
                                    rows.iter_mut().find(|row| row.id == live.id)
                                {
                                    if live.revision > existing.revision {
                                        *existing = live;
                                    }
                                } else if self.extra_tools.logger_source_filter.as_ref().is_none_or(
                                    |src| match (src, &live.source) {
                                        (
                                            mio_core::TrafficSource::Plugin(_),
                                            mio_core::TrafficSource::Plugin(_),
                                        ) => true,
                                        _ => &live.source == src,
                                    },
                                ) && (plain_filter.is_empty()
                                    || live.url.to_lowercase().contains(&plain_filter)
                                    || live.method.to_lowercase().contains(&plain_filter)
                                    || live
                                        .status
                                        .is_some_and(|code| code.to_string().contains(&plain_filter)))
                                {
                                    rows.push(live);
                                }
                            }
                        } else {
                            let mut merged = std::mem::take(&mut self.extra_tools.logger_entries);
                            for row in rows {
                                if let Some(existing) =
                                    merged.iter_mut().find(|item| item.id == row.id)
                                {
                                    if row.revision > existing.revision {
                                        *existing = row;
                                    }
                                } else {
                                    merged.push(row);
                                }
                            }
                            rows = merged;
                        }
                        rows.sort_by(|a, b| {
                            a.started_unix_ms
                                .cmp(&b.started_unix_ms)
                                .then_with(|| a.id.as_hyphenated().cmp(b.id.as_hyphenated()))
                        });
                        let max_entries = self.extra_tools.logger_max_entries.max(1);
                        if rows.len() > max_entries {
                            let excess = rows.len() - max_entries;
                            rows.drain(0..excess);
                        }
                        self.extra_tools.logger_entries = rows;
                        let target_limit = total.min(max_entries);
                        let next_offset = offset.saturating_add(page_len);
                        if page_len > 0 && next_offset < target_limit {
                            let next_limit = 200.min(target_limit - next_offset);
                            let cancel = self.shutdown.child_token();
                            if self.enqueue(
                                Command::QueryLoggerHistory {
                                    project,
                                    query_revision,
                                    offset: next_offset,
                                    limit: next_limit,
                                    filter: self.extra_tools.logger_query_filter.clone(),
                                    source: self.extra_tools.logger_source_filter.clone(),
                                    cancel: cancel.clone(),
                                },
                                cx,
                            ) {
                                self.extra_tools.logger_query_cancel = Some(cancel);
                            }
                        }
                        cx.notify();
                    }
                    Err(err) => {
                        if !err.contains("已取消") {
                            self.fail(err, cx);
                        }
                    }
                }
            }
            Event::HttpHistoryPage { query, result } if self.http_filter_query_matches(&query) => {
                if self
                    .http_filter
                    .history_shift
                    .as_ref()
                    .is_some_and(|shift| shift.query_revision == query.query_revision)
                {
                    if result
                        .as_ref()
                        .err()
                        .is_some_and(|err| Self::is_cancelled_http_query_error(err))
                    {
                        self.http_filter.history_shift = None;
                        if self.http_filter.pending.is_none() {
                            self.history_loading = false;
                            self.history.update(cx, |table, cx| {
                                table.delegate_mut().loading = false;
                                cx.notify();
                            });
                        }
                        let sort = self.http_filter.sort;
                        self.history.update(cx, |table, cx| {
                            table.delegate_mut().apply_sort(sort);
                            table.refresh(cx);
                        });
                        cx.notify();
                        return ControlFlow::Break(());
                    }
                    self.history_shift_page_received(&query, &result, window, cx);
                    let sort = self.http_filter.sort;
                    self.history.update(cx, |table, cx| {
                        table.delegate_mut().apply_sort(sort);
                        table.refresh(cx);
                    });
                    cx.notify();
                    return ControlFlow::Break(());
                }
                let applied = match result {
                    Ok(result) => {
                        let ids: HashSet<_> = query.selected_ids.iter().copied().collect();
                        if result.selected_matches.len() != ids.len()
                            || result
                                .selected_matches
                                .iter()
                                .any(|(id, _)| !ids.contains(id))
                            || result
                                .selected_ranks
                                .iter()
                                .any(|(_, rank)| *rank >= result.total)
                            || result.rows.len() > query.limit
                            || result.rows.len() > result.total.saturating_sub(query.offset)
                        {
                            Err("HTTP 历史回执数量或选择身份不匹配；原列表保持。".into())
                        } else {
                            self.live_http_annotations = result.live_annotations;
                            if !result.live_numbers.is_empty() || !result.row_metadata.is_empty() {
                                self.history.update(cx, |table, _| {
                                    let delegate = table.delegate_mut();
                                    for (flow_id, number) in &result.live_numbers {
                                        delegate.live_numbers.insert(*flow_id, *number);
                                    }
                                    for (entry_id, meta) in &result.row_metadata {
                                        delegate.row_metadata.insert(*entry_id, meta.clone());
                                    }
                                });
                            }
                            for row in &result.rows {
                                if let Some(summary) = row.live() {
                                    self.record_logger_flow(summary);
                                }
                            }
                            self.apply_http_history_page(
                                query.offset,
                                result.total,
                                result.rows,
                                Some((
                                    (&result.selected_matches, &result.selected_ranks),
                                    result.selected_rows,
                                )),
                                window,
                                cx,
                            )
                        }
                    }
                    Err(error) => Err(error),
                };
                self.complete_http_filter_query(&query, applied, cx);
                let sort = self.http_filter.sort;
                self.history.update(cx, |table, cx| {
                    table.delegate_mut().apply_sort(sort);
                    table.refresh(cx);
                });
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}
