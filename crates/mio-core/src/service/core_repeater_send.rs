use super::*;

pub(in crate::service) fn spawn_unpack_response(
    response: HttpResponse,
    method: String,
    max_body_bytes: usize,
    cancel: CancellationToken,
    deadline: Instant,
) -> tokio::task::JoinHandle<std::result::Result<Option<HttpResponse>, http::UnpackError>> {
    tokio::task::spawn_blocking(move || {
        http::unpack_response(&response, &method, max_body_bytes, &cancel, deadline)
    })
}

pub(in crate::service) async fn unpack_encoded_response_within_timeout(
    request: &HttpRequest,
    config: &SendConfig,
    unpack: bool,
    response: &HttpResponse,
    cancel: &CancellationToken,
) -> Option<HttpResponse> {
    let encoded = response
        .headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("content-encoding"));
    if !unpack || !encoded {
        return None;
    }
    let worker = spawn_unpack_response(
        response.clone(),
        request.method.clone(),
        config.limits.max_body_bytes,
        cancel.clone(),
        Instant::now() + config.limits.timeout,
    );
    match tokio::time::timeout_at(
        tokio::time::Instant::now() + config.limits.timeout,
        worker,
    )
    .await
    {
        Ok(Ok(Ok(Some(unpacked)))) => Some(unpacked),
        _ => None,
    }
}

pub(in crate::service) struct RepeaterSendRuntimeConfig {
    pub(in crate::service) upstream_tls: mio_proxy::UpstreamTls,
    pub(in crate::service) upstream_tls_options: mio_proxy::UpstreamTlsOptions,
    pub(in crate::service) upstream_route: mio_proxy::UpstreamRoute,
    pub(in crate::service) route_resolver: mio_proxy::UpstreamRouteResolver,
    pub(in crate::service) hostname_overrides: Arc<[(String, std::net::IpAddr)]>,
    pub(in crate::service) client_certificates:
        Arc<[(ClientCertificateRule, mio_proxy::ParsedClientIdentity)]>,
    pub(in crate::service) streaming_timeout: Option<Duration>,
    pub(in crate::service) request_timeout: Duration,
    pub(in crate::service) strip_100_continue_headers: bool,
    pub(in crate::service) default_to_http2: bool,
    pub(in crate::service) platform_auth_enabled: bool,
    pub(in crate::service) platform_auth_rules: Arc<[mio_proxy::PlatformAuthRuleConfig]>,
    pub(in crate::service) upstream_auth_rules: Arc<[mio_proxy::UpstreamAuthRuleConfig]>,
}

impl RepeaterSendRuntimeConfig {
    pub(in crate::service) fn build_send_config(
        &self,
        target_url: Option<&str>,
        timeout: Duration,
        version: Option<HttpVersion>,
    ) -> SendConfig {
        let route = target_url
            .and_then(|raw| url::Url::parse(raw).ok())
            .and_then(|parsed| parsed.host_str().map(|host| (self.route_resolver)(host)))
            .unwrap_or_else(|| self.upstream_route.clone());
        let mut config = SendConfig::default();
        config.network.upstream_route = route;
        config.network.hostname_overrides = self.hostname_overrides.clone();
        config.tls.trust = self.upstream_tls.clone();
        config.tls.options = self.upstream_tls_options.clone();
        config.tls.client_certificates = self.client_certificates.clone();
        config.limits.timeout = timeout;
        config.limits.streaming_timeout = self.streaming_timeout;
        config.http.version = version;
        config.http.strip_100_continue_headers = self.strip_100_continue_headers;
        config.http.default_to_http2 = self.default_to_http2;
        config.auth.platform_enabled = self.platform_auth_enabled;
        config.auth.platform_rules = self.platform_auth_rules.clone();
        config.auth.upstream_rules = self.upstream_auth_rules.clone();
        config
    }
}

pub(in crate::service) fn apply_repeater_protocol_options(
    run_id: FlowId,
    base_request: &HttpRequest,
    request: &mut HttpRequest,
    settings: &RepeaterSettings,
    version: HttpVersion,
    provenance: &ProvenanceTracker,
) -> bool {
    let before = request.clone();
    settings.apply_protocol_options(request, version);
    let mutated = *request != before;
    if mutated {
        provenance.record_request(
            run_id,
            base_request,
            ProvenanceActor::ProtocolNormalization {
                description: "Repeater 协议选项".into(),
            },
            request,
        );
    }
    mutated
}

pub(in crate::service) fn pending_repeater_flow_record(
    run_id: FlowId,
    request: HttpRequest,
    started_unix_ms: i64,
) -> FlowRecord {
    FlowRecord {
        summary: FlowSummary {
            id: run_id,
            revision: 1,
            source: TrafficSource::Repeater,
            method: request.method.clone(),
            url: request.url.clone(),
            status: None,
            state: FlowState::Pending,
            request_bytes: request.body.len() as u64,
            response_bytes: 0,
            started_unix_ms,
            elapsed_ms: 0,
        },
        request,
        response: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::service) async fn apply_repeater_session_rules(
    sessions: &crate::sessions::SessionRuntime,
    tool: SessionTool,
    provenance: &ProvenanceTracker,
    run_id: FlowId,
    base_request: &HttpRequest,
    record: &mut FlowRecord,
    settings: &RepeaterSettings,
    protocol_version: Option<HttpVersion>,
    record_post_session_norm: bool,
    macro_send_config: &SendConfig,
    cancel: &CancellationToken,
    deadline: Instant,
    started: Instant,
) -> Option<bool> {
    let (changed, steps) = sessions
        .apply_to_flow_record_with_steps(
            tool,
            record,
            macro_send_config,
            cancel,
            deadline,
            started,
        )
        .await?;
    for (rule_name, snap) in steps {
        provenance.record_request(
            run_id,
            base_request,
            ProvenanceActor::SessionRule { rule_name },
            &snap,
        );
    }
    if changed {
        let ver = protocol_version.unwrap_or(record.request.version);
        if record_post_session_norm {
            apply_repeater_protocol_options(
                run_id,
                base_request,
                &mut record.request,
                settings,
                ver,
                provenance,
            );
        } else {
            settings.apply_protocol_options(&mut record.request, ver);
        }
        record.summary.method = record.request.method.clone();
        record.summary.url = record.request.url.clone();
        record.summary.request_bytes = record.request.body.len() as u64;
    }
    Some(changed)
}

#[allow(clippy::too_many_arguments)]
fn resolve_repeater_redirect_hop(
    current_origin: &str,
    current_request: &HttpRequest,
    response: &HttpResponse,
    follow_redirects: RepeaterRedirectMode,
    project_scope: &TargetScope,
    network_http: &NetworkHttpSettings,
    process_cookies_in_redirects: bool,
    cross_domain_protocol_redirects: bool,
) -> Result<Option<(String, HttpRequest, Vec<u8>)>> {
    if follow_redirects == RepeaterRedirectMode::InScopeOnly {
        http::resolve_redirect_request_with_options(
            current_origin,
            current_request,
            response,
            RepeaterRedirectMode::Always,
            network_http,
            process_cookies_in_redirects,
            cross_domain_protocol_redirects,
        )
        .map(|opt| {
            opt.filter(|(_, next_request, _)| project_scope.is_url_in_scope(&next_request.url))
        })
    } else {
        http::resolve_redirect_request_with_options(
            current_origin,
            current_request,
            response,
            follow_redirects,
            network_http,
            process_cookies_in_redirects,
            cross_domain_protocol_redirects,
        )
    }
}

pub(super) async fn finish_repeater_unpack(
    record: &mut FlowRecord,
    worker: tokio::task::JoinHandle<std::result::Result<Option<HttpResponse>, http::UnpackError>>,
) -> Option<Box<FlowRecord>> {
    // The send job owns this handle until the codec exits, including cancellation.
    match worker.await {
        Ok(Ok(Some(response))) => {
            let received = Box::new(record.clone());
            record.summary.revision += 1;
            record.summary.response_bytes = response.body.len() as u64;
            record.response = Some(response);
            Some(received)
        }
        Ok(Ok(None)) => None,
        Ok(Err(http::UnpackError::Cancelled)) => {
            record.summary.state = FlowState::Cancelled;
            None
        }
        Ok(Err(http::UnpackError::Deadline)) => {
            record.summary.state =
                FlowState::Failed("Repeater 响应解压超时；已保留完整收到的原文".into());
            None
        }
        Ok(Err(http::UnpackError::Failed(_))) => {
            record.summary.state =
                FlowState::Failed("Repeater 响应解压失败；已保留完整收到的原文".into());
            None
        }
        Err(_) => {
            record.summary.state =
                FlowState::Failed("Repeater 响应解压任务异常退出；已保留完整收到的原文".into());
            None
        }
    }
}

impl State {
    pub(in crate::service) fn build_repeater_send_runtime_config(
        &mut self,
        workspace: &Workspace,
        settings: &RepeaterSettings,
        timeout_max_secs: u64,
        force_insecure_tls: bool,
        use_settings_streaming_fallback: bool,
        sync_cache: bool,
    ) -> Result<RepeaterSendRuntimeConfig> {
        let upstream_tls = if force_insecure_tls {
            mio_proxy::UpstreamTls::Insecure
        } else {
            self.preferences
                .effective_upstream_tls(&workspace.upstream_tls)?
        };
        let upstream_tls_options = workspace.upstream_tls.to_proxy_tls_options();
        let upstream_settings = workspace
            .upstream_proxy
            .effective_for_runtime(&self.user_level_connections_cache);
        if sync_cache {
            upstream_settings.sync_user_level_cache(&mut self.user_level_connections_cache);
        }
        let route_resolver = upstream_settings.build_route_resolver(upstream_tls.clone())?;
        let hostname_overrides = upstream_settings.compiled_hostname_overrides()?;
        let upstream_route = upstream_settings.to_route_with_tls(upstream_tls.clone())?;
        let streaming_timeout = upstream_settings.effective_streaming_timeout().or_else(|| {
            use_settings_streaming_fallback.then(|| {
                if settings.streaming_response_timeout_secs == 0 {
                    Duration::ZERO
                } else {
                    settings
                        .effective_streaming_timeout()
                        .unwrap_or(Duration::ZERO)
                }
            })
        });
        let request_timeout =
            Duration::from_secs(settings.request_timeout_secs.clamp(1, timeout_max_secs));
        Ok(RepeaterSendRuntimeConfig {
            upstream_tls,
            upstream_tls_options,
            upstream_route,
            route_resolver,
            hostname_overrides,
            client_certificates: self.tls.compiled_client_certs.clone(),
            streaming_timeout,
            request_timeout,
            strip_100_continue_headers: workspace.network_http.remove_100_continue_headers,
            default_to_http2: workspace.network_http.default_to_http2,
            platform_auth_enabled: upstream_settings.platform_auth.enabled,
            platform_auth_rules: upstream_settings.compiled_platform_auth_rules(),
            upstream_auth_rules: upstream_settings.compiled_upstream_auth_rules(),
        })
    }

    pub(super) async fn open_repeater(&mut self, request: Option<&HttpRequest>) -> Result<()> {
        let tab = WorkTab::from_request(request, "1".into());
        self.emit(Event::RepeaterOpened {
            project: self.project,
            tab,
        })
        .await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn begin_send(
        &mut self,
        project: ProjectKey,
        tab_id: TabId,
        run_id: FlowId,
        draft: Draft,
        settings: RepeaterSettings,
        group_ticket: Option<crate::RepeaterGroupSendTicket>,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(!self.quiesced, "正在保存并切换项目；未发送请求");
        ensure!(!cancel.is_cancelled(), "Repeater 已取消；未发送请求");
        ensure!(
            !self.repeater.sends.contains_key(&tab_id),
            "此标签已有在途请求；不会重复发送"
        );
        ensure!(
            self.repeater.sends.len() < MAX_REPEATER_SENDS,
            "并发 Repeater 请求达到 64 个限额"
        );
        ensure!(
            self.store.flows().read_optional(run_id)?.is_none(),
            "Repeater run ID 已存在；未重发请求"
        );
        let workspace = self.store.metadata().workspace()?;
        let tab = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .context("工作标签已关闭")?;
        ensure!(
            tab.draft == draft,
            "发送草稿与当前已保存 revision 不一致；未发送旧稿"
        );
        let update_content_length = settings.update_content_length;
        let normalize_http1_line_endings = settings.normalize_http1_line_endings;
        let follow_redirects = settings.follow_redirects;
        let project_scope = workspace.scope.clone();
        let network_http = workspace.network_http.clone();
        let runtime_cfg = self.build_repeater_send_runtime_config(
            &workspace,
            &settings,
            3600,
            false,
            true,
            true,
        )?;
        let group_mode = group_ticket
            .as_ref()
            .map_or(crate::RepeaterGroupSendMode::Default, |t| t.mode);
        let connection_pool_group_id = group_ticket
            .as_ref()
            .and_then(|t| t.connection_pool_group_id);
        let mut single_packet_barrier = group_ticket
            .and_then(|t| t.single_packet_barrier);
        let mut request = http::parse_repeater_request(
            &draft.raw,
            &draft.target,
            draft.version,
            update_content_length,
        )?;
        ensure!(
            !project_scope.should_drop_url(&request.url),
            "目标 URL 属于范围外且已开启自动丢弃范围外请求；未发送请求"
        );
        let parsed_draft_request = request.clone();
        let mut effective_proto_settings = settings.clone();
        if group_mode == crate::RepeaterGroupSendMode::SequentialSingleConnection {
            effective_proto_settings.enforce_http1_connection_close = false;
        }
        let protocol_mutated = apply_repeater_protocol_options(
            run_id,
            &parsed_draft_request,
            &mut request,
            &effective_proto_settings,
            draft.version,
            &self.storage.provenance,
        );
        let max_redirect_hops = settings.effective_max_redirect_hops();
        let (http1_keep_alive, http2_reuse, use_connection_pool) = match group_mode {
            crate::RepeaterGroupSendMode::SequentialSingleConnection => (true, true, true),
            crate::RepeaterGroupSendMode::SequentialSeparateConnections
            | crate::RepeaterGroupSendMode::ParallelSeparateConnections => (false, false, false),
            crate::RepeaterGroupSendMode::ParallelSinglePacketAttack => {
                let is_h2 = draft.version == HttpVersion::Http2;
                (false, is_h2, is_h2)
            }
            crate::RepeaterGroupSendMode::Default => {
                let h1 =
                    settings.http1_connection_reuse && !settings.enforce_http1_connection_close;
                let h2 = settings.http2_connection_reuse;
                (h1, h2, h1 || h2)
            }
        };
        let allow_http2_alpn_override = settings.allow_http2_alpn_override;
        let process_cookies_in_redirects = settings.process_cookies_in_redirects;
        let cross_domain_protocol_redirects = settings.cross_domain_protocol_redirects;
        let cert_callback = self.build_server_cert_callback(project);
        let session_hooks = self.strict_session_action_hook_entries();
        let http_hooks = self.tool_http_hook_entries();
        let hook_scope = self
            .store
            .metadata()
            .workspace_arc()
            .map(|ws| ws.scope.clone())
            .unwrap_or_default();
        let plugin_events = self.events.clone();
        let mut record =
            pending_repeater_flow_record(run_id, request.clone(), crate::codec_util::now_unix_ms());
        if let Err(error) = self
            .store
            .repeater_history_mut()
            .begin_send(tab_id, &draft, &record, &cancel)
        {
            self.storage.recovery.push_back(record.clone().into());
            self.storage_failed(&error);
            return Err(error);
        }
        let task_cancel = cancel.clone();
        let flow_id = record.summary.id;
        let draft_revision = draft.revision;
        let unpack_compressed_responses = settings.unpack_compressed_responses;
        let sessions = self.sessions.runtime.clone();
        let provenance = self.storage.provenance.clone();
        let (progress_tx, progress_rx) = watch::channel(None);
        let cancel_for_hooks = task_cancel.clone();
        let job = self.repeater.jobs.spawn(async move {
            let started = Instant::now();
            let default_config = SendConfig::default();
            let deadline = started + runtime_cfg.request_timeout;
            let max_body_bytes = default_config.limits.max_body_bytes;
            let mut current_origin = draft.target.clone();
            let mut current_request = request;
            let macro_send_config = runtime_cfg.build_send_config(
                None,
                deadline.saturating_duration_since(Instant::now()),
                None,
            );
            let mut request_mutated = match apply_repeater_session_rules(
                &sessions,
                SessionTool::Repeater,
                &provenance,
                flow_id,
                &parsed_draft_request,
                &mut record,
                &effective_proto_settings,
                Some(draft.version),
                false,
                &macro_send_config,
                &cancel_for_hooks,
                deadline,
                started,
            )
            .await
            {
                Some(changed) => {
                    if changed {
                        current_request = record.request.clone();
                    }
                    changed || protocol_mutated
                }
                None => {
                    return Completion {
                        project,
                        tab_id,
                        draft_revision: draft.revision,
                        received: None,
                        record,
                    };
                }
            };
            if !session_hooks.is_empty() || !http_hooks.is_empty() {
                let mut current = current_request.clone();
                match plugin_service::hooks::run_pre_send_plugin_hooks_configured(
                    &session_hooks,
                    &http_hooks,
                    Some(&hook_scope),
                    Some(&plugin_events),
                    flow_id,
                    protocol::SourceDto::Repeater,
                    &mut current,
                    Some((&provenance, &parsed_draft_request)),
                    deadline,
                    &cancel_for_hooks,
                )
                .await
                {
                    plugin_service::hooks::PreSendHookOutcome::Proceed { mutated } => {
                        if mutated {
                            request_mutated = true;
                        }
                        effective_proto_settings
                            .apply_protocol_options(&mut current, draft.version);
                        current_request = current;
                        plugin_service::hooks::sync_flow_request(
                            &mut record,
                            current_request.clone(),
                        );
                    }
                    plugin_service::hooks::PreSendHookOutcome::Dropped => {
                        plugin_service::hooks::sync_flow_request(&mut record, current);
                        record.summary.state = FlowState::Dropped;
                        record.summary.revision += 1;
                        record.summary.elapsed_ms =
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        return Completion {
                            project,
                            tab_id,
                            draft_revision: draft.revision,
                            received: None,
                            record,
                        };
                    }
                    plugin_service::hooks::PreSendHookOutcome::Failed(message) => {
                        plugin_service::hooks::sync_flow_request(&mut record, current);
                        record.summary.state = if cancel_for_hooks.is_cancelled() {
                            FlowState::Cancelled
                        } else {
                            FlowState::Failed(message)
                        };
                        record.summary.revision += 1;
                        record.summary.elapsed_ms =
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        return Completion {
                            project,
                            tab_id,
                            draft_revision: draft.revision,
                            received: None,
                            record,
                        };
                    }
                }
            }
            let mut first_raw_override = (!request_mutated
                && !normalize_http1_line_endings
                && draft.version != HttpVersion::Http2)
                .then(|| std::sync::Arc::<[u8]>::from(draft.raw.as_slice()));
            if group_mode == crate::RepeaterGroupSendMode::ParallelSinglePacketAttack
                && first_raw_override.is_none()
                && current_request.version != HttpVersion::Http2
            {
                first_raw_override =
                    Some(std::sync::Arc::<[u8]>::from(http::request_to_raw(&current_request)));
            }
            // send cancels its internal child on success; decoding uses the parent token.
            let mut final_response = None;
            let mut last_hop_response: Option<HttpResponse> = None;
            for hop in 0..=max_redirect_hops {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    record.summary.revision += 1;
                    if let Some(prev) = last_hop_response.take() {
                        record.summary.status = Some(prev.status);
                        record.summary.response_bytes = prev.body.len() as u64;
                        record.response = Some(prev);
                    }
                    record.summary.state =
                        FlowState::Failed(mio_proxy::ProxyError::Timeout.to_string());
                    break;
                }
                let force_streaming_response =
                    network_http.is_url_in_streaming_scope(&current_request.url);
                let mut config = runtime_cfg.build_send_config(
                    Some(&current_request.url),
                    remaining,
                    Some(current_request.version),
                );
                config.tls.on_server_cert_observed = Some(cert_callback.clone());
                config.http.update_content_length = update_content_length;
                config.http.normalize_http1_line_endings = if hop == 0 {
                    normalize_http1_line_endings
                } else {
                    true
                };
                config.http.raw_http1_override = if hop == 0 {
                    first_raw_override.take()
                } else {
                    None
                };
                config.http.http1_keep_alive = http1_keep_alive;
                config.http.http2_reuse = http2_reuse;
                config.http.use_connection_pool = use_connection_pool;
                config.http.allow_http2_alpn_override = allow_http2_alpn_override;
                config.http.force_streaming_response = force_streaming_response;
                config.group.connection_pool_group_id = connection_pool_group_id;
                config.group.single_packet_barrier = if hop == 0 {
                    single_packet_barrier.take()
                } else {
                    None
                };
                const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(50);
                let mut last_progress_at: Option<Instant> = None;
                let mut emitted_nonempty_body = false;
                let hop_result = mio_proxy::send_streaming(
                    current_request.clone(),
                    config,
                    task_cancel.clone(),
                    |resp| {
                        let will_redirect = follow_redirects != RepeaterRedirectMode::Never
                            && hop < max_redirect_hops
                            && resolve_repeater_redirect_hop(
                                &current_origin,
                                &current_request,
                                resp,
                                follow_redirects,
                                &project_scope,
                                &network_http,
                                process_cookies_in_redirects,
                                cross_domain_protocol_redirects,
                            )
                            .is_ok_and(|opt| opt.is_some());
                        if !will_redirect {
                            let now = Instant::now();
                            let is_head = resp.body.is_empty() && last_progress_at.is_none();
                            let is_first_chunk = !resp.body.is_empty() && !emitted_nonempty_body;
                            let has_trailers = !resp.trailers.is_empty();
                            let interval_due = last_progress_at.is_none_or(|prev| {
                                now.saturating_duration_since(prev) >= PROGRESS_MIN_INTERVAL
                            });
                            if is_head || is_first_chunk || has_trailers || interval_due {
                                if !resp.body.is_empty() {
                                    emitted_nonempty_body = true;
                                }
                                last_progress_at = Some(now);
                                let elapsed_ms =
                                    started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                                let _ = progress_tx.send(Some((resp.clone(), elapsed_ms)));
                            }
                        }
                    },
                )
                .await;
                match hop_result {
                    mio_proxy::SendStreamOutcome {
                        response: Some(response),
                        error: None,
                    } => {
                        sessions.observe_response(
                            SessionTool::Repeater,
                            &current_request.url,
                            &response,
                        );
                        if follow_redirects != RepeaterRedirectMode::Never
                            && hop < max_redirect_hops
                            && let Ok(Some((next_origin, next_request, _))) =
                                resolve_repeater_redirect_hop(
                                    &current_origin,
                                    &current_request,
                                    &response,
                                    follow_redirects,
                                    &project_scope,
                                    &network_http,
                                    process_cookies_in_redirects,
                                    cross_domain_protocol_redirects,
                                )
                        {
                            last_hop_response = Some(response);
                            current_origin = next_origin;
                            current_request = next_request;
                            continue;
                        }
                        final_response = Some(response);
                        break;
                    }
                    mio_proxy::SendStreamOutcome {
                        response: maybe_response,
                        error: Some(error),
                    } => {
                        record.summary.revision += 1;
                        if let Some(response) = maybe_response.or_else(|| last_hop_response.take())
                        {
                            record.summary.status = Some(response.status);
                            record.summary.response_bytes = response.body.len() as u64;
                            record.response = Some(response);
                        }
                        record.summary.state = match error {
                            mio_proxy::ProxyError::Cancelled => FlowState::Cancelled,
                            error => FlowState::Failed(error.to_string()),
                        };
                        break;
                    }
                    mio_proxy::SendStreamOutcome {
                        response: None,
                        error: None,
                    } => {
                        record.summary.revision += 1;
                        if let Some(prev) = last_hop_response.take() {
                            record.summary.status = Some(prev.status);
                            record.summary.response_bytes = prev.body.len() as u64;
                            record.response = Some(prev);
                        }
                        record.summary.state =
                            FlowState::Failed(mio_proxy::ProxyError::Timeout.to_string());
                        break;
                    }
                }
            }
            record.summary.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            let mut received = None;
            if let Some(response) = final_response {
                record.summary.revision += 1;
                record.summary.status = Some(response.status);
                record.summary.response_bytes = response.body.len() as u64;
                record.summary.state = FlowState::Complete;
                record.response = Some(response);
                if unpack_compressed_responses {
                    let worker = spawn_unpack_response(
                        record.response.as_ref().unwrap().clone(),
                        current_request.method.clone(),
                        max_body_bytes,
                        task_cancel,
                        deadline,
                    );
                    received = finish_repeater_unpack(&mut record, worker).await;
                    if let (Some(orig_rec), Some(unpacked_resp)) = (&received, &record.response)
                        && let Some(orig_resp) = &orig_rec.response
                        && orig_resp != unpacked_resp
                    {
                        provenance.record_response(
                            flow_id,
                            orig_resp,
                            ProvenanceActor::UnpackCompressed,
                            unpacked_resp,
                        );
                    }
                }
            }
            record.summary.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            Completion {
                project,
                tab_id,
                draft_revision: draft.revision,
                received,
                record,
            }
        });
        self.repeater.sends.insert(
            tab_id,
            ActiveSend {
                cancel,
                task_id: job.id(),
                flow_id,
                draft_revision,
                progress_rx: Some(progress_rx),
            },
        );
        self.emit(Event::SendStarted {
            project,
            tab_id,
            run_id,
            draft_revision: tab.draft.revision,
        })
        .await;
        self.history_changed().await;
        Ok(())
    }
}
