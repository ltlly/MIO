use mio_core::{PluginOutcome, Uuid};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn plugin_operation(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        result: Result<PluginOutcome, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project) {
            return;
        }
        let Some(operation) = self.extensions.operations.remove(&id) else {
            return;
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.plugin_operation_error(operation, &error, window, cx);
                return;
            }
        };
        match (operation, result) {
            (Operation::Inspect { replace }, PluginOutcome::Inspected(info)) => {
                let is_wasm = info.manifest.engine == "wasm";
                self.extensions.push_log(format!(
                    "已检查插件包：{} {}（engine={}）",
                    info.manifest.id, info.manifest.version, info.manifest.engine
                ));
                self.extensions.inspection = Some(Inspection {
                    info,
                    trusted: false,
                    grants: HashSet::new(),
                    replace,
                });
                self.extensions.add_panel_open = false;
                self.extensions.page = ExtensionsPage::Installed;
                self.tool = Tool::Extensions;
                self.status = if is_wasm {
                    "插件检查完成；尚未运行。请确认受限 WASM 模块与每项宿主权限。".into()
                } else {
                    "插件检查完成；尚未运行。请确认本地程序信任与每项宿主权限。".into()
                };
            }
            (Operation::Load { replace }, PluginOutcome::Loaded { info, panel }) => {
                if let Some(old) = replace {
                    self.retain_plugin(old, "已重载；此旧实例的编辑内容保留供复制。", cx);
                }
                let plugin_id = info.manifest.id.clone();
                let new_id = info.identity.instance_id;
                let has_persistence = self.extensions.plugins_dir.is_some();
                if has_persistence {
                    let mut removed_ids = Vec::new();
                    self.extensions.instances.retain(|inst| {
                        let removable = !inst.live
                            && inst.info.tab_id.is_none()
                            && inst.context_owner.is_none()
                            && inst.info.manifest.id == plugin_id
                            && !inst.panel.read(cx).has_unsaved_edits(cx);
                        if removable {
                            removed_ids.push(inst.info.identity.instance_id);
                        }
                        !removable
                    });
                    for old_id in removed_ids {
                        self.extensions.auto_reload.remove(&old_id);
                    }
                    for inst in &mut self.extensions.instances {
                        if inst.info.tab_id.is_none()
                            && inst.context_owner.is_none()
                            && inst.info.manifest.id == plugin_id
                        {
                            inst.persisted = false;
                        }
                    }
                }
                self.extensions.add_panel_open = false;
                self.extensions.push_log(format!(
                    "已加载插件：{} {}（{}）",
                    info.manifest.id, info.manifest.version, info.identity.instance_id
                ));
                self.install_plugin(info, panel, None, window, cx);
                if has_persistence {
                    if let Some(inst) = self.extensions.instance_mut(new_id) {
                        inst.persisted = true;
                    }
                    if !self
                        .extensions
                        .persisted_order
                        .iter()
                        .any(|id| id == &plugin_id)
                    {
                        self.extensions.persisted_order.push(plugin_id);
                    }
                }
            }
            (Operation::Create(pending), PluginOutcome::Loaded { info, panel }) => {
                let candidate = info.identity.instance_id;
                let plugin_id = info.manifest.id.clone();
                let captured = self.capture_drafts(window, cx);
                if captured.as_ref().is_ok_and(|changed| *changed) {
                    self.workspace_dirty = true;
                }
                let valid = info.tab_id == Some(pending.tab_id)
                    && self
                        .workspace
                        .tabs
                        .iter()
                        .any(|tab| tab.id == pending.tab_id && tab.draft == pending.draft)
                    && captured.is_ok();
                self.install_plugin(info, panel, Some(pending.draft), window, cx);
                if valid {
                    if let Some(old) = pending.replace {
                        self.disable_plugin(old, cx);
                    } else {
                        let duplicates: Vec<Uuid> = self
                            .extensions
                            .instances
                            .iter()
                            .filter(|inst| {
                                inst.live
                                    && inst.info.tab_id == Some(pending.tab_id)
                                    && inst.info.manifest.id == plugin_id
                                    && inst.info.identity.instance_id != candidate
                            })
                            .map(|inst| inst.info.identity.instance_id)
                            .collect();
                        for dup in duplicates {
                            self.disable_plugin(dup, cx);
                        }
                    }
                    self.extensions
                        .active_messages
                        .insert(pending.tab_id, candidate);
                    self.extensions
                        .push_log(format!("已创建消息编辑器实例：{candidate}"));
                    self.tool = Tool::Repeater;
                } else {
                    self.disable_plugin(candidate, cx);
                    self.fail("创建插件编辑器期间草稿已改变；未绑定旧稿，新输入保留。", cx);
                }
            }
            (
                Operation::Commit {
                    instance,
                    tab_id,
                    mut send,
                    editor,
                },
                PluginOutcome::Committed {
                    identity,
                    tab_id: reply_tab,
                    draft_revision,
                    request,
                },
            ) => {
                let draft = &send.draft;
                let Some(view) = self.extensions.instance(instance) else {
                    return;
                };
                let panel = view.panel.clone();
                let current = panel.update(cx, |panel, cx| {
                    panel.capture_editor(editor.document_id, window, cx)
                });
                let matches = identity == view.info.identity
                    && view.live
                    && reply_tab == tab_id
                    && draft_revision == draft.revision
                    && self
                        .workspace
                        .tabs
                        .iter()
                        .any(|tab| tab.id == tab_id && tab.draft == *draft)
                    && current.as_ref().is_ok_and(|current| current == &editor);
                let captured = self.capture_drafts(window, cx);
                if captured.as_ref().is_ok_and(|changed| *changed) {
                    self.workspace_dirty = true;
                }
                if !matches
                    || self.transition.is_some()
                    || captured.is_err()
                    || !self
                        .workspace
                        .tabs
                        .iter()
                        .any(|tab| tab.id == tab_id && tab.draft == *draft)
                {
                    self.fail(
                        "插件提交结果已过期或输入仍在变化；没有覆盖草稿或发送旧请求。",
                        cx,
                    );
                    return;
                }
                let tab = self
                    .workspace
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .unwrap();
                let raw = request_to_raw(&request);
                let changed = tab.draft.raw != raw
                    || tab.draft.target != request.url
                    || tab.draft.version != request.version;
                if changed {
                    let Some(revision) = tab.draft.revision.checked_add(1) else {
                        self.fail("草稿 revision 已耗尽；未修改或发送。", cx);
                        return;
                    };
                    tab.draft.revision = revision;
                    tab.draft.raw = raw;
                    tab.draft.target = request.url;
                    tab.draft.version = request.version;
                }
                let next = tab.draft.clone();
                self.apply_draft_to_native(tab_id, window, cx);
                self.extensions.saved_commits.insert(
                    tab_id,
                    SavedCommit {
                        instance,
                        editor,
                        draft: next.clone(),
                    },
                );
                send.draft = next;
                self.pending_sends.insert(tab_id, send);
                if self.save_workspace(cx) {
                    self.status = "插件提交已校验，正在保存最新草稿；等待确认后发送。".into();
                } else {
                    self.plugins_save_failed(cx);
                }
            }
            (
                Operation::Rebind {
                    instance,
                    tab_id,
                    draft,
                },
                PluginOutcome::Opened {
                    identity,
                    tab_id: reply_tab,
                    draft_revision,
                },
            ) => {
                let current = self
                    .workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.id == tab_id && tab.draft == draft);
                if let Some(view) = self.extensions.instance_mut(instance) {
                    if view.live
                        && view.info.identity == identity
                        && reply_tab == tab_id
                        && draft_revision == draft.revision
                        && current
                    {
                        view.bound = Some(draft);
                    } else {
                        view.bound = None;
                    }
                    view.panel
                        .update(cx, |panel, cx| panel.set_enabled(view.live, cx));
                }
            }
            (Operation::Disable, PluginOutcome::Disabled { identity }) => {
                self.extensions
                    .push_log(format!("插件已停用：{}", identity.instance_id));
                let is_persisted_suite = self.extensions.instance(identity.instance_id).is_some_and(
                    |inst| inst.persisted && inst.info.tab_id.is_none() && inst.context_owner.is_none(),
                );
                let reason = if is_persisted_suite {
                    "已停用（文件保留在持久化插件目录）；重新勾选 Loaded 即可启动。"
                } else {
                    "已停用；编辑内容保留供复制，未自动加载。"
                };
                self.retain_plugin(identity.instance_id, reason, cx);
            }
            (
                Operation::Workspace {
                    instance,
                    tab_id,
                    mutation,
                    ..
                },
                PluginOutcome::WorkspaceApplied,
            ) => {
                if let WorkspaceMutation::Repeater { proposed, .. } = &*mutation {
                    let draft = self
                        .workspace
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id && tab.draft == proposed.draft)
                        .map(|tab| tab.draft.clone());
                    if let Some(view) = self.extensions.instance_mut(instance)
                        && view.info.tab_id == Some(tab_id)
                    {
                        view.bound = draft;
                    }
                }
            }
            (Operation::Reorder, PluginOutcome::Reordered { order }) => {
                self.apply_plugin_order(order);
                self.extensions.push_log("插件执行顺序已更新");
            }
            (
                Operation::OpenContext {
                    owner,
                    context_id,
                    revision,
                },
                PluginOutcome::ContextOpened {
                    info,
                    context_id: reply_context,
                    revision: reply_revision,
                    panel,
                },
            ) => {
                self.finish_context_open(
                    owner,
                    ((context_id, revision), (reply_context, reply_revision)),
                    info,
                    panel,
                    window,
                    cx,
                );
            }
            (
                Operation::CommitContext {
                    owner,
                    instance,
                    context_id,
                    revision,
                    editor,
                },
                PluginOutcome::ContextCommitted {
                    identity,
                    context_id: reply_context,
                    revision: reply_revision,
                    request,
                },
            ) => {
                self.finish_context_commit(
                    owner,
                    (instance, context_id, revision, editor),
                    (identity, reply_context, reply_revision, request),
                    window,
                    cx,
                );
            }
            (
                Operation::OpenResponse {
                    owner,
                    tab_id,
                    context_id,
                    revision,
                },
                PluginOutcome::ResponseOpened {
                    info,
                    context_id: reply_context,
                    revision: reply_revision,
                    panel,
                },
            ) => {
                self.finish_response_open(
                    &owner,
                    tab_id,
                    ((context_id, revision), (reply_context, reply_revision)),
                    info,
                    panel,
                    window,
                    cx,
                );
            }
            (
                Operation::CommitResponse {
                    owner,
                    instance,
                    context_id,
                    revision,
                    editor,
                    forward_drop,
                },
                PluginOutcome::ResponseCommitted {
                    identity,
                    context_id: reply_context,
                    revision: reply_revision,
                    response,
                },
            ) => {
                self.finish_response_commit(
                    &owner,
                    (instance, context_id, revision, editor),
                    (identity, reply_context, reply_revision, response),
                    forward_drop,
                    window,
                    cx,
                );
            }
            (
                Operation::GeneratePayloads { tab_id },
                PluginOutcome::PayloadsGenerated {
                    identity,
                    tab_id: reply_tab,
                    payloads,
                },
            ) => {
                self.finish_plugin_generated_payloads(
                    tab_id, identity, reply_tab, &payloads, window, cx,
                );
            }
            (
                Operation::ProcessPayloads { tab_id, position },
                PluginOutcome::PayloadsProcessed {
                    identity,
                    tab_id: reply_tab,
                    position: reply_position,
                    payloads,
                },
            ) => {
                self.finish_plugin_processed_payloads(
                    (tab_id, position),
                    (identity, reply_tab, reply_position),
                    payloads,
                    window,
                    cx,
                );
            }
            (
                Operation::Reload { instance },
                PluginOutcome::Reloaded {
                    old_instance,
                    info,
                    panel,
                },
            ) if instance == old_instance => {
                let was_auto_reload = self.extensions.auto_reload.remove(&old_instance);
                let was_persisted = self
                    .extensions
                    .instance(old_instance)
                    .is_some_and(|inst| inst.persisted);
                let old_has_edits = self
                    .extensions
                    .instance(old_instance)
                    .is_some_and(|inst| inst.panel.read(cx).has_unsaved_edits(cx));
                let new_id = info.identity.instance_id;
                self.retain_plugin(old_instance, "已热重载；此旧实例的编辑内容保留供复制。", cx);
                if was_persisted {
                    if let Some(old) = self.extensions.instance_mut(old_instance) {
                        old.persisted = false;
                    }
                    if !old_has_edits && self.extensions.plugins_dir.is_some() {
                        self.extensions
                            .instances
                            .retain(|inst| inst.info.identity.instance_id != old_instance);
                    }
                }
                self.extensions.push_log(format!(
                    "已热重载插件：{} {}（{}）",
                    info.manifest.id, info.manifest.version, new_id
                ));
                self.install_plugin(info, panel, None, window, cx);
                if was_persisted && let Some(inst) = self.extensions.instance_mut(new_id) {
                    inst.persisted = true;
                }
                if was_auto_reload {
                    self.extensions.auto_reload.insert(new_id);
                } else {
                    self.extensions.auto_reload.remove(&new_id);
                }
            }
            (
                Operation::EnableInstalled { placeholder_id },
                PluginOutcome::InstalledEnabled {
                    info,
                    panel,
                    auto_reload,
                    order,
                },
            ) => {
                let plugin_id = info.manifest.id.clone();
                let new_id = info.identity.instance_id;
                let mut removed_ids = Vec::new();
                self.extensions.instances.retain(|inst| {
                    let removable = (inst.info.identity.instance_id == placeholder_id
                        || (inst.info.tab_id.is_none()
                            && inst.context_owner.is_none()
                            && inst.info.manifest.id == plugin_id))
                        && !inst.live
                        && !inst.panel.read(cx).has_unsaved_edits(cx);
                    if removable {
                        removed_ids.push(inst.info.identity.instance_id);
                    }
                    !removable
                });
                for old_id in removed_ids {
                    self.extensions.auto_reload.remove(&old_id);
                }
                for inst in &mut self.extensions.instances {
                    if inst.info.tab_id.is_none()
                        && inst.context_owner.is_none()
                        && inst.info.manifest.id == plugin_id
                    {
                        inst.persisted = false;
                    }
                }
                self.install_plugin(info.clone(), panel, None, window, cx);
                if let Some(inst) = self.extensions.instance_mut(new_id) {
                    inst.persisted = true;
                }
                if auto_reload {
                    self.extensions.auto_reload.insert(new_id);
                } else {
                    self.extensions.auto_reload.remove(&new_id);
                }
                self.apply_plugin_order_with_persisted(&order);
                self.extensions.push_log(format!(
                    "已启用持久化扩展：{} {}（{}）",
                    info.manifest.id, info.manifest.version, new_id
                ));
                self.status = format!("已启用扩展：{}", info.manifest.id);
            }
            (
                Operation::Uninstall {
                    instance_id: req_inst,
                },
                PluginOutcome::Uninstalled {
                    plugin_id,
                    identity,
                },
            ) => {
                let mut removed_ids = Vec::new();
                self.extensions.instances.retain(|inst| {
                    let id = inst.info.identity.instance_id;
                    let matches_target = id == req_inst
                        || identity.is_some_and(|i| i.instance_id == id)
                        || (inst.info.tab_id.is_none()
                            && inst.context_owner.is_none()
                            && inst.info.manifest.id == plugin_id);
                    if matches_target {
                        removed_ids.push(id);
                    }
                    !matches_target
                });
                for id in removed_ids {
                    self.extensions.auto_reload.remove(&id);
                    if self.extensions.active_top_suite_tab == Some(id) {
                        self.extensions.active_top_suite_tab = None;
                    }
                    self.extensions
                        .active_messages
                        .retain(|_, active| *active != id);
                    self.extensions
                        .active_responses
                        .retain(|_, active| *active != id);
                    self.extensions
                        .saved_commits
                        .retain(|_, commit| commit.instance != id);
                    self.extensions
                        .context_viewers
                        .retain(|_, viewer| viewer.instance_id != Some(id));
                    self.extensions
                        .response_viewers
                        .retain(|_, viewer| viewer.instance_id != Some(id));
                }
                self.extensions
                    .persisted_order
                    .retain(|id| id != &plugin_id);
                if self
                    .extensions
                    .selected
                    .is_some_and(|sel| self.extensions.instance(sel).is_none())
                {
                    self.extensions.selected = self
                        .extensions
                        .instances
                        .iter()
                        .rfind(|inst| {
                            inst.info.tab_id.is_none() && inst.context_owner.is_none()
                        })
                        .map(|inst| inst.info.identity.instance_id);
                }
                self.extensions
                    .push_log(format!("已卸载并移除扩展：{plugin_id}"));
                self.status = format!("已卸载扩展：{plugin_id}");
            }
            (
                Operation::SetAutoReload { instance, .. },
                PluginOutcome::AutoReloadUpdated {
                    instance_id,
                    enabled,
                },
            ) if instance == instance_id => {
                self.apply_plugin_auto_reload_updated(instance_id, enabled);
            }
            (
                Operation::ContextMenu { instance: op_id },
                PluginOutcome::ContextMenuInvoked {
                    identity,
                    status_message,
                    log_line,
                    open_repeater,
                    replace_request,
                },
            ) if op_id == identity.instance_id => {
                if !log_line.is_empty() {
                    self.extensions.push_log(log_line);
                }
                if let Some(raw_request) = replace_request
                    && (self.tool == Tool::Repeater
                        || (self.tool == Tool::Proxy && self.proxy_page == ProxyPage::Intercept))
                {
                    self.apply_plugin_context_replace_request(raw_request, window, cx);
                } else if let Some(request) = open_repeater {
                    self.open_plugin_context_repeater(request, window, cx);
                }
                if !status_message.is_empty() {
                    self.status = status_message;
                }
            }
            (
                _,
                PluginOutcome::EventQueued
                | PluginOutcome::CancellationRequested
                | PluginOutcome::WorkspaceRejected,
            ) => {}
            (operation, _) => {
                if let Operation::Rebind { instance, .. } = operation
                    && let Some(view) = self.extensions.instance_mut(instance)
                {
                    view.bound = None;
                    view.panel
                        .update(cx, |panel, cx| panel.set_enabled(view.live, cx));
                }
                self.fail("插件返回了与当前操作不匹配的结果；未应用。", cx);
            }
        }
        cx.notify();
    }
}
