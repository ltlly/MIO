use super::*;

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
    pub(in crate::workbench) entity_id: gpui_kit::EntityId,
    pub(in crate::workbench) col_ix: usize,
}

impl gpui_kit::Render for ResizeDashboardIssueCol {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
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
    pub(in crate::workbench) summary_sort_asc: bool,
    pub(in crate::workbench) summary_table_scroll: UniformListScrollHandle,
    pub(in crate::workbench) summary_pane_scroll: ScrollHandle,
    pub(in crate::workbench) severity_filter: Option<IssueSeverity>,
    pub(in crate::workbench) issue_filter_input: Entity<InputState>,
    pub(in crate::workbench) selected_finding_id: Option<Uuid>,
    pub(in crate::workbench) selected_finding_ids: HashSet<Uuid>,
    pub(in crate::workbench) finding_selection_anchor: Option<Uuid>,
    pub(in crate::workbench) issue_sort: Option<(DashboardIssueSortColumn, bool)>,
    pub(in crate::workbench) issue_col_widths: [f32; DASHBOARD_ISSUE_COL_COUNT],
    pub(in crate::workbench) issue_col_visible: [bool; DASHBOARD_ISSUE_COL_COUNT],
    pub(in crate::workbench) issue_col_resize_origin: Option<(usize, f32, f32)>,
    pub(in crate::workbench) pending_batch_transfer: Option<PendingIssueBatchTransfer>,
    pub(in crate::workbench) original_triage: HashMap<Uuid, (IssueSeverity, IssueConfidence)>,
    pub(in crate::workbench) false_positive_findings: HashSet<Uuid>,
    pub(in crate::workbench) verified_confidence_findings: HashSet<Uuid>,
    pub(in crate::workbench) finding_highlights: HashMap<Uuid, u8>,
    pub(in crate::workbench) finding_notes: HashMap<Uuid, String>,
    pub(in crate::workbench) editing_note_finding_id: Option<Uuid>,
    pub(in crate::workbench) finding_note_input: Entity<InputState>,
    pub(in crate::workbench) expanded_issue_groups: HashSet<String>,
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
    pub(in crate::workbench) _subscriptions: Vec<Subscription>,
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

        let mut subscriptions = Vec::new();
        for editor in [
            &scan_request_editor.raw,
            &scan_request_editor.hex,
        ] {
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
            &scan_title_input,
            &scan_url_input,
            &scan_request_editor.search,
            &task_search_input,
            &issue_filter_input,
            &finding_note_input,
            &issue_request_editor.search,
            &issue_response_editor.search,
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
        subscriptions.push(cx.subscribe_in(
            &finding_note_input,
            window,
            |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_audit_finding_note(window, cx);
                }
            },
        ));

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
            summary_sort_asc: false,
            summary_table_scroll: UniformListScrollHandle::new(),
            summary_pane_scroll: ScrollHandle::new(),
            severity_filter: None,
            issue_filter_input,
            selected_finding_id: None,
            selected_finding_ids: HashSet::new(),
            finding_selection_anchor: None,
            issue_sort: None,
            issue_col_widths: DASHBOARD_ISSUE_DEFAULT_WIDTHS,
            issue_col_visible: [true; DASHBOARD_ISSUE_COL_COUNT],
            issue_col_resize_origin: None,
            pending_batch_transfer: None,
            original_triage: HashMap::new(),
            false_positive_findings: HashSet::new(),
            verified_confidence_findings: HashSet::new(),
            finding_highlights: HashMap::new(),
            finding_notes: HashMap::new(),
            editing_note_finding_id: None,
            finding_note_input,
            expanded_issue_groups: HashSet::new(),
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
            _subscriptions: subscriptions,
        }
    }
}

fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            other => out.push(other),
        }
    }
    out
}

pub(in crate::workbench) fn render_audit_report_xml(
    snapshot: &AuditStateSnapshot,
    false_positives: &HashSet<Uuid>,
    verified: &HashSet<Uuid>,
    notes: &HashMap<Uuid, String>,
) -> String {
    use std::fmt::Write as _;
    let mut findings: Vec<&AuditFinding> = snapshot.findings.iter().collect();
    findings.sort_by(|a, b| {
        b.severity()
            .rank()
            .cmp(&a.severity().rank())
            .then_with(|| a.issue_id.cmp(&b.issue_id))
            .then_with(|| a.url.cmp(&b.url))
    });
    let mut out = String::with_capacity(1024 + findings.len() * 512);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<issues format=\"mio.audit_report.v1\" revision=\"{}\" livePassiveEnabled=\"{}\" livePassiveCrawlEnabled=\"{}\">",
        snapshot.revision, snapshot.live_passive_enabled, snapshot.live_passive_crawl_enabled
    );
    for (idx, finding) in findings.into_iter().enumerate() {
        let is_fp = false_positives.contains(&finding.id);
        let is_verified = verified.contains(&finding.id);
        let severity_str = if is_fp {
            "False positive"
        } else {
            finding.severity().en_label()
        };
        let confidence_str = if is_verified {
            "Verified"
        } else {
            match finding.confidence {
                IssueConfidence::Certain => "Certain",
                IssueConfidence::Firm => "Firm",
                IssueConfidence::Tentative => "Tentative",
            }
        };
        let phase_str = match finding.phase {
            AuditPhase::Passive => "Passive",
            AuditPhase::Active => "Active",
        };
        let path_part = finding
            .url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("/");
        let location = match &finding.parameter {
            Some(param) if !param.is_empty() => format!("{path_part} [{param}]"),
            _ => path_part.to_owned(),
        };
        out.push_str("  <issue>\n");
        let _ = writeln!(out, "    <serialNumber>{}</serialNumber>", idx + 1);
        let _ = writeln!(out, "    <id>{}</id>", escape_xml(&finding.id.to_string()));
        let _ = writeln!(out, "    <type>{}</type>", escape_xml(&finding.issue_id));
        let _ = writeln!(out, "    <name>{}</name>", escape_xml(&finding.title));
        let _ = writeln!(out, "    <host>{}</host>", escape_xml(&finding.host));
        let _ = writeln!(out, "    <url>{}</url>", escape_xml(&finding.url));
        let _ = writeln!(out, "    <path>{}</path>", escape_xml(path_part));
        let _ = writeln!(out, "    <location>{}</location>", escape_xml(&location));
        let _ = writeln!(out, "    <severity>{}</severity>", escape_xml(severity_str));
        let _ = writeln!(
            out,
            "    <confidence>{}</confidence>",
            escape_xml(confidence_str)
        );
        let _ = writeln!(out, "    <phase>{}</phase>", escape_xml(phase_str));
        if let Some(param) = &finding.parameter {
            let _ = writeln!(out, "    <parameter>{}</parameter>", escape_xml(param));
        }
        let _ = writeln!(
            out,
            "    <issueBackground>{}</issueBackground>",
            escape_xml(&finding.summary)
        );
        let _ = writeln!(
            out,
            "    <remediationBackground>{}</remediationBackground>",
            escape_xml(&finding.remediation)
        );
        let _ = writeln!(
            out,
            "    <evidence>{}</evidence>",
            escape_xml(&finding.evidence)
        );
        if let Some(note) = notes.get(&finding.id).filter(|n| !n.trim().is_empty()) {
            let _ = writeln!(out, "    <notes>{}</notes>", escape_xml(note));
        }
        let _ = writeln!(
            out,
            "    <detectedUnixMs>{}</detectedUnixMs>",
            finding.detected_unix_ms
        );
        out.push_str("  </issue>\n");
    }
    out.push_str("</issues>\n");
    out
}

impl Workbench {
    pub(in crate::workbench) fn reconcile_audit_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(flow) = self.audit_dashboard.loaded_flow.as_deref() {
            self.audit_dashboard
                .flow_cache
                .insert(flow.summary.id, flow.clone());
        }
        let valid_ids: HashSet<Uuid> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| f.id)
            .collect();
        self.audit_dashboard
            .selected_finding_ids
            .retain(|id| valid_ids.contains(id));
        if self
            .audit_dashboard
            .finding_selection_anchor
            .is_some_and(|anchor| !valid_ids.contains(&anchor))
        {
            self.audit_dashboard.finding_selection_anchor = None;
        }
        if let Some(selected_id) = self.audit_dashboard.selected_finding_id
            && !valid_ids.contains(&selected_id)
        {
            self.audit_dashboard.selected_finding_id = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .find(|f| self.audit_dashboard.selected_finding_ids.contains(&f.id))
                .map(|f| f.id);
            self.audit_dashboard.pending_flow_read = None;
            self.audit_dashboard.loaded_flow = None;
            self.audit_dashboard.flow_error = None;
            self.audit_dashboard
                .issue_request_editor
                .load(&[], window, cx);
            self.audit_dashboard
                .issue_response_editor
                .load(&[], window, cx);
            if let Some(next_id) = self.audit_dashboard.selected_finding_id {
                self.load_audit_finding_detail_only(next_id, false, window, cx);
            }
        }
        if let Some(editing_id) = self.audit_dashboard.editing_note_finding_id
            && !valid_ids.contains(&editing_id)
        {
            self.audit_dashboard.editing_note_finding_id = None;
        }
    }

    pub(in crate::workbench) fn load_audit_finding_detail_only(
        &mut self,
        finding_id: Uuid,
        switch_to_issues_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        self.clear_dashboard_intruder_selection(window, cx);
        if self.audit_dashboard.selected_audit_task_id.is_none() {
            self.audit_dashboard.selected_builtin_task = DashboardBuiltinTask::LivePassiveAudit;
        }
        if switch_to_issues_tab {
            self.audit_dashboard.active_task_tab = DashboardTaskTab::Issues;
        }
        if let Some(prev_flow) = self.audit_dashboard.loaded_flow.take() {
            self.audit_dashboard
                .flow_cache
                .insert(prev_flow.summary.id, *prev_flow);
        }
        self.audit_dashboard.selected_finding_id = Some(finding_id);
        self.audit_dashboard.pending_flow_read = None;
        self.audit_dashboard.flow_error = None;
        self.audit_dashboard
            .issue_request_editor
            .load(&[], window, cx);
        self.audit_dashboard
            .issue_response_editor
            .load(&[], window, cx);
        if let Some(flow_id) = finding.flow_id
            && let Some(project) = self.project
            && self.backend_available()
            && self.transition.is_none()
        {
            if let Some(cached) = self.audit_dashboard.flow_cache.get(&flow_id).cloned() {
                let req_raw = request_to_raw(&cached.request);
                let resp_raw = cached
                    .response
                    .as_ref()
                    .map(response_to_raw)
                    .unwrap_or_default();
                self.audit_dashboard
                    .issue_request_editor
                    .load(&req_raw, window, cx);
                self.audit_dashboard
                    .issue_response_editor
                    .load(&resp_raw, window, cx);
                self.audit_dashboard.loaded_flow = Some(Box::new(cached));
                self.apply_selected_finding_evidence_highlight(&req_raw, &resp_raw, window, cx);
                cx.notify();
                return;
            }
            self.selection_revision += 1;
            let revision = self.selection_revision;
            if self.enqueue(
                Command::ReadFlow {
                    project,
                    selection_revision: revision,
                    flow_id,
                },
                cx,
            ) {
                self.audit_dashboard.pending_flow_read = Some((project, revision, flow_id));
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_audit_finding_in_place(
        &mut self,
        finding_id: Uuid,
        switch_to_issues_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .any(|f| f.id == finding_id)
        {
            return;
        }
        self.audit_dashboard.selected_finding_ids.clear();
        self.audit_dashboard.selected_finding_ids.insert(finding_id);
        self.audit_dashboard.finding_selection_anchor = Some(finding_id);
        self.load_audit_finding_detail_only(finding_id, switch_to_issues_tab, window, cx);
    }

    pub(in crate::workbench) fn handle_audit_issue_row_click(
        &mut self,
        row_members: &[Uuid],
        visible_order: &[Uuid],
        shift: bool,
        toggle: bool,
        switch_to_issues_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&lead_id) = row_members.first() else {
            return;
        };
        if self.audit_dashboard.selected_finding_id.is_none() {
            self.audit_dashboard.selected_finding_ids.clear();
            self.audit_dashboard.finding_selection_anchor = None;
        }
        let valid_ids: HashSet<Uuid> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| f.id)
            .collect();
        self.audit_dashboard
            .selected_finding_ids
            .retain(|id| valid_ids.contains(id));
        if self.audit_dashboard.selected_finding_ids.is_empty()
            && let Some(cur) = self
                .audit_dashboard
                .selected_finding_id
                .filter(|id| valid_ids.contains(id))
        {
            self.audit_dashboard.selected_finding_ids.insert(cur);
        }

        if shift {
            let anchor = self
                .audit_dashboard
                .finding_selection_anchor
                .filter(|id| valid_ids.contains(id))
                .or_else(|| {
                    self.audit_dashboard
                        .selected_finding_id
                        .filter(|id| valid_ids.contains(id))
                })
                .unwrap_or(lead_id);
            let start_pos = visible_order.iter().position(|id| *id == anchor);
            let end_pos = row_members
                .iter()
                .filter_map(|id| visible_order.iter().position(|v| v == id))
                .next()
                .or_else(|| visible_order.iter().position(|id| *id == lead_id));
            if let (Some(s), Some(e)) = (start_pos, end_pos) {
                let lo = s.min(e);
                let hi = row_members
                    .iter()
                    .filter_map(|id| visible_order.iter().position(|v| v == id))
                    .max()
                    .unwrap_or(e)
                    .max(s.max(e));
                self.audit_dashboard.selected_finding_ids =
                    visible_order[lo..=hi].iter().copied().collect();
            } else {
                self.audit_dashboard.selected_finding_ids =
                    row_members.iter().copied().collect();
            }
            if self.audit_dashboard.finding_selection_anchor.is_none() {
                self.audit_dashboard.finding_selection_anchor = Some(anchor);
            }
            self.load_audit_finding_detail_only(lead_id, switch_to_issues_tab, window, cx);
            return;
        }

        if toggle {
            let all_selected = row_members
                .iter()
                .all(|id| self.audit_dashboard.selected_finding_ids.contains(id));
            if all_selected {
                for id in row_members {
                    self.audit_dashboard.selected_finding_ids.remove(id);
                }
                if self.audit_dashboard.selected_finding_ids.is_empty() {
                    self.audit_dashboard.selected_finding_id = None;
                    self.audit_dashboard.finding_selection_anchor = None;
                    self.audit_dashboard.pending_flow_read = None;
                    self.audit_dashboard.loaded_flow = None;
                    self.audit_dashboard.flow_error = None;
                    self.audit_dashboard
                        .issue_request_editor
                        .load(&[], window, cx);
                    self.audit_dashboard
                        .issue_response_editor
                        .load(&[], window, cx);
                    cx.notify();
                } else if self
                    .audit_dashboard
                    .selected_finding_id
                    .is_none_or(|cur| !self.audit_dashboard.selected_finding_ids.contains(&cur))
                {
                    let next_lead = visible_order
                        .iter()
                        .copied()
                        .find(|id| self.audit_dashboard.selected_finding_ids.contains(id))
                        .or_else(|| self.audit_dashboard.selected_finding_ids.iter().copied().next());
                    if let Some(next_id) = next_lead {
                        self.load_audit_finding_detail_only(
                            next_id,
                            switch_to_issues_tab,
                            window,
                            cx,
                        );
                    } else {
                        cx.notify();
                    }
                } else {
                    cx.notify();
                }
            } else {
                for &id in row_members {
                    self.audit_dashboard.selected_finding_ids.insert(id);
                }
                self.audit_dashboard.finding_selection_anchor = Some(lead_id);
                self.load_audit_finding_detail_only(lead_id, switch_to_issues_tab, window, cx);
            }
            return;
        }

        self.audit_dashboard.selected_finding_ids = row_members.iter().copied().collect();
        self.audit_dashboard.finding_selection_anchor = Some(lead_id);
        self.load_audit_finding_detail_only(lead_id, switch_to_issues_tab, window, cx);
    }

    pub(in crate::workbench) fn handle_audit_issue_row_right_down(
        &mut self,
        row_members: &[Uuid],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&lead_id) = row_members.first() else {
            return;
        };
        if self.audit_dashboard.selected_finding_id.is_none() {
            self.audit_dashboard.selected_finding_ids.clear();
            self.audit_dashboard.finding_selection_anchor = None;
        }
        let any_in_selection = row_members
            .iter()
            .any(|id| self.audit_dashboard.selected_finding_ids.contains(id))
            || self.audit_dashboard.selected_finding_id == Some(lead_id);
        if any_in_selection && self.audit_dashboard.selected_finding_ids.len() > 1 {
            for &id in row_members {
                self.audit_dashboard.selected_finding_ids.insert(id);
            }
            cx.notify();
            return;
        }
        self.audit_dashboard.selected_finding_ids = row_members.iter().copied().collect();
        self.audit_dashboard.finding_selection_anchor = Some(lead_id);
        if self.audit_dashboard.selected_finding_id != Some(lead_id) {
            self.load_audit_finding_detail_only(lead_id, false, window, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::workbench) fn select_audit_finding(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_audit_finding_in_place(finding_id, true, window, cx);
    }

    pub(in crate::workbench) fn audit_selected_target_ids(&self, fallback_id: Uuid) -> Vec<Uuid> {
        if self
            .audit_dashboard
            .selected_finding_ids
            .contains(&fallback_id)
            && self.audit_dashboard.selected_finding_ids.len() > 1
        {
            let ordered: Vec<Uuid> = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .map(|f| f.id)
                .filter(|id| self.audit_dashboard.selected_finding_ids.contains(id))
                .collect();
            if !ordered.is_empty() {
                return ordered;
            }
        }
        vec![fallback_id]
    }

    pub(in crate::workbench) fn toggle_audit_issue_sort(
        &mut self,
        col: DashboardIssueSortColumn,
        cx: &mut Context<Self>,
    ) {
        self.audit_dashboard.issue_sort = match self.audit_dashboard.issue_sort {
            Some((cur, asc)) if cur == col => Some((col, !asc)),
            _ => Some((col, true)),
        };
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_issue_sort(
        &mut self,
        col: DashboardIssueSortColumn,
        asc: bool,
        cx: &mut Context<Self>,
    ) {
        self.audit_dashboard.issue_sort = Some((col, asc));
        cx.notify();
    }

    pub(in crate::workbench) fn audit_issue_cell_text(
        &self,
        col_ix: usize,
        finding: &AuditFinding,
    ) -> String {
        let is_fp = self
            .audit_dashboard
            .false_positive_findings
            .contains(&finding.id);
        let is_verified = self
            .audit_dashboard
            .verified_confidence_findings
            .contains(&finding.id);
        match col_ix {
            0 => severity_display_style(finding.severity(), is_fp).0.to_owned(),
            1 => finding.title.clone(),
            2 => match &finding.parameter {
                Some(param) if !param.is_empty() => format!("{} ({param})", finding.host),
                _ => finding.host.clone(),
            },
            3 => finding.url.clone(),
            4 => confidence_display_label(finding.confidence, is_verified).to_owned(),
            _ => String::new(),
        }
    }

    pub(in crate::workbench) fn copy_audit_issue_column(
        &mut self,
        col_ix: usize,
        finding_ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        let by_id: HashMap<Uuid, &AuditFinding> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| (f.id, f))
            .collect();
        let mut lines = Vec::with_capacity(finding_ids.len());
        for &id in finding_ids {
            if let Some(finding) = by_id.get(&id) {
                lines.push(self.audit_issue_cell_text(col_ix, finding));
            }
        }
        let count = lines.len();
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!(
            "已复制「{}」列（{count} 行）到剪贴板。",
            DASHBOARD_ISSUE_COL_NAMES.get(col_ix).copied().unwrap_or("Issues")
        );
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_audit_issue_column_visibility(
        &mut self,
        col_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let visible_count = self
            .audit_dashboard
            .issue_col_visible
            .iter()
            .filter(|&&v| v)
            .count();
        if self.audit_dashboard.issue_col_visible[col_ix] {
            if visible_count > 1 {
                self.audit_dashboard.issue_col_visible[col_ix] = false;
            }
        } else {
            self.audit_dashboard.issue_col_visible[col_ix] = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn fit_audit_issue_column_width(
        &mut self,
        col_ix: usize,
        finding_ids: &[Uuid],
        cx: &mut Context<Self>,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest = measure(DASHBOARD_ISSUE_COL_NAMES[col_ix]) + 22.0;
        let by_id: HashMap<Uuid, &AuditFinding> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .map(|f| (f.id, f))
            .collect();
        for &id in finding_ids.iter().take(250) {
            if let Some(finding) = by_id.get(&id) {
                let text = self.audit_issue_cell_text(col_ix, finding);
                let preview = theme::truncate_cell_preview(&text);
                widest = widest.max(measure(&preview));
            }
        }
        let fitted = (widest + 22.0).round().clamp(56.0, 560.0);
        self.audit_dashboard.issue_col_widths[col_ix] = fitted;
        cx.notify();
    }

    pub(in crate::workbench) fn begin_audit_issue_column_resize(
        &mut self,
        col_ix: usize,
        start_x: f32,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let start_w = self.audit_dashboard.issue_col_widths[col_ix];
        self.audit_dashboard.issue_col_resize_origin = Some((col_ix, start_x, start_w));
    }

    pub(in crate::workbench) fn update_audit_issue_column_resize(
        &mut self,
        col_ix: usize,
        current_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= DASHBOARD_ISSUE_COL_COUNT {
            return;
        }
        let Some((drag_col, start_x, start_w)) = self.audit_dashboard.issue_col_resize_origin
        else {
            return;
        };
        if drag_col != col_ix {
            return;
        }
        let scale = theme::ui_scale().max(0.75);
        let delta_unscaled = (current_x - start_x) / scale;
        let next_w = (start_w + delta_unscaled).round().clamp(56.0, 560.0);
        if (self.audit_dashboard.issue_col_widths[col_ix] - next_w).abs() >= 0.5 {
            self.audit_dashboard.issue_col_widths[col_ix] = next_w;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn reset_audit_issue_column_layout(&mut self, cx: &mut Context<Self>) {
        self.audit_dashboard.issue_col_widths = DASHBOARD_ISSUE_DEFAULT_WIDTHS;
        self.audit_dashboard.issue_col_visible = [true; DASHBOARD_ISSUE_COL_COUNT];
        self.audit_dashboard.issue_col_resize_origin = None;
        self.audit_dashboard.issue_sort = None;
        cx.notify();
    }

    pub(in crate::workbench) fn switch_audit_editor_pretty(
        &mut self,
        which: AuditEditorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = match which {
            AuditEditorSlot::ScanRequest => &mut self.audit_dashboard.scan_request_editor,
            AuditEditorSlot::IssueRequest => &mut self.audit_dashboard.issue_request_editor,
            AuditEditorSlot::IssueResponse => &mut self.audit_dashboard.issue_response_editor,
        };
        if let Err(err) = editor.switch_pretty(window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_audit_editor_view(
        &mut self,
        which: AuditEditorSlot,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = match which {
            AuditEditorSlot::ScanRequest => &mut self.audit_dashboard.scan_request_editor,
            AuditEditorSlot::IssueRequest => &mut self.audit_dashboard.issue_request_editor,
            AuditEditorSlot::IssueResponse => &mut self.audit_dashboard.issue_response_editor,
        };
        if let Err(err) = editor.switch(view, window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn handle_audit_search_action(
        &mut self,
        which: AuditEditorSlot,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = match which {
            AuditEditorSlot::ScanRequest => &mut self.audit_dashboard.scan_request_editor,
            AuditEditorSlot::IssueRequest => &mut self.audit_dashboard.issue_request_editor,
            AuditEditorSlot::IssueResponse => &mut self.audit_dashboard.issue_response_editor,
        };
        if let Err(err) = editor.handle_search_bar_action(action, window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn scoped_audit_snapshot(
        &self,
        scope: AuditReportScope,
    ) -> Result<AuditStateSnapshot, &'static str> {
        let base = &self.audit_dashboard.snapshot;
        match scope {
            AuditReportScope::All => Ok(base.clone()),
            AuditReportScope::SingleFinding(finding_id) => {
                let Some(finding) = base.findings.iter().find(|f| f.id == finding_id).cloned()
                else {
                    return Err("未找到要导出的漏洞告警记录。");
                };
                Ok(AuditStateSnapshot {
                    revision: base.revision,
                    live_passive_enabled: base.live_passive_enabled,
                    live_passive_crawl_enabled: base.live_passive_crawl_enabled,
                    tasks: Vec::new(),
                    findings: vec![finding],
                })
            }
            AuditReportScope::Task(task_id) => {
                let Some(task) = base.tasks.iter().find(|t| t.id == task_id).cloned() else {
                    return Err("未找到对应的扫描任务记录。");
                };
                let findings = base
                    .findings
                    .iter()
                    .filter(|f| self.finding_belongs_to_audit_task(f, &task))
                    .cloned()
                    .collect();
                Ok(AuditStateSnapshot {
                    revision: base.revision,
                    live_passive_enabled: base.live_passive_enabled,
                    live_passive_crawl_enabled: base.live_passive_crawl_enabled,
                    tasks: vec![task],
                    findings,
                })
            }
        }
    }

    pub(in crate::workbench) fn render_audit_report_bytes(
        &self,
        snapshot: &AuditStateSnapshot,
        format: AuditReportFormat,
    ) -> Result<Vec<u8>, String> {
        match format {
            AuditReportFormat::Html => Ok(render_audit_report_html(snapshot).into_bytes()),
            AuditReportFormat::Json => render_audit_report_json(snapshot)
                .map_err(|err| format!("生成 JSON 审计报告失败：{err}")),
            AuditReportFormat::Xml => Ok(render_audit_report_xml(
                snapshot,
                &self.audit_dashboard.false_positive_findings,
                &self.audit_dashboard.verified_confidence_findings,
                &self.audit_dashboard.finding_notes,
            )
            .into_bytes()),
        }
    }

    pub(in crate::workbench) fn save_audit_report_with_format(
        &mut self,
        format: AuditReportFormat,
        scope: AuditReportScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() || self.file_picker_pending() {
            return;
        }
        let snapshot = match self.scoped_audit_snapshot(scope) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let payload = match self.render_audit_report_bytes(&snapshot, format) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let finding_count = snapshot.findings.len();
        let suggested_name = match (scope, format) {
            (AuditReportScope::SingleFinding(_), AuditReportFormat::Html) => {
                "mio-issue-report.html"
            }
            (AuditReportScope::SingleFinding(_), AuditReportFormat::Json) => {
                "mio-issue-report.json"
            }
            (AuditReportScope::SingleFinding(_), AuditReportFormat::Xml) => "mio-issue-report.xml",
            (AuditReportScope::Task(_), AuditReportFormat::Html) => "mio-task-report.html",
            (AuditReportScope::Task(_), AuditReportFormat::Json) => "mio-task-report.json",
            (AuditReportScope::Task(_), AuditReportFormat::Xml) => "mio-task-report.xml",
            (AuditReportScope::All, AuditReportFormat::Html) => "mio-audit-report.html",
            (AuditReportScope::All, AuditReportFormat::Json) => "mio-audit-report.json",
            (AuditReportScope::All, AuditReportFormat::Xml) => "mio-audit-report.xml",
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested_name),
        );
        let owner = self.owner_window;
        let fp_snapshot = self.audit_dashboard.false_positive_findings.clone();
        let verified_snapshot = self.audit_dashboard.verified_confidence_findings.clone();
        let notes_snapshot = self.audit_dashboard.finding_notes.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，本次审计报告未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension(format.extension());
                            }
                            let ext = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(str::to_ascii_lowercase);
                            let final_bytes = match ext.as_deref() {
                                Some("json") if format != AuditReportFormat::Json => {
                                    match render_audit_report_json(&snapshot) {
                                        Ok(b) => b,
                                        Err(err) => {
                                            view.fail(format!("生成 JSON 报告失败：{err}"), cx);
                                            return;
                                        }
                                    }
                                }
                                Some("html" | "htm") if format != AuditReportFormat::Html => {
                                    render_audit_report_html(&snapshot).into_bytes()
                                }
                                Some("xml") if format != AuditReportFormat::Xml => {
                                    render_audit_report_xml(
                                        &snapshot,
                                        &fp_snapshot,
                                        &verified_snapshot,
                                        &notes_snapshot,
                                    )
                                    .into_bytes()
                                }
                                _ => payload,
                            };
                            match mio_core::ui_files::write_export_file(&path, &final_bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存审计报告到文件：{}（共 {} 条告警）",
                                        path.display(),
                                        finding_count
                                    );
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入审计报告文件失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择报告保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn save_audit_report_to_file(
        &mut self,
        as_html: bool,
        single_finding_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let format = if as_html {
            AuditReportFormat::Html
        } else {
            AuditReportFormat::Json
        };
        let scope = match single_finding_id {
            Some(id) => AuditReportScope::SingleFinding(id),
            None => AuditReportScope::All,
        };
        self.save_audit_report_with_format(format, scope, window, cx);
    }
}
