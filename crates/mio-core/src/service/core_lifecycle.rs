use super::*;

impl State {
    pub(super) async fn run(mut self, mut commands: mpsc::Receiver<Command>) -> Result<()> {
        self.project_opened().await?;
        let mut scheduler_tick = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(15),
            Duration::from_secs(15),
        );
        scheduler_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // No new command or producer event is admitted while Stop notifications await the UI.
            self.flush_proxy_notifications().await;
            self.flush_repeater_notifications().await;
            let intruder_commands_ready = self.intruder.can_accept_commands();
            let commands_ready = self.ext_archive.can_accept_commands() && intruder_commands_ready;
            let can_emit_repeater_progress = self.repeater_notifications.is_empty();
            let incoming = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => Incoming::Stop,
                _ = self.events.closed() => Incoming::Stop,
                result = proxy_completion(&mut self.proxy) => Incoming::ProxyFinished(result),
                result = extra_proxy_completion(&mut self.extra_proxies) => Incoming::ProxyFinished(result),
                Some(result) = self.jobs.join_next(), if !self.jobs.is_empty() => Incoming::Completed(result),
                Some(done) = self.ipc_jobs.join_next(), if !self.ipc_jobs.is_empty() => Incoming::IpcRepeaterCompleted(done),
                tab_id = next_repeater_progress(&mut self.sends, &self.events, can_emit_repeater_progress) => Incoming::RepeaterProgress(tab_id),
                signal = self.intruder.next_signal(&self.events, false) => Incoming::Intruder(signal),
                signal = self.ext_archive.next_signal(&self.events, false) => Incoming::Ext(signal),
                signal = self.websocket_query.next_signal(&self.events, false) => Incoming::WebSocketQuery(signal),
                signal = self.http_query.next_signal(&self.events, false) => Incoming::HttpQuery(signal),
                command = commands.recv(), if commands_ready => command.map(|command| Incoming::Command(Box::new(command))).unwrap_or(Incoming::Stop),
                Some(call) = ai_ipc::next_ipc_call(&mut self.ai_ipc_rx), if commands_ready => Incoming::AiIpc(call),
                event = self.proxy_events.recv(), if intruder_commands_ready => event.map(Incoming::Proxy).unwrap_or(Incoming::Stop),
                Some(trace) = self.session_trace_rx.recv(), if intruder_commands_ready => Incoming::SessionTrace(trace),
                Some(cert) = self.server_cert_rx.recv(), if intruder_commands_ready => Incoming::ServerCertObserved(cert),
                Some(fail) = self.tls_failure_rx.recv(), if intruder_commands_ready => Incoming::ClientTlsFailure(fail),
                signal = self.plugins.next_signal(), if intruder_commands_ready => Incoming::Plugin(signal),
                Some(event) = self.ws_repeater.next_signal(), if intruder_commands_ready => Incoming::WebSocketRepeater(event),
                Some(event) = self.sequencer.next_signal(), if intruder_commands_ready => Incoming::Sequencer(event),
                event = self.collaborator.next_signal(), if intruder_commands_ready => Incoming::Collaborator(event),
                Some(signal) = self.audit.next_signal(), if intruder_commands_ready => Incoming::Audit(signal),
                _ = scheduler_tick.tick(), if commands_ready => Incoming::SchedulerTick,
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
                    self.accept_ai_ipc_call(call).await;
                }
                Incoming::IpcRepeaterCompleted(done) => {
                    self.complete_ipc_repeater(done);
                }
                Incoming::Intruder(signal) => {
                    if let Err(error) = self.intruder_signal(signal).await {
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
                        if self.observed_cert_keys.len() < 512 && self.observed_cert_keys.insert(key)
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
                        self.handle_client_tls_failure(project, host, port).await;
                    }
                }
                Incoming::SchedulerTick => {
                    self.run_scheduled_tasks_tick().await;
                }
            }
            if self.unsaved_count() != 0 && self.intruder.can_accept_commands() {
                if (self.proxy.is_some()
                    || !self.extra_proxies.is_empty()
                    || !self.jobs.is_empty()
                    || !self.ipc_jobs.is_empty()
                    || self.plugins.has_resources()
                    || self.intruder.has_resources()
                    || self.ws_repeater.has_resources()
                    || self.sequencer.has_resources()
                    || self.audit.has_resources()
                    || self.collaborator.has_resources())
                    && let Err(error) = self.stop_resources().await
                {
                    self.storage_error.get_or_insert_with(|| error.to_string());
                }
                self.storage_state().await;
            }
        }
        self.cancel.cancel();
        if let Some(mut ipc_server) = self.ai_ipc_server.take() {
            ipc_server.shutdown().await;
        }
        self.ai_ipc_rx = None;
        self.stop_ipc_jobs().await;
        // Shutdown must still persist terminal states after query cancellation.
        self.store.cancel_queries_with(CancellationToken::new());
        let result = self.stop_resources().await;
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
        self.refresh_project_disk_bytes();
        self.emit(Event::ProjectOpened {
            project: self.project,
            path: self.store.path.clone(),
            workspace: self.store.workspace()?,
        })
        .await;
        self.proxy_state().await;
        if let Ok(audit_state) = self.store.read_audit_state()
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
            address: self.proxy.as_ref().map(ProxyHandle::address),
            cancel: self.proxy.as_ref().map(ProxyHandle::cancellation_token),
            intercept_requests: self.intercept_requests,
            intercept_responses: self.intercept_responses,
        })
        .await;
        if !self.extra_proxies.is_empty() {
            let mut listeners = Vec::new();
            if let Some(proxy) = &self.proxy {
                listeners.push((
                    self.primary_proxy_id.unwrap_or(Uuid::from_u128(1)),
                    proxy.address(),
                ));
            }
            for (id, proxy) in &self.extra_proxies {
                listeners.push((*id, proxy.address()));
            }
            self.emit(Event::ProxyListeners {
                project: self.project,
                listeners,
            })
            .await;
        }
    }

    pub(super) async fn authority(&mut self) -> Result<MitmAuthority> {
        if self.ca.is_none() {
            self.ca = Some(tokio::task::spawn_blocking(authority::load_or_create).await??);
        }
        Ok(self.ca.as_ref().unwrap().clone())
    }

    #[cfg(test)]
    pub(super) async fn replace_project(&mut self, next: ProjectStore) -> Result<()> {
        self.replace_project_with(next, false, None).await
    }

    pub(super) async fn replace_project_with(
        &mut self,
        mut next: ProjectStore,
        pause_automated_tasks: bool,
        extensions_disabled: Option<bool>,
    ) -> Result<()> {
        self.require_saved()?;
        self.store.require_intruder_sessions_resolved()?;
        let intruder =
            intruder_service::IntruderState::with_resource_pools(next.intruder_resource_pools()?);
        self.stop_resources().await?;
        self.require_saved()?;
        self.store.checkpoint()?;
        let effective_pause = pause_automated_tasks
            || !next
                .workspace_arc()
                .map(|w| w.tasks_auto_start)
                .unwrap_or(true);
        next.finish_pending(true, false)?;
        next.recover_intruder_runs_on_open(effective_pause)?;
        next.cancel_queries_with(self.cancel.clone());
        let previous_project = self.project;
        self.project = ProjectKey {
            id: next.id,
            generation: self.project.generation + 1,
        };
        self.store = next;
        if let Some(disabled) = extensions_disabled {
            self.extensions_disabled = disabled;
        }
        self.history_dirty = false;
        self.websocket_history_dirty = false;
        self.intruder = intruder;
        self.quiesced = false;
        self.provenance.clear();
        self.ipc_repeater_revisions.clear();
        self.observed_cert_keys.clear();
        self.last_scheduled_fire.clear();
        let restored_annotated = self.store.annotated_live_flow_ids().unwrap_or_default();
        *self
            .annotated_flows
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = restored_annotated;
        self.ws_repeater.drop_project(previous_project);
        self.sequencer.drop_project(previous_project);
        self.audit.drop_project(previous_project);
        self.collaborator.stop_project(previous_project).await?;
        self.store
            .recover_audit_tasks_on_open(effective_pause, &self.cancel)?;
        self.ws_repeater.set_project(self.project);
        self.ws_repeater
            .set_injectors(self.proxy.as_ref().map(ProxyHandle::websocket_injectors));
        let workspace = self.store.workspace_arc()?;
        self.sync_compiled_client_certs(&workspace).await;
        self.sessions.reset_project(self.project, &workspace);
        self.sync_sessions_workspace(&workspace);
        self.ws_repeater
            .restore_workspace_tabs(self.project, workspace.websocket_tabs.clone());
        if let Ok(config) = Self::ws_repeater_send_config(
            &self.preferences,
            &workspace,
            self.compiled_client_certs.clone(),
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
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        let name = self
            .store
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
        self.websocket_query.cancel();
        self.http_query.cancel();
        self.intruder.cancel_all();
        for send in self.sends.values_mut() {
            send.cancel.cancel();
            send.progress_rx = None;
        }
        self.stop_ipc_jobs().await;
        if let Some(proxy) = &self.proxy {
            proxy.request_stop();
        }
        for (_, proxy) in &self.extra_proxies {
            proxy.request_stop();
        }
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
            self.store.finish_pending(false, false)?;
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
        let Ok(mut workspace) = self.store.workspace() else {
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
        if self.store.save_workspace(&workspace).is_ok() {
            if let Ok(matcher) =
                crate::http::build_ssl_pass_through_matcher(&workspace.ssl_pass_through_rules)
            {
                if let Some(proxy) = &self.proxy {
                    proxy.set_ssl_pass_through(matcher.clone());
                }
                for (_, proxy) in &self.extra_proxies {
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
        let Ok(workspace) = self.store.workspace_arc() else {
            return;
        };
        if workspace.scheduled_tasks.is_empty() {
            return;
        }
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
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
            self.last_scheduled_fire.insert(task.id, now_min);
            match task.action {
                ScheduledTaskAction::PauseTasks => {
                    if let Ok(audit_state) = self.store.read_audit_state() {
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
                    if let Ok(audit_state) = self.store.read_audit_state() {
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
                    let target_url = if task.description.trim().starts_with("http://")
                        || task.description.trim().starts_with("https://")
                    {
                        Some(task.description.trim().to_owned())
                    } else {
                        workspace
                            .scope
                            .include
                            .iter()
                            .find(|r| r.enabled && !r.prefix.trim().is_empty())
                            .map(|r| r.prefix.trim().to_owned())
                    };
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

fn current_local_hh_mm(now_secs: i64) -> (u32, u32) {
    crate::codec_util::local_hh_mm(now_secs)
}

fn scheduled_task_due(trigger_spec: &str, now_secs: i64, last_fired_min: Option<i64>) -> bool {
    let spec = trigger_spec.trim();
    if spec.is_empty() {
        return false;
    }
    let now_min = now_secs / 60;
    if last_fired_min == Some(now_min) {
        return false;
    }
    if let Some(rest) = spec
        .strip_prefix("every:")
        .or_else(|| spec.strip_prefix("EVERY:"))
        .map(str::trim)
    {
        let mins = rest
            .trim_end_matches(['m', 'M'])
            .parse::<i64>()
            .ok()
            .filter(|&m| m >= 1);
        if let Some(interval) = mins {
            return last_fired_min.is_none_or(|prev| now_min - prev >= interval)
                && (now_min % interval == 0 || last_fired_min.is_some());
        }
    }
    if let Some((hh, mm)) = spec.split_once(':')
        && let (Ok(h), Ok(m)) = (hh.trim().parse::<u32>(), mm.trim().parse::<u32>())
    {
        let (cur_h, cur_m) = current_local_hh_mm(now_secs);
        return cur_h == h && cur_m == m;
    }
    false
}
