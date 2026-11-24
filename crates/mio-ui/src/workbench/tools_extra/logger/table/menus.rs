use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{FontWeight, WeakEntity};
use mio_core::FlowId;

use super::*;

const MIO_ADD_TO_SITEMAP_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><circle cx="3.2" cy="11.2" r="1.3"/><path d="M3.2 9.9V3.2h5.1"/><path d="M3.2 7.2h5.1"/><rect x="8.3" y="1.9" width="3.8" height="2.6" rx="0.6"/><rect x="8.3" y="5.9" width="3.8" height="2.6" rx="0.6"/></svg>"#;

fn strip_default_port(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_owned();
    };
    let default_port = if scheme.eq_ignore_ascii_case("http") {
        ":80"
    } else if scheme.eq_ignore_ascii_case("https") {
        ":443"
    } else {
        return trimmed.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if let Some(stripped_authority) = authority.strip_suffix(default_port) {
        format!("{scheme}://{stripped_authority}{tail}")
    } else {
        trimmed.to_owned()
    }
}

fn truncate_menu_url(url: &str) -> String {
    let normalized = strip_default_port(url);
    let chars: Vec<char> = normalized.chars().collect();
    if chars.is_empty() {
        return "Logger item".into();
    }
    if chars.len() <= 58 {
        normalized
    } else {
        let prefix: String = chars[..27].iter().collect();
        let suffix: String = chars[chars.len() - 28..].iter().collect();
        format!("{prefix}...{suffix}")
    }
}

fn menu_shortcut_row(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    shortcut: &'static str,
) -> impl gpui_kit::IntoElement {
    let key = gpui_kit::Keystroke::parse(shortcut).ok().map(|ks| {
        gpui_kit::component::kbd::Kbd::new(ks)
            .p_0()
            .flex_nowrap()
            .border_0()
            .bg(gpui_kit::transparent_black())
    });
    div()
        .id(id)
        .test_support()
        .role(gpui_kit::accesskit::Role::MenuItem)
        .aria_label(label)
        .w_full()
        .min_h(gpui_kit::px(21.))
        .flex()
        .items_center()
        .justify_between()
        .gap(gpui_kit::px(12.))
        .child(label)
        .children(key)
}

fn high_contrast_logger_highlight_fg(color: u8) -> gpui_kit::Hsla {
    gpui_kit::rgb(match color {
        1 | 6 | 8 => 0xffffff,
        _ => 0x111111,
    })
    .into()
}


pub(in crate::workbench::tools_extra::logger) fn build_logger_header_context_menu(
    view: WeakEntity<Workbench>,
    col_ix: usize,
    rows_rc: Rc<Vec<(usize, FlowId)>>,
    col_visible: [bool; LOGGER_COL_COUNT],
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let visible_count = col_visible.iter().filter(|&&v| v).count();
    let can_hide = col_ix != 0 && col_ix < LOGGER_COL_COUNT && visible_count > 1;
    let has_rows = !rows_rc.is_empty();

    let v_asc = view.clone();
    let v_desc = view.clone();
    let v_copy = view.clone();
    let v_hide = view.clone();
    let v_fit = view.clone();
    let v_widen = view.clone();
    let v_narrow = view.clone();
    let v_cols = view.clone();
    let v_reset = view;

    let copy_rows = Rc::clone(&rows_rc);
    let fit_rows = rows_rc;

    let cols_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0..LOGGER_COL_COUNT).fold(sub.compact().min_w(px(180.)), |sub, idx| {
            let owner = v_cols.clone();
            let is_vis = col_visible[idx];
            let disabled = idx == 0 || (is_vis && visible_count <= 1);
            sub.item(
                PopupMenuItem::new(LOGGER_COL_NAMES[idx])
                    .checked(is_vis)
                    .disabled(disabled)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |wb, cx| {
                            wb.toggle_logger_column_visibility(idx, cx);
                        });
                    }),
            )
        })
    });

    menu.compact()
        .item(
            PopupMenuItem::new("按此列升序排序 (Sort ascending)").on_click(move |_, _, cx| {
                let _ = v_asc.update(cx, |wb, cx| wb.set_logger_col_sort(col_ix, true, cx));
            }),
        )
        .item(
            PopupMenuItem::new("按此列降序排序 (Sort descending)").on_click(move |_, _, cx| {
                let _ = v_desc.update(cx, |wb, cx| wb.set_logger_col_sort(col_ix, false, cx));
            }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制整列内容 (Copy column)")
                .disabled(!has_rows)
                .on_click(move |_, _, cx| {
                    let _ = v_copy.update(cx, |wb, cx| {
                        wb.copy_logger_column(col_ix, &copy_rows, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("隐藏此列 (Hide column)")
                .disabled(!can_hide)
                .on_click(move |_, _, cx| {
                    let _ = v_hide.update(cx, |wb, cx| {
                        wb.toggle_logger_column_visibility(col_ix, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("自适应列宽 (Auto-fit column width)").on_click(move |_, _, cx| {
                let _ = v_fit.update(cx, |wb, cx| {
                    wb.fit_logger_column_width(col_ix, &fit_rows, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new("加宽列 (+24px)").on_click(move |_, _, cx| {
                let _ = v_widen.update(cx, |wb, cx| {
                    wb.adjust_logger_column_width(col_ix, 24.0, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new("缩窄列 (-24px)").on_click(move |_, _, cx| {
                let _ = v_narrow.update(cx, |wb, cx| {
                    wb.adjust_logger_column_width(col_ix, -24.0, cx);
                });
            }),
        )
        .separator()
        .item(PopupMenuItem::submenu("显示/隐藏列 (Columns)", cols_submenu))
        .item(
            PopupMenuItem::new("重置列布局 (Reset column layout)").on_click(move |_, _, cx| {
                let _ = v_reset.update(cx, |wb, cx| wb.reset_logger_column_layout(cx));
            }),
        )
}

pub(in crate::workbench::tools_extra::logger) fn logger_row_menu(
    view: WeakEntity<Workbench>,
    flow_id: FlowId,
    row_url: String,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (available, can_export_har, existing_findings, current_highlight, in_scope, target_ids) = {
        let workbench = entity.read(cx);
        let available = workbench.project.is_some()
            && workbench.backend_available()
            && workbench.transition.is_none();
        let can_export_har = available
            && workbench.extra_tools.logger_selected.is_some_and(|sel_id| {
                workbench
                    .extra_tools
                    .logger_loaded_detail
                    .as_deref()
                    .is_some_and(|(record, _)| record.summary.id == sel_id)
            });
        let findings = workbench.existing_audit_finding_ids();
        let hl = workbench
            .history
            .read(cx)
            .delegate()
            .live_highlights
            .borrow()
            .get(&flow_id)
            .map(|h| h.color)
            .filter(|c| *c <= 9)
            .unwrap_or(0);
        let in_scope = workbench.workspace.scope.has_enabled_includes()
            && workbench.workspace.scope.is_url_in_scope(&row_url);
        let target_ids = workbench.logger_selected_target_ids(flow_id);
        (available, can_export_har, findings, hl, in_scope, target_ids)
    };

    let multi = target_ids.len() > 1;
    let header_url = if multi {
        format!("已选中 {} 条日志记录", target_ids.len())
    } else {
        truncate_menu_url(&row_url)
    };
    let mod_key = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    let attach_owner = view.clone();
    let existing_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        if existing_findings.is_empty() {
            sub.item(PopupMenuItem::new("暂无现有问题").disabled(true))
        } else {
            existing_findings
                .into_iter()
                .fold(sub, |sub, (fid, title)| {
                    let owner = attach_owner.clone();
                    sub.item(
                        PopupMenuItem::new(title)
                            .disabled(!available)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |v, cx| {
                                    v.attach_logger_flow_to_issue(fid, cx);
                                });
                            }),
                    )
                })
        }
    });

    let issue_owner = view.clone();
    let record_issue_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_new = issue_owner.clone();
        sub.item(
            PopupMenuItem::new("创建新问题")
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = o_new.update(cx, |v, cx| v.record_manual_issue_from_logger(cx));
                }),
        )
        .item(PopupMenuItem::submenu("添加到现有问题", existing_submenu))
    });

    let req_browser_owner = view.clone();
    let req_browser_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_orig = req_browser_owner.clone();
        let o_curr = req_browser_owner.clone();
        sub.item(
            PopupMenuItem::new("使用原始会话")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_orig.update(cx, |v, cx| {
                        v.open_current_request_in_browser_with_session(true, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("使用当前会话")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_curr.update(cx, |v, cx| {
                        v.open_current_request_in_browser_with_session(false, window, cx);
                    });
                }),
        )
    });

    let engage_owner = view.clone();
    let engagement_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_refs = engage_owner.clone();
        let o_csrf = engage_owner.clone();
        let o_har = engage_owner.clone();
        sub.item(
            PopupMenuItem::new("查找引用")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_refs.update(cx, |v, cx| v.find_logger_references(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("生成 CSRF PoC")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_csrf.update(cx, |v, cx| v.generate_logger_csrf_poc(window, cx));
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制为 HAR (JSON)")
                .disabled(!can_export_har)
                .on_click(move |_, window, cx| {
                    let _ = o_har.update(cx, |v, cx| v.copy_selected_flow_har(window, cx));
                }),
        )
    });

    let hl_owner = view.clone();
    let hl_ids = target_ids.clone();
    let highlight_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0u8..=9).fold(sub.min_w(theme::scaled_px(116.)), |sub, color| {
            let owner = hl_owner.clone();
            let ids = hl_ids.clone();
            let name = LOGGER_HIGHLIGHT_NAMES[usize::from(color)];
            let checked = current_highlight == color;
            sub.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .id(("logger-highlight-color", usize::from(color)))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label(format!("高亮 {name}"))
                        .w_full()
                        .min_h(theme::scaled_px(24.))
                        .mx_neg_2()
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when_some(
                            crate::history::highlight_background(color),
                            |this, bg| this.bg(bg),
                        )
                        .text_color(if color == 0 {
                            theme::foreground()
                        } else {
                            high_contrast_logger_highlight_fg(color)
                        })
                        .child(
                            div()
                                .w(px(10.))
                                .h(px(10.))
                                .when(color == 0, |this| {
                                    this.border_1()
                                        .border_dashed()
                                        .border_color(rgb(0x888888))
                                })
                                .when_some(
                                    crate::history::highlight_background(color),
                                    |this, swatch| {
                                        this.bg(swatch)
                                            .border_1()
                                            .border_color(rgb(0x444444))
                                    },
                                ),
                        )
                        .child(if checked {
                            format!("{name} ✓")
                        } else {
                            name.to_owned()
                        })
                })
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |v, cx| {
                        v.set_logger_batch_highlight(&ids, color, cx);
                    });
                }),
            )
        })
    });

    let add_item =
        |menu: PopupMenu,
         label: &'static str,
         icon: Option<IconName>,
         action: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
            let owner = view.clone();
            let mut item = PopupMenuItem::new(label)
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| action(view, window, cx));
                });
            if let Some(ic) = icon {
                item = item.icon(ic);
            }
            menu.item(item)
        };

    let o_intruder = view.clone();
    let o_repeater = view.clone();
    let o_cmp_req = view.clone();
    let o_cmp_resp = view.clone();
    let o_org = view.clone();
    let o_ai = view.clone();
    let o_del = view.clone();
    let o_sel_csv = view.clone();
    let o_urls = view.clone();

    let ids_intruder = target_ids.clone();
    let ids_repeater = target_ids.clone();
    let ids_cmp_req = target_ids.clone();
    let ids_cmp_resp = target_ids.clone();
    let ids_org = target_ids.clone();
    let ids_del = target_ids.clone();
    let ids_sel_csv = target_ids.clone();
    let ids_urls = target_ids;

    let menu = menu
        .compact()
        .scrollable(true)
        .max_h(theme::scaled_px(620.))
        .item(
            PopupMenuItem::element(move |_, _| {
                div()
                    .id("logger-row-url-header")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::MenuItem)
                    .aria_label(header_url.clone())
                    .w_full()
                    .min_h(gpui_kit::px(21.))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(header_url.clone())
            })
            .disabled(true),
        )
        .separator();
    let menu = if in_scope {
        add_item(
            menu,
            "从范围排除",
            Some(IconName::CircleMinus),
            Workbench::exclude_from_scope,
        )
    } else {
        add_item(
            menu,
            "添加到范围",
            Some(IconName::CirclePlus),
            Workbench::add_to_scope,
        )
    }
    .separator();
    let menu = add_item(menu, "扫描 (Scan)", None, Workbench::open_scan_launcher);
    let menu = add_item(
        menu,
        "被动扫描所选消息",
        None,
        Workbench::send_to_passive_scanner,
    );
    let menu = add_item(menu, "主动扫描所选消息", None, Workbench::send_to_scanner)
        .separator()
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-intruder",
                    "发送到 Intruder",
                    if mod_key == "cmd" { "cmd-i" } else { "ctrl-i" },
                )
            })
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_intruder.update(cx, |v, cx| {
                    v.send_logger_batch_to_intruder(&ids_intruder, window, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-repeater",
                    "发送到 Repeater",
                    if mod_key == "cmd" { "cmd-r" } else { "ctrl-r" },
                )
            })
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_repeater.update(cx, |v, cx| {
                    v.send_logger_batch_to_repeater(&ids_repeater, window, cx);
                });
            }),
        );
    let menu = add_item(menu, "发送到 Sequencer", None, Workbench::send_to_sequencer)
        .item(
            PopupMenuItem::new("发送到 Comparer (请求)")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_cmp_req.update(cx, |v, cx| {
                        v.send_logger_batch_to_comparer(&ids_cmp_req, false, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Comparer (响应)")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_cmp_resp.update(cx, |v, cx| {
                        v.send_logger_batch_to_comparer(&ids_cmp_resp, true, window, cx);
                    });
                }),
        );
    let menu = add_item(menu, "发送到 Decoder", None, Workbench::send_to_decoder)
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-organizer",
                    "发送到 Organizer",
                    if mod_key == "cmd" { "cmd-o" } else { "ctrl-o" },
                )
            })
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_org.update(cx, |v, cx| {
                    v.send_logger_batch_to_organiser(&ids_org, window, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-mio-at",
                    "发送到 MioAT",
                    if mod_key == "cmd" { "cmd-g" } else { "ctrl-g" },
                )
            })
            .icon(IconName::Zap)
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_ai.update(cx, |v, cx| v.send_logger_selected_to_mio_at(window, cx));
            }),
        )
        .separator()
        .item(PopupMenuItem::submenu("记录问题", record_issue_submenu))
        .separator();
    let menu = add_item(
        menu,
        "在浏览器中打开响应",
        None,
        Workbench::open_current_response_in_browser,
    )
    .item(PopupMenuItem::submenu("在浏览器中请求", req_browser_submenu))
    .separator()
    .item(PopupMenuItem::submenu("参与工具", engagement_submenu))
    .separator();
    let menu = add_item(
        menu,
        "编辑备注",
        Some(IconName::File),
        Workbench::open_http_notes,
    )
    .item(PopupMenuItem::submenu("高亮", highlight_submenu))
    .separator()
    .item(
        PopupMenuItem::new("删除选中记录 (Delete selected)")
            .icon(IconName::Trash)
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_del.update(cx, |v, cx| {
                    v.delete_logger_selected_entries(&ids_del, window, cx);
                });
            }),
    );
    let menu = add_item(
        menu,
        "清空日志 (Clear log)",
        None,
        Workbench::clear_logger_entries,
    )
    .separator();
    let menu = if multi {
        menu.item(
            PopupMenuItem::new("导出选中项为 CSV")
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = o_sel_csv.update(cx, |v, cx| {
                        v.export_logger_selected_csv(&ids_sel_csv, cx);
                    });
                }),
        )
    } else {
        menu
    };
    let menu = add_item(
        menu,
        "导出为 CSV (Export as CSV)",
        None,
        Workbench::export_logger_csv,
    )
    .item(
        PopupMenuItem::new("复制 URL")
            .icon(IconName::Copy)
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_urls.update(cx, |v, cx| {
                    v.copy_logger_batch_urls(&ids_urls, window, cx);
                });
            }),
    );
    let menu = add_item(
        menu,
        "复制为 curl 命令 (bash)",
        Some(IconName::Copy),
        Workbench::copy_curl_command,
    );
    let menu = add_item(
        menu,
        "复制响应中的链接",
        Some(IconName::Copy),
        Workbench::copy_links_in_response,
    );
    let menu = add_item(
        menu,
        "保存条目 (Save item)",
        Some(IconName::Save),
        Workbench::save_selected_logger_item_to_file,
    )
    .separator();
    let sitemap_owner = view.clone();
    menu.item(
        PopupMenuItem::new("添加到站点地图 (Add to site map)")
            .icon(gpui_kit::component::Icon::default().data(MIO_ADD_TO_SITEMAP_SVG))
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = sitemap_owner.update(cx, |v, cx| v.add_to_site_map(window, cx));
            }),
    )
}
