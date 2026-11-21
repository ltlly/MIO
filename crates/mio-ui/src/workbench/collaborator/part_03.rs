use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    button::{ButtonRounded, ButtonVariants},
    popover::Popover,
};
use gpui_kit::{ClickEvent, uniform_list};
use std::rc::Rc;

impl Workbench {
    pub(in crate::workbench) fn copy_collaborator_interactions_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        match mio_core::render_collaborator_interactions_json(
            &self.collaborator_pane.status.interactions,
        ) {
            Ok(json) => {
                let count = self.collaborator_pane.status.interactions.len();
                cx.write_to_clipboard(ClipboardItem::new_string(json));
                self.collaborator_pane.error = None;
                self.collaborator_pane.notice =
                    Some(format!("已复制 {count} 条交互记录 (JSON) 到剪贴板。"));
                cx.notify();
            }
            Err(err) => self.fail_collaborator(err.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn export_collaborator_interactions_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() || self.file_picker_pending() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let count = self.collaborator_pane.status.interactions.len();
        let json = match mio_core::render_collaborator_interactions_json(
            &self.collaborator_pane.status.interactions,
        ) {
            Ok(json) => json,
            Err(err) => {
                self.fail_collaborator(err.to_string(), cx);
                return;
            }
        };
        let Some(project) = self.project else { return };
        let directory = self.default_directory();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &directory,
            Some("collaborator-interactions.json"),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => match std::fs::write(&path, json.as_bytes()) {
                        Ok(()) => {
                            view.collaborator_pane.error = None;
                            let msg = format!("已导出 {count} 条交互记录到 {}。", path.display());
                            view.collaborator_pane.notice = Some(msg.clone());
                            view.status = msg;
                        }
                        Err(err) => view.fail_collaborator(format!("导出交互记录失败：{err}"), cx),
                    },
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => view.fail_collaborator(format!("无法选择导出路径：{err}"), cx),
                    Err(_) => view.fail_collaborator("文件选择器已关闭。", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn render_collaborator_onboarding(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let radar = self.collaborator_pane.radar_image(dark, cx);
        let heading_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };
        let body_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x2b2b2b).into()
        };
        let learn_more_bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xeeeeee }).into();
        let learn_more_border: gpui_kit::Hsla =
            rgb(if dark { 0x4e535b } else { 0xdfdfdf }).into();
        let link_fg: gpui_kit::Hsla = rgb(if dark { 0xe58d5d } else { 0xa75f31 }).into();

        let learn_more_popover = Popover::new("collaborator-learn-more-popover")
            .anchor(gpui_kit::Anchor::BottomCenter)
            .trigger(
                Button::new("collaborator-learn-more")
                    .small()
                    .min_w(theme::scaled_px(113.))
                    .h(theme::scaled_px(24.))
                    .px(theme::scaled_px(16.))
                    .rounded(ButtonRounded::Size(px(3.)))
                    .bg(learn_more_bg)
                    .border_1()
                    .border_color(learn_more_border)
                    .text_color(body_fg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .label("了解更多")
                    .tooltip("查看本地回环与自托管 Collaborator 工作原理"),
            )
            .content(move |_, _, _| {
                div()
                    .id("collaborator-learn-more-content")
                    .test_support()
                    .w(px(360.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .text_size(theme::scaled_px(12.5))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(theme::scaled_px(13.5))
                            .child("External Collaborator 工作说明"),
                    )
                    .child(LOCAL_LOOPBACK_NOTICE)
                    .child("1. 点击「开始使用」进入工作区，按需启动本地 Loopback 监听器或直接生成 HTTP / DNS Payload。")
                    .child("2. 将复制的回调地址植入目标请求，捕获到的交互将实时显示在交互列表中。")
                    .child("3. 选中任意交互可在下方 Pretty / Raw / Hex 视图检查原始报文，并一键发送到 Repeater 重放。")
            });

        let get_started_btn = Button::new("collaborator-get-started")
            .primary()
            .small()
            .min_w(theme::scaled_px(112.))
            .h(theme::scaled_px(24.))
            .px(theme::scaled_px(16.))
            .rounded(ButtonRounded::Size(px(3.)))
            .font_weight(FontWeight::SEMIBOLD)
            .label("开始使用")
            .tooltip("进入 Collaborator 工作区生成载荷并监听交互")
            .on_click(cx.listener(|this, _, _, cx| {
                this.dismiss_collaborator_onboarding(cx);
            }));

        let dont_show_link = flat_button("collaborator-dont-show-again", false, window, cx)
            .accessibility_label("不再显示")
            .mt(theme::scaled_px(30.))
            .px(px(6.))
            .py(px(2.))
            .rounded(px(3.))
            .bg(gpui_kit::transparent_black())
            .text_size(theme::scaled_px(13.))
            .text_color(link_fg)
            .hover(|style| style.underline())
            .cursor_pointer()
            .child("不再显示")
            .on_click(cx.listener(|this, _, _, cx| {
                this.dismiss_collaborator_onboarding(cx);
            }));

        div()
            .id("collaborator-onboarding")
            .test_support()
            .size_full()
            .relative()
            .bg(theme::background())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                Button::new("collaborator-dismiss-onboarding")
                    .ghost()
                    .small()
                    .compact()
                    .w(px(26.))
                    .h(px(26.))
                    .p_0()
                    .rounded(ButtonRounded::Size(px(3.)))
                    .absolute()
                    .top(px(7.))
                    .right(px(6.))
                    .child(
                        Icon::default()
                            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 14 14\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.65\" stroke-linecap=\"round\"><path d=\"M1.85 1.85L12.15 12.15M12.15 1.85L1.85 12.15\"/></svg>")
                            .size(px(14.))
                            .text_color(body_fg),
                    )
                    .accessibility_label("关闭欢迎页")
                    .tooltip("关闭欢迎页并进入 Collaborator 工作区")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dismiss_collaborator_onboarding(cx);
                    })),
            )
            .child(
                div()
                    .w_full()
                    .max_w(theme::scaled_px(440.))
                    .px(theme::scaled_px(24.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .id("collaborator-onboarding-radar")
                            .test_support()
                            .mb(theme::scaled_px(36.))
                            .when_some(radar, |box_div, image| {
                                box_div.child(
                                    img(image)
                                        .w(theme::scaled_px(126.))
                                        .h(theme::scaled_px(83.))
                                        .flex_shrink_0(),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("collaborator-onboarding-title")
                            .test_support()
                            .mb(theme::scaled_px(24.))
                            .text_size(theme::scaled_px(20.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(heading_fg)
                            .child("External Collaborator"),
                    )
                    .child(
                        div()
                            .max_w(theme::scaled_px(365.))
                            .mb(theme::scaled_px(28.))
                            .text_center()
                            .text_size(theme::scaled_px(13.))
                            .line_height(theme::scaled_px(19.))
                            .text_color(body_fg)
                            .child(
                                "External Collaborator 可用于测试能否诱导目标应用通过 DNS、HTTP 或 SMTP 与外部服务交互，帮助您更轻松地发现盲打或异步类安全漏洞。",
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(theme::scaled_px(20.))
                            .child(learn_more_popover)
                            .child(get_started_btn),
                    )
                    .child(dont_show_link),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_collaborator_toolbar_payload_controls(
        &self,
        can_operate: bool,
        running: bool,
        count_input: &Entity<InputState>,
        label_input: &Entity<InputState>,
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
                        this.generate_collaborator_payloads(CopyMode::Http, window, cx);
                    })),
            )
            .child(
                button("collaborator-copy-dns", "复制 DNS")
                    .icon(IconName::SendHorizontal)
                    .tooltip("签发新的带外测试 Token 并将 DNS 回调域名复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.generate_collaborator_payloads(CopyMode::Dns, window, cx);
                    })),
            )
            .child(
                button("collaborator-poll-now", "立即轮询")
                    .icon(IconName::RefreshCw)
                    .tooltip("立即向本地或自托管 OAST 服务器发起一次拉取轮询")
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.poll_collaborator_now(window, cx);
                    })),
            )
            .child(divider())
            .child(
                div()
                    .flex_shrink_0()
                    .w(theme::scaled_px(48.).max(px(44.)))
                    .child(
                        Input::new(count_input)
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
                        Input::new(label_input)
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
                    this.toggle_collaborator_server(window, cx);
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
    fn render_collaborator_toolbar_filter_group(
        &self,
        protocol_filter: Option<OastProtocol>,
        search_input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                                this.set_collaborator_protocol_filter(None, window, cx);
                            })),
                    )
                    .child(
                        button("collaborator-filter-http", "HTTP")
                            .px(theme::scaled_px(8.))
                            .tooltip("仅显示 HTTP 协议回调交互记录")
                            .selected(protocol_filter == Some(OastProtocol::Http))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_collaborator_protocol_filter(
                                    Some(OastProtocol::Http),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("collaborator-filter-dns", "DNS")
                            .px(theme::scaled_px(8.))
                            .tooltip("仅显示 DNS 协议回调交互记录")
                            .selected(protocol_filter == Some(OastProtocol::Dns))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_collaborator_protocol_filter(
                                    Some(OastProtocol::Dns),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(theme::scaled_px(148.).max(px(120.)))
                    .child(
                        Input::new(search_input)
                            .id("collaborator-search")
                            .aria_label("筛选交互")
                            .small(),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_collaborator_toolbar_actions(
        &self,
        can_operate: bool,
        file_picker_pending: bool,
        config_open: bool,
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
                        this.copy_collaborator_interactions_json(window, cx);
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
                        this.export_collaborator_interactions_file(window, cx);
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
                        this.clear_collaborator_interactions(window, cx);
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
                    .selected(config_open)
                    .disabled(!can_operate)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_collaborator_config_panel(window, cx);
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_collaborator_toolbar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_operate = self.collaborator_can_operate();
        let file_picker_pending = self.file_picker_pending();
        let running = self.collaborator_pane.status.running;
        let config_open = self.collaborator_pane.config_open;
        let protocol_filter = self.collaborator_pane.protocol_filter;
        let (badge, badge_bg, badge_fg) = server_badge_text(&self.collaborator_pane.status);
        let count_input = self.collaborator_pane.count_input.clone();
        let label_input = self.collaborator_pane.label_input.clone();
        let search_input = self.collaborator_pane.search_input.clone();

        let payload_controls = self.render_collaborator_toolbar_payload_controls(
            can_operate,
            running,
            &count_input,
            &label_input,
            badge,
            badge_bg,
            badge_fg,
            cx,
        );
        let filter_group =
            self.render_collaborator_toolbar_filter_group(protocol_filter, &search_input, cx);
        let actions = self.render_collaborator_toolbar_actions(
            can_operate,
            file_picker_pending,
            config_open,
            cx,
        );

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

    pub(in crate::workbench) fn render_collaborator_payload_panel(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.collaborator_pane;
        let preview = pane
            .latest_payload_preview
            .clone()
            .unwrap_or_else(|| "尚未生成 Payload。".into());
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
        let notice = pane.notice.clone().unwrap_or_default();
        let issued_count = pane.status.issued_tokens.len();
        let drawer_open = pane.payloads_drawer_open;
        let weak = cx.weak_entity();
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
                                this.toggle_collaborator_payloads_drawer(cx);
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
                                    let Some(owner) = weak.upgrade() else {
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

    pub(in crate::workbench) fn render_collaborator_interaction_rows(
        &self,
        interactions: Vec<(usize, &OastInteraction)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let col_num = theme::scaled_px(48.);
        let col_time = theme::scaled_px(108.);
        let col_type = theme::scaled_px(76.);
        let col_ip = theme::scaled_px(156.);
        let col_payload = theme::scaled_px(210.);

        let header_cell = |label: &'static str, width: Option<gpui_kit::Pixels>, last: bool| {
            div()
                .h_full()
                .px(px(8.))
                .flex()
                .items_center()
                .when_some(width, |cell, w| cell.w(w).flex_shrink_0())
                .when(width.is_none(), |cell| cell.flex_1().min_w_0())
                .when(!last, |cell| {
                    cell.border_r_1().border_color(theme::border())
                })
                .truncate()
                .child(label)
        };

        let table_header = div()
            .id("collaborator-table-header")
            .test_support()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(header_bg)
            .text_color(header_fg)
            .text_size(theme::scaled_px(12.))
            .border_b_1()
            .border_color(theme::border())
            .child(header_cell("#", Some(col_num), false))
            .child(header_cell("时间", Some(col_time), false))
            .child(header_cell("类型", Some(col_type), false))
            .child(header_cell("Client IP", Some(col_ip), false))
            .child(header_cell("Payload", Some(col_payload), false))
            .child(header_cell("摘要", None, true));

        let table_body: AnyElement = if interactions.is_empty() {
            let empty_msg = if self.collaborator_pane.status.interactions.is_empty() {
                "尚无交互记录。点击「复制 Payload」生成载荷后，等待 HTTP 或 DNS 回调。"
            } else {
                "当前筛选条件下没有匹配的交互记录。"
            };
            div()
                .id("collaborator-interaction-list")
                .flex_1()
                .min_h_0()
                .bg(theme::surface())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .px(px(16.))
                .child(
                    div()
                        .id("collaborator-interaction-empty")
                        .test_support()
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::foreground())
                        .child(empty_msg),
                )
                .child(
                    div()
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::muted_foreground())
                        .child(LOCAL_LOOPBACK_NOTICE),
                )
                .into_any_element()
        } else {
            let selected_id = self.collaborator_pane.selected_interaction;
            let total = self.collaborator_pane.status.interactions.len();
            let weak = cx.weak_entity();
            let rows_rc = Rc::new(
                interactions
                    .into_iter()
                    .map(|(full_index, interaction)| (full_index, interaction.id))
                    .collect::<Vec<_>>(),
            );
            let row_count = rows_rc.len();
            uniform_list(
                "collaborator-interaction-list",
                row_count,
                move |range, _, cx| {
                    let Some(owner) = weak.upgrade() else {
                        return Vec::new();
                    };
                    let view = owner.read(cx);
                    let dark = theme::is_dark();
                    let zebra_bg: gpui_kit::Hsla =
                        rgb(if dark { 0x25272b } else { 0xfafafa }).into();
                    let hover_bg: gpui_kit::Hsla =
                        rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
                    range
                        .filter_map(|index| {
                            let &(full_index, interaction_id) = rows_rc.get(index)?;
                            let interaction = view
                                .collaborator_pane
                                .status
                                .interactions
                                .get(full_index)
                                .filter(|item| item.id == interaction_id)?;
                            let label = view
                                .collaborator_pane
                                .token_label(interaction.token.as_deref().unwrap_or_default())
                                .unwrap_or_default()
                                .to_owned();
                            Some((index, full_index, interaction, label))
                        })
                        .map(|(visible_index, full_index, interaction, label)| {
                            let ordinal = total - full_index;
                            let (badge, badge_bg, badge_fg) =
                                protocol_badge_style(interaction.protocol);
                            let token = interaction.token.clone().unwrap_or_default();
                            let payload_col = if label.is_empty() {
                                token.clone()
                            } else {
                                format!("{token} ({label})")
                            };
                            let selected = selected_id == Some(interaction.id);
                            let interaction_id = interaction.id;
                            let right_view = weak.clone();
                            let click_view = weak.clone();
                            let menu_view = weak.clone();
                            let row_bg = if selected {
                                theme::selected()
                            } else if visible_index % 2 == 1 {
                                zebra_bg
                            } else {
                                theme::surface()
                            };
                            div()
                                .id(SharedString::from(format!(
                                    "collaborator-interaction-{}",
                                    interaction.id
                                )))
                                .test_support()
                                .aria_label(format!(
                                    "#{} {} {} {} {}",
                                    ordinal,
                                    badge,
                                    token,
                                    interaction.peer_addr,
                                    interaction.summary
                                ))
                                .aria_selected(selected)
                                .w_full()
                                .h(theme::row_height())
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .bg(row_bg)
                                .when(!selected, |row| row.hover(move |s| s.bg(hover_bg)))
                                .border_b_1()
                                .border_color(theme::border())
                                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                                    let _ = right_view.update(cx, |this, cx| {
                                        if this.collaborator_pane.selected_interaction
                                            != Some(interaction_id)
                                        {
                                            this.select_collaborator_interaction(
                                                interaction_id,
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                })
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    let _ = click_view.update(cx, |this, cx| {
                                        this.select_collaborator_interaction(
                                            interaction_id,
                                            window,
                                            cx,
                                        );
                                    });
                                })
                                .context_menu(move |menu, _, cx| {
                                    collaborator_row_menu(menu_view.clone(), interaction_id, menu, cx)
                                })
                                .child(
                                    div()
                                        .w(col_num)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(format!("{ordinal}")),
                                )
                                .child(
                                    div()
                                        .w(col_time)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(format_local_hms(interaction.observed_unix_ms)),
                                )
                                .child(
                                    div()
                                        .w(col_type)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .flex()
                                        .items_center()
                                        .child(
                                            div()
                                                .id(("collaborator-protocol-badge", full_index))
                                                .rounded(px(8.))
                                                .px(px(6.))
                                                .py(px(1.))
                                                .text_size(theme::scaled_px(10.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .bg(rgb(badge_bg))
                                                .text_color(rgb(badge_fg))
                                                .child(badge),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(col_ip)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(interaction.peer_addr.clone()),
                                )
                                .child(
                                    div()
                                        .w(col_payload)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .child(payload_col),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(interaction.summary.clone()),
                                )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(table_header)
            .child(table_body)
            .into_any_element()
    }
}

fn collaborator_row_menu(
    view: WeakEntity<Workbench>,
    interaction_id: Uuid,
    menu: PopupMenu,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(owner) = view.upgrade() else {
        return menu;
    };
    let state = owner.read(cx);
    let can_operate = state.collaborator_can_operate();
    let interaction = state
        .collaborator_pane
        .status
        .interactions
        .iter()
        .find(|i| i.id == interaction_id);
    let has_interaction = interaction.is_some();
    let is_http = interaction.is_some_and(|i| i.protocol == OastProtocol::Http);
    let raw_empty = interaction.is_none_or(|i| i.raw_request.is_empty());
    let has_payload = interaction
        .and_then(|i| i.token.as_deref())
        .is_some_and(|t| !t.is_empty());
    let file_picker_pending = state.file_picker_pending();

    let repeater_view = view.clone();
    let intruder_view = view.clone();
    let comparer_view = view.clone();
    let decoder_view = view.clone();
    let mio_ai_view = view.clone();
    let payload_view = view.clone();
    let delete_view = view.clone();
    let save_view = view;

    menu.item(
        PopupMenuItem::new("发送到 Repeater")
            .disabled(!can_operate || !is_http || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = repeater_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_repeater(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Intruder")
            .disabled(!can_operate || !is_http || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = intruder_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_intruder(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Comparer")
            .disabled(!has_interaction || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = comparer_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_comparer(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Decoder")
            .disabled(!has_interaction || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = decoder_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_decoder(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 MioAT")
            .disabled(!has_interaction)
            .on_click(move |_, _, cx| {
                let _ = mio_ai_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_mio_ai(Some(interaction_id), cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("复制 Payload")
            .disabled(!has_payload)
            .on_click(move |_, _, cx| {
                let _ = payload_view.update(cx, |v, cx| {
                    v.copy_collaborator_interaction_payload(Some(interaction_id), cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("删除此交互")
            .disabled(!has_interaction)
            .on_click(move |_, window, cx| {
                let _ = delete_view.update(cx, |v, cx| {
                    v.delete_collaborator_interaction(interaction_id, window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("保存交互详情…")
            .disabled(!can_operate || !has_interaction || file_picker_pending)
            .on_click(move |_, window, cx| {
                let _ = save_view.update(cx, |v, cx| {
                    v.save_collaborator_interaction_details(Some(interaction_id), window, cx)
                });
            }),
    )
}
