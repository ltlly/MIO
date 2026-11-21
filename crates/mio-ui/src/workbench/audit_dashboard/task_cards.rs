use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;

impl Workbench {
    #[inline(never)]
    fn render_audit_state_error_banner(&self, err: String) -> AnyElement {
        div()
            .id("dashboard-audit-state-error")
            .test_support()
            .role(Role::Alert)
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(if theme::is_dark() { 0x6b2c2a } else { 0xf3c6c3 }))
            .bg(rgb(if theme::is_dark() { 0x4a2020 } else { 0xfce8e6 }))
            .text_color(rgb(if theme::is_dark() { 0xf28b82 } else { 0x9b2727 }))
            .child(format!("审计状态读取失败：{err}"))
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_live_audit_task_card(
        &self,
        live_selected: bool,
        live_passive: bool,
        can_operate: bool,
        live_scope_suffix: &str,
        live_counts: [usize; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let live_title = format!("2. 实时被动审计 ({live_scope_suffix})");
        let menu_owner = cx.weak_entity();
        let live_actions = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                Button::new("dashboard-live-audit-pause")
                    .ghost()
                    .compact()
                    .p_0()
                    .size(theme::scaled_px(24.))
                    .rounded_full()
                    .child(
                        mio_circle_pause_icon(live_passive, theme::scaled_px(18.))
                            .text_color(theme::foreground()),
                    )
                    .accessibility_label(if live_passive {
                        "暂停实时被动审计"
                    } else {
                        "恢复实时被动审计"
                    })
                    .tooltip(if live_passive {
                        "暂停对 Proxy 全流量的实时被动审计"
                    } else {
                        "恢复对 Proxy 全流量的实时被动审计"
                    })
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_audit_live_passive(cx);
                    })),
            )
            .child(
                Button::new("dashboard-live-audit-menu")
                    .ghost()
                    .compact()
                    .p_0()
                    .size(theme::scaled_px(24.))
                    .rounded_full()
                    .child(
                        Icon::new(IconName::EllipsisVertical)
                            .size(theme::scaled_px(17.))
                            .text_color(theme::foreground()),
                    )
                    .accessibility_label("实时被动审计操作菜单")
                    .tooltip("打开操作菜单（刷新状态、导出 JSON/HTML 报告、保存报告、清空漏洞）")
                    .dropdown_menu(move |menu, _, cx| {
                        let Some(entity) = menu_owner.upgrade() else {
                            return menu;
                        };
                        let (enabled, loading, has_findings, picker_busy) = {
                            let v = entity.read(cx);
                            (
                                v.audit_can_operate(),
                                v.audit_dashboard.loading,
                                !v.audit_dashboard.snapshot.findings.is_empty(),
                                v.file_picker_pending(),
                            )
                        };
                        let refresh_owner = menu_owner.clone();
                        let scan_owner = menu_owner.clone();
                        let json_owner = menu_owner.clone();
                        let html_owner = menu_owner.clone();
                        let save_html_owner = menu_owner.clone();
                        let save_json_owner = menu_owner.clone();
                        let clear_owner = menu_owner.clone();
                        let settings_owner = menu_owner.clone();
                        menu.item(
                            PopupMenuItem::new("刷新审计状态")
                                .disabled(!enabled || loading)
                                .on_click(move |_, _, cx| {
                                    let _ = refresh_owner.update(cx, |v, cx| {
                                        if let Some(project) = v.project {
                                            v.query_audit_state(project, cx);
                                        }
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("新建主动 / 被动扫描…")
                                .disabled(!enabled)
                                .on_click(move |_, _, cx| {
                                    let _ = scan_owner.update(cx, |v, cx| {
                                        v.audit_dashboard.new_live_task_open = false;
                                        v.audit_dashboard.new_scan_open = true;
                                        v.audit_dashboard.scan_error = None;
                                        cx.notify();
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("复制 JSON 审计报告")
                                .disabled(!enabled)
                                .on_click(move |_, _, cx| {
                                    let _ = json_owner.update(cx, |v, cx| {
                                        v.copy_audit_report_json(cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("复制 HTML 审计报告")
                                .disabled(!enabled)
                                .on_click(move |_, _, cx| {
                                    let _ = html_owner.update(cx, |v, cx| {
                                        v.copy_audit_report_html(cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("保存为 HTML 审计报告 (.html)…")
                                .disabled(!enabled || picker_busy)
                                .on_click(move |_, window, cx| {
                                    let _ = save_html_owner.update(cx, |v, cx| {
                                        v.save_audit_report_to_file(true, None, window, cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("保存为 JSON 审计报告 (.json)…")
                                .disabled(!enabled || picker_busy)
                                .on_click(move |_, window, cx| {
                                    let _ = save_json_owner.update(cx, |v, cx| {
                                        v.save_audit_report_to_file(false, None, window, cx);
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("清空全部漏洞告警")
                                .disabled(!enabled || !has_findings)
                                .on_click(move |_, window, cx| {
                                    let _ = clear_owner.update(cx, |v, cx| {
                                        v.clear_audit_findings(window, cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("任务与资源池设置…").on_click(
                                move |_, window, cx| {
                                    let _ = settings_owner.update(cx, |v, cx| {
                                        v.open_project_tasks_settings(window, cx);
                                    });
                                },
                            ),
                        )
                    }),
            );

        div()
            .relative()
            .flex_shrink_0()
            .w_full()
            .rounded(px(6.))
            .shadow(mio_card_shadows())
            .child(
                flat_button("dashboard-live-audit-task", true, window, cx)
                    .role(Role::Tab)
                    .accessibility_label(live_title.clone())
                    .aria_selected(live_selected)
                    .disabled(self.transition.is_some())
                    .flex_shrink_0()
                    .relative()
                    .w_full()
                    .overflow_hidden()
                    .p(px(14.))
                    .pr(px(18.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .justify_start()
                    .line_height(theme::scaled_px(18.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if live_selected {
                        mio_card_selected_border()
                    } else {
                        mio_card_border()
                    })
                    .bg(if live_selected {
                        mio_card_selected_bg()
                    } else {
                        theme::surface()
                    })
                    .child(
                        div()
                            .w_full()
                            .pr(theme::scaled_px(54.))
                            .truncate()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::foreground())
                            .child(live_title),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(theme::scaled_px(12.5))
                            .text_color(theme::foreground())
                            .child("被动审计检查"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.))
                            .child(
                                div()
                                    .id("dashboard-live-audit-capturing-label")
                                    .test_support()
                                    .aria_label(if live_passive {
                                        "捕获中"
                                    } else {
                                        "已暂停"
                                    })
                                    .text_size(theme::scaled_px(12.5))
                                    .text_color(if live_passive {
                                        theme::foreground()
                                    } else {
                                        theme::muted_foreground()
                                    })
                                    .child(if live_passive { "捕获中" } else { "已暂停" }),
                            )
                            .child(
                                div()
                                    .id("dashboard-live-audit-capturing-wrap")
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .child(
                                        Switch::new("dashboard-live-audit-capturing")
                                            .color(mio_blue())
                                            .checked(live_passive)
                                            .disabled(!can_operate)
                                            .accessibility_label("实时被动审计捕获开关")
                                            .tooltip(if live_passive {
                                                "实时捕获 Proxy 流量并执行被动漏洞审计"
                                            } else {
                                                "已暂停实时被动审计；点击恢复捕获"
                                            })
                                            .on_change(cx.listener(|this, _, _, cx| {
                                                this.toggle_audit_live_passive(cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(render_issue_count_pills(
                        live_counts,
                        "dashboard-live-audit",
                        None,
                        cx,
                    ))
                    .when(live_selected, |card| {
                        card.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(4.))
                                .bg(mio_blue()),
                        )
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.audit_dashboard.selected_audit_task_id = None;
                        this.audit_dashboard.selected_builtin_task =
                            DashboardBuiltinTask::LivePassiveAudit;
                        this.clear_dashboard_intruder_selection(window, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .absolute()
                    .right(px(11.))
                    .top(px(10.))
                    .child(live_actions),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_live_crawl_task_card(
        &self,
        crawl_selected: bool,
        live_crawl: bool,
        can_operate: bool,
        live_scope_suffix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let crawl_title = format!("1. 实时被动爬行 ({live_scope_suffix})");
        let menu_owner = cx.weak_entity();
        let crawl_actions = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                Button::new("dashboard-live-crawl-pause")
                    .ghost()
                    .compact()
                    .p_0()
                    .size(theme::scaled_px(24.))
                    .rounded_full()
                    .child(
                        mio_circle_pause_icon(live_crawl, theme::scaled_px(18.))
                            .text_color(theme::foreground()),
                    )
                    .accessibility_label(if live_crawl {
                        "暂停实时被动爬行"
                    } else {
                        "恢复实时被动爬行"
                    })
                    .tooltip(if live_crawl {
                        "暂停对 Proxy 流量的实时被动爬行"
                    } else {
                        "恢复对 Proxy 流量的实时被动爬行"
                    })
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_audit_live_passive_crawl(cx);
                    })),
            )
            .child(
                Button::new("dashboard-live-crawl-menu")
                    .ghost()
                    .compact()
                    .p_0()
                    .size(theme::scaled_px(24.))
                    .rounded_full()
                    .child(
                        Icon::new(IconName::EllipsisVertical)
                            .size(theme::scaled_px(17.))
                            .text_color(theme::foreground()),
                    )
                    .accessibility_label("实时被动爬行操作菜单")
                    .tooltip("打开实时被动爬行操作菜单（打开站点地图、切换捕获、任务设置）")
                    .dropdown_menu(move |menu, _, cx| {
                        let enabled = menu_owner
                            .upgrade()
                            .is_some_and(|e| e.read(cx).audit_can_operate());
                        let is_on = menu_owner.upgrade().is_some_and(|e| {
                            e.read(cx).audit_dashboard.snapshot.live_passive_crawl_enabled
                        });
                        let sitemap_owner = menu_owner.clone();
                        let toggle_owner = menu_owner.clone();
                        let settings_owner = menu_owner.clone();
                        menu.item(PopupMenuItem::new("打开站点地图 (Site map)").on_click(
                            move |_, window, cx| {
                                let _ = sitemap_owner.update(cx, |v, cx| {
                                    v.select_tool(Tool::Target, window, cx);
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new(if is_on {
                                "暂停被动爬行捕获"
                            } else {
                                "恢复被动爬行捕获"
                            })
                            .disabled(!enabled)
                            .on_click(move |_, _, cx| {
                                let _ = toggle_owner.update(cx, |v, cx| {
                                    v.toggle_audit_live_passive_crawl(cx);
                                });
                            }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("任务与资源池设置…").on_click(
                                move |_, window, cx| {
                                    let _ = settings_owner.update(cx, |v, cx| {
                                        v.open_project_tasks_settings(window, cx);
                                    });
                                },
                            ),
                        )
                    }),
            );

        div()
            .relative()
            .flex_shrink_0()
            .w_full()
            .rounded(px(6.))
            .shadow(mio_card_shadows())
            .child(
                flat_button("dashboard-live-crawl-task", true, window, cx)
                    .role(Role::Tab)
                    .accessibility_label(crawl_title.clone())
                    .aria_selected(crawl_selected)
                    .disabled(self.transition.is_some())
                    .flex_shrink_0()
                    .relative()
                    .w_full()
                    .overflow_hidden()
                    .p(px(14.))
                    .pr(px(18.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .justify_start()
                    .line_height(theme::scaled_px(18.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if crawl_selected {
                        mio_card_selected_border()
                    } else {
                        mio_card_border()
                    })
                    .bg(if crawl_selected {
                        mio_card_selected_bg()
                    } else {
                        theme::surface()
                    })
                    .child(
                        div()
                            .w_full()
                            .pr(theme::scaled_px(56.))
                            .truncate()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::foreground())
                            .child(crawl_title),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(theme::scaled_px(12.5))
                            .line_height(theme::scaled_px(17.))
                            .text_color(theme::foreground())
                            .child("添加链接。添加当前项本身、同域名以及套件范围内的 URL。"),
                    )
                    .child(
                        div()
                            .pt(px(2.))
                            .flex()
                            .items_center()
                            .gap(px(14.))
                            .child(
                                div()
                                    .id("dashboard-live-crawl-capturing-label")
                                    .test_support()
                                    .aria_label(if live_crawl {
                                        "捕获中"
                                    } else {
                                        "已暂停"
                                    })
                                    .text_size(theme::scaled_px(12.5))
                                    .text_color(if live_crawl {
                                        theme::foreground()
                                    } else {
                                        theme::muted_foreground()
                                    })
                                    .child(if live_crawl { "捕获中" } else { "已暂停" }),
                            )
                            .child(
                                div()
                                    .id("dashboard-live-crawl-capturing-wrap")
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .child(
                                        Switch::new("dashboard-live-crawl-capturing")
                                            .color(mio_blue())
                                            .checked(live_crawl)
                                            .disabled(!can_operate)
                                            .accessibility_label("实时被动爬行捕获开关")
                                            .tooltip(if live_crawl {
                                                "实时捕获 Proxy 流量并自动补充站点地图链接"
                                            } else {
                                                "已暂停实时被动爬行；点击恢复捕获"
                                            })
                                            .on_change(cx.listener(|this, _, _, cx| {
                                                this.toggle_audit_live_passive_crawl(cx);
                                            })),
                                    ),
                            ),
                    )
                    .when(crawl_selected, |card| {
                        card.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(4.))
                                .bg(mio_blue()),
                        )
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.audit_dashboard.selected_audit_task_id = None;
                        this.audit_dashboard.selected_builtin_task =
                            DashboardBuiltinTask::LivePassiveCrawl;
                        this.clear_dashboard_intruder_selection(window, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .absolute()
                    .right(px(11.))
                    .top(px(10.))
                    .child(crawl_actions),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_scan_task_card(
        &self,
        idx: usize,
        task: AuditTaskSummary,
        selected: bool,
        can_operate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let task_id = task.id;
        let running = task.state == AuditTaskState::Running;
        let paused = task.state == AuditTaskState::Paused;
        let progress_line = format!(
            "{} · 请求 {}/{}",
            task.state.label(),
            task.requests_sent,
            task.total_planned,
        );
        let mode_line = format!("{} · {}", task.mode.label(), task.target_url);
        let base_title = if task.title.is_empty() {
            format!("扫描任务 · {}", &task_id.to_string()[..8])
        } else {
            task.title.clone()
        };
        let card_title = format!("{}. {base_title}", idx + 3);
        let task_counts = count_findings_by_severity(
            self.audit_dashboard
                .snapshot
                .findings
                .iter()
                .filter(|f| {
                    !self.audit_dashboard.false_positive_findings.contains(&f.id)
                        && self.finding_belongs_to_audit_task(f, &task)
                }),
        );

        let menu_owner = cx.weak_entity();
        let mut actions = div().flex().items_center().gap(px(2.));
        if running {
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "dashboard-audit-pause-{task_id}"
                )))
                .ghost()
                .compact()
                .p_0()
                .size(theme::scaled_px(24.))
                .rounded_full()
                .child(
                    mio_circle_pause_icon(true, theme::scaled_px(18.))
                        .text_color(theme::foreground()),
                )
                .accessibility_label("暂停扫描任务")
                .tooltip("暂停当前扫描任务")
                .disabled(!can_operate)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.pause_audit_scan(task_id, cx);
                })),
            );
        } else if paused {
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "dashboard-audit-resume-{task_id}"
                )))
                .ghost()
                .compact()
                .p_0()
                .size(theme::scaled_px(24.))
                .rounded_full()
                .child(
                    mio_circle_pause_icon(false, theme::scaled_px(18.))
                        .text_color(theme::foreground()),
                )
                .accessibility_label("恢复扫描任务")
                .tooltip("恢复执行已暂停的扫描任务")
                .disabled(!can_operate)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.resume_audit_scan(task_id, cx);
                })),
            );
        }
        if running || paused {
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "dashboard-audit-stop-{task_id}"
                )))
                .ghost()
                .compact()
                .p_0()
                .size(theme::scaled_px(24.))
                .rounded_full()
                .child(
                    mio_circle_stop_icon(theme::scaled_px(18.))
                        .text_color(theme::foreground()),
                )
                .accessibility_label("停止扫描任务")
                .tooltip("终止当前扫描任务并保留已发现的告警")
                .disabled(!can_operate)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.stop_audit_scan(task_id, cx);
                })),
            );
        }
        actions = actions.child(
            Button::new(SharedString::from(format!(
                "dashboard-audit-menu-{task_id}"
            )))
            .ghost()
            .compact()
            .p_0()
            .size(theme::scaled_px(24.))
            .rounded_full()
            .child(
                Icon::new(IconName::EllipsisVertical)
                    .size(theme::scaled_px(17.))
                    .text_color(theme::foreground()),
            )
            .accessibility_label("扫描任务操作菜单")
            .tooltip("打开扫描任务操作菜单（查看漏洞、导出报告、删除任务）")
            .dropdown_menu(move |menu, window, cx| {
                let Some(entity) = menu_owner.upgrade() else {
                    return menu;
                };
                let (enabled, picker_busy, is_running, is_paused) = {
                    let v = entity.read(cx);
                    let state = v
                        .audit_dashboard
                        .snapshot
                        .tasks
                        .iter()
                        .find(|t| t.id == task_id)
                        .map(|t| t.state);
                    (
                        v.audit_can_operate(),
                        v.file_picker_pending(),
                        state == Some(AuditTaskState::Running),
                        state == Some(AuditTaskState::Paused),
                    )
                };
                let issues_owner = menu_owner.clone();
                let pause_owner = menu_owner.clone();
                let stop_owner = menu_owner.clone();
                let export_owner = menu_owner.clone();
                let del_owner = menu_owner.clone();
                let mut m = menu.item(PopupMenuItem::new("查看任务漏洞").on_click(
                    move |_, window, cx| {
                        let _ = issues_owner.update(cx, |v, cx| {
                            v.audit_dashboard.selected_audit_task_id = Some(task_id);
                            v.audit_dashboard.active_task_tab = DashboardTaskTab::Issues;
                            v.clear_dashboard_intruder_selection(window, cx);
                            cx.notify();
                        });
                    },
                ));
                if is_running {
                    m = m.item(
                        PopupMenuItem::new("暂停扫描")
                            .disabled(!enabled)
                            .on_click(move |_, _, cx| {
                                let _ = pause_owner.update(cx, |v, cx| {
                                    v.pause_audit_scan(task_id, cx);
                                });
                            }),
                    );
                } else if is_paused {
                    m = m.item(
                        PopupMenuItem::new("继续扫描")
                            .disabled(!enabled)
                            .on_click(move |_, _, cx| {
                                let _ = pause_owner.update(cx, |v, cx| {
                                    v.resume_audit_scan(task_id, cx);
                                });
                            }),
                    );
                }
                if is_running || is_paused {
                    m = m.item(
                        PopupMenuItem::new("停止扫描")
                            .disabled(!enabled)
                            .on_click(move |_, _, cx| {
                                let _ = stop_owner.update(cx, |v, cx| {
                                    v.stop_audit_scan(task_id, cx);
                                });
                            }),
                    );
                }
                m.separator()
                    .submenu("导出报告", window, cx, move |sub, _, _| {
                        let save_html_owner = export_owner.clone();
                        let save_json_owner = export_owner.clone();
                        let save_xml_owner = export_owner.clone();
                        let copy_html_owner = export_owner.clone();
                        let copy_json_owner = export_owner.clone();
                        sub.item(
                            PopupMenuItem::new("导出 HTML 报告 (.html)…")
                                .disabled(!enabled || picker_busy)
                                .on_click(move |_, window, cx| {
                                    let _ = save_html_owner.update(cx, |v, cx| {
                                        v.save_audit_report_with_format(
                                            AuditReportFormat::Html,
                                            AuditReportScope::Task(task_id),
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("导出 JSON 报告 (.json)…")
                                .disabled(!enabled || picker_busy)
                                .on_click(move |_, window, cx| {
                                    let _ = save_json_owner.update(cx, |v, cx| {
                                        v.save_audit_report_with_format(
                                            AuditReportFormat::Json,
                                            AuditReportScope::Task(task_id),
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("导出 XML 报告 (.xml)…")
                                .disabled(!enabled || picker_busy)
                                .on_click(move |_, window, cx| {
                                    let _ = save_xml_owner.update(cx, |v, cx| {
                                        v.save_audit_report_with_format(
                                            AuditReportFormat::Xml,
                                            AuditReportScope::Task(task_id),
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("复制任务 HTML 报告")
                                .disabled(!enabled)
                                .on_click(move |_, _, cx| {
                                    let _ = copy_html_owner.update(cx, |v, cx| {
                                        v.copy_audit_report_html_for_scope(
                                            AuditReportScope::Task(task_id),
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("复制任务 JSON 报告")
                                .disabled(!enabled)
                                .on_click(move |_, _, cx| {
                                    let _ = copy_json_owner.update(cx, |v, cx| {
                                        v.copy_audit_report_json_for_scope(
                                            AuditReportScope::Task(task_id),
                                            cx,
                                        );
                                    });
                                }),
                        )
                    })
                    .separator()
                    .item(
                        PopupMenuItem::new("删除扫描任务")
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| {
                                let _ = del_owner.update(cx, |v, cx| {
                                    v.delete_audit_task(task_id, window, cx);
                                });
                            }),
                    )
            }),
        );

        let actions_pad = if running || paused { 78. } else { 32. };

        div()
            .relative()
            .flex_shrink_0()
            .w_full()
            .rounded(px(6.))
            .shadow(mio_card_shadows())
            .child(
                flat_button(
                    SharedString::from(format!("dashboard-audit-task-{task_id}")),
                    true,
                    window,
                    cx,
                )
                .role(Role::Tab)
                .accessibility_label(card_title.clone())
                .aria_selected(selected)
                .disabled(self.transition.is_some())
                .flex_shrink_0()
                .relative()
                .w_full()
                .overflow_hidden()
                .p(px(14.))
                .pr(px(18.))
                .flex()
                .flex_col()
                .items_start()
                .justify_start()
                .line_height(theme::scaled_px(18.))
                .gap(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(if selected {
                    mio_card_selected_border()
                } else {
                    mio_card_border()
                })
                .bg(if selected {
                    mio_card_selected_bg()
                } else {
                    theme::surface()
                })
                .child(
                    div()
                        .w_full()
                        .pr(theme::scaled_px(actions_pad))
                        .truncate()
                        .text_size(theme::scaled_px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child(card_title),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::muted_foreground())
                        .child(mode_line),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::foreground())
                        .child(progress_line),
                )
                .when_some(task.error.clone(), |card, err| {
                    card.child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::danger_fg())
                            .child(format!("错误：{err}")),
                    )
                })
                .child(render_issue_count_pills(
                    task_counts,
                    &format!("dashboard-audit-task-{task_id}"),
                    Some(task_id),
                    cx,
                ))
                .when(selected, |card| {
                    card.child(
                        div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(4.))
                            .bg(mio_blue()),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !this.has_dashboard_intruder_selection()
                        && this.audit_dashboard.selected_audit_task_id == Some(task_id)
                    {
                        this.audit_dashboard.selected_audit_task_id = None;
                    } else {
                        this.audit_dashboard.selected_audit_task_id = Some(task_id);
                        this.audit_dashboard.active_task_tab = DashboardTaskTab::Issues;
                    }
                    this.clear_dashboard_intruder_selection(window, cx);
                    cx.notify();
                })),
            )
            .child(div().absolute().right(px(11.)).top(px(11.)).child(actions))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_audit_task_cards(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let tasks = self.audit_dashboard.snapshot.tasks.clone();
        let can_operate = self.audit_can_operate();
        let live_passive = self.audit_dashboard.snapshot.live_passive_enabled;
        let live_crawl = self.audit_dashboard.snapshot.live_passive_crawl_enabled;
        let live_scope_suffix = self.audit_dashboard.live_task_scope.card_suffix();
        let selected_builtin = self.audit_dashboard.selected_builtin_task;
        let intruder_selected = self.has_dashboard_intruder_selection();
        let selected_task = self.audit_dashboard.selected_audit_task_id;
        let mut elements = Vec::with_capacity(tasks.len() + 3);

        if let Some(err) = self.audit_dashboard.state_error.clone() {
            elements.push(self.render_audit_state_error_banner(err));
        }

        // Built-in card 2 (top, selected by default): Live audit from Proxy (all traffic)
        if self.dashboard_filter_matches_live(cx) {
            let live_selected = !intruder_selected
                && selected_task.is_none()
                && selected_builtin == DashboardBuiltinTask::LivePassiveAudit;
            let live_counts = count_findings_by_severity(
                self.audit_dashboard
                    .snapshot
                    .findings
                    .iter()
                    .filter(|f| !self.audit_dashboard.false_positive_findings.contains(&f.id)),
            );
            elements.push(self.render_live_audit_task_card(
                live_selected,
                live_passive,
                can_operate,
                live_scope_suffix,
                live_counts,
                window,
                cx,
            ));
        }

        // Built-in card 1 (below card 2): Live passive crawl from Proxy (all traffic)
        if self.dashboard_filter_matches_live_crawl(cx) {
            let crawl_selected = !intruder_selected
                && selected_task.is_none()
                && selected_builtin == DashboardBuiltinTask::LivePassiveCrawl;
            elements.push(self.render_live_crawl_task_card(
                crawl_selected,
                live_crawl,
                can_operate,
                live_scope_suffix,
                window,
                cx,
            ));
        }

        // Active / completed scan task cards
        for (idx, task) in tasks.into_iter().enumerate() {
            if !self.dashboard_filter_matches_scan(&task, cx) {
                continue;
            }
            let selected = !intruder_selected && selected_task == Some(task.id);
            elements.push(self.render_scan_task_card(
                idx,
                task,
                selected,
                can_operate,
                window,
                cx,
            ));
        }
        elements
    }

    pub(in crate::workbench) fn finding_belongs_to_audit_task(
        &self,
        finding: &AuditFinding,
        task: &AuditTaskSummary,
    ) -> bool {
        if !finding_matches_audit_task(finding, task) {
            return false;
        }
        if self
            .audit_dashboard
            .task_finding_ids
            .get(&task.id)
            .is_some_and(|ids| ids.contains(&finding.id))
        {
            return true;
        }
        if self
            .audit_dashboard
            .task_finding_ids
            .iter()
            .any(|(other_id, ids)| *other_id != task.id && ids.contains(&finding.id))
        {
            return false;
        }
        true
    }

}
