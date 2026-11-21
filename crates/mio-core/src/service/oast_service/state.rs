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
                self.collaborator.stop_auto_poll().await;
                self.collaborator.start_with_config(project, config).await?;
                if self.collaborator.config.enabled
                    && let Some((request, send_config)) = self.prepare_collaborator_poll()?
                {
                    self.collaborator.start_auto_poll(
                        self.collaborator.config.poll_interval_ms,
                        request,
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

    fn prepare_collaborator_poll(&self) -> Result<Option<(HttpRequest, SendConfig)>> {
        let Some(polling_url) = self.collaborator.config.validate()? else {
            return Ok(None);
        };
        let workspace = self.store.workspace_arc()?;
        let upstream_tls = self
            .preferences
            .effective_upstream_tls(&workspace.upstream_tls)?;
        let upstream_route = workspace
            .upstream_proxy
            .route_for_url_with_tls(polling_url.as_str(), upstream_tls.clone())?;
        let hostname_overrides = workspace.upstream_proxy.compiled_hostname_overrides()?;
        let timeout_secs = workspace
            .repeater_settings
            .request_timeout_secs
            .clamp(1, 60);
        let send_config = SendConfig {
            network: mio_proxy::SendNetworkConfig {
                upstream_route,
                hostname_overrides,
            },
            tls: mio_proxy::UpstreamTlsConfig {
                trust: upstream_tls,
                client_certificates: self.compiled_client_certs.clone(),
                ..Default::default()
            },
            limits: mio_proxy::SendLimits {
                max_body_bytes: MAX_POLL_BODY_BYTES,
                timeout: Duration::from_secs(timeout_secs),
                ..Default::default()
            },
            http: mio_proxy::SendHttpConfig {
                version: Some(HttpVersion::Http11),
                ..Default::default()
            },
            ..SendConfig::default()
        };
        let request = build_poll_request(
            &polling_url,
            self.collaborator.config.authorization_header.as_deref(),
        );
        Ok(Some((request, send_config)))
    }
}

impl CollaboratorService {
    pub(super) async fn stop_auto_poll(&mut self) {
        if let Some((cancel, handle)) = self.auto_poll.take() {
            cancel.cancel();
            let _ = timeout_at(TokioInstant::now() + POLL_JOIN_TIMEOUT, handle).await;
        }
    }

    pub(super) fn start_auto_poll(
        &mut self,
        poll_interval_ms: u64,
        request: HttpRequest,
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
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    biased;
                    _ = fwd_cancel.cancelled() => None,
                    item = local_rx.recv() => item,
                };
                let Some(item) = next else {
                    break;
                };
                if fwd_tx.send(item).await.is_err() {
                    break;
                }
            }
        });
        let period = Duration::from_millis(poll_interval_ms.max(250));
        let loop_cancel = cancel.clone();
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
                    r = send(request.clone(), send_config.clone(), req_cancel.clone()) => r,
                };
                if let Ok(resp) = res
                    && (200..300).contains(&resp.status)
                    && let Ok(items) = serde_json::from_slice::<Vec<OastInteraction>>(&resp.body)
                {
                    for item in items {
                        if merged_tx.send(item).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        self.auto_poll = Some((cancel, handle));
    }
}
