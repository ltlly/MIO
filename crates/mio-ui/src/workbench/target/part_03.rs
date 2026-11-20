use std::rc::Rc;

use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::{ClickEvent, uniform_list};

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

struct CrawlPathNodeView {
    key: String,
    display_label: String,
    depth: u8,
    kind: SiteMapNodeKind,
    is_https: bool,
    in_scope: bool,
    subtree_count: usize,
    out_degree: usize,
    has_children: bool,
    expanded: bool,
    matched_row: Option<HistoryRow>,
}

fn build_crawl_path_tree_views(
    nodes: &[SiteMapNode],
    visible: &[&SiteMapNode],
    expanded_set: &HashSet<String>,
    items: &[HistoryRow],
) -> Vec<CrawlPathNodeView> {
    let mut child_count_by_key: HashMap<&str, usize> = HashMap::with_capacity(nodes.len());
    for n in nodes {
        if let Some(parent) = n.parent_key.as_deref() {
            *child_count_by_key.entry(parent).or_insert(0) += 1;
        }
    }

    let mut row_by_prefix: HashMap<String, (String, HistoryRow)> = HashMap::new();
    for row in items {
        let summary = super::mime::target_row_summary(row);
        let path_clean = summary
            .url
            .split('?')
            .next()
            .unwrap_or(summary.url.as_str())
            .trim_end_matches('/');
        let full_url = format!("{}{}", summary.host.trim_end_matches('/'), summary.url);
        let method = if summary.method.is_empty() {
            "GET".to_owned()
        } else {
            summary.method.clone()
        };
        let norm_prefix = format!(
            "{}{}",
            summary.host.trim_end_matches('/'),
            if path_clean.is_empty() { "/" } else { path_clean }
        );
        row_by_prefix
            .entry(norm_prefix)
            .or_insert_with(|| (method.clone(), row.clone()));
        row_by_prefix
            .entry(full_url)
            .or_insert_with(|| (method, row.clone()));
    }

    let mut out = Vec::with_capacity(visible.len());
    for node in visible {
        let out_degree = child_count_by_key.get(node.key.as_str()).copied().unwrap_or(0);
        let has_children = out_degree > 0 || node.subtree_count > node.direct_count;
        let expanded = expanded_set.contains(&node.key);
        if node.kind == SiteMapNodeKind::Origin {
            let host_label = display_tree_node_label(node);
            out.push(CrawlPathNodeView {
                key: node.key.clone(),
                display_label: format!("[Direct] → {host_label}"),
                depth: node.depth,
                kind: node.kind,
                is_https: origin_is_https(node),
                in_scope: node.in_scope,
                subtree_count: node.subtree_count,
                out_degree,
                has_children,
                expanded,
                matched_row: None,
            });
            continue;
        }
        let clean_prefix = node.url_prefix.trim_end_matches('/');
        let matched = row_by_prefix
            .get(node.url_prefix.as_str())
            .or_else(|| row_by_prefix.get(clean_prefix))
            .or_else(|| row_by_prefix.get(node.key.as_str()))
            .cloned();
        let method = matched
            .as_ref()
            .map(|(m, _)| m.as_str())
            .unwrap_or(if node.kind == SiteMapNodeKind::Folder {
                "NAV"
            } else {
                "GET"
            });
        let seg = format!("/{}", node.label.trim_start_matches('/'));
        let transition_label = if out_degree > 0 {
            format!("↳ {method} {seg} (→{out_degree})")
        } else {
            format!("↳ {method} {seg}")
        };
        out.push(CrawlPathNodeView {
            key: node.key.clone(),
            display_label: transition_label,
            depth: node.depth,
            kind: node.kind,
            is_https: false,
            in_scope: node.in_scope,
            subtree_count: node.subtree_count,
            out_degree,
            has_children,
            expanded,
            matched_row: matched.map(|(_, r)| r),
        });
    }
    out
}

fn display_tree_node_label(node: &SiteMapNode) -> String {
    if node.kind != SiteMapNodeKind::Origin {
        return node.label.clone();
    }
    let raw = node.label.trim();
    if let Some((scheme_raw, rest)) = raw.split_once("://") {
        let scheme = scheme_raw.to_ascii_lowercase();
        let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let stripped = if scheme == "http" {
            authority.strip_suffix(":80").unwrap_or(authority)
        } else if scheme == "https" {
            authority.strip_suffix(":443").unwrap_or(authority)
        } else {
            authority
        };
        if !stripped.is_empty() {
            return stripped.to_owned();
        }
    }
    raw.strip_suffix(":80")
        .or_else(|| raw.strip_suffix(":443"))
        .unwrap_or(raw)
        .to_owned()
}

pub(super) fn origin_is_https(node: &SiteMapNode) -> bool {
    node.url_prefix
        .trim()
        .to_ascii_lowercase()
        .starts_with("https://")
        || node
            .key
            .trim()
            .to_ascii_lowercase()
            .starts_with("https://")
}

pub(super) fn site_map_host_icon(is_https: bool, halo_bg: gpui_kit::Hsla) -> AnyElement {
    let dark = theme::is_dark();
    let globe_color: gpui_kit::Hsla = rgb(if dark { 0x6ba4e8 } else { 0x26649d }).into();
    let lock_color: gpui_kit::Hsla = if is_https {
        rgb(if dark { 0x40c057 } else { 0x2b8a3e }).into()
    } else {
        rgb(if dark { 0xff6b6b } else { 0xd92d20 }).into()
    };
    // External host icon: crisp 15x15 padlock (open red shackle for HTTP, closed green shackle
    // for HTTPS) with a keyhole, overlaid at bottom-right by a small blue globe badge.
    let lock_svg: &'static [u8] = if is_https {
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><rect x=\"2\" y=\"6.8\" width=\"10.5\" height=\"7.4\" rx=\"1.4\"/><path d=\"M4.4 6.8V4.6a2.85 2.85 0 0 1 5.7 0v2.2\"/><circle cx=\"7.25\" cy=\"10.1\" r=\"1\" fill=\"currentColor\" stroke=\"none\"/><path d=\"M7.25 10.6v1.7\" stroke-width=\"1.4\"/></svg>"
    } else {
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><rect x=\"2\" y=\"6.8\" width=\"10.5\" height=\"7.4\" rx=\"1.4\"/><path d=\"M4.4 6.8V4.6a2.85 2.85 0 0 1 5.1-1.7\"/><circle cx=\"7.25\" cy=\"10.1\" r=\"1\" fill=\"currentColor\" stroke=\"none\"/><path d=\"M7.25 10.6v1.7\" stroke-width=\"1.4\"/></svg>"
    };
    div()
        .relative()
        .w(px(16.))
        .h(px(16.))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .child(
                    Icon::default()
                        .data(lock_svg)
                        .size(px(15.))
                        .text_color(lock_color),
                ),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .rounded_full()
                .bg(halo_bg)
                .child(
                    Icon::new(IconName::Globe)
                        .size(px(8.5))
                        .text_color(globe_color),
                ),
        )
        .into_any_element()
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
    pub(in crate::workbench) fn render_site_map(
        &mut self,
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
        let view_mode = pane.site_map_view_mode;
        let layout_mode = pane.site_map_layout;
        let loading = pane.site_map_loading;
        let search_input = pane.search.clone();
        let search_val = search_input.read(cx).value().to_string();
        let filter_summary_text = site_map_filter_summary(pane, &search_val);
        let dark = theme::is_dark();
        let filter_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xeeeeee }).into();
        let inactive_seg_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xdfdfdf }).into();
        let active_seg_bg: gpui_kit::Hsla = rgb(0x26649d).into();
        let owner = cx.weak_entity();
        let filter_owner = owner.clone();
        let options_owner = owner.clone();

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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.target_pane.site_map_view_mode = SiteMapViewMode::UrlView;
                        cx.notify();
                    })),
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.target_pane.site_map_view_mode = SiteMapViewMode::CrawlPathsView;
                        cx.notify();
                    })),
            );

        let filter_popover = div().flex_1().min_w_0().child(
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
                                                            this.target_pane.status_classes[idx] =
                                                                !this.target_pane.status_classes
                                                                    [idx];
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
                                                    .update(cx, |s, cx| s.set_value("", window, cx));
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
        );

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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.target_pane.site_map_layout = SiteMapLayoutMode::ThreeColumn;
                        cx.notify();
                    })),
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.target_pane.site_map_layout = SiteMapLayoutMode::TreeLeftStackedRight;
                        cx.notify();
                    })),
            );

        let top_filter_bar = div()
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
            .child(layout_toggle);

        let right_header = div()
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
            );

        let split_body = match layout_mode {
            SiteMapLayoutMode::TreeLeftStackedRight => h_resizable("target-sitemap-hsplit")
                .child(
                    resizable_panel()
                        .size(px(260.))
                        .size_range(px(160.)..px(720.))
                        .child(self.render_target_tree(cx)),
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
                                            .child(self.render_target_items(window, cx)),
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
                        .child(self.render_target_tree(cx)),
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
                                        self.render_target_items(window, cx)
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
        };

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

    pub(in crate::workbench) fn render_target_tree(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_mode = self.target_pane.site_map_view_mode;
        let hide_empty = self.target_pane.hide_empty_folders;
        let visible: Vec<&SiteMapNode> = self
            .target_pane
            .tree_state
            .visible_nodes()
            .into_iter()
            .filter(|node| !(hide_empty && node.kind == SiteMapNodeKind::Folder && node.subtree_count == 0))
            .collect();
        let selected_key = self.target_pane.tree_state.selected_key.clone();
        let weak = cx.weak_entity();
        let all_menu_view = weak.clone();
        let hover_bg: gpui_kit::Hsla = rgb(if theme::is_dark() { 0x32353a } else { 0xf0f2f5 }).into();
        let tree_scroll = self.target_pane.tree_scroll.clone();

        let mut rows = div()
            .id("target-sitemap-tree")
            .test_support()
            .key_context("DataTable")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&tree_scroll)
            .flex()
            .flex_col()
            .py(px(3.))
            .bg(theme::surface())
            .on_click(cx.listener(|this, _, window, cx| {
                if this.target_pane.tree_state.selected_key.is_some() {
                    this.select_target_node(None, window, cx);
                }
            }))
            .context_menu(move |menu, _, cx| {
                super::part_04::target_all_menu(all_menu_view.clone(), menu, cx)
            });

        let crawl_views = if view_mode == SiteMapViewMode::CrawlPathsView {
            build_crawl_path_tree_views(
                &self.target_pane.tree_state.nodes,
                &visible,
                &self.target_pane.tree_state.expanded,
                &self.target_pane.items,
            )
        } else {
            Vec::new()
        };

        let item_count = if view_mode == SiteMapViewMode::CrawlPathsView {
            crawl_views.len()
        } else {
            visible.len()
        };

        for index in 0..item_count {
            let (
                key,
                display_label,
                depth,
                kind,
                is_https,
                in_scope,
                subtree_count,
                has_children,
                expanded,
                matched_row,
            ) = if view_mode == SiteMapViewMode::CrawlPathsView {
                let cv = &crawl_views[index];
                let _ = cv.out_degree;
                (
                    cv.key.clone(),
                    cv.display_label.clone(),
                    cv.depth,
                    cv.kind,
                    cv.is_https,
                    cv.in_scope,
                    cv.subtree_count,
                    cv.has_children,
                    cv.expanded,
                    cv.matched_row.clone(),
                )
            } else {
                let node = visible[index];
                let k = node.key.clone();
                let exp = self.target_pane.tree_state.expanded.contains(&k);
                let hc = self.target_pane.tree_state.has_children(node);
                (
                    k,
                    display_tree_node_label(node),
                    node.depth,
                    node.kind,
                    origin_is_https(node),
                    node.in_scope,
                    node.subtree_count,
                    hc,
                    exp,
                    None,
                )
            };
            let selected = selected_key.as_deref() == Some(key.as_str());
            let row_id = ("target-sitemap-node", index);
            let fold_id = ("target-sitemap-fold", index);
            let menu_view = weak.clone();
            let key_right = key.clone();
            let key_click = key.clone();
            let key_menu = key.clone();
            let key_fold = key.clone();
            let aria_text = format!(
                "{} ({}){}",
                display_label,
                subtree_count,
                if in_scope { " · in scope" } else { "" }
            );
            let icon_el: AnyElement = match kind {
                SiteMapNodeKind::Origin => site_map_host_icon(
                    is_https,
                    if selected {
                        theme::selected()
                    } else {
                        theme::surface()
                    },
                ),
                SiteMapNodeKind::Folder => Icon::new(if expanded && has_children {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                })
                .size(theme::scaled_px(13.))
                .text_color(theme::muted_foreground())
                .into_any_element(),
                SiteMapNodeKind::File => Icon::new(IconName::FileText)
                    .size(theme::scaled_px(13.))
                    .text_color(theme::muted_foreground())
                    .into_any_element(),
            };
            let disclosure_el: AnyElement = if has_children {
                div()
                    .id(fold_id)
                    .test_support()
                    .w(px(14.))
                    .h(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.target_pane.tree_state.toggle_node(key_fold.clone());
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(11.))
                        .text_color(theme::muted_foreground()),
                    )
                    .into_any_element()
            } else {
                div().w(px(14.)).flex_shrink_0().into_any_element()
            };
            rows = rows.child(
                div()
                    .id(row_id)
                    .test_support()
                    .aria_label(aria_text)
                    .aria_selected(selected)
                    .h(theme::scaled_px(21.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .pl(px(6. + f32::from(depth) * 16.))
                    .pr(px(8.))
                    .cursor_pointer()
                    .bg(if selected {
                        theme::selected()
                    } else {
                        theme::surface()
                    })
                    .when(!selected, |row| row.hover(move |s| s.bg(hover_bg)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            this.select_target_node(Some(key_right.clone()), window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.select_target_node(Some(key_click.clone()), window, cx);
                        if let Some(ref row) = matched_row {
                            this.select_target_row(row, window, cx);
                        }
                    }))
                    .context_menu(move |menu, window, cx| {
                        target_node_menu(menu_view.clone(), key_menu.clone(), menu, window, cx)
                    })
                    .child(disclosure_el)
                    .child(icon_el)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child(display_label),
                    ),
            );
        }

        div()
            .size_full()
            .min_h_0()
            .flex()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(rows)
            .child(mio_vertical_scrollbar_track(
                "target-sitemap-tree-vscroll",
                &tree_scroll,
            ))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_target_items(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_id = self.target_pane.selected_id;
        let sort = self.target_pane.item_sort;
        let loaded_count = self.target_pane.items.len();
        let total_items = self.target_pane.total_items;
        let loading = self.target_pane.site_map_loading;
        let hide_not_found = self.target_pane.hide_not_found;
        let status_classes = self.target_pane.status_classes;
        let hide_binary = self.target_pane.hide_css_image_binary;
        let items_v_scroll = self.target_pane.items_v_scroll.clone();
        let items_h_scroll = self.target_pane.items_h_scroll.clone();

        let items_ptr = self.target_pane.items.as_ptr() as usize;
        let first_item = self
            .target_pane
            .items
            .first()
            .map(|r| (r.id(), row_status_code(r), row_response_length(r)));
        let last_item = self
            .target_pane
            .items
            .last()
            .map(|r| (r.id(), row_status_code(r), row_response_length(r)));
        let query_revision = self.target_pane.query_revision;
        let history_query_revision = self.query_revision;
        let row_metadata_len = self.history.read(cx).delegate().row_metadata.len();
        let selected_response_len = self.target_pane.selected_response_bytes.len();
        let response_rev =
            crate::workbench::inspector::message_editor_rev_key(&self.target_pane.response, cx);
        let hidden_item_ids_len = self.target_pane.hidden_item_ids.len();
        let hidden_item_ids_sig = self
            .target_pane
            .hidden_item_ids
            .iter()
            .fold(0u64, |acc, id| {
                let raw = match *id {
                    HistoryEntryId::Live(flow_id) => flow_id.as_u128(),
                    HistoryEntryId::External { source_id, item_id } => {
                        source_id.as_u128() ^ item_id.as_u128().rotate_left(17)
                    }
                };
                let mut x = (raw as u64) ^ ((raw >> 64) as u64);
                x ^= x >> 30;
                x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
                x ^= x >> 27;
                x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
                acc.wrapping_add(x ^ (x >> 31))
            });

        let cached_rows = {
            let guard = self.target_pane.items_cache.borrow();
            guard
                .as_ref()
                .filter(|entry| {
                    let k = &entry.key;
                    k.items_ptr == items_ptr
                        && k.items_len == loaded_count
                        && k.first_item == first_item
                        && k.last_item == last_item
                        && k.query_revision == query_revision
                        && k.history_query_revision == history_query_revision
                        && k.row_metadata_len == row_metadata_len
                        && k.selected_id == selected_id
                        && k.selected_response_len == selected_response_len
                        && k.response_rev == response_rev
                        && k.hidden_item_ids_len == hidden_item_ids_len
                        && k.hidden_item_ids_sig == hidden_item_ids_sig
                        && k.sort == sort
                        && k.hide_not_found == hide_not_found
                        && k.status_classes == status_classes
                        && k.hide_css_image_binary == hide_binary
                        && k.hidden_prefixes.as_slice()
                            == self.target_pane.tree_state.hidden_prefixes.as_slice()
                })
                .map(|entry| Rc::clone(&entry.rows))
        };

        let cached_rows = if let Some(hit) = cached_rows {
            hit
        } else {
            let fallback_response_text = if selected_id.is_some()
                && self.target_pane.selected_response_bytes.is_empty()
                && self.target_pane.response.raw.read(cx).text().len() > 0
            {
                Some(self.target_pane.response.raw.read(cx).value())
            } else {
                None
            };
            let loaded_response: Option<&[u8]> = if selected_id.is_none() {
                None
            } else if !self.target_pane.selected_response_bytes.is_empty() {
                Some(self.target_pane.selected_response_bytes.as_slice())
            } else {
                fallback_response_text
                    .as_deref()
                    .filter(|raw| !raw.is_empty())
                    .map(str::as_bytes)
            };
            let built = {
                let row_metadata = &self.history.read(cx).delegate().row_metadata;
                Rc::new(build_sorted_target_rows_with_metadata(
                    &self.target_pane.items,
                    selected_id,
                    loaded_response,
                    Some(row_metadata),
                    &self.target_pane.tree_state.hidden_prefixes,
                    &self.target_pane.hidden_item_ids,
                    hide_not_found,
                    status_classes,
                    hide_binary,
                    sort,
                ))
            };
            *self.target_pane.items_cache.borrow_mut() = Some(CachedTargetItems {
                key: TargetItemsCacheKey {
                    items_ptr,
                    items_len: loaded_count,
                    first_item,
                    last_item,
                    query_revision,
                    history_query_revision,
                    row_metadata_len,
                    selected_id,
                    selected_response_len,
                    response_rev,
                    hidden_prefixes: self.target_pane.tree_state.hidden_prefixes.clone(),
                    hidden_item_ids_len,
                    hidden_item_ids_sig,
                    sort,
                    hide_not_found,
                    status_classes,
                    hide_css_image_binary: hide_binary,
                },
                rows: Rc::clone(&built),
            });
            built
        };

        let row_count = cached_rows.len();
        let weak = cx.weak_entity();
        let col_widths = self.target_pane.item_col_widths;
        let col_visible = self.target_pane.item_col_visible;
        let visible_cols: Vec<usize> = (0..SITEMAP_COL_COUNT)
            .filter(|&col_ix| col_visible[col_ix])
            .collect();
        let flex_col_ix = if col_visible[2] {
            2
        } else {
            visible_cols.last().copied().unwrap_or(0)
        };
        let total_min_w = theme::scaled_px(
            visible_cols
                .iter()
                .map(|&col_ix| col_widths[col_ix])
                .sum::<f32>()
                .max(260.),
        );
        let selected_ids_rc = std::rc::Rc::new(self.target_pane.selected_ids.clone());
        let visible_ids: std::rc::Rc<Vec<HistoryEntryId>> = std::rc::Rc::new(
            cached_rows
                .iter()
                .filter_map(|(item_index, _)| {
                    self.target_pane.items.get(*item_index).map(HistoryRow::id)
                })
                .collect(),
        );
        let list_rows = std::rc::Rc::clone(&cached_rows);
        let stripe_bg = if theme::table_zebra_stripe() {
            table_stripe_bg()
        } else {
            theme::surface()
        };
        let sel_border = selected_row_border();
        let corner_bg: gpui_kit::Hsla = rgb(if theme::is_dark() { 0x2b2d30 } else { 0xf7f7f7 }).into();
        let corner_border: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x3c3f41 } else { 0xe0e0e0 }).into();

        let list = uniform_list("target-sitemap-items", row_count, move |range, _, cx| {
            let Some(owner) = weak.upgrade() else {
                return Vec::new();
            };
            if !loading
                && loaded_count > 0
                && loaded_count < total_items
                && range.end.saturating_add(30) >= row_count
            {
                let load_owner = weak.clone();
                cx.defer(move |cx| {
                    let _ = load_owner.update(cx, |this, cx| {
                        this.load_more_target_site_map(loaded_count, cx);
                    });
                });
            }
            let workbench = owner.read(cx);
            let items = &workbench.target_pane.items;
            let delegate = workbench.history.read(cx).delegate();
            let live_highlights = delegate.live_highlights.borrow();
            let live_notes = delegate.live_notes.borrow();
            let rendered = range
                .filter_map(|index| {
                    let (item_index, summary) = list_rows.get(index)?;
                    let row = items.get(*item_index)?;
                    Some((index, row, summary))
                })
                .map(|(index, row, summary)| {
                    let selected = selected_ids_rc.contains(&row.id())
                        || (selected_ids_rc.is_empty() && selected_id == Some(row.id()));
                    let hl_color = match row {
                        HistoryRow::ExtProxy(archive) => {
                            archive.highlight.as_ref().map(|h| h.color)
                        }
                        HistoryRow::Live(flow) => live_highlights.get(&flow.id).map(|h| h.color),
                    }
                    .filter(|c| (1..=9).contains(c));
                    let note_preview = match row {
                        HistoryRow::ExtProxy(archive) => archive
                            .notes_preview
                            .replace("\r\n", " ")
                            .replace(['\r', '\n', '\t'], " "),
                        HistoryRow::Live(flow) => live_notes
                            .get(&flow.id)
                            .map(|(_, text)| {
                                text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ")
                            })
                            .unwrap_or_default(),
                    };
                    let has_note = !note_preview.trim().is_empty();
                    let row_label = format!(
                        "{} {} {} {} {} {} {}{}",
                        summary.host,
                        summary.method,
                        summary.url,
                        summary.status,
                        summary.length,
                        summary.mime,
                        summary.title,
                        if has_note {
                            format!(" {note_preview}")
                        } else {
                            String::new()
                        }
                    );
                    let row_bg = if selected {
                        theme::selected()
                    } else if let Some(bg) = hl_color.and_then(crate::history::highlight_background)
                    {
                        bg
                    } else if index % 2 == 1 {
                        stripe_bg
                    } else {
                        theme::surface()
                    };
                    let right_view = weak.clone();
                    let click_view = weak.clone();
                    let menu_view = weak.clone();
                    let row_right = row.clone();
                    let row_click = row.clone();
                    let click_order = std::rc::Rc::clone(&visible_ids);
                    let mut row_cells = Vec::with_capacity(visible_cols.len());
                    for &col_ix in &visible_cols {
                        let w = col_widths[col_ix];
                        let text = match col_ix {
                            0 => summary.host.clone(),
                            1 => summary.method.clone(),
                            2 => summary.url.clone(),
                            3 => summary.params.clone(),
                            4 => summary.status.clone(),
                            5 => summary.length.clone(),
                            6 => summary.mime.clone(),
                            7 => summary.title.clone(),
                            8 if has_note => note_preview.clone(),
                            _ => String::new(),
                        };
                        let cell = if col_ix == flex_col_ix {
                            div()
                                .flex_1()
                                .min_w(theme::scaled_px(w))
                                .px(px(6.))
                                .truncate()
                                .child(text)
                        } else {
                            div()
                                .w(theme::scaled_px(w))
                                .flex_shrink_0()
                                .px(px(6.))
                                .truncate()
                                .child(text)
                        };
                        row_cells.push(cell);
                    }
                    div()
                        .id(("target-sitemap-row", index))
                        .test_support()
                        .aria_label(row_label)
                        .aria_selected(selected)
                        .w_full()
                        .h(theme::row_height())
                        .flex()
                        .items_center()
                        .gap_0()
                        .cursor_pointer()
                        .bg(row_bg)
                        .when(selected, |el| el.border_1().border_color(sel_border))
                        .when(!selected && hl_color.is_some(), |el| {
                            el.text_color(crate::history::highlight_foreground(
                                hl_color.unwrap_or(0),
                            ))
                        })
                        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                            let _ = right_view.update(cx, |this, cx| {
                                this.handle_target_row_right_down(&row_right, window, cx);
                            });
                        })
                        .on_click(move |event: &ClickEvent, window, cx| {
                            let shift = event.modifiers().shift;
                            let toggle = event.modifiers().secondary(cx);
                            let _ = click_view.update(cx, |this, cx| {
                                this.handle_target_row_click(
                                    &row_click,
                                    &click_order,
                                    shift,
                                    toggle,
                                    window,
                                    cx,
                                );
                            });
                        })
                        .context_menu(move |menu, window, cx| {
                            target_item_menu(menu_view.clone(), menu, window, cx)
                        })
                        .children(row_cells)
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            drop(live_notes);
            drop(live_highlights);
            rendered
        })
        .track_scroll(&items_v_scroll)
        .size_full();

        let header = self.render_target_items_header(sort, cached_rows, cx);
        let h_scroll_body = div()
            .id("target-sitemap-items-hwrap")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(&items_h_scroll)
            .child(
                div()
                    .h_full()
                    .w_full()
                    .min_w(total_min_w)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().w_full().child(list)),
            );

        let v_scroll_col = div()
            .w(px(12.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .h(theme::table_header_height())
                    .flex_shrink_0()
                    .border_b_1()
                    .border_l_1()
                    .border_color(theme::border())
                    .bg(table_header_bg()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(mio_vertical_scrollbar_track(
                        "target-sitemap-items-vscroll",
                        &items_v_scroll,
                    )),
            );

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(h_scroll_body)
                    .child(v_scroll_col),
            )
            .child(
                div()
                    .w_full()
                    .h(px(12.))
                    .flex_shrink_0()
                    .flex()
                    .child(mio_horizontal_scrollbar_track(
                        "target-sitemap-items-hscroll",
                        &items_h_scroll,
                    ))
                    .child(
                        div()
                            .w(px(12.))
                            .h_full()
                            .flex_shrink_0()
                            .border_t_1()
                            .border_l_1()
                            .border_color(corner_border)
                            .bg(corner_bg),
                    ),
            )
            .into_any_element()
    }
}
