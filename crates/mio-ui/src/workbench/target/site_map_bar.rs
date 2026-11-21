use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};

use super::*;

fn site_map_filter_summary(pane: &TargetPane, search_val: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if pane.in_scope_only {
        parts.push("仅显示范围内".into());
    }
    if pane.hide_not_found {
        parts.push("隐藏未找到项".into());
    }
    if pane.hide_css_image_binary {
        parts.push("隐藏 CSS、图像和常规二进制内容".into());
    }
    if pane.status_classes != [true; 4] {
        let hidden: Vec<&str> = ["2xx", "3xx", "4xx", "5xx"]
            .into_iter()
            .enumerate()
            .filter_map(|(i, lbl)| (!pane.status_classes[i]).then_some(lbl))
            .collect();
        if !hidden.is_empty() {
            parts.push(format!("隐藏 {} 响应", hidden.join("、")));
        }
    }
    if pane.hide_empty_folders {
        parts.push("隐藏空文件夹".into());
    }
    let trimmed = search_val.trim();
    if !trimmed.is_empty() {
        parts.push(format!("搜索 \"{trimmed}\""));
    }
    let hidden_total = pane.tree_state.hidden_prefixes.len() + pane.hidden_item_ids.len();
    if hidden_total > 0 {
        parts.push(format!("已隐藏 {hidden_total} 项"));
    }
    if parts.is_empty() {
        "站点地图筛选：显示全部".into()
    } else {
        format!("站点地图筛选：{}", parts.join("；"))
    }
}

fn site_map_layout_icon(mode: SiteMapLayoutMode, selected: bool) -> AnyElement {
    let color: gpui_kit::Hsla = if selected {
        rgb(0xffffff).into()
    } else {
        theme::foreground()
    };
    match mode {
        SiteMapLayoutMode::ThreeColumn => div()
            .w(px(8.))
            .h(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .child(div().w(px(3.)).h_full().bg(color))
            .child(div().w(px(3.)).h_full().bg(color))
            .into_any_element(),
        SiteMapLayoutMode::TreeLeftStackedRight => {
            div().w(px(9.)).h(px(9.)).bg(color).into_any_element()
        }
    }
}

fn site_map_subtab(id: &'static str, label: String, selected: bool) -> Button {
    let fg: gpui_kit::Hsla = if selected {
        if theme::is_dark() {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        }
    } else {
        theme::foreground()
    };
    Button::new(id)
        .ghost()
        .small()
        .rounded(ButtonRounded::None)
        .relative()
        .h(px(35.))
        .px(theme::scaled_px(12.))
        .bg(theme::surface())
        .text_color(fg)
        .selected(selected)
        .toggled(selected)
        .accessibility_label(label.clone())
        .child(
            div()
                .text_size(theme::ui_font_size())
                .font_weight(if selected {
                    gpui_kit::FontWeight::MEDIUM
                } else {
                    gpui_kit::FontWeight::NORMAL
                })
                .child(label),
        )
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

impl Workbench {
    #[inline(never)]
    fn render_site_map_filter_popover(
        &self,
        filter_summary_text: String,
        filter_bg: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter_owner = cx.weak_entity();
        div()
            .flex_1()
            .min_w_0()
            .child(
                Popover::new("target-sitemap-filter-popover")
                    .anchor(gpui_kit::Anchor::TopLeft)
                    .trigger_style(gpui_kit::StyleRefinement::default().w_full())
                    .trigger(
                        Button::new("target-sitemap-filter-bar")
                            .ghost()
                            .small()
                            .compact()
                            .w_full()
                            .h(theme::scaled_px(23.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .px(theme::scaled_px(8.))
                            .bg(filter_bg)
                            .text_color(theme::foreground())
                            .justify_start()
                            .accessibility_label(filter_summary_text.clone())
                            .tooltip("点击配置站点地图筛选条件（范围、404、MIME、状态码分类）")
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(theme::scaled_px(6.))
                                    .child(
                                        Icon::new(IconName::Funnel)
                                            .size(theme::scaled_px(13.))
                                            .text_color(theme::foreground()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_left()
                                            .text_size(theme::ui_font_size())
                                            .child(
                                                filter_summary_text
                                                    .clone()
                                                    .test_text(filter_summary_text),
                                            ),
                                    ),
                            ),
                    )
                    .content(move |_, _, cx| {
                        let Some(wb) = filter_owner.upgrade() else {
                            return div().into_any_element();
                        };
                        let (
                            in_scope_only,
                            hide_not_found,
                            hide_binary,
                            hide_empty_folders,
                            status_classes,
                            hidden_cnt,
                            is_loading,
                        ) = {
                            let p = &wb.read(cx).target_pane;
                            (
                                p.in_scope_only,
                                p.hide_not_found,
                                p.hide_css_image_binary,
                                p.hide_empty_folders,
                                p.status_classes,
                                p.tree_state.hidden_prefixes.len() + p.hidden_item_ids.len(),
                                p.site_map_loading,
                            )
                        };
                        let o_scope = filter_owner.clone();
                        let o_404 = filter_owner.clone();
                        let o_binary = filter_owner.clone();
                        let o_empty = filter_owner.clone();
                        let o_refresh = filter_owner.clone();
                        let o_reset = filter_owner.clone();
                        let o_restore = filter_owner.clone();
                        div()
                            .id("target-sitemap-filter-content")
                            .test_support()
                            .w(px(360.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(13.5))
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child("站点地图筛选设置"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("按请求与内容类型筛选"),
                                    )
                                    .child(
                                        mio_checkbox(
                                            "target-sitemap-in-scope",
                                            "仅显示范围内条目",
                                            in_scope_only,
                                            !is_loading,
                                        )
                                        .on_change(move |_, _, window, cx| {
                                            let _ = o_scope.update(cx, |this, cx| {
                                                this.target_pane.in_scope_only =
                                                    !this.target_pane.in_scope_only;
                                                this.refresh_target_site_map(0, window, cx);
                                            });
                                        }),
                                    )
                                    .child(
                                        mio_checkbox(
                                            "target-sitemap-hide-404",
                                            "隐藏未找到项 (404 / 无响应)",
                                            hide_not_found,
                                            !is_loading,
                                        )
                                        .on_change(move |_, _, window, cx| {
                                            let _ = o_404.update(cx, |this, cx| {
                                                this.target_pane.hide_not_found =
                                                    !this.target_pane.hide_not_found;
                                                this.refresh_target_site_map(0, window, cx);
                                            });
                                        }),
                                    )
                                    .child(
                                        mio_checkbox(
                                            "target-sitemap-hide-binary",
                                            "隐藏 CSS、图像和常规二进制内容",
                                            hide_binary,
                                            !is_loading,
                                        )
                                        .on_change(move |_, _, _, cx| {
                                            let _ = o_binary.update(cx, |this, cx| {
                                                this.target_pane.hide_css_image_binary =
                                                    !this.target_pane.hide_css_image_binary;
                                                cx.notify();
                                            });
                                        }),
                                    )
                                    .child(
                                        mio_checkbox(
                                            "target-sitemap-hide-empty-folders",
                                            "隐藏空文件夹",
                                            hide_empty_folders,
                                            !is_loading,
                                        )
                                        .on_change(move |_, _, _, cx| {
                                            let _ = o_empty.update(cx, |this, cx| {
                                                this.target_pane.hide_empty_folders =
                                                    !this.target_pane.hide_empty_folders;
                                                cx.notify();
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("按状态码显示"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(10.))
                                            .children(
                                                [
                                                    (0usize, "target-sitemap-status-2xx", "2xx [成功]"),
                                                    (1usize, "target-sitemap-status-3xx", "3xx [重定向]"),
                                                    (2usize, "target-sitemap-status-4xx", "4xx [请求错误]"),
                                                    (3usize, "target-sitemap-status-5xx", "5xx [服务器错误]"),
                                                ]
                                                .into_iter()
                                                .map(|(idx, cb_id, lbl)| {
                                                    let o_st = filter_owner.clone();
                                                    div().w(px(150.)).child(
                                                        mio_checkbox(
                                                            cb_id,
                                                            lbl,
                                                            status_classes[idx],
                                                            !is_loading,
                                                        )
                                                        .on_change(move |_, _, window, cx| {
                                                            let _ = o_st.update(cx, |this, cx| {
                                                                this.target_pane.status_classes
                                                                    [idx] = !this
                                                                    .target_pane
                                                                    .status_classes[idx];
                                                                this.refresh_target_site_map(
                                                                    0, window, cx,
                                                                );
                                                            });
                                                        }),
                                                    )
                                                }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(8.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        button("target-sitemap-refresh", "刷新")
                                            .icon(Icon::new(IconName::RefreshCw).size(px(12.)))
                                            .disabled(is_loading)
                                            .on_click(move |_, window, cx| {
                                                let _ = o_refresh.update(cx, |this, cx| {
                                                    this.refresh_target_site_map(0, window, cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        button("target-sitemap-reset-filter", "显示全部")
                                            .disabled(is_loading)
                                            .on_click(move |_, window, cx| {
                                                let _ = o_reset.update(cx, |this, cx| {
                                                    this.target_pane.in_scope_only = false;
                                                    this.target_pane.hide_not_found = false;
                                                    this.target_pane.hide_css_image_binary = false;
                                                    this.target_pane.hide_empty_folders = false;
                                                    this.target_pane.status_classes = [true; 4];
                                                    this.target_pane
                                                        .search
                                                        .update(cx, |s, cx| {
                                                            s.set_value("", window, cx)
                                                        });
                                                    this.refresh_target_site_map(0, window, cx);
                                                });
                                            }),
                                    )
                                    .when(hidden_cnt > 0, |row| {
                                        row.child(
                                            button(
                                                "target-sitemap-popover-show-hidden",
                                                format!("恢复隐藏 ({hidden_cnt})"),
                                            )
                                            .on_click(move |_, window, cx| {
                                                let _ = o_restore.update(cx, |this, cx| {
                                                    this.show_hidden_target_branches(window, cx);
                                                });
                                            }),
                                        )
                                    }),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_site_map_top_bar(
        &self,
        hidden_count: usize,
        view_ctx: &TargetViewContext<'_>,
        filter_summary_text: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_mode = view_ctx.site_map_view_mode;
        let layout_mode = view_ctx.site_map_layout;
        let view_url = view_ctx.view.clone();
        let view_crawl = view_ctx.view.clone();
        let layout_cols = view_ctx.view.clone();
        let layout_stacked = view_ctx.view.clone();
        let dark = theme::is_dark();
        let filter_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xeeeeee }).into();
        let inactive_seg_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xdfdfdf }).into();
        let active_seg_bg: gpui_kit::Hsla = rgb(0x26649d).into();

        let view_toggle = div()
            .h(theme::scaled_px(23.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .rounded(px(3.))
            .overflow_hidden()
            .child(
                Button::new("target-sitemap-view-url")
                    .ghost()
                    .small()
                    .compact()
                    .rounded(ButtonRounded::None)
                    .h(theme::scaled_px(23.))
                    .px(theme::scaled_px(9.))
                    .bg(if view_mode == SiteMapViewMode::UrlView {
                        active_seg_bg
                    } else {
                        inactive_seg_bg
                    })
                    .text_color(if view_mode == SiteMapViewMode::UrlView {
                        rgb(0xffffff).into()
                    } else {
                        theme::foreground()
                    })
                    .accessibility_label("URL view")
                    .tooltip("按 URL 域名与目录层级结构浏览站点地图")
                    .selected(view_mode == SiteMapViewMode::UrlView)
                    .toggled(view_mode == SiteMapViewMode::UrlView)
                    .label("URL view")
                    .on_click(move |_, _, cx| {
                        let _ = view_url.update(cx, |view, cx| {
                            view.set_view_mode(SiteMapViewMode::UrlView, cx);
                        });
                    }),
            )
            .child(
                Button::new("target-sitemap-view-crawl")
                    .ghost()
                    .small()
                    .compact()
                    .rounded(ButtonRounded::None)
                    .h(theme::scaled_px(23.))
                    .px(theme::scaled_px(9.))
                    .bg(if view_mode == SiteMapViewMode::CrawlPathsView {
                        active_seg_bg
                    } else {
                        inactive_seg_bg
                    })
                    .text_color(if view_mode == SiteMapViewMode::CrawlPathsView {
                        rgb(0xffffff).into()
                    } else {
                        theme::foreground()
                    })
                    .accessibility_label("Crawl paths view")
                    .tooltip("按已发现的抓取路径层级查看站点地图节点")
                    .selected(view_mode == SiteMapViewMode::CrawlPathsView)
                    .toggled(view_mode == SiteMapViewMode::CrawlPathsView)
                    .label("Crawl paths view")
                    .on_click(move |_, _, cx| {
                        let _ = view_crawl.update(cx, |view, cx| {
                            view.set_view_mode(SiteMapViewMode::CrawlPathsView, cx);
                        });
                    }),
            );

        let filter_popover =
            self.render_site_map_filter_popover(filter_summary_text, filter_bg, cx);

        let help_popover = Popover::new("target-sitemap-help-popover")
            .anchor(gpui_kit::Anchor::TopRight)
            .trigger(
                Button::new("target-sitemap-help")
                    .ghost()
                    .compact()
                    .size(px(22.))
                    .p_0()
                    .rounded(px(11.))
                    .accessibility_label("站点地图帮助")
                    .tooltip("查看 Site map 树形导航、筛选与右键功能说明")
                    .child(
                        Icon::new(IconName::CircleQuestionMark)
                            .size(px(18.))
                            .text_color(theme::foreground()),
                    ),
            )
            .content(|_, _, _| {
                div()
                    .id("target-sitemap-help-content")
                    .test_support()
                    .w(px(300.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Site map（站点地图）"),
                    )
                    .child("左侧树按主机与路径层级汇总已捕获的目标站点，右键可添加到范围、发起扫描或隐藏分支。")
                    .child("点击上方筛选栏可按范围、未找到项、MIME 类型与状态码分类过滤站点地图条目。")
                    .child("右侧顶部切换 Contents 与 Issues 视图，点击条目可在下方检视请求、响应与 Inspector。")
            });

        let layout_toggle = div()
            .h(px(21.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .rounded(px(3.))
            .overflow_hidden()
            .child(
                Button::new("target-sitemap-layout-columns")
                    .ghost()
                    .small()
                    .compact()
                    .rounded(ButtonRounded::None)
                    .w(px(24.))
                    .h(px(21.))
                    .p_0()
                    .bg(if layout_mode == SiteMapLayoutMode::ThreeColumn {
                        active_seg_bg
                    } else {
                        inactive_seg_bg
                    })
                    .accessibility_label("三栏并排布局")
                    .tooltip("三栏并排布局（站点树 | 内容列表 | 请求与响应）")
                    .selected(layout_mode == SiteMapLayoutMode::ThreeColumn)
                    .toggled(layout_mode == SiteMapLayoutMode::ThreeColumn)
                    .child(site_map_layout_icon(
                        SiteMapLayoutMode::ThreeColumn,
                        layout_mode == SiteMapLayoutMode::ThreeColumn,
                    ))
                    .on_click(move |_, _, cx| {
                        let _ = layout_cols.update(cx, |view, cx| {
                            view.set_layout_mode(SiteMapLayoutMode::ThreeColumn, cx);
                        });
                    }),
            )
            .child(
                Button::new("target-sitemap-layout-stacked")
                    .ghost()
                    .small()
                    .compact()
                    .rounded(ButtonRounded::None)
                    .w(px(24.))
                    .h(px(21.))
                    .p_0()
                    .bg(if layout_mode == SiteMapLayoutMode::TreeLeftStackedRight {
                        active_seg_bg
                    } else {
                        inactive_seg_bg
                    })
                    .accessibility_label("左树右上下分栏布局")
                    .tooltip("经典布局（左侧站点树，右侧上方内容列表、下方请求与响应）")
                    .selected(layout_mode == SiteMapLayoutMode::TreeLeftStackedRight)
                    .toggled(layout_mode == SiteMapLayoutMode::TreeLeftStackedRight)
                    .child(site_map_layout_icon(
                        SiteMapLayoutMode::TreeLeftStackedRight,
                        layout_mode == SiteMapLayoutMode::TreeLeftStackedRight,
                    ))
                    .on_click(move |_, _, cx| {
                        let _ = layout_stacked.update(cx, |view, cx| {
                            view.set_layout_mode(SiteMapLayoutMode::TreeLeftStackedRight, cx);
                        });
                    }),
            );

        div()
            .w_full()
            .h(theme::scaled_px(36.))
            .flex_shrink_0()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(view_toggle)
            .child(filter_popover)
            .when(hidden_count > 0, |bar| {
                bar.child(
                    button(
                        "target-sitemap-show-hidden",
                        format!("显示已隐藏分支 ({hidden_count})"),
                    )
                    .icon(Icon::new(IconName::Eye).size(px(12.)))
                    .tooltip("恢复显示站点地图中所有已被手动隐藏的分支")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_hidden_target_branches(window, cx);
                    })),
                )
            })
            .child(help_popover)
            .child(layout_toggle)
            .into_any_element()
    }

    #[inline(never)]
    fn render_site_map_right_header(
        &self,
        subview: SiteMapSubView,
        issues_count: usize,
        loading: bool,
        search_input: Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options_owner = cx.weak_entity();
        div()
            .w_full()
            .h(px(36.))
            .flex_shrink_0()
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                site_map_subtab(
                    "target-sitemap-subtab-contents",
                    "Contents".into(),
                    subview == SiteMapSubView::Contents,
                )
                .tooltip("查看当前选中站点分支下的 HTTP 请求与响应列表")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.target_pane.site_map_subview = SiteMapSubView::Contents;
                    cx.notify();
                })),
            )
            .child(
                site_map_subtab(
                    "target-sitemap-subtab-issues",
                    format!("Issues ({issues_count})"),
                    subview == SiteMapSubView::Issues,
                )
                .tooltip("查看当前选中站点分支关联的扫描漏洞与安全告警")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.target_pane.site_map_subview = SiteMapSubView::Issues;
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(theme::scaled_px(210.))
                    .min_w(theme::scaled_px(120.))
                    .flex_shrink(1.0)
                    .child(
                        Input::new(&search_input)
                            .id("target-sitemap-search")
                            .aria_label("站点地图搜索")
                            .small()
                            .h(theme::scaled_px(23.))
                            .focus_bordered(false)
                            .suffix(
                                Button::new("target-sitemap-apply")
                                    .ghost()
                                    .compact()
                                    .h(theme::scaled_px(19.))
                                    .icon(IconName::Search)
                                    .accessibility_label("应用搜索")
                                    .tooltip("按输入的关键词筛选站点地图节点与报文")
                                    .disabled(loading)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.refresh_target_site_map(0, window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                Button::new("target-sitemap-options-menu")
                    .small()
                    .ghost()
                    .compact()
                    .size(px(20.))
                    .rounded(px(3.))
                    .icon(IconName::EllipsisVertical)
                    .accessibility_label("站点地图选项")
                    .tooltip("站点地图操作菜单（刷新、提取选中响应链接、展开/折叠树）")
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, cx| {
                        let Some(entity) = options_owner.upgrade() else {
                            return menu;
                        };
                        let (
                            can_extract,
                            has_nodes,
                            has_hidden,
                            has_sort,
                            has_selected_node,
                            is_loading,
                        ) = {
                            let wb = entity.read(cx);
                            let p = &wb.target_pane;
                            (
                                !p.site_map_loading
                                    && p.selected_row.is_some()
                                    && wb.project.is_some()
                                    && wb.transition.is_none()
                                    && wb.backend_available(),
                                !p.tree_state.nodes.is_empty(),
                                !p.tree_state.hidden_prefixes.is_empty()
                                    || !p.hidden_item_ids.is_empty(),
                                p.item_sort.is_some(),
                                p.tree_state.selected_key.is_some(),
                                p.site_map_loading,
                            )
                        };
                        let o_ref = options_owner.clone();
                        let o_ext = options_owner.clone();
                        let o_all = options_owner.clone();
                        let o_exp = options_owner.clone();
                        let o_col = options_owner.clone();
                        let o_hid = options_owner.clone();
                        let o_srt = options_owner.clone();
                        menu.compact()
                            .min_w(px(176.))
                            .item(
                                PopupMenuItem::new("刷新站点地图")
                                    .disabled(is_loading)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_ref.update(cx, |v, cx| {
                                            v.refresh_target_site_map(0, window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("提取选中响应链接")
                                    .disabled(!can_extract)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_ext.update(cx, |v, cx| {
                                            v.extract_selected_target_response_links(window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("显示全部站点")
                                    .disabled(!has_selected_node)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_all.update(cx, |v, cx| {
                                            v.select_target_node(None, window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("展开全部节点")
                                    .disabled(!has_nodes)
                                    .on_click(move |_, _, cx| {
                                        let _ = o_exp.update(cx, |v, cx| {
                                            for node in &v.target_pane.tree_state.nodes {
                                                v.target_pane
                                                    .tree_state
                                                    .expanded
                                                    .insert(node.key.clone());
                                            }
                                            cx.notify();
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("折叠全部节点")
                                    .disabled(!has_nodes)
                                    .on_click(move |_, _, cx| {
                                        let _ = o_col.update(cx, |v, cx| {
                                            v.target_pane.tree_state.expanded.clear();
                                            cx.notify();
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("恢复已隐藏分支")
                                    .disabled(!has_hidden)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_hid.update(cx, |v, cx| {
                                            v.show_hidden_target_branches(window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("清除排序")
                                    .disabled(!has_sort)
                                    .on_click(move |_, _, cx| {
                                        let _ = o_srt.update(cx, |v, cx| {
                                            v.target_pane.item_sort = None;
                                            cx.notify();
                                        });
                                    }),
                            )
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_site_map_split_body(
        &mut self,
        view_ctx: &TargetViewContext<'_>,
        subview: SiteMapSubView,
        issues_count: usize,
        loading: bool,
        search_input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let right_header =
            self.render_site_map_right_header(subview, issues_count, loading, search_input, cx);
        match view_ctx.site_map_layout {
            SiteMapLayoutMode::TreeLeftStackedRight => h_resizable("target-sitemap-hsplit")
                .child(
                    resizable_panel()
                        .size(px(260.))
                        .size_range(px(160.)..px(720.))
                        .child(self.render_target_tree(view_ctx, cx)),
                )
                .child(
                    resizable_panel().size_range(px(280.)..px(10000.)).child(
                        div()
                            .size_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(right_header)
                            .child(div().flex_1().min_h_0().child(match subview {
                                SiteMapSubView::Contents => v_resizable("target-sitemap-vsplit")
                                    .child(
                                        resizable_panel()
                                            .size(px(260.))
                                            .size_range(px(120.)..px(800.))
                                            .child(self.render_target_items(view_ctx, window, cx)),
                                    )
                                    .child(
                                        resizable_panel()
                                            .size_range(px(140.)..px(10000.))
                                            .child(self.render_target_message_pair(cx)),
                                    )
                                    .into_any_element(),
                                SiteMapSubView::Issues => {
                                    self.render_site_map_issues_pane(window, cx)
                                }
                            })),
                    ),
                )
                .into_any_element(),
            SiteMapLayoutMode::ThreeColumn => h_resizable("target-sitemap-hsplit-3col")
                .child(
                    resizable_panel()
                        .size(px(230.))
                        .size_range(px(150.)..px(560.))
                        .child(self.render_target_tree(view_ctx, cx)),
                )
                .child(
                    resizable_panel()
                        .size(px(460.))
                        .size_range(px(220.)..px(900.))
                        .child(
                            div()
                                .size_full()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .border_r_1()
                                .border_color(theme::border())
                                .child(right_header)
                                .child(div().flex_1().min_h_0().child(match subview {
                                    SiteMapSubView::Contents => {
                                        self.render_target_items(view_ctx, window, cx)
                                    }
                                    SiteMapSubView::Issues => {
                                        self.render_site_map_issues_pane(window, cx)
                                    }
                                })),
                        ),
                )
                .child(
                    resizable_panel()
                        .size_range(px(260.)..px(10000.))
                        .child(self.render_target_message_pair(cx)),
                )
                .into_any_element(),
        }
    }

    #[inline(never)]
    pub(super) fn render_site_map(
        &mut self,
        view_ctx: &TargetViewContext<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.target_pane;
        let hidden_count = pane.tree_state.hidden_prefixes.len() + pane.hidden_item_ids.len();
        let issues_count = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            pane.tree_state.selected_key.as_deref(),
            &pane.tree_state.hidden_prefixes,
        )
        .len();
        let subview = pane.site_map_subview;
        let loading = pane.site_map_loading;
        let search_input = pane.search.clone();
        let search_val = search_input.read(cx).value().to_string();
        let filter_summary_text = site_map_filter_summary(pane, &search_val);

        let top_filter_bar =
            self.render_site_map_top_bar(hidden_count, view_ctx, filter_summary_text, cx);
        let split_body = self.render_site_map_split_body(
            view_ctx,
            subview,
            issues_count,
            loading,
            search_input,
            window,
            cx,
        );
        let engagement_overlay = self.render_target_engagement_overlay(cx);
        let compare_overlay = self.render_target_compare_overlay(cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(top_filter_bar)
            .child(div().flex_1().min_h_0().child(split_body))
            .when_some(self.target_pane.site_map_error.clone(), |this, error| {
                this.child(
                    toolbar()
                        .id("target-site-map-error")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Alert)
                        .aria_label(error.clone())
                        .bg(rgb(if theme::is_dark() { 0x4a2020 } else { 0xffefed }))
                        .child(error),
                )
            })
            .when_some(engagement_overlay, |this, overlay| this.child(overlay))
            .when_some(compare_overlay, |this, overlay| this.child(overlay))
            .into_any_element()
    }
}
