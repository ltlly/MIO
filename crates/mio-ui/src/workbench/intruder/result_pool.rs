use std::time::{Duration, Instant};

use super::*;

pub(in crate::workbench) struct PendingPoolChange {
    project: ProjectKey,
    operation: Uuid,
    run: IntruderRunSummary,
    pool: Uuid,
    cancel: CancellationToken,
    deadline: Instant,
    expired: bool,
}

impl Drop for PendingPoolChange {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn valid_pool_receipt(pending: &PendingPoolChange, run: &IntruderRunSummary) -> bool {
    if !results::same_run(&pending.run, run)
        || run.resource_pool != Some(pending.pool)
        || run.resource_pool_revision != pending.run.resource_pool_revision + 1
        || run.revision < pending.run.revision
        || run.title_revision < pending.run.title_revision
        || run.title.len() > 4096
        || run.title_revision > i64::MAX as u64
        || (run.title_revision == pending.run.title_revision && run.title != pending.run.title)
    {
        return false;
    }
    if run.revision == pending.run.revision {
        let mut expected = pending.run.clone();
        expected.resource_pool = run.resource_pool;
        expected.resource_pool_revision = run.resource_pool_revision;
        expected.title.clone_from(&run.title);
        expected.title_revision = run.title_revision;
        expected == *run
    } else {
        run.payload_progress().is_some()
    }
}

impl Workbench {
    pub(in crate::workbench) fn intruder_pool_change_pending(&self, run_id: Uuid) -> bool {
        self.intruder.pool_changes.contains_key(&run_id)
    }

    pub(in crate::workbench) fn can_change_intruder_pool(&self, view_id: Uuid) -> bool {
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return false;
        };
        self.backend_available()
            && self.transition.is_none()
            && self.quit_attempt.is_none()
            && !self.shutdown_requested
            && !self.file_picker_pending()
            && self.pending_records == 0
            && self.storage_error.is_none()
            && !view.discarded
            && view.summary.is_some()
            && view.pending.is_none()
            && view.error.is_none()
            && !self.intruder_run_status_pending(view.run_id)
            && !self.intruder_pool_change_pending(view.run_id)
            && !self.intruder_baseline_pending(view.run_id)
            && !self.intruder_result_close_pending(view.run_id)
            && self.intruder.resource_pools.pending.is_none()
            && self.intruder.resource_pools.error.is_none()
            && self.intruder.resource_pools.catalog.is_some()
    }

    fn pool_change_error(&mut self, run_id: Uuid, message: String, cx: &mut Context<Self>) {
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|v| v.run_id == run_id)
        {
            view.pool_error = Some(message.clone());
        }
        self.status = message;
        cx.notify();
    }

    pub(in crate::workbench) fn change_intruder_result_pool(
        &mut self,
        view_id: Uuid,
        pool: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_change_intruder_pool(view_id) {
            return;
        }
        let view = &self.intruder.results.views[&view_id];
        if view.owner != window.window_handle() {
            return;
        }
        let run = view.summary.clone().unwrap();
        if run.resource_pool == Some(pool)
            || run.resource_pool_revision >= i64::MAX as u64
            || !self
                .intruder
                .resource_pools
                .catalog
                .as_ref()
                .is_some_and(|p| p.find(pool).is_some())
        {
            return;
        }
        let Some(project) = self.project else { return };
        let operation = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        if !self.enqueue(
            Command::SetIntruderResourcePool {
                project,
                run_id: run.id,
                operation_id: operation,
                expected_pool_revision: run.resource_pool_revision,
                resource_pool: pool,
                cancel: cancel.clone(),
            },
            cx,
        ) {
            cancel.cancel();
            self.pool_change_error(
                run.id,
                "资源池切换未入队；原绑定保留，请明确重试。".into(),
                cx,
            );
            return;
        }
        let run_id = run.id;
        self.intruder.pool_changes.insert(
            run_id,
            PendingPoolChange {
                project,
                operation,
                run,
                pool,
                cancel,
                deadline: cx.background_executor().now() + Duration::from_secs(30),
                expired: false,
            },
        );
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|v| v.run_id == run_id)
        {
            view.pool_error = None;
        }
        let timer = cx.background_executor().timer(Duration::from_secs(30));
        let owner = self.owner_window;
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    if let Some(pending) = view.intruder.pool_changes.get_mut(&run_id)
                        && pending.project == project
                        && pending.operation == operation
                        && cx.background_executor().now() >= pending.deadline
                    {
                        pending.expired = true;
                        pending.cancel.cancel();
                        view.pool_change_error(
                            run_id,
                            "资源池切换确认超时；已请求取消，等待后台确认实际绑定，未自动重试。"
                                .into(),
                            cx,
                        );
                    }
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn on_intruder_resource_pool_changed(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        result: Result<IntruderRunSummary, String>,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project)
            || !self
                .intruder
                .pool_changes
                .get(&run_id)
                .is_some_and(|p| p.project == project && p.operation == operation_id)
        {
            return;
        }
        let pending = self.intruder.pool_changes.remove(&run_id).unwrap();
        match result {
            Ok(run) if valid_pool_receipt(&pending, &run) => {
                self.update_intruder_run_summary(&run);
                for view in self
                    .intruder
                    .results
                    .views
                    .values_mut()
                    .filter(|v| v.run_id == run_id)
                {
                    view.pool_error = None;
                }
                self.status = if pending.expired {
                    "资源池切换已由迟到回执确认。"
                } else {
                    "已切换资源池；在途请求保留原池占用。"
                }
                .into();
            }
            Ok(_) => {
                self.pool_change_error(
                    run_id,
                    "资源池回执身份或版本不一致；未采用回执，请重新打开侧栏核对。".into(),
                    cx,
                );
                self.query_intruder_run_status(run_id, cx);
            }
            Err(error) => {
                self.pool_change_error(
                    run_id,
                    format!("资源池切换未确认：{error}；正在核对实际绑定。"),
                    cx,
                );
                self.query_intruder_run_status(run_id, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_pool_disconnected(&mut self, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.intruder.pool_changes);
        for (id, _) in pending {
            self.pool_change_error(id, "后台已断开；资源池切换未确认。".into(), cx);
        }
    }
}
