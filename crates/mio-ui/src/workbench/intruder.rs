use mio_core::{IntruderRunSummary, IntruderSpec, Uuid};

use super::*;

mod baseline;
mod core_actions;
mod core_start;
mod core_sync;
pub(super) mod dashboard;
mod default_pool;
mod payloads;
mod pool_management;
mod positions;
mod render;
pub(super) mod resource_pools;
mod result_pool;
mod results;
pub(super) mod results_close;
mod results_close_dialog;
mod results_filter;
#[cfg(test)]
mod results_query_boundaries_tests;
#[cfg(test)]
mod results_query_tests;
#[cfg(test)]
mod results_tests;
pub(super) mod results_view;
#[cfg(test)]
mod results_view_tests;
mod tabs;
mod task_name;
mod transfer;

#[derive(Default)]
pub(super) struct Intruder {
    pub(super) editors: HashMap<TabId, IntruderEditors>,
    pub(super) dashboard: dashboard::Dashboard,
    pub(super) session_runs: HashSet<Uuid>,
    pub(super) onboarding_dismissed: bool,
    starts: HashMap<TabId, PendingStart>,
    runs: HashMap<Uuid, IntruderRun>,
    tabs_scroll: ScrollHandle,
    transfer_reads: HashMap<u64, (FlowId, bool)>,
    pub(in crate::workbench) results: results::IntruderResults,
    positions_request: Option<positions::PendingPositions>,
    pub(super) rename: Option<tabs::Rename>,
    tabs_focus: Option<FocusHandle>,
    resize_original: Option<u16>,
    task_name: Option<task_name::TaskRename>,
    baseline_requests: HashMap<Uuid, baseline::PendingBaseline>,
    default_pool: Option<default_pool::PendingDefaultPool>,
    pub(super) resource_pools: resource_pools::ResourcePoolsUi,
    pub(super) pool_changes: HashMap<Uuid, result_pool::PendingPoolChange>,
}

pub(super) struct IntruderEditors {
    pub(super) request: positions::PositionsEditor,
    hex_length: Option<usize>,
    pub(super) target: Entity<InputState>,
    pub(super) search: Entity<InputState>,
    pub(super) payload_input: Entity<InputState>,
    pub(super) numbers_from: Entity<InputState>,
    pub(super) numbers_to: Entity<InputState>,
    pub(super) numbers_step: Entity<InputState>,
    pub(super) numbers_min_digits: Entity<InputState>,
    pub(super) numbers_hex: bool,
    pub(super) grep_match_input: Entity<InputState>,
    pub(super) grep_extract_start: Entity<InputState>,
    pub(super) grep_extract_end: Entity<InputState>,
    pub(super) payload_type: mio_core::intruder::IntruderPayloadType,
    payload_position: Option<usize>,
    payload_selection: HashSet<usize>,
    payloads_revision: u64,
    payloads_identical: bool,
    payloads_open: bool,
    settings_open: bool,
    resource_pool_open: bool,
    pub(super) pool_form: resource_pools::CreatePoolForm,
    baseline_restore: Option<Uuid>,
    loading_payloads: bool,
    dirty: bool,
    loaded_revision: u64,
    _subscriptions: Vec<Subscription>,
}

impl IntruderEditors {
    pub(in crate::workbench) fn has_non_request_focus(&self, window: &Window, cx: &App) -> bool {
        [
            &self.target,
            &self.payload_input,
            &self.numbers_from,
            &self.numbers_to,
            &self.numbers_step,
            &self.numbers_min_digits,
            &self.grep_match_input,
            &self.grep_extract_start,
            &self.grep_extract_end,
        ]
        .into_iter()
        .any(|input| input.focus_handle(cx).is_focused(window))
            || self
                .pool_form
                .inputs()
                .iter()
                .any(|input| input.focus_handle(cx).is_focused(window))
    }

    fn has_composition(&self, window: &mut Window, cx: &mut App) -> bool {
        [&self.request.raw, &self.request.hex]
            .into_iter()
            .any(|input| {
                input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                })
            })
            || [
                &self.target,
                &self.numbers_from,
                &self.numbers_to,
                &self.numbers_step,
                &self.numbers_min_digits,
                &self.grep_match_input,
                &self.grep_extract_start,
                &self.grep_extract_end,
            ]
            .into_iter()
            .any(|input| {
                input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                })
            })
            || self.pool_form.has_composition(window, cx)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StartPhase {
    Saving,
    Starting,
}

struct PendingStart {
    project: ProjectKey,
    run_id: Uuid,
    spec: IntruderSpec,
    after_revision: u64,
    phase: StartPhase,
    cancel: CancellationToken,
}

struct IntruderRun {
    summary: IntruderRunSummary,
    cancel: CancellationToken,
}

impl Intruder {
    fn reset(&mut self) {
        self.baseline_requests.clear();
        for pending in self.starts.values() {
            pending.cancel.cancel();
        }
        for run in self.runs.values() {
            run.cancel.cancel();
        }
        *self = Self::default();
    }
}

fn revised_spec(previous: &IntruderSpec, mut next: IntruderSpec) -> Result<IntruderSpec, String> {
    next.revision = previous.revision;
    next.draft.revision = previous.draft.revision;
    if next != *previous {
        next.revision = advance_revision(previous.revision)?;
        if next.draft != previous.draft {
            next.draft.revision = advance_revision(previous.draft.revision)?;
        }
    }
    next.validate_draft().map_err(|error| error.to_string())?;
    Ok(next)
}

fn advance_revision(revision: u64) -> Result<u64, String> {
    revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or_else(|| "Intruder revision 已耗尽；当前输入仍保留，无法继续保存此配置。".to_owned())
}

fn rebased_spec(local: &IntruderSpec, stored: &IntruderSpec) -> Result<IntruderSpec, String> {
    let mut next = local.clone();
    let draft_changed = local.draft.raw != stored.draft.raw
        || local.draft.target != stored.draft.target
        || local.draft.version != stored.draft.version;
    next.draft.revision = local.draft.revision.max(if draft_changed {
        advance_revision(stored.draft.revision)?
    } else {
        stored.draft.revision
    });
    next.revision = stored.revision;
    // A retained newer draft revision is itself part of the complete persisted spec.
    let minimum = if next != *stored {
        advance_revision(stored.revision)?
    } else {
        stored.revision
    };
    next.revision = local.revision.max(minimum);
    next.validate_draft().map_err(|error| error.to_string())?;
    Ok(next)
}

#[cfg(test)]
mod tests;
