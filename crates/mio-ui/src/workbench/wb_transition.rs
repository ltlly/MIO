use super::*;

impl Workbench {
    pub(in crate::workbench) fn request_transition(
        &mut self,
        action: AfterSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.startup_wizard_active() {
            if matches!(action, AfterSave::Quit) {
                self.request_startup_wizard_quit(window, cx);
            }
            return;
        }
        if self.runtime_result.is_none()
            && self.intruder.resource_pools.management.mutation_pending()
        {
            self.intruder.resource_pools.management.cancel_pending();
            self.reject_quit(cx);
            self.fail(
                "资源池修改尚未确认；已请求取消，等待实际结果后再切换项目或退出。",
                cx,
            );
            return;
        }
        if self.quitting || self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        self.cancel_history_transfer("已请求切换项目或退出；未转送所选请求。", cx);
        if self.block_http_notes_batch(cx) {
            return;
        }
        let quitting = matches!(action, AfterSave::Quit);
        if quitting {
            if cx
                .global::<crate::native_terminate::NativeTerminate>()
                .rejected()
            {
                self.reject_quit(cx);
                return;
            }
            self.quit_attempt.get_or_insert_with(mio_core::Uuid::new_v4);
        }
        if self.runtime_result.is_some() {
            if quitting {
                self.confirm_joined_exit(window, cx);
            } else {
                self.fail("后台服务已退出；当前编辑仍保留，继续工作需要重新启动。", cx);
            }
            return;
        }
        let can_save = self.backend_available();
        if !can_save && !quitting {
            self.fail("后台正在退出；当前编辑仍保留，请等待线程回收确认。", cx);
            return;
        }
        if can_save && self.ext_import.blocks_transition() {
            self.reject_quit(cx);
            self.ext_import.visible = true;
            self.fail(
                "请先取消导入或关闭预检，并等待后台清理完成后再切换项目或退出。",
                cx,
            );
            return;
        }
        if !self.intruder.session_runs.is_empty() {
            self.reject_quit(cx);
            self.focus.focus(window, cx);
            self.open_dashboard(cx);
            self.fail(format!("还有 {} 个 Intruder 攻击仅保留在内存。请在 Dashboard 的攻击 Settings 中保存到项目，或明确丢弃，再切换项目或退出。普通保存不会包含这些攻击。", self.intruder.session_runs.len()), cx);
            return;
        }
        if !self.plugins_before_quit(&action, window, cx) {
            return;
        }
        self.ext_repeater_save_failed("已请求切换项目或退出，未载入归档请求。", cx);
        self.intruder_save_failed(cx);
        let has_content = self.has_traffic
            || self.websocket_notes_unsaved(cx)
            || self.http_notes_unsaved(cx)
            || self.http_highlights_unsaved()
            || self.repeater_target_dialog_unsaved(cx)
            || self.workspace.repeater_settings != RepeaterSettings::default()
            || !self.workspace.match_and_replace_rules.is_empty()
            || !self.workspace.ssl_pass_through_rules.is_empty()
            || !self.workspace.ws_match_and_replace_rules.is_empty()
            || !self.workspace.proxy_interception.is_default()
            || !self.workspace.proxy_messages.is_default()
            || !self.workspace.proxy_websocket_interception.is_default()
            || !self.workspace.scope.is_empty()
            || !self.workspace.upstream_proxy.is_default()
            || !self.workspace.upstream_tls.is_default()
            || !self.workspace.proxy_listeners.is_empty()
            || self.workspace_dirty
            || self.pending_workspace.is_some()
            || !self.workspace.tabs.is_empty()
            || !self.workspace.closed.is_empty()
            || !self.workspace.websocket_tabs.is_empty()
            || !self.workspace.intruder.tabs.is_empty()
            || !self.workspace.intruder.closed.is_empty()
            || self.proxy_address.is_some()
            || self.proxy_busy
            || !self.sending.is_empty()
            || !self.pending_sends.is_empty()
            || !self.intercepts.is_empty();
        if !has_content {
            self.perform_transition(action, window, cx);
            return;
        }
        if self.project_path.is_some() && !quitting && self.pending_records == 0 {
            self.begin_saved_transition(action, cx);
            return;
        }
        let can_discard = quitting || self.pending_records == 0;
        let message = if !can_save {
            "后台正在退出，未保存的编辑无法写入项目。继续退出？".into()
        } else if self.pending_records > 0 {
            format!("还有 {} 条流量尚未写入项目", self.pending_records)
        } else if self.project_path.is_some() {
            "保存当前项目后退出？".into()
        } else {
            "保存当前临时项目？".into()
        };
        let choices = if !can_save {
            vec!["放弃并继续", "取消"]
        } else if can_discard {
            vec!["保存", "放弃并继续", "取消"]
        } else {
            vec!["保存", "取消"]
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("未保存的流量、工作标签与项目设置在关闭项目后无法恢复。在途请求将停止，不会自动重发。"),
            &choices,
            cx,
        );
        self.transition = Some(TransitionPhase::Confirming);
        let attempt = self.quit_attempt;
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = owner.update(cx, |_, window, cx| {
                this.update(cx, |view, cx| {
                    if view.transition != Some(TransitionPhase::Confirming) {
                        return;
                    }
                    view.transition = None;
                    if !view.quit_answer_is_current(attempt, cx) {
                        cx.notify();
                        return;
                    }
                    match answer {
                        Ok(0) if can_save => view.begin_saved_transition(action, cx),
                        Ok(choice) if can_discard && choice == usize::from(can_save) => {
                            view.perform_transition(action, window, cx)
                        }
                        _ => {
                            view.reject_quit(cx);
                            view.send_http_highlights(false, cx);
                            view.send_http_notes(false, cx);
                            view.send_websocket_notes(false, cx);
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn begin_saved_transition(
        &mut self,
        action: AfterSave,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            self.reject_quit(cx);
            self.fail("当前项目不可用，本轮保存与退出已取消。", cx);
            return;
        };
        if self.transition.is_some() {
            return;
        }
        self.invalidate_all_plugin_sends(cx);
        if self.enqueue(Command::Quiesce { project }, cx) {
            self.after_save = Some(action);
            self.transition = Some(TransitionPhase::Quiescing);
            self.pending_sends.clear();
            self.pending_project_save = None;
            self.status = "正在停止连接并收齐最后的响应；随后保存项目…".into();
            cx.notify();
        } else {
            self.reject_quit(cx);
        }
    }

    pub(in crate::workbench) fn cancel_transition(&mut self, cx: &mut Context<Self>) {
        self.notes_save_wait = None;
        self.reject_quit(cx);
        // A visible sheet keeps ownership until its expired answer is consumed.
        if self.transition.is_none()
            || matches!(
                self.transition,
                Some(
                    TransitionPhase::Confirming
                        | TransitionPhase::Resuming
                        | TransitionPhase::ShuttingDown
                )
            )
        {
            return;
        }
        self.after_save = None;
        self.pending_project_save = None;
        self.transition = Some(TransitionPhase::Resuming);
        self.status = "已取消项目切换/退出；监听与请求不会自动重启。".into();
        let Some(project) = self.project else {
            self.transition = None;
            return;
        };
        let commands = self.commands.clone();
        // Cancellation must survive a full data queue without blocking the GPUI thread.
        cx.spawn(async move |this, cx| {
            let result = commands.send(Command::Resume { project }).await;
            let _ = this.update(cx, |view, cx| {
                if view.project == Some(project)
                    && view.transition == Some(TransitionPhase::Resuming)
                {
                    view.transition = None;
                    if result.is_err() {
                        view.fail("后台服务已断开；取消后的草稿仍保留在当前窗口。", cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn perform_transition(
        &mut self,
        action: AfterSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = match action {
            AfterSave::NewProject => Command::NewProject { path: None },
            AfterSave::OpenProject(path) => Command::OpenProject { path },
            AfterSave::Quit => {
                self.begin_shutdown(window, cx);
                return;
            }
        };
        if self.enqueue(command, cx) {
            self.transition = Some(TransitionPhase::Switching);
            self.focus.focus(window, cx);
            self.status = "正在完成项目操作并回收在途连接…".into();
        } else {
            self.cancel_transition(cx);
        }
    }
}
