use gpui_kit::accesskit::Role;
use gpui_kit::base::{Checkbox, CheckboxIndicator, Radio};
use gpui_kit::component::{
    Icon, IconName,
    button::ButtonVariants,
    scroll::{Scrollbar, ScrollbarMode},
};

use super::*;

const TABLE_RIGHT_TRIANGLE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 7 10"><polygon points="0,0 7,5 0,10" fill="currentColor"/></svg>"#;

pub(super) fn settings_blue() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x3b82c4 } else { 0x26649d }).into()
}

pub(super) fn settings_checkbox_border(checked: bool) -> gpui_kit::Hsla {
    if checked {
        settings_blue()
    } else {
        rgb(if theme::is_dark() { 0x5c626b } else { 0xb5b5b5 }).into()
    }
}

fn settings_nav_selected() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x2e436e } else { 0xd0dfef }).into()
}

fn settings_table_border() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x45484f } else { 0xc8c8c8 }).into()
}

pub(super) fn settings_table_th(label: &'static str, _width: f32) -> gpui_kit::Div {
    div()
        .size_full()
        .px(px(6.))
        .flex()
        .items_center()
        .text_size(theme::scaled_px(12.5))
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .text_color(theme::foreground())
        .whitespace_nowrap()
        .truncate()
        .child(label)
}

pub(super) fn settings_table_checkbox_cell(checkbox: impl IntoElement) -> gpui_kit::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(checkbox)
}

pub(super) fn settings_table_frame(table: impl IntoElement, show_triangle: bool) -> gpui_kit::Div {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_1()
                .border_color(settings_table_border())
                .bg(theme::surface())
                .overflow_hidden()
                .child(table),
        )
        .when(show_triangle, |this| {
            this.child(
                div()
                    .ml(px(2.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(
                        Icon::default()
                            .data(TABLE_RIGHT_TRIANGLE_SVG)
                            .w(px(7.))
                            .h(px(10.))
                            .text_color(theme::foreground()),
                    ),
            )
        })
}

pub(super) fn settings_scrollable_pane(
    id: impl Into<gpui_kit::ElementId>,
    handle: &gpui_kit::ScrollHandle,
    content: impl IntoElement,
) -> gpui_kit::Div {
    let id = id.into();
    let dark = theme::is_dark();
    let track_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf5f5f5 }).into();
    let thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5c626b } else { 0xcbcbcb }).into();
    let thumb_hover: gpui_kit::Hsla = rgb(if dark { 0x707782 } else { 0xb0b0b0 }).into();
    let thumb_active: gpui_kit::Hsla = rgb(if dark { 0x828a96 } else { 0x999999 }).into();
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .flex()
        .child(
            div()
                .id(id.clone())
                .test_support()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_y_scroll()
                .track_scroll(handle)
                .child(content),
        )
        .child(
            div()
                .relative()
                .w(px(10.))
                .h_full()
                .flex_shrink_0()
                .bg(track_bg)
                .child(
                    Scrollbar::vertical(handle)
                        .id((id, "vscroll"))
                        .viewport_from_layout()
                        .mode(ScrollbarMode::Always)
                        .styles(move |styles| {
                            styles
                                .track(|s| s.width(px(10.)).bg(track_bg))
                                .track_hover(|s| s.width(px(10.)).bg(track_bg))
                                .track_active(|s| s.width(px(10.)).bg(track_bg))
                                .thumb(|s| {
                                    s.width(px(6.))
                                        .inset(px(2.))
                                        .radius(px(3.))
                                        .min_length(px(28.))
                                        .bg(thumb_bg)
                                })
                                .thumb_hover(|s| {
                                    s.width(px(6.))
                                        .inset(px(2.))
                                        .radius(px(3.))
                                        .min_length(px(28.))
                                        .bg(thumb_hover)
                                })
                                .thumb_active(|s| {
                                    s.width(px(6.))
                                        .inset(px(2.))
                                        .radius(px(3.))
                                        .min_length(px(28.))
                                        .bg(thumb_active)
                                })
                        }),
                ),
        )
}

fn settings_icon_color() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0xdfe1e5 } else { 0x1e1e1e }).into()
}

fn scope_badge_pill(label: &'static str) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let bg = rgb(if dark { 0x273b54 } else { 0xc8d7ec });
    let fg = rgb(if dark { 0xd4e6fa } else { 0x1f2d3d });
    div()
        .flex_shrink_0()
        .h(theme::scaled_px(19.).max(px(19.)))
        .flex()
        .items_center()
        .px(px(8.))
        .rounded_full()
        .bg(bg)
        .text_size(theme::scaled_px(11.5))
        .text_color(fg)
        .whitespace_nowrap()
        .child(label)
}

pub(super) fn project_scope_badge() -> impl IntoElement {
    scope_badge_pill("项目设置")
}

pub(super) fn project_scope_badge_with_id(id: &'static str) -> impl IntoElement {
    scope_badge_pill("项目设置")
        .id(id)
        .test_support()
        .aria_label("项目设置")
}

pub(super) fn settings_section_heading(
    id: impl Into<gpui_kit::ElementId>,
    title: &'static str,
    badge: impl IntoElement,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .min_w_0()
                .child(
                    div()
                        .w(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(gpui_kit::assets::IconName::CircleQuestionMark)
                                .size(px(21.))
                                .text_color(settings_icon_color()),
                        ),
                )
                .child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Heading)
                        .aria_label(title)
                        .text_size(theme::scaled_px(15.))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child(title),
                ),
        )
        .child(badge)
}

pub(super) fn settings_section_heading_plain(
    title: &'static str,
    badge: impl IntoElement,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .min_w_0()
                .child(
                    div()
                        .w(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(gpui_kit::assets::IconName::CircleQuestionMark)
                                .size(px(21.))
                                .text_color(settings_icon_color()),
                        ),
                )
                .child(
                    div()
                        .text_size(theme::scaled_px(15.))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .whitespace_nowrap()
                        .child(title),
                ),
        )
        .child(badge)
}

pub(super) fn settings_desc_row(desc: &'static str) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .child(
            div()
                .w(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(gpui_kit::assets::IconName::Settings)
                        .size(px(21.))
                        .text_color(settings_icon_color()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn settings_desc_row_plain(desc: &'static str) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .child(div().w(px(22.)).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn settings_desc_row_with_action(
    action: impl IntoElement,
    desc: &'static str,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .child(
            div()
                .w(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(action),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn settings_gear_icon_button(
    id: impl Into<gpui_kit::ElementId>,
    tooltip: &'static str,
    disabled: bool,
) -> Button {
    Button::new(id)
        .accessibility_label("恢复默认设置")
        .tooltip(tooltip)
        .size(px(22.))
        .p_0()
        .ghost()
        .disabled(disabled)
        .child(
            Icon::new(gpui_kit::assets::IconName::Settings)
                .size(px(21.))
                .text_color(settings_icon_color()),
        )
}

pub(super) fn settings_action_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let bg = rgb(if dark { 0x3c3f44 } else { 0xeaeaea });
    let border = rgb(if dark { 0x4c5057 } else { 0xd4d4d4 });
    let hover_bg = rgb(if dark { 0x474b52 } else { 0xdedede });
    let fg = rgb(if dark { 0xdfe1e5 } else { 0x000000 });
    button(id, label)
        .disabled(disabled)
        .h(theme::scaled_px(24.).max(px(24.)))
        .rounded(px(4.))
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .when(!disabled, |this| this.hover(move |style| style.bg(hover_bg)))
}

pub(super) fn settings_secondary_action_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let bg = rgb(if dark { 0x2f3237 } else { 0xfafafa });
    let border = rgb(if dark { 0x484c54 } else { 0xd8d8d8 });
    let hover_bg = rgb(if dark { 0x3a3e45 } else { 0xf0f0f0 });
    let fg = rgb(if dark { 0xc8ccd4 } else { 0x333333 });
    button(id, label)
        .disabled(disabled)
        .h(theme::scaled_px(24.).max(px(24.)))
        .rounded(px(4.))
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .when(!disabled, |this| this.hover(move |style| style.bg(hover_bg)))
}

pub(super) fn settings_pill_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Button {
    let dark = theme::is_dark();
    let active_bg = settings_blue();
    let inactive_bg: gpui_kit::Hsla = rgb(if dark { 0x3a3d42 } else { 0xefefef }).into();
    let inactive_hover: gpui_kit::Hsla = rgb(if dark { 0x464a50 } else { 0xe3e3e3 }).into();
    let inactive_border: gpui_kit::Hsla = rgb(if dark { 0x52565e } else { 0xcccccc }).into();
    let inactive_fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into();
    Button::new(id)
        .label(label)
        .small()
        .min_w(theme::scaled_px(64.).max(px(62.)))
        .h(theme::scaled_px(25.).max(px(24.)))
        .px(px(16.))
        .rounded(px(12.))
        .rounded_full()
        .text_size(theme::scaled_px(12.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .selected(active)
        .when(active, |this| {
            this.bg(active_bg)
                .border_color(active_bg)
                .text_color(rgb(0xffffff))
        })
        .when(!active, |this| {
            this.bg(inactive_bg)
                .border_color(inactive_border)
                .text_color(inactive_fg)
                .hover(move |style| style.bg(inactive_hover))
        })
}

pub(super) fn settings_entity(window: &Window, cx: &App) -> Option<Entity<Settings>> {
    window
        .root::<gpui_kit::base::Root>()??
        .read(cx)
        .view()
        .clone()
        .downcast::<Settings>()
        .ok()
}

pub(super) fn settings_navigation(
    id: &'static str,
    label: &'static str,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let focus = window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let background = if selected {
        settings_nav_selected()
    } else {
        theme::surface()
    };
    let hover_bg = rgb(if theme::is_dark() { 0x34373c } else { 0xf2f4f7 });
    gpui_kit::base::Button::new(id)
        .w_full()
        .min_h(theme::scaled_px(27.).max(px(27.)))
        .py(px(3.))
        .pl(px(36.))
        .pr(px(10.))
        .rounded(px(4.))
        .justify_start()
        .text_size(theme::ui_font_size())
        .text_color(theme::foreground())
        .bg(background)
        .border_1()
        .border_color(background)
        .when(!selected, |this| {
            this.hover(move |style| style.bg(hover_bg).border_color(hover_bg))
        })
        .when(
            focus.is_focused(window) && window.last_input_was_keyboard(),
            |this| this.border_color(settings_blue()),
        )
        .track_focus(&focus)
        .role(Role::TreeItem)
        .accessibility_label(label)
        .selected(selected)
        .aria_selected(selected)
        .child(label)
}

pub(super) fn settings_radio(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Radio {
    let focus = window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let dark = theme::is_dark();
    let dot_color: gpui_kit::Hsla = if enabled {
        settings_blue()
    } else {
        rgb(if dark { 0x6f7580 } else { 0xb8b8b8 }).into()
    };
    let ring_border: gpui_kit::Hsla = if !enabled {
        rgb(if dark { 0x5c626b } else { 0xb8b8b8 }).into()
    } else {
        settings_checkbox_border(checked)
    };
    let label_color: gpui_kit::Hsla = if enabled {
        theme::foreground()
    } else {
        rgb(if dark { 0x858a94 } else { 0x6b6b6b }).into()
    };
    Radio::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(&focus)
        .flex()
        .items_center()
        .min_h(theme::scaled_px(21.).max(px(21.)))
        .py(px(1.))
        .gap(px(8.))
        .text_size(theme::ui_font_size())
        .text_color(label_color)
        .rounded(px(2.))
        .when(enabled && focus.is_focused(window), |this| {
            this.bg(settings_nav_selected())
        })
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(ring_border)
                .bg(theme::surface())
                .when(checked, |this| {
                    this.child(div().size(px(8.)).rounded_full().bg(dot_color))
                }),
        )
        .child(label)
}

pub(super) fn settings_checkbox(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    focus: &FocusHandle,
    window: &Window,
) -> Checkbox {
    let blue = settings_blue();
    Checkbox::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(focus)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(2.))
        .py(px(1.5))
        .rounded(px(2.))
        .when(focus.is_focused(window), |this| {
            this.bg(settings_nav_selected())
        })
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(settings_checkbox_border(checked))
                .bg(if checked { blue } else { theme::surface() })
                .rounded(px(3.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(px(11.5))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .child(label)
}

impl Settings {
    pub(super) const ALL_PAGES: [SettingsPage; 12] = [
        SettingsPage::Proxy,
        SettingsPage::Intruder,
        SettingsPage::Repeater,
        SettingsPage::Sessions,
        SettingsPage::ProjectTasks,
        SettingsPage::ProjectScope,
        SettingsPage::NetworkConnections,
        SettingsPage::NetworkHttp,
        SettingsPage::NetworkTls,
        SettingsPage::UserDisplay,
        SettingsPage::UserMessageEditor,
        SettingsPage::UserMisc,
    ];
    pub(super) const NAV_ORDER: [SettingsPage; 12] = Self::ALL_PAGES;

    pub(super) fn page_matches_scope(page: SettingsPage, scope: SettingsScopeFilter) -> bool {
        match scope {
            SettingsScopeFilter::All => true,
            SettingsScopeFilter::Project => matches!(
                page,
                SettingsPage::Proxy
                    | SettingsPage::Repeater
                    | SettingsPage::Sessions
                    | SettingsPage::ProjectTasks
                    | SettingsPage::ProjectScope
                    | SettingsPage::NetworkConnections
                    | SettingsPage::NetworkHttp
                    | SettingsPage::NetworkTls
            ),
            SettingsScopeFilter::User => matches!(
                page,
                SettingsPage::Intruder
                    | SettingsPage::NetworkTls
                    | SettingsPage::UserDisplay
                    | SettingsPage::UserMessageEditor
                    | SettingsPage::UserMisc
            ),
        }
    }

    pub(super) fn page_keywords(page: SettingsPage) -> &'static [&'static str] {
        match page {
            SettingsPage::Proxy => &[
                "tools",
                "proxy",
                "listener",
                "listeners",
                "bind",
                "intercept",
                "interception",
                "ssl",
                "passthrough",
                "pass through",
                "websocket",
                "ws",
                "match",
                "replace",
                "unpack",
                "response modification",
                "form",
                "代理",
                "监听",
                "拦截",
                "解包",
                "直通",
                "替换",
            ],
            SettingsPage::Intruder => &[
                "tools",
                "intruder",
                "attack",
                "closing",
                "running",
                "finished",
                "攻击",
                "关闭结果窗口",
            ],
            SettingsPage::Repeater => &[
                "tools",
                "repeater",
                "redirect",
                "redirection",
                "http/2",
                "http/1",
                "content-length",
                "timeout",
                "layout",
                "unpack",
                "connection",
                "重定向",
                "超时",
                "布局",
                "换行",
                "解包",
            ],
            SettingsPage::Sessions => &[
                "sessions",
                "session",
                "cookie",
                "cookie jar",
                "macro",
                "macros",
                "token",
                "csrf",
                "rule",
                "handling",
                "会话",
                "宏",
                "令牌",
            ],
            SettingsPage::ProjectTasks => &[
                "project",
                "tasks",
                "task",
                "pool",
                "pools",
                "resource pool",
                "concurrency",
                "delay",
                "429",
                "503",
                "任务",
                "资源池",
                "并发",
            ],
            SettingsPage::ProjectScope => &[
                "project",
                "scope",
                "target",
                "target scope",
                "include",
                "exclude",
                "prefix",
                "目标范围",
                "包含",
                "排除",
            ],
            SettingsPage::NetworkConnections => &[
                "network",
                "connections",
                "connection",
                "upstream",
                "socks",
                "socks5",
                "authentication",
                "timeout",
                "timeouts",
                "dns",
                "hostname",
                "resolution",
                "port",
                "上游代理",
                "认证",
                "超时",
                "主机名",
                "解析",
                "端口",
                "连接",
            ],
            SettingsPage::NetworkHttp => &[
                "network",
                "http",
                "http/1",
                "http/2",
                "100 continue",
                "streaming",
                "redirect",
                "keep-alive",
                "chunked",
                "event-stream",
                "流式",
                "长连接",
            ],
            SettingsPage::NetworkTls => &[
                "network",
                "tls",
                "mtls",
                "ssl",
                "certificate",
                "certificates",
                "ca",
                "custom ca",
                "client tls",
                "pem",
                "pkcs12",
                "p12",
                "证书",
                "根证书",
                "客户端证书",
                "双向",
            ],
            SettingsPage::UserDisplay => &[
                "user", "display", "dark", "light", "theme", "font", "scale", "zoom", "preview",
                "显示", "外观", "深色", "浅色", "主题", "字号", "缩放",
            ],
            SettingsPage::UserMessageEditor => &[
                "user",
                "message editor",
                "editor",
                "font",
                "wrap",
                "soft wrap",
                "pretty",
                "inspector",
                "search",
                "case",
                "hide",
                "header",
                "headers",
                "specified",
                "hide specified headers",
                "编辑器",
                "自动换行",
                "字号",
                "格式化",
                "标头",
                "隐藏",
            ],
            SettingsPage::UserMisc => &[
                "user",
                "misc",
                "miscellaneous",
                "shortcut",
                "shortcuts",
                "hotkey",
                "hotkeys",
                "curl",
                "redact",
                "confirm",
                "update",
                "updates",
                "version",
                "快捷键",
                "热键",
                "杂项",
                "脱敏",
                "更新",
                "自动更新",
                "版本",
            ],
        }
    }

    pub(super) fn page_matches(
        page: SettingsPage,
        scope: SettingsScopeFilter,
        query: &str,
    ) -> bool {
        if !Self::page_matches_scope(page, scope) {
            return false;
        }
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return true;
        }
        let lower = trimmed.to_lowercase();
        let keywords = Self::page_keywords(page);
        lower
            .split_whitespace()
            .all(|token| keywords.iter().any(|kw| kw.contains(token)))
    }

    pub(super) fn current_search_query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().to_string()
    }

    pub(super) fn is_page_visible(&self, page: SettingsPage, cx: &App) -> bool {
        let query = self.current_search_query(cx);
        Self::page_matches(page, self.scope_filter, &query)
    }

    pub(super) fn set_scope_filter(&mut self, filter: SettingsScopeFilter, cx: &mut Context<Self>) {
        if self.scope_filter == filter {
            return;
        }
        self.scope_filter = filter;
        self.sync_active_page_with_filter(cx);
    }

    pub(super) fn page_group_index(page: SettingsPage) -> usize {
        match page {
            SettingsPage::Proxy | SettingsPage::Intruder | SettingsPage::Repeater => 0,
            SettingsPage::ProjectScope | SettingsPage::ProjectTasks => 1,
            SettingsPage::Sessions => 2,
            SettingsPage::NetworkConnections
            | SettingsPage::NetworkHttp
            | SettingsPage::NetworkTls => 3,
            SettingsPage::UserDisplay
            | SettingsPage::UserMessageEditor
            | SettingsPage::UserMisc => 4,
        }
    }

    pub(super) fn toggle_nav_group(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(slot) = self.collapsed_groups.get_mut(index) {
            *slot = !*slot;
            cx.notify();
        }
    }

    pub(super) fn expand_all_nav_groups(&mut self, cx: &mut Context<Self>) {
        self.collapsed_groups = [false; 5];
        cx.notify();
    }

    pub(super) fn collapse_all_nav_groups(&mut self, cx: &mut Context<Self>) {
        self.collapsed_groups = [true, true, false, true, true];
        cx.notify();
    }

    pub(super) fn sync_active_page_with_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.current_search_query(cx);
        if !Self::page_matches(self.page, self.scope_filter, &query)
            && let Some(first) = Self::NAV_ORDER
                .into_iter()
                .find(|&p| Self::page_matches(p, self.scope_filter, &query))
        {
            self.page = first;
            self.collapsed_groups[Self::page_group_index(first)] = false;
            if first == SettingsPage::ProjectTasks {
                self.refresh_pool_page(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn select_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = page;
        self.content_scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
        self.user_pages
            .editor_scroll
            .set_offset(gpui_kit::point(px(0.), px(0.)));
        self.user_pages
            .misc_scroll
            .set_offset(gpui_kit::point(px(0.), px(0.)));
        self.collapsed_groups[Self::page_group_index(page)] = false;
        self.focus.focus(window, cx);
        if page == SettingsPage::ProjectTasks {
            self.refresh_pool_page(cx);
        }
        cx.notify();
    }

    pub(super) fn save(&self, save_as: bool, cx: &mut Context<Self>) {
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let project = self.state.project;
        cx.defer(move |cx| {
            let _ = owner_window.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if view.project == project {
                        view.save_project(save_as, window, cx);
                    }
                })
            });
        });
    }

    pub(super) fn settings_error(&self, error: impl Into<String>, cx: &mut Context<Self>) {
        let error = error.into();
        let _ = self.owner.update(cx, |view, cx| view.fail(error, cx));
    }
}
