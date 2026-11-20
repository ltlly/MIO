use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    button::ButtonVariants,
    checkbox::Checkbox,
    menu::{ContextMenuExt, PopupMenuItem},
    popover::Popover,
};
use gpui_kit::{ClipboardItem, MouseButton};
use mio_core::protocol::{
    MAX_FRAME_BYTES, MAX_INLINE_BODY, MAX_STORAGE_KEY_BYTES, MAX_STORAGE_VALUE_BYTES,
    PROTOCOL_MAJOR, PROTOCOL_MINOR, Permission,
};

use super::*;

const GAUGE_ICON_SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.4\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 11.5a5.5 5.5 0 1 1 11 0\"/><path d=\"M8 10.5L10.8 6.8\"/><circle cx=\"8\" cy=\"10.5\" r=\"1.1\" fill=\"currentColor\"/></svg>";

impl Workbench {
    pub(in crate::workbench) fn render_extensions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_active_top_suite_tab();
        if self.extensions.search_subscription.is_none() {
            let search = self.extensions.search.clone();
            self.extensions.search_subscription = Some(cx.subscribe(
                &search,
                |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                },
            ));
        }
        if let Some(top_id) = self.extensions.active_top_suite_tab
            && let Some(instance) = self.extensions.instance(top_id)
        {
            return div()
                .id("plugin-top-suite-view")
                .test_support()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .min_w_0()
                .child(instance.panel.clone())
                .into_any_element();
        }

        let inspecting = self.extensions.operations.values().any(|operation| {
            matches!(
                operation,
                Operation::Inspect { .. } | Operation::Load { .. } | Operation::Reload { .. }
            )
        });
        let moving = self
            .extensions
            .operations
            .values()
            .any(|operation| matches!(operation, Operation::Reorder));
        let busy = inspecting
            || moving
            || !self.backend_available()
            || self.transition.is_some()
            || self.file_picker_pending();

        let body = match self.extensions.page {
            ExtensionsPage::Installed => {
                self.render_extensions_installed(busy, inspecting, cx)
            }
            ExtensionsPage::PluginStore => self.render_extensions_plugin_store(busy, cx),
            ExtensionsPage::Apis => self.render_extensions_apis(busy, cx),
            ExtensionsPage::CustomScanChecks => {
                self.render_extensions_custom_scan_checks(busy, cx)
            }
            ExtensionsPage::ScriptPresetLibrary => self.render_extensions_script_library(busy, cx),
            ExtensionsPage::Settings => self.render_extensions_settings(busy, cx),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(theme::surface())
            .child(self.render_extensions_sub_tabs(cx))
            .child(div().flex_1().min_h_0().min_w_0().flex().flex_col().child(body))
            .into_any_element()
    }

    fn render_extensions_sub_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = self.extensions.page;
        div()
            .id("extensions-sub-tab-bar")
            .relative()
            .w_full()
            .h(theme::secondary_tab_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(theme::toolbar_surface())
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
                    "extensions-tab-installed",
                    "已安装",
                    None,
                    page == ExtensionsPage::Installed,
                    "管理当前项目中已安装的 WASM 与本地 Native 扩展",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::Installed;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-plugin-store",
                    "Plugin Store",
                    None,
                    page == ExtensionsPage::PluginStore,
                    "查看离线扩展包说明与本地安装入口",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::PluginStore;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-apis",
                    "APIs",
                    None,
                    page == ExtensionsPage::Apis,
                    "查看 MIO 宿主插件协议 API 与宿主权限参考",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::Apis;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-custom-scan-checks",
                    "自定义扫描检查",
                    None,
                    page == ExtensionsPage::CustomScanChecks,
                    "查看与管理自定义被动/主动安全扫描检查规则与扩展入口",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::CustomScanChecks;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-script-library",
                    "脚本库",
                    None,
                    page == ExtensionsPage::ScriptPresetLibrary,
                    "查看在 Proxy 历史筛选与 Match and replace 中复用的 脚本库",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::ScriptPresetLibrary;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .px(px(6.))
                    .h(theme::secondary_tab_height() - px(1.))
                    .self_start()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(div().w(px(1.)).h(px(22.)).bg(theme::border())),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-settings",
                    "扩展设置",
                    Some(IconName::Settings),
                    page == ExtensionsPage::Settings,
                    "配置手动加载路径、查看安全沙箱策略并打开全局设置",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::Settings;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn render_extensions_installed(
        &self,
        busy: bool,
        inspecting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(theme::surface())
            .child(self.render_extensions_impact_strip())
            .child(self.render_extensions_table_section(busy, inspecting, cx));

        if let Some(panel) = self.render_extensions_add_or_inspection(busy, cx) {
            root = root.child(panel);
        }

        root.child(self.render_extensions_detail_pane(busy, cx))
            .into_any_element()
    }

    fn render_extensions_impact_strip(&self) -> AnyElement {
        let dark = theme::is_dark();
        let live_count = self.extensions.instances.iter().filter(|i| i.live).count();
        let (impact_label, fill_fraction, fill_color): (SharedString, f32, gpui_kit::Hsla) =
            match live_count {
                0 => ("无".into(), 0.0, rgb(0x2e8b57).into()),
                1..=2 => (
                    format!("低 ({live_count} 个运行中)").into(),
                    0.22,
                    rgb(0x2e8b57).into(),
                ),
                3..=5 => (
                    format!("中 ({live_count} 个运行中)").into(),
                    0.55,
                    rgb(0xd97706).into(),
                ),
                _ => (
                    format!("高 ({live_count} 个运行中)").into(),
                    0.85,
                    rgb(0xd92d20).into(),
                ),
            };
        let track_width = 380.0_f32;
        let fill_width = (track_width * fill_fraction).round();

        div()
            .id("extensions-impact-strip")
            .w_full()
            .h(px(36.))
            .flex_shrink_0()
            .px(px(16.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Icon::default()
                    .data(GAUGE_ICON_SVG)
                    .size(px(15.))
                    .text_color(theme::foreground()),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("预计系统总影响:"),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(impact_label),
            )
            .child(
                div()
                    .ml(px(4.))
                    .w(px(track_width))
                    .h(px(8.))
                    .rounded(px(4.))
                    .bg(rgb(if dark { 0x35383d } else { 0xeeeeee }))
                    .overflow_hidden()
                    .when(fill_width > 0.0, |track| {
                        track.child(
                            div()
                                .h_full()
                                .w(px(fill_width))
                                .rounded(px(4.))
                                .bg(fill_color),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_extensions_table_section(
        &self,
        busy: bool,
        inspecting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();
        let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        let sel_border: gpui_kit::Hsla = rgb(if dark { 0x5b8ad6 } else { 0x4876b5 }).into();

        let search_raw = self.extensions.search.read(cx).value().to_string();
        let search_query = search_raw.trim().to_lowercase();
        let search_non_empty = !search_query.is_empty();

        let selected_instance = self
            .extensions
            .selected
            .and_then(|id| self.extensions.instance(id));
        let selected_id = selected_instance.map(|inst| inst.info.identity.instance_id);
        let selected_live = selected_instance.is_some_and(|inst| inst.live);
        let selected_movable = selected_instance.is_some_and(|inst| {
            inst.live && inst.info.tab_id.is_none() && inst.context_owner.is_none()
        });

        let remove_id = match (selected_id, selected_live) {
            (Some(_), true) => "plugin-disable",
            (Some(_), false) => "plugin-remove-retained",
            (None, _) => "plugin-remove",
        };
        let remove_tip = if selected_live {
            "停用选中扩展并将未保存的编辑保留为只读"
        } else {
            "移除已停止的插件实例及其保留的只读内容"
        };

        let left_buttons = div()
            .w(px(90.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                mio_neutral_button(
                    "plugin-choose",
                    "添加",
                    "选择包含 manifest.json 的本地插件包目录并检查权限",
                    !busy,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.extensions.add_panel_open = true;
                    this.choose_plugin(window, cx);
                })),
            )
            .child(
                mio_neutral_button(
                    remove_id,
                    "移除",
                    remove_tip,
                    selected_id.is_some() && !busy,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let Some(id) = selected_id else {
                        return;
                    };
                    if this.extensions.active_top_suite_tab == Some(id) {
                        this.extensions.active_top_suite_tab = None;
                    }
                    if selected_live {
                        this.disable_plugin(id, cx);
                    } else {
                        this.remove_retained_plugin(id, window, cx);
                    }
                })),
            )
            .child(
                mio_neutral_button(
                    "plugin-move-up",
                    "上移",
                    "上调此插件在钩子链中的执行优先级",
                    !busy && selected_movable,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = selected_id {
                        this.reorder_plugin(id, true, cx);
                    }
                })),
            )
            .child(
                mio_neutral_button(
                    "plugin-move-down",
                    "下移",
                    "下调此插件在钩子链中的执行优先级",
                    !busy && selected_movable,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = selected_id {
                        this.reorder_plugin(id, false, cx);
                    }
                })),
            );

        let table_header = div()
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
                    .w(px(68.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Loaded"),
            )
            .child(
                div()
                    .w(px(82.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Type"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(140.))
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Name"),
            )
            .child(
                div()
                    .w(px(86.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("AI enabled"),
            )
            .child(
                div()
                    .w(px(112.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("AI credits used"),
            )
            .child(
                div()
                    .w(px(164.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("System impact"),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex_shrink_0()
                    .h_full()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .child("Auto-reload"),
            );

        let mut rows = div()
            .id("plugin-instance-rows")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        let filtered: Vec<&Instance> = self
            .extensions
            .instances
            .iter()
            .filter(|instance| {
                if search_query.is_empty() {
                    return true;
                }
                let id_str = instance.info.manifest.id.to_lowercase();
                let engine_str = instance.info.manifest.engine.to_lowercase();
                let dir_str = instance.info.directory.display().to_string().to_lowercase();
                let title_str = instance.panel.read(&*cx).title().to_lowercase();
                id_str.contains(&search_query)
                    || engine_str.contains(&search_query)
                    || dir_str.contains(&search_query)
                    || title_str.contains(&search_query)
            })
            .collect();

        if filtered.is_empty() {
            let empty_hint = if inspecting {
                "正在检查插件包；尚未执行。"
            } else if self.extensions.instances.is_empty() {
                "尚未加载扩展。点击左侧「添加」选择包含 manifest.json 的本地插件包。"
            } else {
                "没有匹配当前搜索关键词的已安装扩展。"
            };
            rows = rows.child(
                div()
                    .flex_1()
                    .min_h(px(90.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(12.))
                    .text_size(px(12.))
                    .text_color(theme::muted_foreground())
                    .child(empty_hint),
            );
        } else {
            for (index, instance) in filtered.into_iter().enumerate() {
                let id = instance.info.identity.instance_id;
                let is_selected = self.extensions.selected == Some(id);
                let live = instance.live;
                let is_movable = live
                    && instance.info.tab_id.is_none()
                    && instance.context_owner.is_none();
                let auto_reload = self.extensions.auto_reload.contains(&id);
                let engine_label = if instance.info.manifest.engine == "wasm" {
                    "WASM"
                } else {
                    "Native"
                };
                let context_suffix = match instance.context_owner {
                    Some(owner) => format!(" · {owner}"),
                    None => instance.info.tab_id.map_or_else(String::new, |tab| {
                        let title = self
                            .workspace
                            .tabs
                            .iter()
                            .chain(&self.workspace.closed)
                            .find(|candidate| candidate.id == tab)
                            .map_or("已关闭", |tab| tab.title.as_str());
                        format!(" · Repeater {title}")
                    }),
                };
                let name_label = format!(
                    "{}{context_suffix}{}",
                    instance.info.manifest.id,
                    if live { "" } else { "（已停止）" }
                );
                let impact_text = if !live {
                    "无"
                } else if instance.info.grants.contains(&Permission::TrafficModify) {
                    "中"
                } else {
                    "低"
                };

                let row_bg = if is_selected {
                    theme::selected()
                } else if index % 2 == 1 {
                    zebra_bg
                } else {
                    theme::surface()
                };
                let row_hover = if is_selected {
                    theme::selected()
                } else {
                    hover_bg
                };

                let row_menu_owner = cx.weak_entity();
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("plugin-instance-{id}")))
                        .test_support()
                        .h(px(22.))
                        .w_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .bg(row_bg)
                        .when(is_selected, |row| {
                            row.border_t_1().border_b_1().border_color(sel_border)
                        })
                        .hover(move |s| s.bg(row_hover))
                        .cursor_pointer()
                        .text_size(theme::ui_font_size())
                        .text_color(if is_selected && !dark {
                            rgb(0x000000).into()
                        } else if live {
                            theme::foreground()
                        } else {
                            theme::muted_foreground()
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                this.extensions.selected = Some(id);
                                cx.notify();
                            }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cancel_plugin_interactions(cx);
                            this.extensions.selected = Some(id);
                            cx.notify();
                        }))
                        .context_menu(move |menu, _, _| {
                            let o_reload = row_menu_owner.clone();
                            let o_toggle = row_menu_owner.clone();
                            let o_ar = row_menu_owner.clone();
                            let o_up = row_menu_owner.clone();
                            let o_down = row_menu_owner.clone();
                            let o_remove = row_menu_owner.clone();
                            menu.compact()
                                .item(
                                    PopupMenuItem::new("重新加载 (Reload)")
                                        .icon(IconName::RefreshCw)
                                        .disabled(busy || !is_movable)
                                        .on_click(move |_, _, cx| {
                                            let _ = o_reload.update(cx, |this, cx| {
                                                if this.extensions.active_top_suite_tab == Some(id) {
                                                    this.extensions.active_top_suite_tab = None;
                                                }
                                                this.reload_plugin(id, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new(if live { "禁用" } else { "启用" })
                                        .disabled(busy)
                                        .on_click(move |_, window, cx| {
                                            let _ = o_toggle.update(cx, |this, cx| {
                                                this.extensions.selected = Some(id);
                                                if live {
                                                    if this.extensions.active_top_suite_tab
                                                        == Some(id)
                                                    {
                                                        this.extensions.active_top_suite_tab = None;
                                                    }
                                                    this.disable_plugin(id, cx);
                                                } else {
                                                    this.inspect_plugin(Some(id), window, cx);
                                                }
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("切换自动重载 (Auto-reload)")
                                        .checked(auto_reload)
                                        .disabled(busy || !is_movable)
                                        .on_click(move |_, _, cx| {
                                            let _ = o_ar.update(cx, |this, cx| {
                                                this.set_plugin_auto_reload(id, !auto_reload, cx);
                                            });
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("上移")
                                        .disabled(busy || !is_movable)
                                        .on_click(move |_, _, cx| {
                                            let _ = o_up.update(cx, |this, cx| {
                                                this.reorder_plugin(id, true, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("下移")
                                        .disabled(busy || !is_movable)
                                        .on_click(move |_, _, cx| {
                                            let _ = o_down.update(cx, |this, cx| {
                                                this.reorder_plugin(id, false, cx);
                                            });
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new(if live {
                                        "移除 / 卸载"
                                    } else {
                                        "移除"
                                    })
                                    .icon(IconName::Trash)
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        let _ = o_remove.update(cx, |this, cx| {
                                            if this.extensions.active_top_suite_tab == Some(id) {
                                                this.extensions.active_top_suite_tab = None;
                                            }
                                            if live {
                                                this.disable_plugin(id, cx);
                                            } else {
                                                this.remove_retained_plugin(id, window, cx);
                                            }
                                        });
                                    }),
                                )
                        })
                        .child(
                            div()
                                .w(px(68.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Checkbox::new(SharedString::from(format!(
                                        "plugin-loaded-{id}"
                                    )))
                                    .small()
                                    .checked(live)
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, checked, window, cx| {
                                        cx.stop_propagation();
                                        this.extensions.selected = Some(id);
                                        if *checked {
                                            this.inspect_plugin(Some(id), window, cx);
                                        } else {
                                            if this.extensions.active_top_suite_tab == Some(id) {
                                                this.extensions.active_top_suite_tab = None;
                                            }
                                            this.disable_plugin(id, cx);
                                        }
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .w(px(82.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .truncate()
                                .child(engine_label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(140.))
                                .px(px(8.))
                                .truncate()
                                .child(name_label),
                        )
                        .child(div().w(px(86.)).flex_shrink_0().px(px(8.)))
                        .child(div().w(px(112.)).flex_shrink_0().px(px(8.)))
                        .child(
                            div()
                                .w(px(164.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .truncate()
                                .child(impact_text),
                        )
                        .child(
                            div()
                                .w(px(96.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_movable, |cell| {
                                    cell.child(
                                        Checkbox::new(SharedString::from(format!(
                                            "plugin-autoreload-{id}"
                                        )))
                                        .small()
                                        .checked(auto_reload)
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, checked, _, cx| {
                                            cx.stop_propagation();
                                            this.set_plugin_auto_reload(id, *checked, cx);
                                        })),
                                    )
                                }),
                        ),
                );
            }
        }

        let table = div()
            .id("plugin-instance-list")
            .test_support()
            .flex_1()
            .min_w_0()
            .h(px(125.))
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(table_header)
            .child(rows);

        div()
            .w_full()
            .flex_shrink_0()
            .px(px(16.))
            .py(px(12.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("已安装扩展 (Extensions)"),
                    )
                    .child(
                        Popover::new("extensions-section-help-popover")
                            .anchor(gpui_kit::Anchor::BottomRight)
                            .trigger(
                                Button::new("extensions-help")
                                    .ghost()
                                    .compact()
                                    .p_0()
                                    .size(px(24.))
                                    .tab_stop(false)
                                    .accessibility_label("扩展帮助")
                                    .tooltip("查看 MIO 扩展加载与安全授权说明")
                                    .child(
                                        Icon::new(IconName::CircleQuestionMark)
                                            .size(px(20.))
                                            .text_color(theme::foreground()),
                                    ),
                            )
                            .content(|_, _, _| {
                                div()
                                    .id("extensions-help-content")
                                    .test_support()
                                    .w(px(300.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .child("已安装扩展 (Extensions)"),
                                    )
                                    .child("点击「添加」选择包含 manifest.json 的本地插件包，检查权限后再显式授权加载。")
                                    .child("WASM 插件在受限沙箱中运行（零 WASI）；Native worker 是可信本地进程，不受 OS 沙箱限制。")
                                    .child("使用「上移」「下移」调整流量/载荷钩子链中的执行顺序。")
                            }),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("通过自定义或第三方扩展定制 MIO 行为。"),
                    )
                    .child(
                        div()
                            .w(px(220.))
                            .flex_shrink_0()
                            .child(
                                Input::new(&self.extensions.search)
                                    .id("extensions-search-input")
                                    .aria_label("搜索已安装扩展")
                                    .small()
                                    .h(px(26.))
                                    .suffix(
                                        Button::new("extensions-search-clear")
                                            .ghost()
                                            .compact()
                                            .h(px(20.))
                                            .icon(if search_non_empty {
                                                IconName::Close
                                            } else {
                                                IconName::Search
                                            })
                                            .accessibility_label(if search_non_empty {
                                                "清除搜索"
                                            } else {
                                                "搜索"
                                            })
                                            .tooltip(if search_non_empty {
                                                "清除扩展搜索关键词"
                                            } else {
                                                "按扩展名称、引擎类型或路径筛选列表"
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if search_non_empty {
                                                    this.extensions.search.update(cx, |state, cx| {
                                                        state.set_value("", window, cx);
                                                    });
                                                } else {
                                                    this.extensions.search.update(cx, |state, cx| {
                                                        state.focus(window, cx);
                                                    });
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .mt(px(10.))
                    .w_full()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(left_buttons)
                    .child(table),
            )
            .into_any_element()
    }

    fn render_extensions_add_or_inspection(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(inspection) = &self.extensions.inspection {
            let is_wasm = inspection.info.manifest.engine == "wasm";
            let mut panel = div()
                .id("plugin-inspection-panel")
                .test_support()
                .w_full()
                .flex_shrink_0()
                .px(px(16.))
                .py(px(10.))
                .bg(theme::toolbar_surface())
                .border_b_1()
                .border_color(theme::border())
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .text_color(theme::foreground())
                                .child(format!(
                                    "确认并授权插件：{} · v{} · {} · API {}",
                                    inspection.info.manifest.id,
                                    inspection.info.manifest.version,
                                    inspection.info.manifest.engine,
                                    inspection.info.manifest.api_version,
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::muted_foreground())
                                .truncate()
                                .child(inspection.info.executable.display().to_string()),
                        ),
                )
                .child(security_warning_callout(is_wasm))
                .child(
                    Checkbox::new("plugin-native-trust")
                        .label(if is_wasm {
                            "我确认加载此受限 WASM 插件模块（零 WASI，无直接 OS 文件/网络访问）"
                        } else {
                            "我信任此本地 native 程序，并理解它不受 OS 沙箱限制"
                        })
                        .checked(inspection.trusted)
                        .on_click(cx.listener(|this, value, _, cx| {
                            if let Some(inspection) = &mut this.extensions.inspection {
                                inspection.trusted = *value;
                            }
                            cx.notify();
                        })),
                );
            for grant in &inspection.info.manifest.permissions {
                let grant = *grant;
                panel = panel.child(
                    Checkbox::new(("plugin-grant", grant as usize))
                        .label(permission_label(grant))
                        .checked(inspection.grants.contains(&grant))
                        .on_click(cx.listener(move |this, checked, _, cx| {
                            if let Some(inspection) = &mut this.extensions.inspection {
                                if *checked {
                                    inspection.grants.insert(grant);
                                } else {
                                    inspection.grants.remove(&grant);
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button(
                            "plugin-load",
                            if inspection.replace.is_some() {
                                "确认并重载"
                            } else {
                                "确认并加载"
                            },
                        )
                        .icon(IconName::Play)
                        .primary()
                        .tooltip("按已勾选的权限启动插件实例")
                        .disabled(!inspection.trusted)
                        .on_click(cx.listener(|this, _, _, cx| this.load_plugin(cx))),
                    )
                    .child(
                        button("plugin-dismiss-inspection", "取消")
                            .icon(IconName::Close)
                            .tooltip("取消本次插件加载")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.extensions.inspection = None;
                                this.extensions.add_panel_open = false;
                                cx.notify();
                            })),
                    ),
            );
            return Some(panel.into_any_element());
        }

        if self.extensions.add_panel_open || self.file_picker_pending() {
            let picker_pending = self.file_picker_pending();
            return Some(
                div()
                    .id("plugin-add-panel")
                    .test_support()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(16.))
                    .py(px(10.))
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_color(theme::foreground())
                                    .child("添加本地扩展包"),
                            )
                            .when(!picker_pending, |row| {
                                row.child(
                                    Button::new("plugin-close-add-panel")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .icon(IconName::Close)
                                        .accessibility_label("关闭添加扩展面板")
                                        .tooltip("收起手动路径面板")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.extensions.add_panel_open = false;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(
                                Input::new(&self.extensions.directory).small(),
                            ))
                            .child(
                                button("plugin-inspect", "检查插件包")
                                    .icon(IconName::ShieldCheck)
                                    .tooltip("解析插件清单并检查申请的宿主权限（不执行插件代码）")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.inspect_plugin(None, window, cx)
                                    })),
                            ),
                    )
                    .child(security_warning_callout(false))
                    .into_any_element(),
            );
        }

        None
    }

    fn render_extensions_detail_pane(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
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

    fn render_extensions_details_body(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();

        let selected = self
            .extensions
            .selected
            .and_then(|id| self.extensions.instance(id));
        let selected_id = selected.map(|inst| inst.info.identity.instance_id);
        let selected_live = selected.is_some_and(|inst| inst.live);
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
                    .disabled(selected_id.is_none() || busy)
                    .on_click(cx.listener(move |this, checked, window, cx| {
                        let Some(id) = selected_id else {
                            return;
                        };
                        if *checked {
                            this.inspect_plugin(Some(id), window, cx);
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
                                    .child("取消勾选「已加载扩展」会安全停用插件进程，并将未保存的插件编辑内容保留为只读。")
                                    .child("重新勾选会再次触发清单检查与显式授权，绝不静默执行。")
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

    fn render_extensions_plugin_store(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let card_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        div()
            .id("extensions-plugin-store-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child("Plugin Store (离线模式)"),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("MIO 采用本地优先、零远程商店依赖的扩展加载模型，不会自动连接第三方在线插件商店。"),
            )
            .child(
                div()
                    .max_w(px(720.))
                    .p(px(14.))
                    .border_1()
                    .border_color(card_border)
                    .rounded(px(3.))
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("安装本地插件包"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("将包含 manifest.json 与 WASM 模块（或 Native worker 可执行文件）的插件目录放置于本地后，通过「添加本地扩展」检查清单并显式授予所需宿主权限。"),
                    )
                    .child(
                        div()
                            .pt(px(4.))
                            .w(px(140.))
                            .child(
                                mio_neutral_button(
                                    "extensions-plugin-add-local",
                                    "添加本地扩展…",
                                    "返回已安装页并选择本地插件包目录",
                                    !busy,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.extensions.page = ExtensionsPage::Installed;
                                    this.extensions.add_panel_open = true;
                                    this.choose_plugin(window, cx);
                                })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn export_plugin_api_interface_files(
        &mut self,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        let suggested = if markdown {
            format!("mio-plugin-api-v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}.md")
        } else {
            format!("mio-plugin-api-v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}.json")
        };
        let json_payload = match build_mio_plugin_api_json() {
            Ok(bytes) => bytes,
            Err(err) => {
                self.fail(format!("生成 MIO 插件接口定义失败：{err}"), cx);
                return;
            }
        };
        let md_payload = build_mio_plugin_api_markdown().into_bytes();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(&suggested),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            let use_md = markdown
                                || path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .is_some_and(|e| e.eq_ignore_ascii_case("md"));
                            if path.extension().is_none() {
                                path.set_extension(if use_md { "md" } else { "json" });
                            }
                            let bytes = if use_md { &md_payload } else { &json_payload };
                            match std::fs::write(&path, bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已导出 MIO 插件接口定义文件：{}",
                                        path.display()
                                    );
                                    view.extensions.push_log(format!(
                                        "已导出 MIO Extensions API 接口定义到：{}",
                                        path.display()
                                    ));
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入接口定义文件失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => view.fail(format!("无法选择保存路径：{error}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    fn render_extensions_apis(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();

        let api_rows: [(&str, &str); 14] = [
            ("Engine: wasm", "受限 WASM 沙箱引擎（零 WASI，无直接 OS 文件或网络访问）"),
            ("Engine: native", "本地 Native worker 进程（不受 OS 沙箱限制，需用户显式信任）"),
            ("Contribution: suite_panel", "在顶部工具栏注册独立扩展标签页视图"),
            ("Contribution: tool_panel", "在 Extensions 详情页中展示扩展管理与交互面板"),
            ("Contribution: message_editor", "在 Repeater / Proxy Intercept / History 注册自定义请求编辑器"),
            ("Contribution: message_response_editor", "注册自定义 HTTP 响应查看与编辑视图"),
            ("Contribution: context_menu", "对当前活动请求/响应执行自定义上下文动作"),
            ("Contribution: payload_generator / processor", "为 Intruder 提供自定义载荷生成与处理规则"),
            ("Permission: traffic.read", permission_label(Permission::TrafficRead)),
            ("Permission: draft.write", permission_label(Permission::DraftWrite)),
            ("Permission: repeater.open", permission_label(Permission::RepeaterOpen)),
            ("Permission: network.send", permission_label(Permission::NetworkSend)),
            ("Permission: traffic.modify", permission_label(Permission::TrafficModify)),
            ("Permission: session.hook / payload / kv", "支持会话钩子、载荷处理与项目专属键值存储授权"),
        ];

        let mut table = div()
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
                    .w_full()
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(table_border)
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .text_color(header_fg)
                    .child(
                        div()
                            .w(px(260.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Capability"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child("Description"),
                    ),
            );

        for (index, (cap, desc)) in api_rows.into_iter().enumerate() {
            table = table.child(
                div()
                    .min_h(px(24.))
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
                            .w(px(260.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .py(px(3.))
                            .font_family(theme::mono_font_family())
                            .text_size(px(12.))
                            .truncate()
                            .child(cap),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(8.))
                            .py(px(3.))
                            .child(desc),
                    ),
            );
        }

        div()
            .id("extensions-apis-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(format!(
                        "MIO Extensions API (v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR})"
                    )),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("MIO 扩展协议支持受限 WASM 沙箱与 Native worker，每项宿主 API 权限均需由用户逐项显式授予。"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(px(240.)).child(
                            mio_neutral_button(
                                "extensions-apis-save-files",
                                "导出接口定义文件… (Save interface files)",
                                "导出 mio-plugin-api 清单 Schema 与 Protobuf/Host 协议 JSON 参考文件",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export_plugin_api_interface_files(false, window, cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(184.)).child(
                            mio_neutral_button(
                                "extensions-apis-save-md",
                                "导出 Markdown 接口文档…",
                                "导出可读的 Markdown 格式 mio-plugin-api 扩展开发参考文档",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export_plugin_api_interface_files(true, window, cx);
                            })),
                        ),
                    ),
            )
            .child(table)
            .into_any_element()
    }

    fn render_extensions_settings(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let card_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let wasm_banner = self
            .extensions
            .inspection
            .as_ref()
            .is_some_and(|inspection| inspection.info.manifest.engine == "wasm");

        div()
            .id("extensions-settings-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child("扩展设置 (Extensions settings)"),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .p(px(14.))
                    .border_1()
                    .border_color(card_border)
                    .rounded(px(3.))
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("从指定本地目录加载扩展包"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(
                                Input::new(&self.extensions.directory).small(),
                            ))
                            .child(
                                button("plugin-choose", "选择目录…")
                                    .icon(IconName::FolderOpen)
                                    .tooltip("浏览并选择包含 manifest.json 的本地插件目录")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_plugin(window, cx)
                                    })),
                            )
                            .child(
                                button("plugin-inspect", "检查插件包")
                                    .icon(IconName::ShieldCheck)
                                    .tooltip("解析插件清单并检查申请的宿主权限（不执行插件代码）")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.inspect_plugin(None, window, cx)
                                    })),
                            ),
                    )
                    .child(security_warning_callout(wasm_banner)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .p(px(14.))
                    .border_1()
                    .border_color(card_border)
                    .rounded(px(3.))
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("运行安全与生命周期策略"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("• 检查插件包（Inspect）仅解析 manifest.json 与入口路径，绝不执行插件代码。"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("• 切换项目时不会自动重启先前项目加载的扩展，未回写编辑内容将以只读状态保留供复制。"),
                    )
                    .child(
                        div()
                            .pt(px(4.))
                            .w(px(140.))
                            .child(
                                mio_neutral_button(
                                    "extensions-open-global-settings",
                                    "打开全局设置…",
                                    "打开 MIO 项目与用户全局设置窗口",
                                    true,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_settings(window, cx);
                                })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn mio_flat_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    selected: bool,
    tip: &'static str,
) -> gpui_kit::base::Button {
    let label = label.into();
    let dark = theme::is_dark();
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(if selected { 0x25272b } else { 0x2f3237 }).into()
    } else {
        rgb(if selected { 0xfbfbfb } else { 0xeeeeee }).into()
    };
    gpui_kit::base::Button::new(id)
        .role(gpui_kit::accesskit::Role::Tab)
        .accessibility_label(label.clone())
        .aria_selected(selected)
        .relative()
        .h(theme::secondary_tab_height() - px(1.))
        .self_start()
        .flex_shrink_0()
        .px(theme::scaled_px(15.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .line_height(theme::scaled_px(17.))
        .text_color(if selected && !dark {
            rgb(0x000000).into()
        } else {
            theme::foreground()
        })
        .flex()
        .items_center()
        .gap(px(6.))
        .bg(theme::toolbar_surface())
        .hover(move |s| s.bg(hover_bg))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .when_some(icon, |tab, ic| {
            tab.child(
                Icon::new(ic)
                    .size(px(18.))
                    .text_color(theme::foreground()),
            )
        })
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

pub(super) fn mio_neutral_button(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    tip: &'static str,
    available: bool,
) -> gpui_kit::base::Button {
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xececec }).into();
    let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into();
    let active_bg: gpui_kit::Hsla = rgb(if dark { 0x4b5056 } else { 0xd8d8d8 }).into();
    let border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xd0d0d0 }).into();
    let fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into();
    gpui_kit::base::Button::new(id)
        .accessibility_label(label)
        .disabled(!available)
        .w_full()
        .h(theme::scaled_px(26.).max(px(22.)))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(fg)
        .hover(move |s| if available { s.bg(hover_bg) } else { s })
        .active(move |s| if available { s.bg(active_bg) } else { s })
        .styles(|styles| styles.disabled(|s| s.opacity(0.45)))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(label)
}

fn security_warning_callout(is_wasm: bool) -> gpui_kit::Div {
    div()
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(3.))
        .bg(theme::danger_bg())
        .border_1()
        .border_color(theme::danger_border())
        .text_size(px(12.))
        .text_color(theme::danger_fg())
        .child(if is_wasm {
            "WASM 插件在受限沙箱中运行：零 WASI，无直接 OS 文件或网络访问。下面的授权只限制 MIO 宿主 API。"
        } else {
            "Native worker 是可信本地程序，不受 OS 沙箱限制，可直接使用当前用户的文件与网络权限。下面的授权只限制 MIO 宿主 API，并不能沙箱化进程。"
        })
}

fn permission_short_name(permission: Permission) -> &'static str {
    match permission {
        Permission::TrafficRead => "traffic.read",
        Permission::DraftWrite => "draft.write",
        Permission::RepeaterOpen => "repeater.open",
        Permission::NetworkSend => "network.send",
        Permission::TrafficModify => "traffic.modify",
        Permission::SessionHook => "session.hook",
        Permission::PayloadProcess => "payload.process",
        Permission::Storage => "storage.kv",
    }
}

fn permission_label(permission: Permission) -> &'static str {
    match permission {
        Permission::TrafficRead => "traffic.read：读取当前项目的流量摘要、请求与响应",
        Permission::DraftWrite => "draft.write：申请修改已绑定的 Repeater 草稿",
        Permission::RepeaterOpen => "repeater.open：申请创建新的 Repeater / Intruder 标签",
        Permission::NetworkSend => "network.send：经宿主发送请求（仅限获授权目标）",
        Permission::TrafficModify => "traffic.modify：注册 Proxy / HTTP / WebSocket 流量处理钩子",
        Permission::SessionHook => "session.hook：在发送前后执行会话处理动作",
        Permission::PayloadProcess => "payload.process：提供 Intruder payload 生成与处理规则",
        Permission::Storage => "storage.kv：读写当前项目内此插件专属键值存储",
    }
}

pub(super) fn build_mio_plugin_api_json() -> Result<Vec<u8>, serde_json::Error> {
    let doc = serde_json::json!({
        "name": "mio-plugin-api",
        "sdk_crate": "mio-plugin-sdk",
        "security_scope": "security isolation / sandboxing is explicitly out of scope; plugins are user-trusted local code",
        "protocol": {
            "major": PROTOCOL_MAJOR,
            "minor": PROTOCOL_MINOR,
            "framing": "4-byte big-endian length prefix + Protobuf Envelope",
            "max_frame_bytes": MAX_FRAME_BYTES,
            "max_inline_body_bytes": MAX_INLINE_BODY,
            "max_storage_key_bytes": MAX_STORAGE_KEY_BYTES,
            "max_storage_value_bytes": MAX_STORAGE_VALUE_BYTES
        },
        "manifest_schema": {
            "type": "object",
            "required": ["id", "version", "api_version", "engine", "entry", "permissions", "contributions"],
            "properties": {
                "id": { "type": "string", "description": "Unique extension identifier (e.g. com.example.ext)" },
                "version": { "type": "string", "description": "SemVer extension version" },
                "api_version": { "type": "integer", "const": PROTOCOL_MAJOR },
                "engine": { "type": "string", "enum": ["native", "wasm"] },
                "entry": { "type": "string", "description": "Relative path to native worker binary" },
                "permissions": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": [
                            "traffic.read",
                            "draft.write",
                            "repeater.open",
                            "network.send",
                            "traffic.modify",
                            "session.hook",
                            "payload.process",
                            "storage.kv"
                        ]
                    }
                },
                "contributions": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": [
                            "suite_panel",
                            "message_editor",
                            "message_response_editor",
                            "context_menu",
                            "http_hook",
                            "ws_hook",
                            "session_action",
                            "payload_generator",
                            "payload_processor",
                            "passive_check",
                            "active_check"
                        ]
                    }
                }
            }
        },
        "permissions": [
            { "id": "traffic.read", "description": permission_label(Permission::TrafficRead) },
            { "id": "draft.write", "description": permission_label(Permission::DraftWrite) },
            { "id": "repeater.open", "description": permission_label(Permission::RepeaterOpen) },
            { "id": "network.send", "description": permission_label(Permission::NetworkSend) },
            { "id": "traffic.modify", "description": permission_label(Permission::TrafficModify) },
            { "id": "session.hook", "description": permission_label(Permission::SessionHook) },
            { "id": "payload.process", "description": permission_label(Permission::PayloadProcess) },
            { "id": "storage.kv", "description": permission_label(Permission::Storage) }
        ],
        "ui_components": [
            "Row", "Column", "Split", "Tabs", "Text", "Button", "Editor",
            "VirtualTable", "Canvas", "ScrollColumn", "Checkbox", "Select", "Badge", "Progress"
        ],
        "envelope_messages": [
            "Hello", "Register", "Ready", "Snapshot", "Patch", "UiEvent",
            "OpenRequest", "CommitEditor", "Committed", "HostCall", "HostResult",
            "Cancel", "Shutdown", "Error", "Resync", "Ack", "CloseRequest",
            "HttpHookCall", "HttpHookResult", "WebSocketHookCall", "WebSocketHookResult",
            "SessionActionCall", "SessionActionResult", "PayloadCall", "PayloadResult",
            "OpenResponse", "CommitResponseEditor", "ResponseCommitted",
            "ContextMenuCall", "ContextMenuResult",
            "PassiveCheckCall", "PassiveCheckResult", "ActiveCheckCall", "ActiveCheckResult"
        ],
        "host_calls": [
            "QueryHistory", "ReadFlow", "UpdateDraft", "OpenRepeater", "NetworkSend",
            "StorageGet", "StorageSet", "OpenIntruder", "PreferencesGet", "PreferencesSet",
            "ScopeCheck", "ScopeUpdate", "QuerySiteMap", "ReadAnnotations", "UpdateAnnotations",
            "OpenOrganiser", "SubmitAuditIssue", "CollaboratorGenerate", "CollaboratorPoll"
        ],
        "custom_scan_check_rhai_api": {
            "variables": ["request", "response", "requestResponse", "utilities"],
            "return_values": [
                "bool (true => finding matched)",
                "string (non-empty => matched with evidence text)",
                "map (#{ matched: bool, evidence: string })"
            ]
        }
    });
    serde_json::to_vec_pretty(&doc)
}

pub(super) fn build_mio_plugin_api_markdown() -> String {
    format!(
        "# MIO Extensions API (v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR})\n\n\
         > Security isolation / sandboxing is out of scope; Native executable workers (`mio-plugin-sdk`) are the sole supported extension architecture.\n\n\
         ## Transport & Framing\n\
         - Framing: 4-byte big-endian length prefix + Protobuf `Envelope`\n\
         - Max frame size: `{MAX_FRAME_BYTES}` bytes\n\
         - Max inline body: `{MAX_INLINE_BODY}` bytes\n\
         - KV Storage limits: key `{MAX_STORAGE_KEY_BYTES}` bytes, value `{MAX_STORAGE_VALUE_BYTES}` bytes\n\n\
         ## Manifest (`manifest.json`)\n\
         ```json\n\
         {{\n  \"id\": \"com.example.extension\",\n  \"version\": \"1.0.0\",\n  \"api_version\": {PROTOCOL_MAJOR},\n  \"engine\": \"native\",\n  \"entry\": \"bin/extension\",\n  \"permissions\": [\"traffic.read\"],\n  \"contributions\": [\"suite_panel\", \"context_menu\", \"passive_check\"]\n}}\n\
         ```\n\n\
         ## Host Permissions\n\
         - `traffic.read`: {}\n\
         - `draft.write`: {}\n\
         - `repeater.open`: {}\n\
         - `network.send`: {}\n\
         - `traffic.modify`: {}\n\
         - `session.hook`: {}\n\
         - `payload.process`: {}\n\
         - `storage.kv`: {}\n\n\
         ## Contribution Points\n\
         `suite_panel`, `message_editor`, `message_response_editor`, `context_menu`, `http_hook`, `ws_hook`, \
         `session_action`, `payload_generator`, `payload_processor`, `passive_check`, `active_check`\n\n\
         ## HostCall Operations (v1.4)\n\
         `QueryHistory`, `ReadFlow`, `UpdateDraft`, `OpenRepeater`, `OpenIntruder`, `NetworkSend`, \
         `StorageGet`, `StorageSet`, `PreferencesGet`, `PreferencesSet`, \
         `ScopeCheck`, `ScopeUpdate`, `QuerySiteMap`, `ReadAnnotations`, `UpdateAnnotations`, \
         `OpenOrganiser`, `SubmitAuditIssue`, `CollaboratorGenerate`, `CollaboratorPoll`\n\n\
         ## Custom Scan Checks (Rhai)\n\
         Variables: `request`, `response`, `requestResponse`, `utilities`.\n\
         Return `true`, a non-empty evidence string, or `#{{ matched: true, evidence: \"...\" }}` to report a finding.\n",
        permission_label(Permission::TrafficRead),
        permission_label(Permission::DraftWrite),
        permission_label(Permission::RepeaterOpen),
        permission_label(Permission::NetworkSend),
        permission_label(Permission::TrafficModify),
        permission_label(Permission::SessionHook),
        permission_label(Permission::PayloadProcess),
        permission_label(Permission::Storage),
    )
}
