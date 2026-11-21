use std::rc::Rc;

use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{ClickEvent, uniform_list};

impl Workbench {
    fn render_summary_vuln_row(
        &self,
        index: usize,
        finding: &AuditFinding,
        row_even_bg: Hsla,
        row_hover_bg: Hsla,
        weak: &WeakEntity<Workbench>,
    ) -> AnyElement {
        let id = finding.id;
        let selected = self.audit_dashboard.selected_finding_id == Some(id);
        let is_fp = self.audit_dashboard.false_positive_findings.contains(&id);
        let has_note = self
            .audit_dashboard
            .finding_notes
            .get(&id)
            .is_some_and(|n| !n.trim().is_empty());
        let hl_color = self
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
        let row_bg = if let Some(hl_bg) = crate::history::highlight_background(hl_color) {
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
                        this.select_audit_finding_in_place(id, false, window, cx);
                    }
                });
            })
            .on_click(move |_: &ClickEvent, window, cx| {
                let _ = click_view.update(cx, |this, cx| {
                    this.select_audit_finding(id, window, cx);
                });
            })
            .context_menu(move |menu, window, cx| {
                issue_menu::build_issue_row_menu(menu_view.clone(), id, menu, window, cx)
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_dashboard_summary_vuln_card(
        &self,
        active_task: Option<&AuditTaskSummary>,
        view_ctx: &DashboardViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sort_asc = view_ctx.summary_sort_asc;
        let sort_view = view_ctx.view.clone();
        let dark = theme::is_dark();

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
                            .on_click(move |_, _, cx| {
                                let _ = sort_view.update(cx, |view, cx| {
                                    view.toggle_summary_sort(cx);
                                });
                            })
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
                            view.render_summary_vuln_row(
                                index,
                                finding,
                                row_even_bg,
                                row_hover_bg,
                                &weak,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(view_ctx.summary_table_scroll)
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
                view_ctx.summary_table_scroll,
                true,
            ));

        div()
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
            .child(table_body)
            .into_any_element()
    }

    fn render_summary_progress_section(
        total_items: u64,
        pending_items: u64,
        in_progress_items: u64,
        completed_items: u64,
        requests_made: u64,
        network_errors: u64,
    ) -> AnyElement {
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
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_dashboard_summary_config_progress_card(
        &self,
        active_task: Option<&AuditTaskSummary>,
        proxy_total: u64,
        is_live_crawl: bool,
        can_operate: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

        div()
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
            .child(div().w_full().h(px(1.)).bg(theme::border()))
            .child(Self::render_summary_progress_section(
                total_items,
                pending_items,
                in_progress_items,
                completed_items,
                requests_made,
                network_errors,
            ))
            .into_any_element()
    }

    #[inline(never)]
    fn render_dashboard_summary_log_card(
        &self,
        active_task: Option<&AuditTaskSummary>,
        recent_history_urls: Vec<String>,
        is_live_crawl: bool,
    ) -> AnyElement {
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

        div()
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
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(super) fn render_dashboard_summary_tab(
        &self,
        active_task: Option<&AuditTaskSummary>,
        view_ctx: &DashboardViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_operate = self.audit_can_operate();
        let left_vuln_card = self.render_dashboard_summary_vuln_card(active_task, view_ctx, cx);

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

        let config_and_progress_card = self.render_dashboard_summary_config_progress_card(
            active_task,
            proxy_total,
            is_live_crawl,
            can_operate,
            cx,
        );
        let task_log_card =
            self.render_dashboard_summary_log_card(active_task, recent_history_urls, is_live_crawl);

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
                    .track_scroll(view_ctx.summary_pane_scroll)
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
                view_ctx.summary_pane_scroll,
                false,
            ))
            .into_any_element()
    }

}
