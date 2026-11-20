use super::*;
use std::net::Ipv4Addr;

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyConfig")
            .field("listen", &self.listen)
            .field("ca", &self.ca)
            .field("upstream_route", &self.upstream_route)
            .field(
                "upstream_route_resolver_configured",
                &self.upstream_route_resolver.is_some(),
            )
            .field("upstream_tls", &self.upstream_tls)
            .field("upstream_tls_options", &self.upstream_tls_options)
            .field("hostname_overrides_len", &self.hostname_overrides.len())
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
            .field(
                "websocket_transform_configured",
                &self.websocket_transform.is_some(),
            )
            .field(
                "ssl_pass_through_configured",
                &self.ssl_pass_through.is_some(),
            )
            .field("flow_transform_configured", &self.flow_transform.is_some())
            .field(
                "async_flow_hook_configured",
                &self.async_flow_hook.is_some(),
            )
            .field(
                "async_websocket_hook_configured",
                &self.async_websocket_hook.is_some(),
            )
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_connections", &self.max_connections)
            .field("request_timeout", &self.request_timeout)
            .field("streaming_timeout", &self.streaming_timeout)
            .field("intercept_timeout", &self.intercept_timeout)
            .field("stop_timeout", &self.stop_timeout)
            .field("shared_connections", &self.shared_connections.is_some())
            .field("shared_requests", &self.shared_requests.is_some())
            .field("shared_injectors", &self.shared_injectors.is_some())
            .field("client_certificates_len", &self.client_certificates.len())
            .field(
                "annotation_checker_configured",
                &self.annotation_checker.is_some(),
            )
            .field("invisible_proxying", &self.invisible_proxying)
            .field("redirect_to_host", &self.redirect_to_host)
            .field("redirect_to_port", &self.redirect_to_port)
            .field("force_use_of_tls", &self.force_use_of_tls)
            .field("enable_http2", &self.enable_http2)
            .field("tls_protocol_filter", &self.tls_protocol_filter)
            .field("cert_mode_custom", &matches!(self.cert_mode, crate::ListenerCertMode::CustomPem { .. }))
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
                "drop_out_of_scope_fn_configured",
                &self.drop_out_of_scope_fn.is_some(),
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
            .field("default_to_http2", &self.default_to_http2)
            .field(
                "streaming_scope_matcher_configured",
                &self.streaming_scope_matcher.is_some(),
            )
            .field("platform_auth_enabled", &self.platform_auth_enabled)
            .field("platform_auth_rules_len", &self.platform_auth_rules.len())
            .field("upstream_auth_rules_len", &self.upstream_auth_rules.len())
            .field(
                "on_client_tls_failure_configured",
                &self.on_client_tls_failure.is_some(),
            )
            .field(
                "on_server_cert_observed_configured",
                &self.on_server_cert_observed.is_some(),
            )
            .finish()
    }
}

impl ProxyConfig {
    pub fn new(ca: MitmAuthority) -> Self {
        Self {
            listen: (Ipv4Addr::LOCALHOST, 0).into(),
            ca,
            upstream_route: UpstreamRoute::Direct,
            upstream_route_resolver: None,
            upstream_tls: UpstreamTls::System,
            upstream_tls_options: crate::UpstreamTlsOptions::default(),
            hostname_overrides: Arc::from([]),
            intercept_requests: false,
            intercept_responses: false,
            intercept_filter: None,
            websocket_intercept_filter: None,
            websocket_transform: None,
            ssl_pass_through: None,
            flow_transform: None,
            async_flow_hook: None,
            async_websocket_hook: None,
            max_body_bytes: 16 * 1024 * 1024,
            max_connections: 64,
            request_timeout: Duration::from_secs(30),
            streaming_timeout: None,
            intercept_timeout: Duration::from_secs(300),
            stop_timeout: Duration::from_secs(3),
            shared_connections: None,
            shared_requests: None,
            shared_injectors: None,
            client_certificates: Arc::from([]),
            annotation_checker: None,
            invisible_proxying: false,
            redirect_to_host: None,
            redirect_to_port: None,
            force_use_of_tls: false,
            enable_http2: true,
            tls_protocol_filter: crate::ListenerTlsProtocols::Default,
            cert_mode: crate::ListenerCertMode::PerHostCaSigned,
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
            drop_out_of_scope_fn: None,
            treat_event_stream_as_streaming: true,
            strip_chunked_metadata_in_streaming: true,
            understand_100_continue: true,
            remove_100_continue_headers: false,
            default_to_http2: true,
            streaming_scope_matcher: None,
            platform_auth_enabled: false,
            platform_auth_rules: Arc::from([]),
            upstream_auth_rules: Arc::from([]),
            on_client_tls_failure: None,
            on_server_cert_observed: None,
        }
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
        self.intercept.send_replace(InterceptMode {
            requests,
            responses,
        });
    }

    pub fn set_transform(&self, transform: Option<FlowTransform>) {
        self.processing
            .send_modify(|processing| processing.0 = transform);
    }

    pub fn set_intercept_filter(&self, filter: Option<InterceptFilter>) {
        self.processing
            .send_modify(|processing| processing.1 = filter);
    }

    pub fn set_websocket_intercept_filter(&self, filter: Option<WebSocketInterceptFilter>) {
        self.processing
            .send_modify(|processing| processing.2 = filter);
    }

    pub fn set_processing(
        &self,
        transform: Option<FlowTransform>,
        filter: Option<InterceptFilter>,
        websocket_filter: Option<WebSocketInterceptFilter>,
    ) {
        self.processing
            .send_replace((transform, filter, websocket_filter));
    }

    pub fn set_websocket_transform(&self, transform: Option<WebSocketTransform>) {
        self.websocket_transform.send_replace(transform);
    }

    pub fn set_ssl_pass_through(&self, matcher: Option<SslPassThroughMatcher>) {
        self.ssl_pass_through.send_replace(matcher);
    }

    pub fn set_plugin_hooks(
        &self,
        flow_hook: Option<AsyncFlowHook>,
        websocket_hook: Option<AsyncWebSocketHook>,
    ) {
        self.plugin_hooks.send_replace((flow_hook, websocket_hook));
    }

    pub fn set_drop_out_of_scope(&self, drop_fn: Option<DropOutOfScopeFn>) {
        self.drop_out_of_scope.send_replace(drop_fn);
    }

    /// Starts cancellation synchronously; `stop` still joins all network tasks.
    pub fn request_stop(&self) {
        self.cancel.cancel();
    }

    /// Recovers terminal records that could not enter the ordinary event channel.
    /// A completed `stop` guarantees no more records can arrive; an error does not.
    pub fn drain_terminal_records(&mut self) -> Vec<CapturedFlow> {
        std::mem::take(
            &mut *self
                .terminal_records
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
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
        let message = client::websocket_outgoing_message(kind, payload, max_body_bytes)?;
        self.injectors.inject(flow_id, message, timeout).await
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
