use super::*;
use gpui_kit::component::Icon;

pub(super) const MIO_AUDIT_ICON_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 11.5 18.5 5v14L4 12.5v-1Z"/><path d="M18.5 9.5a3 3 0 0 1 0 5"/><path d="M8.5 14.3 9.8 19"/></svg>"#;

pub(super) const MIO_WARNING_TRIANGLE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z"/><path d="M12 9v4"/><circle cx="12" cy="17" r="0.6" fill="currentColor"/></svg>"#;

pub(super) const MIO_PROGRESS_PIE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 3v9h9"/></svg>"#;

pub(super) const MIO_CLOCK_LOG_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>"#;

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

pub(super) fn format_finding_time(unix_ms: i64) -> String {
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

pub(super) fn format_finding_host(finding: &AuditFinding) -> String {
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

pub(super) fn format_history_origin(raw: &str) -> String {
    if let Some((scheme, tail)) = raw.trim().split_once("://") {
        let authority = tail.split(['/', '?', '#']).next().unwrap_or("");
        if !authority.is_empty() {
            return format!("{}://{authority}", scheme.to_ascii_lowercase());
        }
    }
    raw.trim().to_owned()
}

#[track_caller]
pub(super) fn render_persistent_v_scrollbar<H>(
    id: &'static str,
    handle: &H,
    left_border: bool,
) -> AnyElement
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

pub(super) fn format_clock_hms(unix_ms: i64) -> String {
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

pub(in crate::workbench) fn severity_badge_style(
    severity: IssueSeverity,
) -> (&'static str, u32, u32) {
    let dark = theme::is_dark();
    match severity {
        IssueSeverity::High => (
            "High 高危",
            if dark { 0x4a2020 } else { 0xfce8e6 },
            if dark { 0xf28b82 } else { 0xc5221f },
        ),
        IssueSeverity::Medium => (
            "Medium 中危",
            if dark { 0x423018 } else { 0xfef7e0 },
            if dark { 0xfdd663 } else { 0xb06000 },
        ),
        IssueSeverity::Low => (
            "Low 低危",
            if dark { 0x1e3554 } else { 0xe8f0fe },
            if dark { 0x8ab4f8 } else { 0x1967d2 },
        ),
        IssueSeverity::Information => (
            "Info 信息",
            if dark { 0x35383d } else { 0xeeeeee },
            if dark { 0x9da2a8 } else { 0x555555 },
        ),
    }
}

pub(in crate::workbench) fn severity_display_style(
    severity: IssueSeverity,
    is_false_positive: bool,
) -> (&'static str, u32, u32) {
    if is_false_positive {
        let dark = theme::is_dark();
        (
            "误报 FP",
            if dark { 0x35383d } else { 0xececec },
            if dark { 0x9da2a8 } else { 0x6b7280 },
        )
    } else {
        severity_badge_style(severity)
    }
}

pub(in crate::workbench) fn confidence_label(confidence: IssueConfidence) -> &'static str {
    match confidence {
        IssueConfidence::Certain => "Certain 确定",
        IssueConfidence::Firm => "Firm 明确",
        IssueConfidence::Tentative => "Tentative 推测",
    }
}

pub(in crate::workbench) fn confidence_display_label(
    confidence: IssueConfidence,
    is_verified: bool,
) -> &'static str {
    if is_verified {
        "Verified 已验证"
    } else {
        confidence_label(confidence)
    }
}

pub(in crate::workbench) fn phase_label(phase: AuditPhase) -> &'static str {
    match phase {
        AuditPhase::Passive => "被动审计 Passive",
        AuditPhase::Active => "主动审计 Active",
    }
}

pub(in crate::workbench) fn format_evidence_spans(
    prefix: &str,
    spans: &[EvidenceSpan],
    evidence: &str,
) -> Option<String> {
    let first = spans.first()?;
    let end = first.offset.saturating_add(first.length);
    let extra = if spans.len() > 1 {
        format!(" (+{} 处)", spans.len() - 1)
    } else {
        String::new()
    };
    let preview: String = evidence.chars().take(96).collect();
    Some(format!(
        "{prefix}: [{}..{end}]{extra} {preview}",
        first.offset
    ))
}

pub(in crate::workbench) fn finding_matches_audit_task(
    finding: &AuditFinding,
    task: &AuditTaskSummary,
) -> bool {
    if task.mode == AuditTaskMode::PassiveOnly && finding.phase == AuditPhase::Active {
        return false;
    }
    if task.started_unix_ms > 0
        && finding.detected_unix_ms > 0
        && finding.detected_unix_ms < task.started_unix_ms
    {
        return false;
    }
    let target = task.target_url.trim();
    if target.is_empty() {
        return false;
    }
    let finding_url = finding.url.trim();
    if finding_url == target {
        return true;
    }
    let target_origin = url_origin_normalized(target);
    let finding_origin = url_origin_normalized(finding_url);
    if let (Some(t_orig), Some(f_orig)) = (&target_origin, &finding_origin) {
        if t_orig != f_orig {
            return false;
        }
        let t_path = url_path_without_query(target);
        if t_path == "/" || t_path.is_empty() {
            return true;
        }
        let f_path = url_path_without_query(finding_url);
        let t_base = t_path.trim_end_matches('/');
        return f_path == t_path
            || f_path == t_base
            || f_path.starts_with(&format!("{t_base}/"));
    }
    !finding.host.is_empty()
        && crate::workbench::wb_proxy::extract_url_host(target)
            .is_some_and(|t_host| t_host.eq_ignore_ascii_case(&finding.host))
}

fn url_origin_normalized(raw: &str) -> Option<(String, String, u16)> {
    let (scheme_raw, tail) = raw.trim().split_once("://")?;
    let scheme = scheme_raw.to_ascii_lowercase();
    let default_port = if scheme == "https" { 443 } else { 80 };
    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let authority = tail[..authority_end]
        .rsplit('@')
        .next()
        .unwrap_or(&tail[..authority_end]);
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (ipv6, port_tail) = rest.split_once(']')?;
        let p = port_tail
            .strip_prefix(':')
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(default_port);
        (format!("[{}]", ipv6.to_ascii_lowercase()), p)
    } else if let Some((h, p_str)) = authority.rsplit_once(':')
        && let Ok(p) = p_str.parse::<u16>()
    {
        (h.to_ascii_lowercase(), p)
    } else {
        (authority.to_ascii_lowercase(), default_port)
    };
    (!host.is_empty()).then_some((scheme, host, port))
}

fn url_path_without_query(raw: &str) -> &str {
    let without_query = raw.trim().split(['?', '#']).next().unwrap_or_default();
    if let Some((_, tail)) = without_query.split_once("://") {
        tail.find('/').map_or("/", |idx| &tail[idx..])
    } else {
        without_query
    }
}
