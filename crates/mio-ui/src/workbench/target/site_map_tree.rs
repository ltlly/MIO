use super::*;

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

impl Workbench {
    pub(super) fn render_target_tree(
        &mut self,
        view_ctx: &TargetViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_mode = view_ctx.site_map_view_mode;
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
        let tree_scroll = view_ctx.tree_scroll.clone();

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
                super::menus_tree::target_all_menu(all_menu_view.clone(), menu, cx)
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
}
