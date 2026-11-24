use gpui_kit::AnyWindowHandle;
use mio_core::{
    IntruderResultFilter, IntruderResultSort, IntruderResultSummary, IntruderResultViewPage,
    IntruderResultViewQuery, IntruderResultViewRow,
};

use super::*;

mod annotation_events;
mod control;
mod events;
mod lifecycle;
mod selection;
mod validation;
mod view_query;
pub(super) use validation::{merge_run_summary, same_run};
use validation::{result_page_bytes, result_row_bytes, validate_result_view_page};

#[path = "results_annotations.rs"]
pub(super) mod results_annotations;

const MAX_RESULT_VIEW_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(in crate::workbench) struct IntruderResults {
    pub(in crate::workbench) runs: RunPage,
    // Keyed by view ID (run ID for detached windows), with one bounded page per view.
    pub(in crate::workbench) views: HashMap<Uuid, ResultView>,
    pub(super) query_revision: u64,
    pub(super) probes: HashMap<Uuid, RunProbe>,
    pub(super) pending_highlights: HashMap<Uuid, results_annotations::PendingHighlight>,
    pub(super) pending_notes: HashMap<Uuid, results_annotations::PendingNotes>,
    pub(super) cached_notes: HashMap<(Uuid, u64), (u64, String)>,
}

#[derive(Default)]
pub(in crate::workbench) struct RunPage {
    pub(in crate::workbench) rows: Vec<IntruderRunSummary>,
    pub(in crate::workbench) after: Option<Uuid>,
    pub(super) next_after: Option<Uuid>,
    pub(super) error: Option<String>,
    pub(in crate::workbench) pending: Option<u64>,
    pub(super) loaded: bool,
    pub(super) dirty: bool,
}

pub(in crate::workbench) struct ResultView {
    pub(super) run_id: Uuid,
    pub(super) discarded: bool,
    pub(in crate::workbench) owner: AnyWindowHandle,
    pub(super) focus: FocusHandle,
    pub(super) summary: Option<IntruderRunSummary>,
    pub(super) rows: Vec<IntruderResultSummary>,
    pub(super) query: ResultQueryState,
    pub(super) baseline_row: Option<IntruderResultSummary>,
    pub(super) after: Option<u64>,
    pub(super) next_after: Option<u64>,
    pub(super) persisted: Option<bool>,
    pub(super) error: Option<String>,
    pub(super) pending: Option<u64>,
    pub(super) selected: Option<u64>,
    pub(super) selected_ordinals: std::collections::BTreeSet<u64>,
    pub(super) selection_anchor: Option<u64>,
    pub(super) pending_pointer: Option<(usize, gpui_kit::MouseButton, bool, bool)>,
    pub(super) detail: Option<Box<FlowRecord>>,
    pub(super) detail_response_raw: Option<std::sync::Arc<[u8]>>,
    pub(super) detail_grep_summary: Option<String>,
    pub(super) grep_operation: Option<Uuid>,
    pub(super) grep_task: Option<Task<()>>,
    pub(super) detail_error: Option<String>,
    pub(in crate::workbench) request: MessageEditor,
    pub(in crate::workbench) response: MessageEditor,
    pub(super) spec: Option<Box<IntruderSpec>>,
    pub(super) spec_error: Option<String>,
    pub(super) spec_pending: Option<u64>,
    pub(super) stop_requested: bool,
    pub(super) control_pending: Option<RunControl>,
    pub(super) control_error: Option<String>,
    pub(super) baseline_error: Option<String>,
    pub(super) pool_error: Option<String>,
    pub(super) close_status: Option<&'static str>,
    pub(super) close_error: Option<String>,
    pub(super) notes_window: Option<AnyWindowHandle>,
    pub(super) notes_ordinal: Option<u64>,
    pub(super) notes_outcomes: HashMap<Uuid, Result<(), String>>,
    pub(super) dirty: bool,
    pub(super) queued_query: Option<u64>,
    pub(super) queued_selection: Option<u64>,
    pub(super) detail_pending: Option<DetailRead>,
    _window_closed: Subscription,
}

pub(super) struct PendingResultQuery {
    pub(super) query: IntruderResultViewQuery,
    pub(super) cancel: CancellationToken,
}

impl Drop for PendingResultQuery {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Default)]
pub(super) struct ResultQueryState {
    pub(super) applied: IntruderResultFilter,
    pub(super) sort: IntruderResultSort,
    pub(super) page: Option<IntruderResultViewPage>,
    pub(super) pending: Option<PendingResultQuery>,
    pub(super) error: Option<String>,
    pub(super) outcome: Option<(u64, Result<(), String>)>,
    pub(super) filter_window: Option<AnyWindowHandle>,
    pub(super) queued_rank: Option<usize>,
    pub(super) visible_range: std::ops::Range<usize>,
    pub(super) visible_rows: HashMap<usize, IntruderResultViewRow>,
    pub(super) visible_budget_exhausted: bool,
}

impl ResultQueryState {
    pub(super) fn requested(&self) -> (IntruderResultFilter, IntruderResultSort) {
        self.pending.as_ref().map_or_else(
            || (self.applied.clone(), self.sort),
            |pending| (pending.query.filter.clone(), pending.query.sort),
        )
    }

    pub(super) fn row_at(&self, rank: usize) -> Option<&IntruderResultViewRow> {
        self.page
            .as_ref()
            .and_then(|page| {
                rank.checked_sub(page.offset)
                    .and_then(|index| page.rows.get(index))
                    .or_else(|| {
                        (page.selected_rank == Some(rank))
                            .then_some(page.selected_row.as_ref())
                            .flatten()
                    })
            })
            .or_else(|| self.visible_rows.get(&rank))
    }

    pub(super) fn rank_of(&self, ordinal: u64) -> Option<usize> {
        let page = self.page.as_ref()?;
        page.rows
            .iter()
            .position(|row| row.ordinal == ordinal)
            .map(|index| page.offset + index)
            .or_else(|| {
                self.visible_rows
                    .iter()
                    .find_map(|(rank, row)| (row.ordinal == ordinal).then_some(*rank))
            })
            .or_else(|| {
                page.selected_row
                    .as_ref()
                    .filter(|row| row.ordinal == ordinal)
                    .and(page.selected_rank)
            })
    }

    pub(super) fn view_row(&self, ordinal: u64) -> Option<&IntruderResultViewRow> {
        self.page
            .as_ref()
            .and_then(|page| {
                page.rows
                    .iter()
                    .find(|row| row.ordinal == ordinal)
                    .or_else(|| {
                        page.selected_row
                            .as_ref()
                            .filter(|row| row.ordinal == ordinal)
                    })
            })
            .or_else(|| {
                self.visible_rows
                    .values()
                    .find(|row| row.ordinal == ordinal)
            })
    }

    pub(super) fn update_rows(
        &mut self,
        ordinal: u64,
        mut update: impl FnMut(&mut IntruderResultViewRow),
    ) {
        if let Some(page) = &mut self.page {
            for row in &mut page.rows {
                if row.ordinal == ordinal {
                    update(row);
                }
            }
            if let Some(selected) = page
                .selected_row
                .as_mut()
                .filter(|row| row.ordinal == ordinal)
            {
                update(selected);
            }
        }
        for row in self.visible_rows.values_mut() {
            if row.ordinal == ordinal {
                update(row);
            }
        }
    }

    pub(super) fn remove_ordinals(&mut self, ordinals: &std::collections::BTreeSet<u64>) -> usize {
        let mut removed = 0usize;
        if let Some(page) = &mut self.page {
            let before = page.rows.len();
            page.rows.retain(|row| !ordinals.contains(&row.ordinal));
            removed = before.saturating_sub(page.rows.len());
            if page
                .selected_row
                .as_ref()
                .is_some_and(|row| ordinals.contains(&row.ordinal))
            {
                page.selected_row = None;
                page.selected_rank = None;
                removed = removed.saturating_add(1);
            }
            page.total = page.total.saturating_sub(removed);
            page.next_offset = (page.offset + page.rows.len() < page.total)
                .then_some(page.offset + page.rows.len());
        }
        self.visible_rows
            .retain(|_, row| !ordinals.contains(&row.ordinal));
        removed
    }
}

#[derive(Clone, Copy)]
pub(super) struct RunControl {
    pub(super) operation_id: Uuid,
    pub(super) paused: bool,
    pub(super) revision: u64,
}

#[derive(Clone, Copy)]
pub(super) struct DetailRead {
    pub(super) selection_revision: u64,
    pub(super) flow_id: FlowId,
    pub(super) flow_revision: u64,
}

pub(super) struct RunProbe {
    pub(super) pending: Option<u64>,
    pub(super) dirty: bool,
    pub(super) error: Option<String>,
}

impl Default for RunProbe {
    fn default() -> Self {
        Self {
            pending: None,
            dirty: true,
            error: None,
        }
    }
}

impl ResultView {
    pub(super) fn new(
        project: ProjectKey,
        view_id: Uuid,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) -> Self {
        let owner = window.window_handle();
        let request = MessageEditor::new(window, cx);
        let editor_id = request.raw.entity_id();
        let weak = cx.weak_entity();
        let window_closed = cx.on_window_closed(move |cx, closed| {
            if closed != owner.window_id() {
                return;
            }
            let weak = weak.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |view, cx| {
                    if view.owns_intruder_result_window(view_id, project, owner, editor_id) {
                        view.forget_intruder_results(view_id, cx);
                    }
                });
            });
        });
        Self {
            run_id,
            discarded: false,
            owner,
            focus: cx.focus_handle(),
            summary: None,
            rows: Vec::new(),
            query: ResultQueryState::default(),
            baseline_row: None,
            after: None,
            next_after: None,
            persisted: None,
            error: None,
            pending: None,
            selected: None,
            selected_ordinals: std::collections::BTreeSet::new(),
            selection_anchor: None,
            pending_pointer: None,
            detail: None,
            detail_response_raw: None,
            detail_grep_summary: None,
            grep_operation: None,
            grep_task: None,
            detail_error: None,
            request,
            response: MessageEditor::new(window, cx),
            spec: None,
            spec_error: None,
            spec_pending: None,
            stop_requested: false,
            control_pending: None,
            control_error: None,
            baseline_error: None,
            pool_error: None,
            close_status: None,
            close_error: None,
            notes_window: None,
            notes_ordinal: None,
            notes_outcomes: HashMap::new(),
            dirty: true,
            queued_query: None,
            queued_selection: None,
            detail_pending: None,
            _window_closed: window_closed,
        }
    }

    pub(super) fn record_notes_outcome(&mut self, operation_id: Uuid, outcome: Result<(), String>) {
        if self.notes_outcomes.len() >= 64
            && !self.notes_outcomes.contains_key(&operation_id)
            && let Some(&evict) = self.notes_outcomes.keys().next()
        {
            self.notes_outcomes.remove(&evict);
        }
        self.notes_outcomes.insert(operation_id, outcome);
    }

    pub(super) fn row(&self, ordinal: u64) -> Option<&IntruderResultSummary> {
        self.query
            .page
            .as_ref()
            .and_then(|page| {
                page.rows
                    .iter()
                    .find(|row| row.ordinal == ordinal)
                    .or_else(|| {
                        page.selected_row
                            .as_ref()
                            .filter(|row| row.ordinal == ordinal)
                    })
                    .and_then(|row| row.result.as_ref())
            })
            .or_else(|| {
                self.query
                    .visible_rows
                    .values()
                    .find(|row| row.ordinal == ordinal)
                    .and_then(|row| row.result.as_ref())
            })
            .or_else(|| {
                self.baseline_row
                    .as_ref()
                    .filter(|row| row.ordinal == ordinal)
                    .or_else(|| self.rows.iter().find(|row| row.ordinal == ordinal))
            })
    }

    pub(super) fn refresh_display_if_changed(&mut self) {
        if !self.discarded
            && let Some(summary) = &self.summary
            && self.query.page.as_ref().is_none_or(|display| {
                display.run.total != summary.total
                    || display.run.dispatched != summary.dispatched
                    || display.run.completed != summary.completed
                    || display.run.baseline != summary.baseline
                    || display.run.state != summary.state
            })
        {
            self.dirty = true;
        }
    }

    pub(super) fn merge_summary(&mut self, summary: &IntruderRunSummary) -> bool {
        let previous = self.summary.as_ref().and_then(|run| run.baseline);
        if let Some(before) = &mut self.summary {
            merge_run_summary(before, summary);
        } else {
            self.summary = Some(summary.clone());
        }
        let baseline = self.summary.as_ref().and_then(|run| run.baseline);
        if previous != baseline {
            self.baseline_row = None;
            self.rows.retain(|row| {
                !previous.is_some_and(|baseline| baseline.ordinal == row.ordinal)
                    && !baseline.is_some_and(|baseline| baseline.ordinal == row.ordinal)
            });
            if self
                .selected
                .is_some_and(|ordinal| previous.is_some_and(|baseline| baseline.ordinal == ordinal))
            {
                // A pending-body reply cannot repopulate the completed baseline's editor.
                self.detail_pending = None;
                self.queued_selection = None;
            }
        }
        previous != baseline
    }

    pub(super) fn clear_detail(&mut self, window: &mut Window, cx: &mut App) {
        debug_assert_eq!(self.owner, window.window_handle());
        if self.request.has_focus(window, cx) || self.response.has_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        self.selected = None;
        self.selected_ordinals.clear();
        self.selection_anchor = None;
        self.query.queued_rank = None;
        self.detail = None;
        self.detail_response_raw = None;
        self.detail_grep_summary = None;
        self.grep_operation = None;
        self.grep_task = None;
        self.detail_error = None;
        self.detail_pending = None;
        self.queued_selection = None;
        self.request.load(&[], window, cx);
        self.response.load(&[], window, cx);
    }

    pub(super) fn load_detail(&mut self, record: &FlowRecord, window: &mut Window, cx: &mut App) {
        debug_assert_eq!(self.owner, window.window_handle());
        let request_focused = [&self.request.raw, &self.request.hex]
            .iter()
            .any(|editor| editor.focus_handle(cx).is_focused(window));
        let response_focused = [&self.response.raw, &self.response.hex]
            .iter()
            .any(|editor| editor.focus_handle(cx).is_focused(window));
        self.request
            .load(&request_to_raw(&record.request), window, cx);
        let response_raw = record.response.as_ref().map(response_to_raw);
        self.response
            .load(response_raw.as_deref().unwrap_or(&[]), window, cx);
        self.detail_response_raw = response_raw.map(std::sync::Arc::from);
        self.detail_grep_summary = None;
        if request_focused {
            self.request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        } else if response_focused {
            self.response
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        self.detail = Some(Box::new(record.clone()));
        self.detail_error = None;
    }
}

pub(in crate::workbench::intruder) fn compute_detail_grep_summary(
    raw_bytes: &[u8],
    config: &mio_core::IntruderGrepConfig,
) -> String {
    let (matched, extracted) = mio_core::intruder::evaluate_grep_config(raw_bytes, config);
    format!(
        "Grep 命中: {} | 提取: {}",
        if matched.is_empty() {
            "无".into()
        } else {
            matched.join(", ")
        },
        if extracted.is_empty() {
            "无".into()
        } else {
            extracted.join(", ")
        }
    )
}
