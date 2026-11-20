use super::*;

impl Workbench {
    pub(in crate::workbench) fn apply_history_selection(
        &mut self,
        selected: HashSet<HistoryEntryId>,
        primary: Option<HistoryEntryId>,
        range: (Option<HistoryEntryId>, Option<HistoryEntryId>),
        context: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if selected.len() > MAX_SELECTED_HISTORY {
            self.restore_history_primary(context, cx);
            self.fail("HTTP 历史最多同时选择 500 条；原选择保持。", cx);
            return;
        }
        let delegate = self.history.read(cx).delegate();
        let selection_changed = selected != delegate.selected_ids;
        if selected.iter().any(|id| delegate.row_by_id(*id).is_none()) {
            self.restore_history_primary(context, cx);
            self.fail("所选 HTTP 历史摘要已不可用；原选择保持。", cx);
            return;
        }
        if self.selected_history != primary && !self.leave_http_notes(window, cx) {
            self.restore_history_primary(context, cx);
            return;
        }
        if let Some(primary) = primary {
            self.select_history_entry(primary, window, cx);
            if self.selected_history != Some(primary) {
                self.restore_history_primary(context, cx);
                return;
            }
        } else {
            self.selected_flow = None;
            self.selected_history = None;
            self.clear_history_detail(window, cx);
            self.reset_imported_history(window, cx);
        }
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.selected_rows.retain(|id, _| selected.contains(id));
            delegate
                .selected_ranks
                .retain(|id, _| selected.contains(id));
            for (index, row) in delegate.rows.iter().enumerate() {
                if selected.contains(&row.id()) {
                    delegate.selected_rows.insert(row.id(), row.clone());
                    delegate
                        .selected_ranks
                        .insert(row.id(), delegate.offset.saturating_add(index));
                }
            }
            delegate.selected_order.retain(|id| selected.contains(id));
            for row in &delegate.rows {
                let id = row.id();
                if selected.contains(&id) && !delegate.selected_order.contains(&id) {
                    delegate.selected_order.push(id);
                }
            }
            delegate.selected_order.sort_by_key(|id| {
                delegate
                    .selected_ranks
                    .get(id)
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            delegate.selected_ids = selected;
            delegate.selection_anchor = range.0;
            delegate.selection_lead = range.1;
            table.refresh(cx);
        });
        self.restore_history_primary(context, cx);
        if selection_changed {
            self.refresh_pending_http_selection(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn apply_history_range_selection(
        &mut self,
        mut collected: Vec<(usize, HistoryRow)>,
        anchor: HistoryEntryId,
        lead: HistoryEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if collected.is_empty() || collected.len() > MAX_SELECTED_HISTORY {
            self.restore_history_primary(false, cx);
            self.fail("HTTP 历史最多同时选择 500 条；原选择保持。", cx);
            return;
        }
        let mut selected = HashSet::with_capacity(collected.len());
        let mut ranks_seen = HashSet::with_capacity(collected.len());
        if collected
            .iter()
            .any(|(rank, row)| !selected.insert(row.id()) || !ranks_seen.insert(*rank))
            || !selected.contains(&anchor)
            || !selected.contains(&lead)
        {
            self.restore_history_primary(false, cx);
            self.fail("HTTP 历史跨页范围选择结果不一致；原选择保持。", cx);
            return;
        }
        collected.sort_by_key(|(rank, _)| *rank);
        let mut fresh_rows: Vec<_> = collected.iter().map(|(_, row)| row.clone()).collect();
        self.refresh_http_notes_rows(&mut fresh_rows, cx);
        self.refresh_http_highlight_rows(&mut fresh_rows, cx);
        let selection_changed = selected != self.history.read(cx).delegate().selected_ids;
        if self.selected_history != Some(lead) && !self.leave_http_notes(window, cx) {
            self.restore_history_primary(false, cx);
            return;
        }
        let lead_row = collected
            .iter()
            .zip(&fresh_rows)
            .find_map(|((_, _), row)| (row.id() == lead).then(|| row.clone()));
        if let Some(HistoryRow::ExtProxy(_)) = &lead_row
            && self.history.read(cx).delegate().row_by_id(lead).is_none()
            && let Some(row) = lead_row
        {
            self.history.update(cx, |table, _| {
                table.delegate_mut().selected_rows.insert(lead, row);
            });
        }
        self.select_history_entry(lead, window, cx);
        if self.selected_history != Some(lead) {
            self.restore_history_primary(false, cx);
            return;
        }
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let mut selected_rows = HashMap::with_capacity(collected.len());
            let mut selected_ranks = HashMap::with_capacity(collected.len());
            let mut selected_order = Vec::with_capacity(collected.len());
            for ((rank, _), row) in collected.into_iter().zip(fresh_rows) {
                let id = row.id();
                let chosen = match (delegate.row_by_id(id), &row) {
                    (Some(HistoryRow::Live(cached)), HistoryRow::Live(incoming))
                        if cached.revision > incoming.revision =>
                    {
                        HistoryRow::Live(cached.clone())
                    }
                    _ => row,
                };
                selected_rows.insert(id, chosen);
                selected_ranks.insert(id, rank);
                selected_order.push(id);
            }
            delegate.selected_rows = selected_rows;
            delegate.selected_ranks = selected_ranks;
            delegate.selected_order = selected_order;
            delegate.selected_ids = selected;
            delegate.selection_anchor = Some(anchor);
            delegate.selection_lead = Some(lead);
            table.refresh(cx);
        });
        self.restore_history_primary(false, cx);
        if selection_changed {
            self.refresh_pending_http_selection(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn retain_history_query_selection(
        &mut self,
        rows: &[HistoryRow],
        selected_rows: &[HistoryRow],
        selection: HistoryMatches<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (matches, ranks) = selection;
        let mut observed = HashSet::new();
        if matches.len() > MAX_SELECTED_HISTORY
            || matches.iter().any(|(id, _)| !observed.insert(*id))
        {
            return Err("HTTP 历史选择回执重复或超过上限；原列表保持。".into());
        }
        let mut page_ids = HashSet::new();
        if rows.iter().any(|row| !page_ids.insert(row.id()))
            || matches
                .iter()
                .any(|(id, matches)| !matches && page_ids.contains(id))
        {
            return Err("HTTP 历史选择回执与分页内容矛盾；原列表保持。".into());
        }
        let removed: HashSet<_> = matches
            .iter()
            .filter(|(_, keep)| !keep)
            .map(|(id, _)| *id)
            .collect();
        let matching: HashSet<_> = matches
            .iter()
            .filter(|(_, keep)| *keep)
            .map(|(id, _)| *id)
            .collect();
        let expected_offpage: HashSet<_> = matching.difference(&page_ids).copied().collect();
        let mut received_offpage = HashSet::new();
        if selected_rows.len() != expected_offpage.len()
            || selected_rows.iter().any(|row| {
                !expected_offpage.contains(&row.id()) || !received_offpage.insert(row.id())
            })
        {
            return Err("HTTP 页外选择摘要缺失、重复或身份不匹配；原列表保持。".into());
        }
        let mut ranked = HashSet::new();
        let mut positions = HashSet::new();
        if ranks.len() != matching.len()
            || ranks.iter().any(|(id, rank)| {
                !matching.contains(id) || !ranked.insert(*id) || !positions.insert(*rank)
            })
        {
            return Err("HTTP 历史选择位置回执不完整或重复；原列表保持。".into());
        }
        let delegate = self.history.read(cx).delegate();
        let selected: HashSet<_> = delegate
            .selected_ids
            .difference(&removed)
            .copied()
            .collect();
        if selected.len() > MAX_SELECTED_HISTORY
            || selected.iter().any(|id| {
                delegate.row_by_id(*id).is_none()
                    && !rows.iter().chain(selected_rows).any(|row| row.id() == *id)
            })
        {
            return Err("HTTP 历史选择摘要不完整或超过上限；原列表保持。".into());
        }
        let clear_primary = self
            .selected_history
            .is_some_and(|id| removed.contains(&id));
        if clear_primary && !self.leave_http_notes(window, cx) {
            return Err("当前备注仍有未完成输入；原列表和选择保持。".into());
        }
        if clear_primary {
            self.selected_history = None;
            self.selected_flow = None;
        }
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.selected_rows.retain(|id, _| selected.contains(id));
            delegate
                .selected_ranks
                .retain(|id, _| selected.contains(id));
            for (id, rank) in ranks {
                if selected.contains(id) {
                    delegate.selected_ranks.insert(*id, *rank);
                }
            }
            for row in delegate.rows.iter().chain(selected_rows).chain(rows) {
                if selected.contains(&row.id()) {
                    if let (Some(HistoryRow::Live(cached)), HistoryRow::Live(incoming)) =
                        (delegate.selected_rows.get(&row.id()), row)
                        && cached.revision > incoming.revision
                    {
                        continue;
                    } else {
                        delegate.selected_rows.insert(row.id(), row.clone());
                    }
                }
            }
            let mut ordered = HashSet::new();
            delegate.selected_order = matches
                .iter()
                .filter(|(_, keep)| *keep)
                .map(|(id, _)| *id)
                .chain(delegate.selected_order.iter().copied())
                .chain(rows.iter().map(HistoryRow::id))
                .filter(|id| selected.contains(id) && ordered.insert(*id))
                .collect();
            delegate.selected_order.sort_by_key(|id| {
                delegate
                    .selected_ranks
                    .get(id)
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            delegate.selected_ids = selected;
            if delegate
                .selection_anchor
                .is_some_and(|id| removed.contains(&id))
            {
                delegate.selection_anchor = None;
            }
            if delegate
                .selection_lead
                .is_some_and(|id| removed.contains(&id))
            {
                delegate.selection_lead = None;
            }
            table.refresh(cx);
        });
        Ok(())
    }

    pub(in crate::workbench) fn restore_history_primary(
        &mut self,
        context: bool,
        cx: &mut Context<Self>,
    ) {
        self.history.update(cx, |table, cx| {
            let row = self
                .selected_history
                .and_then(|id| table.delegate().rows.iter().position(|row| row.id() == id));
            if table.selected_row() != row {
                if let Some(row) = row {
                    if context {
                        table.set_selected_row_preserving_context(row, cx);
                    } else {
                        table.set_selected_row(row, cx);
                    }
                } else {
                    table.clear_selection(cx);
                }
            }
        });
    }

    pub(in crate::workbench) fn history_selection_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.history.focus_handle(cx).is_focused(window) {
            return;
        }
        let key = &event.keystroke;
        let delegate = self.history.read(cx).delegate();
        if delegate.rows.is_empty() {
            return;
        }
        if key.key == "a" && key.modifiers.secondary(cx) && !key.modifiers.shift {
            let selected = delegate.rows.iter().map(HistoryRow::id).collect();
            let primary = self
                .selected_history
                .filter(|id| delegate.rows.iter().any(|row| row.id() == *id))
                .or_else(|| delegate.rows.first().map(HistoryRow::id));
            let anchor = delegate.rows.first().map(HistoryRow::id);
            let lead = delegate.rows.last().map(HistoryRow::id);
            self.apply_history_selection(selected, primary, (anchor, lead), false, window, cx);
        } else if key.modifiers.shift && matches!(key.key.as_str(), "up" | "down" | "home" | "end")
        {
            let current = delegate
                .selection_lead
                .or(self.selected_history)
                .and_then(|id| delegate.rows.iter().position(|row| row.id() == id))
                .unwrap_or(0);
            let next = match key.key.as_str() {
                "up" => current.saturating_sub(1),
                "down" => (current + 1).min(delegate.rows.len() - 1),
                "home" => 0,
                _ => delegate.rows.len() - 1,
            };
            let id = delegate.rows[next].id();
            self.history_pointer_selection(id, MouseButton::Left, key.modifiers, window, cx);
        } else {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
    }
}
