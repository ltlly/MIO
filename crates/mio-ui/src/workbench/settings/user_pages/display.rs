use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::{
    Icon,
    menu::{DropdownMenu, PopupMenuItem},
    select::Select,
};

use super::*;

fn preview_card_frame(selected: bool, dark: bool) -> gpui_kit::Div {
    let border_color = if selected {
        settings_blue()
    } else {
        rgb(if dark { 0x454952 } else { 0xd8dce3 }).into()
    };
    let card = div()
        .w(px(168.))
        .h(px(94.))
        .rounded(px(7.))
        .overflow_hidden()
        .bg(rgb(if dark { 0x26292e } else { 0xe9ecf0 }))
        .flex()
        .flex_col()
        .cursor_pointer();
    if selected {
        card.border_2().border_color(border_color)
    } else {
        card.border_1().border_color(border_color)
    }
}

fn display_dpi_checkbox(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    focus: &FocusHandle,
    window: &Window,
) -> Checkbox {
    let dark = theme::is_dark();
    let blue = settings_blue();
    let box_border: gpui_kit::Hsla = if checked {
        blue
    } else if !enabled {
        rgb(if dark { 0x444850 } else { 0xd8d8d8 }).into()
    } else {
        settings_checkbox_border(false)
    };
    let label_color: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x222222).into()
    };
    Checkbox::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(focus)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(2.))
        .py(px(3.))
        .rounded(px(2.))
        .text_color(label_color)
        .when(focus.is_focused(window), |this| {
            this.bg(rgb(if dark { 0x2e436e } else { 0xd0dfef }))
        })
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(box_border)
                .bg(if checked { blue } else { theme::surface() })
                .rounded(px(3.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(AssetIconName::Check)
                            .size(px(11.5))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .child(label)
}

fn render_table_preview_card(id: &'static str, zebra: bool, selected: bool) -> gpui_kit::base::ObservedElement<gpui_kit::Stateful<gpui_kit::Div>> {
    let dark = theme::is_dark();
    let chrome_bg = rgb(if dark { 0x2f333a } else { 0xe8ebf0 });
    let cell_bg = rgb(if dark { 0x1e2024 } else { 0xffffff });
    let zebra_bg = rgb(if dark { 0x282b30 } else { 0xf2f4f7 });
    let text_col = rgb(if dark { 0xdfe1e5 } else { 0x222222 });
    preview_card_frame(selected, dark)
        .id(id)
        .test_support()
        .child(div().h(px(12.)).w_full().bg(chrome_bg).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().w(px(13.)).h_full().bg(chrome_bg).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .bg(cell_bg)
                        .border_t_1()
                        .border_l_1()
                        .border_color(rgb(if dark { 0x3b3f47 } else { 0xdfe3e8 }))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_1()
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .bg(cell_bg)
                                .text_size(px(11.5))
                                .text_color(text_col)
                                .child("数据"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .bg(if zebra { zebra_bg } else { cell_bg })
                                .text_size(px(11.5))
                                .text_color(text_col)
                                .child("数据"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .bg(cell_bg)
                                .text_size(px(11.5))
                                .text_color(text_col)
                                .child("数据"),
                        ),
                ),
        )
}

fn render_issues_preview_card(id: &'static str, grouped: bool, selected: bool) -> gpui_kit::base::ObservedElement<gpui_kit::Stateful<gpui_kit::Div>> {
    let dark = theme::is_dark();
    let chrome_bg = rgb(if dark { 0x2f333a } else { 0xe8ebf0 });
    let body_bg = rgb(if dark { 0x1e2024 } else { 0xffffff });
    let pill_fg = rgb(if dark { 0x9aa3b2 } else { 0x4b5563 });
    let line_fg = rgb(if dark { 0x4b515c } else { 0xc9ced6 });
    let sep_col = rgb(if dark { 0x3b3f47 } else { 0xe2e6ec });

    let tab_pill = |w: f32| div().w(px(w)).h(px(4.)).rounded_full().bg(pill_fg);
    let tab_bar = if grouped {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(tab_pill(14.))
            .child(tab_pill(14.))
            .child(
                div()
                    .px(px(4.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(rgb(if dark { 0x4f6d96 } else { 0x9cb3cf }))
                    .bg(rgb(if dark { 0x243142 } else { 0xf4f7fb }))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .size(px(5.))
                            .rounded(px(1.))
                            .bg(settings_blue()),
                    )
                    .child(tab_pill(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .child(tab_pill(14.))
                            .child(div().w(px(14.)).h(px(1.5)).bg(pill_fg)),
                    )
                    .child(tab_pill(12.)),
            )
    } else {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .py(px(3.))
            .child(tab_pill(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(tab_pill(14.))
                    .child(div().w(px(14.)).h(px(1.5)).bg(pill_fg)),
            )
            .child(tab_pill(14.))
            .child(tab_pill(14.))
            .child(tab_pill(14.))
    };

    preview_card_frame(selected, dark)
        .id(id)
        .test_support()
        .child(div().h(px(11.)).w_full().bg(chrome_bg).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().w(px(12.)).h_full().bg(chrome_bg).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .bg(body_bg)
                        .border_t_1()
                        .border_l_1()
                        .border_color(sep_col)
                        .px(px(8.))
                        .py(px(7.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(tab_bar)
                        .child(div().w_full().h(px(1.)).bg(sep_col))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(5.))
                                .pt(px(2.))
                                .child(div().w(px(96.)).h(px(4.)).rounded_full().bg(line_fg))
                                .child(div().w(px(82.)).h(px(4.)).rounded_full().bg(line_fg))
                                .child(div().w(px(66.)).h(px(4.)).rounded_full().bg(line_fg)),
                        ),
                ),
        )
}

impl Settings {
    pub(super) fn set_user_display_theme(&mut self, mode: UserThemeMode, cx: &mut Context<Self>) {
        if self.state.preferences.display.theme == mode {
            return;
        }
        let mut next = self.state.preferences.display.clone();
        next.theme = mode;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        self.submit_user_workbench(next, editor, misc, cx);
    }

    pub(super) fn set_user_display_font_size(&mut self, px_size: u8, cx: &mut Context<Self>) {
        let clamped = px_size.clamp(11, 18);
        if self.state.preferences.display.ui_font_size_px == clamped {
            return;
        }
        let mut next = self.state.preferences.display.clone();
        next.ui_font_size_px = clamped;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        self.submit_user_workbench(next, editor, misc, cx);
    }

    pub(super) fn set_user_display_scale_percent(&mut self, percent: u16, cx: &mut Context<Self>) {
        let clamped = percent.clamp(80, 150);
        if self.state.preferences.display.scale_factor_percent == clamped {
            return;
        }
        let mut next = self.state.preferences.display.clone();
        next.scale_factor_percent = clamped;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        self.submit_user_workbench(next, editor, misc, cx);
    }

    pub(super) fn set_user_display_use_system_dpi(
        &mut self,
        use_system_dpi: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.preferences.display.use_system_dpi == use_system_dpi {
            return;
        }
        let mut next = self.state.preferences.display.clone();
        next.use_system_dpi = use_system_dpi;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(next, editor, misc, cx) {
            self.user_pages.use_system_dpi = use_system_dpi;
        }
    }

    pub(super) fn set_user_display_scaling(
        &mut self,
        percent: u16,
        use_system_dpi: bool,
        cx: &mut Context<Self>,
    ) {
        let clamped = percent.clamp(80, 150);
        let current = &self.state.preferences.display;
        if current.scale_factor_percent == clamped && current.use_system_dpi == use_system_dpi {
            return;
        }
        let mut next = current.clone();
        next.scale_factor_percent = clamped;
        next.use_system_dpi = use_system_dpi;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(next, editor, misc, cx) {
            self.user_pages.use_system_dpi = use_system_dpi;
        }
    }

    pub(super) fn set_user_display_table_zebra_stripe(
        &mut self,
        zebra: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.preferences.display.table_zebra_stripe == zebra {
            return;
        }
        let mut next = self.state.preferences.display.clone();
        next.table_zebra_stripe = zebra;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(next, editor, misc, cx) {
            self.user_pages.table_zebra_stripe = zebra;
        }
    }

    pub(super) fn set_user_display_issues_grouped_evidence(
        &mut self,
        grouped: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.preferences.display.issues_grouped_evidence == grouped {
            return;
        }
        let mut next = self.state.preferences.display.clone();
        next.issues_grouped_evidence = grouped;
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(next, editor, misc, cx) {
            self.user_pages.issues_grouped_evidence = grouped;
        }
    }

    pub(super) fn reset_user_display_defaults(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let defaults = UserDisplaySettings::default();
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(defaults.clone(), editor, misc, cx) {
            self.user_pages.configure_scaling = false;
            self.user_pages.use_system_dpi = false;
            self.user_pages.table_zebra_stripe = true;
            self.user_pages.issues_grouped_evidence = true;
            self.user_pages.ui_font_size_select.update(cx, |s, cx| {
                s.set_selected_index(
                    Some(IndexPath::new(font_size_to_index(defaults.ui_font_size_px))),
                    window,
                    cx,
                )
            });
            self.user_pages.scale_factor_select.update(cx, |s, cx| {
                s.set_selected_index(
                    Some(IndexPath::new(scale_percent_to_index(
                        defaults.scale_factor_percent,
                    ))),
                    window,
                    cx,
                )
            });
        }
    }

    pub(in crate::workbench) fn render_user_display(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = (
            self.user_pages.use_system_dpi,
            self.user_pages.table_zebra_stripe,
            self.user_pages.issues_grouped_evidence,
        );
        let enabled = self.user_settings_enabled();
        let display = &self.state.preferences.display;
        let configure_scaling = self.user_pages.configure_scaling
            || display.scale_factor_percent != 100
            || display.use_system_dpi;
        let use_system_dpi = display.use_system_dpi;
        let scale_dropdown_enabled = enabled && configure_scaling && !use_system_dpi;
        let zebra = display.table_zebra_stripe;
        let grouped = display.issues_grouped_evidence;

        // Section 1: Appearance (uses ⚙ icon)
        let owner_gear = cx.weak_entity();
        let appearance_gear = user_gear_menu_button(
            "settings-user-display-settings",
            "外观设置菜单",
            !enabled,
        )
        .dropdown_menu(move |menu, _, _| {
            let owner = owner_gear.clone();
            menu.item(PopupMenuItem::new("恢复默认设置").on_click(move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| this.reset_user_display_defaults(window, cx));
            }))
        });

        let light_owner = cx.entity();
        let dark_owner = cx.entity();
        let theme_group = div()
            .id("settings-user-theme-group")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label("主题")
            .flex()
            .items_center()
            .gap(px(14.))
            .child(div().flex_shrink_0().child("主题："))
            .child(
                settings_radio(
                    "settings-user-theme-light",
                    "浅色",
                    display.theme == UserThemeMode::Light,
                    enabled,
                    window,
                    cx,
                )
                .on_change(move |_, _, _, cx| {
                    light_owner.update(cx, |this, cx| {
                        this.set_user_display_theme(UserThemeMode::Light, cx);
                    });
                }),
            )
            .child(
                settings_radio(
                    "settings-user-theme-dark",
                    "深色",
                    display.theme == UserThemeMode::Dark,
                    enabled,
                    window,
                    cx,
                )
                .on_change(move |_, _, _, cx| {
                    dark_owner.update(cx, |this, cx| {
                        this.set_user_display_theme(UserThemeMode::Dark, cx);
                    });
                }),
            );

        let appearance_section = div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(user_section_heading(
                Some("settings-user-display-heading"),
                Some("settings-user-display-scope"),
                "外观",
            ))
            .child(user_desc_row_with_icon(
                appearance_gear,
                "使用以下设置控制用户界面的外观。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_shrink_0().child("界面字号："))
                            .child(
                                div()
                                    .w(theme::scaled_px(68.).max(px(68.)))
                                    .flex_shrink_0()
                                    .child(
                                        Select::new(&self.user_pages.ui_font_size_select)
                                            .id("settings-user-ui-font-select")
                                            .small()
                                            .w_full()
                                            .disabled(!enabled),
                                    ),
                            ),
                    )
                    .child(theme_group),
            );

        // Section 2: Scaling (uses ↺ icon)
        let weak_cfg = cx.weak_entity();
        let configure_scaling_cb = settings_checkbox(
            "settings-user-scale-configure",
            "配置缩放设置",
            configure_scaling,
            enabled,
            &self.user_pages.configure_scaling_focus,
            window,
        )
        .on_change(move |st, _, window, cx| {
            let checked = st == CheckboxState::Checked;
            let _ = weak_cfg.update(cx, |this, cx| {
                this.user_pages.configure_scaling = checked;
                if !checked
                    && (this.state.preferences.display.scale_factor_percent != 100
                        || this.state.preferences.display.use_system_dpi)
                {
                    this.set_user_display_scaling(100, false, cx);
                    this.user_pages.scale_factor_select.update(cx, |s, cx| {
                        s.set_selected_index(
                            Some(IndexPath::new(scale_percent_to_index(100))),
                            window,
                            cx,
                        )
                    });
                }
                cx.notify();
            });
        });

        let weak_dpi = cx.weak_entity();
        let system_dpi_cb = display_dpi_checkbox(
            "settings-user-scale-system-dpi",
            "使用系统 DPI 设置",
            use_system_dpi,
            enabled && configure_scaling,
            &self.user_pages.system_dpi_focus,
            window,
        )
        .on_change(move |st, _, _, cx| {
            let _ = weak_dpi.update(cx, |this, cx| {
                this.set_user_display_use_system_dpi(st == CheckboxState::Checked, cx);
            });
        });

        let dark = theme::is_dark();
        let scale_label_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x222222).into()
        };
        let scale_select_border = rgb(if dark { 0x454952 } else { 0xd6d6d6 });

        let scaling_section = div()
            .pt(px(10.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(user_section_heading(
                Some("settings-user-scaling-heading"),
                None,
                "缩放",
            ))
            .child(user_desc_row_with_icon(
                user_reset_icon_button(
                    "settings-user-display-reset",
                    "将主题外观、界面字号与缩放比例恢复为用户级默认值",
                    !enabled,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.reset_user_display_defaults(window, cx)
                })),
                "使用以下设置控制高分辨率屏幕上的界面缩放显示。部分更改需重启后生效。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(4.))
                    .child(configure_scaling_cb)
                    .child(div().pl(px(22.)).child(system_dpi_cb))
                    .child(
                        div()
                            .pl(px(22.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(scale_label_color)
                                    .child("缩放比例："),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(76.).max(px(76.)))
                                    .flex_shrink_0()
                                    .child(
                                        Select::new(&self.user_pages.scale_factor_select)
                                            .id("settings-user-scale-select")
                                            .small()
                                            .w_full()
                                            .disabled(!scale_dropdown_enabled)
                                            .when(!scale_dropdown_enabled, |this| {
                                                this.bg(theme::surface())
                                                    .opacity(1.0)
                                                    .border_color(scale_select_border)
                                                    .text_color(scale_label_color)
                                            }),
                                    ),
                            ),
                    ),
            );

        // Section 3: Table appearance
        let e_zebra_card = cx.entity();
        let e_zebra_radio = cx.entity();
        let e_no_zebra_card = cx.entity();
        let e_no_zebra_radio = cx.entity();
        let table_section = div()
            .pt(px(10.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(user_section_heading(
                Some("settings-user-table-appearance-heading"),
                None,
                "表格外观",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "使用以下设置控制表格的视觉样式与外观。",
            ))
            .child(
                div()
                    .id("settings-user-table-appearance-group")
                    .test_support()
                    .role(Role::RadioGroup)
                    .aria_label("表格外观")
                    .pl(px(29.))
                    .pt(px(3.))
                    .flex()
                    .items_start()
                    .gap(px(64.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(5.))
                            .child(
                                render_table_preview_card(
                                    "settings-user-table-zebra-card",
                                    true,
                                    zebra,
                                )
                                .on_click(move |_, _, cx| {
                                    e_zebra_card.update(cx, |this, cx| {
                                        if this.user_settings_enabled() {
                                            this.set_user_display_table_zebra_stripe(true, cx);
                                        }
                                    });
                                }),
                            )
                            .child(
                                settings_radio(
                                    "settings-user-table-zebra",
                                    "斑马纹",
                                    zebra,
                                    enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |_, _, _, cx| {
                                    e_zebra_radio.update(cx, |this, cx| {
                                        this.set_user_display_table_zebra_stripe(true, cx);
                                    });
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(5.))
                            .child(
                                render_table_preview_card(
                                    "settings-user-table-no-zebra-card",
                                    false,
                                    !zebra,
                                )
                                .on_click(move |_, _, cx| {
                                    e_no_zebra_card.update(cx, |this, cx| {
                                        if this.user_settings_enabled() {
                                            this.set_user_display_table_zebra_stripe(false, cx);
                                        }
                                    });
                                }),
                            )
                            .child(
                                settings_radio(
                                    "settings-user-table-no-zebra",
                                    "无斑马纹",
                                    !zebra,
                                    enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |_, _, _, cx| {
                                    e_no_zebra_radio.update(cx, |this, cx| {
                                        this.set_user_display_table_zebra_stripe(false, cx);
                                    });
                                }),
                            ),
                    ),
            );

        // Section 4: Issues
        let e_grp_card = cx.entity();
        let e_grp_radio = cx.entity();
        let e_ungrp_card = cx.entity();
        let e_ungrp_radio = cx.entity();
        let issues_section = div()
            .pt(px(10.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(user_section_heading(
                Some("settings-user-issues-heading"),
                None,
                "问题展示",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "使用以下设置控制扫描问题证据的展示方式。",
            ))
            .child(
                div()
                    .id("settings-user-issues-group")
                    .test_support()
                    .role(Role::RadioGroup)
                    .aria_label("问题展示")
                    .pl(px(29.))
                    .pt(px(3.))
                    .flex()
                    .items_start()
                    .gap(px(64.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(5.))
                            .child(
                                render_issues_preview_card(
                                    "settings-user-issues-grouped-card",
                                    true,
                                    grouped,
                                )
                                .on_click(move |_, _, cx| {
                                    e_grp_card.update(cx, |this, cx| {
                                        if this.user_settings_enabled() {
                                            this.set_user_display_issues_grouped_evidence(true, cx);
                                        }
                                    });
                                }),
                            )
                            .child(
                                settings_radio(
                                    "settings-user-issues-grouped",
                                    "分组证据",
                                    grouped,
                                    enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |_, _, _, cx| {
                                    e_grp_radio.update(cx, |this, cx| {
                                        this.set_user_display_issues_grouped_evidence(true, cx);
                                    });
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(5.))
                            .child(
                                render_issues_preview_card(
                                    "settings-user-issues-ungrouped-card",
                                    false,
                                    !grouped,
                                )
                                .on_click(move |_, _, cx| {
                                    e_ungrp_card.update(cx, |this, cx| {
                                        if this.user_settings_enabled() {
                                            this.set_user_display_issues_grouped_evidence(
                                                false, cx,
                                            );
                                        }
                                    });
                                }),
                            )
                            .child(
                                settings_radio(
                                    "settings-user-issues-ungrouped",
                                    "非分组证据",
                                    !grouped,
                                    enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |_, _, _, cx| {
                                    e_ungrp_radio.update(cx, |this, cx| {
                                        this.set_user_display_issues_grouped_evidence(false, cx);
                                    });
                                }),
                            ),
                    ),
            );

        let body = div()
            .id("settings-user-display-inner")
            .px(px(22.))
            .pt(px(12.))
            .pb(px(24.))
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(appearance_section)
            .child(scaling_section)
            .child(table_section)
            .child(issues_section);

        settings_scrollable_pane(
            "settings-user-display-page",
            &self.content_scroll,
            self.append_user_pages_feedback(body, cx),
        )
        .into_any_element()
    }
}
