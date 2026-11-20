use gpui_kit::component::popover::Popover;

use super::*;

const EMPTY_BOOK_SHADOW_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 71"><path d="M5 41.5L43.5 69L95 36.5L56.5 14Z" fill="currentColor"/></svg>"#;
const EMPTY_BOOK_COVER_EDGE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 71"><path d="M1.5 32.5L1.5 36L43.5 60.5L98.5 29L98.5 25.5L43.5 57Z" fill="currentColor"/></svg>"#;
const EMPTY_BOOK_LEFT_PAGE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 71"><path d="M1.5 32.5L28.75 16.75L71 41.25L43.5 57Z" fill="currentColor"/></svg>"#;
const EMPTY_BOOK_RIGHT_PAGE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 71"><path d="M28.75 16.75L56 1L98.5 25.5L71 41.25Z" fill="currentColor"/></svg>"#;
const EMPTY_BOOK_BARS_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 71" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M19.6 34.2L33.3 26.3M25.0 39.9L42.7 29.7M33.7 43.7L46.7 36.1M42.4 46.9L48.4 43.4"/></svg>"#;
const EMPTY_BOOK_CLOCK_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 71" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="63.6" cy="21.1" rx="11.0" ry="6.4"/><path d="M58.4 18.2L63.6 21.1L70.2 21.1"/></svg>"#;

fn render_empty_book_illustration() -> AnyElement {
    let dark = theme::is_dark();
    let shadow_color: gpui_kit::Hsla = if dark {
        rgb(0x141619).into()
    } else {
        rgb(0xe2e2e2).into()
    };
    let edge_color: gpui_kit::Hsla = if dark {
        rgb(0x25334a).into()
    } else {
        rgb(0x42526c).into()
    };
    let left_page_color: gpui_kit::Hsla = if dark {
        rgb(0x326aa3).into()
    } else {
        rgb(0x2a619a).into()
    };
    let right_page_color: gpui_kit::Hsla = if dark {
        rgb(0x7c9cbe).into()
    } else {
        rgb(0x93b3ce).into()
    };
    let bars_color: gpui_kit::Hsla = rgb(0xffffff).into();
    let clock_color: gpui_kit::Hsla = if dark {
        rgb(0x1f3f63).into()
    } else {
        rgb(0x2a619a).into()
    };

    div()
        .relative()
        .w(theme::scaled_px(100.))
        .h(theme::scaled_px(71.))
        .flex_shrink_0()
        .child(
            gpui_kit::svg()
                .data(EMPTY_BOOK_SHADOW_SVG)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(shadow_color),
        )
        .child(
            gpui_kit::svg()
                .data(EMPTY_BOOK_COVER_EDGE_SVG)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(edge_color),
        )
        .child(
            gpui_kit::svg()
                .data(EMPTY_BOOK_LEFT_PAGE_SVG)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(left_page_color),
        )
        .child(
            gpui_kit::svg()
                .data(EMPTY_BOOK_RIGHT_PAGE_SVG)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(right_page_color),
        )
        .child(
            gpui_kit::svg()
                .data(EMPTY_BOOK_BARS_SVG)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(bars_color),
        )
        .child(
            gpui_kit::svg()
                .data(EMPTY_BOOK_CLOCK_SVG)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(clock_color),
        )
        .into_any_element()
}

impl Workbench {
    pub(in crate::workbench::websocket) fn render_websocket_empty_state(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let heading_fg: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };
        let desc_fg: gpui_kit::Hsla = theme::foreground();
        let learn_bg: gpui_kit::Hsla = if dark {
            rgb(0x35383d).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let learn_hover: gpui_kit::Hsla = if dark {
            rgb(0x41464d).into()
        } else {
            rgb(0xe2e2e2).into()
        };
        let learn_border: gpui_kit::Hsla = if dark {
            theme::control_border()
        } else {
            rgb(0xd4d4d4).into()
        };
        let browser_disabled = self.ext_import.visible
            || self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some();

        let learn_more = Popover::new("websocket-empty-learn-more-popover")
            .anchor(gpui_kit::Anchor::TopCenter)
            .trigger(
                Button::new("websocket-empty-learn-more")
                    .small()
                    .w(theme::scaled_px(113.))
                    .h(theme::scaled_px(25.))
                    .rounded(ButtonRounded::Size(px(3.)))
                    .bg(learn_bg)
                    .hover(move |s| s.bg(learn_hover))
                    .border_1()
                    .border_color(learn_border)
                    .text_color(theme::foreground())
                    .accessibility_label("了解更多")
                    .tooltip("查看 WebSockets 历史记录使用说明")
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("了解更多"),
                    ),
            )
            .content(|_, _, _| {
                div()
                    .id("websocket-empty-learn-more-content")
                    .test_support()
                    .w(px(340.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .text_size(theme::scaled_px(12.5))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(theme::scaled_px(13.5))
                            .child("WebSockets 历史记录"),
                    )
                    .child("1. 点击「打开浏览器」或将客户端代理指向当前 Proxy 监听端口，访问使用 WebSockets 的目标应用。")
                    .child("2. 所有经代理转发的客户端与服务端 WebSocket 文本/二进制数据帧将按序记录在此列表。")
                    .child("3. 捕获到消息后，可使用高级筛选、快速搜索、Pretty/Raw/Hex 视图、Inspector 检视与 Notes 批注，或右键发送到 WebSocket Repeater 重放。")
            });

        let open_browser = Button::new("websocket-empty-open-browser")
            .primary()
            .small()
            .w(theme::scaled_px(126.))
            .h(theme::scaled_px(25.))
            .rounded(ButtonRounded::Size(px(3.)))
            .when(!browser_disabled, |b| {
                b.bg(rgb(0xdb612f))
                    .hover(|s| s.bg(rgb(0xc45122)))
                    .text_color(rgb(0xffffff))
            })
            .disabled(browser_disabled)
            .accessibility_label("打开浏览器")
            .tooltip("启动已配置 Proxy 监听地址与 CA 信任的浏览器")
            .child(
                div()
                    .text_size(theme::scaled_px(13.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("打开浏览器"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_configured_browser(cx);
            }));

        div()
            .id("websocket-empty-state")
            .test_support()
            .size_full()
            .bg(theme::background())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(render_empty_book_illustration())
                    .child(
                        div()
                            .id("websocket-empty-heading")
                            .test_support()
                            .mt(theme::scaled_px(24.))
                            .text_size(theme::scaled_px(18.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(heading_fg)
                            .child(
                                "WebSockets 历史记录为空"
                                    .test_text("WebSockets 历史记录为空"),
                            ),
                    )
                    .child(
                        div()
                            .mt(theme::scaled_px(16.))
                            .max_w(theme::scaled_px(440.))
                            .text_center()
                            .text_size(theme::scaled_px(13.))
                            .line_height(theme::scaled_px(19.))
                            .text_color(desc_fg)
                            .child("此处显示配置浏览器与目标应用之间传输的全部 WebSocket 流量历史记录。WebSockets 提供持久连接，用于流式数据及其他异步通信流量。"),
                    )
                    .child(
                        div()
                            .mt(theme::scaled_px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(theme::scaled_px(16.))
                            .child(learn_more)
                            .child(open_browser),
                    ),
            )
            .into_any_element()
    }
}
