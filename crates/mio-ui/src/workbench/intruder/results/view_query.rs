use mio_core::{
    IntruderResultFilter, IntruderResultSort, IntruderResultViewPage, IntruderResultViewQuery,
};

use super::*;

impl Workbench {
    pub(in crate::workbench::intruder) fn refresh_intruder_grep_summaries(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let project = self.project;
        for (&view_id, view) in &mut self.intruder.results.views {
            view.grep_task = None;
            view.grep_operation = None;
            view.detail_grep_summary = None;
            let Some((raw, config)) = view.detail_response_raw.as_ref().zip(
                view.summary
                    .as_ref()
                    .and_then(|run| self.workspace.intruder.grep_configs.get(&run.tab_id))
                    .filter(|config| !config.is_empty()),
            ) else {
                continue;
            };
            let raw = raw.clone();
            let config = config.clone();
            let operation = Uuid::new_v4();
            view.grep_operation = Some(operation);
            let computation = cx
                .background_executor()
                .spawn(async move { compute_detail_grep_summary(&raw, &config) });
            view.grep_task = Some(cx.spawn(async move |this, cx| {
                let summary = computation.await;
                let _ = this.update(cx, |this, cx| {
                    if this.project != project {
                        return;
                    }
                    if let Some(view) = this.intruder.results.views.get_mut(&view_id)
                        && view.grep_operation == Some(operation)
                    {
                        view.grep_operation = None;
                        view.detail_grep_summary = Some(summary);
                        cx.notify();
                    }
                });
            }));
        }
    }

    pub(in crate::workbench::intruder) fn query_intruder_result_view(
        &mut self,
        view_id: Uuid,
        offset: usize,
        filter: IntruderResultFilter,
        sort: IntruderResultSort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let project = self.project?;
        if !self.backend_available() || self.transition.is_some() {
            return None;
        }
        let view = self.intruder.results.views.get(&view_id)?;
        if view.discarded {
            return None;
        }
        let revision = self.next_intruder_query();
        let grep_config = self.intruder.results.views[&view_id]
            .summary
            .as_ref()
            .and_then(|run| self.workspace.intruder.grep_configs.get(&run.tab_id))
            .cloned()
            .unwrap_or_default();
        let view = self.intruder.results.views.get_mut(&view_id).unwrap();
        if view.query.requested() != (filter.clone(), sort) {
            view.query.queued_rank = None;
        }
        let query = IntruderResultViewQuery {
            project,
            run_id: view.run_id,
            view_id,
            query_revision: revision,
            offset,
            limit: PAGE_SIZE,
            filter,
            sort,
            selected_ordinal: view.selected,
            grep_config,
        };
        if let Some(previous) = view.query.pending.take() {
            previous.cancel.cancel();
        }
        let cancel = self.shutdown.child_token();
        view.query.pending = Some(PendingResultQuery {
            query: query.clone(),
            cancel: cancel.clone(),
        });
        view.query.error = None;
        view.dirty = false;
        let command = Command::QueryIntruderResultView { query, cancel };
        if !self.enqueue(command, cx) {
            let view = self.intruder.results.views.get_mut(&view_id).unwrap();
            view.query.pending = None;
            view.query.error = self.error.clone();
            view.query.outcome = Some((
                revision,
                Err(self
                    .error
                    .clone()
                    .unwrap_or_else(|| "结果查询未入队；当前列表保留。".into())),
            ));
            cx.notify();
            return None;
        }
        let _ = window;
        cx.notify();
        Some(revision)
    }

    pub(in crate::workbench::intruder) fn ensure_intruder_result_rank(
        &mut self,
        view_id: Uuid,
        rank: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return;
        };
        if view.discarded || view.query.pending.is_some() || view.query.error.is_some() {
            return;
        }
        if let Some(page) = &view.query.page
            && (rank >= page.total || view.query.row_at(rank).is_some())
        {
            return;
        }
        let offset = rank;
        let (filter, sort) = view.query.requested();
        self.query_intruder_result_view(view_id, offset, filter, sort, window, cx);
    }

    pub(in crate::workbench::intruder) fn ensure_intruder_result_range(
        &mut self,
        view_id: Uuid,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.intruder.results.views.get_mut(&view_id) else {
            return;
        };
        let range = range.start..range.end.min(range.start.saturating_add(PAGE_SIZE));
        if view.query.visible_range != range && view.query.visible_budget_exhausted {
            view.query.visible_budget_exhausted = false;
            view.query.error = None;
        }
        view.query.visible_range = range;
        view.query
            .visible_rows
            .retain(|rank, _| view.query.visible_range.contains(rank));
        let missing = view.query.page.as_ref().and_then(|page| {
            (view.query.visible_range.start..view.query.visible_range.end.min(page.total))
                .find(|rank| view.query.row_at(*rank).is_none())
        });
        if let Some(rank) = missing {
            self.ensure_intruder_result_rank(view_id, rank, window, cx);
        }
    }

    pub(in crate::workbench) fn refresh_pending_intruder_selection(
        &mut self,
        view_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return;
        };
        let Some(query) = view
            .query
            .pending
            .as_ref()
            .map(|pending| pending.query.clone())
        else {
            return;
        };
        if query.selected_ordinal != view.selected {
            self.query_intruder_result_view(
                view_id,
                query.offset,
                query.filter,
                query.sort,
                window,
                cx,
            );
        }
    }

    pub(in crate::workbench::intruder) fn select_intruder_result_rank(
        &mut self,
        view_id: Uuid,
        rank: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.intruder.results.views.get_mut(&view_id) else {
            return;
        };
        let pointer = view
            .pending_pointer
            .take()
            .filter(|(r, _, _, _)| *r == rank);
        let row = view.query.row_at(rank).cloned();
        if let Some(row) = row {
            let ordinal = row.ordinal;
            if let Some((_, button, shift, toggle)) = pointer {
                if button == gpui_kit::MouseButton::Right {
                    if view.selected_ordinals.contains(&ordinal) && view.selected_ordinals.len() > 1
                    {
                        cx.notify();
                        return;
                    }
                } else if button == gpui_kit::MouseButton::Left && shift {
                    let anchor = view.selection_anchor.or(view.selected).unwrap_or(ordinal);
                    let anchor_rank = view.query.rank_of(anchor).unwrap_or(rank);
                    let (lo, hi) = (anchor_rank.min(rank), anchor_rank.max(rank));
                    let mut range_ordinals = std::collections::BTreeSet::new();
                    for r in lo..=hi {
                        if let Some(entry) = view.query.row_at(r) {
                            range_ordinals.insert(entry.ordinal);
                        }
                    }
                    range_ordinals.insert(anchor);
                    range_ordinals.insert(ordinal);
                    view.selected_ordinals = range_ordinals;
                    if view.selection_anchor.is_none() {
                        view.selection_anchor = Some(anchor);
                    }
                    if row.result.is_none() {
                        let saved = view.selected_ordinals.clone();
                        let saved_anchor = view.selection_anchor;
                        view.clear_detail(window, cx);
                        view.selected = Some(ordinal);
                        view.selected_ordinals = saved;
                        view.selection_anchor = saved_anchor;
                        self.refresh_pending_intruder_selection(view_id, window, cx);
                        cx.notify();
                    } else {
                        self.select_intruder_result(view_id, ordinal, window, cx);
                    }
                    return;
                } else if button == gpui_kit::MouseButton::Left && toggle {
                    if view.selected_ordinals.is_empty()
                        && let Some(cur) = view.selected
                    {
                        view.selected_ordinals.insert(cur);
                    }
                    if view.selected_ordinals.contains(&ordinal) {
                        view.selected_ordinals.remove(&ordinal);
                        if view.selected_ordinals.is_empty() {
                            view.clear_detail(window, cx);
                            self.refresh_pending_intruder_selection(view_id, window, cx);
                            cx.notify();
                            return;
                        }
                        if view.selected == Some(ordinal) {
                            let next_ord = *view.selected_ordinals.iter().next().unwrap();
                            self.select_intruder_result(view_id, next_ord, window, cx);
                        } else {
                            cx.notify();
                        }
                        return;
                    }
                    view.selected_ordinals.insert(ordinal);
                    view.selection_anchor = Some(ordinal);
                    if row.result.is_none() {
                        let saved = view.selected_ordinals.clone();
                        let saved_anchor = view.selection_anchor;
                        view.clear_detail(window, cx);
                        view.selected = Some(ordinal);
                        view.selected_ordinals = saved;
                        view.selection_anchor = saved_anchor;
                        self.refresh_pending_intruder_selection(view_id, window, cx);
                        cx.notify();
                    } else {
                        self.select_intruder_result(view_id, ordinal, window, cx);
                    }
                    return;
                }
            } else if view.selected == Some(ordinal)
                && view.selected_ordinals.len() > 1
                && view.selected_ordinals.contains(&ordinal)
            {
                return;
            }
            view.selected_ordinals.clear();
            view.selected_ordinals.insert(ordinal);
            view.selection_anchor = Some(ordinal);
            if row.result.is_none() {
                view.clear_detail(window, cx);
                view.selected = Some(row.ordinal);
                view.selected_ordinals.insert(row.ordinal);
                view.selection_anchor = Some(row.ordinal);
                self.refresh_pending_intruder_selection(view_id, window, cx);
                cx.notify();
            } else {
                self.select_intruder_result(view_id, row.ordinal, window, cx);
            }
        } else {
            view.query.queued_rank = Some(rank);
            if let Some(pending) = &view.query.pending {
                let same_view = pending.query.filter == view.query.applied
                    && pending.query.sort == view.query.sort;
                if !same_view {
                    view.query.queued_rank = None;
                    return;
                }
            }
            self.ensure_intruder_result_rank(view_id, rank, window, cx);
        }
    }

    pub(in crate::workbench) fn receive_intruder_result_view(
        &mut self,
        query: &IntruderResultViewQuery,
        result: &Result<IntruderResultViewPage, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(view) = self.intruder.results.views.get(&query.view_id) else {
            return false;
        };
        if self.project != Some(query.project)
            || view.run_id != query.run_id
            || view
                .query
                .pending
                .as_ref()
                .is_none_or(|pending| &pending.query != query)
        {
            return false;
        }
        if query.selected_ordinal != view.selected
            || result.as_ref().is_ok_and(|page| {
                view.summary.as_ref().is_some_and(|before| {
                    same_run(before, &page.run) && before.revision > page.run.revision
                })
            })
        {
            self.intruder
                .results
                .views
                .get_mut(&query.view_id)
                .unwrap()
                .query
                .pending = None;
            self.query_intruder_result_view(
                query.view_id,
                query.offset,
                query.filter.clone(),
                query.sort,
                window,
                cx,
            );
            return true;
        }
        let result = result.as_ref().map_err(Clone::clone).and_then(|page| {
            validate_result_view_page(query, page)?;
            if view
                .summary
                .as_ref()
                .is_some_and(|before| !same_run(before, &page.run))
            {
                return Err("结果查询的运行身份或 revision 已变化；当前列表保留。".into());
            }
            Ok(page.clone())
        });
        let view = self.intruder.results.views.get_mut(&query.view_id).unwrap();
        view.query.pending = None;
        match result {
            Ok(mut page) => {
                let desired_rank = view.query.queued_rank.take();
                let same_snapshot = view.query.applied == query.filter
                    && view.query.sort == query.sort
                    && view
                        .query
                        .page
                        .as_ref()
                        .is_some_and(|previous| previous.run.revision == page.run.revision);
                if same_snapshot {
                    if let Some(previous) = view.query.page.take() {
                        for (index, row) in previous.rows.into_iter().enumerate() {
                            let rank = previous.offset + index;
                            if view.query.visible_range.contains(&rank) {
                                view.query.visible_rows.insert(rank, row);
                            }
                        }
                    }
                } else {
                    view.query.visible_rows.clear();
                }
                if !view.query.visible_rows.is_empty() && !view.query.visible_range.is_empty() {
                    let original_offset = page.offset;
                    let keep = view.query.visible_range.clone();
                    let mut first = None;
                    page.rows = page
                        .rows
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, row)| {
                            let rank = original_offset + index;
                            if keep.contains(&rank) {
                                first.get_or_insert(rank);
                                Some(row)
                            } else {
                                if page.selected_rank == Some(rank) {
                                    page.selected_row = Some(row);
                                }
                                None
                            }
                        })
                        .collect();
                    page.offset = first.unwrap_or(query.offset);
                    page.next_offset = (page.offset + page.rows.len() < page.total)
                        .then_some(page.offset + page.rows.len());
                }
                view.query.visible_rows.retain(|rank, _| {
                    view.query.visible_range.contains(rank)
                        && !(page.offset..page.offset + page.rows.len()).contains(rank)
                        && page.selected_rank != Some(*rank)
                });
                let mut bytes = result_page_bytes(&page);
                let mut exhausted = false;
                view.query.visible_rows.retain(|_, row| {
                    let next = bytes.saturating_add(result_row_bytes(row));
                    if next > MAX_RESULT_VIEW_BYTES {
                        exhausted = true;
                        false
                    } else {
                        bytes = next;
                        true
                    }
                });
                view.query.visible_budget_exhausted = exhausted;
                view.merge_summary(&page.run);
                view.persisted = Some(page.persisted);
                view.query.applied = query.filter.clone();
                view.query.sort = query.sort;
                view.query.error = exhausted.then(|| {
                    "当前可见结果超过 1 MiB 摘要预算；已保留可读取的结果，请滚动后继续。".into()
                });
                view.query.outcome = Some((query.query_revision, Ok(())));
                if query.selected_ordinal == view.selected
                    && view.selected.is_some()
                    && page.selected_rank.is_none()
                {
                    view.clear_detail(window, cx);
                }
                let run = page.run.clone();
                let unpersisted =
                    !page.persisted && run.retention == mio_core::IntruderRetention::Project;
                view.query.page = Some(page);
                let selected = view.selected;
                let select_rank = desired_rank;
                self.update_intruder_run_summary(&run);
                if unpersisted {
                    self.status =
                        "Intruder Project 结果尚未保存（待写盘）；保存仅重试写盘，不会重发请求。"
                            .into();
                }
                if let Some(rank) = select_rank {
                    self.select_intruder_result_rank(query.view_id, rank, window, cx);
                } else if let Some(ordinal) = selected {
                    self.select_intruder_result(query.view_id, ordinal, window, cx);
                }
            }
            Err(error) => {
                view.query.error = Some(error.clone());
                view.query.outcome = Some((query.query_revision, Err(error)));
            }
        }
        cx.notify();
        true
    }
}
