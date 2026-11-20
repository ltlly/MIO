use super::*;

impl GroupEditor {
    pub(super) fn new(
        (owner, owner_window): (Entity<Workbench>, AnyWindowHandle),
        project: ProjectKey,
        group: TabGroup,
        (new, initial): (bool, Option<TabId>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let base_groups = owner.read(cx).workspace.groups.clone();
        let tabs = owner
            .read(cx)
            .workspace
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.title.clone(), tab.group))
            .collect::<Vec<_>>();
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale().max(1.0);
        let mut target_w = px((413. * scale).round());
        let mut target_h = px((425. * scale).round());
        if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds();
            target_w = target_w.min(visible.size.width);
            target_h = target_h.min(visible.size.height);
        }
        window.resize(size(
            (target_w - (frame.width - viewport.width)).max(px(300.)),
            (target_h - (frame.height - viewport.height)).max(px(240.)),
        ));
        let name = cx.new(|cx| InputState::new(window, cx).default_value(group.title.clone()));
        name.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        let selected = tabs
            .iter()
            .filter(|(id, _, member)| {
                if new {
                    Some(*id) == initial
                } else {
                    *member == Some(group.id)
                }
            })
            .map(|(id, _, _)| *id)
            .collect();
        let base_members = tabs.iter().map(|(id, _, group)| (*id, *group)).collect();
        let tabs = tabs
            .into_iter()
            .filter(|(id, _, member)| {
                member.is_none() || *member == Some(group.id) || new && Some(*id) == initial
            })
            .map(|(id, title, _)| (id, title))
            .collect();
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            if view.project != Some(this.project) {
                window.remove_window();
                return;
            }
            this.available = view.backend_available()
                && view.transition.is_none()
                && view.pending_workspace.is_none();
            cx.notify();
        });
        let handle = window.window_handle();
        let released = owner.downgrade();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.update(cx, |view, _| {
                    if view.tab_group_dialog == Some(handle) {
                        view.tab_group_dialog = None;
                    }
                });
            });
        })
        .detach();
        window.set_window_title(if new { "新建组" } else { "编辑组" });
        Self {
            owner: owner.downgrade(),
            owner_window,
            project,
            group,
            base_groups,
            base_members,
            tabs,
            selected,
            name,
            new,
            submitting: false,
            pending: None,
            available: owner.read(cx).backend_available()
                && owner.read(cx).pending_workspace.is_none(),
            error: None,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available || self.pending.is_some() || self.submitting || self.selected.is_empty()
        {
            return;
        }
        let title = match editing::input_snapshot(&self.name, window, cx) {
            Ok(title) => title.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if title.is_empty() || title.len() > 4096 {
            self.error = Some("组名称须为 1–4096 字节；当前输入保留。".into());
            cx.notify();
            return;
        }
        self.group.title = title;
        let name_revision = self.name.read(cx).document_revision();
        let group = self.group.clone();
        let selected = self.selected.clone();
        let groups = self.base_groups.clone();
        let members = self.base_members.clone();
        let project = self.project;
        let new = self.new;
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let dialog = cx.weak_entity();
        let dialog_window = window.window_handle();
        self.submitting = true;
        cx.notify();
        cx.defer(move |cx| {
            if !cx.windows().contains(&dialog_window) {
                return;
            }
            let mut applied_metadata = None;
            let result = owner_window.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| -> Result<u64, String> {
                    if view.project != Some(project)
                        || !view.backend_available()
                        || view.transition.is_some()
                        || view.pending_workspace.is_some()
                    {
                        return Err("项目状态已改变；分组草稿仍保留。".into());
                    }
                    let current_members = view
                        .workspace
                        .tabs
                        .iter()
                        .map(|tab| (tab.id, tab.group))
                        .collect::<Vec<_>>();
                    if view.workspace.groups != groups || current_members != members {
                        return Err("标签或分组已被其它操作修改；未覆盖当前工作区。".into());
                    }
                    let changed = view.capture_drafts(window, cx)?;
                    view.workspace_dirty |= changed;
                    let mut workspace = view.workspace.clone();
                    if let Some(existing) = workspace
                        .groups
                        .iter_mut()
                        .find(|existing| existing.id == group.id)
                    {
                        *existing = group.clone();
                    } else if new {
                        workspace.groups.push(group.clone());
                    } else {
                        return Err("原分组已不存在；未新建替代分组。".into());
                    }
                    for tab in &mut workspace.tabs {
                        if selected.contains(&tab.id) {
                            tab.group = Some(group.id);
                        } else if tab.group == Some(group.id) {
                            tab.group = None;
                        }
                    }
                    workspace
                        .validate_structure()
                        .map_err(|error| error.to_string())?;
                    applied_metadata = Some((
                        workspace.groups.clone(),
                        workspace
                            .tabs
                            .iter()
                            .map(|tab| (tab.id, tab.group))
                            .collect(),
                    ));
                    view.workspace = workspace;
                    if !view.save_workspace(cx) {
                        return Err(view
                            .error
                            .clone()
                            .unwrap_or_else(|| "分组保存未入队；输入保留。".into()));
                    }
                    view.pending_workspace
                        .ok_or_else(|| "分组保存尚未提交；输入保留。".into())
                })
            });
            let _ = dialog.update(cx, |this, cx| {
                this.submitting = false;
                if let Some((groups, members)) = applied_metadata {
                    this.base_groups = groups;
                    this.base_members = members;
                }
                match result {
                    Ok(Ok(Ok(revision))) => {
                        this.pending = Some((revision, name_revision));
                        this.error = None;
                    }
                    Ok(Ok(Err(error))) => this.error = Some(error),
                    _ => this.error = Some("项目窗口已关闭；未提交分组。".into()),
                };
                cx.notify();
            });
        });
    }
}

impl Render for GroupEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-group-editor",
            if self.new { "新建组" } else { "编辑组" },
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let enabled = self.available && !self.submitting && self.pending.is_none();
        let all_selected =
            !self.tabs.is_empty() && self.tabs.iter().all(|(id, _)| self.selected.contains(id));
        let mut members = div()
            .id("group-members")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(6.))
            .py(px(8.))
            .gap(px(2.));
        for (id, title) in &self.tabs {
            let id = *id;
            let owner = cx.weak_entity();
            members = members.child(
                intruder::resource_pools::pool_checkbox(
                    SharedString::from(format!("group-member-{id}")),
                    title.clone(),
                    self.selected.contains(&id),
                    enabled,
                )
                .on_change(move |state, _, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        if state == gpui_kit::base::CheckboxState::Checked {
                            view.selected.insert(id);
                        } else {
                            view.selected.remove(&id);
                        }
                        cx.notify();
                    });
                }),
            );
        }
        let mut colors = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme::scaled_px(6.))
            .min_h(theme::scaled_px(30.).max(px(30.)))
            .px(theme::scaled_px(16.))
            .py(px(4.))
            .border_t_1()
            .border_b_1()
            .border_color(theme::border())
            .flex_shrink_0();
        for &(value, color, name) in PALETTE {
            let owner = cx.weak_entity();
            let selected = self.group.color == Some(value);
            let color_tip = SharedString::from(format!("将标签组颜色设为{name}"));
            colors = colors.child(
                gpui_kit::base::Radio::new(("group-color", value as usize))
                    .checked(selected)
                    .disabled(!enabled)
                    .accessibility_label(name)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(color_tip.clone())
                            .build(window, cx)
                    })
                    .size(theme::scaled_px(16.))
                    .rounded_full()
                    .border_1()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::control_border()
                    })
                    .bg(rgb(color))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |this| {
                        this.child(
                            div()
                                .size(theme::scaled_px(8.))
                                .rounded_full()
                                .bg(rgb(0x1f2328)),
                        )
                    })
                    .on_change(move |_, _, _, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            view.group.color = Some(value);
                            cx.notify();
                        });
                    }),
            );
        }
        let content = div()
            .id("group-editor")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .child(
                div()
                    .px(theme::scaled_px(10.))
                    .py(theme::scaled_px(10.))
                    .flex()
                    .flex_col()
                    .gap(theme::scaled_px(10.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme::border())
                    .child("组名称：")
                    .child(Input::new(&self.name).small().disabled(!enabled)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_h(theme::scaled_px(36.).max(px(36.)))
                    .flex_shrink_0()
                    .px(theme::scaled_px(10.))
                    .py(px(4.))
                    .border_b_1()
                    .border_color(theme::border())
                    .child("将标签添加到组：")
                    .child(div().flex_1())
                    .child(
                        button(
                            "group-select-all",
                            if all_selected {
                                "取消全选"
                            } else {
                                "全选"
                            },
                        )
                        .link()
                        .px(px(0.))
                        .underline()
                        .text_color(rgb(0xb6531b))
                        .icon(gpui_kit::assets::IconName::ListChecks)
                        .tooltip(if all_selected {
                            "取消勾选列表中的全部标签页"
                        } else {
                            "一次性勾选列表中的全部可用标签页"
                        })
                        .disabled(!enabled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.tabs.is_empty()
                                && this.tabs.iter().all(|(id, _)| this.selected.contains(id))
                            {
                                this.selected.clear();
                            } else {
                                this.selected = this.tabs.iter().map(|(id, _)| *id).collect();
                            }
                            cx.notify();
                        })),
                    ),
            )
            .child(members)
            .child(colors)
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("group-editor-error")
                        .px(theme::scaled_px(10.))
                        .text_color(theme::danger_fg())
                        .line_clamp(2)
                        .max_h(theme::scaled_px(48.))
                        .overflow_y_scroll()
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(theme::scaled_px(6.))
                    .min_h(theme::scaled_px(44.).max(px(44.)))
                    .px(theme::scaled_px(10.))
                    .py(px(6.))
                    .flex_shrink_0()
                    .child(
                        button("group-cancel", "取消")
                            .icon(gpui_kit::assets::IconName::Close)
                            .tooltip("放弃当前分组更改并关闭窗口")
                            .min_w(theme::scaled_px(84.))
                            .disabled(self.pending.is_some())
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        button("group-submit", if self.new { "创建" } else { "保存" })
                            .primary()
                            .icon(gpui_kit::assets::IconName::Check)
                            .tooltip(if self.new {
                                "创建新标签分组并将所选标签移入该组"
                            } else {
                                "保存分组名称、颜色与成员变更"
                            })
                            .min_w(theme::scaled_px(72.))
                            .disabled(!enabled || self.selected.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::foreground())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(content))
    }
}
