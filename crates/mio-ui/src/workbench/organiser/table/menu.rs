use gpui_kit::FontWeight;
use gpui_kit::assets::IconName;

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

fn truncate_organiser_menu_url(url: &str) -> String {
    let normalized = strip_default_port(url);
    let chars: Vec<char> = normalized.chars().collect();
    if chars.is_empty() {
        return "Organizer item".into();
    }
    if chars.len() <= 58 {
        normalized
    } else {
        let prefix: String = chars[..27].iter().collect();
        let suffix: String = chars[chars.len() - 28..].iter().collect();
        format!("{prefix}...{suffix}")
    }
}

fn organiser_shortcut_row(
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

fn move_organiser_items_to_collection(
    view: &mut Workbench,
    source_collection: Option<mio_core::Uuid>,
    target_collection: mio_core::Uuid,
    item_ids: Vec<mio_core::Uuid>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if !view.organiser_editable() {
        return;
    }
    let existing: std::collections::HashSet<mio_core::Uuid> = view
        .workspace
        .organiser_items
        .iter()
        .map(|item| item.id)
        .collect();
    let Some(target_name) = view
        .workspace
        .organiser_collections
        .iter()
        .find(|c| c.id == target_collection)
        .map(|c| c.name.clone())
    else {
        return;
    };
    let mut moved_ids: std::collections::HashSet<mio_core::Uuid> = std::collections::HashSet::new();
    let mut changed = false;
    if let Some(target) = view
        .workspace
        .organiser_collections
        .iter_mut()
        .find(|c| c.id == target_collection)
    {
        for id in &item_ids {
            if !existing.contains(id) {
                continue;
            }
            if target.item_ids.contains(id) {
                moved_ids.insert(*id);
                continue;
            }
            if target.item_ids.len() >= mio_core::MAX_ORGANISER_ITEMS {
                break;
            }
            target.item_ids.push(*id);
            moved_ids.insert(*id);
            changed = true;
        }
    }
    if let Some(source_id) = source_collection.filter(|src| *src != target_collection)
        && let Some(source) = view
            .workspace
            .organiser_collections
            .iter_mut()
            .find(|c| c.id == source_id)
    {
        let before = source.item_ids.len();
        source.item_ids.retain(|id| !moved_ids.contains(id));
        if source.item_ids.len() != before {
            changed = true;
        }
    }
    let moved_count = moved_ids.len();
    view.error = None;
    view.status = if changed {
        format!("已将 {moved_count} 条条目移动到集合「{target_name}」。")
    } else {
        format!("所选条目已在集合「{target_name}」中。")
    };
    view.sync_organiser_table(window, cx);
    if changed {
        view.save_workspace(cx);
    }
}

fn duplicate_selected_organiser_item(
    view: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if !view.organiser_editable() {
        return;
    }
    if let Err(err) = view.validate_organiser_composition(window, cx) {
        view.fail(err, cx);
        return;
    }
    if !view.flush_workspace(window, cx) {
        return;
    }
    let Some(source) = view.selected_organiser_item_cloned() else {
        view.fail("请先选中一条 Organiser 条目。", cx);
        return;
    };
    let number = match next_organiser_number(&view.workspace.organiser_items) {
        Ok(number) => number,
        Err(err) => {
            view.fail(err.to_string(), cx);
            return;
        }
    };
    let target_or_url = if !source.target.is_empty() {
        source.target.as_str()
    } else {
        source.url.as_str()
    };
    let mut copy = match OrganiserItem::from_raw_parts(
        number,
        "Organizer",
        target_or_url,
        source.request_raw,
        source.response_raw,
        source.status_code,
        source.http_version,
    ) {
        Ok(item) => item,
        Err(err) => {
            view.fail(err.to_string(), cx);
            return;
        }
    };
    copy.notes = source.notes;
    copy.highlight = source.highlight;
    let added_number = copy.number;
    view.workspace.organiser_items.push(copy);
    if let Err(err) = validate_organiser_items(&view.workspace.organiser_items)
        .and_then(|()| view.workspace.validate_structure())
    {
        view.workspace.organiser_items.pop();
        view.fail(format!("未添加到 Organiser：{err}"), cx);
        return;
    }
    view.sync_organiser_table(window, cx);
    view.save_workspace(cx);
    view.error = None;
    view.status = format!("已发送到 Organizer (#{added_number})。");
    cx.notify();
}

pub(super) fn build_organiser_context_menu(
    entry: OrganiserRowSummary,
    multi_selected: bool,
    owner: WeakEntity<Workbench>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<TableState<OrganiserTableDelegate>>,
) -> PopupMenu {
    let (editable, in_scope, existing_findings, collections, active_collection) = owner
        .upgrade()
        .map(|entity| {
            let view = entity.read(cx);
            let cols: Vec<(mio_core::Uuid, String)> = view
                .organiser
                .custom_collections
                .iter()
                .map(|c| (c.id, c.name.clone()))
                .collect();
            let in_scope = view.workspace.scope.has_enabled_includes()
                && view.workspace.scope.is_url_in_scope(&entry.url);
            (
                view.organiser_editable(),
                in_scope,
                view.existing_audit_finding_ids(),
                cols,
                view.organiser.active_collection,
            )
        })
        .unwrap_or((false, false, Vec::new(), Vec::new(), None));
    let id = entry.id;
    let current_status = entry.workflow_status;
    let current_highlight = entry.highlight.unwrap_or(0);
    let has_response = entry.has_response;
    let header_url = truncate_organiser_menu_url(&entry.url);
    let mod_key = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    let move_owner = owner.clone();
    let move_collections = collections.clone();
    let move_to_submenu = PopupMenu::build(window, cx, move |submenu, _, _| {
        let submenu = if let Some(active_cid) = active_collection {
            let inbox_owner = move_owner.clone();
            let submenu = submenu.item(
                PopupMenuItem::new("收件箱")
                    .disabled(!editable)
                    .on_click(move |_, window, cx| {
                        let _ = inbox_owner.update(cx, |view, cx| {
                            let ids = if multi_selected {
                                view.organiser.selected_ids.iter().copied().collect()
                            } else {
                                vec![id]
                            };
                            view.remove_organiser_items_from_collection(
                                active_cid, ids, window, cx,
                            );
                        });
                    }),
            );
            move_collections
                .into_iter()
                .filter(|(cid, _)| *cid != active_cid)
                .fold(submenu, |submenu, (cid, cname)| {
                    let col_owner = move_owner.clone();
                    submenu.item(
                        PopupMenuItem::new(cname)
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = col_owner.update(cx, |view, cx| {
                                    let ids = if multi_selected {
                                        view.organiser.selected_ids.iter().copied().collect()
                                    } else {
                                        vec![id]
                                    };
                                    move_organiser_items_to_collection(
                                        view,
                                        Some(active_cid),
                                        cid,
                                        ids,
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                })
        } else if move_collections.is_empty() {
            submenu.item(PopupMenuItem::new("暂无自定义集合").disabled(true))
        } else {
            move_collections
                .into_iter()
                .fold(submenu, |submenu, (cid, cname)| {
                    let col_owner = move_owner.clone();
                    submenu.item(
                        PopupMenuItem::new(cname)
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = col_owner.update(cx, |view, cx| {
                                    let ids = if multi_selected {
                                        view.organiser.selected_ids.iter().copied().collect()
                                    } else {
                                        vec![id]
                                    };
                                    move_organiser_items_to_collection(
                                        view, None, cid, ids, window, cx,
                                    );
                                });
                            }),
                    )
                })
        };
        let o_new_col = move_owner.clone();
        submenu.separator().item(
            PopupMenuItem::new("新建集合…")
                .icon(IconName::Plus)
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_new_col.update(cx, |view, cx| {
                        view.toggle_create_organiser_collection(window, cx);
                    });
                }),
        )
    });

    let copy_owner = owner.clone();
    let copy_to_submenu = PopupMenu::build(window, cx, move |submenu, _, _| {
        let submenu = if collections.is_empty() {
            submenu.item(PopupMenuItem::new("暂无自定义集合").disabled(true))
        } else {
            collections
                .into_iter()
                .fold(submenu, |submenu, (cid, cname)| {
                    let col_owner = copy_owner.clone();
                    submenu.item(
                        PopupMenuItem::new(cname)
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = col_owner.update(cx, |view, cx| {
                                    let ids = if multi_selected {
                                        view.organiser.selected_ids.iter().copied().collect()
                                    } else {
                                        vec![id]
                                    };
                                    view.assign_organiser_items_to_collection(
                                        cid, ids, window, cx,
                                    );
                                });
                            }),
                    )
                })
        };
        let o_new_col = copy_owner.clone();
        submenu.separator().item(
            PopupMenuItem::new("新建集合…")
                .icon(IconName::Plus)
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_new_col.update(cx, |view, cx| {
                        view.toggle_create_organiser_collection(window, cx);
                    });
                }),
        )
    });

    let status_owner = owner.clone();
    let status_submenu = PopupMenu::build(window, cx, move |submenu, _, _| {
        OrganiserStatus::ALL
            .into_iter()
            .fold(submenu, |submenu, st| {
                let status_owner = status_owner.clone();
                submenu.item(
                    PopupMenuItem::new(st.label())
                        .disabled(!editable)
                        .checked(!multi_selected && current_status == st)
                        .on_click(move |_, window, cx| {
                            let _ = status_owner.update(cx, |view, cx| {
                                if multi_selected {
                                    view.set_batch_organiser_status(st, window, cx);
                                } else {
                                    view.set_organiser_item_status(id, st, window, cx);
                                }
                            });
                        }),
                )
            })
    });

    let hl_owner = owner.clone();
    let hl_submenu = PopupMenu::build(window, cx, move |submenu, _, _| {
        (0u8..=9).fold(submenu.min_w(theme::scaled_px(116.)), |submenu, color| {
            let hl_owner = hl_owner.clone();
            let target = (color > 0).then_some(color);
            let name = HIGHLIGHT_NAMES[usize::from(color)];
            let is_checked = !multi_selected && current_highlight == color;
            submenu.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .id(("organiser-highlight-color", usize::from(color)))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label(format!("Highlight {name}"))
                        .w_full()
                        .min_h(theme::scaled_px(24.))
                        .mx_neg_2()
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when_some(
                            crate::history::highlight_background(color),
                            |this, background| this.bg(background),
                        )
                        .text_color(if color == 0 {
                            theme::foreground()
                        } else {
                            super::high_contrast_highlight_fg(color)
                        })
                        .child(
                            div()
                                .w(px(10.))
                                .h(px(10.))
                                .when(color == 0, |this| {
                                    this.border_1().border_dashed().border_color(rgb(0x888888))
                                })
                                .when_some(
                                    crate::history::highlight_background(color),
                                    |this, swatch| {
                                        this.bg(swatch).border_1().border_color(rgb(0x444444))
                                    },
                                ),
                        )
                        .child(if is_checked {
                            format!("{name} ✓")
                        } else {
                            name.to_owned()
                        })
                })
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = hl_owner.update(cx, |view, cx| {
                        if multi_selected {
                            view.set_batch_organiser_highlight(target, window, cx);
                        } else {
                            view.set_organiser_item_highlight(id, target, window, cx);
                        }
                    });
                }),
            )
        })
    });

    let attach_owner = owner.clone();
    let existing_issues_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        if existing_findings.is_empty() {
            sub.item(PopupMenuItem::new("暂无现有问题").disabled(true))
        } else {
            existing_findings
                .into_iter()
                .fold(sub, |sub, (fid, title)| {
                    let o = attach_owner.clone();
                    sub.item(
                        PopupMenuItem::new(title)
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = o.update(cx, |view, cx| {
                                    if view.select_organiser_item(id, window, cx) {
                                        view.attach_organiser_item_to_issue(fid, cx);
                                    }
                                });
                            }),
                    )
                })
        }
    });

    let issue_owner = owner.clone();
    let record_issue_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_new = issue_owner.clone();
        sub.item(
            PopupMenuItem::new("创建新问题")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_new.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            view.record_manual_issue_from_organiser(cx);
                        }
                    });
                }),
        )
        .item(PopupMenuItem::submenu(
            "添加到现有问题",
            existing_issues_submenu,
        ))
    });

    let req_browser_owner = owner.clone();
    let req_browser_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_orig = req_browser_owner.clone();
        let o_curr = req_browser_owner.clone();
        sub.item(
            PopupMenuItem::new("使用原始会话")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_orig.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            view.open_current_request_in_browser_with_session(true, window, cx);
                        }
                    });
                }),
        )
        .item(
            PopupMenuItem::new("使用当前会话")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_curr.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            view.open_current_request_in_browser_with_session(false, window, cx);
                        }
                    });
                }),
        )
    });

    let engage_owner = owner.clone();
    let engagement_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_refs = engage_owner.clone();
        let o_discover = engage_owner.clone();
        let o_schedule = engage_owner.clone();
        let o_csrf = engage_owner.clone();
        sub.item(
            PopupMenuItem::new("查找引用")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_refs.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            view.engagement_find_references(window, cx);
                        }
                    });
                }),
        )
        .item(
            PopupMenuItem::new("发现内容")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_discover.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            view.engagement_discover_content(window, cx);
                        }
                    });
                }),
        )
        .item(
            PopupMenuItem::new("计划任务")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_schedule.update(cx, |view, cx| {
                        view.engagement_schedule_task(window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("生成 CSRF PoC")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = o_csrf.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            view.generate_organiser_csrf_poc(cx);
                        }
                    });
                }),
        )
    });

    let add_item =
        |menu: PopupMenu,
         label: &'static str,
         icon: Option<IconName>,
         enabled: bool,
         action: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
            let owner = owner.clone();
            let mut item = PopupMenuItem::new(label)
                .disabled(!enabled)
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        if view.select_organiser_item(id, window, cx) {
                            action(view, window, cx);
                        }
                    });
                });
            if let Some(ic) = icon {
                item = item.icon(ic);
            }
            menu.item(item)
        };

    let o_intruder = owner.clone();
    let o_repeater = owner.clone();
    let o_org = owner.clone();
    let o_ai = owner.clone();
    let delete_owner = owner.clone();
    let csv_owner = owner.clone();
    let sitemap_owner = owner.clone();

    let menu = menu
        .compact()
        .with_size(gpui_kit::component::Size::Size(gpui_kit::px(21.)))
        .scrollable(true)
        .max_h(theme::scaled_px(760.))
        .item(
            PopupMenuItem::element(move |_, _| {
                div()
                    .id("organiser-row-url-header")
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
        .separator()
        .item(PopupMenuItem::submenu("移动到…", move_to_submenu))
        .item(PopupMenuItem::submenu("复制到…", copy_to_submenu))
        .separator();
    let menu = if in_scope {
        add_item(
            menu,
            "从范围中排除",
            Some(IconName::CircleMinus),
            editable,
            Workbench::exclude_from_scope,
        )
    } else {
        add_item(
            menu,
            "添加到范围",
            Some(IconName::CirclePlus),
            editable,
            Workbench::add_to_scope,
        )
    }
    .separator();
    let menu = add_item(menu, "扫描", None, editable, Workbench::open_scan_launcher);
    let menu = add_item(
        menu,
        "被动扫描所选消息",
        None,
        editable,
        Workbench::send_to_passive_scanner,
    );
    let menu = add_item(
        menu,
        "主动扫描所选消息",
        None,
        editable,
        Workbench::send_to_scanner,
    )
    .separator()
    .item(
        PopupMenuItem::element(move |_, _| {
            organiser_shortcut_row(
                "organiser-ctx-intruder",
                "发送到 Intruder",
                if mod_key == "cmd" { "cmd-i" } else { "ctrl-i" },
            )
        })
        .disabled(!editable)
        .on_click(move |_, window, cx| {
            let _ = o_intruder.update(cx, |view, cx| {
                if view.select_organiser_item(id, window, cx) {
                    view.send_organiser_to_intruder(window, cx);
                }
            });
        }),
    )
    .item(
        PopupMenuItem::element(move |_, _| {
            organiser_shortcut_row(
                "organiser-ctx-repeater",
                "发送到 Repeater",
                if mod_key == "cmd" { "cmd-r" } else { "ctrl-r" },
            )
        })
        .disabled(!editable)
        .on_click(move |_, window, cx| {
            let _ = o_repeater.update(cx, |view, cx| {
                if view.select_organiser_item(id, window, cx) {
                    view.send_organiser_to_repeater(window, cx);
                }
            });
        }),
    );
    let menu = add_item(
        menu,
        "发送到 Sequencer",
        None,
        editable,
        Workbench::send_to_sequencer,
    );
    let menu = add_item(
        menu,
        "发送到 Comparer (请求)",
        None,
        editable,
        |v, w, cx| v.send_organiser_to_comparer(false, w, cx),
    );
    let menu = add_item(
        menu,
        "发送到 Comparer (响应)",
        None,
        editable && has_response,
        |v, w, cx| v.send_organiser_to_comparer(true, w, cx),
    );
    let menu = add_item(
        menu,
        "发送到 Decoder",
        None,
        editable,
        Workbench::send_to_decoder,
    )
    .item(
        PopupMenuItem::element(move |_, _| {
            organiser_shortcut_row(
                "organiser-ctx-organizer",
                "发送到 Organizer",
                if mod_key == "cmd" { "cmd-o" } else { "ctrl-o" },
            )
        })
        .disabled(!editable)
        .on_click(move |_, window, cx| {
            let _ = o_org.update(cx, |view, cx| {
                if view.select_organiser_item(id, window, cx) {
                    duplicate_selected_organiser_item(view, window, cx);
                }
            });
        }),
    )
    .item(
        PopupMenuItem::element(move |_, _| {
            organiser_shortcut_row(
                "organiser-ctx-mio-at",
                "发送到 MioAT",
                if mod_key == "cmd" { "cmd-g" } else { "ctrl-g" },
            )
        })
        .icon(IconName::Zap)
        .disabled(!editable)
        .on_click(move |_, window, cx| {
            let _ = o_ai.update(cx, |view, cx| {
                if view.select_organiser_item(id, window, cx) {
                    view.send_organiser_to_mio_at(cx);
                }
            });
        }),
    )
    .separator()
    .item(PopupMenuItem::submenu("记录问题", record_issue_submenu))
    .separator();
    let menu = add_item(
        menu,
        "在浏览器中打开响应",
        None,
        editable && has_response,
        Workbench::open_current_response_in_browser,
    )
    .item(PopupMenuItem::submenu(
        "在浏览器中请求",
        req_browser_submenu,
    ))
    .separator()
    .item(PopupMenuItem::submenu("参与工具", engagement_submenu))
    .separator();
    let menu = add_item(
        menu,
        "添加备注",
        Some(IconName::File),
        editable,
        Workbench::focus_organiser_notes_editor,
    )
    .item(PopupMenuItem::submenu("高亮", hl_submenu))
    .item(PopupMenuItem::submenu("状态", status_submenu))
    .separator()
    .item(
        PopupMenuItem::new("删除条目")
            .icon(IconName::Trash)
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = delete_owner.update(cx, |view, cx| {
                    if multi_selected {
                        view.delete_selected_organiser_batch(window, cx);
                    } else {
                        view.delete_organiser_item(id, window, cx);
                    }
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("导出为 CSV")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = csv_owner.update(cx, |view, cx| {
                    view.export_organiser_tsv(window, cx);
                });
            }),
    );
    let menu = add_item(
        menu,
        "复制 URL",
        Some(IconName::Copy),
        editable,
        Workbench::copy_organiser_url,
    );
    let menu = add_item(
        menu,
        "复制为 curl 命令 (bash)",
        Some(IconName::Copy),
        editable,
        Workbench::copy_organiser_curl,
    );
    let menu = add_item(
        menu,
        "复制响应中的链接",
        Some(IconName::Copy),
        editable && has_response,
        Workbench::copy_links_in_response,
    );
    let menu = add_item(
        menu,
        "保存条目",
        Some(IconName::Save),
        editable,
        Workbench::save_selected_organiser_item_to_file,
    )
    .separator();
    menu.item(
        PopupMenuItem::new("添加到站点地图")
            .icon(gpui_kit::component::Icon::default().data(MIO_ADD_TO_SITEMAP_SVG))
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = sitemap_owner.update(cx, |view, cx| {
                    if view.select_organiser_item(id, window, cx) {
                        view.add_to_site_map(window, cx);
                    }
                });
            }),
    )
}
