use super::*;
use std::collections::VecDeque;
use mio_core::target_issues::IssueSeverity;

const FOOTER_LOG_MAX_ENTRIES: usize = 512;
const FOOTER_LOG_MAX_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum FooterLogSeverity {
    Info,
    Warning,
    Error,
}

impl FooterLogSeverity {
    pub(in crate::workbench) fn label(self) -> &'static str {
        match self {
            Self::Info => "Information",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FooterActiveDrawer {
    Events,
    Issues,
    Info,
    Activity,
}

#[derive(Clone)]
pub(in crate::workbench) struct FooterEventLogEntry {
    pub(in crate::workbench) id: u64,
    pub(in crate::workbench) timestamp_unix_ms: u64,
    pub(in crate::workbench) severity: FooterLogSeverity,
    pub(in crate::workbench) message: String,
    pub(in crate::workbench) detail: String,
}

impl FooterEventLogEntry {
    pub(in crate::workbench) fn time_label(&self) -> String {
        let seconds = i64::try_from(self.timestamp_unix_ms / 1000)
            .expect("event timestamp must fit a calendar timestamp");
        let calendar = crate::local_time::from_unix_seconds(seconds)
            .expect("event timestamp must have a local calendar representation");
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            calendar.year, calendar.month, calendar.day, calendar.hour, calendar.min,
            calendar.sec, self.timestamp_unix_ms % 1000,
        )
    }

    pub(in crate::workbench) fn copy_text(&self) -> String {
        format!(
            "{}\t{}\t{}\n{}",
            self.time_label(), self.severity.label(), self.message, self.detail,
        )
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.message.capacity() + self.detail.capacity()
    }
}

pub(in crate::workbench) struct FooterDrawerState {
    active: Option<FooterActiveDrawer>,
    pub(in crate::workbench) entries: VecDeque<FooterEventLogEntry>,
    retained_bytes: usize,
    next_entry_id: u64,
    selected_event: Option<u64>,
    event_severity: Option<FooterLogSeverity>,
    event_search: Entity<InputState>,
    issue_severity: Option<IssueSeverity>,
    issue_search: Entity<InputState>,
    selected_issue: Option<mio_core::Uuid>,
    _subscriptions: Vec<Subscription>,
}

impl FooterDrawerState {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let event_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索事件与详情"));
        let issue_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索标题、URL、参数、证据"));
        let subscriptions = [&event_search, &issue_search]
            .into_iter()
            .map(|input| cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }))
            .collect();
        Self {
            active: None,
            entries: VecDeque::new(),
            retained_bytes: 0,
            next_entry_id: 0,
            selected_event: None,
            event_severity: None,
            event_search,
            issue_severity: None,
            issue_search,
            selected_issue: None,
            _subscriptions: subscriptions,
        }
    }

    fn push(&mut self, severity: FooterLogSeverity, message: String, detail: String) {
        let message: String = message.chars().take(2048).collect();
        let detail: String = detail.chars().take(4096).collect();
        let timestamp_unix_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time must follow the Unix epoch")
                .as_millis(),
        ).expect("event timestamp must fit u64");
        self.next_entry_id = self.next_entry_id.checked_add(1)
            .expect("footer event identity capacity exhausted");
        let entry = FooterEventLogEntry {
            id: self.next_entry_id,
            timestamp_unix_ms,
            severity,
            message,
            detail,
        };
        self.retained_bytes += entry.retained_bytes();
        self.entries.push_back(entry);
        while self.entries.len() > FOOTER_LOG_MAX_ENTRIES
            || self.retained_bytes > FOOTER_LOG_MAX_BYTES
        {
            let removed = self.entries.pop_front().expect("bounded log must contain an entry");
            self.retained_bytes -= removed.retained_bytes();
            if self.selected_event == Some(removed.id) {
                self.selected_event = None;
            }
        }
    }

    pub(in crate::workbench) fn clear_events(&mut self) {
        self.entries.clear();
        self.retained_bytes = 0;
        self.selected_event = None;
    }
}

fn footer_memory_ratio(process_bytes: u64, total_bytes: u64) -> f32 {
    if total_bytes == 0 {
        return 0.;
    }
    (process_bytes as f64 / total_bytes as f64).clamp(0., 1.) as f32
}

fn format_resource_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1}GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{}KB", (bytes + 512) / 1024)
    } else {
        format!("{bytes}B")
    }
}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workbench {
    #[inline(never)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        theme::sync_window_system_dpi(window.scale_factor(), cx);
        if self.startup_wizard_active() && !self.shutdown_requested {
            return self.render_startup_wizard(window, cx).into_any_element();
        }
        self.sync_inspector_auto_open(window, cx);
        let footer_project_name = self.current_footer_project_name();
        let window_title = match footer_project_name.as_deref() {
            Some(name) => format!("MIO — {name}"),
            None => "MIO".to_owned(),
        };
        let body = self.render_workbench_active_body(window, cx);
        let footer_bar = self.render_workbench_footer_bar(footer_project_name, window, cx);
        let banners = self.render_workbench_banners(cx);
        let mut root = self.render_workspace_action_root(window, cx);
        self.bind_extended_top_menu_actions(&mut root, cx);
        let title_bar = crate::chrome::render_title_bar(
            "mio-workbench",
            window_title,
            crate::chrome::TitleBarControls::FULL,
            |_, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            },
            window,
            cx,
        );
        root.child(title_bar)
            .child(self.render_tool_tabs(window, cx))
            .child(div().relative().flex_1().min_h_0().child(body)
                .children(self.render_global_overlays(window, cx)))
            .children(banners)
            .child(footer_bar)
            .into_any_element()
    }
}

impl Workbench {
    pub(in crate::workbench) fn record_footer_event(
        &mut self,
        severity: FooterLogSeverity,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.footer_drawer.push(severity, message.into(), detail.into());
    }

    pub(in crate::workbench) fn reset_footer_drawer_on_project_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.footer_drawer.clear_events();
        self.footer_drawer.active = None;
        self.footer_drawer.event_severity = None;
        self.footer_drawer.issue_severity = None;
        self.footer_drawer.selected_issue = None;
        for input in [&self.footer_drawer.event_search, &self.footer_drawer.issue_search] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        let project = self.current_footer_project_name().unwrap_or_else(|| "临时项目".into());
        let detail = self.project_path.as_ref()
            .map_or_else(|| "临时项目已打开".into(), |path| path.display().to_string());
        self.record_footer_event(FooterLogSeverity::Info, format!("项目已打开：{project}"), detail);
    }

    pub(in crate::workbench) fn observe_footer_event_before_dispatch(
        &mut self,
        event: &Event,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let observation = match event {
            Event::SendStarted { project, tab_id, run_id, .. }
                if self.project == Some(*project) => Some((
                    FooterLogSeverity::Info,
                    format!("Repeater 请求开始发送（标签 {tab_id}）"),
                    format!("run_id={run_id}"),
                )),
            Event::SendFinished { project, tab_id, record, persisted, .. }
                if self.project == Some(*project) => {
                let severity = match record.summary.state {
                    mio_core::FlowState::Failed(_) => FooterLogSeverity::Error,
                    mio_core::FlowState::Cancelled => FooterLogSeverity::Warning,
                    _ => FooterLogSeverity::Info,
                };
                Some((severity,
                    format!("Repeater 请求{}（标签 {tab_id}）", record.summary.state.label()),
                    format!("{} {}\n{} ms · {} 字节 · 已存储：{persisted}\n{}",
                        record.request.method, record.request.url, record.summary.elapsed_ms,
                        record.summary.response_bytes,
                        match &record.summary.state { mio_core::FlowState::Failed(reason) => reason.as_str(), _ => "" },
                    ),
                ))
            }
            Event::PluginDiagnostic { message, .. } => Some((
                FooterLogSeverity::Warning, "扩展诊断".into(), message.clone(),
            )),
            Event::PluginStopped { reason, .. } => {
                let expected = super::plugins::is_expected_plugin_stop_reason(reason);
                Some((
                    if expected {
                        FooterLogSeverity::Info
                    } else {
                        FooterLogSeverity::Warning
                    },
                    "扩展已停止".into(),
                    reason.clone(),
                ))
            }
            Event::PluginAutoLoaded { info, .. } => Some((
                FooterLogSeverity::Info,
                format!("已自动加载扩展：{}", info.manifest.id),
                format!("{} · v{}", info.manifest.id, info.manifest.version),
            )),
            Event::PluginAutoLoadFailed {
                plugin_id, message, ..
            } => Some((
                FooterLogSeverity::Error,
                format!("扩展自动加载失败：{plugin_id}"),
                message.clone(),
            )),
            Event::Audit(mio_core::AuditEvent::TaskUpdated { project, task, new_findings, .. })
                if self.project == Some(*project) => Some((
                    if task.error.is_some() { FooterLogSeverity::Error } else { FooterLogSeverity::Info },
                    format!("审计任务{}：{}", task.state.label(), task.title),
                    format!("{}\n请求 {} / {} · 新增告警 {}\n{}", task.target_url,
                        task.requests_sent, task.total_planned, new_findings.len(),
                        task.error.as_deref().unwrap_or("")),
                )),
            Event::Audit(mio_core::AuditEvent::PassiveFindings { project, findings, .. })
                if self.project == Some(*project) && !findings.is_empty() => Some((
                    FooterLogSeverity::Info, format!("被动审计发现 {} 条告警", findings.len()),
                    findings.iter().map(|finding| format!("{} · {}", finding.title, finding.url))
                        .take(32).collect::<Vec<_>>().join("\n"),
                )),
            _ => None,
        };
        if let Some((severity, message, detail)) = observation {
            self.record_footer_event(severity, message, detail);
        }
    }

    pub(in crate::workbench) fn observe_footer_state_after_dispatch(
        &mut self,
        previous_status: &str,
        previous_error: &Option<String>,
        previous_storage_error: &Option<String>,
    ) {
        if self.status != previous_status
            && self.footer_drawer.entries.back().is_none_or(|entry| {
                entry.message != self.status
                    && !(self.status == "项目已打开" && entry.message.starts_with("项目已打开"))
            })
        {
            self.record_footer_event(FooterLogSeverity::Info, self.status.clone(), self.status.clone());
        }
        if &self.error != previous_error && let Some(error) = self.error.clone()
            && self.footer_drawer.entries.back().is_none_or(|entry| entry.message != error)
        {
            self.record_footer_event(FooterLogSeverity::Error, error.clone(), error);
        }
        if &self.storage_error != previous_storage_error && let Some(error) = self.storage_error.clone() {
            self.record_footer_event(FooterLogSeverity::Error, "项目存储错误", error);
        }
    }

    fn toggle_footer_drawer(&mut self, drawer: FooterActiveDrawer, cx: &mut Context<Self>) {
        self.footer_drawer.active = if self.footer_drawer.active == Some(drawer) { None } else { Some(drawer) };
        if matches!(
            self.footer_drawer.active,
            Some(FooterActiveDrawer::Issues | FooterActiveDrawer::Activity)
        )
            && let Some(project) = self.project
            && self.backend_available() && self.transition.is_none()
            && !self.audit_dashboard.loading
        {
            self.query_audit_state(project, cx);
        }
        if self.footer_drawer.active == Some(FooterActiveDrawer::Activity)
            && self.project.is_some()
            && self.backend_available()
            && self.transition.is_none()
            && self.intruder.results.runs.pending.is_none()
        {
            let after = self.intruder.results.runs.after;
            self.query_intruder_runs(after, cx);
        }
        cx.notify();
    }

    fn current_footer_project_name(&self) -> Option<String> {
        self.workspace
            .project_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.project_path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
    }

    #[inline(never)]
    fn render_workbench_active_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.shutdown_requested {
            div()
                .id("runtime-shutdown")
                .test_support()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("正在等待后台任务、连接与项目文件完成清理…")
                .into_any_element()
        } else if self.transition == Some(TransitionPhase::Switching) {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("正在打开项目；完成前不会丢弃当前工作区。")
                .into_any_element()
        } else if self.ext_import.visible {
            self.render_ext_import(cx)
        } else {
            match self.tool {
                Tool::Dashboard => self.render_intruder_dashboard(window, cx),
                Tool::Target => self.render_target(window, cx),
                Tool::Proxy => self.render_proxy(window, cx),
                Tool::Intruder => self.render_intruder(window, cx),
                Tool::Repeater => self.render_repeater_with_redirect(window, cx),
                Tool::Collaborator => self.render_collaborator(window, cx),
                Tool::Sequencer => self.render_sequencer(window, cx),
                Tool::Extensions => self.render_extensions(cx),
                Tool::Decoder => self.render_decoder(window, cx),
                Tool::Comparer => self.render_comparer(window, cx),
                Tool::Logger => self.render_logger(window, cx),
                Tool::Organiser => self.render_organiser(window, cx),
                Tool::Discover => self.render_discover(window, cx),
            }
        }
    }

    #[inline(never)]
    fn render_discover(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ipc_path = mio_core::env_ipc_socket_path()
            .unwrap_or_else(mio_core::default_ipc_socket_path);
        let ipc_path_str = ipc_path.display().to_string();
        let cli_status_cmd = format!("mio-cli --socket \"{}\" status", ipc_path_str);
        let backend_ok = self.backend_available();
        let proxy_summary = self
            .proxy_address
            .map(|addr| format!("运行中 ({addr})"))
            .unwrap_or_else(|| "未启动".into());
        let card_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x232529 } else { 0xffffff }).into();
        let hero_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x1e2024 } else { 0xf6f7f9 }).into();
        let copy_cmd = cli_status_cmd.clone();
        let copy_sock = ipc_path_str.clone();
        div()
            .id("discover-main-pane")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .py(px(16.))
                    .px(px(24.))
                    .bg(hero_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(693.))
                            .max_w_full()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(28.))
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_color(rgb(0xdb612f))
                                            .child("MIO,"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(28.))
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_color(theme::foreground())
                                            .child("redefined."),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme::foreground())
                                    .child("MioAT brings agentic AI to human-led pentesting with MIO's proven tools, project context, and purpose-built skills."),
                            )
                            .child(
                                div()
                                    .p(px(8.))
                                    .rounded(px(6.))
                                    .bg(card_bg)
                                    .border_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .id("discover-ipc-status")
                                            .test_support()
                                            .text_size(px(12.))
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .text_color(theme::foreground())
                                            .child(format!(
                                                "Agent IPC 状态：{} · Proxy：{}",
                                                if backend_ok { "已就绪" } else { "已断开" },
                                                proxy_summary
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("discover-ipc-socket-path")
                                            .test_support()
                                            .font_family(theme::mono_font_family())
                                            .text_size(px(11.5))
                                            .text_color(theme::muted_foreground())
                                            .child(format!("MIO_IPC_SOCKET={ipc_path_str}")),
                                    )
                                    .child(
                                        div()
                                            .font_family(theme::mono_font_family())
                                            .text_size(px(11.5))
                                            .text_color(theme::foreground())
                                            .child(cli_status_cmd),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        Button::new("discover-copy-ipc-cmd")
                                            .bg(rgb(0xdb612f))
                                            .text_color(rgb(0xffffff))
                                            .small()
                                            .w(px(143.))
                                            .h(px(23.))
                                            .label("Launch MioAT")
                                            .accessibility_label("Launch MioAT")
                                            .tooltip("复制 mio-cli Agent IPC 接入命令到剪贴板")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        copy_cmd.clone(),
                                                    ),
                                                );
                                                this.error = None;
                                                this.status =
                                                    "已复制 Agent IPC CLI 接入命令到剪贴板。".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("discover-copy-ipc-socket")
                                            .small()
                                            .w(px(128.))
                                            .h(px(23.))
                                            .label("Learn more")
                                            .accessibility_label("Learn more")
                                            .tooltip("复制当前 MIO_IPC_SOCKET 套接字路径到剪贴板")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        copy_sock.clone(),
                                                    ),
                                                );
                                                this.error = None;
                                                this.status =
                                                    "已复制 MIO_IPC_SOCKET 路径到剪贴板。".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("discover-open-mio-ai-panel")
                                            .small()
                                            .h(px(23.))
                                            .px(px(10.))
                                            .bg(rgb(0x4946ae))
                                            .text_color(rgb(0xffffff))
                                            .label("MioAT 控制中心")
                                            .accessibility_label("MioAT 控制中心")
                                            .tooltip("打开交互式 MioAT / Agent IPC 提示词与上下文控制面板")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.mio_ai_panel_open = true;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(1270.))
                    .py(px(14.))
                    .px(px(24.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("Discover more"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(px(14.))
                            .child(self.render_discover_card(
                                "discover-card-command-palette",
                                "Go faster with command palette",
                                "Find and run MIO commands instantly to keep your workflow moving without digging through menus.",
                                cx.listener(|this, _, window, cx| {
                                    this.open_command_palette(window, cx);
                                }),
                            ))
                            .child(self.render_discover_card(
                                "discover-card-custom-logic",
                                "Customize MIO with your own logic",
                                "Shape your workflow and surface the data that matters most using custom actions, columns, and filters.",
                                cx.listener(|this, _, window, cx| {
                                    this.select_tool(Tool::Proxy, window, cx);
                                    this.proxy_page = ProxyPage::History;
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_discover_card(
                                "discover-card-scan-checks",
                                "Extend scanning with custom scan checks",
                                "Import or create additional checks to expand what MIO Scanner looks for during a scan.",
                                cx.listener(|this, _, _, cx| {
                                    this.open_dashboard(cx);
                                    this.audit_dashboard.new_scan_open = true;
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_discover_card(
                                "discover-card-organiser",
                                "Share your findings securely",
                                "Use collections to group requests and responses and share them through a single, secure link.",
                                cx.listener(|this, _, window, cx| {
                                    this.select_tool(Tool::Organiser, window, cx);
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_discover_card(
        &self,
        button_id: &'static str,
        title: &'static str,
        description: &'static str,
        on_take_me_there: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let card_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x232529 } else { 0xffffff }).into();
        div()
            .w(px(322.))
            .p(px(12.))
            .rounded(px(6.))
            .bg(card_bg)
            .border_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme::muted_foreground())
                            .child(description),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Button::new(button_id)
                            .small()
                            .w(px(145.))
                            .h(px(23.))
                            .label("Take me there")
                            .accessibility_label(format!("Take me there: {title}"))
                            .on_click(on_take_me_there),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_workbench_banners(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let visible_error = match (&self.error, &self.user_preferences.remember_error) {
            (Some(error), Some(remember)) if error != remember => {
                Some(format!("{error}\n{remember}"))
            }
            (Some(error), _) => Some(error.clone()),
            (None, remember) => remember.clone(),
        };
        let mut out = Vec::new();
        if let Some(error) = self.storage_error.clone() {
            out.push(
                toolbar()
                    .h_auto()
                    .min_h(theme::scaled_px(34.))
                    .py(px(3.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(div().flex_1().child(format!(
                        "存储错误（{} 条待恢复）：{error}",
                        self.pending_records
                    )))
                    .child(
                        button("storage-retry", "保存重试")
                            .icon(gpui_kit::assets::IconName::RefreshCw)
                            .tooltip("重试将待恢复数据写入当前项目文件")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_project(false, window, cx)
                            })),
                    )
                    .child(
                        button("storage-save-as", "另存为…")
                            .icon(gpui_kit::assets::IconName::FolderOpen)
                            .tooltip("将当前项目与待恢复数据另存为新的 .mio 文件")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_project(true, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = visible_error {
            out.push(
                toolbar()
                    .id("workspace-error")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Alert)
                    .aria_label(error.clone())
                    .h_auto()
                    .min_h(theme::scaled_px(30.))
                    .py(px(2.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(div().flex_1().child(error))
                    .child(
                        button("dismiss-error", "关闭提示")
                            .icon(gpui_kit::assets::IconName::Close)
                            .tooltip("隐藏当前工作区错误提示横幅")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                this.user_preferences.remember_error = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    #[inline(never)]
    fn render_footer_drawer(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active = self.footer_drawer.active?;
        let title = match active {
            FooterActiveDrawer::Events => "Event log",
            FooterActiveDrawer::Issues => "All issues",
            FooterActiveDrawer::Info => "项目与内存信息",
            FooterActiveDrawer::Activity => "后台活动与任务控制",
        };
        let id = match active {
            FooterActiveDrawer::Events => "footer-event-log-drawer",
            FooterActiveDrawer::Issues => "footer-all-issues-drawer",
            FooterActiveDrawer::Info => "footer-info-popup",
            FooterActiveDrawer::Activity => "footer-activity-drawer",
        };
        let content = match active {
            FooterActiveDrawer::Events => self.render_footer_event_content(cx),
            FooterActiveDrawer::Issues => self.render_footer_issue_content(cx),
            FooterActiveDrawer::Activity => self.render_footer_activity_content(cx),
            FooterActiveDrawer::Info => {
                let project_name = self.current_footer_project_name().unwrap_or_else(|| "临时项目".into());
                let status = self.footer_status(window, cx);
                let disk = self.project_disk_bytes;
                let process = mio_core::current_process_memory_bytes();
                let total = mio_core::total_physical_memory_bytes();
                div().p(px(12.)).flex().flex_col().gap(px(8.))
                    .child(div().id("footer-project-name").test_support().aria_label(project_name.clone()).child(project_name))
                    .child(div().id("footer-info-project-status").test_support().role(gpui_kit::accesskit::Role::Status)
                        .aria_label(status.clone()).child(status))
                    .child(format!("存储: {} · 内存: {} · 主机物理内存总量: {}",
                        disk.map_or_else(|| "—".into(), format_resource_bytes), format_resource_bytes(process),
                        if total > 0 { format_resource_bytes(total) } else { "读取不可用".into() }))
                    .child("Memory X of Y 表示当前进程物理内存占主机物理内存总量的比例。")
                    .child(button("footer-slim-save-as", "瘦身另存为…")
                        .tooltip("将当前项目保存到新文件并清理冗余流量")
                        .disabled(!self.backend_available() || self.transition.is_some() || self.file_picker_pending())
                        .on_click(cx.listener(|this, _, window, cx| this.choose_slim_save_as(window, cx))))
                    .into_any_element()
            }
        };
        Some(div().id(id).test_support().role(gpui_kit::accesskit::Role::Pane)
            .aria_label(title).w_full().h(theme::scaled_px(if active == FooterActiveDrawer::Info { 210. } else { 320. }))
            .flex_shrink_0().min_h_0().flex().flex_col().bg(theme::surface())
            .border_t_1().border_color(theme::border())
            .child(toolbar().child(title).child(div().flex_1())
                .child(button("footer-drawer-close", "关闭")
                    .icon(gpui_kit::assets::IconName::Close).tooltip("收起底部面板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.footer_drawer.active = None;
                        cx.notify();
                    }))))
            .child(div().flex_1().min_h_0().child(content)).into_any_element())
    }

    fn render_footer_activity_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_operate = self.audit_can_operate();
        let live_passive = self.audit_dashboard.snapshot.live_passive_enabled;
        let scan_tasks = &self.audit_dashboard.snapshot.tasks;
        let running_scans: Vec<_> = scan_tasks
            .iter()
            .filter(|t| t.state == mio_core::AuditTaskState::Running)
            .map(|t| t.id)
            .collect();
        let paused_scans: Vec<_> = scan_tasks
            .iter()
            .filter(|t| t.state == mio_core::AuditTaskState::Paused)
            .map(|t| t.id)
            .collect();
        let intruder_runs = &self.intruder.results.runs.rows;
        let running_intruder = intruder_runs
            .iter()
            .filter(|r| !r.state.is_terminal())
            .count();
        let intercept_on = self.intercept_requests || self.intercept_responses;
        let proxy_summary = match self.proxy_address {
            Some(addr) => format!(
                "Proxy 监听中：{addr} · Intercept：{} · 挂起队列：{}",
                if intercept_on { "开启" } else { "关闭" },
                self.intercepts.len()
            ),
            None => format!(
                "Proxy 已停止 · Intercept：{} · 挂起队列：{}",
                if intercept_on { "开启" } else { "关闭" },
                self.intercepts.len()
            ),
        };
        let activity_summary = format!(
            "{proxy_summary} · 扫描任务：运行中 {} / 已暂停 {} / 共 {} · 活跃 Intruder 攻击：{}",
            running_scans.len(),
            paused_scans.len(),
            scan_tasks.len(),
            running_intruder
        );
        let mut list = div()
            .id("footer-activity-task-list")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if scan_tasks.is_empty() && intruder_runs.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_color(theme::muted_foreground())
                    .child("当前没有扫描或 Intruder 攻击任务。"),
            );
        }
        for task in scan_tasks {
            let task_id = task.id;
            let label = format!(
                "[扫描] {} · {} · {} · 请求 {}/{} · 告警 {}",
                if task.title.is_empty() {
                    &task.target_url
                } else {
                    &task.title
                },
                task.mode.label(),
                task.state.label(),
                task.requests_sent,
                task.total_planned,
                task.issues_found
            );
            list = list.child(
                div()
                    .id(SharedString::from(format!("footer-activity-scan-row-{task_id}")))
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(label.clone())
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .truncate()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::selected()))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.audit_dashboard.selected_audit_task_id = Some(task_id);
                        this.clear_dashboard_intruder_selection(window, cx);
                        this.open_dashboard(cx);
                    })),
            );
        }
        for run in intruder_runs {
            let run_id = run.id;
            let title = self.dashboard_title(run);
            let label = format!(
                "[Intruder] {} · {}",
                title,
                crate::workbench::intruder::results_view::run_progress(run)
            );
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "footer-activity-intruder-row-{run_id}"
                    )))
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(label.clone())
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .truncate()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::selected()))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_dashboard(cx);
                        this.select_dashboard_intruder_run(run_id, window, cx);
                    })),
            );
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child(
                        button(
                            "footer-activity-live-passive",
                            if live_passive {
                                "实时被动审计：开启"
                            } else {
                                "实时被动审计：暂停"
                            },
                        )
                        .selected(live_passive)
                        .disabled(!can_operate)
                        .tooltip("开启或暂停对 Proxy 实时流量的自动被动漏洞审计")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_audit_live_passive(cx);
                        })),
                    )
                    .child(
                        button("footer-activity-pause-all-scans", "暂停全部扫描")
                            .disabled(!can_operate || running_scans.is_empty())
                            .tooltip("暂停当前所有正在运行的主动与被动扫描任务")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for id in &running_scans {
                                    this.pause_audit_scan(*id, cx);
                                }
                            })),
                    )
                    .child(
                        button("footer-activity-resume-all-scans", "继续全部扫描")
                            .disabled(!can_operate || paused_scans.is_empty())
                            .tooltip("恢复当前所有已暂停的主动与被动扫描任务")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for id in &paused_scans {
                                    this.resume_audit_scan(*id, cx);
                                }
                            })),
                    )
                    .child(
                        button("footer-activity-new-scan", "新建扫描…")
                            .disabled(!can_operate)
                            .tooltip("打开 Dashboard 扫描启动器并发起新的安全扫描")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_scan_launcher(window, cx);
                            })),
                    )
                    .child(
                        button("footer-activity-open-browser", "打开配置浏览器")
                            .disabled(!can_operate || self.ext_import.visible)
                            .tooltip("启动已配置 Proxy 端口与 CA 信任的浏览器")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_configured_browser(cx);
                            })),
                    )
                    .child(
                        button("footer-activity-open-dashboard", "打开 Dashboard")
                            .disabled(self.ext_import.visible)
                            .tooltip("切换到 Dashboard 任务与漏洞活动面板")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_dashboard(cx);
                            })),
                    )
                    .child(
                        button("footer-activity-task-settings", "任务与资源池设置…")
                            .tooltip("打开项目任务与并发资源池设置")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_project_tasks_settings(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("footer-activity-summary")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Status)
                    .aria_label(activity_summary.clone())
                    .px(px(12.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(theme::border())
                    .text_color(theme::muted_foreground())
                    .child(activity_summary),
            )
            .child(list)
            .into_any_element()
    }

    fn render_footer_event_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let search = self.footer_drawer.event_search.read(cx).value().trim().to_lowercase();
        let severity = self.footer_drawer.event_severity;
        let filtered: Vec<_> = self.footer_drawer.entries.iter().rev().filter(|entry| {
            severity.is_none_or(|selected| selected == entry.severity)
                && (search.is_empty() || entry.message.to_lowercase().contains(&search)
                    || entry.detail.to_lowercase().contains(&search))
        }).collect();
        let selected = self.footer_drawer.selected_event.and_then(|id| self.footer_drawer.entries.iter().find(|entry| entry.id == id));
        let selected_id = self.footer_drawer.selected_event;
        let mut list = div().id("footer-event-list").test_support().flex_1().min_h_0()
            .overflow_y_scroll().flex().flex_col();
        if filtered.is_empty() {
            list = list.child(div().p(px(12.)).text_color(theme::muted_foreground()).child("没有匹配的事件。"));
        }
        for entry in &filtered {
            let id = entry.id;
            let row_label = format!("{} · {} · {}", entry.time_label(), entry.severity.label(), entry.message);
            list = list.child(div().id(SharedString::from(format!("footer-event-row-{id}")))
                .test_support().role(gpui_kit::accesskit::Role::Button).aria_label(row_label.clone())
                .flex_shrink_0().h(theme::scaled_px(25.)).px(px(12.)).cursor_pointer()
                .bg(if selected_id == Some(id) { theme::selected() } else { theme::surface() })
                .text_color(if entry.severity == FooterLogSeverity::Error { theme::danger_fg() } else { theme::foreground() })
                .truncate().child(row_label).on_click(cx.listener(move |this, _, _, cx| {
                    this.footer_drawer.selected_event = Some(id);
                    cx.notify();
                })));
        }
        div().size_full().min_h_0().flex().flex_col()
            .child(toolbar().children([
                ("footer-event-filter-all", "全部", None),
                ("footer-event-filter-info", "Information", Some(FooterLogSeverity::Info)),
                ("footer-event-filter-warning", "Warning", Some(FooterLogSeverity::Warning)),
                ("footer-event-filter-error", "Error", Some(FooterLogSeverity::Error)),
            ].into_iter().map(|(id, label, target)| button(id, label).selected(severity == target)
                .tooltip(format!("筛选事件：{label}"))
                .on_click(cx.listener(move |this, _, _, cx| { this.footer_drawer.event_severity = target; cx.notify(); }))))
                .child(div().w(px(230.)).child(Input::new(&self.footer_drawer.event_search).id("footer-event-search").aria_label("搜索事件").small()))
                .child(div().flex_1())
                .child(format!("{} / {}", filtered.len(), self.footer_drawer.entries.len()))
                .child(button("footer-event-copy-all", "复制全部").tooltip("复制当前项目保留的全部事件")
                    .disabled(self.footer_drawer.entries.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let text = this.footer_drawer.entries.iter().map(FooterEventLogEntry::copy_text).collect::<Vec<_>>().join("\n\n");
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    })))
                .child(button("footer-event-clear", "清空").tooltip("清空当前项目的事件记录")
                    .disabled(self.footer_drawer.entries.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| { this.footer_drawer.clear_events(); cx.notify(); }))))
            .child(div().flex_1().min_h_0().flex().child(list)
                .child(div().w(px(430.)).h_full().min_h_0().border_l_1().border_color(theme::border()).flex().flex_col()
                    .child(toolbar().child("事件详情").child(div().flex_1())
                        .child(button("footer-event-copy-selected", "复制").tooltip("复制选中事件的时间、严重度与详情")
                            .disabled(selected.is_none()).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(entry) = this.footer_drawer.selected_event.and_then(|id| this.footer_drawer.entries.iter().find(|entry| entry.id == id)) {
                                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(entry.copy_text()));
                                }
                            }))))
                    .child(div().id("footer-event-detail").test_support().flex_1().min_h_0().overflow_y_scroll().p(px(12.))
                        .child(selected.map_or_else(|| "选择事件查看详情。".into(), FooterEventLogEntry::copy_text)))))
            .into_any_element()
    }

    fn render_footer_issue_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let pane = &self.audit_dashboard;
        let severity = self.footer_drawer.issue_severity;
        let keyword = self.footer_drawer.issue_search.read(cx).value().trim().to_lowercase();
        let filtered: Vec<_> = pane.snapshot.findings.iter().filter(|finding| {
            severity.is_none_or(|selected| finding.severity() == selected)
                && (keyword.is_empty() || [finding.title.as_str(), finding.url.as_str(), finding.issue_id.as_str(),
                    finding.parameter.as_deref().unwrap_or(""), finding.evidence.as_str()]
                    .iter().any(|value| value.to_lowercase().contains(&keyword)))
        }).collect();
        let mut list = div().id("footer-issue-list").test_support().flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if let Some(error) = &pane.state_error {
            list = list.child(div().id("footer-issues-query-error").test_support().role(gpui_kit::accesskit::Role::Alert)
                .p(px(12.)).text_color(theme::danger_fg()).child(error.clone()));
        } else if pane.loading {
            list = list.child(div().id("footer-issues-loading").test_support().p(px(12.)).child("正在读取项目审计告警…"));
        } else if filtered.is_empty() {
            list = list.child(div().p(px(12.)).child(if pane.loaded { "没有匹配的告警。" } else { "审计状态尚未读取。" }));
        }
        for finding in &filtered {
            let finding_id = finding.id;
            let label = format!("{} · {} · {}", finding.severity().label(), finding.title, finding.url);
            list = list.child(div().id(SharedString::from(format!("footer-issue-row-{finding_id}")))
                .test_support().role(gpui_kit::accesskit::Role::Button).aria_label(label.clone())
                .h(theme::scaled_px(28.)).flex_shrink_0().px(px(12.)).truncate().cursor_pointer()
                .bg(if self.footer_drawer.selected_issue == Some(finding_id) { theme::selected() } else { theme::surface() })
                .child(label).on_click(cx.listener(move |this, _, window, cx| {
                    this.footer_drawer.selected_issue = Some(finding_id);
                    this.select_audit_finding(finding_id, window, cx);
                })));
        }
        let selected = self.footer_drawer.selected_issue.and_then(|id| pane.snapshot.findings.iter().find(|finding| finding.id == id));
        let detail = match selected {
            Some(finding) => self.render_issue_finding_detail(finding, cx),
            None => div().p(px(12.)).child("选择告警查看问题描述、修复建议与关联报文。").into_any_element(),
        };
        div().size_full().min_h_0().flex().flex_col()
            .child(toolbar().children([
                ("footer-issues-filter-all", "全部", None),
                ("footer-issues-filter-high", "High", Some(IssueSeverity::High)),
                ("footer-issues-filter-medium", "Medium", Some(IssueSeverity::Medium)),
                ("footer-issues-filter-low", "Low", Some(IssueSeverity::Low)),
                ("footer-issues-filter-info", "Information", Some(IssueSeverity::Information)),
            ].into_iter().map(|(id, label, target)| button(id, label).tooltip(format!("筛选告警：{label}"))
                .selected(severity == target).on_click(cx.listener(move |this, _, _, cx| { this.footer_drawer.issue_severity = target; cx.notify(); }))))
                .child(div().w(px(230.)).child(Input::new(&self.footer_drawer.issue_search).id("footer-issues-search").aria_label("搜索告警").small()))
                .child(div().flex_1()).child(format!("{} / {}", filtered.len(), pane.snapshot.findings.len()))
                .child(button("footer-issues-refresh", "刷新").tooltip("从后台读取当前项目全部审计告警")
                    .disabled(!self.audit_can_operate() || pane.loading).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(project) = this.project { this.query_audit_state(project, cx); }
                        cx.notify();
                    }))))
            .child(div().flex_1().min_h_0().flex().child(list)
                .child(div().w(px(680.)).min_w_0().h_full().min_h_0().flex().flex_col()
                    .border_l_1().border_color(theme::border()).child(detail)))
            .into_any_element()
    }

    #[inline(never)]
    fn render_workbench_footer_bar(
        &self,
        _footer_project_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let process_mem_bytes = mio_core::current_process_memory_bytes();
        let total_mem_bytes = mio_core::total_physical_memory_bytes();
        let memory_label = format!("Memory: {} of {}", format_resource_bytes(process_mem_bytes),
            if total_mem_bytes > 0 { format_resource_bytes(total_mem_bytes) } else { "unavailable".into() });
        let memory_ratio = footer_memory_ratio(process_mem_bytes, total_mem_bytes);
        let issue_count = self.audit_dashboard.snapshot.findings.len();
        let event_count = self.footer_drawer.entries.len();
        let event_log_label = if event_count > 0 {
            format!("Event log ({event_count})")
        } else {
            "Event log".to_owned()
        };
        let footer_status = self.footer_status(window, cx);
        let bar = div()
            .id("mio-status-bar")
            .test_support()
                    .relative()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px(px(15.))
                    .pt(px(4.))
                    .flex()
                    .items_center()
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .line_height(px(17.))
                    .bg(theme::toolbar_surface())
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .id("footer-event-log")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Button)
                            .aria_label(event_log_label.clone())
                            .flex_shrink_0()
                            .cursor_pointer()
                            .min_w(px(86.))
                            .h(px(22.))
                            .mr(px(15.))
                            .flex()
                            .items_center()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_footer_drawer(FooterActiveDrawer::Events, cx);
                            }))
                            .child(
                                div()
                                    .id("footer-event-log-text")
                                    .test_support()
                                    .min_w(px(71.))
                                    .h(px(17.))
                                    .whitespace_nowrap()
                                    .child(event_log_label),
                            )
                            .when(event_count > 0, |this| {
                                this.child(
                                    div()
                                        .id("footer-event-log-dot")
                                        .test_support()
                                        .ml(px(2.))
                                        .self_start()
                                        .mt(px(2.))
                                        .size(px(6.))
                                        .rounded(px(3.))
                                        .bg(gpui_kit::rgb(0x26649d)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("footer-all-issues")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Button)
                            .aria_label(format!("All issues ({issue_count})"))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_footer_drawer(FooterActiveDrawer::Issues, cx);
                            }))
                            .child(
                                div()
                                    .id("footer-all-issues-text")
                                    .test_support()
                                    .h(px(17.))
                                    .whitespace_nowrap()
                                    .child(format!("All issues ({issue_count})")),
                            )
                            .when(issue_count > 0, |this| {
                                this.child(
                                    div()
                                        .id("footer-all-issues-dot")
                                        .test_support()
                                        .self_start()
                                        .mt(px(2.))
                                        .size(px(6.))
                                        .rounded(px(3.))
                                        .bg(gpui_kit::rgb(0xd4380d)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("project-status")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Status)
                            .aria_label(footer_status.clone())
                            .flex_1()
                            .min_w_0()
                            .h(px(17.)),
                    )
                    .child(
                        div()
                            .id("footer-resource-usage")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Button)
                            .aria_label(format!("{memory_label}（进程物理内存 / 主机物理内存总量）"))
                            .relative()
                            .flex_shrink_0()
                            .cursor_pointer()
                            .w(px(262.))
                            .h(px(22.))
                            .mr(px(20.))
                            .bg(theme::toolbar_surface())
                            .text_color(theme::foreground())
                            .hover(|s| s.bg(theme::selected()))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "查看项目状态与进程物理内存占主机物理内存总量比例",
                                )
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_footer_drawer(FooterActiveDrawer::Info, cx);
                            }))
                            .child(
                                div()
                                    .id("footer-memory-info-icon")
                                    .test_support()
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(3.5))
                                    .w(px(15.))
                                    .h(px(15.))
                                    .child(
                                        gpui_kit::component::Icon::new(
                                            gpui_kit::assets::IconName::Info,
                                        )
                                        .size(px(15.))
                                        .text_color(theme::foreground()),
                                    ),
                            )
                            .child(
                                div()
                                    .id("footer-memory-text")
                                    .test_support()
                                    .aria_label(memory_label.clone())
                                    .absolute()
                                    .left(px(25.))
                                    .top(px(2.5))
                                    .w(px(180.))
                                    .h(px(17.))
                                    .truncate()
                                    .child(memory_label),
                            )
                            .child(
                                div()
                                    .id("footer-memory-progress")
                                    .test_support()
                                    .absolute()
                                    .left(px(210.))
                                    .top(px(7.))
                                    .w(px(52.))
                                    .h(px(8.))
                                    .rounded(px(4.))
                                    .overflow_hidden()
                                    .bg(gpui_kit::rgb(0xeeeeee))
                                    .child(
                                        div()
                                            .id("footer-memory-progress-fill")
                                            .test_support()
                                            .h_full()
                                            .rounded(px(4.))
                                            .w(px(if process_mem_bytes > 0 {
                                                (52.0 * memory_ratio).max(6.0)
                                            } else {
                                                0.0
                                            }))
                                            .bg(gpui_kit::rgb(0xc9c9c9)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(0.))
                            .h(px(0.))
                            .overflow_hidden()
                            .child(self.render_cross_platform_menu_button(cx)),
                    )
                    .child(
                        div()
                            .id("footer-settings-activity-capsule")
                            .test_support()
                            .flex_shrink_0()
                            .w(px(58.))
                            .h(px(22.))
                            .rounded(px(3.))
                            .border_2()
                            .border_color(gpui_kit::rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }))
                            .bg(theme::toolbar_surface())
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("footer-open-settings")
                                    .test_support()
                                    .role(gpui_kit::accesskit::Role::Button)
                                    .aria_label("打开设置")
                                    .cursor_pointer()
                                    .w(px(27.))
                                    .h_full()
                                    .hover(|s| s.bg(theme::selected()))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            if cfg!(target_os = "macos") {
                                                "打开全局与项目设置 (⌘,)"
                                            } else {
                                                "打开全局与项目设置 (Ctrl+,)"
                                            },
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                                    )
                                    .child(
                                        gpui_kit::component::Icon::default()
                                            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"currentColor\"><path d=\"M6.5 1C6.8 3.8 8.2 5.2 11 5.5 8.2 5.8 6.8 7.2 6.5 10 6.2 7.2 4.8 5.8 2 5.5 4.8 5.2 6.2 3.8 6.5 1ZM12 8.5c.2 1.6 1 2.3 2.5 2.5-1.5.2-2.3.9-2.5 2.5-.2-1.6-1-2.3-2.5-2.5 1.5-.2 2.3-.9 2.5-2.5ZM4 10.5c.15 1.1.7 1.6 1.8 1.8-1.1.2-1.65.7-1.8 1.8-.15-1.1-.7-1.6-1.8-1.8 1.1-.2 1.65-.7 1.8-1.8Z\"/></svg>")
                                            .size(px(15.))
                                        .text_color(gpui_kit::rgb(0x4949a9)),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(18.))
                                    .flex_shrink_0()
                                    .bg(theme::border()),
                            )
                            .child(
                                div()
                                    .id("footer-open-activity")
                                    .test_support()
                                    .role(gpui_kit::accesskit::Role::Button)
                                    .aria_label("后台活动与任务控制")
                                    .cursor_pointer()
                                    .w(px(28.))
                                    .h_full()
                                    .hover(|s| s.bg(theme::selected()))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "查看并控制后台扫描任务、实时被动审计与 Proxy 活动",
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_footer_drawer(FooterActiveDrawer::Activity, cx)
                                    }))
                                    .child(
                                        gpui_kit::component::Icon::new(
                                            gpui_kit::assets::IconName::ChevronDown,
                                        )
                                        .size(px(14.))
                                        .text_color(theme::foreground()),
                                    ),
                            ),
                    )
                    .when(
                        matches!(
                            self.transition,
                            Some(
                                TransitionPhase::Quiescing
                                    | TransitionPhase::Ready
                                    | TransitionPhase::Saving(_)
                            )
                        ),
                        |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .left(px(250.))
                                    .top(px(10.))
                                    .child(
                                        button("cancel-transition", "取消切换/退出")
                                            .h(theme::scaled_px(20.).max(px(20.)))
                                            .icon(gpui_kit::assets::IconName::Ban)
                                            .tooltip("中止正在进行的项目切换或退出流程并留在当前项目")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.cancel_transition(cx)),
                                            ),
                                    ),
                            )
                        },
                    )
            .into_any_element();
        div().w_full().flex_shrink_0().flex().flex_col()
            .children(self.render_footer_drawer(window, cx)).child(bar).into_any_element()
    }

    #[inline(never)]
    fn render_workspace_action_root(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + ParentElement + InteractiveElement + Styled + use<> {
        let mut root = div()
            .id("mio-workspace")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .bg(theme::background())
            .text_color(theme::foreground())
            .key_context("MioWorkspace")
            .track_focus(&self.focus);
        let r = root.interactivity();
        r.on_action(cx.listener(|this, _: &crate::OpenSettings, window, cx| {
                this.open_settings(window, cx)
            }));
        r.on_action(cx.listener(|this, _: &crate::OpenCommandPalette, window, cx| {
                this.open_command_palette(window, cx)
            }));
        r.on_action(cx.listener(|this, _: &crate::OpenGlobalSearch, window, cx| {
                this.open_global_search(window, cx)
            }));
        if self.backend_available()
            && self.transition.is_none()
            && !self.file_picker_pending()
        {
                    r.on_action(cx.listener(|this, _: &NewProject, window, cx| {
                        this.request_transition(AfterSave::NewProject, window, cx)
                    }));
                    r.on_action(
                        cx.listener(|this, _: &OpenProject, window, cx| {
                            this.choose_open(window, cx)
                        }),
                    );
                    r.on_action(cx.listener(|this, _: &ImportExtProject, window, cx| {
                        this.choose_ext_import(window, cx)
                    }));
                    r.on_action(cx.listener(|this, _: &ShowExtReport, window, cx| {
                        this.show_ext_report(window, cx)
                    }));
                    r.on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                        this.save_project(false, window, cx)
                    }));
                    r.on_action(cx.listener(|this, _: &SaveProjectAs, window, cx| {
                        this.save_project(true, window, cx)
                    }));
                    r.on_action(
                        cx.listener(|this, _: &crate::SlimSaveProjectAs, window, cx| {
                            this.choose_slim_save_as(window, cx)
                        }),
                    );
                    r.on_action(
                        cx.listener(|this, _: &ExportCa, window, cx| this.export_ca(window, cx)),
                    );
        }
        r.on_action(|_: &crate::Quit, _, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            });
        r.on_action(cx.listener(Self::search_message_action));
        if !self.ext_import.visible {
                r.on_action(cx.listener(|this, _: &OpenInIntruder, window, cx| {
                    this.open_in_intruder(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &AddHttpNotes, window, cx| {
                    this.open_http_notes(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &OpenInRepeater, window, cx| {
                    this.open_in_repeater(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToDecoder, window, cx| {
                    this.send_to_decoder(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToSequencer, window, cx| {
                    this.send_to_sequencer(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToScanner, window, cx| {
                    this.send_to_scanner(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::OpenScanLauncher, window, cx| {
                    this.open_scan_launcher(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToPassiveScanner, window, cx| {
                    this.send_to_passive_scanner(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToOrganiser, window, cx| {
                    this.send_selected_to_organiser(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::AddToSiteMap, window, cx| {
                    this.add_to_site_map(window, cx)
                }));
                r.on_action(
                    cx.listener(|this, _: &crate::SendToComparerRequest, window, cx| {
                        this.send_to_comparer(false, window, cx)
                    }),
                );
                r.on_action(
                    cx.listener(|this, _: &crate::SendToComparerResponse, window, cx| {
                        this.send_to_comparer(true, window, cx)
                    }),
                );
                r.on_action(cx.listener(|this, _: &CopyUrl, window, cx| this.copy_url(window, cx)));
                r.on_action(cx.listener(|this, _: &CopyCurlCommand, window, cx| {
                    this.copy_curl_command(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::CopyResponse, window, cx| {
                    this.copy_response(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::CopyLinksInResponse, window, cx| {
                    this.copy_links_in_response(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::OpenResponseInBrowser, window, cx| {
                    this.open_current_response_in_browser(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &CopyPrettified, window, cx| {
                    this.copy_prettified(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &ChangeRequestMethod, window, cx| {
                    this.change_request_method(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &PasteUrlAsRequest, window, cx| {
                    this.paste_url_as_request(window, cx)
                }));
                r.on_action(
                    cx.listener(|this, _: &crate::DeleteSelectedHttpHistory, window, cx| {
                        this.delete_selected_http_history(window, cx)
                    }),
                );
                r.on_action(
                    cx.listener(|this, _: &crate::ClearLiveProxyHistory, window, cx| {
                        this.clear_live_proxy_history(window, cx)
                    }),
                );
                r.on_action(cx.listener(|this, _: &crate::AddToScope, window, cx| {
                    this.add_to_scope(window, cx)
                }));
                r.on_action(
                    cx.listener(|this, _: &crate::ExcludeFromScope, window, cx| {
                        this.exclude_from_scope(window, cx)
                    }),
                );
                r.on_action(cx.listener(Self::convert_selection_action));
                r.on_action(cx.listener(Self::dispatch_copy_converted_selection_action));
                r.on_action(cx.listener(
                    |this, _: &editing::OpenSelectionInCyberChef, window, cx| {
                        this.open_selection_in_cyberchef(window, cx)
                    },
                ));
                r.on_action(cx.listener(
                    |this, action: &editing::InvokePluginContextMenu, window, cx| {
                        this.invoke_plugin_context_menu(action.0, window, cx)
                    },
                ));
                r.on_action(cx.listener(|this, _: &SendRequest, window, cx| this.send(window, cx)));
                r.on_action(cx.listener(|this, _: &CancelRequest, _, cx| this.cancel(cx)));
                r.on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)));
                r.on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                    let active = match this.tool {
                        Tool::Dashboard | Tool::Target | Tool::Sequencer => None,
                        Tool::Collaborator => None,
                        Tool::Intruder => this.workspace.intruder.active,
                        Tool::Repeater => this
                            .workspace
                            .active
                            .or(this.workspace.active_websocket_tab),
                        _ => None,
                    };
                    if let Some(id) = active {
                        this.close_tab(id, window, cx);
                    }
                }));
                r.on_action(
                    cx.listener(|this, _: &RestoreTab, window, cx| this.restore_tab(window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &PreviousTab, window, cx| this.step_tab(-1, window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &MoveTabUp, window, cx| this.move_active(-1, window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &MoveTabDown, window, cx| {
                        this.move_active(1, window, cx)
                    }),
                );
                r.on_action(cx.listener(|this, _: &SelectToolDashboard, window, cx| {
                    this.select_tool(Tool::Dashboard, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolTarget, window, cx| {
                    this.select_tool(Tool::Target, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolProxy, window, cx| {
                    this.select_tool(Tool::Proxy, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolIntruder, window, cx| {
                    this.select_tool(Tool::Intruder, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolRepeater, window, cx| {
                    this.select_tool(Tool::Repeater, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolCollaborator, window, cx| {
                    this.select_tool(Tool::Collaborator, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolSequencer, window, cx| {
                    this.select_tool(Tool::Sequencer, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolDecoder, window, cx| {
                    this.select_tool(Tool::Decoder, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolComparer, window, cx| {
                    this.select_tool(Tool::Comparer, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolLogger, window, cx| {
                    this.select_tool(Tool::Logger, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolOrganiser, window, cx| {
                    this.select_tool(Tool::Organiser, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolExtensions, window, cx| {
                    this.select_tool(Tool::Extensions, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolDiscover, window, cx| {
                    this.select_tool(Tool::Discover, window, cx)
                }));
                r.on_action(cx.listener(
                    |this, _: &ToggleTabOrientation, window, cx| {
                        this.toggle_tab_orientation(window, cx)
                    },
                ));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{EntityInputHandler, TestAppContext};
    use mio_core::{
        AuditCommand, AuditEvent, AuditStateSnapshot, AuditTaskMode, AuditTaskState,
        AuditTaskSummary, Command, Event, FlowState, ProjectKey, Uuid,
        audit_engine::{AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence},
        target_issues::IssueSeverity,
    };

    use super::*;
    use crate::workbench::tests::{drain, fixture, record, tab};

    #[test]
    fn footer_memory_ratio_handles_empty_fractional_and_over_total_metrics() {
        assert_eq!(footer_memory_ratio(0, 0), 0.0);
        assert_eq!(footer_memory_ratio(1, 0), 0.0);
        assert_eq!(footer_memory_ratio(1, 4), 0.25);
        assert_eq!(footer_memory_ratio(4, 4), 1.0);
        assert_eq!(footer_memory_ratio(5, 4), 1.0);
        assert_eq!(footer_memory_ratio(u64::MAX, 1), 1.0);
    }

    fn assert_footer_dot_paints(window: &Window, id: &'static str, color: u32) {
        let scale = window.scale_factor();
        let bounds = window.find(id).bounds().scale(scale);
        let diameter = gpui_kit::ScaledPixels(6. * scale);
        let radius = gpui_kit::ScaledPixels(3. * scale);
        assert!(
            window.painted_quads().iter().any(|quad| {
                quad.bounds.contains(&bounds.center())
                    && quad.bounds.size == gpui_kit::size(diameter, diameter)
                    && quad.background == gpui_kit::rgb(color).into()
                    && quad.corner_radii == gpui_kit::Corners::all(radius)
                    && quad.bounds.intersect(&quad.content_mask.bounds).size == quad.bounds.size
            }),
            "{id} 需要完整绘制指定颜色的 6×6 实心圆点"
        );
    }

    #[gpui_kit::test]
    fn footer_geometry_and_real_physical_memory_metrics(cx: &mut TestAppContext) {
        let mut f = fixture(cx, vec![tab("1: /api")], 32);
        drain(&mut f.commands);

        let total_mem = mio_core::total_physical_memory_bytes();
        let process_mem = mio_core::current_process_memory_bytes();
        assert!(total_mem >= process_mem && total_mem > 0);
        assert_eq!(footer_memory_ratio(0, total_mem), 0.0);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let bar = window.find("mio-status-bar").bounds();
            assert_eq!(bar.origin.y + bar.size.height, window.viewport_size().height);
            assert_eq!(bar.size.height, px(40.));

            let event_text = window.find("footer-event-log-text").bounds();
            assert_eq!(event_text.origin.x - bar.origin.x, px(15.));
            assert_eq!(event_text.origin.y - bar.origin.y, px(14.));
            assert!(event_text.size.width >= px(71.));
            assert_eq!(event_text.size.height, px(17.));
            let event_dot = window.find("footer-event-log-dot").bounds();
            assert_eq!(event_dot.size, gpui_kit::size(px(6.), px(6.)));
            assert!(event_dot.origin.x >= event_text.origin.x + event_text.size.width);
            assert!(event_dot.origin.y <= event_text.origin.y);
            assert_footer_dot_paints(window, "footer-event-log-dot", 0x26649d);

            let all_issues = window.find("footer-all-issues").bounds();
            assert!(all_issues.origin.x - bar.origin.x >= px(116.));
            assert!(window.try_find("footer-all-issues-dot").is_none());

            let resource = window.find("footer-resource-usage");
            let res_bounds = resource.bounds();
            assert_eq!(res_bounds.size, gpui_kit::size(px(262.), px(22.)));
            let icon = window.find("footer-memory-info-icon").bounds();
            assert_eq!(icon.origin.x - res_bounds.origin.x, px(0.));
            assert_eq!(icon.size.width, px(15.));
            let text = window.find("footer-memory-text").bounds();
            assert_eq!(text.origin.x - res_bounds.origin.x, px(25.));
            assert_eq!(text.size.width, px(180.));
            let progress = window.find("footer-memory-progress").bounds();
            assert_eq!(progress.origin.x - res_bounds.origin.x, px(210.));
            assert_eq!(progress.origin.y - res_bounds.origin.y, px(7.));
            assert_eq!(progress.size, gpui_kit::size(px(52.), px(8.)));
            let fill = window.find("footer-memory-progress-fill").bounds();
            assert_eq!(fill.size.height, px(8.));
            assert!(fill.size.width > px(0.) && fill.size.width <= px(52.));
            let scale = window.scale_factor();
            let progress_scaled = progress.scale(scale);
            let fill_scaled = fill.scale(scale);
            let painted_quads = window.painted_quads();
            assert!(
                painted_quads.iter().any(|quad| {
                    quad.bounds.contains(&progress_scaled.center())
                        && quad.bounds.size == progress_scaled.size
                        && quad.background == gpui_kit::rgb(0xeeeeee).into()
                        && quad.bounds.intersect(&quad.content_mask.bounds).size == quad.bounds.size
                }),
                "内存进度条需要完整绘制 52×8 浅灰背景"
            );
            assert!(
                painted_quads.iter().any(|quad| {
                    quad.bounds.contains(&fill_scaled.center())
                        && quad.background == gpui_kit::rgb(0xc9c9c9).into()
                        && quad.bounds.size.height == fill_scaled.size.height
                        && (quad.bounds.size.width.0 - fill_scaled.size.width.0).abs() <= 1.
                        && quad.bounds.origin.x >= progress_scaled.origin.x.floor()
                        && quad.bounds.right() <= progress_scaled.right().ceil()
                }),
                "内存进度条需要按真实占用比例绘制灰色填充"
            );

            let capsule = window.find("footer-settings-activity-capsule").bounds();
            assert_eq!(capsule.size, gpui_kit::size(px(58.), px(22.)));
            assert_eq!(bar.origin.x + bar.size.width - (capsule.origin.x + capsule.size.width), px(15.));

            let label = window.find("footer-memory-text").label().unwrap().to_owned();
            assert!(label.starts_with("Memory: ") && label.contains(&format!(" of {}", format_resource_bytes(total_mem))));

            window.click("footer-resource-usage", cx);
            window.render_frame(cx);
            assert!(window.find("footer-info-popup").visible());
            assert_eq!(window.find("footer-project-name").label(), Some("临时项目"));
            assert!(window.find("project-status").label().unwrap().contains("项目已打开"));
            window.click("footer-slim-save-as", cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_new_path());
    }

    #[gpui_kit::test]
    fn footer_event_log_captures_real_events_and_bounds_retention(cx: &mut TestAppContext) {
        let mut f = fixture(cx, vec![tab("1: /api")], 64);
        drain(&mut f.commands);
        let tab_id = cx.read(|cx| f.view.read(cx).workspace.tabs[0].id);

        let addr: std::net::SocketAddr = "127.0.0.1:18080".parse().unwrap();
        f.events.try_send(Event::ProxyState {
            project: f.project,
            address: Some(addr),
            cancel: Some(CancellationToken::new()),
            intercept_requests: false,
            intercept_responses: false,
        }).unwrap();
        f.events.try_send(Event::Error {
            project: Some(f.project),
            operation: "save_project",
            message: "磁盘空间不足".into(),
        }).unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-repeater", cx);
            f.view.update(cx, |view, cx| view.send(window, cx));
        }).unwrap();
        let cmds = drain(&mut f.commands);
        let run_id = cmds.into_iter().find_map(|cmd| match cmd {
            Command::Send { run_id, .. } => Some(run_id),
            _ => None,
        }).expect("send command issued");
        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert!(view.cancel_send(tab_id, cx));
            });
            window.render_frame(cx);
        }).unwrap();
        let mut cancelled = record("http://127.0.0.1:9001/", "");
        cancelled.summary.id = run_id;
        cancelled.summary.state = FlowState::Cancelled;
        f.events.try_send(Event::SendFinished {
            project: f.project,
            tab_id,
            draft_revision: 1,
            record: Box::new(cancelled),
            persisted: false,
        }).unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("footer-event-log", cx);
            window.render_frame(cx);
            assert!(window.find("footer-event-log-drawer").visible());
            assert!(f.view.read(cx).tool != Tool::Dashboard);

            let entries: Vec<_> = f.view.read(cx).footer_drawer.entries.iter().cloned().collect();
            assert!(entries.iter().any(|e| e.message.contains("项目已打开")));
            assert!(entries.iter().any(|e| e.message.contains("Proxy 正在监听 127.0.0.1:18080")));
            assert!(entries.iter().any(|e| e.severity == FooterLogSeverity::Error && e.message.contains("磁盘空间不足")));
            assert!(entries.iter().any(|e| e.message.contains("Repeater 请求取消已提交")));
            assert!(entries.iter().any(|e| e.severity == FooterLogSeverity::Warning && e.message.contains("Repeater 请求已取消")));

            let err_id = entries.iter().find(|e| e.severity == FooterLogSeverity::Error).unwrap().id;
            window.click("footer-event-filter-error", cx);
            window.render_frame(cx);
            window.click(SharedString::from(format!("footer-event-row-{err_id}")), cx);
            window.render_frame(cx);
            window.click("footer-event-copy-selected", cx);
            let clip = cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default();
            assert!(clip.contains("Error") && clip.contains("磁盘空间不足"));

            f.view.update(cx, |view, _| {
                for idx in 0..800 {
                    view.record_footer_event(FooterLogSeverity::Info, format!("event-{idx}"), "x".repeat(3000));
                }
                assert!(view.footer_drawer.retained_bytes <= FOOTER_LOG_MAX_BYTES);
                assert!(view.footer_drawer.entries.len() <= FOOTER_LOG_MAX_ENTRIES);
            });
            window.render_frame(cx);
            window.click("footer-event-clear", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).footer_drawer.entries.is_empty());
            assert!(window.try_find("footer-event-log-dot").is_none());
        }).unwrap();

        let next_project = ProjectKey { id: Uuid::new_v4(), generation: 2 };
        f.events.try_send(Event::ProjectOpened {
            project: next_project,
            path: None,
            workspace: mio_core::Workspace::default(),
        }).unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            let drawer = &f.view.read(cx).footer_drawer;
            assert!(drawer.active.is_none());
            assert_eq!(drawer.entries.len(), 1);
            assert!(drawer.entries[0].message.contains("项目已打开"));
        });
    }

    #[gpui_kit::test]
    fn footer_all_issues_drawer_queries_filters_and_selects_real_finding(cx: &mut TestAppContext) {
        let mut f = fixture(cx, vec![tab("1: /api")], 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("footer-all-issues", cx);
            window.render_frame(cx);
            assert!(window.find("footer-all-issues-drawer").visible());
            assert!(f.view.read(cx).tool != Tool::Dashboard);
        }).unwrap();

        let cmds = drain(&mut f.commands);
        let [Command::Audit(AuditCommand::QueryState { project, query_revision })] = cmds.as_slice() else {
            panic!("opening All issues drawer must query backend audit state");
        };
        assert_eq!(*project, f.project);

        let mut flow = record("http://127.0.0.1:9001/vuln?id=1", "");
        flow.response = Some(mio_core::HttpResponse {
            status: 200,
            version: mio_core::HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![],
            body: b"SQL syntax error".to_vec(),
            trailers: vec![],
        });
        let high = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00100200".into(),
            title: "SQL 注入".into(),
            severity_label: "高危".into(),
            severity_rank: IssueSeverity::High.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Active,
            host: "127.0.0.1".into(),
            url: flow.request.url.clone(),
            parameter: Some("id".into()),
            flow_id: Some(flow.summary.id),
            summary: "响应包含数据库语法错误。".into(),
            evidence: "SQL syntax error".into(),
            request_highlights: vec![],
            response_highlights: vec![EvidenceSpan { offset: 0, length: 16 }],
            remediation: "使用参数化查询。".into(),
            detected_unix_ms: 100,
        };
        let low = AuditFinding {
            id: Uuid::new_v4(),
            severity_label: "低危".into(),
            severity_rank: IssueSeverity::Low.rank(),
            title: "缺少安全响应头".into(),
            flow_id: None,
            ..high.clone()
        };
        f.events.try_send(Event::Audit(AuditEvent::State {
            project: f.project,
            query_revision: *query_revision,
            result: Ok(Box::new(AuditStateSnapshot {
                revision: 1,
                live_passive_enabled: true,
                live_passive_crawl_enabled: true,
                tasks: vec![AuditTaskSummary {
                    id: Uuid::new_v4(),
                    title: "扫描".into(),
                    target_url: flow.request.url.clone(),
                    mode: AuditTaskMode::PassiveAndActive,
                    state: AuditTaskState::Completed,
                    requests_sent: 2,
                    total_planned: 2,
                    issues_found: 2,
                    started_unix_ms: 90,
                    error: None,
                }],
                findings: vec![high.clone(), low.clone()],
            })),
        })).unwrap();
        cx.run_until_parked();

        let search = cx.read(|cx| f.view.read(cx).footer_drawer.issue_search.clone());
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("footer-all-issues").label(), Some("All issues (2)"));
            let issues_text = window.find("footer-all-issues-text").bounds();
            let issues_dot = window.find("footer-all-issues-dot").bounds();
            assert_eq!(issues_dot.size, gpui_kit::size(px(6.), px(6.)));
            assert!(issues_dot.origin.x >= issues_text.origin.x + issues_text.size.width);
            assert!(issues_dot.origin.y <= issues_text.origin.y);
            assert_footer_dot_paints(window, "footer-all-issues-dot", 0xd4380d);
            window.click("footer-issues-filter-high", cx);
            window.render_frame(cx);
            assert!(window.try_find(SharedString::from(format!("footer-issue-row-{}", low.id))).is_none());
            search.update(cx, |state, cx| state.replace_text_in_range(None, "SQL", window, cx));
            window.render_frame(cx);
            window.click(SharedString::from(format!("footer-issue-row-{}", high.id)), cx);
            window.render_frame(cx);
        }).unwrap();

        let cmds = drain(&mut f.commands);
        let [Command::ReadFlow { project, selection_revision, flow_id }] = cmds.as_slice() else {
            panic!("selecting finding in All issues drawer must read associated flow");
        };
        assert_eq!((*project, *flow_id), (f.project, flow.summary.id));
        f.events.try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(flow),
            original: Box::default(),
        }).unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("dashboard-issue-remediation").label().unwrap().contains("参数化查询"));
            window.click("dashboard-issue-tab-response", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).audit_dashboard.issue_response_editor.raw.read(cx).value().contains("SQL syntax error"));

            window.click("footer-open-activity", cx);
            window.render_frame(cx);
            assert!(window.find("footer-activity-drawer").visible());
        }).unwrap();

        let running_task_id = Uuid::new_v4();
        f.events.try_send(Event::Audit(AuditEvent::State {
            project: f.project,
            query_revision: 0,
            result: Ok(Box::new(AuditStateSnapshot {
                revision: 2,
                live_passive_enabled: true,
                live_passive_crawl_enabled: true,
                tasks: vec![AuditTaskSummary {
                    id: running_task_id,
                    title: "主站扫描".into(),
                    target_url: "http://127.0.0.1:9001/".into(),
                    mode: AuditTaskMode::PassiveAndActive,
                    state: AuditTaskState::Running,
                    requests_sent: 1,
                    total_planned: 5,
                    issues_found: 0,
                    started_unix_ms: 100,
                    error: None,
                }],
                findings: vec![],
            })),
        })).unwrap();
        cx.run_until_parked();
        let _ = drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find("footer-activity-summary")
                    .label()
                    .unwrap()
                    .contains("扫描任务：运行中 1")
            );
            window.click("footer-activity-pause-all-scans", cx);
            window.click("footer-activity-live-passive", cx);
        }).unwrap();
        let cmds = drain(&mut f.commands);
        assert!(cmds.iter().any(|cmd| matches!(
            cmd,
            Command::Audit(AuditCommand::PauseScan { project, task_id })
                if *project == f.project && *task_id == running_task_id
        )));
        assert!(cmds.iter().any(|cmd| matches!(
            cmd,
            Command::Audit(AuditCommand::SetLivePassive { project, enabled: false })
                if *project == f.project
        )));
    }

    #[gpui_kit::test]
    fn discover_tab_and_suite_ai_button_navigate_and_copy_real_ipc_command(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, vec![tab("1: /api")], 32);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let discover_tab = window.find("tool-discover");
            assert!(discover_tab.visible());
            assert_eq!(discover_tab.bounds().size.width, px(74.));

            let ai_btn = window.find("suiteMioAiButton");
            assert!(ai_btn.visible());
            assert_eq!(ai_btn.bounds().size, gpui_kit::size(px(120.), px(25.)));
            assert_eq!(ai_btn.label(), Some("MioAT"));

            window.click("suiteMioAiButton", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).tool, Tool::Discover);
            assert!(window.find("discover-main-pane").visible());
            assert!(window.find("discover-ipc-status").visible());
            assert!(window.find("discover-ipc-socket-path").visible());

            window.click("discover-copy-ipc-cmd", cx);
            window.render_frame(cx);
            let clip = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            assert!(clip.starts_with("mio-cli --socket \""));
            assert!(clip.ends_with("\" status"));

            window.click("discover-card-command-palette", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).command_palette_open);
            f.view.update(cx, |view, _| view.command_palette_open = false);
            window.render_frame(cx);

            window.click("discover-card-custom-logic", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).tool, Tool::Proxy);
            assert_eq!(f.view.read(cx).proxy_page, ProxyPage::History);

            window.click("tool-discover", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).tool, Tool::Discover);

            window.click("discover-card-scan-checks", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).tool, Tool::Dashboard);
            assert!(f.view.read(cx).audit_dashboard.new_scan_open);

            window.click("tool-discover", cx);
            window.render_frame(cx);
            window.click("discover-card-organiser", cx);
            window.render_frame(cx);
            assert_eq!(f.view.read(cx).tool, Tool::Organiser);
        })
        .unwrap();
    }
}
