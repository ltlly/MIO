use gpui_kit::{AnyWindowHandle, EntityId};
use mio_core::{IntruderResultFilter, IntruderResultSort};

use super::*;

impl Workbench {
    pub(in crate::workbench::intruder) fn owns_intruder_result_window(
        &self,
        view_id: Uuid,
        project: ProjectKey,
        owner: AnyWindowHandle,
        editor_id: EntityId,
    ) -> bool {
        self.project == Some(project)
            && self
                .intruder
                .results
                .views
                .get(&view_id)
                .is_some_and(|view| {
                    view.owner == owner && view.request.raw.entity_id() == editor_id
                })
    }

    pub(in crate::workbench) fn defer_intruder_result_window(
        &mut self,
        view_id: Uuid,
        window: &Window,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return false;
        };
        let owner = view.owner;
        if owner == window.window_handle() {
            return false;
        }
        let editor_id = view.request.raw.entity_id();
        let weak = cx.weak_entity();
        // Do not re-enter the Workbench entity while processing a different window's event.
        cx.defer(move |cx| {
            let result = owner.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |view, cx| {
                    if view.owns_intruder_result_window(view_id, project, owner, editor_id) {
                        action(view, window, cx);
                    }
                });
            });
            if result.is_err() {
                let _ = weak.update(cx, |view, cx| {
                    if view.owns_intruder_result_window(view_id, project, owner, editor_id) {
                        view.forget_intruder_results(view_id, cx);
                    }
                });
            }
        });
        true
    }

    pub(in crate::workbench::intruder) fn forget_intruder_results(
        &mut self,
        view_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        self.intruder_result_view_forgotten(view_id, cx);
        let Some(view) = self.intruder.results.views.remove(&view_id) else {
            return;
        };
        self.cancel_invalidated_result_transfer("攻击结果窗口已关闭。", cx);
        if let Some(handle) = view.query.filter_window {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        if let Some(handle) = view.notes_window {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        self.intruder
            .results
            .pending_highlights
            .retain(|_, pending| pending.view_id != view_id);
        self.intruder
            .results
            .pending_notes
            .retain(|_, pending| pending.view_id != view_id);
        let run_id = view.run_id;
        if self.intruder.runs.contains_key(&run_id)
            && !self
                .intruder
                .results
                .views
                .values()
                .any(|view| view.run_id == run_id)
        {
            let probe = self.intruder.results.probes.entry(run_id).or_default();
            probe.dirty = true;
            probe.error = None;
            if !self.backend_disconnected && probe.pending.is_none() {
                self.query_intruder_run_status(run_id, cx);
            }
        }
        // Observation and editors never own the running task's cancellation token.
        cx.notify();
    }

    pub(in crate::workbench) fn next_intruder_query(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let Some(next) = self.intruder.results.query_revision.checked_add(1) else {
            self.intruder.results.runs.dirty = false;
            for view in self.intruder.results.views.values_mut() {
                view.dirty = false;
            }
            for probe in self.intruder.results.probes.values_mut() {
                probe.dirty = false;
            }
            self.fail("Intruder 查询 revision 已耗尽，请重新打开项目。", cx);
            return None;
        };
        self.intruder.results.query_revision = next;
        Some(next)
    }

    pub(in crate::workbench) fn query_intruder_runs(
        &mut self,
        after: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(revision) = self.next_intruder_query(cx) else {
            let page = &mut self.intruder.results.runs;
            page.loaded = true;
            page.pending = None;
            page.error = Some("Intruder 查询 revision 已耗尽，请重新打开项目。".into());
            return;
        };
        let page = &mut self.intruder.results.runs;
        page.loaded = true;
        page.dirty = false;
        page.pending = None;
        page.error = None;
        if page.after != after {
            page.rows.clear();
            page.next_after = None;
        }
        page.after = after;
        if self.enqueue(
            Command::QueryIntruderRuns {
                project,
                query_revision: revision,
                after,
                limit: PAGE_SIZE,
            },
            cx,
        ) {
            self.intruder.results.runs.pending = Some(revision);
        } else {
            self.intruder.results.runs.error = self.error.clone();
        }
        cx.notify();
    }

    pub(in crate::workbench::intruder) fn attach_intruder_result_window(
        &mut self,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.attach_intruder_result_view(run_id, run_id, window, cx);
    }

    pub(in crate::workbench) fn attach_intruder_result_view(
        &mut self,
        view_id: Uuid,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if self.intruder.results.views.contains_key(&view_id) {
            return;
        }
        let mut view = ResultView::new(project, view_id, run_id, window, cx);
        view.summary = self
            .intruder
            .runs
            .get(&run_id)
            .map(|run| &run.summary)
            .into_iter()
            .chain(
                self.intruder
                    .results
                    .runs
                    .rows
                    .iter()
                    .filter(|run| run.id == run_id),
            )
            .chain(
                self.intruder
                    .results
                    .views
                    .values()
                    .filter(|view| view.run_id == run_id)
                    .filter_map(|view| view.summary.as_ref()),
            )
            .cloned()
            .reduce(|mut latest, summary| {
                merge_run_summary(&mut latest, &summary);
                latest
            });
        if let Some(other) = self
            .intruder
            .results
            .views
            .values()
            .find(|view| view.run_id == run_id)
        {
            view.stop_requested = other.stop_requested;
            view.control_pending = other.control_pending;
            view.control_error.clone_from(&other.control_error);
            view.baseline_error.clone_from(&other.baseline_error);
        }
        if let Some(probe) = self.intruder.results.probes.remove(&run_id) {
            view.pending = probe.pending;
            view.error = probe.error;
        }
        if self.backend_disconnected {
            view.error = Some("后台服务已断开，结果读取未确认。".into());
        }
        // Reuse an admission probe already in flight, then read this view's first page.
        let query = view.pending.is_none() && view.error.is_none();
        self.intruder.results.views.insert(view_id, view);
        if query {
            self.query_intruder_result_view(
                view_id,
                0,
                IntruderResultFilter::default(),
                IntruderResultSort::default(),
                window,
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_intruder_run(
        &mut self,
        view_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let view = self
            .intruder
            .results
            .views
            .entry(view_id)
            .or_insert_with(|| ResultView::new(project, view_id, view_id, window, cx));
        let read_page = view.pending.is_none()
            && view.query.pending.is_none()
            && view.queued_query.is_none()
            && (view.summary.is_none() || view.error.is_some());
        let read_spec =
            view.spec_pending.is_none() && (view.spec.is_none() || view.spec_error.is_some());
        if read_page {
            let (filter, sort) = self.intruder.results.views[&view_id].query.requested();
            self.query_intruder_result_view(view_id, 0, filter, sort, window, cx);
        }
        if read_spec {
            self.read_intruder_result_spec(view_id, cx);
        }
    }

    pub(in crate::workbench) fn close_intruder_results(
        &mut self,
        view_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.defer_intruder_result_window(view_id, window, cx, move |view, window, cx| {
            view.close_intruder_results(view_id, window, cx);
        }) {
            return;
        }
        if let Some(view) = self.intruder.results.views.get_mut(&view_id) {
            view.clear_detail(window, cx);
            self.forget_intruder_results(view_id, cx);
            self.refresh_intruder_results(window, cx);
            cx.notify();
        }
        // Closing a view never implies that its network task has stopped.
    }

    pub(in crate::workbench) fn query_intruder_results(
        &mut self,
        view_id: Uuid,
        after: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none()
            || self.backend_disconnected
            || self
                .intruder
                .results
                .views
                .get(&view_id)
                .is_none_or(|view| view.discarded)
        {
            return;
        }
        let Some(revision) = self.next_intruder_query(cx) else {
            return;
        };
        let view = self.intruder.results.views.get_mut(&view_id).unwrap();
        view.queued_query = Some(revision);
        view.dirty = false;
        view.pending = None;
        view.error = None;
        if self.defer_intruder_result_window(view_id, window, cx, move |view, window, cx| {
            view.send_intruder_result_query(view_id, revision, after, window, cx);
        }) {
            return;
        }
        self.send_intruder_result_query(view_id, revision, after, window, cx);
    }

    pub(in crate::workbench) fn send_intruder_result_query(
        &mut self,
        view_id: Uuid,
        revision: u64,
        after: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(view) = self.intruder.results.views.get_mut(&view_id) else {
            return;
        };
        if view.queued_query != Some(revision) {
            return;
        }
        view.queued_query = None;
        view.dirty = false;
        if view.after != after {
            let selected_baseline = view.selected.filter(|ordinal| {
                view.summary.as_ref().is_some_and(|run| {
                    run.baseline
                        .is_some_and(|baseline| baseline.ordinal == *ordinal)
                })
            });
            if selected_baseline.is_none() || view.baseline_row.is_none() {
                view.clear_detail(window, cx);
                view.selected = selected_baseline;
            }
            view.rows.clear();
            view.next_after = None;
        }
        view.after = after;
        let run_id = view.run_id;
        if self.enqueue(
            Command::QueryIntruderResults {
                project,
                run_id,
                query_revision: revision,
                after,
                limit: 1,
            },
            cx,
        ) {
            self.intruder
                .results
                .views
                .get_mut(&view_id)
                .unwrap()
                .pending = Some(revision);
        } else {
            self.intruder.results.views.get_mut(&view_id).unwrap().error = self.error.clone();
            self.fail_intruder_close_read(
                view_id,
                self.error
                    .clone()
                    .unwrap_or_else(|| "终态查询未入队。".into()),
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::workbench) fn read_intruder_result_spec(
        &mut self,
        view_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if self
            .intruder
            .results
            .views
            .get(&view_id)
            .is_none_or(|view| view.discarded)
        {
            return;
        }
        let Some(revision) = self.next_intruder_query(cx) else {
            return;
        };
        let view = self.intruder.results.views.get_mut(&view_id).unwrap();
        view.spec_pending = None;
        view.spec_error = None;
        let run_id = view.run_id;
        if self.enqueue(
            Command::ReadIntruderSpec {
                project,
                run_id,
                selection_revision: revision,
            },
            cx,
        ) {
            self.intruder
                .results
                .views
                .get_mut(&view_id)
                .unwrap()
                .spec_pending = Some(revision);
        } else {
            self.intruder
                .results
                .views
                .get_mut(&view_id)
                .unwrap()
                .spec_error = self.error.clone();
        }
        cx.notify();
    }
}
