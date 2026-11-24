use super::*;

impl WebSocketRepeaterService {
    pub(super) fn tab(&self, project: ProjectKey, tab_id: TabId) -> Option<&TabState> {
        self.tabs
            .get(&project)?
            .iter()
            .find(|tab| !tab.metadata.closed && tab.metadata.id == tab_id)
    }

    pub(super) fn tab_mut(&mut self, project: ProjectKey, tab_id: TabId) -> Option<&mut TabState> {
        self.tabs
            .get_mut(&project)?
            .iter_mut()
            .find(|tab| !tab.metadata.closed && tab.metadata.id == tab_id)
    }

    pub(super) fn source_is_alive(&self, flow_id: Option<FlowId>) -> bool {
        flow_id.is_some_and(|flow_id| {
            self.injectors
                .as_ref()
                .is_some_and(|injectors| injectors.contains(flow_id))
        })
    }

    pub(super) fn tab_mode_state(&self, tab: &TabState) -> WebSocketRepeaterConnectionMode {
        if let Some(active) = &tab.active {
            WebSocketRepeaterConnectionMode::Standalone {
                flow_id: active.session_id,
            }
        } else if !tab.detached_from_source && self.source_is_alive(tab.metadata.source_flow_id) {
            WebSocketRepeaterConnectionMode::ProxyShared {
                flow_id: tab
                    .metadata
                    .source_flow_id
                    .expect("source flow id exists when proxy shared"),
            }
        } else {
            WebSocketRepeaterConnectionMode::Disconnected
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn open(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        title: Option<String>,
        url: String,
        handshake_headers: Vec<mio_plugin_api::Header>,
        source_flow_id: Option<FlowId>,
        source_message_id: Option<Uuid>,
        source_direction: WebSocketDirection,
        source_kind: WebSocketKind,
        source_payload: Vec<u8>,
    ) -> WebSocketRepeaterEvent {
        let alive = self.source_is_alive(source_flow_id);
        let tabs = self.tabs.entry(project).or_default();
        let result = (|| {
            anyhow::ensure!(
                tabs.len() < MAX_WEBSOCKET_REPEATER_TABS,
                "WebSocket Repeater 标签数量达到 {} 上限",
                MAX_WEBSOCKET_REPEATER_TABS
            );
            let tab = WebSocketRepeaterTab {
                id: Uuid::new_v4(),
                title: title.unwrap_or_else(|| "WebSocket Repeater".into()),
                group: None,
                url,
                handshake_headers,
                source_flow_id,
                source_message_id,
                direction: source_direction,
                draft_revision: 1,
                draft_payload: source_payload,
                draft_kind: source_kind,
                history: Vec::new(),
                closed: false,
            };
            tab.validate()?;
            Ok(tab)
        })();
        match result {
            Ok(metadata) => {
                let mode = if alive {
                    WebSocketRepeaterConnectionMode::ProxyShared {
                        flow_id: metadata
                            .source_flow_id
                            .expect("source flow id exists when proxy shared"),
                    }
                } else {
                    WebSocketRepeaterConnectionMode::Disconnected
                };
                tabs.push(TabState {
                    metadata: metadata.clone(),
                    detached_from_source: false,
                    pending_session_id: None,
                    connect_cancel: CancellationToken::new(),
                    active: None,
                });
                WebSocketRepeaterEvent::Opened {
                    project,
                    operation_id,
                    result: Ok(metadata),
                    mode,
                }
            }
            Err(error) => WebSocketRepeaterEvent::Opened {
                project,
                operation_id,
                result: Err(error.to_string()),
                mode: WebSocketRepeaterConnectionMode::default(),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn connect(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        url: String,
        handshake_headers: Vec<mio_plugin_api::Header>,
        source_flow_id: Option<FlowId>,
        cancel: CancellationToken,
    ) -> Option<WebSocketRepeaterEvent> {
        let injectors = self.injectors.clone();
        let connect_config = self.connect_config.clone();
        let Some(tab) = self.tab_mut(project, tab_id) else {
            return Some(rejected_connected(
                project,
                operation_id,
                tab_id,
                "标签不存在",
            ));
        };
        if tab.pending_session_id.is_some() {
            return Some(rejected_connected(
                project,
                operation_id,
                tab_id,
                "已有连接任务正在执行",
            ));
        }
        if !url.is_empty() {
            tab.metadata.url = url;
        }
        if !handshake_headers.is_empty() {
            tab.metadata.handshake_headers = handshake_headers;
        }
        if source_flow_id.is_some() {
            tab.metadata.source_flow_id = source_flow_id;
        }
        if let Err(error) = tab.metadata.validate() {
            return Some(rejected_connected(
                project,
                operation_id,
                tab_id,
                &error.to_string(),
            ));
        }
        let source_alive = tab.metadata.source_flow_id.is_some_and(|flow_id| {
            injectors
                .as_ref()
                .is_some_and(|registry| registry.contains(flow_id))
        });
        if tab.active.is_none() && !tab.detached_from_source && source_alive {
            let mode = WebSocketRepeaterConnectionMode::ProxyShared {
                flow_id: tab
                    .metadata
                    .source_flow_id
                    .expect("source flow id exists when proxy shared"),
            };
            return Some(WebSocketRepeaterEvent::Connected {
                project,
                operation_id,
                tab_id,
                mode,
                response: None,
                result: Ok(()),
            });
        }
        let Some(config) = connect_config else {
            return Some(rejected_connected(
                project,
                operation_id,
                tab_id,
                "WebSocket 独立会话连接配置不可用",
            ));
        };
        let session_id = Uuid::new_v4();
        tab.pending_session_id = Some(session_id);
        tab.detached_from_source = true;
        tab.connect_cancel = cancel.child_token();
        let context = JobContext {
            id: Uuid::new_v4(),
            project,
            tab_id,
            operation_id,
            cancel: tab.connect_cancel.clone(),
        };
        let request = tab.metadata.clone();
        let peers = self.peers_tx.clone();
        let job_context = context.clone();
        let job_cancel = context.cancel.clone();
        self.spawn_job_with_session(context, Some(session_id), async move {
            match mio_proxy::connect_websocket_session(
                &request.url,
                &request.handshake_headers,
                config,
                job_cancel,
            )
            .await
            {
                Ok(mut session) => {
                    let flow_id = session.flow_id;
                    let receiver = session.take_receiver();
                    let cancel = session.cancellation_token();
                    let session = Arc::new(AsyncMutex::new(session));
                    let reader = tokio::spawn(read_standalone_session(
                        receiver, cancel, peers, project, tab_id, session_id, flow_id,
                    ));
                    let response = Box::new(session.lock().await.response.clone());
                    JobOutput::Connected {
                        context: job_context,
                        session_id,
                        session,
                        reader,
                        response: Some(response),
                    }
                }
                Err(error) => JobOutput::ConnectFailed {
                    context: job_context,
                    error: error.to_string(),
                },
            }
        });
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn send(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        draft_revision: u64,
        cancel: CancellationToken,
    ) -> Option<WebSocketRepeaterEvent> {
        let Some(tab) = self.tab_mut(project, tab_id) else {
            return Some(rejected_sent(
                project,
                operation_id,
                tab_id,
                draft_revision,
                "标签不存在",
            ));
        };
        if draft_revision < tab.metadata.draft_revision {
            return Some(rejected_sent(
                project,
                operation_id,
                tab_id,
                draft_revision,
                "发送草稿 revision 已过期",
            ));
        }
        tab.metadata.draft_revision = draft_revision;
        tab.metadata.direction = direction;
        tab.metadata.draft_kind = kind;
        tab.metadata.draft_payload = payload.clone();
        let Some(active) = tab.active.as_ref() else {
            let Some(flow_id) = tab.metadata.source_flow_id else {
                return Some(rejected_sent(
                    project,
                    operation_id,
                    tab_id,
                    draft_revision,
                    "WebSocket 连接未建立；请先点击连接",
                ));
            };
            if tab.detached_from_source || !self.source_is_alive(Some(flow_id)) {
                return Some(rejected_sent(
                    project,
                    operation_id,
                    tab_id,
                    draft_revision,
                    "源 Proxy WebSocket 连接已断开；请点击连接建立独立会话",
                ));
            }
            let Some(injectors) = self.injectors.clone() else {
                return Some(rejected_sent(
                    project,
                    operation_id,
                    tab_id,
                    draft_revision,
                    "源 Proxy 连接注入器不可用",
                ));
            };
            let context = JobContext {
                id: Uuid::new_v4(),
                project,
                tab_id,
                operation_id,
                cancel,
            };
            let job_context = context.clone();
            let job_cancel = context.cancel.clone();
            let job = async move {
                let result = tokio::select! {
                    biased;
                    _ = job_cancel.cancelled() => Err(mio_proxy::ProxyError::Cancelled),
                    res = injectors.inject_directed(flow_id, direction, kind, payload, SEND_TIMEOUT) => res,
                };
                JobOutput::Sent {
                    context: job_context,
                    draft_revision,
                    result: result
                        .map(|message| WebSocketRepeaterHistoryItem::from_message(&message))
                        .map_err(|error| error.to_string()),
                }
            };
            self.spawn_job(context, job);
            return None;
        };
        if direction != WebSocketDirection::ClientToServer {
            return Some(rejected_sent(
                project,
                operation_id,
                tab_id,
                draft_revision,
                "独立重连会话作为客户端连接源站，仅支持 ClientToServer",
            ));
        }
        let context = JobContext {
            id: Uuid::new_v4(),
            project,
            tab_id,
            operation_id,
            cancel,
        };
        let job_context = context.clone();
        let session = active.session.clone();
        let send_cancel = context.cancel.clone();
        let job = async move {
            let result = session.lock().await.send(kind, payload, send_cancel).await;
            JobOutput::Sent {
                context: job_context,
                draft_revision,
                result: result
                    .map(|message| WebSocketRepeaterHistoryItem::from_message(&message))
                    .map_err(|error| error.to_string()),
            }
        };
        self.spawn_job(context, job);
        None
    }

    pub(super) fn disconnect(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
    ) -> Option<WebSocketRepeaterEvent> {
        let Some(tab) = self.tab_mut(project, tab_id) else {
            return Some(rejected_disconnected(
                project,
                operation_id,
                tab_id,
                "标签不存在",
            ));
        };
        if let Some(active) = tab.active.take() {
            let context = JobContext {
                id: Uuid::new_v4(),
                project,
                tab_id,
                operation_id,
                cancel: active.cancel.clone(),
            };
            self.spawn_disconnect_job(context, active.session, active.reader);
            return None;
        }
        if tab.pending_session_id.take().is_some() {
            tab.connect_cancel.cancel();
            tab.detached_from_source = true;
            return Some(WebSocketRepeaterEvent::Disconnected {
                project,
                operation_id,
                tab_id,
                result: Ok(()),
            });
        }
        if tab.metadata.source_flow_id.is_some() && !tab.detached_from_source {
            tab.detached_from_source = true;
            return Some(WebSocketRepeaterEvent::Disconnected {
                project,
                operation_id,
                tab_id,
                result: Ok(()),
            });
        }
        Some(WebSocketRepeaterEvent::Disconnected {
            project,
            operation_id,
            tab_id,
            result: Err("WebSocket Repeater 未连接".into()),
        })
    }

    pub(super) fn close(&mut self, project: ProjectKey, tab_id: TabId) -> WebSocketRepeaterEvent {
        let Some(project_tabs) = self.tabs.get_mut(&project) else {
            return WebSocketRepeaterEvent::Closed { project, tab_id };
        };
        let Some(index) = project_tabs
            .iter()
            .position(|tab| tab.metadata.id == tab_id)
        else {
            return WebSocketRepeaterEvent::Closed { project, tab_id };
        };
        let mut tab = project_tabs.remove(index);
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
        WebSocketRepeaterEvent::Closed { project, tab_id }
    }
}
