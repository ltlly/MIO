use std::sync::atomic::{AtomicBool, Ordering};
use mio_plugin_api::{Header, HttpVersion};
use rama::extensions::ExtensionsRef;
use rama::http::{HeaderMap, HeaderValue};

use rama::tls::rustls::dep::rustls;

use super::*;

impl ActiveFlow {
    pub(super) async fn finish(&mut self, result: Result<(), &ProxyError>) {
        let record = self.record.as_mut().expect("active flow owns its record");
        record.summary.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        record.summary.revision += 1;
        record.summary.state = match result {
            Ok(()) => FlowState::Complete,
            Err(ProxyError::Cancelled) => FlowState::Cancelled,
            Err(ProxyError::Dropped) => FlowState::Dropped,
            Err(error) => FlowState::Failed(error.to_string()),
        };
        self.state
            .terminal(
                &mut self.record,
                &mut self.response_stream,
                &self.capture_metrics,
                self.permit.clone(),
            )
            .await;
    }
}

impl Drop for ActiveFlow {
    fn drop(&mut self) {
        let Some(mut record) = self.record.take() else {
            return;
        };
        // HTTP/2 resets drop the handler future before its normal terminal event.
        record.summary.state = FlowState::Cancelled;
        record.summary.revision += 1;
        record.summary.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let state = self.state.clone();
        let permit = self.permit.clone();
        let response_stream = self.response_stream.take();
        let capture_metrics = self.capture_metrics.clone();
        if state.cancel.is_cancelled() || state.events.is_closed() {
            if state.events.is_closed() {
                state.cancel.cancel();
            }
            if let Ok(slot) = state.events.try_reserve() {
                slot.send(ProxyEvent::Captured {
                    project: state.project,
                    record,
                    response_stream,
                    capture_metrics,
                    listener_port: (state.listener_port != 0).then_some(state.listener_port),
                });
            } else {
                state.retain_terminal(
                    CapturedFlow {
                        record,
                        response_stream,
                        capture_metrics,
                        listener_port: (state.listener_port != 0).then_some(state.listener_port),
                    },
                    permit,
                );
            }
            return;
        }
        self.state.tasks.spawn(async move {
            let mut record = Some(record);
            let mut response_stream = response_stream;
            state
                .terminal(&mut record, &mut response_stream, &capture_metrics, permit)
                .await;
        });
    }
}

pub async fn start(
    mut config: ProxyConfig,
    project: ProjectKey,
    events: mpsc::Sender<ProxyEvent>,
) -> Result<ProxyHandle, ProxyError> {
    if config.max_connections == 0
        || config.max_body_bytes == 0
        || config.request_timeout.is_zero()
        || config.intercept_timeout.is_zero()
    {
        return Err(ProxyError::invalid("连接/正文预算及请求期限必须大于零"));
    }
    if config.redirect_to_port == Some(0) {
        return Err(ProxyError::invalid("重定向目标端口必须大于零"));
    }
    config.cert_mode.validate()?;
    config
        .upstream_tls
        .config_with_options(None, &config.upstream_tls_options)?;
    config.upstream_route.rama()?;
    config.upstream_route.tls_config_with_options(
        &config.client_certificates,
        &config.upstream_tls_options,
    )?;
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(ProxyError::network)?;
    let address = listener.local_addr().map_err(ProxyError::network)?;
    let cancel = CancellationToken::new();
    let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
    let exec = Executor::graceful(shutdown.guard());
    let tasks = TaskTracker::new();
    let (intercept, receiver) = watch::channel(InterceptMode {
        requests: config.intercept_requests,
        responses: config.intercept_responses,
    });
    let (processing, processing_receiver) = watch::channel((
        config.flow_transform.take(),
        config.intercept_filter.take(),
        config.websocket_intercept_filter.take(),
    ));
    let (websocket_transform, websocket_transform_receiver) =
        watch::channel(config.websocket_transform.take());
    let (ssl_pass_through, ssl_pass_through_receiver) =
        watch::channel(config.ssl_pass_through.take());
    let (plugin_hooks, plugin_hooks_receiver) = watch::channel((
        config.async_flow_hook.take(),
        config.async_websocket_hook.take(),
    ));
    let (drop_out_of_scope, drop_out_of_scope_receiver) =
        watch::channel(config.drop_out_of_scope_fn.take());
    let stop_timeout = config.stop_timeout;
    let connections = config
        .shared_connections
        .clone()
        .unwrap_or_else(|| Arc::new(Semaphore::new(config.max_connections)));
    let requests = config
        .shared_requests
        .clone()
        .unwrap_or_else(|| Arc::new(Semaphore::new(config.max_connections)));
    let terminal_records = TerminalRecords::default();
    let injectors = config.shared_injectors.clone().unwrap_or_default();
    let state = Arc::new(State {
        project,
        listener_port: address.port(),
        requests,
        config,
        events,
        cancel: cancel.clone(),
        intercept: receiver,
        processing: processing_receiver,
        websocket_transform: websocket_transform_receiver,
        ssl_pass_through: ssl_pass_through_receiver,
        plugin_hooks: plugin_hooks_receiver,
        drop_out_of_scope: drop_out_of_scope_receiver,
        tasks: tasks.clone(),
        exec,
        terminal_records: terminal_records.clone(),
        injectors: injectors.clone(),
    });
    let runner = tokio::spawn(async move {
        let result = async {
            loop {
                let permit = tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => break,
                    _ = state.events.closed() => break,
                    permit = connections.clone().acquire_owned() => permit.map_err(ProxyError::network)?,
                };
                let accepted = tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => break,
                    _ = state.events.closed() => break,
                    result = listener.accept() => result,
                };
                let (stream, _) = accepted.map_err(ProxyError::network)?;
                stream.set_nodelay(true).map_err(ProxyError::network)?;
                let state = state.clone();
                tasks.spawn(async move {
                    let permit = Arc::new(permit);
                    match socks5::peek_first_byte(
                        &stream,
                        state.config.request_timeout,
                        &state.cancel,
                    )
                    .await
                    {
                        Ok(Some(socks5::SOCKS5_LEAD_BYTE)) => {
                            socks5::serve_socks5(stream, state, permit).await;
                            return;
                        }
                        Ok(Some(socks5::TLS_CLIENT_HELLO_LEAD_BYTE))
                            if state.config.invisible_proxying =>
                        {
                            serve_invisible_tls(stream, state, permit).await;
                            return;
                        }
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => return,
                    }
                    let io = GracefulIo::new(
                        Box::pin(state.cancel.clone().cancelled_owned()),
                        TcpStream::new(stream),
                    );
                    let service_state = state.clone();
                    let service = service_fn(move |request: Request| {
                        let state = service_state.clone();
                        let permit = permit.clone();
                        async move { Ok::<_, Infallible>(handle_entry(request, state, permit).await) }
                    });
                    if state.config.enable_http2 {
                        let mut server = HttpServer::auto(state.exec.clone());
                        server
                            .http1_mut()
                            .set_keep_alive(state.config.http1_keep_alive);
                        server.h2_mut().set_max_concurrent_streams(
                            state.config.max_connections.min(u32::MAX as usize) as u32,
                        );
                        tokio::select! {
                            biased;
                            _ = state.cancel.cancelled() => {},
                            _ = server.serve(io, service) => {},
                        }
                    } else {
                        let mut server = HttpServer::new_http1(state.exec.clone());
                        server
                            .http1_mut()
                            .set_keep_alive(state.config.http1_keep_alive);
                        tokio::select! {
                            biased;
                            _ = state.cancel.cancelled() => {},
                            _ = server.serve(io, service) => {},
                        }
                    }
                });
            }
            Ok(())
        }
        .await;
        state.cancel.cancel();
        drop(listener);
        tasks.close();
        tasks.wait().await;
        drop(state);
        shutdown.shutdown().await;
        tasks.wait().await;
        result
    });
    Ok(ProxyHandle {
        address,
        cancel,
        intercept,
        processing,
        websocket_transform,
        ssl_pass_through,
        plugin_hooks,
        drop_out_of_scope,
        runner: Some(runner),
        stop_timeout,
        terminal_records,
        injectors,
    })
}

async fn serve_invisible_tls(
    stream: tokio::net::TcpStream,
    state: Arc<State>,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
    let _permit = permit;
    let fallback_host = state
        .config
        .redirect_to_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .or_else(|| match &state.config.cert_mode {
            crate::ListenerCertMode::SpecificHostname(host) if !host.trim().is_empty() => {
                Some(host.trim())
            }
            _ => None,
        })
        .unwrap_or("localhost")
        .to_owned();
    let observed_sni = Arc::new(std::sync::Mutex::new(None::<String>));
    let tls_config = state.config.ca.invisible_server_config(
        state.listener_tls_options(),
        fallback_host.clone(),
        observed_sni.clone(),
    );
    let io = GracefulIo::new(
        Box::pin(state.cancel.clone().cancelled_owned()),
        TcpStream::new(stream),
    );
    let served_any = Arc::new(AtomicBool::new(false));
    let served_flag = served_any.clone();
    let handler_state = state.clone();
    let sni_for_req = observed_sni.clone();
    let fallback_for_req = fallback_host.clone();
    let service = service_fn(move |request: Request| {
        served_flag.store(true, Ordering::Relaxed);
        let state = handler_state.clone();
        let authority = request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                sni_for_req
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_else(|| fallback_for_req.clone());
        async move {
            Ok::<_, Infallible>(
                handle_http_with_ingress(request, state, Some(authority), true).await,
            )
        }
    });
    let run = async {
        let serve_result = if state.config.enable_http2 {
            let mut server = HttpServer::auto(state.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(state.config.http1_keep_alive);
            server.h2_mut().set_max_concurrent_streams(
                state.config.max_connections.min(u32::MAX as usize) as u32,
            );
            TlsAcceptorService::new(tls_config, server.service(service), true)
                .serve(io)
                .await
        } else {
            let mut server = HttpServer::new_http1(state.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(state.config.http1_keep_alive);
            TlsAcceptorService::new(tls_config, server.service(service), true)
                .serve(io)
                .await
        };
        serve_result
            .or_else(|err| {
                if served_any.load(Ordering::Relaxed)
                    || is_benign_tunnel_disconnect(err.as_ref())
                {
                    Ok(())
                } else {
                    let sni_host = observed_sni
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                        .unwrap_or_else(|| fallback_host.clone());
                    let port = state.config.redirect_to_port.unwrap_or(443);
                    state.notify_client_tls_failure(sni_host, port);
                    Err(err)
                }
            })
            .map_err(ProxyError::network)
    };
    tokio::select! {
        biased;
        _ = state.cancel.cancelled() => {},
        result = run => {
            if let Err(error) = result {
                let sni_host = observed_sni
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .unwrap_or_else(|| fallback_host.clone());
                let failed_request = HttpRequest {
                    method: "CONNECT".into(),
                    url: format!("{sni_host}:{}", state.config.redirect_to_port.unwrap_or(443)),
                    version: HttpVersion::Http11,
                    headers: Vec::new(),
                    body: Vec::new(),
                    trailers: Vec::new(),
                };
                state.failure(failed_request, &error).await;
            }
        }
    }
}

async fn handle_entry(
    request: Request,
    state: Arc<State>,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> Response {
    if request.method() != Method::CONNECT {
        return handle_http(request, state, None).await;
    }
    let failed_request = message::invalid_request_head(&request);
    let authority = request.uri().to_string();
    let result = async {
        let uri: Uri = format!("https://{authority}/")
            .parse()
            .map_err(ProxyError::invalid)?;
        let host = uri
            .host_str()
            .ok_or_else(|| ProxyError::invalid("CONNECT 目标缺少主机"))?;
        let port = uri
            .port_u16()
            .ok_or_else(|| ProxyError::invalid("CONNECT 目标缺少端口"))?;
        if ssl_passthrough::should_pass_through(&state, &host, port) {
            let (pt_host, pt_port) = state.effective_passthrough_target(&host, port);
            return ssl_passthrough::spawn_passthrough(
                &request,
                pt_host,
                pt_port,
                state.clone(),
                permit,
                failed_request.clone(),
            );
        }
        let tls_options = state.listener_tls_options();
        let config = tokio::time::timeout(
            state.config.request_timeout,
            state
                .config
                .ca
                .server_config_with_options(&host, &tls_options),
        )
        .await
        .map_err(|_| ProxyError::Timeout)??;
        let tls_host = host.to_string();
        let tls_port = port;
        let upgrade = handle_upgrade(&request);
        let task_state = state.clone();
        let tunnel_request = failed_request.clone();
        state.tasks.spawn(async move {
            let _permit = permit;
            let served_any = Arc::new(AtomicBool::new(false));
            let served_flag = served_any.clone();
            let run = async {
                let upgraded = upgrade.await.map_err(ProxyError::network)?;
                let handler_state = task_state.clone();
                let service = service_fn(move |request: Request| {
                    served_flag.store(true, Ordering::Relaxed);
                    let state = handler_state.clone();
                    let authority = authority.clone();
                    async move {
                        Ok::<_, Infallible>(
                            handle_http_with_ingress(request, state, Some(authority), true).await,
                        )
                    }
                });
                let serve_result = if task_state.config.enable_http2 {
                    let mut server = HttpServer::auto(task_state.exec.clone());
                    server
                        .http1_mut()
                        .set_keep_alive(task_state.config.http1_keep_alive);
                    server.h2_mut().set_max_concurrent_streams(
                        task_state.config.max_connections.min(u32::MAX as usize) as u32,
                    );
                    TlsAcceptorService::new(config, server.service(service), true)
                        .serve(upgraded)
                        .await
                } else {
                    let mut server = HttpServer::new_http1(task_state.exec.clone());
                    server
                        .http1_mut()
                        .set_keep_alive(task_state.config.http1_keep_alive);
                    TlsAcceptorService::new(config, server.service(service), true)
                        .serve(upgraded)
                        .await
                };
                serve_result
                    .or_else(|err| {
                        if served_any.load(Ordering::Relaxed)
                            || is_benign_tunnel_disconnect(err.as_ref())
                        {
                            Ok(())
                        } else {
                            task_state.notify_client_tls_failure(tls_host.clone(), tls_port);
                            Err(err)
                        }
                    })
                    .map_err(ProxyError::network)
            };
            tokio::select! {
                biased;
                _ = task_state.cancel.cancelled() => {},
                result = run => {
                    if let Err(error) = result {
                        task_state.failure(tunnel_request, &error).await;
                    }
                },
            }
        });
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::OK;
        Ok(response)
    }
    .await;
    match result {
        Ok(response) => response,
        Err(error) => {
            state.failure(failed_request, &error).await;
            state.error_response(&error)
        }
    }
}

pub(in crate::server) fn is_benign_tunnel_disconnect(
    err: &(dyn std::error::Error + 'static),
) -> bool {
    let mut saw_disconnect = false;
    let mut cur = Some(err);
    while let Some(e) = cur {
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => return false,
                std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected => saw_disconnect = true,
                _ => {}
            }
        }
        if e.downcast_ref::<rustls::Error>().is_some() {
            return false;
        }
        cur = e.source();
    }
    saw_disconnect
}

fn maybe_serve_local_ca_portal(request: &Request, state: &State) -> Option<Response> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return None;
    }
    let uri = request.uri();
    let host_cow = uri.host_str().or_else(|| {
        request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(|h| std::borrow::Cow::Borrowed(h.split(':').next().unwrap_or(h)))
    })?;
    let host = host_cow.trim_end_matches('.');
    if !host.eq_ignore_ascii_case("mio") {
        return None;
    }
    let path_cow = uri.path_or_root();
    let path = path_cow.as_ref();
    let head_only = request.method() == Method::HEAD;
    match path {
        "/cert" | "/cert.der" | "/mio-ca.cer" | "/cacert.der" => {
            let der = state.config.ca.certificate_der();
            let mut resp = Response::new(rama::http::Body::from(if head_only {
                Vec::new()
            } else {
                der
            }));
            *resp.status_mut() = rama::http::StatusCode::OK;
            resp.headers_mut().insert(
                "content-type",
                rama::http::HeaderValue::from_static("application/x-x509-ca-cert"),
            );
            resp.headers_mut().insert(
                "content-disposition",
                rama::http::HeaderValue::from_static("attachment; filename=\"mio-ca.der\""),
            );
            Some(resp)
        }
        "/cert.pem" | "/mio-ca.pem" => {
            let pem = state.config.ca.certificate_pem().into_bytes();
            let mut resp = Response::new(rama::http::Body::from(if head_only {
                Vec::new()
            } else {
                pem
            }));
            *resp.status_mut() = rama::http::StatusCode::OK;
            resp.headers_mut().insert(
                "content-type",
                rama::http::HeaderValue::from_static("application/x-pem-file"),
            );
            resp.headers_mut().insert(
                "content-disposition",
                rama::http::HeaderValue::from_static("attachment; filename=\"mio-ca.pem\""),
            );
            Some(resp)
        }
        "/" | "" => {
            const PORTAL_HTML: &str = "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>MIO Proxy</title><style>body{margin:0;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#f4f5f7;color:#1f2328}header{background:#fff;border-bottom:1px solid #d0d7de;padding:14px 28px;display:flex;align-items:center;justify-content:space-between}.brand{font-weight:700;font-size:16px}.actions a{display:inline-block;margin-left:10px;padding:7px 14px;border-radius:4px;text-decoration:none;font-size:13px;font-weight:600}.primary{background:#d95b26;color:#fff}.secondary{background:#fff;color:#1f2328;border:1px solid #c4c9d0}main{max-width:720px;margin:48px auto;background:#fff;border:1px solid #d0d7de;border-radius:6px;padding:28px}</style></head><body><header><div class=\"brand\">MIO — MITM Inspection &amp; Orchestration</div><div class=\"actions\"><a class=\"primary\" href=\"/cert\">CA Certificate (DER)</a><a class=\"secondary\" href=\"/cert.pem\">CA Certificate (PEM)</a></div></header><main><h2>欢迎使用 MIO 拦截代理</h2><p>当前浏览器流量已成功接入 MIO 本地代理监听器。点击右上角按钮即可下载用户级持久化根证书（<code>MIO local CA</code>），导入系统或浏览器受信任根证书颁发机构后即可拦截检视 HTTPS / HTTP/2 / WSS 流量。</p></main></body></html>";
            let mut resp = Response::new(rama::http::Body::from(if head_only {
                ""
            } else {
                PORTAL_HTML
            }));
            *resp.status_mut() = rama::http::StatusCode::OK;
            resp.headers_mut().insert(
                "content-type",
                rama::http::HeaderValue::from_static("text/html; charset=utf-8"),
            );
            Some(resp)
        }
        _ => None,
    }
}

pub(in crate::server) async fn handle_http(
    request: Request,
    state: Arc<State>,
    tunnel: Option<String>,
) -> Response {
    handle_http_with_ingress(request, state, tunnel, false).await
}

pub(in crate::server) async fn handle_http_with_ingress(
    request: Request,
    state: Arc<State>,
    tunnel: Option<String>,
    allow_origin_form: bool,
) -> Response {
    if tunnel.is_none()
        && !state.config.disable_web_interface
        && let Some(portal_response) = maybe_serve_local_ca_portal(&request, &state)
    {
        return portal_response;
    }
    let started = Instant::now();
    if tunnel.is_none()
        && !allow_origin_form
        && request.uri().scheme().is_none()
        && !state.config.invisible_proxying
        && state
            .config
            .redirect_to_host
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        let error = ProxyError::invalid("缺少请求目标 authority（如需直连请启用隐形代理）");
        state
            .failure(message::invalid_request_head(&request), &error)
            .await;
        return state.error_response(&error);
    }
    let mut model = match message::request_head_with_fallback(
        &request,
        tunnel.as_deref(),
        state.config.redirect_to_host.as_deref(),
    ) {
        Ok(model) => model,
        Err(error) => {
            state
                .failure(message::invalid_request_head(&request), &error)
                .await;
            return state.error_response(&error);
        }
    };
    if let Err(error) = apply_listener_target_overrides(&mut model, &state.config) {
        state.failure(model, &error).await;
        return state.error_response(&error);
    }
    if state.should_drop_out_of_scope(&model.url) {
        return state.error_response(&ProxyError::Dropped);
    }
    if !state.config.understand_100_continue
        && !state.config.remove_100_continue_headers
        && has_expect_100_continue(request.headers())
    {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::EXPECTATION_FAILED;
        return resp;
    }
    let is_ws = websocket::requested(&request);
    apply_proxy_misc_request_transforms(&mut model, &state.config, is_ws);
    let permit = match state.requests.clone().try_acquire_owned() {
        Ok(permit) => Arc::new(permit),
        Err(_) => {
            let error = ProxyError::Network("活动请求达到配置上限".into());
            state.failure(model, &error).await;
            return state.error_response(&error);
        }
    };
    let mut active = ActiveFlow {
        state: state.clone(),
        record: Some(new_record(model)),
        response_stream: None,
        capture_metrics: CapturedFlowMetrics::default(),
        request_was_intercepted: false,
        request_was_modified: false,
        started,
        permit: permit.clone(),
    };
    if is_ws {
        return websocket::handle(request, active).await;
    }
    let cancel = state.cancel.child_token();
    let cancel_on_drop = cancel.clone().drop_guard();
    let record = active.record.as_mut().expect("active flow owns its record");
    let ingress_version = message::model_version(request.version());
    let upstream = process_http(
        request,
        &state,
        record,
        permit,
        cancel.clone(),
        &mut active.request_was_intercepted,
        &mut active.request_was_modified,
    )
    .await;
    let upstream = upstream.map(|(response, deadline, capture_metrics)| {
        active.capture_metrics = capture_metrics;
        (response, deadline)
    });
    let result = match upstream {
        Ok((response, _))
            if sse::detected_or_in_scope(
                &response,
                &record.request,
                state.config.treat_event_stream_as_streaming,
                state.config.streaming_scope_matcher.as_ref(),
            ) =>
        {
            return sse::handle(response, active, ingress_version, cancel, cancel_on_drop).await;
        }
        Ok((response, deadline)) => {
            let status = response.status().as_u16();
            let version = message::model_version(response.version());
            let headers = message::headers(response.headers());
            async {
                let (body, trailers) = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
                    result = tokio::time::timeout_at(deadline, message::body(response.into_body(), state.config.max_body_bytes)) => result.map_err(|_| ProxyError::Timeout)??,
                };
                cancel.cancel();
                record.summary.status = Some(status);
                record.summary.response_bytes = body.len() as u64;
                record.response = Some(HttpResponse { status, version, reason_phrase: None, headers, body, trailers });
                state.intercept(record, true, None, &active.capture_metrics, active.request_was_intercepted, active.request_was_modified).await?;
                let response_client_version = if state.config.use_http10_in_responses
                    && ingress_version != HttpVersion::Http2
                {
                    HttpVersion::Http10
                } else {
                    ingress_version
                };
                let mut outgoing = message::to_response(
                    record.response.as_ref().expect("upstream response is present"),
                    state.config.max_body_bytes,
                    response_client_version,
                    record.request.method.eq_ignore_ascii_case("HEAD"),
                )?;
                if state.config.set_response_connection_close
                    && response_client_version != HttpVersion::Http2
                {
                    outgoing
                        .headers_mut()
                        .insert("connection", HeaderValue::from_static("close"));
                }
                Ok(outgoing)
            }
            .await
        }
        Err(error) => Err(error),
    };
    cancel.cancel();
    active.finish(result.as_ref().map(|_| ())).await;
    result.unwrap_or_else(|error| state.error_response(&error))
}

fn has_expect_100_continue(headers: &HeaderMap) -> bool {
    headers
        .get_all("expect")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("100-continue"))
        })
}

pub(in crate::server) fn apply_listener_target_overrides(
    model: &mut HttpRequest,
    config: &ProxyConfig,
) -> Result<(), ProxyError> {
    let redirect_host = config
        .redirect_to_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    let redirect_port = config.redirect_to_port.filter(|&p| p > 0);
    if redirect_host.is_none() && redirect_port.is_none() && !config.force_use_of_tls {
        return Ok(());
    }
    let uri: Uri = model.url.parse().map_err(ProxyError::invalid)?;
    let original_scheme = uri.scheme_str().unwrap_or("http");
    let effective_scheme = if config.force_use_of_tls {
        if original_scheme.eq_ignore_ascii_case("ws") || original_scheme.eq_ignore_ascii_case("wss")
        {
            "wss"
        } else {
            "https"
        }
    } else {
        original_scheme
    };
    let original_host = uri
        .host_str()
        .ok_or_else(|| ProxyError::invalid("缺少请求目标主机"))?;
    let host_raw = redirect_host.unwrap_or(original_host.as_ref());
    let formatted_host = if host_raw.contains(':') && !host_raw.starts_with('[') && !host_raw.ends_with(']') {
        format!("[{host_raw}]")
    } else {
        host_raw.to_owned()
    };
    let effective_port = redirect_port.or_else(|| uri.port_u16());
    let mut origin_target = rama::bytes::BytesMut::new();
    uri.write_http_origin_form(&mut origin_target)
        .map_err(ProxyError::invalid)?;
    let path_and_query = std::str::from_utf8(&origin_target).map_err(ProxyError::invalid)?;
    model.url = match effective_port {
        Some(port) => format!("{effective_scheme}://{formatted_host}:{port}{path_and_query}"),
        None => format!("{effective_scheme}://{formatted_host}{path_and_query}"),
    };
    Ok(())
}

pub(in crate::server) fn apply_proxy_misc_request_transforms(
    model: &mut HttpRequest,
    config: &ProxyConfig,
    is_websocket: bool,
) {
    if config.strip_proxy_headers {
        model.headers.retain(|header| {
            !header
                .name
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("proxy-"))
        });
    }
    if config.remove_unsupported_accept_encodings {
        filter_unsupported_accept_encodings(&mut model.headers);
    }
    if config.remove_100_continue_headers {
        message::strip_expect_100_continue_vec(&mut model.headers);
    }
    if is_websocket && config.strip_sec_websocket_extensions {
        model
            .headers
            .retain(|header| !header.name.eq_ignore_ascii_case("sec-websocket-extensions"));
    }
    if !is_websocket {
        if config.use_http10_in_requests {
            model.version = HttpVersion::Http10;
        }
        if config.set_request_connection_header && model.version != HttpVersion::Http2 {
            let value = if !config.http1_keep_alive || config.use_http10_in_requests {
                b"close".to_vec()
            } else {
                b"keep-alive".to_vec()
            };
            let mut updated = false;
            model.headers.retain_mut(|header| {
                if !header.name.eq_ignore_ascii_case("connection") {
                    return true;
                }
                if updated {
                    return false;
                }
                header.value = value.clone();
                updated = true;
                true
            });
            if !updated {
                model.headers.push(Header {
                    name: "Connection".into(),
                    value,
                });
            }
        }
    }
}

fn filter_unsupported_accept_encodings(headers: &mut Vec<Header>) {
    headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case("accept-encoding") {
            return true;
        }
        let Ok(text) = std::str::from_utf8(&header.value) else {
            return true;
        };
        let kept: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|part| {
                let coding = part.split(';').next().unwrap_or("").trim();
                matches!(
                    coding.to_ascii_lowercase().as_str(),
                    "gzip" | "x-gzip" | "deflate" | "br" | "identity"
                )
            })
            .collect();
        if kept.is_empty() {
            false
        } else {
            header.value = kept.join(", ").into_bytes();
            true
        }
    });
}

pub(in crate::server) fn new_record(model: HttpRequest) -> FlowRecord {
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: model.method.clone(),
            url: model.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: 0,
            response_bytes: 0,
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(i64::MAX as u128) as i64,
            elapsed_ms: 0,
        },
        request: model,
        response: None,
    }
}

pub(in crate::server) async fn process_http(
    request: Request,
    state: &Arc<State>,
    record: &mut FlowRecord,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    cancel: CancellationToken,
    request_was_intercepted: &mut bool,
    request_was_modified: &mut bool,
) -> Result<(Response, tokio::time::Instant, CapturedFlowMetrics), ProxyError> {
    let (body, trailers) = tokio::select! {
        biased;
        _ = state.cancel.cancelled() => return Err(ProxyError::Cancelled),
        result = tokio::time::timeout(state.config.request_timeout, message::body(request.into_body(), state.config.max_body_bytes)) => result.map_err(|_| ProxyError::Timeout)??,
    };
    record.request.body = body;
    record.request.trailers = trailers;
    record.summary.request_bytes = record.request.body.len() as u64;
    state
        .emit(ProxyEvent::Captured {
            project: state.project,
            record: record.clone(),
            response_stream: None,
            listener_port: (state.listener_port != 0).then_some(state.listener_port),
            capture_metrics: CapturedFlowMetrics::default(),
        })
        .await?;
    (*request_was_intercepted, *request_was_modified) =
        state.intercept(record, false, None, &CapturedFlowMetrics::default(), false, false).await?;
    let route = state.resolve_upstream_route_for_url(&record.request.url);
    let is_https_target = record.request.url.len() >= 8
        && record.request.url[..8].eq_ignore_ascii_case("https://");
    let outgoing_http_version = if state.config.use_http10_in_requests {
        Some(HttpVersion::Http10)
    } else if !state.config.enable_http2
        || (is_https_target && !state.config.default_to_http2 && record.request.version != HttpVersion::Http2)
    {
        Some(HttpVersion::Http11)
    } else {
        None
    };
    let config = SendConfig {
        upstream_route: route,
        upstream_tls: state.config.upstream_tls.clone(),
        upstream_tls_options: state.config.upstream_tls_options,
        max_body_bytes: state.config.max_body_bytes,
        timeout: state.config.request_timeout,
        streaming_timeout: state.config.streaming_timeout,
        http_version: outgoing_http_version,
        update_content_length: true,
        normalize_http1_line_endings: true,
        raw_http1_override: None,
        http1_keep_alive: state.config.http1_keep_alive && !state.config.use_http10_in_requests,
        set_connection_keep_alive: state.config.set_request_connection_header
            && state.config.http1_keep_alive
            && !state.config.use_http10_in_requests,
        http2_reuse: state.config.enable_http2,
        use_connection_pool: state.config.use_connection_pool,
        allow_http2_alpn_override: false,
        strip_100_continue_headers: state.config.remove_100_continue_headers,
        strip_proxy_headers: state.config.strip_proxy_headers,
        on_server_cert_observed: state.config.on_server_cert_observed.clone(),
        client_certificates: state.config.client_certificates.clone(),
        hostname_overrides: state.config.hostname_overrides.clone(),
        default_to_http2: state.config.default_to_http2,
        force_streaming_response: state
            .config
            .streaming_scope_matcher
            .as_ref()
            .is_some_and(|m| m(&record.request.url)),
        platform_auth_enabled: state.config.platform_auth_enabled,
        platform_auth_rules: state.config.platform_auth_rules.clone(),
        upstream_auth_rules: state.config.upstream_auth_rules.clone(),
        ..SendConfig::default()
    };
    let _ = message::to_request_with_config(&record.request, &config)?;
    let outbound_model = record.request.clone();
    let deadline = tokio::time::Instant::now() + config.timeout;
    let (sender, response) = oneshot::channel();
    // Keep the driver and request budget until cancellation has actually joined the socket.
    state.tasks.spawn(async move {
        let _permit = permit;
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        let exec = Executor::graceful(shutdown.guard());
        let attempt_started = Instant::now();
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = client::exchange_model_with_auth(outbound_model, &config, deadline, cancel.clone(), exec) => {
                match result {
                    Ok((resp, timing)) => {
                        let exchange_returned = Instant::now();
                        let reason_phrase = resp
                            .extensions()
                            .get_ref::<rama::http::proto::h1::ext::ReasonPhrase>()
                            .map(|reason| reason.as_bytes().to_vec());
                        let (remote_ip, received) = {
                            let guard = timing
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            (
                                guard.peer_ip(),
                                guard.first_response_byte().or(Some(exchange_returned)),
                            )
                        };
                        let response_received_ms = received.map(|received| received
                            .saturating_duration_since(attempt_started)
                            .as_millis()
                            .min(u64::MAX as u128) as u64);
                        Ok((resp, CapturedFlowMetrics { remote_ip, response_received_ms, reason_phrase }))
                    }
                    Err(err) => Err(err),
                }
            },
        };
        if sender.send(result).is_ok() {
            cancel.cancelled().await;
        }
        cancel.cancel();
        shutdown.shutdown().await;
    });
    let (response, metrics) = response.await.map_err(ProxyError::network)??;
    Ok((response, deadline, metrics))
}
