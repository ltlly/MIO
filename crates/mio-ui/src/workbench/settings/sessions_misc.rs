use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    button::ButtonVariants,
    menu::{DropdownMenu, PopupMenuItem},
};
use mio_core::{FinishedAttackClose, RunningAttackClose};

use super::*;

fn intruder_user_badge() -> gpui_kit::Div {
    let dark = theme::is_dark();
    div()
        .flex_shrink_0()
        .px(px(10.))
        .py(px(2.))
        .rounded_full()
        .bg(rgb(if dark { 0x273b54 } else { 0xc8d7ec }))
        .text_size(theme::scaled_px(12.))
        .text_color(rgb(if dark { 0xd4e6fa } else { 0x1f2d3d }))
        .whitespace_nowrap()
        .child("用户设置")
}

fn intruder_section_heading(title: &'static str) -> gpui_kit::Div {
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
                        .w(px(20.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::CircleQuestionMark)
                                .size(px(19.))
                                .text_color(theme::foreground()),
                        ),
                )
                .child(
                    div()
                        .text_size(theme::scaled_px(15.))
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(theme::foreground())
                        .whitespace_nowrap()
                        .child(title),
                ),
        )
        .child(intruder_user_badge())
}

fn intruder_desc_row(icon_control: impl IntoElement, desc: &'static str) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .child(
            div()
                .w(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(icon_control),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

fn intruder_reset_btn(
    id: &'static str,
    tooltip: &'static str,
    disabled: bool,
    tab_stop: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .size(px(22.))
        .p_0()
        .tab_stop(tab_stop)
        .disabled(disabled)
        .accessibility_label("恢复默认设置")
        .tooltip(tooltip)
        .child(
            Icon::new(IconName::RotateCcw)
                .size(px(19.))
                .text_color(theme::foreground()),
        )
}

fn intruder_compact_radio(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    tab_stop: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Radio {
    settings_radio(id, label, checked, enabled, window, cx)
        .min_h(theme::scaled_px(22.).max(px(21.)))
        .py(px(1.))
        .tab_stop(tab_stop)
}

impl Workbench {
    pub(in crate::workbench) fn set_proxy_unpack(
        &mut self,
        project: Option<ProjectKey>,
        response: bool,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let current = if response {
            &mut self.workspace.proxy_messages.unpack_compressed_responses
        } else {
            &mut self.workspace.proxy_messages.unpack_compressed_requests
        };
        if *current == checked {
            return;
        }
        *current = checked;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_proxy_websocket_interception(
        &mut self,
        project: Option<ProjectKey>,
        change: impl FnOnce(&mut mio_core::ProxyWebSocketInterceptionSettings),
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let mut next = self.workspace.proxy_websocket_interception;
        change(&mut next);
        if next == self.workspace.proxy_websocket_interception {
            return;
        }
        self.workspace.proxy_websocket_interception = next;
        self.save_workspace(cx);
        cx.notify();
    }
}

impl Settings {
    fn update_intruder_misc(
        &mut self,
        change: impl FnOnce(&mut mio_core::UserMiscSettings),
        cx: &mut Context<Self>,
    ) {
        if !self.state.preferences_available || !self.state.preferences.can_save() {
            return;
        }
        let mut misc = self.state.preferences.misc.clone();
        change(&mut misc);
        if misc == self.state.preferences.misc {
            return;
        }
        let display = self.state.preferences.display.clone();
        let editor = self.state.preferences.message_editor.clone();
        let _ = self.submit_user_workbench(display, editor, misc, cx);
    }

    pub(in crate::workbench) fn render_intruder_preferences(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prefs = &self.state.preferences;
        let enabled = self.state.preferences_available && prefs.can_save();
        let running = prefs.running;
        let finished = prefs.finished;
        let misc = &prefs.misc;

        // Section 1: Default Intruder side panel layout
        let settings_weak = cx.weak_entity();
        let layout_gear = Button::new("intruder-side-panel-settings")
            .ghost()
            .size(px(22.))
            .p_0()
            .tab_stop(false)
            .disabled(!enabled)
            .accessibility_label("默认 Intruder 侧边栏布局设置")
            .tooltip("默认 Intruder 侧边栏布局设置")
            .child(
                Icon::new(IconName::Settings)
                    .size(px(19.))
                    .text_color(theme::foreground()),
            )
            .dropdown_menu(move |menu, _, _| {
                let settings_weak = settings_weak.clone();
                menu.item(PopupMenuItem::new("恢复默认设置").on_click(move |_, _, cx| {
                    let _ = settings_weak.update(cx, |this, cx| {
                        this.update_intruder_misc(
                            |m| {
                                m.intruder_side_panel_left = false;
                                m.intruder_side_panel_auto_expand = true;
                            },
                            cx,
                        );
                    });
                }))
            });
        let sw_left = cx.weak_entity();
        let sw_right = cx.weak_entity();
        let sw_auto = cx.weak_entity();
        let sw_collapsed = cx.weak_entity();
        let side_panel_section = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(intruder_section_heading("默认 Intruder 侧边栏布局"))
            .child(intruder_desc_row(
                layout_gear,
                "使用以下设置选择 Intruder 侧边栏的默认布局。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .id("intruder-side-panel-position")
                            .test_support()
                            .role(Role::RadioGroup)
                            .aria_label("位置")
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child("位置")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(16.))
                                    .child(
                                        intruder_compact_radio(
                                            "intruder-side-panel-left",
                                            "左侧",
                                            misc.intruder_side_panel_left,
                                            enabled,
                                            false,
                                            window,
                                            cx,
                                        )
                                        .on_change(move |_, _, _, cx| {
                                            let _ = sw_left.update(cx, |this, cx| {
                                                this.update_intruder_misc(
                                                    |m| m.intruder_side_panel_left = true,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        intruder_compact_radio(
                                            "intruder-side-panel-right",
                                            "右侧",
                                            !misc.intruder_side_panel_left,
                                            enabled,
                                            false,
                                            window,
                                            cx,
                                        )
                                        .on_change(move |_, _, _, cx| {
                                            let _ = sw_right.update(cx, |this, cx| {
                                                this.update_intruder_misc(
                                                    |m| m.intruder_side_panel_left = false,
                                                    cx,
                                                );
                                            });
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(42.))
                            .mx(px(24.))
                            .bg(theme::border())
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .id("intruder-side-panel-display-mode")
                            .test_support()
                            .role(Role::RadioGroup)
                            .aria_label("显示模式")
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child("显示模式")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(16.))
                                    .child(
                                        intruder_compact_radio(
                                            "intruder-side-panel-auto",
                                            "空间充足时自动展开",
                                            misc.intruder_side_panel_auto_expand,
                                            enabled,
                                            false,
                                            window,
                                            cx,
                                        )
                                        .on_change(move |_, _, _, cx| {
                                            let _ = sw_auto.update(cx, |this, cx| {
                                                this.update_intruder_misc(
                                                    |m| m.intruder_side_panel_auto_expand = true,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        intruder_compact_radio(
                                            "intruder-side-panel-collapsed",
                                            "始终折叠",
                                            !misc.intruder_side_panel_auto_expand,
                                            enabled,
                                            false,
                                            window,
                                            cx,
                                        )
                                        .on_change(move |_, _, _, cx| {
                                            let _ = sw_collapsed.update(cx, |this, cx| {
                                                this.update_intruder_misc(
                                                    |m| m.intruder_side_panel_auto_expand = false,
                                                    cx,
                                                );
                                            });
                                        }),
                                    ),
                            ),
                    ),
            );

        // Section 2: Automatic payload placement
        let sw_reset_payload = cx.weak_entity();
        let sw_replace = cx.weak_entity();
        let sw_append = cx.weak_entity();
        let payload_section = div()
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(intruder_section_heading("自动 Payload 位置"))
            .child(intruder_desc_row(
                intruder_reset_btn(
                    "intruder-payload-placement-reset",
                    "将自动 Payload 位置恢复为「替换基础参数值」",
                    !enabled,
                    false,
                )
                .on_click(move |_, _, cx| {
                    let _ = sw_reset_payload.update(cx, |this, cx| {
                        this.update_intruder_misc(|m| m.intruder_auto_payload_append = false, cx);
                    });
                }),
                "使用以下设置控制自动添加的 Payload 位置是替换还是追加到现有参数值。",
            ))
            .child(
                div()
                    .id("intruder-payload-placement-options")
                    .test_support()
                    .role(Role::RadioGroup)
                    .aria_label("自动 Payload 位置")
                    .pl(px(29.))
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        intruder_compact_radio(
                            "intruder-payload-replace",
                            "替换基础参数值",
                            !misc.intruder_auto_payload_append,
                            enabled,
                            false,
                            window,
                            cx,
                        )
                        .on_change(move |_, _, _, cx| {
                            let _ = sw_replace.update(cx, |this, cx| {
                                this.update_intruder_misc(
                                    |m| m.intruder_auto_payload_append = false,
                                    cx,
                                );
                            });
                        }),
                    )
                    .child(
                        intruder_compact_radio(
                            "intruder-payload-append",
                            "追加到基础参数值",
                            misc.intruder_auto_payload_append,
                            enabled,
                            false,
                            window,
                            cx,
                        )
                        .on_change(move |_, _, _, cx| {
                            let _ = sw_append.update(cx, |this, cx| {
                                this.update_intruder_misc(
                                    |m| m.intruder_auto_payload_append = true,
                                    cx,
                                );
                            });
                        }),
                    ),
            );

        // Section 3: New tab configuration
        let sw_reset_new_tab = cx.weak_entity();
        let mut new_tab_group = div()
            .id("intruder-new-tab-config-options")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label("新建标签配置")
            .pl(px(29.))
            .flex()
            .flex_col()
            .gap(px(1.));
        for (id, label, value) in [
            ("intruder-new-tab-default", "使用默认攻击配置", 0u8),
            ("intruder-new-tab-first", "从首个标签复制配置", 1u8),
            ("intruder-new-tab-last", "从最后标签复制配置", 2u8),
        ] {
            let sw_target = cx.weak_entity();
            new_tab_group = new_tab_group.child(
                intruder_compact_radio(
                    id,
                    label,
                    misc.intruder_new_tab_config_mode == value,
                    enabled,
                    false,
                    window,
                    cx,
                )
                .on_change(move |_, _, _, cx| {
                    let _ = sw_target.update(cx, |this, cx| {
                        this.update_intruder_misc(
                            |m| m.intruder_new_tab_config_mode = value,
                            cx,
                        );
                    });
                }),
            );
        }
        let new_tab_section = div()
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(intruder_section_heading("新建标签配置"))
            .child(intruder_desc_row(
                intruder_reset_btn(
                    "intruder-new-tab-config-reset",
                    "将新建标签配置恢复为「使用默认攻击配置」",
                    !enabled,
                    false,
                )
                .on_click(move |_, _, cx| {
                    let _ = sw_reset_new_tab.update(cx, |this, cx| {
                        this.update_intruder_misc(|m| m.intruder_new_tab_config_mode = 0, cx);
                    });
                }),
                "使用以下设置控制打开新标签时 Intruder 使用的配置来源。",
            ))
            .child(new_tab_group);

        // Section 4: Behavior when closing result windows
        let mut running_group = div()
            .id("intruder-running-close-options")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label("关闭正在进行的攻击时")
            .pl(px(29.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(div().mb(px(3.)).child("关闭正在进行的攻击时："));
        for (id, label, value) in [
            (
                "intruder-running-continue",
                "在后台继续攻击",
                RunningAttackClose::Continue,
            ),
            (
                "intruder-running-delete",
                "删除攻击",
                RunningAttackClose::Delete,
            ),
            ("intruder-running-ask", "每次询问", RunningAttackClose::Ask),
        ] {
            let owner = self.owner.clone();
            running_group = running_group.child(
                intruder_compact_radio(id, label, running == value, enabled, true, window, cx)
                    .on_change(move |_, _, _, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            view.set_user_closing_preferences(
                                value,
                                view.user_preferences.finished,
                                cx,
                            )
                        });
                    }),
            );
        }
        let mut finished_group = div()
            .id("intruder-finished-close-options")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label("关闭已结束的攻击时")
            .pl(px(29.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(div().mb(px(3.)).child("关闭已结束的攻击时："));
        for (id, label, value) in [
            (
                "intruder-finished-save",
                "将攻击保存到项目文件",
                FinishedAttackClose::SaveProject,
            ),
            (
                "intruder-finished-keep",
                "保留在内存",
                FinishedAttackClose::KeepMemory,
            ),
            (
                "intruder-finished-delete",
                "删除攻击",
                FinishedAttackClose::Delete,
            ),
            (
                "intruder-finished-ask",
                "每次询问",
                FinishedAttackClose::Ask,
            ),
        ] {
            let owner = self.owner.clone();
            finished_group = finished_group.child(
                intruder_compact_radio(id, label, finished == value, enabled, true, window, cx)
                    .on_change(move |_, _, _, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            view.set_user_closing_preferences(
                                view.user_preferences.running,
                                value,
                                cx,
                            )
                        });
                    }),
            );
        }
        let closing_section = div()
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(intruder_section_heading("关闭结果窗口时的行为"))
            .child(intruder_desc_row(
                intruder_reset_btn(
                    "intruder-closing-defaults",
                    "将关闭运行中或已结束攻击窗口的行为均恢复为「每次询问」",
                    !enabled,
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let _ = this.owner.update(cx, |view, cx| {
                        view.set_user_closing_preferences(
                            RunningAttackClose::Ask,
                            FinishedAttackClose::Ask,
                            cx,
                        )
                    });
                })),
                "使用以下设置控制 Intruder 在关闭结果窗口时的行为。",
            ))
            .child(running_group)
            .child(finished_group);

        let mut body = div()
            .id("settings-intruder-inner")
            .px(px(22.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.));

        for (id, error) in [
            ("user-preferences-load-error", &prefs.load_error),
            ("user-preferences-save-error", &prefs.error),
        ] {
            if let Some(error) = error {
                body = body.child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .ml(px(29.))
                        .p(px(6.))
                        .rounded(px(4.))
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .child(error.clone()),
                );
            }
        }
        if prefs.load_error.is_some() || prefs.error.is_some() {
            body = body.child(
                div().pl(px(29.)).child(
                    settings_action_button(
                        "user-preferences-retry",
                        "重试",
                        !self.state.preferences_available || prefs.busy(),
                    )
                    .tab_stop(false)
                    .tooltip("重新读取并同步用户级 Intruder 偏好设置")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let _ = this
                            .owner
                            .update(cx, |view, cx| view.retry_user_preferences(cx));
                    })),
                ),
            );
        }

        let content = body
            .child(side_panel_section)
            .child(payload_section)
            .child(new_tab_section)
            .child(closing_section);
        settings_scrollable_pane("settings-intruder-page", &self.content_scroll, content)
            .into_any_element()
    }
}
