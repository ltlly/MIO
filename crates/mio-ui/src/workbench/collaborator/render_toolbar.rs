use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::uniform_list;

impl CollaboratorView {
    #[inline(never)]
    fn render_toolbar_payload_controls(
        &self,
        can_operate: bool,
        running: bool,
        badge: String,
        badge_bg: u32,
        badge_fg: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let divider = || {
            div()
                .w(px(1.))
                .h(theme::scaled_px(16.))
                .flex_shrink_0()
                .bg(theme::border())
        };
        div()
            .flex()
            .items_center()
            .gap(theme::scaled_px(6.))
            .child(
                button("collaborator-copy-http", "复制 Payload")
                    .primary()
                    .icon(IconName::Copy)
                    .tooltip("签发新的带外测试 Token 并将 HTTP 回调 URL 复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.generate_payloads(CopyMode::Http, window, cx);
                    })),
            )
            .child(
                button("collaborator-copy-dns", "复制 DNS")
                    .icon(IconName::SendHorizontal)
                    .tooltip("签发新的带外测试 Token 并将 DNS 回调域名复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.generate_payloads(CopyMode::Dns, window, cx);
                    })),
            )
            .child(
                button("collaborator-poll-now", "立即轮询")
                    .icon(IconName::RefreshCw)
                    .tooltip("立即向本地或自托管 OAST 服务器发起一次拉取轮询")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let _ = this
                            .workbench
                            .update(cx, |wb, cx| wb.poll_collaborator_now(window, cx));
                        cx.notify();
                    })),
            )
            .child(divider())
            .child(
                div()
                    .flex_shrink_0()
                    .w(theme::scaled_px(48.).max(px(44.)))
                    .child(
                        Input::new(&self.count_input)
                            .id("collaborator-count")
                            .aria_label("生成 payload 数量")
                            .small()
                            .disabled(!can_operate),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(theme::scaled_px(110.).max(px(96.)))
                    .child(
                        Input::new(&self.label_input)
                            .id("collaborator-label")
                            .aria_label("payload 备注标签")
                            .small()
                            .disabled(!can_operate),
                    ),
            )
            .child(divider())
            .child(
                button(
                    "collaborator-toggle-server",
                    if running {
                        "停止监听"
                    } else {
                        "启动监听"
                    },
                )
                .icon(if running {
                    IconName::Square
                } else {
                    IconName::Play
                })
                .tooltip("启动或停止本地 Loopback / 自托管 OAST 回调监听器")
                .disabled(!can_operate)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_server(window, cx);
                })),
            )
            .child(
                div()
                    .id("collaborator-server-badge")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(badge.clone())
                    .max_w(theme::scaled_px(210.))
                    .truncate()
                    .rounded(px(9.))
                    .px(px(8.))
                    .py(px(1.))
                    .text_size(theme::scaled_px(11.))
                    .bg(rgb(badge_bg))
                    .text_color(rgb(badge_fg))
                    .child(badge),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_toolbar_filter_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let protocol_filter = self.protocol_filter;
        div()
            .flex()
            .items_center()
            .gap(theme::scaled_px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        button("collaborator-filter-all", "全部")
                            .px(theme::scaled_px(8.))
                            .tooltip("显示全部协议类型的带外交互记录")
                            .selected(protocol_filter.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_protocol_filter(None, window, cx);
                            })),
                    )
                    .child(
                        button("collaborator-filter-http", "HTTP")
                            .px(theme::scaled_px(8.))
                            .tooltip("仅显示 HTTP 协议回调交互记录")
                            .selected(protocol_filter == Some(OastProtocol::Http))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_protocol_filter(Some(OastProtocol::Http), window, cx);
                            })),
                    )
                    .child(
                        button("collaborator-filter-dns", "DNS")
                            .px(theme::scaled_px(8.))
                            .tooltip("仅显示 DNS 协议回调交互记录")
                            .selected(protocol_filter == Some(OastProtocol::Dns))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_protocol_filter(Some(OastProtocol::Dns), window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(theme::scaled_px(148.).max(px(120.)))
                    .child(
                        Input::new(&self.search_input)
                            .id("collaborator-search")
                            .aria_label("筛选交互")
                            .small(),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_toolbar_actions(
        &self,
        can_operate: bool,
        file_picker_pending: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                Button::new("collaborator-copy-json")
                    .ghost()
                    .small()
                    .w(px(26.))
                    .h(theme::control_height())
                    .px(px(0.))
                    .rounded(px(3.))
                    .icon(IconName::Copy)
                    .accessibility_label("复制交互记录 (JSON)")
                    .tooltip("将当前全部交互记录以 JSON 格式复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let _ = this.workbench.update(cx, |wb, cx| {
                            wb.copy_collaborator_interactions_json(window, cx)
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("collaborator-export-json-file")
                    .ghost()
                    .small()
                    .w(px(26.))
                    .h(theme::control_height())
                    .px(px(0.))
                    .rounded(px(3.))
                    .icon(IconName::Download)
                    .accessibility_label("导出交互记录…")
                    .tooltip("将当前全部交互记录保存为本地 JSON 文件")
                    .disabled(!can_operate || file_picker_pending)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let _ = this.workbench.update(cx, |wb, cx| {
                            wb.export_collaborator_interactions_file(window, cx)
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("collaborator-clear")
                    .ghost()
                    .small()
                    .w(px(26.))
                    .h(theme::control_height())
                    .px(px(0.))
                    .rounded(px(3.))
                    .icon(IconName::Trash)
                    .accessibility_label("清空交互")
                    .tooltip("清空当前已捕获的带外交互记录（保留已签发的 Payload Token）")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let _ = this.workbench.update(cx, |wb, cx| {
                            wb.clear_collaborator_interactions(window, cx)
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("collaborator-toggle-config")
                    .ghost()
                    .small()
                    .w(px(26.))
                    .h(theme::control_height())
                    .px(px(0.))
                    .rounded(px(3.))
                    .icon(IconName::Settings)
                    .accessibility_label("自托管配置")
                    .tooltip("展开或收起自托管 OAST 服务器配置面板")
                    .selected(self.config_open)
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_config_panel(window, cx);
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(super) fn render_toolbar(
        &mut self,
        wb_entity: &Entity<Workbench>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (can_operate, file_picker_pending, running, badge, badge_bg, badge_fg) = {
            let wb = wb_entity.read(cx);
            let (badge, badge_bg, badge_fg) = server_badge_text(&wb.collaborator_pane.status);
            (
                wb.collaborator_can_operate(),
                wb.file_picker_pending(),
                wb.collaborator_pane.status.running,
                badge,
                badge_bg,
                badge_fg,
            )
        };

        let payload_controls = self.render_toolbar_payload_controls(
            can_operate,
            running,
            badge,
            badge_bg,
            badge_fg,
            cx,
        );
        let filter_group = self.render_toolbar_filter_group(cx);
        let actions = self.render_toolbar_actions(can_operate, file_picker_pending, cx);

        toolbar()
            .flex_wrap()
            .h_auto()
            .min_h(theme::scaled_px(36.))
            .py(px(4.))
            .gap(theme::scaled_px(6.))
            .child(payload_controls)
            .child(div().flex_1().min_w(px(4.)))
            .child(filter_group)
            .child(actions)
            .into_any_element()
    }

    pub(super) fn render_payload_panel(
        &self,
        wb_entity: &Entity<Workbench>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (preview, notice, issued_count) = {
            let pane = &wb_entity.read(cx).collaborator_pane;
            (
                pane.latest_payload_preview
                    .clone()
                    .unwrap_or_else(|| "尚未生成 Payload。".into()),
                pane.notice.clone().unwrap_or_default(),
                pane.status.issued_tokens.len(),
            )
        };
        let display_preview = {
            let mut lines = preview.lines();
            let first = lines.next().unwrap_or("");
            let extra = lines.count();
            if extra > 0 {
                format!("{first} (+{extra})")
            } else {
                first.to_owned()
            }
        };
        let drawer_open = self.payloads_drawer_open;
        let wb_weak = self.workbench.clone();
        div()
            .id("collaborator-payload-panel")
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .child(
                div()
                    .w_full()
                    .min_h(theme::scaled_px(28.))
                    .px(theme::scaled_px(10.))
                    .py(px(3.))
                    .flex()
                    .items_center()
                    .gap(theme::scaled_px(8.))
                    .child(
                        Button::new("collaborator-toggle-payloads")
                            .ghost()
                            .small()
                            .h(px(22.))
                            .px(px(6.))
                            .rounded(px(3.))
                            .icon(if drawer_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label(format!("已签发 Payload ({issued_count})"))
                            .tooltip("展开或收起已签发 Payload 列表")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_payloads_drawer(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("collaborator-payload-preview")
                            .test_support()
                            .aria_label(preview.clone())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::mono_font_family())
                            .text_size(theme::scaled_px(11.5))
                            .text_color(theme::foreground())
                            .child(display_preview),
                    )
                    .when(!notice.is_empty(), |row| {
                        row.child(
                            div()
                                .id("collaborator-payload-notice")
                                .test_support()
                                .flex_shrink_0()
                                .text_size(theme::scaled_px(11.))
                                .text_color(rgb(if theme::is_dark() {
                                    0x81c995
                                } else {
                                    0x137333
                                }))
                                .child(notice),
                        )
                    }),
            )
            .when(drawer_open && issued_count > 0, |section| {
                section.child(
                    div()
                        .w_full()
                        .h(px(116.))
                        .border_t_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .child(
                            uniform_list(
                                "collaborator-payload-list",
                                issued_count,
                                move |range, _, cx| {
                                    let Some(owner) = wb_weak.upgrade() else {
                                        return Vec::new();
                                    };
                                    let tokens =
                                        &owner.read(cx).collaborator_pane.status.issued_tokens;
                                    let len = tokens.len();
                                    range
                                        .filter_map(|index| {
                                            let rev_idx = len.checked_sub(index + 1)?;
                                            tokens.get(rev_idx).map(|token| (index, token))
                                        })
                                        .map(|(index, token)| {
                                            div()
                                                .id(("collaborator-payload", index))
                                                .test_support()
                                                .aria_label(format!(
                                                    "{} {}",
                                                    token.token, token.label
                                                ))
                                                .w_full()
                                                .h(theme::row_height())
                                                .px(px(10.))
                                                .flex()
                                                .items_center()
                                                .gap(px(10.))
                                                .border_b_1()
                                                .border_color(theme::border())
                                                .text_size(theme::scaled_px(11.5))
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(150.))
                                                        .flex_shrink_0()
                                                        .truncate()
                                                        .font_family(theme::mono_font_family())
                                                        .child(token.token.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .font_family(theme::mono_font_family())
                                                        .text_color(theme::muted_foreground())
                                                        .child(token.http_callback_url.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .w(theme::scaled_px(160.))
                                                        .flex_shrink_0()
                                                        .truncate()
                                                        .child(token.label.clone()),
                                                )
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .w_full()
                            .h_full(),
                        ),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_config_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let field_row = |label: &'static str, input: &Entity<InputState>, id: &'static str, aria: &'static str, fixed_w: Option<f32>| {
            let input_box = div()
                .when_some(fixed_w, |d, w| d.w(theme::scaled_px(w).max(px(w))).flex_shrink_0())
                .when(fixed_w.is_none(), |d| d.flex_1().min_w_0())
                .child(Input::new(input).id(id).aria_label(aria).small());
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .min_w(theme::scaled_px(120.))
                        .flex_shrink_0()
                        .child(label),
                )
                .child(input_box)
        };
        div()
            .id("collaborator-config-panel")
            .test_support()
            .px(px(8.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("自托管 OAST 轮询配置")
                    .child(
                        button(
                            "collaborator-self-hosted-enabled",
                            if self.self_hosted_enabled {
                                "关闭外部轮询"
                            } else {
                                "启用外部轮询"
                            },
                        )
                        .tooltip("启用或关闭自定义自托管 OAST 服务器轮询")
                        .selected(self.self_hosted_enabled)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_self_hosted(window, cx);
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("collaborator-apply-config", "应用并重启监听器")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("应用自托管配置并重启 OAST 监听器")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_server(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .child(format!(
                        "{LOCAL_LOOPBACK_NOTICE}。{SELF_HOSTED_SECURITY_NOTE}"
                    )),
            )
            .when(self.self_hosted_enabled, |section| {
                section
                    .child(field_row(
                        "轮询 URL：",
                        &self.polling_url_input,
                        "collaborator-polling-url",
                        "自托管轮询 URL",
                        None,
                    ))
                    .child(field_row(
                        "服务域名：",
                        &self.server_domain_input,
                        "collaborator-server-domain",
                        "自托管服务域名",
                        None,
                    ))
                    .child(field_row(
                        "认证请求头：",
                        &self.auth_header_input,
                        "collaborator-auth-header",
                        "自托管认证请求头",
                        None,
                    ))
                    .child(field_row(
                        "轮询间隔 (ms)：",
                        &self.poll_interval_input,
                        "collaborator-poll-interval",
                        "轮询间隔 (ms)",
                        Some(160.),
                    ))
            })
            .into_any_element()
    }
}
