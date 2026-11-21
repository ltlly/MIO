use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon,
    checkbox::Checkbox,
    menu::{ContextMenuExt, PopupMenuItem},
    popover::Popover,
};
use gpui_kit::MouseButton;
use mio_core::protocol::Permission;

use super::*;

const GAUGE_ICON_SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.4\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 11.5a5.5 5.5 0 1 1 11 0\"/><path d=\"M8 10.5L10.8 6.8\"/><circle cx=\"8\" cy=\"10.5\" r=\"1.1\" fill=\"currentColor\"/></svg>";

impl Workbench {
    #[inline(never)]
    pub(super) fn render_extensions_installed(
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

    #[inline(never)]
    fn render_extensions_impact_strip(&self) -> AnyElement {
        let dark = theme::is_dark();
        let live_count = self
            .extensions
            .instances
            .iter()
            .filter(|i| i.live && i.info.tab_id.is_none() && i.context_owner.is_none())
            .count();
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

    #[inline(never)]
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

        let search_raw = self.extensions.search.read(cx).value().to_string();
        let search_query = search_raw.trim().to_lowercase();
        let search_non_empty = !search_query.is_empty();

        let selected_instance = self
            .extensions
            .selected
            .and_then(|id| self.extensions.instance(id))
            .filter(|inst| inst.info.tab_id.is_none() && inst.context_owner.is_none());
        let selected_id = selected_instance.map(|inst| inst.info.identity.instance_id);
        let selected_live = selected_instance.is_some_and(|inst| inst.live);
        let selected_persisted = selected_instance.is_some_and(|inst| inst.persisted);
        let selected_movable = selected_instance.is_some_and(|inst| {
            inst.live && inst.info.tab_id.is_none() && inst.context_owner.is_none()
        });

        let remove_id = if selected_persisted {
            "plugin-uninstall"
        } else {
            match (selected_id, selected_live) {
                (Some(_), true) => "plugin-disable",
                (Some(_), false) => "plugin-remove-retained",
                (None, _) => "plugin-remove",
            }
        };
        let remove_tip = if selected_persisted {
            "从持久化插件目录卸载并移除选中扩展"
        } else if selected_live {
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
                    "选择本地插件包（目录或 .tar.gz / .tgz / .mio-plugin 文件）并检查权限",
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
                    if selected_persisted {
                        this.uninstall_plugin(id, window, cx);
                    } else if selected_live {
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

        let top_level_instances: Vec<&Instance> = self
            .extensions
            .instances
            .iter()
            .filter(|instance| instance.info.tab_id.is_none() && instance.context_owner.is_none())
            .collect();
        let has_any_installed = !top_level_instances.is_empty();
        let filtered: Vec<&Instance> = top_level_instances
            .into_iter()
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
            } else if !has_any_installed {
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
                rows = rows.child(self.render_plugin_instance_row(
                    index,
                    instance,
                    busy,
                    dark,
                    zebra_bg,
                    cx,
                ));
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
                                    .child("Native worker 是可信本地进程，不受 OS 沙箱限制，授权仅控制 MIO 宿主 API。")
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

    #[inline(never)]
    fn render_plugin_instance_row(
        &self,
        index: usize,
        instance: &Instance,
        busy: bool,
        dark: bool,
        zebra_bg: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        let sel_border: gpui_kit::Hsla = rgb(if dark { 0x5b8ad6 } else { 0x4876b5 }).into();
        let id = instance.info.identity.instance_id;
        let plugin_id = instance.info.manifest.id.clone();
        let persisted_suite = instance.persisted;
        let extensions_disabled = self.extensions.extensions_disabled;
        let is_selected = self.extensions.selected == Some(id);
        let live = instance.live;
        let is_movable =
            live && instance.info.tab_id.is_none() && instance.context_owner.is_none();
        let auto_reload = self.extensions.auto_reload.contains(&id);
        let engine_label = "Native";
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
        let toggle_plugin_id = plugin_id.clone();
        let ctx_plugin_id = plugin_id.clone();

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
                let ctx_plugin_id = ctx_plugin_id.clone();
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
                            .disabled(busy || (!live && extensions_disabled))
                            .on_click(move |_, window, cx| {
                                let ctx_plugin_id = ctx_plugin_id.clone();
                                let _ = o_toggle.update(cx, |this, cx| {
                                    this.extensions.selected = Some(id);
                                    if live {
                                        if this.extensions.active_top_suite_tab == Some(id) {
                                            this.extensions.active_top_suite_tab = None;
                                        }
                                        this.disable_plugin(id, cx);
                                    } else if persisted_suite {
                                        this.enable_installed_plugin(id, ctx_plugin_id, cx);
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
                        PopupMenuItem::new(if persisted_suite || live {
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
                                if persisted_suite {
                                    this.uninstall_plugin(id, window, cx);
                                } else if live {
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
                        Checkbox::new(SharedString::from(format!("plugin-loaded-{id}")))
                            .small()
                            .checked(live)
                            .disabled(busy || (!live && extensions_disabled))
                            .on_click(cx.listener(move |this, checked, window, cx| {
                                cx.stop_propagation();
                                this.extensions.selected = Some(id);
                                if *checked {
                                    if persisted_suite {
                                        this.enable_installed_plugin(
                                            id,
                                            toggle_plugin_id.clone(),
                                            cx,
                                        );
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
                            Checkbox::new(SharedString::from(format!("plugin-autoreload-{id}")))
                                .small()
                                .checked(auto_reload)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, checked, _, cx| {
                                    cx.stop_propagation();
                                    this.set_plugin_auto_reload(id, *checked, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_extensions_add_or_inspection(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(inspection) = &self.extensions.inspection {
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
                .child(security_warning_callout())
                .child(
                    Checkbox::new("plugin-native-trust")
                        .label("我信任此本地 native 程序，并理解它不受 OS 沙箱限制")
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
                    .child(security_warning_callout())
                    .into_any_element(),
            );
        }

        None
    }
}
