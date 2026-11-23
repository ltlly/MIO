use gpui_kit::component::select::Select;

use super::*;

pub(crate) struct WsMatchReplaceTable {
    pub(super) owner: WeakEntity<Settings>,
}

const WS_MR_COLUMNS: [(&str, f32); 6] = [
    ("Enabled", 66.),
    ("Direction", 126.),
    ("Match", 192.),
    ("Replace", 192.),
    ("Regex", 80.),
    ("Comment", 160.),
];

impl TableDelegate for WsMatchReplaceTable {
    fn columns_count(&self, _: &App) -> usize {
        WS_MR_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), WS_MR_COLUMNS[index].0)
            .width((WS_MR_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).proxy_advanced.ws_rules.len())
    }
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(WS_MR_COLUMNS[col_ix].0, WS_MR_COLUMNS[col_ix].1)
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
        div().id(("proxy-ws-mr-row", row)).border_b_0()
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
            let Some(rule) = settings.proxy_advanced.ws_rules.get(row).cloned() else {
                return div().into_any_element();
            };
            (rule, settings.state.enabled, settings.state.project)
        };
        match column {
            0 => {
                let id = rule.id;
                let wb = owner.read(cx).owner.clone();
                settings_table_checkbox_cell(
                    adv_checkbox(("proxy-ws-mr-toggle", row), "", rule.enabled, enabled)
                        .p_0()
                        .gap_0()
                        .accessibility_label(format!(
                            "启用 WebSocket 规则 {}",
                            rule.match_pattern
                        ))
                        .on_change(move |state, _, _, cx| {
                            let checked = state == CheckboxState::Checked;
                            let _ = wb.update(cx, |view, cx| {
                                let _ = view.update_proxy_advanced(
                                    project,
                                    |_, ws, _| {
                                        if let Some(r) = ws.iter_mut().find(|r| r.id == id) {
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
            1 => div()
                .px(px(6.))
                .truncate()
                .child(ws_direction_label(rule.direction))
                .into_any_element(),
            2 => div().px(px(6.)).truncate().child(rule.match_pattern).into_any_element(),
            3 => div().px(px(6.)).truncate().child(rule.replace_with).into_any_element(),
            4 => div()
                .px(px(6.))
                .child(if rule.is_regex { "Yes" } else { "No" })
                .into_any_element(),
            _ => div().px(px(6.)).truncate().child(rule.comment).into_any_element(),
        }
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let rule = owner.read(cx).proxy_advanced.ws_rules.get(row)?;
        Some(match column {
            0 => if rule.enabled { "Yes" } else { "No" }.into(),
            1 => ws_direction_label(rule.direction).into(),
            2 => rule.match_pattern.clone().into(),
            3 => rule.replace_with.clone().into(),
            4 => if rule.is_regex { "Yes" } else { "No" }.into(),
            _ => rule.comment.clone().into(),
        })
    }
}

impl Settings {
    pub(super) fn render_ws_match_replace_card(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let enabled = self.state.enabled && !self.proxy_advanced.ws_editor_open;
        let project = self.state.project;
        let selected_id = self.proxy_advanced.ws_selected;
        let selected = selected_id.and_then(|id| {
            self.proxy_advanced
                .ws_rules
                .iter()
                .position(|rule| rule.id == id)
        });
        let ws_len = self.proxy_advanced.ws_rules.len();
        let ws_rules = self.proxy_advanced.ws_rules.clone();

        let mut actions = div()
            .w(theme::scaled_px(84.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            (
                "proxy-ws-mr-add",
                "添加",
                "新增 WebSocket 匹配替换规则",
                enabled,
                0u8,
            ),
            (
                "proxy-ws-mr-edit",
                "编辑",
                "编辑选中的 WebSocket 匹配替换规则",
                enabled && selected.is_some(),
                1u8,
            ),
            (
                "proxy-ws-mr-remove",
                "移除",
                "移除选中的 WebSocket 匹配替换规则",
                enabled && selected.is_some(),
                2u8,
            ),
            (
                "proxy-ws-mr-up",
                "上移",
                "上移选中的 WebSocket 匹配替换规则",
                enabled && selected.is_some_and(|i| i > 0),
                3u8,
            ),
            (
                "proxy-ws-mr-down",
                "下移",
                "下移选中的 WebSocket 匹配替换规则",
                enabled && selected.is_some_and(|i| i + 1 < ws_len),
                4u8,
            ),
        ] {
            let rule = selected.and_then(|i| ws_rules.get(i)).cloned();
            actions = actions.child(
                settings_action_button(id, label, !available)
                    .w_full()
                    .tooltip(tip)
                    .tab_stop(false)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action == 0 {
                            this.proxy_advanced.ws_editor_open = true;
                            this.proxy_advanced.ws_editing = None;
                            this.proxy_advanced.ws_error = None;
                            this.proxy_advanced.ws_is_regex = false;
                            this.proxy_advanced.ws_direction = WsReplaceDirection::Both;
                            this.proxy_advanced.ws_direction_select.update(cx, |s, cx| {
                                s.set_selected_index(
                                    Some(gpui_kit::component::IndexPath::new(0)),
                                    window,
                                    cx,
                                );
                            });
                            for input in [
                                &this.proxy_advanced.ws_match,
                                &this.proxy_advanced.ws_replace,
                                &this.proxy_advanced.ws_comment,
                            ] {
                                input.update(cx, |s, cx| s.set_value("", window, cx));
                            }
                            cx.notify();
                            return;
                        }
                        let Some(id) = selected_id else {
                            return;
                        };
                        match action {
                            1 => {
                                if let Some(rule) = &rule {
                                    this.load_ws_rule_into_editor(rule, window, cx);
                                }
                            }
                            2 => {
                                if this.proxy_advanced.ws_editing == Some(id) {
                                    this.proxy_advanced.ws_editing = None;
                                }
                                let _ = this.owner.update(cx, |view, cx| {
                                    let _ = view.update_proxy_advanced(
                                        project,
                                        |_, ws, _| ws.retain(|r| r.id != id),
                                        cx,
                                    );
                                });
                                cx.notify();
                            }
                            3 => {
                                let _ = this.owner.update(cx, |view, cx| {
                                    let _ = view.update_proxy_advanced(
                                        project,
                                        |_, ws, _| {
                                            if let Some(pos) = ws.iter().position(|r| r.id == id)
                                                && pos > 0
                                            {
                                                ws.swap(pos, pos - 1);
                                            }
                                        },
                                        cx,
                                    );
                                });
                            }
                            4 => {
                                let _ = this.owner.update(cx, |view, cx| {
                                    let _ = view.update_proxy_advanced(
                                        project,
                                        |_, ws, _| {
                                            if let Some(pos) = ws.iter().position(|r| r.id == id)
                                                && pos + 1 < ws.len()
                                            {
                                                ws.swap(pos, pos + 1);
                                            }
                                        },
                                        cx,
                                    );
                                });
                            }
                            _ => {}
                        }
                    })),
            );
        }

        let regex_entity = cx.entity();
        let ws_scope_owner = self.owner.clone();
        let ws_in_scope_only = self.state.proxy_messages.ws_match_replace_in_scope_only;
        div()
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(settings_section_heading(
                "proxy-ws-match-replace-heading",
                "WebSocket match and replace rules",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "自动匹配并替换 WebSocket 文本帧内容（保留修改前原始帧 payload，安全跳过二进制帧）。",
            ))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        adv_checkbox(
                            "proxy-ws-mr-in-scope-only",
                            "Only apply to in-scope items",
                            ws_in_scope_only,
                            self.state.enabled,
                        )
                        .on_change(move |state, _, _, cx| {
                            let checked = state == CheckboxState::Checked;
                            let _ = ws_scope_owner.update(cx, |view, cx| {
                                view.set_proxy_message_flag(
                                    project,
                                    |msgs| msgs.ws_match_replace_in_scope_only = checked,
                                    cx,
                                );
                            });
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(9.))
                            .h(theme::scaled_px(148.).max(px(148.)))
                            .child(actions)
                            .child(settings_table_frame(
                                DataTable::new(&self.proxy_advanced.ws_table)
                                    .with_size(px(22.))
                                    .header_row_height(px(24.))
                                    .bordered(false)
                                    .stripe(false)
                                    .scrollbar_visible(true, false)
                                    .scroll_bounce(false)
                                    .accessibility_label("WebSocket match and replace rules"),
                                true,
                            )),
                    ),
            )
            .when(self.proxy_advanced.ws_editor_open, |this| {
                this.child(
                    div()
                        .id("proxy-ws-mr-editor")
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
                                .child(
                                    div()
                                        .w(theme::scaled_px(88.).max(px(88.)))
                                        .flex_shrink_0()
                                        .child("Direction:"),
                                )
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Select::new(&self.proxy_advanced.ws_direction_select)
                                            .id("proxy-ws-mr-direction-select")
                                            .small()
                                            .w_full(),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().w(theme::scaled_px(88.).max(px(88.))).child("Match:"))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&self.proxy_advanced.ws_match)
                                            .id("proxy-ws-mr-match-input")
                                            .aria_label("WebSocket 匹配模式")
                                            .small(),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().w(theme::scaled_px(88.).max(px(88.))).child("Replace:"))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&self.proxy_advanced.ws_replace)
                                            .id("proxy-ws-mr-replace-input")
                                            .aria_label("WebSocket 替换内容")
                                            .small(),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().w(theme::scaled_px(88.).max(px(88.))).child("Comment:"))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&self.proxy_advanced.ws_comment)
                                            .id("proxy-ws-mr-comment-input")
                                            .aria_label("WebSocket 规则备注")
                                            .small(),
                                    ),
                                ),
                        )
                        .child(
                            adv_checkbox(
                                "proxy-ws-mr-regex",
                                "Regex match",
                                self.proxy_advanced.ws_is_regex,
                                true,
                            )
                            .on_change(move |state, _, _, cx| {
                                regex_entity.update(cx, |this, cx| {
                                    this.proxy_advanced.ws_is_regex = state == CheckboxState::Checked;
                                    cx.notify();
                                });
                            }),
                        )
                        .when_some(self.proxy_advanced.ws_error.clone(), |this, error| {
                            this.child(
                                div()
                                    .id("proxy-ws-mr-error")
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
                                    button("proxy-ws-mr-submit", "OK")
                                        .primary()
                                        .tab_stop(false)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_ws_match_replace(window, cx)
                                        })),
                                )
                                .child(
                                    settings_action_button("proxy-ws-mr-cancel", "Cancel", false)
                                        .tab_stop(false)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.proxy_advanced.ws_editing = None;
                                            this.proxy_advanced.ws_editor_open = false;
                                            this.proxy_advanced.ws_is_regex = false;
                                            this.proxy_advanced.ws_error = None;
                                            for input in [
                                                &this.proxy_advanced.ws_match,
                                                &this.proxy_advanced.ws_replace,
                                                &this.proxy_advanced.ws_comment,
                                            ] {
                                                input.update(cx, |s, cx| s.set_value("", window, cx));
                                            }
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
    }

    pub(super) fn render_response_modification_card(&self, window: &Window) -> gpui_kit::Div {
        let enabled = self.state.enabled;
        let mods = self.state.proxy_messages.response_modification;
        let mut checks = div().pl(px(31.)).flex().flex_col().items_start().gap(px(2.));
        for (idx, (id, label, checked)) in [
            (
                "proxy-resp-mod-unhide-hidden",
                "Unhide hidden form fields",
                mods.unhide_hidden_form_fields,
            ),
            (
                "proxy-resp-mod-highlight-unhidden",
                "Prominently highlight unhidden fields",
                mods.highlight_unhidden_fields,
            ),
            (
                "proxy-resp-mod-enable-disabled",
                "Enable disabled form fields",
                mods.enable_disabled_form_fields,
            ),
            (
                "proxy-resp-mod-remove-length-limits",
                "Remove input field length limits",
                mods.remove_input_field_length_limits,
            ),
            (
                "proxy-resp-mod-remove-js-validation",
                "Remove JavaScript form validation",
                mods.remove_javascript_form_validation,
            ),
            (
                "proxy-resp-mod-remove-all-js",
                "Remove all JavaScript",
                mods.remove_all_javascript,
            ),
            (
                "proxy-resp-mod-remove-object-tags",
                "Remove <object> tags",
                mods.remove_object_tags,
            ),
            (
                "proxy-resp-mod-convert-https-links",
                "Convert HTTPS links to HTTP",
                mods.convert_https_links_to_http,
            ),
            (
                "proxy-resp-mod-remove-cookie-secure",
                "Remove secure flag from cookies",
                mods.remove_secure_flag_from_cookies,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let focus = &self.proxy_advanced.resp_mod_focus[idx];
            let entity = self.owner.clone();
            let project = self.state.project;
            let row_enabled = if idx == 1 {
                enabled && mods.unhide_hidden_form_fields
            } else {
                enabled
            };
            checks = checks.child(
                div()
                    .when(idx == 1, |el| el.pl(px(20.)))
                    .child(
                        settings_checkbox(id, label, checked, row_enabled, focus, window)
                            .tab_stop(false)
                            .on_change(move |state, _, _, cx| {
                                let next_checked = state == CheckboxState::Checked;
                                let _ = entity.update(cx, |view, cx| {
                                    let _ = view.update_proxy_advanced(
                                        project,
                                        |_, _, resp_mod| match idx {
                                            0 => resp_mod.unhide_hidden_form_fields = next_checked,
                                            1 => resp_mod.highlight_unhidden_fields = next_checked,
                                            2 => resp_mod.enable_disabled_form_fields = next_checked,
                                            3 => resp_mod.remove_input_field_length_limits = next_checked,
                                            4 => resp_mod.remove_javascript_form_validation = next_checked,
                                            5 => resp_mod.remove_all_javascript = next_checked,
                                            6 => resp_mod.remove_object_tags = next_checked,
                                            7 => resp_mod.convert_https_links_to_http = next_checked,
                                            _ => resp_mod.remove_secure_flag_from_cookies = next_checked,
                                        },
                                        cx,
                                    );
                                });
                            }),
                    ),
            );
        }
        div()
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(settings_section_heading(
                "proxy-response-modification-heading",
                "Response modification",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "对 HTML 响应执行前端表单调试辅助变换（原始响应仍保留在 Original 视图中）。",
            ))
            .child(checks)
    }

    pub(super) fn render_proxy_misc_extended_card(&self, window: &Window) -> gpui_kit::Div {
        let enabled = self.state.enabled;
        let msgs = self.state.proxy_messages;
        let misc = msgs.misc;
        let project = self.state.project;
        let mut checks = div().pl(px(31.)).flex().flex_col().items_start().gap(px(2.));
        for (response, id, label, checked) in [
            (
                false,
                "proxy-unpack-requests",
                "解包压缩请求",
                msgs.unpack_compressed_requests,
            ),
            (
                true,
                "proxy-unpack-responses",
                "解包压缩响应",
                msgs.unpack_compressed_responses,
            ),
        ] {
            let owner = self.owner.clone();
            checks = checks.child(
                settings_checkbox(
                    id,
                    label,
                    checked,
                    enabled,
                    &self.proxy_message_focus[usize::from(response)],
                    window,
                )
                .on_change(move |state, _, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.set_proxy_unpack(
                            project,
                            response,
                            state == CheckboxState::Checked,
                            cx,
                        );
                    });
                }),
            );
        }
        for (idx, (id, label, checked)) in [
            (
                "proxy-misc-http10-requests",
                "Use HTTP/1.0 in requests to server",
                misc.use_http10_in_requests,
            ),
            (
                "proxy-misc-http10-responses",
                "Use HTTP/1.0 in responses to client",
                misc.use_http10_in_responses,
            ),
            (
                "proxy-misc-http1-keep-alive",
                "Use keep-alive for HTTP/1 if the server supports it",
                misc.use_http1_keep_alive,
            ),
            (
                "proxy-misc-resp-connection-close",
                "Set response header \"Connection: close\"",
                misc.set_response_connection_close,
            ),
            (
                "proxy-misc-req-connection-header",
                "Set \"Connection\" header on incoming requests when using HTTP/1",
                misc.set_request_connection_header,
            ),
            (
                "proxy-misc-strip-proxy-headers",
                "Strip Proxy-* headers in incoming requests",
                misc.strip_proxy_headers,
            ),
            (
                "proxy-misc-remove-unsupported-encodings",
                "Remove unsupported encodings from Accept-Encoding headers in incoming requests",
                misc.remove_unsupported_accept_encodings,
            ),
            (
                "proxy-misc-strip-ws-extensions",
                "Strip Sec-WebSocket-Extensions headers in incoming requests",
                misc.strip_sec_websocket_extensions,
            ),
            (
                "proxy-misc-disable-web-interface",
                "Disable web interface at http://mio",
                misc.disable_web_interface,
            ),
            (
                "proxy-misc-suppress-browser-errors",
                "Suppress MIO error messages in browser",
                misc.suppress_browser_error_messages,
            ),
            (
                "proxy-misc-disable-history-logging",
                "Don't send items to Proxy history or live tasks",
                misc.disable_history_logging,
            ),
            (
                "proxy-misc-disable-out-of-scope-logging",
                "Don't send items to Proxy history or live tasks, if out of scope",
                misc.disable_out_of_scope_history_logging,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let owner = self.owner.clone();
            checks = checks.child(
                adv_checkbox(id, label, checked, enabled).on_change(move |state, _, _, cx| {
                    let next = state == CheckboxState::Checked;
                    let _ = owner.update(cx, |view, cx| {
                        view.set_proxy_message_flag(
                            project,
                            |m| match idx {
                                0 => m.misc.use_http10_in_requests = next,
                                1 => m.misc.use_http10_in_responses = next,
                                2 => m.misc.use_http1_keep_alive = next,
                                3 => m.misc.set_response_connection_close = next,
                                4 => m.misc.set_request_connection_header = next,
                                5 => m.misc.strip_proxy_headers = next,
                                6 => m.misc.remove_unsupported_accept_encodings = next,
                                7 => m.misc.strip_sec_websocket_extensions = next,
                                8 => m.misc.disable_web_interface = next,
                                9 => m.misc.suppress_browser_error_messages = next,
                                10 => m.misc.disable_history_logging = next,
                                _ => m.misc.disable_out_of_scope_history_logging = next,
                            },
                            cx,
                        );
                    });
                }),
            );
        }
        div()
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(settings_section_heading(
                "proxy-miscellaneous-heading",
                "Miscellaneous",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "控制 Proxy 的压缩解包、HTTP 版本协商、连接头处理、头部剥离与历史记录策略。",
            ))
            .child(checks)
    }
}
