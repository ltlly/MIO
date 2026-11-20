use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;

use super::*;

fn ordered_all_repeater_tab_ids(workspace: &Workspace) -> Vec<TabId> {
    let mut ids = Vec::with_capacity(workspace.tabs.len() + workspace.websocket_tabs.len());
    let mut seen_groups = HashSet::new();
    for tab in ordered_tabs(&workspace.tabs) {
        if let Some(group_id) = tab.group {
            if seen_groups.insert(group_id)
                && workspace.groups.iter().any(|group| group.id == group_id)
            {
                ids.extend(
                    workspace
                        .tabs
                        .iter()
                        .filter(|member| member.group == Some(group_id))
                        .map(|member| member.id),
                );
                ids.extend(
                    workspace
                        .websocket_tabs
                        .iter()
                        .filter(|member| !member.closed && member.group == Some(group_id))
                        .map(|member| member.id),
                );
            }
        } else {
            ids.push(tab.id);
        }
    }
    for group in workspace
        .groups
        .iter()
        .filter(|group| !seen_groups.contains(&group.id))
    {
        ids.extend(
            workspace
                .websocket_tabs
                .iter()
                .filter(|member| !member.closed && member.group == Some(group.id))
                .map(|member| member.id),
        );
    }
    ids.extend(
        workspace
            .websocket_tabs
            .iter()
            .filter(|tab| !tab.closed && tab.group.is_none())
            .map(|tab| tab.id),
    );
    ids
}

pub(super) fn ws_repeater_tab_menu(
    view: WeakEntity<Workbench>,
    id: TabId,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    ws_tab_menu(view, id, menu, window, cx)
}

pub(super) fn ws_tab_menu(
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
    let orientation = state.workspace.orientation;
    let can_restore = !state.workspace.closed.is_empty()
        || state.workspace.websocket_tabs.iter().any(|tab| tab.closed);
    let editable = state.backend_available() && state.transition.is_none();
    let groups = state.workspace.groups.clone();
    let current_group = state
        .workspace
        .websocket_tabs
        .iter()
        .find(|tab| !tab.closed && tab.id == id)
        .and_then(|tab| tab.group);
    let all_ordered = ordered_all_repeater_tab_ids(&state.workspace);
    let (left_ids, right_ids) = match all_ordered.iter().position(|tab_id| *tab_id == id) {
        Some(idx) => (all_ordered[..idx].to_vec(), all_ordered[idx + 1..].to_vec()),
        None => (Vec::new(), Vec::new()),
    };
    let other_http_ids: Vec<TabId> = state.workspace.tabs.iter().map(|tab| tab.id).collect();
    let other_ws_ids: Vec<TabId> = state
        .workspace
        .websocket_tabs
        .iter()
        .filter(|tab| !tab.closed && tab.id != id)
        .map(|tab| tab.id)
        .collect();
    let all_ws_ids: Vec<TabId> = state
        .workspace
        .websocket_tabs
        .iter()
        .filter(|tab| !tab.closed)
        .map(|tab| tab.id)
        .collect();
    let has_others = !other_http_ids.is_empty() || !other_ws_ids.is_empty();
    let rename_view = view.clone();
    let close_view = view.clone();
    let close_others_view = view.clone();
    let close_left_view = view.clone();
    let close_right_view = view.clone();
    let close_all_ws_view = view.clone();
    let group_menu_view = view.clone();
    let ungroup_view = view.clone();
    let restore_view = view.clone();
    let layout_view = view;
    menu.compact()
        .item(
            PopupMenuItem::new("重命名标签")
                .icon(IconName::Pencil)
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = rename_view.update(cx, |view, cx| {
                        view.begin_rename(RenameTarget::WebSocketTab(id), window, cx)
                    });
                }),
        )
        .item(
            PopupMenuItem::new("关闭标签")
                .icon(IconName::Close)
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = close_view.update(cx, |view, cx| {
                        view.close_websocket_repeater_tab(id, window, cx)
                    });
                }),
        )
        .item(
            PopupMenuItem::new("关闭其它标签")
                .icon(IconName::Close)
                .disabled(!editable || !has_others)
                .on_click(move |_, window, cx| {
                    let _ = close_others_view.update(cx, |view, cx| {
                        let mut ids = other_http_ids.clone();
                        ids.extend(other_ws_ids.iter().copied());
                        view.close_tabs(ids, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new(if orientation == Orientation::Vertical {
                "关闭上方标签"
            } else {
                "关闭左侧标签"
            })
            .disabled(!editable || left_ids.is_empty())
            .on_click(move |_, window, cx| {
                let _ = close_left_view.update(cx, |view, cx| {
                    view.close_tabs(left_ids.clone(), window, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new(if orientation == Orientation::Vertical {
                "关闭下方标签"
            } else {
                "关闭右侧标签"
            })
            .disabled(!editable || right_ids.is_empty())
            .on_click(move |_, window, cx| {
                let _ = close_right_view.update(cx, |view, cx| {
                    view.close_tabs(right_ids.clone(), window, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new("关闭全部 WebSocket 标签")
                .disabled(!editable || all_ws_ids.is_empty())
                .on_click(move |_, window, cx| {
                    let _ = close_all_ws_view.update(cx, |view, cx| {
                        view.close_tabs(all_ws_ids.clone(), window, cx);
                    });
                }),
        )
        .separator()
        .submenu_with_icon(
            Some(Icon::new(IconName::FolderPlus)),
            "添加到分组",
            window,
            cx,
            move |menu, _, _| {
                let create_group_view = group_menu_view.clone();
                let mut menu = menu.item(
                    PopupMenuItem::new("新建分组并移入")
                        .disabled(!editable)
                        .on_click(move |_, window, cx| {
                            let _ = create_group_view.update(cx, |view, cx| {
                                view.create_group_for_websocket_tab(id, window, cx);
                            });
                        }),
                );
                if !groups.is_empty() {
                    menu = menu.separator();
                }
                for group in &groups {
                    let assign_view = group_menu_view.clone();
                    let group_id = group.id;
                    menu = menu.item(
                        PopupMenuItem::new(group.title.clone())
                            .disabled(!editable)
                            .checked(current_group == Some(group_id))
                            .on_click(move |_, window, cx| {
                                let _ = assign_view.update(cx, |view, cx| {
                                    view.move_websocket_tab_to_group(
                                        id,
                                        Some(group_id),
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    );
                }
                let remove_view = ungroup_view.clone();
                menu.when(current_group.is_some(), |menu| {
                    menu.separator().item(
                        PopupMenuItem::new("移出分组")
                            .disabled(!editable)
                            .on_click(move |_, window, cx| {
                                let _ = remove_view.update(cx, |view, cx| {
                                    view.move_websocket_tab_to_group(id, None, window, cx);
                                });
                            }),
                    )
                })
            },
        )
        .item(
            PopupMenuItem::new("重新打开已关闭标签")
                .icon(IconName::RotateCcw)
                .disabled(!editable || !can_restore)
                .on_click(move |_, window, cx| {
                    let _ = restore_view.update(cx, |view, cx| view.restore_tab(window, cx));
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
                                if view.workspace.orientation != desired {
                                    view.toggle_orientation(window, cx);
                                }
                            });
                        }),
                )
            })
        })
}

pub(super) fn repeater_tab_menu(
    view: WeakEntity<Workbench>,
    id: TabId,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    tab_menu(view, id, menu, window, cx)
}

pub(super) fn tab_menu(
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
    let tabs = ordered_tabs(&state.workspace.tabs);
    let Some(index) = tabs.iter().position(|tab| tab.id == id) else {
        return menu;
    };
    let groups = state.workspace.groups.clone();
    let left: Vec<_> = tabs[..index].iter().map(|tab| tab.id).collect();
    let right: Vec<_> = tabs[index + 1..].iter().map(|tab| tab.id).collect();
    let others: Vec<_> = left.iter().chain(&right).copied().collect();
    let current_group = tabs[index].group;
    let grouped = current_group.is_some();
    let orientation = state.workspace.orientation;
    let can_restore = !state.workspace.closed.is_empty()
        || state.workspace.websocket_tabs.iter().any(|tab| tab.closed);
    let editable = state.backend_available() && state.transition.is_none();
    let rename_view = view.clone();
    let group_view = view.clone();
    let ungroup_view = view.clone();
    let close_view = view.clone();
    let others_view = view.clone();
    let left_view = view.clone();
    let right_view = view.clone();
    let restore_view = view.clone();
    let layout_view = view.clone();
    menu.compact()
        .item(
            PopupMenuItem::new("重命名标签")
                .icon(IconName::Pencil)
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = rename_view.update(cx, |view, cx| {
                        view.begin_rename(RenameTarget::Tab(id), window, cx)
                    });
                }),
        )
        .item(
            PopupMenuItem::new("关闭标签")
                .icon(IconName::Close)
                .disabled(!editable)
                .on_click(move |_, window, cx| {
                    let _ = close_view.update(cx, |view, cx| view.close_tab(id, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("关闭其它标签")
                .icon(IconName::Close)
                .disabled(!editable || others.is_empty())
                .on_click(move |_, window, cx| {
                    let _ = others_view
                        .update(cx, |view, cx| view.close_tabs(others.clone(), window, cx));
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
                let _ = left_view.update(cx, |view, cx| view.close_tabs(left.clone(), window, cx));
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
                let _ =
                    right_view.update(cx, |view, cx| view.close_tabs(right.clone(), window, cx));
            }),
        )
        .separator()
        .submenu_with_icon(
            Some(Icon::new(IconName::FolderPlus)),
            "添加到组",
            window,
            cx,
            move |menu, _, _| {
                let create = group_view.clone();
                let menu = menu.item(PopupMenuItem::new("新建组").disabled(!editable).on_click(
                    move |_, window, cx| {
                        let _ = create.update(cx, |view, cx| view.new_group(id, window, cx));
                    },
                ));
                let menu = groups.iter().fold(menu, |menu, group| {
                    let view = view.clone();
                    let group_id = group.id;
                    menu.item(
                        PopupMenuItem::new(group.title.clone())
                            .disabled(!editable)
                            .checked(current_group == Some(group_id))
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.move_to_group(id, Some(group_id), window, cx)
                                });
                            }),
                    )
                });
                let remove = ungroup_view.clone();
                menu.when(grouped, |menu| {
                    menu.separator()
                        .item(PopupMenuItem::new("移出组").disabled(!editable).on_click(
                            move |_, window, cx| {
                                let _ = remove.update(cx, |view, cx| {
                                    view.move_to_group(id, None, window, cx)
                                });
                            },
                        ))
                })
            },
        )
        .item(
            PopupMenuItem::new("重新打开已关闭标签")
                .icon(IconName::RotateCcw)
                .disabled(!editable || !can_restore)
                .on_click(move |_, window, cx| {
                    let _ = restore_view.update(cx, |view, cx| view.restore_tab(window, cx));
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
                                if view.workspace.orientation != desired {
                                    view.toggle_orientation(window, cx);
                                }
                            });
                        }),
                )
            })
        })
}

pub(in crate::workbench) fn repeater_group_menu(
    id: Uuid,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    owner: WeakEntity<Workbench>,
) -> PopupMenu {
    let editable = owner.upgrade().is_some_and(|owner| {
        let view = owner.read(cx);
        view.backend_available() && view.transition.is_none()
    });
    let can_send = owner
        .upgrade()
        .is_some_and(|owner| owner.read(cx).can_send_group(id));
    let orientation = owner
        .upgrade()
        .map(|owner| owner.read(cx).workspace.orientation);
    let send_all = owner.clone();
    let send_sequential = owner.clone();
    let duplicate = owner.clone();
    let edit = owner.clone();
    let ungroup = owner.clone();
    let close = owner.clone();
    let layout = owner;
    menu.compact()
        .item(
            PopupMenuItem::new("发送本组请求")
            .disabled(!can_send)
            .on_click(move |_, window, cx| {
                let _ = send_all.update(cx, |view, cx| view.send_group(id, window, cx));
            }),
    )
    .item(
        PopupMenuItem::new("顺序发送本组全部请求 (Sequential)")
            .disabled(!can_send)
            .on_click(move |_, window, cx| {
                let _ = send_sequential.update(cx, |view, cx| {
                    view.send_group_sequential(id, window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("复制本组标签")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ =
                    duplicate.update(cx, |view, cx| view.duplicate_repeater_group(id, window, cx));
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("编辑组")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = edit.update(cx, |view, cx| {
                    view.open_tab_group_editor(Some(id), None, window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("取消标签分组")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = ungroup.update(cx, |view, cx| view.ungroup_repeater_tabs(id, window, cx));
            }),
    )
    .item(
        PopupMenuItem::new("关闭组内所有标签")
            .disabled(!editable)
            .on_click(move |_, window, cx| {
                let _ = close.update(cx, |view, cx| {
                    let mut ids: Vec<TabId> = view
                        .workspace
                        .tabs
                        .iter()
                        .filter(|tab| tab.group == Some(id))
                        .map(|tab| tab.id)
                        .collect();
                    ids.extend(
                        view.workspace
                            .websocket_tabs
                            .iter()
                            .filter(|tab| !tab.closed && tab.group == Some(id))
                            .map(|tab| tab.id),
                    );
                    view.close_tabs(ids, window, cx);
                });
            }),
    )
    .submenu("标签视图设置", window, cx, move |menu, _, _| {
        [
            ("横向标签", Orientation::Horizontal),
            ("垂直标签", Orientation::Vertical),
        ]
        .into_iter()
        .fold(menu, |menu, (label, desired)| {
            let owner = layout.clone();
            menu.item(
                PopupMenuItem::new(label)
                    .disabled(!editable)
                    .checked(orientation == Some(desired))
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            if view.workspace.orientation != desired {
                                view.toggle_orientation(window, cx);
                            }
                        });
                    }),
            )
        })
    })
}
