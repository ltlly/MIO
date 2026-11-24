use gpui_kit::{Div, Stateful, component::button::ButtonVariants};
use mio_core::{RepeaterLayoutMode, Uuid, protocol::Permission};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn render_repeater_messages(
        &self,
        tab_id: TabId,
        editors: &TabEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (tabs, active, active_resp) = self.render_repeater_plugin_tabs(tab_id, cx);
        let body = if let Some(instance) = active.and_then(|id| self.extensions.instance(id)) {
            let id = instance.info.identity.instance_id;
            let mut sources = toolbar()
                .id("plugin-send-sources")
                .overflow_x_scroll()
                .child("发送来源编辑器：");
            sources = sources.child(
                button("plugin-send-follow-editor", "跟随当前编辑器")
                    .tooltip("自动以当前焦点所在的插件编辑器文档作为请求发送来源")
                    .selected(instance.document.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(instance) = this.extensions.instance_mut(id) {
                            instance.document = None;
                        }
                        cx.notify();
                    })),
            );
            let selected = instance
                .document
                .or_else(|| instance.panel.read(cx).focused_document());
            for (document, label) in instance.panel.read(cx).writable_documents() {
                let tip = format!("指定插件编辑器文档「{label}」作为请求发送来源");
                sources = sources.child(
                    button(("plugin-send-document", document), label)
                        .tooltip(tip)
                        .selected(selected == Some(document))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(instance) = this.extensions.instance_mut(id) {
                                instance.document = Some(document);
                            }
                            cx.notify();
                        })),
                );
            }
            let suite_source = self
                .extensions
                .instances
                .iter()
                .find(|cand| {
                    cand.live
                        && cand.info.tab_id.is_none()
                        && cand.context_owner.is_none()
                        && cand.info.manifest.id == instance.info.manifest.id
                })
                .map(|cand| cand.info.identity.instance_id);
            let draft_stale = !self.plugin_waiting(tab_id)
                && self
                    .workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .is_some_and(|tab| instance.bound.as_ref() != Some(&tab.draft));
            sources = sources.child(div().flex_1());
            if draft_stale {
                sources = sources.child(
                    div()
                        .id("plugin-draft-stale-hint")
                        .test_support()
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::danger_bg())
                        .border_1()
                        .border_color(theme::danger_border())
                        .text_color(theme::danger_fg())
                        .text_size(theme::scaled_px(11.))
                        .child("当前草稿已变更，请先同步"),
                );
            }
            sources = sources.child(
                button("plugin-rebind", "加载当前草稿")
                    .when(draft_stale, |btn| btn.primary())
                    .tooltip("将当前 Repeater 请求草稿同步载入到此插件编辑器")
                    .disabled(self.plugin_waiting(tab_id) || suite_source.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(source_id) = suite_source {
                            this.create_plugin_editor(source_id, Some(id), window, cx);
                        }
                    })),
            );
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(sources)
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .border_r_1()
                                .border_color(theme::border())
                                .child(instance.panel.clone()),
                        )
                        .when(
                            self.workspace.repeater_settings.layout != RepeaterLayoutMode::Combined,
                            |split| split.child(self.render_repeater_response_pane(editors, active_resp, cx)),
                        ),
                )
                .into_any_element()
        } else if active_resp.is_some() {
            if self.workspace.repeater_settings.layout == RepeaterLayoutMode::Combined {
                self.render_repeater_response_pane(editors, active_resp, cx)
            } else {
                self.render_repeater_native_req_plugin_resp(editors, active_resp, cx)
            }
        } else {
            self.message_pair("repeater", &editors.request, &editors.response, false, cx)
        };
        let Some(tabs) = tabs else {
            return body;
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_repeater_plugin_tabs(
        &self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> (Option<Stateful<Div>>, Option<Uuid>, Option<Uuid>) {
        let active = self.extensions.active_messages.get(&tab_id).copied();
        let mut has_plugin_tabs = active.is_some();
        let mut tabs = toolbar()
            .id("plugin-message-tabs")
            .overflow_x_scroll()
            .child(
                button("plugin-message-native", "原生消息")
                    .tooltip("切换回内置原生 HTTP 请求编辑器")
                    .selected(active.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.cancel_plugin_interactions(cx);
                        this.extensions.active_messages.remove(&tab_id);
                        this.focus.focus(window, cx);
                        cx.notify();
                    })),
            );
        let mut live_child_plugins = HashSet::new();
        for instance in self
            .extensions
            .instances
            .iter()
            .filter(|instance| instance.info.tab_id == Some(tab_id) && instance.live)
        {
            has_plugin_tabs = true;
            live_child_plugins.insert(instance.info.manifest.id.as_str());
            let id = instance.info.identity.instance_id;
            let title = instance.panel.read(cx).title().to_owned();
            tabs = tabs.child(
                button(
                    SharedString::from(format!("plugin-message-{id}")),
                    title.clone(),
                )
                .tooltip(format!("切换到插件请求消息编辑器「{title}」"))
                .selected(active == Some(id))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.cancel_plugin_interactions(cx);
                    this.extensions.active_messages.insert(tab_id, id);
                    this.focus.focus(window, cx);
                    cx.notify();
                })),
            );
        }
        let creating_sources: HashSet<Uuid> = self
            .extensions
            .operations
            .values()
            .filter_map(|op| match op {
                Operation::Create(p) if p.tab_id == tab_id && p.replace.is_none() => Some(p.source),
                _ => None,
            })
            .chain(
                self.extensions
                    .create_after_save
                    .iter()
                    .filter(|p| p.tab_id == tab_id && p.replace.is_none())
                    .map(|p| p.source),
            )
            .collect();
        for instance in self.extensions.instances.iter().filter(|instance| {
            instance.live
                && instance.info.tab_id.is_none()
                && instance.context_owner.is_none()
                && !live_child_plugins.contains(instance.info.manifest.id.as_str())
                && instance.info.grants.contains(&Permission::TrafficRead)
                && instance
                    .info
                    .manifest
                    .contributions
                    .iter()
                    .any(|contribution| contribution == "message_editor")
        }) {
            has_plugin_tabs = true;
            let id = instance.info.identity.instance_id;
            let suite_title = instance.panel.read(cx).title().trim().to_owned();
            let add_label = if suite_title.is_empty() {
                instance.info.manifest.id.clone()
            } else {
                suite_title
            };
            let creating = creating_sources.contains(&id);
            tabs = tabs.child(
                button(
                    SharedString::from(format!("plugin-add-editor-{id}")),
                    add_label,
                )
                .tooltip("为此标签打开该插件的请求消息编辑器视图")
                .disabled(creating)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.create_plugin_editor(id, None, window, cx)
                })),
            );
        }
        let (tabs, active_resp, has_resp_tabs) =
            self.append_repeater_response_tabs(tabs, tab_id, cx);
        (
            (has_plugin_tabs || has_resp_tabs).then_some(tabs),
            active,
            active_resp,
        )
    }

    pub(in crate::workbench) fn render_intercept_editor(
        &self,
        pending: &PendingIntercept,
        native_editor: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stage = match &pending.target {
            InterceptTarget::Http { stage, .. } => Some(*stage),
            InterceptTarget::WebSocket { .. } => None,
        };
        let Some(stage) = stage else {
            return native_editor;
        };
        if stage == InterceptStage::Response {
            return self.render_intercept_response_editor(pending, native_editor, cx);
        }
        let available = self.live_message_editor_plugins(false);
        let viewer = self.extensions.context_viewers.get("intercept");
        if available.is_empty() && viewer.is_none() {
            return native_editor;
        }
        let writable_sources: HashSet<Uuid> = self
            .live_message_editor_plugins(true)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let active_instance = viewer.and_then(|viewer| viewer.instance_id);
        let opening = viewer.is_some_and(|viewer| viewer.instance_id.is_none());
        let committing = viewer.is_some_and(|viewer| viewer.committing);
        let mut tabs = toolbar()
            .id("intercept-plugin-message-tabs")
            .overflow_x_scroll()
            .child(
                button("intercept-plugin-message-native", "原生消息")
                    .tooltip("切换回内置原生拦截请求编辑器")
                    .selected(viewer.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_plugin_interactions(cx);
                        this.clear_context_viewer("intercept", cx);
                        this.focus.focus(window, cx);
                        cx.notify();
                    })),
            );
        let active_plugin_id = active_instance
            .and_then(|id| self.extensions.instance(id))
            .map(|inst| inst.info.manifest.id.clone());
        if let Some(instance) = active_instance.and_then(|id| self.extensions.instance(id)) {
            let id = instance.info.identity.instance_id;
            let title = instance.panel.read(cx).title().to_owned();
            tabs = tabs.child(
                button(
                    SharedString::from(format!("intercept-plugin-message-{id}")),
                    title.clone(),
                )
                .tooltip(format!("当前活动拦截请求插件视图：{title}"))
                .selected(true),
            );
        } else if opening {
            tabs = tabs.child("正在打开插件视图…");
        }
        for (source_id, plugin_id) in available {
            if active_plugin_id.as_deref() == Some(plugin_id.as_str()) {
                continue;
            }
            let read_only = !writable_sources.contains(&source_id);
            tabs = tabs.child(
                button(
                    SharedString::from(format!("intercept-plugin-add-editor-{source_id}")),
                    format!("＋{plugin_id}"),
                )
                .tooltip("在扩展插件视图中查看或编辑当前拦截请求")
                .disabled(pending.deciding || opening || committing)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_plugin_context_viewer("intercept", source_id, read_only, window, cx)
                })),
            );
        }
        if let Some(instance) = active_instance.and_then(|id| self.extensions.instance(id))
            && viewer.is_some_and(|viewer| !viewer.read_only)
        {
            tabs = tabs.child(div().flex_1()).child(
                button("intercept-plugin-apply", "应用插件修改")
                    .primary()
                    .tooltip("将插件编辑器中的修改提交到当前拦截请求草稿")
                    .disabled(
                        pending.deciding
                            || committing
                            || !instance.live
                            || instance.panel.read(cx).writable_documents().is_empty(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.commit_plugin_context_editor("intercept", window, cx)
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

    pub(in crate::workbench) fn render_context_message_pair(
        &self,
        owner: &'static str,
        request: &MessageEditor,
        response: &MessageEditor,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let available = self.live_message_editor_plugins(false);
        let resp_available = self.live_response_editor_plugins(false);
        let viewer = self.extensions.context_viewers.get(owner);
        let resp_viewer = self.extensions.response_viewers.get(owner);
        if available.is_empty()
            && resp_available.is_empty()
            && viewer.is_none()
            && resp_viewer.is_none()
        {
            return self.message_pair(owner, request, response, readonly, cx);
        }
        let active_instance = viewer.and_then(|viewer| viewer.instance_id);
        let active_resp = resp_viewer.and_then(|viewer| viewer.instance_id);
        let opening = viewer.is_some_and(|viewer| viewer.instance_id.is_none());
        let mut tabs = toolbar()
            .id(SharedString::from(format!("{owner}-plugin-message-tabs")))
            .overflow_x_scroll()
            .child(
                button(
                    SharedString::from(format!("{owner}-plugin-message-native")),
                    "原生消息",
                )
                .tooltip("切换回内置原生 HTTP 消息视图")
                .selected(viewer.is_none() && resp_viewer.is_none())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.cancel_plugin_interactions(cx);
                    this.clear_context_viewer(owner, cx);
                    this.focus.focus(window, cx);
                    cx.notify();
                })),
            );
        let active_plugin_id = active_instance
            .and_then(|id| self.extensions.instance(id))
            .map(|inst| inst.info.manifest.id.clone());
        if let Some(instance) = active_instance.and_then(|id| self.extensions.instance(id)) {
            let id = instance.info.identity.instance_id;
            let title = instance.panel.read(cx).title().to_owned();
            tabs = tabs.child(
                button(
                    SharedString::from(format!("{owner}-plugin-message-{id}")),
                    title.clone(),
                )
                .tooltip(format!("当前活动请求插件只读视图：{title}"))
                .selected(true),
            );
        } else if opening {
            tabs = tabs.child("正在打开插件视图…");
        }
        let can_open = match owner {
            "history" => self.detail.is_some(),
            "imported-history" => self.imported_history_request_ready(),
            "logger" => self.extra_tools.logger_selected.is_some_and(|id| {
                self.extra_tools
                    .logger_loaded_detail
                    .as_deref()
                    .is_some_and(|(record, _)| record.summary.id == id)
            }),
            "target" => self.target_pane.selected_row.is_some(),
            "organiser" => self.organiser.selected_id.is_some(),
            "dashboard" => {
                self.audit_dashboard.selected_finding_id.is_some()
                    && self.audit_dashboard.loaded_flow.is_some()
            }
            _ => false,
        };
        for (source_id, plugin_id) in available {
            if active_plugin_id.as_deref() == Some(plugin_id.as_str()) {
                continue;
            }
            tabs = tabs.child(
                button(
                    SharedString::from(format!("{owner}-plugin-add-editor-{source_id}")),
                    format!("＋{plugin_id}"),
                )
                .tooltip("使用此扩展插件以只读方式解析当前请求报文")
                .disabled(!can_open || opening)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_plugin_context_viewer(owner, source_id, true, window, cx)
                })),
            );
        }
        tabs = self.append_context_response_tabs(tabs, owner, can_open, cx);
        let body = if active_instance.is_some() || active_resp.is_some() {
            self.render_context_plugin_split(
                owner,
                request,
                response,
                active_instance,
                active_resp,
                cx,
            )
        } else {
            self.message_pair(owner, request, response, readonly, cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
