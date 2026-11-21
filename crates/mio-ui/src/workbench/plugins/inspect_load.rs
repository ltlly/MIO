use mio_core::{PluginAction, Uuid, protocol::Permission};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn plugin_action(
        &mut self,
        action: PluginAction,
        operation: Operation,
        cx: &mut Context<Self>,
    ) -> Option<Uuid> {
        let project = self.project?;
        let operation_id = Uuid::new_v4();
        if self.enqueue(
            Command::Plugin {
                project,
                operation_id,
                action,
            },
            cx,
        ) {
            self.extensions.operations.insert(operation_id, operation);
            Some(operation_id)
        } else {
            None
        }
    }

    pub(in crate::workbench) fn plugin_control(
        &mut self,
        action: PluginAction,
        operation: Operation,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let operation_id = Uuid::new_v4();
        self.extensions.operations.insert(operation_id, operation);
        let commands = self.commands.clone();
        cx.spawn(async move |this, cx| {
            let result = commands
                .send(Command::Plugin {
                    project,
                    operation_id,
                    action,
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    this.extensions.operations.remove(&operation_id);
                    if this.project == Some(project) {
                        this.fail(format!("插件控制无法提交：{error}"), cx);
                    }
                });
            }
        })
        .detach();
    }

    pub(in crate::workbench) fn inspect_plugin(
        &mut self,
        replace: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() || !self.backend_available() {
            return;
        }
        let directory = if let Some(id) = replace {
            let Some(instance) = self.extensions.instance(id) else {
                return;
            };
            instance.info.directory.clone()
        } else {
            match editing::input_snapshot(&self.extensions.directory, window, cx) {
                Ok(value) if !value.trim().is_empty() => PathBuf::from(value.as_ref()),
                Ok(_) => {
                    self.fail(
                        "请选择或输入插件包目录或归档文件（.tar.gz / .tgz / .mio-plugin）。",
                        cx,
                    );
                    return;
                }
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            }
        };
        if self
            .plugin_action(
                PluginAction::Inspect { directory },
                Operation::Inspect { replace },
                cx,
            )
            .is_some()
        {
            self.status = "正在检查 manifest 与原生可执行入口；尚未执行插件。".into();
        }
    }

    pub(in crate::workbench) fn choose_plugin(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() || !self.backend_available() {
            return;
        }
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: true,
                multiple: false,
                prompt: Some("选择本地插件包（目录或 .tar.gz / .tgz / .mio-plugin 文件）".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.finish_file_picker(picker, cx) || !this.backend_available() {
                    return;
                }
                if this.project != project {
                    this.fail("项目已切换，本次文件选择未应用到新项目。", cx);
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.extensions.directory.update(cx, |state, cx| {
                                state.set_value(path.to_string_lossy().into_owned(), window, cx)
                            });
                            this.inspect_plugin(None, window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.fail(format!("无法选择插件包：{error}"), cx),
                    Err(_) => this.fail("插件选择器已关闭。", cx),
                }
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn load_plugin(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() {
            return;
        }
        let Some(inspection) = &self.extensions.inspection else {
            return;
        };
        if !inspection.trusted {
            self.fail(
                "必须明确确认信任此 native 本地程序后才能加载。",
                cx,
            );
            return;
        }
        let replace = inspection.replace;
        let action = PluginAction::Load {
            inspection_id: inspection.info.id,
            native_trusted: true,
            grants: inspection.grants.iter().copied().collect(),
            replace_instance: replace,
        };
        if self
            .plugin_action(action, Operation::Load { replace }, cx)
            .is_some()
        {
            self.extensions.inspection = None;
            self.extensions.add_panel_open = false;
            self.status = "正在启动已确认的 native worker…".into();
        }
    }

    pub(in crate::workbench) fn reorder_plugin(
        &mut self,
        id: Uuid,
        move_up: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if !self.extensions.instance(id).is_some_and(|instance| {
            instance.live
                && instance.info.tab_id.is_none()
                && instance.context_owner.is_none()
                && self.project == Some(instance.info.identity.project)
        }) {
            return;
        }
        let _ = self.plugin_action(
            PluginAction::Reorder {
                instance_id: id,
                move_up,
            },
            Operation::Reorder,
            cx,
        );
    }

    pub(in crate::workbench) fn reload_plugin(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if !self.extensions.instance(id).is_some_and(|instance| {
            instance.live
                && instance.info.tab_id.is_none()
                && instance.context_owner.is_none()
                && self.project == Some(instance.info.identity.project)
        }) {
            return;
        }
        if self
            .plugin_action(
                PluginAction::Reload { instance_id: id },
                Operation::Reload { instance: id },
                cx,
            )
            .is_some()
        {
            self.status = "正在热重载插件…".into();
        }
    }

    pub(in crate::workbench) fn enable_installed_plugin(
        &mut self,
        placeholder_id: Uuid,
        plugin_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.extensions.extensions_disabled {
            self.fail(
                "当前项目已禁用扩展（Disable extensions），无法启用扩展。",
                cx,
            );
            return;
        }
        if let Some(inst) = self.extensions.instance_mut(placeholder_id)
            && !inst.live
        {
            inst.status = "正在启用持久化扩展…".into();
        }
        if self
            .plugin_action(
                PluginAction::EnableInstalled { plugin_id },
                Operation::EnableInstalled { placeholder_id },
                cx,
            )
            .is_some()
        {
            self.status = "正在启用持久化扩展…".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn set_plugin_auto_reload(
        &mut self,
        id: Uuid,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if !self.extensions.instance(id).is_some_and(|instance| {
            instance.live
                && instance.info.tab_id.is_none()
                && instance.context_owner.is_none()
                && self.project == Some(instance.info.identity.project)
        }) {
            return;
        }
        if self
            .plugin_action(
                PluginAction::SetAutoReload {
                    instance_id: id,
                    enabled,
                },
                Operation::SetAutoReload { instance: id },
                cx,
            )
            .is_some()
        {
            self.status = if enabled {
                "正在启用插件文件变更自动重载…".into()
            } else {
                "正在关闭插件自动重载…".into()
            };
            cx.notify();
        }
    }

    pub(in crate::workbench) fn active_context_menu_plugins(&self) -> Vec<(Uuid, String)> {
        self.extensions
            .instances
            .iter()
            .filter(|instance| {
                instance.live
                    && instance.info.tab_id.is_none()
                    && instance.context_owner.is_none()
                    && instance.info.grants.contains(&Permission::TrafficRead)
                    && instance
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "context_menu")
            })
            .map(|instance| {
                (
                    instance.info.identity.instance_id,
                    instance.info.manifest.id.clone(),
                )
            })
            .collect()
    }

    pub(in crate::workbench) fn invoke_plugin_context_menu(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let payload = match self.extract_plugin_context_menu_target(window, cx) {
            Ok(payload) => payload,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if self
            .plugin_action(
                PluginAction::InvokeContextMenu {
                    instance_id: id,
                    source: payload.source,
                    action_id: "default".into(),
                    selection_start: payload.selection_start,
                    selection_end: payload.selection_end,
                    selected_bytes: payload.selected_bytes,
                    request: payload.request,
                    response: payload.response,
                },
                Operation::ContextMenu { instance: id },
                cx,
            )
            .is_some()
        {
            self.status = "正在执行插件上下文动作…".into();
        }
    }

    pub(in crate::workbench) fn open_plugin_context_repeater(
        &mut self,
        request: mio_core::HttpRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let composition_error = self.capture_drafts(window, cx).err();
        let mut number = self.workspace.tabs.len() + self.workspace.closed.len() + 1;
        while self
            .workspace
            .tabs
            .iter()
            .chain(&self.workspace.closed)
            .any(|tab| tab.title == number.to_string())
        {
            number += 1;
        }
        let id = Uuid::new_v4();
        let previous = self.workspace.active;
        let previous_ws = self.workspace.active_websocket_tab;
        self.workspace.tabs.push(WorkTab {
            id,
            title: number.to_string(),
            group: None,
            last_flow: None,
            archive_response: None,
            history: None,
            draft: Draft {
                revision: 1,
                target: request.url.clone(),
                raw: request_to_raw(&request),
                version: request.version,
            },
        });
        if composition_error.is_none() {
            self.workspace.active = Some(id);
            self.workspace.active_websocket_tab = None;
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.pop();
            self.workspace.active = previous;
            self.workspace.active_websocket_tab = previous_ws;
            self.fail(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        if composition_error.is_none() {
            self.tool = Tool::Repeater;
        } else if let Some(error) = composition_error {
            self.fail(error, cx);
        }
    }

    pub(in crate::workbench) fn apply_plugin_order(&mut self, order: Vec<Uuid>) {
        if !self.extensions.persisted_order.is_empty() {
            let ordered_live_ids: Vec<String> = order
                .iter()
                .filter_map(|id| {
                    self.extensions
                        .instance(*id)
                        .filter(|inst| {
                            inst.persisted
                                && inst.info.tab_id.is_none()
                                && inst.context_owner.is_none()
                        })
                        .map(|inst| inst.info.manifest.id.clone())
                })
                .collect();
            let live_set: HashSet<&str> = ordered_live_ids.iter().map(String::as_str).collect();
            let mut live_iter = ordered_live_ids.iter().cloned();
            let mut updated = Vec::with_capacity(self.extensions.persisted_order.len());
            for id in &self.extensions.persisted_order {
                if live_set.contains(id.as_str()) {
                    if let Some(next_id) = live_iter.next() {
                        updated.push(next_id);
                    }
                } else {
                    updated.push(id.clone());
                }
            }
            for remaining in live_iter {
                updated.push(remaining);
            }
            self.extensions.persisted_order = updated;
        }
        self.apply_plugin_order_with_persisted(&order);
    }

    pub(in crate::workbench) fn apply_plugin_context_replace_request(
        &mut self,
        raw_request: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        match self.tool {
            Tool::Repeater => {
                let Some(tab_id) = self.workspace.active else {
                    return;
                };
                if self.plugin_waiting(tab_id) || self.sending.contains_key(&tab_id) {
                    self.fail("当前 Repeater 标签正在发送或等待插件提交；未替换草稿。", cx);
                    return;
                }
                let _ = self.capture_drafts(window, cx);
                let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                let Some(next_revision) = tab.draft.revision.checked_add(1) else {
                    self.fail("草稿 revision 已耗尽；无法原地替换请求。", cx);
                    return;
                };
                tab.draft.raw = raw_request.clone();
                tab.draft.revision = next_revision;
                if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                    let prev_view = editors.request.view;
                    editors.request.load(&raw_request, window, cx);
                    let _ = editors.request.switch(prev_view, window, cx);
                    editors.loaded_revision = next_revision;
                    editors.dirty = false;
                }
                self.save_workspace(cx);
                self.extensions.push_log("已原地替换当前 Repeater 请求草稿");
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let Some(id) = self.selected_intercept else {
                    return;
                };
                if let Some(pending) = self.intercepts.iter_mut().find(|p| {
                    p.id() == id
                        && !p.deciding
                        && matches!(
                            p.target,
                            InterceptTarget::Http {
                                stage: InterceptStage::Request,
                                ..
                            }
                        )
                }) {
                    let prev_view = pending.editor.view;
                    pending.editor.load(&raw_request, window, cx);
                    let _ = pending.editor.switch(prev_view, window, cx);
                    self.extensions.push_log("已原地替换当前拦截请求报文");
                }
            }
            _ => {}
        }
        cx.notify();
    }
}
