use gpui_kit::accesskit::Role;
use gpui_kit::component::kbd::Kbd;

use super::*;

pub(super) fn truncate_middle(text: &str, max_chars: usize) -> String {
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

pub(super) fn strip_default_port(url: &str) -> String {
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

pub(super) fn shortcut_kbd(mac_key: &'static str, other_key: &'static str) -> Option<Kbd> {
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

pub(super) fn shortcut_menu_item(
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

pub(super) fn build_view_submenu(
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
                            wb.target_view.update(cx, |tv, cx| {
                                tv.set_view_mode(SiteMapViewMode::UrlView, cx);
                            });
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Crawl paths view")
                    .checked(view_mode == SiteMapViewMode::CrawlPathsView)
                    .on_click(move |_, _, cx| {
                        let _ = v_crawl.update(cx, |wb, cx| {
                            wb.target_view.update(cx, |tv, cx| {
                                tv.set_view_mode(SiteMapViewMode::CrawlPathsView, cx);
                            });
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("经典上下分栏")
                    .checked(layout_mode == SiteMapLayoutMode::TreeLeftStackedRight)
                    .on_click(move |_, _, cx| {
                        let _ = v_stacked.update(cx, |wb, cx| {
                            wb.target_view.update(cx, |tv, cx| {
                                tv.set_layout_mode(SiteMapLayoutMode::TreeLeftStackedRight, cx);
                            });
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("三栏并排布局")
                    .checked(layout_mode == SiteMapLayoutMode::ThreeColumn)
                    .on_click(move |_, _, cx| {
                        let _ = v_cols.update(cx, |wb, cx| {
                            wb.target_view.update(cx, |tv, cx| {
                                tv.set_layout_mode(SiteMapLayoutMode::ThreeColumn, cx);
                            });
                        });
                    }),
            )
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_engagement_submenu(
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
