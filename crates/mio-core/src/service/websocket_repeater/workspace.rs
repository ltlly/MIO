use super::*;

impl WebSocketRepeaterService {
    pub(crate) fn new(project: ProjectKey) -> Self {
        let (peers_tx, peers_rx) = mpsc::channel(MAX_PEER_QUEUE);
        Self {
            project,
            connect_config: None,
            injectors: None,
            tabs: HashMap::new(),
            jobs: JoinSet::new(),
            job_cancels: HashMap::new(),
            peers_tx,
            peers_rx,
            pending_events: VecDeque::new(),
        }
    }

    pub(crate) fn drain_pending_events(
        &mut self,
        project: ProjectKey,
    ) -> Vec<WebSocketRepeaterEvent> {
        let mut current = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(event) = self.pending_events.pop_front() {
            if event.project() == project {
                current.push(event);
            } else {
                retained.push_back(event);
            }
        }
        self.pending_events = retained;
        current
    }

    pub(crate) fn drop_project(&mut self, project: ProjectKey) {
        self.tabs.remove(&project);
        self.cancel_jobs(project);
        self.pending_events
            .retain(|event| event.project() != project);
        let mut buffered = Vec::new();
        while let Ok(update) = self.peers_rx.try_recv() {
            if update.project != project {
                buffered.push(update);
            }
        }
        for update in buffered {
            let events = self.accept_peer(update);
            self.pending_events.extend(events);
        }
    }

    pub(crate) async fn quiesce_project(&mut self, project: ProjectKey) -> Result<()> {
        while let Ok(update) = self.peers_rx.try_recv() {
            let events = self.accept_peer(update);
            self.pending_events.extend(events);
        }
        self.cancel_jobs(project);
        let mut closing_tabs = Vec::new();
        let mut closed_tab_ids = Vec::new();
        if let Some(tabs) = self.tabs.get_mut(&project) {
            for tab in tabs {
                tab.connect_cancel.cancel();
                tab.pending_session_id = None;
                if let Some(active) = tab.active.take() {
                    closed_tab_ids.push(tab.metadata.id);
                    closing_tabs.push(TabState {
                        metadata: tab.metadata.clone(),
                        detached_from_source: tab.detached_from_source,
                        pending_session_id: None,
                        connect_cancel: CancellationToken::new(),
                        active: Some(active),
                    });
                }
            }
        }
        let result = self.stop_tab_states(closing_tabs).await;
        while let Ok(update) = self.peers_rx.try_recv() {
            if update.project != project {
                let events = self.accept_peer(update);
                self.pending_events.extend(events);
            } else if let PeerPayload::Frame(_) = update.payload {
                let events = self.accept_peer(update);
                self.pending_events.extend(events);
            }
        }
        for tab_id in closed_tab_ids {
            self.pending_events
                .push_back(self.session_closed(project, tab_id, None));
        }
        result
    }

    #[cfg(test)]
    pub(crate) async fn stop_project(&mut self, project: ProjectKey) -> Result<()> {
        let result = self.quiesce_project(project).await;
        self.drop_project(project);
        result
    }

    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        let tabs = self.tabs.drain().flat_map(|(_, tabs)| tabs).collect();
        self.cancel_all_jobs();
        let stop_result = self.stop_tab_states(tabs).await;
        self.jobs.abort_all();
        while let Some(output) = self.jobs.join_next().await {
            let _ = self.accept_job(output);
        }
        self.pending_events.clear();
        stop_result
    }

    pub(crate) fn set_project(&mut self, project: ProjectKey) {
        self.project = project;
    }

    pub(crate) fn set_connect_config(&mut self, config: SendConfig) {
        self.connect_config = Some(config);
    }

    pub(crate) fn set_injectors(&mut self, injectors: Option<WebSocketInjectorRegistry>) {
        self.injectors = injectors.map(|injectors| Arc::new(injectors) as _);
    }

    #[cfg(test)]
    pub(super) fn set_test_injector(&mut self, injector: Option<Arc<dyn ProxyWebSocketInjector>>) {
        self.injectors = injector;
    }

    pub(crate) fn restore_workspace_tabs(
        &mut self,
        project: ProjectKey,
        tabs: Vec<WebSocketRepeaterTab>,
    ) {
        self.tabs.insert(
            project,
            tabs.into_iter()
                .map(|metadata| TabState {
                    metadata,
                    detached_from_source: true,
                    pending_session_id: None,
                    connect_cancel: CancellationToken::new(),
                    active: None,
                })
                .collect(),
        );
    }

    pub(crate) fn sync_workspace_tabs(
        &mut self,
        project: ProjectKey,
        tabs: &[WebSocketRepeaterTab],
    ) {
        let mut existing: HashMap<TabId, TabState> = self
            .tabs
            .remove(&project)
            .unwrap_or_default()
            .into_iter()
            .map(|tab| (tab.metadata.id, tab))
            .collect();
        let mut synced = Vec::with_capacity(tabs.len());
        for metadata in tabs {
            if metadata.closed {
                if let Some(mut removed) = existing.remove(&metadata.id) {
                    self.cleanup_removed_tab(project, &mut removed);
                }
                synced.push(TabState {
                    metadata: metadata.clone(),
                    detached_from_source: true,
                    pending_session_id: None,
                    connect_cancel: CancellationToken::new(),
                    active: None,
                });
                continue;
            }
            if let Some(mut state) = existing.remove(&metadata.id) {
                let source_changed = state.metadata.source_flow_id != metadata.source_flow_id;
                let prev_history = std::mem::take(&mut state.metadata.history);
                let prev_revision = state.metadata.draft_revision;
                let prev_direction = state.metadata.direction;
                let prev_kind = state.metadata.draft_kind;
                let prev_payload = std::mem::take(&mut state.metadata.draft_payload);
                state.metadata = metadata.clone();
                if metadata.draft_revision < prev_revision {
                    state.metadata.draft_revision = prev_revision;
                    state.metadata.direction = prev_direction;
                    state.metadata.draft_kind = prev_kind;
                    state.metadata.draft_payload = prev_payload;
                }
                for item in prev_history {
                    if !state
                        .metadata
                        .history
                        .iter()
                        .any(|existing| existing.id == item.id)
                    {
                        state.metadata.push_history(item);
                    }
                }
                if source_changed {
                    state.detached_from_source = false;
                }
                synced.push(state);
            } else {
                synced.push(TabState {
                    metadata: metadata.clone(),
                    detached_from_source: false,
                    pending_session_id: None,
                    connect_cancel: CancellationToken::new(),
                    active: None,
                });
            }
        }
        for (_, mut removed) in existing {
            self.cleanup_removed_tab(project, &mut removed);
        }
        self.tabs.insert(project, synced);
    }

    fn cleanup_removed_tab(&mut self, project: ProjectKey, tab: &mut TabState) {
        let tab_id = tab.metadata.id;
        if let Some(active) = tab.active.take() {
            active.cancel.cancel();
            let context = JobContext {
                id: Uuid::new_v4(),
                project,
                tab_id,
                operation_id: Uuid::nil(),
                cancel: active.cancel.clone(),
            };
            self.spawn_disconnect_job(context, active.session, active.reader);
        }
        tab.connect_cancel.cancel();
        tab.pending_session_id = None;
    }

    pub(crate) fn next_tab_title(&self, project: ProjectKey) -> String {
        let existing = self.tabs.get(&project).map(Vec::as_slice).unwrap_or(&[]);
        let mut next = 1usize;
        loop {
            let candidate = format!("WS {next}");
            if !existing.iter().any(|tab| tab.metadata.title == candidate) {
                return candidate;
            }
            next = next.saturating_add(1);
        }
    }

    pub(crate) fn cancel_all(&mut self) {
        self.cancel_all_jobs();
        for tabs in self.tabs.values_mut() {
            for tab in tabs {
                tab.connect_cancel.cancel();
                tab.pending_session_id = None;
                if let Some(active) = &tab.active {
                    active.cancel.cancel();
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn workspace_tabs(&self, project: ProjectKey) -> Vec<WebSocketRepeaterTab> {
        self.tabs
            .get(&project)
            .map(|tabs| tabs.iter().map(|tab| tab.metadata.clone()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn tab_mode(
        &self,
        project: ProjectKey,
        tab_id: TabId,
    ) -> WebSocketRepeaterConnectionMode {
        self.tab(project, tab_id)
            .map(|tab| self.tab_mode_state(tab))
            .unwrap_or_default()
    }

    pub(crate) fn has_resources(&self) -> bool {
        !self.jobs.is_empty()
            || self
                .tabs
                .values()
                .flat_map(|tabs| tabs.iter())
                .any(|tab| tab.active.is_some() || tab.pending_session_id.is_some())
    }

    pub(crate) fn has_in_flight_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub(crate) async fn handle(
        &mut self,
        command: WebSocketRepeaterCommand,
    ) -> Option<WebSocketRepeaterEvent> {
        if command.project() != self.project {
            return Some(command.rejected("项目已切换，已拒绝旧操作"));
        }
        match command {
            WebSocketRepeaterCommand::Open {
                project,
                operation_id,
                title,
                url,
                handshake_headers,
                source_flow_id,
                source_message_id,
                source_direction,
                source_kind,
                source_payload,
            } => Some(self.open(
                project,
                operation_id,
                title,
                url,
                handshake_headers,
                source_flow_id,
                source_message_id,
                source_direction,
                source_kind,
                source_payload,
            )),
            WebSocketRepeaterCommand::OpenFromHistory {
                project,
                operation_id,
                ..
            } => Some(WebSocketRepeaterEvent::Opened {
                project,
                operation_id,
                result: Err("历史 WebSocket 消息打开应由项目服务解析源握手后分派".into()),
                mode: WebSocketRepeaterConnectionMode::default(),
            }),
            WebSocketRepeaterCommand::Connect {
                project,
                operation_id,
                tab_id,
                url,
                handshake_headers,
                source_flow_id,
                cancel,
            } => self.connect(
                project,
                operation_id,
                tab_id,
                url,
                handshake_headers,
                source_flow_id,
                cancel,
            ),
            WebSocketRepeaterCommand::Send {
                project,
                operation_id,
                tab_id,
                url: _,
                source_flow_id: _,
                direction,
                kind,
                payload,
                draft_revision,
                cancel,
            } => self.send(
                project,
                operation_id,
                tab_id,
                direction,
                kind,
                payload,
                draft_revision,
                cancel,
            ),
            WebSocketRepeaterCommand::Disconnect {
                project,
                operation_id,
                tab_id,
            } => self.disconnect(project, operation_id, tab_id),
            WebSocketRepeaterCommand::Close { project, tab_id } => {
                Some(self.close(project, tab_id))
            }
        }
    }
}
