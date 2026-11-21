use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    checkbox::Checkbox,
    menu::{ContextMenuExt, PopupMenuItem},
    popover::Popover,
};
use gpui_kit::ClipboardItem;
use mio_core::protocol::Permission;

use super::*;

impl Workbench {
    #[inline(never)]
    pub(super) fn render_extensions_detail_pane(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active_tab = self.extensions.detail_tab;
        let output_label: SharedString = if self.extensions.logs.is_empty() {
            "输出".into()
        } else {
            format!("输出 ({})", self.extensions.logs.len()).into()
        };
        let errors_label: SharedString = if self.extensions.error_logs.is_empty() {
            "错误".into()
        } else {
            format!("错误 ({})", self.extensions.error_logs.len()).into()
        };

        let tabs = div()
            .id("extensions-detail-tab-bar")
            .relative()
            .w_full()
            .h(theme::secondary_tab_height())
            .flex_shrink_0()
            .pl(px(12.))
            .bg(theme::toolbar_surface())
            .flex()
            .items_center()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme::border()),
            )
            .child(
                mio_flat_tab(
                    "extensions-detail-details",
                    "详情",
                    None,
                    active_tab == ExtensionsDetailTab::Details,
                    "查看选中扩展的元信息、加载状态、宿主权限与扩展面板",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.detail_tab = ExtensionsDetailTab::Details;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-detail-output",
                    output_label,
                    None,
                    active_tab == ExtensionsDetailTab::Output,
                    "查看扩展运行日志与标准输出记录",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.detail_tab = ExtensionsDetailTab::Output;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-detail-errors",
                    errors_label,
                    None,
                    active_tab == ExtensionsDetailTab::Errors,
                    "查看扩展操作失败与诊断错误记录",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.detail_tab = ExtensionsDetailTab::Errors;
                    cx.notify();
                })),
            );

        let body = match active_tab {
            ExtensionsDetailTab::Details => self.render_extensions_details_body(busy, cx),
            ExtensionsDetailTab::Output => self.render_extensions_output_body(cx),
            ExtensionsDetailTab::Errors => self.render_extensions_errors_body(cx),
        };

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    #[inline(never)]
    fn render_extensions_details_body(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();

        let selected = self
            .extensions
            .selected
            .and_then(|id| self.extensions.instance(id))
            .filter(|inst| inst.info.tab_id.is_none() && inst.context_owner.is_none());
        let selected_id = selected.map(|inst| inst.info.identity.instance_id);
        let selected_live = selected.is_some_and(|inst| inst.live);
        let selected_persisted = selected.is_some_and(|inst| inst.persisted);
        let selected_plugin_id = selected
            .map(|inst| inst.info.manifest.id.clone())
            .unwrap_or_default();
        let extensions_disabled = self.extensions.extensions_disabled;
        let movable = selected.is_some_and(|inst| {
            inst.live && inst.info.tab_id.is_none() && inst.context_owner.is_none()
        });
        let has_ctx_menu = movable
            && selected.is_some_and(|inst| {
                inst.info.grants.contains(&Permission::TrafficRead)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "context_menu")
            });

        let control_row = div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(
                Checkbox::new("extensions-detail-loaded")
                    .label("已加载扩展")
                    .checked(selected_live)
                    .disabled(
                        selected_id.is_none() || busy || (!selected_live && extensions_disabled),
                    )
                    .on_click(cx.listener(move |this, checked, window, cx| {
                        let Some(id) = selected_id else {
                            return;
                        };
                        if *checked {
                            if selected_persisted {
                                this.enable_installed_plugin(id, selected_plugin_id.clone(), cx);
                            } else {
                                this.inspect_plugin(Some(id), window, cx);
                            }
                        } else {
                            if this.extensions.active_top_suite_tab == Some(id) {
                                this.extensions.active_top_suite_tab = None;
                            }
                            this.disable_plugin(id, cx);
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(has_ctx_menu, |bar| {
                        bar.child(
                            button("plugin-context-menu", "执行上下文动作")
                                .tooltip("对当前活动报文触发此插件注册的上下文菜单动作")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(id) = selected_id {
                                        this.invoke_plugin_context_menu(id, window, cx);
                                    }
                                })),
                        )
                    })
                    .when(movable, |bar| {
                        bar.child(
                            button("plugin-reload", "重载插件")
                                .icon(IconName::RefreshCw)
                                .tooltip("使用当前已授予的宿主权限热重载此插件实例")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(id) = selected_id {
                                        if this.extensions.active_top_suite_tab == Some(id) {
                                            this.extensions.active_top_suite_tab = None;
                                        }
                                        this.reload_plugin(id, cx);
                                    }
                                })),
                        )
                    })
                    .child(
                        Popover::new("extensions-detail-help-popover")
                            .anchor(gpui_kit::Anchor::BottomRight)
                            .trigger(
                                Button::new("extensions-detail-help")
                                    .ghost()
                                    .compact()
                                    .p_0()
                                    .size(px(24.))
                                    .tab_stop(false)
                                    .accessibility_label("扩展详情帮助")
                                    .tooltip("查看扩展详情与热重载说明")
                                    .child(
                                        Icon::new(IconName::CircleQuestionMark)
                                            .size(px(20.))
                                            .text_color(theme::foreground()),
                                    ),
                            )
                            .content(|_, _, _| {
                                div()
                                    .id("extensions-detail-help-content")
                                    .test_support()
                                    .w(px(280.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .child("扩展详情 (Details)"),
                                    )
                                    .child("取消勾选「已加载扩展 / Loaded」会安全停用插件进程但保留用户目录下的持久化安装；重新勾选即可按原授权启动。")
                                    .child("点击「移除」会从用户持久化插件目录中彻底卸载并删除扩展包。")
                            }),
                    ),
            );

        let item_header = div()
            .h(px(24.))
            .w_full()
            .flex_shrink_0()
            .bg(header_bg)
            .border_b_1()
            .border_color(table_border)
            .flex()
            .items_center()
            .text_size(theme::ui_font_size())
            .text_color(header_fg)
            .child(
                div()
                    .w(px(240.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Item"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .child("Detail"),
            );

        let mut detail_table = div()
            .id("extensions-detail-table")
            .test_support()
            .w_full()
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(item_header);

        if let Some(instance) = selected {
            let ext_type = if instance.info.manifest.engine == "wasm" {
                "WASM (受限沙箱)"
            } else {
                "Native worker (本地进程)"
            };
            let filename = instance
                .info
                .directory
                .join(&instance.info.manifest.entry)
                .display()
                .to_string();
            let grants = if instance.info.grants.is_empty() {
                "无".to_owned()
            } else {
                instance
                    .info
                    .grants
                    .iter()
                    .map(|grant| permission_short_name(*grant))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let contributions = if instance.info.manifest.contributions.is_empty() {
                "无".to_owned()
            } else {
                instance.info.manifest.contributions.join(", ")
            };
            let entries = [
                (
                    "Name",
                    format!(
                        "{} (v{})",
                        instance.info.manifest.id, instance.info.manifest.version
                    ),
                ),
                ("Extension type", ext_type.to_owned()),
                ("Filename", filename),
                ("Status", instance.status.clone()),
                ("Permissions", grants),
                ("Contributions", contributions),
            ];
            for (index, (key, value)) in entries.into_iter().enumerate() {
                detail_table = detail_table.child(
                    div()
                        .min_h(px(22.))
                        .w_full()
                        .bg(if index % 2 == 1 {
                            zebra_bg
                        } else {
                            theme::surface()
                        })
                        .flex()
                        .items_center()
                        .text_size(theme::ui_font_size())
                        .text_color(theme::foreground())
                        .child(
                            div()
                                .w(px(240.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .py(px(2.))
                                .truncate()
                                .child(key),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.))
                                .py(px(2.))
                                .truncate()
                                .child(value),
                        ),
                );
            }

            let panel_title = instance.panel.read(cx).title().trim().to_owned();
            let panel_header_label = if panel_title.is_empty() {
                format!("{} 扩展面板", instance.info.manifest.id)
            } else {
                format!("{panel_title} — 扩展面板")
            };

            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .px(px(16.))
                .py(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(control_row)
                .child(detail_table.flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .border_1()
                        .border_color(table_border)
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .h(px(24.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .bg(header_bg)
                                .border_b_1()
                                .border_color(table_border)
                                .flex()
                                .items_center()
                                .text_size(px(12.))
                                .text_color(header_fg)
                                .child(panel_header_label),
                        )
                        .child(div().flex_1().min_h_0().child(instance.panel.clone())),
                )
                .into_any_element()
        } else {
            detail_table = detail_table.flex_1().min_h_0().child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(theme::muted_foreground())
                    .child("选择上方列表中的扩展以查看详情。"),
            );

            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .px(px(16.))
                .py(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(control_row)
                .child(detail_table)
                .into_any_element()
        }
    }

    #[inline(never)]
    fn render_extensions_output_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let box_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let has_logs = !self.extensions.logs.is_empty();
        let selected_all = self.extensions.output_selected_all && has_logs;
        let out_owner = cx.weak_entity();
        let mut lines = div()
            .id("plugin-output-log")
            .test_support()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .border_1()
            .border_color(box_border)
            .bg(theme::surface())
            .p(px(10.))
            .font_family(theme::mono_font_family())
            .text_size(theme::editor_font_size())
            .flex()
            .flex_col()
            .gap(px(3.))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.extensions.output_selected_all {
                    this.extensions.output_selected_all = false;
                    cx.notify();
                }
            }))
            .context_menu(move |menu, _, cx| {
                let not_empty = out_owner
                    .upgrade()
                    .is_some_and(|e| !e.read(cx).extensions.logs.is_empty());
                let o_copy = out_owner.clone();
                let o_sel = out_owner.clone();
                let o_clear = out_owner.clone();
                menu.compact()
                    .item(
                        PopupMenuItem::new("复制")
                            .icon(IconName::Copy)
                            .disabled(!not_empty)
                            .on_click(move |_, _, cx| {
                                let _ = o_copy.update(cx, |this, cx| {
                                    let joined: String = this
                                        .extensions
                                        .logs
                                        .iter()
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    cx.write_to_clipboard(ClipboardItem::new_string(joined));
                                    this.status = "已复制扩展输出日志到剪贴板。".into();
                                    cx.notify();
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("全选")
                            .disabled(!not_empty)
                            .on_click(move |_, _, cx| {
                                let _ = o_sel.update(cx, |this, cx| {
                                    this.extensions.output_selected_all = true;
                                    cx.notify();
                                });
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("清空输出")
                            .icon(IconName::Trash)
                            .disabled(!not_empty)
                            .on_click(move |_, _, cx| {
                                let _ = o_clear.update(cx, |this, cx| {
                                    this.extensions.logs.clear();
                                    this.extensions.output_selected_all = false;
                                    cx.notify();
                                });
                            }),
                    )
            });

        if self.extensions.logs.is_empty() {
            lines = lines.child(
                div()
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .text_color(theme::muted_foreground())
                    .child("暂无插件输出记录。"),
            );
        } else {
            for line in &self.extensions.logs {
                lines = lines.child(
                    div()
                        .when(selected_all, |row| row.bg(theme::selected()))
                        .text_color(theme::foreground())
                        .child(line.clone()),
                );
            }
        }

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted_foreground())
                            .child(format!("标准输出与运行日志 ({} 条)", self.extensions.logs.len())),
                    )
                    .child(
                        div().w(px(84.)).child(
                            mio_neutral_button(
                                "plugin-clear-logs",
                                "清空",
                                "清空插件输出日志",
                                has_logs,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.extensions.logs.clear();
                                this.extensions.output_selected_all = false;
                                cx.notify();
                            })),
                        ),
                    ),
            )
            .child(lines)
            .into_any_element()
    }

    #[inline(never)]
    fn render_extensions_errors_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let box_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let has_errors = !self.extensions.error_logs.is_empty();
        let selected_all = self.extensions.errors_selected_all && has_errors;
        let err_owner = cx.weak_entity();
        let mut lines = div()
            .id("plugin-error-log")
            .test_support()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .border_1()
            .border_color(box_border)
            .bg(theme::surface())
            .p(px(10.))
            .font_family(theme::mono_font_family())
            .text_size(theme::editor_font_size())
            .flex()
            .flex_col()
            .gap(px(3.))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.extensions.errors_selected_all {
                    this.extensions.errors_selected_all = false;
                    cx.notify();
                }
            }))
            .context_menu(move |menu, _, cx| {
                let not_empty = err_owner
                    .upgrade()
                    .is_some_and(|e| !e.read(cx).extensions.error_logs.is_empty());
                let o_copy = err_owner.clone();
                let o_sel = err_owner.clone();
                let o_clear = err_owner.clone();
                menu.compact()
                    .item(
                        PopupMenuItem::new("复制")
                            .icon(IconName::Copy)
                            .disabled(!not_empty)
                            .on_click(move |_, _, cx| {
                                let _ = o_copy.update(cx, |this, cx| {
                                    let joined: String = this
                                        .extensions
                                        .error_logs
                                        .iter()
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    cx.write_to_clipboard(ClipboardItem::new_string(joined));
                                    this.status = "已复制扩展错误日志到剪贴板。".into();
                                    cx.notify();
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("全选")
                            .disabled(!not_empty)
                            .on_click(move |_, _, cx| {
                                let _ = o_sel.update(cx, |this, cx| {
                                    this.extensions.errors_selected_all = true;
                                    cx.notify();
                                });
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("清空错误")
                            .icon(IconName::Trash)
                            .disabled(!not_empty)
                            .on_click(move |_, _, cx| {
                                let _ = o_clear.update(cx, |this, cx| {
                                    this.extensions.error_logs.clear();
                                    this.extensions.errors_selected_all = false;
                                    cx.notify();
                                });
                            }),
                    )
            });

        if self.extensions.error_logs.is_empty() {
            lines = lines.child(
                div()
                    .font_family(theme::ui_font_family())
                    .text_size(theme::ui_font_size())
                    .text_color(theme::muted_foreground())
                    .child("暂无插件错误或诊断记录。"),
            );
        } else {
            for line in &self.extensions.error_logs {
                lines = lines.child(
                    div()
                        .when(selected_all, |row| row.bg(theme::selected()))
                        .text_color(theme::danger_fg())
                        .child(line.clone()),
                );
            }
        }

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted_foreground())
                            .child(format!(
                                "错误与诊断记录 ({} 条)",
                                self.extensions.error_logs.len()
                            )),
                    )
                    .child(
                        div().w(px(84.)).child(
                            mio_neutral_button(
                                "plugin-clear-errors",
                                "清空",
                                "清空插件错误与诊断日志",
                                has_errors,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.extensions.error_logs.clear();
                                this.extensions.errors_selected_all = false;
                                cx.notify();
                            })),
                        ),
                    ),
            )
            .child(lines)
            .into_any_element()
    }
}
