use super::*;

impl WebSocketRepeaterCommand {
    pub(crate) fn project(&self) -> ProjectKey {
        match self {
            Self::Open { project, .. }
            | Self::OpenFromHistory { project, .. }
            | Self::Connect { project, .. }
            | Self::Send { project, .. }
            | Self::Disconnect { project, .. }
            | Self::Close { project, .. } => *project,
        }
    }

    pub(crate) fn rejected(self, message: &str) -> WebSocketRepeaterEvent {
        match self {
            Self::Open {
                project,
                operation_id,
                ..
            }
            | Self::OpenFromHistory {
                project,
                operation_id,
                ..
            } => WebSocketRepeaterEvent::Opened {
                project,
                operation_id,
                result: Err(message.to_owned()),
                mode: WebSocketRepeaterConnectionMode::default(),
            },
            Self::Connect {
                project,
                operation_id,
                tab_id,
                ..
            } => rejected_connected(project, operation_id, tab_id, message),
            Self::Send {
                project,
                operation_id,
                tab_id,
                draft_revision,
                ..
            } => rejected_sent(project, operation_id, tab_id, draft_revision, message),
            Self::Disconnect {
                project,
                operation_id,
                tab_id,
            } => rejected_disconnected(project, operation_id, tab_id, message),
            Self::Close { project, tab_id } => WebSocketRepeaterEvent::Closed { project, tab_id },
        }
    }
}

impl WebSocketRepeaterEvent {
    pub(crate) fn project(&self) -> ProjectKey {
        match self {
            Self::Opened { project, .. }
            | Self::Connected { project, .. }
            | Self::Sent { project, .. }
            | Self::Received { project, .. }
            | Self::SessionClosed { project, .. }
            | Self::Disconnected { project, .. }
            | Self::Closed { project, .. } => *project,
        }
    }
}

pub(super) fn rejected_connected(
    project: ProjectKey,
    operation_id: Uuid,
    tab_id: TabId,
    message: &str,
) -> WebSocketRepeaterEvent {
    WebSocketRepeaterEvent::Connected {
        project,
        operation_id,
        tab_id,
        mode: WebSocketRepeaterConnectionMode::default(),
        response: None,
        result: Err(message.to_owned()),
    }
}

pub(super) fn rejected_sent(
    project: ProjectKey,
    operation_id: Uuid,
    tab_id: TabId,
    draft_revision: u64,
    message: &str,
) -> WebSocketRepeaterEvent {
    WebSocketRepeaterEvent::Sent {
        project,
        operation_id,
        tab_id,
        draft_revision,
        mode: WebSocketRepeaterConnectionMode::default(),
        result: Err(message.to_owned()),
    }
}

pub(super) fn rejected_disconnected(
    project: ProjectKey,
    operation_id: Uuid,
    tab_id: TabId,
    message: &str,
) -> WebSocketRepeaterEvent {
    WebSocketRepeaterEvent::Disconnected {
        project,
        operation_id,
        tab_id,
        result: Err(message.to_owned()),
    }
}

fn normalize_handshake_url(url: &str) -> Result<String> {
    let Some((scheme, target)) = url.split_once("://") else {
        bail!("WebSocket 源流量 URL 缺少协议头：{url}");
    };
    if scheme.eq_ignore_ascii_case("ws") || scheme.eq_ignore_ascii_case("wss") {
        Ok(url.to_owned())
    } else if scheme.eq_ignore_ascii_case("http") {
        Ok(format!("ws://{target}"))
    } else if scheme.eq_ignore_ascii_case("https") {
        Ok(format!("wss://{target}"))
    } else {
        bail!("WebSocket 源流量 URL 协议不支持：{scheme}");
    }
}

fn filter_handshake_headers(headers: &[Header]) -> Vec<Header> {
    headers
        .iter()
        .filter(|header| {
            ![
                "upgrade",
                "connection",
                "sec-websocket-key",
                "sec-websocket-version",
                "sec-websocket-extensions",
                "content-length",
                "transfer-encoding",
            ]
            .iter()
            .any(|hop| header.name.eq_ignore_ascii_case(hop))
        })
        .cloned()
        .collect()
}

impl State {
    pub(crate) fn ws_repeater_send_config(
        preferences: &PreferencesStore,
        workspace: &Workspace,
        client_certificates: Arc<
            [(
                crate::ClientCertificateRule,
                mio_proxy::ParsedClientIdentity,
            )],
        >,
    ) -> Result<SendConfig> {
        let upstream_tls = preferences.effective_upstream_tls(&workspace.upstream_tls)?;
        let upstream_route = workspace
            .upstream_proxy
            .to_route_with_tls(upstream_tls.clone())?;
        let hostname_overrides = workspace.upstream_proxy.compiled_hostname_overrides()?;
        let timeout = Duration::from_secs(
            workspace
                .repeater_settings
                .request_timeout_secs
                .clamp(1, 3600),
        );
        Ok(SendConfig {
            network: mio_proxy::SendNetworkConfig {
                upstream_route,
                hostname_overrides,
            },
            tls: mio_proxy::UpstreamTlsConfig {
                trust: upstream_tls,
                client_certificates,
                ..Default::default()
            },
            limits: mio_proxy::SendLimits {
                timeout,
                ..Default::default()
            },
            ..SendConfig::default()
        })
    }

    pub(crate) fn sync_ws_repeater_workspace(&mut self, workspace: &Workspace) {
        self.ws_repeater
            .sync_workspace_tabs(self.project, &workspace.websocket_tabs);
        if let Ok(config) = Self::ws_repeater_send_config(
            &self.preferences,
            workspace,
            self.compiled_client_certs.clone(),
        ) {
            self.ws_repeater.set_connect_config(config);
        }
    }

    pub(crate) async fn sync_compiled_client_certs(&mut self, workspace: &Workspace) {
        let rules = workspace.client_certificates.clone();
        if rules.is_empty() {
            self.compiled_client_certs = Arc::from([]);
            return;
        }
        if let Ok(certs) = self.compile_client_certificates(&rules).await {
            self.compiled_client_certs = certs;
        }
    }

    pub(crate) async fn compile_client_certificates(
        &self,
        rules: &[crate::ClientCertificateRule],
    ) -> Result<
        Arc<
            [(
                crate::ClientCertificateRule,
                mio_proxy::ParsedClientIdentity,
            )],
        >,
    > {
        let enabled: Vec<_> = rules.iter().filter(|rule| rule.enabled).cloned().collect();
        if enabled.is_empty() {
            return Ok(Arc::from([]));
        }
        let parsed = tokio::task::spawn_blocking(move || {
            enabled
                .iter()
                .map(|rule| {
                    rule.parse_identity()
                        .map(|identity| (rule.clone(), identity))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .map_err(|error| anyhow::anyhow!("客户端证书解析任务异常结束：{error}"))??;
        Ok(parsed.into())
    }

    pub(crate) async fn flush_ws_repeater_events(&mut self) {
        for event in self.ws_repeater.drain_pending_events(self.project) {
            self.emit(Event::WebSocketRepeater(event)).await;
        }
    }

    fn refresh_ws_repeater_runtime_config(&mut self) -> Result<()> {
        let workspace = self.store.workspace()?;
        let config = Self::ws_repeater_send_config(
            &self.preferences,
            &workspace,
            self.compiled_client_certs.clone(),
        )?;
        self.ws_repeater.set_connect_config(config);
        self.ws_repeater.set_injectors(
            self.proxy
                .as_ref()
                .map(mio_proxy::ProxyHandle::websocket_injectors),
        );
        Ok(())
    }

    pub(crate) async fn websocket_repeater_command(&mut self, command: WebSocketRepeaterCommand) {
        if command.project() != self.project {
            self.emit(Event::WebSocketRepeater(
                command.rejected("项目已切换，已拒绝旧操作"),
            ))
            .await;
            return;
        }
        if self.quiesced && !matches!(command, WebSocketRepeaterCommand::Close { .. }) {
            self.emit(Event::WebSocketRepeater(
                command.rejected("正在保存或切换项目；已拒绝 WebSocket Repeater 操作"),
            ))
            .await;
            return;
        }
        let command = match command {
            WebSocketRepeaterCommand::OpenFromHistory {
                project,
                operation_id,
                message_id,
                use_original,
                payload_override,
            } => {
                let resolved = (|| -> Result<WebSocketRepeaterCommand> {
                    self.refresh_ws_repeater_runtime_config()?;
                    let (message, original_payload) = self.read_websocket_detail(message_id)?;
                    let flow = self.read_flow(message.flow_id)?;
                    let url = normalize_handshake_url(&flow.request.url)?;
                    let handshake_headers = filter_handshake_headers(&flow.request.headers);
                    let (source_kind, source_payload) =
                        if let Some(override_payload) = payload_override {
                            override_payload
                        } else if use_original && let Some(original) = original_payload {
                            (message.kind, original)
                        } else {
                            (message.kind, message.payload)
                        };
                    let title = Some(self.ws_repeater.next_tab_title(project));
                    Ok(WebSocketRepeaterCommand::Open {
                        project,
                        operation_id,
                        title,
                        url,
                        handshake_headers,
                        source_flow_id: Some(message.flow_id),
                        source_message_id: Some(message.id),
                        source_direction: message.direction,
                        source_kind,
                        source_payload,
                    })
                })();
                match resolved {
                    Ok(open) => open,
                    Err(error) => {
                        self.emit(Event::WebSocketRepeater(WebSocketRepeaterEvent::Opened {
                            project,
                            operation_id,
                            result: Err(error.to_string()),
                            mode: WebSocketRepeaterConnectionMode::default(),
                        }))
                        .await;
                        return;
                    }
                }
            }
            other => {
                if matches!(
                    other,
                    WebSocketRepeaterCommand::Open { .. }
                        | WebSocketRepeaterCommand::Connect { .. }
                ) && let Err(error) = self.refresh_ws_repeater_runtime_config()
                {
                    self.emit(Event::WebSocketRepeater(other.rejected(&error.to_string())))
                        .await;
                    return;
                }
                if matches!(other, WebSocketRepeaterCommand::Send { .. }) {
                    self.ws_repeater.set_injectors(
                        self.proxy
                            .as_ref()
                            .map(mio_proxy::ProxyHandle::websocket_injectors),
                    );
                }
                other
            }
        };
        if let Some(event) = self.ws_repeater.handle(command).await {
            self.emit(Event::WebSocketRepeater(event)).await;
        }
    }
}
