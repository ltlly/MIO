use mio_core::{UserPreferences, UserPreferencesState, Uuid};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn receive_user_preferences(
        &mut self,
        operation: Uuid,
        state: UserPreferencesState,
        cx: &mut Context<Self>,
    ) {
        self.expire_user_preferences(cx);
        if self
            .user_preferences
            .query
            .as_ref()
            .map(|(operation, _, _)| *operation)
            != Some(operation)
        {
            return;
        }
        self.user_preferences.query = None;
        if state.load_error.is_none()
            && state.preferences.version == 1
            && self
                .user_preferences
                .confirmed
                .as_ref()
                .is_some_and(|current| {
                    current.revision == state.preferences.revision && current != &state.preferences
                })
        {
            self.user_preferences.load_error = Some(
                "用户设置回执在相同 revision 下包含不同选项；保留原设置并暂使用询问策略。".into(),
            );
            self.user_preferences.status = "用户设置回执冲突，请重新读取核对。".into();
            cx.notify();
            return;
        }
        if self
            .user_preferences
            .confirmed
            .as_ref()
            .is_some_and(|current| state.preferences.revision < current.revision)
            && state.load_error.is_none()
            && state.preferences.version == 1
        {
            self.user_preferences.status = "已忽略旧的用户设置回执。".into();
            cx.notify();
            return;
        }
        let load_error = state
            .load_error
            .or_else(|| {
                (state.preferences.version != 1)
                    .then(|| "不支持的用户设置版本；暂使用询问策略。".into())
            })
            .or_else(|| {
                mio_core::UpstreamTlsSettings {
                    custom_ca_ders: state.preferences.custom_ca_ders.clone(),
                    include_system_roots: true,
                    insecure_skip_verify: false,
                    ..Default::default()
                }
                .validate()
                .err()
                .map(|error| format!("用户 TLS 证书回执无效：{error}"))
            })
            .or_else(|| {
                state
                    .preferences
                    .display
                    .validate()
                    .and_then(|()| state.preferences.message_editor.validate())
                    .and_then(|()| state.preferences.misc.validate())
                    .err()
                    .map(|error| format!("用户界面设置回执无效：{error}"))
            });
        if load_error.is_none() {
            if !self.user_preferences.retain_selection && self.user_preferences.pending.is_none() {
                self.user_preferences.running = state.preferences.running;
                self.user_preferences.finished = state.preferences.finished;
            }
            if !self.user_preferences.retain_workbench && self.user_preferences.pending.is_none() {
                self.user_preferences
                    .display
                    .clone_from(&state.preferences.display);
                self.user_preferences
                    .message_editor
                    .clone_from(&state.preferences.message_editor);
                self.user_preferences
                    .misc
                    .clone_from(&state.preferences.misc);
            }
            theme::apply_user_display(
                &state.preferences.display,
                &state.preferences.message_editor,
                cx,
            );
            theme::apply_user_misc(&state.preferences.misc, cx);
            let auto_expand_inspector = state.preferences.message_editor.auto_expand_inspector;
            self.user_preferences.confirmed = Some(state.preferences);
            self.apply_confirmed_workbench_preferences(auto_expand_inspector, cx);
            #[cfg(not(test))]
            if !self.startup_update_checked && self.user_preferences.misc.auto_check_updates {
                self.startup_update_checked = true;
                let _ = self
                    .commands
                    .try_send(mio_core::Command::CheckForUpdates { manual: false });
            }
            self.user_preferences.status = if self.user_preferences.error.is_some() {
                "已重新读取磁盘设置；上次保存错误仍保留，请核对所选选项后明确重试。".into()
            } else {
                "用户设置已读取。".into()
            };
        } else {
            self.user_preferences.confirmed = None;
            self.user_preferences.status = "用户设置未载入，关闭时将询问。".into();
        }
        self.user_preferences.load_error = load_error;
        cx.notify();
    }

    pub(in crate::workbench) fn receive_user_preferences_saved(
        &mut self,
        operation: Uuid,
        result: Result<UserPreferences, String>,
        cx: &mut Context<Self>,
    ) {
        self.expire_user_preferences(cx);
        let Some(pending) = self
            .user_preferences
            .pending
            .as_ref()
            .filter(|pending| pending.operation == operation)
            .cloned()
        else {
            return;
        };
        self.user_preferences.pending = None;
        let result = result.and_then(|saved| {
            if saved.version != 1
                || pending.expected_revision.checked_add(1) != Some(saved.revision)
                || saved.running != pending.running
                || saved.finished != pending.finished
                || saved.display != pending.display
                || saved.message_editor != pending.message_editor
                || saved.misc != pending.misc
                || saved.custom_ca_ders != pending.certificates
                || self
                    .user_preferences
                    .confirmed
                    .as_ref()
                    .is_some_and(|current| {
                        current.revision > saved.revision
                            || (current.revision == saved.revision && current != &saved)
                    })
            {
                Err("用户设置保存回执不匹配；结果未确认。".into())
            } else {
                Ok(saved)
            }
        });
        match result {
            Ok(saved) => {
                // Retire an older query so it cannot roll back this acknowledged save.
                self.user_preferences.query = None;
                match pending.origin {
                    SaveOrigin::Settings => {
                        self.user_preferences.running = saved.running;
                        self.user_preferences.finished = saved.finished;
                    }
                    SaveOrigin::Workbench => {
                        self.user_preferences.display = saved.display.clone();
                        self.user_preferences.message_editor = saved.message_editor.clone();
                        self.user_preferences.misc = saved.misc.clone();
                    }
                    SaveOrigin::Remember(RememberCloseChoice::Running(_)) => {
                        self.user_preferences.running = saved.running
                    }
                    SaveOrigin::Remember(RememberCloseChoice::Finished(_)) => {
                        self.user_preferences.finished = saved.finished
                    }
                    SaveOrigin::Tls => {}
                }
                self.user_preferences.retain_selection = self.user_preferences.running
                    != saved.running
                    || self.user_preferences.finished != saved.finished;
                self.user_preferences.retain_workbench = self.user_preferences.display
                    != saved.display
                    || self.user_preferences.message_editor != saved.message_editor
                    || self.user_preferences.misc != saved.misc;
                theme::apply_user_display(&saved.display, &saved.message_editor, cx);
                theme::apply_user_misc(&saved.misc, cx);
                let auto_expand_inspector = saved.message_editor.auto_expand_inspector;
                self.user_preferences.confirmed = Some(saved);
                self.apply_confirmed_workbench_preferences(auto_expand_inspector, cx);
                self.user_preferences.load_error = None;
                self.user_preferences.error = None;
                self.user_preferences.status = if matches!(pending.origin, SaveOrigin::Tls) {
                    "用户 TLS 证书已保存；关闭行为的未保存选择仍保留。".into()
                } else if self.user_preferences.retain_selection {
                    "本次关闭类别的用户设置已保存；另一类所选选项尚未保存。".into()
                } else {
                    "用户设置已保存。".into()
                };
            }
            Err(error) => {
                self.user_preferences.error = Some(format!("用户设置保存失败：{error}"));
                self.user_preferences.load_error =
                    Some("保存结果尚未核对，暂使用询问策略。".into());
                self.user_preferences.query = None;
                self.query_user_preferences(cx);
                if matches!(pending.origin, SaveOrigin::Remember(_)) {
                    self.remember_preference_failed(&error, cx);
                }
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn apply_confirmed_workbench_preferences(
        &mut self,
        auto_expand_inspector: bool,
        cx: &mut Context<Self>,
    ) {
        if auto_expand_inspector {
            self.inspector.open = true;
        }
        let owner_window = self.owner_window;
        let weak = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = owner_window.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |view, cx| {
                    view.refresh_editor_preferences_in_window(window, cx);
                });
            });
        });
    }

    pub(in crate::workbench) fn refresh_editor_preferences_in_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for editors in self.tab_editors.values_mut() {
            editors.request.apply_editor_preferences(window, cx);
            editors.response.apply_editor_preferences(window, cx);
        }
        let mut detached_views = Vec::new();
        for (&view_id, result_view) in &mut self.intruder.results.views {
            if result_view.owner == self.owner_window {
                result_view.request.apply_editor_preferences(window, cx);
                result_view.response.apply_editor_preferences(window, cx);
            } else {
                detached_views.push((view_id, result_view.owner));
            }
        }
        if !detached_views.is_empty() {
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                for (view_id, owner) in detached_views {
                    let weak = weak.clone();
                    let _ = owner.update(cx, |_, det_window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            if let Some(rv) = view.intruder.results.views.get_mut(&view_id)
                                && rv.owner == owner
                            {
                                rv.request.apply_editor_preferences(det_window, cx);
                                rv.response.apply_editor_preferences(det_window, cx);
                            }
                        });
                    });
                }
            });
        }
        self.history_request.apply_editor_preferences(window, cx);
        self.history_response.apply_editor_preferences(window, cx);
        for pending in &mut self.intercepts {
            pending.editor.apply_editor_preferences(window, cx);
        }
        for editors in self.websocket_repeater.editors.values_mut() {
            editors.draft.apply_editor_preferences(window, cx);
            editors.history.apply_editor_preferences(window, cx);
        }
        self.websocket.payload.apply_editor_preferences(window, cx);
        self.target_pane
            .request
            .apply_editor_preferences(window, cx);
        self.target_pane
            .response
            .apply_editor_preferences(window, cx);
        self.extra_tools
            .logger_request
            .apply_editor_preferences(window, cx);
        self.extra_tools
            .logger_response
            .apply_editor_preferences(window, cx);
        self.sequencer_pane
            .request
            .apply_editor_preferences(window, cx);
        let soft_wrap = theme::editor_soft_wrap();
        for raw_editor in [
            &self.extra_tools.decoder_input,
            &self.extra_tools.decoder_output,
            &self.extra_tools.comparer_view,
            &self.extra_tools.comparer_left_view,
            &self.extra_tools.comparer_right_view,
            &self.sequencer_pane.preview_samples,
            &self.sequencer_pane.manual_editor,
        ] {
            raw_editor.update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        }
        self.collaborator_pane
            .raw_editor
            .apply_editor_preferences(window, cx);
        self.organiser.request.apply_editor_preferences(window, cx);
        self.organiser.response.apply_editor_preferences(window, cx);
        self.audit_dashboard
            .scan_request_editor
            .apply_editor_preferences(window, cx);
        self.audit_dashboard
            .issue_request_editor
            .apply_editor_preferences(window, cx);
        self.audit_dashboard
            .issue_response_editor
            .apply_editor_preferences(window, cx);
        if let Some(editor) = self.ext_import.message_editor() {
            editor.apply_editor_preferences(window, cx);
        }
        self.history.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
        self.websocket.table.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
        self.organiser.table.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
        if let Some(wizard) = &self.startup_wizard {
            wizard.recent_table.update(cx, |table, cx| {
                table.refresh(cx);
                cx.notify();
            });
        }
        self.ext_import.refresh_tables(soft_wrap, window, cx);
        if self.inspector.open {
            self.sync_inspector_selection(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn user_preferences_disconnected(&mut self, cx: &mut Context<Self>) {
        let remember = self
            .user_preferences
            .pending
            .take()
            .is_some_and(|pending| matches!(pending.origin, SaveOrigin::Remember(_)));
        self.user_preferences.query = None;
        self.user_preferences.load_error =
            Some("后台已断开；用户设置未确认，所选选项仍保留。".into());
        self.user_preferences.status = "关闭时暂使用询问策略。".into();
        if remember {
            self.user_preferences.error = Some("下次关闭的选择因后台断开而未确认。".into());
            self.remember_preference_failed("后台已断开。", cx);
            let owner = cx.weak_entity();
            // The caller also reports backend shutdown; retain this independent preference failure.
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| {
                    if !view.backend_available() {
                        view.remember_preference_failed("后台已断开。", cx);
                    }
                });
            });
        }
        cx.notify();
    }
}
