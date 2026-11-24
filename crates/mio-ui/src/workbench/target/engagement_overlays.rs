use gpui_kit::ClipboardItem;

use super::engagement::*;
use super::*;

impl Workbench {

    pub(in crate::workbench) fn render_target_engagement_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let report = self.target_pane.engagement_report.as_ref()?;
        let title = report.title.clone();
        let subtitle = report.subtitle.clone();
        let summary = report.summary.clone();
        let search_query = report.search_query.clone();
        let entries = report.entries.clone();
        let copy_text = entries
            .iter()
            .map(|e| format!("[{}] {} -> {}", e.kind, e.location, e.detail))
            .collect::<Vec<_>>()
            .join("\n");
        let dark = theme::is_dark();

        let search_btn = search_query.map(|query| {
            button("target-engagement-global-search", "在全局搜索中查找")
                .icon(Icon::new(IconName::Search).size(px(12.)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.target_pane.engagement_report = None;
                    this.open_global_search_with_term(query.clone(), window, cx);
                }))
        });
        let copy_btn = (!copy_text.is_empty()).then(|| {
            button("target-engagement-copy", "复制全部结果")
                .icon(Icon::new(IconName::Copy).size(px(12.)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                    this.status = "已复制参与工具分析结果到剪贴板。".into();
                    cx.notify();
                }))
        });
        let close_top_btn = Button::new("target-engagement-close")
            .ghost()
            .small()
            .compact()
            .icon(IconName::Close)
            .accessibility_label("关闭参与工具结果")
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.engagement_report = None;
                cx.notify();
            }));
        let close_bottom_btn = button("target-engagement-done", "关闭")
            .primary()
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.engagement_report = None;
                cx.notify();
            }));

        Some(
            div()
                .id("target-engagement-overlay")
                .test_support()
                .occlude()
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0., 0., 0., 0.35))
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    this.target_pane.engagement_report = None;
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("target-engagement-dialog")
                        .test_support()
                        .occlude()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .w(px(720.))
                        .max_w_full()
                        .h(px(460.))
                        .max_h_full()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(10.))
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_size(theme::scaled_px(14.))
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .text_size(theme::scaled_px(12.))
                                                .text_color(theme::muted_foreground())
                                                .truncate()
                                                .child(subtitle),
                                        ),
                                )
                                .child(close_top_btn),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(rgb(if dark { 0x25272b } else { 0xf8f9fa }))
                                .text_size(theme::ui_font_size())
                                .child(summary),
                        )
                        .child(
                            div()
                                .id("target-engagement-entries")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p(px(10.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .when(entries.is_empty(), |box_el| {
                                    box_el.child(
                                        div()
                                            .p(px(10.))
                                            .text_color(theme::muted_foreground())
                                            .child("当前范围内暂无直接匹配项。"),
                                    )
                                })
                                .children(entries.into_iter().enumerate().map(|(idx, entry)| {
                                    div()
                                        .id(("target-engagement-entry", idx))
                                        .p(px(8.))
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .bg(theme::surface())
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .text_size(theme::scaled_px(12.))
                                                        .child(entry.kind),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(theme::scaled_px(11.5))
                                                        .text_color(theme::muted_foreground())
                                                        .truncate()
                                                        .child(entry.location),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family(theme::mono_font_family())
                                                .text_size(theme::scaled_px(12.))
                                                .child(entry.detail),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_t_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(px(8.))
                                .when_some(search_btn, |bar, btn| bar.child(btn))
                                .when_some(copy_btn, |bar, btn| bar.child(btn))
                                .child(close_bottom_btn),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(in crate::workbench) fn render_target_compare_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.target_pane.compare_modal.as_ref()?;
        let left_label = if state.left_origin.is_empty() {
            "当前站点".to_owned()
        } else {
            state.left_origin.clone()
        };
        let right_label = state
            .external_label
            .clone()
            .or_else(|| state.right_origin.clone())
            .unwrap_or_else(|| "未选择对比目标".into());
        let other_origins: Vec<String> = state
            .available_origins
            .iter()
            .filter(|o| **o != state.left_origin)
            .cloned()
            .collect();
        let active_right = state.right_origin.clone();
        let rows = state.rows.clone();
        let stats = format!(
            "仅在源站点：{} · 仅在目标站点：{} · 状态/长度不同：{} · 完全一致：{}",
            state.only_left, state.only_right, state.different, state.same
        );
        let dark = theme::is_dark();
        let origin_buttons: Vec<Button> = other_origins
            .into_iter()
            .enumerate()
            .map(|(idx, origin)| {
                let selected = active_right.as_deref() == Some(origin.as_str());
                let o_click = origin.clone();
                button(
                    SharedString::from(format!("target-compare-origin-{idx}")),
                    origin,
                )
                .selected(selected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_compare_site_map_right_origin(o_click.clone(), cx);
                }))
            })
            .collect();
        let has_origin_buttons = !origin_buttons.is_empty();
        let close_top_btn = Button::new("target-compare-close")
            .ghost()
            .small()
            .compact()
            .icon(IconName::Close)
            .accessibility_label("关闭站点地图对比")
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.compare_modal = None;
                cx.notify();
            }));
        let load_file_btn = button("target-compare-load-file", "从文件加载快照…")
            .icon(Icon::new(IconName::Upload).size(px(12.)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.load_compare_site_map_file(window, cx);
            }));
        let to_comparer_btn = button("target-compare-to-comparer", "发送到 Comparer 对比")
            .icon(Icon::new(IconName::GitCompare).size(px(12.)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.send_compare_site_maps_to_comparer(window, cx);
            }));
        let close_bottom_btn = button("target-compare-done", "关闭")
            .primary()
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.compare_modal = None;
                cx.notify();
            }));

        Some(
            div()
                .id("target-compare-overlay")
                .test_support()
                .occlude()
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0., 0., 0., 0.35))
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    this.target_pane.compare_modal = None;
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("target-compare-dialog")
                        .test_support()
                        .occlude()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .w(px(820.))
                        .max_w_full()
                        .h(px(520.))
                        .max_h_full()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(10.))
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(14.))
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .child("比较站点地图 (Compare site maps)"),
                                )
                                .child(close_top_btn),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(theme::border())
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_size(theme::ui_font_size())
                                                .child(format!("源站点：{left_label}   ↔   对比目标：{right_label}")),
                                        )
                                        .child(load_file_btn),
                                )
                                .when(has_origin_buttons, |col| {
                                    col.child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                div()
                                                    .text_size(theme::scaled_px(12.))
                                                    .text_color(theme::muted_foreground())
                                                    .child("切换对比主机："),
                                            )
                                            .children(origin_buttons),
                                    )
                                })
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(12.))
                                        .text_color(theme::muted_foreground())
                                        .child(stats),
                                ),
                        )
                        .child(
                            div()
                                .id("target-compare-rows")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .when(rows.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .p(px(16.))
                                            .text_color(theme::muted_foreground())
                                            .child("暂无可对比路径；可从文件加载 XML/JSON 站点地图快照或捕获多个主机后重试。"),
                                    )
                                })
                                .children(rows.into_iter().enumerate().map(|(idx, row)| {
                                    let (badge, row_bg) = match row.kind {
                                        TargetCompareDiffKind::OnlyLeft => (
                                            "仅源站点",
                                            rgb(if dark { 0x1f3626 } else { 0xedf7ed }),
                                        ),
                                        TargetCompareDiffKind::OnlyRight => (
                                            "仅目标",
                                            rgb(if dark { 0x3a2424 } else { 0xfdecea }),
                                        ),
                                        TargetCompareDiffKind::Different => (
                                            "有差异",
                                            rgb(if dark { 0x3b311e } else { 0xfff8e1 }),
                                        ),
                                        TargetCompareDiffKind::Same => (
                                            "一致",
                                            rgb(if dark { 0x25272b } else { 0xffffff }),
                                        ),
                                    };
                                    div()
                                        .id(("target-compare-row", idx))
                                        .px(px(12.))
                                        .py(px(5.))
                                        .border_b_1()
                                        .border_color(theme::border())
                                        .bg(row_bg)
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .child(
                                            div()
                                                .w(px(64.))
                                                .flex_shrink_0()
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .text_size(theme::scaled_px(11.5))
                                                .child(badge),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family(theme::mono_font_family())
                                                .text_size(theme::scaled_px(12.))
                                                .child(row.path),
                                        )
                                        .child(
                                            div()
                                                .w(px(190.))
                                                .flex_shrink_0()
                                                .truncate()
                                                .text_size(theme::scaled_px(11.5))
                                                .child(row.left_summary),
                                        )
                                        .child(
                                            div()
                                                .w(px(190.))
                                                .flex_shrink_0()
                                                .truncate()
                                                .text_size(theme::scaled_px(11.5))
                                                .child(row.right_summary),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_t_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(px(8.))
                                .child(to_comparer_btn)
                                .child(close_bottom_btn),
                        ),
                )
                .into_any_element(),
        )
    }
}

pub(super) struct DetachedSiteMapWindow {
    owner: WeakEntity<Workbench>,
    project: Option<ProjectKey>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl DetachedSiteMapWindow {
    pub(super) fn new(owner: Entity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = owner.read(cx).project;
        let handle = window.window_handle();
        let subscription = cx.observe_in(&owner, window, move |this, owner, window, cx| {
            if owner.read(cx).project != this.project {
                window.remove_window();
                return;
            }
            cx.notify();
        });
        let released = owner.downgrade();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.update(cx, |view, _| {
                    if view.target_pane.detached_site_map_window == Some(handle) {
                        view.target_pane.detached_site_map_window = None;
                    }
                });
            });
        })
        .detach();
        window.set_window_title("Site map");
        Self {
            owner: owner.downgrade(),
            project,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }
}

impl Render for DetachedSiteMapWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div()
                .id("target-sitemap-detached-empty")
                .size_full()
                .into_any_element();
        };
        let title_bar = crate::chrome::render_title_bar(
            "target-sitemap-detached",
            "Site map",
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let (nodes, rows, req_preview, resp_preview) = {
            let view = owner.read(cx);
            let nodes: Vec<(u8, String, usize)> = view
                .target_pane
                .tree_state
                .visible_nodes()
                .into_iter()
                .take(300)
                .map(|n| (n.depth, n.label.clone(), n.subtree_count))
                .collect();
            let rows: Vec<(String, String, String, String)> = view
                .target_pane
                .items
                .iter()
                .filter(|r| !view.target_pane.hidden_item_ids.contains(&r.id()))
                .take(200)
                .map(|row| {
                    let s = target_row_summary_with_metadata(row, None, None);
                    (s.host, s.method, s.url, s.status)
                })
                .collect();
            (
                nodes,
                rows,
                view.target_pane.request.raw.read(cx).value().to_string(),
                view.target_pane.response.raw.read(cx).value().to_string(),
            )
        };
        let body = div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("target-sitemap-detached-tree")
                    .w(px(260.))
                    .h_full()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .py(px(4.))
                    .children(nodes.into_iter().map(|(depth, label, count)| {
                        div()
                            .h(px(22.))
                            .pl(px(8. + f32::from(depth) * 14.))
                            .pr(px(8.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(theme::muted_foreground())
                                    .child(count.to_string()),
                            )
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("target-sitemap-detached-items")
                            .h(px(280.))
                            .overflow_y_scroll()
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .flex_col()
                            .children(rows.into_iter().map(|(host, method, url, status)| {
                                div()
                                    .h(theme::row_height())
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .child(div().w(px(140.)).truncate().child(host))
                                    .child(div().w(px(54.)).child(method))
                                    .child(div().w(px(48.)).child(status))
                                    .child(div().flex_1().min_w_0().truncate().child(url))
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(
                                div()
                                    .id("target-sitemap-detached-request")
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .p(px(10.))
                                    .overflow_y_scroll()
                                    .border_r_1()
                                    .border_color(theme::border())
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::editor_font_size())
                                    .child(if req_preview.is_empty() {
                                        "Request".into()
                                    } else {
                                        req_preview
                                    }),
                            )
                            .child(
                                div()
                                    .id("target-sitemap-detached-response")
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .p(px(10.))
                                    .overflow_y_scroll()
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::editor_font_size())
                                    .child(if resp_preview.is_empty() {
                                        "Response".into()
                                    } else {
                                        resp_preview
                                    }),
                            ),
                    ),
            );
        div()
            .id("target-sitemap-detached-window")
            .test_support()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
