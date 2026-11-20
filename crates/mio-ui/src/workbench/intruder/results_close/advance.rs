use mio_core::IntruderRetention;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn start_intruder_close_deadline(&mut self, cx: &mut Context<Self>) {
        let Some(close) = self.intruder_result_close.as_ref() else {
            return;
        };
        let operation_id = close.operation_id;
        let executor = cx.background_executor().clone();
        let deadline = executor.now() + CLOSE_DEADLINE;
        let timer = executor.timer(CLOSE_DEADLINE);
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| {
                if view
                    .intruder_result_close
                    .as_ref()
                    .is_some_and(|close| close.operation_id == operation_id && !close.expired)
                {
                    view.intruder_close_timed_out(cx);
                }
            });
        });
        let close = self.intruder_result_close.as_mut().unwrap();
        close.deadline = Some((deadline, executor));
        close._deadline = Some(task);
    }

    pub(in crate::workbench) fn expire_intruder_close_if_due(&mut self, cx: &mut Context<Self>) {
        if self
            .intruder_result_close
            .as_ref()
            .is_some_and(|close| !close.expired && close.deadline_elapsed())
        {
            self.intruder_close_timed_out(cx);
        }
    }

    pub(in crate::workbench) fn intruder_close_timed_out(&mut self, cx: &mut Context<Self>) {
        let submitted = self.intruder_result_close.as_ref().is_some_and(|close| {
            matches!(
                close.phase,
                ClosePhase::Discarding { .. }
                    | ClosePhase::Retaining { .. }
                    | ClosePhase::SavingProject { path: Some(_) }
                    | ClosePhase::Continuing
            )
        });
        if submitted {
            self.discard_pending_sends(cx);
            self.intruder_save_failed(cx);
            self.plugins_save_failed(cx);
            let close = self.intruder_result_close.as_mut().unwrap();
            close.expired = true;
            if let Some(view) = close
                .target
                .view_id()
                .and_then(|id| self.intruder.results.views.get_mut(&id))
            {
                view.close_status = None;
                view.close_error = Some("等待超过 30 秒，操作结果未确认。当前编辑和回执归属保留；收到确认后可再次关闭，未自动重发。".into());
            } else {
                self.status = "任务操作等待超过 30 秒；实际结果未确认，未自动重发。".into();
                self.error = Some("任务操作回执归属仍保留；收到确认后可重新操作。".into());
            }
            cx.notify();
        } else {
            self.fail_intruder_result_close(
                "关闭等待超时（30 秒）；结果和编辑仍保留。请再次关闭以重新确认，未自动丢弃。",
                cx,
            );
        }
    }

    pub(in crate::workbench) fn set_intruder_close_status(&mut self, status: &'static str) {
        if let Some(close) = &self.intruder_result_close
            && close.target.owns(self)
            && !close.expired
        {
            if let Some(view) = close
                .target
                .view_id()
                .and_then(|id| self.intruder.results.views.get_mut(&id))
            {
                view.close_status = Some(status);
                view.close_error = None;
            } else {
                self.status = status.into();
                self.error = None;
            }
        }
    }

    pub(in crate::workbench) fn finish_intruder_result_close(&mut self, cx: &mut Context<Self>) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = self.intruder_result_close.as_mut() else {
            return;
        };
        if close.expired || !close.close_view {
            let close = self.intruder_result_close.take().unwrap();
            if let Some(view) = close
                .target
                .view_id()
                .and_then(|id| self.intruder.results.views.get_mut(&id))
            {
                view.close_status = None;
                view.close_error = close
                    .expired
                    .then(|| "操作已确认；先前关闭等待已结束，当前窗口保留，请再次关闭。".into());
                view.refresh_display_if_changed();
            } else if close.expired {
                self.error = Some("任务操作已确认；先前等待已结束，当前工作窗口保留。".into());
            }
            cx.notify();
            return;
        }
        close.phase = ClosePhase::Continuing;
        let CloseTarget::Result(target) = close.target.clone() else {
            return;
        };
        let operation_id = close.operation_id;
        cx.defer(move |cx| {
            let result = target.owner.update(cx, |_, window, cx| {
                let _ = target.workbench.update(cx, |view, cx| {
                    view.expire_intruder_close_if_due(cx);
                    if view.intruder_result_close.as_ref().is_some_and(|close| {
                        close.operation_id == operation_id
                            && matches!(close.phase, ClosePhase::Continuing)
                    }) {
                        if view.intruder_result_close.as_ref().unwrap().expired {
                            view.finish_intruder_result_close(cx);
                            return;
                        }
                        if !target.owns(view) || window.window_handle() != target.owner {
                            view.fail_intruder_result_close(
                                "结果窗口或项目已改变；未应用旧的关闭选择。",
                                cx,
                            );
                            return;
                        }
                        let saved_detach = view
                            .intruder_result_close
                            .as_ref()
                            .is_some_and(ResultClose::saved_detach);
                        if saved_detach && !view.saved_intruder_result_confirmed(target.view_id) {
                            view.intruder_result_close = None;
                            let result = view
                                .intruder
                                .results
                                .views
                                .get_mut(&target.view_id)
                                .unwrap();
                            result.close_status = None;
                            result.close_error = Some(
                                "结果保存状态已改变；窗口和待保存编辑保留，请核对存储错误后重试。"
                                    .into(),
                            );
                            cx.notify();
                            return;
                        }
                        view.intruder_result_close = None;
                        view.close_intruder_results(target.view_id, window, cx);
                        if target.owner != view.owner_window {
                            window.remove_window();
                        }
                    }
                });
            });
            if result.is_err() {
                let _ = target.workbench.update(cx, |view, cx| {
                    if view
                        .intruder_result_close
                        .as_ref()
                        .is_some_and(|close| close.operation_id == operation_id)
                    {
                        view.fail_intruder_result_close(
                            "结果窗口已不可用；后台数据仍由项目管理。",
                            cx,
                        );
                    }
                });
            }
        });
    }

    pub(in crate::workbench) fn advance_intruder_result_close(&mut self, cx: &mut Context<Self>) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = &self.intruder_result_close else {
            return;
        };
        if !matches!(close.phase, ClosePhase::Waiting) || close.expired {
            return;
        }
        if self.intruder_baseline_pending(close.target.run_id()) {
            return;
        }
        if !close.target.owns(self) {
            self.fail_intruder_result_close("结果窗口或项目已改变；本次操作已取消。", cx);
            return;
        }
        if !self.backend_available() || self.commands.is_closed() {
            self.fail_intruder_result_close("后台服务已断开；操作未确认，数据和窗口仍保留。", cx);
            return;
        }
        if self.transition.is_some() || self.quit_attempt.is_some() {
            self.fail_intruder_result_close("项目正在切换或退出；本次关闭已取消。", cx);
            return;
        }
        let Some(run) = self.intruder_close_summary(&close.target) else {
            return;
        };
        if !results::same_run(&close.run, &run) || run.revision < close.run.revision {
            self.fail_intruder_result_close(
                "Intruder 运行身份或 revision 已改变；结果仍保留。",
                cx,
            );
            return;
        }
        if !run.state.is_terminal()
            && !(close.explicit_save() && self.can_save_active_intruder_run(&run))
        {
            return;
        }
        if self.pending_workspace.is_some()
            || self.pending_project_save.is_some()
            || self.project_saves_in_flight > 0
            || self.choosing_save_as
        {
            self.set_intruder_close_status("等待当前工作区保存…");
            cx.notify();
            return;
        }
        let save = close.choice == Some(CloseChoice::Save);
        let same_retention = match close.choice {
            Some(CloseChoice::Keep) => run.retention == IntruderRetention::Session,
            Some(CloseChoice::Save) => run.retention == IntruderRetention::Project,
            _ => false,
        };
        if same_retention && (!save || close.saved_to_file) {
            self.finish_intruder_result_close(cx);
            return;
        }
        let project_save = save && (self.project_path.is_none() || same_retention);
        let operation_id = close.operation_id;
        self.intruder_result_close.as_mut().unwrap().phase = if project_save {
            ClosePhase::PreparingSave
        } else {
            ClosePhase::Preparing
        };
        let owner = self.owner_window;
        let weak = cx.weak_entity();
        cx.defer(move |cx| {
            let result = owner.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |view, cx| {
                    if project_save {
                        view.prepare_intruder_project_save(operation_id, window, cx);
                    } else {
                        view.prepare_intruder_mutation(operation_id, window, cx);
                    }
                });
            });
            if result.is_err() {
                let _ = weak.update(cx, |view, cx| {
                    if view
                        .intruder_result_close
                        .as_ref()
                        .is_some_and(|close| close.operation_id == operation_id)
                    {
                        view.fail_intruder_result_close(
                            "主工作窗口已不可用；无法采集最新草稿，结果仍保留。",
                            cx,
                        );
                    }
                });
            }
        });
    }
}
