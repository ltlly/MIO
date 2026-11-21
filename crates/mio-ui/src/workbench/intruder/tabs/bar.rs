use gpui_kit::Axis;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::resizable::{HandleEdge, resize_handle};
use gpui_kit::{DragMoveEvent, FontWeight, WeakEntity};
use mio_core::IntruderTab;

use super::*;

const INTRUDER_GROUP_PALETTE: &[(u8, u32, &str)] = &[
    (7, 0xe2e2fb, "浅紫色"),
    (1, 0xfce5e4, "浅粉色"),
    (2, 0xf5e7d5, "浅橙色"),
    (3, 0xdeecd8, "浅绿色"),
    (5, 0xeff4f8, "浅蓝色"),
    (4, 0xe0fafe, "浅青色"),
    (6, 0xfdeafa, "浅洋红色"),
    (0, 0xd3d3d3, "灰色"),
    (12, 0xb8b9fa, "紫色"),
    (8, 0xedacb0, "粉色"),
    (9, 0xeacb97, "橙色"),
    (10, 0xafd3a0, "绿色"),
    (11, 0xb1c7db, "蓝色"),
    (13, 0xb8e0e7, "青色"),
    (14, 0xdeadbe, "洋红色"),
];

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_tab_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vertical = self.workspace.intruder.orientation == Orientation::Vertical;
        let disabled =
            self.project.is_none() || self.transition.is_some() || self.backend_disconnected;
        let total_tabs = self.workspace.intruder.tabs.len();
        let mut items = Vec::new();
        for tab in self
            .workspace
            .intruder
            .tabs
            .iter()
            .filter(|tab| tab.group.is_none())
        {
            items.push(self.render_intruder_tab(tab, window, cx));
        }
        for group in &self.workspace.intruder.groups {
            let id = group.id;
            items.push(self.render_intruder_group_pill(
                id,
                &group.title,
                group.color,
                group.collapsed,
                vertical,
                window,
                cx,
            ));
            if !group.collapsed {
                for tab in self
                    .workspace
                    .intruder
                    .tabs
                    .iter()
                    .filter(|tab| tab.group == Some(id))
                {
                    items.push(self.render_intruder_tab(tab, window, cx));
                }
            }
        }
        items.push(
            flat_button("intruder-new", false, window, cx)
                .accessibility_label("新建 Intruder 工作标签")
                .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(5.))
                .flex_shrink_0()
                .rounded(px(4.))
                .when(!vertical, |button| {
                    button
                        .size(theme::scaled_px(24.).max(px(22.)))
                        .bg(gpui_kit::transparent_black())
                        .hover(|style| style.bg(theme::foreground().opacity(0.08)))
                })
                .when(vertical, |button| {
                    button
                        .w_full()
                        .h(theme::scaled_px(28.).max(px(28.)))
                        .mt(px(4.))
                        .px(px(10.))
                        .rounded(px(5.))
                        .bg(gpui_kit::transparent_black())
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::muted_foreground())
                        .hover(|style| {
                            style
                                .bg(theme::foreground().opacity(0.06))
                                .text_color(theme::foreground())
                        })
                })
                .disabled(disabled)
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("新建 Intruder 攻击配置标签页")
                        .build(window, cx)
                })
                .child(gpui_kit::component::Icon::new(IconName::Plus).size(theme::scaled_px(14.)))
                .when(vertical, |button| button.child("新建攻击"))
                .on_click(cx.listener(|this, _, window, cx| this.new_intruder_tab(window, cx)))
                .into_any_element(),
        );
        if !vertical {
            items.push(
                div()
                    .ml_auto()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        Button::new("intruder-toggle-orientation")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .w(theme::scaled_px(22.).max(px(20.)))
                            .h(theme::scaled_px(22.).max(px(20.)))
                            .px(px(0.))
                            .icon(
                                gpui_kit::component::Icon::new(IconName::PanelLeft)
                                    .size(theme::scaled_px(14.)),
                            )
                            .accessibility_label("切换为垂直标签")
                            .tooltip("切换为左侧垂直侧栏标签布局")
                            .tab_stop(false)
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_intruder_orientation(Orientation::Vertical, window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        let width = self.workspace.intruder.tab_sidebar_width;
        let sidebar_w = px(if width == 0 { 180. } else { f32::from(width) });
        let strip_h = theme::scaled_px(40.).max(px(38.));
        let list = div()
            .id("intruder-work-tabs")
            .test_support()
            .flex()
            .flex_shrink_0()
            .min_w_0()
            .min_h_0()
            .bg(theme::toolbar_surface())
            .track_scroll(&self.intruder.tabs_scroll)
            .when(vertical, |this| {
                this.flex_col()
                    .w(sidebar_w)
                    .flex_1()
                    .px(px(6.))
                    .py(px(6.))
                    .gap(px(2.))
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .border_r_1()
                    .border_color(theme::border())
            })
            .when(!vertical, |this| {
                this.w_full()
                    .h(strip_h)
                    .items_center()
                    .px(px(8.))
                    .gap(px(5.))
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .border_b_1()
                    .border_color(theme::border())
            })
            .children(items)
            .into_any_element();
        if vertical {
            let vertical_header = div()
                .id("intruder-vertical-header")
                .test_support()
                .w(sidebar_w)
                .h(theme::scaled_px(34.).max(px(32.)))
                .px(px(8.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(4.))
                .bg(theme::toolbar_surface())
                .border_r_1()
                .border_b_1()
                .border_color(theme::border())
                .child(
                    div()
                        .text_size(theme::scaled_px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::muted_foreground())
                        .truncate()
                        .child(format!("攻击标签 ({total_tabs})")),
                )
                .child(
                    Button::new("intruder-toggle-orientation")
                        .ghost()
                        .small()
                        .rounded(ButtonRounded::Size(px(3.)))
                        .w(theme::scaled_px(22.).max(px(20.)))
                        .h(theme::scaled_px(22.).max(px(20.)))
                        .px(px(0.))
                        .icon(
                            gpui_kit::component::Icon::new(IconName::PanelTop)
                                .size(theme::scaled_px(14.)),
                        )
                        .accessibility_label("切换为水平标签")
                        .tooltip("切换回顶部横向标签栏布局")
                        .tab_stop(false)
                        .disabled(disabled)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_intruder_orientation(Orientation::Horizontal, window, cx);
                        })),
                );
            div()
                .relative()
                .w(sidebar_w)
                .h_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(vertical_header)
                .child(list)
                .when(
                    self.backend_available() && self.transition.is_none(),
                    |this| {
                        this.child(
                            resize_handle("intruder-work-tabs-resize", Axis::Horizontal)
                                .inside(HandleEdge::Trailing)
                                .on_drag(ResizeIntruderTabBar, |drag, _, _, cx| {
                                    cx.new(|_| drag.clone())
                                }),
                        )
                    },
                )
                .into_any_element()
        } else {
            list
        }
    }

    #[inline(never)]
    fn render_intruder_group_pill(
        &self,
        id: Uuid,
        title: &str,
        color: Option<u8>,
        collapsed: bool,
        vertical: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_rename = self
            .intruder
            .rename
            .as_ref()
            .filter(|rename| rename.is_group && rename.id == id);
        let group_rename_width = group_rename.filter(|_| !vertical).map(|rename| {
            let value = rename.input.read(cx).value();
            let run = window.text_style().to_run(value.len());
            let width = window
                .text_system()
                .shape_line(value, theme::ui_font_size(), &[run], None)
                .width;
            (width + theme::scaled_px(36.)).clamp(theme::scaled_px(64.), theme::scaled_px(220.))
        });
        let group_bg: gpui_kit::Hsla = if color.is_some() {
            crate::workbench::tab_groups::group_color(color)
        } else {
            rgb(if theme::is_dark() { 0x35383d } else { 0xe7e7e7 }).into()
        };
        let has_custom_color = color.is_some();
        let group_weak = cx.weak_entity();
        let pill_label = format!("{} {}", if collapsed { "▸" } else { "▾" }, title);
        div()
            .id(SharedString::from(format!("intruder-group-{id}")))
            .test_support()
            .px(px(9.))
            .min_h(theme::scaled_px(28.).max(px(26.)))
            .when(!vertical, |this| this.rounded(px(4.)))
            .when(vertical, |this| {
                this.w_full().rounded(px(5.)).px(px(8.)).mt(px(4.))
            })
            .when_some(group_rename_width, |this, width| this.w(width))
            .flex()
            .items_center()
            .flex_shrink_0()
            .bg(group_bg)
            .when(has_custom_color, |this| this.text_color(rgb(0x1f2328)))
            .text_size(theme::scaled_px(12.))
            .font_weight(if vertical {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .cursor_pointer()
            .child(match group_rename {
                Some(rename) => div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Input::new(&rename.input)
                            .small()
                            .appearance(false)
                            .px_0()
                            .aria_label("重命名 Intruder 分组"),
                    )
                    .into_any_element(),
                None => div().truncate().child(pill_label).into_any_element(),
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.transition.is_some()
                    || this
                        .intruder
                        .rename
                        .as_ref()
                        .is_some_and(|rename| rename.is_group && rename.id == id)
                {
                    return;
                }
                if let Some(group) = this
                    .workspace
                    .intruder
                    .groups
                    .iter_mut()
                    .find(|group| group.id == id)
                {
                    group.collapsed = !group.collapsed;
                    this.save_workspace(cx);
                    cx.notify();
                }
            }))
            .context_menu(move |menu, window, cx| {
                intruder_group_menu(group_weak.clone(), id, menu, window, cx)
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_tab(
        &self,
        tab: &IntruderTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = tab.id;
        let vertical = self.workspace.intruder.orientation == Orientation::Vertical;
        let selected = self.workspace.intruder.active == Some(id);
        let dark = theme::is_dark();
        let dirty = self
            .intruder
            .editors
            .get(&id)
            .is_some_and(|editors| editors.dirty);
        let title = format!("{}{}", tab.title, if dirty { " •" } else { "" });
        let rename = self
            .intruder
            .rename
            .as_ref()
            .filter(|rename| !rename.is_group && rename.id == id);
        let rename_width = rename.filter(|_| !vertical).map(|rename| {
            let value = rename.input.read(cx).value();
            let run = window.text_style().to_run(value.len());
            let width = window
                .text_system()
                .shape_line(value, theme::ui_font_size(), &[run], None)
                .width;
            (width + theme::scaled_px(40.)).clamp(theme::scaled_px(46.), theme::scaled_px(240.))
        });
        let vertical_active_bg: gpui_kit::Hsla =
            rgb(if dark { 0x2f333a } else { 0xe8eaed }).into();
        let tab_bg = if vertical {
            if selected {
                vertical_active_bg
            } else {
                gpui_kit::transparent_black()
            }
        } else if selected {
            rgb(if dark { 0x35383d } else { 0xeeeeee }).into()
        } else {
            theme::background()
        };
        let tab_hover_bg = if vertical {
            if selected {
                vertical_active_bg
            } else {
                theme::foreground().opacity(0.06)
            }
        } else if selected {
            rgb(if dark { 0x35383d } else { 0xeeeeee }).into()
        } else {
            theme::surface().opacity(0.85)
        };
        let weak = cx.weak_entity();
        div()
            .id(SharedString::from(format!("intruder-tab-{id}")))
            .test_support()
            .role(gpui_kit::accesskit::Role::Tab)
            .aria_label(tab.title.clone())
            .aria_selected(selected)
            .relative()
            .flex()
            .items_center()
            .h(theme::scaled_px(26.).max(px(26.)))
            .flex_shrink_0()
            .min_w(theme::scaled_px(58.))
            .px(px(8.))
            .gap(px(6.))
            .when(vertical, |this| {
                this.w_full()
                    .h(theme::scaled_px(30.).max(px(30.)))
                    .rounded(px(5.))
                    .px(theme::scaled_px(10.))
                    .border_b_1()
                    .border_color(gpui_kit::transparent_black())
                    .text_color(theme::foreground())
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
            })
            .when(!vertical, |this| {
                this.max_w(theme::scaled_px(240.))
                    .rounded(px(4.))
                    .overflow_hidden()
            })
            .when(!vertical && selected && !dark, |this| {
                this.text_color(rgb(0x000000))
            })
            .when_some(rename_width, |this, width| this.w(width))
            .bg(tab_bg)
            .hover(move |style| style.bg(tab_hover_bg))
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
            .child(match rename {
                Some(rename) => div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Input::new(&rename.input)
                            .small()
                            .appearance(false)
                            .px_0()
                            .aria_label("重命名 Intruder 标签"),
                    )
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .truncate()
                    .child(title.clone())
                    .into_any_element(),
            })
            .child(
                flat_button(
                    SharedString::from(format!("intruder-close-{id}")),
                    false,
                    window,
                    cx,
                )
                .accessibility_label("关闭 Intruder 标签")
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("关闭此 Intruder 标签页")
                        .build(window, cx)
                })
                .w(theme::scaled_px(14.).max(px(14.)))
                .h(theme::scaled_px(14.).max(px(14.)))
                .rounded(px(3.))
                .flex()
                .items_center()
                .justify_center()
                .hover(|style| style.bg(theme::foreground().opacity(0.12)))
                .text_size(theme::scaled_px(15.))
                .child("×")
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_intruder_tab(id, window, cx);
                })),
            )
            .when(selected && !vertical, |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(3.))
                        .bg(theme::accent()),
                )
            })
            .when(selected && vertical, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(5.))
                        .bottom(px(5.))
                        .left_0()
                        .w(px(3.))
                        .rounded_r(px(2.))
                        .bg(theme::accent()),
                )
            })
            .on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    if this
                        .intruder
                        .rename
                        .as_ref()
                        .is_some_and(|rename| !rename.is_group && rename.id == id)
                    {
                        return;
                    }
                    this.select_intruder_tab(id, window, cx);
                    if event.click_count() == 2 {
                        this.begin_intruder_rename(id, window, cx);
                    }
                }),
            )
            .on_drag(IntruderTabDrag { id, title }, |drag, _, _, cx| {
                cx.new(|_| drag.clone())
            })
            .on_drag_move::<IntruderTabDrag>(cx.listener(
                move |this, event: &DragMoveEvent<IntruderTabDrag>, _, cx| {
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
            .on_drop(
                cx.listener(move |this, drag: &IntruderTabDrag, window, cx| {
                    this.move_intruder_tab(drag.id, id, window, cx)
                }),
            )
            .context_menu(move |menu, window, cx| {
                intruder_tab_menu(weak.clone(), id, menu, window, cx)
            })
            .into_any_element()
    }
}

#[inline(never)]
fn intruder_tab_menu(
    view: WeakEntity<Workbench>,
    id: TabId,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let state = entity.read(cx);
    let workspace = &state.workspace.intruder;
    let visible: Vec<_> = workspace
        .tabs
        .iter()
        .filter(|tab| tab.group.is_none())
        .chain(
            workspace
                .groups
                .iter()
                .filter(|group| !group.collapsed)
                .flat_map(|group| {
                    workspace
                        .tabs
                        .iter()
                        .filter(move |tab| tab.group == Some(group.id))
                }),
        )
        .collect();
    let Some(index) = visible.iter().position(|tab| tab.id == id) else {
        return menu;
    };
    let left: Vec<_> = visible[..index].iter().map(|tab| tab.id).collect();
    let right: Vec<_> = visible[index + 1..].iter().map(|tab| tab.id).collect();
    let others: Vec<_> = workspace
        .tabs
        .iter()
        .filter(|tab| tab.id != id)
        .map(|tab| tab.id)
        .collect();
    let orientation = workspace.orientation;
    let can_restore = !workspace.closed.is_empty();
    let editable = state.backend_available() && state.transition.is_none();
    let grouped = visible[index].group.is_some();
    let current_group = visible[index].group;
    let groups = workspace.groups.clone();
    let rename_view = view.clone();
    let duplicate_view = view.clone();
    let group_view = view.clone();
    let ungroup_view = view.clone();
    let close_view = view.clone();
    let others_view = view.clone();
    let left_view = view.clone();
    let right_view = view.clone();
    let restore_view = view.clone();
    let layout_view = view.clone();
    menu.item(
        PopupMenuItem::new("重命名")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ =
                    rename_view.update(cx, |view, cx| view.begin_intruder_rename(id, window, cx));
            }),
    )
    .item(
        PopupMenuItem::new("关闭标签")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = close_view.update(cx, |view, cx| view.close_intruder_tab(id, window, cx));
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("关闭其它标签")
            .disabled(!editable || others.is_empty())
            .on_click(move |_, window, cx| {
                let _ = others_view
                    .update(cx, |view, cx| view.close_intruder_tabs(&others, window, cx));
            }),
    )
    .item(
        PopupMenuItem::new(if orientation == Orientation::Vertical {
            "关闭上方标签"
        } else {
            "关闭左侧标签"
        })
        .disabled(!editable || left.is_empty())
        .on_click(move |_, window, cx| {
            let _ = left_view.update(cx, |view, cx| view.close_intruder_tabs(&left, window, cx));
        }),
    )
    .item(
        PopupMenuItem::new(if orientation == Orientation::Vertical {
            "关闭下方标签"
        } else {
            "关闭右侧标签"
        })
        .disabled(!editable || right.is_empty())
        .on_click(move |_, window, cx| {
            let _ = right_view.update(cx, |view, cx| view.close_intruder_tabs(&right, window, cx));
        }),
    )
    .separator()
    .item(
        PopupMenuItem::new("重新打开已关闭标签")
            .disabled(!editable || !can_restore)
            .on_click(move |_, window, cx| {
                let _ = restore_view.update(cx, |view, cx| view.restore_intruder_tab(window, cx));
            }),
    )
    .separator()
    .submenu("标签视图设置", window, cx, move |menu, _, _| {
        [
            ("横向标签", Orientation::Horizontal),
            ("垂直标签", Orientation::Vertical),
        ]
        .into_iter()
        .fold(menu, |menu, (label, desired)| {
            let view = layout_view.clone();
            menu.item(
                PopupMenuItem::new(label)
                    .disabled(!editable)
                    .checked(orientation == desired)
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |view, cx| {
                            view.set_intruder_orientation(desired, window, cx)
                        });
                    }),
            )
        })
    })
    .separator()
    .item(
        PopupMenuItem::new("复制标签页 (Duplicate)")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = duplicate_view
                    .update(cx, |view, cx| view.duplicate_intruder_tab(id, window, cx));
            }),
    )
    .submenu("标签分组管理", window, cx, move |menu, _, _| {
        let create = group_view.clone();
        let menu = menu.item(
            PopupMenuItem::new("创建新分组")
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = create.update(cx, |view, cx| view.new_intruder_group(id, window, cx));
                }),
        );
        let menu = groups.iter().fold(menu, |menu, group| {
            let view = view.clone();
            let group_id = group.id;
            menu.item(
                PopupMenuItem::new(format!("移入分组：{}", group.title))
                    .disabled(!editable)
                    .checked(current_group == Some(group_id))
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |view, cx| {
                            view.move_intruder_to_group(id, Some(group_id), window, cx)
                        });
                    }),
            )
        });
        let remove = ungroup_view.clone();
        menu.when(grouped, |menu| {
            menu.separator().item(
                PopupMenuItem::new("移出当前分组")
                    .disabled(!editable)
                    .on_click(move |_, window, cx| {
                        let _ = remove.update(cx, |view, cx| {
                            view.move_intruder_to_group(id, None, window, cx)
                        });
                    }),
            )
        })
    })
}

#[inline(never)]
fn intruder_group_menu(
    view: WeakEntity<Workbench>,
    group_id: Uuid,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let state = entity.read(cx);
    let Some(group) = state
        .workspace
        .intruder
        .groups
        .iter()
        .find(|group| group.id == group_id)
    else {
        return menu;
    };
    let editable = state.backend_available() && state.transition.is_none();
    let current_color = group.color;
    let has_open_tabs = state
        .workspace
        .intruder
        .tabs
        .iter()
        .any(|tab| tab.group == Some(group_id));
    let rename_view = view.clone();
    let color_view = view.clone();
    let ungroup_view = view.clone();
    let close_view = view;
    menu.item(
        PopupMenuItem::new("重命名组")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = rename_view.update(cx, |view, cx| {
                    view.begin_intruder_group_rename(group_id, window, cx)
                });
            }),
    )
    .submenu("变更颜色", window, cx, move |menu, _, _| {
        let reset_view = color_view.clone();
        let menu = menu.item(
            PopupMenuItem::new("默认颜色")
                .disabled(!editable)
                .checked(current_color.is_none())
                .on_click(move |_, window, cx| {
                    let _ = reset_view.update(cx, |view, cx| {
                        view.set_intruder_group_color(group_id, None, window, cx)
                    });
                }),
        );
        INTRUDER_GROUP_PALETTE
            .iter()
            .fold(menu, |menu, &(code, hex, label)| {
                let view = color_view.clone();
                menu.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .size(px(12.))
                                    .rounded(px(2.))
                                    .bg(rgb(hex))
                                    .border_1()
                                    .border_color(theme::border()),
                            )
                            .child(label)
                    })
                    .disabled(!editable)
                    .checked(current_color == Some(code))
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |view, cx| {
                            view.set_intruder_group_color(group_id, Some(code), window, cx)
                        });
                    }),
                )
            })
    })
    .item(
        PopupMenuItem::new("取消分组")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = ungroup_view.update(cx, |view, cx| {
                    view.ungroup_intruder_tabs(group_id, window, cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("关闭组内所有标签")
            .disabled(!editable || !has_open_tabs)
            .on_click(move |_, window, cx| {
                let _ = close_view.update(cx, |view, cx| {
                    view.close_intruder_group_tabs(group_id, window, cx)
                });
            }),
    )
}
