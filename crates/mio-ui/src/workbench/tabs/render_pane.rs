use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;

use super::*;

mod drawers;
mod history_controls;
mod layouts;
mod target_dialog;

pub(in crate::workbench) use target_dialog::RepeaterTargetDialog;

gpui_kit::actions!(mio_repeater_pane, [SaveRepeaterEntireHistory]);

type RepeaterRedirectCacheKey = (
    TabId,
    crate::workbench::inspector::MessageEditorRevKey,
    HttpVersion,
);

#[derive(Default)]
pub(in crate::workbench) struct RepeaterUiCaches {
    pub(in crate::workbench::tabs::render_pane) open_target_dialogs:
        std::cell::RefCell<Vec<gpui_kit::WeakEntity<RepeaterTargetDialog>>>,
    pub(in crate::workbench::tabs::render_pane) redirect_cache:
        std::cell::RefCell<Option<(RepeaterRedirectCacheKey, bool)>>,
    pub(in crate::workbench::tabs::render_pane) response_len_cache:
        std::cell::RefCell<Option<(crate::workbench::inspector::MessageEditorRevKey, usize)>>,
    pub(in crate::workbench::tabs::render_pane) explanation_cache: std::cell::RefCell<
        Option<(drawers::ExplanationCacheKey, std::rc::Rc<drawers::RepeaterExplanationReport>)>,
    >,
}

impl RepeaterUiCaches {
    pub(in crate::workbench) fn clear_caches(&self) {
        self.open_target_dialogs
            .borrow_mut()
            .retain(|entry| entry.upgrade().is_some());
        *self.redirect_cache.borrow_mut() = None;
        *self.response_len_cache.borrow_mut() = None;
        *self.explanation_cache.borrow_mut() = None;
    }
}

#[cfg(test)]
std::thread_local! {
    static AUTO_OPEN_REPEATER_INSPECTOR_IN_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

impl Workbench {
    fn cached_message_byte_len(&self, editor: &MessageEditor, cx: &App) -> usize {
        let text_len = if editor.pretty_active() || editor.view() == MessageView::Raw {
            editor.raw.read(cx).text().len()
        } else {
            editor.hex.read(cx).text().len()
        };
        if text_len == 0 {
            return 0;
        }
        let key = crate::workbench::inspector::message_editor_rev_key(editor, cx);
        if let Some(len) = self
            .repeater_ui_caches
            .response_len_cache
            .borrow()
            .as_ref()
            .filter(|(cached_key, _)| *cached_key == key)
            .map(|(_, len)| *len)
        {
            return len;
        }
        let exact_len = editor.bytes_lossless(cx).len();
        *self.repeater_ui_caches.response_len_cache.borrow_mut() = Some((key, exact_len));
        exact_len
    }
}

#[cfg(test)]
pub(super) fn set_auto_open_repeater_inspector_in_test(enabled: bool) {
    AUTO_OPEN_REPEATER_INSPECTOR_IN_TEST.with(|cell| cell.set(enabled));
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
    #[allow(clippy::too_many_arguments)]
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
                let cache_key = (tab.id, resp_key, tab.draft.version);
                if let Some(cached) = self
                    .repeater_ui_caches
                    .redirect_cache
                    .borrow()
                    .as_ref()
                    .filter(|(k, _)| *k == cache_key)
                    .map(|(_, v)| *v)
                {
                    return cached;
                }
                let result = self.can_follow_repeater_redirect(tab, window, cx);
                *self.repeater_ui_caches.redirect_cache.borrow_mut() = Some((cache_key, result));
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
                        let loaded_len = self.cached_message_byte_len(&editors.response, cx) as u64;
                        if loaded_len > 0 {
                            loaded_len
                        } else {
                            flow.summary.response_bytes
                        }
                    };
                    format!("{resp_bytes} bytes | {} millis", flow.summary.elapsed_ms)
                })
                .unwrap_or_else(|| {
                    let resp_len = self.cached_message_byte_len(&editors.response, cx);
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
}
