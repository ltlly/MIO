use gpui_kit::accesskit::Role;

use super::menus::*;
use super::*;

pub(in crate::workbench) fn target_node_menu(
    view: WeakEntity<Workbench>,
    key: String,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (
        available,
        audit_ok,
        node_opt,
        in_scope,
        branch_issues_count,
        view_mode,
        layout_mode,
    ) = {
        let wb = entity.read(cx);
        let avail = wb.project.is_some() && wb.backend_available() && wb.transition.is_none();
        let node = wb
            .target_pane
            .tree_state
            .nodes
            .iter()
            .find(|n| n.key == key)
            .cloned();
        let scope_url = node
            .as_ref()
            .map(|n| {
                if n.url_prefix.is_empty() {
                    n.key.as_str()
                } else {
                    n.url_prefix.as_str()
                }
            })
            .unwrap_or(key.as_str());
        let in_scope = wb.workspace.scope.is_url_in_scope(scope_url);
        let issues_count = filter_branch_findings(
            &wb.audit_dashboard.snapshot.findings,
            Some(&key),
            &wb.target_pane.tree_state.hidden_prefixes,
        )
        .len();
        let tv = wb.target_view.read(cx);
        (
            avail,
            wb.audit_can_operate(),
            node,
            in_scope,
            issues_count,
            tv.site_map_view_mode,
            tv.site_map_layout,
        )
    };

    let raw_header = node_opt
        .as_ref()
        .map(|node| {
            let base = if node.url_prefix.is_empty() {
                node.key.clone()
            } else {
                node.url_prefix.clone()
            };
            if node.kind == SiteMapNodeKind::Origin && !base.ends_with('/') {
                format!("{base}/")
            } else {
                base
            }
        })
        .unwrap_or_else(|| key.clone());
    let raw_header = strip_default_port(&raw_header);
    let header_title = truncate_middle(&raw_header, 52);
    let node_kind = node_opt
        .as_ref()
        .map_or(SiteMapNodeKind::Origin, |n| n.kind);
    let node_https = node_opt
        .as_ref()
        .is_some_and(super::site_map_tree::origin_is_https);
    let search_host = crate::workbench::wb_proxy::extract_url_host(&raw_header)
        .unwrap_or_else(|| raw_header.clone());

    let engagement_submenu = build_engagement_submenu(
        view.clone(),
        available,
        Some(key.clone()),
        search_host,
        true,
        false,
        window,
        cx,
    );

    let view_submenu = build_view_submenu(view.clone(), view_mode, layout_mode, window, cx);

    let v_scope = view.clone();
    let v_expand = view.clone();
    let v_expand_req = view.clone();
    let v_collapse = view.clone();
    let v_report = view.clone();
    let v_del_issues = view.clone();
    let v_scan = view.clone();
    let v_passive = view.clone();
    let v_active = view.clone();
    let v_delete_item = view.clone();
    let v_copy_urls = view.clone();
    let v_copy_links = view.clone();
    let v_save = view.clone();
    let v_compare = view.clone();
    let v_new_win = view.clone();
    let v_docs = view;

    let k_scope = key.clone();
    let k_expand = key.clone();
    let k_expand_req = key.clone();
    let k_collapse = key.clone();
    let k_report = key.clone();
    let k_del_issues = key.clone();
    let k_scan = key.clone();
    let k_passive = key.clone();
    let k_active = key.clone();
    let k_delete_item = key.clone();
    let k_copy_urls = key.clone();
    let k_copy_links = key.clone();
    let k_save = key.clone();
    let k_compare = key;

    let scope_item = if in_scope {
        PopupMenuItem::new("从范围中排除")
            .icon(IconName::CircleMinus)
            .disabled(!available)
            .on_click(move |_, _, cx| {
                let k = k_scope.clone();
                let _ = v_scope.update(cx, |wb, cx| {
                    let prefix = wb
                        .target_pane
                        .tree_state
                        .nodes
                        .iter()
                        .find(|node| node.key == k)
                        .map(|node| node.url_prefix.clone())
                        .unwrap_or(k);
                    if wb.workspace.scope.advanced_mode {
                        wb.add_advanced_scope_rule_from_prefix(prefix, false, cx);
                    } else {
                        wb.add_scope_rule(wb.project, false, prefix, cx);
                    }
                });
            })
    } else {
        PopupMenuItem::new("添加到范围")
            .icon(IconName::CirclePlus)
            .disabled(!available)
            .on_click(move |_, _, cx| {
                let k = k_scope.clone();
                let _ = v_scope.update(cx, |wb, cx| {
                    let prefix = wb
                        .target_pane
                        .tree_state
                        .nodes
                        .iter()
                        .find(|node| node.key == k)
                        .map(|node| node.url_prefix.clone())
                        .unwrap_or(k);
                    if wb.workspace.scope.advanced_mode {
                        wb.add_advanced_scope_rule_from_prefix(prefix, true, cx);
                    } else {
                        wb.add_scope_rule(wb.project, true, prefix, cx);
                    }
                });
            })
    };

    menu.compact()
        .with_size(gpui_kit::component::Size::Size(px(21.)))
        .scrollable(true)
        .max_h(px(640.))
        .item(
            PopupMenuItem::element(move |_, _| {
                let icon_el: AnyElement = match node_kind {
                    SiteMapNodeKind::Origin => {
                        super::site_map_tree::site_map_host_icon(node_https, theme::surface())
                    }
                    SiteMapNodeKind::Folder => Icon::new(IconName::Folder)
                        .size(px(14.))
                        .text_color(theme::muted_foreground())
                        .into_any_element(),
                    SiteMapNodeKind::File => Icon::new(IconName::FileText)
                        .size(px(14.))
                        .text_color(theme::muted_foreground())
                        .into_any_element(),
                };
                div()
                    .id("target-node-menu-header")
                    .test_support()
                    .role(Role::MenuItem)
                    .aria_label(header_title.clone())
                    .w_full()
                    .min_h(px(21.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(icon_el)
                    .child(div().min_w_0().truncate().child(header_title.clone()))
            })
            .disabled(true),
        )
        .separator()
        .item(scope_item)
        .item(PopupMenuItem::new("展开分支").on_click(move |_, _, cx| {
            let k = k_expand.clone();
            let _ = v_expand.update(cx, |wb, cx| wb.expand_target_branch(k, cx));
        }))
        .item(PopupMenuItem::new("展开已请求项").on_click(move |_, _, cx| {
            let k = k_expand_req.clone();
            let _ = v_expand_req.update(cx, |wb, cx| wb.expand_target_requested_items(k, cx));
        }))
        .item(PopupMenuItem::new("折叠分支").on_click(move |_, _, cx| {
            let k = k_collapse.clone();
            let _ = v_collapse.update(cx, |wb, cx| wb.collapse_target_branch(k, cx));
        }))
        .separator()
        .item(
            PopupMenuItem::new("报告问题")
                .disabled(!audit_ok || branch_issues_count == 0)
                .on_click(move |_, window, cx| {
                    let k = k_report.clone();
                    let _ = v_report.update(cx, |wb, cx| {
                        wb.save_branch_audit_report_to_file(k, true, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("删除问题")
                .disabled(!audit_ok || branch_issues_count == 0)
                .on_click(move |_, window, cx| {
                    let k = k_del_issues.clone();
                    let _ = v_del_issues.update(cx, |wb, cx| {
                        wb.delete_branch_audit_findings(k, window, cx);
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("扫描")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let k = k_scan.clone();
                    let _ = v_scan.update(cx, |wb, cx| {
                        wb.run_target_branch_action(
                            k,
                            BranchItemAction::OpenScanLauncher,
                            window,
                            cx,
                        );
                    });
                }),
        )
        .item(
            PopupMenuItem::new("被动扫描此主机")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let k = k_passive.clone();
                    let _ = v_passive.update(cx, |wb, cx| {
                        wb.run_target_branch_action(k, BranchItemAction::PassiveScan, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("主动扫描此主机")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let k = k_active.clone();
                    let _ = v_active.update(cx, |wb, cx| {
                        wb.run_target_branch_action(k, BranchItemAction::ActiveScan, window, cx);
                    });
                }),
        )
        .item(PopupMenuItem::submenu("参与工具", engagement_submenu).disabled(!available))
        .separator()
        .item(
            PopupMenuItem::new("删除项")
                .icon(IconName::Trash)
                .on_click(move |_, window, cx| {
                    let k = k_delete_item.clone();
                    let _ = v_delete_item.update(cx, |wb, cx| {
                        wb.hide_target_branch(k, window, cx);
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制此主机中的所有 URL")
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    let k = k_copy_urls.clone();
                    let _ = v_copy_urls.update(cx, |wb, cx| {
                        wb.copy_target_branch_urls(k, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("复制此主机中的链接")
                .icon(IconName::Copy)
                .on_click(move |_, window, cx| {
                    let k = k_copy_links.clone();
                    let _ = v_copy_links.update(cx, |wb, cx| {
                        wb.copy_target_branch_links(k, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("保存项")
                .icon(IconName::Save)
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let k = k_save.clone();
                    let _ = v_save.update(cx, |wb, cx| {
                        wb.save_target_branch_items_to_file(k, true, window, cx);
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("比较站点地图")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let k = k_compare.clone();
                    let _ = v_compare.update(cx, |wb, cx| {
                        wb.open_compare_site_maps(Some(k), window, cx);
                    });
                }),
        )
        .separator()
        .item(PopupMenuItem::submenu("视图", view_submenu))
        .item(
            PopupMenuItem::new("打开新站点地图窗口")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = v_new_win.update(cx, |wb, cx| {
                        wb.open_detached_site_map_window(window, cx);
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("站点地图文档").on_click(move |_, _, cx| {
                let doc_url =
                    "https://github.com/ltlly/MIO";
                cx.open_url(doc_url);
                let _ = v_docs.update(cx, |wb, cx| {
                    wb.status = format!("已打开站点地图文档：{doc_url}");
                    cx.notify();
                });
            }),
        )
}

pub(super) fn target_all_menu(
    view: WeakEntity<Workbench>,
    menu: PopupMenu,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (available, has_nodes, has_hidden) = {
        let wb = entity.read(cx);
        (
            wb.project.is_some() && wb.backend_available() && wb.transition.is_none(),
            !wb.target_pane.tree_state.nodes.is_empty(),
            !wb.target_pane.tree_state.hidden_prefixes.is_empty()
                || !wb.target_pane.hidden_item_ids.is_empty(),
        )
    };
    let expand_view = view.clone();
    let collapse_view = view.clone();
    let show_hidden_view = view.clone();
    let refresh_view = view.clone();
    let compare_view = view.clone();
    let new_win_view = view;
    menu.compact()
        .with_size(gpui_kit::component::Size::Size(px(21.)))
        .item(
            PopupMenuItem::new("展开全部节点")
                .disabled(!has_nodes)
                .on_click(move |_, _, cx| {
                    let _ = expand_view.update(cx, |v, cx| {
                        for node in &v.target_pane.tree_state.nodes {
                            v.target_pane.tree_state.expanded.insert(node.key.clone());
                        }
                        cx.notify();
                    });
                }),
        )
        .item(
            PopupMenuItem::new("折叠全部节点")
                .disabled(!has_nodes)
                .on_click(move |_, _, cx| {
                    let _ = collapse_view.update(cx, |v, cx| {
                        v.target_pane.tree_state.expanded.clear();
                        cx.notify();
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("恢复已隐藏分支")
                .disabled(!has_hidden)
                .on_click(move |_, window, cx| {
                    let _ = show_hidden_view.update(cx, |v, cx| {
                        v.show_hidden_target_branches(window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("刷新站点地图")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ =
                        refresh_view.update(cx, |v, cx| v.refresh_target_site_map(0, window, cx));
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("比较站点地图")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = compare_view
                        .update(cx, |v, cx| v.open_compare_site_maps(None, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("打开新站点地图窗口")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = new_win_view
                        .update(cx, |v, cx| v.open_detached_site_map_window(window, cx));
                }),
        )
}
