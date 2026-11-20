use super::*;
use gpui_kit::FontWeight;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu as _;

mod vertical;

impl Workbench {
    pub(in crate::workbench) fn render_tab_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vertical = self.workspace.orientation == Orientation::Vertical;
        let sidebar_width = vertical::effective_vertical_sidebar_width(
            self.workspace.tab_sidebar_width,
            self.tab_width,
        );
        let open_ws_count = self
            .workspace
            .websocket_tabs
            .iter()
            .filter(|tab| !tab.closed)
            .count();
        let total_open_tabs = self.workspace.tabs.len() + open_ws_count;
        let mut items = Vec::new();
        let mut seen_groups = HashSet::new();
        for tab in ordered_tabs(&self.workspace.tabs) {
            if let Some(group_id) = tab.group {
                if seen_groups.insert(group_id)
                    && let Some(group) = self
                        .workspace
                        .groups
                        .iter()
                        .find(|group| group.id == group_id)
                {
                    self.render_repeater_group(group, window, cx, &mut items);
                }
            } else {
                items.push(self.render_work_tab(tab, window, cx));
            }
        }
        for group in self
            .workspace
            .groups
            .iter()
            .filter(|group| !seen_groups.contains(&group.id))
        {
            self.render_repeater_group(group, window, cx, &mut items);
        }
        for ws_tab in self
            .workspace
            .websocket_tabs
            .iter()
            .filter(|tab| !tab.closed && tab.group.is_none())
        {
            items.push(self.render_websocket_work_tab(ws_tab, window, cx));
        }
        let new_disabled =
            self.project.is_none() || self.transition.is_some() || !self.backend_available();
        let new_menu_owner = cx.weak_entity();
        items.push(
            div()
                .id("repeater-new-menu")
                .test_support()
                .flex()
                .items_center()
                .flex_shrink_0()
                .when(!vertical, |row| row.ml(px(2.)))
                .when(vertical, |row| row.w_full().mt(px(4.)))
                .child(
                    flat_button("repeater-new", false, window, cx)
                        .accessibility_label("新建请求标签")
                        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                        .when(!vertical, |b| b.size(px(21.)).rounded(px(3.)).justify_center())
                        .when(vertical, |b| {
                            b.flex_1()
                                .h(theme::scaled_px(28.).max(px(28.)))
                                .px(px(10.))
                                .rounded(px(5.))
                                .justify_start()
                                .gap(px(6.))
                        })
                        .bg(gpui_kit::transparent_black())
                        .hover(|s| s.bg(theme::foreground().opacity(0.06)))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .disabled(new_disabled)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if cfg!(target_os = "macos")
                            {
                                "新建空白 HTTP 请求标签页（⌘T；右键可新建 WebSocket 标签页或分组）"
                            } else {
                                "新建空白 HTTP 请求标签页（Ctrl+T；右键可新建 WebSocket 标签页或分组）"
                            })
                            .build(window, cx)
                        })
                        .child(
                            gpui_kit::component::Icon::new(IconName::Plus)
                                .size(if vertical {
                                    px(14.)
                                } else {
                                    theme::scaled_px(17.)
                                })
                                .text_color(if vertical {
                                    theme::muted_foreground()
                                } else {
                                    theme::foreground()
                                }),
                        )
                        .when(vertical, |b| {
                            b.child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child("新建标签页"),
                            )
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
                )
                .context_menu(move |menu, _, cx| {
                    let Some(entity) = new_menu_owner.upgrade() else {
                        return menu;
                    };
                    let view = entity.read(cx);
                    let editable = view.project.is_some()
                        && view.backend_available()
                        && view.transition.is_none();
                    let can_new_group = editable
                        && (!view.workspace.tabs.is_empty()
                            || !view
                                .workspace
                                .websocket_tabs
                                .iter()
                                .all(|t| t.closed));
                    let seed_http = view
                        .workspace
                        .active
                        .or_else(|| view.workspace.tabs.first().map(|t| t.id));
                    let seed_ws = view.workspace.active_websocket_tab.or_else(|| {
                        view.workspace
                            .websocket_tabs
                            .iter()
                            .find(|t| !t.closed)
                            .map(|t| t.id)
                    });
                    let new_http = new_menu_owner.clone();
                    let new_ws = new_menu_owner.clone();
                    let new_group = new_menu_owner.clone();
                    menu.item(
                        PopupMenuItem::new("新建 HTTP 请求标签页")
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = new_http.update(cx, |this, cx| {
                                    this.new_tab(window, cx);
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("新建 WebSocket 会话标签页")
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = new_ws.update(cx, |this, cx| {
                                    this.new_websocket_repeater_tab(window, cx);
                                });
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("新建标签分组…")
                            .disabled(!can_new_group)
                            .on_click(move |_, window, cx| {
                                let _ = new_group.update(cx, |this, cx| {
                                    if seed_http.is_some() {
                                        this.open_tab_group_editor(None, seed_http, window, cx);
                                    } else if let Some(ws_id) = seed_ws {
                                        this.create_group_for_websocket_tab(ws_id, window, cx);
                                    }
                                });
                            }),
                    )
                })
                .into_any_element(),
        );
        if !vertical {
            let search_owner = cx.weak_entity();
            let strip_actions_disabled =
                self.project.is_none() || self.transition.is_some() || !self.backend_available();
            items.push(
                div()
                    .ml_auto()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new("repeater-search-tabs")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .w(px(22.))
                            .h(px(22.))
                            .p_0()
                            .tooltip("搜索并快速切换已打开的 Repeater 标签页")
                            .accessibility_label("Search tabs")
                            .tab_stop(false)
                            .disabled(strip_actions_disabled)
                            .child(
                                gpui_kit::component::Icon::new(IconName::Search)
                                    .size(px(16.))
                                    .text_color(if strip_actions_disabled {
                                        theme::muted_foreground()
                                    } else {
                                        theme::foreground()
                                    }),
                            )
                            .dropdown_menu(move |menu, _, cx| {
                                let Some(entity) = search_owner.upgrade() else {
                                    return menu;
                                };
                                let view = entity.read(cx);
                                let active_id = view.workspace.active;
                                let active_ws = view.workspace.active_websocket_tab;
                                let menu = view
                                    .workspace
                                    .tabs
                                    .iter()
                                    .fold(menu, |menu, tab| {
                                        let owner = search_owner.clone();
                                        let tab_id = tab.id;
                                        let title = tab.title.clone();
                                        menu.item(
                                            PopupMenuItem::new(title)
                                                .checked(active_id == Some(tab_id))
                                                .on_click(move |_, window, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        this.select_tab(tab_id, window, cx);
                                                    });
                                                }),
                                        )
                                    });
                                view.workspace
                                    .websocket_tabs
                                    .iter()
                                    .filter(|tab| !tab.closed)
                                    .fold(menu, |menu, tab| {
                                        let owner = search_owner.clone();
                                        let tab_id = tab.id;
                                        let title = if tab.title.starts_with("WS ") {
                                            tab.title.clone()
                                        } else {
                                            format!("WS {}", tab.title)
                                        };
                                        menu.item(
                                            PopupMenuItem::new(title)
                                                .checked(
                                                    active_id.is_none()
                                                        && active_ws == Some(tab_id),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = owner.update(cx, |this, cx| {
                                                        this.select_websocket_repeater_tab(
                                                            tab_id, window, cx,
                                                        );
                                                    });
                                                }),
                                        )
                                    })
                            }),
                    )
                    .child(
                        Button::new("repeater-toggle-vertical-tabs")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .w(px(22.))
                            .h(px(22.))
                            .p_0()
                            .tooltip("切换为左侧垂直侧栏标签布局，便于管理大量请求标签与分组")
                            .accessibility_label("切换为垂直标签")
                            .tab_stop(false)
                            .disabled(strip_actions_disabled)
                            .child(
                                gpui_kit::component::Icon::new(IconName::PanelLeft)
                                    .size(px(15.))
                                    .text_color(if strip_actions_disabled {
                                        theme::muted_foreground()
                                    } else {
                                        theme::foreground()
                                    }),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_tab_orientation(window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        let vertical_sidebar_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x1e2024 } else { 0xf6f7f9 }).into();
        let list = div()
            .id("work-tabs")
            .test_support()
            .flex()
            .flex_shrink_0()
            .min_w_0()
            .min_h_0()
            .bg(if vertical {
                vertical_sidebar_bg
            } else {
                theme::toolbar_surface()
            })
            .track_scroll(&self.tabs_scroll)
            .when(vertical, |this| {
                this.flex_col()
                    .w_full()
                    .flex_1()
                    .px(px(6.))
                    .py(px(6.))
                    .gap(px(2.))
                    .overflow_y_scroll()
                    .overflow_x_hidden()
            })
            .when(!vertical, |this| {
                this.w_full()
                    .h(px(47.))
                    .items_center()
                    .px(px(10.))
                    .gap(px(5.))
                    .overflow_x_scroll()
                    .overflow_y_hidden()
            })
            .children(items)
            .on_drag_move::<TabDrag>(cx.listener(
                move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                    let mut offset = this.tabs_scroll.offset();
                    let position = if vertical {
                        event.event.position.y
                    } else {
                        event.event.position.x
                    };
                    let start = if vertical {
                        event.bounds.origin.y
                    } else {
                        event.bounds.origin.x
                    };
                    let end = if vertical {
                        event.bounds.bottom()
                    } else {
                        event.bounds.right()
                    };
                    let delta = if position < start + px(22.) {
                        px(14.)
                    } else if position > end - px(22.) {
                        px(-14.)
                    } else {
                        px(0.)
                    };
                    if vertical {
                        offset.y += delta;
                    } else {
                        offset.x += delta;
                    }
                    this.tabs_scroll.set_offset(offset);
                    cx.notify();
                },
            ));
        if vertical {
            div()
                .id("repeater-vertical-sidebar")
                .test_support()
                .relative()
                .w(px(sidebar_width))
                .h_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(theme::border())
                .child(self.render_vertical_sidebar_header(total_open_tabs, cx))
                .child(list)
                .child(
                    resize_handle("repeater-vertical-tabs-resize", Axis::Horizontal)
                        .inside(HandleEdge::Trailing)
                        .on_drag(ResizeTabBar, |drag, _, _, cx| cx.new(|_| drag.clone())),
                )
                .into_any_element()
        } else {
            div()
                .relative()
                .h(px(47.))
                .flex_shrink_0()
                .child(list)
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(theme::border()),
                )
                .into_any_element()
        }
    }

    fn render_repeater_group(
        &self,
        group: &TabGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
        items: &mut Vec<AnyElement>,
    ) {
        let id = group.id;
        let count = self
            .workspace
            .tabs
            .iter()
            .filter(|tab| tab.group == Some(id))
            .count()
            + self
                .workspace
                .websocket_tabs
                .iter()
                .filter(|tab| !tab.closed && tab.group == Some(id))
                .count();
        if self.workspace.orientation == Orientation::Vertical {
            items.push(self.render_vertical_group_header(group, count, cx));
            if !group.collapsed {
                for tab in self
                    .workspace
                    .tabs
                    .iter()
                    .filter(|tab| tab.group == Some(id))
                {
                    items.push(self.render_work_tab(tab, window, cx));
                }
                for ws_tab in self
                    .workspace
                    .websocket_tabs
                    .iter()
                    .filter(|tab| !tab.closed && tab.group == Some(id))
                {
                    items.push(self.render_websocket_work_tab(ws_tab, window, cx));
                }
            }
            return;
        }
        let weak = cx.weak_entity();
        let group_color = super::tab_groups::group_color(group.color);
        let group_tooltip = SharedString::from(format!(
            "{}标签分组「{}」（共 {count} 个标签，右键管理分组）",
            if group.collapsed { "展开" } else { "折叠" },
            group.title
        ));
        items.push(
            div()
                .id(SharedString::from(format!("group-{id}")))
                .role(gpui_kit::accesskit::Role::Button)
                .test_support()
                .flex()
                .items_center()
                .gap(theme::scaled_px(5.))
                .px(theme::scaled_px(9.))
                .h(px(26.))
                .rounded(px(4.))
                .flex_shrink_0()
                .bg(group_color)
                .text_color(rgb(0x1f2328))
                .hover(move |s| s.bg(group_color.opacity(0.88)))
                .cursor_pointer()
                .aria_label(group.title.clone())
                .aria_expanded(!group.collapsed)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(group_tooltip.clone())
                        .build(window, cx)
                })
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::component::IconName::Folder)
                        .size(theme::scaled_px(14.)),
                )
                .child(group.title.clone())
                .child(
                    div()
                        .px(theme::scaled_px(6.))
                        .rounded(px(8.))
                        .bg(gpui_kit::black().opacity(0.12))
                        .child(count.to_string()),
                )
                .child(
                    gpui_kit::component::Icon::new(if group.collapsed {
                        gpui_kit::component::IconName::ChevronRight
                    } else {
                        gpui_kit::component::IconName::ChevronLeft
                    })
                    .size(theme::scaled_px(12.)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.backend_available() || this.transition.is_some() {
                        return;
                    }
                    if let Some(group) = this
                        .workspace
                        .groups
                        .iter_mut()
                        .find(|group| group.id == id)
                    {
                        group.collapsed = !group.collapsed;
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
                .into_any_element(),
        );
        if !group.collapsed {
            for tab in self
                .workspace
                .tabs
                .iter()
                .filter(|tab| tab.group == Some(id))
            {
                items.push(self.render_work_tab(tab, window, cx));
            }
            for ws_tab in self
                .workspace
                .websocket_tabs
                .iter()
                .filter(|tab| !tab.closed && tab.group == Some(id))
            {
                items.push(self.render_websocket_work_tab(ws_tab, window, cx));
            }
        }
    }

    fn render_work_tab(
        &self,
        tab: &WorkTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = tab.id;
        let vertical = self.workspace.orientation == Orientation::Vertical;
        let is_active = self.workspace.active == Some(id);
        let tab_group = tab
            .group
            .and_then(|gid| self.workspace.groups.iter().find(|group| group.id == gid));
        let group_accent = tab_group.map(|group| super::tab_groups::group_color(group.color));
        let horizontal_tinted = !vertical && group_accent.is_some();
        let title = format!(
            "{}{}{}",
            if self.sending.contains_key(&id) || self.pending_sends.contains_key(&id) {
                "◌ "
            } else {
                ""
            },
            tab.title,
            if self
                .tab_editors
                .get(&id)
                .is_some_and(|editors| editors.dirty)
            {
                " •"
            } else {
                ""
            }
        );
        let tab_tooltip = SharedString::from(match tab_group {
            Some(group) => format!(
                "切换到请求标签「{}」（所属分组：{}，右键查看更多操作）",
                tab.title, group.title
            ),
            None => format!("切换到请求标签「{}」（右键重命名、分组或复制）", tab.title),
        });
        let vertical_active_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2f333a } else { 0xe8eaed }).into();
        let tab_hover_bg: gpui_kit::Hsla = if vertical {
            if is_active {
                vertical_active_bg
            } else {
                theme::foreground().opacity(0.06)
            }
        } else if let Some(accent) = group_accent {
            accent.opacity(0.88)
        } else if is_active {
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
            .id(SharedString::from(format!("work-tab-{id}")))
            .role(gpui_kit::accesskit::Role::Tab)
            .aria_label(title.clone())
            .aria_selected(is_active)
            .test_support()
            .relative()
            .flex()
            .items_center()
            .h(px(26.))
            .flex_shrink_0()
            .min_w(theme::scaled_px(46.))
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
                    .font_weight(if is_active {
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
            .when(!vertical && !horizontal_tinted && is_active && !theme::is_dark(), |this| {
                this.text_color(rgb(0x000000))
            })
            .bg(if vertical {
                if is_active {
                    vertical_active_bg
                } else {
                    gpui_kit::transparent_black()
                }
            } else {
                group_accent.unwrap_or_else(|| {
                    if is_active {
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
            .when(self.drop_target == Some(id), |this| {
                match (vertical, self.drop_after) {
                    (true, false) => this.border_t_2(),
                    (true, true) => this.border_b_2(),
                    (false, false) => this.border_l_2(),
                    (false, true) => this.border_r_2(),
                }
                .border_color(theme::accent())
            })
            .cursor_pointer()
            .child(div().flex_1().truncate().child(title.clone()))
            .child(
                flat_button(
                    SharedString::from(format!("close-tab-{id}")),
                    false,
                    window,
                    cx,
                )
                .accessibility_label("关闭标签")
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("关闭此标签页").build(window, cx)
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
                    this.close_tab(id, window, cx);
                })),
            )
            .when(is_active, |tab| {
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
            .on_click(cx.listener(move |this, _, window, cx| this.select_tab(id, window, cx)))
            .on_drag(TabDrag { id, title }, |drag, _, _, cx| {
                cx.new(|_| drag.clone())
            })
            .on_drag_move::<TabDrag>(cx.listener(
                move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                    if event.bounds.contains(&event.event.position) {
                        this.drop_target = Some(id);
                        this.drop_after = if vertical {
                            event.event.position.y > event.bounds.center().y
                        } else {
                            event.event.position.x > event.bounds.center().x
                        };
                        cx.notify();
                    }
                },
            ))
            .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                this.move_tab(drag.id, id, window, cx)
            }))
            .context_menu(move |menu, window, cx| {
                repeater_tab_menu(weak.clone(), id, menu, window, cx)
            })
            .into_any_element()
    }
}
