use super::*;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone)]
pub(in crate::workbench::websocket) enum WsBatchOpKind {
    Organiser,
    Comparer,
    CopyPayload { as_hex: bool },
}

#[derive(Clone)]
pub(in crate::workbench::websocket) struct PendingWsBatchOp {
    pub(in crate::workbench::websocket) kind: WsBatchOpKind,
    pub(in crate::workbench::websocket) remaining: VecDeque<Uuid>,
    pub(in crate::workbench::websocket) collected: Vec<WebSocketMessage>,
}

#[derive(Default, Clone)]
pub(in crate::workbench::websocket) struct WsMultiSelectionState {
    pub(in crate::workbench::websocket) selected_ids: HashSet<Uuid>,
    pub(in crate::workbench::websocket) anchor_id: Option<Uuid>,
    pub(in crate::workbench::websocket) pending_pointer:
        Option<(usize, gpui_kit::MouseButton, bool, bool)>,
    pub(in crate::workbench::websocket) pending_batch: Option<PendingWsBatchOp>,
}

std::thread_local! {
    static WS_MULTI_SELECTION: std::cell::RefCell<HashMap<gpui_kit::EntityId, WsMultiSelectionState>> =
        std::cell::RefCell::new(HashMap::new());
}

pub(in crate::workbench::websocket) fn record_ws_pointer_down(
    owner_id: gpui_kit::EntityId,
    row: usize,
    button: gpui_kit::MouseButton,
    shift: bool,
    toggle: bool,
) {
    WS_MULTI_SELECTION.with(|map| {
        let mut map = map.borrow_mut();
        map.entry(owner_id).or_default().pending_pointer =
            Some((row, button, shift, toggle));
    });
}

pub(in crate::workbench::websocket) fn is_ws_id_selected(
    owner_id: gpui_kit::EntityId,
    id: Uuid,
    primary_selected: Option<Uuid>,
) -> bool {
    if primary_selected == Some(id) {
        return true;
    }
    WS_MULTI_SELECTION.with(|map| {
        map.borrow()
            .get(&owner_id)
            .is_some_and(|st| st.selected_ids.contains(&id))
    })
}

pub(in crate::workbench::websocket) fn ws_selected_ids_snapshot(
    owner_id: gpui_kit::EntityId,
    primary_selected: Option<Uuid>,
) -> HashSet<Uuid> {
    let mut set = WS_MULTI_SELECTION.with(|map| {
        map.borrow()
            .get(&owner_id)
            .map(|st| st.selected_ids.clone())
            .unwrap_or_default()
    });
    if set.is_empty()
        && let Some(primary) = primary_selected
    {
        set.insert(primary);
    }
    set
}

pub(in crate::workbench::websocket) fn clear_ws_multi_selection(owner_id: gpui_kit::EntityId) {
    WS_MULTI_SELECTION.with(|map| {
        let mut map = map.borrow_mut();
        if let Some(st) = map.get_mut(&owner_id) {
            st.selected_ids.clear();
            st.anchor_id = None;
            st.pending_pointer = None;
            st.pending_batch = None;
        }
    });
}

impl Workbench {
    pub(in crate::workbench) fn select_websocket_row(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.leave_websocket_notes(window, cx) {
            return;
        }
        let owner_id = cx.entity_id();
        let pointer = WS_MULTI_SELECTION.with(|map| {
            let mut map = map.borrow_mut();
            map
                .get_mut(&owner_id)
                .and_then(|st| st.pending_pointer.take())
                .filter(|(r, _, _, _)| *r == row)
        });
        let delegate = self.websocket.table.read(cx).delegate();
        if let Some(message) = delegate.row(row) {
            let id = message.id;
            if let Some((_, button, shift, toggle)) = pointer {
                if button == gpui_kit::MouseButton::Right {
                    let already_in_multi = WS_MULTI_SELECTION.with(|map| {
                        map.borrow()
                            .get(&owner_id)
                            .is_some_and(|st| st.selected_ids.contains(&id) && st.selected_ids.len() > 1)
                    });
                    if already_in_multi {
                        if let Some(primary) = self.websocket.selected
                            && let Some(pos) = delegate.rows.iter().position(|r| r.id == primary)
                        {
                            let primary_row = delegate.offset + pos;
                            self.websocket.table.update(cx, |table, cx| {
                                table.set_selected_row_preserving_context(primary_row, cx);
                            });
                        }
                        cx.notify();
                        return;
                    }
                } else if button == gpui_kit::MouseButton::Left && shift {
                    let anchor = WS_MULTI_SELECTION
                        .with(|map| map.borrow().get(&owner_id).and_then(|st| st.anchor_id))
                        .or(self.websocket.selected)
                        .unwrap_or(id);
                    let start_idx = delegate
                        .rows
                        .iter()
                        .position(|r| r.id == anchor)
                        .unwrap_or_else(|| row.saturating_sub(delegate.offset));
                    let end_idx = row
                        .saturating_sub(delegate.offset)
                        .min(delegate.rows.len().saturating_sub(1));
                    let (lo, hi) = (start_idx.min(end_idx), start_idx.max(end_idx));
                    let range_ids: HashSet<Uuid> = delegate.rows[lo..=hi]
                        .iter()
                        .map(|r| r.id)
                        .collect();
                    WS_MULTI_SELECTION.with(|map| {
                        let mut map = map.borrow_mut();
                        let st = map.entry(owner_id).or_default();
                        st.selected_ids = range_ids;
                        if st.anchor_id.is_none() {
                            st.anchor_id = Some(anchor);
                        }
                    });
                    self.select_websocket(id, window, cx);
                    return;
                } else if button == gpui_kit::MouseButton::Left && toggle {
                    let mut next_primary = Some(id);
                    let mut next_row_idx = None;
                    let mut cleared_all = false;
                    WS_MULTI_SELECTION.with(|map| {
                        let mut map = map.borrow_mut();
                        let st = map.entry(owner_id).or_default();
                        if st.selected_ids.is_empty()
                            && let Some(cur) = self.websocket.selected
                        {
                            st.selected_ids.insert(cur);
                        }
                        if st.selected_ids.contains(&id) {
                            st.selected_ids.remove(&id);
                            if st.selected_ids.is_empty() {
                                st.anchor_id = None;
                                next_primary = None;
                                cleared_all = true;
                            } else if self.websocket.selected == Some(id) {
                                next_primary = delegate
                                    .rows
                                    .iter()
                                    .find(|r| st.selected_ids.contains(&r.id))
                                    .map(|r| r.id);
                                next_row_idx = next_primary.and_then(|pid| {
                                    delegate
                                        .rows
                                        .iter()
                                        .position(|r| r.id == pid)
                                        .map(|pos| delegate.offset + pos)
                                });
                            } else {
                                next_primary = self.websocket.selected;
                            }
                        } else {
                            st.selected_ids.insert(id);
                            st.anchor_id = Some(id);
                        }
                    });
                    if cleared_all {
                        self.clear_websocket_detail(window, cx);
                        self.websocket.table.update(cx, |table, cx| {
                            table.clear_selection(cx);
                        });
                        cx.notify();
                        return;
                    }
                    if let Some(primary_id) = next_primary {
                        self.select_websocket(primary_id, window, cx);
                    }
                    if let Some(target_row) = next_row_idx {
                        self.websocket.table.update(cx, |table, cx| {
                            table.set_selected_row_without_scroll(target_row, cx);
                        });
                    }
                    cx.notify();
                    return;
                }
            }
            WS_MULTI_SELECTION.with(|map| {
                let mut map = map.borrow_mut();
                let st = map.entry(owner_id).or_default();
                st.selected_ids.clear();
                st.selected_ids.insert(id);
                st.anchor_id = Some(id);
            });
            self.select_websocket(id, window, cx);
        } else if row < delegate.total {
            self.clear_websocket_detail(window, cx);
            self.websocket.pending_row = Some(row);
            self.ensure_websocket_visible(window, cx);
        }
    }

    pub(in crate::workbench) fn ensure_websocket_visible(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Proxy || self.proxy_page != ProxyPage::WebSocketHistory {
            return;
        }
        let delegate = self.websocket.table.read(cx).delegate();
        if delegate.loading || delegate.error.is_some() {
            return;
        }
        if self.websocket.selection_filter_pending && self.websocket.selected.is_some() {
            let offset = delegate.offset;
            self.query_websocket_history(self.websocket.filter, offset, window, cx);
            return;
        }
        let range = &delegate.visible;
        let offset = self
            .websocket
            .pending_row
            .filter(|row| *row < delegate.total && delegate.row(*row).is_none())
            .or_else(|| {
                (!range.is_empty()
                    && (delegate.row(range.start).is_none()
                        || delegate.row(range.end - 1).is_none()))
                .then_some(range.start)
            });
        if let Some(offset) = offset {
            self.query_websocket_history(self.websocket.filter, offset, window, cx);
        }
    }

    pub(in crate::workbench) fn clear_websocket_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.websocket.payload.has_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        clear_ws_multi_selection(cx.entity_id());
        self.websocket.clear_detail(window, cx);
    }

    pub(in crate::workbench) fn refresh_websocket_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Proxy || self.proxy_page != ProxyPage::WebSocketHistory {
            return;
        }
        let (loading, offset) = {
            let delegate = self.websocket.table.read(cx).delegate();
            (
                delegate.loading,
                if delegate.visible.is_empty() {
                    delegate.offset
                } else {
                    delegate.visible.start
                },
            )
        };
        if self.websocket.dirty
            && !loading
            && self.websocket.table.read(cx).delegate().error.is_none()
        {
            self.query_websocket_history(self.websocket.filter, offset, window, cx);
        }
    }

    pub(in crate::workbench) fn query_websocket_history(
        &mut self,
        filter: Option<FlowId>,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.query_websocket_filtered_with_scope(
            filter,
            offset,
            self.websocket.applied_filter.clone(),
            self.websocket.in_scope_only,
            self.websocket.filter_enabled,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn query_websocket_filtered_with_scope(
        &mut self,
        flow_id: Option<FlowId>,
        offset: usize,
        filter: WebSocketHistoryFilter,
        in_scope_only: bool,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let project = self.project?;
        let Some(revision) = self.websocket.query_revision.checked_add(1) else {
            self.websocket.dirty = false;
            self.fail("WebSocket 查询 revision 已耗尽。请重新打开项目。", cx);
            return None;
        };
        self.websocket.dirty = false;
        self.websocket.query_revision = revision;
        self.websocket.query_offset = offset;
        self.websocket.query_filter = filter.clone();
        self.websocket.query_in_scope_only = in_scope_only;
        self.websocket.query_filter_enabled = enabled;
        if !self.enqueue(
            Command::QueryWebSocketHistory {
                project,
                query_revision: revision,
                flow_id,
                selected_message_id: self.websocket.selected,
                filter: if enabled {
                    filter.clone()
                } else {
                    WebSocketHistoryFilter::default()
                },
                offset,
                limit: WS_HISTORY_BATCH_SIZE,
            },
            cx,
        ) {
            self.websocket.query_offset = offset;
            let error = self.error.clone();
            self.websocket.table.update(cx, |state, cx| {
                state.delegate_mut().error = error;
                state.delegate_mut().loading = false;
                state.refresh(cx);
            });
            self.websocket.filter_outcome = Some((
                revision,
                Err(self
                    .error
                    .clone()
                    .unwrap_or_else(|| "筛选查询未发送。".into())),
            ));
            return None;
        }
        let changed_filter = self.websocket.filter != flow_id;
        self.websocket.filter = flow_id;
        if self.websocket.applied_filter != filter
            || self.websocket.in_scope_only != in_scope_only
            || self.websocket.filter_enabled != enabled
            || self.websocket.applied_search != self.websocket.query_search
        {
            self.websocket.pending_row = None;
        }
        if changed_filter {
            self.clear_websocket_detail(window, cx);
        }
        self.websocket.table.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.loading = true;
            delegate.error = None;
            if changed_filter {
                delegate.rows.clear();
                delegate.offset = offset;
                delegate.total = 0;
                delegate.visible = 0..0;
                state.clear_selection(cx);
                state.scroll_to_row(0, cx);
            }
            state.refresh(cx);
        });
        cx.notify();
        Some(revision)
    }

    pub(in crate::workbench) fn select_websocket(
        &mut self,
        message_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if self.websocket.selected == Some(message_id) && self.websocket.detail_error.is_none() {
            return;
        }
        let Some(revision) = self.websocket.selection_revision.checked_add(1) else {
            self.fail("WebSocket 选择 revision 已耗尽。请重新打开项目。", cx);
            return;
        };
        let summary = self
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .find(|row| row.id == message_id)
            .cloned()
            .or_else(|| {
                self.websocket
                    .selected_summary
                    .clone()
                    .filter(|row| row.id == message_id)
            });
        let Some(summary) = summary else { return };
        if !self.leave_websocket_notes(window, cx) {
            return;
        }
        self.clear_websocket_detail(window, cx);
        self.websocket.selected = Some(message_id);
        self.websocket.selected_summary = Some(summary);
        self.websocket.selection_filter_pending = self.websocket.applied_filter
            != self.websocket.query_filter
            || self.websocket.in_scope_only != self.websocket.query_in_scope_only
            || self.websocket.filter_enabled != self.websocket.query_filter_enabled
            || self.websocket.applied_search != self.websocket.query_search;
        self.websocket.selection_revision = revision;
        if !self.enqueue(
            Command::ReadWebSocket {
                project,
                selection_revision: revision,
                message_id,
            },
            cx,
        ) {
            self.websocket.detail_error = self.error.clone();
        }
        self.select_websocket_notes(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn on_websocket_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::WebSocketHistoryChanged { project } if self.project == Some(project) => {
                self.websocket.dirty = true;
                self.has_traffic = true;
            }
            Event::WebSocketHistoryPage {
                project,
                query_revision,
                flow_id,
                filter,
                selected_match,
                offset,
                total,
                mut rows,
            } if self.project == Some(project)
                && query_revision == self.websocket.query_revision
                && flow_id == self.websocket.filter
                && filter
                    == if self.websocket.query_filter_enabled {
                        self.websocket.query_filter.clone()
                    } else {
                        WebSocketHistoryFilter::default()
                    } =>
            {
                for row in &mut rows {
                    if let Some(previous) = self
                        .websocket
                        .table
                        .read(cx)
                        .delegate()
                        .rows
                        .iter()
                        .chain(self.websocket.selected_summary.iter())
                        .find(|previous| {
                            previous.id == row.id && previous.notes_revision > row.notes_revision
                        })
                    {
                        row.notes_revision = previous.notes_revision;
                        row.notes_preview = previous.notes_preview.clone();
                    }
                }
                self.has_traffic |= total > 0;
                self.websocket.filter_outcome = Some((query_revision, Ok(())));
                let changed_filter = self.websocket.applied_filter != self.websocket.query_filter
                    || self.websocket.in_scope_only != self.websocket.query_in_scope_only
                    || self.websocket.filter_enabled != self.websocket.query_filter_enabled
                    || self.websocket.applied_search != self.websocket.query_search;
                if changed_filter {
                    self.websocket.pending_row = None;
                }
                self.websocket.applied_filter = self.websocket.query_filter.clone();
                self.websocket.in_scope_only = self.websocket.query_in_scope_only;
                self.websocket.filter_enabled = self.websocket.query_filter_enabled;
                self.websocket.applied_search = self.websocket.query_search.clone();
                let mut total = total;
                let mut selected_match = selected_match;
                let deleted_ids = self
                    .websocket
                    .table
                    .read(cx)
                    .delegate()
                    .deleted_ids
                    .clone();
                if !deleted_ids.is_empty() {
                    let before_del = rows.len();
                    rows.retain(|row| !deleted_ids.contains(&row.id));
                    let removed_del = before_del.saturating_sub(rows.len());
                    total = total.saturating_sub(removed_del).max(rows.len());
                    if let Some((sel_id, ref mut matched)) = selected_match
                        && deleted_ids.contains(&sel_id)
                    {
                        *matched = false;
                    }
                }
                let raw_rows_len = rows.len();
                if self.websocket.filter_enabled && self.websocket.in_scope_only {
                    let before_len = rows.len();
                    rows.retain(|row| {
                        super::websocket_url_in_scope(&self.workspace.scope, &row.url)
                    });
                    let filtered_out = before_len.saturating_sub(rows.len());
                    if offset == 0 && raw_rows_len < WS_HISTORY_BATCH_SIZE {
                        total = rows.len();
                    } else {
                        total = total.saturating_sub(filtered_out).max(rows.len());
                    }
                    if let Some((sel_id, ref mut matched)) = selected_match
                        && *matched
                        && let Some(sel_summary) = self
                            .websocket
                            .selected_summary
                            .as_ref()
                            .filter(|s| s.id == sel_id)
                        && !super::websocket_url_in_scope(
                            &self.workspace.scope,
                            &sel_summary.url,
                        )
                    {
                        *matched = false;
                    }
                }
                let search_term = self.websocket.applied_search.trim().to_owned();
                if !search_term.is_empty() {
                    let before_len = rows.len();
                    rows.retain(|row| websocket_summary_matches_search(row, &search_term));
                    let filtered_out = before_len.saturating_sub(rows.len());
                    if offset == 0 && raw_rows_len < WS_HISTORY_BATCH_SIZE {
                        total = rows.len();
                    } else {
                        total = total.saturating_sub(filtered_out).max(rows.len());
                    }
                    if let Some((sel_id, ref mut matched)) = selected_match
                        && *matched
                        && let Some(sel_summary) = self
                            .websocket
                            .selected_summary
                            .as_ref()
                            .filter(|s| s.id == sel_id)
                        && !websocket_summary_matches_search(sel_summary, &search_term)
                    {
                        *matched = false;
                    }
                }
                let selection_matches = selected_match
                    .filter(|(id, _)| Some(*id) == self.websocket.selected)
                    .map(|(_, matches)| matches);
                if selection_matches.is_some() {
                    self.websocket.selection_filter_pending = false;
                } else if changed_filter && self.websocket.selected.is_some() {
                    self.websocket.selection_filter_pending = true;
                }
                let in_rows = self
                    .websocket
                    .selected
                    .and_then(|id| rows.iter().find(|row| row.id == id));
                let changed = in_rows.is_some_and(|current| {
                    self.websocket
                        .selected_summary
                        .as_ref()
                        .is_none_or(|previous| {
                            previous.revision != current.revision
                                || previous.flow_id != current.flow_id
                        })
                });
                if in_rows.is_some() {
                    self.websocket.selection_filter_pending = false;
                }
                let removed = selection_matches == Some(false)
                    || selection_matches.is_none()
                        && in_rows.is_none()
                        && if changed_filter {
                            false
                        } else {
                            let delegate = self.websocket.table.read(cx).delegate();
                            delegate.offset == offset
                                && delegate
                                    .rows
                                    .iter()
                                    .any(|row| Some(row.id) == self.websocket.selected)
                        };
                let reread = changed.then_some(self.websocket.selected).flatten();
                if removed || changed {
                    self.clear_websocket_detail(window, cx);
                }
                if let Some(summary) = in_rows.cloned() {
                    self.websocket.selected_summary = Some(summary);
                }
                let selected_id = self.websocket.selected;
                let pending_row = self.websocket.pending_row;
                let mut pending = None;
                self.websocket.table.update(cx, |state, cx| {
                    let delegate = state.delegate_mut();
                    delegate.rows = rows;
                    delegate.offset = offset;
                    delegate.total = total;
                    delegate.loading = false;
                    delegate.error = None;
                    if delegate.has_explicit_sort() {
                        delegate.sort_loaded_rows();
                    }
                    let selected = selected_id
                        .and_then(|id| delegate.rows.iter().position(|row| row.id == id));
                    pending = pending_row.and_then(|row| {
                        row.checked_sub(offset)
                            .and_then(|row| delegate.rows.get(row))
                            .map(|row| row.id)
                    });
                    if changed_filter {
                        delegate.visible = 0..0;
                        state.clear_selection(cx);
                        state.scroll_to_row(0, cx);
                    }
                    if let Some(row) = selected {
                        let row = offset + row;
                        if state.selected_row() != Some(row) {
                            state.set_selected_row(row, cx);
                        }
                    } else if removed {
                        state.clear_selection(cx);
                    }
                    state.refresh(cx);
                });
                if let Some(id) = reread {
                    self.select_websocket(id, window, cx);
                }
                if let Some(id) = pending {
                    self.select_websocket(id, window, cx);
                }
                self.ensure_websocket_visible(window, cx);
            }
            Event::WebSocketHistoryError {
                project,
                query_revision,
                message,
            } if self.project == Some(project)
                && query_revision == self.websocket.query_revision =>
            {
                self.websocket.dirty = false;
                self.websocket.filter_outcome = Some((query_revision, Err(message.clone())));
                self.websocket.table.update(cx, |state, cx| {
                    state.delegate_mut().loading = false;
                    state.delegate_mut().error = Some(message);
                    state.refresh(cx);
                });
            }
            Event::WebSocketDetail {
                project,
                selection_revision,
                record,
                original_payload,
            } if self.project == Some(project)
                && selection_revision == self.websocket.selection_revision
                && self.websocket.selected == Some(record.id) =>
            {
                if self.websocket.selected_summary.as_ref().is_some_and(|row| {
                    row.id == record.id
                        && row.revision == record.revision
                        && row.flow_id == record.flow_id
                }) {
                    self.websocket.detail = Some(record);
                    self.websocket.detail_error = None;
                    self.websocket.original_payload = original_payload;
                    if self.websocket.original_payload.is_none() {
                        self.websocket.original_view = false;
                    }
                    self.show_websocket_version(self.websocket.original_view, window, cx);
                    self.complete_pending_websocket_transfer(window, cx);
                    if let Some((copy_id, as_hex)) = self.websocket.pending_copy.take()
                        && self.websocket.selected == Some(copy_id)
                    {
                        self.copy_websocket_payload(Some(copy_id), as_hex, window, cx);
                    }
                    self.advance_pending_ws_batch_op(window, cx);
                } else {
                    let summary = self.websocket.selected_summary.clone();
                    self.clear_websocket_detail(window, cx);
                    self.websocket.selected = Some(record.id);
                    self.websocket.selected_summary = summary;
                    self.websocket.detail_error =
                        Some("消息版本已改变，请刷新历史记录后重试。".into());
                }
            }
            Event::WebSocketDetailError {
                project,
                selection_revision,
                message_id,
                message,
            } if self.project == Some(project)
                && selection_revision == self.websocket.selection_revision
                && self.websocket.selected == Some(message_id) =>
            {
                self.websocket.pending_transfer = None;
                self.websocket.pending_copy = None;
                let owner_id = cx.entity_id();
                WS_MULTI_SELECTION.with(|map| {
                    let mut map = map.borrow_mut();
                    if let Some(st) = map.get_mut(&owner_id) {
                        st.pending_batch = None;
                    }
                });
                self.websocket.detail_error = Some(message);
            }
            event @ (Event::WebSocketNotes { .. } | Event::WebSocketNotesChanged { .. }) => {
                let synced_note = match &event {
                    Event::WebSocketNotesChanged {
                        project,
                        message_id,
                        result: Ok(notes),
                        ..
                    } if self.project == Some(*project)
                        && self.websocket.selected == Some(*message_id) =>
                    {
                        Some((*message_id, notes.text.clone()))
                    }
                    _ => None,
                };
                self.on_websocket_notes_event(event, window, cx);
                if let Some((changed_id, note_text)) = synced_note {
                    let owner_id = cx.entity_id();
                    let other_ids: Vec<Uuid> = WS_MULTI_SELECTION.with(|map| {
                        map.borrow()
                            .get(&owner_id)
                            .filter(|st| st.selected_ids.len() > 1 && st.selected_ids.contains(&changed_id))
                            .map(|st| {
                                st.selected_ids
                                    .iter()
                                    .copied()
                                    .filter(|id| *id != changed_id)
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                    if !other_ids.is_empty() {
                        let need_sync: Vec<Uuid> = self
                            .websocket
                            .table
                            .read(cx)
                            .delegate()
                            .rows
                            .iter()
                            .filter(|r| other_ids.contains(&r.id) && r.notes_preview != note_text)
                            .map(|r| r.id)
                            .collect();
                        if !need_sync.is_empty() {
                            self.set_websocket_batch_notes(&need_sync, note_text, cx);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn start_ws_batch_op(
        &mut self,
        ids: &[Uuid],
        kind: WsBatchOpKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let mut remaining: VecDeque<Uuid> = ids.iter().copied().collect();
        let mut collected = Vec::with_capacity(ids.len());
        while let Some(&front_id) = remaining.front() {
            if let Some(detail) = self
                .websocket
                .detail
                .as_deref()
                .filter(|r| r.id == front_id)
            {
                collected.push(detail.clone());
                remaining.pop_front();
            } else {
                break;
            }
        }
        let owner_id = cx.entity_id();
        WS_MULTI_SELECTION.with(|map| {
            let mut map = map.borrow_mut();
            map.entry(owner_id).or_default().pending_batch = Some(PendingWsBatchOp {
                kind,
                remaining,
                collected,
            });
        });
        self.advance_pending_ws_batch_op(window, cx);
    }

    fn advance_pending_ws_batch_op(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner_id = cx.entity_id();
        let loaded = self.websocket.detail.as_deref().cloned();
        let mut next_to_read = None;
        let mut finished = None;
        WS_MULTI_SELECTION.with(|map| {
            let mut map = map.borrow_mut();
            let Some(st) = map.get_mut(&owner_id) else {
                return;
            };
            let Some(batch) = st.pending_batch.as_mut() else {
                return;
            };
            while let Some(&front_id) = batch.remaining.front() {
                if let Some(ref detail) = loaded
                    && detail.id == front_id
                {
                    batch.collected.push(detail.clone());
                    batch.remaining.pop_front();
                } else {
                    next_to_read = Some(front_id);
                    break;
                }
            }
            if next_to_read.is_none() && batch.remaining.is_empty() {
                finished = st.pending_batch.take();
            }
        });
        if let Some(target_id) = next_to_read {
            self.select_websocket(target_id, window, cx);
            return;
        }
        let Some(done) = finished else {
            return;
        };
        match done.kind {
            WsBatchOpKind::Organiser => {
                for record in &done.collected {
                    self.commit_websocket_organiser_item(
                        record.id,
                        record.direction,
                        record.kind,
                        record.observed_unix_ms,
                        &record.payload,
                        window,
                        cx,
                    );
                }
                if done.collected.len() > 1 {
                    self.status = format!(
                        "已批量将 {} 条 WebSocket 消息添加到 Organizer。",
                        done.collected.len()
                    );
                    cx.notify();
                }
            }
            WsBatchOpKind::Comparer => {
                for record in &done.collected {
                    if record.payload.len() > 256 * 1024
                        || self.extra_tools.comparer_items.len() >= 32
                    {
                        break;
                    }
                    let next_id = self
                        .extra_tools
                        .comparer_items
                        .last()
                        .map_or(1, |entry| entry.id + 1);
                    let tag = format!("WebSocket #{}", record.sequence);
                    self.extra_tools
                        .comparer_items
                        .push(tools_extra::ComparerItem {
                            id: next_id,
                            label: format!("{next_id}: {tag} ({} bytes)", record.payload.len()),
                            bytes: record.payload.clone(),
                        });
                }
                let count = self.extra_tools.comparer_items.len();
                if count >= 2 {
                    self.extra_tools.left_index = count - 2;
                    self.extra_tools.right_index = count - 1;
                }
                self.refresh_comparer(window, cx);
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Comparer;
                self.focus.focus(window, cx);
                cx.notify();
            }
            WsBatchOpKind::CopyPayload { as_hex } => {
                let parts: Vec<String> = done
                    .collected
                    .iter()
                    .map(|record| {
                        if as_hex {
                            editing::hex_encode(&record.payload)
                        } else {
                            String::from_utf8_lossy(&record.payload).into_owned()
                        }
                    })
                    .collect();
                let count = parts.len();
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(parts.join("\n")));
                self.error = None;
                self.status = if as_hex {
                    format!("已批量复制 {count} 条 WebSocket 载荷 Hex 到剪贴板。")
                } else {
                    format!("已批量复制 {count} 条 WebSocket 载荷文本到剪贴板。")
                };
                cx.notify();
            }
        }
    }

    pub(in crate::workbench::websocket) fn send_websocket_batch_to_repeater(
        &mut self,
        ids: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        for &message_id in ids {
            let use_original = self.websocket.selected == Some(message_id)
                && self.websocket.detail.is_some()
                && self.websocket.original_view;
            self.send_websocket_to_repeater(message_id, use_original, window, cx);
        }
    }

    pub(in crate::workbench::websocket) fn send_websocket_batch_to_organiser(
        &mut self,
        ids: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        if ids.len() == 1 {
            self.send_websocket_to_organiser(Some(ids[0]), window, cx);
            return;
        }
        self.start_ws_batch_op(ids, WsBatchOpKind::Organiser, window, cx);
    }

    pub(in crate::workbench::websocket) fn send_websocket_batch_to_comparer(
        &mut self,
        ids: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        if ids.len() == 1 {
            self.send_websocket_to_comparer(Some(ids[0]), window, cx);
            return;
        }
        self.start_ws_batch_op(ids, WsBatchOpKind::Comparer, window, cx);
    }

    pub(in crate::workbench::websocket) fn set_websocket_batch_highlight(
        &mut self,
        ids: &[Uuid],
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || color > 9 || ids.is_empty() {
            return;
        }
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            for &message_id in ids {
                if color == 0 {
                    delegate.highlights.remove(&message_id);
                } else {
                    delegate.highlights.insert(message_id, color);
                }
            }
            table.refresh(cx);
        });
        self.error = None;
        self.status = if color == 0 {
            format!("已清除 {} 条 WebSocket 消息高亮。", ids.len())
        } else {
            format!("已批量更新 {} 条 WebSocket 消息高亮颜色。", ids.len())
        };
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn copy_websocket_batch_urls(
        &mut self,
        ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || ids.is_empty() {
            return;
        }
        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
        let urls: Vec<String> = self
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .filter(|row| id_set.contains(&row.id) && !row.url.is_empty())
            .map(|row| row.url.clone())
            .collect();
        if urls.is_empty() {
            self.fail("所选 WebSocket 消息没有可复制的 URL。", cx);
            return;
        }
        let count = urls.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(urls.join("\n")));
        self.error = None;
        self.status = if count == 1 {
            "已复制 URL 到剪贴板。".into()
        } else {
            format!("已批量复制 {count} 条 WebSocket URL 到剪贴板。")
        };
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn copy_websocket_batch_payloads(
        &mut self,
        ids: &[Uuid],
        as_hex: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.copy_websocket_payload(ids.first().copied(), as_hex, window, cx);
            return;
        }
        self.start_ws_batch_op(ids, WsBatchOpKind::CopyPayload { as_hex }, window, cx);
    }

    pub(in crate::workbench::websocket) fn set_websocket_batch_notes(
        &mut self,
        ids: &[Uuid],
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || ids.is_empty()
        {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let rev_map: HashMap<Uuid, u64> = self
            .websocket
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|r| (r.id, r.notes_revision))
            .collect();
        for &id in ids {
            let expected_notes_revision = self
                .websocket
                .notes
                .drafts
                .get(&id)
                .map(|d| d.stored.revision)
                .or_else(|| rev_map.get(&id).copied())
                .unwrap_or(0);
            let _ = self.enqueue(
                Command::SetWebSocketNotes {
                    project,
                    operation_id: Uuid::new_v4(),
                    message_id: id,
                    expected_notes_revision,
                    text: text.clone(),
                    cancel: CancellationToken::new(),
                },
                cx,
            );
            self.update_websocket_notes_summary(
                id,
                &mio_core::WebSocketNotes {
                    revision: expected_notes_revision.saturating_add(1),
                    text: text.clone(),
                },
                cx,
            );
        }
        self.error = None;
        self.status = if text.is_empty() {
            format!("已批量清除 {} 条 WebSocket 消息备注。", ids.len())
        } else {
            format!("已批量更新 {} 条 WebSocket 消息备注。", ids.len())
        };
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn delete_websocket_batch_entries(
        &mut self,
        ids: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || ids.is_empty() {
            return;
        }
        if ids.len() == 1 {
            self.delete_websocket_history_entry(ids[0], window, cx);
            return;
        }
        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
        if self
            .websocket
            .selected
            .is_some_and(|sel| id_set.contains(&sel))
        {
            if !self.leave_websocket_notes(window, cx) {
                return;
            }
            self.clear_websocket_detail(window, cx);
        } else {
            clear_ws_multi_selection(cx.entity_id());
        }
        let mut removed_count = 0usize;
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.deleted_ids.extend(id_set.iter().copied());
            let before = delegate.rows.len();
            delegate.rows.retain(|row| !id_set.contains(&row.id));
            for id in &id_set {
                delegate.highlights.remove(id);
            }
            removed_count = before.saturating_sub(delegate.rows.len());
            delegate.total = delegate.total.saturating_sub(removed_count);
            table.clear_selection(cx);
            table.refresh(cx);
        });
        self.error = None;
        self.status = format!("已批量删除 {} 条选中的 WebSocket 历史条目。", removed_count.max(ids.len()));
        cx.notify();
    }
}

fn websocket_summary_matches_search(row: &WebSocketSummary, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    row.url.to_lowercase().contains(&needle)
        || row.notes_preview.to_lowercase().contains(&needle)
        || super::direction_label(row.direction)
            .to_lowercase()
            .contains(&needle)
        || row.ordinal.to_string().contains(&needle)
        || row.connection_id.to_string().contains(&needle)
        || row
            .listener_port
            .is_some_and(|port| port.to_string().contains(&needle))
}
