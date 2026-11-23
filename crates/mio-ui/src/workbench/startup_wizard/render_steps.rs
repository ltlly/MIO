use gpui_kit::{FontWeight, component::progress::Progress};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn render_wizard_step_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wizard = self.startup_wizard.as_ref().expect("wizard checked");
        let step = wizard.step;
        let mode = wizard.mode;
        let new_name_input = wizard.new_name_input.clone();
        let new_file_input = wizard.new_file_input.clone();
        let open_file_input = wizard.open_file_input.clone();
        let config_file_input = wizard.config_file_input.clone();
        let recent_table = wizard.recent_table.clone();
        let selected_recent_row = recent_table.read(cx).selected_row();
        let missing_recent_count = recent_table.read(cx).delegate().missing_count();
        let recent_save_pending = wizard.pending_recent_save.is_some();
        let config_list_scroll = wizard.config_list_scroll.clone();
        let trust_project_file = wizard.trust_project_file;
        let pause_automated_tasks = wizard.pause_automated_tasks;
        let config_choice = wizard.config_choice;
        let default_in_future = wizard.default_in_future;
        let disable_extensions = wizard.disable_extensions;
        let cancel_confirm_open = wizard.cancel_confirm_open;
        let recent_scroll_handle = recent_table.read(cx).vertical_scroll_handle.clone();
        let table_header_h = theme::scaled_px(25.).max(px(22.));

        let controls_enabled = self.backend_available()
            && !self.file_picker_pending()
            && step != WizardStep::Starting
            && !cancel_confirm_open;
        let new_enabled = controls_enabled && mode == ProjectMode::NewOnDisk;
        let open_enabled = controls_enabled && mode == ProjectMode::OpenExisting;
        let cfg_file_enabled = controls_enabled && config_choice == ConfigChoice::ConfigFile;
        let project_cfg_enabled = controls_enabled && mode == ProjectMode::OpenExisting;

        let saved_config_path: Option<String> = if step == WizardStep::Step2 {
            self.user_preferences
                .confirmed
                .as_ref()
                .and_then(|prefs| match prefs.startup.default_config.as_ref() {
                    Some(StartupDefaultConfig::ConfigFile { path }) => {
                        checked_utf8_path(path, "启动配置文件").ok()
                    }
                    _ => None,
                })
        } else {
            None
        };
        let current_config_path: String = if step == WizardStep::Step2 {
            config_file_input.read(cx).value().trim().to_owned()
        } else {
            String::new()
        };

        let project_summary: String = if step == WizardStep::Starting {
            match mode {
                ProjectMode::Temporary => "内存中临时项目".into(),
                ProjectMode::NewOnDisk => {
                    let name_val = new_name_input.read(cx).value();
                    let file_val = new_file_input.read(cx).value();
                    let name = name_val.trim();
                    let file = file_val.trim();
                    match (name.is_empty(), file.is_empty()) {
                        (false, false) => format!("磁盘新建项目 · {name} ({file})"),
                        (false, true) => format!("磁盘新建项目 · {name}"),
                        (true, false) => format!("磁盘新建项目 · {file}"),
                        (true, true) => "磁盘上新建项目".into(),
                    }
                }
                ProjectMode::OpenExisting => {
                    let file_val = open_file_input.read(cx).value();
                    let file = file_val.trim();
                    if file.is_empty() {
                        "打开已有项目".into()
                    } else {
                        format!("打开已有项目 · {file}")
                    }
                }
            }
        } else {
            String::new()
        };

        let config_summary: String = if step == WizardStep::Starting {
            let base = match config_choice {
                ConfigChoice::Defaults => "MIO 默认配置".into(),
                ConfigChoice::Project => "随项目保存的设置".into(),
                ConfigChoice::ConfigFile => {
                    let file_val = config_file_input.read(cx).value();
                    let file = file_val.trim();
                    if file.is_empty() {
                        "外部配置文件".into()
                    } else {
                        format!("配置文件 · {file}")
                    }
                }
            };
            if disable_extensions {
                format!("{base}（已禁用扩展）")
            } else {
                base
            }
        } else {
            String::new()
        };

        let left_col_w = theme::scaled_px(244.).max(px(236.));
        let field_indent = theme::scaled_px(52.);

        match step {
            WizardStep::Step1 => div()
                .id("wizard-step-1-body")
                .flex_1()
                .min_h_0()
                .px(px(18.))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .child(
                    div().pt(px(12.)).pb(px(16.)).flex_shrink_0().child({
                        let wb = cx.entity();
                        wizard_radio(
                            "wizard-mode-temp",
                            "内存中临时项目",
                            mode == ProjectMode::Temporary,
                            controls_enabled,
                            window,
                            cx,
                        )
                        .on_change(move |checked, _, window, cx| {
                            if checked && controls_enabled {
                                wb.update(cx, |this, cx| {
                                    this.select_wizard_project_mode(
                                        ProjectMode::Temporary,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        })
                    }),
                )
                .child(wizard_divider_line())
                .child(
                    div()
                        .py(px(18.))
                        .flex_shrink_0()
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(
                            div().w(left_col_w).flex_shrink_0().child({
                                let wb = cx.entity();
                                wizard_radio(
                                    "wizard-mode-new",
                                    "磁盘上新建项目",
                                    mode == ProjectMode::NewOnDisk,
                                    controls_enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |checked, _, window, cx| {
                                    if checked && controls_enabled {
                                        wb.update(cx, |this, cx| {
                                            this.select_wizard_project_mode(
                                                ProjectMode::NewOnDisk,
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                })
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(wizard_field_label("名称：", controls_enabled))
                                        .child(div().flex_1().min_w_0().child(wizard_input(
                                            &new_name_input,
                                            controls_enabled,
                                            new_enabled,
                                        ))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(wizard_field_label("文件：", controls_enabled))
                                        .child(div().flex_1().min_w_0().child(wizard_input(
                                            &new_file_input,
                                            controls_enabled,
                                            new_enabled,
                                        )))
                                        .child(
                                            wizard_neutral_button(
                                                "wizard-choose-new-file",
                                                "选择文件…",
                                                None,
                                                controls_enabled,
                                                WizardButtonSize::Browse,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.choose_wizard_new_file(window, cx);
                                            })),
                                        ),
                                ),
                        ),
                )
                .child(wizard_divider_line())
                .child(
                    div()
                        .pt(px(18.))
                        .pb(px(4.))
                        .flex_1()
                        .min_h(px(230.))
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(
                            div().w(left_col_w).flex_shrink_0().child({
                                let wb = cx.entity();
                                wizard_radio(
                                    "wizard-mode-open",
                                    "打开已有项目",
                                    mode == ProjectMode::OpenExisting,
                                    controls_enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |checked, _, window, cx| {
                                    if checked && controls_enabled {
                                        wb.update(cx, |this, cx| {
                                            this.select_wizard_project_mode(
                                                ProjectMode::OpenExisting,
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                })
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .pl(field_indent)
                                        .flex_1()
                                        .min_h(px(150.))
                                        .w_full()
                                        .child(
                                            div()
                                                .relative()
                                                .size_full()
                                                .border_1()
                                                .border_color(wizard_border())
                                                .bg(theme::surface())
                                                .overflow_hidden()
                                                .child(
                                                    DataTable::new(&recent_table)
                                                        .accessibility_label("最近项目列表")
                                                        .with_size(
                                                            theme::scaled_px(24.).max(px(22.)),
                                                        )
                                                        .header_row_height(table_header_h)
                                                        .stripe(false)
                                                        .bordered(false)
                                                        .scrollbar_visible(false, false),
                                                )
                                                .child(wizard_vertical_scrollbar(
                                                    "wizard-recent-scrollbar",
                                                    &recent_scroll_handle,
                                                    table_header_h,
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .pl(field_indent)
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            wizard_neutral_button(
                                                "wizard-remove-recent",
                                                "移除选中",
                                                Some("将当前选中的最近项目记录从历史列表中移除（不会删除磁盘上的项目文件）"),
                                                controls_enabled
                                                    && !recent_save_pending
                                                    && selected_recent_row.is_some(),
                                                WizardButtonSize::Compact,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.remove_selected_wizard_recent(window, cx);
                                            })),
                                        )
                                        .child(
                                            wizard_neutral_button(
                                                "wizard-prune-recent",
                                                if missing_recent_count > 0 {
                                                    format!("清理失效项目 ({missing_recent_count})")
                                                } else {
                                                    "清理失效项目".to_owned()
                                                },
                                                Some("批量清理磁盘上已不存在的最近项目历史记录"),
                                                controls_enabled
                                                    && !recent_save_pending
                                                    && missing_recent_count > 0,
                                                WizardButtonSize::Compact,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.prune_missing_wizard_recent(window, cx);
                                            })),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(wizard_field_label("文件：", controls_enabled))
                                        .child(div().flex_1().min_w_0().child(wizard_input(
                                            &open_file_input,
                                            controls_enabled,
                                            open_enabled,
                                        )))
                                        .child(
                                            wizard_neutral_button(
                                                "wizard-choose-open-file",
                                                "选择文件…",
                                                None,
                                                controls_enabled,
                                                WizardButtonSize::Browse,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.choose_wizard_open_file(window, cx);
                                            })),
                                        ),
                                )
                                .child(
                                    div()
                                        .pl(field_indent)
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.))
                                        .child({
                                            let wb = cx.entity();
                                            wizard_checkbox(
                                                "wizard-trust-project",
                                                "信任此项目文件",
                                                trust_project_file,
                                                open_enabled,
                                                window,
                                                cx,
                                            )
                                            .on_change(move |state, _, _, cx| {
                                                if open_enabled {
                                                    let checked =
                                                        state == gpui_kit::base::CheckboxState::Checked;
                                                    wb.update(cx, |this, cx| {
                                                        if let Some(wizard) =
                                                            &mut this.startup_wizard
                                                            && wizard.mode == ProjectMode::OpenExisting
                                                        {
                                                            wizard.trust_project_file = checked;
                                                            cx.notify();
                                                        }
                                                    });
                                                }
                                            })
                                        })
                                        .child({
                                            let wb = cx.entity();
                                            wizard_checkbox(
                                                "wizard-pause-tasks",
                                                "暂停自动任务",
                                                pause_automated_tasks,
                                                open_enabled,
                                                window,
                                                cx,
                                            )
                                            .on_change(move |state, _, _, cx| {
                                                if open_enabled {
                                                    let checked =
                                                        state == gpui_kit::base::CheckboxState::Checked;
                                                    wb.update(cx, |this, cx| {
                                                        if let Some(wizard) =
                                                            &mut this.startup_wizard
                                                            && wizard.mode == ProjectMode::OpenExisting
                                                        {
                                                            wizard.pause_automated_tasks = checked;
                                                            cx.notify();
                                                        }
                                                    });
                                                }
                                            })
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
            WizardStep::Step2 => div()
                .id("wizard-step-2-body")
                .flex_1()
                .min_h_0()
                .px(px(18.))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .child(
                    div().pt(px(12.)).pb(px(16.)).flex_shrink_0().child({
                        let wb = cx.entity();
                        wizard_radio(
                            "wizard-cfg-defaults",
                            "使用 MIO 默认配置",
                            config_choice == ConfigChoice::Defaults,
                            controls_enabled,
                            window,
                            cx,
                        )
                        .on_change(move |checked, _, _, cx| {
                            if checked && controls_enabled {
                                wb.update(cx, |this, cx| {
                                    if let Some(wizard) = &mut this.startup_wizard {
                                        wizard.config_choice = ConfigChoice::Defaults;
                                        wizard.config_choice_customized = true;
                                        wizard.last_start_failed = false;
                                        wizard.error = None;
                                        cx.notify();
                                    }
                                });
                            }
                        })
                    }),
                )
                .child(wizard_divider_line())
                .child(
                    div().py(px(16.)).flex_shrink_0().child({
                        let wb = cx.entity();
                        wizard_radio(
                            "wizard-cfg-project",
                            "使用随项目保存的设置",
                            config_choice == ConfigChoice::Project,
                            project_cfg_enabled,
                            window,
                            cx,
                        )
                        .on_change(move |checked, _, _, cx| {
                            if checked && project_cfg_enabled {
                                wb.update(cx, |this, cx| {
                                    if let Some(wizard) = &mut this.startup_wizard
                                        && wizard.mode == ProjectMode::OpenExisting
                                    {
                                        wizard.config_choice = ConfigChoice::Project;
                                        wizard.config_choice_customized = true;
                                        wizard.last_start_failed = false;
                                        wizard.error = None;
                                        cx.notify();
                                    }
                                });
                            }
                        })
                    }),
                )
                .child(wizard_divider_line())
                .child(
                    div()
                        .pt(px(18.))
                        .pb(px(10.))
                        .flex_1()
                        .min_h(px(210.))
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(
                            div().w(left_col_w).flex_shrink_0().child({
                                let wb = cx.entity();
                                wizard_radio(
                                    "wizard-cfg-file",
                                    "从配置文件加载",
                                    config_choice == ConfigChoice::ConfigFile,
                                    controls_enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |checked, _, _, cx| {
                                    if checked && controls_enabled {
                                        wb.update(cx, |this, cx| {
                                            if let Some(wizard) = &mut this.startup_wizard {
                                                wizard.config_choice = ConfigChoice::ConfigFile;
                                                wizard.config_choice_customized = true;
                                                wizard.last_start_failed = false;
                                                wizard.error = None;
                                                cx.notify();
                                            }
                                        });
                                    }
                                })
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .pl(field_indent)
                                        .flex_1()
                                        .min_h(px(150.))
                                        .w_full()
                                        .child(
                                            div()
                                                .id("wizard-config-file-list")
                                                .relative()
                                                .size_full()
                                                .border_1()
                                                .border_color(wizard_border())
                                                .bg(theme::surface())
                                                .overflow_hidden()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .h(table_header_h)
                                                        .px(px(8.))
                                                        .flex_shrink_0()
                                                        .bg(wizard_table_header_bg())
                                                        .border_b_1()
                                                        .border_color(wizard_border())
                                                        .flex()
                                                        .items_center()
                                                        .font_family(theme::ui_font_family())
                                                        .text_size(theme::ui_font_size())
                                                        .font_weight(FontWeight::NORMAL)
                                                        .text_color(wizard_heading_text())
                                                        .child("文件"),
                                                )
                                                .child(
                                                    div()
                                                        .id("wizard-saved-config-list")
                                                        .flex_1()
                                                        .min_h_0()
                                                        .overflow_y_scroll()
                                                        .track_scroll(&config_list_scroll)
                                                        .bg(theme::surface())
                                                        .when_some(saved_config_path, |list, path| {
                                                            let selected =
                                                                current_config_path == path;
                                                            let click_path = path.clone();
                                                            list.child(
                                                                div()
                                                                    .id("wizard-saved-config-row")
                                                                    .h(theme::scaled_px(24.)
                                                                        .max(px(22.)))
                                                                    .px(px(8.))
                                                                    .flex()
                                                                    .items_center()
                                                                    .truncate()
                                                                    .font_family(
                                                                        theme::ui_font_family(),
                                                                    )
                                                                    .text_size(
                                                                        theme::ui_font_size(),
                                                                    )
                                                                    .text_color(
                                                                        theme::foreground(),
                                                                    )
                                                                    .when(
                                                                        selected
                                                                            && cfg_file_enabled,
                                                                        |row| {
                                                                            row.bg(theme::selected())
                                                                        },
                                                                    )
                                                                    .when(controls_enabled, |row| {
                                                                        row.on_click(cx.listener(
                                                                            move |this, _, window, cx| {
                                                                                if let Some(wizard) =
                                                                                    &mut this
                                                                                        .startup_wizard
                                                                                {
                                                                                    wizard.config_choice =
                                                                                        ConfigChoice::ConfigFile;
                                                                                    wizard.config_choice_customized =
                                                                                        true;
                                                                                    wizard.last_start_failed =
                                                                                        false;
                                                                                    let input = wizard
                                                                                        .config_file_input
                                                                                        .clone();
                                                                                    input.update(
                                                                                        cx,
                                                                                        |state, cx| {
                                                                                            state.set_value(
                                                                                                click_path.clone(),
                                                                                                window,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    );
                                                                                    wizard.error = None;
                                                                                    cx.notify();
                                                                                }
                                                                            },
                                                                        ))
                                                                    })
                                                                    .child(path),
                                                            )
                                                        }),
                                                )
                                                .child(wizard_vertical_scrollbar(
                                                    "wizard-config-scrollbar",
                                                    &config_list_scroll,
                                                    table_header_h,
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(wizard_field_label("文件：", controls_enabled))
                                        .child(div().flex_1().min_w_0().child(wizard_input(
                                            &config_file_input,
                                            controls_enabled,
                                            cfg_file_enabled,
                                        )))
                                        .child(
                                            wizard_neutral_button(
                                                "wizard-choose-config-file",
                                                "选择文件…",
                                                None,
                                                controls_enabled,
                                                WizardButtonSize::Browse,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.choose_wizard_config_file(window, cx);
                                            })),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .pt(px(6.))
                        .pb(px(4.))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child({
                            let wb = cx.entity();
                            wizard_checkbox(
                                "wizard-default-future",
                                "今后默认使用以上配置",
                                default_in_future,
                                controls_enabled,
                                window,
                                cx,
                            )
                            .on_change(move |state, _, _, cx| {
                                if controls_enabled {
                                    let checked = state == gpui_kit::base::CheckboxState::Checked;
                                    wb.update(cx, |this, cx| {
                                        if let Some(wizard) = &mut this.startup_wizard {
                                            wizard.default_in_future = checked;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                        })
                        .child({
                            let wb = cx.entity();
                            wizard_checkbox(
                                "wizard-disable-extensions",
                                "禁用扩展",
                                disable_extensions,
                                controls_enabled,
                                window,
                                cx,
                            )
                            .on_change(move |state, _, _, cx| {
                                if controls_enabled {
                                    let checked = state == gpui_kit::base::CheckboxState::Checked;
                                    wb.update(cx, |this, cx| {
                                        if let Some(wizard) = &mut this.startup_wizard {
                                            wizard.disable_extensions = checked;
                                            wizard.disable_extensions_customized = true;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                        }),
                )
                .into_any_element(),
            WizardStep::Starting => div()
                .id("wizard-starting-body")
                .flex_1()
                .min_h_0()
                .px(px(18.))
                .py(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(theme::scaled_px(440.).max(px(380.)))
                        .p(px(24.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(wizard_border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(
                                    gpui_kit::img(crate::chrome::logo_image())
                                        .w(px(28.))
                                        .h(px(28.))
                                        .flex_shrink_0()
                                        .rounded(px(4.)),
                                )
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(15.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(wizard_heading_text())
                                        .child("正在启动项目，请稍候…"),
                                ),
                        )
                        .child(
                            div()
                                .px(px(12.))
                                .py(px(10.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(wizard_border())
                                .bg(wizard_subtle_bg())
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(theme::scaled_px(12.5))
                                        .text_color(theme::foreground())
                                        .child(format!("项目：{project_summary}")),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(theme::scaled_px(12.5))
                                        .text_color(wizard_muted_text())
                                        .child(format!("配置：{config_summary}")),
                                ),
                        )
                        .child(
                            Progress::new("wizard-starting-progress")
                                .loading(!cfg!(test))
                                .value(48.)
                                .color(theme::accent())
                                .h(px(6.))
                                .accessibility_label("项目启动进度"),
                        )
                        .child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("正在初始化工作区与代理组件，完成后将自动进入主界面。"),
                        ),
                )
                .into_any_element(),
        }
    }
}
