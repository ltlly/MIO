use std::sync::{Arc, OnceLock};

use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    popover::Popover,
};
use gpui_kit::{BoxShadow, FontWeight, Image, ImageFormat, hsla, img, point};

use super::*;

const INTRUDER_ROCKET_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120" viewBox="0 0 120 120">
  <g transform="translate(60 60) rotate(135)">
    <line x1="-29" y1="-26" x2="-29" y2="-4" stroke="#dbdddf" stroke-width="4.2" stroke-linecap="round"/>
    <line x1="-29" y1="8" x2="-29" y2="21" stroke="#dbdddf" stroke-width="4.2" stroke-linecap="round"/>
    <line x1="29" y1="-16" x2="29" y2="4" stroke="#dbdddf" stroke-width="4.2" stroke-linecap="round"/>
    <line x1="29" y1="16" x2="29" y2="34" stroke="#dbdddf" stroke-width="4.2" stroke-linecap="round"/>
    <path d="M -8 21 C -11 30, -8 40, 0 46 C 8 40, 11 30, 8 21 Z" fill="#f3c9a2"/>
    <path d="M -6.2 21 C -8.2 29, -5.8 37, 0 42.5 C 5.8 37, 8.2 29, 6.2 21 Z" fill="#eb7438"/>
    <path d="M -3.2 21.5 C -4.2 27, -2.8 32, 0 35.5 C 2.8 32, 4.2 27, 3.2 21.5 Z" fill="#f8bc73"/>
    <rect x="-8" y="18" width="16" height="4.5" rx="2" fill="#27496f"/>
    <path d="M -12 4 L -21 15 C -22 16.5, -21.5 19, -19.5 19.5 L -11 19.5 Z" fill="#88add6"/>
    <path d="M 12 4 L 21 15 C 22 16.5, 21.5 19, 19.5 19.5 L 11 19.5 Z" fill="#88add6"/>
    <path d="M -12.5 -16 C -14.5 -4, -14 10, -10.5 19 L 10.5 19 C 14 10, 14.5 -4, 12.5 -16 Z" fill="#325c8a"/>
    <path d="M -12.5 -16 C -11 -27, -5.5 -36.5, 0 -41.5 C 5.5 -36.5, 11 -27, 12.5 -16 Z" fill="#88add6"/>
    <path d="M -6.5 1 L -2 -4 L 2 4 L 6.5 -1" fill="none" stroke="#ffffff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>
  </g>
</svg>"##;

const CURSOR_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 14 14"><path d="M2 1.5 L2 12.2 L5.1 9.3 L7.6 13.3 L9.3 12.4 L6.8 8.4 L10.8 8.4 Z" fill="#000000"/></svg>"##;

const CURVED_ARROW_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="152" height="100" viewBox="0 0 152 100"><path d="M 2 6 C 84 6, 140 22, 140 52 C 140 76, 124 90, 108 90" fill="none" stroke="#000000" stroke-width="1.0" stroke-dasharray="1.1 2.4" stroke-linecap="round"/><path d="M 109.5 87.5 L 103.5 90 L 109.5 92.5 Z" fill="#000000"/></svg>"##;

const DISMISS_CLOSE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 14 14"><path d="M 1.75 1.75 L 12.25 12.25 M 12.25 1.75 L 1.75 12.25" fill="none" stroke="#000000" stroke-width="1.7" stroke-linecap="round"/></svg>"##;

pub(in crate::workbench::intruder) fn intruder_rocket_image() -> Arc<Image> {
    static ROCKET: OnceLock<Arc<Image>> = OnceLock::new();
    ROCKET
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                INTRUDER_ROCKET_SVG.as_bytes().to_vec(),
            ))
        })
        .clone()
}

fn skeleton_line(width_pct: f32, height: f32, color: gpui_kit::Hsla) -> gpui_kit::Div {
    div()
        .w(gpui_kit::relative(width_pct))
        .h(px(height))
        .rounded(px(height / 2.))
        .bg(color)
}

impl Workbench {
    pub(super) fn render_intruder_onboarding(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let left_bg: gpui_kit::Hsla = rgb(if dark { 0x202731 } else { 0xebf1f7 }).into();
        let right_bg: gpui_kit::Hsla = if dark {
            theme::surface()
        } else {
            rgb(0xffffff).into()
        };
        let heading_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x141414).into()
        };
        let body_fg: gpui_kit::Hsla = if dark {
            theme::muted_foreground()
        } else {
            rgb(0x2b2b2b).into()
        };
        let card_bg: gpui_kit::Hsla = if dark {
            rgb(0x2a2f37).into()
        } else {
            rgb(0xffffff).into()
        };
        let response_card_bg: gpui_kit::Hsla = if dark {
            rgb(0x262c34).into()
        } else {
            rgb(0xf4f8fb).into()
        };
        let skel_top: gpui_kit::Hsla = rgb(if dark { 0x3a414c } else { 0xe3e5e7 }).into();
        let skel_menu: gpui_kit::Hsla = rgb(if dark { 0x454c57 } else { 0xd4d7da }).into();
        let menu_bg: gpui_kit::Hsla = if dark {
            rgb(0x2e333b).into()
        } else {
            rgb(0xffffff).into()
        };
        let menu_border: gpui_kit::Hsla = rgb(0x3d444e).into();
        let menu_shadows = vec![
            BoxShadow {
                color: hsla(0., 0., 0., if dark { 0.34 } else { 0.09 }),
                offset: point(px(0.), px(4.)),
                blur_radius: px(14.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: hsla(0., 0., 0., if dark { 0.18 } else { 0.04 }),
                offset: point(px(0.), px(1.)),
                blur_radius: px(3.),
                spread_radius: px(0.),
                inset: false,
            },
        ];
        let arrow_fg: gpui_kit::Hsla = rgb(if dark { 0x5c6570 } else { 0xc6cdd6 }).into();
        let learn_more_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xececec }).into();
        let learn_more_border: gpui_kit::Hsla = rgb(if dark { 0x4b5058 } else { 0xe2e2e2 }).into();
        let new_disabled =
            self.project.is_none() || self.transition.is_some() || self.backend_disconnected;

        let schematic_top = div()
            .w_full()
            .bg(card_bg)
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(skeleton_line(1.0, 9., skel_top))
            .child(skeleton_line(1.0, 9., skel_top))
            .child(skeleton_line(1.0, 9., skel_top))
            .child(skeleton_line(1.0, 9., skel_top));

        let context_menu_card = div()
            .absolute()
            .top(px(74.))
            .left(px(58.))
            .w(px(154.))
            .bg(menu_bg)
            .when(dark, |card| card.border_1().border_color(menu_border))
            .shadow(menu_shadows)
            .rounded(px(2.))
            .py(px(9.))
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(div().px(px(12.)).child(skeleton_line(0.84, 6., skel_menu)))
            .child(div().px(px(12.)).child(skeleton_line(0.76, 6., skel_menu)))
            .child(div().px(px(12.)).child(skeleton_line(0.54, 6., skel_menu)))
            .child(
                div()
                    .w_full()
                    .px(px(12.))
                    .py(px(6.))
                    .bg(rgb(0x28679f))
                    .text_color(rgb(0xffffff))
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("发送到 Intruder"),
            )
            .child(div().px(px(12.)).child(skeleton_line(0.58, 6., skel_menu)))
            .child(div().px(px(12.)).child(skeleton_line(0.74, 6., skel_menu)))
            .child(div().px(px(12.)).child(skeleton_line(0.46, 6., skel_menu)));

        let schematic_bottom = div()
            .relative()
            .w_full()
            .h(px(214.))
            .flex()
            .gap(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(184.))
                    .bg(card_bg)
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.5))
                            .text_color(theme::muted_foreground())
                            .child("Request"),
                    )
                    .child(
                        div()
                            .mt(px(15.))
                            .ml(px(22.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                gpui_kit::svg()
                                    .data(CURSOR_SVG)
                                    .w(px(13.))
                                    .h(px(13.))
                                    .flex_shrink_0()
                                    .text_color(heading_fg),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(heading_fg)
                                    .child("右键点击"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(184.))
                    .bg(response_card_bg)
                    .p(px(12.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.5))
                            .text_color(theme::muted_foreground())
                            .child("Response"),
                    ),
            )
            .child(context_menu_card)
            .child(
                div().absolute().top(px(46.)).left(px(114.)).child(
                    gpui_kit::svg()
                        .data(CURVED_ARROW_SVG)
                        .w(px(152.))
                        .h(px(100.))
                        .text_color(arrow_fg),
                ),
            );

        let left_half = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(left_bg)
            .px(px(56.))
            .py(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(480.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(
                        div()
                            .text_size(theme::scaled_px(20.))
                            .line_height(theme::scaled_px(28.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(heading_fg)
                            .mb(px(26.))
                            .child("开始使用：从任意位置发送请求到 Intruder。"),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(schematic_top)
                            .child(schematic_bottom),
                    ),
            );

        let learn_more_popover = Popover::new("intruder-learn-more-popover")
            .anchor(gpui_kit::Anchor::BottomCenter)
            .trigger(
                Button::new("intruder-onboarding-learn-more")
                    .small()
                    .w(theme::scaled_px(114.))
                    .h(theme::scaled_px(28.))
                    .px(px(12.))
                    .rounded(ButtonRounded::Size(px(4.)))
                    .bg(learn_more_bg)
                    .border_1()
                    .border_color(learn_more_border)
                    .text_color(heading_fg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .label("了解更多")
                    .tooltip("查看 Intruder 攻击模式、§ 标记位与载荷集说明"),
            )
            .content(move |_, _, _| {
                div()
                    .id("intruder-learn-more-content")
                    .test_support()
                    .w(px(340.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .text_size(theme::scaled_px(12.5))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(theme::scaled_px(13.5))
                            .child("Intruder 自动化攻击指南"),
                    )
                    .child("1. 从 Proxy、Target 或 Repeater 右键选择「发送到 Intruder」，或点击下方「不再显示此提示」进入工作区新建标签。")
                    .child("2. 在请求编辑器中选中目标参数并使用 Add § / Auto § 标记载荷位置。")
                    .child("3. 选择攻击模式（Sniper / Battering ram / Pitchfork / Cluster bomb）并在右侧 Payloads 面板配置字典或生成器。")
                    .child("4. 通过 Resource pool 控制并发速率，启动攻击并在结果窗口按状态码、长度与 Grep 匹配分析响应。")
            });

        let dont_show_link = flat_button("intruder-new", false, window, cx)
            .accessibility_label("不再显示此提示")
            .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
            .mt(px(18.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_size(theme::scaled_px(13.))
            .font_weight(FontWeight::NORMAL)
            .text_color(theme::accent())
            .hover(|style| style.bg(theme::accent().opacity(0.08)))
            .disabled(new_disabled)
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("隐藏此引导页并进入 Intruder 攻击工作区")
                    .build(window, cx)
            })
            .child("不再显示此提示")
            .on_click(cx.listener(|this, _, window, cx| {
                this.set_intruder_onboarding_dismissed(true, cx);
                this.new_intruder_tab(window, cx);
            }));

        let right_half = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .bg(right_bg)
            .px(px(32.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                Button::new("intruder-onboarding-dismiss")
                    .ghost()
                    .small()
                    .compact()
                    .w(px(28.))
                    .h(px(28.))
                    .p_0()
                    .rounded(ButtonRounded::Size(px(4.)))
                    .absolute()
                    .top(px(12.))
                    .right(px(14.))
                    .accessibility_label("关闭引导页")
                    .tooltip("关闭引导页并显示工作标签栏")
                    .child(
                        gpui_kit::svg()
                            .data(DISMISS_CLOSE_SVG)
                            .w(theme::scaled_px(14.))
                            .h(theme::scaled_px(14.))
                            .flex_shrink_0()
                            .text_color(heading_fg),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_intruder_onboarding_dismissed(true, cx);
                    })),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(340.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        img(intruder_rocket_image())
                            .w(theme::scaled_px(108.))
                            .h(theme::scaled_px(108.))
                            .flex_shrink_0()
                            .mb(px(16.)),
                    )
                    .child(
                        div()
                            .text_size(theme::scaled_px(16.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(heading_fg)
                            .mb(px(14.))
                            .child("Intruder"),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(theme::scaled_px(13.))
                            .line_height(theme::scaled_px(20.))
                            .text_color(body_fg)
                            .mb(px(22.))
                            .child(
                                "配置自动化攻击，反复发送同一请求并使用预设载荷替换指定位置的值。",
                            ),
                    )
                    .child(learn_more_popover)
                    .child(dont_show_link),
            );

        div()
            .id("intruder-onboarding")
            .test_support()
            .size_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .child(left_half)
            .child(right_half)
            .into_any_element()
    }

    pub(super) fn render_intruder_dismissed_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("intruder-empty-state")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .child(
                div().text_color(theme::muted_foreground()).child(
                    "使用工作标签栏的 + 新建 Intruder 请求，或从消息右键菜单发送到 Intruder。",
                ),
            )
            .child(
                Button::new("intruder-show-onboarding")
                    .ghost()
                    .small()
                    .label("查看 Intruder 引导页")
                    .tooltip("重新显示 Intruder 入门引导界面")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_intruder_onboarding_dismissed(false, cx);
                    })),
            )
            .into_any_element()
    }
}
