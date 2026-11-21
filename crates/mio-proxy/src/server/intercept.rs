use super::*;
use std::{collections::VecDeque, sync::OnceLock};

const INTERCEPT_DNS_CACHE_LIMIT: usize = 256;
const INTERCEPT_DNS_CACHE_TTL: Duration = Duration::from_secs(30);

enum LocalTargetIp {
    Resolved(Option<String>),
    NeedsDns { host: String, port: u16 },
}

fn intercept_dns_cache() -> &'static Mutex<VecDeque<(String, Instant, String)>> {
    static CACHE: OnceLock<Mutex<VecDeque<(String, Instant, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(INTERCEPT_DNS_CACHE_LIMIT)))
}

fn purge_expired_dns_entries(cache: &mut VecDeque<(String, Instant, String)>, now: Instant) {
    cache.retain(|(_, inserted_at, _)| {
        now.saturating_duration_since(*inserted_at) < INTERCEPT_DNS_CACHE_TTL
    });
}

fn lookup_cached_dns_ip(host_lower: &str) -> Option<String> {
    let now = Instant::now();
    let mut cache = crate::lock_unpoison(intercept_dns_cache());
    purge_expired_dns_entries(&mut cache, now);
    let pos = cache.iter().position(|(host, _, _)| host == host_lower)?;
    let entry = cache.remove(pos).expect("position came from cache");
    let ip = entry.2.clone();
    cache.push_back(entry);
    Some(ip)
}

fn store_cached_dns_ip(host_lower: String, ip: String) {
    let now = Instant::now();
    let mut cache = crate::lock_unpoison(intercept_dns_cache());
    purge_expired_dns_entries(&mut cache, now);
    if let Some(pos) = cache.iter().position(|(host, _, _)| host == &host_lower) {
        cache.remove(pos);
    }
    while cache.len() >= INTERCEPT_DNS_CACHE_LIMIT {
        cache.pop_front();
    }
    cache.push_back((host_lower, now, ip));
}

fn is_websocket_request(request: &HttpRequest) -> bool {
    request
        .headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("sec-websocket-key"))
}

impl State {
    fn validate_intercepted_request(
        &self,
        request: &HttpRequest,
        is_ws: bool,
    ) -> Result<(), ProxyError> {
        if is_ws {
            websocket::validate_outgoing_request(
                request,
                self.config.limits.max_body_bytes,
                self.config.http.strip_sec_websocket_extensions,
            )
        } else {
            message::validate_request(request, self.config.limits.max_body_bytes)
        }
    }

    fn validate_intercepted_response(
        &self,
        response: &HttpResponse,
        response_head_only: bool,
    ) -> Result<(), ProxyError> {
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
        message::validate_response(response, self.config.limits.max_body_bytes)
    }

    async fn emit_captured(
        &self,
        record: FlowRecord,
        response_stream: Option<StreamedResponse>,
        capture_metrics: &CapturedFlowMetrics,
    ) -> Result<(), ProxyError> {
        self.emit(ProxyEvent::Captured {
            project: self.project,
            record,
            response_stream,
            listener_port: (self.listener_port != 0).then_some(self.listener_port),
            capture_metrics: capture_metrics.clone(),
        })
        .await
    }

    async fn emit_backup_revision(
        &self,
        mut backup: FlowRecord,
        record: &mut FlowRecord,
        capture_metrics: &CapturedFlowMetrics,
    ) -> Result<(), ProxyError> {
        backup.summary.revision += 1;
        record.summary.revision = backup.summary.revision;
        self.emit_captured(backup, None, capture_metrics).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_flow_transform(
        &self,
        transform: &FlowTransform,
        record: &mut FlowRecord,
        response_stage: bool,
        response_head_only: bool,
        is_ws: bool,
        capture_metrics: &CapturedFlowMetrics,
        transformed: &mut bool,
        request_modified: &mut bool,
        original_request: &mut Option<HttpRequest>,
    ) -> Result<(), ProxyError> {
        let deadline = Instant::now()
            .checked_add(self.config.limits.request_timeout)
            .ok_or(ProxyError::Timeout)?;
        let backup = record.clone();
        let transformed_result = transform(
            record,
            response_stage,
            response_head_only,
            &self.cancel,
            (deadline, self.config.limits.max_body_bytes),
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
        let accepted = if !transform_applied {
            true
        } else if !response_stage {
            self.validate_intercepted_request(&record.request, is_ws)
                .is_ok()
        } else if let Some(response) = &record.response {
            self.validate_intercepted_response(response, response_head_only)
                .is_ok()
        } else {
            false
        };
        if !accepted {
            *record = backup;
        } else if !response_stage && backup.request != record.request {
            *transformed = transform_applied;
            *request_modified = true;
            *original_request = Some(backup.request);
        } else {
            *transformed = transform_applied;
            if response_stage && !response_head_only && backup.response != record.response {
                self.emit_backup_revision(backup, record, capture_metrics)
                    .await?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_async_flow_hook(
        &self,
        hook: &AsyncFlowHook,
        record: &mut FlowRecord,
        response_stage: bool,
        response_head_only: bool,
        is_ws: bool,
        capture_metrics: &CapturedFlowMetrics,
        transformed: &mut bool,
        request_modified: &mut bool,
        original_request: &mut Option<HttpRequest>,
        force_intercept: &mut bool,
    ) -> Result<(), ProxyError> {
        let deadline = Instant::now()
            .checked_add(self.config.limits.request_timeout)
            .ok_or(ProxyError::Timeout)?;
        let tokio_deadline = tokio::time::Instant::from_std(deadline);
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
                self.validate_intercepted_request(&request, is_ws)?;
                if record.request != request {
                    *transformed = true;
                    *request_modified = true;
                    if original_request.is_none() {
                        *original_request = Some(record.request.clone());
                    }
                    record.summary.method = request.method.clone();
                    record.summary.url = request.url.clone();
                    record.summary.request_bytes = request.body.len() as u64;
                    record.request = request;
                }
                *force_intercept = force_intercept_decision;
            }
            FlowHookDecision::ModifyResponse(response)
            | FlowHookDecision::ForceIntercept {
                request: None,
                response: Some(response),
            } if response_stage => {
                self.validate_intercepted_response(&response, response_head_only)?;
                if record.response.as_ref() != Some(&response) {
                    if !response_head_only && !*transformed && record.response.is_some() {
                        let backup = record.clone();
                        self.emit_backup_revision(backup, record, capture_metrics)
                            .await?;
                    }
                    *transformed = true;
                    record.summary.status = Some(response.status);
                    record.summary.response_bytes = response.body.len() as u64;
                    record.response = Some(response);
                }
                *force_intercept = force_intercept_decision;
            }
            FlowHookDecision::ForceIntercept {
                request: None,
                response: None,
            } => *force_intercept = true,
            _ => return Err(ProxyError::InterceptStage),
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn evaluate_intercept_filter(
        &self,
        filter: &InterceptFilter,
        record: &FlowRecord,
        response_stage: bool,
        response_head_only: bool,
        capture_metrics: &CapturedFlowMetrics,
        request_was_intercepted: bool,
        request_was_modified: bool,
    ) -> Result<bool, ProxyError> {
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let was_annotated = self
            .config
            .annotation_checker
            .as_ref()
            .is_some_and(|checker| checker(record.summary.id));
        let local_ip =
            self.resolve_intercept_target_ip_local(&record.request.url, capture_metrics);
        let run_filter = |target_ip: Option<&str>| -> Result<InterceptFilterOutcome, ProxyError> {
            let deadline = Instant::now()
                .checked_add(self.config.limits.request_timeout)
                .ok_or(ProxyError::Timeout)?;
            let ctx = InterceptEvalContext {
                listener_port: self.listener_port,
                target_ip,
                was_annotated,
            };
            let result = filter(
                record,
                response_stage,
                response_head_only,
                request_was_intercepted,
                request_was_modified,
                ctx,
                &self.cancel,
                (deadline, self.config.limits.max_body_bytes),
            );
            if self.cancel.is_cancelled() {
                return Err(ProxyError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ProxyError::Timeout);
            }
            result
        };
        let outcome = match local_ip {
            LocalTargetIp::Resolved(resolved_ip) => run_filter(resolved_ip.as_deref())?,
            LocalTargetIp::NeedsDns { host, port } => match run_filter(None)? {
                InterceptFilterOutcome::NeedsTargetIp => {
                    if self.cancel.is_cancelled() {
                        return Err(ProxyError::Cancelled);
                    }
                    let resolved_ip = self.lookup_intercept_target_ip(&host, port).await;
                    if self.cancel.is_cancelled() {
                        return Err(ProxyError::Cancelled);
                    }
                    let Some(ip) = resolved_ip else {
                        return Err(ProxyError::invalid(
                            "目标 IP 尚未确定；未按负向条件自动放行",
                        ));
                    };
                    store_cached_dns_ip(host.to_ascii_lowercase(), ip.clone());
                    run_filter(Some(ip.as_str()))?
                }
                other => other,
            },
        };
        match outcome {
            InterceptFilterOutcome::Intercept => Ok(true),
            InterceptFilterOutcome::Bypass => Ok(false),
            InterceptFilterOutcome::NeedsTargetIp => Err(ProxyError::invalid(
                "目标 IP 尚未确定；未按负向条件自动放行",
            )),
        }
    }

    async fn wait_for_intercept_decision(
        &self,
        dynamic_rx: &mut watch::Receiver<DynamicProxyState>,
        response_stage: bool,
        force_intercept: bool,
        mut receive: oneshot::Receiver<InterceptDecision>,
    ) -> Result<Option<InterceptDecision>, ProxyError> {
        let enabled = |state: &DynamicProxyState| {
            if response_stage {
                state.intercept.responses
            } else {
                state.intercept.requests
            }
        };
        let wait = async {
            loop {
                if self.cancel.is_cancelled() {
                    return Err(ProxyError::Cancelled);
                }
                if !force_intercept && !enabled(&dynamic_rx.borrow_and_update()) {
                    return Ok(None);
                }
                tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => return Err(ProxyError::Cancelled),
                    changed = dynamic_rx.changed() => {
                        changed.map_err(|_| ProxyError::Cancelled)?;
                    }
                    decision = &mut receive => {
                        return decision
                            .map(Some)
                            .map_err(|_| ProxyError::EventConsumerClosed);
                    }
                }
            }
        };
        tokio::time::timeout(self.config.limits.intercept_timeout, wait)
            .await
            .map_err(|_| ProxyError::Timeout)?
    }

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
        let is_ws = is_websocket_request(&record.request);
        let mut transformed = false;
        let mut original_request = None;
        let mut request_modified = false;
        let mut force_intercept = false;
        let (flow_transform, filter, async_flow_hook) = {
            let dyn_state = self.dynamic.borrow();
            (
                dyn_state.flow_transform.clone(),
                dyn_state.intercept_filter.clone(),
                dyn_state.async_flow_hook.clone(),
            )
        };
        if let Some(transform) = flow_transform {
            self.apply_flow_transform(
                &transform,
                record,
                response_stage,
                response_head_only,
                is_ws,
                capture_metrics,
                &mut transformed,
                &mut request_modified,
                &mut original_request,
            )
            .await?;
        }
        if let Some(hook) = async_flow_hook
            && !matches!(record.summary.source, TrafficSource::Plugin(_))
        {
            self.apply_async_flow_hook(
                &hook,
                record,
                response_stage,
                response_head_only,
                is_ws,
                capture_metrics,
                &mut transformed,
                &mut request_modified,
                &mut original_request,
                &mut force_intercept,
            )
            .await?;
        }
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let mut dynamic_rx = self.dynamic.clone();
        let intercept_enabled = {
            let state = dynamic_rx.borrow_and_update();
            if response_stage {
                state.intercept.responses
            } else {
                state.intercept.requests
            }
        };
        let should_intercept = if force_intercept {
            true
        } else if !intercept_enabled {
            false
        } else if let Some(filter) = filter {
            self.evaluate_intercept_filter(
                &filter,
                record,
                response_stage,
                response_head_only,
                capture_metrics,
                request_was_intercepted,
                request_was_modified,
            )
            .await?
        } else {
            true
        };
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        if !should_intercept {
            if transformed {
                record.summary.state = FlowState::Pending;
                record.summary.revision += 1;
                self.emit_captured(record.clone(), response_stream.cloned(), capture_metrics)
                    .await?;
            }
            return Ok((false, request_modified));
        }
        record.summary.state = FlowState::Intercepted;
        record.summary.revision += 1;
        let (reply, receive) = oneshot::channel();
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
        let decision = self
            .wait_for_intercept_decision(&mut dynamic_rx, response_stage, force_intercept, receive)
            .await?;
        if self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        match decision {
            Some(InterceptDecision::ForwardRequest(request)) if !response_stage => {
                self.validate_intercepted_request(&request, is_ws)?;
                request_modified = original_request.as_ref().unwrap_or(&record.request) != &request;
                record.summary.method = request.method.clone();
                record.summary.url = request.url.clone();
                record.summary.request_bytes = request.body.len() as u64;
                record.request = request;
            }
            Some(InterceptDecision::ForwardResponse(response)) if response_stage => {
                self.validate_intercepted_response(&response, response_head_only)?;
                if !response_head_only
                    && !transformed
                    && record.response.as_ref() != Some(&response)
                {
                    let mut backup = record.clone();
                    backup.summary.state = FlowState::Pending;
                    self.emit_backup_revision(backup, record, capture_metrics)
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
        self.emit_captured(record.clone(), response_stream.cloned(), capture_metrics)
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
        let host = crate::route::strip_host_brackets(&host_raw);
        if let Ok(ip) = host.parse::<IpAddr>() {
            return LocalTargetIp::Resolved(Some(ip.to_string()));
        }
        let route = self.resolve_upstream_route_for_host(host);
        if !route.is_direct_or_local_dns() {
            return LocalTargetIp::Resolved(None);
        }
        if let Some(ip) = crate::transport::resolve_hostname_override(
            &self.config.network.hostname_overrides,
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
        let lookup_timeout = self
            .config
            .limits
            .request_timeout
            .min(Duration::from_secs(2));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intercept_dns_cache_evicts_oldest_entry_instead_of_clearing_all() {
        let now = Instant::now();
        let mut cache = VecDeque::new();
        for i in 0..INTERCEPT_DNS_CACHE_LIMIT {
            cache.push_back((format!("host-{i}.example.invalid"), now, format!("10.0.0.{}", i % 250)));
        }
        assert_eq!(cache.len(), INTERCEPT_DNS_CACHE_LIMIT);
        // Simulate inserting one more entry using the same LRU eviction rule.
        while cache.len() >= INTERCEPT_DNS_CACHE_LIMIT {
            cache.pop_front();
        }
        cache.push_back(("newest.example.invalid".into(), now, "10.0.1.1".into()));
        assert_eq!(cache.len(), INTERCEPT_DNS_CACHE_LIMIT);
        assert!(!cache.iter().any(|(h, _, _)| h == "host-0.example.invalid"));
        assert!(cache.iter().any(|(h, _, _)| h == "host-1.example.invalid"));
        assert!(cache.iter().any(|(h, _, _)| h == "newest.example.invalid"));
    }
}
