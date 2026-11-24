use super::*;

impl std::fmt::Debug for ProxyNetworkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyNetworkConfig")
            .field("listen", &self.listen)
            .field("invisible_proxying", &self.invisible_proxying)
            .field("redirect_to_host", &self.redirect_to_host)
            .field("redirect_to_port", &self.redirect_to_port)
            .field("upstream_route", &self.upstream_route)
            .field(
                "upstream_route_resolver_configured",
                &self.upstream_route_resolver.is_some(),
            )
            .field("hostname_overrides_len", &self.hostname_overrides.len())
            .finish()
    }
}

impl std::fmt::Debug for ProxyTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyTlsConfig")
            .field("ca", &self.ca)
            .field("listener_protocols", &self.listener_protocols)
            .field(
                "cert_mode_custom",
                &matches!(self.cert_mode, crate::ListenerCertMode::CustomPem { .. }),
            )
            .field("force_use_of_tls", &self.force_use_of_tls)
            .field("upstream", &self.upstream)
            .field(
                "on_client_tls_failure_configured",
                &self.on_client_tls_failure.is_some(),
            )
            .finish()
    }
}

impl Default for ProxyLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 16 * 1024 * 1024,
            max_connections: 64,
            request_timeout: Duration::from_secs(30),
            streaming_timeout: None,
            intercept_timeout: Duration::from_secs(300),
            stop_timeout: Duration::from_secs(3),
        }
    }
}

impl Default for ProxyHttpConfig {
    fn default() -> Self {
        Self {
            enable_http2: true,
            default_to_http2: true,
            use_http10_in_requests: false,
            use_http10_in_responses: false,
            http1_keep_alive: true,
            use_connection_pool: false,
            set_response_connection_close: false,
            set_request_connection_header: false,
            strip_proxy_headers: true,
            remove_unsupported_accept_encodings: false,
            strip_sec_websocket_extensions: true,
            disable_web_interface: false,
            suppress_browser_error_messages: false,
            treat_event_stream_as_streaming: true,
            strip_chunked_metadata_in_streaming: true,
            understand_100_continue: true,
            remove_100_continue_headers: false,
            streaming_scope_matcher: None,
        }
    }
}

impl std::fmt::Debug for ProxyHttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyHttpConfig")
            .field("enable_http2", &self.enable_http2)
            .field("default_to_http2", &self.default_to_http2)
            .field("use_http10_in_requests", &self.use_http10_in_requests)
            .field("use_http10_in_responses", &self.use_http10_in_responses)
            .field("http1_keep_alive", &self.http1_keep_alive)
            .field("use_connection_pool", &self.use_connection_pool)
            .field(
                "set_response_connection_close",
                &self.set_response_connection_close,
            )
            .field(
                "set_request_connection_header",
                &self.set_request_connection_header,
            )
            .field("strip_proxy_headers", &self.strip_proxy_headers)
            .field(
                "remove_unsupported_accept_encodings",
                &self.remove_unsupported_accept_encodings,
            )
            .field(
                "strip_sec_websocket_extensions",
                &self.strip_sec_websocket_extensions,
            )
            .field("disable_web_interface", &self.disable_web_interface)
            .field(
                "suppress_browser_error_messages",
                &self.suppress_browser_error_messages,
            )
            .field(
                "treat_event_stream_as_streaming",
                &self.treat_event_stream_as_streaming,
            )
            .field(
                "strip_chunked_metadata_in_streaming",
                &self.strip_chunked_metadata_in_streaming,
            )
            .field("understand_100_continue", &self.understand_100_continue)
            .field(
                "remove_100_continue_headers",
                &self.remove_100_continue_headers,
            )
            .field(
                "streaming_scope_matcher_configured",
                &self.streaming_scope_matcher.is_some(),
            )
            .finish()
    }
}

impl std::fmt::Debug for ProxyHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyHooks")
            .field("intercept_requests", &self.intercept_requests)
            .field("intercept_responses", &self.intercept_responses)
            .field(
                "intercept_filter_configured",
                &self.intercept_filter.is_some(),
            )
            .field(
                "websocket_intercept_filter_configured",
                &self.websocket_intercept_filter.is_some(),
            )
            .field("flow_transform_configured", &self.flow_transform.is_some())
            .field(
                "websocket_transform_configured",
                &self.websocket_transform.is_some(),
            )
            .field(
                "ssl_pass_through_configured",
                &self.ssl_pass_through.is_some(),
            )
            .field(
                "async_flow_hook_configured",
                &self.async_flow_hook.is_some(),
            )
            .field(
                "async_websocket_hook_configured",
                &self.async_websocket_hook.is_some(),
            )
            .field(
                "drop_out_of_scope_fn_configured",
                &self.drop_out_of_scope_fn.is_some(),
            )
            .field(
                "annotation_checker_configured",
                &self.annotation_checker.is_some(),
            )
            .finish()
    }
}

impl std::fmt::Debug for ProxySharedResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxySharedResources")
            .field("connections", &self.connections.is_some())
            .field("requests", &self.requests.is_some())
            .field("injectors", &self.injectors.is_some())
            .finish()
    }
}

impl ProxyConfig {
    pub fn new(ca: MitmAuthority) -> Self {
        Self {
            network: ProxyNetworkConfig {
                listen: (Ipv4Addr::LOCALHOST, 0).into(),
                invisible_proxying: false,
                redirect_to_host: None,
                redirect_to_port: None,
                upstream_route: UpstreamRoute::Direct,
                upstream_route_resolver: None,
                hostname_overrides: Arc::from([]),
            },
            tls: ProxyTlsConfig {
                ca,
                listener_protocols: crate::ListenerTlsProtocols::Default,
                cert_mode: crate::ListenerCertMode::PerHostCaSigned,
                force_use_of_tls: false,
                upstream: UpstreamTlsConfig::default(),
                on_client_tls_failure: None,
            },
            limits: ProxyLimits::default(),
            http: ProxyHttpConfig::default(),
            auth: AuthConfig::default(),
            hooks: ProxyHooks::default(),
            shared: ProxySharedResources::default(),
        }
    }

    pub fn to_send_config(
        &self,
        url: &str,
        request_version: HttpVersion,
        is_websocket: bool,
    ) -> SendConfig {
        let route = resolve_upstream_route_for_url(&self.network, url);
        build_send_config(
            &self.network,
            &self.tls,
            &self.limits,
            &self.http,
            &self.auth,
            route,
            url,
            request_version,
            is_websocket,
        )
    }

    pub(super) fn into_parts(self) -> (StaticProxyConfig, DynamicProxyState, ProxySharedResources) {
        let ca = self.tls.ca.clone();
        let static_cfg = StaticProxyConfig {
            network: self.network,
            tls: self.tls,
            limits: self.limits,
            http: self.http,
            auth: self.auth,
            annotation_checker: self.hooks.annotation_checker,
        };
        let dynamic = DynamicProxyState {
            ca,
            intercept: InterceptMode {
                requests: self.hooks.intercept_requests,
                responses: self.hooks.intercept_responses,
            },
            flow_transform: self.hooks.flow_transform,
            intercept_filter: self.hooks.intercept_filter,
            websocket_intercept_filter: self.hooks.websocket_intercept_filter,
            websocket_transform: self.hooks.websocket_transform,
            ssl_pass_through: self.hooks.ssl_pass_through,
            async_flow_hook: self.hooks.async_flow_hook,
            async_websocket_hook: self.hooks.async_websocket_hook,
            drop_out_of_scope: self.hooks.drop_out_of_scope_fn,
        };
        (static_cfg, dynamic, self.shared)
    }
}

fn resolve_upstream_route_for_host(network: &ProxyNetworkConfig, host: &str) -> UpstreamRoute {
    let route = if let Some(resolver) = &network.upstream_route_resolver {
        resolver(host)
    } else {
        network.upstream_route.clone()
    };
    route.resolve_for_host(host)
}

fn resolve_upstream_route_for_url(network: &ProxyNetworkConfig, url: &str) -> UpstreamRoute {
    if let Ok(uri) = url.parse::<Uri>()
        && let Some(host) = uri.host_str()
    {
        return resolve_upstream_route_for_host(network, &host);
    }
    network.upstream_route.clone()
}

#[allow(clippy::too_many_arguments)]
fn build_send_config(
    network: &ProxyNetworkConfig,
    tls: &ProxyTlsConfig,
    limits: &ProxyLimits,
    http: &ProxyHttpConfig,
    auth: &AuthConfig,
    upstream_route: UpstreamRoute,
    url: &str,
    request_version: HttpVersion,
    is_websocket: bool,
) -> SendConfig {
    let send_http = if is_websocket {
        SendHttpConfig {
            version: Some(HttpVersion::Http11),
            default_to_http2: http.default_to_http2,
            allow_http2_alpn_override: false,
            update_content_length: true,
            normalize_http1_line_endings: true,
            raw_http1_override: None,
            http1_keep_alive: true,
            set_connection_keep_alive: false,
            http2_reuse: false,
            use_connection_pool: false,
            strip_100_continue_headers: http.remove_100_continue_headers,
            strip_proxy_headers: http.strip_proxy_headers,
            force_streaming_response: false,
        }
    } else {
        let is_https_target = url
            .as_bytes()
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"https://"));
        let version = if http.use_http10_in_requests {
            Some(HttpVersion::Http10)
        } else if !http.enable_http2
            || (is_https_target
                && !http.default_to_http2
                && request_version != HttpVersion::Http2)
        {
            Some(HttpVersion::Http11)
        } else {
            None
        };
        let keep_alive = http.http1_keep_alive && !http.use_http10_in_requests;
        SendHttpConfig {
            version,
            default_to_http2: http.default_to_http2,
            allow_http2_alpn_override: false,
            update_content_length: true,
            normalize_http1_line_endings: true,
            raw_http1_override: None,
            http1_keep_alive: keep_alive,
            set_connection_keep_alive: http.set_request_connection_header && keep_alive,
            http2_reuse: http.enable_http2,
            use_connection_pool: http.use_connection_pool,
            strip_100_continue_headers: http.remove_100_continue_headers,
            strip_proxy_headers: http.strip_proxy_headers,
            force_streaming_response: http
                .streaming_scope_matcher
                .as_ref()
                .is_some_and(|matcher| matcher(url)),
        }
    };
    SendConfig {
        network: SendNetworkConfig {
            upstream_route,
            hostname_overrides: network.hostname_overrides.clone(),
        },
        tls: tls.upstream.clone(),
        limits: SendLimits {
            max_body_bytes: limits.max_body_bytes,
            timeout: limits.request_timeout,
            streaming_timeout: limits.streaming_timeout,
        },
        http: send_http,
        auth: auth.clone(),
        group: SendGroupConfig::default(),
    }
}

impl State {
    pub(super) fn resolve_upstream_route_for_host(&self, host: &str) -> UpstreamRoute {
        resolve_upstream_route_for_host(&self.config.network, host)
    }

    pub(super) fn resolve_upstream_route_for_url(&self, url: &str) -> UpstreamRoute {
        resolve_upstream_route_for_url(&self.config.network, url)
    }

    pub(super) fn send_config(
        &self,
        url: &str,
        request_version: HttpVersion,
        is_websocket: bool,
    ) -> SendConfig {
        let route = self.resolve_upstream_route_for_url(url);
        build_send_config(
            &self.config.network,
            &self.config.tls,
            &self.config.limits,
            &self.config.http,
            &self.config.auth,
            route,
            url,
            request_version,
            is_websocket,
        )
    }

    pub(super) fn listener_tls_options(&self) -> crate::tls::ListenerTlsOptions {
        crate::tls::ListenerTlsOptions {
            enable_http2: self.config.http.enable_http2,
            protocol_filter: self.config.tls.listener_protocols,
            cert_mode: self.config.tls.cert_mode.clone(),
        }
    }

    pub(super) fn should_drop_out_of_scope(&self, url: &str) -> bool {
        self.dynamic
            .borrow()
            .drop_out_of_scope
            .as_ref()
            .is_some_and(|drop_fn| drop_fn(url))
    }

    pub(super) fn notify_client_tls_failure(&self, host: String, port: u16) {
        if let Some(callback) = &self.config.tls.on_client_tls_failure {
            let clean = crate::route::strip_host_brackets(&host).to_owned();
            callback(clean, port);
        }
    }

    pub(super) fn error_response(&self, error: &ProxyError) -> Response {
        message::error_response_with_options(
            error,
            self.config.http.suppress_browser_error_messages,
        )
    }

    pub(super) fn effective_passthrough_target(&self, host: &str, port: u16) -> (String, u16) {
        let effective_host = self
            .config
            .network
            .redirect_to_host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .unwrap_or(host)
            .to_owned();
        let effective_port = self
            .config
            .network
            .redirect_to_port
            .filter(|&p| p > 0)
            .unwrap_or(port);
        (effective_host, effective_port)
    }
}

impl ProxyHandle {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Turning interception off forwards queued messages and invalidates old replies.
    pub fn set_intercept(&self, requests: bool, responses: bool) {
        self.dynamic.send_modify(|state| {
            state.intercept = InterceptMode {
                requests,
                responses,
            };
        });
    }

    pub fn set_transform(&self, transform: Option<FlowTransform>) {
        self.dynamic
            .send_modify(|state| state.flow_transform = transform);
    }

    pub fn set_intercept_filter(&self, filter: Option<InterceptFilter>) {
        self.dynamic
            .send_modify(|state| state.intercept_filter = filter);
    }

    pub fn set_websocket_intercept_filter(&self, filter: Option<WebSocketInterceptFilter>) {
        self.dynamic
            .send_modify(|state| state.websocket_intercept_filter = filter);
    }

    pub fn set_processing(
        &self,
        transform: Option<FlowTransform>,
        filter: Option<InterceptFilter>,
        websocket_filter: Option<WebSocketInterceptFilter>,
    ) {
        self.dynamic.send_modify(|state| {
            state.flow_transform = transform;
            state.intercept_filter = filter;
            state.websocket_intercept_filter = websocket_filter;
        });
    }

    pub fn set_websocket_transform(&self, transform: Option<WebSocketTransform>) {
        self.dynamic
            .send_modify(|state| state.websocket_transform = transform);
    }

    pub fn set_ssl_pass_through(&self, matcher: Option<SslPassThroughMatcher>) {
        self.dynamic
            .send_modify(|state| state.ssl_pass_through = matcher);
    }

    pub fn set_plugin_hooks(
        &self,
        flow_hook: Option<AsyncFlowHook>,
        websocket_hook: Option<AsyncWebSocketHook>,
    ) {
        self.dynamic.send_modify(|state| {
            state.async_flow_hook = flow_hook;
            state.async_websocket_hook = websocket_hook;
        });
    }

    pub fn set_drop_out_of_scope(&self, drop_fn: Option<DropOutOfScopeFn>) {
        self.dynamic
            .send_modify(|state| state.drop_out_of_scope = drop_fn);
    }

    pub fn set_authority(&self, ca: MitmAuthority) {
        self.dynamic.send_modify(|state| state.ca = ca);
    }

    /// Starts cancellation synchronously; `stop` still joins all network tasks.
    pub fn request_stop(&self) {
        self.cancel.cancel();
    }

    /// Recovers terminal records that could not enter the ordinary event channel.
    /// A completed `stop` guarantees no more records can arrive; an error does not.
    pub fn drain_terminal_records(&mut self) -> Vec<CapturedFlow> {
        std::mem::take(&mut *crate::lock_unpoison(&self.terminal_records))
            .into_iter()
            .map(|terminal| terminal.capture)
            .collect()
    }

    pub fn websocket_injectors(&self) -> WebSocketInjectorRegistry {
        self.injectors.clone()
    }

    pub async fn inject_websocket(
        &self,
        flow_id: Uuid,
        kind: mio_plugin_api::WebSocketKind,
        payload: &[u8],
        max_body_bytes: usize,
        timeout: Duration,
    ) -> Result<(), ProxyError> {
        self.injectors
            .inject_with_limit(
                flow_id,
                WebSocketDirection::ClientToServer,
                kind,
                payload.to_vec(),
                max_body_bytes,
                timeout,
            )
            .await
            .map(|_| ())
    }

    pub fn remove_websocket_injector(&self, flow_id: Uuid) {
        self.injectors.remove(flow_id);
    }

    /// Waits for the runner without cancelling it; dropping this wait retains the same join.
    pub async fn join(&mut self) -> Result<(), ProxyError> {
        let Some(runner) = self.runner.as_mut() else {
            return Ok(());
        };
        let result = runner.await;
        self.runner.take();
        result.map_err(ProxyError::network)?
    }

    /// A runner error consumes the join too; a Stop deadline does not.
    pub fn is_joined(&self) -> bool {
        self.runner.is_none()
    }

    /// A deadline leaves cleanup joinable; retrying waits for that same cleanup.
    pub async fn stop(&mut self) -> Result<(), ProxyError> {
        self.request_stop();
        tokio::time::timeout(self.stop_timeout, self.join())
            .await
            .unwrap_or(Err(ProxyError::StopDeadline))
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
