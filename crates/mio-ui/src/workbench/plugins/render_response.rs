use gpui_kit::{Div, Stateful, component::button::ButtonVariants};
use mio_core::Uuid;

use super::*;

impl Workbench {
    pub(super) fn append_repeater_response_tabs(
        &self,
        mut tabs: Stateful<Div>,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Option<Uuid>, bool) {
        let resp_owner = format!("repeater:{tab_id}");
        let resp_viewer = self.extensions.response_viewers.get(&resp_owner);
        let active_resp = self
            .extensions
            .active_responses
            .get(&tab_id)
            .copied()
            .or_else(|| resp_viewer.and_then(|v| v.instance_id));
        let resp_opening = resp_viewer.is_some_and(|v| v.instance_id.is_none());
        let resp_plugins = self.live_response_editor_plugins(false);
        let has_resp_tabs = !resp_plugins.is_empty() || active_resp.is_some() || resp_opening;
        if has_resp_tabs {
            tabs = tabs.child(div().flex_1());
            if let Some(badge) = self.plugin_response_editor_badge(tab_id) {
                tabs = tabs.child(
                    div()
                        .id("plugin-response-editor-status")
                        .text_color(theme::muted_foreground())
                        .child(badge),
                );
            }
            tabs = tabs.child(
                button("plugin-response-native", "原生响应")
                    .tooltip("切换回内置原生 HTTP 响应视图")
                    .selected(active_resp.is_none() && !resp_opening)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.cancel_plugin_interactions(cx);
                        this.extensions.active_responses.remove(&tab_id);
                        this.clear_response_viewer(&format!("repeater:{tab_id}"), cx);
                        this.focus.focus(window, cx);
                        cx.notify();
                    })),
            );
            let active_resp_plugin_id = active_resp
                .and_then(|id| self.extensions.instance(id))
                .map(|inst| inst.info.manifest.id.clone());
            if let Some(instance) = active_resp.and_then(|id| self.extensions.instance(id)) {
                let id = instance.info.identity.instance_id;
                let title = instance.panel.read(cx).title().to_owned();
                tabs = tabs.child(
                    button(
                        SharedString::from(format!("plugin-response-{id}")),
                        title.clone(),
                    )
                    .tooltip(format!("当前活动响应插件视图：{title}"))
                    .selected(true),
                );
            } else if resp_opening {
                tabs = tabs.child("正在打开响应视图…");
            }
            for (source_id, plugin_id) in resp_plugins {
                if active_resp_plugin_id.as_deref() == Some(plugin_id.as_str()) {
                    continue;
                }
                tabs = tabs.child(
                    button(
                        SharedString::from(format!("plugin-response-tab-{source_id}")),
                        format!("＋{plugin_id} (响应)"),
                    )
                    .tooltip("使用此扩展插件解析并渲染当前 HTTP 响应报文")
                    .disabled(resp_opening)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_plugin_repeater_response_viewer(tab_id, source_id, window, cx)
                    })),
                );
            }
        }
        (tabs, active_resp, has_resp_tabs)
    }

    pub(super) fn render_repeater_response_pane(
        &self,
        editors: &TabEditors,
        active_resp: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(instance) = active_resp.and_then(|id| self.extensions.instance(id)) {
            return div()
                .flex_1()
                .min_w_0()
                .child(instance.panel.clone())
                .into_any_element();
        }
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header(
                crate::workbench::wb_editors::EditorSlot::RepeaterResponse,
                "Response",
                editors.response.view(),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_response_editor_body(
                        crate::workbench::wb_editors::EditorSlot::RepeaterResponse,
                        &editors.response,
                        cx,
                    )),
            )
            .child(editors.response.render_search_bar(
                "repeater-response",
                cx,
                |this, action, window, cx| {
                    this.handle_message_search_action(
                        crate::workbench::wb_editors::EditorSlot::RepeaterResponse,
                        action,
                        window,
                        cx,
                    );
                },
            ))
            .into_any_element()
    }

    pub(super) fn render_repeater_native_req_plugin_resp(
        &self,
        editors: &TabEditors,
        active_resp: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let replace_readonly =
            self.is_message_replace_readonly(crate::workbench::wb_editors::EditorSlot::RepeaterRequest);
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(self.message_header(
                        crate::workbench::wb_editors::EditorSlot::RepeaterRequest,
                        "Request",
                        editors.request.view(),
                        cx,
                    ))
                    .child(
                        div().flex_1().min_h_0().child(
                            editing::message_editor(&editors.request)
                                .readonly(editors.request.is_readonly_preview())
                                .size_full()
                                .context_menu(self.request_context_menu(
                                    editors.request.state(),
                                    false,
                                ))
                                .aria_label("请求消息编辑器"),
                        ),
                    )
                    .child(editors.request.render_search_bar_editable(
                        "repeater-request",
                        replace_readonly || editors.request.is_readonly_preview(),
                        cx,
                        |this, action, window, cx| {
                            this.handle_message_search_action(
                                crate::workbench::wb_editors::EditorSlot::RepeaterRequest,
                                action,
                                window,
                                cx,
                            );
                        },
                    )),
            )
            .child(self.render_repeater_response_pane(editors, active_resp, cx))
            .into_any_element()
    }

    pub(super) fn render_intercept_response_editor(
        &self,
        pending: &PendingIntercept,
        native_editor: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let available = self.live_response_editor_plugins(false);
        let viewer = self.extensions.response_viewers.get("intercept");
        if available.is_empty() && viewer.is_none() {
            return native_editor;
        }
        let writable_sources: HashSet<Uuid> = self
            .live_response_editor_plugins(true)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let active_instance = viewer.and_then(|viewer| viewer.instance_id);
        let opening = viewer.is_some_and(|viewer| viewer.instance_id.is_none());
        let committing = viewer.is_some_and(|viewer| viewer.committing);
        let mut tabs = toolbar()
            .id("intercept-plugin-response-tabs")
            .overflow_x_scroll()
            .child(
                button("intercept-plugin-response-native", "原生响应")
                    .tooltip("切换回内置原生拦截响应编辑器")
                    .selected(viewer.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_plugin_interactions(cx);
                        this.clear_response_viewer("intercept", cx);
                        this.focus.focus(window, cx);
                        cx.notify();
                    })),
            );
        let active_resp_plugin_id = active_instance
            .and_then(|id| self.extensions.instance(id))
            .map(|inst| inst.info.manifest.id.clone());
        if let Some(instance) = active_instance.and_then(|id| self.extensions.instance(id)) {
            let id = instance.info.identity.instance_id;
            let title = instance.panel.read(cx).title().to_owned();
            tabs = tabs.child(
                button(
                    SharedString::from(format!("intercept-plugin-response-{id}")),
                    title.clone(),
                )
                .tooltip(format!("当前活动拦截响应插件视图：{title}"))
                .selected(true),
            );
        } else if opening {
            tabs = tabs.child("正在打开响应视图…");
        }
        for (source_id, plugin_id) in available {
            if active_resp_plugin_id.as_deref() == Some(plugin_id.as_str()) {
                continue;
            }
            let read_only = !writable_sources.contains(&source_id);
            tabs = tabs.child(
                button(
                    SharedString::from(format!("intercept-plugin-response-tab-{source_id}")),
                    format!("＋{plugin_id} (响应)"),
                )
                .tooltip("在扩展插件视图中查看或编辑当前拦截响应")
                .disabled(pending.deciding || opening || committing)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_plugin_response_viewer("intercept", source_id, read_only, window, cx)
                })),
            );
        }
        if let Some(instance) = active_instance.and_then(|id| self.extensions.instance(id))
            && viewer.is_some_and(|viewer| !viewer.read_only)
        {
            tabs = tabs.child(div().flex_1()).child(
                button("intercept-plugin-response-apply", "应用插件修改")
                    .primary()
                    .tooltip("将插件编辑器中的修改提交到当前拦截响应草稿")
                    .disabled(
                        pending.deciding
                            || committing
                            || !instance.live
                            || instance.panel.read(cx).writable_documents().is_empty(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        let _ = this.commit_plugin_response_editor("intercept", None, window, cx);
                    })),
            );
        }
        let body =
            if let Some(instance) = active_instance.and_then(|id| self.extensions.instance(id)) {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(toolbar().child(pending.header_label()))
                    .child(div().flex_1().min_h_0().child(instance.panel.clone()))
                    .into_any_element()
            } else {
                native_editor
            };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    pub(super) fn append_context_response_tabs(
        &self,
        mut tabs: Stateful<Div>,
        owner: &'static str,
        can_open: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let resp_available = self.live_response_editor_plugins(false);
        let resp_viewer = self.extensions.response_viewers.get(owner);
        if resp_available.is_empty() && resp_viewer.is_none() {
            return tabs;
        }
        let active_resp = resp_viewer.and_then(|viewer| viewer.instance_id);
        let resp_opening = resp_viewer.is_some_and(|viewer| viewer.instance_id.is_none());
        tabs = tabs.child(div().flex_1()).child(
            button(
                SharedString::from(format!("{owner}-plugin-response-native")),
                "原生响应",
            )
            .tooltip("切换回内置原生 HTTP 响应视图")
            .selected(resp_viewer.is_none())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.cancel_plugin_interactions(cx);
                this.clear_response_viewer(owner, cx);
                this.focus.focus(window, cx);
                cx.notify();
            })),
        );
        let active_resp_plugin_id = active_resp
            .and_then(|id| self.extensions.instance(id))
            .map(|inst| inst.info.manifest.id.clone());
        if let Some(instance) = active_resp.and_then(|id| self.extensions.instance(id)) {
            let id = instance.info.identity.instance_id;
            let title = instance.panel.read(cx).title().to_owned();
            tabs = tabs.child(
                button(
                    SharedString::from(format!("{owner}-plugin-response-{id}")),
                    title.clone(),
                )
                .tooltip(format!("当前活动响应插件只读视图：{title}"))
                .selected(true),
            );
        } else if resp_opening {
            tabs = tabs.child("正在打开响应视图…");
        }
        for (source_id, plugin_id) in resp_available {
            if active_resp_plugin_id.as_deref() == Some(plugin_id.as_str()) {
                continue;
            }
            tabs = tabs.child(
                button(
                    SharedString::from(format!("{owner}-plugin-response-tab-{source_id}")),
                    format!("＋{plugin_id} (响应)"),
                )
                .tooltip("使用此扩展插件以只读方式解析当前响应报文")
                .disabled(!can_open || resp_opening)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_plugin_response_viewer(owner, source_id, true, window, cx)
                })),
            );
        }
        tabs
    }

    pub(super) fn render_context_plugin_split(
        &self,
        owner: &'static str,
        request: &MessageEditor,
        response: &MessageEditor,
        active_instance: Option<Uuid>,
        active_resp: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (request_slot, response_slot) = match owner {
            "logger" => (
                crate::workbench::wb_editors::EditorSlot::LoggerRequest,
                crate::workbench::wb_editors::EditorSlot::LoggerResponse,
            ),
            "target" => (
                crate::workbench::wb_editors::EditorSlot::TargetRequest,
                crate::workbench::wb_editors::EditorSlot::TargetResponse,
            ),
            "organiser" => (
                crate::workbench::wb_editors::EditorSlot::OrganiserRequest,
                crate::workbench::wb_editors::EditorSlot::OrganiserResponse,
            ),
            "dashboard" => (
                crate::workbench::wb_editors::EditorSlot::DashboardIssueRequest,
                crate::workbench::wb_editors::EditorSlot::DashboardIssueResponse,
            ),
            _ => (
                crate::workbench::wb_editors::EditorSlot::HistoryRequest,
                crate::workbench::wb_editors::EditorSlot::HistoryResponse,
            ),
        };
        let request_id = request_slot.id();
        let response_id = response_slot.id();
        let left = if let Some(instance) =
            active_instance.and_then(|id| self.extensions.instance(id))
        {
            div()
                .flex_1()
                .min_w_0()
                .border_r_1()
                .border_color(theme::border())
                .child(instance.panel.clone())
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(theme::border())
                .child(self.message_header(request_slot, "Request", request.view(), cx))
                .child(
                    div().flex_1().min_h_0().child(
                        editing::message_editor(request)
                            .readonly(true)
                            .size_full()
                            .context_menu(self.request_context_menu(request.state(), true))
                            .aria_label("请求消息编辑器"),
                    ),
                )
                .child(request.render_search_bar(
                    request_id,
                    cx,
                    move |this, action, window, cx| {
                        this.handle_message_search_action(request_slot, action, window, cx);
                    },
                ))
                .into_any_element()
        };
        let right = if let Some(instance) = active_resp.and_then(|id| self.extensions.instance(id))
        {
            div()
                .flex_1()
                .min_w_0()
                .child(instance.panel.clone())
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(self.message_header(response_slot, "Response", response.view(), cx))
                .child(
                    div().flex_1().min_h_0().child(
                        editing::message_editor(response)
                            .readonly(true)
                            .size_full()
                            .context_menu(self.request_context_menu(response.state(), true))
                            .aria_label("响应消息编辑器"),
                    ),
                )
                .child(response.render_search_bar(
                    response_id,
                    cx,
                    move |this, action, window, cx| {
                        this.handle_message_search_action(response_slot, action, window, cx);
                    },
                ))
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .child(left)
            .child(right)
            .into_any_element()
    }
}
