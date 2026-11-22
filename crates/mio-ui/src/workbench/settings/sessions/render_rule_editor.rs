use gpui_kit::base::CheckboxState;
use gpui_kit::component::select::Select;

use super::render_rules::describe_action;
use super::*;

impl Settings {
    pub(super) fn render_rule_editor_panel(
        &self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let jar_opts = window.use_keyed_state(
            "settings-sessions-jar-action-opts",
            cx,
            |_, _| (true, true),
        );
        let (jar_add_new, jar_update_existing) = *jar_opts.read(cx);

        let mut draft_actions_row = div().flex().flex_wrap().items_center().gap(px(6.));
        for (idx, act) in self.sessions.draft_actions.iter().enumerate() {
            let label = describe_action(act, &self.state.sessions.macros);
            draft_actions_row = draft_actions_row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(format!("{}. {label}", idx + 1))
                    .child(
                        settings_action_button(
                            ("settings-sessions-draft-act-del", idx),
                            "×",
                            !enabled || self.sessions.draft_actions.len() <= 1,
                        )
                        .tooltip("从当前草稿规则中移除此会话处理动作")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if idx < this.sessions.draft_actions.len()
                                && this.sessions.draft_actions.len() > 1
                            {
                                this.sessions.draft_actions.remove(idx);
                                cx.notify();
                            }
                        })),
                    ),
            );
        }

        let kind = self.sessions.draft_action_kind;
        let action_editor = match kind {
            DraftActionKind::UseCookieJar => {
                let opts_add = jar_opts.clone();
                let opts_upd = jar_opts.clone();
                let e_add = cx.entity();
                let e_upd = cx.entity();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child("Use cookies from MIO's cookie jar：从当前项目 Cookie Jar 匹配域、路径、Secure 与未过期条目注入请求。")
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                inline_checkbox(
                                    "settings-sessions-act-jar-add-new",
                                    "添加请求中缺失的新 Cookie",
                                    jar_add_new,
                                    enabled,
                                )
                                .on_change(move |st, _, _, cx| {
                                    let checked = st == CheckboxState::Checked;
                                    opts_add.update(cx, |o, cx| {
                                        o.0 = checked;
                                        cx.notify();
                                    });
                                    e_add.update(cx, |this, cx| {
                                        for act in &mut this.sessions.draft_actions {
                                            if let SessionRuleAction::UseCookieJar {
                                                add_new_cookies,
                                                ..
                                            } = act
                                            {
                                                *add_new_cookies = checked;
                                            }
                                        }
                                        cx.notify();
                                    });
                                }),
                            )
                            .child(
                                inline_checkbox(
                                    "settings-sessions-act-jar-update-existing",
                                    "更新请求中已有的 Cookie",
                                    jar_update_existing,
                                    enabled,
                                )
                                .on_change(move |st, _, _, cx| {
                                    let checked = st == CheckboxState::Checked;
                                    opts_upd.update(cx, |o, cx| {
                                        o.1 = checked;
                                        cx.notify();
                                    });
                                    e_upd.update(cx, |this, cx| {
                                        for act in &mut this.sessions.draft_actions {
                                            if let SessionRuleAction::UseCookieJar {
                                                update_existing_cookies,
                                                ..
                                            } = act
                                            {
                                                *update_existing_cookies = checked;
                                            }
                                        }
                                        cx.notify();
                                    });
                                }),
                            ),
                    )
                    .into_any_element()
            }
            DraftActionKind::SetHeader => {
                let entity = cx.entity();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(theme::scaled_px(160.).max(px(160.))).child(
                            Input::new(&self.sessions.action_header_name)
                                .id("settings-sessions-act-hdr-name")
                                .aria_label("Header 名称")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        div().w(theme::scaled_px(220.).max(px(220.))).child(
                            Input::new(&self.sessions.action_header_val)
                                .id("settings-sessions-act-hdr-val")
                                .aria_label("Header 值")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        inline_checkbox(
                            "settings-sessions-act-only-present",
                            "仅替换已有",
                            self.sessions.action_only_present,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.action_only_present = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        }),
                    )
                    .into_any_element()
            }
            DraftActionKind::SetCookie => {
                let entity = cx.entity();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(theme::scaled_px(160.).max(px(160.))).child(
                            Input::new(&self.sessions.action_cookie_name)
                                .id("settings-sessions-act-cookie-name")
                                .aria_label("Cookie 名称")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        div().w(theme::scaled_px(220.).max(px(220.))).child(
                            Input::new(&self.sessions.action_cookie_val)
                                .id("settings-sessions-act-cookie-val")
                                .aria_label("Cookie 值")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        inline_checkbox(
                            "settings-sessions-act-cookie-only-present",
                            "仅替换已有",
                            self.sessions.action_only_present,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.action_only_present = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        }),
                    )
                    .into_any_element()
            }
            DraftActionKind::RewriteUrlParam => {
                let entity = cx.entity();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(theme::scaled_px(150.).max(px(150.))).child(
                            Input::new(&self.sessions.action_param_name)
                                .id("settings-sessions-act-param-name")
                                .aria_label("URL 参数名")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        div().w(theme::scaled_px(180.).max(px(180.))).child(
                            Input::new(&self.sessions.action_param_val)
                                .id("settings-sessions-act-param-val")
                                .aria_label("URL 参数值")
                                .small()
                                .disabled(!enabled || self.sessions.action_param_remove),
                        ),
                    )
                    .child(
                        inline_checkbox(
                            "settings-sessions-act-param-remove",
                            "删除参数",
                            self.sessions.action_param_remove,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.action_param_remove = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        }),
                    )
                    .into_any_element()
            }
            DraftActionKind::ReplaceRequestText => {
                let entity = cx.entity();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(theme::scaled_px(190.).max(px(190.))).child(
                            Input::new(&self.sessions.action_replace_match)
                                .id("settings-sessions-act-replace-match")
                                .aria_label("匹配模式")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        div().w(theme::scaled_px(190.).max(px(190.))).child(
                            Input::new(&self.sessions.action_replace_with)
                                .id("settings-sessions-act-replace-with")
                                .aria_label("替换为")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child(
                        inline_checkbox(
                            "settings-sessions-act-replace-regex",
                            "Regex",
                            self.sessions.action_replace_regex,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.action_replace_regex = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        }),
                    )
                    .into_any_element()
            }
            DraftActionKind::RunMacro => {
                let has_macros = !self.state.sessions.macros.is_empty();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child("Target macro:")
                    .child(
                        div()
                            .w(theme::scaled_px(240.).max(px(240.)))
                            .flex_shrink_0()
                            .child(
                                Select::new(&self.sessions.action_macro_select)
                                    .id("settings-sessions-act-macro-select")
                                    .small()
                                    .w_full()
                                    .disabled(!enabled || !has_macros),
                            ),
                    )
                    .into_any_element()
            }
        };


        div()
            .p(px(10.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::background())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(theme::scaled_px(210.).max(px(210.))).child(
                            Input::new(&self.sessions.rule_name)
                                .id("settings-sessions-rule-name")
                                .aria_label("Session 规则名称")
                                .small()
                                .disabled(!enabled),
                        ),
                    )
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-rule-enabled",
                            "启用",
                            self.sessions.rule_enabled,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_enabled = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child("工具：")
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-proxy",
                            "Proxy",
                            self.sessions.rule_tools.proxy,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.proxy = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-repeater",
                            "Repeater",
                            self.sessions.rule_tools.repeater,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.repeater = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-intruder",
                            "Intruder",
                            self.sessions.rule_tools.intruder,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.intruder = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-scanner",
                            "Scanner",
                            self.sessions.rule_tools.scanner,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.scanner = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-sequencer",
                            "Sequencer",
                            self.sessions.rule_tools.sequencer,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.sequencer = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-extensions",
                            "Extensions",
                            self.sessions.rule_tools.extensions,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.extensions = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = cx.entity();
                        inline_checkbox(
                            "settings-sessions-tool-ai",
                            "MioAI",
                            self.sessions.rule_tools.mio_ai,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.sessions.rule_tools.mio_ai = st == CheckboxState::Checked;
                                cx.notify();
                            });
                        })
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child("URL scope:")
                    .child(
                        div()
                            .w(theme::scaled_px(230.).max(px(230.)))
                            .flex_shrink_0()
                            .child(
                                Select::new(&self.sessions.url_scope_select)
                                    .id("settings-sessions-url-scope-select")
                                    .small()
                                    .w_full()
                                    .disabled(!enabled),
                            ),
                    )
                    .when(self.sessions.rule_url_mode == 2, |this| {
                        this.child(
                            div().w(theme::scaled_px(210.).max(px(210.))).child(
                                Input::new(&self.sessions.rule_prefixes)
                                    .id("settings-sessions-rule-prefixes")
                                    .aria_label("自定义 URL 前缀")
                                    .small()
                                    .disabled(!enabled),
                            ),
                        )
                    })
                    .child(
                        div().w(theme::scaled_px(150.).max(px(150.))).child(
                            Input::new(&self.sessions.rule_req_param)
                                .id("settings-sessions-rule-req-param")
                                .aria_label("必需参数名")
                                .small()
                                .disabled(!enabled),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child("Action type:")
                    .child(
                        div()
                            .w(theme::scaled_px(290.).max(px(290.)))
                            .flex_shrink_0()
                            .child(
                                Select::new(&self.sessions.action_kind_select)
                                    .id("settings-sessions-action-kind-select")
                                    .small()
                                    .w_full()
                                    .disabled(!enabled),
                            ),
                    )
                    .child(
                        sessions_mio_button("settings-sessions-action-add", "加入动作", !enabled)
                            .tooltip("将当前编辑的会话动作追加到草稿规则动作链")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_draft_rule_action_with_jar_opts(
                                    jar_add_new,
                                    jar_update_existing,
                                    window,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(action_editor)
            .child(draft_actions_row)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        sessions_mio_button(
                            "settings-sessions-rule-save",
                            if self.sessions.editing_rule_id.is_some() {
                                "更新规则"
                            } else {
                                "保存规则"
                            },
                            !enabled,
                        )
                        .tooltip(if self.sessions.editing_rule_id.is_some() {
                            "保存对当前会话处理规则的修改"
                        } else {
                            "将当前草稿保存为新的会话处理规则"
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.save_session_rule(window, cx)),
                        ),
                    )
                    .when(self.sessions.editing_rule_id.is_none(), |this| {
                        let opts_tpl = jar_opts.clone();
                        this.child(
                            sessions_mio_button(
                                "settings-sessions-rule-default-template",
                                "套用默认 Cookie Jar 模板",
                                !enabled,
                            )
                            .tooltip("填充「Use cookies from MIO's cookie jar」默认规则模板")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                opts_tpl.update(cx, |o, _| *o = (true, true));
                                this.apply_default_cookie_jar_template(window, cx);
                            })),
                        )
                    })
                    .child(
                        sessions_mio_button("settings-sessions-rule-cancel", "取消", !enabled)
                            .tooltip("关闭会话处理规则编辑器")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sessions.editing_rule_id = None;
                                this.sessions.rule_editor_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}
