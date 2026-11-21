use super::*;

impl Workbench {
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

    pub(in crate::workbench) fn refresh_target_site_map(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let search_term = match editing::input_snapshot(&self.target_pane.search, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if let Some(cancel) = self.target_pane.query_cancel.take() {
            cancel.cancel();
        }
        let query = SiteMapQuery {
            selected_node_key: self.target_pane.tree_state.selected_key.clone(),
            filter: SiteMapFilter {
                in_scope_only: self.target_pane.in_scope_only,
                search_term,
                status_classes: self.target_pane.status_classes,
                hide_not_found: self.target_pane.hide_not_found,
            },
            offset,
            limit: mio_core::MAX_SITE_MAP_PAGE_LIMIT,
        };
        self.target_pane.query_revision += 1;
        let query_revision = self.target_pane.query_revision;
        let cancel = self.shutdown.child_token();
        if self.enqueue(
            Command::QuerySiteMap {
                project,
                query_revision,
                query,
                cancel: cancel.clone(),
            },
            cx,
        ) {
            self.target_pane.query_cancel = Some(cancel);
            self.target_pane.items_offset = offset;
            self.target_pane.site_map_loading = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn load_more_target_site_map(
        &mut self,
        offset: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available()
            || self.transition.is_some()
            || self.target_pane.site_map_loading
            || offset == 0
            || offset != self.target_pane.items.len()
            || offset >= self.target_pane.total_items
        {
            return;
        }
        let search_state = self.target_pane.search.read(cx);
        if search_state.has_marked_text() {
            return;
        }
        let search_term = search_state.value().trim().to_owned();
        let query = SiteMapQuery {
            selected_node_key: self.target_pane.tree_state.selected_key.clone(),
            filter: SiteMapFilter {
                in_scope_only: self.target_pane.in_scope_only,
                search_term,
                status_classes: self.target_pane.status_classes,
                hide_not_found: self.target_pane.hide_not_found,
            },
            offset,
            limit: mio_core::MAX_SITE_MAP_PAGE_LIMIT,
        };
        self.target_pane.query_revision += 1;
        let query_revision = self.target_pane.query_revision;
        let cancel = self.shutdown.child_token();
        if self.enqueue(
            Command::QuerySiteMap {
                project,
                query_revision,
                query,
                cancel: cancel.clone(),
            },
            cx,
        ) {
            self.target_pane.query_cancel = Some(cancel);
            self.target_pane.site_map_loading = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_target_node(
        &mut self,
        key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.pending_branch_scan = None;
        self.target_pane.pending_branch_scan_prefix = None;
        if self.target_pane.tree_state.selected_key == key {
            self.refresh_target_site_map(0, window, cx);
            return;
        }
        self.target_pane.tree_state.selected_key = key;
        self.target_pane.selected_id = None;
        self.target_pane.selected_ids.clear();
        self.target_pane.selection_anchor = None;
        self.target_pane.selected_row = None;
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        self.refresh_target_site_map(0, window, cx);
    }

    pub(in crate::workbench) fn select_target_row(
        &mut self,
        row: &HistoryRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.selected_ids.clear();
        self.target_pane.selected_ids.insert(row.id());
        self.target_pane.selection_anchor = Some(row.id());
        self.select_target_row_detail(row, window, cx);
    }

    fn select_target_row_detail(
        &mut self,
        row: &HistoryRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.pending_branch_scan = None;
        self.target_pane.selected_id = Some(row.id());
        self.target_pane.selected_row = Some(row.clone());
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        self.load_selected_target_detail(true, window, cx);
    }

    fn clear_target_row_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.target_pane.pending_branch_scan = None;
        self.target_pane.selected_id = None;
        self.target_pane.selected_ids.clear();
        self.target_pane.selection_anchor = None;
        self.target_pane.selected_row = None;
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn handle_target_row_click(
        &mut self,
        row: &HistoryRow,
        visible_order: &[HistoryEntryId],
        shift: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clicked_id = row.id();
        if self.target_pane.selected_id.is_none() {
            self.target_pane.selected_ids.clear();
            self.target_pane.selection_anchor = None;
        }
        let valid_ids: HashSet<HistoryEntryId> =
            self.target_pane.items.iter().map(HistoryRow::id).collect();
        self.target_pane
            .selected_ids
            .retain(|id| valid_ids.contains(id));
        if self.target_pane.selected_ids.is_empty()
            && let Some(cur) = self
                .target_pane
                .selected_id
                .filter(|id| valid_ids.contains(id))
        {
            self.target_pane.selected_ids.insert(cur);
        }

        if shift {
            let anchor = self
                .target_pane
                .selection_anchor
                .filter(|id| valid_ids.contains(id))
                .or_else(|| {
                    self.target_pane
                        .selected_id
                        .filter(|id| valid_ids.contains(id))
                })
                .unwrap_or(clicked_id);
            if let (Some(s), Some(e)) = (
                visible_order.iter().position(|id| *id == anchor),
                visible_order.iter().position(|id| *id == clicked_id),
            ) {
                let (lo, hi) = (s.min(e), s.max(e));
                self.target_pane.selected_ids =
                    visible_order[lo..=hi].iter().copied().collect();
            } else {
                self.target_pane.selected_ids.insert(clicked_id);
            }
            if self.target_pane.selection_anchor.is_none() {
                self.target_pane.selection_anchor = Some(anchor);
            }
            self.select_target_row_detail(row, window, cx);
            return;
        }

        if toggle {
            if self.target_pane.selected_ids.contains(&clicked_id) {
                self.target_pane.selected_ids.remove(&clicked_id);
                if self.target_pane.selected_ids.is_empty() {
                    self.clear_target_row_selection(window, cx);
                } else if self.target_pane.selected_id == Some(clicked_id) {
                    let next_row = visible_order
                        .iter()
                        .copied()
                        .find(|id| self.target_pane.selected_ids.contains(id))
                        .or_else(|| self.target_pane.selected_ids.iter().copied().next())
                        .and_then(|next_id| {
                            self.target_pane
                                .items
                                .iter()
                                .find(|r| r.id() == next_id)
                                .cloned()
                        });
                    if let Some(next_row) = next_row {
                        self.select_target_row_detail(&next_row, window, cx);
                    } else {
                        cx.notify();
                    }
                } else {
                    cx.notify();
                }
            } else {
                self.target_pane.selected_ids.insert(clicked_id);
                self.target_pane.selection_anchor = Some(clicked_id);
                self.select_target_row_detail(row, window, cx);
            }
            return;
        }

        self.select_target_row(row, window, cx);
    }

    pub(in crate::workbench) fn handle_target_row_right_down(
        &mut self,
        row: &HistoryRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clicked_id = row.id();
        if self.target_pane.selected_id.is_none() {
            self.target_pane.selected_ids.clear();
            self.target_pane.selection_anchor = None;
        }
        let in_multi = self.target_pane.selected_ids.contains(&clicked_id)
            && self.target_pane.selected_ids.len() > 1;
        if in_multi {
            cx.notify();
            return;
        }
        if self.target_pane.selected_id != Some(clicked_id) {
            self.select_target_row(row, window, cx);
        } else {
            self.target_pane.selected_ids.clear();
            self.target_pane.selected_ids.insert(clicked_id);
            self.target_pane.selection_anchor = Some(clicked_id);
        }
    }

    pub(in crate::workbench) fn selected_target_rows_in_order(&self) -> Vec<HistoryRow> {
        if self.target_pane.selected_ids.len() > 1 {
            let ordered: Vec<HistoryRow> = self
                .target_pane
                .items
                .iter()
                .filter(|r| self.target_pane.selected_ids.contains(&r.id()))
                .cloned()
                .collect();
            if !ordered.is_empty() {
                return ordered;
            }
        }
        self.target_pane.selected_row.iter().cloned().collect()
    }

    pub(in crate::workbench) fn selected_target_response_bytes(&self, cx: &App) -> Option<Vec<u8>> {
        if !self.target_pane.selected_response_bytes.is_empty() {
            return Some(self.target_pane.selected_response_bytes.clone());
        }
        let raw = self.target_pane.response.raw.read(cx).value();
        (!raw.is_empty()).then(|| raw.as_bytes().to_vec())
    }

    pub(in crate::workbench) fn load_selected_target_detail(
        &mut self,
        force: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        let Some(row) = self.target_pane.selected_row.clone() else {
            return;
        };
        if !force && !self.target_pane.request.raw.read(cx).value().is_empty() {
            return;
        }
        if let Some(&(entry_id, entry_revision)) = self.target_pane.explicit_entries.get(&row.id()) {
            self.selection_revision += 1;
            let selection_revision = self.selection_revision;
            let cancel = self.shutdown.child_token();
            if self.enqueue(Command::ReadSiteMapEntry { project, selection_revision, entry_id, revision: entry_revision, cancel }, cx) {
                self.target_pane.pending_reads.insert(selection_revision, TargetRead::SiteMap { row_id: row.id(), entry_id, revision: entry_revision });
            }
            cx.notify();
            return;
        }
        match row.id() {
            HistoryEntryId::Live(flow_id) => {
                self.selection_revision += 1;
                let revision = self.selection_revision;
                if self.enqueue(
                    Command::ReadFlow {
                        project,
                        selection_revision: revision,
                        flow_id,
                    },
                    cx,
                ) {
                    self.target_pane
                        .pending_reads
                        .insert(revision, TargetRead::Live(flow_id));
                }
            }
            HistoryEntryId::External { source_id, item_id } => {
                let HistoryRow::ExtProxy(archive) = &row else {
                    return;
                };
                let Some(summary) = archive.edited_request.as_ref().or(archive.request.as_ref())
                else {
                    self.target_pane.site_map_error = Some("所选归档条目没有可读取的请求。".into());
                    return;
                };
                self.selection_revision += 1;
                let revision = self.selection_revision;
                if self.enqueue(
                    Command::ReadExtMessage {
                        project,
                        selection_revision: revision,
                        message_id: summary.id,
                        offset: 0,
                        limit: summary.raw_bytes.min(64 * 1024) as usize,
                    },
                    cx,
                ) {
                    self.target_pane.pending_reads.insert(
                        revision,
                        TargetRead::ExtRequest {
                            id: HistoryEntryId::External { source_id, item_id },
                            message_id: summary.id,
                        },
                    );
                }
                if let Some(response) = archive
                    .edited_response
                    .as_ref()
                    .or(archive.response.as_ref())
                {
                    self.selection_revision += 1;
                    let revision = self.selection_revision;
                    if self.enqueue(
                        Command::ReadExtMessage {
                            project,
                            selection_revision: revision,
                            message_id: response.id,
                            offset: 0,
                            limit: response.raw_bytes.min(64 * 1024) as usize,
                        },
                        cx,
                    ) {
                        self.target_pane.pending_reads.insert(
                            revision,
                            TargetRead::ExtResponse {
                                id: HistoryEntryId::External { source_id, item_id },
                                message_id: response.id,
                            },
                        );
                    }
                }
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn target_transfer_entry(
        &self,
        row: &HistoryRow,
    ) -> Option<HistoryTransferEntry> {
        if let Some(&(entry_id, revision)) = self.target_pane.explicit_entries.get(&row.id()) {
            return Some(HistoryTransferEntry::SiteMap { row_id: row.id(), entry_id, revision });
        }
        match row {
            HistoryRow::Live(summary) if summary.revision > 0 => Some(HistoryTransferEntry::Live {
                flow_id: summary.id,
                revision: summary.revision,
                original: false,
            }),
            HistoryRow::ExtProxy(archive) => {
                let message = archive
                    .edited_request
                    .as_ref()
                    .or(archive.request.as_ref())?;
                Some(HistoryTransferEntry::External {
                    source_id: archive.item.source_id,
                    item_id: archive.item.id,
                    message_id: message.id,
                })
            }
            _ => None,
        }
    }

    pub(in crate::workbench) fn copy_target_url(&mut self, cx: &mut Context<Self>) {
        let rows = self.selected_target_rows_in_order();
        if rows.is_empty() {
            return;
        }
        let multi = rows.len() > 1;
        let mut urls = Vec::with_capacity(rows.len());
        for row in &rows {
            let url = match row {
                HistoryRow::Live(summary) => summary.url.clone(),
                HistoryRow::ExtProxy(archive) => {
                    let request = archive.request.as_ref().or(archive.edited_request.as_ref());
                    let Some(request) = request else {
                        if !multi {
                            self.fail("归档条目缺少可复制的请求 URL。", cx);
                            return;
                        }
                        continue;
                    };
                    let target = request.request_target.clone().unwrap_or_default();
                    if target.starts_with('/') {
                        format!("{}{target}", archive.origin.clone().unwrap_or_default())
                    } else {
                        target
                    }
                }
            };
            if !url.is_empty() {
                urls.push(url);
            }
        }
        if urls.is_empty() {
            self.fail("没有可复制的 URL。", cx);
            return;
        }
        let count = urls.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(urls.join("\n")));
        self.error = None;
        self.status = if count > 1 {
            format!("已复制 {count} 条 URL 到剪贴板。")
        } else {
            "已复制 URL 到剪贴板。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn add_advanced_scope_rule(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let mut values: [String; 3] = std::array::from_fn(|_| String::new());
        let inputs = if include {
            &self.target_pane.advanced_scope_inputs.include
        } else {
            &self.target_pane.advanced_scope_inputs.exclude
        };
        for (index, input) in inputs.iter().enumerate() {
            match editing::input_snapshot(input, window, cx) {
                Ok(value) => values[index] = value.trim().to_owned(),
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            }
        }
        let rule = AdvancedScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            protocol: self.target_pane.scope_protocol,
            host: values[0].clone(),
            port: values[1].clone(),
            file: values[2].clone(),
        };
        let mut next = self.workspace.scope.clone();
        let previous = self.workspace.scope.clone();
        let side = usize::from(!include);
        let editing_id = self.target_pane.editing_advanced_scope[side];
        let rules = if include {
            &mut next.advanced_include
        } else {
            &mut next.advanced_exclude
        };
        let new_id = rule.id;
        let updated = if let Some(id) = editing_id
            && let Some(existing) = rules.iter_mut().find(|item| item.id == id)
        {
            existing.protocol = rule.protocol;
            existing.host = rule.host;
            existing.port = rule.port;
            existing.file = rule.file;
            true
        } else {
            rules.push(rule);
            false
        };
        if let Err(error) = next.validate() {
            self.fail(error.to_string(), cx);
            return;
        }
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(format!("未添加高级 Scope 规则：{error}"), cx);
            return;
        }
        self.target_pane.editing_advanced_scope[side] = None;
        self.target_pane.scope_editor_open[side] = false;
        self.target_pane.selected_advanced_scope[side] = editing_id.or(Some(new_id));
        self.save_workspace(cx);
        let inputs = if include {
            &self.target_pane.advanced_scope_inputs.include
        } else {
            &self.target_pane.advanced_scope_inputs.exclude
        };
        for input in inputs {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.status = if updated {
            "已更新高级目标范围规则。".into()
        } else if include {
            "已添加高级目标范围规则。".into()
        } else {
            "已添加高级目标范围排除规则。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn remove_advanced_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let side = usize::from(!include);
        if self.target_pane.editing_advanced_scope[side] == Some(id) {
            self.target_pane.editing_advanced_scope[side] = None;
        }
        if self.target_pane.selected_advanced_scope[side] == Some(id) {
            self.target_pane.selected_advanced_scope[side] = None;
        }
        if include {
            self.workspace
                .scope
                .advanced_include
                .retain(|rule| rule.id != id);
        } else {
            self.workspace
                .scope
                .advanced_exclude
                .retain(|rule| rule.id != id);
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_advanced_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let rules = if include {
            &mut self.workspace.scope.advanced_include
        } else {
            &mut self.workspace.scope.advanced_exclude
        };
        if let Some(rule) = rules.iter_mut().find(|rule| rule.id == id) {
            rule.enabled = !rule.enabled;
            self.save_workspace(cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn ensure_default_issue_selected(&mut self) {
        if self.target_pane.issue_selected.is_none() {
            let (col, asc) = self.target_pane.issue_sort;
            let matches = sort_issue_definitions(
                filter_issue_definitions(
                    &self.target_pane.issue_search_term,
                    self.target_pane.issue_severity,
                ),
                col,
                asc,
            );
            self.target_pane.issue_selected = matches.first().copied();
        }
    }

    pub(in crate::workbench) fn toggle_issue_sort(
        &mut self,
        column: IssueSortColumn,
        cx: &mut Context<Self>,
    ) {
        let (cur_col, cur_asc) = self.target_pane.issue_sort;
        self.target_pane.issue_sort = if cur_col == column {
            (column, !cur_asc)
        } else {
            (column, true)
        };
        cx.notify();
    }

    pub(in crate::workbench) fn apply_issue_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = match editing::input_snapshot(&self.target_pane.issue_search, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.target_pane.issue_search_term = query;
        let (col, asc) = self.target_pane.issue_sort;
        let matches = sort_issue_definitions(
            filter_issue_definitions(
                &self.target_pane.issue_search_term,
                self.target_pane.issue_severity,
            ),
            col,
            asc,
        );
        self.target_pane.issue_selected =
            matches.first().copied().or(self.target_pane.issue_selected);
        cx.notify();
    }

    pub(in crate::workbench) fn set_issue_severity(
        &mut self,
        severity: Option<MioIssueSeverity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.issue_severity = severity;
        self.apply_issue_search(window, cx);
    }

    pub(in crate::workbench) fn render_target(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self.target_pane.page;
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_target_tabs(window, cx))
            .child(div().flex_1().min_h_0().child(match page {
                TargetPage::SiteMap => self.render_site_map(window, cx),
                TargetPage::Scope => self.render_scope_page(window, cx),
                TargetPage::Issues => self.render_issue_page(window, cx),
            }))
            .into_any_element()
    }

    pub(in crate::workbench) fn toggle_target_advanced_mode(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        self.workspace.scope.advanced_mode = !self.workspace.scope.advanced_mode;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope.advanced_mode = !self.workspace.scope.advanced_mode;
            self.fail(error.to_string(), cx);
            return;
        }
        self.target_pane.advanced_mode_input = self.workspace.scope.advanced_mode;
        self.target_pane.scope_editor_open = [false, false];
        self.save_workspace(cx);
        cx.notify();
    }
}
