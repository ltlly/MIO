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

    #[cfg(test)]
    pub(crate) fn start_app_with_preferences_and_ipc(
        path: std::path::PathBuf,
        ipc_path: std::path::PathBuf,
    ) -> Result<(Self, Client)> {
        Self::start_inner(Some(path), Some((ipc_path, false, false)))
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
                        let mut bound_ipc_path = None;
                        if let Some((sock_path, strict_parent, must_succeed)) = ipc_spec {
                            let _enter = runtime.enter();
                            match ai_ipc::start_ipc_server(&sock_path, strict_parent, &state.cancel)
                            {
                                Ok((server, rx)) => {
                                    state.ai_ipc.server = Some(server);
                                    state.ai_ipc.rx = Some(rx);
                                    bound_ipc_path = Some(sock_path);
                                }
                                Err(error) if must_succeed => {
                                    let _ = ready.send(Err(error.to_string()));
                                    return Err(error);
                                }
                                Err(_) => {
                                    let fallback = ai_ipc::instance_ipc_socket_path_near(
                                        &sock_path,
                                        std::process::id(),
                                    );
                                    if let Ok((server, rx)) = ai_ipc::start_ipc_server(
                                        &fallback,
                                        strict_parent,
                                        &state.cancel,
                                    ) {
                                        state.ai_ipc.server = Some(server);
                                        state.ai_ipc.rx = Some(rx);
                                        bound_ipc_path = Some(fallback);
                                    }
                                }
                            }
                        }
                        let _ = ready.send(Ok::<Option<std::path::PathBuf>, String>(bound_ipc_path));
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
            Ok(Ok(ipc_socket_path)) => Ok((
                Self {
                    cancel: cancel.clone(),
                    worker: Some(worker),
                },
                Client {
                    commands,
                    events: event_receiver,
                    shutdown: cancel,
                    ipc_socket_path,
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
    let futures: Vec<_> = proxies
        .iter_mut()
        .filter(|(_, proxy)| !proxy.is_joined())
        .map(|(_, proxy)| Box::pin(proxy.join()))
        .collect();
    if futures.is_empty() {
        return std::future::pending().await;
    }
    futures_util::future::select_all(futures).await.0
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
            intruder_service::IntruderState::with_resource_pools(store.resource_pools().list()?);
        store.cancel_queries_with(cancel.clone());
        let project = ProjectKey {
            id: store.id,
            generation: 1,
        };
        let workspace = store.metadata().workspace_arc()?;
        let mut ws_repeater = websocket_repeater::WebSocketRepeaterService::new(project);
        ws_repeater.restore_workspace_tabs(project, workspace.websocket_tabs.clone());
        if let Ok(config) = Self::ws_repeater_send_config(&preferences, &workspace, Arc::from([])) {
            ws_repeater.set_connect_config(config);
        }
        let sessions = SessionServiceState::from_workspace(project, &workspace);
        if let Ok(upstream_tls) = preferences.effective_upstream_tls(&workspace.upstream_tls)
            && let Ok(upstream_route) = workspace
                .upstream_proxy
                .to_route_with_tls(upstream_tls.clone())
        {
            let mut send_cfg = SendConfig::default();
            send_cfg.network.upstream_route = upstream_route;
            send_cfg.tls.trust = upstream_tls;
            send_cfg.tls.options = workspace.upstream_tls.to_proxy_tls_options();
            sessions.runtime.set_proxy_send_config(send_cfg);
        }
        let plugin_registry = crate::plugins::PluginRegistryStore::from_env_or_user_preferences(
            preferences_path.as_deref(),
        );
        let plugin_prefs = if std::env::var_os("MIO_PLUGIN_PREFERENCES_PATH")
            .is_some_and(|value| !value.is_empty())
        {
            crate::plugins::PluginPreferencesStore::from_env_or_memory()
        } else if let Some(user_pref_path) = preferences_path.as_deref() {
            crate::plugins::PluginPreferencesStore::new(Some(
                user_pref_path.with_file_name("plugin-preferences.json"),
            ))
        } else {
            crate::plugins::PluginPreferencesStore::new(None)
        };
        Ok(Self {
            preferences,
            preferences_path,
            user_level_connections_cache: UserLevelConnectionsCache::default(),
            project,
            project_lock: None,
            store,
            storage: StorageState::new(),
            events,
            awaiting_event_slot: false,
            cancel,
            quiesced: false,
            automated_tasks_paused: false,
            extensions_disabled: false,
            proxy: ProxyRuntimeState::new(),
            repeater: RepeaterState::new(),
            queries: QueryWorkersState::default(),
            ai_ipc: AiIpcState::new(),
            tls: TlsRuntimeState::new(),
            sessions,
            last_scheduled_fire: HashMap::new(),
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
                result = proxy_completion(&mut self.proxy.primary) => self.proxy_finished(result),
                result = extra_proxy_completion(&mut self.proxy.extra) => self.proxy_finished(result),
                Some(result) = self.repeater.jobs.join_next(), if !self.repeater.jobs.is_empty() => self.repeater_completed(result).await,
                Some(done) = self.ai_ipc.jobs.join_next(), if !self.ai_ipc.jobs.is_empty() => self.complete_ipc_repeater(done),
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
                signal = self.queries.websocket.next_signal(&self.events, true) => self.websocket_query_signal(signal),
                signal = self.queries.http.next_signal(&self.events, true) => self.http_query_signal(signal),
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

    pub(super) fn queue_proxy_notification(&mut self, event: Event) {
        if Self::coalesce_history_notification(&self.proxy.notifications, &event) {
            return;
        }
        while self.proxy.notifications.len() >= MAX_PROXY_NOTIFICATIONS {
            self.proxy.notifications.pop_front();
        }
        self.proxy.notifications.push_back(event);
    }

    pub(super) async fn flush_proxy_notifications(&mut self) {
        while !self.proxy.notifications.is_empty() {
            let Some(permit) = self.event_permit().await else {
                return;
            };
            let event = self
                .proxy
                .notifications
                .pop_front()
                .expect("only the service task consumes Proxy notifications");
            permit.send(event);
        }
    }

    pub(super) fn queue_repeater_event(&mut self, event: Event) {
        let event = if self.repeater.notifications.is_empty() && !self.awaiting_event_slot {
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
        if Self::coalesce_history_notification(&self.repeater.notifications, &event) {
            return;
        }
        if self.repeater.notifications.len() >= MAX_REPEATER_NOTIFICATIONS {
            let drop_idx = self
                .repeater
                .notifications
                .iter()
                .position(|ev| {
                    matches!(
                        ev,
                        Event::HistoryChanged { .. }
                            | Event::WebSocketHistoryChanged { .. }
                            | Event::Audit(_)
                    )
                })
                .unwrap_or(0);
            self.repeater.notifications.remove(drop_idx);
        }
        self.repeater.notifications.push_back(event);
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
        while !self.repeater.notifications.is_empty() {
            let Some(permit) = self.event_permit().await else {
                return;
            };
            permit.send(self.repeater.notifications.pop_front().unwrap());
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
