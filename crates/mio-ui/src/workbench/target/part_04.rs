use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;

use super::*;
const _: Option<fn(&Entity<gpui_kit::component::input::EditorState>) -> Editor> = Some(Editor::new);

impl Workbench {
    pub(in crate::workbench) fn render_target_message_pair(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pair = self.render_context_message_pair(
            "target",
            &self.target_pane.request,
            &self.target_pane.response,
            true,
            cx,
        );
        self.render_http_notes(pair, cx)
    }

    pub(in crate::workbench) fn render_scope_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let advanced = self.target_pane.advanced_mode_input;
        let editable =
            self.project.is_some() && self.transition.is_none() && self.backend_available();
        let owner = cx.weak_entity();
        let adv_owner = owner.clone();
        let gear_owner = owner.clone();
        let title_fg: gpui_kit::Hsla = if theme::is_dark() {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };

        let header_row = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Popover::new("target-scope-help-popover")
                    .anchor(gpui_kit::Anchor::TopLeft)
                    .trigger(
                        Button::new("target-scope-help")
                            .ghost()
                            .compact()
                            .size(px(22.))
                            .p_0()
                            .rounded(px(11.))
                            .accessibility_label("目标范围帮助")
                            .tooltip("查看 Target scope 范围规则说明")
                            .child(
                                Icon::new(IconName::CircleQuestionMark)
                                    .size(px(19.))
                                    .text_color(title_fg),
                            ),
                    )
                    .content(|_, _, _| {
                        div()
                            .id("target-scope-help-content")
                            .test_support()
                            .w(px(320.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child("目标范围"),
                            )
                            .child("包含规则定义哪些主机与 URL 属于当前测试范围。")
                            .child("排除规则优先级高于包含规则，可用于过滤登出页或敏感管理路径。")
                            .child("勾选「使用高级范围控制」可按协议、主机、端口与文件路径正则表达式精细匹配。")
                    }),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(title_fg)
                    .child("目标范围"),
            );

        let desc_row = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::new("target-scope-settings-menu")
                    .ghost()
                    .compact()
                    .size(px(22.))
                    .p_0()
                    .rounded(px(3.))
                    .accessibility_label("目标范围设置菜单")
                    .tooltip("导入、导出或重置目标范围配置")
                    .child(
                        Icon::new(IconName::Settings)
                            .size(px(19.))
                            .text_color(title_fg),
                    )
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::TopLeft, move |menu, _, cx| {
                        let can_edit = gear_owner
                            .upgrade()
                            .is_some_and(|e| {
                                let wb = e.read(cx);
                                wb.project.is_some()
                                    && wb.transition.is_none()
                                    && wb.backend_available()
                            });
                        let o_copy = gear_owner.clone();
                        let o_paste = gear_owner.clone();
                        let o_reset = gear_owner.clone();
                        menu.compact()
                            .min_w(px(172.))
                            .item(
                                PopupMenuItem::new("复制 Scope (JSON)").on_click(
                                    move |_, window, cx| {
                                        let _ = o_copy.update(cx, |v, cx| {
                                            v.copy_target_scope_json(window, cx);
                                        });
                                    },
                                ),
                            )
                            .item(
                                PopupMenuItem::new("粘贴 Scope (JSON)")
                                    .disabled(!can_edit)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_paste.update(cx, |v, cx| {
                                            v.paste_target_scope_json(window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("清空全部范围规则")
                                    .disabled(!can_edit)
                                    .on_click(move |_, _, cx| {
                                        let _ = o_reset.update(cx, |v, cx| {
                                            let project = v.project;
                                            v.reset_scope_rules(project, cx);
                                        });
                                    }),
                            )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("使用这些设置精确定义当前工作所属的主机与 URL 范围。此配置会影响整个工作台各工具的行为。"),
            );

        let advanced_checkbox = div().pl(px(30.)).child(
            mio_checkbox(
                "target-scope-advanced-toggle",
                "使用高级范围控制",
                advanced,
                editable,
            )
            .on_change(move |_, _, _, cx| {
                let _ = adv_owner.update(cx, |this, cx| {
                    this.toggle_target_advanced_mode(cx);
                });
            }),
        );

        let body = if advanced {
            self.render_advanced_scope(window, cx)
        } else {
            self.render_simple_scope(window, cx)
        };

        div()
            .id("target-scope-page")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::surface())
            .px(px(16.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(header_row)
            .child(desc_row)
            .child(advanced_checkbox)
            .child(div().pt(px(4.)).pl(px(30.)).child(body))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_simple_scope(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editable =
            self.project.is_some() && self.transition.is_none() && self.backend_available();
        let include_section = self.render_simple_scope_section(true, editable, cx);
        let exclude_section = self.render_simple_scope_section(false, editable, cx);
        div()
            .w_full()
            .max_w(px(820.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(include_section)
            .child(exclude_section)
            .into_any_element()
    }

    fn render_simple_scope_section(
        &self,
        include: bool,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let side = usize::from(!include);
        let rules = if include {
            &self.workspace.scope.include
        } else {
            &self.workspace.scope.exclude
        };
        let selected_id = self.target_pane.selected_simple_scope[side]
            .filter(|id| rules.iter().any(|r| r.id == *id));
        let editor_open = self.target_pane.scope_editor_open[side];
        let editing = self.target_pane.editing_simple_scope[side].is_some();
        let editor_include_subdomains = self.target_pane.simple_scope_include_subdomains[side];
        let editor_sub_owner = cx.weak_entity();
        let (
            title,
            input_id,
            input_aria,
            add_id,
            edit_btn_id,
            remove_btn_id,
            paste_btn_id,
            load_btn_id,
            save_btn_id,
            cancel_btn_id,
        ) = if include {
            (
                "包含在范围内",
                "target-scope-include-input",
                "包含规则前缀",
                "target-scope-include-add",
                "target-scope-include-edit-btn",
                "target-scope-include-remove-btn",
                "target-scope-include-paste-url",
                "target-scope-include-load",
                "target-scope-include-save",
                "target-scope-include-cancel",
            )
        } else {
            (
                "从范围中排除",
                "target-scope-exclude-input",
                "排除规则前缀",
                "target-scope-exclude-add",
                "target-scope-exclude-edit-btn",
                "target-scope-exclude-remove-btn",
                "target-scope-exclude-paste-url",
                "target-scope-exclude-load",
                "target-scope-exclude-save",
                "target-scope-exclude-cancel",
            )
        };
        let input_entity = if include {
            &self.target_pane.simple_scope_inputs.include
        } else {
            &self.target_pane.simple_scope_inputs.exclude
        };
        let rows = self.render_scope_rules(include, editable, selected_id, cx);
        let header_bg = table_header_bg();
        let header_fg = table_header_fg();

        let side_buttons = div()
            .w(theme::scaled_px(102.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                mio_neutral_button(add_id, "添加", Some(102.))
                    .tooltip("添加新的 URL 前缀范围规则")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.target_pane.scope_editor_open[side] {
                            this.submit_target_simple_scope(include, window, cx);
                        } else {
                            this.open_simple_scope_add(include, window, cx);
                        }
                    })),
            )
            .child(
                mio_neutral_button(edit_btn_id, "编辑", Some(102.))
                    .tooltip("编辑当前选中的目标范围规则")
                    .disabled(!editable || selected_id.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(id) = selected_id {
                            this.edit_simple_scope_rule(include, id, window, cx);
                        }
                    })),
            )
            .child(
                mio_neutral_button(remove_btn_id, "移除", Some(102.))
                    .tooltip("移除当前选中的目标范围规则")
                    .disabled(!editable || selected_id.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(id) = selected_id {
                            this.remove_scope_rule(this.project, id, cx);
                            if this.target_pane.selected_simple_scope[side] == Some(id) {
                                this.target_pane.selected_simple_scope[side] = None;
                            }
                        }
                    })),
            )
            .child(
                mio_neutral_button(paste_btn_id, "粘贴 URL", Some(102.))
                    .tooltip("从剪贴板粘贴 URL 并直接添加到此范围列表")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.paste_scope_url_from_clipboard(include, cx);
                    })),
            )
            .child(
                mio_neutral_button(load_btn_id, "加载…", Some(102.))
                    .tooltip("从本地文本文件按行批量导入 URL 范围规则")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_scope_urls_from_file(include, window, cx);
                    })),
            );

        let table_header = div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(header_bg)
            .text_color(header_fg)
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .w(theme::scaled_px(76.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .child("Enabled"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(140.))
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .child("Prefix"),
            )
            .child(
                div()
                    .w(theme::scaled_px(148.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .child("Include subdomains"),
            );

        let table_box = div()
            .flex_1()
            .min_w_0()
            .h(theme::scaled_px(170.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(table_header)
            .child(rows);

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child(title),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(side_buttons)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .child(table_box)
                            .child(mio_scope_table_triangle()),
                    ),
            )
            .when(editor_open, |section| {
                section.child(
                    div()
                        .pl(theme::scaled_px(110.))
                        .child(
                            div()
                                .w_full()
                                .p(px(8.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(input_entity)
                                            .id(input_id)
                                            .aria_label(input_aria)
                                            .small()
                                            .disabled(!editable),
                                    ),
                                )
                                .child(
                                    mio_checkbox(
                                        if include {
                                            "target-scope-include-subdomains-editor"
                                        } else {
                                            "target-scope-exclude-subdomains-editor"
                                        },
                                        "包含子域",
                                        editor_include_subdomains,
                                        editable,
                                    )
                                    .on_change(move |_, _, _, cx| {
                                        let _ = editor_sub_owner.update(cx, |this, cx| {
                                            this.target_pane.simple_scope_include_subdomains[side] =
                                                !this.target_pane.simple_scope_include_subdomains
                                                    [side];
                                            cx.notify();
                                        });
                                    }),
                                )
                                .child(
                                    button(save_btn_id, if editing { "保存" } else { "确定" })
                                        .primary()
                                        .disabled(!editable)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.submit_target_simple_scope(include, window, cx);
                                        })),
                                )
                                .child(
                                    button(cancel_btn_id, "取消")
                                        .disabled(!editable)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.cancel_edit_simple_scope_rule(include, window, cx);
                                        })),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    pub(in crate::workbench) fn render_scope_rules(
        &self,
        include: bool,
        editable: bool,
        selected_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let side = usize::from(!include);
        let rules = if include {
            &self.workspace.scope.include
        } else {
            &self.workspace.scope.exclude
        };
        let row_prefix = if include {
            "target-scope-include-row"
        } else {
            "target-scope-exclude-row"
        };
        let toggle_prefix = if include {
            "target-scope-toggle"
        } else {
            "target-scope-exclude-toggle"
        };
        let subdomains_toggle_prefix = if include {
            "target-scope-include-subdomains-toggle"
        } else {
            "target-scope-exclude-subdomains-toggle"
        };
        let stripe_bg = if theme::table_zebra_stripe() {
            table_stripe_bg()
        } else {
            theme::surface()
        };
        let owner = cx.weak_entity();
        let mut rows = div()
            .id(if include {
                "target-simple-include-rows"
            } else {
                "target-simple-exclude-rows"
            })
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (index, rule) in rules.iter().enumerate() {
            let id = rule.id;
            let is_selected = selected_id == Some(id);
            let toggle_owner = owner.clone();
            let sub_toggle_owner = owner.clone();
            let prefix_text = rule.prefix.clone();
            rows = rows.child(
                div()
                    .id((row_prefix, index))
                    .test_support()
                    .aria_label(prefix_text.clone())
                    .aria_selected(is_selected)
                    .w_full()
                    .h(theme::scaled_px(22.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(rgb(if theme::is_dark() { 0x33363b } else { 0xf0f0f0 }))
                    .bg(if is_selected {
                        theme::selected()
                    } else if index % 2 == 1 {
                        stripe_bg
                    } else {
                        theme::surface()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.target_pane.selected_simple_scope[side] = Some(id);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(theme::scaled_px(76.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child(
                                mio_checkbox((toggle_prefix, index), "", rule.enabled, editable)
                                    .accessibility_label(format!("启用范围规则 {}", rule.prefix))
                                    .on_change(move |_, _, _, cx| {
                                        let _ = toggle_owner.update(cx, |this, cx| {
                                            this.toggle_scope_rule(this.project, id, cx);
                                        });
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(140.))
                            .px(px(8.))
                            .truncate()
                            .text_size(theme::ui_font_size())
                            .child(prefix_text),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(148.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child(
                                mio_checkbox(
                                    (subdomains_toggle_prefix, index),
                                    "",
                                    rule.include_subdomains,
                                    editable,
                                )
                                .accessibility_label(format!("包含子域 {}", rule.prefix))
                                .on_change(move |_, _, _, cx| {
                                    let _ = sub_toggle_owner.update(cx, |this, cx| {
                                        this.toggle_scope_rule_subdomains(this.project, id, cx);
                                    });
                                }),
                            ),
                    ),
            );
        }
        rows.into_any_element()
    }
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
