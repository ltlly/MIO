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

    async fn start_proxy_listeners(
        &mut self,
        project: ProjectKey,
        workspace: &Workspace,
        net: &ProxyRuntimeNetConfig,
        desired_listeners: &[(ProxyListenerConfig, SocketAddr)],
        emit_state_on_failure: bool,
    ) -> Result<()> {
        let authority = self.authority().await?;
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
        let mut handles: Vec<(Uuid, ProxyHandle)> = Vec::new();
        for (listener_cfg, bind) in desired_listeners {
            let mut config = ProxyConfig::new(authority.clone());
            config.network.listen = *bind;
            config.limits.max_connections = PROXY_MAX_CONNECTIONS;
            config.hooks.intercept_requests = self.proxy.intercept_requests;
            config.hooks.intercept_responses = self.proxy.intercept_responses;
            config.network.upstream_route = net.upstream_route.clone();
            config.network.upstream_route_resolver = Some(net.upstream_route_resolver.clone());
            config.tls.upstream.trust = net.upstream_tls.clone();
            config.tls.upstream.options = net.upstream_tls_options;
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
            config.hooks.intercept_filter = Some(crate::http::build_intercept_filter(
                workspace.proxy_interception.clone(),
                workspace.scope.clone(),
            )?);
            config.hooks.websocket_intercept_filter =
                Some(crate::http::build_websocket_intercept_filter(
                    workspace.proxy_websocket_interception,
                    workspace.scope.clone(),
                )?);
            config.hooks.flow_transform =
                crate::http::build_flow_transform_with_scope_and_provenance(
                    workspace.match_and_replace_rules.clone(),
                    workspace.proxy_messages,
                    workspace.scope.clone(),
                    Some(self.storage.provenance.clone()),
                )?;
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
                Ok(proxy) => handles.push((listener_cfg.id, proxy)),
                Err(error) => {
                    for (_, started) in &handles {
                        started.request_stop();
                    }
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                    for (_, started) in &mut handles {
                        let _ = tokio::time::timeout_at(deadline, started.stop()).await;
                    }
                    self.proxy.clear_handles();
                    if emit_state_on_failure {
                        self.ws_repeater.set_injectors(None);
                        self.proxy_state().await;
                    }
                    return Err(error.into());
                }
            }
        }
        if !handles.is_empty() {
            let (first_id, first) = handles.remove(0);
            self.proxy.primary = Some(first);
            self.proxy.primary_id = Some(first_id);
        }
        self.proxy.extra.extend(handles);
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
        let allow_non_loopback =
            std::env::var_os("MIO_ALLOW_NON_LOOPBACK_PROXY").is_some_and(|v| v == "1");
        ensure!(
            listen.ip().is_loopback() || allow_non_loopback,
            "当前监听只允许 loopback 地址"
        );
        let workspace = self.store.metadata().workspace()?;
        let client_certificates = self
            .compile_client_certificates(&workspace.client_certificates)
            .await?;
        self.tls.compiled_client_certs = client_certificates.clone();
        let net = self.build_proxy_runtime_net_config(&workspace, true)?;
        self.sessions.runtime.sync_workspace(self.project, &workspace);
        let mut session_send_cfg = SendConfig::default();
        session_send_cfg.network.upstream_route = net.upstream_route.clone();
        session_send_cfg.network.hostname_overrides = net.hostname_overrides.clone();
        session_send_cfg.tls.trust = net.upstream_tls.clone();
        session_send_cfg.tls.options = net.upstream_tls_options;
        session_send_cfg.tls.client_certificates = client_certificates;
        session_send_cfg.limits.timeout = net.request_timeout;
        session_send_cfg.limits.streaming_timeout = net.streaming_timeout;
        session_send_cfg.http.strip_100_continue_headers =
            workspace.network_http.remove_100_continue_headers;
        session_send_cfg.http.default_to_http2 = workspace.network_http.default_to_http2;
        session_send_cfg.auth.platform_enabled = net.eff_upstream.platform_auth.enabled;
        session_send_cfg.auth.platform_rules = net.eff_upstream.compiled_platform_auth_rules();
        session_send_cfg.auth.upstream_rules = net.eff_upstream.compiled_upstream_auth_rules();
        self.sessions.runtime.set_proxy_send_config(session_send_cfg);
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
                ensure!(
                    bind.ip().is_loopback() || allow_non_loopback,
                    "当前监听只允许 loopback 地址"
                );
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut stopped_any = false;
        if self.proxy.primary.is_some()
            && self.proxy.primary_id.unwrap_or(Uuid::from_u128(1)) == listener_id
        {
            if let Some(mut primary) = self.proxy.primary.take() {
                primary.request_stop();
                let _ = tokio::time::timeout_at(deadline, primary.stop()).await;
                stopped_any = true;
            }
            if !self.proxy.extra.is_empty() {
                let (next_id, next_handle) = self.proxy.extra.remove(0);
                self.proxy.primary = Some(next_handle);
                self.proxy.primary_id = Some(next_id);
            } else {
                self.proxy.primary_id = None;
            }
        } else if let Some(pos) = self
            .proxy
            .extra
            .iter()
            .position(|(id, _)| *id == listener_id)
        {
            let (_, mut handle) = self.proxy.extra.remove(pos);
            handle.request_stop();
            let _ = tokio::time::timeout_at(deadline, handle.stop()).await;
            stopped_any = true;
        }
        ensure!(stopped_any, "未找到正在运行的指定 Proxy 监听器");
        self.ws_repeater
            .set_injectors(self.proxy.websocket_injectors());
        self.proxy_state().await;
        let listeners = self.proxy.active_listeners();
        self.emit(Event::ProxyListeners {
            project: self.project,
            listeners,
        })
        .await;
        Ok(())
    }

    pub(super) async fn reload_running_proxy_from_workspace(
        &mut self,
        workspace: &Workspace,
    ) -> Result<()> {
        if !self.proxy.is_running() {
            return Ok(());
        }
        let allow_non_loopback =
            std::env::var_os("MIO_ALLOW_NON_LOOPBACK_PROXY").is_some_and(|v| v == "1");
        let net = self.build_proxy_runtime_net_config(workspace, false)?;

        let current_addrs: Vec<SocketAddr> =
            self.proxy.handles().map(ProxyHandle::address).collect();

        let listeners = workspace.effective_proxy_listeners();
        let has_explicit_listeners = !workspace.proxy_listeners.is_empty();
        let desired_listeners: Vec<(ProxyListenerConfig, SocketAddr)> =
            if has_explicit_listeners && listeners.iter().any(|l| l.enabled) {
                listeners
                    .iter()
                    .filter(|l| l.enabled)
                    .map(|l| {
                        let mut addr: SocketAddr = l.bind_address.trim().parse()?;
                        ensure!(
                            addr.ip().is_loopback() || allow_non_loopback,
                            "当前监听只允许 loopback 地址"
                        );
                        if addr.port() == 0
                            && let Some(first_cur) = current_addrs.first()
                            && first_cur.ip() == addr.ip()
                        {
                            addr.set_port(first_cur.port());
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

        self.stop_proxy().await?;
        let project = self.project;
        Box::pin(self.start_proxy_listeners(
            project,
            workspace,
            &net,
            &desired_listeners,
            true,
        ))
        .await
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
        let mut intercept_disabled = false;
        if self.proxy.intercept_requests || self.proxy.intercept_responses {
            self.proxy.set_intercept(false, false);
            intercept_disabled = true;
        }
        if self.proxy.primary.is_none() {
            let workspace = self.store.metadata().workspace()?;
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
                .await?;
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
        let profile_folder = match &session {
            BrowserRequestSession::OriginalSession { .. } => "browser-profile-original-session",
            BrowserRequestSession::Default | BrowserRequestSession::CurrentSession => {
                "browser-profile"
            }
        };
        let executable = tokio::task::spawn_blocking(move || -> Result<String> {
            let identity_dir = match identity_dir_override {
                Some(dir) => dir,
                None => authority::default_identity_dir()?,
            };
            let base_dir = identity_dir
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(&identity_dir);
            let profile_dir = base_dir.join(profile_folder);
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
        .await??;
        self.emit(Event::BrowserLaunched {
            project,
            address,
            executable,
        })
        .await;
        Ok(())
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
    vec![
        format!("--proxy-server=http://{proxy_address}"),
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
        let profile_dir = temp.path().join("browser-profile-original-session");
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
