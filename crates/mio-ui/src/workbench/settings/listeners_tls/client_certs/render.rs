use gpui_kit::component::select::Select;

use super::*;

impl Settings {
    pub(in crate::workbench::settings) fn render_client_tls_card(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let table = &self.listeners_tls.client_cert_table;
        let selected_idx = self.listeners_tls.client_cert_selected;
        let cert_count = self.state.client_certificates.len();
        let selected_from_table = table.read(cx).selected_row();
        let active_selected = selected_from_table
            .or(selected_idx)
            .filter(|&row| row < cert_count);
        let enabled = self.state.enabled;

        let side_buttons = self.render_client_cert_side_buttons(active_selected, cert_count, cx);
        let table_row = div()
            .flex()
            .gap(px(9.))
            .h(px(132.))
            .flex_shrink_0()
            .child(side_buttons)
            .child(mio_table_frame(table, "Client TLS certificates"));

        let mut content = div()
            .pl(px(30.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(table_row);

        if self.listeners_tls.client_cert_editor_visible {
            content = content.child(self.render_client_cert_editor_form(enabled, cx));
        }

        if let Some(error) = self.listeners_tls.client_cert_error.clone() {
            content = content.child(
                div()
                    .id("settings-client-cert-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .p(px(8.))
                    .rounded(px(4.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(error),
            );
        }

        let client_certs_override = self.state.upstream_tls.client_certs_override_project_only;
        div()
            .mt(px(6.))
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(mio_section_heading(
                "settings-tls-client-certs-heading",
                "客户端 TLS 证书",
                mio_dual_scope_pills(client_certs_override),
            ))
            .child(mio_desc_row(
                "使用这些设置配置当目标主机请求客户端 TLS 证书时所提供的证书。可配置多张证书并指定每张证书对应的目标主机；当主机请求客户端证书时，将使用列表中第一个主机配置匹配的证书。",
            ))
            .child(mio_override_card(
                "settings-client-certs-override",
                client_certs_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let next = *checked;
                    this.update_upstream_tls(
                        move |s| s.client_certs_override_project_only = next,
                        cx,
                    );
                }),
            ))
            .child(content)
            .into_any_element()
    }

    fn render_client_cert_side_buttons(
        &self,
        active_selected: Option<usize>,
        cert_count: usize,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let table = &self.listeners_tls.client_cert_table;
        let enabled = self.state.enabled && !self.listeners_tls.client_cert_saving;
        let editor_open = self.listeners_tls.client_cert_editor_visible;
        let mutate_enabled = enabled && !editor_open;
        let project = self.state.project;
        let remove_index = active_selected;
        let can_move_up = active_selected.is_some_and(|row| row > 0);
        let can_move_down = active_selected.is_some_and(|row| row + 1 < cert_count);

        let table_up = table.clone();
        let table_down = table.clone();
        let table_rm = table.clone();

        div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(
                mio_action_button("settings-client-cert-add", "添加", !enabled)
                    .w_full()
                    .tooltip("打开或关闭客户端 TLS 证书添加表单")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.listeners_tls.client_cert_editor_visible
                            && this.listeners_tls.client_cert_editing.is_none()
                        {
                            this.listeners_tls.client_cert_editor_visible = false;
                            this.listeners_tls.client_cert_error = None;
                            cx.notify();
                        } else {
                            this.open_client_cert_editor(None, window, cx);
                        }
                    })),
            )
            .child(
                mio_action_button(
                    "settings-client-cert-edit",
                    "编辑",
                    !enabled || active_selected.is_none(),
                )
                .w_full()
                .tooltip("编辑选中的客户端 TLS 证书规则")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(index) = active_selected {
                        this.open_client_cert_editor(Some(index), window, cx);
                    }
                })),
            )
            .child(
                mio_action_button(
                    "settings-client-cert-remove-btn",
                    "移除",
                    !mutate_enabled || remove_index.is_none(),
                )
                .w_full()
                .tooltip("移除选中的客户端 TLS 证书规则")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.state.enabled
                        || this.listeners_tls.client_cert_saving
                        || this.listeners_tls.client_cert_editor_visible
                    {
                        return;
                    }
                    let Some(index) = remove_index else {
                        return;
                    };
                    let res = this.owner.update(cx, |view, cx| {
                        view.remove_client_certificate_rule(project, index, cx)
                    });
                    if matches!(res, Ok(Ok(()))) {
                        let remaining = this.state.client_certificates.len().saturating_sub(1);
                        let next_sel = (remaining > 0).then(|| index.min(remaining - 1));
                        this.listeners_tls.client_cert_selected = next_sel;
                        table_rm.update(cx, |state, cx| {
                            if let Some(row) = next_sel {
                                state.set_selected_row(row, cx);
                            } else {
                                state.clear_selection(cx);
                            }
                        });
                    }
                    this.listeners_tls.client_cert_error = match res {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error),
                        Err(_) => Some("主工作台已关闭；客户端证书未删除。".into()),
                    };
                    cx.notify();
                })),
            )
            .child(
                mio_action_button("settings-client-cert-up", "上移", !mutate_enabled || !can_move_up)
                    .w_full()
                    .tooltip("将选中的客户端 TLS 证书向上移动一位")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.state.enabled
                            || this.listeners_tls.client_cert_saving
                            || this.listeners_tls.client_cert_editor_visible
                        {
                            return;
                        }
                        let Some(index) = active_selected.filter(|&row| row > 0) else {
                            return;
                        };
                        let target = index - 1;
                        let res = this.owner.update(cx, |view, cx| {
                            view.move_client_certificate_rule(project, index, target, cx)
                        });
                        if matches!(res, Ok(Ok(()))) {
                            this.listeners_tls.client_cert_selected = Some(target);
                            table_up.update(cx, |state, cx| state.set_selected_row(target, cx));
                        }
                        this.listeners_tls.client_cert_error = match res {
                            Ok(Ok(())) => None,
                            Ok(Err(error)) => Some(error),
                            Err(_) => Some("主工作台已关闭；客户端证书未调整顺序。".into()),
                        };
                        cx.notify();
                    })),
            )
            .child(
                mio_action_button(
                    "settings-client-cert-down",
                    "下移",
                    !mutate_enabled || !can_move_down,
                )
                .w_full()
                .tooltip("将选中的客户端 TLS 证书向下移动一位")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.state.enabled
                        || this.listeners_tls.client_cert_saving
                        || this.listeners_tls.client_cert_editor_visible
                    {
                        return;
                    }
                    let Some(index) = active_selected.filter(|&row| row + 1 < cert_count) else {
                        return;
                    };
                    let target = index + 1;
                    let res = this.owner.update(cx, |view, cx| {
                        view.move_client_certificate_rule(project, index, target, cx)
                    });
                    if matches!(res, Ok(Ok(()))) {
                        this.listeners_tls.client_cert_selected = Some(target);
                        table_down.update(cx, |state, cx| state.set_selected_row(target, cx));
                    }
                    this.listeners_tls.client_cert_error = match res {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error),
                        Err(_) => Some("主工作台已关闭；客户端证书未调整顺序。".into()),
                    };
                    cx.notify();
                })),
            )
    }

    fn render_client_cert_editor_form(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let format_select = &self.listeners_tls.client_cert_format_select;
        let is_pem = self.listeners_tls.client_cert_format == ClientCertificateFormat::Pem;
        let is_editing = self.listeners_tls.client_cert_editing.is_some();
        let form_enabled = enabled && !self.listeners_tls.client_cert_saving;
        let label_col = |text: &'static str| {
            div()
                .w(theme::scaled_px(135.).max(px(130.)))
                .flex_shrink_0()
                .child(text)
        };
        let input_box = |input: &Entity<InputState>, id: &'static str, aria: &'static str, off: bool, masked: bool| {
            let mut field = Input::new(input)
                .id(id)
                .aria_label(aria)
                .small()
                .disabled(off);
            if masked {
                field = field.mask_toggle();
            }
            div()
                .flex_1()
                .max_w(theme::scaled_px(360.).max(px(360.)))
                .min_w(px(180.))
                .child(field)
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
                    .gap(px(12.))
                    .child(label_col("证书格式"))
                    .child(
                        Select::new(format_select)
                            .id("settings-client-cert-format-select")
                            .small()
                            .w(theme::scaled_px(220.).max(px(220.)))
                            .disabled(!form_enabled),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(label_col("目标主机模式"))
                    .child(input_box(
                        &self.listeners_tls.client_cert_host,
                        "settings-client-cert-host",
                        "目标主机模式",
                        !form_enabled,
                        false,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(label_col(if is_editing {
                        "证书文件路径（留空沿用）"
                    } else {
                        "证书文件路径"
                    }))
                    .child(input_box(
                        &self.listeners_tls.client_cert_path,
                        "settings-client-cert-path",
                        "证书文件路径",
                        !form_enabled,
                        false,
                    ))
                    .child(
                        mio_action_button("settings-client-cert-browse", "选择文件...", !form_enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pick_client_cert_file(false, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(label_col("PEM 私钥路径（可选）"))
                    .child(input_box(
                        &self.listeners_tls.client_cert_key_path,
                        "settings-client-cert-key-path",
                        "PEM 私钥路径",
                        !form_enabled || !is_pem,
                        false,
                    ))
                    .child(
                        mio_action_button(
                            "settings-client-cert-key-browse",
                            "选择文件...",
                            !form_enabled || !is_pem,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.pick_client_cert_file(true, window, cx)
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(label_col("PKCS#12 口令"))
                    .child(input_box(
                        &self.listeners_tls.client_cert_password,
                        "settings-client-cert-password",
                        "PKCS#12 口令",
                        !form_enabled || is_pem,
                        true,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        mio_action_button(
                            "settings-client-cert-submit",
                            if self.listeners_tls.client_cert_saving {
                                "校验中…"
                            } else if is_editing {
                                "保存修改"
                            } else {
                                "确认添加"
                            },
                            !form_enabled,
                        )
                        .tooltip("读取并添加当前客户端 TLS 证书规则到项目设置")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.submit_add_client_cert(window, cx)
                        })),
                    )
                    .child(
                        mio_action_button("settings-client-cert-cancel", "取消", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.listeners_tls.client_cert_editor_visible = false;
                                this.listeners_tls.client_cert_editing = None;
                                this.listeners_tls.client_cert_saving = false;
                                this.listeners_tls.client_cert_error = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
    }
}
