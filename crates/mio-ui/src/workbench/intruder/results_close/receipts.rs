use mio_core::{IntruderDiscardReceipt, IntruderRetention, IntruderRetentionReceipt};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_intruder_discarded(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        result: Result<IntruderDiscardReceipt, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = &self.intruder_result_close else {
            return;
        };
        if !self.matches_intruder_close_receipt(project, run_id, operation_id) {
            return;
        }
        let ClosePhase::Discarding {
            expected_revision,
            workspace,
        } = &close.phase
        else {
            return;
        };
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                self.reject_intruder_result_close(format!("Intruder 未丢弃：{error}"), cx);
                return;
            }
        };
        if expected_revision.checked_add(1) != Some(receipt.revision)
            || receipt.workspace != **workspace
            || !self.intruder_close_workspace_matches(workspace)
        {
            self.fail_intruder_result_close(
                "Intruder 丢弃确认的 revision 或工作区不一致；窗口和编辑仍保留，未自动重试。",
                cx,
            );
            return;
        }
        if window.window_handle() != self.owner_window {
            let owner = self.owner_window;
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        view.on_intruder_discarded(
                            project,
                            run_id,
                            operation_id,
                            Ok(receipt),
                            window,
                            cx,
                        )
                    });
                });
            });
            return;
        }
        let mut close = self.intruder_result_close.take().unwrap();
        self.merge_intruder_receipt_workspace(&receipt.workspace, Some(run_id), window, cx);
        self.intruder.session_runs.remove(&run_id);
        self.invalidate_discarded_intruder_results(run_id, cx);
        self.status = format!(
            "Intruder 已丢弃；删除 {} 条流量，保留 {} 条另有引用的流量。",
            receipt.deleted_flows, receipt.retained_flows
        );
        close.phase = ClosePhase::Continuing;
        self.intruder_result_close = Some(close);
        self.finish_intruder_result_close(cx);
        self.refresh_history_after_intruder_retention(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn on_intruder_retention_changed(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        result: Result<IntruderRetentionReceipt, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = &self.intruder_result_close else {
            return;
        };
        if !self.matches_intruder_close_receipt(project, run_id, operation_id) {
            return;
        }
        let ClosePhase::Retaining {
            retention,
            workspace,
        } = &close.phase
        else {
            return;
        };
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                self.reject_intruder_result_close(format!("Intruder 保存方式未改变：{error}"), cx);
                return;
            }
        };
        let mut expected = close.run.clone();
        let Some(revision) = expected.revision.checked_add(1) else {
            return;
        };
        expected.revision = revision;
        expected.retention = *retention;
        let valid_title = receipt.run.title.len() <= 4096
            && receipt.run.title_revision <= i64::MAX as u64
            && receipt.run.title_revision >= expected.title_revision
            && (receipt.run.title_revision != expected.title_revision
                || receipt.run.title == expected.title);
        expected.title.clone_from(&receipt.run.title);
        expected.title_revision = receipt.run.title_revision;
        if !valid_title
            || receipt.run != expected
            || receipt.retention != *retention
            || receipt.workspace != **workspace
            || !self.intruder_close_workspace_matches(workspace)
        {
            self.fail_intruder_result_close(
                "Intruder 保存方式确认的身份、revision 或工作区不一致；数据和窗口仍保留。",
                cx,
            );
            return;
        }
        if window.window_handle() != self.owner_window {
            let owner = self.owner_window;
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        view.on_intruder_retention_changed(
                            project,
                            run_id,
                            operation_id,
                            Ok(receipt),
                            window,
                            cx,
                        )
                    });
                });
            });
            return;
        }
        let mut close = self.intruder_result_close.take().unwrap();
        let session = receipt.retention == IntruderRetention::Session;
        self.merge_intruder_receipt_workspace(
            &receipt.workspace,
            session.then_some(run_id),
            window,
            cx,
        );
        if session {
            self.intruder.session_runs.insert(run_id);
        } else {
            self.intruder.session_runs.remove(&run_id);
        }
        self.update_intruder_run_summary(&receipt.run);
        for view in self.intruder.results.views.values_mut().filter(|view| {
            view.run_id == run_id
                && view.summary.as_ref().is_some_and(|summary| {
                    results::same_run(summary, &receipt.run)
                        && summary.revision <= receipt.run.revision
                })
        }) {
            view.persisted = Some(!session);
        }
        close.run = receipt.run;
        close.phase = ClosePhase::Waiting;
        self.intruder_result_close = Some(close);
        if session {
            self.status = "Intruder 已保留在内存；普通保存和另存为不会包含此攻击。".into();
            self.finish_intruder_result_close(cx);
        } else {
            self.status = "Intruder 已写入当前项目；正在确认项目文件保存。".into();
            self.advance_intruder_result_close(cx);
            if self
                .intruder_result_close
                .as_ref()
                .is_some_and(|close| close.expired)
            {
                self.finish_intruder_result_close(cx);
            }
        }
        self.history_loading = false;
        self.history_dirty = true;
        self.refresh_history_after_intruder_retention(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn matches_intruder_close_receipt(
        &self,
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
    ) -> bool {
        self.project == Some(project)
            && self.intruder_result_close.as_ref().is_some_and(|close| {
                close.target.project() == project
                    && close.target.run_id() == run_id
                    && close.operation_id == operation_id
            })
    }

    pub(in crate::workbench) fn intruder_close_workspace_matches(
        &self,
        workspace: &Workspace,
    ) -> bool {
        self.pending_workspace == Some(workspace.revision)
            && self.workspace.revision.checked_add(1) == Some(workspace.revision)
    }

    pub(in crate::workbench) fn merge_intruder_receipt_workspace(
        &mut self,
        workspace: &Workspace,
        clear_run: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input_error = match self.capture_result_close_drafts(window, cx) {
            Ok(changed) => {
                self.workspace_dirty |= changed;
                None
            }
            Err(error) => {
                self.discard_pending_sends(cx);
                self.intruder_save_failed(cx);
                self.plugins_save_failed(cx);
                Some(error)
            }
        };
        if let Some(run_id) = clear_run {
            clear_last_run(&mut self.workspace, run_id);
        }
        self.workspace.revision = workspace.revision;
        self.workspace_dirty |= *self.workspace != *workspace;
        self.apply_workspace_ack(workspace, window, cx);
        if let Some(error) = input_error {
            self.fail(
                format!("结果操作已确认；最新输入仍保留，未发送旧稿：{error}"),
                cx,
            );
        }
    }

    pub(in crate::workbench) fn refresh_history_after_intruder_retention(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_dirty && !self.history_loading {
            let error = self.error.take();
            let offset = self.history.read(cx).delegate().offset;
            self.query_history(offset, window, cx);
            self.error = self.error.take().or(error);
        }
    }

    pub(in crate::workbench) fn prepare_intruder_project_save(
        &mut self,
        operation_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = &self.intruder_result_close else {
            return;
        };
        if close.operation_id != operation_id || !matches!(close.phase, ClosePhase::PreparingSave) {
            return;
        }
        if !close.target.owns(self)
            || window.window_handle() != self.owner_window
            || !self.backend_available()
        {
            self.fail_intruder_result_close("项目或后台状态已改变；项目文件保存未开始。", cx);
            return;
        }
        self.intruder_result_close.as_mut().unwrap().phase =
            ClosePhase::SavingProject { path: None };
        self.set_intruder_close_status("正在保存项目文件…");
        self.begin_project_save(false, window, cx);
    }

    pub(in crate::workbench) fn intruder_close_save_operation(&self) -> Option<Uuid> {
        self.intruder_result_close
            .as_ref()
            .filter(|close| matches!(close.phase, ClosePhase::SavingProject { .. }))
            .map(|close| close.operation_id)
    }

    pub(in crate::workbench) fn intruder_close_save_is_current(
        &self,
        operation: Option<Uuid>,
    ) -> bool {
        operation.is_none_or(|id| {
            self.intruder_result_close.as_ref().is_some_and(|close| {
                close.operation_id == id
                    && !close.expired
                    && !close.deadline_elapsed()
                    && close.target.owns(self)
            })
        })
    }

    pub(in crate::workbench) fn intruder_close_project_save_sent(
        &mut self,
        path: Option<&PathBuf>,
    ) {
        let destination = path.or(self.project_path.as_ref()).cloned();
        if let Some(close) = self.intruder_result_close.as_mut()
            && let ClosePhase::SavingProject { path } = &mut close.phase
        {
            *path = destination;
        }
    }

    pub(in crate::workbench) fn intruder_close_project_saved(
        &mut self,
        path: &PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.expire_intruder_close_if_due(cx);
        let Some(close) = &self.intruder_result_close else {
            return;
        };
        if !matches!(&close.phase, ClosePhase::SavingProject { path: Some(expected) } if expected == path)
        {
            return;
        }
        self.status = "Intruder 项目文件保存已确认。".into();
        match self.capture_result_close_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.fail_intruder_result_close(error, cx);
                return;
            }
        }
        let close = self.intruder_result_close.as_mut().unwrap();
        close.phase = ClosePhase::Waiting;
        close.saved_to_file = true;
        if close.expired {
            self.finish_intruder_result_close(cx);
        } else if self.workspace_dirty || self.pending_workspace.is_some() {
            self.fail_intruder_result_close(
                "项目文件已保存；保存期间的新编辑仍保留，请再次关闭以确认最新内容。",
                cx,
            );
        } else {
            self.advance_intruder_result_close(cx);
        }
    }
}

pub(super) fn clear_last_run(workspace: &mut Workspace, run_id: Uuid) {
    for tab in workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut workspace.intruder.closed)
    {
        if tab.last_run == Some(run_id) {
            tab.last_run = None;
        }
    }
}
