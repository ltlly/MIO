use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;

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

    #[inline(never)]
    fn render_dashboard_tasks_header_title_row(
        &self,
        can_operate: bool,
        new_scan_open: bool,
        new_live_task_open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_dashboard_tasks_header_actions_row(
        &self,
        can_operate: bool,
        any_running: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_dashboard_tasks_header_filter_row(
        &self,
        active_filter: DashboardTaskFilter,
        has_search: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter_owner = cx.weak_entity();
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
            )
            .into_any_element()
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
            .child(self.render_dashboard_tasks_header_title_row(
                can_operate,
                new_scan_open,
                new_live_task_open,
                cx,
            ))
            .child(self.render_dashboard_tasks_header_actions_row(can_operate, any_running, cx))
            .child(self.render_dashboard_tasks_header_filter_row(active_filter, has_search, cx))
            .into_any_element()
    }

    #[inline(never)]
    fn render_audit_workspace_header_bar(
        &self,
        task_title: String,
        task_running: bool,
        pause_task_id: Option<Uuid>,
        is_live_crawl: bool,
        can_operate: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
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
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_audit_workspace_tab_bar(
        &self,
        active_tab: DashboardTaskTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        tab_bar.into_any_element()
    }

    #[inline(never)]
    fn render_audit_finding_note_bar(
        &self,
        note_fid: Uuid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
    }

    #[inline(never)]
    pub(super) fn render_audit_task_workspace(
        &mut self,
        view_ctx: &DashboardViewContext<'_>,
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

        let header_bar = self.render_audit_workspace_header_bar(
            task_title,
            task_running,
            pause_task_id,
            is_live_crawl,
            can_operate,
            cx,
        );
        let tab_bar = self.render_audit_workspace_tab_bar(active_tab, window, cx);

        let active_task_ref = active_task.as_ref().map(|(_, t)| t);
        let tab_body = match active_tab {
            DashboardTaskTab::Summary => {
                self.render_dashboard_summary_tab(active_task_ref, view_ctx, cx)
            }
            DashboardTaskTab::AuditItems => {
                self.render_dashboard_audit_items_tab(active_task_ref, cx)
            }
            DashboardTaskTab::Issues => self.render_issue_activity_pane(view_ctx, cx),
            DashboardTaskTab::EventLog => self.render_dashboard_event_log_tab(active_task_ref, cx),
            DashboardTaskTab::Logger => self.render_dashboard_logger_tab(cx),
            DashboardTaskTab::AuditLog => self.render_dashboard_audit_log_tab(active_task_ref, cx),
        };

        let note_editor_bar = self
            .audit_dashboard
            .editing_note_finding_id
            .map(|note_fid| self.render_audit_finding_note_bar(note_fid, cx));

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
}
