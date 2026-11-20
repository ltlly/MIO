use std::time::{Duration, Instant};

use mio_core::IntruderResourcePools;

use super::*;

const DEADLINE: Duration = Duration::from_secs(30);

pub(super) struct PendingDefaultPool {
    project: ProjectKey,
    draft: Draft,
    operation: Option<Uuid>,
    cancel: CancellationToken,
    deadline: Option<Instant>,
    _timer: Option<Task<()>>,
}

impl Drop for PendingDefaultPool {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Workbench {
    pub(in crate::workbench) fn pending_intruder_default_draft(&self) -> Option<Draft> {
        self.intruder
            .default_pool
            .as_ref()
            .filter(|pending| self.project == Some(pending.project))
            .map(|pending| pending.draft.clone())
    }

    pub(in crate::workbench) fn cancel_intruder_default_pool(
        &mut self,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = &mut self.intruder.default_pool {
            pending.cancel.cancel();
            pending.operation = None;
            pending.deadline = None;
            pending._timer = None;
            self.fail(
                format!("{reason} 请通过原新建／发送到 Intruder 入口重试保留的请求。"),
                cx,
            );
        }
    }

    fn default_pool_creation_available(&self, window: &Window) -> bool {
        window.window_handle() == self.owner_window
            && self.backend_available()
            && !self.shutdown_requested
            && self.transition.is_none()
            && self.quit_attempt.is_none()
            && !self.file_picker_pending()
            && self.pending_project_save.is_none()
            && !self.saving_project_snapshot
            && self.project_saves_in_flight == 0
            && self.pending_records == 0
            && self.storage_error.is_none()
    }

    fn expire_intruder_default_pool(&mut self, cx: &mut Context<Self>) {
        if self.intruder.default_pool.as_ref().is_some_and(|pending| {
            pending.operation.is_some()
                && pending
                    .deadline
                    .is_some_and(|deadline| cx.background_executor().now() >= deadline)
        }) {
            self.cancel_intruder_default_pool(
                "默认资源池确认超时；已取消准入，未创建标签或发送请求。",
                cx,
            );
        }
    }

    pub(in crate::workbench) fn open_intruder_draft(
        &mut self,
        mut draft: Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            self.fail("尚未打开项目，未创建 Intruder 标签。", cx);
            return;
        };
        if window.window_handle() != self.owner_window {
            self.fail("请求来源窗口已改变，未创建 Intruder 标签。", cx);
            return;
        }
        self.expire_intruder_default_pool(cx);
        if let Some(pending) = &self.intruder.default_pool {
            if pending.project != project {
                self.fail("先前待导入请求属于其它项目，未带入当前项目。", cx);
                return;
            }
            if pending.draft.target != draft.target
                || pending.draft.raw != draft.raw
                || pending.draft.version != draft.version
            {
                self.fail("当前请求已改变；先前待导入请求仍保留。请用 Intruder 的 + 新建入口重试原请求，完成后再发送当前请求。", cx);
                return;
            }
            if pending.operation.is_some() {
                self.fail(
                    "正在确认首个请求的默认资源池；重复操作未覆盖该请求，请等待确认。",
                    cx,
                );
                return;
            }
        } else {
            draft.revision = 1;
            self.intruder.default_pool = Some(PendingDefaultPool {
                project,
                draft,
                operation: None,
                cancel: self.shutdown.child_token(),
                deadline: None,
                _timer: None,
            });
        }
        if !self.default_pool_creation_available(window) {
            self.cancel_intruder_default_pool(
                "当前项目操作或后台状态不允许新建标签；待导入请求保留。",
                cx,
            );
            return;
        }
        if !self.flush_workspace(window, cx) {
            self.cancel_intruder_default_pool("当前工作区输入尚未确认；待导入请求保留。", cx);
            return;
        }
        let operation_id = Uuid::new_v4();
        let cancel = self.shutdown.child_token();
        let pending = self.intruder.default_pool.as_mut().unwrap();
        pending.cancel.cancel();
        pending.cancel = cancel.clone();
        pending.operation = Some(operation_id);
        pending.deadline = Some(cx.background_executor().now() + DEADLINE);
        pending._timer = None;
        if !self.enqueue(
            Command::EnsureIntruderDefaultPool {
                project,
                operation_id,
                cancel,
            },
            cx,
        ) {
            let error = self
                .error
                .clone()
                .unwrap_or_else(|| "后台未接受请求".into());
            self.cancel_intruder_default_pool(&format!("默认资源池确认未入队：{error}"), cx);
            return;
        }
        let timer = cx.background_executor().timer(DEADLINE);
        let owner = self.owner_window;
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.project == Some(project)
                        && view
                            .intruder
                            .default_pool
                            .as_ref()
                            .is_some_and(|pending| pending.operation == Some(operation_id))
                    {
                        view.expire_intruder_default_pool(cx);
                    }
                });
            });
        });
        self.intruder.default_pool.as_mut().unwrap()._timer = Some(task);
        self.status = "正在确认默认资源池；首个待导入请求已保留，尚未创建标签。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn on_intruder_default_pool_ready(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        result: Result<IntruderResourcePools, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project)
            || !self.intruder.default_pool.as_ref().is_some_and(|pending| {
                pending.project == project && pending.operation == Some(operation_id)
            })
        {
            return;
        }
        self.expire_intruder_default_pool(cx);
        if self
            .intruder
            .default_pool
            .as_ref()
            .is_none_or(|pending| pending.operation != Some(operation_id))
        {
            return;
        }
        if !self.default_pool_creation_available(window)
            || self
                .intruder
                .default_pool
                .as_ref()
                .unwrap()
                .cancel
                .is_cancelled()
        {
            self.cancel_intruder_default_pool(
                "默认资源池回执到达时项目操作已改变；待导入请求保留，未创建标签。",
                cx,
            );
            return;
        }
        let catalog = match result {
            Ok(catalog) => catalog,
            Err(error) => {
                self.cancel_intruder_default_pool(
                    &format!("默认资源池未确认：{error}；待导入请求保留。"),
                    cx,
                );
                return;
            }
        };
        let pool_id = match catalog
            .validate()
            .map_err(|error| error.to_string())
            .and_then(|_| {
                catalog
                    .default_pool_id
                    .ok_or_else(|| "回执缺少默认资源池".to_owned())
            }) {
            Ok(pool_id) => pool_id,
            Err(error) => {
                self.cancel_intruder_default_pool(
                    &format!("默认资源池回执无效：{error}；未创建标签。"),
                    cx,
                );
                return;
            }
        };
        if !self.flush_workspace(window, cx) {
            self.cancel_intruder_default_pool(
                "默认资源池已确认，但当前工作区输入未确认；待导入请求保留。",
                cx,
            );
            return;
        }
        if !self.accept_intruder_resource_pools(project, &catalog, cx) {
            self.cancel_intruder_default_pool(
                "默认资源池目录与已确认版本冲突；待导入请求保留。",
                cx,
            );
            return;
        }
        let draft = self.intruder.default_pool.as_ref().unwrap().draft.clone();
        self.error = None;
        if self.append_intruder_default_draft(draft, pool_id, window, cx) {
            self.intruder.default_pool = None;
            self.status = "已创建 Intruder 标签并绑定默认资源池。".into();
        } else {
            self.cancel_intruder_default_pool("当前工作区不能加入标签；待导入请求保留。", cx);
        }
    }
}

#[cfg(test)]
#[path = "default_pool_tests.rs"]
mod tests;
