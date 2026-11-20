use mio_core::{
    LoadedPlugin, PluginAction, Uuid, panel::EditorSnapshot, protocol::ChannelIdentity,
};

use crate::plugin_panel::{NativePanel, PanelEvent};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn plugin_response_editor_badge(
        &self,
        tab_id: TabId,
    ) -> Option<String> {
        let active_id = self
            .extensions
            .active_responses
            .get(&tab_id)
            .copied()
            .or_else(|| self.extensions.active_messages.get(&tab_id).copied());
        if let Some(instance) = active_id.and_then(|id| self.extensions.instance(id))
            && instance.live
            && instance
                .info
                .manifest
                .contributions
                .iter()
                .any(|c| c == "message_response_editor")
        {
            return Some(format!("插件响应视图就绪（{}）", instance.info.manifest.id));
        }
        self.extensions
            .instances
            .iter()
            .find(|instance| {
                instance.live
                    && instance.info.tab_id.is_none()
                    && instance.context_owner.is_none()
                    && instance.info.grants.contains(&Permission::TrafficRead)
                    && instance
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "message_response_editor")
            })
            .map(|instance| format!("插件响应视图可用（{}）", instance.info.manifest.id))
    }

    pub(in crate::workbench) fn live_response_editor_plugins(
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
                        .any(|contribution| contribution == "message_response_editor")
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

    pub(in crate::workbench) fn create_plugin_editor(
        &mut self,
        source: Uuid,
        replace: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.workspace.active else {
            return;
        };
        if self.transition.is_some() || self.plugin_waiting(tab_id) {
            return;
        }
        if !self.extensions.instance(source).is_some_and(|instance| {
            instance.live && self.project == Some(instance.info.identity.project)
        }) {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        if tab.draft.revision == 0 {
            tab.draft.revision = 1;
            self.save_workspace(cx);
        }
        let draft = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .unwrap()
            .draft
            .clone();
        let pending = CreateEditor {
            source,
            tab_id,
            draft,
            replace,
        };
        if self.pending_workspace.is_some() || self.workspace_dirty {
            self.extensions
                .create_after_save
                .retain(|request| request.tab_id != tab_id || request.source != source);
            self.extensions.create_after_save.push(pending);
            self.status = "正在保存当前草稿，确认后为此标签创建独立插件 worker。".into();
        } else {
            self.start_plugin_editor(pending, cx);
        }
    }

    pub(in crate::workbench) fn start_plugin_editor(
        &mut self,
        pending: CreateEditor,
        cx: &mut Context<Self>,
    ) {
        if !self
            .workspace
            .tabs
            .iter()
            .any(|tab| tab.id == pending.tab_id && tab.draft == pending.draft)
        {
            self.fail("等待期间草稿已改变，请重新选择插件消息编辑器。", cx);
            return;
        }
        self.plugin_action(
            PluginAction::CreateEditor {
                source_instance: pending.source,
                tab_id: pending.tab_id,
                draft_revision: pending.draft.revision,
            },
            Operation::Create(pending),
            cx,
        );
    }

    pub(in crate::workbench) fn begin_plugin_send(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(instance_id) = self.extensions.active_messages.get(&tab_id).copied() else {
            return false;
        };
        if !self.flush_workspace(window, cx) {
            return true;
        }
        let Some(instance) = self.extensions.instance(instance_id) else {
            return true;
        };
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == tab_id) else {
            return true;
        };
        if !instance.live
            || instance.info.identity.project != self.project.unwrap()
            || instance.bound.as_ref() != Some(&tab.draft)
        {
            self.fail("插件编辑器与当前草稿不一致。请点击“加载当前草稿”建立新的独立编辑上下文；现有插件文本会保留。", cx);
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
            self.fail("插件没有可写的消息编辑器，未发送请求。", cx);
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
        let Some(send) = self.capture_send(tab.draft.clone(), cx) else {
            return true;
        };
        if self
            .plugin_action(
                PluginAction::CommitEditor {
                    instance_id,
                    tab_id,
                    draft_revision: send.draft.revision,
                    editor: editor.clone(),
                },
                Operation::Commit {
                    instance: instance_id,
                    tab_id,
                    send,
                    editor,
                },
                cx,
            )
            .is_some()
        {
            self.status = "正在提交当前插件编辑；校验、项目保存确认完成后才发送。".into();
        }
        true
    }

    pub(in crate::workbench) fn install_plugin(
        &mut self,
        info: LoadedPlugin,
        snapshot: std::sync::Arc<mio_core::panel::PanelSnapshot>,
        bound: Option<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.install_plugin_instance(info, snapshot, bound, None, window, cx);
    }

    pub(in crate::workbench) fn install_plugin_instance(
        &mut self,
        info: LoadedPlugin,
        snapshot: std::sync::Arc<mio_core::panel::PanelSnapshot>,
        bound: Option<Draft>,
        context_owner: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(info.identity.project) {
            return false;
        }
        let id = info.identity.instance_id;
        if let Err(error) = snapshot.validate() {
            self.fail(error, cx);
            self.plugin_control(
                PluginAction::Disable { instance_id: id },
                Operation::Disable,
                cx,
            );
            return false;
        }
        let panel = cx.new(|cx| {
            NativePanel::new(snapshot, window, cx).expect("same immutable snapshot was validated")
        });
        let subscription =
            cx.subscribe_in(&panel, window, move |this, _, event, _, cx| match event {
                PanelEvent::Ui { revision, event } => {
                    if this.extensions.instance(id).is_some_and(|instance| {
                        instance.live && this.project == Some(instance.info.identity.project)
                    }) {
                        this.plugin_action(
                            PluginAction::Ui {
                                instance_id: id,
                                panel_revision: *revision,
                                event: event.clone(),
                            },
                            Operation::Event {
                                instance: id,
                                node_id: event.node_id,
                            },
                            cx,
                        );
                    }
                }
                PanelEvent::Edited => cx.notify(),
                PanelEvent::Error(error) => this.fail(error.clone(), cx),
            });
        let default_auto_reload =
            info.manifest.engine == "wasm" && info.tab_id.is_none() && context_owner.is_none();
        let status = if info.manifest.engine == "wasm" {
            "已加载 · 受限 WASM 沙箱（零 WASI，无直接 OS 文件/网络访问）".into()
        } else {
            "已加载 · native worker 未受 OS 沙箱限制".into()
        };
        self.extensions.instances.push(Instance {
            info,
            panel,
            live: true,
            status,
            bound,
            document: None,
            context_owner,
            _subscription: subscription,
        });
        if default_auto_reload {
            self.extensions.auto_reload.insert(id);
        }
        if context_owner.is_none() {
            self.extensions.selected = Some(id);
        }
        self.status = "插件已加载，已申请但未授权的宿主能力保持不可用。".into();
        true
    }

    pub(in crate::workbench) fn plugin_panel(
        &mut self,
        identity: ChannelIdentity,
        snapshot: std::sync::Arc<mio_core::panel::PanelSnapshot>,
        editor_bases: &[EditorSnapshot],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(instance) = self.extensions.instance(identity.instance_id) else {
            return;
        };
        if !instance.live
            || instance.info.identity != identity
            || self.project != Some(identity.project)
        {
            return;
        }
        let panel = instance.panel.clone();
        if let Err(error) = panel.update(cx, |panel, cx| {
            panel.replace(snapshot, editor_bases, window, cx)
        }) {
            self.fail(error, cx);
        }
    }

    pub(in crate::workbench) fn plugin_diagnostic(
        &mut self,
        identity: ChannelIdentity,
        message: String,
        stopped: bool,
        cx: &mut Context<Self>,
    ) {
        if !self
            .extensions
            .instance(identity.instance_id)
            .is_some_and(|instance| instance.info.identity == identity)
        {
            return;
        }
        self.extensions.push_error_log(if stopped {
            format!("插件停止（{}）：{message}", identity.instance_id)
        } else {
            format!("插件诊断（{}）：{message}", identity.instance_id)
        });
        if stopped {
            self.invalidate_plugin_commits(Some(identity.instance_id), None, cx);
            self.retain_plugin(
                identity.instance_id,
                format!("插件已停止：{message}；编辑内容保留。"),
                cx,
            );
        } else if let Some(instance) = self.extensions.instance_mut(identity.instance_id) {
            instance
                .panel
                .update(cx, |panel, cx| panel.event_failed(None, &message, cx));
            instance.status = message;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn apply_draft_to_native(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == tab_id)
            && let Some(editors) = self.tab_editors.get_mut(&tab_id)
        {
            editors.request.load(&tab.draft.raw, window, cx);
            editors.target.update(cx, |state, cx| {
                state.set_value(tab.draft.target.clone(), window, cx)
            });
            editors.loaded_revision = tab.draft.revision;
            editors.dirty = false;
        }
    }

    pub(in crate::workbench) fn plugins_before_workspace_ack(
        &mut self,
        saved: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.extensions.saved_commits.is_empty() {
            match self.capture_drafts(window, cx) {
                Ok(changed) => self.workspace_dirty |= changed,
                Err(error) => {
                    self.invalidate_plugin_commits(None, None, cx);
                    self.fail(format!("保存确认时输入尚未完成：{error}；未发送旧稿。"), cx);
                    return;
                }
            }
        }
        let ids: Vec<_> = self.extensions.saved_commits.keys().copied().collect();
        for id in ids {
            let commit = self.extensions.saved_commits.remove(&id).unwrap();
            let valid = self
                .extensions
                .instance(commit.instance)
                .filter(|instance| instance.live)
                .map(|instance| instance.panel.clone())
                .and_then(|panel| {
                    panel
                        .update(cx, |panel, cx| {
                            panel.capture_editor(commit.editor.document_id, window, cx)
                        })
                        .ok()
                        .map(|current| (panel, current))
                })
                .filter(|(_, current)| current == &commit.editor)
                .filter(|_| {
                    self.workspace
                        .tabs
                        .iter()
                        .any(|tab| tab.id == id && tab.draft == commit.draft)
                });
            if let Some((panel, _)) = valid {
                if !self
                    .pending_sends
                    .get(&id)
                    .is_some_and(|send| send.acknowledged_by(id, saved))
                {
                    self.extensions.saved_commits.insert(id, commit);
                    continue;
                }
                panel.update(cx, |panel, cx| {
                    panel.mark_saved(commit.editor.document_id, commit.editor.revision, cx)
                });
                let clean = panel
                    .update(cx, |panel, cx| panel.all_edits_saved(window, cx))
                    .unwrap_or(false);
                if clean && self.transition.is_none() {
                    let instance = commit.instance;
                    if self
                        .plugin_action(
                            PluginAction::OpenRequest {
                                instance_id: instance,
                                tab_id: id,
                                draft_revision: commit.draft.revision,
                            },
                            Operation::Rebind {
                                instance,
                                tab_id: id,
                                draft: commit.draft,
                            },
                            cx,
                        )
                        .is_some()
                    {
                        panel.update(cx, |panel, cx| panel.set_enabled(false, cx));
                    }
                }
            } else {
                self.pending_sends.remove(&id);
                self.fail("等待保存确认期间插件文本已改变或解绑；没有发送旧稿。", cx);
            }
        }
    }

    pub(in crate::workbench) fn plugins_save_failed(&mut self, cx: &mut Context<Self>) {
        self.invalidate_plugin_commits(None, None, cx);
        self.extensions.create_after_save.clear();
    }
}
