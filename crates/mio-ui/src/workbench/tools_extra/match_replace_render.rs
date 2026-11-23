use gpui_kit::assets::IconName;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::select::Select;
use gpui_kit::component::table::DataTable;
use mio_core::WsReplaceDirection;

use super::*;

#[derive(Clone, Copy)]
struct MatchReplacePalette {
    dark: bool,
    title_color: gpui_kit::Hsla,
    table_border: gpui_kit::Hsla,
    editor_bg: gpui_kit::Hsla,
    icon_color: gpui_kit::Hsla,
    gutter_indent: gpui_kit::Pixels,
    button_col_w: gpui_kit::Pixels,
    table_h: gpui_kit::Pixels,
}

impl MatchReplacePalette {
    fn current() -> Self {
        let dark = theme::is_dark();
        Self {
            dark,
            title_color: if dark { theme::foreground() } else { rgb(0x000000).into() },
            table_border: if dark { theme::border() } else { rgb(0xd4d4d4).into() },
            editor_bg: if dark { theme::toolbar_surface() } else { rgb(0xfafafa).into() },
            icon_color: if dark { theme::foreground() } else { rgb(0x1a1a1a).into() },
            gutter_indent: theme::scaled_px(28.),
            button_col_w: theme::scaled_px(84.).max(px(80.)),
            table_h: theme::scaled_px(150.).max(px(138.)),
        }
    }
}

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_match_and_replace(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MatchReplacePalette::current();
        let page_bg: gpui_kit::Hsla = if p.dark {
            theme::background()
        } else {
            rgb(0xffffff).into()
        };
        let divider_color: gpui_kit::Hsla = if p.dark {
            theme::border()
        } else {
            rgb(0xe0e0e0).into()
        };
        let editable =
            self.project.is_some() && self.transition.is_none() && self.backend_available();
        let project = self.project;

        div()
            .id("mr-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(page_bg)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(self.render_http_match_replace_section(p, editable, project, cx))
                    .child(div().w_full().h(px(1.)).flex_shrink_0().bg(divider_color))
                    .child(self.render_ws_match_replace_section(p, editable, project, cx)),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_http_match_replace_section(
        &self,
        p: MatchReplacePalette,
        editable: bool,
        project: Option<ProjectKey>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let http_table_w = mr_table_width(&WB_MR_COLUMNS);
        // Section 1: HTTP match and replace rules
        let http_rules = &self.workspace.match_and_replace_rules;
        let http_len = http_rules.len();
        let http_has_disabled = http_rules.iter().any(|r| !r.enabled);
        let http_has_enabled = http_rules.iter().any(|r| r.enabled);
        let selected_id = self.extra_tools.mr_selected;
        let selected = selected_id.and_then(|id| http_rules.iter().position(|r| r.id == id));
        let in_scope_only = self.workspace.proxy_messages.match_replace_in_scope_only;

        let mut http_actions = div()
            .w(p.button_col_w)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            ("mr-side-add", "添加", "添加新的 HTTP 匹配与替换规则", editable, 0u8),
            (
                "mr-edit",
                "编辑",
                "编辑选中的 HTTP 匹配与替换规则",
                editable && selected.is_some(),
                1u8,
            ),
            (
                "mr-remove",
                "移除",
                "移除选中的 HTTP 匹配与替换规则",
                editable && selected.is_some(),
                2u8,
            ),
            (
                "mr-up",
                "上移",
                "上移选中的规则以提高优先级",
                editable && selected.is_some_and(|i| i > 0),
                3u8,
            ),
            (
                "mr-down",
                "下移",
                "下移选中的规则以降低优先级",
                editable && selected.is_some_and(|i| i + 1 < http_len),
                4u8,
            ),
        ] {
            let rule = selected.and_then(|i| http_rules.get(i)).cloned();
            http_actions = http_actions.child(mr_action_button(id, label, tip, available).on_click(
                cx.listener(move |this, _, window, cx| {
                    if action == 0 {
                        if let Some(project) = this.project {
                            if let Some(prev) = this.extra_tools.mr_dialog.take() {
                                let _ = prev.update(cx, |_, window, _| window.remove_window());
                            }
                            this.extra_tools.mr_dialog = open_http_match_replace_dialog(
                                cx.weak_entity(),
                                window.window_handle(),
                                project,
                                None,
                                "mr",
                                window,
                                cx,
                            )
                            .ok();
                        }
                        cx.notify();
                        return;
                    }
                    let Some(id) = selected_id else {
                        return;
                    };
                    match action {
                        1 => {
                            if let Some(rule) = &rule
                                && let Some(project) = this.project
                            {
                                if let Some(prev) = this.extra_tools.mr_dialog.take() {
                                    let _ = prev.update(cx, |_, window, _| window.remove_window());
                                }
                                this.extra_tools.mr_dialog = open_http_match_replace_dialog(
                                    cx.weak_entity(),
                                    window.window_handle(),
                                    project,
                                    Some(rule.clone()),
                                    "mr",
                                    window,
                                    cx,
                                )
                                .ok();
                            }
                        }
                        2 => this.remove_http_match_replace_rule(id, cx),
                        3 => this.move_http_match_replace_rule(id, false, cx),
                        _ => this.move_http_match_replace_rule(id, true, cx),
                    }
                }),
            ));
        }

        let http_owner = cx.weak_entity();
        let http_gear = Button::new("mr-http-settings")
            .ghost()
            .compact()
            .p_0()
            .min_w_0()
            .w(theme::scaled_px(20.))
            .h(theme::scaled_px(20.))
            .tab_stop(false)
            .accessibility_label("HTTP match and replace rules 设置")
            .tooltip("HTTP 匹配与替换规则选项")
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(theme::scaled_px(19.))
                    .text_color(p.icon_color),
            )
            .dropdown_menu(move |menu, _, _| {
                let scope_owner = http_owner.clone();
                let enable_owner = http_owner.clone();
                let disable_owner = http_owner.clone();
                let clear_owner = http_owner.clone();
                let settings_owner = http_owner.clone();
                menu.item(
                    PopupMenuItem::new("仅应用于范围内的项目")
                        .disabled(!editable)
                        .checked(in_scope_only)
                        .on_click(move |_, _, cx| {
                            let _ = scope_owner.update(cx, |this, cx| {
                                this.set_proxy_message_flag(
                                    project,
                                    |m| m.match_replace_in_scope_only = !in_scope_only,
                                    cx,
                                );
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("全部启用 HTTP 规则")
                        .disabled(!editable || !http_has_disabled)
                        .on_click(move |_, _, cx| {
                            let _ = enable_owner.update(cx, |this, cx| {
                                this.set_all_http_match_replace_enabled(true, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("全部禁用 HTTP 规则")
                        .disabled(!editable || !http_has_enabled)
                        .on_click(move |_, _, cx| {
                            let _ = disable_owner.update(cx, |this, cx| {
                                this.set_all_http_match_replace_enabled(false, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("清空全部 HTTP 规则")
                        .disabled(!editable || http_len == 0)
                        .on_click(move |_, _, cx| {
                            let _ = clear_owner.update(cx, |this, cx| {
                                this.clear_http_match_replace_rules(cx);
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("打开 Proxy 设置…").on_click(move |_, window, cx| {
                    let _ = settings_owner.update(cx, |this, cx| {
                        this.open_settings_filtered(
                            Some(crate::workbench::settings::SettingsPage::Proxy),
                            None,
                            false,
                            window,
                            cx,
                        );
                    });
                }))
            });

        let http_scope_entity = cx.entity();
        let http_section = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .pl(px(12.))
            .pr(px(18.))
            .pt(px(14.))
            .pb(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(mr_help_popover(
                        "mr-http-help-popover",
                        "mr-http-help",
                        "HTTP match and replace rules",
                        &[
                            "按列表顺序自动匹配并替换流经 Proxy 的 HTTP 请求头、请求体、响应头、响应体与参数内容。",
                            "点击左侧“添加”或“编辑”可配置普通文本、正则表达式或 脚本规则。",
                        ],
                    ))
                    .child(
                        div()
                            .id("mr-http-heading")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Heading)
                            .aria_label("HTTP match and replace rules")
                            .text_size(theme::scaled_px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(p.title_color)
                            .child("HTTP match and replace rules"),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(http_gear)
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("使用这些设置自动替换通过 Proxy 的 HTTP 请求和响应中的内容。"),
                    ),
            )
            .child(
                div().mt(px(10.)).pl(p.gutter_indent).child(
                    mr_checkbox(
                        "proxy-http-mr-in-scope-only",
                        "仅应用于范围内的项目",
                        "Only apply to in-scope items",
                        in_scope_only,
                        editable,
                    )
                    .on_change(move |state, _, _, cx| {
                        http_scope_entity.update(cx, |this, cx| {
                            this.set_proxy_message_flag(
                                project,
                                |m| m.match_replace_in_scope_only = state == CheckboxState::Checked,
                                cx,
                            );
                        });
                    }),
                ),
            )
            .child(
                div()
                    .id("mr-rules")
                    .mt(px(10.))
                    .pl(p.gutter_indent)
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .child(http_actions)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .flex_shrink(1.0)
                            .min_w_0()
                            .child(
                                div()
                                    .w(http_table_w)
                                    .flex_shrink(1.0)
                                    .min_w_0()
                                    .h(p.table_h)
                                    .border_1()
                                    .border_color(p.table_border)
                                    .bg(theme::surface())
                                    .overflow_hidden()
                                    .child(
                                        DataTable::new(&self.extra_tools.mr_table)
                                            .with_size(theme::scaled_px(22.).max(px(18.)))
                                            .header_row_height(theme::table_header_height())
                                            .bordered(false)
                                            .stripe(true)
                                            .scroll_bounce(false)
                                            .scrollbar_visible(true, false)
                                            .accessibility_label("HTTP match and replace rules"),
                                    ),
                            )
                            .child(mr_table_right_triangle(p.dark)),
                    ),
            );
        http_section.into_any_element()
    }

    #[inline(never)]
    fn render_ws_match_replace_section(
        &self,
        p: MatchReplacePalette,
        editable: bool,
        project: Option<ProjectKey>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ws_table_w = mr_table_width(&WB_WS_MR_COLUMNS);
        // Section 2: WebSocket match and replace rules
        let ws_rules = &self.workspace.ws_match_and_replace_rules;
        let ws_len = ws_rules.len();
        let ws_has_disabled = ws_rules.iter().any(|r| !r.enabled);
        let ws_has_enabled = ws_rules.iter().any(|r| r.enabled);
        let ws_selected_id = self.extra_tools.ws_mr_selected;
        let ws_selected = ws_selected_id.and_then(|id| ws_rules.iter().position(|r| r.id == id));
        let ws_is_editing = self.extra_tools.ws_mr_editing.is_some();
        let ws_editor_open = self.extra_tools.ws_mr_editor_open;
        let ws_regex = self.extra_tools.ws_mr_regex;
        let ws_error = self.extra_tools.ws_mr_error.clone();
        let ws_in_scope_only = self.workspace.proxy_messages.ws_match_replace_in_scope_only;

        let mut ws_actions = div()
            .w(p.button_col_w)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            ("ws-mr-add", "添加", "添加新的 WebSocket 匹配与替换规则", editable, 0u8),
            (
                "ws-mr-edit",
                "编辑",
                "编辑选中的 WebSocket 匹配与替换规则",
                editable && ws_selected.is_some(),
                1u8,
            ),
            (
                "ws-mr-remove",
                "移除",
                "移除选中的 WebSocket 匹配与替换规则",
                editable && ws_selected.is_some(),
                2u8,
            ),
            (
                "ws-mr-up",
                "上移",
                "上移选中的 WebSocket 规则以提高优先级",
                editable && ws_selected.is_some_and(|i| i > 0),
                3u8,
            ),
            (
                "ws-mr-down",
                "下移",
                "下移选中的 WebSocket 规则以降低优先级",
                editable && ws_selected.is_some_and(|i| i + 1 < ws_len),
                4u8,
            ),
        ] {
            let rule = ws_selected.and_then(|i| ws_rules.get(i)).cloned();
            ws_actions = ws_actions.child(mr_action_button(id, label, tip, available).on_click(
                cx.listener(move |this, _, window, cx| {
                    if action == 0 {
                        this.extra_tools.ws_mr_editing = None;
                        this.extra_tools.ws_mr_editor_open = true;
                        this.extra_tools.ws_mr_error = None;
                        this.extra_tools.ws_mr_regex = false;
                        this.extra_tools.ws_mr_direction = WsReplaceDirection::Both;
                        this.extra_tools
                            .ws_mr_direction_select
                            .update(cx, |s, cx| {
                                s.set_selected_index(
                                    Some(gpui_kit::component::IndexPath::new(0)),
                                    window,
                                    cx,
                                );
                            });
                        for input in [
                            &this.extra_tools.ws_mr_match,
                            &this.extra_tools.ws_mr_replace,
                            &this.extra_tools.ws_mr_comment,
                        ] {
                            input.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                        this.extra_tools
                            .ws_mr_match
                            .update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                        return;
                    }
                    let Some(id) = ws_selected_id else {
                        return;
                    };
                    match action {
                        1 => {
                            if let Some(rule) = &rule {
                                this.load_ws_match_replace_for_edit(rule, window, cx);
                            }
                        }
                        2 => this.remove_ws_match_replace_rule(id, cx),
                        3 => this.move_ws_match_replace_rule(id, false, cx),
                        _ => this.move_ws_match_replace_rule(id, true, cx),
                    }
                }),
            ));
        }

        let ws_owner = cx.weak_entity();
        let ws_gear = Button::new("mr-ws-settings")
            .ghost()
            .compact()
            .p_0()
            .min_w_0()
            .w(theme::scaled_px(20.))
            .h(theme::scaled_px(20.))
            .tab_stop(false)
            .accessibility_label("WebSocket match and replace rules 设置")
            .tooltip("WebSocket 匹配与替换规则选项")
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(theme::scaled_px(19.))
                    .text_color(p.icon_color),
            )
            .dropdown_menu(move |menu, _, _| {
                let scope_owner = ws_owner.clone();
                let enable_owner = ws_owner.clone();
                let disable_owner = ws_owner.clone();
                let clear_owner = ws_owner.clone();
                let settings_owner = ws_owner.clone();
                menu.item(
                    PopupMenuItem::new("仅应用于范围内的项目")
                        .disabled(!editable)
                        .checked(ws_in_scope_only)
                        .on_click(move |_, _, cx| {
                            let _ = scope_owner.update(cx, |this, cx| {
                                this.set_proxy_message_flag(
                                    project,
                                    |m| m.ws_match_replace_in_scope_only = !ws_in_scope_only,
                                    cx,
                                );
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("全部启用 WebSocket 规则")
                        .disabled(!editable || !ws_has_disabled)
                        .on_click(move |_, _, cx| {
                            let _ = enable_owner.update(cx, |this, cx| {
                                this.set_all_ws_match_replace_enabled(true, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("全部禁用 WebSocket 规则")
                        .disabled(!editable || !ws_has_enabled)
                        .on_click(move |_, _, cx| {
                            let _ = disable_owner.update(cx, |this, cx| {
                                this.set_all_ws_match_replace_enabled(false, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("清空全部 WebSocket 规则")
                        .disabled(!editable || ws_len == 0)
                        .on_click(move |_, _, cx| {
                            let _ = clear_owner.update(cx, |this, cx| {
                                this.clear_ws_match_replace_rules(cx);
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("打开 Proxy 设置…").on_click(move |_, window, cx| {
                    let _ = settings_owner.update(cx, |this, cx| {
                        this.open_settings_filtered(
                            Some(crate::workbench::settings::SettingsPage::Proxy),
                            None,
                            false,
                            window,
                            cx,
                        );
                    });
                }))
            });

        let ws_inline_editor = if ws_editor_open {
            let ws_regex_entity = cx.entity();
            Some(
                div()
                    .mt(px(10.))
                    .pl(p.gutter_indent)
                    .child(
                        div()
                            .id("ws-mr-inline-editor")
                            .test_support()
                            .max_w(p.button_col_w + px(6.) + ws_table_w)
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .p(px(10.))
                            .border_1()
                            .border_color(p.table_border)
                            .rounded(px(3.))
                            .bg(p.editor_bg)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("消息方向:"),
                                    )
                                    .child(
                                        div().w(theme::scaled_px(220.).max(px(200.))).child(
                                            Select::new(&self.extra_tools.ws_mr_direction_select)
                                                .id("ws-mr-direction-select")
                                                .small()
                                                .w_full()
                                                .disabled(!editable),
                                        ),
                                    )
                                    .child(
                                        mr_checkbox(
                                            "ws-mr-regex-toggle",
                                            "Regex match",
                                            "Regex match",
                                            ws_regex,
                                            editable,
                                        )
                                        .on_change(move |state, _, _, cx| {
                                            ws_regex_entity.update(cx, |this, cx| {
                                                this.extra_tools.ws_mr_regex =
                                                    state == CheckboxState::Checked;
                                                cx.notify();
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("匹配内容:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.ws_mr_match)
                                                .id("ws-mr-match-input")
                                                .aria_label("WebSocket 匹配模式")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("替换为:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.ws_mr_replace)
                                                .id("ws-mr-replace-input")
                                                .aria_label("WebSocket 替换内容")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("备注:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.ws_mr_comment)
                                                .id("ws-mr-comment-input")
                                                .aria_label("WebSocket 规则备注")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .when_some(ws_error, |card, error| {
                                card.child(
                                    div()
                                        .id("ws-mr-error")
                                        .test_support()
                                        .role(gpui_kit::accesskit::Role::Alert)
                                        .aria_label(error.clone())
                                        .text_color(theme::danger_fg())
                                        .child(error),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .child(
                                        button(
                                            "ws-mr-submit",
                                            if ws_is_editing { "确定" } else { "添加" },
                                        )
                                        .primary()
                                        .tooltip("保存并启用 WebSocket 匹配与替换规则")
                                        .disabled(!editable)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_ws_match_replace(window, cx)
                                        })),
                                    )
                                    .child(
                                        button("ws-mr-cancel", "取消")
                                            .tooltip("关闭 WebSocket 规则编辑表单")
                                            .disabled(!editable)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.extra_tools.ws_mr_editing = None;
                                                this.extra_tools.ws_mr_editor_open = false;
                                                this.extra_tools.ws_mr_error = None;
                                                this.extra_tools.ws_mr_regex = false;
                                                for input in [
                                                    &this.extra_tools.ws_mr_match,
                                                    &this.extra_tools.ws_mr_replace,
                                                    &this.extra_tools.ws_mr_comment,
                                                ] {
                                                    input.update(cx, |s, cx| {
                                                        s.set_value("", window, cx)
                                                    });
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
        } else {
            None
        };

        let ws_scope_entity = cx.entity();
        let ws_section = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .pl(px(12.))
            .pr(px(18.))
            .pt(px(14.))
            .pb(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(mr_help_popover(
                        "mr-ws-help-popover",
                        "mr-ws-help",
                        "WebSocket match and replace rules",
                        &[
                            "按列表顺序自动匹配并替换流经 Proxy 的 WebSocket 文本消息内容（自动跳过二进制帧）。",
                            "支持限定双向、客户端到服务端或服务端到客户端方向，以及正则表达式替换。",
                        ],
                    ))
                    .child(
                        div()
                            .id("mr-ws-heading")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Heading)
                            .aria_label("WebSocket match and replace rules")
                            .text_size(theme::scaled_px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(p.title_color)
                            .child("WebSocket match and replace rules"),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ws_gear)
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("使用这些设置自动替换通过 Proxy 的 WebSocket 消息中的内容。"),
                    ),
            )
            .child(
                div().mt(px(10.)).pl(p.gutter_indent).child(
                    mr_checkbox(
                        "proxy-ws-mr-in-scope-only",
                        "仅应用于范围内的项目",
                        "Only apply to in-scope items",
                        ws_in_scope_only,
                        editable,
                    )
                    .on_change(move |state, _, _, cx| {
                        ws_scope_entity.update(cx, |this, cx| {
                            this.set_proxy_message_flag(
                                project,
                                |m| {
                                    m.ws_match_replace_in_scope_only =
                                        state == CheckboxState::Checked
                                },
                                cx,
                            );
                        });
                    }),
                ),
            )
            .when_some(ws_inline_editor, |section, editor| section.child(editor))
            .child(
                div()
                    .id("ws-mr-rules")
                    .mt(px(10.))
                    .pl(p.gutter_indent)
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .child(ws_actions)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .flex_shrink(1.0)
                            .min_w_0()
                            .child(
                                div()
                                    .w(ws_table_w)
                                    .flex_shrink(1.0)
                                    .min_w_0()
                                    .h(p.table_h)
                                    .border_1()
                                    .border_color(p.table_border)
                                    .bg(theme::surface())
                                    .overflow_hidden()
                                    .child(
                                        DataTable::new(&self.extra_tools.ws_mr_table)
                                            .with_size(theme::scaled_px(22.).max(px(18.)))
                                            .header_row_height(theme::table_header_height())
                                            .bordered(false)
                                            .stripe(true)
                                            .scroll_bounce(false)
                                            .scrollbar_visible(true, false)
                                            .accessibility_label("WebSocket match and replace rules"),
                                    ),
                            )
                            .child(mr_table_right_triangle(p.dark)),
                    ),
            );
        ws_section.into_any_element()
    }
}
