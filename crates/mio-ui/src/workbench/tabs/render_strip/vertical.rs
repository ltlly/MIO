use super::*;
use gpui_kit::FontWeight;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};

impl Workbench {
    pub(super) fn render_vertical_sidebar_header(
        &self,
        total_tabs: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled =
            self.project.is_none() || self.transition.is_some() || !self.backend_available();
        let seed_tab = self
            .workspace
            .active
            .or_else(|| self.workspace.tabs.first().map(|tab| tab.id));
        let vertical_sidebar_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x1e2024 } else { 0xf6f7f9 }).into();
        div()
            .id("repeater-vertical-header")
            .test_support()
            .min_h(theme::scaled_px(34.).max(px(34.)))
            .px(theme::scaled_px(10.))
            .py(px(2.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme::scaled_px(4.))
            .border_b_1()
            .border_color(theme::border())
            .bg(vertical_sidebar_bg)
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::muted_foreground())
                    .truncate()
                    .child(format!("标签页 ({total_tabs})")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(
                        Button::new("repeater-vertical-new-tab")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .h(theme::scaled_px(24.).max(px(24.)))
                            .w(theme::scaled_px(24.).max(px(24.)))
                            .p_0()
                            .accessibility_label("新建请求标签")
                            .tooltip(if cfg!(target_os = "macos") {
                                "新建空白 HTTP 请求标签页（⌘T）"
                            } else {
                                "新建空白 HTTP 请求标签页（Ctrl+T）"
                            })
                            .tab_stop(false)
                            .disabled(disabled)
                            .child(
                                gpui_kit::component::Icon::new(IconName::Plus)
                                    .size(px(14.))
                                    .text_color(if disabled {
                                        theme::muted_foreground()
                                    } else {
                                        theme::foreground()
                                    }),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
                    )
                    .child(
                        Button::new("repeater-vertical-new-group")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .h(theme::scaled_px(24.).max(px(24.)))
                            .w(theme::scaled_px(24.).max(px(24.)))
                            .p_0()
                            .accessibility_label("新建分组")
                            .tooltip("新建 Repeater 标签分组以便批量管理与并发发送请求")
                            .tab_stop(false)
                            .disabled(disabled || self.workspace.tabs.is_empty())
                            .child(
                                gpui_kit::component::Icon::new(IconName::FolderPlus)
                                    .size(px(14.))
                                    .text_color(if disabled || self.workspace.tabs.is_empty() {
                                        theme::muted_foreground()
                                    } else {
                                        theme::foreground()
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_tab_group_editor(None, seed_tab, window, cx);
                            })),
                    )
                    .child(
                        Button::new("repeater-toggle-vertical-tabs")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .h(theme::scaled_px(24.).max(px(24.)))
                            .w(theme::scaled_px(24.).max(px(24.)))
                            .p_0()
                            .tooltip("切换回顶部横向标签栏布局")
                            .accessibility_label("切换为水平标签")
                            .tab_stop(false)
                            .selected(true)
                            .disabled(disabled)
                            .child(
                                gpui_kit::component::Icon::new(IconName::PanelTop)
                                    .size(px(14.))
                                    .text_color(if disabled {
                                        theme::muted_foreground()
                                    } else {
                                        theme::foreground()
                                    }),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_tab_orientation(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_vertical_group_header(
        &self,
        group: &TabGroup,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = group.id;
        let color = super::super::tab_groups::group_color(group.color);
        let can_edit = self.backend_available() && self.transition.is_none();
        let can_send = self.can_send_group(id);
        let weak = cx.weak_entity();
        let row_group = SharedString::from(format!("vgroup-row-{id}"));
        let arrow = if group.collapsed { "▸" } else { "▾" };
        div()
            .id(SharedString::from(format!("group-{id}")))
            .group(row_group.clone())
            .role(gpui_kit::accesskit::Role::Button)
            .test_support()
            .w_full()
            .min_h(theme::scaled_px(28.).max(px(28.)))
            .mt(px(4.))
            .px(theme::scaled_px(10.))
            .rounded(px(5.))
            .flex_shrink_0()
            .relative()
            .flex()
            .items_center()
            .gap(theme::scaled_px(6.))
            .hover(|s| s.bg(theme::foreground().opacity(0.05)))
            .cursor_pointer()
            .aria_label(group.title.clone())
            .aria_expanded(!group.collapsed)
            .child(
                div()
                    .absolute()
                    .top(px(5.))
                    .bottom(px(5.))
                    .left(px(2.))
                    .w(px(3.))
                    .rounded(px(1.5))
                    .bg(color),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::foreground())
                    .child(format!("{arrow} {} ({count})", group.title)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .opacity(0.)
                    .group_hover(row_group, |style| style.opacity(1.))
                    .child(
                        Button::new(SharedString::from(format!("group-send-{id}")))
                            .icon(IconName::SendHorizontal)
                            .small()
                            .h(theme::row_height())
                            .w(theme::row_height())
                            .px(px(0.))
                            .accessibility_label("发送")
                            .tooltip("并发发送此分组内的全部请求标签页")
                            .tab_stop(false)
                            .disabled(!can_send)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.send_group(id, window, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("group-dup-{id}")))
                            .icon(IconName::Copy)
                            .small()
                            .h(theme::row_height())
                            .w(theme::row_height())
                            .px(px(0.))
                            .accessibility_label("复制")
                            .tooltip("复制整个标签分组及其包含的所有请求草稿")
                            .tab_stop(false)
                            .disabled(!can_edit)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.duplicate_repeater_group(id, window, cx);
                            })),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.backend_available() || this.transition.is_some() {
                    return;
                }
                if let Some(target) = this.workspace.groups.iter_mut().find(|g| g.id == id) {
                    target.collapsed = !target.collapsed;
                }
                this.save_workspace(cx);
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                this.move_to_group(drag.id, Some(id), window, cx)
            }))
            .context_menu(move |menu, window, cx| {
                repeater_group_menu(id, menu, window, cx, weak.clone())
            })
            .into_any_element()
    }

    pub(super) fn render_websocket_work_tab(
        &self,
        tab: &mio_core::WebSocketRepeaterTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = tab.id;
        let vertical = self.workspace.orientation == Orientation::Vertical;
        let selected =
            self.workspace.active.is_none() && self.workspace.active_websocket_tab == Some(id);
        let tab_group = tab
            .group
            .and_then(|gid| self.workspace.groups.iter().find(|group| group.id == gid));
        let group_accent = tab_group.map(|group| super::super::tab_groups::group_color(group.color));
        let horizontal_tinted = !vertical && group_accent.is_some();
        let editors = self.websocket_repeater.editors.get(&id);
        let busy = editors.is_some_and(|ed| ed.connecting.is_some() || ed.sending.is_some());
        let dirty = editors.is_some_and(|ed| ed.dirty);
        let base_title = if tab.title.starts_with("WS ") {
            tab.title.clone()
        } else {
            format!("WS {}", tab.title)
        };
        let title = format!(
            "{}{}{}",
            if busy { "◌ " } else { "" },
            base_title,
            if dirty { " •" } else { "" }
        );
        let tab_tooltip = SharedString::from(match tab_group {
            Some(group) => format!(
                "切换到 WebSocket 标签「{}」（所属分组：{}，右键查看更多操作）",
                base_title, group.title
            ),
            None => format!("切换到 WebSocket 标签「{}」（右键重命名、分组或复制）", base_title),
        });
        let vertical_active_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2f333a } else { 0xe8eaed }).into();
        let tab_hover_bg: gpui_kit::Hsla = if vertical {
            if selected {
                vertical_active_bg
            } else {
                theme::foreground().opacity(0.06)
            }
        } else if let Some(accent) = group_accent {
            accent.opacity(0.88)
        } else if selected {
            rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into()
        } else {
            theme::surface().opacity(0.85)
        };
        let close_hover_bg = if horizontal_tinted {
            gpui_kit::black().opacity(0.12)
        } else {
            theme::foreground().opacity(0.12)
        };
        let weak = cx.weak_entity();
        div()
            .id(SharedString::from(format!("ws-work-tab-{id}")))
            .role(gpui_kit::accesskit::Role::Tab)
            .aria_label(title.clone())
            .aria_selected(selected)
            .test_support()
            .relative()
            .flex()
            .items_center()
            .h(px(26.))
            .flex_shrink_0()
            .min_w(theme::scaled_px(58.))
            .px(theme::scaled_px(8.))
            .gap(theme::scaled_px(7.))
            .when(vertical, |this| {
                this.w_full()
                    .h(theme::scaled_px(30.).max(px(30.)))
                    .rounded(px(5.))
                    .px(theme::scaled_px(10.))
                    .border_b_1()
                    .border_color(gpui_kit::transparent_black())
                    .when(group_accent.is_some(), |row| row.pl(theme::scaled_px(14.)))
                    .text_color(theme::foreground())
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
            })
            .when(!vertical, |this| {
                this.max_w(theme::scaled_px(240.))
                    .pb(px(2.))
                    .rounded(px(4.))
                    .overflow_hidden()
            })
            .when(horizontal_tinted, |this| this.text_color(rgb(0x1f2328)))
            .when(
                !vertical && !horizontal_tinted && selected && !theme::is_dark(),
                |this| this.text_color(rgb(0x000000)),
            )
            .bg(if vertical {
                if selected {
                    vertical_active_bg
                } else {
                    gpui_kit::transparent_black()
                }
            } else {
                group_accent.unwrap_or_else(|| {
                    if selected {
                        rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into()
                    } else {
                        theme::background()
                    }
                })
            })
            .hover(move |s| s.bg(tab_hover_bg))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tab_tooltip.clone()).build(window, cx)
            })
            .when_some(group_accent.filter(|_| vertical), |this, accent| {
                this.child(
                    div()
                        .absolute()
                        .top(px(5.))
                        .bottom(px(5.))
                        .left(px(5.))
                        .w(px(3.))
                        .rounded(px(1.5))
                        .bg(accent),
                )
            })
            .cursor_pointer()
            .child(div().flex_1().truncate().child(title))
            .child(
                flat_button(
                    SharedString::from(format!("close-ws-tab-{id}")),
                    false,
                    window,
                    cx,
                )
                .accessibility_label("关闭标签")
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("关闭此 WebSocket 标签页")
                        .build(window, cx)
                })
                .size(theme::scaled_px(14.))
                .rounded(px(3.))
                .flex()
                .items_center()
                .justify_center()
                .when(vertical, |b| b.text_color(theme::muted_foreground()))
                .when(horizontal_tinted, |b| b.text_color(rgb(0x1f2328)))
                .hover(move |s| s.bg(close_hover_bg))
                .child(
                    gpui_kit::component::Icon::new(IconName::Close)
                        .size(theme::scaled_px(13.)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_websocket_repeater_tab(id, window, cx);
                })),
            )
            .when(selected, |tab| {
                tab.child(if vertical {
                    div()
                        .absolute()
                        .top(px(5.))
                        .bottom(px(5.))
                        .left_0()
                        .w(px(3.))
                        .rounded_r(px(2.))
                        .bg(theme::accent())
                } else {
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(3.))
                        .rounded_b(px(4.))
                        .bg(theme::accent())
                })
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_websocket_repeater_tab(id, window, cx)
            }))
            .context_menu(move |menu, window, cx| {
                ws_repeater_tab_menu(weak.clone(), id, menu, window, cx)
            })
            .into_any_element()
    }
}
