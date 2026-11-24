use gpui_kit::accesskit::Role;
use gpui_kit::base::{CheckboxState, Switch as BaseSwitch};
use gpui_kit::component::select::Select;
use mio_core::SocksProxySettings;

use super::*;

mod actions;
mod table;
#[cfg(test)]
mod tests;

pub(super) use table::ConnectionsUi;
use table::{ConnectionsTable, read_bypass_list_file};

impl Settings {
    #[inline(never)]
    fn render_connections_upstream_editor(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_protocol = self
            .connections
            .rule_protocol
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or("HTTP");
        let is_direct = selected_protocol == "Direct";
        let selected_auth = self
            .connections
            .rule_auth
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or("None");
        let auth = !is_direct && selected_auth != "None";
        let is_ntlm = !is_direct && matches!(selected_auth, "NTLM v1" | "NTLM v2");
        let mut editor = div()
            .ml(px(30.))
            .p(px(10.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::background())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_field(
                "代理协议",
                Select::new(&self.connections.rule_protocol)
                    .id("settings-upstream-protocol")
                    .small()
                    .w(px(280.))
                    .disabled(!enabled),
            ))
            .child(connection_field(
                "认证类型",
                Select::new(&self.connections.rule_auth)
                    .id("settings-upstream-auth-type")
                    .small()
                    .w(px(280.))
                    .disabled(!enabled || is_direct),
            ));
        for (id, label, input, field_enabled) in [
            (
                "settings-upstream-destination",
                "Destination host",
                &self.connections.rule_destination,
                true,
            ),
            (
                "settings-upstream-rule-host",
                "Proxy host",
                &self.connections.rule_host,
                !is_direct,
            ),
            (
                "settings-upstream-rule-port",
                "Proxy port",
                &self.connections.rule_port,
                !is_direct,
            ),
            (
                "settings-upstream-rule-username",
                "Username",
                &self.connections.rule_username,
                auth,
            ),
            (
                "settings-upstream-rule-password",
                "Password",
                &self.connections.rule_password,
                auth,
            ),
            (
                "settings-upstream-rule-domain",
                "Domain",
                &self.connections.rule_domain,
                is_ntlm,
            ),
            (
                "settings-upstream-rule-workstation",
                "Workstation",
                &self.connections.rule_workstation,
                is_ntlm,
            ),
            (
                "settings-upstream-rule-description",
                "Description",
                &self.connections.rule_description,
                true,
            ),
        ] {
            let mut field = Input::new(input)
                .id(id)
                .aria_label(label)
                .small()
                .disabled(!enabled || !field_enabled);
            if id == "settings-upstream-rule-password" {
                field = field.mask_toggle();
            }
            editor = editor.child(connection_field(label, field));
        }
        editor
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-upstream-rule-save",
                            "保存",
                            !enabled,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_upstream_rule(window, cx)
                        })),
                    )
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-upstream-rule-cancel",
                            "取消",
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connections.rule_editor_visible = false;
                            this.connections.editing_rule = None;
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_connections_upstream_section(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rules = &self.state.upstream_proxy.rules;
        let selected = self
            .connections
            .selected_rule
            .and_then(|id| rules.iter().position(|rule| rule.id == id));

        let mut upstream_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (id, label, action, available) in [
            ("settings-upstream-add", "添加", 0u8, true),
            ("settings-upstream-edit", "编辑", 1, selected.is_some()),
            ("settings-upstream-remove", "移除", 2, selected.is_some()),
            (
                "settings-upstream-up",
                "上移",
                3,
                selected.is_some_and(|index| index > 0),
            ),
            (
                "settings-upstream-down",
                "下移",
                4,
                selected.is_some_and(|index| index + 1 < rules.len()),
            ),
        ] {
            upstream_actions = upstream_actions.child(
                listeners_tls::mio_action_button(id, label, !enabled || !available)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action < 2 {
                            this.open_upstream_editor(action == 0, window, cx);
                            return;
                        }
                        let selected = this.connections.selected_rule;
                        if action == 2 && selected.is_some() && this.connections.editing_rule == selected {
                            this.connections.rule_editor_visible = false;
                            this.connections.editing_rule = None;
                        }
                        this.change_upstream_rules(
                            |rules| {
                                let Some(index) =
                                    rules.iter().position(|rule| Some(rule.id) == selected)
                                else {
                                    return;
                                };
                                match action {
                                    2 => {
                                        rules.remove(index);
                                    }
                                    3 if index > 0 => rules.swap(index, index - 1),
                                    4 if index + 1 < rules.len() => rules.swap(index, index + 1),
                                    _ => {}
                                }
                            },
                            cx,
                        );
                    })),
            );
        }

        let upstream_override = self.state.upstream_proxy.override_project_only;
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_dual_section_header(
                "settings-upstream-heading",
                "上游代理服务器",
                "使用这些设置控制将出站请求发送到上游代理服务器，或直接发送到目标 Web 服务器。将使用与每个目标主机匹配的第一条规则。要将所有流量发送到单个代理服务器，请创建目标主机为 * 的规则。",
                upstream_override,
                true,
            ))
            .child(sleek_override_card(
                "settings-upstream-override",
                upstream_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let mut next = this.state.upstream_proxy.clone();
                    next.override_project_only = *checked;
                    this.commit_connections(next, cx);
                }),
            ))
            .child(div().pl(px(30.)).child(connection_table(
                &self.connections.rules_table,
                upstream_actions,
                144.,
                "Upstream proxy servers",
            )));
        if self.connections.rule_editor_visible {
            section = section.child(self.render_connections_upstream_editor(enabled, cx));
        }
        section.into_any_element()
    }

    #[inline(never)]
    fn render_connections_socks_editor(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let dns_owner = cx.entity();
        div()
            .mt(px(2.))
            .p(px(10.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::background())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(14.))
                    .child(connection_compact_field(
                        "SOCKS 代理主机",
                        120.,
                        210.,
                        Input::new(&self.upstream_host)
                            .id("settings-upstream-host")
                            .aria_label("SOCKS proxy host")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(connection_compact_field(
                        "SOCKS 代理端口",
                        120.,
                        110.,
                        Input::new(&self.upstream_port)
                            .id("settings-upstream-port")
                            .aria_label("SOCKS proxy port")
                            .small()
                            .disabled(!enabled),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(14.))
                    .child(connection_compact_field(
                        "用户名",
                        120.,
                        210.,
                        Input::new(&self.upstream_username)
                            .id("settings-upstream-username")
                            .aria_label("Username")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(connection_compact_field(
                        "密码",
                        120.,
                        210.,
                        Input::new(&self.upstream_password)
                            .id("settings-upstream-password")
                            .aria_label("Password")
                            .small()
                            .mask_toggle()
                            .disabled(!enabled),
                    )),
            )
            .child(
                listeners_tls::indexed_checkbox(
                    "settings-socks-dns",
                    "通过 SOCKS 代理执行 DNS 解析",
                    self.connections.socks_dns_remote,
                    enabled,
                )
                .on_change(move |value, _, _, cx| {
                    dns_owner.update(cx, |this, cx| {
                        this.connections.socks_dns_remote = value == CheckboxState::Checked;
                        cx.notify();
                    })
                }),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-upstream-apply",
                            "保存",
                            !enabled,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_socks(window, cx)
                        })),
                    )
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-upstream-reset",
                            "重置",
                            !enabled,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            let mut next = this.state.upstream_proxy.clone();
                            next.socks = SocksProxySettings::default();
                            this.commit_connections(next.clone(), cx);
                            this.restore_upstream_draft(&next, window, cx);
                            this.connections.socks_editor_visible = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-socks-close",
                            "收起",
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connections.socks_editor_visible = false;
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_connections_socks_bypass(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let has_bypass_sel = self
            .connections
            .selected_socks_bypass
            .is_some_and(|idx| idx < self.connections.socks_bypass_hosts.len());
        let has_bypass_items = !self.connections.socks_bypass_hosts.is_empty();
        let bypass_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                listeners_tls::mio_action_button("settings-socks-bypass-paste", "粘贴", !enabled)
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| this.paste_socks_bypass_hosts(cx))),
            )
            .child(
                listeners_tls::mio_action_button(
                    "settings-socks-bypass-load",
                    "加载...",
                    !enabled,
                )
                .w_full()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.load_socks_bypass_hosts(window, cx)
                })),
            )
            .child(
                listeners_tls::mio_action_button(
                    "settings-socks-bypass-remove",
                    "移除",
                    !enabled || !has_bypass_sel,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(idx) = this.connections.selected_socks_bypass
                        && idx < this.connections.socks_bypass_hosts.len()
                    {
                        this.connections.socks_bypass_hosts.remove(idx);
                        this.connections.selected_socks_bypass =
                            this.connections.socks_bypass_hosts.len().checked_sub(1);
                        this.commit_socks_bypass_hosts(cx);
                    }
                })),
            )
            .child(
                listeners_tls::mio_action_button(
                    "settings-socks-bypass-clear",
                    "清空",
                    !enabled || !has_bypass_items,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.connections.socks_bypass_hosts.is_empty() {
                        this.connections.socks_bypass_hosts.clear();
                        this.connections.selected_socks_bypass = None;
                        this.commit_socks_bypass_hosts(cx);
                    }
                })),
            );

        let mut bypass_list = div()
            .id("settings-socks-bypass-list")
            .flex_1()
            .min_w_0()
            .h_full()
            .border_1()
            .border_color(listeners_tls::mio_table_border())
            .bg(theme::surface())
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (idx, host) in self.connections.socks_bypass_hosts.iter().enumerate() {
            let is_sel = self.connections.selected_socks_bypass == Some(idx);
            let zebra = idx % 2 == 1;
            bypass_list = bypass_list.child(
                div()
                    .id(("settings-socks-bypass-row", idx))
                    .test_support()
                    .aria_label(host.clone())
                    .h(theme::scaled_px(22.).max(px(20.)))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(is_sel, |this| this.bg(theme::selected()))
                    .when(!is_sel && zebra, |this| {
                        this.bg(rgb(if theme::is_dark() { 0x25272b } else { 0xfafafa }))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.connections.selected_socks_bypass = Some(idx);
                        cx.notify();
                    }))
                    .child(div().min_w_0().flex_1().truncate().child(host.clone())),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .mt(px(4.))
                    .text_color(theme::foreground())
                    .child("如需指定绕过 SOCKS 代理的主机，请在此处添加。"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(9.))
                    .h(px(126.))
                    .flex_shrink_0()
                    .child(bypass_actions)
                    .child(bypass_list),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-socks-bypass-add",
                            "添加",
                            !enabled,
                        )
                        .w(theme::scaled_px(88.).max(px(84.)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_socks_bypass_host(window, cx)
                        })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.connections.socks_bypass_input)
                                    .id("settings-socks-bypass-input")
                                    .aria_label("输入新条目")
                                    .small()
                                    .disabled(!enabled),
                            )
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_connections_socks_section(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let socks = &self.state.upstream_proxy.socks;
        let socks_summary = if !socks.host.trim().is_empty() && socks.port > 0 {
            format!("已配置代理 {}:{}。", socks.host, socks.port)
        } else {
            "未配置代理。".into()
        };
        let socks_override = socks.override_project_only;

        let mut socks_details = div()
            .pl(px(32.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .id("settings-socks-summary")
                    .test_support()
                    .aria_label(socks_summary.clone())
                    .text_color(theme::foreground())
                    .child(socks_summary),
            )
            .child(
                div().flex().child(
                    listeners_tls::mio_action_button("settings-socks-configure", "配置", !enabled)
                        .w(theme::scaled_px(96.).max(px(92.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connections.socks_editor_visible =
                                !this.connections.socks_editor_visible;
                            cx.notify();
                        })),
                ),
            );
        if self.connections.socks_editor_visible {
            socks_details = socks_details.child(self.render_connections_socks_editor(enabled, cx));
        }
        socks_details = socks_details.child(self.render_connections_socks_bypass(enabled, cx));

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_dual_section_header(
                "settings-socks-heading",
                "SOCKS 代理",
                "使用这些设置配置 MIO 对所有出站通信使用 SOCKS 代理。此设置在 TCP 层生效，除绕过列表外的出站请求均会经由此代理发送；命中绕过列表的主机将回退匹配上方上游 HTTP 代理规则或直连。",
                socks_override,
                false,
            ))
            .child(sleek_override_card(
                "settings-socks-override",
                socks_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let mut next = this.state.upstream_proxy.clone();
                    next.socks.override_project_only = *checked;
                    this.commit_connections(next, cx);
                }),
            ))
            .child(
                div()
                    .pl(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(sleek_mio_switch(
                        "settings-socks-enabled",
                        "使用 SOCKS 代理",
                        self.connections.socks_enabled,
                        enabled,
                        cx.listener(|this, checked: &bool, window, cx| {
                            this.connections.socks_enabled = *checked;
                            if !*checked {
                                let mut next = this.state.upstream_proxy.clone();
                                next.socks.use_proxy = false;
                                this.commit_connections(next, cx);
                                return;
                            }
                            let configured = &this.state.upstream_proxy.socks;
                            if configured.host.trim().is_empty() {
                                this.connections.socks_editor_visible = true;
                                cx.notify();
                            } else if configured.port > 0 {
                                this.save_socks(window, cx);
                            } else {
                                cx.notify();
                            }
                        }),
                    ))
                    .child(socks_details),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_connections_timeouts_section(&self, enabled: bool) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_project_section_header(
                "settings-timeouts-heading",
                "超时",
                "使用这些设置指定各项网络任务的超时时间（单位：秒）。普通超时范围为 1–3600 秒；开放式响应超时设为 0 或留空表示无超时限制，失焦或按回车后自动应用。",
            ))
            .child(
                div()
                    .pl(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(connection_compact_field(
                        "普通超时（秒）:",
                        200.,
                        92.,
                        Input::new(&self.connections.normal_timeout_input)
                            .id("settings-normal-timeout-input")
                            .aria_label("普通超时（秒）")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(connection_compact_field(
                        "开放式响应超时（秒）:",
                        200.,
                        92.,
                        Input::new(&self.connections.open_ended_timeout_input)
                            .id("settings-open-ended-timeout-input")
                            .aria_label("开放式响应超时（秒）")
                            .small()
                            .disabled(!enabled),
                    )),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_connections_hostname_section(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut hostname_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (id, label, action) in [
            ("settings-hostname-add", "添加", 0u8),
            ("settings-hostname-edit", "编辑", 1),
            ("settings-hostname-remove", "移除", 2),
        ] {
            hostname_actions = hostname_actions.child(
                listeners_tls::mio_action_button(
                    id,
                    label,
                    !enabled || (action > 0 && self.connections.selected_dns.is_none()),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if action < 2 {
                        this.open_hostname_editor(action == 0, window, cx);
                    } else {
                        let selected = this.connections.selected_dns;
                        if selected.is_some() && this.connections.editing_dns == selected {
                            this.connections.dns_editor_visible = false;
                            this.connections.editing_dns = None;
                        }
                        let mut next = this.state.upstream_proxy.clone();
                        next.hostname_resolutions
                            .retain(|entry| Some(entry.id) != selected);
                        this.commit_connections(next, cx);
                    }
                })),
            );
        }

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_project_section_header(
                "settings-hostname-heading",
                "主机名解析",
                "使用这些设置覆盖指定主机名的 DNS 解析结果并路由至目标 IP 地址。",
            ))
            .child(div().pl(px(30.)).child(connection_table(
                &self.connections.dns_table,
                hostname_actions,
                124.,
                "Hostname resolution",
            )));

        if self.connections.dns_editor_visible {
            section = section.child(
                div()
                    .ml(px(30.))
                    .p(px(10.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::background())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(connection_field(
                        "Hostname",
                        Input::new(&self.connections.dns_hostname_input)
                            .id("settings-hostname-input")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(connection_field(
                        "IP address",
                        Input::new(&self.connections.dns_ip_input)
                            .id("settings-hostname-ip-input")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-hostname-save",
                                    "保存",
                                    !enabled,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_hostname_rule(window, cx)
                                })),
                            )
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-hostname-cancel",
                                    "取消",
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.connections.dns_editor_visible = false;
                                    this.connections.editing_dns = None;
                                    cx.notify();
                                })),
                            ),
                    ),
            );
        }

        section.into_any_element()
    }

    #[inline(never)]
    fn render_connections_platform_auth_editor(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut plat_editor = div()
            .ml(px(30.))
            .p(px(10.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::background())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_field(
                "Destination host",
                Input::new(&self.connections.platform_destination)
                    .id("settings-platform-auth-destination")
                    .aria_label("Destination host")
                    .small()
                    .disabled(!enabled),
            ))
            .child(connection_field(
                "Auth type",
                Select::new(&self.connections.platform_auth_type)
                    .id("settings-platform-auth-type")
                    .small()
                    .w(px(280.))
                    .disabled(!enabled),
            ));
        let selected_plat_auth = self
            .connections
            .platform_auth_type
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or("Basic");
        let is_ntlm = matches!(selected_plat_auth, "NTLM v1" | "NTLM v2");
        for (id, label, input, field_enabled) in [
            (
                "settings-platform-auth-username",
                "Username",
                &self.connections.platform_username,
                true,
            ),
            (
                "settings-platform-auth-password",
                "Password",
                &self.connections.platform_password,
                true,
            ),
            (
                "settings-platform-auth-domain",
                "Domain",
                &self.connections.platform_domain,
                is_ntlm,
            ),
            (
                "settings-platform-auth-hostname",
                "Hostname",
                &self.connections.platform_hostname,
                is_ntlm,
            ),
        ] {
            let mut field = Input::new(input)
                .id(id)
                .aria_label(label)
                .small()
                .disabled(!enabled || !field_enabled);
            if id == "settings-platform-auth-password" {
                field = field.mask_toggle();
            }
            plat_editor = plat_editor.child(connection_field(label, field));
        }
        plat_editor
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-platform-auth-save",
                            "保存",
                            !enabled,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_platform_auth_rule(window, cx)
                        })),
                    )
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-platform-auth-cancel",
                            "取消",
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connections.platform_editor_visible = false;
                            this.connections.editing_platform_auth = None;
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_connections_platform_auth_section(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Section 5: Platform authentication
        let plat_rules = &self.state.upstream_proxy.platform_auth.rules;
        let plat_selected = self
            .connections
            .selected_platform_auth
            .and_then(|id| plat_rules.iter().position(|r| r.id == id));
        let mut plat_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (id, label, action, available) in [
            ("settings-platform-auth-add", "添加", 0u8, true),
            ("settings-platform-auth-edit", "编辑", 1, plat_selected.is_some()),
            (
                "settings-platform-auth-remove",
                "移除",
                2,
                plat_selected.is_some(),
            ),
            (
                "settings-platform-auth-up",
                "上移",
                3,
                plat_selected.is_some_and(|idx| idx > 0),
            ),
            (
                "settings-platform-auth-down",
                "下移",
                4,
                plat_selected.is_some_and(|idx| idx + 1 < plat_rules.len()),
            ),
        ] {
            plat_actions = plat_actions.child(
                listeners_tls::mio_action_button(id, label, !enabled || !available)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action < 2 {
                            this.open_platform_auth_editor(action == 0, window, cx);
                            return;
                        }
                        let selected = this.connections.selected_platform_auth;
                        if action == 2
                            && selected.is_some()
                            && this.connections.editing_platform_auth == selected
                        {
                            this.connections.platform_editor_visible = false;
                            this.connections.editing_platform_auth = None;
                        }
                        let mut next = this.state.upstream_proxy.clone();
                        let rules = &mut next.platform_auth.rules;
                        let Some(index) = rules.iter().position(|r| Some(r.id) == selected) else {
                            return;
                        };
                        match action {
                            2 => {
                                rules.remove(index);
                            }
                            3 if index > 0 => rules.swap(index, index - 1),
                            4 if index + 1 < rules.len() => rules.swap(index, index + 1),
                            _ => {}
                        }
                        this.commit_connections(next, cx);
                    })),
            );
        }

        let plat_override = self
            .state
            .upstream_proxy
            .platform_auth
            .override_project_only;
        let plat_enabled = self.state.upstream_proxy.platform_auth.enabled;
        let plat_prompt = self.state.upstream_proxy.platform_auth.prompt_on_failure;
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_dual_section_header(
                "settings-platform-auth-heading",
                "平台认证",
                "使用这些设置配置 MIO 自动对目标 Web 服务器执行 HTTP 平台认证（支持 Basic、Digest、NTLM v1 与 NTLM v2）。将使用与每个目标主机匹配的第一条启用规则。",
                plat_override,
                false,
            ))
            .child(sleek_override_card(
                "settings-platform-auth-override",
                plat_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let mut next = this.state.upstream_proxy.clone();
                    next.platform_auth.override_project_only = *checked;
                    this.commit_connections(next, cx);
                }),
            ))
            .child(
                div()
                    .pl(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(sleek_mio_switch(
                        "settings-platform-auth-enabled",
                        "执行平台认证 (Do platform authentication)",
                        plat_enabled,
                        enabled,
                        cx.listener(|this, checked: &bool, _, cx| {
                            let mut next = this.state.upstream_proxy.clone();
                            next.platform_auth.enabled = *checked;
                            this.commit_connections(next, cx);
                        }),
                    ))
                    .child(
                        div()
                            .id("settings-platform-auth-prompt-wrap")
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "保存该兼容选项以便导出与跨项目同步；当前运行时暂不触发交互式失败弹窗重试。",
                                )
                                .build(w, cx)
                            })
                            .child(sleek_mio_switch(
                                "settings-platform-auth-prompt",
                                "平台认证失败时提示输入凭据 (Prompt for credentials on platform authentication failure)",
                                plat_prompt,
                                enabled,
                                cx.listener(|this, checked: &bool, _, cx| {
                                    let mut next = this.state.upstream_proxy.clone();
                                    next.platform_auth.prompt_on_failure = *checked;
                                    this.commit_connections(next, cx);
                                }),
                            )),
                    )
                    .child(connection_table(
                        &self.connections.platform_table,
                        plat_actions,
                        144.,
                        "Platform authentication",
                    )),
            );

        if self.connections.platform_editor_visible {
            section = section.child(self.render_connections_platform_auth_editor(enabled, cx));
        }

        section.into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_connections(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.state.enabled;
        let content = div()
            .px(px(22.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.render_connections_upstream_section(enabled, cx))
            .child(self.render_connections_socks_section(enabled, cx))
            .child(self.render_connections_timeouts_section(enabled))
            .child(self.render_connections_hostname_section(enabled, cx))
            .child(self.render_connections_platform_auth_section(enabled, cx))
            .when_some(
                self.connections
                    .error
                    .clone()
                    .or_else(|| self.state.error.clone()),
                |this, error| {
                    this.child(
                        div()
                            .id("settings-upstream-error")
                            .test_support()
                            .role(Role::Alert)
                            .aria_label(error.clone())
                            .ml(px(30.))
                            .p(px(8.))
                            .rounded(px(4.))
                            .bg(theme::danger_bg())
                            .text_color(theme::danger_fg())
                            .child(error),
                    )
                },
            );
        settings_scrollable_pane("settings-connections-page", &self.content_scroll, content)
            .into_any_element()
    }
}

fn sleek_mio_switch(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> BaseSwitch {
    let dark = theme::is_dark();
    let track_bg: gpui_kit::Hsla = if checked {
        settings_blue()
    } else {
        rgb(if dark { 0x4e535b } else { 0xb0afaf }).into()
    };
    let thumb_bg = if checked {
        rgb(0xffffff)
    } else if dark {
        rgb(0xd4d4d4)
    } else {
        rgb(0x2b2b2b)
    };
    BaseSwitch::new(id)
        .checked(checked)
        .disabled(!enabled)
        .accessibility_label(label)
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_not_allowed()))
        .on_change(move |next, _, window, cx| on_change(&next, window, cx))
        .child(
            div()
                .w(px(24.))
                .h(px(13.))
                .flex_shrink_0()
                .rounded_full()
                .p(px(2.))
                .flex()
                .items_center()
                .when(checked, |this| this.justify_end())
                .when(!checked, |this| this.justify_start())
                .bg(track_bg)
                .child(div().size(px(9.)).rounded_full().bg(thumb_bg)),
        )
        .child(
            div()
                .text_size(theme::ui_font_size())
                .text_color(theme::foreground())
                .child(label),
        )
}

fn sleek_override_card(
    id: &'static str,
    checked: bool,
    enabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    div()
        .ml(px(30.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(3.))
        .border_1()
        .border_color(listeners_tls::mio_table_border())
        .bg(theme::surface())
        .child(sleek_mio_switch(
            id,
            "仅为当前项目覆盖选项",
            checked,
            enabled,
            on_change,
        ))
}

fn connection_dual_section_header(
    id: &'static str,
    title: &'static str,
    desc: &'static str,
    override_project: bool,
    first: bool,
) -> gpui_kit::Div {
    div()
        .when(!first, |this| {
            this.mt(px(6.))
                .pt(px(12.))
                .border_t_1()
                .border_color(theme::border())
        })
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(listeners_tls::mio_section_heading(
            id,
            title,
            listeners_tls::mio_dual_scope_pills(override_project),
        ))
        .child(listeners_tls::mio_desc_row(desc))
}

fn connection_project_section_header(
    id: &'static str,
    title: &'static str,
    desc: &'static str,
) -> gpui_kit::Div {
    div()
        .mt(px(6.))
        .pt(px(12.))
        .border_t_1()
        .border_color(theme::border())
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(listeners_tls::mio_section_heading(
            id,
            title,
            listeners_tls::mio_project_pill(),
        ))
        .child(listeners_tls::mio_desc_row(desc))
}

fn connection_compact_field(
    label: &'static str,
    label_w: f32,
    input_w: f32,
    input: impl IntoElement,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().w(px(label_w)).flex_shrink_0().child(label))
        .child(div().w(px(input_w)).min_w_0().child(input))
}

fn connection_field(label: &'static str, input: impl IntoElement) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(180.)).flex_shrink_0().child(label))
        .child(div().w(px(300.)).min_w_0().child(input))
}

fn connection_table(
    table: &Entity<TableState<ConnectionsTable>>,
    actions: gpui_kit::Div,
    height: f32,
    label: &'static str,
) -> gpui_kit::Div {
    div()
        .flex()
        .gap(px(9.))
        .h(px(height))
        .flex_shrink_0()
        .child(actions)
        .child(listeners_tls::mio_table_frame(table, label))
}
