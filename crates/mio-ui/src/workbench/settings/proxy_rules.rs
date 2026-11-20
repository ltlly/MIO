use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;
use mio_core::{ProxyInterceptionSettings, Uuid};

use super::*;

impl Settings {
    fn move_interception_rule(&mut self, response: bool, down: bool, cx: &mut Context<Self>) {
        let selected = self.interception_selected[usize::from(response)];
        let base = self.state.proxy_interception.clone();
        self.change_interception(
            self.state.project,
            response,
            &base,
            |set| {
                if let Some(index) =
                    selected.and_then(|id| set.rules.iter().position(|rule| rule.id == id))
                {
                    let destination = if down {
                        index
                            .checked_add(1)
                            .filter(|index| *index < set.rules.len())
                    } else {
                        index.checked_sub(1)
                    };
                    if let Some(destination) = destination {
                        set.rules.swap(index, destination);
                    }
                }
            },
            cx,
        );
    }

    fn remove_interception_rule(&mut self, response: bool, cx: &mut Context<Self>) {
        let selected = self.interception_selected[usize::from(response)];
        let base = self.state.proxy_interception.clone();
        self.change_interception(
            self.state.project,
            response,
            &base,
            |set| set.rules.retain(|rule| Some(rule.id) != selected),
            cx,
        );
    }

    fn restore_interception_defaults(
        &mut self,
        response: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.interception_available() {
            return;
        }
        let operation = Uuid::new_v4();
        let base = self.state.proxy_interception.clone();
        let project = self.state.project;
        self.interception_restore = Some(operation);
        let answer = window.prompt(
            PromptLevel::Warning,
            "恢复默认设置",
            Some(if response {
                "是否恢复 Response interception rules 默认设置？"
            } else {
                "是否恢复 Request interception rules 默认设置？"
            }),
            &["否", "是"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let accepted = answer.await.ok() == Some(1);
            let _ = this.update_in(cx, |this, _, cx| {
                if this.interception_restore != Some(operation) {
                    return;
                }
                this.interception_restore = None;
                if accepted && this.state.project == project {
                    let defaults = ProxyInterceptionSettings::default();
                    this.change_interception(
                        project,
                        response,
                        &base,
                        |set| *set = intercept_set(&defaults, response).clone(),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_proxy_rules(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.interception_available();
        let mut body = div()
            .px(px(22.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(self.render_proxy_listeners_section(window, cx));
        for response in [false, true] {
            let index = usize::from(response);
            let set = intercept_set(&self.state.proxy_interception, response);
            let selected = self.interception_selected[index]
                .and_then(|id| set.rules.iter().position(|rule| rule.id == id));
            let name = if response { "response" } else { "request" };
            let checkbox_id = if response {
                "proxy-response-rules-enabled"
            } else {
                "proxy-request-rules-enabled"
            };
            let base = self.state.proxy_interception.clone();
            let project = self.state.project;
            let change_owner = cx.weak_entity();
            let title = if response {
                "Response interception rules"
            } else {
                "Request interception rules"
            };
            let mut actions = div()
                .w(theme::scaled_px(84.).max(px(84.)))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(5.));
            let stage_zh = if response { "响应" } else { "请求" };
            for (action, label, tip, available) in [
                (
                    "add",
                    "添加",
                    format!("新增一条 Proxy {stage_zh}拦截规则"),
                    enabled,
                ),
                (
                    "edit",
                    "编辑",
                    format!("编辑当前选中的 Proxy {stage_zh}拦截规则"),
                    enabled && selected.is_some(),
                ),
                (
                    "remove",
                    "移除",
                    format!("移除当前选中的 Proxy {stage_zh}拦截规则"),
                    enabled && selected.is_some(),
                ),
                (
                    "up",
                    "上移",
                    format!("上移选中的 {stage_zh}拦截规则以提高求值顺序"),
                    enabled && selected.is_some_and(|index| index > 0),
                ),
                (
                    "down",
                    "下移",
                    format!("下移选中的 {stage_zh}拦截规则以延后求值顺序"),
                    enabled && selected.is_some_and(|index| index + 1 < set.rules.len()),
                ),
            ] {
                actions = actions.child(
                    settings_action_button(
                        SharedString::from(format!("proxy-{name}-rule-{action}")),
                        label,
                        !available,
                    )
                    .w_full()
                    .tooltip(tip)
                    .on_click(cx.listener(move |this, _, window, cx| match action {
                        "add" => this.open_interception_editor(response, true, window, cx),
                        "edit" => this.open_interception_editor(response, false, window, cx),
                        "remove" => this.remove_interception_rule(response, cx),
                        "up" => this.move_interception_rule(response, false, cx),
                        _ => this.move_interception_rule(response, true, cx),
                    })),
                );
            }
            let gear_btn = settings_gear_icon_button(
                SharedString::from(format!("proxy-{name}-rules-defaults")),
                if response {
                    "恢复 Proxy 响应拦截规则列表与开关为出厂默认配置"
                } else {
                    "恢复 Proxy 请求拦截规则列表与开关为出厂默认配置"
                },
                !enabled,
            )
            .tab_stop(false)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.restore_interception_defaults(response, window, cx)
            }));
            let master_off = !response && !self.state.intercept_requests;
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(9.))
                    .when(response, |this| {
                        this.pt(px(16.)).border_t_1().border_color(theme::border())
                    })
                    .child(settings_section_heading(
                        SharedString::from(format!("proxy-{name}-rules-heading")),
                        title,
                        project_scope_badge(),
                    ))
                    .child(settings_desc_row_with_action(
                        gear_btn,
                        if response {
                            "控制在 Intercept 中等待查看和编辑的响应。"
                        } else {
                            "控制在 Intercept 中等待查看和编辑的请求。"
                        },
                    ))
                    .child(
                        div()
                            .pl(px(31.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        settings_checkbox(
                                            checkbox_id,
                                            if response {
                                                "根据以下规则拦截响应："
                                            } else {
                                                "根据以下规则拦截请求："
                                            },
                                            set.enabled,
                                            enabled,
                                            &self.interception_focus[index],
                                            window,
                                        )
                                        .on_change(move |checked, _, _, cx| {
                                            let _ = change_owner.update(cx, |this, cx| {
                                                this.change_interception(
                                                    project,
                                                    response,
                                                    &base,
                                                    |set| {
                                                        set.enabled =
                                                            checked == CheckboxState::Checked
                                                    },
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .when(master_off, |this| {
                                        this.child(
                                            div()
                                                .italic()
                                                .text_color(rgb(0xc62828))
                                                .child("主拦截开关已关闭"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(9.))
                                    .h({
                                        let base = if response { 156. } else { 148. };
                                        theme::scaled_px(base).max(px(base))
                                    })
                                    .child(actions)
                                    .child(settings_table_frame(
                                        DataTable::new(&self.interception_tables[index])
                                            .with_size(px(22.))
                                            .header_row_height(px(24.))
                                            .bordered(false)
                                            .stripe(false)
                                            .scrollbar_visible(true, false)
                                            .scroll_bounce(false)
                                            .accessibility_label(title),
                                        true,
                                    )),
                            )
                            .when(!response, |this| {
                                let owner_cl = self.owner.clone();
                                let owner_nl = self.owner.clone();
                                let req_cl =
                                    self.state.proxy_messages.auto_update_request_content_length;
                                let req_nl = self.state.proxy_messages.auto_fix_request_newlines;
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap(px(2.))
                                        .child(
                                            super::proxy_advanced::adv_checkbox(
                                                "proxy-req-auto-fix-newlines",
                                                "Automatically fix missing or superfluous new lines at end of request",
                                                req_nl,
                                                self.state.enabled,
                                            )
                                            .on_change(move |state, _, _, cx| {
                                                let checked = state == CheckboxState::Checked;
                                                let _ = owner_nl.update(cx, |view, cx| {
                                                    view.set_proxy_message_flag(
                                                        project,
                                                        |msgs| {
                                                            msgs.auto_fix_request_newlines = checked
                                                        },
                                                        cx,
                                                    );
                                                });
                                            }),
                                        )
                                        .child(
                                            super::proxy_advanced::adv_checkbox(
                                                "proxy-req-auto-content-length",
                                                "Automatically update Content-Length header when the request is edited",
                                                req_cl,
                                                self.state.enabled,
                                            )
                                            .on_change(move |state, _, _, cx| {
                                                let checked = state == CheckboxState::Checked;
                                                let _ = owner_cl.update(cx, |view, cx| {
                                                    view.set_proxy_message_flag(
                                                        project,
                                                        |msgs| {
                                                            msgs.auto_update_request_content_length =
                                                                checked
                                                        },
                                                        cx,
                                                    );
                                                });
                                            }),
                                        ),
                                )
                            })
                            .when(response, |this| {
                                let owner_cl = self.owner.clone();
                                let resp_cl =
                                    self.state.proxy_messages.auto_update_response_content_length;
                                this.child(
                                    super::proxy_advanced::adv_checkbox(
                                        "proxy-resp-auto-content-length",
                                        "Automatically update Content-Length header when the response is edited",
                                        resp_cl,
                                        self.state.enabled,
                                    )
                                    .on_change(move |state, _, _, cx| {
                                        let checked = state == CheckboxState::Checked;
                                        let _ = owner_cl.update(cx, |view, cx| {
                                            view.set_proxy_message_flag(
                                                project,
                                                |msgs| {
                                                    msgs.auto_update_response_content_length =
                                                        checked
                                                },
                                                cx,
                                            );
                                        });
                                    }),
                                )
                            }),
                    ),
            );
        }
        let mut ws_checks = div().pl(px(31.)).flex().flex_col().items_start().gap(px(2.));
        for (index, (id, label, checked)) in [
            (
                "proxy-ws-intercept-client-to-server",
                "拦截客户端到服务端消息",
                self.state
                    .proxy_websocket_interception
                    .client_to_server_messages,
            ),
            (
                "proxy-ws-intercept-server-to-client",
                "拦截服务端到客户端消息",
                self.state
                    .proxy_websocket_interception
                    .server_to_client_messages,
            ),
            (
                "proxy-ws-intercept-in-scope-only",
                "仅拦截 Target Scope 内的消息",
                self.state
                    .proxy_websocket_interception
                    .intercept_in_scope_only,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let owner = self.owner.clone();
            let project = self.state.project;
            ws_checks = ws_checks.child(
                settings_checkbox(
                    id,
                    label,
                    checked,
                    self.state.enabled,
                    &self.proxy_ws_interception_focus[index],
                    window,
                )
                .on_change(move |state, _, _, cx| {
                    let checked = state == CheckboxState::Checked;
                    let _ = owner.update(cx, |view, cx| {
                        view.set_proxy_websocket_interception(
                            project,
                            |ws| match index {
                                0 => ws.client_to_server_messages = checked,
                                1 => ws.server_to_client_messages = checked,
                                _ => ws.intercept_in_scope_only = checked,
                            },
                            cx,
                        );
                    });
                }),
            );
        }
        let websocket_rules = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(9.))
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .child(settings_section_heading(
                "proxy-ws-rules-heading",
                "WebSocket interception rules",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "控制在 Intercept 中等待查看和编辑的 WebSocket 消息。",
            ))
            .child(ws_checks);
        body = body.child(websocket_rules);
        body = body.child(self.render_proxy_advanced_cards(window, cx));
        if let Some(error) = self
            .interception_error
            .clone()
            .or_else(|| self.state.error.clone())
        {
            body = body.child(
                div()
                    .id("proxy-interception-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .ml(px(31.))
                    .p(px(8.))
                    .rounded(px(4.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(error),
            );
        }
        if let Some(status) = self.state.status {
            body = body.child(
                div()
                    .id("proxy-interception-status")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(status)
                    .pl(px(31.))
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child(status),
            );
        }
        settings_scrollable_pane(
            SharedString::from(format!("settings-proxy-project-{:?}", self.state.project)),
            &self.content_scroll,
            body,
        )
        .into_any_element()
    }
}
