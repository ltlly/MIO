use super::*;

impl Workbench {
    pub(in crate::workbench) fn open_tab_group_editor(
        &mut self,
        group_id: Option<Uuid>,
        tab_id: Option<TabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(project) = self.project else { return };
        let owner = cx.entity();
        let owner_window = window.window_handle();
        let scale = theme::ui_scale().max(1.0);
        let target_w = px((413. * scale).round());
        let content_h = px((425. * scale).round());
        let desired_h = if cfg!(test) {
            content_h
        } else {
            content_h + px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
        };
        let bounds = if let Some(display) = window.display(cx).or_else(|| cx.primary_display()) {
            let visible = display.visible_bounds();
            let fitted = size(
                target_w.min(visible.size.width),
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
            Bounds::centered(None, size(target_w, desired_h), cx)
        };
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            if let Some(existing) = owner.read(cx).tab_group_dialog
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let workspace = &owner.read(cx).workspace;
            let group = match group_id {
                Some(id) => {
                    let Some(group) = workspace.groups.iter().find(|group| group.id == id) else {
                        return;
                    };
                    group.clone()
                }
                None => {
                    let mut number = 1;
                    while workspace
                        .groups
                        .iter()
                        .any(|group| group.title == format!("Group {number}"))
                    {
                        number += 1;
                    }
                    TabGroup {
                        id: Uuid::new_v4(),
                        title: format!("Group {number}"),
                        collapsed: false,
                        color: Some(7),
                    }
                }
            };
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        GroupEditor::new(
                            (owner.clone(), owner_window),
                            project,
                            group,
                            (group_id.is_none(), tab_id),
                            window,
                            cx,
                        )
                    })
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((window, _)) => view.tab_group_dialog = Some(window),
                Err(error) => view.fail(format!("无法打开分组编辑窗口：{error}"), cx),
            });
        });
    }

    pub(in crate::workbench) fn tab_groups_workspace_acknowledged(
        &mut self,
        saved: &Workspace,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.tab_group_dialog else {
            return;
        };
        let saved_revision = saved.revision;
        let groups = saved.groups.clone();
        let members = editor::group_member_states(saved);
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let Some(editor) = group_editor(window, cx) else {
                    return;
                };
                editor.update(cx, |editor, cx| {
                    let Some((revision, name_revision)) = editor.pending else {
                        return;
                    };
                    if revision != saved_revision {
                        return;
                    }
                    editor.pending = None;
                    if groups.iter().find(|group| group.id == editor.group.id)
                        == Some(&editor.group)
                        && members.iter().all(|(id, group)| {
                            editor.selected.contains(id) == (*group == Some(editor.group.id))
                        })
                    {
                        editor.base_groups = groups;
                        editor.base_members = members;
                        editor.new = false;
                        let unchanged = editor.name.read(cx).document_revision() == name_revision
                            && editing::input_snapshot(&editor.name, window, cx)
                                .is_ok_and(|name| name.as_ref() == editor.group.title);
                        if unchanged {
                            window.remove_window();
                        } else {
                            editor.error =
                                Some("已保存提交的分组；后续输入仍保留，请再次保存或取消。".into());
                            window.set_window_title("编辑组");
                            cx.notify();
                        }
                    } else {
                        editor.error = Some("保存回执与分组成员或颜色不一致；编辑仍保留。".into());
                        cx.notify();
                    }
                });
            });
        });
    }

    pub(in crate::workbench) fn tab_groups_save_failed(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.tab_group_dialog else {
            return;
        };
        let message = message.to_owned();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                if let Some(editor) = group_editor(window, cx) {
                    editor.update(cx, |editor, cx| {
                        if editor.pending.take().is_some() {
                            editor.error = Some(message);
                            cx.notify();
                        }
                    });
                }
            });
        });
    }

    pub(in crate::workbench) fn ungroup_repeater_tabs(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        for tab in self
            .workspace
            .tabs
            .iter_mut()
            .chain(&mut self.workspace.closed)
        {
            if tab.group == Some(id) {
                tab.group = None;
            }
        }
        for tab in &mut self.workspace.websocket_tabs {
            if tab.group == Some(id) {
                tab.group = None;
            }
        }
        self.workspace.groups.retain(|group| group.id != id);
        self.save_workspace(cx);
        cx.notify();
    }
}

#[allow(dead_code)]
pub(in crate::workbench) fn group_menu(
    id: Uuid,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    owner: WeakEntity<Workbench>,
) -> PopupMenu {
    super::tabs::menus::repeater_group_menu(id, menu, window, cx, owner)
}

pub(super) fn group_editor(window: &Window, cx: &App) -> Option<Entity<GroupEditor>> {
    window
        .root::<gpui_kit::base::Root>()??
        .read(cx)
        .view()
        .clone()
        .downcast::<GroupEditor>()
        .ok()
}
