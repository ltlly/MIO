use gpui_kit::component::{
    Icon, IconName,
    button::ButtonVariants,
    menu::{DropdownMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
};

use super::results_view::run_progress;
use super::*;
use crate::workbench::audit_dashboard::{
    mio_blue, mio_card_border, mio_card_selected_bg, mio_card_selected_border,
    mio_card_shadows,
};

impl Workbench {
    pub(in crate::workbench) fn render_intruder_dashboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.activate_pro_dashboard(cx);
        let tasks_header = self.render_dashboard_tasks_header(window, cx);
        let audit_cards = self.render_audit_task_cards(window, cx);
        let page = &self.intruder.results.runs;
        let pending = page.pending.is_some();
        let disabled = self.backend_disconnected || self.transition.is_some();
        if !pending && page.error.is_none() && self.intruder.dashboard.scroll_to_end {
            self.intruder.dashboard.scroll_to_end = false;
            self.intruder.dashboard.scroll.scroll_to_bottom();
        }
        let mut cards = div()
            .id("dashboard-tasks")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.intruder.dashboard.scroll)
            .on_scroll(
                cx.listener(|this, event: &gpui_kit::ScrollEvent, window, cx| {
                    this.scroll_intruder_tasks(event.delta.pixel_delta(window.line_height()).y, cx);
                }),
            )
            .px(px(16.))
            .pt(px(4.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .children(audit_cards);
        for run in &page.rows {
            let run_id = run.id;
            let title = self.dashboard_title(run);
            if !self.dashboard_filter_matches_intruder(&title, !run.state.is_terminal(), cx) {
                continue;
            }
            let selected = self
                .intruder
                .dashboard
                .selection
                .as_ref()
                .is_some_and(|selection| selection.run_id == run_id);
            let description = self.dashboard_spec(run).map_or_else(
                || {
                    run.payload_progress().map_or_else(
                        || "请求数量未确认".into(),
                        |(_, total)| format!("{total} 个请求"),
                    )
                },
                |spec| {
                    format!(
                        "{} · {} 个请求",
                        crate::workbench::intruder::render::attack_label(spec.attack_type),
                        run.payload_progress()
                            .map_or_else(|| "未确认".into(), |(_, total)| total.to_string())
                    )
                },
            );
            let weak = cx.weak_entity();
            let options = Button::new(SharedString::from(format!(
                "dashboard-intruder-options-{run_id}"
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
            .accessibility_label("任务选项")
            .tooltip("打开任务操作菜单（在独立窗口打开、重命名、保存到项目或删除）")
            .dropdown_menu(move |menu, _, cx| {
                let Some(owner) = weak.upgrade() else {
                    return menu;
                };
                let view = owner.read(cx);
                let current = view
                    .intruder
                    .results
                    .runs
                    .rows
                    .iter()
                    .find(|run| run.id == run_id);
                let editable = current.is_some()
                    && view.backend_available()
                    && view.transition.is_none()
                    && !view.file_picker_pending();
                let terminal = current.is_some_and(|run| run.state.is_terminal());
                let delete = weak.clone();
                let save = weak.clone();
                let rename = weak.clone();
                let open = weak.clone();
                menu.item(PopupMenuItem::new("删除").disabled(!editable).on_click(
                    move |_, window, cx| {
                        let _ = delete.update(cx, |view, cx| {
                            view.request_dashboard_intruder_delete(run_id, window, cx);
                        });
                    },
                ))
                .item(
                    PopupMenuItem::new("保存到项目文件")
                        .disabled(!editable || !terminal)
                        .on_click(move |_, window, cx| {
                            let _ = save.update(cx, |view, cx| {
                                view.save_dashboard_intruder_run(run_id, window, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("重命名任务")
                        .disabled(!editable)
                        .on_click(move |_, window, cx| {
                            let _ = rename.update(cx, |view, cx| {
                                view.open_intruder_task_name(run_id, window, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("在新窗口中打开")
                        .disabled(!editable)
                        .on_click(move |_, _, cx| {
                            let _ = open.update(cx, |view, cx| {
                                view.open_intruder_results_window(run_id, cx);
                            });
                        }),
                )
            });
            cards = cards.child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w_full()
                    .rounded(px(6.))
                    .shadow(mio_card_shadows())
                    .child(
                        flat_button(
                            SharedString::from(format!("dashboard-intruder-{run_id}")),
                            true,
                            window,
                            cx,
                        )
                        .role(gpui_kit::accesskit::Role::Tab)
                        .accessibility_label(title.clone())
                        .aria_selected(selected)
                        .disabled(self.transition.is_some())
                        .flex_shrink_0()
                        .relative()
                        .w_full()
                        .overflow_hidden()
                        .min_h(px(96.))
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
                                .pr(theme::scaled_px(28.))
                                .truncate()
                                .text_size(theme::scaled_px(13.))
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .text_color(theme::foreground())
                                .child(title),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(theme::scaled_px(12.5))
                                .text_color(theme::muted_foreground())
                                .child(description),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::foreground())
                                .child(format!(
                                    "{} · {}",
                                    run_progress(run),
                                    self.dashboard_retention(run),
                                )),
                        )
                        .when(selected, |this| {
                            this.child(
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
                                this.select_dashboard_intruder_run(run_id, window, cx);
                            },
                        )),
                    )
                    .child(div().absolute().right(px(11.)).top(px(11.)).child(options)),
            );
        }
        if pending {
            cards = cards.child(
                div()
                    .id("dashboard-loading")
                    .test_support()
                    .px(px(4.))
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::muted_foreground())
                    .child("正在读取任务…"),
            );
        } else if page.rows.is_empty() && page.error.is_none() {
            if self.project.is_none() || page.after.is_some() {
                cards = cards.child(
                    div()
                        .id("dashboard-empty")
                        .test_support()
                        .px(px(4.))
                        .pt(px(2.))
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::muted_foreground())
                        .child(if self.project.is_none() {
                            "尚未打开项目。"
                        } else {
                            "此页没有 Intruder 任务。"
                        }),
                );
            } else {
                cards = cards.child(
                    div()
                        .id("dashboard-empty")
                        .test_support()
                        .w_full()
                        .h(px(1.))
                        .flex_shrink_0(),
                );
            }
        }
        if let Some(error) = &page.error {
            cards = cards.child(
                div()
                    .id("dashboard-error")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_color(theme::danger_fg())
                            .child(format!("任务读取失败：{error}")),
                    )
                    .child(
                        button("dashboard-retry", "重试")
                            .icon(Icon::new(IconName::RefreshCw).size(theme::scaled_px(13.)))
                            .tooltip("重新加载 Intruder 任务列表")
                            .disabled(disabled || pending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let after = this.intruder.results.runs.after;
                                this.query_intruder_runs(after, cx);
                            })),
                    ),
            );
        }
        let tasks = div()
            .size_full()
            .min_w_0()
            .h_full()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::border())
            .child(tasks_header)
            .child(cards)
            .when(self.backend_disconnected, |this| {
                this.child(
                    div()
                        .px(px(16.))
                        .py(px(10.))
                        .text_size(theme::scaled_px(12.))
                        .text_color(rgb(if theme::is_dark() { 0xf28b82 } else { 0x9b2727 }))
                        .child("后台服务已断开；已读取的任务摘要仍保留。"),
                )
            });

        let mut detail = div()
            .id("dashboard-detail")
            .test_support()
            .size_full()
            .min_w_0()
            .h_full()
            .bg(theme::surface())
            .flex()
            .flex_col();
        if let Some(selection) = &self.intruder.dashboard.selection {
            let result = self.intruder.results.views.get(&selection.view_id);
            let summary = result
                .and_then(|view| view.summary.as_ref())
                .or_else(|| page.rows.iter().find(|run| run.id == selection.run_id));
            let title = summary.map_or_else(
                || format!("Intruder 攻击 · {}", &selection.run_id.to_string()[..8]),
                |run| self.dashboard_title(run),
            );
            detail = detail.child(
                div()
                    .px(px(20.))
                    .pt(px(14.))
                    .pb(px(10.))
                    .border_b_1()
                    .border_color(theme::border())
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::scaled_px(16.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child(title),
                    )
                    .child(
                        button("dashboard-show-issue-activity", "返回实时审计")
                            .icon(Icon::new(IconName::ArrowLeft).size(theme::scaled_px(13.)))
                            .tooltip("取消选中 Intruder 任务并返回实时审计工作区")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.clear_dashboard_intruder_selection(window, cx);
                            })),
                    ),
            );
            if let Some(error) = &selection.error {
                detail = detail.child(
                    div()
                        .p(px(18.))
                        .text_color(rgb(if theme::is_dark() { 0xf28b82 } else { 0x9b2727 }))
                        .child(error.clone()),
                );
            } else if let Some(results) = &selection.results {
                detail = detail.child(if result.is_some() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(results.clone())
                        .into_any_element()
                } else {
                    div()
                        .p(px(18.))
                        .child("结果视图已关闭；单击左侧任务可重新打开。")
                        .into_any_element()
                });
            } else {
                detail = detail.child(div().p(px(18.)).child("正在打开结果视图…"));
            }
        } else {
            detail = detail.child(self.render_audit_task_workspace(window, cx));
        }

        let new_scan_panel = self.render_new_scan_panel(cx);
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
