use std::collections::{HashMap, HashSet};

use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    button::ButtonVariants,
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
    scroll::{Scrollbar, ScrollbarMode},
    switch::Switch,
};
use gpui_kit::{
    AnyElement, BoxShadow, ClipboardItem, Context, DragMoveEvent, Entity, EntityId, FontWeight,
    Hsla, MouseButton, MouseDownEvent, Render, Subscription, UniformListScrollHandle, WeakEntity,
    Window, hsla, point,
};
use mio_core::{
    AuditCommand, AuditEvent, AuditStateSnapshot, AuditTaskMode, AuditTaskState, AuditTaskSummary,
    Command, Draft, FlowId, FlowRecord, HttpVersion, ProjectKey, Uuid, WorkTab,
    audit_engine::{
        AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence, render_audit_report_html,
        render_audit_report_json,
    },
    parse_repeater_request, request_to_raw, response_to_raw,
    target_issues::IssueSeverity,
};
use tokio_util::sync::CancellationToken;

use super::*;

mod events;
mod issue_activity;
mod issue_batch;
mod issue_detail;
mod issue_menu;
mod issue_triage;
mod lifecycle;
mod reports;
mod scan_modals;
mod selection;
mod task_cards;
#[cfg(test)]
mod tests;
mod theme_helpers;
mod workspace;
mod workspace_audit_items;
mod workspace_logs;
mod workspace_summary;

pub(in crate::workbench) use theme_helpers::*;
use issue_menu::{ISSUE_HIGHLIGHT_LABELS, format_finding_context_text};

pub(in crate::workbench) const DEFAULT_SCAN_URL: &str = "http://127.0.0.1:8080/";
pub(in crate::workbench) const DEFAULT_SCAN_RAW: &[u8] =
    b"GET / HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n";

pub(in crate::workbench) const MAX_SCAN_URL_BYTES: usize = 4096;

pub(in crate::workbench) const MAX_SCAN_TITLE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum AuditEditorSlot {
    ScanRequest,
    IssueRequest,
    IssueResponse,
}

pub(in crate::workbench) fn extract_highlight_needle(
    raw: &[u8],
    span: EvidenceSpan,
    fallback: &str,
) -> String {
    let end = span.offset.saturating_add(span.length);
    if span.length > 0
        && end <= raw.len()
        && let Ok(text) = std::str::from_utf8(&raw[span.offset..end])
    {
        let first_line = text.lines().next().unwrap_or("").trim();
        if !first_line.is_empty() {
            return first_line.chars().take(128).collect();
        }
    }
    fallback
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(128)
        .collect()
}

pub(in crate::workbench) fn set_editor_plain_query(
    editor: &mut MessageEditor,
    query: String,
    window: &mut Window,
    cx: &mut App,
) {
    if editor.search.update(cx, |state, cx| {
        state.marked_text_range(window, cx).is_some()
    }) {
        return;
    }
    editor.set_regex(false);
    editor
        .search
        .update(cx, |state, cx| state.set_value(query, window, cx));
    let _ = editor.update_search(Some(true), window, cx);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum IssueDetailTab {
    #[default]
    Advisory,
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum DashboardTaskTab {
    #[default]
    Summary,
    AuditItems,
    Issues,
    EventLog,
    Logger,
    AuditLog,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum DashboardBuiltinTask {
    #[default]
    LivePassiveAudit,
    LivePassiveCrawl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum DashboardTaskFilter {
    #[default]
    All,
    Running,
    Paused,
    LiveTasks,
    ScanTasks,
    IntruderTasks,
}

impl DashboardTaskFilter {
    pub(in crate::workbench) fn label(self) -> &'static str {
        match self {
            Self::All => "全部任务",
            Self::Running => "运行中 / 捕获中",
            Self::Paused => "已暂停",
            Self::LiveTasks => "实时任务",
            Self::ScanTasks => "扫描任务",
            Self::IntruderTasks => "Intruder 任务",
        }
    }

    pub(in crate::workbench) fn short_label(self) -> &'static str {
        match self {
            Self::All => "筛选",
            Self::Running => "运行中",
            Self::Paused => "已暂停",
            Self::LiveTasks => "实时任务",
            Self::ScanTasks => "扫描任务",
            Self::IntruderTasks => "Intruder",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum LiveTaskTrafficScope {
    #[default]
    AllTraffic,
    SuiteScopeOnly,
}

impl LiveTaskTrafficScope {
    pub(in crate::workbench) fn label(self) -> &'static str {
        match self {
            Self::AllTraffic => "Proxy (全流量)",
            Self::SuiteScopeOnly => "Proxy (仅限套件范围)",
        }
    }

    pub(in crate::workbench) fn card_suffix(self) -> &'static str {
        match self {
            Self::AllTraffic => "Proxy 全流量",
            Self::SuiteScopeOnly => "仅限套件范围",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum AuditReportFormat {
    Html,
    Json,
    Xml,
}

impl AuditReportFormat {
    pub(in crate::workbench) fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::Xml => "xml",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum AuditReportScope {
    All,
    SingleFinding(Uuid),
    Task(Uuid),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum DashboardIssueSortColumn {
    Severity,
    IssueType,
    Host,
    Url,
    Confidence,
}

pub(in crate::workbench) const DASHBOARD_ISSUE_COL_COUNT: usize = 5;

pub(in crate::workbench) const DASHBOARD_ISSUE_DEFAULT_WIDTHS: [f32; DASHBOARD_ISSUE_COL_COUNT] = [
    96., 200., 120., 280., 96.,
];

pub(in crate::workbench) const DASHBOARD_ISSUE_COL_NAMES: [&str; DASHBOARD_ISSUE_COL_COUNT] = [
    "严重度",
    "漏洞名称",
    "主机 / 参数",
    "URL",
    "置信度",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum DashboardBatchTransferKind {
    Repeater,
    Intruder,
    Organiser,
}

#[derive(Clone, Debug)]
pub(in crate::workbench) struct PendingIssueBatchTransfer {
    pub(in crate::workbench) kind: DashboardBatchTransferKind,
    pub(in crate::workbench) flow_ids: Vec<FlowId>,
    pub(in crate::workbench) pending_revisions: HashMap<u64, FlowId>,
}

#[derive(Clone, Copy)]
pub(in crate::workbench) struct ResizeDashboardIssueCol {
    pub(in crate::workbench) entity_id: EntityId,
    pub(in crate::workbench) col_ix: usize,
}

impl Render for ResizeDashboardIssueCol {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

pub(super) struct DashboardViewContext<'a> {
    pub(super) view: WeakEntity<AuditDashboardView>,
    pub(super) view_entity_id: EntityId,
    pub(super) summary_sort_asc: bool,
    pub(super) summary_table_scroll: &'a UniformListScrollHandle,
    pub(super) summary_pane_scroll: &'a ScrollHandle,
    pub(super) expanded_issue_groups: &'a HashSet<String>,
}

pub(in crate::workbench) struct AuditDashboardPane {
    pub(in crate::workbench) loaded: bool,
    pub(in crate::workbench) loading: bool,
    pub(in crate::workbench) query_revision: u64,
    pub(in crate::workbench) snapshot: AuditStateSnapshot,
    pub(in crate::workbench) state_error: Option<String>,
    pub(in crate::workbench) new_scan_open: bool,
    pub(in crate::workbench) new_live_task_open: bool,
    pub(in crate::workbench) live_task_draft_passive_audit: bool,
    pub(in crate::workbench) live_task_draft_passive_crawl: bool,
    pub(in crate::workbench) live_task_draft_scope: LiveTaskTrafficScope,
    pub(in crate::workbench) live_task_scope: LiveTaskTrafficScope,
    pub(in crate::workbench) scan_title_input: Entity<InputState>,
    pub(in crate::workbench) scan_url_input: Entity<InputState>,
    pub(in crate::workbench) scan_request_editor: MessageEditor,
    pub(in crate::workbench) scan_version: HttpVersion,
    pub(in crate::workbench) scan_mode: AuditTaskMode,
    pub(in crate::workbench) crawl_same_origin: bool,
    pub(in crate::workbench) scan_error: Option<String>,
    pub(in crate::workbench) selected_audit_task_id: Option<Uuid>,
    pub(in crate::workbench) selected_builtin_task: DashboardBuiltinTask,
    pub(in crate::workbench) active_task_tab: DashboardTaskTab,
    pub(in crate::workbench) task_filter: DashboardTaskFilter,
    pub(in crate::workbench) task_search_input: Entity<InputState>,
    pub(in crate::workbench) severity_filter: Option<IssueSeverity>,
    pub(in crate::workbench) issue_filter_input: Entity<InputState>,
    pub(in crate::workbench) selected_finding_id: Option<Uuid>,
    pub(in crate::workbench) selected_finding_ids: HashSet<Uuid>,
    pub(in crate::workbench) finding_selection_anchor: Option<Uuid>,
    pub(in crate::workbench) issue_sort: Option<(DashboardIssueSortColumn, bool)>,
    pub(in crate::workbench) issue_col_widths: [f32; DASHBOARD_ISSUE_COL_COUNT],
    pub(in crate::workbench) issue_col_visible: [bool; DASHBOARD_ISSUE_COL_COUNT],
    pub(in crate::workbench) pending_batch_transfer: Option<PendingIssueBatchTransfer>,
    pub(in crate::workbench) original_triage: HashMap<Uuid, (IssueSeverity, IssueConfidence)>,
    pub(in crate::workbench) false_positive_findings: HashSet<Uuid>,
    pub(in crate::workbench) verified_confidence_findings: HashSet<Uuid>,
    pub(in crate::workbench) finding_highlights: HashMap<Uuid, u8>,
    pub(in crate::workbench) finding_notes: HashMap<Uuid, String>,
    pub(in crate::workbench) editing_note_finding_id: Option<Uuid>,
    pub(in crate::workbench) finding_note_input: Entity<InputState>,
    pub(in crate::workbench) task_finding_ids: HashMap<Uuid, HashSet<Uuid>>,
    pub(in crate::workbench) dismissed_audit_items: HashSet<HistoryEntryId>,
    pub(in crate::workbench) pending_flow_read: Option<(ProjectKey, u64, FlowId)>,
    pub(in crate::workbench) loaded_flow: Option<Box<FlowRecord>>,
    pub(in crate::workbench) flow_cache: HashMap<FlowId, FlowRecord>,
    pub(in crate::workbench) flow_error: Option<String>,
    pub(in crate::workbench) detail_tab: IssueDetailTab,
    pub(in crate::workbench) issue_request_editor: MessageEditor,
    pub(in crate::workbench) issue_response_editor: MessageEditor,
    pub(in crate::workbench) running_cancels: HashMap<Uuid, (ProjectKey, CancellationToken)>,
}

impl AuditDashboardPane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let scan_title_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("可选扫描名称（默认按目标生成）"));
        let scan_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_SCAN_URL)
                .placeholder("http://127.0.0.1:8080/")
        });
        let mut scan_request_editor = MessageEditor::new(window, cx);
        scan_request_editor.load(DEFAULT_SCAN_RAW, window, cx);
        let task_search_input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let issue_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("按标题、URL、参数或证据筛选"));
        let finding_note_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("输入问题备注，按 Enter 或点击保存…"));
        let issue_request_editor = MessageEditor::new(window, cx);
        let issue_response_editor = MessageEditor::new(window, cx);

        Self {
            loaded: false,
            loading: false,
            query_revision: 0,
            snapshot: AuditStateSnapshot::default(),
            state_error: None,
            new_scan_open: false,
            new_live_task_open: false,
            live_task_draft_passive_audit: true,
            live_task_draft_passive_crawl: true,
            live_task_draft_scope: LiveTaskTrafficScope::AllTraffic,
            live_task_scope: LiveTaskTrafficScope::AllTraffic,
            scan_title_input,
            scan_url_input,
            scan_request_editor,
            scan_version: HttpVersion::Http11,
            scan_mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: false,
            scan_error: None,
            selected_audit_task_id: None,
            selected_builtin_task: DashboardBuiltinTask::LivePassiveAudit,
            active_task_tab: DashboardTaskTab::Summary,
            task_filter: DashboardTaskFilter::All,
            task_search_input,
            severity_filter: None,
            issue_filter_input,
            selected_finding_id: None,
            selected_finding_ids: HashSet::new(),
            finding_selection_anchor: None,
            issue_sort: None,
            issue_col_widths: DASHBOARD_ISSUE_DEFAULT_WIDTHS,
            issue_col_visible: [true; DASHBOARD_ISSUE_COL_COUNT],
            pending_batch_transfer: None,
            original_triage: HashMap::new(),
            false_positive_findings: HashSet::new(),
            verified_confidence_findings: HashSet::new(),
            finding_highlights: HashMap::new(),
            finding_notes: HashMap::new(),
            editing_note_finding_id: None,
            finding_note_input,
            task_finding_ids: HashMap::new(),
            dismissed_audit_items: HashSet::new(),
            pending_flow_read: None,
            loaded_flow: None,
            flow_cache: HashMap::new(),
            flow_error: None,
            detail_tab: IssueDetailTab::Advisory,
            issue_request_editor,
            issue_response_editor,
            running_cancels: HashMap::new(),
        }
    }
}

pub(in crate::workbench) struct AuditDashboardView {
    workbench: WeakEntity<Workbench>,
    summary_sort_asc: bool,
    summary_table_scroll: UniformListScrollHandle,
    summary_pane_scroll: ScrollHandle,
    issue_col_resize_origin: Option<(usize, f32, f32)>,
    expanded_issue_groups: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl AuditDashboardView {
    pub(in crate::workbench) fn new(
        workbench: WeakEntity<Workbench>,
        pane: &AuditDashboardPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        for editor in [&pane.scan_request_editor.raw, &pane.scan_request_editor.hex] {
            let mut composing = false;
            subscriptions.push(cx.observe_in(editor, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(editor, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        for input in [
            &pane.scan_title_input,
            &pane.scan_url_input,
            &pane.scan_request_editor.search,
            &pane.task_search_input,
            &pane.issue_filter_input,
            &pane.finding_note_input,
            &pane.issue_request_editor.search,
            &pane.issue_response_editor.search,
        ] {
            let mut composing = false;
            subscriptions.push(cx.observe_in(input, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        let wb_enter = workbench.clone();
        subscriptions.push(cx.subscribe_in(
            &pane.finding_note_input,
            window,
            move |_, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let _ = wb_enter.update(cx, |this, cx| {
                        this.save_audit_finding_note(window, cx);
                    });
                }
            },
        ));

        Self {
            workbench,
            summary_sort_asc: false,
            summary_table_scroll: UniformListScrollHandle::new(),
            summary_pane_scroll: ScrollHandle::new(),
            issue_col_resize_origin: None,
            expanded_issue_groups: HashSet::new(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.summary_sort_asc = false;
        self.issue_col_resize_origin = None;
        self.expanded_issue_groups.clear();
        cx.notify();
    }

    pub(super) fn clear_issue_column_resize(&mut self, cx: &mut Context<Self>) {
        self.issue_col_resize_origin = None;
        cx.notify();
    }

    pub(super) fn toggle_summary_sort(&mut self, cx: &mut Context<Self>) {
        self.summary_sort_asc = !self.summary_sort_asc;
        cx.notify();
    }

    pub(super) fn toggle_issue_group(&mut self, group_key: &str, cx: &mut Context<Self>) {
        if !self.expanded_issue_groups.remove(group_key) {
            self.expanded_issue_groups.insert(group_key.to_owned());
        }
        cx.notify();
    }

    pub(super) fn begin_issue_column_resize(
        &mut self,
        col_ix: usize,
        start_x: f32,
        start_w: f32,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        self.issue_col_resize_origin = Some((col_ix, start_x, start_w));
    }

    pub(super) fn update_issue_column_resize(
        &mut self,
        col_ix: usize,
        current_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let Some((drag_col, start_x, start_w)) = self.issue_col_resize_origin else {
            return;
        };
        if drag_col != col_ix {
            return;
        }
        let scale = theme::ui_scale().max(0.75);
        let delta_unscaled = (current_x - start_x) / scale;
        let next_w = (start_w + delta_unscaled).round().clamp(56.0, 560.0);
        let Some(wb_entity) = self.workbench.upgrade() else {
            return;
        };
        let changed = wb_entity.update(cx, |wb, _| {
            if (wb.audit_dashboard.issue_col_widths[col_ix] - next_w).abs() >= 0.5 {
                wb.audit_dashboard.issue_col_widths[col_ix] = next_w;
                true
            } else {
                false
            }
        });
        if changed {
            cx.notify();
        }
    }
}

impl Render for AuditDashboardView {
    #[inline(never)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(wb_entity) = self.workbench.upgrade() else {
            return div()
                .id("intruder-dashboard")
                .size_full()
                .into_any_element();
        };
        // Register dependency on Workbench so gpui-fast splices re-render
        // this view whenever Workbench state changes.
        let _ = wb_entity.read(cx);
        let view_ctx = DashboardViewContext {
            view: cx.weak_entity(),
            view_entity_id: cx.entity_id(),
            summary_sort_asc: self.summary_sort_asc,
            summary_table_scroll: &self.summary_table_scroll,
            summary_pane_scroll: &self.summary_pane_scroll,
            expanded_issue_groups: &self.expanded_issue_groups,
        };
        let (tasks, detail, new_scan_panel) = wb_entity.update(cx, |wb, cx| {
            wb.activate_pro_dashboard(cx);
            let tasks_header = wb.render_dashboard_tasks_header(window, cx);
            let audit_cards = wb.render_audit_task_cards(window, cx);
            let tasks = wb.render_intruder_dashboard_tasks_column(
                tasks_header,
                audit_cards,
                window,
                cx,
            );
            let detail =
                if let Some(intruder_detail) = wb.render_intruder_dashboard_selection_detail(cx) {
                    intruder_detail
                } else {
                    div()
                        .id("dashboard-detail")
                        .test_support()
                        .size_full()
                        .min_w_0()
                        .h_full()
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .child(wb.render_audit_task_workspace(&view_ctx, window, cx))
                        .into_any_element()
                };
            let new_scan_panel = wb.render_new_scan_panel(cx);
            (tasks, detail, new_scan_panel)
        });

        let main_split = h_resizable("dashboard-main-hsplit")
            .child(
                resizable_panel()
                    .size(px(326.))
                    .size_range(px(270.)..px(560.))
                    .child(tasks),
            )
            .child(
                resizable_panel()
                    .size_range(px(360.)..px(10000.))
                    .child(detail),
            )
            .into_any_element();

        div()
            .id("intruder-dashboard")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .text_size(theme::ui_font_size())
            .when_some(new_scan_panel, |this, panel| this.child(panel))
            .child(div().flex_1().min_h_0().child(main_split))
            .into_any_element()
    }
}
