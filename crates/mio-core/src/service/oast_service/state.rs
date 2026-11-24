use super::*;

impl State {
    pub(in crate::service) async fn collaborator_command(
        &mut self,
        command: CollaboratorCommand,
    ) -> Result<()> {
        match command {
            CollaboratorCommand::QueryStatus { project } => {
                self.check_project(project)?;
                self.collaborator.project = Some(project);
                let observed = self.collaborator.drain_local_rx();
                for interaction in observed {
                    let event = CollaboratorEvent::InteractionObserved {
                        project,
                        interaction: Box::new(interaction),
                    };
                    self.on_collaborator_event_for_audit(&event).await;
                    self.emit(Event::Collaborator(event)).await;
                }
                self.emit_collaborator_status(project).await;
            }
            CollaboratorCommand::Start { project, config } => {
                self.check_project(project)?;
                let poll_target = self.prepare_collaborator_poll_target_for_config(&config)?;
                self.collaborator.stop_auto_poll().await;
                self.collaborator.start_with_config(project, config).await?;
                self.flush_collaborator_events().await;
                if let Some((polling_url, auth_header, send_config)) = poll_target {
                    self.collaborator.start_auto_poll(
                        self.collaborator.config.poll_interval_ms,
                        polling_url,
                        auth_header,
                        send_config,
                        self.cancel.child_token(),
                    );
                }
                self.emit_collaborator_status(project).await;
            }
            CollaboratorCommand::Stop { project } => {
                self.check_project(project)?;
                self.collaborator.project = Some(project);
                self.collaborator.stop_auto_poll().await;
                self.collaborator.stop_server().await?;
                self.flush_collaborator_events().await;
                self.emit_collaborator_status(project).await;
            }
            CollaboratorCommand::GeneratePayloads {
                project,
                count,
                label,
            } => {
                self.check_project(project)?;
                ensure!(
                    (1..=MAX_COLLABORATOR_BATCH_PAYLOADS).contains(&count),
                    "Collaborator 单次生成 payload 数量必须在 1..={MAX_COLLABORATOR_BATCH_PAYLOADS} 之间"
                );
                self.collaborator.ensure_default_started(project).await?;
                let server = self
                    .collaborator
                    .server
                    .as_mut()
                    .expect("local OAST server started");
                let mut payloads = Vec::with_capacity(count);
                for _ in 0..count {
                    payloads.push(server.issue_payload(label.clone()));
                }
                self.emit_collaborator_status(project).await;
                self.emit(Event::Collaborator(CollaboratorEvent::PayloadsGenerated {
                    project,
                    payloads,
                }))
                .await;
            }
            CollaboratorCommand::PollNow { project } => {
                self.check_project(project)?;
                self.collaborator.project = Some(project);
                let local_observed = self.collaborator.drain_local_rx();
                for interaction in local_observed {
                    let event = CollaboratorEvent::InteractionObserved {
                        project,
                        interaction: Box::new(interaction),
                    };
                    self.on_collaborator_event_for_audit(&event).await;
                    self.emit(Event::Collaborator(event)).await;
                }
                if self.collaborator.config.enabled {
                    match self.prepare_collaborator_poll() {
                        Ok(Some((request, send_config))) => {
                            self.collaborator.spawn_poll(
                                project,
                                request,
                                send_config,
                                self.cancel.clone(),
                            );
                        }
                        Ok(None) => {
                            self.emit_collaborator_status(project).await;
                        }
                        Err(error) => {
                            self.emit(Event::Collaborator(CollaboratorEvent::Error {
                                project,
                                message: error.to_string(),
                            }))
                            .await;
                        }
                    }
                } else {
                    self.emit_collaborator_status(project).await;
                }
            }
            CollaboratorCommand::ClearInteractions { project } => {
                self.check_project(project)?;
                self.collaborator.project = Some(project);
                let _ = self.collaborator.drain_local_rx();
                self.collaborator.clear_interactions();
                self.emit_collaborator_status(project).await;
            }
            CollaboratorCommand::DeleteInteraction {
                project,
                interaction_id,
            } => {
                self.check_project(project)?;
                self.collaborator.project = Some(project);
                let _ = self.collaborator.drain_local_rx();
                let _ = self.collaborator.delete_interaction(interaction_id);
                self.emit_collaborator_status(project).await;
            }
        }
        Ok(())
    }

    pub(in crate::service) async fn flush_collaborator_events(&mut self) {
        for event in self.collaborator.drain_pending_events(self.project) {
            self.on_collaborator_event_for_audit(&event).await;
            self.emit(Event::Collaborator(event)).await;
        }
    }

    pub(in crate::service) async fn emit_collaborator_status(&mut self, project: ProjectKey) {
        let status = Box::new(self.collaborator.status());
        self.emit(Event::Collaborator(CollaboratorEvent::Status {
            project,
            status,
        }))
        .await;
    }

    fn prepare_collaborator_poll_target_for_config(
        &self,
        config: &SelfHostedOastConfig,
    ) -> Result<Option<(Url, Option<String>, SendConfig)>> {
        let Some(polling_url) = config.validate()? else {
            return Ok(None);
        };
        let workspace = self.store.metadata().workspace_arc()?;
        let mut send_config =
            self.build_tool_send_config_for_url(&workspace, polling_url.as_str())?;
        let timeout_secs = workspace
            .repeater_settings
            .request_timeout_secs
            .clamp(1, 60);
        send_config.limits.max_body_bytes = MAX_POLL_BODY_BYTES;
        send_config.limits.timeout = Duration::from_secs(timeout_secs);
        send_config.http.version = Some(HttpVersion::Http11);
        Ok(Some((
            polling_url,
            config.authorization_header.clone(),
            send_config,
        )))
    }

    fn prepare_collaborator_poll_target(
        &self,
    ) -> Result<Option<(Url, Option<String>, SendConfig)>> {
        self.prepare_collaborator_poll_target_for_config(&self.collaborator.config)
    }
    fn prepare_collaborator_poll(&self) -> Result<Option<(HttpRequest, SendConfig)>> {
        let Some((polling_url, auth_header, send_config)) =
            self.prepare_collaborator_poll_target()?
        else {
            return Ok(None);
        };
        let cursor = self
            .collaborator
            .poll_telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cursor
            .clone();
        let request = build_poll_request(&polling_url, auth_header.as_deref(), cursor.as_deref());
        Ok(Some((request, send_config)))
    }
}

impl CollaboratorService {
    pub(super) async fn stop_auto_poll(&mut self) {
        if let Some((cancel, mut fwd_handle, mut poll_handle)) = self.auto_poll.take() {
            cancel.cancel();
            let poll_deadline = TokioInstant::now() + POLL_JOIN_TIMEOUT;
            if timeout_at(poll_deadline, &mut poll_handle).await.is_err() {
                poll_handle.abort();
            }
            let fwd_deadline = TokioInstant::now() + Duration::from_secs(1);
            match timeout_at(fwd_deadline, &mut fwd_handle).await {
                Ok(Ok((unsent, local_rx))) => {
                    let observed = self.drain_local_rx();
                    if let Some(project) = self.project {
                        for item in observed {
                            self.pending_events
                                .push_back(CollaboratorEvent::InteractionObserved {
                                    project,
                                    interaction: Box::new(item),
                                });
                        }
                    }
                    if let Some(mut item) = unsent
                        && self.record_interaction(&mut item)
                        && let Some(project) = self.project
                    {
                        self.pending_events
                            .push_back(CollaboratorEvent::InteractionObserved {
                                project,
                                interaction: Box::new(item),
                            });
                    }
                    self.rx = local_rx;
                }
                Ok(Err(_)) => {}
                Err(_) => {
                    fwd_handle.abort();
                }
            }
        }
    }

    pub(super) fn start_auto_poll(
        &mut self,
        poll_interval_ms: u64,
        polling_url: Url,
        auth_header: Option<String>,
        send_config: SendConfig,
        cancel: CancellationToken,
    ) {
        let Some(mut local_rx) = self.rx.take() else {
            return;
        };
        let (merged_tx, merged_rx) = mpsc::channel(INTERACTION_CHANNEL_CAPACITY);
        self.rx = Some(merged_rx);
        let fwd_tx = merged_tx.clone();
        let fwd_cancel = cancel.clone();
        let fwd_handle = tokio::spawn(async move {
            let mut disconnected = false;
            let mut unsent = None;
            loop {
                let next = tokio::select! {
                    biased;
                    _ = fwd_cancel.cancelled() => {
                        break;
                    }
                    item = local_rx.recv() => item,
                };
                let Some(item) = next else {
                    disconnected = true;
                    break;
                };
                let permit = tokio::select! {
                    biased;
                    _ = fwd_cancel.cancelled() => {
                        if let Err(mpsc::error::TrySendError::Full(item) | mpsc::error::TrySendError::Closed(item)) =
                            fwd_tx.try_send(item)
                        {
                            unsent = Some(item);
                        }
                        break;
                    }
                    permit = fwd_tx.reserve() => match permit {
                        Ok(permit) => permit,
                        Err(_) => {
                            unsent = Some(item);
                            break;
                        }
                    },
                };
                permit.send(item);
            }
            (unsent, if disconnected { None } else { Some(local_rx) })
        });
        let period = Duration::from_millis(poll_interval_ms.max(250));
        let loop_cancel = cancel.clone();
        let telemetry = Arc::clone(&self.poll_telemetry);
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval_at(TokioInstant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    _ = loop_cancel.cancelled() => break,
                    _ = merged_tx.closed() => break,
                    _ = interval.tick() => {}
                }
                let cursor = telemetry
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .cursor
                    .clone();
                let request = build_poll_request(&polling_url, auth_header.as_deref(), cursor.as_deref());
                let req_cancel = loop_cancel.child_token();
                let res = tokio::select! {
                    biased;
                    _ = loop_cancel.cancelled() => {
                        req_cancel.cancel();
                        break;
                    }
                    _ = merged_tx.closed() => {
                        req_cancel.cancel();
                        break;
                    }
                    r = send(request, send_config.clone(), req_cancel.clone()) => r,
                };
                match res {
                    Ok(resp) if (200..300).contains(&resp.status) => {
                        match parse_poll_response(&resp.body) {
                            Ok((items, next_cursor)) => {
                                {
                                    let mut guard = telemetry
                                        .lock()
                                        .unwrap_or_else(PoisonError::into_inner);
                                    if let Some(c) = next_cursor.filter(|c| !c.trim().is_empty()) {
                                        guard.cursor = Some(c);
                                    }
                                    guard.last_poll_unix_ms = Some(crate::codec_util::now_unix_ms());
                                    guard.last_poll_error = None;
                                }
                                for item in items {
                                    if merged_tx.send(item).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            Err(err) => {
                                telemetry
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .last_poll_error = Some(err);
                            }
                        }
                    }
                    Ok(resp) => {
                        telemetry
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .last_poll_error = Some(format!("Collaborator 轮询返回 HTTP {}", resp.status));
                    }
                    Err(err) => {
                        telemetry
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .last_poll_error = Some(format!("Collaborator 轮询请求失败：{err}"));
                    }
                }
            }
        });
        self.auto_poll = Some((cancel, fwd_handle, handle));
    }
}
