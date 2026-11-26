use super::*;

pub(crate) struct HttpMatchReplaceTable {
    pub(super) owner: WeakEntity<Settings>,
}

const HTTP_MR_COLUMNS: [(&str, f32); 7] = [
    ("启用", 64.),
    ("匹配项", 110.),
    ("名称", 110.),
    ("匹配", 164.),
    ("替换", 164.),
    ("类型", 86.),
    ("备注", 118.),
];

impl TableDelegate for HttpMatchReplaceTable {
    fn columns_count(&self, _: &App) -> usize {
        HTTP_MR_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), HTTP_MR_COLUMNS[index].0)
            .width((HTTP_MR_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).proxy_advanced.http_rules.len())
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(HTTP_MR_COLUMNS[col_ix].0, HTTP_MR_COLUMNS[col_ix].1)
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("proxy-http-mr-row", row)).border_b_0()
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (rule, enabled) = {
            let settings = owner.read(cx);
            let Some(rule) = settings.proxy_advanced.http_rules.get(row).cloned() else {
                return div().into_any_element();
            };
            (rule, settings.state.enabled)
        };
        match column {
            0 => {
                let id = rule.id;
                let wb = owner.read(cx).owner.clone();
                settings_table_checkbox_cell(
                    adv_checkbox(("proxy-http-mr-toggle", row), "", rule.enabled, enabled)
                        .p_0()
                        .gap_0()
                        .accessibility_label(format!("启用 HTTP 规则 {}", rule.match_pattern))
                        .on_change(move |_, _, _, cx| {
                            let _ = wb.update(cx, |view, cx| {
                                view.toggle_http_match_replace_rule(id, cx);
                            });
                        }),
                )
                .into_any_element()
            }
            _ => div()
                .px(px(6.))
                .truncate()
                .child(crate::workbench::tools_extra::mr_rule_cell(&rule, column))
                .into_any_element(),
        }
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let rule = owner.read(cx).proxy_advanced.http_rules.get(row)?;
        Some(crate::workbench::tools_extra::mr_rule_cell(rule, column).into())
    }
}

pub(crate) struct SslPassThroughTable {
    pub(super) owner: WeakEntity<Settings>,
}

const SSL_PT_COLUMNS: [(&str, f32); 4] = [
    ("启用", 72.),
    ("主机 / IP 范围", 516.),
    ("端口", 114.),
    ("正则", 114.),
];

impl TableDelegate for SslPassThroughTable {
    fn columns_count(&self, _: &App) -> usize {
        SSL_PT_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), SSL_PT_COLUMNS[index].0)
            .width((SSL_PT_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).proxy_advanced.ssl_rules.len())
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(SSL_PT_COLUMNS[col_ix].0, SSL_PT_COLUMNS[col_ix].1)
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("proxy-ssl-pt-row", row)).border_b_0()
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (rule, enabled, project) = {
            let settings = owner.read(cx);
            let Some(rule) = settings.proxy_advanced.ssl_rules.get(row).cloned() else {
                return div().into_any_element();
            };
            (rule, settings.state.enabled, settings.state.project)
        };
        match column {
            0 => {
                let id = rule.id;
                let wb = owner.read(cx).owner.clone();
                settings_table_checkbox_cell(
                    adv_checkbox(("proxy-ssl-pt-toggle", row), "", rule.enabled, enabled)
                        .p_0()
                        .gap_0()
                        .accessibility_label(format!("启用 TLS 透传 {}", rule.host_pattern))
                        .on_change(move |state, _, _, cx| {
                            let checked = state == CheckboxState::Checked;
                            let _ = wb.update(cx, |view, cx| {
                                let _ = view.update_proxy_advanced(
                                    project,
                                    |ssl, _, _| {
                                        if let Some(r) = ssl.iter_mut().find(|r| r.id == id) {
                                            r.enabled = checked;
                                        }
                                    },
                                    cx,
                                );
                            });
                        }),
                )
                .into_any_element()
            }
            1 => div().px(px(6.)).truncate().child(rule.host_pattern).into_any_element(),
            2 => div()
                .px(px(6.))
                .child(rule.port.map_or_else(|| "任意".into(), |p| p.to_string()))
                .into_any_element(),
            _ => div()
                .px(px(6.))
                .child(if rule.is_regex { "是" } else { "否" })
                .into_any_element(),
        }
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let rule = owner.read(cx).proxy_advanced.ssl_rules.get(row)?;
        Some(match column {
            0 => if rule.enabled { "是" } else { "否" }.into(),
            1 => rule.host_pattern.clone().into(),
            2 => rule.port.map_or_else(|| "任意".into(), |p| p.to_string()).into(),
            _ => if rule.is_regex { "是" } else { "否" }.into(),
        })
    }
}

impl Settings {
    pub(in crate::workbench::settings) fn render_proxy_advanced_cards(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(16.))
            .child(self.render_proxy_misc_extended_card(window))
            .child(self.render_http_match_replace_card(cx))
            .child(self.render_ssl_passthrough_card(cx))
            .child(self.render_ws_match_replace_card(window, cx))
            .child(self.render_response_modification_card(window))
    }

    fn render_http_match_replace_card(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let enabled = self.state.enabled;
        let selected_id = self.proxy_advanced.http_selected;
        let selected = selected_id.and_then(|id| {
            self.proxy_advanced
                .http_rules
                .iter()
                .position(|rule| rule.id == id)
        });
        let http_len = self.proxy_advanced.http_rules.len();
        let http_rules = self.proxy_advanced.http_rules.clone();

        let mut actions = div()
            .w(theme::scaled_px(84.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            ("proxy-http-mr-add", "添加", "新增 HTTP 匹配替换规则", enabled, 0u8),
            ("proxy-http-mr-edit", "编辑", "编辑选中的 HTTP 匹配替换规则", enabled && selected.is_some(), 1u8),
            ("proxy-http-mr-remove", "移除", "移除选中的 HTTP 匹配替换规则", enabled && selected.is_some(), 2u8),
            ("proxy-http-mr-up", "上移", "上移选中的 HTTP 匹配替换规则", enabled && selected.is_some_and(|i| i > 0), 3u8),
            ("proxy-http-mr-down", "下移", "下移选中的 HTTP 匹配替换规则", enabled && selected.is_some_and(|i| i + 1 < http_len), 4u8),
        ] {
            let rule = selected.and_then(|i| http_rules.get(i)).cloned();
            actions = actions.child(
                settings_action_button(id, label, !available)
                    .w_full()
                    .tooltip(tip)
                    .tab_stop(false)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action == 0 {
                            if let Some(project) = this.state.project {
                                if let Some(prev) = this.proxy_advanced.http_dialog.take() {
                                    let _ = prev.update(cx, |_, window, _| window.remove_window());
                                }
                                this.proxy_advanced.http_dialog =
                                    crate::workbench::tools_extra::open_http_match_replace_dialog(
                                        this.owner.clone(),
                                        this.owner_window,
                                        project,
                                        None,
                                        "proxy-http-mr",
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
                                    && let Some(project) = this.state.project
                                {
                                    if let Some(prev) = this.proxy_advanced.http_dialog.take() {
                                        let _ = prev.update(cx, |_, window, _| window.remove_window());
                                    }
                                    this.proxy_advanced.http_dialog =
                                        crate::workbench::tools_extra::open_http_match_replace_dialog(
                                            this.owner.clone(),
                                            this.owner_window,
                                            project,
                                            Some(rule.clone()),
                                            "proxy-http-mr",
                                            window,
                                            cx,
                                        )
                                        .ok();
                                }
                            }
                            2 => {
                                let _ = this.owner.update(cx, |view, cx| {
                                    view.remove_http_match_replace_rule(id, cx);
                                });
                                cx.notify();
                            }
                            3 => {
                                let _ = this.owner.update(cx, |view, cx| {
                                    view.move_http_match_replace_rule(id, false, cx);
                                });
                            }
                            4 => {
                                let _ = this.owner.update(cx, |view, cx| {
                                    view.move_http_match_replace_rule(id, true, cx);
                                });
                            }
                            _ => {}
                        }
                    })),
            );
        }

        let scope_owner = self.owner.clone();
        let project = self.state.project;
        let in_scope_only = self.state.proxy_messages.match_replace_in_scope_only;
        div()
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(settings_section_heading(
                "proxy-http-match-replace-heading",
                "HTTP 匹配与替换规则",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "按顺序自动匹配并改写流经 Proxy 的 HTTP 请求头、请求体、响应头与响应体。",
            ))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        adv_checkbox(
                            "proxy-http-mr-in-scope-only",
                            "仅应用于目标范围内的项目",
                            in_scope_only,
                            self.state.enabled,
                        )
                        .on_change(move |state, _, _, cx| {
                            let checked = state == CheckboxState::Checked;
                            let _ = scope_owner.update(cx, |view, cx| {
                                view.set_proxy_message_flag(
                                    project,
                                    |m| m.match_replace_in_scope_only = checked,
                                    cx,
                                );
                            });
                        }),
                    )
                    .child(
                        div()
                            .id("proxy-http-mr-table-wrap")
                            .flex()
                            .gap(px(9.))
                            .h(theme::scaled_px(168.).max(px(168.)))
                            .child(actions)
                            .child(settings_table_frame(
                                DataTable::new(&self.proxy_advanced.http_table)
                                    .with_size(px(22.))
                                    .header_row_height(px(24.))
                                    .bordered(false)
                                    .stripe(false)
                                    .scrollbar_visible(true, false)
                                    .scroll_bounce(false)
                                    .accessibility_label("HTTP 匹配与替换规则"),
                                true,
                            )),
                    ),
            )
    }

    fn render_ssl_passthrough_card(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let enabled = self.state.enabled && !self.proxy_advanced.ssl_editor_open;
        let selected_id = self.proxy_advanced.ssl_selected;
        let selected = selected_id.and_then(|id| {
            self.proxy_advanced
                .ssl_rules
                .iter()
                .position(|rule| rule.id == id)
        });
        let ssl_rules = self.proxy_advanced.ssl_rules.clone();
        let project = self.state.project;

        let mut actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            ("proxy-ssl-pt-add", "添加", "新增 TLS 透传目标", enabled, 0u8),
            ("proxy-ssl-pt-edit", "编辑", "编辑选中的 TLS 透传目标", enabled && selected.is_some(), 1u8),
            ("proxy-ssl-pt-remove", "移除", "移除选中的 TLS 透传目标", enabled && selected.is_some(), 2u8),
            ("proxy-ssl-pt-paste-url", "粘贴 URL", "从剪贴板解析 URL 并添加主机与端口", enabled, 3u8),
            ("proxy-ssl-pt-clear", "清空", "清空全部 TLS 透传规则", enabled && !ssl_rules.is_empty(), 4u8),
        ] {
            let rule = selected.and_then(|i| ssl_rules.get(i)).cloned();
            actions = actions.child(
                settings_action_button(id, label, !available)
                    .w_full()
                    .tooltip(tip)
                    .tab_stop(false)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action == 0 {
                            this.proxy_advanced.ssl_editor_open = true;
                            this.proxy_advanced.ssl_editing = None;
                            this.proxy_advanced.ssl_error = None;
                            this.proxy_advanced.ssl_is_regex = false;
                            for input in [
                                &this.proxy_advanced.ssl_host,
                                &this.proxy_advanced.ssl_port,
                            ] {
                                input.update(cx, |s, cx| s.set_value("", window, cx));
                            }
                            cx.notify();
                            return;
                        }
                        if action == 3 {
                            this.paste_ssl_pass_through_url(window, cx);
                            return;
                        }
                        if action == 4 {
                            this.proxy_advanced.ssl_editing = None;
                            this.proxy_advanced.ssl_editor_open = false;
                            this.proxy_advanced.ssl_error = None;
                            let _ = this.owner.update(cx, |view, cx| {
                                let _ = view.update_proxy_advanced(
                                    project,
                                    |ssl, _, _| ssl.clear(),
                                    cx,
                                );
                            });
                            cx.notify();
                            return;
                        }
                        let Some(id) = selected_id else {
                            return;
                        };
                        match action {
                            1 => {
                                if let Some(rule) = &rule {
                                    this.load_ssl_rule_into_editor(rule, window, cx);
                                }
                            }
                            2 => {
                                if this.proxy_advanced.ssl_editing == Some(id) {
                                    this.proxy_advanced.ssl_editing = None;
                                    this.proxy_advanced.ssl_editor_open = false;
                                }
                                let _ = this.owner.update(cx, |view, cx| {
                                    let _ = view.update_proxy_advanced(
                                        project,
                                        |ssl, _, _| ssl.retain(|r| r.id != id),
                                        cx,
                                    );
                                });
                                cx.notify();
                            }
                            _ => {}
                        }
                    })),
            );
        }

        let regex_entity = cx.entity();
        let ssl_auto_owner = self.owner.clone();
        let ssl_auto_add = self.state.proxy_messages.ssl_pass_through_auto_add_on_failure;
        div()
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(settings_section_heading(
                "proxy-ssl-passthrough-heading",
                "TLS 透传",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "匹配的 CONNECT 目标跳过动态证书签发，直接透传端到端原生 TLS 字节流。",
            ))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        adv_checkbox(
                            "proxy-ssl-pt-auto-add",
                            "客户端 TLS 协商失败时自动添加条目",
                            ssl_auto_add,
                            self.state.enabled,
                        )
                        .on_change(move |state, _, _, cx| {
                            let checked = state == CheckboxState::Checked;
                            let _ = ssl_auto_owner.update(cx, |view, cx| {
                                view.set_proxy_message_flag(
                                    project,
                                    |m| m.ssl_pass_through_auto_add_on_failure = checked,
                                    cx,
                                );
                            });
                        }),
                    )
                    .child(
                        div()
                            .id("proxy-ssl-pt-table-wrap")
                            .flex()
                            .gap(px(9.))
                            .h(theme::scaled_px(136.).max(px(136.)))
                            .child(actions)
                            .child(settings_table_frame(
                                DataTable::new(&self.proxy_advanced.ssl_table)
                                    .with_size(px(22.))
                                    .header_row_height(px(24.))
                                    .bordered(false)
                                    .stripe(false)
                                    .scrollbar_visible(true, false)
                                    .scroll_bounce(false)
                                    .accessibility_label("TLS 透传"),
                                true,
                            )),
                    ),
            )
            .when(self.proxy_advanced.ssl_editor_open, |this| {
                this.child(
                    div()
                        .id("proxy-ssl-pt-editor")
                        .test_support()
                        .ml(px(26.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(10.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::background())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().w(theme::scaled_px(110.).max(px(110.))).child("主机 / IP 范围："))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&self.proxy_advanced.ssl_host)
                                            .id("proxy-ssl-pt-host-input")
                                            .aria_label("TLS 透传主机模式")
                                            .small(),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().w(theme::scaled_px(110.).max(px(110.))).child("端口："))
                                .child(
                                    div().w(theme::scaled_px(140.).max(px(140.))).child(
                                        Input::new(&self.proxy_advanced.ssl_port)
                                            .id("proxy-ssl-pt-port-input")
                                            .aria_label("TLS 透传端口")
                                            .small(),
                                    ),
                                ),
                        )
                        .child(
                            adv_checkbox(
                                "proxy-ssl-pt-regex",
                                "正则匹配",
                                self.proxy_advanced.ssl_is_regex,
                                true,
                            )
                            .on_change(move |state, _, _, cx| {
                                regex_entity.update(cx, |this, cx| {
                                    this.proxy_advanced.ssl_is_regex = state == CheckboxState::Checked;
                                    cx.notify();
                                });
                            }),
                        )
                        .when_some(self.proxy_advanced.ssl_error.clone(), |this, error| {
                            this.child(
                                div()
                                    .id("proxy-ssl-pt-error")
                                    .test_support()
                                    .role(Role::Alert)
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
                                    button("proxy-ssl-pt-submit", "确定")
                                        .primary()
                                        .tab_stop(false)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_ssl_pass_through(window, cx)
                                        })),
                                )
                                .child(
                                    settings_action_button("proxy-ssl-pt-cancel", "取消", false)
                                        .tab_stop(false)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.proxy_advanced.ssl_editing = None;
                                            this.proxy_advanced.ssl_editor_open = false;
                                            this.proxy_advanced.ssl_is_regex = false;
                                            this.proxy_advanced.ssl_error = None;
                                            this.proxy_advanced
                                                .ssl_host
                                                .update(cx, |s, cx| s.set_value("", window, cx));
                                            this.proxy_advanced
                                                .ssl_port
                                                .update(cx, |s, cx| s.set_value("", window, cx));
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
    }
}
