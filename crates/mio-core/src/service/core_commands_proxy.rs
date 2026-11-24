use std::path::{Path, PathBuf};
use super::*;

struct ProxyRuntimeNetConfig {
    upstream_tls: mio_proxy::UpstreamTls,
    upstream_tls_options: mio_proxy::UpstreamTlsOptions,
    eff_upstream: UpstreamProxySettings,
    upstream_route: mio_proxy::UpstreamRoute,
    upstream_route_resolver: mio_proxy::UpstreamRouteResolver,
    hostname_overrides: Arc<[(String, std::net::IpAddr)]>,
    request_timeout: Duration,
    streaming_timeout: Option<Duration>,
}

impl State {
    pub(super) async fn command_proxy_intercept(&mut self, command: Command) -> Result<()> {
        Box::pin(self.command_proxy_intercept_inner(command)).await
    }

    async fn command_proxy_intercept_inner(&mut self, command: Command) -> Result<()> {
        match command {
            Command::StartProxy { project, listen } => {
                self.start_proxy_with_override(project, listen, None)
                    .await?;
            }
            Command::StopProxy { project } => {
                self.check_project(project)?;
                self.queries.cancel_all();
                self.stop_proxy().await?;
            }
            Command::StopProxyListener {
                project,
                listener_id,
            } => {
                self.check_project(project)?;
                self.stop_single_proxy_listener(listener_id).await?;
            }
            Command::SetIntercept {
                project,
                requests,
                responses,
            } => {
                self.check_project(project)?;
                self.proxy.set_intercept(requests, responses);
                self.proxy_state().await;
            }
            Command::OpenConfiguredBrowser {
                project,
                url,
                session,
            } => {
                self.open_configured_browser(project, url, session).await?;
            }
            Command::Decide {
                project,
                flow_id,
                revision,
                decision,
            } => {
                self.check_project(project)?;
                let pending = self.proxy.pending.get(&flow_id).context("拦截已结束")?;
                ensure!(
                    pending.revision == revision && !pending.reply.is_closed(),
                    "拦截决定已过期；未发送旧修改"
                );
                ensure!(
                    matches!(
                        (&decision, pending.stage),
                        (Decision::Drop, _)
                            | (Decision::ForwardRequest(_), InterceptStage::Request)
                            | (Decision::ForwardResponse(_), InterceptStage::Response)
                    ),
                    "拦截阶段不匹配"
                );
                if pending.response_head_only
                    && let Decision::ForwardResponse(response) = &decision
                {
                    ensure!(
                        response.body.is_empty() && response.trailers.is_empty(),
                        "SSE 响应尚在流式接收，只能修改响应头；未丢弃正文修改或发送拦截决定"
                    );
                    ensure!(
                        (200..=999).contains(&response.status),
                        "SSE 最终响应不能改为临时响应、协议升级或无效状态码；拦截仍等待决定"
                    );
                }
                let pending = self.proxy.pending.remove(&flow_id).unwrap();
                let decision = match decision {
                    Decision::Drop => InterceptDecision::Drop,
                    Decision::ForwardRequest(request) => {
                        if request != pending.intercepted_request {
                            self.storage.provenance.record_request(
                                flow_id,
                                &pending.intercepted_request,
                                ProvenanceActor::ManualInterceptEdit,
                                &request,
                            );
                        }
                        InterceptDecision::ForwardRequest(request)
                    }
                    Decision::ForwardResponse(response) => {
                        if let Some(intercepted_resp) = &pending.intercepted_response
                            && intercepted_resp != &response
                        {
                            self.storage.provenance.record_response(
                                flow_id,
                                intercepted_resp,
                                ProvenanceActor::ManualInterceptEdit,
                                &response,
                            );
                        }
                        InterceptDecision::ForwardResponse(response)
                    }
                };
                ensure!(
                    pending.reply.send(decision).is_ok(),
                    "拦截已结束；决定未发送"
                );
                self.emit(Event::InterceptResolved { project, flow_id })
                    .await;
            }
            Command::DecideWebSocket {
                project,
                message_id,
                decision,
            } => {
                self.check_project(project)?;
                let pending = self
                    .proxy
                    .pending_websockets
                    .get(&message_id)
                    .context("WebSocket 拦截已结束")?;
                ensure!(
                    !pending.reply.is_closed(),
                    "WebSocket 拦截决定已过期；未发送旧修改"
                );
                if let WebSocketDecision::Forward(payload) = &decision {
                    ensure!(
                        payload.len() <= crate::http::MAX_BODY,
                        "WebSocket 消息超过 16 MiB 限额；修改仍保留，未转发"
                    );
                    if pending.kind == WebSocketKind::Text {
                        ensure!(
                            std::str::from_utf8(payload).is_ok(),
                            "WebSocket 文本消息必须是有效 UTF-8；修改仍保留，未转发"
                        );
                    }
                }
                let pending = self.proxy.pending_websockets.remove(&message_id).unwrap();
                let decision = match decision {
                    WebSocketDecision::Forward(payload) => {
                        mio_proxy::WebSocketInterceptDecision::Forward(payload)
                    }
                    WebSocketDecision::Drop => mio_proxy::WebSocketInterceptDecision::Drop,
                };
                ensure!(
                    pending.reply.send(decision).is_ok(),
                    "WebSocket 拦截已结束；决定未发送"
                );
                self.emit(Event::WebSocketInterceptResolved {
                    project,
                    message_id,
                })
                .await;
            }
            Command::SetLiveFlowAnnotated {
                project,
                flow_id,
                annotated,
            } => {
                self.check_project(project)?;
                {
                    let mut annotated_flows = self
                        .storage
                        .annotated_flows
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if annotated {
                        annotated_flows.insert(flow_id);
                    } else {
                        annotated_flows.remove(&flow_id);
                    }
                }
                self.emit(Event::LiveFlowAnnotated {
                    project,
                    flow_id,
                    annotated,
                })
                .await;
            }
            _ => unreachable!("routed by State::command"),
        }
        Ok(())
    }

    pub(crate) fn build_sessions_send_config(&self, workspace: &Workspace) -> Result<SendConfig> {
        let upstream_tls = self
            .preferences
            .effective_upstream_tls(&workspace.upstream_tls)?;
        let upstream_tls_options = workspace.upstream_tls.to_proxy_tls_options();
        let eff_upstream = workspace
            .upstream_proxy
            .effective_for_runtime(&self.user_level_connections_cache);
        let upstream_route = eff_upstream.to_route_with_tls(upstream_tls.clone())?;
        let hostname_overrides = eff_upstream.compiled_hostname_overrides()?;
        let request_timeout = Duration::from_secs(
            workspace
                .repeater_settings
                .request_timeout_secs
                .clamp(1, 3600),
        );
        let streaming_timeout = eff_upstream
            .effective_streaming_timeout()
            .or_else(|| workspace.repeater_settings.effective_streaming_timeout());
        let mut cfg = SendConfig::default();
        cfg.network.upstream_route = upstream_route;
        cfg.network.hostname_overrides = hostname_overrides;
        cfg.tls.trust = upstream_tls;
        cfg.tls.options = upstream_tls_options;
        cfg.tls.client_certificates = self.tls.compiled_client_certs.clone();
        cfg.limits.timeout = request_timeout;
        cfg.limits.streaming_timeout = streaming_timeout;
        cfg.http.strip_100_continue_headers = workspace.network_http.remove_100_continue_headers;
        cfg.http.default_to_http2 = workspace.network_http.default_to_http2;
        cfg.auth.platform_enabled = eff_upstream.platform_auth.enabled;
        cfg.auth.platform_rules = eff_upstream.compiled_platform_auth_rules();
        cfg.auth.upstream_rules = eff_upstream.compiled_upstream_auth_rules();
        Ok(cfg)
    }

    pub(crate) fn build_tool_send_config_for_url(
        &self,
        workspace: &Workspace,
        url: &str,
    ) -> Result<SendConfig> {
        let mut cfg = self.build_sessions_send_config(workspace)?;
        let eff_upstream = workspace
            .upstream_proxy
            .effective_for_runtime(&self.user_level_connections_cache);
        cfg.network.upstream_route =
            eff_upstream.route_for_url_with_tls(url, cfg.tls.trust.clone())?;
        Ok(cfg)
    }

    fn build_proxy_runtime_net_config(
        &mut self,
        workspace: &Workspace,
        sync_cache: bool,
    ) -> Result<ProxyRuntimeNetConfig> {
        let upstream_tls = self
            .preferences
            .effective_upstream_tls(&workspace.upstream_tls)?;
        let upstream_tls_options = workspace.upstream_tls.to_proxy_tls_options();
        let eff_upstream = workspace
            .upstream_proxy
            .effective_for_runtime(&self.user_level_connections_cache);
        if sync_cache {
            eff_upstream.sync_user_level_cache(&mut self.user_level_connections_cache);
        }
        let upstream_route = eff_upstream.to_route_with_tls(upstream_tls.clone())?;
        let upstream_route_resolver = eff_upstream.build_route_resolver(upstream_tls.clone())?;
        let hostname_overrides = eff_upstream.compiled_hostname_overrides()?;
        let request_timeout = Duration::from_secs(
            workspace
                .repeater_settings
                .request_timeout_secs
                .clamp(1, 3600),
        );
        let streaming_timeout = eff_upstream
            .effective_streaming_timeout()
            .or_else(|| workspace.repeater_settings.effective_streaming_timeout());
        Ok(ProxyRuntimeNetConfig {
            upstream_tls,
            upstream_tls_options,
            eff_upstream,
            upstream_route,
            upstream_route_resolver,
            hostname_overrides,
            request_timeout,
            streaming_timeout,
        })
    }

    fn listener_locks_dir(&self) -> Option<PathBuf> {
        listener_lock::resolve_listener_locks_dir(self.preferences_path.as_deref())
    }

    async fn start_proxy_listeners(
        &mut self,
        project: ProjectKey,
        workspace: &Workspace,
        net: &ProxyRuntimeNetConfig,
        desired_listeners: &[(ProxyListenerConfig, SocketAddr)],
        emit_state_on_failure: bool,
    ) -> Result<()> {
        let authority = self.authority().await?;
        let locks_dir = self.listener_locks_dir();
        let (plugin_flow_hook, plugin_ws_hook) = self.build_proxy_plugin_hooks();
        let shared_connections = Arc::new(tokio::sync::Semaphore::new(PROXY_MAX_CONNECTIONS));
        let shared_requests = Arc::new(tokio::sync::Semaphore::new(PROXY_MAX_CONNECTIONS));
        let shared_injectors = mio_proxy::WebSocketInjectorRegistry::new();
        let annotation_tracker =
            annotated_flows_tracker(self.storage.annotated_flows.clone(), self.cancel.clone());
        let drop_scope_fn = build_drop_out_of_scope_fn(workspace.scope.clone());
        let cert_callback = self.build_server_cert_callback(project);
        let tls_fail_callback = self.build_client_tls_failure_callback(project);
        let ssl_matcher =
            crate::http::build_ssl_pass_through_matcher(&workspace.ssl_pass_through_rules)?;
        let ws_transform = crate::http::build_websocket_transform_with_scope(
            workspace.ws_match_and_replace_rules.clone(),
            workspace.proxy_messages.ws_match_replace_in_scope_only,
            workspace.scope.clone(),
        )?;
        let intercept_filter = Some(crate::http::build_intercept_filter(
            workspace.proxy_interception.clone(),
            workspace.scope.clone(),
        )?);
        let websocket_intercept_filter = Some(crate::http::build_websocket_intercept_filter(
            workspace.proxy_websocket_interception,
            workspace.scope.clone(),
        )?);
        let flow_transform = crate::http::build_flow_transform_with_scope_and_provenance(
            workspace.match_and_replace_rules.clone(),
            workspace.proxy_messages,
            workspace.scope.clone(),
            Some(self.storage.provenance.clone()),
        )?;
        let mut handles: Vec<(Uuid, ProxyHandle)> = Vec::new();
        let mut locks: Vec<(Uuid, listener_lock::ActiveListenerLock)> = Vec::new();
        for (listener_cfg, bind) in desired_listeners {
            let acquired_lock = match listener_lock::acquire_listener_lock(locks_dir.as_deref(), *bind) {
                Ok(lock) => lock,
                Err(error) => {
                    for (_, started) in &handles {
                        started.request_stop();
                    }
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                    for (_, started) in &mut handles {
                        let _ = tokio::time::timeout_at(deadline, started.stop()).await;
                    }
                    drop(locks);
                    self.proxy.clear_handles();
                    if emit_state_on_failure {
                        self.ws_repeater.set_injectors(None);
                        self.proxy_state().await;
                    }
                    return Err(error);
                }
            };
            let mut config = ProxyConfig::new(authority.clone());
            config.network.listen = *bind;
            config.limits.max_connections = PROXY_MAX_CONNECTIONS;
            config.hooks.intercept_requests = self.proxy.intercept_requests;
            config.hooks.intercept_responses = self.proxy.intercept_responses;
            config.network.upstream_route = net.upstream_route.clone();
            config.network.upstream_route_resolver = Some(net.upstream_route_resolver.clone());
            config.tls.upstream.trust = net.upstream_tls.clone();
            config.tls.upstream.options = net.upstream_tls_options.clone();
            config.network.hostname_overrides = net.hostname_overrides.clone();
            config.limits.request_timeout = net.request_timeout;
            config.limits.streaming_timeout = net.streaming_timeout;
            apply_listener_and_workspace_options(
                &mut config,
                listener_cfg,
                workspace,
                &net.eff_upstream,
            );
            config.hooks.drop_out_of_scope_fn = drop_scope_fn.clone();
            config.tls.on_client_tls_failure = Some(tls_fail_callback.clone());
            config.tls.upstream.on_server_cert_observed = Some(cert_callback.clone());
            config.hooks.intercept_filter = intercept_filter.clone();
            config.hooks.websocket_intercept_filter = websocket_intercept_filter.clone();
            config.hooks.flow_transform = flow_transform.clone();
            config.hooks.ssl_pass_through = ssl_matcher.clone();
            config.hooks.websocket_transform = ws_transform.clone();
            config.tls.upstream.client_certificates = self.tls.compiled_client_certs.clone();
            config.shared.connections = Some(shared_connections.clone());
            config.shared.requests = Some(shared_requests.clone());
            config.shared.injectors = Some(shared_injectors.clone());
            config.hooks.annotation_checker = Some(annotation_tracker.clone());
            config.hooks.async_flow_hook = plugin_flow_hook.clone();
            config.hooks.async_websocket_hook = plugin_ws_hook.clone();
            match mio_proxy::start(config, project, self.proxy.sender.clone()).await {
                Ok(proxy) => {
                    handles.push((listener_cfg.id, proxy));
                    if let Some(lock) = acquired_lock {
                        locks.push((listener_cfg.id, lock));
                    }
                }
                Err(error) => {
                    drop(acquired_lock);
                    for (_, started) in &handles {
                        started.request_stop();
                    }
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                    for (_, started) in &mut handles {
                        let _ = tokio::time::timeout_at(deadline, started.stop()).await;
                    }
                    drop(locks);
                    self.proxy.clear_handles();
                    if emit_state_on_failure {
                        self.ws_repeater.set_injectors(None);
                        self.proxy_state().await;
                    }
                    return Err(format_proxy_start_error(*bind, error));
                }
            }
        }
        if !handles.is_empty() {
            let (first_id, first) = handles.remove(0);
            self.proxy.primary = Some(first);
            self.proxy.primary_id = Some(first_id);
        }
        self.proxy.extra.extend(handles);
        self.proxy.listener_locks.extend(locks);
        self.ws_repeater
            .set_injectors(self.proxy.websocket_injectors());
        self.proxy_state().await;
        Ok(())
    }

    pub(super) async fn start_proxy_with_override(
        &mut self,
        project: ProjectKey,
        listen: SocketAddr,
        explicit_listen_override: Option<SocketAddr>,
    ) -> Result<()> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(!self.quiesced, "正在保存并切换项目；监听未启动");
        ensure!(!self.proxy.is_running(), "Proxy 已在监听");
        if !self.preferences.is_isolated() {
            let _ = self.preferences.query();
        }
        let workspace = self.store.metadata().workspace()?;
        let client_certificates = self
            .compile_client_certificates(&workspace.client_certificates)
            .await?;
        self.tls.compiled_client_certs = client_certificates.clone();
        let net = self.build_proxy_runtime_net_config(&workspace, true)?;
        self.sessions.runtime.sync_workspace(self.project, &workspace);
        if let Ok(session_send_cfg) = self.build_sessions_send_config(&workspace) {
            self.sessions.runtime.set_proxy_send_config(session_send_cfg);
        }
        let listeners = workspace.effective_proxy_listeners();
        let has_explicit_listeners = !workspace.proxy_listeners.is_empty();
        let enabled_listeners: Vec<ProxyListenerConfig> = if let Some(explicit) =
            explicit_listen_override
        {
            let mut base = listeners
                .iter()
                .find(|l| l.enabled)
                .cloned()
                .unwrap_or_default();
            base.bind_address = explicit.to_string();
            vec![base]
        } else if has_explicit_listeners && listeners.iter().any(|l| l.enabled) {
            listeners
                .iter()
                .filter(|l| l.enabled)
                .cloned()
                .collect()
        } else {
            let mut base = listeners.first().cloned().unwrap_or_default();
            base.bind_address = listen.to_string();
            vec![base]
        };
        let desired_listeners: Vec<(ProxyListenerConfig, SocketAddr)> = enabled_listeners
            .into_iter()
            .map(|listener_cfg| {
                let bind: SocketAddr = listener_cfg.bind_address.trim().parse()?;
                Ok((listener_cfg, bind))
            })
            .collect::<Result<Vec<_>>>()?;
        Box::pin(self.start_proxy_listeners(
            project,
            &workspace,
            &net,
            &desired_listeners,
            false,
        ))
        .await
    }

    pub(super) async fn stop_single_proxy_listener(&mut self, listener_id: Uuid) -> Result<()> {
        let is_primary = self.proxy.primary.is_some()
            && self.proxy.primary_id.unwrap_or(Uuid::from_u128(1)) == listener_id;
        if is_primary && self.proxy.extra.is_empty() {
            let res = self.stop_proxy().await;
            self.emit(Event::ProxyListeners {
                project: self.project,
                listeners: Vec::new(),
            })
            .await;
            return res;
        }
        let extra_pos = if is_primary {
            None
        } else {
            self.proxy.extra.iter().position(|(id, _)| *id == listener_id)
        };
        ensure!(is_primary || extra_pos.is_some(), "未找到正在运行的指定 Proxy 监听器");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let (stop_res, joined, terminal) = {
            let handle = if is_primary {
                self.proxy.primary.as_mut().unwrap()
            } else {
                &mut self.proxy.extra[extra_pos.unwrap()].1
            };
            handle.request_stop();
            let stop_res = tokio::time::timeout_at(deadline, handle.stop())
                .await
                .unwrap_or(Err(mio_proxy::ProxyError::StopDeadline));
            let joined = handle.is_joined();
            let terminal = handle.drain_terminal_records();
            (stop_res, joined, terminal)
        };
        for capture in terminal {
            if let Err(error) = self.capture_without_waiting_for_ui(
                ProxyEvent::Captured {
                    project: self.project,
                    record: capture.record,
                    response_stream: capture.response_stream,
                    listener_port: capture.listener_port,
                    capture_metrics: capture.capture_metrics,
                },
                true,
            ) {
                self.emit(Event::Error {
                    project: Some(self.project),
                    operation: "capture",
                    message: error.to_string(),
                })
                .await;
            }
        }
        if !joined {
            return Err(stop_res
                .err()
                .unwrap_or(mio_proxy::ProxyError::StopDeadline)
                .into());
        }
        if is_primary {
            self.proxy.primary.take();
            self.proxy.listener_locks.remove(&listener_id);
            let (next_id, next_handle) = self.proxy.extra.remove(0);
            self.proxy.primary = Some(next_handle);
            self.proxy.primary_id = Some(next_id);
        } else if let Some(pos) = extra_pos {
            self.proxy.extra.remove(pos);
            self.proxy.listener_locks.remove(&listener_id);
        }
        self.ws_repeater
            .set_injectors(self.proxy.websocket_injectors());
        self.proxy_state().await;
        let listeners = self.proxy.active_listeners();
        self.emit(Event::ProxyListeners {
            project: self.project,
            listeners,
        })
        .await;
        stop_res.map_err(anyhow::Error::from)
    }

    pub(super) async fn reload_running_proxy_from_workspace(
        &mut self,
        prev_workspace: Option<&Workspace>,
        workspace: &Workspace,
    ) -> Result<()> {
        if !self.proxy.is_running() {
            return Ok(());
        }
        if !self.preferences.is_isolated() {
            let _ = self.preferences.query();
        }
        let net = self.build_proxy_runtime_net_config(workspace, false)?;

        let active_listeners = self.proxy.active_listeners();
        let current_addrs: Vec<SocketAddr> =
            active_listeners.iter().map(|(_, addr)| *addr).collect();
        let prev_listeners: Vec<(ProxyListenerConfig, SocketAddr)> = active_listeners
            .iter()
            .map(|(id, addr)| {
                let cfg = prev_workspace
                    .and_then(|ws| ws.proxy_listeners.iter().find(|l| l.id == *id))
                    .cloned()
                    .unwrap_or_else(|| ProxyListenerConfig {
                        id: *id,
                        enabled: true,
                        bind_address: addr.to_string(),
                        ..ProxyListenerConfig::default()
                    });
                (cfg, *addr)
            })
            .collect();

        let listeners = workspace.effective_proxy_listeners();
        let has_explicit_listeners = !workspace.proxy_listeners.is_empty();
        let desired_listeners: Vec<(ProxyListenerConfig, SocketAddr)> =
            if has_explicit_listeners {
                if !listeners.iter().any(|l| l.enabled) {
                    return self.stop_proxy().await;
                }
                listeners
                    .iter()
                    .filter(|l| l.enabled)
                    .map(|l| {
                        let mut addr: SocketAddr = l.bind_address.trim().parse()?;
                        if addr.port() == 0
                            && let Some((_, cur)) = active_listeners
                                .iter()
                                .find(|(id, cur)| *id == l.id && cur.ip() == addr.ip())
                        {
                            addr.set_port(cur.port());
                        }
                        Ok((l.clone(), addr))
                    })
                    .collect::<Result<Vec<_>>>()?
            } else if let Some(&first_cur) = current_addrs.first() {
                let mut base = listeners.first().cloned().unwrap_or_default();
                base.id = self.proxy.primary_id.unwrap_or(base.id);
                vec![(base, first_cur)]
            } else {
                return Ok(());
            };

        let locks_dir = self.listener_locks_dir();
        for (_, desired_bind) in &desired_listeners {
            if desired_bind.port() != 0
                && !current_addrs
                    .iter()
                    .any(|held| crate::proxy_listener_addrs_overlap(*held, *desired_bind))
            {
                let test_lock =
                    listener_lock::acquire_listener_lock(locks_dir.as_deref(), *desired_bind)?;
                drop(test_lock);
            }
        }

        self.stop_proxy().await?;
        let project = self.project;
        if let Err(error) = Box::pin(self.start_proxy_listeners(
            project,
            workspace,
            &net,
            &desired_listeners,
            prev_listeners.is_empty(),
        ))
        .await
        {
            if !prev_listeners.is_empty() {
                if let Some(prev_ws) = prev_workspace
                    && let Ok(prev_net) = self.build_proxy_runtime_net_config(prev_ws, false)
                {
                    let _ = Box::pin(self.start_proxy_listeners(
                        project,
                        prev_ws,
                        &prev_net,
                        &prev_listeners,
                        true,
                    ))
                    .await;
                } else {
                    let _ = Box::pin(self.start_proxy_listeners(
                        project,
                        workspace,
                        &net,
                        &prev_listeners,
                        true,
                    ))
                    .await;
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn build_server_cert_callback(
        &self,
        project: ProjectKey,
    ) -> mio_proxy::ServerCertObservedCallback {
        self.tls.build_server_cert_callback(project)
    }

    pub(super) fn build_client_tls_failure_callback(
        &self,
        project: ProjectKey,
    ) -> mio_proxy::ClientTlsFailureCallback {
        self.tls.build_client_tls_failure_callback(project)
    }

    pub(super) async fn start_default_proxy_with_port_fallback(
        &mut self,
        project: ProjectKey,
        default_addr: SocketAddr,
        max_offset: u16,
    ) -> Result<()> {
        match self
            .start_proxy_with_override(project, default_addr, Some(default_addr))
            .await
        {
            Ok(()) => Ok(()),
            Err(first_err) if listener_lock::is_addr_in_use_or_conflict(&first_err) => {
                let mut bound = None;
                for offset in 1..=max_offset {
                    let Some(port) = default_addr.port().checked_add(offset) else {
                        break;
                    };
                    let candidate = SocketAddr::new(default_addr.ip(), port);
                    match self
                        .start_proxy_with_override(project, candidate, Some(candidate))
                        .await
                    {
                        Ok(()) => {
                            bound = Some(candidate);
                            break;
                        }
                        Err(err) if listener_lock::is_addr_in_use_or_conflict(&err) => continue,
                        Err(err) => return Err(err),
                    }
                }
                let Some(candidate) = bound else {
                    return Err(first_err);
                };
                let mut workspace = self.store.metadata().workspace()?;
                workspace.proxy_listeners = vec![ProxyListenerConfig {
                    id: Uuid::from_u128(1),
                    enabled: true,
                    bind_address: candidate.to_string(),
                    ..ProxyListenerConfig::default()
                }];
                workspace.revision = workspace.revision.saturating_add(1);
                self.store.metadata_mut().save_workspace(&workspace)?;
                self.emit(Event::WorkspaceChanged {
                    project,
                    workspace,
                })
                .await;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    pub(super) async fn open_configured_browser(
        &mut self,
        project: ProjectKey,
        url: Option<String>,
        session: BrowserRequestSession,
    ) -> Result<()> {
        self.open_configured_browser_with(project, url, session, None, None)
            .await
    }

    pub(super) async fn open_configured_browser_with(
        &mut self,
        project: ProjectKey,
        url: Option<String>,
        session: BrowserRequestSession,
        identity_dir_override: Option<PathBuf>,
        executable_override: Option<PathBuf>,
    ) -> Result<()> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(!self.quiesced, "正在保存并切换项目；浏览器未启动");
        if let Some(target) = &url {
            let trimmed = target.trim();
            ensure!(
                trimmed.starts_with("http://")
                    || trimmed.starts_with("https://")
                    || trimmed == "about:blank",
                "配置浏览器仅支持打开 http://、https:// 或 about:blank 地址"
            );
        }
        let target_url = url
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "about:blank".into());
        self.sessions
            .runtime
            .stage_browser_navigation_session(&target_url, &session)?;
        let prev_intercept = (self.proxy.intercept_requests, self.proxy.intercept_responses);
        let intercept_disabled = prev_intercept.0 || prev_intercept.1;
        if intercept_disabled {
            self.proxy.set_intercept(false, false);
        }
        if self.proxy.primary.is_none() {
            let workspace = self.store.metadata().workspace()?;
            let start_result: Result<()> = if workspace.proxy_listeners.is_empty() {
                let default_addr: SocketAddr = DEFAULT_PROXY_LISTENER
                    .parse()
                    .context("Proxy 默认监听地址无效")?;
                self.start_default_proxy_with_port_fallback(project, default_addr, 100)
                    .await
            } else {
                let listeners = workspace.effective_proxy_listeners();
                let primary_addr = listeners
                    .iter()
                    .find(|l| l.enabled)
                    .or_else(|| listeners.first())
                    .map(|l| l.bind_address.trim())
                    .unwrap_or(DEFAULT_PROXY_LISTENER);
                let listen: SocketAddr = primary_addr
                    .parse()
                    .context("Proxy 监听地址不是合法的 IP:Port")?;
                self.start_proxy_with_override(project, listen, None)
                    .await
            };
            if let Err(err) = start_result {
                self.proxy.set_intercept(prev_intercept.0, prev_intercept.1);
                return Err(err);
            }
        } else {
            self.sync_proxy_plugin_hooks();
            if intercept_disabled {
                self.proxy_state().await;
            }
        }
        let address = self
            .proxy
            .primary_address()
            .context("Proxy 监听器尚未绑定地址")?;
        let authority = self.authority().await?;
        let spki_base64 = authority.spki_sha256_base64()?;
        let ca_der = authority.certificate_der();
        let ca_pem = authority.certificate_pem();
        let profile_prefix = match &session {
            BrowserRequestSession::OriginalSession { .. } => "browser-profile-original-session",
            BrowserRequestSession::Default | BrowserRequestSession::CurrentSession => {
                "browser-profile"
            }
        };
        let profile_folder = browser_profile_folder_name(profile_prefix, address);
        let prefs_path = self.preferences_path.clone();
        let is_isolated = self.preferences.is_isolated();
        let launch_result = tokio::task::spawn_blocking(move || -> Result<String> {
            let base_dir_buf = if let Some(dir) = identity_dir_override {
                dir.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(Path::to_path_buf)
                    .unwrap_or(dir)
            } else if is_isolated {
                std::env::temp_dir().join(format!("mio-isolated-browser-{}", std::process::id()))
            } else {
                let identity_dir = authority::resolved_identity_dir(prefs_path.as_deref())?;
                identity_dir
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(Path::to_path_buf)
                    .unwrap_or(identity_dir)
            };
            let profile_dir =
                resolve_ca_scoped_profile_dir(&base_dir_buf, &profile_folder, &spki_base64);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&profile_dir)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(&profile_dir)?;
            }
            authority::export(spki_base64.as_bytes(), &profile_dir.join(".mio-ca-spki"))?;
            authority::export(&ca_der, &profile_dir.join("mio-ca.cer"))?;
            authority::export(ca_pem.as_bytes(), &profile_dir.join("mio-ca.pem"))?;
            let bin = match executable_override {
                Some(bin) => bin,
                None => resolve_chromium_executable()?,
            };
            let args =
                build_configured_browser_args(address, &spki_base64, &profile_dir, &target_url);
            std::process::Command::new(&bin)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .with_context(|| format!("启动配置浏览器失败（{}）", bin.display()))?;
            Ok(bin.to_string_lossy().into_owned())
        })
        .await?;
        let executable = match launch_result {
            Ok(exe) => exe,
            Err(err) => {
                if intercept_disabled {
                    self.proxy.set_intercept(prev_intercept.0, prev_intercept.1);
                    if self.proxy.is_running() {
                        self.proxy_state().await;
                    }
                }
                return Err(err);
            }
        };
        self.emit(Event::BrowserLaunched {
            project,
            address,
            executable,
        })
        .await;
        Ok(())
    }
}

fn format_proxy_start_error(bind: SocketAddr, error: mio_proxy::ProxyError) -> anyhow::Error {
    match &error {
        mio_proxy::ProxyError::Network(msg)
            if msg.contains("已被占用")
                || msg.contains("已在占用")
                || msg.contains("Address already in use")
                || msg.contains("Only one usage")
                || msg.contains("os error 48")
                || msg.contains("os error 98")
                || msg.contains("os error 10048") =>
        {
            anyhow::Error::new(listener_lock::ListenerConflictError(
                listener_lock::listener_conflict_message(bind),
            ))
        }
        _ => error.into(),
    }
}

pub(crate) fn resolve_ca_scoped_profile_dir(
    base_dir: &Path,
    profile_folder: &str,
    spki_base64: &str,
) -> PathBuf {
    use sha2::{Digest as _, Sha256};

    let base_profile_dir = base_dir.join(profile_folder);
    let spki_marker = base_profile_dir.join(".mio-ca-spki");
    if let Ok(existing) = std::fs::read_to_string(&spki_marker)
        && existing.trim() != spki_base64
    {
        let digest = Sha256::digest(spki_base64.as_bytes());
        let short_tag: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        return base_dir.join(format!("{profile_folder}-ca-{short_tag}"));
    }
    base_profile_dir
}

pub(crate) fn browser_profile_folder_name(prefix: &str, address: SocketAddr) -> String {
    let is_standard_v4 = matches!(
        address.ip(),
        std::net::IpAddr::V4(v4) if v4 == std::net::Ipv4Addr::LOCALHOST || v4.is_unspecified()
    );
    if is_standard_v4 {
        if address.port() == 8080 {
            prefix.to_owned()
        } else {
            format!("{prefix}-{}", address.port())
        }
    } else {
        let sanitized_ip = address.ip().to_string().replace([':', '.'], "_");
        format!("{prefix}-{sanitized_ip}-{}", address.port())
    }
}

pub(super) fn build_drop_out_of_scope_fn(
    scope: TargetScope,
) -> Option<mio_proxy::DropOutOfScopeFn> {
    if !scope.drop_out_of_scope {
        return None;
    }
    Some(Arc::new(move |url: &str| scope.should_drop_url(url)))
}

fn parse_listener_tls_protocols(label: &str) -> mio_proxy::ListenerTlsProtocols {
    let lower = label.trim().to_ascii_lowercase();
    if lower.contains("1.3") && !lower.contains("1.2") {
        mio_proxy::ListenerTlsProtocols::Tls13Only
    } else if lower.contains("1.2") && !lower.contains("1.3") {
        mio_proxy::ListenerTlsProtocols::Tls12Only
    } else {
        mio_proxy::ListenerTlsProtocols::Default
    }
}

fn build_listener_cert_mode(listener: &ProxyListenerConfig) -> mio_proxy::ListenerCertMode {
    match listener.certificate_mode {
        ProxyListenerCertMode::PerHostCaSigned => mio_proxy::ListenerCertMode::PerHostCaSigned,
        ProxyListenerCertMode::SelfSigned => mio_proxy::ListenerCertMode::SelfSigned,
        ProxyListenerCertMode::SpecificHostname => {
            if listener.specific_cert_hostname.trim().is_empty() {
                mio_proxy::ListenerCertMode::PerHostCaSigned
            } else {
                mio_proxy::ListenerCertMode::SpecificHostname(
                    listener.specific_cert_hostname.trim().to_owned(),
                )
            }
        }
        ProxyListenerCertMode::CustomCertificate => {
            if let Some(cert_pem) = listener
                .custom_cert_pem
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                mio_proxy::ListenerCertMode::CustomPem {
                    cert_pem: cert_pem.to_owned(),
                    key_pem: listener.custom_key_pem.clone().unwrap_or_default(),
                }
            } else {
                mio_proxy::ListenerCertMode::PerHostCaSigned
            }
        }
    }
}

fn apply_listener_and_workspace_options(
    config: &mut ProxyConfig,
    listener: &ProxyListenerConfig,
    workspace: &Workspace,
    eff_up: &UpstreamProxySettings,
) {
    config.network.invisible_proxying = listener.invisible_proxying;
    config.network.redirect_to_host = (!listener.redirect_to_host.trim().is_empty())
        .then(|| listener.redirect_to_host.trim().to_owned());
    config.network.redirect_to_port = listener.redirect_to_port;
    config.tls.force_use_of_tls = listener.force_use_of_tls;
    config.http.enable_http2 = listener.enable_http2;
    config.tls.listener_protocols = parse_listener_tls_protocols(&listener.tls_protocols_label);
    config.tls.cert_mode = build_listener_cert_mode(listener);

    let misc = workspace.proxy_messages.misc;
    config.http.use_http10_in_requests = misc.use_http10_in_requests;
    config.http.use_http10_in_responses = misc.use_http10_in_responses;
    config.http.http1_keep_alive = misc.use_http1_keep_alive;
    config.http.use_connection_pool =
        (misc.use_http1_keep_alive && !misc.use_http10_in_requests) || listener.enable_http2;
    config.http.set_response_connection_close = misc.set_response_connection_close;
    config.http.set_request_connection_header = misc.set_request_connection_header;
    config.http.strip_proxy_headers = misc.strip_proxy_headers;
    config.http.remove_unsupported_accept_encodings = misc.remove_unsupported_accept_encodings;
    config.http.strip_sec_websocket_extensions = misc.strip_sec_websocket_extensions;
    config.http.disable_web_interface = misc.disable_web_interface;
    config.http.suppress_browser_error_messages = misc.suppress_browser_error_messages;

    let http = &workspace.network_http;
    config.http.default_to_http2 = http.default_to_http2;
    config.http.streaming_scope_matcher = http.build_streaming_scope_matcher();
    config.http.treat_event_stream_as_streaming = http.treat_event_stream_as_streaming;
    config.http.strip_chunked_metadata_in_streaming = http.strip_chunked_metadata_in_streaming;
    config.http.understand_100_continue = http.understand_100_continue;
    config.http.remove_100_continue_headers = http.remove_100_continue_headers;

    config.auth.platform_enabled = eff_up.platform_auth.enabled;
    config.auth.platform_rules = eff_up.compiled_platform_auth_rules();
    config.auth.upstream_rules = eff_up.compiled_upstream_auth_rules();
}

pub fn build_configured_browser_args(
    proxy_address: SocketAddr,
    spki_sha256_base64: &str,
    profile_dir: &Path,
    target_url: &str,
) -> Vec<String> {
    let connect_address = if proxy_address.ip().is_unspecified() {
        let local_ip = match proxy_address {
            SocketAddr::V4(_) => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            SocketAddr::V6(_) => std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        };
        SocketAddr::new(local_ip, proxy_address.port())
    } else {
        proxy_address
    };
    vec![
        format!("--proxy-server=http://{connect_address}"),
        "--proxy-bypass-list=<-loopback>".into(),
        format!("--ignore-certificate-errors-spki-list={spki_sha256_base64}"),
        format!("--user-data-dir={}", profile_dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--new-window".into(),
        target_url.to_owned(),
    ]
}

fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_chromium_executable() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("MIO_BROWSER_EXECUTABLE").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(&custom);
        if path.is_file() {
            return Ok(path);
        }
        if let Some(found) = custom.to_str().and_then(find_executable_in_path) {
            return Ok(found);
        }
        anyhow::bail!(
            "MIO_BROWSER_EXECUTABLE 指定的浏览器可执行文件不存在：{}",
            path.display()
        );
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let app_rel_paths = [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ];
        for rel in app_rel_paths {
            candidates.push(Path::new("/Applications").join(rel));
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            let user_apps = Path::new(&home).join("Applications");
            for rel in app_rel_paths {
                candidates.push(user_apps.join(rel));
            }
        }
    }
    #[cfg(windows)]
    {
        let win_rel_paths = [
            r"Google\Chrome\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
            r"Microsoft\Edge\Application\msedge.exe",
        ];
        for env_key in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(env_key).filter(|v| !v.is_empty()) {
                for rel in win_rel_paths {
                    candidates.push(Path::new(&root).join(rel));
                }
            }
        }
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    for bin_name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "brave-browser",
        "microsoft-edge",
        "chrome",
        "msedge",
    ] {
        if let Some(found) = find_executable_in_path(bin_name) {
            return Ok(found);
        }
    }
    anyhow::bail!(
        "未检测到本机安装的 Chrome / Chromium / Brave / Edge 浏览器，可通过 MIO_BROWSER_EXECUTABLE 指定可执行文件路径"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_browser_args_include_proxy_loopback_bypass_spki_and_profile() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let authority = MitmAuthority::generate().unwrap();
        let spki = authority.spki_sha256_base64().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let args = build_configured_browser_args(addr, &spki, dir.path(), "https://example.com");
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:8080".to_owned()));
        assert!(args.contains(&"--proxy-bypass-list=<-loopback>".to_owned()));
        assert!(args.contains(&format!("--ignore-certificate-errors-spki-list={spki}")));
        assert!(args.contains(&format!("--user-data-dir={}", dir.path().display())));
        assert_eq!(args.last().map(String::as_str), Some("https://example.com"));

        let v4_all: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        let args_v4_all =
            build_configured_browser_args(v4_all, &spki, dir.path(), "https://example.invalid");
        assert!(args_v4_all.contains(&"--proxy-server=http://127.0.0.1:8080".to_owned()));

        let v6_all: SocketAddr = "[::]:8443".parse().unwrap();
        let args_v6_all =
            build_configured_browser_args(v6_all, &spki, dir.path(), "https://example.invalid");
        assert!(args_v6_all.contains(&"--proxy-server=http://[::1]:8443".to_owned()));
    }

    #[tokio::test]
    async fn open_configured_browser_starts_proxy_disables_intercept_and_exports_ca() {
        let (events, mut receiver) = mpsc::channel(64);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let mut ws = state.store.workspace().unwrap();
        ws.revision += 1;
        ws.proxy_listeners = vec![ProxyListenerConfig {
            id: Uuid::new_v4(),
            enabled: true,
            bind_address: "127.0.0.1:0".into(),
            ..ProxyListenerConfig::default()
        }];
        state.store.save_workspace(&ws).unwrap();
        state.proxy.intercept_requests = true;
        state.proxy.intercept_responses = true;

        let temp = tempfile::tempdir().unwrap();
        let identity_dir = temp.path().join("identity");
        let bin = find_executable_in_path("true").unwrap_or_else(|| PathBuf::from("/usr/bin/true"));
        state
            .open_configured_browser_with(
                project,
                Some("https://example.com".into()),
                BrowserRequestSession::OriginalSession {
                    raw_request: b"GET / HTTP/1.1\r\nHost: example.com\r\nCookie: sid=orig\r\n\r\n"
                        .to_vec(),
                },
                Some(identity_dir),
                Some(bin),
            )
            .await
            .unwrap();

        assert!(!state.proxy.intercept_requests && !state.proxy.intercept_responses);
        assert!(state.proxy.primary.is_some());
        let bound_addr = state.proxy.primary_address().unwrap();
        let profile_dir = temp.path().join(browser_profile_folder_name(
            "browser-profile-original-session",
            bound_addr,
        ));
        assert!(profile_dir.join("mio-ca.cer").is_file());
        assert!(profile_dir.join("mio-ca.pem").is_file());
        let mut probe_req = HttpRequest {
            method: "GET".into(),
            url: "https://example.com/dashboard".into(),
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: b"example.com".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let (changed, _) = state
            .sessions
            .runtime
            .apply_before_send_with_steps(
                SessionTool::Proxy,
                &mut probe_req,
                &SendConfig::default(),
                &CancellationToken::new(),
                Instant::now() + Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert!(changed);
        assert!(probe_req.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case("cookie") && h.value == b"sid=orig"
        }));

        let mut saw_launched = false;
        while let Ok(ev) = receiver.try_recv() {
            if let Event::BrowserLaunched {
                project: ev_proj,
                address,
                ..
            } = ev
            {
                assert_eq!(ev_proj, project);
                assert_ne!(address.port(), 0);
                saw_launched = true;
            }
        }
        assert!(saw_launched);
        state.stop_proxy().await.unwrap();
    }
}
