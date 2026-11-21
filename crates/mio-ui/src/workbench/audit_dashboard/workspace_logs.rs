use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;

impl Workbench {
    pub(super) fn render_dashboard_event_log_tab(
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

    pub(super) fn render_dashboard_logger_tab(&self, cx: &mut Context<Self>) -> AnyElement {
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

    pub(super) fn render_dashboard_audit_log_tab(
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
