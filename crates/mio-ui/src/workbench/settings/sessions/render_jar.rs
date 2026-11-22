use gpui_kit::base::CheckboxState;
use mio_core::SessionTool;

use super::render_rules::uniform_sessions_button;
use super::*;

impl Settings {
    pub(super) fn render_cookie_jar_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.state.enabled;
        let jar = &self.state.sessions.cookie_jar;
        let has_sel = self.sessions.selected_cookie.is_some();
        let non_empty = !jar.entries.is_empty();
        let e1 = cx.entity();
        let e2 = cx.entity();
        let e3 = cx.entity();
        let e4 = cx.entity();
        let e5 = cx.entity();
        let e6 = cx.entity();
        let e7 = cx.entity();
        let e8 = cx.entity();
        let e9 = cx.entity();
        let e_ai = cx.entity();

        div()
            .pb(px(14.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(sessions_section_heading(
                "settings-cookie-jar-heading",
                "Cookie jar",
            ))
            .child(sessions_desc_row(
                "系统维护一个 Cookie Jar，用于保存已访问站点下发的所有 Cookie。会话处理规则可以使用并更新这些 Cookie，以在测试期间维持有效会话。使用以下设置控制如何根据各工具的流量自动更新 Cookie Jar。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(10.))
                    .child("使用以下设置控制哪些工具可以更新 Cookie Jar。")
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(82.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-proxy",
                                            "Proxy",
                                            jar.monitor_proxy,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e1.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::Proxy,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-repeater",
                                            "Repeater",
                                            jar.monitor_repeater,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e2.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::Repeater,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-sequencer",
                                            "Sequencer",
                                            jar.monitor_sequencer,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e8.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::Sequencer,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(78.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-scanner",
                                            "Scanner",
                                            jar.monitor_scanner,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e7.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::Scanner,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-ai",
                                            "MioAI",
                                            jar.monitor_mio_ai,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e_ai.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::MioAi,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w(px(78.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-intruder",
                                            "Intruder",
                                            jar.monitor_intruder,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e3.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::Intruder,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        inline_checkbox(
                                            "settings-cookie-jar-monitor-extensions",
                                            "Extensions",
                                            jar.monitor_extensions,
                                            enabled,
                                        )
                                        .on_change(move |st, _, _, cx| {
                                            e9.update(cx, |this, cx| {
                                                this.set_cookie_jar_monitor(
                                                    SessionTool::Extensions,
                                                    st == CheckboxState::Checked,
                                                    cx,
                                                );
                                            });
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        uniform_sessions_button(
                            "settings-cookie-jar-open",
                            "打开 Cookie Jar",
                            !enabled,
                        )
                        .px(px(14.))
                        .tooltip("展开或收起项目 Cookie Jar 条目管理面板")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sessions.cookie_jar_open = !this.sessions.cookie_jar_open;
                            cx.notify();
                        })),
                    )
                    .when(self.sessions.cookie_jar_open, |this| {
                        this.child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex()
                                        .items_stretch()
                                        .gap(px(8.))
                                        .h(theme::scaled_px(145.).max(px(145.)))
                                        .child(
                                            div()
                                                .w(theme::scaled_px(88.).max(px(84.)))
                                                .flex_shrink_0()
                                                .flex()
                                                .flex_col()
                                                .gap(px(5.))
                                                .child(
                                                    sessions_mio_button(
                                                        "settings-cookie-jar-edit",
                                                        "编辑",
                                                        !enabled || !has_sel,
                                                    )
                                                    .w_full()
                                                    .tooltip(
                                                        "将选中的 Cookie 条目载入下方表单进行修改",
                                                    )
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.load_selected_cookie_for_edit(
                                                                window, cx,
                                                            )
                                                        },
                                                    )),
                                                )
                                                .child(
                                                    sessions_mio_button(
                                                        "settings-cookie-jar-remove",
                                                        "移除",
                                                        !enabled || !has_sel,
                                                    )
                                                    .w_full()
                                                    .tooltip(
                                                        "从项目 Cookie Jar 中移除当前选中的条目",
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.remove_selected_cookie(cx)
                                                    })),
                                                )
                                                .child(
                                                    sessions_mio_button(
                                                        "settings-cookie-jar-clear",
                                                        "清空",
                                                        !enabled || !non_empty,
                                                    )
                                                    .w_full()
                                                    .tooltip(
                                                        "清空当前项目 Cookie Jar 中的全部 Cookie 条目",
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.clear_cookie_jar(cx)
                                                    })),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .w(theme::scaled_px(774.).max(px(774.)))
                                                .max_w_full()
                                                .h_full()
                                                .child(sessions_table_frame(
                                                    DataTable::new(&self.sessions.jar_table)
                                                        .with_size(
                                                            theme::scaled_px(22.).max(px(20.)),
                                                        )
                                                        .header_row_height(
                                                            theme::scaled_px(24.).max(px(22.)),
                                                        )
                                                        .bordered(false)
                                                        .scroll_bounce(false)
                                                        .accessibility_label("Cookie jar entries"),
                                                )),
                                        ),
                                )
                                .child(
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
                                                    div()
                                                        .w(theme::scaled_px(150.).max(px(150.)))
                                                        .child(
                                                            Input::new(&self.sessions.cookie_domain)
                                                                .id("settings-cookie-domain")
                                                                .aria_label("Cookie Domain")
                                                                .small()
                                                                .disabled(!enabled),
                                                        ),
                                                )
                                                .child(
                                                    inline_checkbox(
                                                        "settings-cookie-subdomains",
                                                        "包含子域",
                                                        self.sessions.cookie_subdomains,
                                                        enabled,
                                                    )
                                                    .on_change(move |st, _, _, cx| {
                                                        e4.update(cx, |this, cx| {
                                                            this.sessions.cookie_subdomains =
                                                                st == CheckboxState::Checked;
                                                            cx.notify();
                                                        });
                                                    }),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(110.).max(px(110.)))
                                                        .child(
                                                            Input::new(&self.sessions.cookie_path)
                                                                .id("settings-cookie-path")
                                                                .aria_label("Cookie Path")
                                                                .small()
                                                                .disabled(!enabled),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(140.).max(px(140.)))
                                                        .child(
                                                            Input::new(&self.sessions.cookie_name)
                                                                .id("settings-cookie-name")
                                                                .aria_label("Cookie Name")
                                                                .small()
                                                                .disabled(!enabled),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(180.).max(px(180.)))
                                                        .child(
                                                            Input::new(&self.sessions.cookie_value)
                                                                .id("settings-cookie-value")
                                                                .aria_label("Cookie Value")
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
                                                .gap(px(10.))
                                                .child(
                                                    inline_checkbox(
                                                        "settings-cookie-secure",
                                                        "Secure",
                                                        self.sessions.cookie_secure,
                                                        enabled,
                                                    )
                                                    .on_change(move |st, _, _, cx| {
                                                        e5.update(cx, |this, cx| {
                                                            this.sessions.cookie_secure =
                                                                st == CheckboxState::Checked;
                                                            cx.notify();
                                                        });
                                                    }),
                                                )
                                                .child(
                                                    inline_checkbox(
                                                        "settings-cookie-httponly",
                                                        "HttpOnly",
                                                        self.sessions.cookie_http_only,
                                                        enabled,
                                                    )
                                                    .on_change(move |st, _, _, cx| {
                                                        e6.update(cx, |this, cx| {
                                                            this.sessions.cookie_http_only =
                                                                st == CheckboxState::Checked;
                                                            cx.notify();
                                                        });
                                                    }),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(210.).max(px(210.)))
                                                        .child(
                                                            Input::new(
                                                                &self.sessions.cookie_expires,
                                                            )
                                                            .id("settings-cookie-expires")
                                                            .aria_label("Cookie Expires")
                                                            .small()
                                                            .disabled(!enabled),
                                                        ),
                                                )
                                                .child(
                                                    sessions_mio_button(
                                                        "settings-cookie-jar-save",
                                                        if self.sessions.editing_cookie_id.is_some()
                                                        {
                                                            "更新 Cookie"
                                                        } else {
                                                            "保存 Cookie"
                                                        },
                                                        !enabled,
                                                    )
                                                    .tooltip(
                                                        if self.sessions.editing_cookie_id.is_some()
                                                        {
                                                            "保存对当前选中 Cookie 条目的修改"
                                                        } else {
                                                            "将当前填写的 Cookie 保存到项目 Cookie Jar"
                                                        },
                                                    )
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.save_cookie_jar_entry(window, cx)
                                                    })),
                                                ),
                                        ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}
