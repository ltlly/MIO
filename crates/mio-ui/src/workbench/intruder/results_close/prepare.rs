use mio_core::IntruderRetention;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn capture_result_close_drafts(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        // Late input notifications can belong to an inactive main-window editor.
        for editors in self.tab_editors.values_mut() {
            editors.dirty = true;
        }
        self.capture_drafts(window, cx)
    }

    pub(in crate::workbench) fn prepare_intruder_mutation(
        &mut self,
        operation_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = &self.intruder_result_close else {
            return;
        };
        if close.operation_id != operation_id || !matches!(close.phase, ClosePhase::Preparing) {
            return;
        }
        if window.window_handle() != self.owner_window || !close.target.owns(self) {
            self.fail_intruder_result_close("工作窗口或项目已改变；未采集草稿，结果仍保留。", cx);
            return;
        }
        self.intruder_result_close.as_mut().unwrap().phase = ClosePhase::Waiting;
        if self.pending_workspace.is_some()
            || self.pending_project_save.is_some()
            || self.project_saves_in_flight > 0
        {
            return;
        }
        if !self.backend_available() || self.transition.is_some() || self.quit_attempt.is_some() {
            self.fail_intruder_result_close("后台或项目状态已改变；操作未确认，数据仍保留。", cx);
            return;
        }
        let close = self.intruder_result_close.as_ref().unwrap();
        let target = close.target.clone();
        if self.intruder_baseline_pending(target.run_id()) {
            return;
        }
        let choice = close.choice.unwrap();
        let Some(run) = self.intruder_close_summary(&target) else {
            return;
        };
        if !results::same_run(&close.run, &run)
            || run.revision < close.run.revision
            || (!run.state.is_terminal()
                && !(close.explicit_save()
                    && run.retention == IntruderRetention::Session
                    && self.can_save_active_intruder_run(&run)))
        {
            return;
        }
        match self.capture_result_close_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.fail_intruder_result_close(error, cx);
                return;
            }
        }
        let Some(revision) = self.workspace.revision.checked_add(1) else {
            self.fail_intruder_result_close("工作区 revision 已耗尽；结果仍保留。", cx);
            return;
        };
        self.workspace.tab_sidebar_width = self.tab_width as u16;
        let mut workspace = Box::new(self.workspace.clone());
        workspace.revision = revision;
        if let Err(error) = workspace.validate_structure() {
            self.fail_intruder_result_close(format!("最新工作区无效：{error}；结果仍保留。"), cx);
            return;
        }
        let retention = if choice == CloseChoice::Keep {
            IntruderRetention::Session
        } else {
            IntruderRetention::Project
        };
        let command = if choice == CloseChoice::Discard {
            Command::DiscardIntruder {
                project: target.project(),
                run_id: target.run_id(),
                operation_id,
                expected_revision: run.revision,
                workspace: workspace.clone(),
            }
        } else {
            Command::SetIntruderRetention {
                project: target.project(),
                run_id: target.run_id(),
                operation_id,
                expected_revision: run.revision,
                retention,
                workspace: workspace.clone(),
            }
        };
        if !self.enqueue(command, cx) {
            self.fail_intruder_result_close(
                self.error
                    .clone()
                    .unwrap_or_else(|| "命令未入队；数据仍保留。".into()),
                cx,
            );
            return;
        }
        if choice == CloseChoice::Discard || retention == IntruderRetention::Session {
            clear_last_run(&mut workspace, target.run_id());
        }
        self.pending_workspace = Some(revision);
        self.workspace_dirty = false;
        let close = self.intruder_result_close.as_mut().unwrap();
        close.run = run;
        close.saved_to_file = false;
        close.phase = if choice == CloseChoice::Discard {
            ClosePhase::Discarding {
                expected_revision: close.run.revision,
                workspace,
            }
        } else {
            ClosePhase::Retaining {
                retention,
                workspace,
            }
        };
        self.set_intruder_close_status("等待结果保存方式的实际确认…");
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_close_owns_workspace(&self) -> bool {
        self.intruder_result_close.as_ref().is_some_and(|close| {
            self.project == Some(close.target.project())
                && matches!(&close.phase, ClosePhase::Discarding { workspace, .. } | ClosePhase::Retaining { workspace, .. }
                    if self.pending_workspace == Some(workspace.revision))
        })
    }

    pub(in crate::workbench) fn fail_intruder_result_close(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        if self.intruder_result_close.as_ref().is_some_and(|close| {
            self.project == Some(close.target.project())
                && matches!(
                    close.phase,
                    ClosePhase::Discarding { .. } | ClosePhase::Retaining { .. }
                )
        }) {
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
                view.close_error = Some(format!(
                    "操作结果未确认：{message} 回执归属仍保留，未自动重试。"
                ));
            }
            self.fail(message, cx);
            return;
        }
        self.reject_intruder_result_close(message, cx);
    }

    pub(in crate::workbench) fn reject_intruder_result_close(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(mut close) = self.intruder_result_close.take() else {
            return;
        };
        let message = message.into();
        if close.target.owns(self)
            && let Some(view) = close
                .target
                .view_id()
                .and_then(|id| self.intruder.results.views.get_mut(&id))
        {
            view.close_status = None;
            view.close_error = Some(message.clone());
        }
        if close.saved_detach() {
            self.fail(message, cx);
            return;
        }
        self.discard_pending_sends(cx);
        self.intruder_save_failed(cx);
        self.plugins_save_failed(cx);
        match &mut close.phase {
            ClosePhase::Confirming { cancelled } => {
                if let Some(dialog) = close.dialog.take() {
                    cx.defer(move |cx| {
                        let _ = dialog.update(cx, |_, window, _| window.remove_window());
                    });
                } else {
                    *cancelled = true;
                    self.intruder_result_close = Some(close);
                }
            }
            ClosePhase::Discarding { workspace, .. } | ClosePhase::Retaining { workspace, .. }
                if self.project == Some(close.target.project())
                    && self.pending_workspace == Some(workspace.revision) =>
            {
                self.pending_workspace = None;
                self.workspace_dirty = true;
            }
            ClosePhase::SavingProject { path: None }
                if self.project == Some(close.target.project()) =>
            {
                self.pending_project_save = None;
            }
            _ => {}
        }
        self.fail(message, cx);
    }

    pub(in crate::workbench) fn fail_intruder_close_read(
        &mut self,
        view_id: Uuid,
        message: String,
        cx: &mut Context<Self>,
    ) {
        if self.intruder_result_close.as_ref().is_some_and(|close| {
            close.target.view_id() == Some(view_id)
                && matches!(close.phase, ClosePhase::Waiting | ClosePhase::Preparing)
        }) {
            self.fail_intruder_result_close(
                format!("Intruder 终态未确认：{message}；未丢弃。"),
                cx,
            );
        }
    }

    pub(in crate::workbench) fn intruder_result_view_forgotten(
        &mut self,
        view_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.intruder_result_close.as_ref().is_some_and(|close| {
            close.target.view_id() == Some(view_id)
                && close.target.owns(self)
                && !matches!(
                    close.phase,
                    ClosePhase::Discarding { .. } | ClosePhase::Retaining { .. }
                )
        }) {
            self.fail_intruder_result_close("结果窗口已关闭或替换；本次关闭选择已取消。", cx);
        }
    }
}
