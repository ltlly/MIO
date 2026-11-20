use mio_core::{
    HttpResponse, LoadedPlugin, PluginAction, Uuid, panel::EditorSnapshot,
    protocol::ChannelIdentity,
};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn open_plugin_repeater_response_viewer(
        &mut self,
        tab_id: TabId,
        source_instance: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(response) = self.repeater_response_for_tab(tab, window, cx).or_else(|| {
            self.tab_editors
                .get(&tab_id)
                .and_then(|ed| ed.loaded_flow.as_ref())
                .and_then(|flow| flow.response.clone())
        }) else {
            self.fail("当前 Repeater 标签尚未收到可解析的 HTTP 响应。", cx);
            return;
        };
        let owner = format!("repeater:{tab_id}");
        self.start_plugin_response_viewer(
            owner,
            Some(tab_id),
            "repeater-response",
            source_instance,
            true,
            response,
            cx,
        );
    }

    pub(in crate::workbench) fn open_plugin_response_viewer(
        &mut self,
        owner: &'static str,
        source_instance: Uuid,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let response = match owner {
            "history" => {
                let Some(record) = self.detail.as_ref() else {
                    return;
                };
                let candidate = if self.history_response_original_view {
                    self.detail_original
                        .response
                        .as_ref()
                        .or(record.response.as_ref())
                        .cloned()
                } else {
                    record.response.clone()
                };
                let Some(resp) = candidate else {
                    self.fail("当前历史记录尚未包含 HTTP 响应。", cx);
                    return;
                };
                resp
            }
            "logger" => {
                let Some(flow_id) = self.extra_tools.logger_selected else {
                    return;
                };
                let Some((record, _)) = self
                    .extra_tools
                    .logger_loaded_detail
                    .as_ref()
                    .filter(|(record, _)| record.summary.id == flow_id)
                else {
                    return;
                };
                let Some(resp) = record.response.clone() else {
                    self.fail("当前 Logger 记录尚未包含 HTTP 响应。", cx);
                    return;
                };
                resp
            }
            "intercept" => {
                let Some(pending) = self
                    .intercepts
                    .iter()
                    .find(|pending| Some(pending.id()) == self.selected_intercept)
                else {
                    return;
                };
                if pending.deciding {
                    return;
                }
                let InterceptTarget::Http {
                    stage: InterceptStage::Response,
                    record,
                    ..
                } = &pending.target
                else {
                    self.fail("当前拦截项不是 HTTP 响应阶段，无法打开插件响应编辑器。", cx);
                    return;
                };
                let raw = match pending.editor.bytes(window, cx) {
                    Ok(raw) => raw,
                    Err(error) => {
                        self.fail(error, cx);
                        return;
                    }
                };
                let version = record
                    .response
                    .as_ref()
                    .map_or(HttpVersion::Http11, |resp| resp.version);
                match parse_response(&raw, version) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        self.fail(format!("当前拦截响应无法解析：{error}"), cx);
                        return;
                    }
                }
            }
            "imported-history" | "target" | "organiser" | "dashboard" => {
                match self.extract_owner_messages(owner, window, cx) {
                    Ok((_, Some(resp))) => resp,
                    Ok((_, None)) => {
                        self.fail("当前记录尚未包含可解析的 HTTP 响应。", cx);
                        return;
                    }
                    Err(error) => {
                        self.fail(error, cx);
                        return;
                    }
                }
            }
            _ => return,
        };
        self.start_plugin_response_viewer(
            owner.to_owned(),
            None,
            owner,
            source_instance,
            read_only,
            response,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn start_plugin_response_viewer(
        &mut self,
        owner: String,
        tab_id: Option<TabId>,
        owner_tag: &'static str,
        source_instance: Uuid,
        read_only: bool,
        response: HttpResponse,
        cx: &mut Context<Self>,
    ) {
        self.clear_response_viewer(&owner, cx);
        if owner_tag == "intercept" {
            self.clear_context_viewer("intercept", cx);
        }
        let context_id = Uuid::new_v4();
        let revision = 1;
        self.extensions.response_viewers.insert(
            owner.clone(),
            ResponseViewer {
                context_id,
                revision,
                read_only,
                response: response.clone(),
                instance_id: None,
                committing: false,
            },
        );
        if self
            .plugin_action(
                PluginAction::OpenResponseViewer {
                    source_instance,
                    context_id,
                    revision,
                    read_only,
                    response,
                },
                Operation::OpenResponse {
                    owner: owner.clone(),
                    tab_id,
                    context_id,
                    revision,
                },
                cx,
            )
            .is_none()
        {
            self.extensions.response_viewers.remove(&owner);
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn finish_response_open(
        &mut self,
        owner: &str,
        tab_id: Option<TabId>,
        replies: ((Uuid, u64), (Uuid, u64)),
        info: LoadedPlugin,
        panel: std::sync::Arc<mio_core::panel::PanelSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (expected, actual) = replies;
        let candidate = info.identity.instance_id;
        let matches = expected == actual
            && self
                .extensions
                .response_viewers
                .get(owner)
                .is_some_and(|viewer| (viewer.context_id, viewer.revision) == expected)
            && tab_id.is_none_or(|tid| self.workspace.tabs.iter().any(|t| t.id == tid));
        if !matches {
            self.plugin_control(
                PluginAction::Disable {
                    instance_id: candidate,
                },
                Operation::Disable,
                cx,
            );
            return;
        }
        let context_tag = match owner {
            "history" => "history-response",
            "imported-history" => "imported-history-response",
            "logger" => "logger-response",
            "target" => "target-response",
            "organiser" => "organiser-response",
            "dashboard" => "dashboard-response",
            "intercept" => "intercept-response",
            _ => "repeater-response",
        };
        if !self.install_plugin_instance(info, panel, None, Some(context_tag), window, cx) {
            return;
        }
        if let Some(viewer) = self.extensions.response_viewers.get_mut(owner) {
            let replaced = viewer.instance_id.replace(candidate);
            viewer.committing = false;
            if let Some(tid) = tab_id {
                self.extensions.active_responses.insert(tid, candidate);
            }
            self.extensions
                .push_log(format!("已打开插件响应视图：{owner}（{candidate}）"));
            if let Some(old) = replaced.filter(|old| *old != candidate) {
                self.disable_plugin(old, cx);
                self.extensions
                    .instances
                    .retain(|instance| instance.info.identity.instance_id != old);
            }
        } else {
            self.disable_plugin(candidate, cx);
            self.extensions
                .instances
                .retain(|instance| instance.info.identity.instance_id != candidate);
        }
    }

    pub(in crate::workbench) fn commit_plugin_response_editor(
        &mut self,
        owner: &str,
        forward_drop: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(viewer) = self.extensions.response_viewers.get(owner) else {
            return false;
        };
        if viewer.read_only {
            return false;
        }
        if viewer.committing {
            return true;
        }
        let Some(instance_id) = viewer.instance_id else {
            return false;
        };
        let (context_id, revision, response) =
            (viewer.context_id, viewer.revision, viewer.response.clone());
        let Some(instance) = self.extensions.instance(instance_id) else {
            return false;
        };
        if !instance.live || self.project != Some(instance.info.identity.project) {
            self.fail("插件响应编辑器实例已停止，无法提交修改。", cx);
            return true;
        }
        let Some(document) = instance
            .document
            .or_else(|| instance.panel.read(cx).focused_document())
            .or_else(|| {
                instance
                    .panel
                    .read(cx)
                    .writable_documents()
                    .first()
                    .map(|(id, _)| *id)
            })
        else {
            if forward_drop.is_some() {
                return false;
            }
            self.fail("插件没有可写的响应编辑器，未应用修改。", cx);
            return true;
        };
        let panel = instance.panel.clone();
        let editor = match panel.update(cx, |panel, cx| panel.capture_editor(document, window, cx))
        {
            Ok(editor) => editor,
            Err(error) => {
                self.fail(error, cx);
                return true;
            }
        };
        if self
            .plugin_action(
                PluginAction::CommitResponseEditor {
                    instance_id,
                    context_id,
                    revision,
                    response,
                    editor: editor.clone(),
                },
                Operation::CommitResponse {
                    owner: owner.to_owned(),
                    instance: instance_id,
                    context_id,
                    revision,
                    editor,
                    forward_drop,
                },
                cx,
            )
            .is_some()
            && let Some(viewer) = self.extensions.response_viewers.get_mut(owner)
        {
            viewer.committing = true;
            self.status = "正在提交插件响应修改…".into();
            cx.notify();
            return true;
        }
        false
    }

    pub(in crate::workbench) fn begin_plugin_response_forward(
        &mut self,
        drop: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if drop {
            return false;
        }
        let is_resp = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
            .is_some_and(|p| {
                !p.deciding
                    && matches!(
                        p.target,
                        InterceptTarget::Http {
                            stage: InterceptStage::Response,
                            ..
                        }
                    )
            });
        if !is_resp {
            return false;
        }
        self.commit_plugin_response_editor("intercept", Some(false), window, cx)
    }

    pub(in crate::workbench) fn finish_response_commit(
        &mut self,
        owner: &str,
        expected: (Uuid, Uuid, u64, EditorSnapshot),
        actual: (ChannelIdentity, Uuid, u64, HttpResponse),
        forward_drop: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (instance, context_id, revision, editor) = expected;
        let (identity, reply_context, reply_revision, response) = actual;
        let current = self
            .extensions
            .instance(instance)
            .filter(|view| view.live && view.info.identity == identity)
            .and_then(|view| {
                view.panel
                    .update(cx, |panel, cx| {
                        panel.capture_editor(editor.document_id, window, cx)
                    })
                    .ok()
            });
        let valid = current.as_ref() == Some(&editor)
            && (reply_context, reply_revision) == (context_id, revision)
            && self
                .extensions
                .response_viewers
                .get(owner)
                .is_some_and(|viewer| {
                    viewer.context_id == context_id
                        && viewer.revision == revision
                        && viewer.instance_id == Some(instance)
                        && viewer.committing
                });
        if !valid {
            if let Some(viewer) = self.extensions.response_viewers.get_mut(owner)
                && viewer.context_id == context_id
                && viewer.revision == revision
            {
                viewer.committing = false;
            }
            self.fail("插件响应提交结果已过期或内容仍在变化；未覆盖当前响应。", cx);
            return;
        }
        if owner == "intercept" {
            let Some(pending) = self.intercepts.iter_mut().find(|pending| {
                Some(pending.id()) == self.selected_intercept
                    && !pending.deciding
                    && matches!(
                        pending.target,
                        InterceptTarget::Http {
                            stage: InterceptStage::Response,
                            ..
                        }
                    )
            }) else {
                if let Some(viewer) = self.extensions.response_viewers.get_mut(owner) {
                    viewer.committing = false;
                }
                self.fail("当前拦截响应已变化或已转发；插件修改未应用。", cx);
                return;
            };
            pending.editor.load(&response_to_raw(&response), window, cx);
        }
        if let Some(view) = self.extensions.instance(instance) {
            view.panel.update(cx, |panel, cx| {
                panel.mark_saved(editor.document_id, editor.revision, cx);
            });
        }
        self.clear_response_viewer(owner, cx);
        self.extensions
            .push_log(format!("已应用插件响应修改：{owner}"));
        self.status = "已将插件响应编辑器修改回写到当前拦截响应。".into();
        if let Some(drop) = forward_drop {
            self.decide(drop, window, cx);
        }
    }
}
