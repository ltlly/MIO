use super::*;
use std::{collections::HashMap, sync::OnceLock};

const INTERCEPT_DNS_CACHE_LIMIT: usize = 256;
const INTERCEPT_DNS_CACHE_TTL: Duration = Duration::from_secs(30);

enum LocalTargetIp {
    Resolved(Option<String>),
    NeedsDns { host: String, port: u16 },
}

fn intercept_dns_cache() -> &'static Mutex<HashMap<String, (Instant, String)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup_cached_dns_ip(host_lower: &str) -> Option<String> {
    let now = Instant::now();
    let mut cache = intercept_dns_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((inserted_at, ip)) = cache.get(host_lower) {
        if now.saturating_duration_since(*inserted_at) < INTERCEPT_DNS_CACHE_TTL {
            return Some(ip.clone());
        }
        cache.remove(host_lower);
    }
    None
}

fn store_cached_dns_ip(host_lower: String, ip: String) {
    let now = Instant::now();
    let mut cache = intercept_dns_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|_, (inserted_at, _)| {
        now.saturating_duration_since(*inserted_at) < INTERCEPT_DNS_CACHE_TTL
    });
    if cache.len() >= INTERCEPT_DNS_CACHE_LIMIT && !cache.contains_key(&host_lower) {
        cache.clear();
    }
    cache.insert(host_lower, (now, ip));
}

impl State {
    pub(super) async fn intercept(
        &self,
        record: &mut FlowRecord,
        response_stage: bool,
        response_stream: Option<&StreamedResponse>,
        capture_metrics: &CapturedFlowMetrics,
        request_was_intercepted: bool,
        request_was_modified: bool,
    ) -> Result<(bool, bool), ProxyError> {
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let response_head_only = response_stream.is_some();
        let mut transformed = false;
        let mut original_request = None;
        let mut request_modified = false;
        let mut force_intercept = false;
        let (transform, filter, _) = self.processing.borrow().clone();
        if let Some(transform) = transform {
            let deadline = Instant::now()
                .checked_add(self.config.request_timeout)
                .ok_or(ProxyError::Timeout)?;
            let mut backup = record.clone();
            let is_ws = record
                .request
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("sec-websocket-key"));
            let transformed_result = transform(
                record,
                response_stage,
                response_head_only,
                &self.cancel,
                (deadline, self.config.max_body_bytes),
            );
            if self.cancel.is_cancelled() {
                *record = backup;
                return Err(ProxyError::Cancelled);
            }
            if Instant::now() >= deadline {
                *record = backup;
                return Err(ProxyError::Timeout);
            }
            let transform_applied = match transformed_result {
                Ok(applied) => applied,
                Err(error) => {
                    *record = backup;
                    return Err(error);
                }
            };
            let mut accepted = true;
            if transform_applied {
                let valid = if !response_stage {
                    if is_ws {
                        websocket::outgoing_request_with_options(
                            &record.request,
                            self.config.max_body_bytes,
                            self.config.strip_sec_websocket_extensions,
                        )
                        .is_ok()
                    } else {
                        message::to_request(&record.request, self.config.max_body_bytes).is_ok()
                    }
                } else if let Some(response) = &record.response {
                    (!response_head_only
                        || (response.body.is_empty()
                            && response.trailers.is_empty()
                            && !(100..200).contains(&response.status)))
                        && message::to_response(
                            response,
                            self.config.max_body_bytes,
                            record.request.version,
                            false,
                        )
                        .is_ok()
                } else {
                    false
                };
                accepted = valid;
            }
            if !accepted {
                *record = backup;
            } else if !response_stage && backup.request != record.request {
                transformed = transform_applied;
                request_modified = true;
                original_request = Some(backup.request);
            } else {
                transformed = transform_applied;
                if response_stage && !response_head_only && backup.response != record.response {
                    backup.summary.revision = backup
                        .summary
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| ProxyError::invalid("流量 revision 已耗尽"))?;
                    record.summary.revision = backup.summary.revision;
                    self.emit(ProxyEvent::Captured {
                        project: self.project,
                        record: backup,
                        response_stream: None,
                        listener_port: (self.listener_port != 0).then_some(self.listener_port),
                        capture_metrics: capture_metrics.clone(),
                    })
                    .await?;
                }
            }
        }
        let (flow_hook, _) = self.plugin_hooks.borrow().clone();
        if let Some(hook) = flow_hook
            && !matches!(record.summary.source, TrafficSource::Plugin(_))
        {
            let deadline = Instant::now()
                .checked_add(self.config.request_timeout)
                .ok_or(ProxyError::Timeout)?;
            let tokio_deadline = tokio::time::Instant::from_std(deadline);
            let mut backup = record.clone();
            let decision = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(ProxyError::Cancelled),
                res = tokio::time::timeout_at(
                    tokio_deadline,
                    hook(
                        record.clone(),
                        response_stage,
                        response_head_only,
                        self.cancel.child_token(),
                        deadline,
                    ),
                ) => res.map_err(|_| ProxyError::Timeout)??,
            };
            let force_intercept_decision =
                matches!(decision, FlowHookDecision::ForceIntercept { .. });
            match decision {
                FlowHookDecision::Pass => {}
                FlowHookDecision::Drop => return Err(ProxyError::Dropped),
                FlowHookDecision::ModifyRequest(request)
                | FlowHookDecision::ForceIntercept {
                    request: Some(request),
                    response: None,
                } if !response_stage => {
                    if record
                        .request
                        .headers
                        .iter()
                        .any(|header| header.name.eq_ignore_ascii_case("sec-websocket-key"))
                    {
                        websocket::outgoing_request_with_options(
                            &request,
                            self.config.max_body_bytes,
                            self.config.strip_sec_websocket_extensions,
                        )?;
                    } else {
                        message::to_request(&request, self.config.max_body_bytes)?;
                    }
                    if backup.request != request {
                        transformed = true;
                        request_modified = true;
                        if original_request.is_none() {
                            original_request = Some(backup.request);
                        }
                        record.request = request;
                        record.summary.method = record.request.method.clone();
                        record.summary.url = record.request.url.clone();
                        record.summary.request_bytes = record.request.body.len() as u64;
                    }
                    force_intercept = force_intercept_decision;
                }
                FlowHookDecision::ModifyResponse(response)
                | FlowHookDecision::ForceIntercept {
                    request: None,
                    response: Some(response),
                } if response_stage => {
                    if response_head_only
                        && (!response.body.is_empty()
                            || !response.trailers.is_empty()
                            || (100..200).contains(&response.status))
                    {
                        return Err(ProxyError::invalid(
                            "SSE 仅拦截响应头，不能在此阶段替换正文、trailers 或最终响应状态",
                        ));
                    }
                    message::to_response(
                        &response,
                        self.config.max_body_bytes,
                        record.request.version,
                        false,
                    )?;
                    if backup.response.as_ref() != Some(&response) {
                        if !response_head_only && !transformed && backup.response.is_some() {
                            backup.summary.revision = backup
                                .summary
                                .revision
                                .checked_add(1)
                                .ok_or_else(|| ProxyError::invalid("流量 revision 已耗尽"))?;
                            record.summary.revision = backup.summary.revision;
                            self.emit(ProxyEvent::Captured {
                                project: self.project,
                                record: backup,
                                response_stream: None,
                                listener_port: (self.listener_port != 0)
                                    .then_some(self.listener_port),
                                capture_metrics: capture_metrics.clone(),
                            })
                            .await?;
                        }
                        transformed = true;
                        record.summary.status = Some(response.status);
                        record.summary.response_bytes = response.body.len() as u64;
                        record.response = Some(response);
                    }
                    force_intercept = force_intercept_decision;
                }
                FlowHookDecision::ForceIntercept {
                    request: None,
                    response: None,
                } => force_intercept = true,
                _ => return Err(ProxyError::InterceptStage),
            }
        }
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let mut mode = self.intercept.clone();
        let enabled = |mode: InterceptMode| {
            if response_stage {
                mode.responses
            } else {
                mode.requests
            }
        };
        let intercept_enabled = enabled(*mode.borrow_and_update());
        let should_intercept = force_intercept
            || (intercept_enabled && {
                match filter {
                    Some(filter) => {
                        if self.cancel.is_cancelled() {
                            return Err(ProxyError::Cancelled);
                        }
                        Instant::now()
                            .checked_add(self.config.request_timeout)
                            .ok_or(ProxyError::Timeout)?;
                        let was_annotated = self
                            .config
                            .annotation_checker
                            .as_ref()
                            .is_some_and(|checker| checker(record.summary.id));
                        let local_ip = self
                            .resolve_intercept_target_ip_local(&record.request.url, capture_metrics);
                        let record_ref: &FlowRecord = record;
                        let run_filter = |target_ip: Option<&str>| -> Result<bool, ProxyError> {
                            let deadline = Instant::now()
                                .checked_add(self.config.request_timeout)
                                .ok_or(ProxyError::Timeout)?;
                            let ctx = InterceptEvalContext {
                                listener_port: self.listener_port,
                                target_ip,
                                was_annotated,
                            };
                            let result = filter(
                                record_ref,
                                response_stage,
                                response_head_only,
                                request_was_intercepted,
                                request_was_modified,
                                ctx,
                                &self.cancel,
                                (deadline, self.config.max_body_bytes),
                            );
                            if self.cancel.is_cancelled() {
                                return Err(ProxyError::Cancelled);
                            }
                            if Instant::now() >= deadline {
                                return Err(ProxyError::Timeout);
                            }
                            result
                        };
                        match local_ip {
                            LocalTargetIp::Resolved(resolved_ip) => {
                                run_filter(resolved_ip.as_deref())?
                            }
                            LocalTargetIp::NeedsDns { host, port } => match run_filter(None) {
                                Ok(decision) => decision,
                                Err(ProxyError::InvalidMessage(msg))
                                    if msg.contains("目标 IP 尚未确定") =>
                                {
                                    if self.cancel.is_cancelled() {
                                        return Err(ProxyError::Cancelled);
                                    }
                                    let resolved_ip =
                                        self.lookup_intercept_target_ip(&host, port).await;
                                    if self.cancel.is_cancelled() {
                                        return Err(ProxyError::Cancelled);
                                    }
                                    match resolved_ip {
                                        Some(ip) => {
                                            store_cached_dns_ip(
                                                host.to_ascii_lowercase(),
                                                ip.clone(),
                                            );
                                            run_filter(Some(ip.as_str()))?
                                        }
                                        None => return Err(ProxyError::InvalidMessage(msg)),
                                    }
                                }
                                Err(err) => return Err(err),
                            },
                        }
                    }
                    None => true,
                }
            });
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        if !should_intercept {
            if transformed {
                record.summary.state = FlowState::Pending;
                record.summary.revision += 1;
                self.emit(ProxyEvent::Captured {
                    project: self.project,
                    record: record.clone(),
                    response_stream: response_stream.cloned(),
                    listener_port: (self.listener_port != 0).then_some(self.listener_port),
                    capture_metrics: capture_metrics.clone(),
                })
                .await?;
            }
            return Ok((false, request_modified));
        }
        record.summary.state = FlowState::Intercepted;
        record.summary.revision += 1;
        let (reply, mut receive) = oneshot::channel();
        self.emit(ProxyEvent::Intercepted {
            project: self.project,
            record: record.clone(),
            response_head_only,
            response_stream: response_stream.cloned(),
            listener_port: (self.listener_port != 0).then_some(self.listener_port),
            capture_metrics: capture_metrics.clone(),
            reply,
        })
        .await?;
        let wait = async {
            loop {
                if self.cancel.is_cancelled() {
                    return Err(ProxyError::Cancelled);
                }
                if !force_intercept && !enabled(*mode.borrow_and_update()) {
                    return Ok(None);
                }
                tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => return Err(ProxyError::Cancelled),
                    changed = mode.changed() => { changed.map_err(|_| ProxyError::Cancelled)?; },
                    decision = &mut receive => return decision.map(Some).map_err(|_| ProxyError::EventConsumerClosed),
                }
            }
        };
        let decision = tokio::time::timeout(self.config.intercept_timeout, wait)
            .await
            .map_err(|_| ProxyError::Timeout)??;
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        match decision {
            Some(InterceptDecision::ForwardRequest(request)) if !response_stage => {
                if record
                    .request
                    .headers
                    .iter()
                    .any(|header| header.name.eq_ignore_ascii_case("sec-websocket-key"))
                {
                    websocket::outgoing_request_with_options(
                        &request,
                        self.config.max_body_bytes,
                        self.config.strip_sec_websocket_extensions,
                    )?;
                } else {
                    message::to_request(&request, self.config.max_body_bytes)?;
                }
                request_modified = original_request.as_ref().unwrap_or(&record.request) != &request;
                record.request = request;
                record.summary.method = record.request.method.clone();
                record.summary.url = record.request.url.clone();
                record.summary.request_bytes = record.request.body.len() as u64;
            }
            Some(InterceptDecision::ForwardResponse(response)) if response_stage => {
                if response_head_only {
                    if !response.body.is_empty() || !response.trailers.is_empty() {
                        return Err(ProxyError::invalid(
                            "SSE 仅拦截响应头，不能在此阶段替换正文或 trailers",
                        ));
                    }
                    if (100..200).contains(&response.status) {
                        return Err(ProxyError::invalid(
                            "SSE 最终响应不能改为临时响应或协议升级",
                        ));
                    }
                }
                message::to_response(
                    &response,
                    self.config.max_body_bytes,
                    record.request.version,
                    false,
                )?;
                if !response_head_only
                    && !transformed
                    && record.response.as_ref() != Some(&response)
                {
                    let mut backup = record.clone();
                    backup.summary.state = FlowState::Pending;
                    backup.summary.revision = backup
                        .summary
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| ProxyError::invalid("流量 revision 已耗尽"))?;
                    record.summary.revision = backup.summary.revision;
                    self.emit(ProxyEvent::Captured {
                        project: self.project,
                        record: backup,
                        response_stream: None,
                        listener_port: (self.listener_port != 0).then_some(self.listener_port),
                        capture_metrics: capture_metrics.clone(),
                    })
                    .await?;
                }
                record.summary.status = Some(response.status);
                record.summary.response_bytes = response.body.len() as u64;
                record.response = Some(response);
            }
            Some(InterceptDecision::Drop) => return Err(ProxyError::Dropped),
            Some(_) => return Err(ProxyError::InterceptStage),
            None => {}
        }
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        record.summary.state = FlowState::Pending;
        record.summary.revision += 1;
        self.emit(ProxyEvent::Captured {
            project: self.project,
            record: record.clone(),
            response_stream: response_stream.cloned(),
            listener_port: (self.listener_port != 0).then_some(self.listener_port),
            capture_metrics: capture_metrics.clone(),
        })
        .await?;
        Ok((true, request_modified))
    }

    fn resolve_intercept_target_ip_local(
        &self,
        url: &str,
        capture_metrics: &CapturedFlowMetrics,
    ) -> LocalTargetIp {
        if let Some(ip) = capture_metrics.remote_ip {
            return LocalTargetIp::Resolved(Some(ip.to_string()));
        }
        let Ok(uri) = url.parse::<Uri>() else {
            return LocalTargetIp::Resolved(None);
        };
        let Some(host_raw) = uri.host_str() else {
            return LocalTargetIp::Resolved(None);
        };
        let host = host_raw.trim_matches(['[', ']']);
        if let Ok(ip) = host.parse::<IpAddr>() {
            return LocalTargetIp::Resolved(Some(ip.to_string()));
        }
        let route = self.resolve_upstream_route_for_host(host);
        if !route.is_direct_or_local_dns() {
            return LocalTargetIp::Resolved(None);
        }
        if let Some(ip) = crate::transport::resolve_hostname_override(
            &self.config.hostname_overrides,
            host,
        ) {
            return LocalTargetIp::Resolved(Some(ip.to_string()));
        }
        if let Some(cached) = lookup_cached_dns_ip(&host.to_ascii_lowercase()) {
            return LocalTargetIp::Resolved(Some(cached));
        }
        let port = uri.port_u16().unwrap_or_else(|| match uri.scheme_str() {
            Some(scheme)
                if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") =>
            {
                443
            }
            _ => 80,
        });
        LocalTargetIp::NeedsDns {
            host: host.to_owned(),
            port,
        }
    }

    async fn lookup_intercept_target_ip(&self, host: &str, port: u16) -> Option<String> {
        let lookup_timeout = self.config.request_timeout.min(Duration::from_secs(2));
        if lookup_timeout.is_zero() {
            return None;
        }
        let lookup = tokio::time::timeout(lookup_timeout, tokio::net::lookup_host((host, port)));
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => None,
            res = lookup => res.ok()?.ok()?.next().map(|addr| addr.ip().to_string()),
        }
    }
}
