use gpui_kit::{
    FontWeight,
    accesskit::Role,
    assets::IconName as AssetIcon,
    component::{
        Icon,
        button::{ButtonRounded, ButtonVariants},
        popover::Popover,
    },
};

use super::*;

fn render_brand_lockup() -> gpui_kit::Div {
    let dark = theme::is_dark();
    let wordmark_fg = rgb(if dark { 0xdfe1e5 } else { 0x0f172a });
    let subtitle_fg = rgb(if dark { 0x7aa2f7 } else { 0x3b63d6 });
    div()
        .flex_shrink_0()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            gpui_kit::img(crate::chrome::logo_image())
                .w(px(34.))
                .h(px(34.))
                .flex_shrink_0()
                .rounded(px(6.)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .font_family(theme::ui_font_family())
                        .text_size(theme::scaled_px(26.))
                        .font_weight(FontWeight::BOLD)
                        .line_height(gpui_kit::relative(1.0))
                        .text_color(wordmark_fg)
                        .child("MIO"),
                )
                .child(
                    div()
                        .font_family(theme::ui_font_family())
                        .text_size(theme::scaled_px(14.))
                        .font_weight(FontWeight::NORMAL)
                        .line_height(gpui_kit::relative(1.15))
                        .text_color(subtitle_fg)
                        .child("Workbench"),
                ),
        )
}

impl Workbench {
    pub(in crate::workbench) fn render_startup_wizard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(wizard) = self.startup_wizard.as_ref() else {
            return div().into_any_element();
        };
        let step = wizard.step;
        let cancel_confirm_open = wizard.cancel_confirm_open;
        let last_start_failed = wizard.last_start_failed;
        let error = wizard.error.clone();
        let controls_enabled = self.backend_available()
            && !self.file_picker_pending()
            && step != WizardStep::Starting
            && !cancel_confirm_open;
        let can_retry = last_start_failed && controls_enabled;

        let intro_text = match step {
            WizardStep::Step1 => "欢迎使用 MIO。请使用下方选项创建或打开项目。",
            WizardStep::Step2 => "请选择要为此项目加载的配置。",
            WizardStep::Starting => "正在启动项目，请稍候…",
        };
        let help_text = match step {
            WizardStep::Step1 => {
                "临时项目仅在内存中保留数据；磁盘项目会持续保存到 .mio 文件，也支持打开已有的 .mio 或外部归档项目。"
            }
            WizardStep::Step2 | WizardStep::Starting => {
                "可使用 MIO 默认配置、沿用项目内保存的设置，或从本地 JSON 配置文件加载初始设置。"
            }
        };
        let body = self.render_wizard_step_body(window, cx);

        let cancel_btn = wizard_neutral_button(
            "wizard-cancel",
            "取消",
            None,
            !cancel_confirm_open,
            WizardButtonSize::Footer,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.open_wizard_cancel_confirm(cx);
        }));
        let new_window_btn = wizard_neutral_button(
            "wizard-new-window",
            "新建窗口",
            Some(if cfg!(target_os = "macos") {
                "打开新的独立 MIO 窗口 (⌘⇧N)"
            } else {
                "打开新的独立 MIO 窗口 (Ctrl+Shift+N)"
            }),
            !cancel_confirm_open,
            WizardButtonSize::Footer,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.open_new_window(cx);
        }));

        let footer_buttons = match step {
            WizardStep::Step1 => div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(10.))
                .child(cancel_btn)
                .child(
                    wizard_primary_button("wizard-next", "下一步", None, controls_enabled, 86.)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.wizard_next(window, cx);
                        })),
                ),
            WizardStep::Step2 | WizardStep::Starting => div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(10.))
                .child(cancel_btn)
                .child(
                    wizard_neutral_button(
                        "wizard-back",
                        "上一步",
                        None,
                        controls_enabled,
                        WizardButtonSize::Footer,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wizard_back(cx);
                    })),
                )
                .child(
                    wizard_primary_button(
                        "wizard-start",
                        "启动 MIO",
                        None,
                        controls_enabled,
                        96.,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.wizard_start(window, cx);
                    })),
                ),
        };

        let title_bar = crate::chrome::render_title_bar(
            "mio-wizard",
            "MIO",
            crate::chrome::TitleBarControls::FULL,
            |_, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            },
            window,
            cx,
        );

        div()
            .id("startup-wizard")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(theme::ui_font_family())
            .bg(theme::surface())
            .text_color(theme::foreground())
            .text_size(theme::ui_font_size())
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &crate::NewWindow, _, cx| {
                this.open_new_window(cx);
            }))
            .when(!self.file_picker_pending(), |el| {
                el.on_action(cx.listener(
                    |this, _: &crate::OpenProjectInNewWindow, window, cx| {
                        this.choose_open_in_new_window(window, cx);
                    },
                ))
            })
            .on_action(cx.listener(|_, _: &crate::MinimizeWindow, window, _| {
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &crate::ZoomWindow, window, _| {
                window.zoom_window();
            }))
            .on_action(cx.listener(|this, _: &crate::BringAllWindowsToFront, _, cx| {
                cx.activate(true);
                this.status = "已前置全部 MIO 窗口。".into();
                cx.notify();
            }))
            .on_action(|_: &crate::Quit, _, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            })
            .child(title_bar)
            .child(
                div()
                    .px(px(18.))
                    .pt(px(14.))
                    .pb(px(6.))
                    .flex_shrink_0()
                    .bg(theme::surface())
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                Popover::new("wizard-help-popover")
                                    .anchor(gpui_kit::Anchor::BottomLeft)
                                    .trigger(
                                        Button::new("wizard-help")
                                            .ghost()
                                            .small()
                                            .compact()
                                            .rounded(ButtonRounded::None)
                                            .p_0()
                                            .border_0()
                                            .bg(gpui_kit::transparent_black())
                                            .size(px(22.))
                                            .flex_shrink_0()
                                            .accessibility_label("启动向导帮助")
                                            .tooltip("查看启动向导说明")
                                            .child(
                                                Icon::new(AssetIcon::CircleQuestionMark)
                                                    .size(px(20.))
                                                    .text_color(wizard_heading_text()),
                                            ),
                                    )
                                    .content(move |_, _, _| {
                                        div()
                                            .w(theme::scaled_px(320.))
                                            .font_family(theme::ui_font_family())
                                            .text_size(theme::ui_font_size())
                                            .line_height(gpui_kit::relative(1.4))
                                            .text_color(theme::foreground())
                                            .child(help_text)
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme::ui_font_family())
                                    .text_size(theme::ui_font_size())
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(wizard_heading_text())
                                    .child(intro_text),
                            ),
                    )
                    .child(render_brand_lockup()),
            )
            .when_some(error, |this, err| {
                this.child(
                    div()
                        .id("wizard-error-banner")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(err.clone())
                        .mx(px(18.))
                        .mt(px(4.))
                        .px(px(12.))
                        .py(px(7.))
                        .flex_shrink_0()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::danger_border())
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            Icon::new(AssetIcon::ShieldAlert)
                                .size(px(15.))
                                .flex_shrink_0()
                                .text_color(theme::danger_fg()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme::scaled_px(12.5))
                                .line_height(gpui_kit::relative(1.35))
                                .child(err),
                        )
                        .when(can_retry, |banner| {
                            banner.child(
                                wizard_neutral_button(
                                    "wizard-retry",
                                    "重试",
                                    Some("使用当前设置重试启动项目"),
                                    true,
                                    WizardButtonSize::Compact,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.wizard_retry(window, cx);
                                })),
                            )
                        })
                        .child(
                            Button::new("wizard-error-dismiss")
                                .ghost()
                                .small()
                                .compact()
                                .rounded(ButtonRounded::Size(px(3.)))
                                .size(px(22.))
                                .p_0()
                                .flex_shrink_0()
                                .text_color(theme::danger_fg())
                                .accessibility_label("关闭错误提示")
                                .tooltip("关闭错误提示")
                                .child(
                                    Icon::new(AssetIcon::X)
                                        .size(px(12.))
                                        .text_color(theme::danger_fg()),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dismiss_wizard_error(cx);
                                })),
                        ),
                )
            })
            .child(body)
            .child(
                div()
                    .px(px(18.))
                    .pt(px(8.))
                    .pb(px(16.))
                    .flex_shrink_0()
                    .bg(theme::surface())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(new_window_btn)
                    .child(footer_buttons),
            )
            .when(cancel_confirm_open, |this| {
                this.child(
                    div()
                        .id("wizard-cancel-confirm-overlay")
                        .test_support()
                        .role(Role::Dialog)
                        .aria_label("取消向导")
                        .absolute()
                        .inset_0()
                        .bg(gpui_kit::rgba(0x00000044))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(theme::scaled_px(380.).max(px(360.)))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(wizard_border())
                                .bg(theme::surface())
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px(px(20.))
                                        .pt(px(18.))
                                        .pb(px(16.))
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_size(theme::scaled_px(15.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(wizard_heading_text())
                                                .child("取消向导"),
                                        )
                                        .child(
                                            div()
                                                .text_size(theme::ui_font_size())
                                                .text_color(wizard_muted_text())
                                                .child("确定要取消启动向导并退出 MIO 吗？"),
                                        ),
                                )
                                .child(
                                    div()
                                        .px(px(20.))
                                        .py(px(12.))
                                        .border_t_1()
                                        .border_color(wizard_border())
                                        .bg(wizard_subtle_bg())
                                        .flex()
                                        .justify_end()
                                        .gap(px(10.))
                                        .child(
                                            wizard_neutral_button(
                                                "wizard-cancel-no",
                                                "否",
                                                None,
                                                true,
                                                WizardButtonSize::Footer,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.answer_wizard_cancel(false, window, cx);
                                            })),
                                        )
                                        .child(
                                            wizard_primary_button(
                                                "wizard-cancel-yes",
                                                "是",
                                                None,
                                                true,
                                                86.,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.answer_wizard_cancel(true, window, cx);
                                            })),
                                        ),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}
