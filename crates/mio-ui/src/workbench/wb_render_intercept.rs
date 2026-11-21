use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;

const INTERCEPT_DOCS_URL: &str =
    "https://github.com/ltlly/MIO";

// Layered isometric 3D traffic-light illustration for the Proxy / Intercept empty state.
// Each layer renders as a crisp vector alpha-mask via GPUI's native SVG painter and is tinted with its
// exact palette color, keeping zero runtime allocation and sharp rendering at 1x/2x DPI.
const TRAFFIC_LIGHT_RAYS_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="84" viewBox="0 0 120 84"><line x1="60" y1="67" x2="60" y2="81" stroke="#000000" stroke-width="4.2" stroke-linecap="round"/><line x1="17" y1="7" x2="36" y2="23" stroke="#000000" stroke-width="3.6" stroke-linecap="round"/><line x1="89" y1="6" x2="111" y2="24" stroke="#000000" stroke-width="3.6" stroke-linecap="round"/><line x1="6" y1="59" x2="27" y2="76" stroke="#000000" stroke-width="3.6" stroke-linecap="round"/><line x1="78" y1="53" x2="102" y2="73" stroke="#000000" stroke-width="3.6" stroke-linecap="round"/></svg>"##;
const TRAFFIC_LIGHT_SHELL_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="84" viewBox="0 0 120 84"><path d="M 41 61 L 41 24 C 41 18 44 14 49 11 L 63 4 C 67 2 71 3 74 6 L 76 9 L 53 71 L 47 70 C 43 69 41 66 41 61 Z" fill="#000000"/></svg>"##;
const TRAFFIC_LIGHT_FACE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="84" viewBox="0 0 120 84"><path d="M 48 63 L 48 27 C 48 21 51 17 56 14 L 70 7 C 75 5 78 8 78 14 L 78 50 C 78 56 75 60 70 63 L 56 70 C 51 72 48 69 48 63 Z" fill="#000000"/></svg>"##;
const TRAFFIC_LIGHT_BEZELS_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="84" viewBox="0 0 120 84"><ellipse cx="62.2" cy="27.5" rx="6.6" ry="9.2" transform="rotate(18 62.2 27.5)" fill="#000000"/><ellipse cx="62.2" cy="49.0" rx="6.6" ry="9.2" transform="rotate(18 62.2 49.0)" fill="#000000"/></svg>"##;
const TRAFFIC_LIGHT_TOP_LAMP_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="84" viewBox="0 0 120 84"><ellipse cx="63.7" cy="28.4" rx="5.3" ry="7.6" transform="rotate(18 63.7 28.4)" fill="#000000"/></svg>"##;
const TRAFFIC_LIGHT_BOTTOM_LAMP_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="84" viewBox="0 0 120 84"><ellipse cx="63.7" cy="49.9" rx="5.3" ry="7.6" transform="rotate(18 63.7 49.9)" fill="#000000"/></svg>"##;

#[inline(never)]
fn render_intercept_traffic_light_svg(intercept_on: bool, dark: bool) -> AnyElement {
    let rays_color: gpui_kit::Hsla = rgb(if dark { 0x3a3d42 } else { 0xe3e3e3 }).into();
    let shell_color: gpui_kit::Hsla = rgb(if dark { 0x3a76b0 } else { 0x2f6499 }).into();
    let face_color: gpui_kit::Hsla = rgb(if dark { 0x4f6275 } else { 0xbfd0e0 }).into();
    let bezel_color: gpui_kit::Hsla = rgb(if dark { 0x25374a } else { 0x2f6499 }).into();
    let lamp_off: gpui_kit::Hsla = rgb(if dark { 0x6a7887 } else { 0xa1b0bd }).into();
    let lamp_green: gpui_kit::Hsla = rgb(if dark { 0x57b33b } else { 0x4e9f33 }).into();
    let lamp_orange: gpui_kit::Hsla = rgb(0xdb612f).into();
    let (top_color, bottom_color) = if intercept_on {
        (lamp_orange, lamp_off)
    } else {
        (lamp_off, lamp_green)
    };
    let layer = |bytes: &'static [u8], color: gpui_kit::Hsla| {
        gpui_kit::svg()
            .data(bytes)
            .absolute()
            .inset_0()
            .size_full()
            .text_color(color)
    };
    div()
        .relative()
        .w(theme::scaled_px(108.))
        .h(theme::scaled_px(76.))
        .flex_shrink_0()
        .child(layer(TRAFFIC_LIGHT_RAYS_SVG, rays_color))
        .child(layer(TRAFFIC_LIGHT_SHELL_SVG, shell_color))
        .child(layer(TRAFFIC_LIGHT_FACE_SVG, face_color))
        .child(layer(TRAFFIC_LIGHT_BEZELS_SVG, bezel_color))
        .child(layer(TRAFFIC_LIGHT_TOP_LAMP_SVG, top_color))
        .child(layer(TRAFFIC_LIGHT_BOTTOM_LAMP_SVG, bottom_color))
        .into_any_element()
}

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_proxy(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_page = self.proxy_page;
        let intercept_tab = wb_render_chrome::secondary_tab(
            "proxy-intercept",
            "Intercept",
            selected_page == ProxyPage::Intercept,
            window,
            cx,
        )
        .w(theme::scaled_px(84.))
        .px_0()
        .justify_center()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "拦截并审查、修改流经 Proxy 的实时 HTTP 与 WebSocket 报文",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.proxy_page = ProxyPage::Intercept;
            cx.notify();
        }));
        let history_tab = wb_render_chrome::secondary_tab(
            "proxy-history",
            "HTTP 历史记录",
            selected_page == ProxyPage::History,
            window,
            cx,
        )
        .w(theme::scaled_px(107.))
        .px_0()
        .justify_center()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "查看、筛选与批注通过 Proxy 捕获的全部 HTTP 请求与响应记录",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.proxy_page = ProxyPage::History;
            cx.notify();
        }));
        let ws_history_tab = wb_render_chrome::secondary_tab(
            "proxy-websocket-history",
            "WebSockets 历史记录",
            selected_page == ProxyPage::WebSocketHistory,
            window,
            cx,
        )
        .w(theme::scaled_px(150.))
        .px_0()
        .justify_center()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "查看与筛选通过 Proxy 捕获的双向 WebSocket 数据帧记录",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.proxy_page = ProxyPage::WebSocketHistory;
            this.refresh_websocket_history(window, cx);
            cx.notify();
        }));
        let match_replace_tab = wb_render_chrome::secondary_tab(
            "proxy-match-replace",
            "Match and replace",
            selected_page == ProxyPage::MatchAndReplace,
            window,
            cx,
        )
        .w(theme::scaled_px(142.))
        .px_0()
        .justify_center()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "配置流经 Proxy 时自动执行的 HTTP 请求/响应头与正文匹配替换规则",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.switch_proxy_page(ProxyPage::MatchAndReplace, window, cx)
        }));
        let dark = theme::is_dark();
        let settings_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x2f3237).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let proxy_settings_btn = flat_button("proxy-settings", true, window, cx)
            .accessibility_label("Proxy settings")
            .w(theme::scaled_px(111.))
            .h(px(35.))
            .self_start()
            .flex_shrink_0()
            .px(theme::scaled_px(10.))
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .font_weight(gpui_kit::FontWeight::NORMAL)
            .line_height(theme::scaled_px(17.))
            .text_color(theme::foreground())
            .flex()
            .items_center()
            .gap(theme::scaled_px(6.))
            .bg(theme::toolbar_surface())
            .hover(move |s| s.bg(settings_hover_bg))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("打开 Proxy 设置")
                    .build(window, cx)
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_settings_filtered(
                    Some(crate::workbench::settings::SettingsPage::Proxy),
                    None,
                    false,
                    window,
                    cx,
                );
            }))
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(theme::scaled_px(14.))
                    .text_color(theme::foreground()),
            )
            .child("Proxy settings");
        let content = match self.proxy_page {
            ProxyPage::Intercept => self.render_intercept(cx),
            ProxyPage::History => self.render_history(window, cx),
            ProxyPage::WebSocketHistory => self.render_websocket_history(cx),
            ProxyPage::MatchAndReplace => self.render_match_and_replace(window, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                wb_render_chrome::secondary_tab_bar()
                    .child(intercept_tab)
                    .child(history_tab)
                    .child(ws_history_tab)
                    .child(match_replace_tab)
                    .child(
                        div()
                            .w(px(26.))
                            .h(px(35.))
                            .self_start()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .pl(px(5.))
                            .child(div().w(px(1.)).h(px(25.)).bg(theme::border())),
                    )
                    .child(proxy_settings_btn),
            )
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }

    fn open_intercept_documentation(&mut self, cx: &mut Context<Self>) {
        cx.open_url(INTERCEPT_DOCS_URL);
        self.error = None;
        self.status = format!("已打开 Proxy Intercept 文档：{INTERCEPT_DOCS_URL}");
        cx.notify();
    }

    #[inline(never)]
    fn build_intercept_action_menu(
        owner: gpui_kit::WeakEntity<Self>,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(entity) = owner.upgrade() else {
            return menu;
        };
        let (
            enabled,
            is_ws,
            is_req_stage,
            has_resp,
            resp_annotated,
            has_host,
            has_ext,
            can_toggle_method,
            flow_id_opt,
            host_label,
            project_opt,
        ) = {
            let view = entity.read(cx);
            let base_ok = view.project.is_some()
                && view.backend_available()
                && view.transition.is_none();
            let cur = view
                .selected_intercept
                .and_then(|id| view.intercepts.iter().find(|p| p.id() == id));
            match cur {
                Some(p) if base_ok && !p.deciding => match &p.target {
                    InterceptTarget::Http { stage, record, .. } => {
                        let req_stage = *stage == InterceptStage::Request;
                        let flow_id = record.summary.id;
                        let url = &record.request.url;
                        let host = super::wb_proxy::extract_url_host(url);
                        let ext = super::wb_proxy::extract_url_file_extension(url);
                        (
                            true,
                            false,
                            req_stage,
                            *stage == InterceptStage::Response || record.response.is_some(),
                            view.annotated_live_flows.contains(&flow_id),
                            host.is_some(),
                            ext.is_some(),
                            req_stage && !p.editor.is_readonly_preview(),
                            Some(flow_id),
                            host.unwrap_or_default(),
                            view.project,
                        )
                    }
                    InterceptTarget::WebSocket { url, .. } => (
                        true,
                        true,
                        false,
                        false,
                        false,
                        !url.is_empty(),
                        false,
                        false,
                        None,
                        String::new(),
                        view.project,
                    ),
                },
                _ => (
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    None,
                    String::new(),
                    None,
                ),
            }
        };
        let item = |menu: PopupMenu,
                    label: &'static str,
                    item_enabled: bool,
                    action: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
            let owner = owner.clone();
            menu.item(
                PopupMenuItem::new(label)
                    .disabled(!item_enabled)
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            action(view, window, cx);
                        });
                    }),
            )
        };
        if is_ws {
            let ws_url_owner = owner.clone();
            let ws_in_owner = owner.clone();
            let ws_ex_owner = owner.clone();
            let menu = item(
                menu,
                "发送到 WebSocket Repeater",
                enabled,
                Workbench::send_intercept_websocket_to_repeater,
            );
            let menu = item(
                menu,
                "发送到 Decoder",
                enabled,
                Workbench::send_intercept_websocket_to_decoder,
            );
            return item(
                menu,
                "发送到 Comparer",
                enabled,
                Workbench::send_intercept_websocket_to_comparer,
            )
            .separator()
            .item(
                PopupMenuItem::new("复制 URL")
                    .disabled(!enabled || !has_host)
                    .on_click(move |_, _, cx| {
                        let _ = ws_url_owner.update(cx, |view, cx| {
                            view.copy_intercept_websocket_url(cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("添加到范围")
                    .disabled(!enabled || !has_host)
                    .on_click(move |_, _, cx| {
                        let _ = ws_in_owner.update(cx, |view, cx| {
                            view.modify_intercept_websocket_scope(true, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("从范围排除")
                    .disabled(!enabled || !has_host)
                    .on_click(move |_, _, cx| {
                        let _ = ws_ex_owner.update(cx, |view, cx| {
                            view.modify_intercept_websocket_scope(false, cx);
                        });
                    }),
            );
        }
        let menu = if is_req_stage {
            let resp_owner = owner.clone();
            let host_owner = owner.clone();
            let ext_owner = owner.clone();
            menu.item(
                PopupMenuItem::new("拦截此请求的响应 (Do intercept response)")
                    .checked(resp_annotated)
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| {
                        let _ = resp_owner.update(cx, |view, cx| {
                            view.toggle_intercept_response_for_selected(cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("不再拦截此主机的请求")
                    .disabled(!enabled || !has_host)
                    .on_click(move |_, _, cx| {
                        let _ = host_owner.update(cx, |view, cx| {
                            view.add_do_not_intercept_host_rule(cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("不再拦截此文件扩展名的请求")
                    .disabled(!enabled || !has_ext)
                    .on_click(move |_, _, cx| {
                        let _ = ext_owner.update(cx, |view, cx| {
                            view.add_do_not_intercept_extension_rule(cx);
                        });
                    }),
            )
            .separator()
        } else {
            menu
        };
        let menu = item(
            menu,
            "发送到 Repeater",
            enabled,
            Workbench::copy_draft_to_repeater,
        );
        let menu = item(
            menu,
            "发送到 Intruder",
            enabled,
            Workbench::open_in_intruder,
        );
        let menu = item(
            menu,
            "发起主动扫描 (Scan)",
            enabled,
            Workbench::send_to_scanner,
        );
        let menu = item(
            menu,
            "发起被动扫描 (Passive scan)",
            enabled,
            Workbench::send_to_passive_scanner,
        );
        let menu = item(
            menu,
            "发送到 Sequencer",
            enabled,
            Workbench::send_to_sequencer,
        );
        let menu = item(
            menu,
            "发送到 Comparer (请求)",
            enabled,
            |view, window, cx| view.send_to_comparer(false, window, cx),
        );
        let menu = item(
            menu,
            "发送到 Comparer (响应)",
            enabled && has_resp,
            |view, window, cx| view.send_to_comparer(true, window, cx),
        );
        let menu = item(
            menu,
            "发送到 Decoder",
            enabled,
            Workbench::send_to_decoder,
        );
        let menu = item(
            menu,
            "发送到 Organiser",
            enabled,
            Workbench::send_selected_to_organiser,
        )
        .separator();
        let menu = item(
            menu,
            "切换请求方法",
            enabled && can_toggle_method,
            Workbench::change_request_method,
        );
        let menu = if let (Some(flow_id), Some(project)) = (flow_id_opt, project_opt) {
            let hl_owner = owner.clone();
            let submenu = PopupMenu::build(window, cx, |sub, _, _| {
                (0..=9u8).fold(sub.min_w(theme::scaled_px(88.)), |sub, color| {
                    let hl_owner = hl_owner.clone();
                    let host_label = host_label.clone();
                    let color_name = [
                        "None", "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Pink",
                        "Magenta", "Gray",
                    ][usize::from(color)];
                    sub.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .id(("intercept-highlight-color", usize::from(color)))
                                .test_support()
                                .role(gpui_kit::accesskit::Role::MenuItem)
                                .aria_label(format!("Highlight {color_name}"))
                                .w_full()
                                .h(theme::scaled_px(26.).max(px(26.)))
                                .mx_neg_2()
                                .px(theme::scaled_px(8.))
                                .flex()
                                .items_center()
                                .gap(theme::scaled_px(6.))
                                .when_some(
                                    crate::history::highlight_background(color),
                                    |this, bg| this.bg(bg),
                                )
                                .text_color(crate::history::highlight_foreground(color))
                                .child(
                                    div()
                                        .w(theme::scaled_px(10.))
                                        .h(theme::scaled_px(10.))
                                        .when(color == 0, |this| {
                                            this.border_1()
                                                .border_dashed()
                                                .border_color(rgb(0x888888))
                                        }),
                                )
                                .child(if host_label.is_empty() {
                                    color_name.to_owned()
                                } else {
                                    host_label.clone()
                                })
                        })
                        .on_click(move |_, _, cx| {
                            let _ = hl_owner.update(cx, |view, cx| {
                                if view.project == Some(project) {
                                    view.set_http_highlights(
                                        &[HistoryEntryId::Live(flow_id)],
                                        color,
                                        cx,
                                    );
                                }
                            });
                        }),
                    )
                })
            });
            menu.item(PopupMenuItem::submenu("Highlight", submenu))
        } else {
            menu
        }
        .separator();
        let menu = item(menu, "复制 URL", enabled, Workbench::copy_url);
        let menu = item(menu, "复制为 curl 命令", enabled, Workbench::copy_curl_command);
        let menu = item(menu, "添加到范围", enabled, Workbench::add_to_scope);
        item(menu, "从范围排除", enabled, Workbench::exclude_from_scope)
    }

    #[inline(never)]
    fn render_intercept_empty_actions(&self, dark: bool, cx: &mut Context<Self>) -> AnyElement {
        let browser_disabled = self.ext_import.visible
            || self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some();

        let learn_bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xececec }).into();
        let learn_hover: gpui_kit::Hsla = rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into();
        let learn_border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xdfdfdf }).into();
        let learn_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x2b2b2b).into()
        };

        let (cta_bg, cta_hover, cta_border, cta_fg): (
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
        ) = if browser_disabled {
            (
                rgb(if dark { 0x303338 } else { 0xe4e4e4 }).into(),
                rgb(if dark { 0x303338 } else { 0xe4e4e4 }).into(),
                rgb(if dark { 0x43474e } else { 0xdfdfdf }).into(),
                rgb(if dark { 0x7d828a } else { 0x666666 }).into(),
            )
        } else {
            (
                rgb(0xdb612f).into(),
                rgb(0xc45122).into(),
                rgb(0xdb612f).into(),
                rgb(0xffffff).into(),
            )
        };

        let btn_h = theme::scaled_px(24.);

        let learn_more_btn = Button::new("intercept-empty-learn-more")
            .ghost()
            .small()
            .compact()
            .p_0()
            .w(theme::scaled_px(110.))
            .h(btn_h)
            .rounded(px(3.))
            .border_1()
            .bg(learn_bg)
            .border_color(learn_border)
            .text_color(learn_fg)
            .hover(move |s| s.bg(learn_hover).border_color(learn_border).text_color(learn_fg))
            .accessibility_label("了解更多")
            .tooltip("查看 Proxy Intercept 工作流程与配置文档")
            .on_click(cx.listener(|this, _, _, cx| this.open_intercept_documentation(cx)))
            .child(
                div()
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(learn_fg)
                    .child("了解更多"),
            );

        let open_browser_btn = Button::new("intercept-empty-open-browser")
            .ghost()
            .small()
            .compact()
            .p_0()
            .w(theme::scaled_px(120.))
            .h(btn_h)
            .rounded(px(3.))
            .border_1()
            .bg(cta_bg)
            .border_color(cta_border)
            .text_color(cta_fg)
            .when(!browser_disabled, |b| {
                b.hover(move |s| s.bg(cta_hover).border_color(cta_hover).text_color(cta_fg))
            })
            .disabled(browser_disabled)
            .accessibility_label("打开浏览器")
            .tooltip("启动已配置 Proxy 监听地址与 CA 信任的浏览器")
            .on_click(cx.listener(|this, _, _, cx| this.open_configured_browser(cx)))
            .child(
                div()
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(cta_fg)
                    .child("打开浏览器"),
            );

        div()
            .mt(theme::scaled_px(18.))
            .flex()
            .items_center()
            .justify_center()
            .gap(theme::scaled_px(20.))
            .child(learn_more_btn)
            .child(open_browser_btn)
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let intercept_on = self.intercept_requests || self.intercept_responses;

        let heading = if intercept_on {
            "正在等待拦截流量…"
        } else {
            "Intercept 已关闭"
        };
        let (desc_line_1, desc_line_2): (SharedString, SharedString) = if intercept_on {
            (
                "Intercept 已开启，符合拦截规则的请求与响应报文将在此挂起等待处理。".into(),
                self.proxy_address
                    .map_or_else(
                        || {
                            "启动代理监听后，打开浏览器或将客户端代理指向监听地址即可捕获流量。"
                                .into()
                        },
                        |address| {
                            format!(
                                "当前代理监听地址：{address}，HTTPS 流量需要显式信任导出的 CA 证书。"
                            )
                            .into()
                        },
                    ),
            )
        } else {
            (
                "开启 Intercept 后，浏览器与目标服务器之间的通信报文将在此暂停。".into(),
                "你可以在转发之前对这些请求和响应报文进行分析与修改。".into(),
            )
        };

        div()
            .id("intercept-empty-state")
            .test_support()
            .size_full()
            .bg(theme::background())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pb(theme::scaled_px(36.))
            .px(theme::scaled_px(24.))
            .child(render_intercept_traffic_light_svg(intercept_on, dark))
            .child(
                div()
                    .mt(theme::scaled_px(24.))
                    .font_family(theme::ui_font_family())
                    .text_size(theme::scaled_px(16.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(if dark {
                        theme::foreground()
                    } else {
                        rgb(0x000000).into()
                    })
                    .child(heading),
            )
            .child(
                div()
                    .mt(theme::scaled_px(16.))
                    .max_w(theme::scaled_px(460.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(theme::scaled_px(3.))
                    .font_family(theme::ui_font_family())
                    .text_size(theme::scaled_px(13.))
                    .line_height(theme::scaled_px(19.))
                    .text_color(if dark {
                        theme::muted_foreground()
                    } else {
                        rgb(0x2b2b2b).into()
                    })
                    .text_center()
                    .child(desc_line_1)
                    .child(desc_line_2),
            )
            .child(self.render_intercept_empty_actions(dark, cx))
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_toggle_button(
        &self,
        intercept_on: bool,
        toggle_disabled: bool,
        dark: bool,
        btn_h: gpui_kit::Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle_label = if intercept_on {
            "Intercept 已开启"
        } else {
            "Intercept 已关闭"
        };
        let (toggle_bg, toggle_hover, toggle_border, toggle_fg): (
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
        ) = if intercept_on {
            if toggle_disabled {
                (
                    rgb(0x5c7f9e).into(),
                    rgb(0x5c7f9e).into(),
                    rgb(0x5c7f9e).into(),
                    rgb(0xe6eef5).into(),
                )
            } else {
                (
                    rgb(0x26649d).into(),
                    rgb(0x1f5587).into(),
                    rgb(0x26649d).into(),
                    rgb(0xffffff).into(),
                )
            }
        } else if toggle_disabled {
            (
                rgb(if dark { 0x2d3034 } else { 0xededed }).into(),
                rgb(if dark { 0x2d3034 } else { 0xededed }).into(),
                rgb(if dark { 0x43474e } else { 0xdfdfdf }).into(),
                rgb(if dark { 0x6c7078 } else { 0x8c8c8c }).into(),
            )
        } else {
            (
                rgb(if dark { 0x33363a } else { 0xededed }).into(),
                rgb(if dark { 0x3e4247 } else { 0xe3e3e3 }).into(),
                rgb(if dark { 0x4e535b } else { 0xdfdfdf }).into(),
                if dark {
                    theme::foreground()
                } else {
                    rgb(0x2b2b2b).into()
                },
            )
        };
        Button::new("intercept-toggle")
            .ghost()
            .small()
            .compact()
            .p_0()
            .w(theme::scaled_px(180.))
            .h(btn_h)
            .rounded(px(3.))
            .border_1()
            .bg(toggle_bg)
            .border_color(toggle_border)
            .text_color(toggle_fg)
            .when(!toggle_disabled, |b| {
                b.hover(move |s| {
                    s.bg(toggle_hover)
                        .border_color(toggle_border)
                        .text_color(toggle_fg)
                })
            })
            .selected(intercept_on)
            .toggled(intercept_on)
            .disabled(toggle_disabled)
            .accessibility_label(toggle_label)
            .tooltip(if intercept_on {
                "总拦截开关已开启：命中拦截规则的报文将在此挂起等待手动决策（点击切换为自动放行）"
            } else {
                "总拦截开关已关闭：所有流经 Proxy 的流量均自动放行并记录至历史（点击开启手动拦截）"
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_intercept(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(theme::scaled_px(7.))
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(toggle_fg)
                    .child(
                        Icon::new(if intercept_on {
                            IconName::ToggleRight
                        } else {
                            IconName::ToggleLeft
                        })
                        .size(theme::scaled_px(16.))
                        .text_color(toggle_fg),
                    )
                    .child(toggle_label),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_decision_split(
        &self,
        is_drop: bool,
        can_decide: bool,
        can_decide_any: bool,
        dark: bool,
        btn_h: gpui_kit::Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let split_inner_h = btn_h - px(2.);
        let (bg, hover, border, divider, fg, menu_bg, menu_hover, menu_fg): (
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
            gpui_kit::Hsla,
        ) = if is_drop {
            if can_decide {
                let b: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xffffff }).into();
                let h: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xf2f2f2 }).into();
                let br: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xd5d5d5 }).into();
                let dv: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xdfdfdf }).into();
                let f: gpui_kit::Hsla = if dark {
                    theme::foreground()
                } else {
                    rgb(0x2b2b2b).into()
                };
                let mf: gpui_kit::Hsla = if dark {
                    theme::foreground()
                } else {
                    rgb(0x4b4b4b).into()
                };
                (b, h, br, dv, f, b, h, mf)
            } else {
                let b: gpui_kit::Hsla = rgb(if dark { 0x26282b } else { 0xffffff }).into();
                let br: gpui_kit::Hsla = rgb(if dark { 0x3c3f41 } else { 0xdfdfdf }).into();
                let f: gpui_kit::Hsla = rgb(if dark { 0x7d828a } else { 0x666666 }).into();
                let mf: gpui_kit::Hsla = rgb(if dark { 0x6c7078 } else { 0x999999 }).into();
                (b, b, br, br, f, b, b, mf)
            }
        } else {
            let (b, h, br, dv, f): (
                gpui_kit::Hsla,
                gpui_kit::Hsla,
                gpui_kit::Hsla,
                gpui_kit::Hsla,
                gpui_kit::Hsla,
            ) = if can_decide {
                (
                    rgb(0xdb612f).into(),
                    rgb(0xc45122).into(),
                    rgb(0xdb612f).into(),
                    gpui_kit::Hsla::from(rgb(0xffffff)).opacity(0.28),
                    rgb(0xffffff).into(),
                )
            } else {
                (
                    rgb(if dark { 0x303338 } else { 0xdfdfdf }).into(),
                    rgb(if dark { 0x303338 } else { 0xdfdfdf }).into(),
                    rgb(if dark { 0x43474e } else { 0xdfdfdf }).into(),
                    rgb(if dark { 0x43474e } else { 0xcfcfcf }).into(),
                    rgb(if dark { 0x7d828a } else { 0x666666 }).into(),
                )
            };
            let (mb, mh, mf) = if can_decide {
                (b, h, f)
            } else if can_decide_any {
                (
                    rgb(if dark { 0x33363a } else { 0xededed }).into(),
                    rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into(),
                    if dark {
                        theme::foreground()
                    } else {
                        rgb(0x2b2b2b).into()
                    },
                )
            } else {
                (b, b, rgb(if dark { 0x6c7078 } else { 0x888888 }).into())
            };
            (b, h, br, dv, f, mb, mh, mf)
        };
        let (btn_id, menu_id, label, tip, menu_label, menu_tip, one_label, all_label) = if is_drop
        {
            (
                "intercept-drop",
                "intercept-drop-menu",
                "丢弃",
                "丢弃当前挂起的拦截报文并终止本次传输，不向目标端发送数据",
                "丢弃选项",
                "更多丢弃选项与报文操作",
                "丢弃当前报文 (Drop)",
                "全部丢弃 (Drop all)",
            )
        } else {
            (
                "intercept-forward",
                "intercept-forward-menu",
                "转发",
                "放行当前挂起的拦截报文并连同编辑器内所做修改继续传输",
                "转发选项",
                "更多转发选项（全部转发）",
                "转发当前报文 (Forward)",
                "全部转发 (Forward all)",
            )
        };
        let menu_owner = cx.weak_entity();
        div()
            .h(btn_h)
            .w(theme::scaled_px(180.))
            .flex()
            .items_center()
            .flex_shrink_0()
            .rounded(px(3.))
            .border_1()
            .border_color(border)
            .bg(bg)
            .child(
                Button::new(btn_id)
                    .ghost()
                    .small()
                    .compact()
                    .p_0()
                    .flex_1()
                    .h(split_inner_h)
                    .rounded(ButtonRounded::None)
                    .rounded_tl(px(2.))
                    .rounded_bl(px(2.))
                    .border_0()
                    .bg(bg)
                    .text_color(fg)
                    .when(can_decide, |b| b.hover(move |s| s.bg(hover).text_color(fg)))
                    .accessibility_label(label)
                    .tooltip(tip)
                    .disabled(!can_decide)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.decide(is_drop, window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(theme::scaled_px(6.))
                            .font_family(theme::ui_font_family())
                            .text_size(theme::ui_font_size())
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(fg)
                            .when(!is_drop, |d| {
                                d.child(
                                    Icon::new(IconName::ArrowRight)
                                        .size(theme::scaled_px(13.))
                                        .text_color(fg),
                                )
                            })
                            .child(label),
                    ),
            )
            .when(is_drop || can_decide || can_decide_any, |split| {
                split.child(
                    div()
                        .w(px(1.))
                        .h(split_inner_h)
                        .flex_shrink_0()
                        .bg(divider),
                )
            })
            .child(
                Button::new(menu_id)
                    .ghost()
                    .small()
                    .compact()
                    .p_0()
                    .min_w_0()
                    .w(theme::scaled_px(28.))
                    .h(split_inner_h)
                    .rounded(ButtonRounded::None)
                    .rounded_tr(px(2.))
                    .rounded_br(px(2.))
                    .border_0()
                    .bg(menu_bg)
                    .text_color(menu_fg)
                    .when(can_decide_any, |b| {
                        b.hover(move |s| s.bg(menu_hover).text_color(menu_fg))
                    })
                    .accessibility_label(menu_label)
                    .tooltip(menu_tip)
                    .disabled(!can_decide_any)
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(theme::scaled_px(13.))
                            .text_color(menu_fg),
                    )
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
                        let Some(entity) = menu_owner.upgrade() else {
                            return menu;
                        };
                        let (menu_can_decide, menu_can_decide_any, menu_can_action) = {
                            let view = entity.read(cx);
                            let base_ok = view.backend_available() && view.transition.is_none();
                            let cur = view.selected_intercept.and_then(|id| {
                                view.intercepts.iter().find(|p| p.id() == id)
                            });
                            let decide = base_ok && cur.is_some_and(|p| !p.deciding);
                            let decide_any =
                                base_ok && view.intercepts.iter().any(|p| !p.deciding);
                            let action = is_drop
                                && decide
                                && view.project.is_some()
                                && cur.is_some_and(|p| {
                                    p.is_http()
                                        || matches!(p.target, InterceptTarget::WebSocket { .. })
                                });
                            (decide, decide_any, action)
                        };
                        let one_owner = menu_owner.clone();
                        let all_owner = menu_owner.clone();
                        let menu = menu
                            .item(
                                PopupMenuItem::new(one_label)
                                    .disabled(!menu_can_decide)
                                    .on_click(move |_, window, cx| {
                                        let _ = one_owner.update(cx, |view, cx| {
                                            view.decide(is_drop, window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(all_label)
                                    .disabled(!menu_can_decide_any)
                                    .on_click(move |_, window, cx| {
                                        let _ = all_owner.update(cx, |view, cx| {
                                            view.decide_all(is_drop, window, cx);
                                        });
                                    }),
                            );
                        if menu_can_action {
                            let actions_owner = menu_owner.clone();
                            let actions_submenu =
                                PopupMenu::build(window, cx, move |sub, window, cx| {
                                    Workbench::build_intercept_action_menu(
                                        actions_owner.clone(),
                                        sub,
                                        window,
                                        cx,
                                    )
                                });
                            menu.separator()
                                .item(PopupMenuItem::submenu("操作 (Actions)", actions_submenu))
                        } else {
                            menu
                        }
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_help_popover(
        &self,
        icon_color: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let help_owner = cx.weak_entity();
        Popover::new("intercept-help-popover")
            .anchor(gpui_kit::Anchor::TopRight)
            .trigger(
                Button::new("intercept-help")
                    .ghost()
                    .small()
                    .compact()
                    .p_0()
                    .min_w_0()
                    .border_0()
                    .size(theme::scaled_px(22.))
                    .rounded(theme::scaled_px(11.))
                    .accessibility_label("帮助")
                    .tooltip("查看 Proxy Intercept 使用说明")
                    .child(
                        Icon::new(IconName::CircleQuestionMark)
                            .size(theme::scaled_px(18.))
                            .text_color(icon_color),
                    ),
            )
            .content(move |_, _, _| {
                let doc_owner = help_owner.clone();
                div()
                    .id("intercept-help-content")
                    .test_support()
                    .w(theme::scaled_px(300.))
                    .flex()
                    .flex_col()
                    .gap(theme::scaled_px(6.))
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Proxy Intercept"),
                    )
                    .child("开启 Intercept 后，流经 Proxy 且命中拦截规则的 HTTP 与 WebSocket 报文将暂停于此。")
                    .child("可直接编辑原始或格式化内容，随后点击「转发」放行或点击「丢弃」中止传输。")
                    .child("点击「转发」或「丢弃」右侧下拉箭头可批量执行全部转发或全部丢弃。")
                    .child(
                        div().pt(theme::scaled_px(4.)).child(
                            button("intercept-help-open-docs", "了解更多").on_click(
                                move |_, _, cx| {
                                    let _ = doc_owner.update(cx, |view, cx| {
                                        view.open_intercept_documentation(cx);
                                    });
                                },
                            ),
                        ),
                    )
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_options_button(
        &self,
        icon_color: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options_owner = cx.weak_entity();
        Button::new("intercept-options-menu")
            .ghost()
            .small()
            .compact()
            .p_0()
            .min_w_0()
            .border_0()
            .size(theme::scaled_px(22.))
            .rounded(px(3.))
            .accessibility_label("Intercept 选项")
            .tooltip("Proxy Intercept 设置与选项")
            .child(
                Icon::new(IconName::EllipsisVertical)
                    .size(theme::scaled_px(18.))
                    .text_color(icon_color),
            )
            .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, cx| {
                let Some(entity) = options_owner.upgrade() else {
                    return menu;
                };
                let (menu_on, menu_toggle_disabled, menu_browser_disabled, menu_can_decide_any) = {
                    let view = entity.read(cx);
                    let base_disabled = view.project.is_none()
                        || !view.backend_available()
                        || view.transition.is_some();
                    (
                        view.intercept_requests || view.intercept_responses,
                        base_disabled,
                        view.ext_import.visible || base_disabled,
                        !base_disabled && view.intercepts.iter().any(|p| !p.deciding),
                    )
                };
                let t_owner = options_owner.clone();
                let b_owner = options_owner.clone();
                let fa_owner = options_owner.clone();
                let da_owner = options_owner.clone();
                let s_owner = options_owner.clone();
                let d_owner = options_owner.clone();
                menu.item(
                    PopupMenuItem::new(if menu_on {
                        "关闭 Intercept"
                    } else {
                        "开启 Intercept"
                    })
                    .disabled(menu_toggle_disabled)
                    .on_click(move |_, _, cx| {
                        let _ = t_owner.update(cx, |view, cx| {
                            view.toggle_intercept(cx);
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new("打开浏览器 (Open browser)")
                        .disabled(menu_browser_disabled)
                        .on_click(move |_, _, cx| {
                            let _ = b_owner.update(cx, |view, cx| {
                                view.open_configured_browser(cx);
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("全部转发 (Forward all)")
                        .disabled(!menu_can_decide_any)
                        .on_click(move |_, window, cx| {
                            let _ = fa_owner.update(cx, |view, cx| {
                                view.decide_all(false, window, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("全部丢弃 (Drop all)")
                        .disabled(!menu_can_decide_any)
                        .on_click(move |_, window, cx| {
                            let _ = da_owner.update(cx, |view, cx| {
                                view.decide_all(true, window, cx);
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("Proxy settings…").on_click(
                    move |_, window, cx| {
                        let _ = s_owner.update(cx, |view, cx| {
                            view.open_settings_filtered(
                                Some(crate::workbench::settings::SettingsPage::Proxy),
                                None,
                                false,
                                window,
                                cx,
                            );
                        });
                    },
                ))
                .item(
                    PopupMenuItem::new("了解更多 (Proxy Intercept 文档)").on_click(
                        move |_, _, cx| {
                            let _ = d_owner.update(cx, |view, cx| {
                                view.open_intercept_documentation(cx);
                            });
                        },
                    ),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_right_controls(
        &self,
        browser_disabled: bool,
        dark: bool,
        btn_h: gpui_kit::Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let browser_bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xededed }).into();
        let browser_hover: gpui_kit::Hsla = rgb(if dark { 0x3e4247 } else { 0xe3e3e3 }).into();
        let browser_border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xdfdfdf }).into();
        let browser_fg: gpui_kit::Hsla = if browser_disabled {
            rgb(if dark { 0x6c7078 } else { 0x8c8c8c }).into()
        } else if dark {
            theme::foreground()
        } else {
            rgb(0x2b2b2b).into()
        };
        let open_browser_toolbar_btn = Button::new("intercept-open-browser")
            .ghost()
            .small()
            .compact()
            .min_w(theme::scaled_px(144.))
            .px(theme::scaled_px(14.))
            .h(btn_h)
            .rounded(px(3.))
            .border_1()
            .bg(browser_bg)
            .border_color(browser_border)
            .text_color(browser_fg)
            .when(!browser_disabled, |b| {
                b.hover(move |s| {
                    s.bg(browser_hover)
                        .border_color(browser_border)
                        .text_color(browser_fg)
                })
            })
            .disabled(browser_disabled)
            .accessibility_label("打开浏览器")
            .tooltip("启动已配置 Proxy 监听地址与 CA 信任的浏览器")
            .on_click(cx.listener(|this, _, _, cx| this.open_configured_browser(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(theme::scaled_px(7.))
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(browser_fg)
                    .child(
                        Icon::new(IconName::Globe)
                            .size(theme::scaled_px(14.))
                            .text_color(browser_fg),
                    )
                    .child("打开浏览器"),
            );

        let help_icon_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x2b2b2b).into()
        };

        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(theme::scaled_px(14.))
            .child(open_browser_toolbar_btn)
            .child(self.render_intercept_help_popover(help_icon_color, cx))
            .child(self.render_intercept_options_button(help_icon_color, cx))
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_active_actions(
        &self,
        pending: &PendingIntercept,
        can_decide: bool,
        can_action: bool,
        btn_h: gpui_kit::Pixels,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut actions = Vec::with_capacity(3);
        let action_owner = cx.weak_entity();
        actions.push(
            button("intercept-action-menu", "操作")
                .h(btn_h)
                .icon(IconName::SlidersHorizontal)
                .dropdown_caret(true)
                .tooltip("将当前拦截报文转发至其他安全测试工具、复制为命令或调整目标范围")
                .disabled(!can_action)
                .dropdown_menu(move |menu, window, cx| {
                    Workbench::build_intercept_action_menu(
                        action_owner.clone(),
                        menu,
                        window,
                        cx,
                    )
                })
                .into_any_element(),
        );
        if matches!(pending.target, InterceptTarget::WebSocket { .. }) {
            actions.push(
                button("intercept-ws-to-repeater", "发送到 Repeater")
                    .h(btn_h)
                    .icon(IconName::Repeat)
                    .tooltip("将当前拦截的 WebSocket 会话与帧内容发送到 WebSocket Repeater")
                    .disabled(!can_decide || self.project.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send_intercept_websocket_to_repeater(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if let InterceptTarget::Http {
            stage: InterceptStage::Request,
            record,
            ..
        } = &pending.target
        {
            let flow_id = record.summary.id;
            let annotated = self.annotated_live_flows.contains(&flow_id);
            actions.push(
                button(
                    "intercept-mark-annotated",
                    if annotated { "已标记注解" } else { "标记注解" },
                )
                .h(btn_h)
                .icon(IconName::Tag)
                .selected(annotated)
                .tooltip("标记此请求为已注解流量，供后续响应拦截规则（Was annotated）匹配触发")
                .disabled(!can_decide || self.project.is_none())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(project) = this.project {
                        let _ = this.enqueue(
                            Command::SetLiveFlowAnnotated {
                                project,
                                flow_id,
                                annotated: !annotated,
                            },
                            cx,
                        );
                    }
                }))
                .into_any_element(),
            );
        }
        actions
    }

    #[inline(never)]
    fn render_intercept_toolbar(
        &self,
        pending: Option<&PendingIntercept>,
        can_decide: bool,
        can_decide_any: bool,
        can_action: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let intercept_on = self.intercept_requests || self.intercept_responses;
        let toggle_disabled = self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some();
        let browser_disabled = self.ext_import.visible || toggle_disabled;
        let has_pending = !self.intercepts.is_empty();
        let btn_h = theme::scaled_px(26.);

        div()
            .w_full()
            .min_w_0()
            .min_h(theme::scaled_px(50.))
            .flex_shrink_0()
            .px(theme::scaled_px(20.))
            .pt(theme::scaled_px(18.))
            .pb(theme::scaled_px(8.))
            .bg(theme::background())
            .when(has_pending, |d| {
                d.border_b_1().border_color(theme::border())
            })
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::scaled_px(10.))
            .child(self.render_intercept_toggle_button(
                intercept_on,
                toggle_disabled,
                dark,
                btn_h,
                cx,
            ))
            .child(self.render_intercept_decision_split(
                false,
                can_decide,
                can_decide_any,
                dark,
                btn_h,
                cx,
            ))
            .child(self.render_intercept_decision_split(
                true,
                can_decide,
                can_decide_any,
                dark,
                btn_h,
                cx,
            ))
            .when_some(pending, |bar, pending| {
                bar.children(self.render_intercept_active_actions(
                    pending, can_decide, can_action, btn_h, cx,
                ))
            })
            .child(div().flex_1())
            .child(self.render_intercept_right_controls(browser_disabled, dark, btn_h, cx))
            .into_any_element()
    }

    #[inline(never)]
    fn build_intercept_queue_row_context_menu(
        owner: gpui_kit::WeakEntity<Self>,
        target_id: mio_core::Uuid,
        menu: PopupMenu,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(entity) = owner.upgrade() else {
            return menu;
        };
        let (can_decide, is_http, has_host, has_ext) = {
            let view = entity.read(cx);
            let base_ok = view.project.is_some()
                && view.backend_available()
                && view.transition.is_none();
            match view.intercepts.iter().find(|p| p.id() == target_id) {
                Some(pending) if base_ok && !pending.deciding => match &pending.target {
                    InterceptTarget::Http { record, .. } => {
                        let url = if record.summary.url.is_empty() {
                            &record.request.url
                        } else {
                            &record.summary.url
                        };
                        (
                            true,
                            true,
                            super::wb_proxy::extract_url_host(url).is_some(),
                            super::wb_proxy::extract_url_file_extension(url).is_some(),
                        )
                    }
                    InterceptTarget::WebSocket { .. } => (true, false, false, false),
                },
                _ => (false, false, false, false),
            }
        };
        let o_fwd = owner.clone();
        let o_drop = owner.clone();
        let o_rep = owner.clone();
        let o_int = owner.clone();
        let o_org = owner.clone();
        let o_host = owner.clone();
        let o_ext = owner.clone();
        menu.compact()
            .item(
                PopupMenuItem::new("放行 (Forward)")
                    .disabled(!can_decide)
                    .on_click(move |_, window, cx| {
                        let _ = o_fwd.update(cx, |view, cx| {
                            view.decide_for(target_id, false, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("丢弃 (Drop)")
                    .disabled(!can_decide)
                    .on_click(move |_, window, cx| {
                        let _ = o_drop.update(cx, |view, cx| {
                            view.decide_for(target_id, true, window, cx);
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("发送到 Repeater")
                    .disabled(!can_decide)
                    .on_click(move |_, window, cx| {
                        let _ = o_rep.update(cx, |view, cx| {
                            if view.selected_intercept != Some(target_id) {
                                view.clear_context_viewer("intercept", cx);
                                view.selected_intercept = Some(target_id);
                            }
                            view.open_in_repeater(window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("发送到 Intruder")
                    .disabled(!can_decide || !is_http)
                    .on_click(move |_, window, cx| {
                        let _ = o_int.update(cx, |view, cx| {
                            if view.selected_intercept != Some(target_id) {
                                view.clear_context_viewer("intercept", cx);
                                view.selected_intercept = Some(target_id);
                            }
                            view.open_in_intruder(window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("发送到 Organizer")
                    .disabled(!can_decide || !is_http)
                    .on_click(move |_, window, cx| {
                        let _ = o_org.update(cx, |view, cx| {
                            if view.selected_intercept != Some(target_id) {
                                view.clear_context_viewer("intercept", cx);
                                view.selected_intercept = Some(target_id);
                            }
                            view.send_selected_to_organiser(window, cx);
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("不拦截此主机 (Do not intercept this host)")
                    .disabled(!can_decide || !is_http || !has_host)
                    .on_click(move |_, _, cx| {
                        let _ = o_host.update(cx, |view, cx| {
                            view.add_do_not_intercept_host_rule_for(target_id, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("不拦截此扩展名 (Do not intercept this extension)")
                    .disabled(!can_decide || !is_http || !has_ext)
                    .on_click(move |_, _, cx| {
                        let _ = o_ext.update(cx, |view, cx| {
                            view.add_do_not_intercept_extension_rule_for(target_id, cx);
                        });
                    }),
            )
    }

    #[inline(never)]
    fn render_intercept_queue_bar(
        &self,
        can_decide_any: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu_owner = cx.weak_entity();
        toolbar()
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::muted_foreground())
                    .child(format!("队列：{}", self.intercepts.len())),
            )
            .children(self.intercepts.iter().map(|pending| {
                let id = pending.id();
                let tip = pending.header_label();
                let row_owner = menu_owner.clone();
                div()
                    .id(SharedString::from(format!("intercept-queue-row-{id}")))
                    .test_support()
                    .child(
                        button(
                            SharedString::from(format!("intercept-{id}")),
                            pending.queue_label(),
                        )
                        .selected(Some(id) == self.selected_intercept)
                        .tooltip(tip)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.selected_intercept != Some(id) {
                                this.clear_context_viewer("intercept", cx);
                            }
                            this.selected_intercept = Some(id);
                            cx.notify();
                        })),
                    )
                    .context_menu(move |menu, _, cx| {
                        Workbench::build_intercept_queue_row_context_menu(
                            row_owner.clone(),
                            id,
                            menu,
                            cx,
                        )
                    })
            }))
            .child(div().flex_1())
            .child(
                button("intercept-forward-all", "全部转发")
                    .icon(IconName::ChevronsRight)
                    .tooltip("按当前草稿批量放行拦截队列中的所有等待报文")
                    .disabled(!can_decide_any)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.decide_all(false, window, cx)
                    })),
            )
            .child(
                button("intercept-drop-all", "全部丢弃")
                    .icon(IconName::Trash)
                    .tooltip("批量丢弃拦截队列中的所有等待报文并中止对应连接")
                    .disabled(!can_decide_any)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.decide_all(true, window, cx)
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intercept_pending_editor(
        &self,
        pending: &PendingIntercept,
        can_decide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        editing::set_url_encode_as_you_type(self.url_encode_as_you_type_enabled());
        let pending_id = pending.id();
        let header_owner = cx.weak_entity();
        let editor_readonly = pending.deciding || pending.editor.is_readonly_preview();
        let editor_base = editing::message_editor(&pending.editor)
            .readonly(editor_readonly)
            .size_full()
            .aria_label("Intercept 消息编辑器");
        let editor = if matches!(pending.target, InterceptTarget::WebSocket { .. }) {
            editor_base.context_menu(self.websocket_payload_context_menu(
                pending.editor.state(),
                editor_readonly,
            ))
        } else {
            editor_base.context_menu(self.request_context_menu(
                pending.editor.state(),
                editor_readonly,
            ))
        };
        let native = div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .id(SharedString::from(format!(
                        "intercept-pending-header-{pending_id}"
                    )))
                    .test_support()
                    .child(pending.header_label())
                    .context_menu(move |menu, _, cx| {
                        Workbench::build_intercept_queue_row_context_menu(
                            header_owner.clone(),
                            pending_id,
                            menu,
                            cx,
                        )
                    }),
            )
            .child(self.message_header(
                crate::workbench::wb_editors::EditorSlot::Intercept,
                "消息",
                pending.editor.view(),
                cx,
            ))
            .child(div().flex_1().min_h_0().child(editor))
            .child(pending.editor.render_search_bar_editable(
                "intercept",
                !can_decide || pending.editor.is_readonly_preview(),
                cx,
                |this, action, window, cx| {
                    this.handle_message_search_action(
                        crate::workbench::wb_editors::EditorSlot::Intercept,
                        action,
                        window,
                        cx,
                    );
                },
            ))
            .into_any_element();
        self.render_intercept_editor(pending, native, cx)
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_intercept(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|pending| pending.id() == id));
        let can_decide = self.backend_available()
            && self.transition.is_none()
            && pending.is_some_and(|pending| !pending.deciding);
        let can_decide_any = self.backend_available()
            && self.transition.is_none()
            && self.intercepts.iter().any(|pending| !pending.deciding);
        let is_ws_intercept = pending
            .is_some_and(|pending| matches!(pending.target, InterceptTarget::WebSocket { .. }));
        let is_http_intercept = pending.is_some_and(PendingIntercept::is_http);
        let can_action =
            can_decide && (is_http_intercept || is_ws_intercept) && self.project.is_some();

        let body = match pending {
            Some(pending) => {
                let editor_view = self.render_intercept_pending_editor(pending, can_decide, cx);
                self.render_inspector_dock(editor_view, cx)
            }
            None => self.render_intercept_empty_state(cx),
        };

        div()
            .size_full()
            .bg(theme::background())
            .flex()
            .flex_col()
            .child(self.render_intercept_toolbar(
                pending,
                can_decide,
                can_decide_any,
                can_action,
                cx,
            ))
            .when(self.intercepts.len() > 1, |this| {
                this.child(self.render_intercept_queue_bar(can_decide_any, cx))
            })
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
