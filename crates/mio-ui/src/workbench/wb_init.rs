use super::*;

impl Workbench {
    pub(crate) fn new(client: Client, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.weak_entity();
        let history = cx.new(|cx| {
            TableState::new(
                HistoryDelegate::default().with_owner(owner.clone()),
                window,
                cx,
            )
            .sortable(true)
            .col_selectable(false)
            .select_on_modifier_click(false)
        });
        let websocket = websocket::WebSocketHistory::new(window, cx);
        let listener_input =
            cx.new(|cx| InputState::new(window, cx).default_value("127.0.0.1:8080"));
        let history_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("按 URL 或方法筛选"));
        let history_subscription =
            cx.subscribe_in(&history, window, |this, _, event, window, cx| match event {
                TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) => {
                    this.history_table_selection(*row, false, window, cx)
                }
                TableEvent::RightClickedRow(Some(row)) => {
                    this.history_table_selection(*row, true, window, cx)
                }
                TableEvent::ColumnWidthsChanged(_) => {
                    this.sync_history_all_columns(cx);
                }
                _ => {}
            });
        let filter_subscription =
            cx.subscribe_in(&history_filter, window, |this, _, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => {
                        this.query_history(0, window, cx);
                    }
                    InputEvent::Change => {
                        cx.notify();
                    }
                    _ => {}
                }
            });
        let websocket_subscription =
            cx.subscribe_in(&websocket.table, window, |this, _, event, window, cx| {
                let row = match event {
                    TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) => Some(*row),
                    TableEvent::RightClickedRow(row) => *row,
                    _ => None,
                };
                if let Some(row) = row {
                    this.select_websocket_row(row, window, cx);
                }
            });
        let owner = window.window_handle();
        let mut events = client.events;
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.recv().await {
                let mut batch = vec![event];
                while batch.len() < 32 {
                    match events.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                if owner
                    .update(cx, |_, window, cx| {
                        this.update(cx, |view, cx| {
                            if view.shutdown_requested || view.runtime_result.is_some() {
                                return;
                            }
                            for event in batch {
                                view.on_event(event, window, cx);
                            }
                            // Automatic refresh preserves errors until an explicit action acknowledges them.
                            let error = view.error.take();
                            if view.history_dirty && !view.history_loading {
                                let offset = view.history.read(cx).delegate().offset;
                                view.query_history(offset, window, cx);
                            }
                            view.refresh_websocket_history(window, cx);
                            view.refresh_intruder_results(window, cx);
                            view.error = view.error.take().or(error);
                        })
                    })
                    .and_then(|result| result)
                    .is_err()
                {
                    return;
                }
            }
            let _ = owner.update(cx, |_, window, cx| {
                this.update(cx, |view, cx| {
                    if !view.quitting && !view.shutdown_requested && view.runtime_result.is_none() {
                        if view.quit_attempt.is_some() {
                            if view.transition == Some(TransitionPhase::Confirming) {
                                view.reject_quit(cx);
                            } else {
                                view.cancel_transition(cx);
                            }
                        }
                        view.backend_disconnected = true;
                        view.workspace_dirty |= view.pending_workspace.is_some();
                        view.pending_history_delete = None;
                        view.pending_sends.clear();
                        view.plugins_save_failed(cx);
                        view.ext_backend_disconnected(window, cx);
                        view.intruder_backend_disconnected(cx);
                        view.intruder_task_name_disconnected(cx);
                        view.user_preferences_disconnected(cx);
                        view.startup_wizard_disconnected(cx);
                        view.websocket.backend_disconnected(cx);
                        view.websocket_repeater_backend_disconnected(cx);
                        view.target_backend_disconnected(cx);
                        view.sequencer_backend_disconnected(cx);
                        view.audit_backend_disconnected(cx);
                        view.collaborator_backend_disconnected(cx);
                        view.global_search_disconnected(window, cx);
                        view.archive_response_disconnected(cx);
                        view.history_disconnected(cx);
                        view.http_notes_disconnected(cx);
                        view.http_highlights_disconnected(cx);
                        view.http_filter_disconnected(cx);
                        if let Some(cancel) = view.extra_tools.logger_query_cancel.take() {
                            cancel.cancel();
                        }
                        view.extra_tools.logger_pending_read = None;
                        view.cancel_history_transfer("后台断开；未转送所选请求。", cx);
                        view.imported_history_disconnected(cx);
                        view.tab_groups_save_failed(
                            "后台已断开，分组编辑保留，保存结果未确认。",
                            cx,
                        );
                        for send in view.sending.values() {
                            send.cancel.cancel();
                        }
                        if let Some(cancel) = &view.proxy_cancel {
                            cancel.cancel();
                        }
                        if view.proxy_address.is_some() || view.proxy_busy {
                            view.proxy_busy = true;
                            view.status = "后台服务已断开；Proxy 停止结果未确认。".into();
                        }
                        view.history_loading = false;
                        view.history_dirty = false;
                        view.history.update(cx, |table, cx| {
                            table.delegate_mut().loading = false;
                            table.refresh(cx);
                        });
                        if view.selected_flow.is_some() && view.detail.is_none() {
                            view.detail_error = Some("后台服务已断开，正文读取未确认。".into());
                        }
                        view.fail("后台服务已断开，尚未提交的编辑保留在当前窗口。", cx);
                    }
                })
            });
        });
        let settings_target = cx.weak_entity();
        App::on_action(cx, move |_: &crate::OpenSettings, cx| {
            let target = settings_target.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |_, window, cx| {
                    target.update(cx, |view, cx| view.open_settings(window, cx))
                });
            });
        });
        window.on_window_should_close(cx, |_, cx| {
            cx.global::<crate::native_terminate::NativeTerminate>()
                .request();
            false
        });
        window.set_window_title("MIO");
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let command_palette = cx.new(|cx| CommandState::new(window, cx));
        let global_search = wb_render_chrome::GlobalSearchUi::new(window, cx);
        let mut view = Self {
            owner_window: owner,
            startup_wizard: None,
            intruder_result_close: None,
            commands: client.commands,
            shutdown: client.shutdown,
            backend_disconnected: false,
            project: None,
            project_path: None,
            settings_window: None,
            tab_group_dialog: None,
            user_preferences: user_preferences::UserPreferencesUi::default(),
            ext_import: ext_import::ExtImport::default(),
            workspace: Workspace::default(),
            workspace_dirty: false,
            pending_workspace: None,
            pending_proxy_interception: None,
            pending_project_save: None,
            project_saves_in_flight: 0,
            notes_save_wait: None,
            project_notes_save: None,
            project_http_notes_save: None,
            http_notes: http_notes::HttpNotes::new(window, cx),
            http_highlights: http_highlights::HttpHighlights::default(),
            project_http_highlights_save: None,
            inspector: inspector::InspectorPane::new(window, cx),
            extra_tools: tools_extra::ExtraTools::new(window, cx),
            project_save_status: String::new(),
            choosing_save_as: false,
            file_picker: None,
            #[cfg(test)]
            save_picker_override: None,
            saving_project_snapshot: false,
            tool: Tool::Proxy,
            proxy_page: ProxyPage::Intercept,
            proxy_address: None,
            proxy_listeners_bound: Vec::new(),
            observed_server_certs: Vec::new(),
            session_traces: Vec::new(),
            session_tracer_paused: false,
            last_ca_fingerprint: None,
            mio_ai_context: None,
            mio_ai_panel_open: false,
            csrf_poc_modal: None,
            repeater_custom_actions_open: false,
            repeater_explanations_open: false,
            annotated_live_flows: HashSet::new(),
            live_http_annotations: Vec::new(),
            proxy_cancel: None,
            proxy_busy: false,
            intercept_requests: false,
            intercept_responses: false,
            listener_input,
            history_filter,
            http_filter: http_filter::HttpFilterState::default(),
            history,
            websocket,
            query_revision: 0,
            history_loading: false,
            history_dirty: false,
            pending_history_delete: None,
            has_traffic: false,
            selection_revision: 0,
            history_detail_revision: 0,
            repeater_reads: HashMap::new(),
            archive_responses: archive_response::ArchiveResponses::default(),
            repeater_history: repeater_history::HistoryNavigation::default(),
            selected_flow: None,
            selected_history: None,
            history_transfer: None,
            imported_history: imported_history::ImportedHistory::default(),
            detail: None,
            detail_error: None,
            detail_original: Box::default(),
            history_original_view: false,
            history_request_original_view: false,
            history_response_original_view: false,
            message_pair_layouts: HashMap::new(),
            message_pair_combined_panes: HashMap::new(),
            history_request: MessageEditor::new(window, cx),
            history_response: MessageEditor::new(window, cx),
            intercepts: Vec::new(),
            selected_intercept: None,
            tab_editors: HashMap::new(),
            tab_editor_lru: Vec::new(),
            websocket_repeater: websocket_repeater::WebSocketRepeaterState::default(),
            target_pane: target::TargetPane::new(window, cx),
            sequencer_pane: sequencer::SequencerPane::new(window, cx),
            audit_dashboard: audit_dashboard::AuditDashboardPane::new(window, cx),
            footer_drawer: FooterDrawerState::new(window, cx),
            collaborator_pane: collaborator::CollaboratorPane::new(window, cx),
            command_palette,
            command_palette_open: false,
            global_search,
            intruder: intruder::Intruder::default(),
            organiser: organiser::OrganiserUi::new(window, cx),
            extensions: plugins::Extensions::new(window, cx),
            sending: HashMap::new(),
            pending_sends: HashMap::new(),
            close_after_send: HashSet::new(),
            rename: None,
            tab_width: 180.,
            tabs_scroll: ScrollHandle::new(),
            resize_original: None,
            drop_target: None,
            drop_after: false,
            focus,
            status: "正在打开临时项目…".into(),
            error: None,
            storage_error: None,
            pending_records: 0,
            after_save: None,
            transition: None,
            quit_attempt: None,
            shutdown_requested: false,
            runtime_result: None,
            quitting: false,
            update_status: mio_core::UpdateStatus::Idle,
            #[cfg(not(test))]
            startup_update_checked: false,
            _subscriptions: vec![
                history_subscription,
                filter_subscription,
                websocket_subscription,
            ],
            _events: event_task,
        };
        view.query_user_preferences(cx);
        view
    }

    pub(crate) fn new_startup(
        client: Client,
        initial_project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(client, window, cx);
        view.init_startup_wizard(initial_project, window, cx);
        view
    }

    pub(in crate::workbench) fn open_dashboard(&mut self, cx: &mut Context<Self>) {
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Dashboard;
        self.activate_intruder_dashboard(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn fail(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        self.record_footer_event(
            FooterLogSeverity::Error,
            message.clone(),
            format!("工作区错误：{message}"),
        );
        self.error = Some(message);
        cx.notify();
    }

    pub(in crate::workbench) fn set_proxy_interception(
        &mut self,
        project: Option<ProjectKey>,
        settings: mio_core::ProxyInterceptionSettings,
        cx: &mut Context<Self>,
    ) -> bool {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return false;
        }
        if self.workspace.proxy_interception == settings {
            return true;
        }
        self.workspace.proxy_interception = settings;
        let saved = self.save_workspace(cx);
        cx.notify();
        saved
    }

    pub(in crate::workbench) fn backend_available(&self) -> bool {
        !self.backend_disconnected && !self.shutdown.is_cancelled()
    }

    pub(in crate::workbench) fn enqueue(
        &mut self,
        command: Command,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.intruder.resource_pools.management.delete_pending()
            && matches!(
                command,
                Command::SaveWorkspace { .. }
                    | Command::SaveProject { .. }
                    | Command::StartProject { .. }
                    | Command::NewProject { .. }
                    | Command::OpenProject { .. }
                    | Command::NewRepeater { .. }
                    | Command::OpenRepeater { .. }
                    | Command::OpenExtRepeater { .. }
                    | Command::StartIntruder { .. }
                    | Command::EnsureIntruderDefaultPool { .. }
                    | Command::SetIntruderResourcePool { .. }
                    | Command::RequestIntruderBaseline { .. }
                    | Command::SetIntruderRetention { .. }
                    | Command::DiscardIntruder { .. }
            )
        {
            self.fail("资源池删除尚未确认；当前编辑保留，请等待后台回执。", cx);
            return false;
        }
        if !self.backend_available() {
            self.fail("后台正在退出或已停止；当前草稿仍保留。", cx);
            return false;
        }
        let interception_snapshot = match &command {
            Command::SaveWorkspace { workspace, .. }
            | Command::SaveProject {
                workspace: Some(workspace),
                ..
            } => Some((
                workspace.revision,
                workspace.proxy_interception.clone(),
                workspace.proxy_messages,
                workspace.proxy_websocket_interception,
            )),
            _ => None,
        };
        let cancel_send_log = match &command {
            Command::CancelSend {
                tab_id,
                run_id,
                draft_revision,
                ..
            } => Some((
                format!("Repeater 请求取消已提交（标签 {tab_id}）"),
                format!(
                    "tab_id={tab_id}, run_id={run_id}, draft_revision={draft_revision}"
                ),
            )),
            _ => None,
        };
        match self.commands.try_send(command) {
            Ok(()) => {
                if let Some((msg, detail)) = cancel_send_log {
                    self.record_footer_event(FooterLogSeverity::Info, msg, detail);
                }
                if let Some(snapshot) = interception_snapshot {
                    self.pending_proxy_interception = Some(snapshot);
                    self.error = None;
                } else if self.error.as_deref() != Some(settings::SNAPSHOT_ERROR) {
                    self.error = None;
                }
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.fail("后台命令队列已满，请稍后重试；当前草稿未丢弃。", cx);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.fail("后台服务已停止；当前草稿未丢弃。", cx);
                false
            }
        }
    }
}
