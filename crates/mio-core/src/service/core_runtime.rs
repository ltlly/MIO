use super::*;

impl Runtime {
    /// Starts an isolated runtime with in-memory user preferences (no user profile access).
    pub fn start() -> Result<(Self, Client)> {
        let ipc = ai_ipc::env_ipc_socket_path().map(|path| (path, false, true));
        Self::start_inner(None, ipc)
    }

    /// Starts an isolated runtime bound to an explicit Unix domain socket for AI IPC.
    pub fn start_with_ipc_socket(socket_path: std::path::PathBuf) -> Result<(Self, Client)> {
        Self::start_inner(None, Some((socket_path, false, true)))
    }

    /// Starts a desktop runtime using the explicitly selected user preference file.
    pub fn start_with_preferences(path: std::path::PathBuf) -> Result<(Self, Client)> {
        let ipc = ai_ipc::env_ipc_socket_path().map(|path| (path, false, true));
        Self::start_inner(Some(path), ipc)
    }

    /// Starts a desktop app runtime with user preferences and the restrained local AI IPC listener.
    pub fn start_app_with_preferences(path: std::path::PathBuf) -> Result<(Self, Client)> {
        let ipc = match ai_ipc::env_ipc_socket_path() {
            Some(explicit) => Some((explicit, false, true)),
            None => Some((ai_ipc::default_ipc_socket_path(), true, false)),
        };
        Self::start_inner(Some(path), ipc)
    }

    fn start_inner(
        preferences_path: Option<std::path::PathBuf>,
        ipc_spec: Option<(std::path::PathBuf, bool, bool)>,
    ) -> Result<(Self, Client)> {
        let (commands, receiver) = mpsc::channel(64);
        let (events, event_receiver) = mpsc::channel(128);
        let (ready, initialized) = std::sync::mpsc::sync_channel(1);
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker = thread::Builder::new()
            .name("mio-services".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(
                        std::thread::available_parallelism()
                            .map(|n| n.get().clamp(4, 8))
                            .unwrap_or(4),
                    )
                    .thread_name("mio-network")
                    .enable_all()
                    .build()?;
                let state = State::with_preference_paths(
                    events,
                    worker_cancel,
                    crate::preferences::PreferencesStore::new(preferences_path.clone()),
                    preferences_path,
                );
                match state {
                    Ok(mut state) => {
                        if let Some((sock_path, strict_parent, must_succeed)) = ipc_spec {
                            let _enter = runtime.enter();
                            match ai_ipc::start_ipc_server(&sock_path, strict_parent, &state.cancel)
                            {
                                Ok((server, rx)) => {
                                    state.ai_ipc_server = Some(server);
                                    state.ai_ipc_rx = Some(rx);
                                }
                                Err(error) if must_succeed => {
                                    let _ = ready.send(Err(error.to_string()));
                                    return Err(error);
                                }
                                Err(_) => {}
                            }
                        }
                        let _ = ready.send(Ok::<_, String>(()));
                        let result = runtime.block_on(Box::pin(state.run(receiver)));
                        runtime.shutdown_timeout(Duration::from_secs(5));
                        result
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        Err(error)
                    }
                }
            })?;
        match initialized.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok((
                Self {
                    cancel: cancel.clone(),
                    worker: Some(worker),
                },
                Client {
                    commands,
                    events: event_receiver,
                    shutdown: cancel,
                },
            )),
            error => {
                cancel.cancel();
                let _ = worker.join();
                anyhow::bail!("后台服务启动失败：{error:?}")
            }
        }
    }

    pub fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.join()
    }

    /// Waits for the service thread and its resources without requesting shutdown.
    pub fn join(mut self) -> Result<()> {
        self.worker
            .take()
            .expect("worker belongs to runtime")
            .join()
            .map_err(|_| anyhow::anyhow!("后台服务异常退出"))?
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) async fn proxy_completion(
    proxy: &mut Option<ProxyHandle>,
) -> Result<(), mio_proxy::ProxyError> {
    match proxy {
        Some(proxy) => proxy.join().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn extra_proxy_completion(
    proxies: &mut [(Uuid, ProxyHandle)],
) -> Result<(), mio_proxy::ProxyError> {
    let mut futures: Vec<_> = proxies
        .iter_mut()
        .filter(|(_, proxy)| !proxy.is_joined())
        .map(|(_, proxy)| Box::pin(proxy.join()))
        .collect();
    if futures.is_empty() {
        return std::future::pending().await;
    }
    std::future::poll_fn(|cx| {
        for fut in &mut futures {
            if let std::task::Poll::Ready(res) = fut.as_mut().poll(cx) {
                return std::task::Poll::Ready(res);
            }
        }
        std::task::Poll::Pending
    })
    .await
}

pub(super) fn annotated_flows_tracker(
    flows: Arc<RwLock<HashSet<FlowId>>>,
    cancel: CancellationToken,
) -> Arc<dyn Fn(FlowId) -> bool + Send + Sync> {
    Arc::new(move |flow_id| {
        !cancel.is_cancelled()
            && flows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&flow_id)
    })
}

impl State {
    #[cfg(test)]
    pub(super) fn new(events: mpsc::Sender<Event>, cancel: CancellationToken) -> Result<Self> {
        Self::with_preferences(
            events,
            cancel,
            crate::preferences::PreferencesStore::new(None),
        )
    }

    #[cfg(test)]
    pub(super) fn with_preferences(
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
        preferences: crate::preferences::PreferencesStore,
    ) -> Result<Self> {
        Self::with_preference_paths(events, cancel, preferences, None)
    }

    pub(super) fn with_preference_paths(
        events: mpsc::Sender<Event>,
        cancel: CancellationToken,
        preferences: crate::preferences::PreferencesStore,
        preferences_path: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        let store = ProjectStore::create(None)?;
        let intruder =
            intruder_service::IntruderState::with_resource_pools(store.intruder_resource_pools()?);
        store.cancel_queries_with(cancel.clone());
        let project = ProjectKey {
            id: store.id,
            generation: 1,
        };
        let workspace = store.workspace_arc()?;
        let mut ws_repeater = websocket_repeater::WebSocketRepeaterService::new(project);
        ws_repeater.restore_workspace_tabs(project, workspace.websocket_tabs.clone());
        if let Ok(config) = Self::ws_repeater_send_config(&preferences, &workspace, Arc::from([])) {
            ws_repeater.set_connect_config(config);
        }
        let sessions = crate::sessions::SessionRuntime::from_workspace(project, &workspace);
        let (session_trace_tx, session_trace_rx) = mpsc::channel(128);
        sessions.set_trace_sender(Some(session_trace_tx));
        let (server_cert_tx, server_cert_rx) = mpsc::channel(128);
        let (tls_failure_tx, tls_failure_rx) = mpsc::channel(64);
        if let Ok(upstream_tls) = preferences.effective_upstream_tls(&workspace.upstream_tls)
            && let Ok(upstream_route) = workspace
                .upstream_proxy
                .to_route_with_tls(upstream_tls.clone())
        {
            sessions.set_proxy_send_config(SendConfig {
                upstream_route,
                upstream_tls,
                upstream_tls_options: workspace.upstream_tls.to_proxy_tls_options(),
                ..SendConfig::default()
            });
        }
        let (proxy_sender, proxy_events) = mpsc::channel(PROXY_EVENT_CAPACITY);
        let plugin_registry = crate::plugins::PluginRegistryStore::from_env_or_user_preferences(
            preferences_path.as_deref(),
        );
        let plugin_prefs = if std::env::var_os("MIO_PLUGIN_PREFERENCES_PATH")
            .is_some_and(|value| !value.is_empty())
        {
            crate::plugins::PluginPreferencesStore::from_env_or_memory()
        } else if let Some(user_pref_path) = preferences_path {
            crate::plugins::PluginPreferencesStore::new(Some(
                user_pref_path.with_file_name("plugin-preferences.json"),
            ))
        } else {
            crate::plugins::PluginPreferencesStore::new(None)
        };
        Ok(Self {
            preferences,
            project,
            store,
            events,
            awaiting_event_slot: false,
            cancel,
            proxy_sender,
            proxy_events,
            proxy_notifications: Mutex::new(VecDeque::new()),
            proxy: None,
            primary_proxy_id: None,
            ca: None,
            intercept_requests: false,
            intercept_responses: false,
            pending: HashMap::new(),
            pending_websockets: HashMap::new(),
            sends: HashMap::new(),
            jobs: JoinSet::new(),
            repeater_notifications: VecDeque::new(),
            history_dirty: false,
            websocket_history_dirty: false,
            recovery: VecDeque::new(),
            websocket_recovery: VecDeque::new(),
            websocket_query: websocket_query::QueryState::default(),
            http_query: http_query::QueryState::default(),
            storage_error: None,
            quiesced: false,
            extra_proxies: Vec::new(),
            annotated_flows: Arc::new(RwLock::new(HashSet::new())),
            compiled_client_certs: Arc::from([]),
            provenance: ProvenanceTracker::new(),
            ipc_repeater_revisions: HashSet::new(),
            ai_ipc_rx: None,
            ai_ipc_server: None,
            ipc_job_cancel: CancellationToken::new(),
            ipc_jobs: JoinSet::new(),
            sessions,
            session_trace_rx,
            server_cert_tx,
            server_cert_rx,
            observed_cert_keys: HashSet::new(),
            tls_failure_tx,
            tls_failure_rx,
            last_scheduled_fire: HashMap::new(),
            extensions_disabled: false,
            plugins: plugin_service::PluginState::with_stores(plugin_prefs, plugin_registry),
            ext_archive: ext_service::ExtState::default(),
            intruder,
            ws_repeater,
            sequencer: sequencer_service::SequencerService::default(),
            audit: audit_service::AuditService::default(),
            collaborator: oast_service::CollaboratorService::default(),
        })
    }

    pub(super) async fn event_permit(&mut self) -> Option<mpsc::OwnedPermit<Event>> {
        // Notices discovered during this wait follow the event that already owns delivery.
        self.awaiting_event_slot = true;
        let permit = loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break None,
                result = proxy_completion(&mut self.proxy) => self.proxy_finished(result),
                result = extra_proxy_completion(&mut self.extra_proxies) => self.proxy_finished(result),
                Some(result) = self.jobs.join_next(), if !self.jobs.is_empty() => self.repeater_completed(result).await,
                Some(done) = self.ipc_jobs.join_next(), if !self.ipc_jobs.is_empty() => self.complete_ipc_repeater(done),
                signal = self.intruder.next_signal(&self.events, true) => {
                    if let Err(error) = self.intruder_signal(signal).await {
                        self.queue_intruder_event(Event::Error {
                            project: Some(self.project),
                            operation: "intruder",
                            message: error.to_string(),
                        });
                    }
                }
                signal = self.ext_archive.next_signal(&self.events, true) => self.ext_signal(signal).await,
                signal = self.websocket_query.next_signal(&self.events, true) => self.websocket_query_signal(signal),
                signal = self.http_query.next_signal(&self.events, true) => self.http_query_signal(signal),
                () = self.ws_repeater.reclaim_signal(), if self.ws_repeater.has_in_flight_jobs() => {}
                () = self.sequencer.reclaim_signal(), if self.sequencer.has_in_flight_jobs() => {}
                () = self.audit.reclaim_signal(), if self.audit.has_in_flight_jobs() => {}
                () = self.collaborator.reclaim_signal(), if self.collaborator.has_in_flight_jobs() => {}
                result = self.events.clone().reserve_owned() => match result {
                    Ok(permit) => break Some(permit),
                    Err(_) => {
                        self.cancel.cancel();
                        break None;
                    }
                },
            }
        };
        self.awaiting_event_slot = false;
        permit
    }

    pub(super) async fn emit(&mut self, event: Event) {
        self.flush_proxy_notifications().await;
        self.flush_repeater_notifications().await;
        if let Some(permit) = self.event_permit().await {
            permit.send(event);
        }
    }

    pub(super) fn queue_proxy_notification(&self, event: Event) {
        let mut pending = self
            .proxy_notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if Self::coalesce_history_notification(&pending, &event) {
            return;
        }
        assert!(
            pending.len() < MAX_PROXY_NOTIFICATIONS,
            "Proxy notifications exceeded their admitted producer budget"
        );
        pending.push_back(event);
    }

    pub(super) async fn flush_proxy_notifications(&mut self) {
        loop {
            if self
                .proxy_notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            {
                return;
            }
            let Some(permit) = self.event_permit().await else {
                return;
            };
            let event = self
                .proxy_notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .expect("only the service task consumes Proxy notifications");
            permit.send(event);
        }
    }

    pub(super) fn queue_repeater_event(&mut self, event: Event) {
        let event = if self.repeater_notifications.is_empty() && !self.awaiting_event_slot {
            match self.events.try_send(event) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.cancel.cancel();
                    return;
                }
                Err(mpsc::error::TrySendError::Full(event)) => event,
            }
        } else {
            event
        };
        if Self::coalesce_history_notification(&self.repeater_notifications, &event) {
            return;
        }
        assert!(
            self.repeater_notifications.len() < MAX_REPEATER_NOTIFICATIONS,
            "Repeater notifications exceeded their admitted producer budget"
        );
        self.repeater_notifications.push_back(event);
    }

    fn coalesce_history_notification(queue: &VecDeque<Event>, candidate: &Event) -> bool {
        match candidate {
            Event::HistoryChanged { project } => queue.iter().any(
                |queued| matches!(queued, Event::HistoryChanged { project: existing } if existing == project),
            ),
            Event::WebSocketHistoryChanged { project } => queue.iter().any(
                |queued| matches!(queued, Event::WebSocketHistoryChanged { project: existing } if existing == project),
            ),
            _ => false,
        }
    }

    pub(super) async fn flush_repeater_notifications(&mut self) {
        while !self.repeater_notifications.is_empty() {
            let Some(permit) = self.event_permit().await else {
                return;
            };
            permit.send(self.repeater_notifications.pop_front().unwrap());
        }
    }

    pub(super) async fn error(&mut self, operation: &'static str, error: impl std::fmt::Display) {
        self.emit(Event::Error {
            project: Some(self.project),
            operation,
            message: error.to_string(),
        })
        .await;
    }
}
