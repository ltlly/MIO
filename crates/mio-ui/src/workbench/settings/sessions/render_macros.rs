use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;

use super::render_rules::uniform_sessions_button;
use super::*;

impl Settings {
    pub(super) fn render_session_macros_card(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.state.enabled;
        let has_sel = self.sessions.selected_macro.is_some();
        let is_retry = matches!(
            self.sessions.macro_failure_policy,
            MacroFailurePolicy::Retry { .. }
        );
        let ext_kind = self.sessions.ext_kind;
        let tgt_kind = self.sessions.ext_target_kind;
        let e_abort = cx.entity();
        let e_retry = cx.entity();
        let e_jar = cx.entity();

        let mut steps_summary = div().flex().flex_col().gap(px(4.));
        let step_len = self.sessions.draft_steps.len();
        for (idx, step) in self.sessions.draft_steps.iter().enumerate() {
            steps_summary = steps_summary.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(format!(
                        "步骤 {}: {} {} (提取 {} 项)",
                        idx + 1,
                        step.method,
                        step.url,
                        step.extractions.len()
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                settings_action_button(
                                    ("settings-macro-draft-step-up", idx),
                                    "↑",
                                    !enabled || idx == 0,
                                )
                                .tooltip("上移此 Macro 请求步骤")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if idx > 0 && idx < this.sessions.draft_steps.len() {
                                        this.sessions.draft_steps.swap(idx, idx - 1);
                                        cx.notify();
                                    }
                                })),
                            )
                            .child(
                                settings_action_button(
                                    ("settings-macro-draft-step-down", idx),
                                    "↓",
                                    !enabled || idx + 1 >= step_len,
                                )
                                .tooltip("下移此 Macro 请求步骤")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if idx + 1 < this.sessions.draft_steps.len() {
                                        this.sessions.draft_steps.swap(idx, idx + 1);
                                        cx.notify();
                                    }
                                })),
                            )
                            .child(
                                settings_action_button(
                                    ("settings-macro-draft-step-del", idx),
                                    "移除",
                                    !enabled,
                                )
                                .tooltip("从当前草稿 Macro 中移除此请求步骤")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if idx < this.sessions.draft_steps.len() {
                                        this.sessions.draft_steps.remove(idx);
                                        cx.notify();
                                    }
                                })),
                            ),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(sessions_section_heading("settings-macros-heading", "宏 (Macros)"))
            .child(sessions_desc_row(
                "Macro 是一组按顺序执行的 HTTP 请求序列。您可以在会话处理规则中使用 Macro 来执行登录应用、获取反 CSRF 令牌等任务。使用以下设置管理您的 Macro。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_stretch()
                            .gap(px(8.))
                            .h(theme::scaled_px(130.).max(px(130.)))
                            .child(
                                div()
                                    .w(theme::scaled_px(88.).max(px(84.)))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .child(
                                        uniform_sessions_button(
                                            "settings-macro-add",
                                            "添加",
                                            !enabled,
                                        )
                                        .w_full()
                                        .tooltip("添加新的会话 Macro")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_new_macro_editor(window, cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-macro-edit",
                                            "编辑",
                                            !enabled || !has_sel,
                                        )
                                        .w_full()
                                        .tooltip("编辑当前选中的已保存 Macro")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.load_selected_macro_for_edit(window, cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-macro-remove",
                                            "移除",
                                            !enabled || !has_sel,
                                        )
                                        .w_full()
                                        .tooltip("移除当前选中的已保存 Macro")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.remove_selected_macro(cx)
                                        })),
                                    ),
                            )
                            .child({
                                let dark = theme::is_dark();
                                let border = rgb(if dark { 0x464a52 } else { 0xd0d0d0 });
                                let zebra = rgb(if dark { 0x282b30 } else { 0xf4f5f7 });
                                let selected_bg = rgb(if dark { 0x2e436e } else { 0xd0def0 });
                                let mut list = div()
                                    .id("settings-session-macros-list")
                                    .test_support()
                                    .role(Role::List)
                                    .aria_label("会话 Macro 列表")
                                    .w(theme::scaled_px(692.).max(px(692.)))
                                    .max_w_full()
                                    .h_full()
                                    .border_1()
                                    .border_color(border)
                                    .rounded(px(0.))
                                    .bg(theme::surface())
                                    .overflow_y_scroll()
                                    .flex()
                                    .flex_col();
                                for (row, mac) in self.state.sessions.macros.iter().enumerate() {
                                    let mac_id = mac.id;
                                    let is_selected = self.sessions.selected_macro == Some(mac_id);
                                    let row_bg = if is_selected {
                                        selected_bg.into()
                                    } else if row % 2 == 1 {
                                        zebra.into()
                                    } else {
                                        theme::surface()
                                    };
                                    let label = format!("{} ({} 步)", mac.name, mac.steps.len());
                                    list = list.child(
                                        div()
                                            .id(("settings-session-macro-row", row))
                                            .test_support()
                                            .role(Role::Row)
                                            .aria_label(mac.name.clone())
                                            .aria_selected(is_selected)
                                            .h(theme::scaled_px(22.).max(px(22.)))
                                            .flex_shrink_0()
                                            .px(px(8.))
                                            .flex()
                                            .items_center()
                                            .bg(row_bg)
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.sessions.selected_macro = Some(mac_id);
                                                cx.notify();
                                            }))
                                            .child(div().truncate().child(label)),
                                    );
                                }
                                list
                            }),
                    )
                    .when(self.sessions.macro_editor_open, |this| {
                        this.child(
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
                                            div().w(theme::scaled_px(200.).max(px(200.))).child(
                                                Input::new(&self.sessions.macro_name)
                                                    .id("settings-macro-name")
                                                    .aria_label("Macro 名称")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            settings_radio(
                                                "settings-macro-policy-abort",
                                                "立即中止拒绝",
                                                !is_retry,
                                                enabled,
                                                window,
                                                cx,
                                            )
                                            .on_change(move |_, _, _, cx| {
                                                e_abort.update(cx, |this, cx| {
                                                    this.sessions.macro_failure_policy =
                                                        MacroFailurePolicy::Abort;
                                                    cx.notify();
                                                });
                                            }),
                                        )
                                        .child(
                                            settings_radio(
                                                "settings-macro-policy-retry",
                                                "重试后拒绝",
                                                is_retry,
                                                enabled,
                                                window,
                                                cx,
                                            )
                                            .on_change(move |_, _, _, cx| {
                                                e_retry.update(cx, |this, cx| {
                                                    this.sessions.macro_failure_policy =
                                                        MacroFailurePolicy::Retry {
                                                            max_retries: 1,
                                                        };
                                                    cx.notify();
                                                });
                                            }),
                                        )
                                        .when(is_retry, |this| {
                                            this.child(
                                                div().w(theme::scaled_px(60.).max(px(60.))).child(
                                                    Input::new(&self.sessions.macro_retries)
                                                        .id("settings-macro-retries")
                                                        .aria_label("Macro 重试次数")
                                                        .small()
                                                        .disabled(!enabled),
                                                ),
                                            )
                                        })
                                        .child("超时(ms):")
                                        .child(
                                            div().w(theme::scaled_px(90.).max(px(90.))).child(
                                                Input::new(&self.sessions.macro_timeout_ms)
                                                    .id("settings-macro-timeout")
                                                    .aria_label("Macro 超时毫秒")
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
                                        .child(
                                            div().w(theme::scaled_px(75.).max(px(75.))).child(
                                                Input::new(&self.sessions.step_method)
                                                    .id("settings-macro-step-method")
                                                    .aria_label("步骤方法")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            div().w(theme::scaled_px(240.).max(px(240.))).child(
                                                Input::new(&self.sessions.step_url)
                                                    .id("settings-macro-step-url")
                                                    .aria_label("步骤 URL")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            div().w(theme::scaled_px(160.).max(px(160.))).child(
                                                Input::new(&self.sessions.step_headers)
                                                    .id("settings-macro-step-headers")
                                                    .aria_label("步骤请求头")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            div().w(theme::scaled_px(130.).max(px(130.))).child(
                                                Input::new(&self.sessions.step_body)
                                                    .id("settings-macro-step-body")
                                                    .aria_label("步骤正文")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            div().w(theme::scaled_px(85.).max(px(85.))).child(
                                                Input::new(&self.sessions.step_status)
                                                    .id("settings-macro-step-status")
                                                    .aria_label("预期状态码")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            inline_checkbox(
                                                "settings-macro-step-jar",
                                                "CookieJar",
                                                self.sessions.step_use_jar,
                                                enabled,
                                            )
                                            .on_change(move |st, _, _, cx| {
                                                e_jar.update(cx, |this, cx| {
                                                    this.sessions.step_use_jar =
                                                        st == CheckboxState::Checked;
                                                    cx.notify();
                                                });
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(
                                            div().w(theme::scaled_px(120.).max(px(120.))).child(
                                                Input::new(&self.sessions.ext_name)
                                                    .id("settings-macro-ext-name")
                                                    .aria_label("提取参数名")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .child(
                                            settings_pill_button(
                                                "settings-macro-ext-delim",
                                                "分隔符",
                                                ext_kind == DraftExtractKind::Delimiter,
                                            )
                                            .tooltip("按起止分隔符从步骤响应正文中提取参数值")
                                            .disabled(!enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.ext_kind =
                                                    DraftExtractKind::Delimiter;
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            settings_pill_button(
                                                "settings-macro-ext-regex",
                                                "正则",
                                                ext_kind == DraftExtractKind::Regex,
                                            )
                                            .tooltip("按正则表达式捕获组从步骤响应正文中提取参数值")
                                            .disabled(!enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.ext_kind = DraftExtractKind::Regex;
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            settings_pill_button(
                                                "settings-macro-ext-hdr",
                                                "响应头",
                                                ext_kind == DraftExtractKind::ResponseHeader,
                                            )
                                            .tooltip("从指定名称的 HTTP 响应头提取参数值")
                                            .disabled(!enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.ext_kind =
                                                    DraftExtractKind::ResponseHeader;
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            div().w(theme::scaled_px(140.).max(px(140.))).child(
                                                Input::new(&self.sessions.ext_arg1)
                                                    .id("settings-macro-ext-arg1")
                                                    .aria_label("提取条件1")
                                                    .small()
                                                    .disabled(!enabled),
                                            ),
                                        )
                                        .when(ext_kind == DraftExtractKind::Delimiter, |this| {
                                            this.child(
                                                div()
                                                    .w(theme::scaled_px(110.).max(px(110.)))
                                                    .child(
                                                        Input::new(&self.sessions.ext_arg2)
                                                            .id("settings-macro-ext-arg2")
                                                            .aria_label("结束分隔符")
                                                            .small()
                                                            .disabled(!enabled),
                                                    ),
                                            )
                                        })
                                        .child("回填:")
                                        .child(
                                            settings_pill_button(
                                                "settings-macro-tgt-param",
                                                "参数",
                                                tgt_kind == DraftTargetKind::UrlOrBodyParam,
                                            )
                                            .tooltip(
                                                "将提取值回填到后续请求的同名 URL 或表单参数",
                                            )
                                            .disabled(!enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.ext_target_kind =
                                                    DraftTargetKind::UrlOrBodyParam;
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            settings_pill_button(
                                                "settings-macro-tgt-hdr",
                                                "头",
                                                tgt_kind == DraftTargetKind::Header,
                                            )
                                            .tooltip("将提取值回填到后续请求的指定 HTTP 请求头")
                                            .disabled(!enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.ext_target_kind =
                                                    DraftTargetKind::Header;
                                                cx.notify();
                                            })),
                                        )
                                        .child(
                                            settings_pill_button(
                                                "settings-macro-tgt-ck",
                                                "Cookie",
                                                tgt_kind == DraftTargetKind::Cookie,
                                            )
                                            .tooltip("将提取值回填到后续请求的指定 Cookie 键")
                                            .disabled(!enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.ext_target_kind =
                                                    DraftTargetKind::Cookie;
                                                cx.notify();
                                            })),
                                        )
                                        .when(
                                            tgt_kind != DraftTargetKind::UrlOrBodyParam,
                                            |this| {
                                                this.child(
                                                    div()
                                                        .w(theme::scaled_px(110.).max(px(110.)))
                                                        .child(
                                                            Input::new(
                                                                &self.sessions.ext_target_name,
                                                            )
                                                            .id("settings-macro-ext-target")
                                                            .aria_label("目标头或Cookie名称")
                                                            .small()
                                                            .disabled(!enabled),
                                                        ),
                                                )
                                            },
                                        ),
                                )
                                .child(steps_summary)
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            sessions_mio_button(
                                                "sessions-macro-import-active",
                                                "从当前请求/历史载入步骤",
                                                !enabled,
                                            )
                                            .tooltip(
                                                "从当前选中 Proxy 历史记录或活动 Repeater 草稿导入请求作为步骤",
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.import_active_macro_step(window, cx)
                                            })),
                                        )
                                        .child(
                                            sessions_mio_button(
                                                "settings-macro-step-add",
                                                "添加步骤",
                                                !enabled,
                                            )
                                            .tooltip(
                                                "将上方填写的请求与提取规则追加到草稿步骤序列",
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.add_draft_macro_step(window, cx)
                                            })),
                                        )
                                        .child(
                                            sessions_mio_button(
                                                "settings-macro-save",
                                                "保存 Macro",
                                                !enabled,
                                            )
                                            .tooltip("保存当前多步会话 Macro 到项目设置")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.save_session_macro(window, cx)
                                            })),
                                        )
                                        .child(
                                            sessions_mio_button(
                                                "settings-macro-cancel",
                                                "取消",
                                                !enabled,
                                            )
                                            .tooltip("关闭 Macro 编辑器")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sessions.editing_macro_id = None;
                                                this.sessions.macro_editor_open = false;
                                                cx.notify();
                                            })),
                                        ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}
