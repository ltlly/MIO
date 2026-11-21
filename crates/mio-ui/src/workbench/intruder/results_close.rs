use std::time::{Duration, Instant};

use mio_core::{IntruderRetention, IntruderRunState};

use super::results_view::ResultTarget;
use super::*;

mod advance;
mod prepare;
mod receipts;
use receipts::clear_last_run;
mod request;

const CLOSE_DEADLINE: Duration = Duration::from_secs(30);

pub(in crate::workbench) struct ResultClose {
    target: CloseTarget,
    operation_id: Uuid,
    run: IntruderRunSummary,
    choice: Option<CloseChoice>,
    phase: ClosePhase,
    expired: bool,
    close_view: bool,
    saved_to_file: bool,
    dialog: Option<gpui_kit::AnyWindowHandle>,
    deadline: Option<(Instant, gpui_kit::BackgroundExecutor)>,
    _deadline: Option<Task<()>>,
}

#[derive(Clone)]
pub(in crate::workbench) enum CloseTarget {
    Result(ResultTarget),
    Task {
        project: ProjectKey,
        run_id: Uuid,
        owner: gpui_kit::AnyWindowHandle,
    },
}

impl CloseTarget {
    fn project(&self) -> ProjectKey {
        match self {
            Self::Result(target) => target.project,
            Self::Task { project, .. } => *project,
        }
    }

    fn run_id(&self) -> Uuid {
        match self {
            Self::Result(target) => target.run_id,
            Self::Task { run_id, .. } => *run_id,
        }
    }

    fn view_id(&self) -> Option<Uuid> {
        match self {
            Self::Result(target) => Some(target.view_id),
            Self::Task { .. } => None,
        }
    }

    fn owns(&self, workbench: &Workbench) -> bool {
        match self {
            Self::Result(target) => target.owns(workbench),
            Self::Task { project, owner, .. } => {
                workbench.project == Some(*project) && workbench.owner_window == *owner
            }
        }
    }
}

impl ResultClose {
    fn explicit_save(&self) -> bool {
        self.choice == Some(CloseChoice::Save) && !self.close_view
    }

    fn saved_detach(&self) -> bool {
        self.choice.is_none() && self.saved_to_file && self.close_view
    }

    fn deadline_elapsed(&self) -> bool {
        self.deadline
            .as_ref()
            .is_some_and(|(deadline, executor)| executor.now() >= *deadline)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseChoice {
    Discard,
    Keep,
    Save,
}

enum ClosePhase {
    Confirming {
        cancelled: bool,
    },
    Continuing,
    Waiting,
    Preparing,
    PreparingSave,
    SavingProject {
        path: Option<PathBuf>,
    },
    Discarding {
        expected_revision: u64,
        workspace: Box<Workspace>,
    },
    Retaining {
        retention: IntruderRetention,
        workspace: Box<Workspace>,
    },
}

impl ResultTarget {
    pub(in crate::workbench) fn request_close(&self, window: &mut Window, cx: &mut App) {
        self.update(window, cx, |workbench, window, cx| {
            workbench.request_intruder_result_close(self.clone(), window, cx);
        });
    }

    pub(in crate::workbench) fn set_project_retention(
        &self,
        save: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.update(window, cx, |workbench, _, cx| {
            workbench.request_intruder_retention(CloseTarget::Result(self.clone()), save, cx);
        });
    }
}

impl Workbench {
    pub(in crate::workbench) fn can_save_active_intruder_run(
        &self,
        run: &IntruderRunSummary,
    ) -> bool {
        self.project_path.is_some()
            && self.backend_available()
            && self.pending_records == 0
            && self.storage_error.is_none()
            && matches!(
                run.state,
                IntruderRunState::Running | IntruderRunState::Paused
            )
            && self.intruder.runs.get(&run.id).is_some_and(|active| {
                results::same_run(&active.summary, run)
                    && active.summary.retention == run.retention
                    && matches!(
                        active.summary.state,
                        IntruderRunState::Running | IntruderRunState::Paused
                    )
                    && !active.cancel.is_cancelled()
            })
            && self
                .intruder
                .results
                .views
                .values()
                .filter(|view| view.run_id == run.id)
                .all(|view| {
                    !view.discarded
                        && !view.stop_requested
                        && view.control_pending.is_none()
                        && view.error.is_none()
                        && view.persisted == Some(run.retention == IntruderRetention::Project)
                })
    }

    pub(in crate::workbench) fn intruder_close_summary(
        &self,
        target: &CloseTarget,
    ) -> Option<IntruderRunSummary> {
        let run_id = target.run_id();
        let summaries = self
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
                    .filter(|view| view.run_id == run_id && !view.discarded)
                    .filter_map(|view| view.summary.as_ref()),
            )
            .chain(
                self.intruder_result_close
                    .as_ref()
                    .filter(|close| {
                        matches!(target, CloseTarget::Task { .. })
                            && close.target.project() == target.project()
                            && close.target.run_id() == run_id
                    })
                    .map(|close| &close.run),
            );
        let mut latest = None;
        for summary in summaries {
            if let Some(current) = &mut latest {
                if !results::same_run(current, summary) {
                    return None;
                }
                results::merge_run_summary(current, summary);
            } else {
                latest = Some(summary.clone());
            }
        }
        latest
    }

    pub(in crate::workbench) fn intruder_close_summary_changed(
        &mut self,
        summary: &IntruderRunSummary,
    ) {
        if let Some(close) = &mut self.intruder_result_close
            && matches!(close.target, CloseTarget::Task { .. })
            && self.project == Some(close.target.project())
            && matches!(
                close.phase,
                ClosePhase::Confirming { .. }
                    | ClosePhase::Waiting
                    | ClosePhase::Preparing
                    | ClosePhase::PreparingSave
            )
            && results::same_run(&close.run, summary)
        {
            results::merge_run_summary(&mut close.run, summary);
        }
    }

    pub(in crate::workbench) fn request_dashboard_intruder_delete(
        &mut self,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let target = CloseTarget::Task {
            project,
            run_id,
            owner: self.owner_window,
        };
        if window.window_handle() != self.owner_window || !self.result_close_available(&target, cx)
        {
            return;
        }
        let Some(run) = self.intruder_close_summary(&target) else {
            self.fail("任务状态尚未确认；未删除，请刷新 Dashboard 后重试。", cx);
            return;
        };
        let operation_id = Uuid::new_v4();
        let label = if run.title.is_empty() {
            format!("Intruder 攻击 {}", &run.id.to_string()[..8])
        } else {
            run.title.clone()
        };
        self.intruder_result_close = Some(Box::new(ResultClose {
            target,
            operation_id,
            run,
            choice: None,
            phase: ClosePhase::Confirming { cancelled: false },
            expired: false,
            close_view: false,
            saved_to_file: false,
            dialog: None,
            deadline: None,
            _deadline: None,
        }));
        self.cancel_invalidated_result_transfer("正在关闭或丢弃攻击结果。", cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            "删除任务",
            Some(&format!("确定要删除任务“{label}”吗？")),
            &["否", "是"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let answer = answer.await.ok();
            let _ = this.update(cx, |view, cx| {
                view.answer_intruder_result_close(operation_id, answer, cx)
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn save_dashboard_intruder_run(
        &mut self,
        run_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let target = CloseTarget::Task {
            project,
            run_id,
            owner: self.owner_window,
        };
        if window.window_handle() != self.owner_window {
            return;
        }
        self.request_intruder_retention(target, true, cx);
    }

    pub(in crate::workbench) fn intruder_result_summary(
        &self,
        view_id: Uuid,
    ) -> Option<&IntruderRunSummary> {
        let view = self.intruder.results.views.get(&view_id)?;
        [
            self.intruder.runs.get(&view.run_id).map(|run| &run.summary),
            view.summary.as_ref(),
        ]
        .into_iter()
        .flatten()
        .max_by_key(|run| run.revision)
    }

    pub(in crate::workbench) fn intruder_result_close_pending(&self, run_id: Uuid) -> bool {
        self.intruder_result_close.as_ref().is_some_and(|close| {
            close.target.run_id() == run_id && self.project == Some(close.target.project())
        })
    }

    pub(in crate::workbench) fn intruder_close_blocks_transition(&self) -> bool {
        self.intruder_result_close.as_ref().is_some_and(|close| {
            matches!(close.phase, ClosePhase::Confirming { .. }) || self.backend_available()
        })
    }

    pub(in crate::workbench) fn result_close_available(
        &mut self,
        target: &CloseTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.intruder_result_close.is_some()
            || self.intruder_pool_change_pending(target.run_id())
            || self.transition.is_some()
            || self.quit_attempt.is_some()
            || self.file_picker_pending()
            || self.shutdown_requested
        {
            if let Some(view) = target
                .view_id()
                .and_then(|id| self.intruder.results.views.get_mut(&id))
            {
                view.close_error = Some("当前操作尚未结束；结果仍保留，请稍后再试。".into());
            } else {
                self.fail("当前操作尚未结束；任务仍保留，请稍后再试。", cx);
            }
            cx.notify();
            return false;
        }
        true
    }

    pub(in crate::workbench) fn saved_intruder_result_confirmed(&self, view_id: Uuid) -> bool {
        self.intruder
            .results
            .views
            .get(&view_id)
            .is_some_and(|view| {
                view.persisted == Some(true)
                    && view
                        .summary
                        .as_ref()
                        .is_some_and(|run| run.retention == IntruderRetention::Project)
            })
            && self.project_path.is_some()
            && self.backend_available()
            && self.pending_records == 0
            && self.storage_error.is_none()
    }
}
