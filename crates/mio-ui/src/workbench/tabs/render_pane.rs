use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;
use gpui_kit::{AnyWindowHandle, Bounds, WeakEntity, size};
use mio_core::RepeaterHistoryPage;

use super::*;

mod drawers;
mod layouts;

gpui_kit::actions!(mio_repeater_pane, [SaveRepeaterEntireHistory]);

const REPEATER_HISTORY_PAGE_SIZE: usize = 64;

type RepeaterRedirectCacheKey = (
    gpui_kit::EntityId,
    TabId,
    crate::workbench::inspector::MessageEditorRevKey,
    HttpVersion,
);

std::thread_local! {
    static OPEN_REPEATER_TARGET_DIALOGS: std::cell::RefCell<Vec<gpui_kit::WeakEntity<RepeaterTargetDialog>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static REPEATER_REDIRECT_CACHE: std::cell::RefCell<Option<(RepeaterRedirectCacheKey, bool)>> =
        const { std::cell::RefCell::new(None) };
    static REPEATER_RESPONSE_LEN_CACHE: std::cell::RefCell<
        Option<(crate::workbench::inspector::MessageEditorRevKey, usize)>,
    > = const { std::cell::RefCell::new(None) };
    #[cfg(test)]
    static AUTO_OPEN_REPEATER_INSPECTOR_IN_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

fn cached_message_byte_len(editor: &MessageEditor, cx: &App) -> usize {
    let text_len = if editor.pretty_active || editor.view == MessageView::Raw {
        editor.raw.read(cx).text().len()
    } else {
        editor.hex.read(cx).text().len()
    };
    if text_len == 0 {
        return 0;
    }
    let key = crate::workbench::inspector::message_editor_rev_key(editor, cx);
    if let Some(len) = REPEATER_RESPONSE_LEN_CACHE.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|(cached_key, _)| *cached_key == key)
            .map(|(_, len)| *len)
    }) {
        return len;
    }
    let exact_len = editor.bytes_lossless(cx).len();
    REPEATER_RESPONSE_LEN_CACHE.with(|cache| {
        *cache.borrow_mut() = Some((key, exact_len));
    });
    exact_len
}

#[cfg(test)]
pub(super) fn set_auto_open_repeater_inspector_in_test(enabled: bool) {
    AUTO_OPEN_REPEATER_INSPECTOR_IN_TEST.with(|cell| cell.set(enabled));
}

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
            this.available = view.project == Some(this.project)
                && view.backend_available()
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
                                        crate::workbench::wb_repeater::validate_repeater_target_input(
                                            target.as_ref(),
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
        let self_weak = cx.weak_entity();
        OPEN_REPEATER_TARGET_DIALOGS.with(|slot| {
            let mut list = slot.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.push(self_weak);
        });
        Self {
            owner: owner.downgrade(),
            owner_window,
            project,
            tab_id,
            target,
            base_target: initial,
            sync_host: crate::workbench::wb_repeater::repeater_target_sync_host_enabled(),
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
                crate::workbench::wb_repeater::validate_repeater_target_input(target.as_ref())
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
                crate::workbench::wb_repeater::validate_repeater_target_input(target.as_ref())
            })
        {
            Ok(target) => target,
            Err(error) => {
                self.error = Some(error.into());
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
                        let next_revision = current.draft.revision.checked_add(1)
                            .ok_or_else(|| "草稿 revision 已耗尽。".to_owned())?;
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
                            let was_pretty = editors.request.pretty_active;
                            let previous_view = editors.request.view;
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
                    crate::workbench::wb_repeater::set_repeater_target_sync_host(sync_host);
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum RepeaterCombinedPane {
    #[default]
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::workbench) struct CombinedTabState {
    pub(super) pane: RepeaterCombinedPane,
    pub(super) observed_response_revision: Option<u64>,
}

impl Workbench {
    #[inline(never)]
    fn render_repeater_toolbar_left(
        &self,
        id: TabId,
        busy: bool,
        group_id: Option<TabId>,
        can_follow_redirect: bool,
        can_configure_settings: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(if busy {
                Button::new("repeater-cancel")
                    .small()
                    .rounded(ButtonRounded::Size(px(3.)))
                    .w(px(65.))
                    .h(px(24.))
                    .px(px(8.))
                    .bg(theme::surface())
                    .border_color(theme::control_border())
                    .text_color(theme::foreground())
                    .accessibility_label("Cancel")
                    .tooltip("取消当前正在进行中的网络连接与请求传输")
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Cancel"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            } else {
                Button::new("repeater-send")
                    .primary()
                    .small()
                    .rounded(ButtonRounded::Size(px(3.)))
                    .w(px(65.))
                    .h(px(24.))
                    .px(px(8.))
                    .bg(rgb(0xdb612f))
                    .text_color(rgb(0xffffff))
                    .accessibility_label("Send")
                    .tooltip(if cfg!(target_os = "macos") {
                        "向目标服务器发送当前草稿请求并接收响应（⌘Enter）"
                    } else {
                        "向目标服务器发送当前草稿请求并接收响应（Ctrl+Enter）"
                    })
                    .disabled(!self.can_send(id))
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Send"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
            })
            .child(self.render_repeater_settings_menu(can_configure_settings, cx))
            .when_some(group_id, |bar, gid| {
                let owner = cx.weak_entity();
                let can_send_current = self.can_send(id);
                let can_send_group = self.can_send_group(gid);
                bar.child(
                    Button::new("repeater-send-mode-menu")
                        .small()
                        .rounded(ButtonRounded::Size(px(3.)))
                        .h(px(24.))
                        .w(px(24.))
                        .px(px(0.))
                        .bg(rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }))
                        .tab_stop(false)
                        .accessibility_label("发送模式菜单")
                        .tooltip("选择单次发送、并行齐射本组或顺序逐个发送本组请求")
                        .disabled(!can_send_current && !can_send_group)
                        .child(
                            gpui_kit::component::Icon::new(
                                gpui_kit::component::IconName::ChevronDown,
                            )
                            .size(theme::scaled_px(12.)),
                        )
                        .dropdown_menu(move |menu, _, cx| {
                            let Some(entity) = owner.upgrade() else {
                                return menu;
                            };
                            let view = entity.read(cx);
                            let can_single = view.can_send(id);
                            let can_group = view.can_send_group(gid);
                            let send_one = owner.clone();
                            let send_seq_single = owner.clone();
                            let send_seq_sep = owner.clone();
                            let send_par_spa = owner.clone();
                            let send_par_sep = owner.clone();
                            let send_par = owner.clone();
                            let send_seq = owner.clone();
                            menu.item(
                                PopupMenuItem::new("发送当前请求")
                                    .disabled(!can_single)
                                    .on_click(move |_, window, cx| {
                                        let _ = send_one.update(cx, |this, cx| {
                                            this.send(window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("Send group in sequence (single connection)")
                                    .disabled(!can_group)
                                    .on_click(move |_, window, cx| {
                                        let _ = send_seq_single.update(cx, |this, cx| {
                                            this.send_group_sequential_single_connection(
                                                gid, window, cx,
                                            );
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(
                                    "Send group in sequence (separate connections)",
                                )
                                .disabled(!can_group)
                                .on_click(move |_, window, cx| {
                                    let _ = send_seq_sep.update(cx, |this, cx| {
                                        this.send_group_sequential_separate_connections(
                                            gid, window, cx,
                                        );
                                    });
                                }),
                            )
                            .item(
                                PopupMenuItem::new(
                                    "Send group in parallel (single-packet attack)",
                                )
                                .disabled(!can_group)
                                .on_click(move |_, window, cx| {
                                    let _ = send_par_spa.update(cx, |this, cx| {
                                        this.send_group_single_packet_parallel(gid, window, cx);
                                    });
                                }),
                            )
                            .item(
                                PopupMenuItem::new(
                                    "Send group in parallel (separate connections)",
                                )
                                .disabled(!can_group)
                                .on_click(move |_, window, cx| {
                                    let _ = send_par_sep.update(cx, |this, cx| {
                                        this.send_group_parallel_separate_connections(
                                            gid, window, cx,
                                        );
                                    });
                                }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("并行发送本组全部请求 (Parallel)")
                                    .disabled(!can_group)
                                    .on_click(move |_, window, cx| {
                                        let _ = send_par.update(cx, |this, cx| {
                                            this.send_group(gid, window, cx);
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("顺序发送本组全部请求 (Sequential)")
                                    .disabled(!can_group)
                                    .on_click(move |_, window, cx| {
                                        let _ = send_seq.update(cx, |this, cx| {
                                            this.send_group_sequential(gid, window, cx);
                                        });
                                    }),
                            )
                        }),
                )
            })
            .when(can_follow_redirect, |bar| {
                let follow_disabled = busy
                    || !self.can_send(id)
                    || self.extensions.active_messages.contains_key(&id)
                    || !self.backend_available()
                    || self.transition.is_some();
                bar.child(
                    Button::new("repeater-follow-redirection")
                        .icon(IconName::CornerDownRight)
                        .label("跟随重定向")
                        .small()
                        .rounded(ButtonRounded::Size(px(3.)))
                        .h(px(24.))
                        .px(theme::scaled_px(10.))
                        .tab_stop(false)
                        .tooltip("根据响应头 Location 解析下一跳目标并自动更新请求后发送")
                        .disabled(follow_disabled)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.follow_repeater_redirection(window, cx)
                        })),
                )
            })
            .child(self.render_repeater_toolbar_history_controls(id, cx))
            .child(
                div()
                    .mx(px(3.))
                    .w(px(1.))
                    .h(px(20.))
                    .flex_shrink_0()
                    .bg(theme::border()),
            )
            .child(
                Button::new("repeater-mio-ai")
                    .primary()
                    .small()
                    .rounded(ButtonRounded::Size(px(3.)))
                    .w(px(104.))
                    .h(px(24.))
                    .px(px(8.))
                    .bg(rgb(0x4946ae))
                    .hover(|s| s.bg(rgb(0x3f3d9a)))
                    .text_color(rgb(0xffffff))
                    .tab_stop(false)
                    .accessibility_label("MioAI")
                    .tooltip("打开 MioAI / Agent IPC 助手控制中心")
                    .disabled(self.ext_import.visible)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.))
                            .child(
                                gpui_kit::component::Icon::new(IconName::Sparkles)
                                    .size(px(14.))
                                    .text_color(rgb(0xffffff)),
                            )
                            .child(
                                div()
                                    .text_size(theme::ui_font_size())
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_color(rgb(0xffffff))
                                    .child("MioAI"),
                            ),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_active_repeater_in_mio_ai(window, cx);
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_toolbar_right(
        &self,
        id: TabId,
        editors: &TabEditors,
        can_edit_target: bool,
        protocol_label: &'static str,
        protocol_disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .ml_auto()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .child(self.render_repeater_target_helper(id, editors, can_edit_target, window, cx))
            .child(div().w(px(1.)).h(px(24.)).bg(theme::border()))
            .child(
                Button::new("repeater-protocol-toggle")
                    .ghost()
                    .small()
                    .h(px(24.))
                    .p_0()
                    .flex_shrink_0()
                    .accessibility_label(protocol_label)
                    .tab_stop(false)
                    .tooltip("The request will be sent using this HTTP protocol.\nThis can also be changed using the inspector.")
                    .disabled(protocol_disabled)
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(if protocol_disabled {
                                theme::muted_foreground()
                            } else {
                                theme::foreground()
                            })
                            .child(protocol_label),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_repeater_protocol(id, window, cx);
                    })),
            )
            .child(
                Popover::new("repeater-help-popover")
                    .anchor(gpui_kit::Anchor::BottomRight)
                    .trigger(
                        Button::new("repeater-help")
                            .ghost()
                            .small()
                            .w(px(22.))
                            .h(px(24.))
                            .p_0()
                            .accessibility_label("Help")
                            .tooltip("查看 Repeater 使用说明")
                            .child(
                                gpui_kit::component::Icon::new(IconName::CircleQuestionMark)
                                    .size(px(19.))
                                    .text_color(theme::foreground()),
                            ),
                    )
                    .content(|_, _, _| {
                        div()
                            .id("repeater-help-content")
                            .test_support()
                            .w(px(288.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child("Repeater")
                            .child("Send 发送当前请求；Cancel 取消当前传输。")
                            .child("点击目标旁的铅笔可以编辑连接目标。")
                            .child("HTTP 协议版本可在请求属性中修改。")
                            .child("左侧设置菜单提供 Host 同步及传输设置。")
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_footer(
        &self,
        id: TabId,
        left_status: String,
        footer_stats: String,
        stale: bool,
        has_response_revision: bool,
    ) -> AnyElement {
        div()
            .w_full()
            .h(px(28.))
            .px(px(5.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(theme::toolbar_surface())
            .border_t_1()
            .border_color(theme::border())
            .child(left_status)
            .when(stale && has_response_revision, |this| {
                this.child(
                    div().text_color(theme::muted_foreground()).child(
                        if self
                            .workspace
                            .tabs
                            .iter()
                            .any(|tab| tab.id == id && tab.archive_response.is_some())
                        {
                            "（归档响应对应载入时的请求）"
                        } else {
                            "（响应对应上次发送的草稿）"
                        },
                    ),
                )
            })
            .when(!footer_stats.is_empty(), |this| {
                this.child(
                    div()
                        .id("repeater-footer-stats")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Status)
                        .aria_label(footer_stats.clone())
                        .ml_auto()
                        .text_color(theme::foreground())
                        .child(footer_stats),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let new_disabled =
            self.project.is_none() || self.transition.is_some() || !self.backend_available();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .bg(theme::surface())
            .child(
                div()
                    .text_size(theme::scaled_px(15.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme::foreground())
                    .child("Repeater 尚无工作标签"),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::muted_foreground())
                    .child("从 HTTP 历史记录发送消息到 Repeater，或新建请求。"),
            )
            .child(
                div().pt(px(4.)).child(
                    Button::new("repeater-create-first")
                        .primary()
                        .small()
                        .rounded(ButtonRounded::Size(px(3.)))
                        .h(px(28.))
                        .px(px(12.))
                        .bg(rgb(0xdb612f))
                        .text_color(rgb(0xffffff))
                        .accessibility_label("新建请求")
                        .tooltip(if cfg!(target_os = "macos") {
                            "新建空白 HTTP 请求标签页（⌘T）"
                        } else {
                            "新建空白 HTTP 请求标签页（Ctrl+T）"
                        })
                        .disabled(new_disabled)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(theme::ui_font_size())
                                .font_weight(gpui_kit::FontWeight::BOLD)
                                .child(
                                    gpui_kit::component::Icon::new(IconName::Plus)
                                        .size(px(14.))
                                        .text_color(rgb(0xffffff)),
                                )
                                .child("新建请求"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
                ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_rename_bar(
        &self,
        rename: &tabs::Rename,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        toolbar()
            .child("名称：")
            .child(
                div()
                    .w(theme::scaled_px(300.))
                    .min_w(theme::scaled_px(120.))
                    .flex_shrink(1.0)
                    .child(Input::new(&rename.input).small()),
            )
            .child(
                button("rename-apply", "确定")
                    .primary()
                    .icon(IconName::Check)
                    .tooltip("保存当前标签页的新名称")
                    .on_click(cx.listener(|this, _, window, cx| this.apply_rename(window, cx))),
            )
            .child(
                button("rename-cancel", "取消")
                    .icon(IconName::Close)
                    .tooltip("放弃重命名并保持原标签名")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rename = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vertical = self.workspace.orientation == Orientation::Vertical;
        let tab_bar = self.render_tab_bar(window, cx);
        let active_ws = self
            .workspace
            .active_websocket_tab
            .filter(|_| self.workspace.active.is_none())
            .filter(|id| {
                self.workspace
                    .websocket_tabs
                    .iter()
                    .any(|tab| !tab.closed && tab.id == *id)
            });
        if let Some(active_id) = self.workspace.active
            && !self.tab_editors.contains_key(&active_id)
        {
            self.ensure_tab_editor(active_id, window, cx);
        }
        let active = self
            .workspace
            .active
            .and_then(|id| self.tab_editors.get(&id).map(|editors| (id, editors)));
        #[cfg(test)]
        let should_auto_open_inspector =
            AUTO_OPEN_REPEATER_INSPECTOR_IN_TEST.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let should_auto_open_inspector = true;
        if should_auto_open_inspector
            && self.project.is_some()
            && (active.is_some() || active_ws.is_some())
        {
            let auto_opened =
                window.use_keyed_state("repeater-inspector-default-opened", cx, |_, _| false);
            if !*auto_opened.read(cx) {
                auto_opened.update(cx, |opened, _| *opened = true);
                if !self.inspector.open
                    && !self.inspector.tab_notes_open
                    && !self.repeater_custom_actions_open
                    && !self.repeater_explanations_open
                    && self.inspector.dismissed_selection.is_none()
                {
                    let entity = cx.entity();
                    let handle = window.window_handle();
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            entity.update(cx, |view, cx| {
                                if view.tool == Tool::Repeater
                                    && !view.inspector.open
                                    && !view.inspector.tab_notes_open
                                    && !view.repeater_custom_actions_open
                                    && !view.repeater_explanations_open
                                    && view.inspector.dismissed_selection.is_none()
                                {
                                    view.open_inspector(window, cx);
                                    cx.notify();
                                }
                            });
                        });
                    });
                }
            }
        }
        let editor = if let Some(ws_id) = active_ws {
            self.render_websocket_repeater_editor(ws_id, window, cx)
        } else if let Some((id, _)) = active {
            let waiting = self.pending_sends.contains_key(&id);
            let busy = self.sending.contains_key(&id)
                || waiting
                || self.plugin_waiting(id)
                || self.has_active_sequential_group_queue();
            let editors = &self.tab_editors[&id];
            let tab_meta = self.workspace.tabs.iter().find(|tab| tab.id == id);
            let can_follow_redirect = tab_meta.is_some_and(|tab| {
                let resp_key =
                    crate::workbench::inspector::message_editor_rev_key(&editors.response, cx);
                let cache_key = (cx.entity_id(), tab.id, resp_key, tab.draft.version);
                if let Some(cached) = REPEATER_REDIRECT_CACHE.with(|cache| {
                    cache
                        .borrow()
                        .as_ref()
                        .filter(|(k, _)| *k == cache_key)
                        .map(|(_, v)| *v)
                }) {
                    return cached;
                }
                let result = self.can_follow_repeater_redirect(tab, window, cx);
                REPEATER_REDIRECT_CACHE.with(|cache| {
                    *cache.borrow_mut() = Some((cache_key, result));
                });
                result
            });
            let revision = tab_meta.map_or(0, |tab| tab.draft.revision);
            let group_id = tab_meta.and_then(|tab| tab.group);
            let protocol_label = tab_meta.map_or("HTTP/1.1", |tab| tab.draft.version.as_str());
            let can_edit_target = self.project.is_some()
                && self.backend_available()
                && self.transition.is_none()
                && !busy;
            let can_configure_settings =
                self.project.is_some() && self.backend_available() && self.transition.is_none();
            let result = editors.result.clone().unwrap_or_else(|| {
                if waiting {
                    "等待草稿保存…".into()
                } else if busy {
                    "正在发送…".into()
                } else {
                    "尚未发送".into()
                }
            });
            let stale = editors.dirty
                || editors
                    .response_revision
                    .is_some_and(|sent| sent != revision);
            let has_response_revision = editors.response_revision.is_some();
            let footer_stats = editors
                .loaded_flow
                .as_ref()
                .map(|flow| {
                    let resp_bytes = if let Some(response) = flow.response.as_ref() {
                        crate::workbench::wb_repeater::raw_response_byte_len(response) as u64
                    } else {
                        let loaded_len = cached_message_byte_len(&editors.response, cx) as u64;
                        if loaded_len > 0 {
                            loaded_len
                        } else {
                            flow.summary.response_bytes
                        }
                    };
                    format!("{resp_bytes} bytes | {} millis", flow.summary.elapsed_ms)
                })
                .unwrap_or_else(|| {
                    let resp_len = cached_message_byte_len(&editors.response, cx);
                    if resp_len > 0 {
                        format!("{resp_len} bytes | 0 millis")
                    } else {
                        String::new()
                    }
                });
            let left_status = if !footer_stats.is_empty() {
                result
                    .split(" · ")
                    .next()
                    .unwrap_or(result.as_str())
                    .to_owned()
            } else {
                result
            };
            let protocol_disabled = self.project.is_none()
                || !self.backend_available()
                || self.transition.is_some()
                || busy
                || self.extensions.active_messages.contains_key(&id);
            let toolbar_el = div()
                .w_full()
                .h(px(44.))
                .px(px(10.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(5.))
                .bg(theme::toolbar_surface())
                .border_b_1()
                .border_color(theme::border())
                .child(self.render_repeater_toolbar_left(
                    id,
                    busy,
                    group_id,
                    can_follow_redirect,
                    can_configure_settings,
                    cx,
                ))
                .child(self.render_repeater_toolbar_right(
                    id,
                    editors,
                    can_edit_target,
                    protocol_label,
                    protocol_disabled,
                    window,
                    cx,
                ));
            let pane_body = if self.extensions.active_messages.contains_key(&id)
                || self.extensions.active_responses.contains_key(&id)
            {
                self.render_repeater_messages(id, editors, cx)
            } else {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .when_some(self.render_repeater_plugin_tabs(id, cx).0, |col, tabs| {
                        col.child(tabs)
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_repeater_native_pane(id, editors, window, cx)),
                    )
                    .into_any_element()
            };
            let dock_el = self.render_repeater_inspector_dock(pane_body, window, cx);
            let footer_el = self.render_repeater_footer(
                id,
                left_status,
                footer_stats,
                stale,
                has_response_revision,
            );
            div()
                .key_context("MioRepeater")
                .on_action(cx.listener(|this, _: &SaveRepeaterEntireHistory, window, cx| {
                    this.save_repeater_entire_history(window, cx);
                }))
                .size_full()
                .flex()
                .flex_col()
                .child(toolbar_el)
                .when_some(self.render_archive_response_error(id, cx), |this, error| {
                    this.child(error)
                })
                .child(div().flex_1().min_h_0().child(dock_el))
                .child(footer_el)
                .into_any_element()
        } else {
            self.render_repeater_empty_state(cx)
        };
        let rename_bar = self
            .rename
            .as_ref()
            .map(|rename| self.render_repeater_rename_bar(rename, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && cx.has_active_drag() {
                        cx.stop_active_drag(window);
                        this.drop_target = None;
                        if let Some(width) = this.resize_original.take() {
                            this.tab_width = width;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_drag_move::<ResizeTabBar>(cx.listener(
                |this, event: &DragMoveEvent<ResizeTabBar>, _, cx| {
                    this.resize_original.get_or_insert(this.tab_width);
                    this.tab_width = (event.event.position.x - event.bounds.origin.x)
                        .as_f32()
                        .clamp(120., 640.);
                    cx.notify();
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resize_original.take().is_some() {
                        this.save_workspace(cx);
                    }
                    this.drop_target = None;
                    cx.notify();
                }),
            )
            .when_some(rename_bar, |this, bar| this.child(bar))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!vertical, |this| this.flex_col())
                    .child(tab_bar)
                    .child(div().flex_1().min_w_0().min_h_0().child(editor)),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_target_helper(
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
        let dialogs = OPEN_REPEATER_TARGET_DIALOGS.with(|slot| {
            let mut list = slot.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.clone()
        });
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
        let dialogs = OPEN_REPEATER_TARGET_DIALOGS.with(|slot| {
            let mut list = slot.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.clone()
        });
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
        let dialogs = OPEN_REPEATER_TARGET_DIALOGS.with(|slot| {
            let mut list = slot.borrow_mut();
            list.retain(|entry| entry.upgrade().is_some());
            list.clone()
        });
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
            if let Err(error) = opened {
                owner.update(cx, |view, cx| view.fail(format!("目标窗口打开失败：{error}"), cx));
            }
        });
    }

    #[inline(never)]
    fn render_repeater_settings_menu(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        Button::new("repeater-settings-menu")
            .small()
            .rounded(ButtonRounded::Size(px(3.)))
            .w(px(24.))
            .h(px(24.))
            .p_0()
            .bg(rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }))
            .border_color(theme::control_border())
            .tab_stop(false)
            .accessibility_label("Repeater 快捷设置")
            .tooltip("配置 Repeater 标头规范化、响应解包与重定向策略")
            .disabled(!enabled)
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(px(17.))
                    .text_color(if enabled {
                        theme::foreground()
                    } else {
                        theme::muted_foreground()
                    }),
            )
            .dropdown_menu(move |menu, _, cx| {
                let Some(entity) = owner.upgrade() else {
                    return menu;
                };
                let view = entity.read(cx);
                let project = view.project;
                let editable =
                    project.is_some() && view.backend_available() && view.transition.is_none();
                let settings = view.workspace.repeater_settings.clone();
                let update_cl = owner.clone();
                let unpack = owner.clone();
                let norm_crlf = owner.clone();
                let h1_close = owner.clone();
                let h2_strip = owner.clone();
                let open_full = owner.clone();
                let mut menu = menu
                    .item(
                        PopupMenuItem::new("自动更新 Content-Length")
                            .disabled(!editable)
                            .checked(settings.update_content_length)
                            .on_click(move |_, window, cx| {
                                let _ = update_cl.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_update_content_length(
                                            project,
                                            !settings.update_content_length,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("自动解包压缩响应 (Gzip / Brotli / Deflate)")
                            .disabled(!editable)
                            .checked(settings.unpack_compressed_responses)
                            .on_click(move |_, window, cx| {
                                let _ = unpack.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_unpack(
                                            project,
                                            !settings.unpack_compressed_responses,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("规范化 HTTP/1 换行符 (\\r\\n)")
                            .disabled(!editable)
                            .checked(settings.normalize_http1_line_endings)
                            .on_click(move |_, window, cx| {
                                let _ = norm_crlf.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_normalize_http1_line_endings(
                                            project,
                                            !settings.normalize_http1_line_endings,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("HTTP/1 强制 Connection: close")
                            .disabled(!editable)
                            .checked(settings.enforce_http1_connection_close)
                            .on_click(move |_, window, cx| {
                                let _ = h1_close.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_enforce_http1_connection_close(
                                            project,
                                            !settings.enforce_http1_connection_close,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("HTTP/2 自动剥离逐跳连接头")
                            .disabled(!editable)
                            .checked(settings.strip_connection_headers_over_http2)
                            .on_click(move |_, window, cx| {
                                let _ = h2_strip.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_strip_http2_connection_headers(
                                            project,
                                            !settings.strip_connection_headers_over_http2,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .separator();
                for (label, mode) in [
                    ("从不跟随重定向", RepeaterRedirectMode::Never),
                    ("仅同站跟随重定向", RepeaterRedirectMode::OnSiteOnly),
                    ("仅范围内跟随重定向", RepeaterRedirectMode::InScopeOnly),
                    ("始终跟随重定向", RepeaterRedirectMode::Always),
                ] {
                    let redirect_owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .disabled(!editable)
                            .checked(settings.follow_redirects == mode)
                            .on_click(move |_, window, cx| {
                                let _ = redirect_owner.update(cx, |this, cx| {
                                    if this.flush_workspace(window, cx) {
                                        this.set_repeater_follow_redirects(project, mode, cx);
                                    }
                                });
                            }),
                    );
                }
                menu.separator().item(
                    PopupMenuItem::new("打开完整 Repeater 设置…").on_click(
                        move |_, window, cx| {
                            let _ = open_full.update(cx, |this, cx| {
                                this.open_settings_filtered(
                                    Some(crate::workbench::settings::SettingsPage::Repeater),
                                    None,
                                    false,
                                    window,
                                    cx,
                                );
                            });
                        },
                    ),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_history_nav_pair(
        &self,
        tab_id: TabId,
        forward: bool,
        disabled: bool,
        page: Option<RepeaterHistoryPage>,
        btn_bg: gpui_kit::Hsla,
        btn_hover_bg: gpui_kit::Hsla,
        btn_border: gpui_kit::Hsla,
        icon_fg: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let (btn_id, menu_id, nav_tip, menu_label, menu_tip, nav_icon) = if forward {
            (
                "repeater-history-next",
                "repeater-history-next-menu",
                "前进到当前 Repeater 标签页的下一次发送历史记录",
                "后面的历史记录",
                "展开晚于当前位置的已发送请求历史列表",
                IconName::ChevronRight,
            )
        } else {
            (
                "repeater-history-previous",
                "repeater-history-previous-menu",
                "回退到当前 Repeater 标签页的上一次发送历史记录",
                "前面的历史记录",
                "展开早于当前位置的已发送请求历史列表",
                IconName::ChevronLeft,
            )
        };
        div()
            .w(px(47.))
            .h(px(24.))
            .flex()
            .items_center()
            .gap_0()
            .child(
                Button::new(btn_id)
                    .ghost()
                    .small()
                    .rounded(ButtonRounded::None)
                    .rounded_tl(px(3.))
                    .rounded_bl(px(3.))
                    .w(px(30.))
                    .h(px(24.))
                    .p_0()
                    .bg(btn_bg)
                    .border_1()
                    .border_color(btn_border)
                    .when(!disabled, |b| b.hover(move |s| s.bg(btn_hover_bg)))
                    .tab_stop(false)
                    .tooltip(nav_tip)
                    .disabled(disabled)
                    .child(
                        gpui_kit::component::Icon::new(nav_icon)
                            .size(px(14.))
                            .text_color(icon_fg),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate_repeater_history(tab_id, forward, window, cx)
                    })),
            )
            .child(
                Button::new(menu_id)
                    .ghost()
                    .small()
                    .rounded(ButtonRounded::None)
                    .rounded_tr(px(3.))
                    .rounded_br(px(3.))
                    .w(px(17.))
                    .h(px(24.))
                    .p_0()
                    .bg(btn_bg)
                    .border_t_1()
                    .border_r_1()
                    .border_b_1()
                    .border_color(btn_border)
                    .when(!disabled, |b| b.hover(move |s| s.bg(btn_hover_bg)))
                    .tab_stop(false)
                    .accessibility_label(menu_label)
                    .tooltip(menu_tip)
                    .child(
                        gpui_kit::component::Icon::new(IconName::ChevronDown)
                            .size(px(11.))
                            .text_color(icon_fg)
                            .test_alt_text("历史下拉箭头"),
                    )
                    .disabled(disabled)
                    .dropdown_menu(move |menu, window, cx| {
                        if let Some(page) = &page {
                            repeater_toolbar_history_menu(
                                menu,
                                page,
                                (tab_id, forward),
                                true,
                                owner.clone(),
                                window,
                                cx,
                            )
                        } else {
                            menu
                        }
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_toolbar_history_controls(
        &self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = self
            .repeater_history
            .owner
            .is_some_and(|(_, id)| id == tab_id);
        let page = owner
            .then_some(self.repeater_history.page.as_ref())
            .flatten();
        let current = page.and_then(|page| page.current_position);
        let busy = self.repeater_history.pending.contains_key(&tab_id)
            || self.repeater_history.querying
            || !self.backend_available()
            || self.transition.is_some();
        let previous_disabled = busy || current.is_none_or(|position| position == 0);
        let next_disabled = busy
            || current.is_none_or(|position| page.is_none_or(|page| position + 1 >= page.total));
        let back_page = page.cloned();
        let forward_page = page.cloned();
        let dark = theme::is_dark();
        let btn_bg: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xffffff }).into();
        let btn_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xf2f2f2 }).into();
        let btn_border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xcccccc }).into();
        let chevron_color = |disabled: bool| -> gpui_kit::Hsla {
            if dark {
                if disabled {
                    theme::muted_foreground()
                } else {
                    theme::foreground()
                }
            } else if disabled {
                rgb(0x666666).into()
            } else {
                rgb(0x444444).into()
            }
        };
        let prev_fg = chevron_color(previous_disabled);
        let next_fg = chevron_color(next_disabled);
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(self.render_repeater_history_nav_pair(
                tab_id,
                false,
                previous_disabled,
                back_page,
                btn_bg,
                btn_hover_bg,
                btn_border,
                prev_fg,
                cx,
            ))
            .child(self.render_repeater_history_nav_pair(
                tab_id,
                true,
                next_disabled,
                forward_page,
                btn_bg,
                btn_hover_bg,
                btn_border,
                next_fg,
                cx,
            ))
            .when_some(
                self.repeater_history.error.as_ref().filter(|_| owner),
                |this, error| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_color(theme::danger_fg())
                            .child(error.clone())
                            .child(
                                button("repeater-history-retry", "重试读取历史")
                                    .icon(IconName::RefreshCw)
                                    .tooltip("重新从项目存储读取当前标签页的发送历史")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.query_repeater_history(
                                            this.repeater_history.query_start,
                                            cx,
                                        )
                                    })),
                            ),
                    )
                },
            )
            .into_any_element()
    }
}

fn repeater_toolbar_history_menu(
    mut menu: PopupMenu,
    page: &RepeaterHistoryPage,
    (tab_id, forward): (TabId, bool),
    initial: bool,
    owner: WeakEntity<Workbench>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(current) = page.current_position else {
        return menu;
    };
    let adjacent = if forward {
        current
            .checked_add(1)
            .filter(|position| *position < page.total)
    } else {
        current.checked_sub(1)
    };
    if initial
        && let Some(position) = adjacent
        && !page.rows.iter().any(|row| row.position == position)
    {
        let start = if forward {
            position
        } else {
            position.saturating_sub((REPEATER_HISTORY_PAGE_SIZE - 1) as u64)
        };
        request_repeater_toolbar_history_page(tab_id, forward, start, owner, window, cx);
        return menu.item(PopupMenuItem::Label("正在读取历史…".into()));
    }
    let mut rows: Vec<_> = page
        .rows
        .iter()
        .filter(|row| {
            if forward {
                row.position > current
            } else {
                row.position < current
            }
        })
        .collect();
    if !forward {
        rows.reverse();
    }
    let next = if forward {
        rows.last()
            .and_then(|row| row.position.checked_add(1))
            .filter(|next| *next < page.total)
            .or_else(|| {
                rows.is_empty()
                    .then_some(current + 1)
                    .filter(|next| *next < page.total)
            })
    } else {
        rows.last()
            .and_then(|row| row.position.checked_sub(1))
            .or_else(|| rows.is_empty().then(|| current.checked_sub(1)).flatten())
    };
    for row in rows {
        let owner = owner.clone();
        let position = row.position;
        let id = row.id;
        menu = menu.item(
            PopupMenuItem::new(format_repeater_toolbar_history_label(row)).on_click(
                move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.select_history_position(tab_id, position, Some(id), window, cx)
                    });
                },
            ),
        );
    }
    if let Some(position) = next {
        menu = menu.submenu(
            "更多历史记录…",
            window,
            cx,
            move |menu, window, cx| {
                let owner = owner.clone();
                let focus = menu.focus_handle(cx);
                cx.on_focus(&focus, window, move |_, window, cx| {
                    let start = if forward {
                        position
                    } else {
                        position.saturating_sub((REPEATER_HISTORY_PAGE_SIZE - 1) as u64)
                    };
                    request_repeater_toolbar_history_page(
                        tab_id,
                        forward,
                        start,
                        owner.clone(),
                        window,
                        cx,
                    );
                })
                .detach();
                menu.item(PopupMenuItem::Label("正在读取历史…".into()))
            },
        );
    }
    menu.scrollable(true).max_h(px(500.))
}

fn format_repeater_toolbar_history_label(row: &mio_core::RepeaterHistoryEntry) -> String {
    let mut label = format!("{}. ", row.position + 1);
    if let Some(method) = row.method.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        label.push_str(method);
        label.push(' ');
    }
    label.push_str(
        row.target
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("URL 未确认"),
    );
    match (
        row.status,
        row.diagnostic
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty()),
    ) {
        (Some(status), Some(diag)) => {
            label.push_str(&format!(" [{status} · {diag}]"));
        }
        (Some(status), None) => {
            label.push_str(&format!(" [{status}]"));
        }
        (None, Some(diag)) => {
            label.push_str(&format!(" [{diag}]"));
        }
        (None, None) => {}
    }
    label
}

fn request_repeater_toolbar_history_page(
    tab_id: TabId,
    forward: bool,
    start: u64,
    owner: WeakEntity<Workbench>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) {
    let popup = cx.weak_entity();
    let handle = window.window_handle();
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, _, cx| {
            let _ = owner.update(cx, |view, cx| {
                if view
                    .repeater_history
                    .owner
                    .is_some_and(|(_, id)| id == tab_id)
                {
                    view.repeater_history.menu = Some((tab_id, forward, popup));
                    view.query_repeater_history(start, cx);
                }
            });
        });
    });
}
