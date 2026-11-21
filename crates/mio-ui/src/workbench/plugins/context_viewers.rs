use mio_core::{
    HttpRequest, LoadedPlugin, PluginAction, Uuid,
    panel::EditorSnapshot,
    protocol::{ChannelIdentity, Permission},
};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn finish_context_open(
        &mut self,
        owner: &'static str,
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
                .context_viewers
                .get(owner)
                .is_some_and(|viewer| (viewer.context_id, viewer.revision) == expected);
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
        if !self.install_plugin_instance(info, panel, None, Some(owner), window, cx) {
            return;
        }
        if let Some(viewer) = self.extensions.context_viewers.get_mut(owner) {
            let replaced = viewer.instance_id.replace(candidate);
            viewer.committing = false;
            self.extensions
                .push_log(format!("已打开插件消息视图：{owner}（{candidate}）"));
            let old = replaced;
            if let Some(old) = old.filter(|old| *old != candidate) {
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

    pub(in crate::workbench) fn finish_context_commit(
        &mut self,
        owner: &'static str,
        expected: (Uuid, Uuid, u64, EditorSnapshot),
        actual: (ChannelIdentity, Uuid, u64, HttpRequest),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (instance, context_id, revision, editor) = expected;
        let (identity, reply_context, reply_revision, request) = actual;
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
                .context_viewers
                .get(owner)
                .is_some_and(|viewer| {
                    viewer.context_id == context_id
                        && viewer.revision == revision
                        && viewer.instance_id == Some(instance)
                        && viewer.committing
                });
        if !valid {
            if let Some(viewer) = self.extensions.context_viewers.get_mut(owner)
                && viewer.context_id == context_id
                && viewer.revision == revision
            {
                viewer.committing = false;
            }
            self.fail("插件提交结果已过期或内容仍在变化；未覆盖当前消息。", cx);
            return;
        }
        if owner == "intercept" {
            let Some(pending) = self.intercepts.iter_mut().find(|pending| {
                Some(pending.id()) == self.selected_intercept
                    && !pending.deciding
                    && matches!(
                        pending.target,
                        InterceptTarget::Http {
                            stage: InterceptStage::Request,
                            ..
                        }
                    )
            }) else {
                if let Some(viewer) = self.extensions.context_viewers.get_mut(owner) {
                    viewer.committing = false;
                }
                self.fail("当前拦截请求已变化或已转发；插件修改未应用。", cx);
                return;
            };
            pending.editor.load(&request_to_raw(&request), window, cx);
        }
        if let Some(view) = self.extensions.instance(instance) {
            view.panel.update(cx, |panel, cx| {
                panel.mark_saved(editor.document_id, editor.revision, cx);
            });
        }
        self.clear_context_viewer(owner, cx);
        self.extensions
            .push_log(format!("已应用插件消息修改：{owner}"));
        self.status = "已将插件编辑器修改回写到当前拦截请求。".into();
    }

    pub(in crate::workbench) fn live_message_editor_plugins(
        &self,
        require_write: bool,
    ) -> Vec<(Uuid, String)> {
        self.extensions
            .instances
            .iter()
            .filter(|instance| {
                instance.live
                    && instance.info.tab_id.is_none()
                    && instance.context_owner.is_none()
                    && instance
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|contribution| contribution == "message_editor")
                    && instance.info.grants.contains(&Permission::TrafficRead)
                    && (!require_write || instance.info.grants.contains(&Permission::DraftWrite))
            })
            .map(|instance| {
                (
                    instance.info.identity.instance_id,
                    instance.info.manifest.id.clone(),
                )
            })
            .collect()
    }

    pub(in crate::workbench) fn open_plugin_context_viewer(
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
        let request = match owner {
            "history" => {
                let Some(record) = self.detail.as_ref() else {
                    return;
                };
                if self.history_original_view {
                    self.detail_original
                        .request
                        .as_ref()
                        .unwrap_or(&record.request)
                        .clone()
                } else {
                    record.request.clone()
                }
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
                record.request.clone()
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
                    stage: InterceptStage::Request,
                    record,
                    ..
                } = &pending.target
                else {
                    self.fail("当前拦截项不是 HTTP 请求阶段，无法打开插件请求编辑器。", cx);
                    return;
                };
                let raw = match pending.editor.bytes(window, cx) {
                    Ok(raw) => raw,
                    Err(error) => {
                        self.fail(error, cx);
                        return;
                    }
                };
                match parse_request(&raw, &record.request.url, record.request.version) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        self.fail(format!("当前拦截请求无法解析：{error}"), cx);
                        return;
                    }
                }
            }
            "imported-history" | "target" | "organiser" | "dashboard" => {
                match self.extract_owner_messages(owner, window, cx) {
                    Ok((req, _)) => req,
                    Err(error) => {
                        self.fail(error, cx);
                        return;
                    }
                }
            }
            _ => return,
        };
        self.clear_context_viewer(owner, cx);
        let context_id = Uuid::new_v4();
        let revision = 1;
        self.extensions.context_viewers.insert(
            owner,
            ContextViewer {
                context_id,
                revision,
                read_only,
                request: request.clone(),
                instance_id: None,
                committing: false,
            },
        );
        if self
            .plugin_action(
                PluginAction::OpenContextViewer {
                    source_instance,
                    context_id,
                    revision,
                    read_only,
                    request,
                },
                Operation::OpenContext {
                    owner,
                    context_id,
                    revision,
                },
                cx,
            )
            .is_none()
        {
            self.extensions.context_viewers.remove(owner);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn commit_plugin_context_editor(
        &mut self,
        owner: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(viewer) = self.extensions.context_viewers.get(owner) else {
            return;
        };
        if viewer.read_only || viewer.committing {
            return;
        }
        let Some(instance_id) = viewer.instance_id else {
            return;
        };
        let (context_id, revision, request) =
            (viewer.context_id, viewer.revision, viewer.request.clone());
        let Some(instance) = self.extensions.instance(instance_id) else {
            return;
        };
        if !instance.live || self.project != Some(instance.info.identity.project) {
            self.fail("插件实例已停止，无法提交修改。", cx);
            return;
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
            self.fail("插件没有可写的消息编辑器，未应用修改。", cx);
            return;
        };
        let panel = instance.panel.clone();
        let expected_panel = Some(panel.read(cx).commit_snapshot());
        let editor = match panel.update(cx, |panel, cx| panel.capture_editor(document, window, cx))
        {
            Ok(editor) => editor,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if self
            .plugin_action(
                PluginAction::CommitContextEditor {
                    instance_id,
                    context_id,
                    revision,
                    request,
                    editor: editor.clone(),
                    expected_panel,
                },
                Operation::CommitContext {
                    owner,
                    instance: instance_id,
                    context_id,
                    revision,
                    editor,
                },
                cx,
            )
            .is_some()
            && let Some(viewer) = self.extensions.context_viewers.get_mut(owner)
        {
            viewer.committing = true;
            self.status = "正在提交插件消息修改…".into();
            cx.notify();
        }
    }
}
