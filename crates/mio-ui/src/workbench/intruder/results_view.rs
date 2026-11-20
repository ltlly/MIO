use gpui_kit::component::{
    input::{Copy, Cut, EditorState, Paste},
    native_menu::NativeMenu,
    table::Column,
};
use gpui_kit::{AnyWindowHandle, EntityId, WeakEntity};
use mio_core::{
    HistoryTransferEntry, IntruderPayloadPreview, IntruderResultOutcome, IntruderResultViewRow,
    IntruderRetention, IntruderRunState,
};

use super::*;

mod pool_table;
mod result_table;
mod results_pane;
mod window_render;
mod window_state;
pub(super) use window_state::IntruderResultsWindow;
use window_state::result_filter_label;

#[cfg(test)]
#[path = "results_transfer_tests.rs"]
mod transfer_tests;

#[derive(Clone, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(super) struct TransferIntruderResult {
    project: ProjectKey,
    view_id: Uuid,
    run_id: Uuid,
    owner: AnyWindowHandle,
    editor_id: EntityId,
    entry: Result<HistoryTransferEntry, String>,
    intruder: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(super) enum ResultDetailActionKind {
    Decoder,
    ComparerRequest,
    ComparerResponse,
    Organiser,
    RecordIssue,
    SendToMioAi,
    CopyUrl,
    CopyCurl,
    GenerateCsrfPoc,
    SaveItem,
}

#[derive(Clone, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(super) struct ResultDetailAction {
    pub(super) project: ProjectKey,
    pub(super) view_id: Uuid,
    pub(super) run_id: Uuid,
    pub(super) owner: AnyWindowHandle,
    pub(super) editor_id: EntityId,
    pub(super) kind: ResultDetailActionKind,
}

#[derive(Clone)]
pub(in crate::workbench) struct ResultTarget {
    pub(in crate::workbench) workbench: WeakEntity<Workbench>,
    pub(in crate::workbench) project: ProjectKey,
    pub(in crate::workbench) view_id: Uuid,
    pub(in crate::workbench) run_id: Uuid,
    pub(in crate::workbench) owner: AnyWindowHandle,
    pub(in crate::workbench) editor_id: EntityId,
}

impl ResultTarget {
    pub(in crate::workbench) fn owns(&self, workbench: &Workbench) -> bool {
        workbench.owns_intruder_result_window(
            self.view_id,
            self.project,
            self.owner,
            self.editor_id,
        ) && workbench.intruder.results.views[&self.view_id].run_id == self.run_id
    }

    pub(in crate::workbench) fn active(&self, workbench: &Workbench) -> bool {
        self.owns(workbench)
            && !workbench.intruder_result_close_pending(self.run_id)
            && !workbench.intruder.results.views[&self.view_id].discarded
    }

    pub(super) fn view<'a>(&self, cx: &'a App) -> Option<&'a results::ResultView> {
        let workbench = self.workbench.upgrade()?.read(cx);
        self.owns(workbench)
            .then(|| &workbench.intruder.results.views[&self.view_id])
    }

    pub(super) fn capture_transfer(
        &self,
        ordinal: Option<u64>,
        window: &Window,
        cx: &App,
    ) -> Result<HistoryTransferEntry, String> {
        if window.window_handle() != self.owner {
            return Err("结果窗口归属已变化；未转送请求。".into());
        }
        let owner = self
            .workbench
            .upgrade()
            .ok_or("结果窗口所属工作台已关闭；未转送请求。")?;
        let workbench = owner.read(cx);
        if !self.owns(workbench)
            || !workbench.backend_available()
            || workbench.shutdown_requested
            || workbench.transition.is_some()
            || workbench.quit_attempt.is_some()
            || workbench.file_picker_pending()
            || workbench.intruder_result_close_pending(self.run_id)
        {
            return Err("当前攻击或结果窗口状态已变化；未转送请求。".into());
        }
        let view = &workbench.intruder.results.views[&self.view_id];
        if view.discarded {
            return Err("此攻击结果已丢弃；未转送请求。".into());
        }
        let ordinal = ordinal
            .or(view.selected)
            .ok_or("没有选中的 Intruder 结果请求。")?;
        if view.summary.as_ref().is_some_and(|run| {
            run.baseline
                .is_some_and(|baseline| !baseline.completed && baseline.ordinal == ordinal)
        }) {
            return Err("基准请求尚未完成；未转送请求。".into());
        }
        let row = view
            .row(ordinal)
            .ok_or("所选 Intruder 结果尚未读取完成；未转送请求。")?;
        match &row.outcome {
            IntruderResultOutcome::Flow(flow) if !flow.id.is_nil() && flow.revision > 0 => {
                Ok(HistoryTransferEntry::Live {
                    flow_id: flow.id,
                    revision: flow.revision,
                    original: false,
                })
            }
            IntruderResultOutcome::Flow(_) => {
                Err("所选 Intruder 结果请求身份或 revision 无效；未转送请求。".into())
            }
            IntruderResultOutcome::Rejected(_) => {
                Err("未发送的 Intruder 结果没有可转送的请求。".into())
            }
        }
    }

    pub(super) fn transfer_result(
        &self,
        entry: HistoryTransferEntry,
        intruder: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if window.window_handle() != self.owner {
            return;
        }
        let _ = self.workbench.update(cx, |workbench, cx| {
            workbench.start_intruder_result_transfer(self.clone(), entry, intruder, window, cx);
        });
    }

    pub(super) fn transfer_selected(&self, intruder: bool, window: &mut Window, cx: &mut App) {
        let multi = !intruder && self.view(cx).is_some_and(|v| v.selected_ordinals.len() > 1);
        if multi {
            match self.capture_transfers(None, window, cx) {
                Ok(entries) => self.transfer_results(entries, intruder, window, cx),
                Err(error) => self.update(window, cx, |workbench, _, cx| workbench.fail(error, cx)),
            }
            return;
        }
        match self.capture_transfer(None, window, cx) {
            Ok(entry) => self.transfer_result(entry, intruder, window, cx),
            Err(error) => self.update(window, cx, |workbench, _, cx| workbench.fail(error, cx)),
        }
    }

    pub(super) fn capture_transfers(
        &self,
        ordinal: Option<u64>,
        window: &Window,
        cx: &App,
    ) -> Result<Vec<HistoryTransferEntry>, String> {
        let ordinals: Vec<u64> = self
            .view(cx)
            .and_then(|view| {
                let target_ord = ordinal.or(view.selected)?;
                if view.selected_ordinals.len() > 1 && view.selected_ordinals.contains(&target_ord)
                {
                    Some(view.selected_ordinals.iter().copied().collect())
                } else {
                    Some(vec![target_ord])
                }
            })
            .unwrap_or_default();
        if ordinals.len() <= 1 {
            return self
                .capture_transfer(ordinal, window, cx)
                .map(|entry| vec![entry]);
        }
        let mut entries = Vec::new();
        let mut last_err = None;
        for ord in ordinals {
            match self.capture_transfer(Some(ord), window, cx) {
                Ok(entry) => entries.push(entry),
                Err(err) => last_err = Some(err),
            }
        }
        if entries.is_empty() {
            Err(last_err.unwrap_or_else(|| "没有选中的 Intruder 结果请求。".into()))
        } else {
            Ok(entries)
        }
    }

    pub(super) fn transfer_results(
        &self,
        entries: Vec<HistoryTransferEntry>,
        intruder: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if window.window_handle() != self.owner || entries.is_empty() {
            return;
        }
        if entries.len() == 1 {
            self.transfer_result(entries.into_iter().next().unwrap(), intruder, window, cx);
            return;
        }
        let target = self.clone();
        let _ = self.workbench.update(cx, |workbench, cx| {
            if !target.active(workbench) {
                workbench.fail("当前攻击或结果窗口状态已变化；未转送请求。", cx);
                return;
            }
            let owner = workbench.owner_window;
            if window.window_handle() != owner {
                let weak = cx.weak_entity();
                cx.defer(move |cx| {
                    let result = owner.update(cx, |_, main_win, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.begin_history_transfer(
                                Some(target),
                                entries,
                                intruder,
                                main_win,
                                cx,
                            );
                        });
                    });
                    if result.is_err() {
                        let _ = weak.update(cx, |view, cx| {
                            view.fail("主工作窗口已不可用；未转送请求。", cx);
                        });
                    }
                });
            } else {
                workbench.begin_history_transfer(Some(target), entries, intruder, window, cx);
            }
        });
    }

    pub(super) fn editor_context_menu(
        &self,
        input: Entity<EditorState>,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        let target = self.clone();
        move |menu, window, cx| {
            input.update(cx, |state, cx| state.focus(window, cx));
            let entry = target.capture_transfer(None, window, cx);
            let unavailable = entry.is_err();
            let has_detail = target
                .view(cx)
                .is_some_and(|v| !v.discarded && v.detail.is_some());
            let has_resp = target
                .view(cx)
                .and_then(|v| v.detail.as_ref())
                .is_some_and(|d| d.response.is_some());
            let selected = !input.read(cx).selected_range().is_empty();
            let action = |intruder| TransferIntruderResult {
                project: target.project,
                view_id: target.view_id,
                run_id: target.run_id,
                owner: target.owner,
                editor_id: target.editor_id,
                entry: entry.clone(),
                intruder,
            };
            let detail_action = |kind| ResultDetailAction {
                project: target.project,
                view_id: target.view_id,
                run_id: target.run_id,
                owner: target.owner,
                editor_id: target.editor_id,
                kind,
            };
            menu.menu_with_disabled("发送到 Intruder", unavailable, Box::new(action(true)))
                .menu_with_disabled("发送到 Repeater", unavailable, Box::new(action(false)))
                .menu_with_disabled(
                    "发送到 Organiser",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::Organiser)),
                )
                .menu_with_disabled(
                    "发送到 Decoder",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::Decoder)),
                )
                .menu_with_disabled(
                    "发送到 Comparer (请求)",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::ComparerRequest)),
                )
                .menu_with_disabled(
                    "发送到 Comparer (响应)",
                    unavailable || !has_resp,
                    Box::new(detail_action(ResultDetailActionKind::ComparerResponse)),
                )
                .menu_with_disabled(
                    "发送到 MioAT",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::SendToMioAi)),
                )
                .separator()
                .menu_with_disabled(
                    "记录问题",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::RecordIssue)),
                )
                .menu_with_disabled(
                    "生成 CSRF PoC",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::GenerateCsrfPoc)),
                )
                .separator()
                .menu_with_disabled(
                    "复制 URL",
                    unavailable,
                    Box::new(detail_action(ResultDetailActionKind::CopyUrl)),
                )
                .menu_with_disabled(
                    "复制为 curl 命令 (bash)",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::CopyCurl)),
                )
                .menu_with_disabled(
                    "保存条目",
                    unavailable || !has_detail,
                    Box::new(detail_action(ResultDetailActionKind::SaveItem)),
                )
                .separator()
                .menu_with_disabled("剪切", true, Box::new(Cut))
                .menu_with_disabled("复制", !selected, Box::new(Copy))
                .menu_with_disabled("粘贴", true, Box::new(Paste))
        }
    }

    pub(super) fn update(
        &self,
        window: &mut Window,
        cx: &mut App,
        action: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>),
    ) {
        if window.window_handle() != self.owner {
            return;
        }
        let _ = self.workbench.update(cx, |workbench, cx| {
            if self.owns(workbench) {
                action(workbench, window, cx);
            }
        });
    }
}

pub(super) struct ResultTable {
    pub(super) target: ResultTarget,
    pub(super) columns: Vec<Column>,
}

pub(super) struct ResultPoolTable {
    pub(super) target: ResultTarget,
    pub(super) columns: Vec<Column>,
}

pub(super) fn payload_preview(payload: &IntruderPayloadPreview) -> String {
    if payload.raw_bytes == 0 {
        return "（空 payload）".into();
    }
    let max_chars = theme::table_cell_preview_chars();
    let complete = payload.prefix.len() as u64 == payload.raw_bytes;
    let text = match std::str::from_utf8(&payload.prefix) {
        Ok(text) => Some(text),
        Err(error) if !complete && error.error_len().is_none() => {
            std::str::from_utf8(&payload.prefix[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
    .filter(|text| !text.chars().any(char::is_control));
    let mut label = if let Some(text) = text {
        text.chars().take(max_chars).collect::<String>()
    } else {
        format!(
            "Hex: {}",
            editing::hex_encode(&payload.prefix[..payload.prefix.len().min(48)])
        )
    };
    let truncated = !complete
        || text.is_some_and(|text| text.chars().count() > max_chars)
        || text.is_none() && payload.prefix.len() > 48;
    if truncated {
        label.push_str(&format!("…（{} 字节）", payload.raw_bytes));
    }
    label
}

pub(super) fn payload_label(row: &IntruderResultViewRow) -> String {
    row.payloads
        .iter()
        .map(payload_preview)
        .collect::<Vec<_>>()
        .join(" | ")
}

pub(super) fn run_state(state: &IntruderRunState) -> &str {
    match state {
        IntruderRunState::Running => "运行中",
        IntruderRunState::Paused => "已暂停",
        IntruderRunState::Stopping => "正在停止",
        IntruderRunState::Stopped => "已停止",
        IntruderRunState::Completed => "已完成",
        IntruderRunState::Interrupted => "已中断",
        IntruderRunState::Failed(error) => error,
    }
}

pub(in crate::workbench) fn run_progress(run: &IntruderRunSummary) -> String {
    let Some((completed, total)) = run.payload_progress() else {
        return "运行计数未确认".into();
    };
    format!(
        "{}  {completed}/{total}{}",
        run_state(&run.state),
        if run.baseline.is_some_and(|baseline| !baseline.completed) && completed == total {
            " · 等待基准请求"
        } else {
            ""
        }
    )
}

pub(super) fn retention_note(
    retention: IntruderRetention,
    persisted: Option<bool>,
) -> Option<&'static str> {
    match retention {
        IntruderRetention::Session => {
            Some("Session · 仅当前会话内存；普通保存/另存为不会写入项目。")
        }
        IntruderRetention::Project if persisted == Some(false) => {
            Some("Project · 结果尚未保存（待写盘）；保存项目仅重试写盘，不会重发请求。")
        }
        IntruderRetention::Project => None,
    }
}
