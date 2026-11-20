use super::*;
use gpui_kit::{KeyDownEvent, Modifiers, MouseButton};

mod retain;

#[cfg(test)]
mod ime_tests;
#[cfg(test)]
mod page_tests;
#[cfg(test)]
mod tests;

const MAX_SELECTED_HISTORY: usize = 500;

impl Workbench {
    pub(in crate::workbench) fn delete_selected_http_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if !self.backend_available()
            || self.transition.is_some()
            || !self.leave_http_notes(window, cx)
        {
            return;
        }
        if self.pending_history_delete.is_some() {
            self.fail("上一条 HTTP 历史删除操作仍在处理中，请稍候。", cx);
            return;
        }
        let mut selected: Vec<_> = self
            .history
            .read(cx)
            .delegate()
            .selected_ids
            .iter()
            .copied()
            .collect();
        if selected.is_empty() {
            selected.extend(self.selected_history);
        }
        if selected.is_empty() {
            self.fail("请先选择要删除的 HTTP 历史记录。", cx);
            return;
        }
        if selected
            .iter()
            .any(|id| matches!(id, HistoryEntryId::External { .. }))
        {
            self.fail(
                "外部源归档记录为只读，不能删除；仅支持删除 MIO 实时 Proxy 记录。",
                cx,
            );
            return;
        }
        let flow_ids = selected
            .into_iter()
            .filter_map(|id| match id {
                HistoryEntryId::Live(id) => Some(id),
                HistoryEntryId::External { .. } => None,
            })
            .collect();
        let operation_id = mio_core::Uuid::new_v4();
        if self.enqueue(
            Command::DeleteHttpHistory {
                project,
                operation_id,
                flow_ids,
            },
            cx,
        ) {
            self.pending_history_delete = Some((project, operation_id));
            self.status = "正在删除选中的实时 Proxy 历史记录…".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn clear_live_proxy_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if !self.backend_available()
            || self.transition.is_some()
            || !self.leave_http_notes(window, cx)
        {
            return;
        }
        if self.pending_history_delete.is_some() {
            self.fail("上一条 HTTP 历史删除操作仍在处理中，请稍候。", cx);
            return;
        }
        let operation_id = mio_core::Uuid::new_v4();
        if self.enqueue(
            Command::ClearLiveProxyHistory {
                project,
                operation_id,
            },
            cx,
        ) {
            self.pending_history_delete = Some((project, operation_id));
            self.status = "正在清空实时 Proxy 历史记录…".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn http_history_deleted(
        &mut self,
        deleted_ids: &[FlowId],
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let deleted: HashSet<_> = deleted_ids.iter().copied().collect();
        let deleted_entry =
            |id: HistoryEntryId| matches!(id, HistoryEntryId::Live(id) if deleted.contains(&id));
        let detail_deleted = self
            .detail
            .as_ref()
            .is_some_and(|detail| deleted.contains(&detail.summary.id));
        if self.selected_history.is_some_and(deleted_entry) || detail_deleted {
            self.selected_history = None;
            self.selected_flow = None;
            self.reset_history_version_view();
            self.selection_revision = self.selection_revision.saturating_add(1);
            self.history_detail_revision = self.selection_revision;
            self.clear_history_detail(window, cx);
        }
        self.http_notes.evict_deleted_live(&deleted, window, cx);
        if self
            .history
            .read(cx)
            .delegate()
            .selected_ids
            .iter()
            .copied()
            .any(deleted_entry)
        {
            self.cancel_history_transfer("所选实时记录已删除；未转送请求。", cx);
        }
        let offset = self.history.read(cx).delegate().offset;
        self.live_http_annotations
            .retain(|(id, ..)| !deleted.contains(id));
        self.annotated_live_flows.retain(|id| !deleted.contains(id));
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate
                .live_numbers
                .retain(|id, _| !deleted.contains(id));
            delegate
                .row_metadata
                .retain(|id, _| !deleted_entry(*id));
            delegate
                .live_notes
                .borrow_mut()
                .retain(|id, _| !deleted.contains(id));
            delegate
                .live_highlights
                .borrow_mut()
                .retain(|id, _| !deleted.contains(id));
            delegate
                .live_highlight_errors
                .borrow_mut()
                .retain(|id, _| !deleted.contains(id));
            delegate.rows.retain(|row| !deleted_entry(row.id()));
            delegate.selected_ids.retain(|id| !deleted_entry(*id));
            delegate.selected_rows.retain(|id, _| !deleted_entry(*id));
            delegate.selected_order.retain(|id| !deleted_entry(*id));
            delegate.selected_ranks.retain(|id, _| !deleted_entry(*id));
            if delegate.selection_anchor.is_some_and(deleted_entry) {
                delegate.selection_anchor = None;
            }
            if delegate.selection_lead.is_some_and(deleted_entry) {
                delegate.selection_lead = None;
            }
            table.refresh(cx);
        });
        self.restore_history_primary(false, cx);
        self.extra_tools
            .logger_entries
            .retain(|entry| !deleted.contains(&entry.id));
        if self
            .extra_tools
            .logger_selected
            .is_some_and(|id| deleted.contains(&id))
        {
            self.extra_tools.logger_selected = None;
            self.extra_tools.logger_pending_read = None;
            self.extra_tools.logger_loaded_detail = None;
            self.extra_tools.logger_request.load(b"", window, cx);
            self.extra_tools.logger_response.load(b"", window, cx);
        }
        if let Some(cancel) = self.extra_tools.logger_query_cancel.take() {
            cancel.cancel();
        }
        self.extra_tools.logger_query_revision =
            self.extra_tools.logger_query_revision.saturating_add(1);
        self.status = format!("已删除 {count} 条实时 Proxy 历史记录。");
        self.history_dirty = true;
        self.query_history(offset, window, cx);
        cx.notify();
    }

    pub(crate) fn history_pointer_selection(
        &mut self,
        id: HistoryEntryId,
        button: MouseButton,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delegate = self.history.read(cx).delegate();
        if !delegate.rows.iter().any(|row| row.id() == id) {
            return;
        }
        if button == MouseButton::Right && delegate.selected_ids.contains(&id) {
            return;
        }
        let mut selected = HashSet::new();
        let mut anchor = delegate.selection_anchor;
        if button == MouseButton::Left && modifiers.shift {
            let start = match anchor {
                Some(anchor) => match delegate.rows.iter().position(|row| row.id() == anchor) {
                    Some(index) => index,
                    None => {
                        self.begin_history_shift_selection(anchor, id, window, cx);
                        return;
                    }
                },
                None => 0,
            };
            let end = delegate.rows.iter().position(|row| row.id() == id).unwrap();
            selected.extend(
                delegate.rows[start.min(end)..=start.max(end)]
                    .iter()
                    .map(HistoryRow::id),
            );
            anchor = Some(delegate.rows[start].id());
        } else if button == MouseButton::Left && modifiers.secondary(cx) {
            selected = delegate.selected_ids.clone();
            if !selected.remove(&id) {
                selected.insert(id);
            }
            anchor = Some(id);
        } else {
            selected.insert(id);
            if button == MouseButton::Left {
                anchor = Some(id);
            }
        }
        let primary = selected.contains(&id).then_some(id).or_else(|| {
            delegate
                .selection_rows()
                .into_iter()
                .map(HistoryRow::id)
                .find(|id| selected.contains(id))
        });
        self.apply_history_selection(
            selected,
            primary,
            (anchor, Some(id)),
            button == MouseButton::Right,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn history_table_selection(
        &mut self,
        row: usize,
        context: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .history
            .read(cx)
            .delegate()
            .rows
            .get(row)
            .map(HistoryRow::id)
        else {
            return;
        };
        // Programmatic restoration emits SelectRow too; do not collapse a retained range.
        if !context && self.selected_history == Some(id) {
            return;
        }
        self.history_pointer_selection(
            id,
            if context {
                MouseButton::Right
            } else {
                MouseButton::Left
            },
            Modifiers::default(),
            window,
            cx,
        );
    }
}
