use gpui_kit::AnyWindowHandle;
use mio_core::Uuid;

use crate::native_terminate::{NativeRequest, NativeTerminate};

use super::*;

impl Workbench {
    pub(crate) fn observe_native(view: &Entity<Self>, owner: AnyWindowHandle, cx: &mut App) {
        let view = view.clone();
        let mut requests = cx.global::<NativeTerminate>().subscribe();
        cx.spawn(async move |cx| {
            while requests.changed().await.is_ok() {
                let native = cx.update(|cx| cx.global::<NativeTerminate>().clone());
                let Some(request) = *requests.borrow_and_update() else {
                    continue;
                };
                if let NativeRequest::Requested(id) = request
                    && !native.is_pending(id)
                {
                    continue;
                }
                let updated = owner.update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| match request {
                        NativeRequest::Requested(_) => {
                            if view.quit_attempt.is_some() {
                                return;
                            }
                            if view.transition.is_some() || view.file_picker_pending() {
                                view.reject_quit(cx);
                                view.fail("当前操作尚未结束，本轮原生退出已拒绝。", cx);
                                return;
                            }
                            view.request_transition(AfterSave::Quit, window, cx);
                        }
                        NativeRequest::TimedOut(_) => view.native_quit_timed_out(cx),
                    });
                });
                if updated.is_err() {
                    cx.update(|cx| {
                        view.update(cx, |view, cx| {
                            view.reject_quit(cx);
                            view.fail("主工作窗口已不可用，本轮原生退出已拒绝；编辑仍保留。", cx);
                        })
                    });
                }
                if let NativeRequest::TimedOut(id) = request {
                    native.acknowledge_timeout(id);
                }
            }
        })
        .detach();
    }

    pub(crate) fn observe_runtime(
        view: &Entity<Self>,
        owner: AnyWindowHandle,
        joined: Task<anyhow::Result<()>>,
        cx: &mut App,
    ) {
        let view = view.clone();
        // The app-scoped observer retains the editor and join if the window disappears.
        cx.spawn(async move |cx| {
            let result = joined.await.map_err(|error| format!("{error:#}"));
            let retained_result = result.clone();
            let updated = owner.update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.runtime_joined(result, window, cx));
            });
            if updated.is_err() {
                cx.update(|cx| {
                    view.update(cx, |view, cx| {
                        view.runtime_result = Some(retained_result);
                        view.http_notes_batch_runtime_joined(cx);
                        view.reset_history_transfer();
                        view.backend_disconnected = true;
                        view.shutdown_requested = false;
                        view.shutdown.cancel();
                        view.reject_quit(cx);
                        view.fail(
                            "主工作窗口已不可用；后台回收结果与编辑已保留，退出未获确认。",
                            cx,
                        );
                    })
                });
            }
        })
        .detach();
    }

    pub(in crate::workbench) fn quit_answer_is_current(
        &self,
        attempt: Option<Uuid>,
        cx: &App,
    ) -> bool {
        attempt.is_none()
            || (self.quit_attempt == attempt && !cx.global::<NativeTerminate>().rejected())
    }

    pub(in crate::workbench) fn reject_quit(&mut self, cx: &App) {
        self.quit_attempt = None;
        self.quitting = false;
        cx.global::<NativeTerminate>().reject();
    }

    pub(in crate::workbench) fn cancel_expired_quit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.quit_attempt.is_some() && cx.global::<NativeTerminate>().rejected() {
            self.native_quit_timed_out(cx);
            true
        } else {
            false
        }
    }

    fn native_quit_timed_out(&mut self, cx: &mut Context<Self>) {
        let had_attempt = self.quit_attempt.is_some();
        let was_quitting = self.quitting;
        self.reject_quit(cx);
        if had_attempt {
            self.after_save = None;
            self.pending_project_save = None;
            if !matches!(
                self.transition,
                Some(TransitionPhase::Confirming | TransitionPhase::ShuttingDown)
            ) {
                self.cancel_transition(cx);
            }
            if was_quitting && self.runtime_result.is_some() {
                self.shutdown_requested = false;
                self.transition = None;
            }
        }
        self.fail("原生退出等待超时，本轮已拒绝。当前编辑保留；已开始的后台回收将继续，退出需要再次确认。", cx);
    }

    pub(in crate::workbench) fn begin_shutdown(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_expired_quit(cx) {
            return;
        }
        cx.global::<NativeTerminate>()
            .extend_deadline_for_user_interaction();
        self.shutdown_requested = true;
        self.after_save = None;
        self.pending_project_save = None;
        self.pending_sends.clear();
        self.transition = Some(TransitionPhase::ShuttingDown);
        self.shutdown.cancel();
        self.focus.focus(window, cx);
        self.status = "正在回收后台任务与连接；磁盘操作结束后将关闭窗口。".into();
        cx.notify();
    }

    fn runtime_joined(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let error = result.as_ref().err().cloned();
        self.runtime_result = Some(result);
        self.http_notes_batch_runtime_joined(cx);
        self.reset_history_transfer();
        self.shutdown.cancel();
        self.backend_disconnected = true;
        let requested = self.shutdown_requested
            && self.quit_attempt.is_some()
            && self.quit_answer_is_current(self.quit_attempt, cx);
        if requested && error.is_none() && self.finish_joined_exit(cx) {
            return;
        }
        self.shutdown_requested = false;
        if self.transition != Some(TransitionPhase::Confirming) {
            self.transition = None;
        }
        self.after_save = None;
        self.pending_project_save = None;
        self.history_loading = false;
        self.history_dirty = false;
        self.ext_backend_disconnected(window, cx);
        self.intruder_backend_disconnected(cx);
        self.intruder_task_name_disconnected(cx);
        self.user_preferences_disconnected(cx);
        self.startup_wizard_disconnected(cx);
        self.websocket.backend_disconnected(cx);
        self.status = "后台服务已退出；当前窗口中的编辑仍保留。".into();
        self.fail(
            error.map_or_else(
                || "后台服务已退出；保存结果请核对项目，继续工作需要重新启动。".into(),
                |error| format!("后台退出时发生错误，保存结果未确认：{error}"),
            ),
            cx,
        );
        if requested {
            self.confirm_joined_exit(window, cx);
        } else {
            self.reject_quit(cx);
        }
    }

    pub(in crate::workbench) fn finish_joined_exit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.runtime_result.is_none()
            || self.quit_attempt.is_none()
            || !self.quit_answer_is_current(self.quit_attempt, cx)
        {
            return false;
        }
        if cx.global::<NativeTerminate>().permit_exit().is_err() {
            return false;
        }
        self.quitting = true;
        #[cfg(all(not(target_os = "macos"), not(test)))]
        cx.quit();
        true
    }

    pub(in crate::workbench) fn confirm_joined_exit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cancel_expired_quit(cx) {
            return;
        }
        cx.global::<NativeTerminate>()
            .extend_deadline_for_user_interaction();
        let (Some(result), Some(attempt)) = (&self.runtime_result, self.quit_attempt) else {
            return;
        };
        let detail = result.as_ref().err().map_or_else(
            || "后台线程已回收，当前窗口中尚未保存的编辑将丢失。".to_owned(),
            |error| format!("后台线程已回收，但操作未全部成功：{error}。当前编辑仍保留在窗口中。"),
        );
        self.transition = Some(TransitionPhase::ShuttingDown);
        let answer = window.prompt(
            PromptLevel::Warning,
            "退出 MIO？",
            Some(&detail),
            &["退出", "保留窗口"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let exit = matches!(answer.await, Ok(0));
            let _ = this.update_in(cx, |view, _, cx| {
                view.transition = None;
                if view.quit_answer_is_current(Some(attempt), cx) {
                    if exit {
                        view.finish_joined_exit(cx);
                    } else {
                        view.reject_quit(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
