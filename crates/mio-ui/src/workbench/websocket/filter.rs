use gpui_kit::component::{
    input::EditorState,
    popover::Popover,
    switch::Switch,
};
use gpui_kit::{AnyWindowHandle, Bounds, size};

use super::*;

mod window;

const FILTER_WINDOW_WIDTH: f32 = 796.;
const FILTER_WINDOW_HEIGHT: f32 = 340.;

impl Workbench {
    pub(in crate::workbench::websocket) fn apply_websocket_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search = match editing::input_snapshot(&self.websocket.search_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if search.len() > 4096 {
            self.fail("搜索关键字超过 4096 字节；当前列表保留。", cx);
            return;
        }
        self.websocket.query_search = search;
        self.query_websocket_filtered_with_scope(
            self.websocket.filter,
            0,
            self.websocket.applied_filter.clone(),
            self.websocket.in_scope_only,
            self.websocket.filter_enabled,
            window,
            cx,
        );
    }

    pub(in crate::workbench::websocket) fn clear_websocket_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = editing::input_snapshot(&self.websocket.search_input, window, cx) {
            self.fail(error, cx);
            return;
        }
        self.websocket.search_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.websocket.query_search.clear();
        self.query_websocket_filtered_with_scope(
            self.websocket.filter,
            0,
            self.websocket.applied_filter.clone(),
            self.websocket.in_scope_only,
            self.websocket.filter_enabled,
            window,
            cx,
        );
    }

    pub(in crate::workbench::websocket) fn reset_websocket_filter_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = editing::input_snapshot(&self.websocket.search_input, window, cx) {
            self.fail(error, cx);
            return;
        }
        self.websocket.search_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.websocket.query_search.clear();
        self.query_websocket_filtered_with_scope(
            None,
            0,
            WebSocketHistoryFilter::default(),
            false,
            true,
            window,
            cx,
        );
    }

    pub(in crate::workbench::websocket) fn toggle_websocket_column_visibility(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        if key == "number" {
            return;
        }
        let scale = theme::ui_scale().max(1.0);
        let all_cols_mut = &mut self.websocket.all_columns;
        self.websocket.table.update(cx, |table, _| {
            let widths: Vec<Option<gpui_kit::Pixels>> = (0..table.delegate().columns.len())
                .map(|ix| table.col_width(ix))
                .collect();
            let delegate = table.delegate_mut();
            for (ix, col) in delegate.columns.iter_mut().enumerate() {
                if let Some(Some(w)) = widths.get(ix)
                    && *w > px(0.)
                {
                    col.width = px((w.as_f32() / scale).round());
                }
                if let Some(slot) = all_cols_mut.iter_mut().find(|c| c.key == col.key) {
                    slot.width = col.width;
                }
            }
        });
        let all_columns = self.websocket.all_columns.clone();
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let active_sort = delegate.active_sort();
            if delegate.columns.iter().any(|c| c.key.as_ref() == key) {
                if delegate.columns.len() > 1 {
                    delegate.columns.retain(|c| c.key.as_ref() != key);
                }
            } else if let Some((target_pos, target_col)) = all_columns
                .iter()
                .enumerate()
                .find(|(_, c)| c.key.as_ref() == key)
            {
                let insert_ix = delegate
                    .columns
                    .iter()
                    .position(|existing| {
                        all_columns
                            .iter()
                            .position(|d| d.key == existing.key)
                            .is_some_and(|pos| pos > target_pos)
                    })
                    .unwrap_or(delegate.columns.len());
                delegate.columns.insert(insert_ix, target_col.clone());
            }
            delegate.apply_sort_key(active_sort);
            table.refresh(cx);
        });
        cx.notify();
    }

    pub(in crate::workbench::websocket) fn restore_default_websocket_columns(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.websocket.all_columns = WebSocketDelegate::default_columns();
        self.websocket.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let active_sort = delegate.active_sort();
            delegate.columns = WebSocketDelegate::default_columns();
            delegate.apply_sort_key(active_sort);
            table.refresh(cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn render_websocket_filter_bar(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = &self.websocket.applied_filter;
        let label: String = if !filter.filter_script.is_empty() {
            "筛选设置：Script mode (MioScript)".into()
        } else {
            let mut descriptions = Vec::new();
            if filter.hide_outgoing {
                descriptions.push("隐藏传出消息".to_owned());
            }
            if filter.hide_incoming {
                descriptions.push("隐藏传入消息".to_owned());
            }
            if let Some(port) = filter.listener_port {
                descriptions.push(format!("监听端口 {port}"));
            }
            if !filter.term.is_empty() {
                descriptions.push("搜索消息正文".into());
            }
            if filter.only_with_notes {
                descriptions.push("仅显示有备注项".into());
            }
            if self.websocket.in_scope_only {
                descriptions.push("仅显示范围内条目".into());
            }
            if descriptions.is_empty() {
                "筛选设置：显示全部".into()
            } else {
                format!("筛选设置：{}", descriptions.join("；"))
            }
        };
        let dark = theme::is_dark();
        let owner = cx.weak_entity();
        let show_clear_search = {
            let search_state = self.websocket.search_input.read(cx);
            let applied = self.websocket.applied_search.as_str();
            !search_state.has_marked_text()
                && !applied.is_empty()
                && search_state.value().as_ref() == applied
                && self.websocket.query_search == applied
        };
        let filter_bg: gpui_kit::Hsla = if dark {
            rgb(0x35383d).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let filter_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x3f434a).into()
        } else {
            rgb(0xe2e2e2).into()
        };
        let filter_btn = gpui_kit::base::Button::new("websocket-filter")
            .accessibility_label(label.clone())
            .h(theme::scaled_px(23.))
            .rounded(px(3.))
            .border_1()
            .border_color(theme::border())
            .px(theme::scaled_px(8.))
            .bg(filter_bg)
            .hover(move |s| s.bg(filter_hover_bg))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_start()
            .gap(theme::scaled_px(6.))
            .overflow_hidden()
            .text_color(theme::foreground())
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "打开 WebSockets 历史记录高级筛选窗口（按消息方向、搜索词、注解与监听端口过滤）",
                )
                .build(window, cx)
            })
            .on_click(
                cx.listener(|this, _, window, cx| this.open_websocket_filter(window, cx)),
            )
            .child(
                gpui_kit::component::Icon::new(IconName::Funnel)
                    .size(theme::scaled_px(13.))
                    .text_color(theme::foreground()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_left()
                    .child(label.clone().test_text(label)),
            );
        toolbar()
            .h(theme::toolbar_height())
            .border_b_0()
            .flex_nowrap()
            .gap(theme::scaled_px(10.))
            .child(filter_btn)
            .child(
                div()
                    .w(theme::scaled_px(254.))
                    .min_w(theme::scaled_px(120.))
                    .flex_shrink(1.0)
                    .child(
                        Input::new(&self.websocket.search_input)
                            .small()
                            .h(theme::scaled_px(23.))
                            .focus_bordered(false)
                            .suffix(
                                Button::new("websocket-search")
                                    .ghost()
                                    .compact()
                                    .h(theme::scaled_px(19.))
                                    .icon(if show_clear_search {
                                        IconName::Close
                                    } else {
                                        IconName::Search
                                    })
                                    .accessibility_label(if show_clear_search {
                                        "清除筛选"
                                    } else {
                                        "筛选"
                                    })
                                    .tooltip(if show_clear_search {
                                        "清除当前搜索关键字并恢复列表"
                                    } else {
                                        "按 URL、方向、端口或备注关键字快速过滤 WebSocket 历史记录"
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if show_clear_search {
                                            this.clear_websocket_search(window, cx);
                                        } else {
                                            this.apply_websocket_search(window, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("websocket-filter-switch-wrap")
                    .w(theme::scaled_px(86.))
                    .h(theme::scaled_px(21.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_shrink_0()
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "启用或暂停当前筛选条件（关闭时显示全部历史记录）",
                        )
                        .build(window, cx)
                    })
                    .child(
                        Switch::new("websocket-filter-enabled")
                            .small()
                            .color(rgb(0x1558b0))
                            .label(if self.websocket.query_filter_enabled {
                                "筛选开启"
                            } else {
                                "筛选关闭"
                            })
                            .tooltip("启用或暂停当前筛选条件（关闭时显示全部历史记录）")
                            .checked(self.websocket.query_filter_enabled)
                            .on_change(cx.listener(|this, checked, window, cx| {
                                this.query_websocket_filtered_with_scope(
                                    this.websocket.filter,
                                    0,
                                    this.websocket.applied_filter.clone(),
                                    this.websocket.in_scope_only,
                                    *checked,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .w(px(46.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_shrink_0()
                    .child(
                        Popover::new("websocket-help-popover")
                            .anchor(gpui_kit::Anchor::BottomRight)
                            .trigger(
                                Button::new("websocket-help")
                                    .small()
                                    .ghost()
                                    .compact()
                                    .size(px(18.))
                                    .rounded(px(9.))
                                    .icon(IconName::CircleQuestionMark)
                                    .accessibility_label("帮助")
                                    .tooltip("查看 WebSockets 历史记录筛选与操作说明"),
                            )
                            .content(|_, _, _| {
                                div()
                                    .id("websocket-help-content")
                                    .test_support()
                                    .w(px(300.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child("WebSockets 历史记录")
                                    .child("点击左侧筛选栏可按方向、监听端口、搜索词、备注或 脚本过滤。")
                                    .child("右侧搜索框支持按 URL、备注、方向、端口或编号即时过滤。")
                                    .child("右键列表行或点击正文工具栏菜单，可将帧发送到 Repeater、Decoder、Comparer 或编辑 Notes。")
                            }),
                    )
                    .child(
                        Button::new("websocket-options-menu")
                            .small()
                            .ghost()
                            .compact()
                            .size(px(18.))
                            .rounded(px(3.))
                            .icon(IconName::EllipsisVertical)
                            .accessibility_label("WebSockets 历史记录选项")
                            .tooltip("WebSockets 历史记录操作选项")
                            .dropdown_menu(move |menu, window, cx| {
                                let Some(entity) = owner.upgrade() else {
                                    return menu;
                                };
                                let view = entity.read(cx);
                                let can_query = view.project.is_some()
                                    && view.backend_available()
                                    && view.transition.is_none();
                                let current_keys: Vec<SharedString> = view
                                    .websocket
                                    .table
                                    .read(cx)
                                    .delegate()
                                    .columns
                                    .iter()
                                    .map(|col| col.key.clone())
                                    .collect();
                                let all_cols = view.websocket.all_columns.clone();
                                let layout_owner = owner.clone();
                                let layout_submenu = gpui_kit::component::menu::PopupMenu::build(
                                    window,
                                    cx,
                                    move |mut sub, _, _| {
                                        let visible_count = current_keys.len();
                                        for col in &all_cols {
                                            let key = col.key.clone();
                                            let visible = current_keys.contains(&key);
                                            let is_number = key.as_ref() == "number";
                                            let can_toggle =
                                                !is_number && (!visible || visible_count > 1);
                                            let toggle_owner = layout_owner.clone();
                                            let label = if visible {
                                                format!("✓ {}", col.name)
                                            } else {
                                                format!("   {}", col.name)
                                            };
                                            sub = sub.item(
                                                PopupMenuItem::new(label)
                                                    .disabled(!can_toggle)
                                                    .on_click(move |_, _, cx| {
                                                        let _ = toggle_owner.update(cx, |this, cx| {
                                                            this.toggle_websocket_column_visibility(
                                                                key.as_ref(),
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            );
                                        }
                                        let restore_owner = layout_owner.clone();
                                        sub.separator().item(
                                            PopupMenuItem::new(
                                                "恢复默认列布局 (Restore default layout)",
                                            )
                                            .on_click(
                                                move |_, _, cx| {
                                                    let _ = restore_owner.update(cx, |this, cx| {
                                                        this.restore_default_websocket_columns(cx);
                                                    });
                                                },
                                            ),
                                        )
                                    },
                                );
                                let reset_owner = owner.clone();
                                let refresh_owner = owner.clone();
                                menu.item(PopupMenuItem::submenu(
                                    "表格列布局 (Table layout)",
                                    layout_submenu,
                                ))
                                .separator()
                                .item(
                                    PopupMenuItem::new("重置筛选并显示全部")
                                        .disabled(!can_query)
                                        .on_click(move |_, window, cx| {
                                            let _ = reset_owner.update(cx, |this, cx| {
                                                this.reset_websocket_filter_all(window, cx);
                                            });
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("刷新历史记录")
                                        .disabled(!can_query)
                                        .on_click(move |_, window, cx| {
                                            let _ = refresh_owner.update(cx, |this, cx| {
                                                this.query_websocket_history(
                                                    this.websocket.filter,
                                                    0,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn open_websocket_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project else { return };
        let owner = cx.entity();
        let owner_window = window.window_handle();
        let scale = theme::ui_scale();
        let content_w = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_WIDTH)
        } else {
            px(FILTER_WINDOW_WIDTH)
        };
        let content_h = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_HEIGHT)
        } else {
            px(FILTER_WINDOW_HEIGHT)
        };
        let desired_h = if cfg!(test) {
            content_h
        } else {
            let bar_h = if scale > 1.0 {
                theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                    .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
            } else {
                px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            };
            content_h + bar_h
        };
        let bounds = if let Some(display) = window.display(cx).or_else(|| cx.primary_display()) {
            let visible = display.visible_bounds();
            let fitted = size(
                content_w.min(visible.size.width),
                desired_h.min(visible.size.height),
            );
            let mut bounds = Bounds::centered(Some(display.id()), fitted, cx);
            bounds.origin.x = bounds.origin.x.clamp(
                visible.origin.x,
                (visible.right() - fitted.width).max(visible.origin.x),
            );
            bounds.origin.y = bounds.origin.y.clamp(
                visible.origin.y,
                (visible.bottom() - fitted.height).max(visible.origin.y),
            );
            bounds
        } else {
            Bounds::centered(None, size(content_w, desired_h), cx)
        };
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            if let Some(existing) = owner.read(cx).websocket.filter_window
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let applied = owner.read(cx).websocket.applied_filter.clone();
            let in_scope_only = owner.read(cx).websocket.in_scope_only;
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        FilterWindow::new(
                            owner.clone(),
                            owner_window,
                            project,
                            applied,
                            in_scope_only,
                            window,
                            cx,
                        )
                    })
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((window, _)) => view.websocket.filter_window = Some(window),
                Err(error) => view.fail(format!("无法打开 WebSockets 筛选窗口：{error}"), cx),
            });
        });
    }
}

pub(super) struct FilterWindow {
    pub(super) owner: WeakEntity<Workbench>,
    pub(super) owner_window: AnyWindowHandle,
    pub(super) project: ProjectKey,
    pub(super) applied: WebSocketHistoryFilter,
    pub(super) applied_in_scope_only: bool,
    pub(super) draft: WebSocketHistoryFilter,
    pub(super) draft_in_scope_only: bool,
    pub(super) script_mode: bool,
    pub(super) port: Entity<InputState>,
    pub(super) term: Entity<InputState>,
    pub(super) filter_script_editor: Entity<EditorState>,
    pub(super) pending: Option<(u64, bool)>,
    pub(super) submitting: bool,
    pub(super) error: Option<String>,
    pub(super) focus: FocusHandle,
    pub(super) _subscription: Subscription,
}

#[cfg(test)]
mod tests;
