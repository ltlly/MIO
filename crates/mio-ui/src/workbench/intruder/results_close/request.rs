use mio_core::{FinishedAttackClose, IntruderRetention, RunningAttackClose};

use super::results_view::ResultTarget;
use super::*;

impl Workbench {
    pub(in crate::workbench) fn request_intruder_result_close(
        &mut self,
        target: ResultTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.intruder_result_close_pending(target.run_id) {
            return;
        }
        if !self.result_close_available(&CloseTarget::Result(target.clone()), cx) {
            return;
        }
        if self
            .intruder
            .results
            .views
            .get(&target.view_id)
            .is_some_and(|view| view.discarded)
        {
            self.close_intruder_results(target.view_id, window, cx);
            if target.owner != self.owner_window {
                window.remove_window();
            }
            return;
        }
        let Some(run) = self.intruder_result_summary(target.view_id).cloned() else {
            self.intruder
                .results
                .views
                .get_mut(&target.view_id)
                .unwrap()
                .close_error =
                Some("运行状态尚未读取；结果窗口仍保留，请重新读取后再关闭。".into());
            cx.notify();
            return;
        };
        let terminal = run.state.is_terminal();
        let can_save = self.project_path.is_some();
        if can_save && run.retention == IntruderRetention::Project {
            if !self.saved_intruder_result_confirmed(target.view_id) {
                self.intruder
                    .results
                    .views
                    .get_mut(&target.view_id)
                    .unwrap()
                    .close_error = Some(
                    "项目结果尚未确认保存；窗口保留，请先读取结果或通过项目保存核对恢复。".into(),
                );
                cx.notify();
                return;
            }
            self.intruder_result_close = Some(ResultClose {
                target: CloseTarget::Result(target),
                operation_id: Uuid::new_v4(),
                run,
                choice: None,
                phase: ClosePhase::Waiting,
                expired: false,
                close_view: true,
                saved_to_file: true,
                dialog: None,
                deadline: None,
                _deadline: None,
            });
            self.finish_intruder_result_close(cx);
            return;
        }
        let (running_policy, finished_policy) = self.intruder_close_preferences();
        let automatic_answer = if terminal {
            match finished_policy {
                FinishedAttackClose::Ask => None,
                FinishedAttackClose::SaveProject => Some(if can_save { 0 } else { 1 }),
                FinishedAttackClose::KeepMemory => Some(1),
                FinishedAttackClose::Delete => Some(2),
            }
        } else {
            match running_policy {
                RunningAttackClose::Ask => None,
                RunningAttackClose::Continue => Some(1),
                RunningAttackClose::Delete => Some(0),
            }
        };
        let operation_id = Uuid::new_v4();
        self.intruder_result_close = Some(ResultClose {
            target: CloseTarget::Result(target),
            operation_id,
            run,
            choice: None,
            phase: ClosePhase::Confirming { cancelled: false },
            expired: false,
            close_view: true,
            saved_to_file: false,
            dialog: None,
            deadline: None,
            _deadline: None,
        });
        self.cancel_invalidated_result_transfer("正在关闭或丢弃攻击结果。", cx);
        if let Some(answer) = automatic_answer {
            self.answer_intruder_result_close(operation_id, Some(answer), cx);
            return;
        }
        self.set_intruder_close_status("等待关闭选择…");
        let parent = window.window_handle();
        let owner = cx.weak_entity();
        cx.defer(move |cx| {
            let Some(entity) = owner.upgrade() else {
                return;
            };
            let current = entity
                .read(cx)
                .intruder_result_close
                .as_ref()
                .is_some_and(|close| {
                    close.operation_id == operation_id
                        && close.target.owns(entity.read(cx))
                        && matches!(close.phase, ClosePhase::Confirming { cancelled: false })
                });
            if !current {
                entity.update(cx, |view, cx| {
                    view.answer_intruder_result_close(operation_id, None, cx)
                });
                return;
            }
            let opened = parent
                .update(cx, |_, window, cx| {
                    super::results_close_dialog::open(
                        owner.clone(),
                        operation_id,
                        terminal,
                        can_save,
                        window,
                        cx,
                        |view, operation, answer, cx| {
                            view.answer_intruder_close_dialog(operation, answer, cx)
                        },
                    )
                })
                .and_then(|result| result);
            match opened {
                Ok((dialog, _)) => {
                    let retained = entity.update(cx, |view, _| {
                        if let Some(close) = view.intruder_result_close.as_mut()
                            && close.operation_id == operation_id
                            && matches!(close.phase, ClosePhase::Confirming { cancelled: false })
                        {
                            close.dialog = Some(dialog);
                            true
                        } else {
                            false
                        }
                    });
                    if !retained {
                        let _ = dialog.update(cx, |_, window, _| window.remove_window());
                    }
                }
                Err(error) => entity.update(cx, |view, cx| {
                    view.answer_intruder_result_close(operation_id, None, cx);
                    view.fail(format!("无法打开关闭确认：{error}"), cx);
                }),
            }
        });
        cx.notify();
    }

    pub(in crate::workbench) fn answer_intruder_close_dialog(
        &mut self,
        operation_id: Uuid,
        answer: Option<(usize, bool)>,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let Some(close) = self.intruder_result_close.as_ref() else {
            return Ok(true);
        };
        if close.operation_id != operation_id {
            return Ok(true);
        }
        if !cx.windows().contains(&self.owner_window) {
            self.reject_intruder_result_close("主工作窗口已关闭；关闭选择未执行。", cx);
            return Ok(true);
        }
        if !matches!(close.phase, ClosePhase::Confirming { cancelled: false })
            || !close.target.owns(self)
        {
            self.answer_intruder_result_close(operation_id, None, cx);
            return Ok(true);
        }
        if let Some((index, true)) = answer {
            use crate::workbench::user_preferences::RememberCloseChoice;
            let preference = match (close.run.state.is_terminal(), index) {
                (false, 0) => RememberCloseChoice::Running(RunningAttackClose::Delete),
                (false, 1) => RememberCloseChoice::Running(RunningAttackClose::Continue),
                (true, 0) if self.project_path.is_some() => {
                    RememberCloseChoice::Finished(FinishedAttackClose::SaveProject)
                }
                (true, 1) => RememberCloseChoice::Finished(FinishedAttackClose::KeepMemory),
                (true, 2) => RememberCloseChoice::Finished(FinishedAttackClose::Delete),
                _ => return Err("关闭选项已失效；运行和结果仍保留。".into()),
            };
            self.remember_intruder_closing_preference(preference, cx)?;
        }
        self.answer_intruder_result_close(operation_id, answer.map(|(index, _)| index), cx);
        Ok(true)
    }

    pub(in crate::workbench) fn request_intruder_retention(
        &mut self,
        target: CloseTarget,
        save: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.result_close_available(&target, cx) {
            return;
        }
        let Some(run) = self.intruder_close_summary(&target) else {
            self.fail("任务状态尚未确认；保存方式未改变。", cx);
            return;
        };
        if !run.state.is_terminal()
            && !(save
                && run.retention == IntruderRetention::Session
                && self.can_save_active_intruder_run(&run))
        {
            self.fail("攻击结束后可更改结果保存方式；当前结果仍保留。", cx);
            return;
        }
        self.intruder_result_close = Some(ResultClose {
            target,
            operation_id: Uuid::new_v4(),
            run,
            choice: Some(if save {
                CloseChoice::Save
            } else {
                CloseChoice::Keep
            }),
            phase: ClosePhase::Waiting,
            expired: false,
            close_view: false,
            saved_to_file: false,
            dialog: None,
            deadline: None,
            _deadline: None,
        });
        self.cancel_invalidated_result_transfer("正在关闭或丢弃攻击结果。", cx);
        self.start_intruder_close_deadline(cx);
        self.advance_intruder_result_close(cx);
    }

    pub(in crate::workbench) fn answer_intruder_result_close(
        &mut self,
        operation_id: Uuid,
        answer: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(close) = self.intruder_result_close.as_ref() else {
            return;
        };
        let ClosePhase::Confirming { cancelled } = &close.phase else {
            return;
        };
        if close.operation_id != operation_id {
            return;
        }
        if *cancelled || !close.target.owns(self) {
            self.intruder_result_close = None;
            cx.notify();
            return;
        }
        let task = matches!(close.target, CloseTarget::Task { .. });
        if task && answer != Some(1) {
            self.intruder_result_close = None;
            cx.notify();
            return;
        }
        let terminal = close.run.state.is_terminal();
        let choice = if task {
            Some(CloseChoice::Discard)
        } else {
            match (terminal, answer) {
                (false, Some(1)) => {
                    self.finish_intruder_result_close(cx);
                    return;
                }
                (false, Some(0)) | (true, Some(2)) => Some(CloseChoice::Discard),
                (true, Some(0)) => Some(CloseChoice::Save),
                (true, Some(1)) => Some(CloseChoice::Keep),
                _ => None,
            }
        };
        let Some(choice) = choice else {
            let close = self.intruder_result_close.take().unwrap();
            if let Some(view) = close
                .target
                .view_id()
                .and_then(|id| self.intruder.results.views.get_mut(&id))
            {
                view.close_status = None;
                view.close_error = Some("关闭确认未完成；运行和结果仍保留。".into());
            }
            cx.notify();
            return;
        };
        let close = self.intruder_result_close.as_mut().unwrap();
        close.choice = Some(choice);
        close.phase = ClosePhase::Waiting;
        let run_id = close.target.run_id();
        self.start_intruder_close_deadline(cx);
        if choice == CloseChoice::Discard {
            self.set_intruder_close_status("等待终态确认后丢弃…");
            if !terminal || self.intruder_baseline_pending(run_id) {
                self.stop_intruder_run(run_id, cx);
            }
        }
        self.advance_intruder_result_close(cx);
        cx.notify();
    }
}
