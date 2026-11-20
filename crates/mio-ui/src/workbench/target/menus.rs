use gpui_kit::accesskit::Role;
use gpui_kit::component::kbd::Kbd;
use mio_core::audit_engine::IssueConfidence;

use super::*;

fn truncate_middle(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 8 {
        return text.to_owned();
    }
    let head_len = (max_chars - 3) / 2;
    let tail_len = max_chars - 3 - head_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    format!("{head}...{tail}")
}

fn strip_default_port(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let default_suffix = if scheme.eq_ignore_ascii_case("http") {
        ":80"
    } else if scheme.eq_ignore_ascii_case("https") {
        ":443"
    } else {
        return url.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if let Some(host) = authority.strip_suffix(default_suffix)
        && !host.is_empty()
    {
        return format!("{scheme}://{host}{tail}");
    }
    url.to_owned()
}

fn shortcut_kbd(mac_key: &'static str, other_key: &'static str) -> Option<Kbd> {
    let spec = if cfg!(target_os = "macos") {
        mac_key
    } else {
        other_key
    };
    gpui_kit::Keystroke::parse(spec).ok().map(|ks| {
        Kbd::new(ks)
            .p_0()
            .flex_nowrap()
            .border_0()
            .bg(gpui_kit::transparent_black())
    })
}

fn shortcut_menu_item(
    id: &'static str,
    label: &'static str,
    mac_key: &'static str,
    other_key: &'static str,
) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        div()
            .id(id)
            .test_support()
            .role(Role::MenuItem)
            .aria_label(label)
            .w_full()
            .min_h(px(21.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(label)
            .children(shortcut_kbd(mac_key, other_key))
    })
}

fn build_view_submenu(
    view: WeakEntity<Workbench>,
    view_mode: SiteMapViewMode,
    layout_mode: SiteMapLayoutMode,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> Entity<PopupMenu> {
    PopupMenu::build(window, cx, move |sub, _, _| {
        let v_url = view.clone();
        let v_crawl = view.clone();
        let v_stacked = view.clone();
        let v_cols = view;
        sub.compact()
            .item(
                PopupMenuItem::new("URL view")
                    .checked(view_mode == SiteMapViewMode::UrlView)
                    .on_click(move |_, _, cx| {
                        let _ = v_url.update(cx, |wb, cx| {
                            wb.target_pane.site_map_view_mode = SiteMapViewMode::UrlView;
                            cx.notify();
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Crawl paths view")
                    .checked(view_mode == SiteMapViewMode::CrawlPathsView)
                    .on_click(move |_, _, cx| {
                        let _ = v_crawl.update(cx, |wb, cx| {
                            wb.target_pane.site_map_view_mode = SiteMapViewMode::CrawlPathsView;
                            cx.notify();
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("经典上下分栏")
                    .checked(layout_mode == SiteMapLayoutMode::TreeLeftStackedRight)
                    .on_click(move |_, _, cx| {
                        let _ = v_stacked.update(cx, |wb, cx| {
                            wb.target_pane.site_map_layout = SiteMapLayoutMode::TreeLeftStackedRight;
                            cx.notify();
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("三栏并排布局")
                    .checked(layout_mode == SiteMapLayoutMode::ThreeColumn)
                    .on_click(move |_, _, cx| {
                        let _ = v_cols.update(cx, |wb, cx| {
                            wb.target_pane.site_map_layout = SiteMapLayoutMode::ThreeColumn;
                            cx.notify();
                        });
                    }),
            )
    })
}

#[allow(clippy::too_many_arguments)]
fn build_engagement_submenu(
    view: WeakEntity<Workbench>,
    available: bool,
    scope_key: Option<String>,
    search_prefill: String,
    include_simulate_manual: bool,
    include_csrf_poc: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> Entity<PopupMenu> {
    PopupMenu::build(window, cx, move |sub, _, _| {
        let v_search = view.clone();
        let v_comments = view.clone();
        let v_scripts = view.clone();
        let v_refs = view.clone();
        let v_analyze = view.clone();
        let v_discover = view.clone();
        let v_sched = view.clone();
        let v_sim = view.clone();
        let v_csrf = view;

        let k_comments = scope_key.clone();
        let k_scripts = scope_key.clone();
        let k_refs = scope_key.clone();
        let k_analyze = scope_key.clone();
        let k_discover = scope_key.clone();
        let k_sim = scope_key.clone();

        let sub = sub
            .compact()
            .item(
                PopupMenuItem::new("搜索")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let term = search_prefill.clone();
                        let _ = v_search.update(cx, |wb, cx| {
                            wb.open_global_search_with_term(term, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("查找注释")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let key = k_comments.clone();
                        let _ = v_comments.update(cx, |wb, cx| {
                            wb.run_target_find_comments(key, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("查找脚本")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let key = k_scripts.clone();
                        let _ = v_scripts.update(cx, |wb, cx| {
                            wb.run_target_find_scripts(key, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("查找引用")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let key = k_refs.clone();
                        let _ = v_refs.update(cx, |wb, cx| {
                            wb.run_target_find_references(key, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("分析目标")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let key = k_analyze.clone();
                        let _ = v_analyze.update(cx, |wb, cx| {
                            wb.run_target_analyze(key, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("发现内容")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let key = k_discover.clone();
                        let _ = v_discover.update(cx, |wb, cx| {
                            wb.run_target_discover_content(key, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("计划任务")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let _ = v_sched.update(cx, |wb, cx| {
                            wb.open_project_tasks_settings(window, cx);
                        });
                    }),
            );
        let sub = if include_simulate_manual {
            sub.item(
                PopupMenuItem::new("模拟手动测试")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        if let Some(key) = k_sim.clone() {
                            let _ = v_sim.update(cx, |wb, cx| {
                                wb.run_target_branch_action(
                                    key,
                                    BranchItemAction::SimulateManualTesting,
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
            )
        } else {
            sub
        };
        if include_csrf_poc {
            sub.separator().item(
                PopupMenuItem::new("生成 CSRF PoC")
                    .disabled(!available)
                    .on_click(move |_, window, cx| {
                        let _ = v_csrf.update(cx, |wb, cx| {
                            wb.open_target_csrf_poc_modal(window, cx);
                        });
                    }),
            )
        } else {
            sub
        }
    })
}

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
        (
            avail,
            wb.audit_can_operate(),
            node,
            in_scope,
            issues_count,
            wb.target_pane.site_map_view_mode,
            wb.target_pane.site_map_layout,
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
        .is_some_and(super::part_03::origin_is_https);
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
                        super::part_03::site_map_host_icon(node_https, theme::surface())
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

pub(in crate::workbench) fn target_item_menu(
    view: WeakEntity<Workbench>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (
        available,
        project_opt,
        selected_id_opt,
        selected_row_opt,
        selected_count,
        annotation_available,
        host_label,
        row_url_full,
        in_scope,
        existing_findings,
        view_mode,
        layout_mode,
    ) = {
        let wb = entity.read(cx);
        let avail = wb.project.is_some() && wb.backend_available() && wb.transition.is_none();
        let sel_id = wb.target_pane.selected_id;
        let sel_row = wb.target_pane.selected_row.clone();
        let sel_count = wb
            .target_pane
            .selected_ids
            .len()
            .max(usize::from(sel_id.is_some()));
        let full_url = sel_row.as_ref().map(row_url).unwrap_or_default();
        let in_scope = !full_url.is_empty() && wb.workspace.scope.is_url_in_scope(&full_url);
        let host = crate::workbench::wb_proxy::extract_url_host(&full_url).unwrap_or_default();
        let annotations = if sel_count > 1 {
            wb.selected_target_rows_in_order()
                .iter()
                .any(|r| !wb.target_is_draft_entry(r.id()))
        } else {
            sel_id.is_some_and(|id| !wb.target_is_draft_entry(id))
        };
        let findings: Vec<(Uuid, String)> = wb
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .take(15)
            .map(|f| {
                let label = if f.host.is_empty() {
                    f.title.clone()
                } else {
                    format!("{} ({})", f.title, f.host)
                };
                (f.id, truncate_middle(&label, 48))
            })
            .collect();
        (
            avail,
            wb.project,
            sel_id,
            sel_row,
            sel_count,
            annotations,
            host,
            full_url,
            in_scope,
            findings,
            wb.target_pane.site_map_view_mode,
            wb.target_pane.site_map_layout,
        )
    };

    let has_selection = selected_row_opt.is_some() || selected_count > 0;
    let can_act = available && has_selection;
    let display_url = strip_default_port(&row_url_full);
    let header_url = if selected_count > 1 {
        format!("已选中 {selected_count} 个站点地图项")
    } else if display_url.is_empty() {
        "Site map item".to_owned()
    } else {
        truncate_middle(&display_url, 58)
    };
    let origin_opt = (!row_url_full.is_empty()).then(|| target_origin_from_url(&row_url_full));
    let search_prefill = if host_label.is_empty() {
        row_url_full.clone()
    } else {
        host_label.clone()
    };

    let has_existing_findings = !existing_findings.is_empty();
    let attach_existing_submenu = PopupMenu::build(window, cx, {
        let v_att = view.clone();
        let findings = existing_findings;
        move |inner, _, _| {
            if findings.is_empty() {
                return inner
                    .compact()
                    .item(PopupMenuItem::new("暂无现有问题").disabled(true));
            }
            findings
                .into_iter()
                .fold(inner.compact().min_w(px(220.)), |inner, (fid, label)| {
                    let owner = v_att.clone();
                    inner.item(
                        PopupMenuItem::new(label)
                            .disabled(!can_act)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |wb, cx| {
                                    wb.attach_target_row_to_issue(fid, cx);
                                });
                            }),
                    )
                })
        }
    });

    let record_issue_submenu = PopupMenu::build(window, cx, {
        let v_new = view.clone();
        move |sub, _, _| {
            sub.compact()
                .item(
                    PopupMenuItem::new("创建新问题")
                        .disabled(!can_act)
                        .on_click(move |_, _, cx| {
                            let _ = v_new.update(cx, |wb, cx| {
                                wb.record_manual_issue_from_target(cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::submenu("添加到现有问题", attach_existing_submenu)
                        .disabled(!can_act || !has_existing_findings),
                )
        }
    });

    let req_browser_submenu = PopupMenu::build(window, cx, {
        let v_orig = view.clone();
        let v_curr = view.clone();
        move |sub, _, _| {
            sub.compact()
                .item(
                    PopupMenuItem::new("使用原始会话")
                        .disabled(!can_act)
                        .on_click(move |_, window, cx| {
                            let _ = v_orig.update(cx, |wb, cx| {
                                wb.open_current_request_in_browser_with_session(true, window, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("使用当前会话")
                        .disabled(!can_act)
                        .on_click(move |_, window, cx| {
                            let _ = v_curr.update(cx, |wb, cx| {
                                wb.open_current_request_in_browser_with_session(false, window, cx);
                            });
                        }),
                )
        }
    });

    let engagement_submenu = build_engagement_submenu(
        view.clone(),
        can_act,
        None,
        search_prefill,
        false,
        true,
        window,
        cx,
    );

    let highlight_submenu = PopupMenu::build(window, cx, {
        let hl_view = view.clone();
        let sel_row = selected_row_opt.clone();
        let host_lbl = host_label.clone();
        move |sub, _, _| {
            (0..=9u8).fold(sub.min_w(theme::scaled_px(92.)), |sub, color| {
                let hl_owner = hl_view.clone();
                let sel_row = sel_row.clone();
                let host_label = host_lbl.clone();
                let color_name = [
                    "None", "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Pink", "Magenta",
                    "Gray",
                ][usize::from(color)];
                sub.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .id(("target-highlight-color", usize::from(color)))
                            .test_support()
                            .role(Role::MenuItem)
                            .aria_label(format!("Highlight {color_name}"))
                            .w_full()
                            .h(theme::scaled_px(24.).max(px(24.)))
                            .mx_neg_2()
                            .px(theme::scaled_px(8.))
                            .flex()
                            .items_center()
                            .gap(theme::scaled_px(6.))
                            .when_some(crate::history::highlight_background(color), |this, bg| {
                                this.bg(bg)
                            })
                            .text_color(crate::history::highlight_foreground(color))
                            .child(
                                div()
                                    .w(theme::scaled_px(10.))
                                    .h(theme::scaled_px(10.))
                                    .when(color == 0, |this| {
                                        this.border_1().border_dashed().border_color(rgb(0x888888))
                                    }),
                            )
                            .child(if host_label.is_empty() {
                                color_name.to_owned()
                            } else {
                                host_label.clone()
                            })
                    })
                    .disabled(!annotation_available || project_opt.is_none() || selected_id_opt.is_none())
                    .on_click(move |_, _, cx| {
                        if let Some(project) = project_opt {
                            let _ = hl_owner.update(cx, |wb, cx| {
                                if wb.project == Some(project) {
                                    if selected_count > 1 {
                                        wb.set_target_selected_highlights(color, cx);
                                    } else if let Some(sel_id) = selected_id_opt {
                                        wb.set_target_row_highlight(
                                            sel_id,
                                            sel_row.clone(),
                                            color,
                                            cx,
                                        );
                                    }
                                }
                            });
                        }
                    }),
                )
            })
        }
    });

    let view_submenu = build_view_submenu(view.clone(), view_mode, layout_mode, window, cx);

    let v_scope = view.clone();
    let v_scan_launcher = view.clone();
    let v_passive = view.clone();
    let v_active = view.clone();
    let v_intr = view.clone();
    let v_rep = view.clone();
    let v_seq = view.clone();
    let v_cmp_req = view.clone();
    let v_cmp_resp = view.clone();
    let v_dec = view.clone();
    let v_org = view.clone();
    let v_at = view.clone();
    let v_open_resp = view.clone();
    let v_notes = view.clone();
    let v_delete = view.clone();
    let v_copy_url = view.clone();
    let v_copy_curl = view.clone();
    let v_copy_links = view.clone();
    let v_save = view.clone();
    let v_compare = view.clone();
    let v_new_win = view.clone();
    let v_docs = view;

    let scope_item = if in_scope {
        PopupMenuItem::new("从范围中排除")
            .icon(IconName::CircleMinus)
            .disabled(!can_act)
            .on_click(move |_, _, cx| {
                let _ = v_scope.update(cx, |wb, cx| {
                    wb.modify_selected_target_scope(false, cx);
                });
            })
    } else {
        PopupMenuItem::new("添加到范围")
            .icon(IconName::CirclePlus)
            .disabled(!can_act)
            .on_click(move |_, _, cx| {
                let _ = v_scope.update(cx, |wb, cx| {
                    wb.modify_selected_target_scope(true, cx);
                });
            })
    };

    menu.compact()
        .with_size(gpui_kit::component::Size::Size(px(21.)))
        .scrollable(true)
        .max_h(px(640.))
        .item(
            PopupMenuItem::element(move |_, _| {
                div()
                    .id("target-item-menu-header")
                    .test_support()
                    .role(Role::MenuItem)
                    .aria_label(header_url.clone())
                    .w_full()
                    .min_h(px(21.))
                    .flex()
                    .items_center()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(div().min_w_0().truncate().child(header_url.clone()))
            })
            .icon(IconName::Globe)
            .disabled(true),
        )
        .separator()
        .item(scope_item)
        .separator()
        .item(
            PopupMenuItem::new("扫描")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_scan_launcher.update(cx, |wb, cx| wb.open_scan_launcher(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("被动扫描所选消息")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_passive.update(cx, |wb, cx| wb.send_to_passive_scanner(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("主动扫描所选消息")
                .action(Box::new(SendToScanner))
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_active.update(cx, |wb, cx| wb.send_to_scanner(window, cx));
                }),
        )
        .separator()
        .item(
            shortcut_menu_item("target-item-to-intruder", "发送到 Intruder", "cmd-i", "ctrl-i")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_intr
                        .update(cx, |wb, cx| wb.send_target_history_transfer(true, window, cx));
                }),
        )
        .item(
            shortcut_menu_item("target-item-to-repeater", "发送到 Repeater", "cmd-r", "ctrl-r")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_rep
                        .update(cx, |wb, cx| wb.send_target_history_transfer(false, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Sequencer")
                .action(Box::new(SendToSequencer))
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_seq.update(cx, |wb, cx| wb.send_to_sequencer(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Comparer (请求)")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_cmp_req.update(cx, |wb, cx| wb.send_to_comparer(false, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Comparer (响应)")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_cmp_resp.update(cx, |wb, cx| wb.send_to_comparer(true, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Decoder")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_dec.update(cx, |wb, cx| wb.send_to_decoder(window, cx));
                }),
        )
        .item(
            shortcut_menu_item("target-item-to-organizer", "发送到 Organizer", "cmd-o", "ctrl-o")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_org.update(cx, |wb, cx| wb.send_selected_to_organiser(window, cx));
                }),
        )
        .item(
            shortcut_menu_item("target-item-to-mio-at", "发送到 MioAT", "cmd-g", "ctrl-g")
                .icon(IconName::Zap)
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_at.update(cx, |wb, cx| wb.send_selected_target_to_mio_at(window, cx));
                }),
        )
        .separator()
        .item(PopupMenuItem::submenu("记录问题", record_issue_submenu).disabled(!can_act))
        .separator()
        .item(
            PopupMenuItem::new("在浏览器中打开响应")
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_open_resp
                        .update(cx, |wb, cx| wb.open_current_response_in_browser(window, cx));
                }),
        )
        .item(PopupMenuItem::submenu("在浏览器中请求", req_browser_submenu).disabled(!can_act))
        .separator()
        .item(PopupMenuItem::submenu("参与工具", engagement_submenu).disabled(!can_act))
        .separator()
        .item(
            PopupMenuItem::new("编辑备注")
                .icon(IconName::File)
                .disabled(!can_act || !annotation_available)
                .on_click(move |_, window, cx| {
                    let _ = v_notes.update(cx, |wb, cx| wb.open_target_item_notes(window, cx));
                }),
        )
        .item(
            PopupMenuItem::submenu("高亮", highlight_submenu)
                .disabled(!can_act || !annotation_available),
        )
        .separator()
        .item(
            PopupMenuItem::new("删除项")
                .icon(IconName::Trash)
                .disabled(!has_selection)
                .on_click(move |_, window, cx| {
                    let _ = v_delete.update(cx, |wb, cx| wb.delete_selected_target_item(window, cx));
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制 URL")
                .icon(IconName::Copy)
                .disabled(!can_act)
                .on_click(move |_, _, cx| {
                    let _ = v_copy_url.update(cx, |wb, cx| wb.copy_target_url(cx));
                }),
        )
        .item(
            PopupMenuItem::new("复制为 curl 命令 (bash)")
                .icon(IconName::Copy)
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_copy_curl.update(cx, |wb, cx| wb.copy_curl_command(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("复制响应中的链接")
                .icon(IconName::Copy)
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_copy_links
                        .update(cx, |wb, cx| wb.copy_selected_target_links(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("保存项")
                .icon(IconName::Save)
                .disabled(!can_act)
                .on_click(move |_, window, cx| {
                    let _ = v_save.update(cx, |wb, cx| {
                        wb.save_selected_target_item_to_file(true, window, cx);
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("比较站点地图")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let origin = origin_opt.clone();
                    let _ = v_compare
                        .update(cx, |wb, cx| wb.open_compare_site_maps(origin, window, cx));
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

pub(in crate::workbench) fn target_issue_row_menu(
    view: WeakEntity<Workbench>,
    finding_id: Uuid,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (
        can_operate,
        available,
        title,
        severity_rank,
        current_sev,
        current_conf,
        is_verified,
        view_mode,
        layout_mode,
    ) = {
        let wb = entity.read(cx);
        let finding = wb
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id);
        let (title, rank, sev, conf) = finding.map_or(
            (
                "安全问题".to_owned(),
                0u8,
                IssueSeverity::Information,
                IssueConfidence::Tentative,
            ),
            |f| (f.title.clone(), f.severity_rank, f.severity(), f.confidence),
        );
        (
            wb.audit_can_operate(),
            wb.project.is_some() && wb.backend_available() && wb.transition.is_none(),
            truncate_middle(&title, 52),
            rank,
            sev,
            conf,
            wb.target_pane.verified_findings.contains(&finding_id),
            wb.target_pane.site_map_view_mode,
            wb.target_pane.site_map_layout,
        )
    };

    let severity_submenu = PopupMenu::build(window, cx, {
        let v_sev = view.clone();
        let v_fp = view.clone();
        let v_restore = view.clone();
        move |sub, _, _| {
            let sub = [
                ("高", IssueSeverity::High),
                ("中", IssueSeverity::Medium),
                ("低", IssueSeverity::Low),
                ("信息", IssueSeverity::Information),
            ]
            .into_iter()
            .fold(sub.compact(), |sub, (label, sev)| {
                let owner = v_sev.clone();
                sub.item(
                    PopupMenuItem::new(label)
                        .checked(current_sev == sev)
                        .disabled(!can_operate)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |wb, cx| {
                                wb.set_target_finding_severity(finding_id, sev, cx);
                            });
                        }),
                )
            });
            sub.item(
                PopupMenuItem::new("误报")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        let _ = v_fp.update(cx, |wb, cx| {
                            wb.delete_target_site_map_finding(finding_id, true, window, cx);
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("恢复原始值")
                    .disabled(!can_operate)
                    .on_click(move |_, _, cx| {
                        let _ = v_restore.update(cx, |wb, cx| {
                            wb.restore_target_finding_severity(finding_id, cx);
                        });
                    }),
            )
        }
    });

    let confidence_submenu = PopupMenu::build(window, cx, {
        let v_ver = view.clone();
        let v_cert = view.clone();
        let v_firm = view.clone();
        let v_tent = view.clone();
        let v_restore = view.clone();
        move |sub, _, _| {
            sub.compact()
                .item(
                    PopupMenuItem::new("已验证")
                        .checked(is_verified && current_conf == IssueConfidence::Certain)
                        .disabled(!can_operate)
                        .on_click(move |_, _, cx| {
                            let _ = v_ver.update(cx, |wb, cx| {
                                wb.set_target_finding_confidence(
                                    finding_id,
                                    IssueConfidence::Certain,
                                    true,
                                    cx,
                                );
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("确定")
                        .checked(!is_verified && current_conf == IssueConfidence::Certain)
                        .disabled(!can_operate)
                        .on_click(move |_, _, cx| {
                            let _ = v_cert.update(cx, |wb, cx| {
                                wb.set_target_finding_confidence(
                                    finding_id,
                                    IssueConfidence::Certain,
                                    false,
                                    cx,
                                );
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("明确")
                        .checked(current_conf == IssueConfidence::Firm)
                        .disabled(!can_operate)
                        .on_click(move |_, _, cx| {
                            let _ = v_firm.update(cx, |wb, cx| {
                                wb.set_target_finding_confidence(
                                    finding_id,
                                    IssueConfidence::Firm,
                                    false,
                                    cx,
                                );
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("推测")
                        .checked(current_conf == IssueConfidence::Tentative)
                        .disabled(!can_operate)
                        .on_click(move |_, _, cx| {
                            let _ = v_tent.update(cx, |wb, cx| {
                                wb.set_target_finding_confidence(
                                    finding_id,
                                    IssueConfidence::Tentative,
                                    false,
                                    cx,
                                );
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("恢复原始值")
                        .disabled(!can_operate)
                        .on_click(move |_, _, cx| {
                            let _ = v_restore.update(cx, |wb, cx| {
                                wb.restore_target_finding_confidence(finding_id, cx);
                            });
                        }),
                )
        }
    });

    let view_submenu = build_view_submenu(view.clone(), view_mode, layout_mode, window, cx);

    let v_report = view.clone();
    let v_delete = view.clone();
    let v_comparer = view.clone();
    let v_at = view.clone();
    let v_new_win = view.clone();
    let v_docs = view;

    menu.compact()
        .with_size(gpui_kit::component::Size::Size(px(21.)))
        .scrollable(true)
        .max_h(px(640.))
        .item(
            PopupMenuItem::element(move |_, _| {
                div()
                    .id("target-issue-menu-header")
                    .test_support()
                    .role(Role::MenuItem)
                    .aria_label(title.clone())
                    .w_full()
                    .min_h(px(21.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(
                        div()
                            .size(px(10.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(super::site_map_ext::finding_severity_dot(severity_rank)),
                    )
                    .child(div().min_w_0().truncate().child(title.clone()))
            })
            .disabled(true),
        )
        .separator()
        .item(
            PopupMenuItem::new("报告问题")
                .disabled(!can_operate)
                .on_click(move |_, window, cx| {
                    let _ = v_report.update(cx, |wb, cx| {
                        wb.save_audit_report_to_file(true, Some(finding_id), window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("删除问题")
                .disabled(!can_operate)
                .on_click(move |_, window, cx| {
                    let _ = v_delete.update(cx, |wb, cx| {
                        wb.delete_target_site_map_finding(finding_id, false, window, cx);
                    });
                }),
        )
        .separator()
        .item(PopupMenuItem::submenu("设置严重性", severity_submenu).disabled(!can_operate))
        .item(PopupMenuItem::submenu("设置置信度", confidence_submenu).disabled(!can_operate))
        .separator()
        .item(
            PopupMenuItem::new("发送到 Comparer")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = v_comparer.update(cx, |wb, cx| {
                        wb.send_target_issue_to_comparer(finding_id, window, cx);
                    });
                }),
        )
        .item(
            shortcut_menu_item("target-issue-to-mio-at", "发送到 MioAT", "cmd-g", "ctrl-g")
                .icon(IconName::Zap)
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = v_at.update(cx, |wb, cx| {
                        wb.send_target_issue_to_mio_at(finding_id, cx);
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
            PopupMenuItem::new("漏洞文档").on_click(move |_, _, cx| {
                let doc_url =
                    "https://github.com/ltlly/MIO";
                cx.open_url(doc_url);
                let _ = v_docs.update(cx, |wb, cx| {
                    wb.status = format!("已打开站点地图漏洞文档：{doc_url}");
                    cx.notify();
                });
            }),
        )
}
