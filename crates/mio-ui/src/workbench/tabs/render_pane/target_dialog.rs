use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::{AnyWindowHandle, Bounds, WeakEntity, size};

use super::*;

pub(in crate::workbench) struct RepeaterTargetDialog {
    owner: WeakEntity<Workbench>,
    owner_window: AnyWindowHandle,
    project: ProjectKey,
    pub(in crate::workbench) tab_id: TabId,
    pub(in crate::workbench) target: Entity<InputState>,
    pub(in crate::workbench) base_target: String,
    sync_host: bool,
    pending: Option<(u64, String, u64)>,
    submitting: bool,
    available: bool,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RepeaterTargetDialog {
    fn new(
        owner: Entity<Workbench>,
        owner_window: AnyWindowHandle,
        project: ProjectKey,
        tab_id: TabId,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                crate::workbench::wb_repeater::format_repeater_target_display(&initial),
            )
        });
        let observation = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            if view.project != Some(this.project) {
                window.remove_window();
                return;
            }
            this.available = view.backend_available()
                && view.transition.is_none()
                && view.workspace.tabs.iter().any(|tab| tab.id == this.tab_id)
                && !view.sending.contains_key(&this.tab_id)
                && !view.pending_sends.contains_key(&this.tab_id)
                && !view.plugin_waiting(this.tab_id)
                && !view.has_active_sequential_group_queue();
            if let Some((revision, submitted, input_revision)) = this.pending.clone() {
                if view.pending_workspace.is_some_and(|current| current < revision) {
                    cx.notify();
                    return;
                }
                if view.pending_workspace == Some(revision) {
                    cx.notify();
                    return;
                }
                let project_matches = view.project == Some(this.project) && view.backend_available();
                let tab_has_submitted = view.workspace.tabs.iter().any(|tab| {
                    tab.id == this.tab_id && tab.draft.target == submitted
                });
                let next_pending = view.pending_workspace;
                let ws_revision = view.workspace.revision;
                let ws_dirty = view.workspace_dirty;
                let view_error = view.error.clone();
                if project_matches && tab_has_submitted {
                    if next_pending.is_some_and(|next| next > revision) {
                        this.pending = next_pending.map(|next| {
                            (next, submitted.clone(), input_revision)
                        });
                    } else if ws_revision >= revision && !ws_dirty {
                        this.pending = None;
                        this.base_target = submitted.clone();
                        let input_unchanged =
                            this.target.read(cx).document_revision() == input_revision
                                && editing::input_snapshot(&this.target, window, cx).is_ok_and(
                                    |target| {
                                        crate::workbench::wb_repeater::normalize_repeater_target_input(
                                            target.as_ref(),
                                            &this.base_target,
                                        )
                                        .is_ok_and(|target| target == submitted)
                                    },
                                );
                        if input_unchanged {
                            window.remove_window();
                            return;
                        }
                        this.error = Some("目标已保存；后续输入尚未保存。".into());
                    } else {
                        this.pending = None;
                        this.error = Some(view_error.unwrap_or_else(|| {
                            "目标保存尚未确认；编辑内容保留。".into()
                        }));
                    }
                } else {
                    this.pending = None;
                    this.error = Some(view_error.unwrap_or_else(|| {
                        "目标保存尚未确认；编辑内容保留。".into()
                    }));
                }
            }
            cx.notify();
        });
        let entered = cx.subscribe_in(&target, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        });
        let notify_owner = owner.downgrade();
        let mut composing = false;
        let ime_observer = cx.observe_in(&target, window, move |_, input, window, cx| {
            let marked = input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            });
            if marked != composing {
                composing = marked;
                let _ = notify_owner.update(cx, |_, cx| cx.notify());
            }
        });
        let change_owner = owner.downgrade();
        let changed = cx.subscribe(&target, move |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                let _ = change_owner.update(cx, |_, cx| cx.notify());
            }
        });
        target.update(cx, |target, cx| {
            target.select_all(window, cx);
            target.focus(window, cx);
        });
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.can_close(window, cx)).unwrap_or(true)
        });
        window.set_window_title("Edit target");
        Self {
            owner: owner.downgrade(),
            owner_window,
            project,
            tab_id,
            target,
            base_target: initial,
            sync_host: owner.read(cx).repeater_target_sync_host,
            pending: None,
            submitting: false,
            available: true,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: vec![observation, entered, ime_observer, changed],
        }
    }

    fn can_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Err(error) = editing::input_snapshot(&self.target, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return false;
        }
        !self.submitting
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_close(window, cx) {
            window.remove_window();
        }
    }

    fn fill_from_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available || self.submitting || self.pending.is_some() {
            return;
        }
        let current = match editing::input_snapshot(&self.target, window, cx) {
            Ok(current) => current,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let use_https = url::Url::parse(current.as_ref())
            .is_ok_and(|current| current.scheme() == "https");
        let result = self.owner_window.update(cx, |_, owner_window, cx| {
            self.owner.update(cx, |view, cx| -> Result<String, String> {
                if view.project != Some(self.project) || view.transition.is_some() {
                    return Err("项目状态已改变；目标编辑内容保留。".into());
                }
                let editors = view.tab_editors.get(&self.tab_id)
                    .ok_or_else(|| "目标标签页已关闭。".to_owned())?;
                let raw = editors.request.bytes(owner_window, cx)?;
                crate::workbench::wb_repeater::extract_origin_from_request_host_header(
                    &raw, use_https,
                )
            })
        });
        match result {
            Ok(Ok(Ok(origin))) => {
                self.target.update(cx, |state, cx| state.set_value(origin, window, cx));
                self.error = None;
            }
            Ok(Ok(Err(error))) => self.error = Some(error),
            _ => self.error = Some("项目窗口已关闭；目标编辑内容保留。".into()),
        }
        cx.notify();
    }

    fn toggle_scheme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available || self.submitting || self.pending.is_some() {
            return;
        }
        let result = editing::input_snapshot(&self.target, window, cx)
            .map_err(str::to_owned)
            .and_then(|target| {
                crate::workbench::wb_repeater::normalize_repeater_target_input(
                    target.as_ref(),
                    &self.base_target,
                )
            })
            .and_then(|target| {
                crate::workbench::wb_repeater::toggle_target_scheme_candidate(&target)
                    .ok_or_else(|| "连接目标无法切换 HTTP/HTTPS 协议。".to_owned())
            });
        match result {
            Ok(target) => {
                self.target.update(cx, |state, cx| state.set_value(target, window, cx));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available || self.submitting || self.pending.is_some() {
            return;
        }
        let target = match editing::input_snapshot(&self.target, window, cx)
            .map_err(str::to_owned)
            .and_then(|target| {
                crate::workbench::wb_repeater::normalize_repeater_target_input(
                    target.as_ref(),
                    &self.base_target,
                )
            })
        {
            Ok(target) => target,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let project = self.project;
        let tab_id = self.tab_id;
        let base_target = self.base_target.clone();
        let sync_host = self.sync_host;
        let input_revision = self.target.read(cx).document_revision();
        let dialog = cx.weak_entity();
        let handle = window.window_handle();
        self.submitting = true;
        cx.notify();
        cx.defer(move |cx| {
            if !cx.windows().contains(&handle) {
                return;
            }
            let result = owner_window.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| -> Result<Option<u64>, String> {
                    if view.project != Some(project)
                        || !view.backend_available()
                        || view.transition.is_some()
                        || view.sending.contains_key(&tab_id)
                        || view.pending_sends.contains_key(&tab_id)
                        || view.plugin_waiting(tab_id)
                        || view.has_active_sequential_group_queue()
                    {
                        return Err("当前目标暂不可保存；编辑内容保留。".into());
                    }
                    let current = view.workspace.tabs.iter().find(|tab| tab.id == tab_id)
                        .ok_or_else(|| "目标标签页已关闭。".to_owned())?;
                    if current.draft.target != base_target && current.draft.target != target {
                        return Err("目标已由其他操作修改；编辑内容保留。".into());
                    }
                    let editors = view.tab_editors.get(&tab_id)
                        .ok_or_else(|| "目标编辑器已关闭。".to_owned())?;
                    let native_target = editing::input_snapshot(&editors.target, window, cx)
                        .map_err(str::to_owned)?;
                    if native_target.as_ref() != base_target && native_target.as_ref() != target {
                        return Err("连接目标存在其他未保存输入；编辑内容保留。".into());
                    }
                    let raw = editors.request.bytes(window, cx)?;
                    let next_raw = if sync_host {
                        if view.extensions.active_messages.contains_key(&tab_id) {
                            return Err("请先切换到原生请求编辑器再更新 Host 标头。".into());
                        }
                        let host = crate::workbench::wb_repeater::format_host_header_for_origin(&target)
                            .ok_or_else(|| "连接目标的 Host 标头无效。".to_owned())?;
                        crate::workbench::wb_repeater::rewrite_request_host_header(&raw, &host)?
                    } else {
                        raw
                    };
                    let changed = view.capture_drafts(window, cx)?;
                    view.workspace_dirty |= changed;
                    let current = view.workspace.tabs.iter().find(|tab| tab.id == tab_id)
                        .expect("目标标签页存在");
                    if current.draft.target != base_target && current.draft.target != target {
                        return Err("目标已由其他操作修改；编辑内容保留。".into());
                    }
                    if current.draft.target != target || current.draft.raw != next_raw {
                        let next_revision = current.draft.revision + 1;
                        let tab = view.workspace.tabs.iter_mut().find(|tab| tab.id == tab_id)
                            .expect("目标标签页存在");
                        tab.draft.target = target.clone();
                        tab.draft.raw = next_raw.clone();
                        tab.draft.revision = next_revision;
                        let editors = view.tab_editors.get_mut(&tab_id)
                            .expect("目标编辑器存在");
                        editors.target.update(cx, |state, cx| {
                            state.set_value(target.clone(), window, cx);
                        });
                        if editors.request.bytes(window, cx)? != next_raw {
                            let was_pretty = editors.request.pretty_active();
                            let previous_view = editors.request.view();
                            editors.request.load(&next_raw, window, cx);
                            if was_pretty {
                                editors.request.switch_pretty(window, cx)?;
                            } else {
                                editors.request.switch(previous_view, window, cx)?;
                            }
                        }
                        editors.loaded_revision = next_revision;
                        editors.dirty = false;
                        view.workspace_dirty = true;
                    }
                    if view.workspace_dirty && !view.save_workspace(cx) {
                        return Err(view.error.clone().unwrap_or_else(|| "目标保存未提交。".into()));
                    }
                    view.status = format!("Repeater 目标：{target}");
                    cx.notify();
                    view.repeater_target_sync_host = sync_host;
                    Ok(view.pending_workspace)
                })
            });
            let _ = handle.update(cx, |_, window, cx| {
                let _ = dialog.update(cx, |this, cx| {
                    this.submitting = false;
                    match result {
                        Ok(Ok(Ok(Some(revision)))) => {
                            this.base_target = target.clone();
                            this.pending = Some((revision, target, input_revision));
                            this.error = None;
                        }
                        Ok(Ok(Ok(None))) => {
                            window.remove_window();
                        }
                        Ok(Ok(Err(error))) => this.error = Some(error),
                        _ => this.error = Some("项目窗口已关闭；编辑内容保留。".into()),
                    }
                    cx.notify();
                });
            });
            let _ = owner_window.update(cx, |_, _, cx| {
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
        });
    }
}

impl Render for RepeaterTargetDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close_entity = cx.weak_entity();
        let title_bar = crate::chrome::render_title_bar(
            "mio-repeater-target", "Edit target", crate::chrome::TitleBarControls::DIALOG,
            move |window, cx| {
                let _ = close_entity.update(cx, |this, cx| this.cancel(window, cx));
            },
            window, cx,
        );
        div().size_full().flex().flex_col().bg(theme::background())
            .text_color(theme::foreground()).child(title_bar)
            .child(div().id("repeater-target-dialog").test_support()
                .role(gpui_kit::accesskit::Role::Dialog).aria_label("Edit target")
                .key_context("MioRepeaterTarget")
                .track_focus(&self.focus).flex_1().min_h_0().p(px(14.))
                .flex().flex_col().gap(px(10.))
                .on_action(cx.listener(|this, _: &crate::CloseSettings, window, cx| {
                    this.cancel(window, cx);
                }))
                .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child("Target:")
                .child(Input::new(&self.target).id("repeater-target-input")
                    .aria_label("连接目标").small())
                .child(div().flex().items_center().gap(px(8.))
                    .child(button("repeater-target-toggle-scheme", "切换 HTTP / HTTPS")
                        .disabled(!self.available || self.submitting || self.pending.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_scheme(window, cx))))
                    .child(button("repeater-target-update-host", "同步更新请求 Host 标头")
                        .selected(self.sync_host).toggled(self.sync_host)
                        .disabled(!self.available || self.submitting || self.pending.is_some())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sync_host = !this.sync_host;
                            cx.notify();
                        }))))
                .child(div().id("repeater-target-status").test_support()
                    .role(gpui_kit::accesskit::Role::Status).flex_1().min_h_0()
                    .text_color(theme::danger_fg())
                    .aria_label(self.error.clone().unwrap_or_else(|| {
                        if self.pending.is_some() { "正在保存目标…".into() } else { String::new() }
                    }))
                    .child(self.error.clone().unwrap_or_else(|| {
                        if self.pending.is_some() { "正在保存目标…".into() } else { String::new() }
                    })))
                .child(div().flex().items_center().gap(px(8.))
                    .child(
                        button("repeater-target-sync-host", "从请求 Host 填充")
                            .disabled(!self.available || self.submitting || self.pending.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.fill_from_host(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(button("repeater-target-ok", "OK").primary()
                        .disabled(!self.available || self.submitting || self.pending.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))))
                    .child(button("repeater-target-cancel", "Cancel")
                        .tooltip("关闭目标编辑窗口")
                        .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))))))
    }
}

impl Workbench {
    pub(super) fn render_repeater_target_helper(
        &self,
        tab_id: TabId,
        _editors: &TabEditors,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let raw_target = self.workspace.tabs.iter().find(|tab| tab.id == tab_id)
            .map_or("", |tab| tab.draft.target.as_str());
        let display_target = crate::workbench::wb_repeater::format_repeater_target_display(raw_target);
        let label = format!("Target: {display_target}");
        let edit_focus = window
            .use_keyed_state("repeater-edit-target-focus", cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let click_focus = edit_focus.clone();
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(9.))
            .child(
                Button::new("repeater-target-label")
                    .ghost()
                    .small()
                    .h(px(24.))
                    .p_0()
                    .min_w_0()
                    .accessibility_label(label.clone())
                    .tab_stop(false)
                    .tooltip("编辑连接目标")
                    .disabled(!enabled)
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme::ui_font_size())
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(if !enabled {
                                theme::muted_foreground()
                            } else if theme::is_dark() {
                                theme::foreground()
                            } else {
                                rgb(0x141414).into()
                            })
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_repeater_target_editor(tab_id, window, cx);
                    })),
            )
            .child(
                Button::new("repeater-edit-target")
                    .ghost()
                    .small()
                    .w(px(22.))
                    .h(px(24.))
                    .p_0()
                    .flex_shrink_0()
                    .track_focus(&edit_focus)
                    .accessibility_label("Edit target")
                    .tooltip("编辑连接目标")
                    .disabled(!enabled)
                    .child(
                        gpui_kit::component::Icon::new(IconName::Pencil)
                            .size(px(19.))
                            .text_color(if enabled {
                                theme::foreground()
                            } else {
                                theme::muted_foreground()
                            }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        click_focus.focus(window, cx);
                        this.open_repeater_target_editor(tab_id, window, cx);
                    })),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn validate_repeater_target_dialog_composition(
        &self,
        cx: &App,
    ) -> Result<(), &'static str> {
        let Some(project) = self.project else {
            return Ok(());
        };
        let owner_id = self.owner_window;
        let dialogs = {
            let mut list = self.repeater_ui_caches.open_target_dialogs.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.clone()
        };
        for weak in dialogs {
            if let Some(dialog) = weak.upgrade() {
                let d = dialog.read(cx);
                if d.project == project
                    && d.owner_window == owner_id
                    && d.target.read(cx).has_marked_text()
                {
                    return Err(editing::COMPOSITION_PENDING);
                }
            }
        }
        Ok(())
    }

    pub(in crate::workbench) fn repeater_target_dialog_unsaved(&self, cx: &App) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        let owner_id = self.owner_window;
        let dialogs = {
            let mut list = self.repeater_ui_caches.open_target_dialogs.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.clone()
        };
        dialogs.into_iter().any(|weak| {
            let Some(dialog) = weak.upgrade() else {
                return false;
            };
            let d = dialog.read(cx);
            if d.project != project || d.owner_window != owner_id {
                return false;
            }
            let current = d.target.read(cx).value();
            let base_display =
                crate::workbench::wb_repeater::format_repeater_target_display(&d.base_target);
            d.target.read(cx).has_marked_text()
                || d.pending.is_some()
                || d.submitting
                || (current.as_ref() != d.base_target && current.as_ref() != base_display)
        })
    }

    #[cfg(test)]
    pub(in crate::workbench) fn open_repeater_target_dialog_for_tab(
        &self,
        tab_id: TabId,
        cx: &App,
    ) -> Option<Entity<RepeaterTargetDialog>> {
        let project = self.project?;
        let owner_id = self.owner_window;
        let dialogs = {
            let mut list = self.repeater_ui_caches.open_target_dialogs.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.clone()
        };
        dialogs.into_iter().find_map(|weak| {
            let dialog = weak.upgrade()?;
            let d = dialog.read(cx);
            (d.project == project && d.owner_window == owner_id && d.tab_id == tab_id)
                .then_some(dialog)
        })
    }

    pub(in crate::workbench) fn open_repeater_target_editor(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some()
            || self.sending.contains_key(&tab_id) || self.pending_sends.contains_key(&tab_id)
            || self.plugin_waiting(tab_id) || self.has_active_sequential_group_queue()
            || self.file_picker_pending()
        {
            return;
        }
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == tab_id) else { return };
        let initial = tab.draft.target.clone();
        let owner = cx.entity();
        let owner_window = window.window_handle();
        let project = self.project.expect("目标编辑项目存在");
        let content_size = size(theme::scaled_px(560.).max(px(560.)), theme::scaled_px(240.).max(px(240.)));
        let bounds = Bounds::centered_at(window.window_bounds().get_bounds().center(), content_size);
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            for handle in cx.windows() {
                if handle == owner_window { continue; }
                let found = handle.update(cx, |_, window, cx| {
                    let editor = window.root::<gpui_kit::base::Root>()
                        .flatten().and_then(|root| root.read(cx).view().clone()
                            .downcast::<RepeaterTargetDialog>().ok());
                    if editor.is_some_and(|editor| {
                        editor.read(cx).owner.entity_id() == owner.downgrade().entity_id()
                            && editor.read(cx).tab_id == tab_id
                    }) {
                        window.activate_window();
                        cx.activate(true);
                        true
                    } else { false }
                }).unwrap_or(false);
                if found { return; }
            }
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| cx.new(|cx| RepeaterTargetDialog::new(
                    owner.clone(), owner_window, project, tab_id, initial, window, cx,
                )),
            );
            match opened {
                Ok((_, dialog)) => {
                    let weak = dialog.downgrade();
                    owner.update(cx, |view, _| {
                        let mut list = view.repeater_ui_caches.open_target_dialogs.borrow_mut();
                        list.retain(|entry| entry.upgrade().is_some());
                        list.push(weak);
                    });
                }
                Err(error) => {
                    owner.update(cx, |view, cx| view.fail(format!("目标窗口打开失败：{error}"), cx));
                }
            }
        });
    }
}
