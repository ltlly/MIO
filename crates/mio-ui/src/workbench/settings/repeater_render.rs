use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;

use super::*;

impl Settings {
    pub(in crate::workbench) fn render_repeater_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.state.project;
        let enabled = self.state.enabled;
        let owner = self.owner.clone();
        let unpack_owner = self.owner.clone();
        let normalize_owner = self.owner.clone();
        let h2_strip_owner = self.owner.clone();
        let project_scope = SharedString::from(format!("settings-project-{project:?}"));
        let rs = &self.state.repeater_settings;

        let o_h1 = self.owner.clone();
        let o_h2 = self.owner.clone();
        let o_alpn = self.owner.clone();
        let connections_section = div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(settings_section_heading(
                "settings-repeater-connections",
                "连接",
                project_scope_badge(),
            ))
            .child(settings_desc_row(
                "控制 Repeater 如何处理连接复用，以及在服务器未通过 ALPN 声明 HTTP/2 支持时是否发送 HTTP/2 请求。",
            ))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(1.))
                    .child(
                        listeners_tls::indexed_checkbox(
                            "settings-repeater-http1-reuse",
                            "HTTP/1 连接复用",
                            rs.http1_connection_reuse,
                            enabled,
                        )
                        .tab_stop(false)
                        .on_change(move |st, _, _, cx| {
                            let checked = st == CheckboxState::Checked;
                            let _ = o_h1.update(cx, |view, cx| {
                                view.update_repeater_settings(
                                    project,
                                    |s| s.http1_connection_reuse = checked,
                                    cx,
                                )
                            });
                        }),
                    )
                    .child(
                        listeners_tls::indexed_checkbox(
                            "settings-repeater-http2-reuse",
                            "HTTP/2 连接复用",
                            rs.http2_connection_reuse,
                            enabled,
                        )
                        .tab_stop(false)
                        .on_change(move |st, _, _, cx| {
                            let checked = st == CheckboxState::Checked;
                            let _ = o_h2.update(cx, |view, cx| {
                                view.update_repeater_settings(
                                    project,
                                    |s| s.http2_connection_reuse = checked,
                                    cx,
                                )
                            });
                        }),
                    )
                    .child(
                        listeners_tls::indexed_checkbox(
                            "settings-repeater-http2-alpn-override",
                            "允许 HTTP/2 ALPN 覆盖",
                            rs.allow_http2_alpn_override,
                            enabled,
                        )
                        .tab_stop(false)
                        .on_change(move |st, _, _, cx| {
                            let checked = st == CheckboxState::Checked;
                            let _ = o_alpn.update(cx, |view, cx| {
                                view.update_repeater_settings(
                                    project,
                                    |s| s.allow_http2_alpn_override = checked,
                                    cx,
                                )
                            });
                        }),
                    ),
            );

        let update_content_length_checkbox = settings_checkbox(
            "repeater-update-content-length",
            "更新 Content-Length",
            self.state.update_content_length,
            enabled,
            &self.update_content_length_focus,
            window,
        )
        .on_change(move |state, _, _, cx| {
            let _ = owner.update(cx, |view, cx| {
                view.set_repeater_update_content_length(
                    project,
                    state == CheckboxState::Checked,
                    cx,
                )
            });
        });
        let checkbox = settings_checkbox(
            "repeater-unpack-responses",
            "解包压缩响应",
            self.state.checked,
            enabled,
            &self.checkbox_focus,
            window,
        )
        .on_change(move |state, _, _, cx| {
            let _ = unpack_owner.update(cx, |view, cx| {
                view.set_repeater_unpack(project, state == CheckboxState::Checked, cx)
            });
        });
        let normalize_http1_checkbox = settings_checkbox(
            "settings-repeater-normalize-http1-line-endings",
            "规范化 HTTP/1 换行符",
            self.state.normalize_http1_line_endings,
            enabled,
            &self.normalize_http1_focus,
            window,
        )
        .on_change(move |state, _, _, cx| {
            let _ = normalize_owner.update(cx, |view, cx| {
                view.set_repeater_normalize_http1_line_endings(
                    project,
                    state == CheckboxState::Checked,
                    cx,
                )
            });
        });
        let strip_h2_focus = window
            .use_keyed_state("settings-repeater-strip-h2-focus", cx, |_, cx| {
                cx.focus_handle().tab_stop(false)
            })
            .read(cx)
            .clone();
        let strip_h2_checkbox = settings_checkbox(
            "settings-repeater-strip-h2-conn-headers",
            "HTTP/2 发送时自动剥离 Connection 等逐跳标头",
            self.state.strip_connection_headers_over_http2,
            enabled,
            &strip_h2_focus,
            window,
        )
        .tab_stop(false)
        .on_change(move |state, _, _, cx| {
            let _ = h2_strip_owner.update(cx, |view, cx| {
                view.set_repeater_strip_http2_connection_headers(
                    project,
                    state == CheckboxState::Checked,
                    cx,
                )
            });
        });

        let mut redirect_radios = div()
            .id("settings-repeater-redirect-group")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label("跟随重定向")
            .flex()
            .flex_col()
            .items_start()
            .gap(px(1.))
            .child(div().mb(px(2.)).child("跟随重定向"));
        for (id, label, mode) in [
            (
                "settings-repeater-redirect-never",
                "从不",
                RepeaterRedirectMode::Never,
            ),
            (
                "settings-repeater-redirect-on-site",
                "仅同站",
                RepeaterRedirectMode::OnSiteOnly,
            ),
            (
                "settings-repeater-redirect-in-scope",
                "仅范围内",
                RepeaterRedirectMode::InScopeOnly,
            ),
            (
                "settings-repeater-redirect-always",
                "始终",
                RepeaterRedirectMode::Always,
            ),
        ] {
            let redir_owner = self.owner.clone();
            redirect_radios = redirect_radios.child(
                settings_radio(
                    id,
                    label,
                    self.state.follow_redirects == mode,
                    enabled,
                    window,
                    cx,
                )
                .tab_stop(false)
                .on_change(move |_, _, _, cx| {
                    let _ = redir_owner.update(cx, |view, cx| {
                        view.set_repeater_follow_redirects(project, mode, cx)
                    });
                }),
            );
        }

        let o_cookies = self.owner.clone();
        let o_proto = self.owner.clone();
        let redirect_extras = div()
            .pt(px(3.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(1.))
            .child(
                listeners_tls::indexed_checkbox(
                    "settings-repeater-redirect-cookies",
                    "在重定向中处理 Cookie",
                    rs.process_cookies_in_redirects,
                    enabled,
                )
                .tab_stop(false)
                .on_change(move |st, _, _, cx| {
                    let checked = st == CheckboxState::Checked;
                    let _ = o_cookies.update(cx, |view, cx| {
                        view.update_repeater_settings(
                            project,
                            |s| s.process_cookies_in_redirects = checked,
                            cx,
                        )
                    });
                }),
            )
            .child(
                listeners_tls::indexed_checkbox(
                    "settings-repeater-redirect-cross-domain-protocol",
                    "跨域名重定向时使用所选协议",
                    rs.cross_domain_protocol_redirects,
                    enabled,
                )
                .tab_stop(false)
                .on_change(move |st, _, _, cx| {
                    let checked = st == CheckboxState::Checked;
                    let _ = o_proto.update(cx, |view, cx| {
                        view.update_repeater_settings(
                            project,
                            |s| s.cross_domain_protocol_redirects = checked,
                            cx,
                        )
                    });
                }),
            );

        let content = div()
            .px(px(22.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(connections_section)
            .child(
                div()
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(settings_section_heading(
                        "settings-message-modification",
                        "Message modification",
                        project_scope_badge_with_id("settings-project-scope"),
                    ))
                    .child(settings_desc_row(
                        "设置 Repeater 发送或接收消息时的修改行为。",
                    ))
                    .child(
                        div()
                            .id(project_scope)
                            .pl(px(31.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(1.))
                            .child(update_content_length_checkbox)
                            .child(checkbox)
                            .child(normalize_http1_checkbox)
                            .child(strip_h2_checkbox),
                    ),
            )
            .child(
                div()
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(settings_section_heading(
                        "settings-repeater-redirects",
                        "重定向",
                        project_scope_badge(),
                    ))
                    .child(settings_desc_row_with_action(
                        settings_gear_icon_button(
                            "settings-repeater-redirects-reset",
                            "恢复为不自动跟随重定向的默认设置",
                            !enabled,
                        )
                        .tab_stop(false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = this.owner.update(cx, |view, cx| {
                                view.reset_repeater_redirect_settings(project, cx)
                            });
                            cx.notify();
                        })),
                        "控制 Repeater 如何处理重定向响应。",
                    ))
                    .child(
                        div()
                            .pl(px(31.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(redirect_radios)
                            .child(redirect_extras),
                    ),
            )
            .child(self.render_timeout(cx));

        let mut wrapper = div()
            .id("settings-repeater-outer")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col();
        if self.state.status.is_some() || self.state.error.is_some() {
            wrapper = wrapper.child(
                self.append_repeater_feedback(
                    div()
                        .id("settings-repeater-feedback-bar")
                        .px(px(22.))
                        .pt(px(10.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.)),
                ),
            );
        }
        wrapper
            .child(settings_scrollable_pane(
                "settings-repeater-page",
                &self.content_scroll,
                content,
            ))
            .into_any_element()
    }
}
