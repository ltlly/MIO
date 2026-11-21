use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shutdown_requested || self.runtime_result.is_some() {
            return;
        }
        self.cancel_expired_quit(cx);
        self.expire_intruder_close_if_due(cx);
        let prev_status = self.status.clone();
        let prev_error = self.error.clone();
        let prev_storage_error = self.storage_error.clone();
        self.observe_footer_event_before_dispatch(&event, window, cx);
        let flow = match event {
            Event::UserPreferences { .. }
            | Event::UserPreferencesSaved { .. }
            | Event::ProjectOpened { .. }
            | Event::ProjectDiskUsage { .. }
            | Event::ProjectSaved { .. }
            | Event::ProjectSlimSaved { .. }
            | Event::Quiesced { .. }
            | Event::WorkspaceChanged { .. }
            | Event::WorkspaceRejected { .. }
            | Event::RepeaterOpened { .. } => self.on_project_workspace_event(event, window, cx),
            Event::SiteMapEntryAdded { .. }
            | Event::SiteMapEntryRead { .. }
            | Event::ExtResponse { .. }
            | Event::ExtMessage { .. }
            | Event::SiteMapPage { .. }
            | Event::GlobalSearchPage { .. }
            | Event::Sequencer(_)
            | Event::Audit(_)
            | Event::Collaborator(_)
            | Event::RepeaterHistoryPage { .. }
            | Event::RepeaterHistorySelected { .. }
            | Event::ExtJob { .. }
            | Event::ExtProgress { .. }
            | Event::ExtReport { .. }
            | Event::ExtItems { .. }
            | Event::ExtItem { .. }
            | Event::ExtRepeaterOpened { .. }
            | Event::ExtMessages { .. }
            | Event::WebSocketHistoryChanged { .. }
            | Event::WebSocketHistoryPage { .. }
            | Event::WebSocketHistoryError { .. }
            | Event::WebSocketDetail { .. }
            | Event::WebSocketDetailError { .. }
            | Event::WebSocketNotes { .. }
            | Event::WebSocketNotesChanged { .. }
            | Event::WebSocketRepeater(_)
            | Event::IntruderPositions { .. }
            | Event::IntruderStarted { .. }
            | Event::IntruderDefaultPoolReady { .. }
            | Event::IntruderResourcePools { .. }
            | Event::IntruderResourcePoolChanged { .. }
            | Event::IntruderPoolUsage { .. }
            | Event::IntruderResourcePoolsUpdated { .. }
            | Event::IntruderResourcePoolDeleted { .. }
            | Event::IntruderRenamed { .. }
            | Event::IntruderBaselineRequested { .. }
            | Event::IntruderDiscarded { .. }
            | Event::IntruderRetentionChanged { .. }
            | Event::IntruderChanged { .. }
            | Event::IntruderResults { .. }
            | Event::IntruderResultView { .. }
            | Event::IntruderResultNotes { .. }
            | Event::IntruderResultHighlight { .. }
            | Event::IntruderRuns { .. }
            | Event::IntruderRunControl { .. }
            | Event::IntruderSpec { .. }
            | Event::HistoryChanged { .. }
            | Event::HttpHistoryDeleted { .. }
            | Event::HistoryPage { .. }
            | Event::LoggerHistoryPage { .. }
            | Event::HttpHistoryPage { .. } => self.on_tool_and_history_event(event, window, cx),
            Event::FlowDetail { .. }
            | Event::FlowDetailError { .. }
            | Event::ProxyState { .. }
            | Event::ProxyListeners { .. }
            | Event::BrowserLaunched { .. }
            | Event::LiveFlowAnnotated { .. }
            | Event::InterceptPending { .. }
            | Event::InterceptResolved { .. }
            | Event::WebSocketInterceptPending { .. }
            | Event::WebSocketInterceptResolved { .. } => {
                self.on_traffic_and_intercept_event(event, window, cx)
            }
            _ => self.on_send_and_system_event(event, window, cx),
        };
        self.observe_footer_state_after_dispatch(&prev_status, &prev_error, &prev_storage_error);
        if flow.is_break() {
            return;
        }
        self.advance_intruder_result_close(cx);
        if !self.websocket_notes_pending()
            && !self.http_notes_pending()
            && !self.http_highlights_pending()
        {
            if let Some(save_as) = self.notes_save_wait.take() {
                self.save_project(save_as, window, cx);
            } else if self.pending_project_save.is_some() {
                self.flush_project_save(window, cx);
            }
        }
        cx.notify();
    }

    fn on_project_workspace_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ControlFlow<()> {
        match event {
            Event::UserPreferences {
                operation_id,
                state,
            } => {
                if !self.receive_startup_wizard_preferences(operation_id, &state, cx) {
                    self.receive_user_preferences(operation_id, state, cx);
                }
                self.sync_startup_wizard_preferences(cx);
                if !cfg!(test) && self.startup_wizard_active() && theme::ui_scale() > 1.0 {
                    let frame = window.bounds().size;
                    let viewport = window.viewport_size();
                    let mut target_w = theme::scaled_px(902.);
                    let mut target_h =
                        theme::scaled_px(610. + crate::chrome::CUSTOM_TITLE_BAR_HEIGHT);
                    if let Some(display) = window.display(cx) {
                        let visible = display.visible_bounds().size;
                        target_w = target_w.min(visible.width);
                        target_h = target_h.min(visible.height);
                    }
                    let w = target_w - (frame.width - viewport.width);
                    let h = target_h - (frame.height - viewport.height);
                    if w > viewport.width || h > viewport.height {
                        window.resize(gpui_kit::size(
                            w.max(viewport.width),
                            h.max(viewport.height),
                        ));
                    }
                }
            }
            Event::UserPreferencesSaved {
                operation_id,
                result,
            } => {
                if !self.receive_startup_wizard_preferences_saved(operation_id, &result, window, cx) {
                    self.receive_user_preferences_saved(operation_id, result, cx);
                }
                self.sync_startup_wizard_preferences(cx);
            }
            Event::ProjectOpened {
                project,
                path,
                workspace,
            } => {
                let initial_startup = self.startup_initial_project_opening(path.as_ref());
                let finished_startup =
                    self.startup_wizard_project_opened(path.as_ref(), window, cx);
                if self.quit_attempt.is_some() && !initial_startup {
                    self.reject_quit(cx);
                }
                if !self.startup_wizard_active() {
                    // Project-owned controls are about to leave the focus/dispatch tree.
                    self.focus.focus(window, cx);
                }
                self.plugins_project_changed(cx);
                self.reject_intruder_result_close("项目已切换；旧结果窗口的关闭操作已取消。", cx);
                self.intruder_project_changed();
                if let Some(cancel) = self.proxy_cancel.take() {
                    cancel.cancel();
                }
                self.proxy_address = None;
                self.proxy_listeners_bound.clear();
                self.annotated_live_flows.clear();
                self.live_http_annotations.clear();
                self.proxy_busy = false;
                self.project = Some(project);
                self.ext_project_opened(path.as_ref(), cx);
                self.project_path = path;
                self.project_disk_bytes = None;
                self.closed_repeater_order.clear();
                self.repeater_ui_caches.clear_caches();
                self.sequential_group_queue = None;
                self.editor_ui_caches.clear();
                self.collaborator_insert_sub = None;
                *self.workspace = workspace;
                self.listener_input.update(cx, |input, cx| {
                    input.set_value(
                        self.workspace.proxy_listener_or_default().to_string(),
                        window,
                        cx,
                    )
                });
                self.workspace_dirty = false;
                self.pending_workspace = None;
                self.pending_proxy_interception = None;
                self.pending_history_delete = None;
                self.pending_project_save = None;
                self.project_saves_in_flight = 0;
                self.notes_save_wait = None;
                self.project_notes_save = None;
                self.project_http_notes_save = None;
                self.project_http_highlights_save = None;
                self.project_save_status.clear();
                self.choosing_save_as = false;
                // An outstanding native picker keeps its token until its callback returns.
                self.saving_project_snapshot = false;
                self.pending_sends.clear();
                self.after_save = None;
                self.transition = None;
                self.storage_error = None;
                self.pending_records = 0;
                self.tab_width = if self.workspace.tab_sidebar_width == 0 {
                    220.
                } else {
                    f32::from(self.workspace.tab_sidebar_width)
                };
                self.tab_editors.clear();
                self.reset_websocket_repeater();
                for (_, send) in self.sending.drain() {
                    send.cancel.cancel();
                }
                self.close_after_send.clear();
                self.intercepts.clear();
                self.selected_intercept = None;
                self.selected_flow = None;
                self.selected_history = None;
                self.reset_imported_history(window, cx);
                self.reset_http_notes(window, cx);
                self.reset_extra_tools(window, cx);
                self.reset_target_pane(window, cx);
                self.reset_sequencer_pane(window, cx);
                self.reset_audit_dashboard(window, cx);
                self.reset_collaborator_pane(window, cx);
                self.reset_global_search(window, cx);
                self.reset_organiser(window, cx);
                self.reset_http_highlights(cx);
                self.reset_http_filter(cx);
                self.reset_history_transfer();
                self.inspector.clear_cache();
                self.observed_server_certs.clear();
                self.session_traces.clear();
                self.mio_ai_context = None;
                self.csrf_poc_modal = None;
                self.reset_history_version_view();
                self.clear_history_detail(window, cx);
                self.history_loading = false;
                self.history_dirty = true;
                self.websocket.reset(window, cx);
                self.has_traffic = false;
                self.query_revision += 1;
                self.selection_revision += 1;
                self.history_detail_revision = self.selection_revision;
                self.repeater_reads.clear();
                self.archive_responses = archive_response::ArchiveResponses::default();
                self.repeater_history = repeater_history::HistoryNavigation::default();
                self.history.update(cx, |state, cx| {
                    state.delegate_mut().rows.clear();
                    state.delegate_mut().selected_ids.clear();
                    state.delegate_mut().selected_rows.clear();
                    state.delegate_mut().selected_order.clear();
                    state.delegate_mut().selected_ranks.clear();
                    state.delegate_mut().selection_anchor = None;
                    state.delegate_mut().selection_lead = None;
                    state.delegate_mut().live_numbers.clear();
                    state.delegate_mut().row_metadata.clear();
                    state.delegate_mut().previous_offsets.clear();
                    state.delegate_mut().offset = 0;
                    state.delegate_mut().total = 0;
                    state.clear_selection(cx);
                    state.refresh(cx);
                });
                self.sync_tab_editors(window, cx);
                self.status = "项目已打开".into();
                self.reset_footer_drawer_on_project_switch(window, cx);
                self.update_title(window);
                if finished_startup {
                    self.finish_startup_wizard_after_open(window, cx);
                }
            }
            Event::ProjectSaved {
                project,
                path,
                total_bytes,
                websocket_notes,
                ext_http_notes,
                ext_http_highlights,
            } if self.project == Some(project) => {
                let finishes_transition = match &self.transition {
                    Some(TransitionPhase::Saving(expected)) if expected == &path => true,
                    Some(TransitionPhase::Saving(_)) => return ControlFlow::Break(()),
                    _ => false,
                };
                self.project_saves_in_flight = self.project_saves_in_flight.saturating_sub(1);
                self.project_path = Some(path.clone());
                self.project_disk_bytes = Some(total_bytes);
                let submitted = self.project_notes_save.take().unwrap_or_default();
                if let Err(error) =
                    self.accept_saved_websocket_notes(&submitted, &websocket_notes, window, cx)
                {
                    self.cancel_transition(cx);
                    self.fail(error, cx);
                    return ControlFlow::Break(());
                }
                let submitted_http = self.project_http_notes_save.take().unwrap_or_default();
                if let Err(error) =
                    self.accept_saved_http_notes(&submitted_http, &ext_http_notes, window, cx)
                {
                    self.cancel_transition(cx);
                    self.fail(error, cx);
                    return ControlFlow::Break(());
                }
                let submitted_highlights =
                    self.project_http_highlights_save.take().unwrap_or_default();
                if let Err(error) = self.accept_saved_http_highlights(
                    &submitted_highlights,
                    &ext_http_highlights,
                    cx,
                ) {
                    self.cancel_transition(cx);
                    self.fail(error, cx);
                    return ControlFlow::Break(());
                }
                self.project_save_status =
                    format!("项目已保存（{} 字节，含 sidecar）", total_bytes);
                self.update_title(window);
                self.intruder_close_project_saved(&path, window, cx);
                self.http_notes_batch_project_saved(cx);
                if finishes_transition {
                    self.transition = Some(TransitionPhase::Ready);
                    match self.capture_drafts(window, cx) {
                        Ok(changed) => {
                            if changed
                                || self.workspace_dirty
                                || self.pending_workspace.is_some()
                                || self.websocket_notes_unsaved(cx)
                                || self.http_notes_unsaved(cx)
                                || self.http_highlights_unsaved()
                            {
                                if (changed || self.workspace_dirty) && !self.save_workspace(cx) {
                                    self.cancel_transition(cx);
                                    return ControlFlow::Break(());
                                }
                                self.pending_project_save = Some(None);
                                self.flush_project_save(window, cx);
                            } else if let Some(action) = self.after_save.take() {
                                self.perform_transition(action, window, cx);
                            }
                        }
                        Err(error) => {
                            self.cancel_transition(cx);
                            self.fail(error, cx);
                        }
                    }
                }
                self.send_websocket_notes(false, cx);
                self.send_http_notes(false, cx);
                self.send_http_highlights(false, cx);
            }
            Event::ProjectDiskUsage { project, bytes } if self.project == Some(project) => {
                self.project_disk_bytes = Some(bytes);
            }
            Event::ProjectSlimSaved {
                project,
                path,
                result,
            } if self.project == Some(project) => match result {
                Ok(report) => {
                    self.status = format!(
                        "瘦身另存为已完成：{}（{:.2} MB → {:.2} MB，清理 {} 条冗余流量）",
                        path.display(),
                        report.old_bytes as f64 / 1_048_576.0,
                        report.new_bytes as f64 / 1_048_576.0,
                        report.removed_flows,
                    );
                }
                Err(message) => {
                    self.fail(format!("瘦身另存为失败：{message}"), cx);
                }
            },
            Event::Quiesced { project }
                if self.project == Some(project)
                    && self.transition == Some(TransitionPhase::Quiescing) =>
            {
                if self.pending_records > 0 {
                    self.cancel_transition(cx);
                    self.fail(
                        "停止期间有流量未写入项目；已保留当前窗口，请保存或另存为恢复。",
                        cx,
                    );
                } else {
                    self.transition = Some(TransitionPhase::Ready);
                    self.save_project(false, window, cx);
                }
            }
            Event::WorkspaceChanged { project, workspace } if self.project == Some(project) => {
                if self.pending_workspace == Some(workspace.revision)
                    && !self.intruder_close_owns_workspace()
                {
                    if !self.proxy_interception_workspace_acknowledged(&workspace, cx) {
                        return ControlFlow::Break(());
                    }
                    self.apply_workspace_ack(&workspace, window, cx);
                    self.advance_settings_pool_delete(cx);
                } else if self.pending_workspace.is_none()
                    && !self.workspace_dirty
                    && !self.intruder_close_owns_workspace()
                    && self.transition.is_none()
                    && !self.choosing_save_as
                    && self.pending_project_save.is_none()
                    && !self.saving_project_snapshot
                    && self.resize_original.is_none()
                    && self.rename.is_none()
                    && workspace.revision > self.workspace.revision
                {
                    let captured = (!self.has_unsaved_plugin_edits(cx))
                        .then(|| self.capture_drafts(window, cx));
                    match captured {
                        Some(Ok(false)) if !self.organiser.notes_dirty => {
                            *self.workspace = workspace.clone();
                            self.target_pane.advanced_mode_input =
                                self.workspace.scope.advanced_mode;
                            self.target_pane.editing_simple_scope = [None, None];
                            self.target_pane.editing_advanced_scope = [None, None];
                            self.target_pane.settings_editing_scope = [None, None];
                            let listener_focused =
                                self.listener_input.focus_handle(cx).is_focused(window)
                                    || self.listener_input.read(cx).has_marked_text();
                            if !listener_focused {
                                let listener_value =
                                    self.workspace.proxy_listener_or_default().to_string();
                                self.listener_input.update(cx, |input, cx| {
                                    input.set_value(listener_value, window, cx);
                                });
                            }
                            self.tab_width = if self.workspace.tab_sidebar_width == 0 {
                                220.
                            } else {
                                f32::from(self.workspace.tab_sidebar_width)
                            };
                            self.sync_tab_editors(window, cx);
                            self.sync_organiser_table(window, cx);
                            self.reload_selected_organiser_detail(window, cx);
                            self.apply_workspace_ack(&workspace, window, cx);
                        }
                        Some(Ok(true)) | Some(Ok(false)) => {
                            self.workspace_dirty = true;
                        }
                        None | Some(Err(_)) => {}
                    }
                }
            }
            Event::WorkspaceRejected {
                project,
                revision,
                stored,
                message,
            } if self.project == Some(project)
                && self.pending_workspace == Some(revision)
                && !self.intruder_close_owns_workspace() =>
            {
                self.fail_intruder_result_close(format!("工作区保存失败：{message}；未丢弃。"), cx);
                self.plugins_workspace_rejected(revision, window, cx);
                self.tab_groups_save_failed(&message, cx);
                let intruder_rebase_error = self.intruder_workspace_rejected(&stored, cx);
                self.ext_repeater_save_failed("当前工作区保存失败，未载入归档请求。", cx);
                self.cancel_history_transfer("工作区保存失败；未转送所选请求。", cx);
                self.history_workspace_rejected("当前工作区保存失败，未切换 Repeater 历史。", cx);
                self.pending_workspace = None;
                self.pending_proxy_interception = None;
                self.saving_project_snapshot = false;
                self.workspace.revision = stored.revision;
                self.workspace.match_and_replace_rules = stored.match_and_replace_rules.clone();
                self.workspace.ssl_pass_through_rules = stored.ssl_pass_through_rules.clone();
                self.workspace.ws_match_and_replace_rules =
                    stored.ws_match_and_replace_rules.clone();
                self.workspace_dirty = true;
                self.pending_sends.clear();
                self.plugins_save_failed(cx);
                if !self.choosing_save_as
                    && !self
                        .pending_project_save
                        .as_ref()
                        .is_some_and(Option::is_some)
                    && self.transition != Some(TransitionPhase::Quiescing)
                {
                    self.pending_project_save = None;
                    self.cancel_transition(cx);
                }
                for tab in self
                    .workspace
                    .tabs
                    .iter_mut()
                    .chain(&mut self.workspace.closed)
                {
                    if let Some(previous) = stored
                        .tabs
                        .iter()
                        .chain(&stored.closed)
                        .find(|old| old.id == tab.id)
                    {
                        let same_contents = tab.draft.raw == previous.draft.raw
                            && tab.draft.target == previous.draft.target
                            && tab.draft.version == previous.draft.version;
                        tab.draft.revision = tab.draft.revision.max(
                            previous
                                .draft
                                .revision
                                .saturating_add(u64::from(!same_contents)),
                        );
                    }
                }
                for tab in &mut self.workspace.websocket_tabs {
                    if let Some(previous) =
                        stored.websocket_tabs.iter().find(|old| old.id == tab.id)
                    {
                        let same_contents = tab.draft_payload == previous.draft_payload
                            && tab.url == previous.url
                            && tab.direction == previous.direction
                            && tab.draft_kind == previous.draft_kind;
                        tab.draft_revision = tab.draft_revision.max(
                            previous
                                .draft_revision
                                .saturating_add(u64::from(!same_contents)),
                        );
                    }
                }
                self.fail(
                    format!(
                        "保存工作区失败：{message}；最新草稿已保留。{}未自动发送请求。",
                        intruder_rebase_error
                            .map_or_else(|| "请重试。".to_owned(), |error| format!("{error} "))
                    ),
                    cx,
                );
                self.flush_project_save(window, cx);
                self.advance_settings_pool_delete(cx);
            }
            Event::RepeaterOpened { project, mut tab } if self.project == Some(project)
                && !self
                    .workspace
                    .tabs
                    .iter()
                    .chain(&self.workspace.closed)
                    .any(|existing| existing.id == tab.id)
                => {
                    if tab.title.parse::<usize>().is_ok() {
                        tab.title = self.next_repeater_tab_title(None, Some(&tab.draft.raw));
                    }
                    let composition_error = self.capture_drafts(window, cx).err();
                    if composition_error.is_none() {
                        self.workspace.active = Some(tab.id);
                        self.workspace.active_websocket_tab = None;
                    }
                    self.workspace.tabs.push(tab);
                    self.sync_tab_editors(window, cx);
                    self.save_workspace(cx);
                    self.tool = Tool::Repeater;
                    if let Some(error) = composition_error {
                        self.fail(error, cx);
                    }
                }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}
