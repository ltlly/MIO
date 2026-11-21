use std::collections::HashMap;

use gpui_kit::{ScrollStrategy, UniformListScrollHandle};
use mio_core::{FlowSummary, TrafficSource};

use super::*;

mod render;
mod table;
pub(super) use render::LoggerRowsCacheEntry;
pub(super) use table::LoggerUiTableState;

pub(super) const LOGGER_MAX_LIMITS: [usize; 4] = [500, 1000, 2000, 5000];
pub(super) const LOGGER_CAPTURE_SOURCE_LABELS: [&str; 5] = [
    "Proxy",
    "Repeater",
    "Intruder",
    "Scanner",
    "Extensions",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoggerExtendedSortColumn {
    Time,
    Host,
    Path,
    Query,
    ParamCount,
    StartResponseTimer,
    Comment,
}

pub(in crate::workbench) struct LoggerFilters {
    pub(in crate::workbench) status_classes: [bool; 4],
    pub(in crate::workbench) in_scope_only: bool,
    pub(in crate::workbench) script_enabled: bool,
    pub(in crate::workbench) capture_in_scope_only: bool,
    pub(in crate::workbench) capture_status_classes: [bool; 4],
    pub(in crate::workbench) capture_sources: [bool; 5],
    pub(in crate::workbench) capture_filter_script: String,
    pub(in crate::workbench) view_filter_script: String,
    pub(super) extended_sort: Option<(LoggerExtendedSortColumn, bool)>,
    pub(super) continuous_scroll: bool,
    pub(super) scroll_handle: UniformListScrollHandle,
    pub(super) h_scroll_handle: ScrollHandle,
    pub(super) param_counts: HashMap<FlowId, usize>,
}

impl Default for LoggerFilters {
    fn default() -> Self {
        Self {
            status_classes: [true; 4],
            in_scope_only: false,
            script_enabled: false,
            capture_in_scope_only: false,
            capture_status_classes: [true; 4],
            capture_sources: [true; 5],
            capture_filter_script: String::new(),
            view_filter_script: String::new(),
            extended_sort: None,
            continuous_scroll: false,
            scroll_handle: UniformListScrollHandle::new(),
            h_scroll_handle: ScrollHandle::new(),
            param_counts: HashMap::new(),
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn set_logger_max_entries(
        &mut self,
        max_entries: usize,
        cx: &mut Context<Self>,
    ) {
        let max_entries = max_entries.max(1);
        self.extra_tools.logger_max_entries = max_entries;
        if self.extra_tools.logger_entries.len() > max_entries {
            let excess = self.extra_tools.logger_entries.len() - max_entries;
            for removed in self.extra_tools.logger_entries.drain(0..excess) {
                self.extra_tools.logger_filters.param_counts.remove(&removed.id);
            }
        }
        cx.notify();
    }

    fn set_logger_source_filter(
        &mut self,
        source: Option<TrafficSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(cancel) = self.extra_tools.logger_query_cancel.take() {
            cancel.cancel();
        }
        self.extra_tools.logger_query_revision =
            self.extra_tools.logger_query_revision.saturating_add(1);
        self.extra_tools.logger_source_filter = source;
        self.refresh_logger_history(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_logger_sort(
        &mut self,
        column: LoggerSortColumn,
        cx: &mut Context<Self>,
    ) {
        if self.extra_tools.logger_filters.extended_sort.is_none()
            && self.extra_tools.logger_sort.column == column
        {
            self.extra_tools.logger_sort.ascending = !self.extra_tools.logger_sort.ascending;
        } else {
            self.extra_tools.logger_filters.extended_sort = None;
            self.extra_tools.logger_sort = LoggerSort {
                column,
                ascending: true,
            };
        }
        cx.notify();
    }

    fn toggle_logger_extended_sort(
        &mut self,
        column: LoggerExtendedSortColumn,
        cx: &mut Context<Self>,
    ) {
        self.extra_tools.logger_filters.extended_sort =
            match self.extra_tools.logger_filters.extended_sort {
                Some((current, ascending)) if current == column => Some((column, !ascending)),
                _ => Some((column, true)),
            };
        cx.notify();
    }

    fn toggle_logger_continuous_scroll(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.extra_tools.logger_filters.continuous_scroll;
        self.extra_tools.logger_filters.continuous_scroll = enabled;
        if enabled && !self.extra_tools.logger_entries.is_empty() {
            let last = self.extra_tools.logger_entries.len().saturating_sub(1);
            self.extra_tools
                .logger_filters
                .scroll_handle
                .scroll_to_item(last, ScrollStrategy::Bottom);
        }
        cx.notify();
    }

    fn reset_logger_capture_filters(&mut self, cx: &mut Context<Self>) {
        self.extra_tools.logger_max_entries = 500;
        self.extra_tools.logger_filters.capture_sources = [true; 5];
        self.extra_tools.logger_filters.capture_in_scope_only = false;
        self.extra_tools.logger_filters.capture_status_classes = [true; 4];
        self.extra_tools.logger_filters.capture_filter_script.clear();
        cx.notify();
    }

    fn reset_logger_view_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source_changed = self.extra_tools.logger_source_filter.is_some();
        self.extra_tools.logger_source_filter = None;
        self.extra_tools.logger_filters.in_scope_only = false;
        self.extra_tools.logger_filters.status_classes = [true; 4];
        self.extra_tools.logger_filters.script_enabled = false;
        self.extra_tools.logger_filters.view_filter_script.clear();
        self.extra_tools
            .logger_search
            .update(cx, |input, cx| input.set_value("", window, cx));
        if source_changed {
            self.refresh_logger_history(window, cx);
        }
        cx.notify();
    }

    fn clear_logger_entries(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.extra_tools.logger_selected.is_some() && !self.leave_http_notes(window, cx) {
            return;
        }
        if let Some(cancel) = self.extra_tools.logger_query_cancel.take() {
            cancel.cancel();
        }
        self.extra_tools.logger_query_revision =
            self.extra_tools.logger_query_revision.saturating_add(1);
        self.extra_tools.logger_entries.clear();
        self.extra_tools.logger_filters.param_counts.clear();
        self.extra_tools.logger_selected = None;
        self.extra_tools.logger_pending_read = None;
        self.extra_tools.logger_loaded_detail = None;
        self.clear_logger_selection_state();
        self.extra_tools.logger_request.load(b"", window, cx);
        self.extra_tools.logger_response.load(b"", window, cx);
        self.clear_context_viewer("logger", cx);
        if self.http_notes.open {
            self.select_http_notes(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn export_logger_csv(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search_term = match editing::input_snapshot(&self.extra_tools.logger_search, window, cx)
        {
            Ok(value) => value.trim().to_string(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let (count, csv) = {
            let live_notes = self.history.read(cx).delegate().live_notes.borrow();
            let rows = table::filter_and_sort_logger(
                &self.extra_tools.logger_entries,
                &self.extra_tools.logger_source_filter,
                &search_term,
                self.extra_tools.logger_sort,
                &self.extra_tools.logger_filters,
                &self.workspace.scope,
                &live_notes,
            );
            (rows.len(), table::format_logger_csv(&rows))
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(csv));
        self.error = None;
        self.status = format!("已复制 {count} 条 Logger 记录 (CSV) 到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn refresh_logger_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let filter = match editing::input_snapshot(&self.extra_tools.logger_search, window, cx) {
            Ok(value) => {
                let trimmed = value.trim();
                if self.extra_tools.logger_filters.script_enabled
                    || table::is_script_expression(trimmed)
                {
                    String::new()
                } else {
                    trimmed.to_string()
                }
            }
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        if let Some(cancel) = self.extra_tools.logger_query_cancel.take() {
            cancel.cancel();
        }
        self.extra_tools.logger_query_revision =
            self.extra_tools.logger_query_revision.saturating_add(1);
        let query_revision = self.extra_tools.logger_query_revision;
        let cancel = self.shutdown.child_token();
        if self.enqueue(
            Command::QueryLoggerHistory {
                project,
                query_revision,
                offset: 0,
                limit: 200,
                filter,
                source: self.extra_tools.logger_source_filter.clone(),
                cancel: cancel.clone(),
            },
            cx,
        ) {
            self.extra_tools.logger_query_cancel = Some(cancel);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn record_logger_flow(&mut self, summary: &FlowSummary) {
        if !self.extra_tools.capture_enabled {
            return;
        }
        if !table::matches_logger_capture_filter(
            summary,
            &self.extra_tools.logger_filters,
            &self.workspace.scope,
        ) {
            return;
        }
        if let Some(existing) = self
            .extra_tools
            .logger_entries
            .iter_mut()
            .find(|item| item.id == summary.id)
        {
            if summary.revision >= existing.revision {
                *existing = summary.clone();
            }
            return;
        }
        let max_entries = self.extra_tools.logger_max_entries.max(1);
        if self.extra_tools.logger_entries.len() >= max_entries {
            let excess = self.extra_tools.logger_entries.len() + 1 - max_entries;
            for removed in self.extra_tools.logger_entries.drain(0..excess) {
                self.extra_tools.logger_filters.param_counts.remove(&removed.id);
            }
        }
        self.extra_tools.logger_entries.push(summary.clone());
        if self.extra_tools.logger_filters.continuous_scroll {
            let last = self.extra_tools.logger_entries.len().saturating_sub(1);
            self.extra_tools
                .logger_filters
                .scroll_handle
                .scroll_to_item(last, ScrollStrategy::Bottom);
        }
    }

    pub(in crate::workbench) fn record_logger_finished(
        &mut self,
        record: &FlowRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.extra_tools.capture_enabled {
            return;
        }
        let prev_len = self.extra_tools.logger_entries.len();
        self.record_logger_flow(&record.summary);
        if self.extra_tools.logger_entries.len() > prev_len
            || self
                .extra_tools
                .logger_entries
                .iter()
                .any(|item| item.id == record.summary.id)
        {
            let count = table::count_request_params(&record.request);
            self.extra_tools
                .logger_filters
                .param_counts
                .insert(record.summary.id, count);
        }
        if self.extra_tools.logger_selected == Some(record.summary.id) {
            let original = self
                .extra_tools
                .logger_loaded_detail
                .as_deref()
                .filter(|(existing, _)| existing.summary.id == record.summary.id)
                .map(|(_, original)| original.clone())
                .unwrap_or_default();
            self.extra_tools.logger_loaded_detail = Some(Box::new((record.clone(), original)));
            self.extra_tools
                .logger_request
                .load(&request_to_raw(&record.request), window, cx);
            self.extra_tools.logger_response.load(
                &record
                    .response
                    .as_ref()
                    .map(response_to_raw)
                    .unwrap_or_default(),
                window,
                cx,
            );
        }
    }

    pub(in crate::workbench) fn select_logger_entry(
        &mut self,
        flow_id: FlowId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.extra_tools.logger_selected != Some(flow_id) {
            if !self.leave_http_notes(window, cx) {
                return;
            }
            self.extra_tools.logger_selected = Some(flow_id);
            self.extra_tools.logger_pending_read = None;
            self.extra_tools.logger_loaded_detail = None;
            self.extra_tools.logger_request.load(b"", window, cx);
            self.extra_tools.logger_response.load(b"", window, cx);
            self.clear_context_viewer("logger", cx);
            if self.http_notes.open {
                self.select_http_notes(window, cx);
            }
        }
        if let Some(record) = self
            .detail
            .as_ref()
            .filter(|detail| detail.summary.id == flow_id)
        {
            let count = table::count_request_params(&record.request);
            self.extra_tools
                .logger_filters
                .param_counts
                .insert(flow_id, count);
            self.extra_tools.logger_pending_read = None;
            self.extra_tools.logger_loaded_detail =
                Some(Box::new(((**record).clone(), (*self.detail_original).clone())));
            self.extra_tools
                .logger_request
                .load(&request_to_raw(&record.request), window, cx);
            self.extra_tools.logger_response.load(
                &record
                    .response
                    .as_ref()
                    .map(response_to_raw)
                    .unwrap_or_default(),
                window,
                cx,
            );
        } else if let Some(project) = self.project {
            self.selection_revision = self.selection_revision.saturating_add(1);
            let selection_revision = self.selection_revision;
            if self.enqueue(
                Command::ReadFlow {
                    project,
                    selection_revision,
                    flow_id,
                },
                cx,
            ) {
                self.extra_tools.logger_pending_read = Some((project, selection_revision, flow_id));
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;

    use super::*;
    use crate::workbench::tests::{drain, fixture};

    #[gpui_kit::test]
    fn logger_multiselect_column_layout_copy_and_batch_actions_work(cx: &mut TestAppContext) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        let mut r1 = crate::workbench::tests::record("http://alpha.test/one", "one");
        r1.summary.url = "http://alpha.test/one".into();
        let id1 = r1.summary.id;
        let mut r2 = crate::workbench::tests::record("http://beta.test/two", "two");
        r2.summary.url = "http://beta.test/two".into();
        let id2 = r2.summary.id;
        let mut r3 = crate::workbench::tests::record("http://gamma.test/three", "three");
        r3.summary.url = "http://gamma.test/three".into();
        let id3 = r3.summary.id;

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-logger", cx);
            f.view.update(cx, |view, cx| {
                view.record_logger_flow(&r1.summary);
                view.record_logger_flow(&r2.summary);
                view.record_logger_flow(&r3.summary);
                cx.notify();
            });
            window.render_frame(cx);
            assert!(window.find("logger-sort-host").visible());

            let visible_ids = vec![id1, id2, id3];
            f.view.update(cx, |view, cx| {
                view.handle_logger_row_click(id1, &visible_ids, false, false, window, cx);
                view.handle_logger_row_click(id3, &visible_ids, true, false, window, cx);
                assert_eq!(
                    view.logger_selected_target_ids(id1),
                    vec![id1, id2, id3]
                );
                view.handle_logger_row_click(id2, &visible_ids, false, true, window, cx);
                assert_eq!(view.logger_selected_target_ids(id1), vec![id1, id3]);
                view.handle_logger_row_right_down(id1, window, cx);
                assert_eq!(view.logger_selected_target_ids(id1), vec![id1, id3]);

                view.copy_logger_batch_urls(&[id1, id3], window, cx);
            });
        })
        .unwrap();
        cx.read(|cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("http://alpha.test/one\nhttp://gamma.test/three".into())
            );
        });

        cx.update_window(f.window, |_, window, cx| {
            let rows = vec![(1usize, id1), (2usize, id2), (3usize, id3)];
            f.view.update(cx, |view, cx| {
                view.copy_logger_column(4, &rows, cx);
                view.toggle_logger_column_visibility(4, cx);
                view.fit_logger_column_width(5, &rows, cx);
            });
            window.render_frame(cx);
            assert!(window.try_find("logger-sort-host").is_none());
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("alpha.test\nbeta.test\ngamma.test".into())
            );

            f.view.update(cx, |view, cx| {
                view.reset_logger_column_layout(cx);
                view.delete_logger_selected_entries(&[id1, id3], window, cx);
            });
            window.render_frame(cx);
            assert!(window.find("logger-sort-host").visible());
            assert_eq!(
                f.view
                    .read(cx)
                    .extra_tools
                    .logger_entries
                    .iter()
                    .map(|e| e.id)
                    .collect::<Vec<_>>(),
                vec![id2]
            );
        })
        .unwrap();
    }
}
