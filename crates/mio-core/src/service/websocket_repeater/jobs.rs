use super::*;

impl WebSocketRepeaterService {
    pub(crate) async fn next_signal(&mut self) -> Option<WebSocketRepeaterEvent> {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return Some(event);
            }
            tokio::select! {
                biased;
                Some(output) = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    let events = self.accept_job(output);
                    self.pending_events.extend(events);
                }
                update = self.peers_rx.recv() => {
                    let update = update.expect("WebSocket Repeater owns its peer channel sender");
                    let events = self.accept_peer(update);
                    self.pending_events.extend(events);
                }
            }
        }
    }

    pub(crate) async fn reclaim_signal(&mut self) {
        match self.jobs.join_next().await {
            Some(output) => {
                let events = self.accept_job(output);
                self.pending_events.extend(events);
            }
            None => std::future::pending().await,
        }
    }

    pub(super) fn spawn_job(
        &mut self,
        context: JobContext,
        job: impl Future<Output = JobOutput> + Send + 'static,
    ) {
        let cancel = context.cancel.clone();
        let project = context.project;
        let id = context.id;
        let handle = self.jobs.spawn(job);
        self.job_cancels.insert(id, (project, cancel));
        drop(handle);
    }

    pub(super) fn spawn_disconnect_job(
        &mut self,
        context: JobContext,
        session: SharedSession,
        reader: JoinHandle<()>,
    ) {
        let job_context = context.clone();
        self.spawn_job(context, async move {
            let mut guard = match timeout_at(
                tokio::time::Instant::now() + DISCONNECT_TIMEOUT,
                session.lock(),
            )
            .await
            {
                Ok(guard) => guard,
                Err(_) => {
                    job_context.cancel.cancel();
                    match timeout_at(
                        tokio::time::Instant::now() + DISCONNECT_TIMEOUT,
                        session.lock(),
                    )
                    .await
                    {
                        Ok(guard) => guard,
                        Err(_) => {
                            reader.abort();
                            return JobOutput::Disconnected {
                                context: job_context,
                                result: Err("断开 WebSocket 会话等待锁超时".into()),
                            };
                        }
                    }
                }
            };
            let result = guard.disconnect().await;
            drop(guard);
            let _ = timeout_at(tokio::time::Instant::now() + DISCONNECT_TIMEOUT, reader).await;
            JobOutput::Disconnected {
                context: job_context,
                result: result.map_err(|error| error.to_string()),
            }
        });
    }

    pub(super) fn accept_job(
        &mut self,
        output: std::result::Result<JobOutput, tokio::task::JoinError>,
    ) -> Vec<WebSocketRepeaterEvent> {
        let Ok(output) = output else {
            return Vec::new();
        };
        self.job_cancels.remove(&output.context_id());
        match output {
            JobOutput::Connected {
                context,
                session_id,
                session,
                reader,
                response,
            } => {
                let tab = self
                    .tab_mut(context.project, context.tab_id)
                    .filter(|tab| tab.pending_session_id == Some(session_id));
                match tab {
                    Some(tab) => {
                        let previous = tab.active.replace(ActiveStandalone {
                            session_id,
                            session,
                            cancel: context.cancel.clone(),
                            reader,
                        });
                        tab.pending_session_id = None;
                        if let Some(old) = previous {
                            let cleanup = JobContext {
                                id: Uuid::new_v4(),
                                project: context.project,
                                tab_id: context.tab_id,
                                operation_id: Uuid::nil(),
                                cancel: old.cancel,
                            };
                            self.spawn_disconnect_job(cleanup, old.session, old.reader);
                        }
                        vec![WebSocketRepeaterEvent::Connected {
                            project: context.project,
                            operation_id: context.operation_id,
                            tab_id: context.tab_id,
                            mode: WebSocketRepeaterConnectionMode::Standalone {
                                flow_id: session_id,
                            },
                            response,
                            result: Ok(()),
                        }]
                    }
                    None => {
                        context.cancel.cancel();
                        let cleanup = JobContext {
                            id: Uuid::new_v4(),
                            project: context.project,
                            tab_id: context.tab_id,
                            operation_id: Uuid::nil(),
                            cancel: context.cancel,
                        };
                        self.spawn_disconnect_job(cleanup, session, reader);
                        Vec::new()
                    }
                }
            }
            JobOutput::ConnectFailed { context, error } => {
                if let Some(tab) = self.tab_mut(context.project, context.tab_id) {
                    tab.pending_session_id = None;
                }
                let mode = self.tab_mode(context.project, context.tab_id);
                vec![WebSocketRepeaterEvent::Connected {
                    project: context.project,
                    operation_id: context.operation_id,
                    tab_id: context.tab_id,
                    mode,
                    response: None,
                    result: Err(error),
                }]
            }
            JobOutput::Sent {
                context,
                draft_revision,
                result,
            } => {
                if let Ok(item) = &result
                    && let Some(tab) = self.tab_mut(context.project, context.tab_id)
                {
                    tab.metadata.push_history(item.clone());
                }
                vec![WebSocketRepeaterEvent::Sent {
                    project: context.project,
                    operation_id: context.operation_id,
                    tab_id: context.tab_id,
                    draft_revision,
                    mode: self.tab_mode(context.project, context.tab_id),
                    result,
                }]
            }
            JobOutput::Disconnected { context, result } => {
                if context.operation_id.is_nil() {
                    return Vec::new();
                }
                vec![WebSocketRepeaterEvent::Disconnected {
                    project: context.project,
                    operation_id: context.operation_id,
                    tab_id: context.tab_id,
                    result,
                }]
            }
        }
    }

    pub(super) fn accept_peer(&mut self, update: PeerUpdate) -> Vec<WebSocketRepeaterEvent> {
        let Some(tab) = self.tab_mut(update.project, update.tab_id) else {
            return Vec::new();
        };
        if !tab
            .active
            .as_ref()
            .is_some_and(|active| active.session_id == update.session_id)
        {
            return Vec::new();
        }
        match update.payload {
            PeerPayload::Frame(item) => {
                let closed = item.kind == WebSocketKind::Close;
                let flow_id = item.flow_id.unwrap_or(update.session_id);
                tab.metadata.push_history(item.clone());
                let mut events = vec![WebSocketRepeaterEvent::Received {
                    project: update.project,
                    tab_id: update.tab_id,
                    flow_id,
                    item,
                }];
                if closed {
                    if let Some(old) = tab.active.take() {
                        let cleanup = JobContext {
                            id: Uuid::new_v4(),
                            project: update.project,
                            tab_id: update.tab_id,
                            operation_id: Uuid::nil(),
                            cancel: old.cancel,
                        };
                        self.spawn_disconnect_job(cleanup, old.session, old.reader);
                    }
                    events.push(self.session_closed(update.project, update.tab_id, None));
                }
                events
            }
            PeerPayload::Closed(reason) => {
                if let Some(old) = tab.active.take() {
                    let cleanup = JobContext {
                        id: Uuid::new_v4(),
                        project: update.project,
                        tab_id: update.tab_id,
                        operation_id: Uuid::nil(),
                        cancel: old.cancel,
                    };
                    self.spawn_disconnect_job(cleanup, old.session, old.reader);
                }
                vec![self.session_closed(update.project, update.tab_id, reason)]
            }
        }
    }

    pub(super) fn session_closed(
        &self,
        project: ProjectKey,
        tab_id: TabId,
        reason: Option<String>,
    ) -> WebSocketRepeaterEvent {
        WebSocketRepeaterEvent::SessionClosed {
            project,
            tab_id,
            mode: self.tab_mode(project, tab_id),
            reason,
        }
    }

    pub(super) fn cancel_jobs(&mut self, project: ProjectKey) {
        for (owner, cancel) in self.job_cancels.values() {
            if *owner == project {
                cancel.cancel();
            }
        }
        self.job_cancels.retain(|_, (owner, _)| *owner != project);
    }

    pub(super) fn cancel_all_jobs(&mut self) {
        for (_, cancel) in self.job_cancels.values() {
            cancel.cancel();
        }
        self.job_cancels.clear();
    }

    pub(super) async fn stop_tab_states(&mut self, tabs: Vec<TabState>) -> Result<()> {
        let deadline = tokio::time::Instant::now() + JOB_WAIT_TIMEOUT;
        let mut sessions = JoinSet::new();
        for mut tab in tabs {
            tab.connect_cancel.cancel();
            tab.pending_session_id = None;
            if let Some(active) = tab.active.take() {
                active.cancel.cancel();
                let reader = active.reader;
                let session = active.session;
                sessions.spawn(async move {
                    let mut guard = session.lock().await;
                    let result = guard.disconnect().await;
                    drop(guard);
                    let _ =
                        timeout_at(tokio::time::Instant::now() + DISCONNECT_TIMEOUT, reader).await;
                    result
                });
            }
        }
        let mut failures = Vec::new();
        while !sessions.is_empty() {
            match timeout_at(deadline, sessions.join_next()).await {
                Ok(Some(Ok(Ok(())))) => {}
                Ok(Some(Ok(Err(error)))) => failures.push(error.to_string()),
                Ok(Some(Err(error))) => failures.push(error.to_string()),
                Ok(None) => break,
                Err(_) => {
                    sessions.abort_all();
                    failures.push("WebSocket Repeater 会话回收超过期限".into());
                    break;
                }
            }
        }
        while !self.jobs.is_empty() {
            match timeout_at(deadline, self.jobs.join_next()).await {
                Ok(Some(output)) => {
                    let _ = self.accept_job(output);
                }
                Ok(None) => break,
                Err(_) => {
                    self.jobs.abort_all();
                    failures.push("WebSocket Repeater 后台任务回收超过期限".into());
                    break;
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("{}", failures.join("；"))
        }
    }
}

impl JobOutput {
    fn context_id(&self) -> Uuid {
        match self {
            Self::Connected { context, .. }
            | Self::ConnectFailed { context, .. }
            | Self::Sent { context, .. }
            | Self::Disconnected { context, .. } => context.id,
        }
    }
}

pub(super) async fn read_standalone_session(
    receiver: Option<
        mpsc::Receiver<Result<mio_plugin_api::WebSocketMessage, mio_proxy::ProxyError>>,
    >,
    cancel: CancellationToken,
    peers: mpsc::Sender<PeerUpdate>,
    project: ProjectKey,
    tab_id: TabId,
    session_id: Uuid,
    flow_id: FlowId,
) {
    let Some(mut receiver) = receiver else {
        let _ = peers
            .send(PeerUpdate {
                project,
                tab_id,
                session_id,
                payload: PeerPayload::Closed(Some("会话缺少接收通道".into())),
            })
            .await;
        return;
    };
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            frame = receiver.recv() => match frame {
                Some(Ok(mut message)) => {
                    message.flow_id = flow_id;
                    let item = WebSocketRepeaterHistoryItem::from_message(&message);
                    let closed = item.kind == WebSocketKind::Close;
                    let sent = peers
                        .send(PeerUpdate {
                            project,
                            tab_id,
                            session_id,
                            payload: PeerPayload::Frame(item),
                        })
                        .await;
                    if sent.is_err() || closed {
                        break;
                    }
                }
                Some(Err(error)) => {
                    let _ = peers
                        .send(PeerUpdate {
                            project,
                            tab_id,
                            session_id,
                            payload: PeerPayload::Closed(Some(error.to_string())),
                        })
                        .await;
                    break;
                }
                None => {
                    let _ = peers
                        .send(PeerUpdate {
                            project,
                            tab_id,
                            session_id,
                            payload: PeerPayload::Closed(None),
                        })
                        .await;
                    break;
                }
            },
        }
    }
}
