use super::*;

impl CollaboratorService {
    pub(in crate::service) fn has_resources(&self) -> bool {
        self.server.is_some() || !self.poll_jobs.is_empty() || self.auto_poll.is_some()
    }

    pub(in crate::service) fn has_in_flight_jobs(&self) -> bool {
        !self.poll_jobs.is_empty()
    }

    pub(in crate::service) fn status(&self) -> CollaboratorStatus {
        let (running, http_addr, dns_addr, issued_tokens) = match self.server.as_ref() {
            Some(server) => (
                true,
                Some(server.http_addr()),
                Some(server.dns_addr()),
                server.issued_tokens(),
            ),
            None => (false, None, None, Vec::new()),
        };
        let (last_poll_unix_ms, last_poll_error) = {
            let guard = self
                .poll_telemetry
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            (guard.last_poll_unix_ms, guard.last_poll_error.clone())
        };
        CollaboratorStatus {
            running,
            http_addr,
            dns_addr,
            config: self.config.clone(),
            issued_tokens,
            interactions: self.interactions.iter().rev().cloned().collect(),
            last_poll_unix_ms,
            last_poll_error,
        }
    }

    pub(in crate::service) fn issue_internal_payload(
        &mut self,
        label: &str,
    ) -> Option<OastPayloadToken> {
        self.server
            .as_mut()
            .map(|server| server.issue_payload(label))
    }

    pub(in crate::service) fn drain_pending_events(
        &mut self,
        project: ProjectKey,
    ) -> Vec<CollaboratorEvent> {
        let mut ready = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(event) = self.pending_events.pop_front() {
            if event.project() == project {
                ready.push(event);
            } else {
                retained.push_back(event);
            }
        }
        self.pending_events = retained;
        ready
    }

    pub(in crate::service) fn cancel_all(&mut self) {
        for (_, cancel) in self.active_polls.values() {
            cancel.cancel();
        }
        if let Some((cancel, _)) = &self.auto_poll {
            cancel.cancel();
        }
    }

    pub(in crate::service) async fn stop_project(&mut self, project: ProjectKey) -> Result<()> {
        for (job_project, cancel) in self.active_polls.values() {
            if *job_project == project {
                cancel.cancel();
            }
        }
        if self.project.is_none_or(|current| current == project) {
            self.stop_auto_poll().await;
        }
        let poll_result = self
            .join_poll_jobs(TokioInstant::now() + POLL_JOIN_TIMEOUT)
            .await;
        let server_result = if self.project.is_none_or(|current| current == project) {
            self.stop_server().await
        } else {
            Ok(())
        };
        self.pending_events
            .retain(|event| event.project() != project);
        if self.project == Some(project) {
            self.clear_interactions();
            *self
                .poll_telemetry
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = RelayPollTelemetry::default();
            self.config = SelfHostedOastConfig::default();
            self.project = None;
        }
        server_result.and(poll_result)
    }

    pub(in crate::service) async fn shutdown(&mut self) -> Result<()> {
        self.cancel_all();
        self.stop_auto_poll().await;
        let poll_result = self
            .join_poll_jobs(TokioInstant::now() + POLL_JOIN_TIMEOUT)
            .await;
        let server_result = self.stop_server().await;
        self.pending_events.clear();
        self.clear_interactions();
        *self
            .poll_telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = RelayPollTelemetry::default();
        self.project = None;
        server_result.and(poll_result)
    }

    pub(in crate::service) async fn next_signal(&mut self) -> CollaboratorEvent {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return event;
            }
            if self.rx.is_none() && self.poll_jobs.is_empty() {
                return std::future::pending().await;
            }
            tokio::select! {
                biased;
                Some(output) = self.poll_jobs.join_next(), if !self.poll_jobs.is_empty() => {
                    self.accept_poll_output(output);
                }
                received = recv_interaction(&mut self.rx) => match received {
                    Some(mut interaction) => {
                        if let Some(project) = self.project
                            && self.record_interaction(&mut interaction)
                        {
                            return CollaboratorEvent::InteractionObserved {
                                project,
                                interaction: Box::new(interaction),
                            };
                        }
                    }
                    None => {
                        self.rx = None;
                    }
                }
            }
        }
    }

    pub(in crate::service) async fn reclaim_signal(&mut self) {
        match self.poll_jobs.join_next().await {
            Some(output) => self.accept_poll_output(output),
            None => std::future::pending().await,
        }
    }

    pub(super) async fn start_with_config(
        &mut self,
        project: ProjectKey,
        config: SelfHostedOastConfig,
    ) -> Result<()> {
        let _ = config.validate()?;
        self.stop_server().await?;
        let (tx, rx) = mpsc::channel(INTERACTION_CHANNEL_CAPACITY);
        let server = LocalOastServer::start_with_config(tx, config.clone()).await?;
        *self
            .poll_telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = RelayPollTelemetry::default();
        self.project = Some(project);
        self.config = config;
        self.server = Some(server);
        self.rx = Some(rx);
        Ok(())
    }

    pub(in crate::service) async fn ensure_default_started(
        &mut self,
        project: ProjectKey,
    ) -> Result<()> {
        if self.server.is_some() && self.project == Some(project) {
            return Ok(());
        }
        self.stop_server().await?;
        let (tx, rx) = mpsc::channel(INTERACTION_CHANNEL_CAPACITY);
        let server = LocalOastServer::start(tx).await?;
        *self
            .poll_telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = RelayPollTelemetry::default();
        self.config = server.self_hosted_config().clone();
        self.project = Some(project);
        self.server = Some(server);
        self.rx = Some(rx);
        Ok(())
    }

    pub(super) async fn stop_server(&mut self) -> Result<()> {
        self.stop_auto_poll().await;
        self.drain_local_rx();
        let result = match self.server.take() {
            Some(server) => server.stop().await,
            None => Ok(()),
        };
        self.drain_local_rx();
        self.rx = None;
        result
    }

    pub(in crate::service) fn drain_local_rx(&mut self) -> Vec<OastInteraction> {
        let mut observed = Vec::new();
        let mut closed = false;
        while let Some(rx) = self.rx.as_mut() {
            match rx.try_recv() {
                Ok(mut interaction) => {
                    if self.record_interaction(&mut interaction) {
                        observed.push(interaction);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            self.rx = None;
        }
        observed
    }

    pub(super) fn record_interaction(&mut self, interaction: &mut OastInteraction) -> bool {
        if let Some(server) = &self.server {
            server.correlate_interaction(interaction);
        }
        if self.interaction_ids.contains(&interaction.id) {
            return false;
        }
        if interaction.raw_request.len() > MAX_INTERACTIONS_RAW_BYTES {
            interaction.raw_request.truncate(MAX_INTERACTIONS_RAW_BYTES);
        }
        let incoming_bytes = interaction.raw_request.len();
        while self.interactions.len() >= MAX_COLLABORATOR_INTERACTIONS
            || (self.total_raw_bytes.saturating_add(incoming_bytes) > MAX_INTERACTIONS_RAW_BYTES
                && !self.interactions.is_empty())
        {
            if let Some(oldest) = self.interactions.pop_front() {
                self.interaction_ids.remove(&oldest.id);
                self.total_raw_bytes = self
                    .total_raw_bytes
                    .saturating_sub(oldest.raw_request.len());
            }
        }
        self.total_raw_bytes = self.total_raw_bytes.saturating_add(incoming_bytes);
        self.interaction_ids.insert(interaction.id);
        self.interactions.push_back(interaction.clone());
        true
    }

    pub(super) fn delete_interaction(&mut self, interaction_id: Uuid) -> bool {
        let Some(pos) = self
            .interactions
            .iter()
            .position(|item| item.id == interaction_id)
        else {
            return false;
        };
        if let Some(removed) = self.interactions.remove(pos) {
            self.total_raw_bytes = self
                .total_raw_bytes
                .saturating_sub(removed.raw_request.len());
            self.interaction_ids.remove(&removed.id);
        }
        true
    }

    pub(super) fn clear_interactions(&mut self) {
        self.interactions.clear();
        self.interaction_ids.clear();
        self.total_raw_bytes = 0;
    }
}

pub(super) async fn recv_interaction(
    rx: &mut Option<mpsc::Receiver<OastInteraction>>,
) -> Option<OastInteraction> {
    match rx.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}
