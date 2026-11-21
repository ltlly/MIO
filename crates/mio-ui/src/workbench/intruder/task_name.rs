use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, Bounds, size};

use super::*;

mod dialog;
#[cfg(test)]
use dialog::TaskNameWindow;
use dialog::TaskNameWindow as TaskNameDialog;

#[cfg(test)]
#[path = "task_name_tests.rs"]
mod tests;

const DEADLINE: Duration = Duration::from_secs(30);

pub(super) struct TaskRename {
    id: Uuid,
    project: ProjectKey,
    run: IntruderRunSummary,
    dialog: Option<AnyWindowHandle>,
    opening: bool,
    pending: Option<Attempt>,
    last_operation: Option<Uuid>,
    status: String,
    disconnected: bool,
    _timer: Option<Task<()>>,
}

struct Attempt {
    operation: Uuid,
    expected_revision: u64,
    title: String,
    input: Entity<InputState>,
    input_revision: u64,
    deadline: Instant,
    expired: bool,
}

impl Workbench {
    pub(in crate::workbench) fn intruder_task_name_pending(&self) -> bool {
        self.intruder.task_name.as_ref().is_some_and(|rename| {
            rename.opening
                || rename.dialog.is_some()
                || (rename
                    .pending
                    .as_ref()
                    .is_some_and(|pending| !pending.expired)
                    && !rename.disconnected)
        })
    }

    fn task_name_summary(&self, run_id: Uuid) -> Option<IntruderRunSummary> {
        self.intruder
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
            .cloned()
            .reduce(|mut current, incoming| {
                results::merge_run_summary(&mut current, &incoming);
                current
            })
    }

    pub(in crate::workbench) fn open_intruder_task_name(
        &mut self,
        run_id: Uuid,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = &self.intruder.task_name {
            if let Some(dialog) = rename.dialog {
                cx.defer(move |cx| {
                    let _ = dialog.update(cx, |_, window, _| window.activate_window());
                });
            } else {
                self.fail(
                    "上次任务重命名仍未确认；请等待回执或重开项目核对磁盘名称。",
                    cx,
                );
            }
            return;
        }
        if !self.backend_available()
            || self.transition.is_some()
            || self.file_picker_pending()
            || self.shutdown_requested
        {
            self.fail("当前无法重命名任务；请先完成正在进行的操作。", cx);
            return;
        }
        let (Some(project), Some(run)) = (self.project, self.task_name_summary(run_id)) else {
            return;
        };
        let title = if run.title.is_empty() {
            self.intruder
                .results
                .views
                .values()
                .filter(|view| view.run_id == run_id)
                .find_map(|view| view.spec.as_ref().map(|spec| spec.draft.target.clone()))
                .or_else(|| {
                    self.workspace
                        .intruder
                        .tabs
                        .iter()
                        .chain(&self.workspace.intruder.closed)
                        .find(|tab| tab.id == run.tab_id && tab.spec.revision == run.spec_revision)
                        .map(|tab| tab.spec.draft.target.clone())
                })
                .filter(|target| !target.is_empty())
                .map_or_else(
                    || format!("Intruder 攻击 · {}", &run_id.to_string()[..8]),
                    |target| format!("Intruder 攻击：{target}"),
                )
        } else {
            run.title.clone()
        };
        let id = Uuid::new_v4();
        self.intruder.task_name = Some(TaskRename {
            id,
            project,
            run,
            dialog: None,
            opening: true,
            pending: None,
            last_operation: None,
            status: String::new(),
            disconnected: false,
            _timer: None,
        });
        let owner = cx.entity();
        let owner_window = self.owner_window;
        cx.defer(move |cx| {
            if !cx.windows().contains(&owner_window)
                || !owner
                    .read(cx)
                    .intruder
                    .task_name
                    .as_ref()
                    .is_some_and(|rename| rename.id == id && rename.project == project)
            {
                return;
            }
            let scale = theme::ui_scale();
            let content_w = if scale > 1.0 {
                theme::scaled_px(415.)
            } else {
                px(415.)
            };
            let content_h = if scale > 1.0 {
                theme::scaled_px(172.)
            } else {
                px(172.)
            };
            let raw_h = if cfg!(test) {
                content_h
            } else {
                let bar_h = if scale > 1.0 {
                    theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                        .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
                } else {
                    px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                };
                content_h + bar_h
            };
            let bounds = if let Some(display) = cx.primary_display() {
                let visible = display.visible_bounds();
                let fitted = size(
                    content_w.min(visible.size.width),
                    raw_h.min(visible.size.height),
                );
                let mut bounds = Bounds::centered(Some(display.id()), fitted, cx);
                bounds.origin.x = bounds.origin.x.clamp(
                    visible.origin.x,
                    (visible.right() - fitted.width).max(visible.origin.x),
                );
                bounds.origin.y = bounds.origin.y.clamp(
                    visible.origin.y,
                    (visible.bottom() - fitted.height).max(visible.origin.y),
                );
                bounds
            } else {
                Bounds::centered(None, size(content_w, raw_h), cx)
            };
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| cx.new(|cx| TaskNameDialog::new(owner.clone(), id, title, window, cx)),
            );
            owner.update(cx, |view, cx| {
                if let Some(rename) = &mut view.intruder.task_name
                    && rename.id == id
                {
                    match opened {
                        Ok((dialog, _)) => {
                            rename.dialog = Some(dialog);
                            rename.opening = false;
                        }
                        Err(error) => {
                            view.intruder.task_name = None;
                            view.fail(format!("无法打开任务重命名窗口：{error}"), cx);
                        }
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    pub(in crate::workbench) fn confirm_task_name(
        &mut self,
        id: Uuid,
        title: String,
        input: Entity<InputState>,
        input_revision: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = &self.intruder.task_name else {
            return;
        };
        if rename.id != id || rename.pending.is_some() || self.project != Some(rename.project) {
            return;
        }
        if !self.backend_available() {
            self.intruder_task_name_disconnected(cx);
            return;
        }
        let current = self.task_name_summary(rename.run.id);
        let Some(current) = current.filter(|run| results::same_run(run, &rename.run)) else {
            self.set_task_name_status("此任务已不可用；输入仍保留，请取消后重新读取任务。", cx);
            return;
        };
        if title.len() > 4096 {
            self.set_task_name_status("名称超过 4096 字节；未提交。", cx);
            return;
        }
        let project = rename.project;
        let operation = Uuid::new_v4();
        let expected_revision = current.title_revision;
        if !self.enqueue(
            Command::RenameIntruder {
                project,
                run_id: current.id,
                operation_id: operation,
                expected_title_revision: expected_revision,
                title: title.clone(),
            },
            cx,
        ) {
            self.set_task_name_status("重命名命令未入队；输入仍保留，请重试。", cx);
            return;
        }
        let executor = cx.background_executor().clone();
        let deadline = executor.now() + DEADLINE;
        let timer = executor.timer(DEADLINE);
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| view.expire_task_name(operation, cx));
        });
        let rename = self.intruder.task_name.as_mut().unwrap();
        rename.run = current;
        rename.pending = Some(Attempt {
            operation,
            expected_revision,
            title,
            input,
            input_revision,
            deadline,
            expired: false,
        });
        rename.last_operation = Some(operation);
        rename.status = "正在确认任务名称…".into();
        rename._timer = Some(task);
        cx.notify();
    }

    fn expire_task_name(&mut self, operation: Uuid, cx: &mut Context<Self>) {
        if let Some(rename) = &mut self.intruder.task_name
            && let Some(pending) = &mut rename.pending
            && pending.operation == operation
            && !pending.expired
        {
            pending.expired = true;
            rename.status =
                "任务名称确认已超时。输入仍保留；取消不会撤销可能已完成的写入，可重开项目核对。"
                    .into();
            self.status = rename.status.clone();
            cx.notify();
        }
    }

    fn set_task_name_status(&mut self, status: &str, cx: &mut Context<Self>) {
        if let Some(rename) = &mut self.intruder.task_name {
            rename.status = status.into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_task_name_disconnected(&mut self, cx: &mut Context<Self>) {
        if let Some(rename) = &mut self.intruder.task_name {
            rename.disconnected = true;
            rename.status =
                "后台已断开；名称写入结果未确认。输入仍保留，取消后可重开项目核对磁盘名称。".into();
            self.status = rename.status.clone();
            if rename.dialog.is_none() && !rename.opening {
                self.intruder.task_name = None;
            }
            cx.notify();
        }
    }

    pub(in crate::workbench) fn cancel_task_name(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(rename) = &mut self.intruder.task_name else {
            return;
        };
        if rename.id != id {
            return;
        }
        let dialog = rename.dialog.take();
        rename.opening = false;
        if rename.pending.is_some() && !rename.disconnected {
            self.status =
                "重命名窗口已关闭，写入未撤销；等待回执，超时后请重开项目核对任务名称。".into();
        } else {
            self.intruder.task_name = None;
        }
        if let Some(dialog) = dialog {
            cx.defer(move |cx| {
                let _ = dialog.update(cx, |_, window, _| window.remove_window());
            });
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_intruder_renamed(
        &mut self,
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        result: Result<IntruderRunSummary, String>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(rename) = &self.intruder.task_name else {
            return false;
        };
        let Some(pending) = &rename.pending else {
            return false;
        };
        if self.project != Some(project)
            || rename.project != project
            || rename.run.id != run_id
            || pending.operation != operation_id
        {
            return false;
        }
        let expired = pending.expired
            || cx.background_executor().now() >= pending.deadline
            || rename.disconnected;
        let valid = result.and_then(|run| {
            if !results::same_run(&rename.run, &run) || run.title_revision != pending.expected_revision + 1 || run.title != pending.title {
                Err("重命名回执的运行身份、名称或 revision 不匹配；输入仍保留，请重新读取任务后重试。".into())
            } else { Ok(run) }
        });
        let rename = self.intruder.task_name.as_mut().unwrap();
        let attempt = rename.pending.take().unwrap();
        rename._timer = None;
        let id = rename.id;
        let closed = rename.dialog.is_none();
        match valid {
            Ok(run) => {
                rename.run = run.clone();
                rename.status = if expired {
                    "迟到回执已确认名称，输入仍保留；可取消关闭或再次明确确认。".into()
                } else {
                    "任务名称已确认。".into()
                };
                self.update_intruder_run_summary(&run);
                self.status = "任务名称已确认。".into();
                if closed {
                    self.cancel_task_name(id, cx);
                } else if !expired {
                    let dialog = self.intruder.task_name.as_ref().unwrap().dialog.unwrap();
                    let owner = cx.weak_entity();
                    // Recheck in the input's own window immediately before removing it.
                    cx.defer(move |cx| {
                        let _ = dialog.update(cx, |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                let Some(rename) = &mut view.intruder.task_name else {
                                    return;
                                };
                                if view.project != Some(project)
                                    || rename.project != project
                                    || rename.id != id
                                    || rename.dialog != Some(dialog)
                                    || rename.last_operation != Some(operation_id)
                                    || rename.pending.is_some()
                                {
                                    return;
                                }
                                if rename.disconnected
                                    || cx.background_executor().now() >= attempt.deadline
                                {
                                    rename.status =
                                        "名称回执已收到，窗口保留供核对；请明确确认或取消。".into();
                                    cx.notify();
                                    return;
                                }
                                let unchanged = attempt.input.read(cx).document_revision()
                                    == attempt.input_revision
                                    && editing::input_snapshot(&attempt.input, window, cx)
                                        .is_ok_and(|value| value.as_ref() == attempt.title);
                                if unchanged {
                                    view.intruder.task_name = None;
                                    window.remove_window();
                                } else {
                                    rename.status =
                                        "已确认提交的名称；后续输入尚未保存，请再次确认或取消。"
                                            .into();
                                }
                                cx.notify();
                            });
                        });
                    });
                }
            }
            Err(error) => {
                rename.status = error.clone();
                self.status = format!("任务重命名失败：{error}");
                if closed {
                    self.intruder.task_name = None;
                }
                self.query_intruder_runs(None, cx);
            }
        }
        cx.notify();
        true
    }
}
