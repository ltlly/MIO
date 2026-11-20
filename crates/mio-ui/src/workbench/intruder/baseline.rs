use std::time::{Duration, Instant};

use mio_core::{IntruderBaseline, IntruderBaselineReceipt, IntruderRunState};

use super::*;

pub(super) struct PendingBaseline {
    project: ProjectKey,
    operation: Uuid,
    run: IntruderRunSummary,
    pub(super) cancel: CancellationToken,
    deadline: Instant,
    expired: bool,
    pub(super) stop_requested: bool,
    status_after: Option<u64>,
}

impl Drop for PendingBaseline {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Workbench {
    pub(in crate::workbench) fn intruder_baseline_pending(&self, run_id: Uuid) -> bool {
        self.intruder.baseline_requests.contains_key(&run_id)
    }

    pub(in crate::workbench) fn can_request_intruder_baseline(
        &self,
        run: &IntruderRunSummary,
    ) -> bool {
        self.backend_available()
            && self.transition.is_none()
            && self.quit_attempt.is_none()
            && !self.shutdown_requested
            && !self.file_picker_pending()
            && !self.intruder_result_close_pending(run.id)
            && self.pending_workspace.is_none()
            && self.pending_project_save.is_none()
            && self.project_saves_in_flight == 0
            && !self.saving_project_snapshot
            && self.pending_records == 0
            && self.storage_error.is_none()
            && run.baseline.is_none()
            && run.payload_progress().is_some()
            && matches!(
                run.state,
                IntruderRunState::Running | IntruderRunState::Paused | IntruderRunState::Completed
            )
            && !self.intruder_baseline_pending(run.id)
            && !self.intruder_pool_change_pending(run.id)
            && self.intruder.baseline_requests.len() < 8
            && self
                .intruder
                .runs
                .get(&run.id)
                .is_none_or(|active| !active.cancel.is_cancelled())
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
                        && view.pending.is_none()
                        && view.error.is_none()
                })
    }

    fn baseline_error(&mut self, run_id: Uuid, message: String, cx: &mut Context<Self>) {
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == run_id)
        {
            view.baseline_error = Some(message.clone());
        }
        self.status = message;
        cx.notify();
    }

    pub(in crate::workbench) fn request_intruder_baseline(
        &mut self,
        view_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        let Some(view) = self
            .intruder
            .results
            .views
            .get(&view_id)
            .filter(|view| view.owner == window.window_handle())
        else {
            return;
        };
        let Some(run) = view.summary.clone() else {
            return;
        };
        if !self.can_request_intruder_baseline(&run) {
            return;
        }
        let operation = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        if !self.enqueue(
            Command::RequestIntruderBaseline {
                project,
                run_id: run.id,
                operation_id: operation,
                expected_revision: run.revision,
                cancel: cancel.clone(),
            },
            cx,
        ) {
            cancel.cancel();
            self.baseline_error(run.id, "基准请求未入队；未发送，请明确重试。".into(), cx);
            return;
        }
        let run_id = run.id;
        self.intruder.baseline_requests.insert(
            run_id,
            PendingBaseline {
                project,
                operation,
                run,
                cancel,
                deadline: cx.background_executor().now() + Duration::from_secs(30),
                expired: false,
                stop_requested: false,
                status_after: None,
            },
        );
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == run_id)
        {
            view.baseline_error = None;
        }
        let timer = cx.background_executor().timer(Duration::from_secs(30));
        let owner = self.owner_window;
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.project == Some(project)
                        && view
                            .intruder
                            .baseline_requests
                            .get(&run_id)
                            .is_some_and(|pending| pending.operation == operation)
                    {
                        view.expire_intruder_baseline(run_id, cx);
                    }
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn expire_intruder_baseline(&mut self, run_id: Uuid, cx: &mut Context<Self>) {
        if let Some(pending) = self.intruder.baseline_requests.get_mut(&run_id)
            && !pending.expired
            && cx.background_executor().now() >= pending.deadline
        {
            pending.expired = true;
            pending.cancel.cancel();
            self.baseline_error(
                run_id,
                "基准请求等待超时；准入取消已请求，实际结果未确认，未重发既有请求。".into(),
                cx,
            );
            self.query_intruder_run_status(run_id, cx);
        }
    }

    pub(in crate::workbench) fn intruder_baseline_disconnected(&mut self, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.intruder.baseline_requests);
        for (run_id, _) in pending {
            self.baseline_error(
                run_id,
                "后台已断开；基准请求结果未确认，未自动重试。".into(),
                cx,
            );
        }
    }

    pub(in crate::workbench) fn confirm_intruder_baseline_stop(
        &mut self,
        run: &IntruderRunSummary,
        query_revision: u64,
    ) {
        if self
            .intruder
            .baseline_requests
            .get(&run.id)
            .is_some_and(|pending| {
                pending.stop_requested
                    && pending
                        .status_after
                        .is_some_and(|after| query_revision > after)
                    && results::same_run(&pending.run, run)
                    && run.revision >= pending.run.revision
                    && run.state.is_terminal()
            })
        {
            self.intruder.baseline_requests.remove(&run.id);
        }
    }

    pub(in crate::workbench) fn on_intruder_baseline_requested(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        result: Result<IntruderBaselineReceipt, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project)
            || !self
                .intruder
                .baseline_requests
                .get(&run_id)
                .is_some_and(|pending| {
                    pending.project == project
                        && pending.operation == operation_id
                        && pending.status_after.is_none()
                })
        {
            return;
        }
        self.expire_intruder_baseline(run_id, cx);
        let mut pending = self.intruder.baseline_requests.remove(&run_id).unwrap();
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                if pending.stop_requested {
                    pending.status_after = Some(self.intruder.results.query_revision);
                    self.intruder.baseline_requests.insert(run_id, pending);
                }
                self.baseline_error(
                    run_id,
                    format!("基准请求未确认：{error}；输入与结果仍保留，请重新读取后明确重试。"),
                    cx,
                );
                let views: Vec<_> = self
                    .intruder
                    .results
                    .views
                    .iter()
                    .filter(|(_, view)| view.run_id == run_id)
                    .map(|(id, _)| *id)
                    .collect();
                if views.is_empty() {
                    self.query_intruder_run_status(run_id, cx);
                }
                for id in views {
                    self.query_intruder_results(id, None, window, cx);
                }
                return;
            }
        };
        let mut expected = pending.run.clone();
        let valid_counts = expected
            .revision
            .checked_add(1)
            .zip(expected.total.checked_add(1));
        let Some((revision, total)) = valid_counts else {
            return;
        };
        expected.revision = revision;
        expected.total = total;
        expected.baseline = Some(IntruderBaseline {
            ordinal: pending.run.dispatched,
            completed: false,
        });
        if expected.state == IntruderRunState::Completed {
            expected.state = IntruderRunState::Running;
        }
        let valid_title = receipt.run.title.len() <= 4096
            && receipt.run.title_revision <= i64::MAX as u64
            && receipt.run.title_revision >= expected.title_revision
            && (receipt.run.title_revision != expected.title_revision
                || receipt.run.title == expected.title);
        expected.title.clone_from(&receipt.run.title);
        expected.title_revision = receipt.run.title_revision;
        if !valid_title || expected != receipt.run || receipt.run.payload_progress().is_none() {
            self.intruder.baseline_requests.insert(run_id, pending);
            self.baseline_error(
                run_id,
                "基准请求回执的身份、revision 或计数不一致；归属仍保留，未自动重试。".into(),
                cx,
            );
            return;
        }
        let mut latest = receipt.run.clone();
        for summary in self
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
        {
            results::merge_run_summary(&mut latest, summary);
        }
        let expired = pending.expired;
        if pending.stop_requested {
            receipt.cancel.cancel();
            pending.run = latest.clone();
            pending.status_after = Some(self.intruder.results.query_revision);
            self.intruder.baseline_requests.insert(run_id, pending);
        }
        if !latest.state.is_terminal() {
            self.intruder.runs.insert(
                run_id,
                IntruderRun {
                    summary: latest.clone(),
                    cancel: receipt.cancel,
                },
            );
        }
        self.update_intruder_run_summary(&latest);
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == run_id)
        {
            view.baseline_error =
                expired.then(|| "迟到回执已确认一次基准请求；既有 payload 未重发。".into());
        }
        let ids: Vec<_> = self
            .intruder
            .results
            .views
            .iter()
            .filter(|(_, view)| view.run_id == run_id)
            .map(|(id, _)| *id)
            .collect();
        let stopping = self.intruder_result_close_pending(run_id)
            || self
                .intruder
                .baseline_requests
                .get(&run_id)
                .is_some_and(|pending| pending.stop_requested);
        for id in ids {
            let view = &self.intruder.results.views[&id];
            if stopping || view.stop_requested || view.control_pending.is_some() {
                self.query_intruder_results(id, None, window, cx);
            } else {
                let offset = view.query.page.as_ref().map_or(0, |page| page.offset);
                let (filter, sort) = view.query.requested();
                self.query_intruder_result_view(id, offset, filter, sort, window, cx);
            }
        }
        if !self
            .intruder
            .results
            .views
            .values()
            .any(|view| view.run_id == run_id)
        {
            self.query_intruder_run_status(run_id, cx);
        }
        cx.notify();
    }
}
