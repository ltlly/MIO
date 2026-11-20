use std::rc::Rc;

use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{ClickEvent, uniform_list};

const MIO_AUDIT_ICON_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 11.5 18.5 5v14L4 12.5v-1Z"/><path d="M18.5 9.5a3 3 0 0 1 0 5"/><path d="M8.5 14.3 9.8 19"/></svg>"#;

const MIO_WARNING_TRIANGLE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z"/><path d="M12 9v4"/><circle cx="12" cy="17" r="0.6" fill="currentColor"/></svg>"#;

const MIO_PROGRESS_PIE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 3v9h9"/></svg>"#;

const MIO_CLOCK_LOG_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>"#;

const MIO_CIRCLE_PAUSE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9.5"/><path d="M10 8.5v7"/><path d="M14 8.5v7"/></svg>"#;

const MIO_CIRCLE_PLAY_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9.5"/><path d="M10 8.5 16 12l-6 3.5v-7Z" fill="currentColor" stroke="none"/></svg>"#;

const MIO_CIRCLE_STOP_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9.5"/><rect x="8.8" y="8.8" width="6.4" height="6.4" rx="1" fill="currentColor" stroke="none"/></svg>"#;

pub(in crate::workbench) fn mio_blue() -> Hsla {
    rgb(0x26649d).into()
}

pub(in crate::workbench) fn mio_card_selected_bg() -> Hsla {
    rgb(if theme::is_dark() { 0x263447 } else { 0xedf2f8 }).into()
}

pub(in crate::workbench) fn mio_card_selected_border() -> Hsla {
    rgb(if theme::is_dark() { 0x3b5270 } else { 0xd5e0ec }).into()
}

pub(in crate::workbench) fn mio_card_border() -> Hsla {
    rgb(if theme::is_dark() { 0x3c3f41 } else { 0xe5e7eb }).into()
}

pub(super) fn mio_panel_gray_bg() -> Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xf7f7f7 }).into()
}

pub(in crate::workbench) fn mio_card_shadows() -> Vec<BoxShadow> {
    let alpha = if theme::is_dark() { 0.28 } else { 0.08 };
    vec![BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(2.)),
        blur_radius: px(8.),
        spread_radius: px(0.),
        inset: false,
    }]
}

pub(super) fn mio_circle_pause_icon(running: bool, size: gpui_kit::Pixels) -> Icon {
    Icon::default()
        .data(if running {
            MIO_CIRCLE_PAUSE_SVG
        } else {
            MIO_CIRCLE_PLAY_SVG
        })
        .size(size)
}

pub(super) fn mio_circle_stop_icon(size: gpui_kit::Pixels) -> Icon {
    Icon::default().data(MIO_CIRCLE_STOP_SVG).size(size)
}

pub(super) fn count_findings_by_severity<'a>(
    findings: impl Iterator<Item = &'a AuditFinding>,
) -> [usize; 4] {
    let mut counts = [0usize; 4];
    for finding in findings {
        match finding.severity() {
            IssueSeverity::High => counts[0] += 1,
            IssueSeverity::Medium => counts[1] += 1,
            IssueSeverity::Low => counts[2] += 1,
            IssueSeverity::Information => counts[3] += 1,
        }
    }
    counts
}

fn severity_circle_color(severity: IssueSeverity) -> u32 {
    match severity {
        IssueSeverity::High => 0xcc2929,
        IssueSeverity::Medium => 0xe86c3a,
        IssueSeverity::Low => 0x3a71a8,
        IssueSeverity::Information => 0x8c96a0,
    }
}

pub(super) fn render_severity_circle_styled(
    severity: IssueSeverity,
    confidence: IssueConfidence,
    is_false_positive: bool,
) -> AnyElement {
    let (bg_hex, glyph) = if is_false_positive {
        (0x8c96a0, "×")
    } else {
        let g = match severity {
            IssueSeverity::Information => "i",
            _ => match confidence {
                IssueConfidence::Tentative => "?",
                IssueConfidence::Certain | IssueConfidence::Firm => "!",
            },
        };
        (severity_circle_color(severity), g)
    };
    div()
        .size(theme::scaled_px(15.))
        .flex_shrink_0()
        .rounded_full()
        .bg(rgb(bg_hex))
        .text_color(rgb(0xffffff))
        .text_size(theme::scaled_px(10.))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

#[allow(dead_code)]
pub(super) fn render_severity_circle(
    severity: IssueSeverity,
    confidence: IssueConfidence,
) -> AnyElement {
    render_severity_circle_styled(severity, confidence, false)
}

pub(super) fn render_issue_count_pills(
    counts: [usize; 4],
    id_prefix: &str,
    task_id_for_click: Option<Uuid>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let pills = [
        ("high", IssueSeverity::High, 0xcc2929, counts[0], "高危"),
        ("medium", IssueSeverity::Medium, 0xe86c3a, counts[1], "中危"),
        ("low", IssueSeverity::Low, 0x3a71a8, counts[2], "低危"),
        (
            "info",
            IssueSeverity::Information,
            0x9ea5ad,
            counts[3],
            "信息",
        ),
    ];

    let mut row = div()
        .w_full()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(5.))
        .child(
            div()
                .text_size(theme::scaled_px(12.))
                .text_color(theme::foreground())
                .mr(px(2.))
                .child("Issues:"),
        );
    for (suffix, sev, bg_hex, count, cn) in pills {
        row = row.child(
            div()
                .id(SharedString::from(format!("{id_prefix}-pill-{suffix}")))
                .test_support()
                .min_w(theme::scaled_px(28.))
                .h(theme::scaled_px(18.))
                .px(px(7.))
                .rounded_full()
                .bg(rgb(bg_hex))
                .text_color(rgb(0xffffff))
                .text_size(theme::scaled_px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "查看 {cn} 漏洞活动（共 {count} 条）"
                    ))
                    .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.clear_dashboard_intruder_selection(window, cx);
                    this.audit_dashboard.selected_audit_task_id = task_id_for_click;
                    this.audit_dashboard.severity_filter = Some(sev);
                    this.audit_dashboard.active_task_tab = DashboardTaskTab::Issues;
                    cx.notify();
                }))
                .child(count.to_string()),
        );
    }
    row.into_any_element()
}

fn format_finding_time(unix_ms: i64) -> String {
    if unix_ms <= 0 {
        return "—".to_owned();
    }
    let seconds = unix_ms / 1000;
    let Some(cal) = crate::local_time::from_unix_seconds(seconds) else {
        return "—".to_owned();
    };
    format!(
        "{:02}:{:02}:{:02} {:02} {}月",
        cal.hour, cal.min, cal.sec, cal.day, cal.month
    )
}

fn format_finding_host(finding: &AuditFinding) -> String {
    let raw = finding.url.trim();
    if let Some((scheme, tail)) = raw.split_once("://") {
        let authority = tail.split(['/', '?', '#']).next().unwrap_or("");
        if !authority.is_empty() {
            return format!("{}://{authority}", scheme.to_ascii_lowercase());
        }
    }
    if !finding.host.is_empty() {
        finding.host.clone()
    } else {
        "—".to_owned()
    }
}

fn format_history_origin(raw: &str) -> String {
    if let Some((scheme, tail)) = raw.trim().split_once("://") {
        let authority = tail.split(['/', '?', '#']).next().unwrap_or("");
        if !authority.is_empty() {
            return format!("{}://{authority}", scheme.to_ascii_lowercase());
        }
    }
    raw.trim().to_owned()
}

#[track_caller]
fn render_persistent_v_scrollbar<H>(id: &'static str, handle: &H, left_border: bool) -> AnyElement
where
    H: gpui_kit::component::scroll::ScrollbarHandle + Clone + 'static,
{
    let dark = theme::is_dark();
    let track_bg: Hsla = rgb(if dark { 0x26282c } else { 0xf2f2f2 }).into();
    let thumb_bg: Hsla = rgb(if dark { 0x555961 } else { 0xc9c9c9 }).into();
    let thumb_hover_bg: Hsla = rgb(if dark { 0x676c76 } else { 0xb5b5b5 }).into();
    div()
        .id(id)
        .test_support()
        .relative()
        .w(px(11.))
        .h_full()
        .flex_shrink_0()
        .bg(track_bg)
        .when(left_border, |rail| {
            rail.border_l_1().border_color(theme::border())
        })
        .child(
            Scrollbar::vertical(handle)
                .id(SharedString::from(format!("{id}-bar")))
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .styles(move |styles| {
                    styles
                        .track(|s| s.width(px(11.)).bg(track_bg))
                        .track_hover(|s| s.width(px(11.)).bg(track_bg))
                        .track_active(|s| s.width(px(11.)).bg(track_bg))
                        .thumb(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(28.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(28.))
                                .bg(thumb_hover_bg)
                        })
                        .thumb_active(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.5))
                                .min_length(px(28.))
                                .bg(thumb_hover_bg)
                        })
                })
                .max_fps(60),
        )
        .into_any_element()
}

fn format_clock_hms(unix_ms: i64) -> String {
    if unix_ms <= 0 {
        return "—".to_owned();
    }
    let Some(cal) = crate::local_time::from_unix_seconds(unix_ms / 1000) else {
        return "—".to_owned();
    };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        cal.hour,
        cal.min,
        cal.sec,
        unix_ms.rem_euclid(1000)
    )
}

impl Workbench {
    fn dashboard_task_search_keyword(&self, cx: &App) -> String {
        self.audit_dashboard
            .task_search_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase()
    }

    pub(super) fn dashboard_filter_matches_live(&self, cx: &App) -> bool {
        let live_on = self.audit_dashboard.snapshot.live_passive_enabled;
        let filter_ok = match self.audit_dashboard.task_filter {
            DashboardTaskFilter::All | DashboardTaskFilter::LiveTasks => true,
            DashboardTaskFilter::Running => live_on,
            DashboardTaskFilter::Paused => !live_on,
            DashboardTaskFilter::ScanTasks | DashboardTaskFilter::IntruderTasks => false,
        };
        if !filter_ok {
            return false;
        }
        let kw = self.dashboard_task_search_keyword(cx);
        kw.is_empty()
            || "2. 实时被动审计 (proxy 全流量) 被动审计检查 live audit"
                .contains(&kw)
    }

    pub(super) fn dashboard_filter_matches_live_crawl(&self, cx: &App) -> bool {
        let crawl_on = self.audit_dashboard.snapshot.live_passive_crawl_enabled;
        let filter_ok = match self.audit_dashboard.task_filter {
            DashboardTaskFilter::All | DashboardTaskFilter::LiveTasks => true,
            DashboardTaskFilter::Running => crawl_on,
            DashboardTaskFilter::Paused => !crawl_on,
            DashboardTaskFilter::ScanTasks | DashboardTaskFilter::IntruderTasks => false,
        };
        if !filter_ok {
            return false;
        }
        let kw = self.dashboard_task_search_keyword(cx);
        kw.is_empty()
            || "1. 实时被动爬行 (proxy 全流量) 添加链接 同域名 live passive crawl"
                .contains(&kw)
    }

    pub(super) fn dashboard_filter_matches_scan(
        &self,
        task: &AuditTaskSummary,
        cx: &App,
    ) -> bool {
        let filter_ok = match self.audit_dashboard.task_filter {
            DashboardTaskFilter::All | DashboardTaskFilter::ScanTasks => true,
            DashboardTaskFilter::Running => task.state == AuditTaskState::Running,
            DashboardTaskFilter::Paused => task.state == AuditTaskState::Paused,
            DashboardTaskFilter::LiveTasks | DashboardTaskFilter::IntruderTasks => false,
        };
        if !filter_ok {
            return false;
        }
        let kw = self.dashboard_task_search_keyword(cx);
        kw.is_empty()
            || task.title.to_lowercase().contains(&kw)
            || task.target_url.to_lowercase().contains(&kw)
            || task.mode.label().to_lowercase().contains(&kw)
    }

    pub(in crate::workbench) fn dashboard_filter_matches_intruder(
        &self,
        title: &str,
        running: bool,
        cx: &App,
    ) -> bool {
        let filter_ok = match self.audit_dashboard.task_filter {
            DashboardTaskFilter::All | DashboardTaskFilter::IntruderTasks => true,
            DashboardTaskFilter::Running => running,
            DashboardTaskFilter::Paused => !running,
            DashboardTaskFilter::LiveTasks | DashboardTaskFilter::ScanTasks => false,
        };
        if !filter_ok {
            return false;
        }
        let kw = self.dashboard_task_search_keyword(cx);
        kw.is_empty() || title.to_lowercase().contains(&kw) || "intruder".contains(&kw)
    }

    fn toggle_all_dashboard_tasks_pause(&mut self, cx: &mut Context<Self>) {
        if !self.audit_can_operate() {
            return;
        }
        let live_on = self.audit_dashboard.snapshot.live_passive_enabled;
        let crawl_on = self.audit_dashboard.snapshot.live_passive_crawl_enabled;
        let running_scans: Vec<Uuid> = self
            .audit_dashboard
            .snapshot
            .tasks
            .iter()
            .filter(|t| t.state == AuditTaskState::Running)
            .map(|t| t.id)
            .collect();
        let paused_scans: Vec<Uuid> = self
            .audit_dashboard
            .snapshot
            .tasks
            .iter()
            .filter(|t| t.state == AuditTaskState::Paused)
            .map(|t| t.id)
            .collect();

        if live_on || crawl_on || !running_scans.is_empty() {
            if live_on {
                self.set_audit_live_passive(false, cx);
            }
            if crawl_on {
                self.set_audit_live_passive_crawl(false, cx);
            }
            for id in running_scans {
                self.pause_audit_scan(id, cx);
            }
        } else {
            if !live_on {
                self.set_audit_live_passive(true, cx);
            }
            if !crawl_on {
                self.set_audit_live_passive_crawl(true, cx);
            }
            for id in paused_scans {
                self.resume_audit_scan(id, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn render_dashboard_tasks_header(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_operate = self.audit_can_operate();
        let live_on = self.audit_dashboard.snapshot.live_passive_enabled;
        let crawl_on = self.audit_dashboard.snapshot.live_passive_crawl_enabled;
        let any_running = live_on
            || crawl_on
            || self
                .audit_dashboard
                .snapshot
                .tasks
                .iter()
                .any(|t| t.state == AuditTaskState::Running);
        let new_scan_open = self.audit_dashboard.new_scan_open;
        let new_live_task_open = self.audit_dashboard.new_live_task_open;
        let active_filter = self.audit_dashboard.task_filter;
        let has_search = !self
            .audit_dashboard
            .task_search_input
            .read(cx)
            .value()
            .trim()
            .is_empty();
        let filter_owner = cx.weak_entity();

        div()
            .id("dashboard-tasks-header")
            .test_support()
            .flex_shrink_0()
            .px(px(16.))
            .pt(px(14.))
            .pb(px(10.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(11.))
            // Row 1: Tasks title + two External orange primary action buttons
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(16.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("任务"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_shrink_0()
                            .child(
                                Button::new("dashboard-new-scan")
                                    .label("新建扫描")
                                    .primary()
                                    .small()
                                    .h(theme::scaled_px(28.))
                                    .px(theme::scaled_px(11.))
                                    .rounded(px(4.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .selected(new_scan_open)
                                    .disabled(!can_operate)
                                    .tooltip("配置目标 URL 或种子请求并发起主动 / 被动安全扫描")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.audit_dashboard.new_live_task_open = false;
                                        this.audit_dashboard.new_scan_open =
                                            !this.audit_dashboard.new_scan_open;
                                        this.audit_dashboard.scan_error = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("dashboard-live-passive-toggle")
                                    .label("新建实时任务")
                                    .primary()
                                    .small()
                                    .h(theme::scaled_px(28.))
                                    .px(theme::scaled_px(11.))
                                    .rounded(px(4.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .selected(new_live_task_open)
                                    .disabled(!can_operate)
                                    .tooltip("配置并应用实时被动审计、实时被动爬行与流量作用域设置")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_new_live_task_modal(cx);
                                    })),
                            ),
                    ),
            )
            // Row 2: Three clean borderless circular icons (pause/resume, settings, help)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new("dashboard-tasks-pause-all")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(26.))
                            .rounded_full()
                            .child(
                                mio_circle_pause_icon(any_running, theme::scaled_px(20.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label(if any_running {
                                "暂停全部任务"
                            } else {
                                "恢复全部任务"
                            })
                            .tooltip(if any_running {
                                "暂停实时被动审计、被动爬行与正在运行的扫描任务"
                            } else {
                                "恢复实时被动审计、被动爬行与已暂停的扫描任务"
                            })
                            .disabled(!can_operate)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_all_dashboard_tasks_pause(cx);
                            })),
                    )
                    .child(
                        Button::new("dashboard-task-settings")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(26.))
                            .rounded_full()
                            .child(
                                Icon::new(IconName::Settings)
                                    .size(theme::scaled_px(19.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label("任务设置")
                            .tooltip("打开项目任务执行与资源池设置")
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_project_tasks_settings(window, cx)
                            })),
                    )
                    .child(
                        Button::new("dashboard-tasks-help")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(26.))
                            .rounded_full()
                            .child(
                                Icon::new(IconName::CircleQuestionMark)
                                    .size(theme::scaled_px(19.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label("任务帮助")
                            .tooltip(
                                "Dashboard 管理实时被动审计、实时被动爬行、主动/被动扫描及 Intruder 攻击任务；选中左侧卡片可在右侧查看概览与漏洞活动",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.status = "提示：点击「新建扫描」可配置主动/被动扫描；通过卡片开关可随时启停 Proxy 全流量实时任务。".into();
                                cx.notify();
                            })),
                    ),
            )
            // Row 3: Filter dropdown on left + compact search box on right
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        Button::new("dashboard-tasks-filter")
                            .ghost()
                            .small()
                            .h(theme::scaled_px(26.))
                            .px(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(theme::scaled_px(13.))
                                    .text_color(theme::foreground())
                                    .child(
                                        Icon::new(IconName::Funnel)
                                            .size(theme::scaled_px(15.))
                                            .text_color(theme::foreground()),
                                    )
                                    .child(active_filter.short_label()),
                            )
                            .dropdown_caret(true)
                            .tooltip("按状态或类型筛选左侧任务卡片")
                            .dropdown_menu(move |menu, _, cx| {
                                let current = filter_owner
                                    .upgrade()
                                    .map(|e| e.read(cx).audit_dashboard.task_filter)
                                    .unwrap_or_default();
                                [
                                    DashboardTaskFilter::All,
                                    DashboardTaskFilter::Running,
                                    DashboardTaskFilter::Paused,
                                    DashboardTaskFilter::LiveTasks,
                                    DashboardTaskFilter::ScanTasks,
                                    DashboardTaskFilter::IntruderTasks,
                                ]
                                .into_iter()
                                .fold(menu, |menu, option| {
                                    let owner = filter_owner.clone();
                                    menu.item(
                                        PopupMenuItem::new(option.label())
                                            .checked(current == option)
                                            .on_click(move |_, _, cx| {
                                                let _ = owner.update(cx, |view, cx| {
                                                    view.audit_dashboard.task_filter = option;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                })
                            }),
                    )
                    .child(
                        div()
                            .id("dashboard-tasks-search-box")
                            .test_support()
                            .w(theme::scaled_px(142.))
                            .flex_shrink_0()
                            .child(
                                Input::new(&self.audit_dashboard.task_search_input)
                                    .id("dashboard-tasks-search")
                                    .aria_label("搜索任务")
                                    .small()
                                    .h(theme::scaled_px(24.))
                                    .focus_bordered(false)
                                .suffix(
                                    Button::new("dashboard-tasks-search-action")
                                        .ghost()
                                        .compact()
                                        .h(theme::scaled_px(18.))
                                        .icon(
                                            Icon::new(if has_search {
                                                IconName::Close
                                            } else {
                                                IconName::Search
                                            })
                                            .size(theme::scaled_px(13.))
                                            .text_color(theme::muted_foreground()),
                                        )
                                        .accessibility_label(if has_search {
                                            "清除任务搜索"
                                        } else {
                                            "搜索任务"
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if has_search {
                                                this.audit_dashboard.task_search_input.update(
                                                    cx,
                                                    |state, cx| state.set_value("", window, cx),
                                                );
                                                cx.notify();
                                            }
                                        })),
                                ),
                        ),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_audit_task_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_operate = self.audit_can_operate();
        let active_task = self
            .audit_dashboard
            .selected_audit_task_id
            .and_then(|task_id| {
                self.audit_dashboard
                    .snapshot
                    .tasks
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.id == task_id)
                    .map(|(idx, t)| (idx, t.clone()))
            });
        let selected_builtin = self.audit_dashboard.selected_builtin_task;
        let live_scope_suffix = self.audit_dashboard.live_task_scope.card_suffix();
        let (task_title, task_running, pause_task_id, is_live_crawl) = match &active_task {
            Some((idx, task)) => {
                let title = if task.title.is_empty() {
                    format!("{}. 扫描任务 · {}", idx + 3, &task.id.to_string()[..8])
                } else {
                    format!("{}. {}", idx + 3, task.title)
                };
                (
                    title,
                    task.state == AuditTaskState::Running,
                    Some(task.id),
                    false,
                )
            }
            None => match selected_builtin {
                DashboardBuiltinTask::LivePassiveAudit => (
                    format!("2. 实时被动审计 ({live_scope_suffix})"),
                    self.audit_dashboard.snapshot.live_passive_enabled,
                    None,
                    false,
                ),
                DashboardBuiltinTask::LivePassiveCrawl => (
                    format!("1. 实时被动爬行 ({live_scope_suffix})"),
                    self.audit_dashboard.snapshot.live_passive_crawl_enabled,
                    None,
                    true,
                ),
            },
        };
        let active_tab = self.audit_dashboard.active_task_tab;

        let header_bar = div()
            .id("dashboard-workspace-header")
            .test_support()
            .flex_shrink_0()
            .px(px(20.))
            .pt(px(14.))
            .pb(px(10.))
            .bg(theme::surface())
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Icon::default()
                            .data(MIO_AUDIT_ICON_SVG)
                            .size(theme::scaled_px(19.))
                            .text_color(theme::foreground()),
                    )
                    .child(
                        div()
                            .id("dashboard-active-task-title")
                            .test_support()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::scaled_px(16.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child(task_title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_shrink_0()
                    .child(
                        Button::new("dashboard-detail-pause")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(26.))
                            .rounded_full()
                            .child(
                                mio_circle_pause_icon(task_running, theme::scaled_px(19.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label(if task_running {
                                "暂停当前任务"
                            } else {
                                "恢复当前任务"
                            })
                            .tooltip(if task_running {
                                "暂停当前任务"
                            } else {
                                "恢复当前任务"
                            })
                            .disabled(!can_operate)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(tid) = pause_task_id {
                                    if task_running {
                                        this.pause_audit_scan(tid, cx);
                                    } else {
                                        this.resume_audit_scan(tid, cx);
                                    }
                                } else if is_live_crawl {
                                    this.toggle_audit_live_passive_crawl(cx);
                                } else {
                                    this.toggle_audit_live_passive(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("dashboard-detail-settings")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(26.))
                            .rounded_full()
                            .child(
                                Icon::new(IconName::Settings)
                                    .size(theme::scaled_px(19.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label("当前任务设置")
                            .tooltip("打开项目任务与资源池配置")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_project_tasks_settings(window, cx);
                            })),
                    )
                    .child(
                        Button::new("dashboard-detail-help")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(26.))
                            .rounded_full()
                            .child(
                                Icon::new(IconName::CircleQuestionMark)
                                    .size(theme::scaled_px(19.))
                                    .text_color(theme::foreground()),
                            )
                            .accessibility_label("任务详情帮助")
                            .tooltip("查看当前任务的漏洞概览、审计项、漏洞活动、事件日志与审计日志")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.status = "提示：切换上方子标签可查看概览、审计项、完整漏洞活动、事件日志、Logger 与审计日志。".into();
                                cx.notify();
                            })),
                    ),
            );

        let tabs = [
            ("dashboard-tab-summary", "概览", DashboardTaskTab::Summary),
            (
                "dashboard-tab-audit-items",
                "审计项",
                DashboardTaskTab::AuditItems,
            ),
            (
                "dashboard-tab-issues",
                "漏洞活动",
                DashboardTaskTab::Issues,
            ),
            (
                "dashboard-tab-event-log",
                "事件日志",
                DashboardTaskTab::EventLog,
            ),
            ("dashboard-tab-logger", "Logger", DashboardTaskTab::Logger),
            (
                "dashboard-tab-audit-log",
                "审计日志",
                DashboardTaskTab::AuditLog,
            ),
        ];

        let mut tab_bar = div()
            .id("dashboard-workspace-tabs")
            .test_support()
            .relative()
            .flex_shrink_0()
            .w_full()
            .h(theme::scaled_px(34.))
            .px(px(16.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(4.));
        for (id, label, target) in tabs {
            let selected = active_tab == target;
            let dark = theme::is_dark();
            let hover_bg = rgb(if dark { 0x33363b } else { 0xf4f4f5 });
            tab_bar = tab_bar.child(
                flat_button(id, true, window, cx)
                    .role(Role::Tab)
                    .accessibility_label(label)
                    .aria_selected(selected)
                    .h_full()
                    .px(theme::scaled_px(12.))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme::scaled_px(13.))
                    .font_weight(if selected {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if selected && !dark {
                        rgb(0x000000).into()
                    } else {
                        theme::foreground()
                    })
                    .hover(move |s| s.bg(hover_bg))
                    .child(label)
                    .when(selected, |btn| {
                        btn.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .h(px(3.))
                                .bg(theme::accent()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.audit_dashboard.active_task_tab = target;
                        if target == DashboardTaskTab::Logger {
                            this.refresh_logger_history(window, cx);
                        }
                        cx.notify();
                    })),
            );
        }

        let active_task_ref = active_task.as_ref().map(|(_, t)| t);
        let tab_body = match active_tab {
            DashboardTaskTab::Summary => self.render_dashboard_summary_tab(active_task_ref, cx),
            DashboardTaskTab::AuditItems => {
                self.render_dashboard_audit_items_tab(active_task_ref, cx)
            }
            DashboardTaskTab::Issues => self.render_issue_activity_pane(cx),
            DashboardTaskTab::EventLog => self.render_dashboard_event_log_tab(active_task_ref, cx),
            DashboardTaskTab::Logger => self.render_dashboard_logger_tab(cx),
            DashboardTaskTab::AuditLog => self.render_dashboard_audit_log_tab(active_task_ref, cx),
        };

        let note_editor_bar = self.audit_dashboard.editing_note_finding_id.map(|note_fid| {
            let note_title = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .find(|f| f.id == note_fid)
                .map(|f| f.title.clone())
                .unwrap_or_else(|| "当前漏洞".to_owned());
            toolbar()
                .id("dashboard-finding-note-editor-bar")
                .test_support()
                .bg(theme::toolbar_surface())
                .child(
                    Icon::new(IconName::StickyNote)
                        .size(theme::scaled_px(14.))
                        .text_color(theme::accent()),
                )
                .child(
                    div()
                        .max_w(px(220.))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("问题备注：{note_title}")),
                )
                .child(
                    div().flex_1().min_w(px(160.)).child(
                        Input::new(&self.audit_dashboard.finding_note_input)
                            .id("dashboard-finding-note-input")
                            .aria_label("编辑漏洞问题备注")
                            .small(),
                    ),
                )
                .child(
                    button("dashboard-finding-note-save", "保存备注")
                        .primary()
                        .tooltip("保存当前问题备注")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_audit_finding_note(window, cx);
                        })),
                )
                .child(
                    button("dashboard-finding-note-cancel", "取消")
                        .tooltip("取消编辑备注")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_audit_finding_note(window, cx);
                        })),
                )
                .into_any_element()
        });

        div()
            .id("dashboard-task-workspace")
            .test_support()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(header_bar)
            .child(tab_bar)
            .when_some(note_editor_bar, |ws, bar| ws.child(bar))
            .child(div().flex_1().min_h_0().w_full().child(tab_body))
            .into_any_element()
    }

    fn render_dashboard_summary_tab(
        &self,
        active_task: Option<&AuditTaskSummary>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_operate = self.audit_can_operate();
        let sort_asc = self.audit_dashboard.summary_sort_asc;
        let dark = theme::is_dark();

        // Collect findings matching the active task, sorted by severity rank (most serious first by default)
        let scope_only_live = active_task.is_none()
            && self.audit_dashboard.live_task_scope == LiveTaskTrafficScope::SuiteScopeOnly;
        let mut sorted_indices: Vec<(usize, Uuid, u8, i64)> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .enumerate()
            .filter(|(_, f)| match active_task {
                Some(task) => self.finding_belongs_to_audit_task(f, task),
                None if scope_only_live => self.workspace.scope.is_url_in_scope(&f.url),
                None => true,
            })
            .map(|(idx, f)| {
                let rank = if self.audit_dashboard.false_positive_findings.contains(&f.id) {
                    0
                } else {
                    f.severity_rank
                };
                (idx, f.id, rank, f.detected_unix_ms)
            })
            .collect();
        sorted_indices.sort_by(|a, b| {
            let sev_cmp = if sort_asc {
                a.2.cmp(&b.2)
            } else {
                b.2.cmp(&a.2)
            };
            sev_cmp.then_with(|| b.3.cmp(&a.3))
        });
        let row_entries: Vec<(usize, Uuid)> = sorted_indices
            .into_iter()
            .map(|(idx, id, _, _)| (idx, id))
            .collect();

        let table_header_bg: Hsla = rgb(if dark { 0x2f3237 } else { 0xf1f1f1 }).into();
        let row_even_bg: Hsla = rgb(if dark { 0x282a2e } else { 0xfafafa }).into();
        let row_hover_bg: Hsla = rgb(if dark { 0x2b394f } else { 0xedf2f8 }).into();

        // Left rounded gray card: Most serious vulnerabilities found (live)
        let mut table_content = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .bg(table_header_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::foreground())
                    .child(
                        div()
                            .id("dashboard-summary-sort-issue-type")
                            .flex_1()
                            .min_w(px(140.))
                            .h_full()
                            .px(px(10.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.summary_sort_asc =
                                    !this.audit_dashboard.summary_sort_asc;
                                cx.notify();
                            }))
                            .child("漏洞类型")
                            .child(
                                Icon::new(if sort_asc {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronUp
                                })
                                .size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground()),
                            ),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(168.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(10.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("主机"),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(136.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .child("时间"),
                    ),
            );

        if row_entries.is_empty() {
            table_content = table_content.child(
                div()
                    .id("dashboard-summary-issues-empty")
                    .test_support()
                    .flex_1()
                    .p(px(16.))
                    .text_size(theme::scaled_px(12.5))
                    .text_color(theme::muted_foreground())
                    .child("暂未发现漏洞告警。保持实时被动审计开启以自动分析 Proxy 流量，或点击「新建扫描」启动主动/被动安全扫描。"),
            );
        } else {
            let weak = cx.weak_entity();
            let entries_rc = Rc::new(row_entries);
            let count = entries_rc.len();
            table_content = table_content.child(
                uniform_list("dashboard-summary-issue-list", count, move |range, _, cx| {
                    let Some(owner) = weak.upgrade() else {
                        return Vec::new();
                    };
                    let view = owner.read(cx);
                    range
                        .filter_map(|index| {
                            let &(raw_idx, finding_id) = entries_rc.get(index)?;
                            let finding = view
                                .audit_dashboard
                                .snapshot
                                .findings
                                .get(raw_idx)
                                .filter(|item| item.id == finding_id)?;
                            Some((index, finding))
                        })
                        .map(|(index, finding)| {
                            let id = finding.id;
                            let selected = view.audit_dashboard.selected_finding_id == Some(id);
                            let is_fp = view
                                .audit_dashboard
                                .false_positive_findings
                                .contains(&id);
                            let has_note = view
                                .audit_dashboard
                                .finding_notes
                                .get(&id)
                                .is_some_and(|n| !n.trim().is_empty());
                            let hl_color = view
                                .audit_dashboard
                                .finding_highlights
                                .get(&id)
                                .copied()
                                .unwrap_or(0);
                            let host_text = format_finding_host(finding);
                            let time_text = format_finding_time(finding.detected_unix_ms);
                            let row_label = format!(
                                "{} {} {}",
                                if is_fp {
                                    "误报"
                                } else {
                                    finding.severity().label()
                                },
                                finding.title,
                                finding.url
                            );
                            let row_bg = if let Some(hl_bg) =
                                crate::history::highlight_background(hl_color)
                            {
                                hl_bg
                            } else if selected {
                                theme::selected()
                            } else if index % 2 == 1 {
                                row_even_bg
                            } else {
                                theme::surface()
                            };
                            let row_fg = if hl_color > 0 {
                                crate::history::highlight_foreground(hl_color)
                            } else {
                                theme::foreground()
                            };
                            let click_view = weak.clone();
                            let right_view = weak.clone();
                            let menu_view = weak.clone();
                            div()
                                .id(SharedString::from(format!("dashboard-issue-row-{id}")))
                                .test_support()
                                .role(Role::Row)
                                .aria_label(row_label)
                                .aria_selected(selected)
                                .w_full()
                                .h(theme::scaled_px(26.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .bg(row_bg)
                                .when(selected && hl_color > 0, |row| {
                                    row.border_l_2().border_color(mio_blue())
                                })
                                .hover(move |s| {
                                    if hl_color > 0 {
                                        s.bg(row_bg)
                                    } else {
                                        s.bg(row_hover_bg)
                                    }
                                })
                                .text_size(theme::scaled_px(12.5))
                                .text_color(row_fg)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(140.))
                                        .px(px(9.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .overflow_hidden()
                                        .child(render_severity_circle_styled(
                                            finding.severity(),
                                            finding.confidence,
                                            is_fp,
                                        ))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .child(finding.title.clone()),
                                        )
                                        .when(has_note, |cell| {
                                            cell.child(
                                                Icon::new(IconName::StickyNote)
                                                    .size(theme::scaled_px(12.))
                                                    .text_color(row_fg),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .w(theme::scaled_px(168.))
                                        .flex_shrink_0()
                                        .px(px(10.))
                                        .truncate()
                                        .child(host_text),
                                )
                                .child(
                                    div()
                                        .w(theme::scaled_px(136.))
                                        .flex_shrink_0()
                                        .px(px(10.))
                                        .truncate()
                                        .text_size(theme::scaled_px(12.))
                                        .child(time_text),
                                )
                                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                                    let _ = right_view.update(cx, |this, cx| {
                                        if this.audit_dashboard.selected_finding_id != Some(id) {
                                            this.select_audit_finding_in_place(
                                                id, false, window, cx,
                                            );
                                        }
                                    });
                                })
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    let _ = click_view.update(cx, |this, cx| {
                                        this.select_audit_finding(id, window, cx);
                                    });
                                })
                                .context_menu(move |menu, window, cx| {
                                    issue_menu::build_issue_row_menu(
                                        menu_view.clone(),
                                        id,
                                        menu,
                                        window,
                                        cx,
                                    )
                                })
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.audit_dashboard.summary_table_scroll)
                .w_full()
                .flex_1()
                .min_h_0(),
            );
        }

        let table_body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .rounded(px(2.))
            .overflow_hidden()
            .child(table_content)
            .child(render_persistent_v_scrollbar(
                "dashboard-summary-table-scrollbar",
                &self.audit_dashboard.summary_table_scroll,
                true,
            ));

        let left_vuln_card = div()
            .id("dashboard-summary-vuln-card")
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .rounded(px(8.))
            .bg(mio_panel_gray_bg())
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex_shrink_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(
                                Icon::default()
                                    .data(MIO_WARNING_TRIANGLE_SVG)
                                    .size(theme::scaled_px(15.))
                                    .text_color(theme::foreground()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme::scaled_px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::foreground())
                                    .child(if active_task.is_some() {
                                        "发现的最严重漏洞"
                                    } else {
                                        "发现的最严重漏洞 (实时)"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("dashboard-summary-view-all")
                            .test_support()
                            .role(Role::Link)
                            .aria_label("查看全部漏洞活动")
                            .flex_shrink_0()
                            .cursor_pointer()
                            .text_size(theme::scaled_px(12.5))
                            .text_color(theme::accent())
                            .underline()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.audit_dashboard.active_task_tab = DashboardTaskTab::Issues;
                                cx.notify();
                            }))
                            .child("查看全部"),
                    ),
            )
            .child(table_body);

        // Right column: Task configuration + Task progress card, and Task log card
        let history_delegate = self.history.read(cx).delegate();
        let proxy_total = history_delegate.total.max(history_delegate.rows.len()) as u64;
        let recent_history_urls: Vec<String> = history_delegate
            .rows
            .iter()
            .take(5)
            .filter_map(|row| match row {
                HistoryRow::Live(flow) => Some(flow.url.clone()),
                HistoryRow::ExtProxy(ext_archive) => {
                    let req = ext_archive.edited_request.as_ref().or(ext_archive.request.as_ref());
                    let target = req
                        .and_then(|r| r.request_target.as_deref())
                        .unwrap_or("");
                    let origin = ext_archive.origin.as_deref().unwrap_or("");
                    if target.contains("://") {
                        Some(target.to_owned())
                    } else if !origin.is_empty() {
                        Some(format!("{origin}{target}"))
                    } else if !target.is_empty() {
                        Some(target.to_owned())
                    } else {
                        None
                    }
                }
            })
            .collect();

        let is_live_crawl = active_task.is_none()
            && self.audit_dashboard.selected_builtin_task == DashboardBuiltinTask::LivePassiveCrawl;
        let (
            task_type_val,
            scope_val,
            config_val,
            capturing_on,
            capturing_task_id,
            total_items,
            pending_items,
            in_progress_items,
            completed_items,
            requests_made,
            network_errors,
        ) = match active_task {
            Some(task) => {
                let pending = task.total_planned.saturating_sub(task.requests_sent);
                let in_prog = if task.state == AuditTaskState::Running {
                    1
                } else {
                    0
                };
                let net_errs = if task.error.is_some() { 1 } else { 0 };
                (
                    "安全扫描".to_owned(),
                    task.target_url.clone(),
                    task.mode.label().to_owned(),
                    task.state == AuditTaskState::Running,
                    Some(task.id),
                    task.total_planned,
                    pending,
                    in_prog,
                    task.requests_sent,
                    task.requests_sent,
                    net_errs,
                )
            }
            None if is_live_crawl => {
                let crawl_on = self.audit_dashboard.snapshot.live_passive_crawl_enabled;
                let in_prog = if crawl_on && self.proxy_address.is_some() {
                    self.pending_records as u64
                } else {
                    0
                };
                (
                    "实时被动爬行".to_owned(),
                    self.audit_dashboard.live_task_scope.label().to_owned(),
                    "添加链接、同域及范围内 URL".to_owned(),
                    crawl_on,
                    None,
                    proxy_total,
                    0,
                    in_prog,
                    proxy_total,
                    0,
                    0,
                )
            }
            None => {
                let live_on = self.audit_dashboard.snapshot.live_passive_enabled;
                let in_prog = if live_on && self.proxy_address.is_some() {
                    self.pending_records as u64
                } else {
                    0
                };
                let net_errs = if self.audit_dashboard.state_error.is_some() {
                    1
                } else {
                    0
                };
                (
                    "实时审计".to_owned(),
                    self.audit_dashboard.live_task_scope.label().to_owned(),
                    "被动审计检查".to_owned(),
                    live_on,
                    None,
                    proxy_total,
                    0,
                    in_prog,
                    proxy_total,
                    0,
                    net_errs,
                )
            }
        };

        let config_row = |label: &'static str, value: String| {
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(px(8.))
                .text_size(theme::scaled_px(12.5))
                .child(
                    div()
                        .w(theme::scaled_px(96.))
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme::foreground())
                        .child(value),
                )
        };

        let progress_row_left = |label: &'static str, value: u64| {
            div()
                .flex()
                .items_center()
                .text_size(theme::scaled_px(12.5))
                .child(
                    div()
                        .w(theme::scaled_px(110.))
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child(label),
                )
                .child(
                    div()
                        .w(theme::scaled_px(50.))
                        .flex_shrink_0()
                        .text_color(theme::foreground())
                        .child(value.to_string()),
                )
        };

        let progress_row_right = |label: &'static str, value: u64| {
            div()
                .flex()
                .items_center()
                .min_w_0()
                .text_size(theme::scaled_px(12.5))
                .child(
                    div()
                        .w(theme::scaled_px(80.))
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme::foreground())
                        .child(value.to_string()),
                )
        };

        let config_and_progress_card = div()
            .id("dashboard-summary-config-card")
            .test_support()
            .flex_shrink_0()
            .w_full()
            .rounded(px(8.))
            .bg(mio_panel_gray_bg())
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(16.))
            // Task configuration
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .min_w_0()
                                    .child(
                                        Icon::default()
                                            .data(MIO_AUDIT_ICON_SVG)
                                            .size(theme::scaled_px(15.))
                                            .text_color(theme::foreground()),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(theme::scaled_px(13.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::foreground())
                                            .child("任务配置"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("dashboard-summary-view-config")
                                    .test_support()
                                    .role(Role::Link)
                                    .aria_label("查看任务配置")
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .text_size(theme::scaled_px(12.5))
                                    .text_color(theme::accent())
                                    .underline()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_project_tasks_settings(window, cx);
                                    }))
                                    .child("查看配置"),
                            ),
                    )
                    .child(config_row("任务类型:", task_type_val))
                    .child(config_row("作用域:", scope_val))
                    .child(config_row("配置:", config_val))
                    .child(
                        div()
                            .w_full()
                            .pt(px(2.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(theme::scaled_px(12.5))
                            .child(
                                div()
                                    .id("dashboard-summary-capturing-label")
                                    .test_support()
                                    .aria_label(if capturing_on {
                                        "捕获中"
                                    } else {
                                        "已暂停"
                                    })
                                    .w(theme::scaled_px(96.))
                                    .flex_shrink_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if capturing_on {
                                        theme::foreground()
                                    } else {
                                        theme::muted_foreground()
                                    })
                                    .child(if capturing_on { "捕获中" } else { "已暂停" }),
                            )
                            .child(
                                Switch::new("dashboard-summary-capturing-switch")
                                    .color(mio_blue())
                                    .checked(capturing_on)
                                    .disabled(!can_operate)
                                    .accessibility_label("捕获开关")
                                    .tooltip(if capturing_on {
                                        "当前正在捕获并分析流量；点击暂停"
                                    } else {
                                        "当前已暂停；点击开启捕获与分析"
                                    })
                                    .on_change(cx.listener(move |this, _, _, cx| {
                                        if let Some(tid) = capturing_task_id {
                                            if capturing_on {
                                                this.pause_audit_scan(tid, cx);
                                            } else {
                                                this.resume_audit_scan(tid, cx);
                                            }
                                        } else if is_live_crawl {
                                            this.toggle_audit_live_passive_crawl(cx);
                                        } else {
                                            this.toggle_audit_live_passive(cx);
                                        }
                                    })),
                            ),
                    ),
            )
            // Subtle divider
            .child(div().w_full().h(px(1.)).bg(theme::border()))
            // Task progress (two-column left-aligned External metrics grid)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                Icon::default()
                                    .data(MIO_PROGRESS_PIE_SVG)
                                    .size(theme::scaled_px(15.))
                                    .text_color(theme::foreground()),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::foreground())
                                    .child("任务进度"),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(progress_row_left("审计项总数:", total_items))
                                    .child(progress_row_left("待审计项:", pending_items))
                                    .child(progress_row_left("审计进行中:", in_progress_items))
                                    .child(progress_row_left("已完成审计项:", completed_items)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(progress_row_right("已发送请求:", requests_made))
                                    .child(progress_row_right("网络错误:", network_errors)),
                            ),
                    ),
            );

        // Task log lines from real findings / history / task state
        let mut log_lines: Vec<String> = Vec::new();
        if let Some(task) = active_task {
            log_lines.push(format!(
                "> 扫描状态 [{}] \"{}\" ({}/{})",
                task.state.label(),
                task.target_url,
                task.requests_sent,
                task.total_planned
            ));
            for finding in self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .filter(|f| self.finding_belongs_to_audit_task(f, task))
                .take(4)
            {
                log_lines.push(format!(
                    "> 发现 [{}] \"{}\" ({})",
                    finding.severity().label(),
                    finding.title,
                    finding.url
                ));
            }
            if let Some(err) = &task.error {
                log_lines.push(format!("> 错误: {err}"));
            }
        } else if is_live_crawl {
            for url in recent_history_urls {
                log_lines.push(format!("> 正在收录 \"{url}\""));
            }
            if log_lines.is_empty() {
                if self.audit_dashboard.snapshot.live_passive_crawl_enabled {
                    log_lines.push("> 实时被动爬行已开启，等待 Proxy 流量…".to_owned());
                } else {
                    log_lines.push("> 实时被动爬行已暂停".to_owned());
                }
            }
        } else {
            for url in recent_history_urls {
                log_lines.push(format!("> 正在审计 \"{url}\""));
            }
            if log_lines.is_empty() {
                for finding in self.audit_dashboard.snapshot.findings.iter().take(5) {
                    log_lines.push(format!("> 正在审计 \"{}\"", finding.url));
                }
            }
            if log_lines.is_empty() {
                if self.audit_dashboard.snapshot.live_passive_enabled {
                    if let Some(addr) = self.proxy_address {
                        log_lines.push(format!("> 实时被动审计已就绪，监听 Proxy {addr}"));
                    } else {
                        log_lines.push("> 实时被动审计已开启，等待 Proxy 流量…".to_owned());
                    }
                } else {
                    log_lines.push("> 实时被动审计已暂停".to_owned());
                }
            }
        }

        let task_log_card = div()
            .id("dashboard-summary-log-card")
            .test_support()
            .flex_1()
            .min_h(px(130.))
            .w_full()
            .rounded(px(8.))
            .bg(mio_panel_gray_bg())
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .overflow_hidden()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Icon::default()
                            .data(MIO_CLOCK_LOG_SVG)
                            .size(theme::scaled_px(15.))
                            .text_color(theme::foreground()),
                    )
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("任务日志"),
                    ),
            )
            .child(
                div()
                    .id("dashboard-summary-log-lines")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .text_size(theme::scaled_px(12.5))
                    .line_height(theme::scaled_px(17.))
                    .children(log_lines.into_iter().enumerate().map(|(i, line)| {
                        let color = match i {
                            0..=2 => theme::foreground(),
                            3 => theme::foreground().opacity(0.68),
                            4 => theme::foreground().opacity(0.46),
                            _ => theme::foreground().opacity(0.30),
                        };
                        div().w_full().truncate().text_color(color).child(line)
                    })),
            );

        let right_column = div()
            .w(theme::scaled_px(332.))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(config_and_progress_card)
            .child(task_log_card);

        div()
            .id("dashboard-summary-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .bg(theme::surface())
            .flex()
            .overflow_hidden()
            .child(
                div()
                    .id("dashboard-summary-scroll")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.audit_dashboard.summary_pane_scroll)
                    .child(
                        div()
                            .size_full()
                            .min_h(px(420.))
                            .p(px(16.))
                            .flex()
                            .gap(px(16.))
                            .child(left_vuln_card)
                            .child(right_column),
                    ),
            )
            .child(render_persistent_v_scrollbar(
                "dashboard-summary-pane-scrollbar",
                &self.audit_dashboard.summary_pane_scroll,
                false,
            ))
            .into_any_element()
    }

    fn synthetic_audit_item_draft(method: &str, url: &str) -> Draft {
        let trimmed_method = method.trim();
        let clean_method = if trimmed_method.is_empty()
            || trimmed_method == "—"
            || trimmed_method.eq_ignore_ascii_case("SCAN")
        {
            "GET"
        } else {
            trimmed_method
        };
        let (host, path) = if let Some((_, rest)) = url.trim().split_once("://") {
            let without_frag = rest.split('#').next().unwrap_or(rest);
            match without_frag.split_once('/') {
                Some((auth, tail)) => (
                    if auth.is_empty() {
                        "localhost".to_owned()
                    } else {
                        auth.to_owned()
                    },
                    format!("/{tail}"),
                ),
                None => {
                    if let Some((auth, q)) = without_frag.split_once('?') {
                        (
                            if auth.is_empty() {
                                "localhost".to_owned()
                            } else {
                                auth.to_owned()
                            },
                            format!("/?{q}"),
                        )
                    } else {
                        (
                            if without_frag.is_empty() {
                                "localhost".to_owned()
                            } else {
                                without_frag.to_owned()
                            },
                            "/".to_owned(),
                        )
                    }
                }
            }
        } else {
            ("localhost".to_owned(), "/".to_owned())
        };
        Draft {
            revision: 1,
            target: url.trim().to_owned(),
            raw: format!("{clean_method} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes(),
            version: HttpVersion::Http11,
        }
    }

    pub(in crate::workbench) fn open_audit_item_in_repeater(
        &mut self,
        flow_id: Option<FlowId>,
        method: &str,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(fid) = flow_id {
            if self.enqueue(
                Command::OpenRepeater {
                    project,
                    flow_id: fid,
                    original: false,
                },
                cx,
            ) {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Repeater;
                self.status = format!("已在 Repeater 中打开：{url}");
                cx.notify();
            }
            return;
        }
        if url.trim().is_empty() {
            return;
        }
        let draft = Self::synthetic_audit_item_draft(method, url);
        let id = Uuid::new_v4();
        let previous = self.workspace.active;
        let title = self.next_repeater_tab_title(None, Some(&draft.raw));
        self.workspace.tabs.push(WorkTab {
            id,
            title,
            group: None,
            draft,
            last_flow: None,
            archive_response: None,
            history: None,
        });
        self.workspace.active = Some(id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.pop();
            self.workspace.active = previous;
            self.fail(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        if let Some(editors) = self.tab_editors.get(&id) {
            editors
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        self.status = format!("已在 Repeater 中打开：{url}");
        cx.notify();
    }

    pub(in crate::workbench) fn scan_audit_item(
        &mut self,
        method: &str,
        url: &str,
        mode: AuditTaskMode,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() || url.trim().is_empty() {
            return;
        }
        let draft = Self::synthetic_audit_item_draft(method, url);
        let prefix = match mode {
            AuditTaskMode::PassiveOnly => "被动扫描",
            AuditTaskMode::PassiveAndActive => "主动扫描",
        };
        let title = format!("{prefix} · {url}");
        if self.dispatch_audit_scan(title, draft, mode, cx) {
            self.status = format!("已启动{prefix}：{url}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn delete_audit_item(
        &mut self,
        scan_task_id: Option<Uuid>,
        entry_id: Option<HistoryEntryId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task_id) = scan_task_id {
            self.delete_audit_task(task_id, window, cx);
            return;
        }
        let Some(entry_id) = entry_id else {
            return;
        };
        self.audit_dashboard.dismissed_audit_items.insert(entry_id);
        match entry_id {
            HistoryEntryId::Live(flow_id) => {
                if self.audit_can_operate()
                    && self.pending_history_delete.is_none()
                    && let Some(project) = self.project
                {
                    let operation_id = Uuid::new_v4();
                    if self.enqueue(
                        Command::DeleteHttpHistory {
                            project,
                            operation_id,
                            flow_ids: vec![flow_id],
                        },
                        cx,
                    ) {
                        self.pending_history_delete = Some((project, operation_id));
                    }
                }
                self.status = "已删除选中的审计项。".into();
            }
            HistoryEntryId::External { .. } => {
                self.status = "已从审计项列表中移除该条目。".into();
            }
        }
        cx.notify();
    }

    fn render_dashboard_audit_items_tab(
        &self,
        active_task: Option<&AuditTaskSummary>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let header_bg: Hsla = rgb(if dark { 0x2f3237 } else { 0xf1f1f1 }).into();
        let row_hover_bg: Hsla = rgb(if dark { 0x2b394f } else { 0xedf2f8 }).into();

        struct AuditItemRow {
            scan_task_id: Option<Uuid>,
            entry_id: Option<HistoryEntryId>,
            flow_id: Option<FlowId>,
            host: String,
            method: String,
            url: String,
            status: String,
            issues: usize,
            progress: String,
        }

        let scope_only_live = active_task.is_none()
            && self.audit_dashboard.live_task_scope == LiveTaskTrafficScope::SuiteScopeOnly;

        let items: Vec<AuditItemRow> = if let Some(task) = active_task {
            let matching_issues = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .filter(|f| self.finding_belongs_to_audit_task(f, task))
                .count();
            vec![AuditItemRow {
                scan_task_id: Some(task.id),
                entry_id: None,
                flow_id: None,
                host: format_history_origin(&task.target_url),
                method: "SCAN".to_owned(),
                url: task.target_url.clone(),
                status: task.state.label().to_owned(),
                issues: matching_issues,
                progress: format!("{}/{}", task.requests_sent, task.total_planned),
            }]
        } else {
            let findings = &self.audit_dashboard.snapshot.findings;
            self.history
                .read(cx)
                .delegate()
                .rows
                .iter()
                .filter(|row| !self.audit_dashboard.dismissed_audit_items.contains(&row.id()))
                .filter_map(|row| match row {
                    HistoryRow::Live(flow) => {
                        if scope_only_live && !self.workspace.scope.is_url_in_scope(&flow.url) {
                            return None;
                        }
                        let issue_cnt = findings
                            .iter()
                            .filter(|f| f.flow_id == Some(flow.id) || f.url == flow.url)
                            .count();
                        Some(AuditItemRow {
                            scan_task_id: None,
                            entry_id: Some(HistoryEntryId::Live(flow.id)),
                            flow_id: Some(flow.id),
                            host: format_history_origin(&flow.url),
                            method: flow.method.clone(),
                            url: flow.url.clone(),
                            status: flow
                                .status
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "—".into()),
                            issues: issue_cnt,
                            progress: "已完成".into(),
                        })
                    }
                    HistoryRow::ExtProxy(ext_archive) => {
                        let req = ext_archive.edited_request.as_ref().or(ext_archive.request.as_ref());
                        let resp = ext_archive.edited_response.as_ref().or(ext_archive.response.as_ref());
                        let origin = ext_archive.origin.as_deref().unwrap_or("");
                        let target = req
                            .and_then(|r| r.request_target.as_deref())
                            .unwrap_or("");
                        let full_url = if target.contains("://") {
                            target.to_owned()
                        } else if !origin.is_empty() {
                            format!("{origin}{target}")
                        } else {
                            target.to_owned()
                        };
                        if scope_only_live && !self.workspace.scope.is_url_in_scope(&full_url) {
                            return None;
                        }
                        let issue_cnt = findings
                            .iter()
                            .filter(|f| !full_url.is_empty() && f.url == full_url)
                            .count();
                        Some(AuditItemRow {
                            scan_task_id: None,
                            entry_id: Some(HistoryEntryId::External {
                                source_id: ext_archive.item.source_id,
                                item_id: ext_archive.item.id,
                            }),
                            flow_id: None,
                            host: if !origin.is_empty() {
                                format_history_origin(origin)
                            } else {
                                format_history_origin(&full_url)
                            },
                            method: req
                                .and_then(|r| r.method.as_deref())
                                .unwrap_or("—")
                                .to_owned(),
                            url: full_url,
                            status: resp
                                .and_then(|r| r.status)
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "—".into()),
                            issues: issue_cnt,
                            progress: "已导入".into(),
                        })
                    }
                })
                .take(200)
                .collect()
        };

        let mut list = div()
            .id("dashboard-audit-items-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        if items.is_empty() {
            list = list.child(
                div()
                    .p(px(16.))
                    .text_color(theme::muted_foreground())
                    .child("暂无已审计项。通过 Proxy 产生流量或启动扫描任务后，审计项将在此实时列出。"),
            );
        } else {
            let weak = cx.weak_entity();
            for (idx, item) in items.into_iter().enumerate() {
                let menu_view = weak.clone();
                let item_task_id = item.scan_task_id;
                let item_entry_id = item.entry_id;
                let item_flow_id = item.flow_id;
                let item_method = item.method.clone();
                let item_url = item.url.clone();
                let row_label = format!("{} {} {}", item.method, item.host, item.url);
                list = list.child(
                    div()
                        .id(("dashboard-audit-item-row", idx))
                        .test_support()
                        .role(Role::Row)
                        .aria_label(row_label)
                        .h(theme::scaled_px(26.))
                        .flex_shrink_0()
                        .px(px(12.))
                        .border_b_1()
                        .border_color(theme::border())
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(row_hover_bg))
                        .text_size(theme::scaled_px(12.5))
                        .child(
                            div()
                                .w(theme::scaled_px(44.))
                                .flex_shrink_0()
                                .text_color(theme::muted_foreground())
                                .child((idx + 1).to_string()),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(64.))
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(item.method),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(170.))
                                .flex_shrink_0()
                                .truncate()
                                .child(item.host),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(item.url))
                        .child(
                            div()
                                .w(theme::scaled_px(64.))
                                .flex_shrink_0()
                                .child(item.status),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(64.))
                                .flex_shrink_0()
                                .child(item.issues.to_string()),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(84.))
                                .flex_shrink_0()
                                .text_color(theme::muted_foreground())
                                .child(item.progress),
                        )
                        .context_menu(move |menu, _, cx| {
                            let can_operate = menu_view
                                .upgrade()
                                .is_some_and(|e| e.read(cx).audit_can_operate());
                            let has_url = !item_url.trim().is_empty();
                            let rep_owner = menu_view.clone();
                            let rep_method = item_method.clone();
                            let rep_url = item_url.clone();
                            let passive_owner = menu_view.clone();
                            let passive_method = item_method.clone();
                            let passive_url = item_url.clone();
                            let active_owner = menu_view.clone();
                            let active_method = item_method.clone();
                            let active_url = item_url.clone();
                            let scope_owner = menu_view.clone();
                            let scope_url = item_url.clone();
                            let copy_owner = menu_view.clone();
                            let copy_url = item_url.clone();
                            let del_owner = menu_view.clone();
                            menu.item(
                                PopupMenuItem::new("在 Repeater 中打开")
                                    .disabled(!can_operate || (!has_url && item_flow_id.is_none()))
                                    .on_click(move |_, window, cx| {
                                        let _ = rep_owner.update(cx, |v, cx| {
                                            v.open_audit_item_in_repeater(
                                                item_flow_id,
                                                &rep_method,
                                                &rep_url,
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("被动扫描此项")
                                    .disabled(!can_operate || !has_url)
                                    .on_click(move |_, _, cx| {
                                        let _ = passive_owner.update(cx, |v, cx| {
                                            v.scan_audit_item(
                                                &passive_method,
                                                &passive_url,
                                                AuditTaskMode::PassiveOnly,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("主动扫描此项")
                                    .disabled(!can_operate || !has_url)
                                    .on_click(move |_, _, cx| {
                                        let _ = active_owner.update(cx, |v, cx| {
                                            v.scan_audit_item(
                                                &active_method,
                                                &active_url,
                                                AuditTaskMode::PassiveAndActive,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("添加到范围")
                                    .disabled(!can_operate || !has_url)
                                    .on_click(move |_, _, cx| {
                                        let prefix =
                                            crate::workbench::target::target_origin_from_url(
                                                &scope_url,
                                            );
                                        let _ = scope_owner.update(cx, |v, cx| {
                                            if v.workspace.scope.advanced_mode {
                                                v.add_advanced_scope_rule_from_prefix(
                                                    prefix, true, cx,
                                                );
                                            } else {
                                                v.add_scope_rule(v.project, true, prefix, cx);
                                            }
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("复制 URL")
                                    .disabled(!has_url)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_url.clone(),
                                        ));
                                        let _ = copy_owner.update(cx, |v, cx| {
                                            v.status = "已复制审计项 URL 到剪贴板。".into();
                                            cx.notify();
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("删除项")
                                    .disabled(item_task_id.is_none() && item_entry_id.is_none())
                                    .on_click(move |_, window, cx| {
                                        let _ = del_owner.update(cx, |v, cx| {
                                            v.delete_audit_item(
                                                item_task_id,
                                                item_entry_id,
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                        }),
                );
            }
        }

        div()
            .id("dashboard-audit-items-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().w(theme::scaled_px(44.)).flex_shrink_0().child("#"))
                    .child(div().w(theme::scaled_px(64.)).flex_shrink_0().child("方法"))
                    .child(
                        div()
                            .w(theme::scaled_px(170.))
                            .flex_shrink_0()
                            .child("主机"),
                    )
                    .child(div().flex_1().min_w_0().child("URL"))
                    .child(div().w(theme::scaled_px(64.)).flex_shrink_0().child("状态"))
                    .child(div().w(theme::scaled_px(64.)).flex_shrink_0().child("漏洞数"))
                    .child(div().w(theme::scaled_px(84.)).flex_shrink_0().child("进度")),
            )
            .child(list)
            .into_any_element()
    }

    fn render_dashboard_event_log_tab(
        &self,
        _active_task: Option<&AuditTaskSummary>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let header_bg: Hsla = rgb(if dark { 0x2f3237 } else { 0xf1f1f1 }).into();
        let row_hover_bg: Hsla = rgb(if dark { 0x2b394f } else { 0xedf2f8 }).into();

        let entries: Vec<(u64, String, FooterLogSeverity, &'static str, String, String, String)> =
            self.footer_drawer
                .entries
                .iter()
                .rev()
                .map(|entry| {
                    (
                        entry.id,
                        entry.time_label(),
                        entry.severity,
                        entry.severity.label(),
                        entry.message.clone(),
                        entry.detail.clone(),
                        entry.copy_text(),
                    )
                })
                .collect();

        let weak = cx.weak_entity();
        let mut rows_col = div()
            .id("dashboard-event-log-rows")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        if entries.is_empty() {
            rows_col = rows_col.child(
                div()
                    .id("dashboard-event-log-empty")
                    .test_support()
                    .p(px(16.))
                    .text_color(theme::muted_foreground())
                    .child("暂无事件日志记录。"),
            );
        } else {
            for (entry_id, ts, sev, level, msg, detail, copy_single) in entries {
                let fg = match sev {
                    FooterLogSeverity::Error => theme::danger_fg(),
                    FooterLogSeverity::Warning => rgb(if dark { 0xfdd663 } else { 0xb06000 }).into(),
                    FooterLogSeverity::Info => theme::foreground(),
                };
                let row_aria = format!("{ts} {level} {msg}");
                let tooltip_detail = if !detail.is_empty() && detail != msg {
                    format!("{msg}\n{detail}")
                } else {
                    msg.clone()
                };
                let menu_view = weak.clone();
                rows_col = rows_col.child(
                    div()
                        .id(SharedString::from(format!("dashboard-event-log-row-{entry_id}")))
                        .test_support()
                        .role(Role::Row)
                        .aria_label(row_aria)
                        .h(theme::scaled_px(26.))
                        .flex_shrink_0()
                        .px(px(12.))
                        .border_b_1()
                        .border_color(theme::border())
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(row_hover_bg))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tooltip_detail.clone())
                                .build(window, cx)
                        })
                        .text_size(theme::scaled_px(12.5))
                        .child(
                            div()
                                .w(theme::scaled_px(148.))
                                .flex_shrink_0()
                                .text_color(theme::muted_foreground())
                                .truncate()
                                .child(ts),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(84.))
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child(level),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(fg)
                                .child(msg),
                        )
                        .context_menu(move |menu, _, _| {
                            let single_owner = menu_view.clone();
                            let single_text = copy_single.clone();
                            let all_owner = menu_view.clone();
                            let clear_owner = menu_view.clone();
                            menu.item(PopupMenuItem::new("复制消息").on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        single_text.clone(),
                                    ));
                                    let _ = single_owner.update(cx, |v, cx| {
                                        v.status = "已复制事件消息到剪贴板。".into();
                                        cx.notify();
                                    });
                                },
                            ))
                            .item(PopupMenuItem::new("复制全部日志").on_click(
                                move |_, _, cx| {
                                    let _ = all_owner.update(cx, |v, cx| {
                                        let all_text = v
                                            .footer_drawer
                                            .entries
                                            .iter()
                                            .map(|entry| entry.copy_text())
                                            .collect::<Vec<_>>()
                                            .join("\n\n");
                                        let count = v.footer_drawer.entries.len();
                                        cx.write_to_clipboard(ClipboardItem::new_string(all_text));
                                        v.status = format!(
                                            "已复制全部 {count} 条事件日志到剪贴板。"
                                        );
                                        cx.notify();
                                    });
                                },
                            ))
                            .separator()
                            .item(PopupMenuItem::new("清空事件日志").on_click(
                                move |_, _, cx| {
                                    let _ = clear_owner.update(cx, |v, cx| {
                                        v.footer_drawer.clear_events();
                                        v.status = "已清空事件日志。".into();
                                        cx.notify();
                                    });
                                },
                            ))
                        }),
                );
            }
        }

        div()
            .id("dashboard-event-log-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().w(theme::scaled_px(148.)).flex_shrink_0().child("时间"))
                    .child(div().w(theme::scaled_px(84.)).flex_shrink_0().child("级别"))
                    .child(div().flex_1().min_w_0().child("事件消息")),
            )
            .child(rows_col)
            .into_any_element()
    }

    fn render_dashboard_logger_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let header_bg: Hsla = rgb(if dark { 0x2f3237 } else { 0xf1f1f1 }).into();

        let entries: Vec<(String, String, String, String, String)> =
            if !self.extra_tools.logger_entries.is_empty() {
                self.extra_tools
                    .logger_entries
                    .iter()
                    .rev()
                    .take(200)
                    .map(|flow| {
                        (
                            format!("{:?}", flow.source),
                            flow.method.clone(),
                            format_history_origin(&flow.url),
                            flow.url.clone(),
                            flow.status
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "—".into()),
                        )
                    })
                    .collect()
            } else {
                self.history
                    .read(cx)
                    .delegate()
                    .rows
                    .iter()
                    .take(200)
                    .filter_map(|row| match row {
                        HistoryRow::Live(flow) => Some((
                            format!("{:?}", flow.source),
                            flow.method.clone(),
                            format_history_origin(&flow.url),
                            flow.url.clone(),
                            flow.status
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "—".into()),
                        )),
                        HistoryRow::ExtProxy(_) => None,
                    })
                    .collect()
            };

        let mut rows_col = div()
            .id("dashboard-logger-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        if entries.is_empty() {
            rows_col = rows_col.child(
                div()
                    .p(px(16.))
                    .text_color(theme::muted_foreground())
                    .child("暂无实时流量日志。"),
            );
        } else {
            for (idx, (source, method, host, url, status)) in entries.into_iter().enumerate() {
                rows_col = rows_col.child(
                    div()
                        .h(theme::scaled_px(26.))
                        .flex_shrink_0()
                        .px(px(12.))
                        .border_b_1()
                        .border_color(theme::border())
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(theme::scaled_px(12.5))
                        .child(
                            div()
                                .w(theme::scaled_px(40.))
                                .flex_shrink_0()
                                .text_color(theme::muted_foreground())
                                .child((idx + 1).to_string()),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(84.))
                                .flex_shrink_0()
                                .truncate()
                                .child(source),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(60.))
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(method),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(170.))
                                .flex_shrink_0()
                                .truncate()
                                .child(host),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(url))
                        .child(
                            div()
                                .w(theme::scaled_px(56.))
                                .flex_shrink_0()
                                .child(status),
                        ),
                );
            }
        }

        div()
            .id("dashboard-logger-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                toolbar()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("任务关联流量 (Logger)"),
                    )
                    .child(div().flex_1())
                    .child(
                        button("dashboard-open-full-logger", "在 Logger 中打开")
                            .icon(Icon::new(IconName::ExternalLink).size(theme::scaled_px(13.)))
                            .tooltip("切换到完整 Logger 面板查看请求响应细节")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_tool(Tool::Logger, window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().w(theme::scaled_px(40.)).flex_shrink_0().child("#"))
                    .child(div().w(theme::scaled_px(84.)).flex_shrink_0().child("来源"))
                    .child(div().w(theme::scaled_px(60.)).flex_shrink_0().child("方法"))
                    .child(
                        div()
                            .w(theme::scaled_px(170.))
                            .flex_shrink_0()
                            .child("主机"),
                    )
                    .child(div().flex_1().min_w_0().child("URL"))
                    .child(div().w(theme::scaled_px(56.)).flex_shrink_0().child("状态")),
            )
            .child(rows_col)
            .into_any_element()
    }

    fn render_dashboard_audit_log_tab(
        &self,
        active_task: Option<&AuditTaskSummary>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut lines: Vec<String> = Vec::new();
        for task in &self.audit_dashboard.snapshot.tasks {
            if active_task.is_some_and(|t| t.id != task.id) {
                continue;
            }
            lines.push(format!(
                "[{}] [TASK] {} | {} | {} | 请求 {}/{} | 告警 {}",
                format_clock_hms(task.started_unix_ms),
                task.title,
                task.mode.label(),
                task.state.label(),
                task.requests_sent,
                task.total_planned,
                task.issues_found
            ));
        }
        for finding in self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .filter(|f| active_task.is_none_or(|t| self.finding_belongs_to_audit_task(f, t)))
        {
            lines.push(format!(
                "[{}] [{}] [{}] {} — {} ({})",
                format_clock_hms(finding.detected_unix_ms),
                phase_label(finding.phase),
                finding.severity().label(),
                finding.title,
                finding.url,
                finding.evidence
            ));
        }
        let copy_payload = lines.join("\n");
        let has_lines = !lines.is_empty();

        div()
            .id("dashboard-audit-log-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                toolbar()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("审计执行日志"))
                    .child(div().flex_1())
                    .child(
                        button("dashboard-audit-log-copy", "复制日志")
                            .icon(Icon::new(IconName::Copy).size(theme::scaled_px(13.)))
                            .disabled(!has_lines)
                            .tooltip("将当前审计日志复制到剪贴板")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    copy_payload.clone(),
                                ));
                                this.status = "已复制审计日志到剪贴板。".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("dashboard-audit-log-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(14.))
                    .font_family(theme::mono_font_family())
                    .text_size(theme::scaled_px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .when(!has_lines, |col| {
                        col.child(
                            div()
                                .text_color(theme::muted_foreground())
                                .child("暂无审计执行日志记录。"),
                        )
                    })
                    .children(lines.into_iter().map(|line| {
                        div().w_full().text_color(theme::foreground()).child(line)
                    })),
            )
            .into_any_element()
    }
}
