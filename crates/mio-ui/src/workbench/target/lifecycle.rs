use super::*;

impl Workbench {
    pub(in crate::workbench) fn active_target_message_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.target_pane.response.has_focus(window, cx) {
            if let Err(error) = self.target_pane.response.focus_search(window, cx) {
                self.fail(error, cx);
            }
        } else if let Err(error) = self.target_pane.request.focus_search(window, cx) {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn reset_target_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query_cancel = self.target_pane.query_cancel.take();
        if let Some(cancel) = query_cancel {
            cancel.cancel();
        }
        self.target_pane.pending_reads.clear();
        for (_, (_, cancel)) in self.target_pane.pending_adds.drain() { cancel.cancel(); }
        self.target_pane.explicit_entries.clear();
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.page = TargetPage::SiteMap;
        self.target_pane.site_map_subview = SiteMapSubView::Contents;
        self.target_pane.item_sort = None;
        self.target_pane.tree_state = TargetTreeState::default();
        self.target_pane.items.clear();
        *self.target_pane.items_cache.borrow_mut() = None;
        self.target_pane.items_offset = 0;
        self.target_pane.total_items = 0;
        self.target_pane.node_count = 0;
        self.target_pane.truncated_nodes = false;
        self.target_pane.site_map_loading = false;
        self.target_pane.site_map_error = None;
        self.target_pane.in_scope_only = false;
        self.target_pane.hide_not_found = true;
        self.target_pane.hide_css_image_binary = true;
        self.target_pane.hide_empty_folders = true;
        self.target_pane.status_classes = [true, true, false, true];
        self.target_pane.site_map_inspector_initialized = false;
        self.target_pane.search.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.target_pane.query_revision += 1;
        self.target_pane.selected_id = None;
        self.target_pane.selected_ids.clear();
        self.target_pane.selection_anchor = None;
        self.target_pane.selected_row = None;
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_site_map_finding_id = None;
        self.target_pane.hidden_item_ids.clear();
        self.target_pane.finding_originals.clear();
        self.target_pane.verified_findings.clear();
        self.target_pane.engagement_report = None;
        self.target_pane.compare_modal = None;
        self.target_pane.detached_site_map_window = None;
        self.target_pane.pending_branch_scan = None;
        self.target_pane.pending_branch_scan_prefix = None;
        self.target_pane.pending_branch_action = BranchItemAction::ActiveScan;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        self.target_pane.scope_protocol = ScopeProtocol::Any;
        self.target_pane.simple_scope_include_subdomains = [false, false];
        self.target_pane.editing_simple_scope = [None, None];
        self.target_pane.editing_advanced_scope = [None, None];
        self.target_pane.selected_simple_scope = [None, None];
        self.target_pane.selected_advanced_scope = [None, None];
        self.target_pane.scope_editor_open = [false, false];
        self.target_pane.settings_editing_scope = [None, None];
        for input in self
            .target_pane
            .advanced_scope_inputs
            .include
            .iter()
            .chain(&self.target_pane.advanced_scope_inputs.exclude)
        {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.target_pane.issue_search.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.target_pane.issue_search_term.clear();
        self.target_pane.issue_severity = None;
        self.target_pane.issue_selected = default_first_issue_definition();
        self.target_pane.issue_sort = (IssueSortColumn::TypeIndex, true);
        self.target_pane.issue_filter_open = false;
        self.target_pane.advanced_mode_input = self.workspace.scope.advanced_mode;
        self.target_pane.include_protocol = ScopeProtocol::Any;
        self.target_pane.exclude_protocol = ScopeProtocol::Any;
        for input in self.target_pane.simple_inputs().into_iter() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.target_view.update(cx, |view, cx| view.reset(cx));
        if self.tool == Tool::Target {
            self.refresh_target_site_map(0, window, cx);
        }
    }

    pub(in crate::workbench) fn target_backend_disconnected(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.target_pane.query_cancel.take() {
            cancel.cancel();
        }
        self.target_pane.pending_reads.clear();
        for (_, (_, cancel)) in self.target_pane.pending_adds.drain() { cancel.cancel(); }
        self.target_pane.pending_branch_scan = None;
        self.target_pane.pending_branch_scan_prefix = None;
        self.target_pane.site_map_loading = false;
        self.target_view.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub(in crate::workbench) fn on_site_map_page(
        &mut self,
        project: ProjectKey,
        query_revision: u64,
        query: SiteMapQuery,
        result: Result<SiteMapSnapshot, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project) || self.target_pane.query_revision != query_revision {
            return;
        }
        self.target_pane.site_map_loading = false;
        self.target_pane.in_scope_only = query.filter.in_scope_only;
        self.target_pane.hide_not_found = query.filter.hide_not_found;
        self.target_pane.status_classes = query.filter.status_classes;
        match result {
            Ok(snapshot) => {
                let previous_explicit = self.target_pane.selected_id.and_then(|id| self.target_pane.explicit_entries.get(&id).copied());
                let appending = snapshot.offset > 0 && snapshot.offset == self.target_pane.items.len();
                if appending {
                    for (id, entry_id, revision) in snapshot.explicit_entries {
                        self.target_pane.explicit_entries.insert(id, (entry_id, revision));
                    }
                } else {
                    self.target_pane.explicit_entries = snapshot
                        .explicit_entries
                        .into_iter()
                        .map(|(id, entry_id, revision)| (id, (entry_id, revision)))
                        .collect();
                }
                self.target_pane.tree_state.nodes = snapshot.nodes.clone();
                if let Some(selected) = self.target_pane.tree_state.selected_key.as_ref() {
                    for node in &self.target_pane.tree_state.nodes {
                        if url_matches_node_prefix(selected, &node.key) {
                            self.target_pane.tree_state.expanded.insert(node.key.clone());
                        }
                    }
                }
                self.target_pane.node_count = snapshot.nodes.len();
                self.target_pane.truncated_nodes = snapshot.truncated_nodes;
                self.target_pane.total_items = snapshot.total_items;
                if appending {
                    let mut existing_ids: HashSet<HistoryEntryId> =
                        self.target_pane.items.iter().map(HistoryRow::id).collect();
                    for row in snapshot.items {
                        if !self.target_pane.hidden_item_ids.contains(&row.id())
                            && existing_ids.insert(row.id())
                        {
                            self.target_pane.items.push(row);
                        }
                    }
                    self.target_pane.items_offset = 0;
                } else {
                    self.target_pane.items_offset = snapshot.offset;
                    self.target_pane.items = snapshot
                        .items
                        .into_iter()
                        .filter(|row| !self.target_pane.hidden_item_ids.contains(&row.id()))
                        .collect();
                }
                let selected = self
                    .target_pane
                    .items
                    .iter()
                    .find(|row| Some(row.id()) == self.target_pane.selected_id)
                    .cloned();
                if let Some(row) = selected {
                    self.target_pane.selected_row = Some(row);
                    if self.target_pane.selected_id.and_then(|id| self.target_pane.explicit_entries.get(&id).copied()) != previous_explicit {
                        self.target_pane.selected_explicit_detail = None;
                        self.target_pane.request.load(&[], window, cx);
                        self.target_pane.response.load(&[], window, cx);
                    }
                } else if !appending {
                    self.target_pane.selected_id = None;
                    self.target_pane.selected_row = None;
                    self.target_pane.selected_response_bytes.clear();
                    self.target_pane.selected_live_record = None;
                    self.target_pane.selected_explicit_detail = None;
                    self.target_pane.request.load(&[], window, cx);
                    self.target_pane.response.load(&[], window, cx);
                }
                self.target_pane.site_map_error = None;
                if self.target_pane.selected_id.is_none() && !self.target_pane.items.is_empty() {
                    let first_visible = {
                        let row_metadata = &self.history.read(cx).delegate().row_metadata;
                        let sorted_indices = sort_site_map_indices_with_metadata(
                            &self.target_pane.items,
                            None,
                            None,
                            Some(row_metadata),
                            &self.target_pane.tree_state.hidden_prefixes,
                            self.target_pane.item_sort,
                        );
                        sorted_indices
                            .iter()
                            .filter_map(|&idx| self.target_pane.items.get(idx))
                            .find(|row| {
                                site_map_row_passes_ui_filters(
                                    row,
                                    None,
                                    row_metadata.get(&row.id()),
                                    self.target_pane.hide_not_found,
                                    self.target_pane.status_classes,
                                    self.target_pane.hide_css_image_binary,
                                )
                            })
                            .cloned()
                    };
                    if let Some(row) = first_visible {
                        self.select_target_row(&row, window, cx);
                    }
                } else {
                    self.load_selected_target_detail(false, window, cx);
                }
                if !self.target_pane.site_map_inspector_initialized
                    && self.target_pane.selected_row.is_some()
                {
                    self.target_pane.site_map_inspector_initialized = true;
                    if !self.inspector.open
                        && !self.http_notes.open
                        && self.inspector.dismissed_selection.is_none()
                    {
                        self.open_inspector(window, cx);
                    }
                }
                if let Some(prefix) = self.target_pane.pending_branch_scan_prefix.take() {
                    let action = self.target_pane.pending_branch_action;
                    self.finish_branch_action_from_items(&prefix, action, window, cx);
                }
            }
            Err(error) => {
                self.target_pane.pending_branch_scan_prefix = None;
                self.target_pane.site_map_error = Some(error);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_target_flow_detail(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        record: &FlowRecord,
        original: &OriginalMessages,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(project)
            || self.target_pane.pending_reads.get(&selection_revision)
                != Some(&TargetRead::Live(record.summary.id))
        {
            return false;
        }
        self.target_pane.pending_reads.remove(&selection_revision);
        if self.target_pane.selected_id != Some(HistoryEntryId::Live(record.summary.id)) { return true; }
        self.target_pane.selected_id = Some(HistoryEntryId::Live(record.summary.id));
        self.target_pane.selected_row = Some(HistoryRow::Live(record.summary.clone()));
        self.target_pane.selected_live_record = Some(Box::new(record.clone()));
        self.target_pane
            .request
            .load(&request_to_raw(&record.request), window, cx);
        let raw_response = record
            .response
            .as_ref()
            .map(response_to_raw)
            .unwrap_or_default();
        self.target_pane.selected_response_bytes = raw_response.clone();
        self.target_pane.response.load(&raw_response, window, cx);
        if self.inspector.open {
            self.sync_inspector_selection(window, cx);
        }
        if self.target_pane.pending_branch_scan == Some(HistoryEntryId::Live(record.summary.id)) {
            self.target_pane.pending_branch_scan = None;
            let draft = Draft {
                revision: 1,
                target: record.request.url.clone(),
                raw: request_to_raw(&record.request),
                version: record.request.version,
            };
            let action = self.target_pane.pending_branch_action;
            self.execute_branch_action_with_draft(draft, action, window, cx);
        }
        let _ = original;
        cx.notify();
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn on_target_ext_response(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        reference: &ExtResponseRef,
        offset: u64,
        result: Result<mio_core::ExtMessageFragment, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.on_target_ext_message(
            project,
            selection_revision,
            reference.message_id,
            offset,
            result,
            window,
            cx,
        )
    }
}
