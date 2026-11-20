use mio_core::IntruderRetention;

use super::results_view::IntruderResultsWindow;
use super::*;

mod render;

#[derive(Default)]
pub(in crate::workbench) struct Dashboard {
    pub(super) selection: Option<Selection>,
    scroll: ScrollHandle,
    previous_pages: Vec<Option<Uuid>>,
    scroll_to_end: bool,
}

pub(super) struct Selection {
    run_id: Uuid,
    view_id: Uuid,
    results: Option<Entity<IntruderResultsWindow>>,
    error: Option<String>,
}

impl Workbench {
    pub(in crate::workbench) fn has_dashboard_intruder_selection(&self) -> bool {
        self.intruder.dashboard.selection.is_some()
    }

    pub(in crate::workbench) fn clear_dashboard_intruder_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = self.intruder.dashboard.selection.take() {
            self.close_intruder_results(previous.view_id, window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn activate_intruder_dashboard(&mut self, cx: &mut Context<Self>) {
        if self.project.is_some() && !self.intruder.results.runs.loaded {
            self.query_intruder_runs(None, cx);
        }
    }

    fn scroll_intruder_tasks(&mut self, delta: gpui_kit::Pixels, cx: &mut Context<Self>) {
        let page = &self.intruder.results.runs;
        if page.pending.is_some()
            || page.error.is_some()
            || self.backend_disconnected
            || self.transition.is_some()
        {
            return;
        }
        let scroll = &self.intruder.dashboard.scroll;
        let next = scroll.offset().y + delta;
        if delta < px(0.) && next <= -scroll.max_offset().y + px(12.) {
            if let Some(after) = page.next_after {
                self.intruder.dashboard.previous_pages.push(page.after);
                self.intruder
                    .dashboard
                    .scroll
                    .set_offset(gpui_kit::point(px(0.), px(0.)));
                self.query_intruder_runs(Some(after), cx);
                cx.stop_propagation();
            }
        } else if delta > px(0.) && next >= px(0.) && page.after.is_some() {
            let after = self.intruder.dashboard.previous_pages.pop().flatten();
            self.intruder.dashboard.scroll_to_end = true;
            self.query_intruder_runs(after, cx);
            cx.stop_propagation();
        }
    }

    pub(in crate::workbench) fn select_dashboard_intruder_run(
        &mut self,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if window.window_handle() != self.owner_window
            || self.transition.is_some()
            || !self
                .intruder
                .results
                .runs
                .rows
                .iter()
                .any(|run| run.id == run_id)
        {
            return;
        }
        if let Some(selection) = &self.intruder.dashboard.selection
            && selection.run_id == run_id
            && (self.intruder.results.views.contains_key(&selection.view_id)
                || (selection.results.is_none() && selection.error.is_none()))
        {
            return;
        }
        if let Some(previous) = self.intruder.dashboard.selection.take() {
            self.close_intruder_results(previous.view_id, window, cx);
        }
        let view_id = Uuid::new_v4();
        self.intruder.dashboard.selection = Some(Selection {
            run_id,
            view_id,
            results: None,
            error: None,
        });
        let weak = cx.weak_entity();
        let owner = self.owner_window;
        // Construct the child outside the Workbench update; its table reads the Workbench.
        cx.defer(move |cx| {
            let Some(workbench) = weak.upgrade() else {
                return;
            };
            let opened = owner.update(cx, |_, window, cx| {
                let view = workbench.read(cx);
                if view.project != Some(project)
                    || !view
                        .intruder
                        .dashboard
                        .selection
                        .as_ref()
                        .is_some_and(|selection| {
                            selection.view_id == view_id && selection.run_id == run_id
                        })
                {
                    return;
                }
                let results = cx.new(|cx| {
                    IntruderResultsWindow::embedded(&workbench, view_id, run_id, window, cx)
                });
                workbench.update(cx, |view, cx| {
                    if let Some(selection) = &mut view.intruder.dashboard.selection
                        && selection.view_id == view_id
                    {
                        selection.results = Some(results);
                    }
                    cx.notify();
                });
            });
            if opened.is_err() {
                workbench.update(cx, |view, cx| {
                    if view.project == Some(project)
                        && let Some(selection) = &mut view.intruder.dashboard.selection
                        && selection.view_id == view_id
                    {
                        selection.error = Some("主工作窗口已不可用，未打开结果视图。".into());
                        cx.notify();
                    }
                });
            }
        });
        cx.notify();
    }

    fn dashboard_spec(&self, run: &IntruderRunSummary) -> Option<&IntruderSpec> {
        self.intruder
            .results
            .views
            .values()
            .filter(|view| view.run_id == run.id)
            .find_map(|view| {
                view.spec
                    .as_deref()
                    .filter(|spec| spec.revision == run.spec_revision)
            })
            .or_else(|| {
                self.workspace
                    .intruder
                    .tabs
                    .iter()
                    .chain(&self.workspace.intruder.closed)
                    .find(|tab| tab.id == run.tab_id && tab.spec.revision == run.spec_revision)
                    .map(|tab| &tab.spec)
            })
    }

    pub(in crate::workbench) fn dashboard_title(&self, run: &IntruderRunSummary) -> String {
        if !run.title.is_empty() {
            return run.title.clone();
        }
        match self.dashboard_spec(run) {
            Some(spec) if !spec.draft.target.is_empty() => {
                format!("Intruder 攻击：{}", spec.draft.target)
            }
            _ => format!("Intruder 攻击 · {}", &run.id.to_string()[..8]),
        }
    }

    fn dashboard_retention(&self, run: &IntruderRunSummary) -> &'static str {
        match run.retention {
            IntruderRetention::Session => "Session · 会话内存",
            IntruderRetention::Project
                if self.intruder.results.views.values().any(|view| {
                    view.run_id == run.id
                        && view
                            .summary
                            .as_ref()
                            .is_some_and(|summary| summary.retention == run.retention)
                        && view.persisted == Some(false)
                }) =>
            {
                "Project · 待写盘"
            }
            IntruderRetention::Project => "Project",
        }
    }
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod tests;
