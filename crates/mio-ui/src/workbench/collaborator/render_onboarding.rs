use super::*;
use gpui_kit::component::{
    Icon,
    button::{ButtonRounded, ButtonVariants},
    popover::Popover,
};

const COLLABORATOR_RADAR_LIGHT_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="126" height="83" viewBox="0 0 252 166">
  <defs>
    <linearGradient id="sweep" gradientUnits="userSpaceOnUse" x1="8" y1="0" x2="114" y2="0">
      <stop offset="0%" stop-color="#786cf5" stop-opacity="0.92"/>
      <stop offset="48%" stop-color="#9b9de9" stop-opacity="0.72"/>
      <stop offset="100%" stop-color="#bfd0e0" stop-opacity="0.08"/>
    </linearGradient>
  </defs>
  <path d="M 4 72 L 4 91 A 122.5 70.5 0 0 0 249 91 L 249 72 A 122.5 70.5 0 0 1 4 72 Z" fill="#cacacd"/>
  <g transform="translate(126.5 72) scale(1 0.576)">
    <circle cx="0" cy="0" r="118" fill="#bfd0e0"/>
    <circle cx="0" cy="0" r="85" fill="none" stroke="#759abc" stroke-width="6.5"/>
    <circle cx="0" cy="0" r="50.5" fill="none" stroke="#517faa" stroke-width="6.5"/>
    <path d="M 0 0 L 101.6 -51.8 A 114 114 0 0 1 101.6 51.8 Z" fill="url(#sweep)"/>
    <line x1="-83.4" y1="-83.4" x2="83.4" y2="83.4" stroke="#2f6499" stroke-width="3.6"/>
    <line x1="-83.4" y1="83.4" x2="83.4" y2="-83.4" stroke="#2f6499" stroke-width="3.6"/>
    <circle cx="0" cy="0" r="118" fill="none" stroke="#2f6499" stroke-width="10"/>
    <circle cx="0" cy="0" r="10" fill="#2f6499"/>
    <circle cx="-112.5" cy="41.7" r="14" fill="#f67f50"/>
    <circle cx="-7.0" cy="-85.1" r="14" fill="#f67f50"/>
    <circle cx="17.5" cy="45.1" r="14" fill="#f67f50"/>
  </g>
</svg>"##;

const COLLABORATOR_RADAR_DARK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="126" height="83" viewBox="0 0 252 166">
  <defs>
    <linearGradient id="sweep" gradientUnits="userSpaceOnUse" x1="8" y1="0" x2="114" y2="0">
      <stop offset="0%" stop-color="#8b80ff" stop-opacity="0.90"/>
      <stop offset="48%" stop-color="#7c82d8" stop-opacity="0.62"/>
      <stop offset="100%" stop-color="#2c3e52" stop-opacity="0.08"/>
    </linearGradient>
  </defs>
  <path d="M 4 72 L 4 91 A 122.5 70.5 0 0 0 249 91 L 249 72 A 122.5 70.5 0 0 1 4 72 Z" fill="#32363d"/>
  <g transform="translate(126.5 72) scale(1 0.576)">
    <circle cx="0" cy="0" r="118" fill="#2c3e52"/>
    <circle cx="0" cy="0" r="85" fill="none" stroke="#5c84ad" stroke-width="6.5"/>
    <circle cx="0" cy="0" r="50.5" fill="none" stroke="#4d79a6" stroke-width="6.5"/>
    <path d="M 0 0 L 101.6 -51.8 A 114 114 0 0 1 101.6 51.8 Z" fill="url(#sweep)"/>
    <line x1="-83.4" y1="-83.4" x2="83.4" y2="83.4" stroke="#6c9ed0" stroke-width="3.6"/>
    <line x1="-83.4" y1="83.4" x2="83.4" y2="-83.4" stroke="#6c9ed0" stroke-width="3.6"/>
    <circle cx="0" cy="0" r="118" fill="none" stroke="#5386ba" stroke-width="10"/>
    <circle cx="0" cy="0" r="10" fill="#6c9ed0"/>
    <circle cx="-112.5" cy="41.7" r="14" fill="#f67f50"/>
    <circle cx="-7.0" cy="-85.1" r="14" fill="#f67f50"/>
    <circle cx="17.5" cy="45.1" r="14" fill="#f67f50"/>
  </g>
</svg>"##;

impl CollaboratorView {
    fn radar_image(&mut self, dark: bool, cx: &App) -> Option<Arc<RenderImage>> {
        let slot = if dark {
            &mut self.radar_dark
        } else {
            &mut self.radar_light
        };
        if slot.is_none() {
            let svg = if dark {
                COLLABORATOR_RADAR_DARK_SVG
            } else {
                COLLABORATOR_RADAR_LIGHT_SVG
            };
            *slot = cx
                .svg_renderer()
                .render_single_frame(svg.as_bytes(), 1.0)
                .ok();
        }
        slot.clone()
    }

    pub(super) fn render_onboarding(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let radar = self.radar_image(dark, cx);
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
                let _ = this
                    .workbench
                    .update(cx, |wb, cx| wb.dismiss_collaborator_onboarding(cx));
                cx.notify();
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
                let _ = this
                    .workbench
                    .update(cx, |wb, cx| wb.dismiss_collaborator_onboarding(cx));
                cx.notify();
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
                        let _ = this
                            .workbench
                            .update(cx, |wb, cx| wb.dismiss_collaborator_onboarding(cx));
                        cx.notify();
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
}
