use mio_core::{PluginAction, Uuid};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn plugin_operation_error(
        &mut self,
        operation: Operation,
        error: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extensions.push_error_log(format!("操作失败：{error}"));
        match operation {
            Operation::Workspace {
                revision, mutation, ..
            } if self.pending_workspace == Some(revision) => {
                self.rollback_plugin_mutation(*mutation, window, cx);
                self.pending_workspace = None;
                self.workspace_dirty = true;
            }
            Operation::Event {
                instance,
                node_id,
                event_seq,
            } => {
                if let Some(view) = self.extensions.instance(instance) {
                    view.panel.update(cx, |panel, cx| {
                        panel.event_failed_with_seq(Some(node_id), Some(event_seq), error, cx)
                    });
                }
            }
            Operation::Rebind { instance, .. } => {
                if let Some(view) = self.extensions.instance_mut(instance) {
                    view.bound = None;
                    view.panel
                        .update(cx, |panel, cx| panel.set_enabled(view.live, cx));
                }
            }
            Operation::EnableInstalled { placeholder_id } => {
                if let Some(inst) = self.extensions.instance_mut(placeholder_id)
                    && !inst.live
                {
                    inst.status = format!("启用失败：{error}");
                }
            }
            Operation::OpenContext {
                owner,
                context_id,
                revision,
            } => {
                if self.extensions.context_viewers.get(owner).is_some_and(|v| {
                    v.context_id == context_id && v.revision == revision && v.instance_id.is_none()
                }) {
                    self.extensions.context_viewers.remove(owner);
                }
            }
            Operation::CommitContext {
                owner,
                context_id,
                revision,
                ..
            } => {
                if let Some(v) = self.extensions.context_viewers.get_mut(owner)
                    && v.context_id == context_id
                    && v.revision == revision
                {
                    v.committing = false;
                }
            }
            Operation::OpenResponse {
                owner,
                context_id,
                revision,
                ..
            } => {
                if self
                    .extensions
                    .response_viewers
                    .get(&owner)
                    .is_some_and(|v| {
                        v.context_id == context_id
                            && v.revision == revision
                            && v.instance_id.is_none()
                    })
                {
                    self.extensions.response_viewers.remove(&owner);
                }
            }
            Operation::CommitResponse {
                owner,
                context_id,
                revision,
                ..
            } => {
                if let Some(v) = self.extensions.response_viewers.get_mut(&owner)
                    && v.context_id == context_id
                    && v.revision == revision
                {
                    v.committing = false;
                }
            }
            _ => {}
        }
        self.fail(format!("插件操作失败：{error}；当前编辑内容保留。"), cx);
    }

    pub(in crate::workbench) fn clear_context_viewer(
        &mut self,
        owner: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.clear_response_viewer(owner, cx);
        if let Some(viewer) = self.extensions.context_viewers.remove(owner)
            && let Some(id) = viewer.instance_id
        {
            self.disable_plugin(id, cx);
            self.extensions
                .instances
                .retain(|instance| instance.info.identity.instance_id != id);
            if self.extensions.selected == Some(id) {
                self.extensions.selected = self
                    .extensions
                    .instances
                    .iter()
                    .rev()
                    .find(|instance| {
                        instance.info.tab_id.is_none() && instance.context_owner.is_none()
                    })
                    .map(|instance| instance.info.identity.instance_id);
            }
        }
    }

    pub(in crate::workbench) fn clear_response_viewer(
        &mut self,
        owner: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = self.extensions.response_viewers.remove(owner)
            && let Some(id) = viewer.instance_id
        {
            self.extensions
                .active_responses
                .retain(|_, instance| *instance != id);
            self.disable_plugin(id, cx);
            self.extensions
                .instances
                .retain(|instance| instance.info.identity.instance_id != id);
            if self.extensions.selected == Some(id) {
                self.extensions.selected = self
                    .extensions
                    .instances
                    .iter()
                    .rev()
                    .find(|instance| {
                        instance.info.tab_id.is_none() && instance.context_owner.is_none()
                    })
                    .map(|instance| instance.info.identity.instance_id);
            }
        }
    }

    pub(in crate::workbench) fn retain_plugin(
        &mut self,
        id: Uuid,
        reason: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let reason = reason.into();
        if let Some(instance) = self.extensions.instance_mut(id) {
            instance.live = false;
            instance.status = reason;
            instance.bound = None;
            instance
                .panel
                .update(cx, |panel, cx| panel.set_enabled(false, cx));
        }
        self.extensions
            .active_messages
            .retain(|_, instance| *instance != id);
        self.extensions
            .active_responses
            .retain(|_, instance| *instance != id);
        self.extensions
            .context_viewers
            .retain(|_, viewer| viewer.instance_id != Some(id));
        self.extensions
            .response_viewers
            .retain(|_, viewer| viewer.instance_id != Some(id));
        self.extensions
            .saved_commits
            .retain(|_, commit| commit.instance != id);
    }

    pub(in crate::workbench) fn disable_plugin(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.invalidate_plugin_commits(Some(id), None, cx);
        self.retain_plugin(id, "已请求停用；编辑内容保留为只读，可复制。", cx);
        self.plugin_control(
            PluginAction::Disable { instance_id: id },
            Operation::Disable,
            cx,
        );
    }

    pub(in crate::workbench) fn remove_retained_plugin(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(instance) = self.extensions.instance(id) else {
            return;
        };
        if instance.live {
            return;
        }
        if !instance.panel.read(cx).has_unsaved_edits(cx) {
            self.extensions
                .instances
                .retain(|instance| instance.info.identity.instance_id != id);
            self.extensions.auto_reload.remove(&id);
            self.extensions.selected = self
                .extensions
                .instances
                .iter()
                .rfind(|inst| inst.info.tab_id.is_none() && inst.context_owner.is_none())
                .map(|instance| instance.info.identity.instance_id);
            cx.notify();
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "移除尚未回写的插件编辑内容？",
            Some("这些文本尚未保存到 MIO 项目。先复制需要保留的内容；移除后无法恢复。"),
            &["保留", "移除"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(1) = answer.await {
                let _ = this.update_in(cx, |this, _, cx| {
                    if this
                        .extensions
                        .instance(id)
                        .is_some_and(|instance| !instance.live)
                    {
                        this.extensions
                            .instances
                            .retain(|instance| instance.info.identity.instance_id != id);
                        this.extensions.auto_reload.remove(&id);
                        this.extensions.selected = this
                            .extensions
                            .instances
                            .iter()
                            .rfind(|inst| {
                                inst.info.tab_id.is_none() && inst.context_owner.is_none()
                            })
                            .map(|instance| instance.info.identity.instance_id);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(in crate::workbench) fn uninstall_plugin(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(instance) = self.extensions.instance(id) else {
            return;
        };
        if instance.info.tab_id.is_some() || instance.context_owner.is_some() {
            return;
        }
        let plugin_id = instance.info.manifest.id.clone();
        let live = instance.live;
        if !instance.panel.read(cx).has_unsaved_edits(cx) {
            self.dispatch_uninstall_plugin(id, plugin_id, live, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "卸载插件并移除尚未回写的编辑内容？",
            Some("将从用户持久化插件目录中删除该扩展，未保存的插件编辑内容也将一并丢弃。"),
            &["保留", "卸载并移除"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(1) = answer.await {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.dispatch_uninstall_plugin(id, plugin_id, live, cx);
                });
            }
        })
        .detach();
    }

    fn dispatch_uninstall_plugin(
        &mut self,
        id: Uuid,
        plugin_id: String,
        live: bool,
        cx: &mut Context<Self>,
    ) {
        if self.extensions.active_top_suite_tab == Some(id) {
            self.extensions.active_top_suite_tab = None;
        }
        if live {
            self.invalidate_plugin_commits(Some(id), None, cx);
            self.retain_plugin(id, "正在从持久化插件目录卸载…", cx);
        } else if let Some(inst) = self.extensions.instance_mut(id) {
            inst.status = "正在从持久化插件目录卸载…".into();
        }
        if self
            .plugin_action(
                PluginAction::Uninstall {
                    plugin_id,
                    instance_id: live.then_some(id),
                },
                Operation::Uninstall { instance_id: id },
                cx,
            )
            .is_some()
        {
            self.status = "正在卸载并删除持久化插件…".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn has_unsaved_plugin_edits(&self, cx: &App) -> bool {
        self.extensions
            .instances
            .iter()
            .any(|instance| instance.panel.read(cx).has_unsaved_edits(cx))
    }

    pub(in crate::workbench) fn plugins_before_quit(
        &mut self,
        action: &AfterSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(action, AfterSave::Quit) {
            return true;
        }
        if std::mem::take(&mut self.extensions.allow_quit) {
            return true;
        }
        if !self
            .extensions
            .instances
            .iter()
            .any(|instance| instance.panel.read(cx).has_unsaved_edits(cx))
        {
            return true;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "插件仍有尚未回写的编辑内容",
            Some(
                "普通项目保存不包含插件临时编辑。请先回写到 Repeater 或复制；退出将丢弃这些文本。",
            ),
            &["返回保留内容", "放弃插件编辑并继续退出"],
            cx,
        );
        self.transition = Some(TransitionPhase::Confirming);
        let attempt = self.quit_attempt;
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.transition != Some(TransitionPhase::Confirming) {
                    return;
                }
                this.transition = None;
                if !this.quit_answer_is_current(attempt, cx) {
                    cx.notify();
                    return;
                }
                if matches!(answer, Ok(1)) {
                    cx.global::<crate::native_terminate::NativeTerminate>()
                        .extend_deadline_for_user_interaction();
                    this.extensions.allow_quit = true;
                    this.request_transition(AfterSave::Quit, window, cx);
                } else {
                    this.reject_quit(cx);
                }
                cx.notify();
            });
        })
        .detach();
        false
    }

    pub(in crate::workbench) fn plugins_project_changed(&mut self, cx: &mut Context<Self>) {
        self.extensions
            .instances
            .retain(|instance| instance.context_owner.is_none());
        for instance in &mut self.extensions.instances {
            instance.live = false;
            instance.persisted = false;
            instance.bound = None;
            instance.status = "原项目已关闭；仅保留编辑内容供复制，插件未自动重启。".into();
            instance
                .panel
                .update(cx, |panel, cx| panel.set_enabled(false, cx));
        }
        self.extensions.extensions_disabled = false;
        self.extensions.operations.clear();
        self.extensions.create_after_save.clear();
        self.extensions.active_messages.clear();
        self.extensions.active_responses.clear();
        self.extensions.context_viewers.clear();
        self.extensions.response_viewers.clear();
        self.extensions.saved_commits.clear();
        self.extensions.workspace_requests.clear();
        self.extensions.inspection = None;
        self.extensions.add_panel_open = false;
        self.extensions.auto_reload.clear();
        self.extensions.output_selected_all = false;
        self.extensions.errors_selected_all = false;
        self.extensions.custom_checks.reset_selection();
        self.extensions.allow_quit = false;
    }

    pub(in crate::workbench) fn apply_plugin_auto_reload_updated(
        &mut self,
        instance_id: Uuid,
        enabled: bool,
    ) {
        if enabled {
            self.extensions.auto_reload.insert(instance_id);
        } else {
            self.extensions.auto_reload.remove(&instance_id);
        }
        let name = self
            .extensions
            .instance(instance_id)
            .map(|inst| inst.info.manifest.id.clone())
            .unwrap_or_else(|| instance_id.to_string());
        let msg = if enabled {
            format!("已启用扩展 {name} 的文件变更自动重载 (Auto-reload)。")
        } else {
            format!("已关闭扩展 {name} 的自动重载 (Auto-reload)。")
        };
        self.extensions.push_log(&msg);
        self.error = None;
        self.status = msg;
    }

    pub(in crate::workbench) fn close_plugin_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        self.invalidate_plugin_commits(None, Some(tab_id), cx);
        self.clear_response_viewer(&format!("repeater:{tab_id}"), cx);
        let instances: Vec<_> = self
            .extensions
            .instances
            .iter()
            .filter(|instance| instance.live && instance.info.tab_id == Some(tab_id))
            .map(|instance| instance.info.identity.instance_id)
            .collect();
        for id in instances {
            self.disable_plugin(id, cx);
        }
        self.extensions.active_messages.remove(&tab_id);
        self.extensions.active_responses.remove(&tab_id);
        self.extensions
            .create_after_save
            .retain(|pending| pending.tab_id != tab_id);
    }

    pub(in crate::workbench) fn invalidate_plugin_commits(
        &mut self,
        instance: Option<Uuid>,
        tab: Option<TabId>,
        cx: &mut Context<Self>,
    ) {
        let operations: Vec<_> = self
            .extensions
            .operations
            .iter()
            .filter_map(|(id, operation)| match operation {
                Operation::Commit {
                    instance: owner,
                    tab_id,
                    ..
                }
                | Operation::Rebind {
                    instance: owner,
                    tab_id,
                    ..
                } if instance.is_none_or(|id| id == *owner)
                    && tab.is_none_or(|id| id == *tab_id) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        for operation_id in operations {
            match self.extensions.operations.remove(&operation_id) {
                Some(Operation::Commit { tab_id, .. }) => {
                    self.pending_sends.remove(&tab_id);
                }
                Some(Operation::Rebind { instance, .. }) => {
                    if let Some(view) = self.extensions.instance_mut(instance) {
                        view.bound = None;
                        view.panel
                            .update(cx, |panel, cx| panel.set_enabled(view.live, cx));
                    }
                }
                _ => {}
            }
            self.plugin_control(PluginAction::Cancel { operation_id }, Operation::Cancel, cx);
        }
        let saved: Vec<_> = self
            .extensions
            .saved_commits
            .iter()
            .filter(|(tab_id, commit)| {
                instance.is_none_or(|id| id == commit.instance)
                    && tab.is_none_or(|id| id == **tab_id)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in saved {
            self.extensions.saved_commits.remove(&id);
            self.pending_sends.remove(&id);
        }
    }

    pub(in crate::workbench) fn invalidate_all_plugin_sends(&mut self, cx: &mut Context<Self>) {
        self.invalidate_plugin_commits(None, None, cx);
    }

    pub(in crate::workbench) fn plugin_waiting(&self, tab_id: TabId) -> bool {
        self.extensions.operations.values().any(
            |operation| matches!(operation, Operation::Commit { tab_id: id, .. } | Operation::Rebind { tab_id: id, .. } if *id == tab_id),
        ) || self.extensions.saved_commits.contains_key(&tab_id)
    }

    pub(in crate::workbench) fn discard_pending_sends(&mut self, cx: &mut Context<Self>) -> bool {
        let commits: Vec<_> = self
            .extensions
            .operations
            .iter()
            .filter_map(|(id, operation)| {
                matches!(operation, Operation::Commit { .. }).then_some(*id)
            })
            .collect();
        let discarded = !commits.is_empty()
            || !self.pending_sends.is_empty()
            || !self.extensions.saved_commits.is_empty();
        self.pending_sends.clear();
        self.extensions.saved_commits.clear();
        for operation_id in commits {
            self.extensions.operations.remove(&operation_id);
            self.plugin_control(PluginAction::Cancel { operation_id }, Operation::Cancel, cx);
        }
        discarded
    }

    pub(in crate::workbench) fn cancel_plugin_send(
        &mut self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.plugin_waiting(tab_id) {
            return false;
        }
        self.invalidate_plugin_commits(None, Some(tab_id), cx);
        self.status = "插件提交/发送等待已取消；未自动发送旧稿。".into();
        true
    }
}
