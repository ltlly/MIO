use super::*;

impl State {
    pub(super) async fn run(mut self, mut commands: mpsc::Receiver<Command>) -> Result<()> {
        self.project_opened().await?;
        let mut scheduler_tick = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(15),
            Duration::from_secs(15),
        );
        scheduler_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cross_window_sync_tick = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(2),
            Duration::from_secs(2),
        );
        cross_window_sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // No new command or producer event is admitted while Stop notifications await the UI.
            self.flush_proxy_notifications().await;
            self.flush_repeater_notifications().await;
            let intruder_commands_ready = self.intruder.can_accept_commands();
            let commands_ready = self.ext_archive.can_accept_commands() && intruder_commands_ready;
            let can_emit_repeater_progress = self.repeater.notifications.is_empty();
            let incoming = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => Incoming::Stop,
                _ = self.events.closed() => Incoming::Stop,
                result = proxy_completion(&mut self.proxy.primary) => Incoming::ProxyFinished(result),
                result = extra_proxy_completion(&mut self.proxy.extra) => Incoming::ProxyFinished(result),
                Some(result) = self.repeater.jobs.join_next(), if !self.repeater.jobs.is_empty() => Incoming::Completed(result),
                Some(done) = self.ai_ipc.jobs.join_next(), if !self.ai_ipc.jobs.is_empty() => Incoming::IpcRepeaterCompleted(Box::new(done)),
                tab_id = next_repeater_progress(&mut self.repeater.sends, &self.events, can_emit_repeater_progress) => Incoming::RepeaterProgress(tab_id),
                signal = self.intruder.next_signal(&self.events, false) => Incoming::Intruder(signal),
                signal = self.ext_archive.next_signal(&self.events, false) => Incoming::Ext(signal),
                signal = self.queries.websocket.next_signal(&self.events, false) => Incoming::WebSocketQuery(signal),
                signal = self.queries.http.next_signal(&self.events, false) => Incoming::HttpQuery(signal),
                command = commands.recv(), if commands_ready => command.map(|command| Incoming::Command(Box::new(command))).unwrap_or(Incoming::Stop),
                Some(call) = ai_ipc::next_ipc_call(&mut self.ai_ipc.rx), if commands_ready => Incoming::AiIpc(call),
                event = self.proxy.events.recv(), if intruder_commands_ready => event.map(Incoming::Proxy).unwrap_or(Incoming::Stop),
                Some(trace) = self.sessions.trace_rx.recv(), if intruder_commands_ready => Incoming::SessionTrace(trace),
                Some(cert) = self.tls.server_cert_rx.recv(), if intruder_commands_ready => Incoming::ServerCertObserved(cert),
                Some(fail) = self.tls.tls_failure_rx.recv(), if intruder_commands_ready => Incoming::ClientTlsFailure(fail),
                signal = self.plugins.next_signal(), if intruder_commands_ready => Incoming::Plugin(signal),
                Some(event) = self.ws_repeater.next_signal(), if intruder_commands_ready => Incoming::WebSocketRepeater(event),
                Some(event) = self.sequencer.next_signal(), if intruder_commands_ready => Incoming::Sequencer(event),
                event = self.collaborator.next_signal(), if intruder_commands_ready => Incoming::Collaborator(event),
                Some(signal) = self.audit.next_signal(), if intruder_commands_ready => Incoming::Audit(signal),
                _ = scheduler_tick.tick(), if commands_ready => Incoming::SchedulerTick,
                _ = cross_window_sync_tick.tick(), if !self.preferences.is_isolated() && commands_ready => Incoming::CrossWindowSyncTick,
            };
            match incoming {
                Incoming::Stop => break,
                Incoming::Command(command) => {
                    let (operation, project) = command_context(&command, self.project);
                    if let Err(error) = Box::pin(self.command(*command)).await {
                        self.emit(Event::Error {
                            project,
                            operation,
                            message: error.to_string(),
                        })
                        .await;
                    }
                }
                Incoming::Proxy(event) => {
                    if let Err(error) = self.capture(event).await {
                        self.error("capture", error).await;
                    }
                }
                Incoming::ProxyFinished(result) => self.proxy_finished(result),
                Incoming::Completed(result) => self.repeater_completed(result).await,
                Incoming::RepeaterProgress(tab_id) => self.emit_repeater_progress(tab_id),
                Incoming::Plugin(signal) => Box::pin(self.plugin_signal(signal)).await,
                Incoming::WebSocketRepeater(event) => {
                    self.emit(Event::WebSocketRepeater(event)).await;
                }
                Incoming::Sequencer(event) => {
                    self.emit(Event::Sequencer(event)).await;
                }
                Incoming::Audit(signal) => {
                    self.on_audit_signal(signal).await;
                }
                Incoming::Collaborator(event) => {
                    self.on_collaborator_event_for_audit(&event).await;
                    self.emit(Event::Collaborator(event)).await;
                }
                Incoming::Ext(signal) => self.ext_signal(signal).await,
                Incoming::WebSocketQuery(signal) => self.websocket_query_signal(signal),
                Incoming::HttpQuery(signal) => self.http_query_signal(signal),
                Incoming::AiIpc(call) => {
                    Box::pin(self.accept_ai_ipc_call(call)).await;
                }
                Incoming::IpcRepeaterCompleted(done) => {
                    self.complete_ipc_repeater(*done);
                }
                Incoming::Intruder(signal) => {
                    if let Err(error) = Box::pin(self.intruder_signal(signal)).await {
                        self.queue_intruder_event(Event::Error {
                            project: Some(self.project),
                            operation: "intruder",
                            message: error.to_string(),
                        });
                    }
                }
                Incoming::SessionTrace((project, entry)) => {
                    if project == self.project && self.events.capacity() > 4 {
                        let _ = self
                            .events
                            .try_send(Event::SessionTraceRecorded { project, entry });
                    }
                }
                Incoming::ServerCertObserved((project, cert)) => {
                    if project == self.project {
                        let key = (
                            cert.host.clone(),
                            cert.port,
                            cert.sha256_fingerprint.clone(),
                        );
                        if self.tls.observed_cert_keys.len() < 512
                            && self.tls.observed_cert_keys.insert(key)
                            && self.events.capacity() > 4
                        {
                            let _ = self
                                .events
                                .try_send(Event::ServerCertObserved { project, cert });
                        }
                    }
                }
                Incoming::ClientTlsFailure((project, host, port)) => {
                    if project == self.project {
                        Box::pin(self.handle_client_tls_failure(project, host, port)).await;
                    }
                }
                Incoming::SchedulerTick => {
                    Box::pin(self.run_scheduled_tasks_tick()).await;
                }
                Incoming::CrossWindowSyncTick => {
                    self.sync_shared_state_from_disk().await;
                }
            }
            if self.unsaved_count() != 0 && self.intruder.can_accept_commands() {
                if (self.proxy.is_running()
                    || !self.repeater.jobs.is_empty()
                    || !self.ai_ipc.jobs.is_empty()
                    || self.plugins.has_resources()
                    || self.intruder.has_resources()
                    || self.ws_repeater.has_resources()
                    || self.sequencer.has_resources()
                    || self.audit.has_resources()
                    || self.collaborator.has_resources())
                    && let Err(error) = Box::pin(self.stop_resources()).await
                {
                    self.storage.error.get_or_insert_with(|| error.to_string());
                }
                self.storage_state().await;
            }
        }
        self.cancel.cancel();
        if let Some(mut ipc_server) = self.ai_ipc.server.take() {
            ipc_server.shutdown().await;
        }
        self.ai_ipc.rx = None;
        self.stop_ipc_jobs().await;
        // Shutdown must still persist terminal states after query cancellation.
        self.store.cancel_queries_with(CancellationToken::new());
        let result = Box::pin(self.stop_resources()).await;
        let proxy_shutdown = self.finish_proxy_shutdown().await;
        let repeater_shutdown = self.finish_repeater_shutdown().await;
        let ws_repeater_shutdown = self.ws_repeater.shutdown().await;
        let sequencer_shutdown = self.sequencer.shutdown().await;
        let audit_shutdown = self.audit.shutdown().await;
        let collaborator_shutdown = self.collaborator.shutdown().await;
        let intruder_shutdown = self.finish_intruder_shutdown().await;
        let ext_shutdown = self.finish_ext_shutdown().await;
        let websocket_shutdown = self.finish_websocket_query_shutdown().await;
        let http_shutdown = self.finish_http_query_shutdown().await;
        let checkpoint = self.store.checkpoint();
        if checkpoint.is_ok() {
            let _ = self.events.try_send(Event::Stopped);
        }
        let unsaved_count = self.unsaved_count();
        let store_close = self.store.close();
        self.project_lock = None;
        checkpoint?;
        ensure!(
            unsaved_count == 0,
            "仍有 {} 条流量或消息变更未能写盘；未自动重发请求",
            unsaved_count
        );
        result
            .and(proxy_shutdown)
            .and(repeater_shutdown)
            .and(ws_repeater_shutdown)
            .and(sequencer_shutdown)
            .and(audit_shutdown)
            .and(collaborator_shutdown)
            .and(ext_shutdown)
            .and(intruder_shutdown)
            .and(websocket_shutdown)
            .and(http_shutdown)
            .and(store_close)
    }

    pub(super) fn check_project(&self, project: ProjectKey) -> Result<()> {
        ensure!(project == self.project, "项目已切换，已拒绝旧操作");
        Ok(())
    }

    pub(super) async fn project_opened(&mut self) -> Result<()> {
        self.emit(Event::ProjectOpened {
            project: self.project,
            path: self.store.path.clone(),
            workspace: self.store.metadata().workspace()?,
        })
        .await;
        self.proxy_state().await;
        self.refresh_project_disk_bytes();
        if let Ok(audit_state) = self.store.audit().read_state()
            && audit_state != AuditStateSnapshot::default()
        {
            self.emit_audit_if_capacity(Some(AuditEvent::State {
                project: self.project,
                query_revision: 0,
                result: Ok(Box::new(audit_state)),
            }));
        }
        Ok(())
    }

    pub(super) async fn proxy_state(&mut self) {
        self.emit(Event::ProxyState {
            project: self.project,
            address: self.proxy.primary_address(),
            cancel: self.proxy.primary_cancel(),
            intercept_requests: self.proxy.intercept_requests,
            intercept_responses: self.proxy.intercept_responses,
        })
        .await;
        if !self.proxy.extra.is_empty()
            || self
                .proxy
                .primary_id
                .is_some_and(|id| id != Uuid::from_u128(1))
        {
            let listeners = self.proxy.active_listeners();
            self.emit(Event::ProxyListeners {
                project: self.project,
                listeners,
            })
            .await;
        }
    }

    pub(super) fn identity_dir(&self) -> Result<std::path::PathBuf> {
        authority::resolved_identity_dir(self.preferences_path.as_deref())
    }

    pub(super) async fn authority(&mut self) -> Result<MitmAuthority> {
        if self.preferences.is_isolated() {
            if let Some(ca) = &self.proxy.ca {
                return Ok(ca.clone());
            }
            let ca = tokio::task::spawn_blocking(MitmAuthority::generate).await??;
            self.proxy.ca = Some(ca.clone());
            return Ok(ca);
        }
        let identity_dir = self.identity_dir()?;
        let current_stamp = self.proxy.ca_stamp.clone();
        if let Some((ca, stamp)) = tokio::task::spawn_blocking(move || {
            authority::load_if_changed_at(&identity_dir, current_stamp.as_ref())
        })
        .await??
        {
            let prev_cert_changed = self
                .proxy
                .ca_stamp
                .as_ref()
                .is_some_and(|prev| prev.cert_sha256 != stamp.cert_sha256);
            self.proxy.ca = Some(ca.clone());
            self.proxy.ca_stamp = Some(stamp);
            if prev_cert_changed {
                let fingerprint = authority::fingerprint_sha256(&ca.certificate_der());
                for proxy in self.proxy.handles() {
                    proxy.set_authority(ca.clone());
                }
                self.emit(Event::CaRegenerated { fingerprint }).await;
            }
        }
        self.proxy
            .ca
            .clone()
            .context("MITM CA 证书尚未加载")
    }

    pub(super) async fn sync_shared_state_from_disk(&mut self) {
        if self.preferences.is_isolated() {
            return;
        }
        if self.proxy.ca_stamp.is_some()
            && let Ok(identity_dir) = self.identity_dir()
            && authority::file_changed_on_disk_at(&identity_dir, self.proxy.ca_stamp.as_ref())
        {
            let _ = self.authority().await;
        }
        if self.preferences.file_changed_on_disk() {
            let prev_cas = self.preferences.current().preferences.custom_ca_ders.clone();
            let state = self.preferences.query();
            if state.load_error.is_none() {
                if state.preferences.custom_ca_ders != prev_cas
                    && let Ok(workspace) = self.store.metadata().workspace()
                {
                    if let Ok(config) = Self::ws_repeater_send_config(
                        &self.preferences,
                        &workspace,
                        self.tls.compiled_client_certs.clone(),
                    ) {
                        self.ws_repeater.set_connect_config(config);
                    }
                    self.sync_sessions_workspace(&workspace);
                    if self.proxy.is_running()
                        && let Err(error) = self
                            .reload_running_proxy_from_workspace(Some(&workspace), &workspace)
                            .await
                    {
                        self.emit(Event::Error {
                            project: Some(self.project),
                            operation: "sync_shared_state",
                            message: format!("同步其他窗口 TLS 根证书后重载代理失败：{error:#}"),
                        })
                        .await;
                    }
                }
                self.emit(Event::UserPreferences {
                    operation_id: Uuid::nil(),
                    state,
                })
                .await;
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn replace_project(&mut self, next: ProjectStore) -> Result<()> {
        let (next_lock, reused_current) = match next.path.as_deref() {
            Some(path) => self.acquire_project_lock_for_switch(path)?,
            None => (None, false),
        };
        self.replace_project_with(next, next_lock, reused_current, false, None)
            .await
    }

    pub(super) fn acquire_project_lock_for_switch(
        &self,
        path: &std::path::Path,
    ) -> Result<(Option<project_lock::ActiveProjectLock>, bool)> {
        if self
            .project_lock
            .as_ref()
            .is_some_and(|lock| lock.matches_path(path))
        {
            return Ok((None, true));
        }
        Ok((Some(project_lock::acquire_active_project_lock(path)?), false))
    }

    pub(super) async fn replace_project_with(
        &mut self,
        mut next: ProjectStore,
        next_lock: Option<project_lock::ActiveProjectLock>,
        reused_current_lock: bool,
        pause_automated_tasks: bool,
        extensions_disabled: Option<bool>,
    ) -> Result<()> {
        self.require_saved()?;
        self.store.intruder().require_sessions_resolved()?;
        let intruder =
            intruder_service::IntruderState::with_resource_pools(next.resource_pools().list()?);
        self.stop_resources().await?;
        self.require_saved()?;
        self.store.checkpoint()?;
        let effective_pause = pause_automated_tasks
            || !next
                .metadata()
                .workspace_arc()
                .map(|w| w.tasks_auto_start)
                .unwrap_or(true);
        next.flows_mut().finish_pending(true, false)?;
        next.intruder_mut().recover_runs_on_open(effective_pause)?;
        next.cancel_queries_with(self.cancel.clone());
        next.audit_mut()
            .recover_tasks_on_open(effective_pause, &self.cancel)?;
        let workspace = next.metadata().workspace_arc()?;
        let previous_project = self.project;
        self.collaborator.stop_project(previous_project).await?;
        self.project = ProjectKey {
            id: next.id,
            generation: self.project.generation + 1,
        };
        self.store = next;
        if !reused_current_lock {
            self.project_lock = next_lock;
        }
        if let Some(disabled) = extensions_disabled {
            self.extensions_disabled = disabled;
        }
        let restored_annotated = self.store.flows().annotated_live_flow_ids().unwrap_or_default();
        self.storage.reset_for_project(restored_annotated);
        self.intruder = intruder;
        self.quiesced = false;
        self.automated_tasks_paused = effective_pause;
        self.ai_ipc.repeater_revisions.clear();
        self.tls.observed_cert_keys.clear();
        self.last_scheduled_fire.clear();
        self.ws_repeater.drop_project(previous_project);
        self.sequencer.drop_project(previous_project);
        self.audit.drop_project(previous_project);
        self.ws_repeater.set_project(self.project);
        self.ws_repeater
            .set_injectors(self.proxy.websocket_injectors());
        self.sync_compiled_client_certs(&workspace).await;
        self.sessions.runtime.reset_project(self.project, &workspace);
        self.sync_sessions_workspace(&workspace);
        self.ws_repeater
            .restore_workspace_tabs(self.project, workspace.websocket_tabs.clone());
        if let Ok(config) = Self::ws_repeater_send_config(
            &self.preferences,
            &workspace,
            self.tls.compiled_client_certs.clone(),
        ) {
            self.ws_repeater.set_connect_config(config);
        }
        self.project_opened().await?;
        if let Some(saved_path) = self.store.path.clone() {
            self.record_recent_project(&saved_path).await;
        }
        self.restore_installed_plugins().await;
        Ok(())
    }

    pub(super) async fn record_recent_project(&mut self, path: &std::path::Path) {
        let unix_ms = crate::codec_util::now_unix_ms();
        let name = self
            .store
            .metadata()
            .workspace_arc()
            .ok()
            .and_then(|workspace| workspace.project_name.clone())
            .unwrap_or_default();
        if self
            .preferences
            .record_recent(name, path.to_path_buf(), unix_ms)
            .is_ok()
        {
            let state = self.preferences.query();
            self.emit(Event::UserPreferences {
                operation_id: Uuid::nil(),
                state,
            })
            .await;
        }
    }

    pub(super) async fn stop_resources(&mut self) -> Result<()> {
        self.queries.cancel_all();
        self.intruder.cancel_all();
        self.repeater.cancel_all();
        self.stop_ipc_jobs().await;
        self.proxy.request_stop_all();
        self.ws_repeater.cancel_all();
        self.sequencer.cancel_all();
        self.audit.cancel_all();
        self.collaborator.cancel_all();
        let ext_result = self.stop_ext().await;
        let plugin_result = self.stop_plugins().await;
        let proxy_result = self.stop_proxy().await;
        let intruder_result = self.stop_intruder().await;
        let repeater_result = self
            .stop_repeater_until(tokio::time::Instant::now() + Duration::from_secs(3))
            .await;
        let ws_repeater_result = self.ws_repeater.quiesce_project(self.project).await;
        let sequencer_result = self.sequencer.quiesce_project(self.project).await;
        let audit_result = self.audit.quiesce_project(self.project).await;
        let collaborator_result = self.collaborator.stop_project(self.project).await;
        let query_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let websocket_result = self.stop_websocket_query_until(query_deadline).await;
        let http_result = self.stop_http_query_until(query_deadline).await;
        proxy_result?;
        plugin_result?;
        ext_result?;
        intruder_result?;
        repeater_result?;
        ws_repeater_result?;
        sequencer_result?;
        audit_result?;
        collaborator_result?;
        websocket_result?;
        http_result?;
        if self.unsaved_count() == 0 {
            self.store.flows_mut().finish_pending(false, false)?;
        }
        Ok(())
    }

    pub(super) async fn handle_client_tls_failure(
        &mut self,
        project: ProjectKey,
        host: String,
        port: u16,
    ) {
        if self.quiesced || self.unsaved_count() != 0 || host.trim().is_empty() {
            return;
        }
        let Ok(mut workspace) = self.store.metadata().workspace() else {
            return;
        };
        if !workspace.proxy_messages.ssl_pass_through_auto_add_on_failure {
            return;
        }
        let normalized_host = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
        if workspace.ssl_pass_through_rules.iter().any(|r| {
            r.enabled
                && !r.is_regex
                && r.host_pattern.eq_ignore_ascii_case(&normalized_host)
                && (r.port.is_none() || r.port == Some(port))
        }) {
            return;
        }
        let rule = SslPassThroughRule {
            id: Uuid::new_v4(),
            enabled: true,
            host_pattern: normalized_host.clone(),
            port: Some(port),
            is_regex: false,
        };
        workspace.ssl_pass_through_rules.push(rule);
        workspace.revision = workspace.revision.saturating_add(1);
        if self.store.metadata_mut().save_workspace(&workspace).is_ok() {
            if let Ok(matcher) =
                crate::http::build_ssl_pass_through_matcher(&workspace.ssl_pass_through_rules)
            {
                for proxy in self.proxy.handles() {
                    proxy.set_ssl_pass_through(matcher.clone());
                }
            }
            self.emit(Event::WorkspaceChanged {
                project,
                workspace,
            })
            .await;
            self.emit(Event::TlsPassThroughAutoAdded {
                project,
                host: normalized_host,
                port: Some(port),
            })
            .await;
        }
    }

    pub(super) async fn run_scheduled_tasks_tick(&mut self) {
        if self.quiesced || self.unsaved_count() != 0 {
            return;
        }
        let Ok(workspace) = self.store.metadata().workspace_arc() else {
            return;
        };
        if workspace.scheduled_tasks.is_empty() {
            return;
        }
        let now_secs = crate::codec_util::now_unix_ms() / 1000;
        let now_min = now_secs / 60;
        let due_tasks: Vec<ScheduledTask> = workspace
            .scheduled_tasks
            .iter()
            .filter(|t| {
                t.enabled
                    && scheduled_task_due(
                        &t.trigger_spec,
                        now_secs,
                        self.last_scheduled_fire.get(&t.id).copied(),
                    )
            })
            .cloned()
            .collect();
        for task in due_tasks {
            match task.action {
                ScheduledTaskAction::PauseTasks => {
                    self.last_scheduled_fire.insert(task.id, now_min);
                    self.automated_tasks_paused = true;
                    if let Ok(audit_state) = self.store.audit().read_state() {
                        for t in audit_state
                            .tasks
                            .iter()
                            .filter(|t| t.state == AuditTaskState::Running)
                        {
                            let _ = self
                                .audit_command(AuditCommand::PauseScan {
                                    project: self.project,
                                    task_id: t.id,
                                })
                                .await;
                        }
                    }
                    let running_intruder: Vec<Uuid> = self.intruder.active_run_ids(self.project);
                    for run_id in running_intruder {
                        let _ = self.set_intruder_paused(self.project, run_id, true);
                    }
                }
                ScheduledTaskAction::ResumeTasks => {
                    self.last_scheduled_fire.insert(task.id, now_min);
                    self.automated_tasks_paused = false;
                    if let Ok(audit_state) = self.store.audit().read_state() {
                        for t in audit_state
                            .tasks
                            .iter()
                            .filter(|t| t.state == AuditTaskState::Paused)
                        {
                            let _ = self
                                .audit_command(AuditCommand::ResumeScan {
                                    project: self.project,
                                    task_id: t.id,
                                })
                                .await;
                        }
                    }
                    let active_intruder: Vec<Uuid> = self.intruder.active_run_ids(self.project);
                    for run_id in active_intruder {
                        let _ = self.set_intruder_paused(self.project, run_id, false);
                    }
                }
                ScheduledTaskAction::RunScan => {
                    if self.automated_tasks_paused || !workspace.tasks_auto_start {
                        continue;
                    }
                    self.last_scheduled_fire.insert(task.id, now_min);
                    let target_url = scheduled_run_scan_target(&workspace, &task.description);
                    if let Some(url) = target_url
                        && let Ok(parsed) = url::Url::parse(&url)
                        && let Some(host) = parsed.host_str()
                    {
                        let origin = format!(
                            "{}://{}:{}",
                            parsed.scheme(),
                            host,
                            parsed.port_or_known_default().unwrap_or(80)
                        );
                        let path = match parsed.query() {
                            Some(q) => format!("{}?{q}", parsed.path()),
                            None => parsed.path().to_owned(),
                        };
                        let raw = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")
                            .into_bytes();
                        let draft = Draft {
                            revision: 1,
                            target: origin,
                            version: HttpVersion::Http11,
                            raw,
                        };
                        let _ = self
                            .audit_command(AuditCommand::StartScan {
                                project: self.project,
                                task_id: Uuid::new_v4(),
                                title: format!("定时扫描: {url}"),
                                draft: Box::new(draft),
                                mode: AuditTaskMode::PassiveAndActive,
                                crawl_same_origin: false,
                                cancel: CancellationToken::new(),
                            })
                            .await;
                    }
                }
            }
        }
    }
}

fn has_regex_meta(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '^' | '$' | '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '\\'))
}

fn concrete_url_from_advanced_scope_rule(rule: &crate::AdvancedScopeRule) -> Option<String> {
    if !rule.enabled {
        return None;
    }
    let host = rule.host.trim();
    if host.is_empty() || has_regex_meta(host) {
        return None;
    }
    let scheme = match rule.protocol {
        crate::ScopeProtocol::Http => "http",
        crate::ScopeProtocol::Https | crate::ScopeProtocol::Any => "https",
    };
    let host_part = if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let port_part = match rule.port.trim() {
        "" => String::new(),
        raw => {
            let p = raw.parse::<u16>().ok().filter(|&p| p > 0)?;
            format!(":{p}")
        }
    };
    let file_raw = rule.file.trim();
    let path = if file_raw.starts_with('/') && !has_regex_meta(file_raw) {
        file_raw
    } else {
        "/"
    };
    Some(format!("{scheme}://{host_part}{port_part}{path}"))
}

fn scheduled_run_scan_target(workspace: &Workspace, description: &str) -> Option<String> {
    let trimmed = description.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_owned());
    }
    if workspace.scope.advanced_mode
        && let Some(url) = workspace
            .scope
            .advanced_include
            .iter()
            .find_map(concrete_url_from_advanced_scope_rule)
    {
        return Some(url);
    }
    workspace
        .scope
        .include
        .iter()
        .find(|r| r.enabled && !r.prefix.trim().is_empty())
        .map(|r| r.prefix.trim().to_owned())
}

fn current_local_hh_mm(now_secs: i64) -> (u32, u32) {
    crate::codec_util::local_hh_mm(now_secs)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScheduledTrigger {
    Interval {
        start: Option<(u32, u32)>,
        interval_mins: i64,
    },
    DailyAt {
        hour: u32,
        minute: u32,
    },
}

fn parse_hh_mm(raw: &str) -> Option<(u32, u32)> {
    let (hh, mm) = raw.trim().split_once(':')?;
    let h = hh.trim().parse::<u32>().ok()?;
    let m = mm.trim().parse::<u32>().ok()?;
    (h <= 23 && m <= 59).then_some((h, m))
}

fn parse_repeat_interval_minutes(raw: &str) -> Option<i64> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let stripped = t
        .strip_prefix("every:")
        .or_else(|| t.strip_prefix("EVERY:"))
        .unwrap_or(t)
        .trim();
    if stripped == "每分钟" {
        return Some(1);
    }
    if stripped == "每小时" || stripped.eq_ignore_ascii_case("hourly") {
        return Some(60);
    }
    let normalized = stripped.strip_prefix('每').unwrap_or(stripped).trim();
    if let Some(num) = normalized
        .strip_suffix("分钟")
        .or_else(|| normalized.strip_suffix('分'))
        .or_else(|| normalized.strip_suffix('m'))
        .or_else(|| normalized.strip_suffix('M'))
    {
        return num.trim().parse::<i64>().ok().filter(|&m| m >= 1);
    }
    if let Some(num) = normalized
        .strip_suffix("小时")
        .or_else(|| normalized.strip_suffix('h'))
        .or_else(|| normalized.strip_suffix('H'))
    {
        let hours = num.trim().parse::<i64>().ok().filter(|&h| h >= 1)?;
        return hours.checked_mul(60);
    }
    None
}

fn is_daily_or_once_keyword(raw: &str) -> bool {
    let t = raw.trim();
    t.is_empty()
        || matches!(t, "每日" | "每天" | "单次")
        || t.eq_ignore_ascii_case("daily")
        || t.eq_ignore_ascii_case("once")
}

fn parse_scheduled_trigger(trigger_spec: &str) -> Option<ScheduledTrigger> {
    let spec = trigger_spec.trim();
    if spec.is_empty() || spec.contains('\0') {
        return None;
    }
    match spec.split_once('|') {
        None => {
            if let Some(interval_mins) = parse_repeat_interval_minutes(spec) {
                Some(ScheduledTrigger::Interval {
                    start: None,
                    interval_mins,
                })
            } else {
                parse_hh_mm(spec).map(|(hour, minute)| ScheduledTrigger::DailyAt { hour, minute })
            }
        }
        Some((left, right)) => {
            if right.contains('|') {
                return None;
            }
            let left = left.trim();
            let right = right.trim();
            if left.is_empty() {
                parse_repeat_interval_minutes(right).map(|interval_mins| {
                    ScheduledTrigger::Interval {
                        start: None,
                        interval_mins,
                    }
                })
            } else if let Some((hour, minute)) = parse_hh_mm(left) {
                if is_daily_or_once_keyword(right) {
                    Some(ScheduledTrigger::DailyAt { hour, minute })
                } else {
                    parse_repeat_interval_minutes(right).map(|interval_mins| {
                        ScheduledTrigger::Interval {
                            start: Some((hour, minute)),
                            interval_mins,
                        }
                    })
                }
            } else {
                None
            }
        }
    }
}

pub fn is_valid_scheduled_task_trigger(trigger_spec: &str) -> bool {
    parse_scheduled_trigger(trigger_spec).is_some()
}

fn scheduled_task_due(trigger_spec: &str, now_secs: i64, last_fired_min: Option<i64>) -> bool {
    let Some(parsed) = parse_scheduled_trigger(trigger_spec) else {
        return false;
    };
    let now_min = now_secs / 60;
    if last_fired_min == Some(now_min) {
        return false;
    }
    match parsed {
        ScheduledTrigger::Interval {
            start,
            interval_mins,
        } => {
            let after_start = start.is_none_or(|(sh, sm)| {
                last_fired_min.is_some() || current_local_hh_mm(now_secs) >= (sh, sm)
            });
            after_start
                && last_fired_min.is_none_or(|prev| now_min - prev >= interval_mins)
                && (now_min % interval_mins == 0 || last_fired_min.is_some())
        }
        ScheduledTrigger::DailyAt { hour, minute } => {
            let (cur_h, cur_m) = current_local_hh_mm(now_secs);
            cur_h == hour && cur_m == minute
        }
    }
}

#[cfg(test)]
mod scheduled_task_tests {
    use super::*;

    #[test]
    fn scheduled_task_trigger_parser_handles_daily_repeat_and_rejects_invalid() {
        for valid in [
            "09:00",
            "09:00 | 每日",
            "09:00 | daily",
            "09:00 | once",
            "every:15m",
            "09:00 | every:30m",
            " | every:10m",
            "每30分钟",
            "00:05 | 每15分钟",
            "2h",
        ] {
            assert!(
                is_valid_scheduled_task_trigger(valid),
                "expected valid trigger spec: {valid}"
            );
        }
        for invalid in ["", "   ", "99:99", "09:60", "09:00 | bogus", "a | b | c", "09:00\0"] {
            assert!(
                !is_valid_scheduled_task_trigger(invalid),
                "expected invalid trigger spec: {invalid}"
            );
        }
    }

    #[test]
    fn scheduled_task_due_fires_for_daily_with_repeat_label_and_intervals() {
        let local_epoch_secs = 9 * 3600;
        let (h, m) = current_local_hh_mm(local_epoch_secs);
        let daily_spec = format!("{h:02}:{m:02} | 每日");
        assert!(scheduled_task_due(&daily_spec, local_epoch_secs, None));
        assert!(!scheduled_task_due(
            &daily_spec,
            local_epoch_secs,
            Some(local_epoch_secs / 60)
        ));

        let interval_spec = "every:15m";
        assert!(scheduled_task_due(interval_spec, 15 * 60, None));
        assert!(!scheduled_task_due(interval_spec, 16 * 60, Some(15)));
        assert!(scheduled_task_due(interval_spec, 30 * 60, Some(15)));

        let start_and_repeat = format!("{h:02}:{m:02} | 每30分钟");
        assert!(scheduled_task_due(&start_and_repeat, local_epoch_secs, None));
        assert!(!scheduled_task_due(
            &format!("{:02}:59 | every:15m", (h + 1) % 24),
            local_epoch_secs,
            None
        ));
    }

    #[test]
    fn run_scan_derives_target_from_concrete_advanced_scope_when_enabled() {
        let mut ws = Workspace::default();
        ws.scope.advanced_mode = true;
        ws.scope.advanced_include.push(crate::AdvancedScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            protocol: crate::ScopeProtocol::Https,
            host: "app.example.invalid".into(),
            port: "8443".into(),
            file: "/api".into(),
        });
        assert_eq!(
            scheduled_run_scan_target(&ws, "").as_deref(),
            Some("https://app.example.invalid:8443/api")
        );
        assert_eq!(
            scheduled_run_scan_target(&ws, "http://127.0.0.1:8080/login").as_deref(),
            Some("http://127.0.0.1:8080/login")
        );
    }
}
