use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::resizable::{resizable_panel, v_resizable};
use gpui_kit::{ClickEvent, uniform_list};

impl Workbench {
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
            elements.push(
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
                    .into_any_element(),
            );
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

            elements.push(
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
                    .into_any_element(),
            );
        }

        // Built-in card 1 (below card 2): Live passive crawl from Proxy (all traffic)
        if self.dashboard_filter_matches_live_crawl(cx) {
            let crawl_selected = !intruder_selected
                && selected_task.is_none()
                && selected_builtin == DashboardBuiltinTask::LivePassiveCrawl;
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

            elements.push(
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
                    .into_any_element(),
            );
        }

        // Active / completed scan task cards
        for (idx, task) in tasks.into_iter().enumerate() {
            if !self.dashboard_filter_matches_scan(&task, cx) {
                continue;
            }
            let task_id = task.id;
            let selected = !intruder_selected && selected_task == Some(task_id);
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

            elements.push(
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
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
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
                            },
                        )),
                    )
                    .child(div().absolute().right(px(11.)).top(px(11.)).child(actions))
                    .into_any_element(),
            );
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

    pub(in crate::workbench) fn render_issue_activity_pane(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let severity_filter = self.audit_dashboard.severity_filter;
        let keyword = self
            .audit_dashboard
            .issue_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let active_task = if self.has_dashboard_intruder_selection() {
            None
        } else {
            self.audit_dashboard
                .selected_audit_task_id
                .and_then(|task_id| {
                    self.audit_dashboard
                        .snapshot
                        .tasks
                        .iter()
                        .find(|t| t.id == task_id)
                        .cloned()
                })
        };
        let active_task_id = active_task.as_ref().map(|t| t.id);
        let scope_only_live = active_task.is_none()
            && self.audit_dashboard.live_task_scope == LiveTaskTrafficScope::SuiteScopeOnly;
        let selected_id = self.audit_dashboard.selected_finding_id;
        let mut filtered_indices: Vec<(usize, Uuid)> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .enumerate()
            .filter(|(_, finding)| match active_task.as_ref() {
                Some(task) => self.finding_belongs_to_audit_task(finding, task),
                None if scope_only_live => self.workspace.scope.is_url_in_scope(&finding.url),
                None => true,
            })
            .filter(|(_, finding)| {
                severity_filter.is_none_or(|expected| {
                    !self
                        .audit_dashboard
                        .false_positive_findings
                        .contains(&finding.id)
                        && finding.severity() == expected
                })
            })
            .filter(|(_, finding)| {
                keyword.is_empty()
                    || [
                        finding.title.as_str(),
                        finding.issue_id.as_str(),
                        finding.host.as_str(),
                        finding.url.as_str(),
                        finding.parameter.as_deref().unwrap_or(""),
                        finding.summary.as_str(),
                        finding.evidence.as_str(),
                        self.audit_dashboard
                            .finding_notes
                            .get(&finding.id)
                            .map(String::as_str)
                            .unwrap_or(""),
                    ]
                    .into_iter()
                    .any(|field| field.to_lowercase().contains(&keyword))
            })
            .map(|(idx, finding)| (idx, finding.id))
            .collect();

        if let Some((sort_col, asc)) = self.audit_dashboard.issue_sort {
            let findings = &self.audit_dashboard.snapshot.findings;
            let fp_set = &self.audit_dashboard.false_positive_findings;
            let ver_set = &self.audit_dashboard.verified_confidence_findings;
            filtered_indices.sort_by(|(idx_a, id_a), (idx_b, id_b)| {
                let fa = &findings[*idx_a];
                let fb = &findings[*idx_b];
                let ord = match sort_col {
                    DashboardIssueSortColumn::Severity => {
                        let rank_a = if fp_set.contains(id_a) {
                            0
                        } else {
                            fa.severity().rank()
                        };
                        let rank_b = if fp_set.contains(id_b) {
                            0
                        } else {
                            fb.severity().rank()
                        };
                        rank_a.cmp(&rank_b).then_with(|| fa.title.cmp(&fb.title))
                    }
                    DashboardIssueSortColumn::IssueType => fa.title.cmp(&fb.title),
                    DashboardIssueSortColumn::Host => {
                        let ha = match &fa.parameter {
                            Some(p) if !p.is_empty() => format!("{} ({p})", fa.host),
                            _ => fa.host.clone(),
                        };
                        let hb = match &fb.parameter {
                            Some(p) if !p.is_empty() => format!("{} ({p})", fb.host),
                            _ => fb.host.clone(),
                        };
                        ha.cmp(&hb)
                    }
                    DashboardIssueSortColumn::Url => fa.url.cmp(&fb.url),
                    DashboardIssueSortColumn::Confidence => {
                        let ca = if ver_set.contains(id_a) {
                            4u8
                        } else {
                            match fa.confidence {
                                IssueConfidence::Certain => 3,
                                IssueConfidence::Firm => 2,
                                IssueConfidence::Tentative => 1,
                            }
                        };
                        let cb = if ver_set.contains(id_b) {
                            4u8
                        } else {
                            match fb.confidence {
                                IssueConfidence::Certain => 3,
                                IssueConfidence::Firm => 2,
                                IssueConfidence::Tentative => 1,
                            }
                        };
                        ca.cmp(&cb)
                    }
                }
                .then_with(|| idx_a.cmp(idx_b));
                if asc { ord } else { ord.reverse() }
            });
        }

        let grouped_evidence = self
            .user_preferences
            .confirmed
            .as_ref()
            .map(|p| p.display.issues_grouped_evidence)
            .unwrap_or(self.user_preferences.display.issues_grouped_evidence);

        let selected_finding = selected_id.and_then(|id| {
            self.audit_dashboard
                .snapshot
                .findings
                .iter()
                .find(|f| f.id == id)
                .cloned()
        });

        let can_operate = self.audit_can_operate();
        let finding_count = self.audit_dashboard.snapshot.findings.len();
        let loading = self.audit_dashboard.loading;
        let picker_busy = self.file_picker_pending();
        let save_menu_owner = cx.weak_entity();
        let toolbar_scope = match active_task_id {
            Some(tid) => AuditReportScope::Task(tid),
            None => AuditReportScope::All,
        };

        let filter_bar = toolbar()
            .id("dashboard-issue-filter-bar")
            .test_support()
            .children(
                [
                    ("dashboard-severity-all", "全部", None),
                    ("dashboard-severity-high", "High", Some(IssueSeverity::High)),
                    (
                        "dashboard-severity-medium",
                        "Medium",
                        Some(IssueSeverity::Medium),
                    ),
                    ("dashboard-severity-low", "Low", Some(IssueSeverity::Low)),
                    (
                        "dashboard-severity-info",
                        "Information",
                        Some(IssueSeverity::Information),
                    ),
                ]
                .into_iter()
                .map(|(id, label, target)| {
                    button(id, label)
                        .tooltip(format!("按严重度筛选漏洞活动：{label}"))
                        .selected(severity_filter == target)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.audit_dashboard.severity_filter = target;
                            cx.notify();
                        }))
                }),
            )
            .child(
                div().min_w(px(120.)).max_w(px(210.)).flex_1().child(
                    Input::new(&self.audit_dashboard.issue_filter_input)
                        .id("dashboard-issue-filter")
                        .aria_label("漏洞活动关键字过滤")
                        .small(),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child(format!(
                        "显示 {}/{}{}",
                        filtered_indices.len(),
                        finding_count,
                        if grouped_evidence { " · 分组" } else { "" }
                    )),
            )
            .child(
                button("dashboard-audit-refresh", "刷新")
                    .icon(Icon::new(IconName::RefreshCw).size(theme::scaled_px(12.)))
                    .tooltip("从后台服务重新同步扫描任务与漏洞列表")
                    .disabled(!can_operate || loading)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(project) = this.project {
                            this.query_audit_state(project, cx);
                        }
                    })),
            )
            .child(
                button("dashboard-copy-json-report", "复制 JSON")
                    .icon(Icon::new(IconName::Copy).size(theme::scaled_px(12.)))
                    .tooltip("将当前漏洞告警导出为结构化 JSON 复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_audit_report_json_for_scope(toolbar_scope, cx);
                    })),
            )
            .child(
                button("dashboard-copy-html-report", "复制 HTML")
                    .icon(Icon::new(IconName::Download).size(theme::scaled_px(12.)))
                    .tooltip("将当前漏洞告警生成独立 HTML 审计报告复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_audit_report_html_for_scope(toolbar_scope, cx);
                    })),
            )
            .child(
                button("dashboard-save-report-file", "保存报告…")
                    .icon(Icon::new(IconName::Download).size(theme::scaled_px(12.)))
                    .dropdown_caret(true)
                    .tooltip("将当前漏洞审计报告导出为 HTML / JSON / XML 文件写入磁盘")
                    .disabled(!can_operate || picker_busy)
                    .dropdown_menu(move |menu, _, cx| {
                        let Some(entity) = save_menu_owner.upgrade() else {
                            return menu;
                        };
                        let enabled = {
                            let view = entity.read(cx);
                            view.audit_can_operate() && !view.file_picker_pending()
                        };
                        let html_owner = save_menu_owner.clone();
                        let json_owner = save_menu_owner.clone();
                        let xml_owner = save_menu_owner.clone();
                        menu.item(
                            PopupMenuItem::new("保存为 HTML 审计报告 (.html)…")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = html_owner.update(cx, |view, cx| {
                                        view.save_audit_report_with_format(
                                            AuditReportFormat::Html,
                                            toolbar_scope,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("保存为 JSON 审计报告 (.json)…")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = json_owner.update(cx, |view, cx| {
                                        view.save_audit_report_with_format(
                                            AuditReportFormat::Json,
                                            toolbar_scope,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("保存为 XML 审计报告 (.xml)…")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = xml_owner.update(cx, |view, cx| {
                                        view.save_audit_report_with_format(
                                            AuditReportFormat::Xml,
                                            toolbar_scope,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                    }),
            )
            .child(
                button("dashboard-clear-issues", "清空")
                    .icon(Icon::new(IconName::Trash).size(theme::scaled_px(12.)))
                    .tooltip("清空当前项目中的所有扫描发现与漏洞告警记录")
                    .disabled(!can_operate || finding_count == 0)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.clear_audit_findings(window, cx);
                    })),
            );

        let task_filter_banner = active_task.as_ref().map(|task| {
            let task_title = if task.title.is_empty() {
                format!("扫描任务 · {}", &task.id.to_string()[..8])
            } else {
                task.title.clone()
            };
            let hint = format!(
                "当前按任务筛选：{}（目标 {}）",
                task_title, task.target_url
            );
            toolbar()
                .id("dashboard-task-filter-banner")
                .test_support()
                .bg(theme::toolbar_surface())
                .child(
                    div()
                        .id("dashboard-task-filter-label")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(hint.clone())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(hint),
                )
                .child(
                    button("dashboard-show-all-task-issues", "显示全部任务漏洞")
                        .icon(IconName::Eye)
                        .tooltip("清除当前扫描任务关联筛选，展示全部漏洞告警")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.audit_dashboard.selected_audit_task_id = None;
                            cx.notify();
                        })),
                )
                .into_any_element()
        });

        let list_section =
            self.render_issue_activity_list(filtered_indices, selected_id, grouped_evidence, cx);
        let detail_section = match selected_finding {
            Some(finding) => self.render_issue_finding_detail(&finding, cx),
            None => div()
                .id("dashboard-issue-detail-empty")
                .test_support()
                .size_full()
                .min_h_0()
                .p(px(16.))
                .text_color(rgb(if theme::is_dark() { 0x9da2a8 } else { 0x666666 }))
                .child("选择上方任意漏洞条目查看修复建议（Advisory）、关联请求响应报文及证据高亮偏移。")
                .into_any_element(),
        };

        div()
            .id("dashboard-issue-activity-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(filter_bar)
            .when_some(task_filter_banner, |pane, banner| pane.child(banner))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("dashboard-issues-vsplit")
                        .child(
                            resizable_panel()
                                .size(px(220.))
                                .size_range(px(110.)..px(720.))
                                .child(list_section),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(120.)..px(10000.))
                                .child(detail_section),
                        ),
                ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn render_issue_activity_list(
        &self,
        filtered_indices: Vec<(usize, Uuid)>,
        selected_id: Option<Uuid>,
        grouped_evidence: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[derive(Clone)]
        enum IssueListRow {
            Single {
                raw_idx: usize,
                finding_id: Uuid,
            },
            GroupHeader {
                group_key: String,
                lead_raw_idx: usize,
                lead_finding_id: Uuid,
                member_ids: Vec<Uuid>,
                count: usize,
                expanded: bool,
            },
            GroupChild {
                raw_idx: usize,
                finding_id: Uuid,
            },
        }

        let all_finding_ids: std::rc::Rc<Vec<Uuid>> =
            std::rc::Rc::new(filtered_indices.iter().map(|&(_, id)| id).collect());

        let display_rows: Vec<IssueListRow> = if grouped_evidence {
            let mut group_order: Vec<(String, Vec<(usize, Uuid)>)> = Vec::new();
            let mut key_to_pos: HashMap<String, usize> = HashMap::new();
            for (raw_idx, finding_id) in filtered_indices {
                let Some(finding) = self.audit_dashboard.snapshot.findings.get(raw_idx) else {
                    continue;
                };
                let key = format!("{}\0{}", finding.issue_id, finding.title);
                if let Some(&pos) = key_to_pos.get(&key) {
                    group_order[pos].1.push((raw_idx, finding_id));
                } else {
                    key_to_pos.insert(key.clone(), group_order.len());
                    group_order.push((key, vec![(raw_idx, finding_id)]));
                }
            }
            let mut out = Vec::with_capacity(group_order.len());
            for (group_key, members) in group_order {
                if members.len() == 1 {
                    let (raw_idx, finding_id) = members[0];
                    out.push(IssueListRow::Single {
                        raw_idx,
                        finding_id,
                    });
                } else {
                    let (lead_raw_idx, lead_finding_id) = members[0];
                    let member_ids: Vec<Uuid> = members.iter().map(|&(_, id)| id).collect();
                    let expanded = self
                        .audit_dashboard
                        .expanded_issue_groups
                        .contains(&group_key);
                    out.push(IssueListRow::GroupHeader {
                        group_key,
                        lead_raw_idx,
                        lead_finding_id,
                        member_ids,
                        count: members.len(),
                        expanded,
                    });
                    if expanded {
                        for (raw_idx, finding_id) in members {
                            out.push(IssueListRow::GroupChild {
                                raw_idx,
                                finding_id,
                            });
                        }
                    }
                }
            }
            out
        } else {
            filtered_indices
                .into_iter()
                .map(|(raw_idx, finding_id)| IssueListRow::Single {
                    raw_idx,
                    finding_id,
                })
                .collect()
        };

        let visible_order: std::rc::Rc<Vec<Uuid>> = std::rc::Rc::new(
            display_rows
                .iter()
                .map(|row| match row {
                    IssueListRow::Single { finding_id, .. }
                    | IssueListRow::GroupChild { finding_id, .. } => *finding_id,
                    IssueListRow::GroupHeader {
                        lead_finding_id, ..
                    } => *lead_finding_id,
                })
                .collect(),
        );

        let col_widths = self.audit_dashboard.issue_col_widths;
        let col_visible = self.audit_dashboard.issue_col_visible;
        let issue_sort = self.audit_dashboard.issue_sort;
        let visible_cols: Vec<usize> = (0..DASHBOARD_ISSUE_COL_COUNT)
            .filter(|&i| col_visible[i])
            .collect();
        let flex_col_ix = if col_visible[3] {
            3
        } else {
            visible_cols.last().copied().unwrap_or(0)
        };
        let workbench_id = cx.entity_id();
        let weak = cx.weak_entity();
        let header_defs: [(&'static str, &'static str, DashboardIssueSortColumn);
            DASHBOARD_ISSUE_COL_COUNT] = [
            (
                "dashboard-issue-col-severity",
                "严重度",
                DashboardIssueSortColumn::Severity,
            ),
            (
                "dashboard-issue-col-title",
                "漏洞名称",
                DashboardIssueSortColumn::IssueType,
            ),
            (
                "dashboard-issue-col-host",
                "主机 / 参数",
                DashboardIssueSortColumn::Host,
            ),
            ("dashboard-issue-col-url", "URL", DashboardIssueSortColumn::Url),
            (
                "dashboard-issue-col-confidence",
                "置信度",
                DashboardIssueSortColumn::Confidence,
            ),
        ];

        let header_cells: Vec<AnyElement> = visible_cols
            .iter()
            .map(|&col_ix| {
                let (id, label, sort_col) = header_defs[col_ix];
                let width = col_widths[col_ix];
                let active_asc = issue_sort
                    .filter(|(c, _)| *c == sort_col)
                    .map(|(_, asc)| asc);
                let ctx_view = weak.clone();
                let ctx_ids = std::rc::Rc::clone(&all_finding_ids);
                let fit_ids = std::rc::Rc::clone(&all_finding_ids);
                let resize_handle = div()
                    .id(("dashboard-issue-col-resize", col_ix))
                    .occlude()
                    .cursor_col_resize()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(6.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count == 2 {
                                this.fit_audit_issue_column_width(col_ix, &fit_ids, cx);
                            } else {
                                this.begin_audit_issue_column_resize(
                                    col_ix,
                                    event.position.x.as_f32(),
                                );
                            }
                        }),
                    )
                    .on_drag(
                        ResizeDashboardIssueCol {
                            entity_id: workbench_id,
                            col_ix,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| *drag)
                        },
                    );
                let cell = div()
                    .id(id)
                    .test_support()
                    .role(Role::ColumnHeader)
                    .aria_label(label)
                    .relative()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pr(px(6.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_audit_issue_sort(sort_col, cx);
                    }))
                    .context_menu(move |menu, window, cx| {
                        issue_menu::build_issue_header_context_menu(
                            ctx_view.clone(),
                            col_ix,
                            sort_col,
                            std::rc::Rc::clone(&ctx_ids),
                            col_visible,
                            menu,
                            window,
                            cx,
                        )
                    })
                    .child(div().min_w_0().truncate().child(label))
                    .when_some(active_asc, |c, asc| {
                        c.child(
                            Icon::new(if asc {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(theme::scaled_px(11.))
                            .text_color(theme::muted_foreground())
                            .flex_shrink_0(),
                        )
                    })
                    .child(resize_handle);
                if col_ix == flex_col_ix {
                    cell.flex_1()
                        .min_w(theme::scaled_px(width).min(px(140.)))
                        .into_any_element()
                } else {
                    cell.w(theme::scaled_px(width))
                        .flex_shrink_0()
                        .into_any_element()
                }
            })
            .collect();

        let mut rows = div()
            .id("dashboard-issue-rows")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col();

        rows = rows.child(
            div()
                .min_h(theme::scaled_px(28.))
                .h(theme::scaled_px(28.))
                .px(px(10.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(theme::toolbar_surface())
                .border_b_1()
                .border_color(theme::border())
                .text_size(theme::scaled_px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .on_drag_move::<ResizeDashboardIssueCol>(cx.listener(
                    |this, event: &DragMoveEvent<ResizeDashboardIssueCol>, _, cx| {
                        let drag = *event.drag(cx);
                        if drag.entity_id == cx.entity_id() {
                            this.update_audit_issue_column_resize(
                                drag.col_ix,
                                event.event.position.x.as_f32(),
                                cx,
                            );
                        }
                    },
                ))
                .children(header_cells),
        );

        if display_rows.is_empty() {
            rows = rows.child(
                div()
                    .id("dashboard-issue-empty")
                    .test_support()
                    .p(px(14.))
                    .text_color(rgb(if theme::is_dark() { 0x9da2a8 } else { 0x666666 }))
                    .child(if self.audit_dashboard.snapshot.findings.is_empty() {
                        "尚无实时漏洞告警。可通过实时被动审计捕获 Proxy/Repeater 流量，或点击「新建扫描」发起主动/被动扫描。"
                    } else {
                        "当前筛选条件下没有匹配的漏洞告警。"
                    }),
            );
        } else {
            let display_rc = std::rc::Rc::new(display_rows);
            let count = display_rc.len();
            let selected_set_rc =
                std::rc::Rc::new(self.audit_dashboard.selected_finding_ids.clone());
            let zebra = theme::table_zebra_stripe();
            let stripe_bg: Hsla = rgb(if theme::is_dark() { 0x25272b } else { 0xfafafa }).into();
            rows = rows.child(
                uniform_list("dashboard-issue-list", count, move |range, _, cx| {
                    let Some(owner) = weak.upgrade() else {
                        return Vec::new();
                    };
                    let view = owner.read(cx);
                    range
                        .filter_map(|index| {
                            let row_kind = display_rc.get(index)?;
                            let (raw_idx, finding_id) = match row_kind {
                                IssueListRow::Single {
                                    raw_idx,
                                    finding_id,
                                }
                                | IssueListRow::GroupChild {
                                    raw_idx,
                                    finding_id,
                                } => (*raw_idx, *finding_id),
                                IssueListRow::GroupHeader {
                                    lead_raw_idx,
                                    lead_finding_id,
                                    ..
                                } => (*lead_raw_idx, *lead_finding_id),
                            };
                            let finding = view
                                .audit_dashboard
                                .snapshot
                                .findings
                                .get(raw_idx)
                                .filter(|item| item.id == finding_id)?;
                            Some((index, row_kind, finding))
                        })
                        .map(|(index, row_kind, finding)| {
                            let id = finding.id;
                            let is_child = matches!(row_kind, IssueListRow::GroupChild { .. });
                            let (group_info, row_members) = match row_kind {
                                IssueListRow::GroupHeader {
                                    group_key,
                                    count,
                                    expanded,
                                    member_ids,
                                    ..
                                } => (Some((group_key.clone(), *count, *expanded)), member_ids.clone()),
                                _ => (None, vec![id]),
                            };
                            let selected = row_members
                                .iter()
                                .any(|mid| selected_set_rc.contains(mid))
                                || (selected_set_rc.is_empty() && selected_id == Some(id));
                            let is_fp = view
                                .audit_dashboard
                                .false_positive_findings
                                .contains(&id);
                            let is_verified = view
                                .audit_dashboard
                                .verified_confidence_findings
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
                            let (sev_text, badge_bg, badge_fg) =
                                severity_display_style(finding.severity(), is_fp);
                            let host_param = match &finding.parameter {
                                Some(param) if !param.is_empty() => {
                                    format!("{} ({param})", finding.host)
                                }
                                _ => finding.host.clone(),
                            };
                            let row_label = match &group_info {
                                Some((_, grp_count, _)) => format!(
                                    "{} {} [{}] {}",
                                    sev_text, finding.title, grp_count, finding.url
                                ),
                                None => format!("{} {} {}", sev_text, finding.title, finding.url),
                            };
                            let row_id = if is_child {
                                SharedString::from(format!("dashboard-issue-evidence-row-{id}"))
                            } else {
                                SharedString::from(format!("dashboard-issue-activity-row-{id}"))
                            };
                            let row_bg = if let Some(hl_bg) =
                                crate::history::highlight_background(hl_color)
                            {
                                hl_bg
                            } else if selected {
                                theme::selected()
                            } else if zebra && index % 2 == 1 {
                                stripe_bg
                            } else {
                                theme::surface()
                            };
                            let row_fg = if hl_color > 0 {
                                crate::history::highlight_foreground(hl_color)
                            } else {
                                theme::foreground()
                            };
                            let conf_fg = if hl_color > 0 {
                                crate::history::highlight_foreground(hl_color)
                            } else {
                                rgb(if theme::is_dark() { 0x9da2a8 } else { 0x555555 }).into()
                            };
                            let right_view = weak.clone();
                            let click_view = weak.clone();
                            let menu_view = weak.clone();
                            let chevron_view = weak.clone();
                            let right_members = row_members.clone();
                            let click_members = row_members;
                            let click_order = std::rc::Rc::clone(&visible_order);

                            let mut title_cell = div().flex().items_center().gap(px(5.)).overflow_hidden();
                            title_cell = if 1 == flex_col_ix {
                                title_cell.flex_1().min_w(theme::scaled_px(col_widths[1]).min(px(140.)))
                            } else {
                                title_cell.w(theme::scaled_px(col_widths[1])).flex_shrink_0()
                            };
                            if let Some((group_key, grp_count, expanded)) = group_info {
                                let gk = group_key.clone();
                                title_cell = title_cell
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "dashboard-issue-group-toggle-{id}"
                                            )))
                                            .test_support()
                                            .flex_shrink_0()
                                            .size(theme::scaled_px(16.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor_pointer()
                                            .on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                let _ = chevron_view.update(cx, |this, cx| {
                                                    if !this
                                                        .audit_dashboard
                                                        .expanded_issue_groups
                                                        .remove(&gk)
                                                    {
                                                        this.audit_dashboard
                                                            .expanded_issue_groups
                                                            .insert(gk.clone());
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                            .child(
                                                Icon::new(if expanded {
                                                    IconName::ChevronDown
                                                } else {
                                                    IconName::ChevronRight
                                                })
                                                .size(theme::scaled_px(12.))
                                                .text_color(row_fg),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(finding.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "dashboard-issue-group-count-{id}"
                                            )))
                                            .test_support()
                                            .flex_shrink_0()
                                            .px(px(5.))
                                            .py(px(1.))
                                            .rounded(px(9.))
                                            .bg(rgb(if theme::is_dark() {
                                                0x3c4047
                                            } else {
                                                0xe5e7eb
                                            }))
                                            .text_color(theme::foreground())
                                            .text_size(theme::scaled_px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(format!("{grp_count}")),
                                    );
                            } else {
                                title_cell = title_cell.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(if is_child {
                                            FontWeight::NORMAL
                                        } else {
                                            FontWeight::SEMIBOLD
                                        })
                                        .child(finding.title.clone()),
                                );
                            }
                            if has_note {
                                title_cell = title_cell.child(
                                    Icon::new(IconName::StickyNote)
                                        .size(theme::scaled_px(12.))
                                        .text_color(row_fg),
                                );
                            }

                            let mut title_cell_opt = Some(title_cell);
                            let mut row_cells: Vec<AnyElement> =
                                Vec::with_capacity(visible_cols.len());
                            for &col_ix in &visible_cols {
                                let w = col_widths[col_ix];
                                let cell: AnyElement = match col_ix {
                                    0 => {
                                        let c = div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .overflow_hidden()
                                            .child(render_severity_circle_styled(
                                                finding.severity(),
                                                finding.confidence,
                                                is_fp,
                                            ))
                                            .child(
                                                div()
                                                    .px(px(5.))
                                                    .py(px(1.))
                                                    .rounded(px(2.))
                                                    .bg(rgb(badge_bg))
                                                    .text_color(rgb(badge_fg))
                                                    .text_size(theme::scaled_px(11.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .truncate()
                                                    .child(sev_text),
                                            );
                                        if col_ix == flex_col_ix {
                                            c.flex_1()
                                                .min_w(theme::scaled_px(w).min(px(140.)))
                                                .into_any_element()
                                        } else {
                                            c.w(theme::scaled_px(w))
                                                .flex_shrink_0()
                                                .into_any_element()
                                        }
                                    }
                                    1 => title_cell_opt
                                        .take()
                                        .unwrap_or_else(div)
                                        .into_any_element(),
                                    2 => {
                                        let c = div().truncate().child(host_param.clone());
                                        if col_ix == flex_col_ix {
                                            c.flex_1()
                                                .min_w(theme::scaled_px(w).min(px(140.)))
                                                .into_any_element()
                                        } else {
                                            c.w(theme::scaled_px(w))
                                                .flex_shrink_0()
                                                .into_any_element()
                                        }
                                    }
                                    3 => {
                                        let c = div().truncate().child(finding.url.clone());
                                        if col_ix == flex_col_ix {
                                            c.flex_1()
                                                .min_w(theme::scaled_px(w).min(px(140.)))
                                                .into_any_element()
                                        } else {
                                            c.w(theme::scaled_px(w))
                                                .flex_shrink_0()
                                                .into_any_element()
                                        }
                                    }
                                    4 => {
                                        let c = div().truncate().text_color(conf_fg).child(
                                            confidence_display_label(
                                                finding.confidence,
                                                is_verified,
                                            ),
                                        );
                                        if col_ix == flex_col_ix {
                                            c.flex_1()
                                                .min_w(theme::scaled_px(w).min(px(140.)))
                                                .into_any_element()
                                        } else {
                                            c.w(theme::scaled_px(w))
                                                .flex_shrink_0()
                                                .into_any_element()
                                        }
                                    }
                                    _ => div().into_any_element(),
                                };
                                row_cells.push(cell);
                            }

                            div()
                                .id(row_id)
                                .test_support()
                                .role(Role::Row)
                                .aria_label(row_label)
                                .aria_selected(selected)
                                .w_full()
                                .h(theme::scaled_px(28.))
                                .pl(if is_child { px(26.) } else { px(10.) })
                                .pr(px(10.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .cursor_pointer()
                                .border_b_1()
                                .border_color(if selected && hl_color > 0 {
                                    mio_blue()
                                } else {
                                    theme::border()
                                })
                                .bg(row_bg)
                                .text_color(row_fg)
                                .children(row_cells)
                                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                                    let _ = right_view.update(cx, |this, cx| {
                                        this.handle_audit_issue_row_right_down(
                                            &right_members,
                                            window,
                                            cx,
                                        );
                                    });
                                })
                                .on_click(move |event: &ClickEvent, window, cx| {
                                    let shift = event.modifiers().shift;
                                    let toggle = event.modifiers().secondary(cx);
                                    let _ = click_view.update(cx, |this, cx| {
                                        this.handle_audit_issue_row_click(
                                            &click_members,
                                            &click_order,
                                            shift,
                                            toggle,
                                            true,
                                            window,
                                            cx,
                                        );
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
                .w_full()
                .flex_1()
                .min_h_0(),
            );
        }
        rows.into_any_element()
    }
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
