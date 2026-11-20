use std::time::{Duration, Instant};

use mio_core::{HistoryTransferEntry, Uuid, WorkTab, validate_target_origin};

use super::*;

const DEADLINE: Duration = Duration::from_secs(30);

pub(super) struct PendingHistoryTransfer {
    project: ProjectKey,
    operation: Uuid,
    result_target: Option<intruder::results_view::ResultTarget>,
    entries: Vec<HistoryTransferEntry>,
    intruder: bool,
    workspace_revision: Option<u64>,
    cancel: CancellationToken,
    deadline: Instant,
    _timer: Option<Task<()>>,
}

impl Drop for PendingHistoryTransfer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Workbench {
    fn history_transfer_available(&self, window: &Window) -> bool {
        window.window_handle() == self.owner_window
            && self.backend_available()
            && !self.shutdown_requested
            && self.transition.is_none()
            && self.quit_attempt.is_none()
            && !self.file_picker_pending()
            && self.pending_project_save.is_none()
            && self.project_saves_in_flight == 0
            && !self.saving_project_snapshot
            && self.pending_records == 0
            && self.storage_error.is_none()
    }

    pub(in crate::workbench) fn cancel_history_transfer(
        &mut self,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = &mut self.history_transfer {
            pending.cancel.cancel();
            pending._timer = None;
            if pending.workspace_revision.is_some() {
                self.fail(
                    format!("{reason} 已请求取消，等待实际读取回执；未创建标签。"),
                    cx,
                );
            } else {
                self.history_transfer = None;
                self.fail(format!("{reason} 未创建工作标签；可从原转送入口重试。"), cx);
            }
        }
    }

    pub(in crate::workbench) fn reset_history_transfer(&mut self) {
        self.history_transfer = None;
    }

    pub(in crate::workbench) fn cancel_invalidated_result_transfer(
        &mut self,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        if self.history_transfer.as_ref().is_some_and(|pending| {
            pending
                .result_target
                .as_ref()
                .is_some_and(|target| !target.active(self))
        }) {
            self.cancel_history_transfer(reason, cx);
        }
    }

    /// Returns false when the existing single-record or other-tool path should handle the action.
    pub(in crate::workbench) fn start_history_transfer(
        &mut self,
        intruder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tool != Tool::Proxy
            || self.proxy_page != ProxyPage::History
            || self.history.read(cx).delegate().selected_ids.len() <= 1
        {
            return false;
        }
        let entries = match self.history_transfer_entries(intruder, cx) {
            Ok(entries) => entries,
            Err(error) => {
                self.fail(error, cx);
                return true;
            }
        };
        self.begin_history_transfer(None, entries, intruder, window, cx);
        true
    }

    pub(in crate::workbench) fn start_intruder_result_transfer(
        &mut self,
        target: intruder::results_view::ResultTarget,
        entry: HistoryTransferEntry,
        intruder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !target.active(self) {
            self.fail("当前攻击或结果窗口状态已变化；未转送请求。", cx);
            return;
        }
        let owner = self.owner_window;
        if window.window_handle() != owner {
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let result = owner.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        view.start_intruder_result_transfer(target, entry, intruder, window, cx);
                    });
                });
                if result.is_err() {
                    let _ = weak.update(cx, |view, cx| {
                        view.fail("主工作窗口已不可用；未转送请求。", cx);
                    });
                }
            });
            return;
        }
        self.begin_history_transfer(Some(target), vec![entry], intruder, window, cx);
    }

    pub(in crate::workbench) fn begin_history_transfer(
        &mut self,
        result_target: Option<intruder::results_view::ResultTarget>,
        entries: Vec<HistoryTransferEntry>,
        intruder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_transfer.is_some() {
            self.fail("正在读取已选择的转送请求；重复操作未创建额外标签。", cx);
            return;
        }
        if !self.history_transfer_available(window) {
            self.fail("当前项目或后台状态不允许转送历史请求；原选择保留。", cx);
            return;
        }
        let Some(project) = self.project else {
            self.fail("尚未打开项目，未转送历史请求。", cx);
            return;
        };
        if !self.leave_http_notes(window, cx) || !self.flush_workspace(window, cx) {
            return;
        }
        let operation = Uuid::new_v4();
        self.history_transfer = Some(PendingHistoryTransfer {
            project,
            operation,
            result_target,
            entries,
            intruder,
            workspace_revision: None,
            cancel: self.shutdown.child_token(),
            deadline: cx.background_executor().now() + DEADLINE,
            _timer: None,
        });
        let timer = cx.background_executor().timer(DEADLINE);
        let owner = self.owner_window;
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.project == Some(project)
                        && view
                            .history_transfer
                            .as_ref()
                            .is_some_and(|pending| pending.operation == operation)
                    {
                        view.cancel_history_transfer("历史请求读取超时，已取消读取。", cx);
                    }
                });
            });
        });
        self.history_transfer.as_mut().unwrap()._timer = Some(task);
        self.flush_history_transfer(window, cx);
    }

    fn history_transfer_entries(
        &self,
        intruder: bool,
        cx: &App,
    ) -> Result<Vec<HistoryTransferEntry>, String> {
        let table = self.history.read(cx).delegate();
        let selected = table.selection_rows();
        if selected.len() != table.selected_ids.len() || selected.len() > 500 {
            return Err("历史选择已变化或超过 500 条限额；未转送部分选择。".into());
        }
        // MIO's multi-selection Intruder action opens only the first visible selected request.
        selected
            .into_iter()
            .take(if intruder { 1 } else { usize::MAX })
            .map(|row| match row {
                HistoryRow::Live(flow) if flow.revision > 0 => Ok(HistoryTransferEntry::Live {
                    flow_id: flow.id,
                    revision: flow.revision,
                    original: false,
                }),
                HistoryRow::Live(_) => Err("所选实时请求 revision 无效；未创建标签。".into()),
                HistoryRow::ExtProxy(row) => {
                    let message = row
                        .edited_request
                        .as_ref()
                        .or(row.request.as_ref())
                        .ok_or("所选归档条目没有可转送的请求；未创建部分标签。")?;
                    if message.source_id != row.item.source_id
                        || message.item_id != row.item.id
                        || message.raw_bytes == 0
                        || message.declared_version.is_none()
                    {
                        return Err("所选归档请求身份、正文或协议版本无效；原文保留。".into());
                    }
                    Ok(HistoryTransferEntry::External {
                        source_id: row.item.source_id,
                        item_id: row.item.id,
                        message_id: message.id,
                    })
                }
            })
            .collect()
    }

    pub(in crate::workbench) fn flush_history_transfer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = &self.history_transfer else {
            return;
        };
        if self.project != Some(pending.project)
            || !self.history_transfer_available(window)
            || pending
                .result_target
                .as_ref()
                .is_some_and(|target| !target.active(self))
            || pending.cancel.is_cancelled()
            || cx.background_executor().now() >= pending.deadline
        {
            self.cancel_history_transfer("项目状态已变化或读取已取消。", cx);
            return;
        }
        if pending.workspace_revision.is_some() || self.pending_workspace.is_some() {
            return;
        }
        let revision = self.workspace.revision;
        let command = Command::ReadHistoryTransfer {
            project: pending.project,
            operation_id: pending.operation,
            workspace_revision: revision,
            entries: pending.entries.clone(),
            cancel: pending.cancel.clone(),
        };
        if self.enqueue(command, cx) {
            self.history_transfer.as_mut().unwrap().workspace_revision = Some(revision);
            self.status = "正在读取所选历史请求，尚未创建标签或发送请求…".into();
            cx.notify();
        } else {
            self.cancel_history_transfer("历史请求读取命令未入队。", cx);
        }
    }

    pub(in crate::workbench) fn on_history_transfer(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Event::HistoryTransferRead {
            project,
            operation_id,
            workspace_revision,
            result,
        } = event
        else {
            return;
        };
        let Some(pending) = &self.history_transfer else {
            return;
        };
        if self.project != Some(project)
            || pending.project != project
            || pending.operation != operation_id
            || pending.workspace_revision != Some(workspace_revision)
        {
            return;
        }
        let pending = self.history_transfer.take().unwrap();
        if !self.history_transfer_available(window)
            || pending
                .result_target
                .as_ref()
                .is_some_and(|target| !target.active(self))
            || pending.cancel.is_cancelled()
            || cx.background_executor().now() >= pending.deadline
        {
            self.fail(
                "历史读取已结束；操作已取消或项目状态已变化，未创建标签。",
                cx,
            );
            return;
        }
        let tabs = match result {
            Ok(tabs) if valid_proposals(&pending.entries, &tabs) => tabs,
            Ok(_) => {
                self.fail("历史读取回执与所选请求不匹配；未创建标签。", cx);
                return;
            }
            Err(error) => {
                self.fail(format!("历史请求读取失败：{error}；未创建标签。"), cx);
                return;
            }
        };
        let intruder = pending.intruder;
        if !self.leave_http_notes(window, cx) {
            return;
        }
        if intruder {
            self.open_intruder_draft(tabs.into_iter().next().unwrap().1.draft, window, cx);
            return;
        }
        match self.capture_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        }
        let mut candidate = self.workspace.clone();
        let mut number = candidate
            .tabs
            .iter()
            .chain(&candidate.closed)
            .filter_map(|tab| tab.title.split(": ").next()?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        for (_, mut tab) in tabs {
            let Some(next) = number.checked_add(1) else {
                self.fail("Repeater 标签编号已耗尽；未创建部分标签。", cx);
                return;
            };
            number = next;
            let urlpath = crate::workbench::tabs::actions::extract_repeater_urlpath(
                Some(&tab.draft.raw),
                Some(&tab.draft.target),
            );
            tab.title = format!("{number}: {urlpath}");
            candidate.active = Some(tab.id);
            candidate.tabs.push(tab);
        }
        if let Err(error) = candidate.validate_structure() {
            self.fail(format!("未转送历史请求：{error}；未创建部分标签。"), cx);
            return;
        }
        self.workspace = candidate;
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.tool = Tool::Repeater;
        self.refresh_archive_response(window, cx);
        self.focus.focus(window, cx);
        self.status = "已按列表顺序载入所选请求，未发送；请核对连接目标。".into();
        if !self.save_workspace(cx) {
            self.fail(
                "转送请求已保留在工作台，但保存尚未确认；请保存或另存为。",
                cx,
            );
        }
        cx.notify();
    }
}

fn valid_proposals(entries: &[HistoryTransferEntry], tabs: &[(HistoryEntryId, WorkTab)]) -> bool {
    let mut ids = HashSet::new();
    entries.len() == tabs.len()
        && !tabs.is_empty()
        && entries.iter().zip(tabs).all(|(entry, (id, tab))| {
            entry.id() == *id
                && !tab.id.is_nil()
                && ids.insert(tab.id)
                && tab.draft.revision == 1
                && (!matches!(entry, HistoryTransferEntry::External { .. })
                    || tab.draft.target.is_empty()
                    || validate_target_origin(&tab.draft.target).is_ok())
                && tab.group.is_none()
                && tab.last_flow.is_none()
                && tab.archive_response.is_none()
                && tab.history.is_none()
        })
}

#[cfg(test)]
mod tests;
