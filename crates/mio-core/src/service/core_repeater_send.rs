use super::*;

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
    pub(super) async fn open_repeater(&mut self, request: Option<&HttpRequest>) -> Result<()> {
        let tab = WorkTab::from_request(request, "1".into());
        self.emit(Event::RepeaterOpened {
            project: self.project,
            tab,
        })
        .await;
        Ok(())
    }

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
            !self.sends.contains_key(&tab_id),
            "此标签已有在途请求；不会重复发送"
        );
        ensure!(
            self.sends.len() < MAX_REPEATER_SENDS,
            "并发 Repeater 请求达到 16 个限额"
        );
        ensure!(
            self.store.read_optional(run_id)?.is_none(),
            "Repeater run ID 已存在；未重发请求"
        );
        let workspace = self.store.workspace()?;
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
        let upstream_tls = self
            .preferences
            .effective_upstream_tls(&workspace.upstream_tls)?;
        let upstream_tls_options = workspace.upstream_tls.to_proxy_tls_options();
        workspace.upstream_proxy.sync_user_level_cache();
        let upstream_settings = workspace.upstream_proxy.effective_for_runtime();
        let route_resolver = upstream_settings.build_route_resolver(upstream_tls.clone())?;
        let hostname_overrides = upstream_settings.compiled_hostname_overrides()?;
        let platform_auth_enabled = upstream_settings.platform_auth.enabled;
        let platform_auth_rules = upstream_settings.compiled_platform_auth_rules();
        let upstream_auth_rules = upstream_settings.compiled_upstream_auth_rules();
        let streaming_timeout = upstream_settings
            .effective_streaming_timeout()
            .or_else(|| {
                if settings.streaming_response_timeout_secs == 0 {
                    Some(Duration::ZERO)
                } else {
                    settings.effective_streaming_timeout()
                }
            });
        let upstream_route = upstream_settings
            .to_route_with_tls(upstream_tls.clone())?;
        let request_timeout = Duration::from_secs(settings.request_timeout_secs.clamp(1, 3600));
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
        let headers_before = request.headers.clone();
        let mut effective_proto_settings = settings.clone();
        if group_mode == crate::RepeaterGroupSendMode::SequentialSingleConnection {
            effective_proto_settings.enforce_http1_connection_close = false;
        }
        effective_proto_settings.apply_protocol_options(&mut request, draft.version);
        let protocol_mutated = request.headers != headers_before;
        if protocol_mutated {
            self.provenance.record_request(
                run_id,
                &parsed_draft_request,
                ProvenanceActor::ProtocolNormalization {
                    description: "Repeater 协议选项".into(),
                },
                &request,
            );
        }
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
        let strip_100_continue_headers = network_http.remove_100_continue_headers;
        let default_to_http2 = network_http.default_to_http2;
        let cert_callback = self.build_server_cert_callback(project);
        let session_hooks = self.strict_session_action_hooks_named();
        let http_hooks = self.tool_http_hooks_named();
        let mut record = FlowRecord {
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
                started_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)?
                    .as_millis()
                    .min(i64::MAX as u128) as i64,
                elapsed_ms: 0,
            },
            request: request.clone(),
            response: None,
        };
        if let Err(error) = self
            .store
            .begin_repeater_send(tab_id, &draft, &record, &cancel)
        {
            self.recovery.push_back(record.clone().into());
            self.storage_failed(&error);
            return Err(error);
        }
        let task_cancel = cancel.clone();
        let flow_id = record.summary.id;
        let draft_revision = draft.revision;
        let unpack_compressed_responses = settings.unpack_compressed_responses;
        let client_certificates = self.compiled_client_certs.clone();
        let sessions = self.sessions.clone();
        let provenance = self.provenance.clone();
        let (progress_tx, progress_rx) = watch::channel(None);
        let cancel_for_hooks = task_cancel.clone();
        let job = self.jobs.spawn(async move {
            let started = Instant::now();
            let default_config = SendConfig::default();
            let deadline = started + request_timeout;
            let max_body_bytes = default_config.limits.max_body_bytes;
            let mut current_origin = draft.target.clone();
            let mut current_request = request;
            let mut macro_send_config = SendConfig::default();
            macro_send_config.network.upstream_route = upstream_route.clone();
            macro_send_config.network.hostname_overrides = hostname_overrides.clone();
            macro_send_config.tls.trust = upstream_tls.clone();
            macro_send_config.tls.client_certificates = client_certificates.clone();
            macro_send_config.limits.timeout = deadline.saturating_duration_since(Instant::now());
            macro_send_config.limits.streaming_timeout = streaming_timeout;
            macro_send_config.http.default_to_http2 = default_to_http2;
            macro_send_config.auth.platform_enabled = platform_auth_enabled;
            macro_send_config.auth.platform_rules = platform_auth_rules.clone();
            macro_send_config.auth.upstream_rules = upstream_auth_rules.clone();
            let mut request_mutated = match sessions
                .apply_to_flow_record_with_steps(
                    SessionTool::Repeater,
                    &mut record,
                    &macro_send_config,
                    &cancel_for_hooks,
                    deadline,
                    started,
                )
                .await
            {
                Some((changed, steps)) => {
                    for (rule_name, snap) in steps {
                        provenance.record_request(
                            flow_id,
                            &parsed_draft_request,
                            ProvenanceActor::SessionRule { rule_name },
                            &snap,
                        );
                    }
                    if changed {
                        current_request = record.request.clone();
                        effective_proto_settings
                            .apply_protocol_options(&mut current_request, draft.version);
                        record.request = current_request.clone();
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
                match plugin_service::hooks::run_pre_send_plugin_hooks_with_provenance(
                    &session_hooks,
                    &http_hooks,
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
                let hop_route = url::Url::parse(&current_request.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| route_resolver(h)))
                    .unwrap_or_else(|| upstream_route.clone());
                let force_streaming_response =
                    network_http.is_url_in_streaming_scope(&current_request.url);
                let mut config = SendConfig::default();
                config.network.upstream_route = hop_route;
                config.network.hostname_overrides = hostname_overrides.clone();
                config.tls.trust = upstream_tls.clone();
                config.tls.options = upstream_tls_options;
                config.tls.client_certificates = client_certificates.clone();
                config.tls.on_server_cert_observed = Some(cert_callback.clone());
                config.limits.timeout = remaining;
                config.limits.streaming_timeout = streaming_timeout;
                config.http.version = Some(current_request.version);
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
                config.http.strip_100_continue_headers = strip_100_continue_headers;
                config.http.default_to_http2 = default_to_http2;
                config.http.force_streaming_response = force_streaming_response;
                config.group.connection_pool_group_id = connection_pool_group_id;
                config.group.single_packet_barrier = if hop == 0 {
                    single_packet_barrier.take()
                } else {
                    None
                };
                config.auth.platform_enabled = platform_auth_enabled;
                config.auth.platform_rules = platform_auth_rules.clone();
                config.auth.upstream_rules = upstream_auth_rules.clone();
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
                            && if follow_redirects == RepeaterRedirectMode::InScopeOnly {
                                http::resolve_redirect_request_with_options(
                                    &current_origin,
                                    &current_request,
                                    resp,
                                    RepeaterRedirectMode::Always,
                                    &network_http,
                                    process_cookies_in_redirects,
                                    cross_domain_protocol_redirects,
                                )
                                .is_ok_and(|opt| {
                                    opt.is_some_and(|(_, next_request, _)| {
                                        project_scope.is_url_in_scope(&next_request.url)
                                    })
                                })
                            } else {
                                http::resolve_redirect_request_with_options(
                                    &current_origin,
                                    &current_request,
                                    resp,
                                    follow_redirects,
                                    &network_http,
                                    process_cookies_in_redirects,
                                    cross_domain_protocol_redirects,
                                )
                                .is_ok_and(|opt| opt.is_some())
                            };
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
                        {
                            let resolved = if follow_redirects == RepeaterRedirectMode::InScopeOnly
                            {
                                http::resolve_redirect_request_with_options(
                                    &current_origin,
                                    &current_request,
                                    &response,
                                    RepeaterRedirectMode::Always,
                                    &network_http,
                                    process_cookies_in_redirects,
                                    cross_domain_protocol_redirects,
                                )
                                .map(|opt| {
                                    opt.filter(|(_, next_request, _)| {
                                        project_scope.is_url_in_scope(&next_request.url)
                                    })
                                })
                            } else {
                                http::resolve_redirect_request_with_options(
                                    &current_origin,
                                    &current_request,
                                    &response,
                                    follow_redirects,
                                    &network_http,
                                    process_cookies_in_redirects,
                                    cross_domain_protocol_redirects,
                                )
                            };
                            if let Ok(Some((next_origin, next_request, _))) = resolved {
                                last_hop_response = Some(response);
                                current_origin = next_origin;
                                current_request = next_request;
                                continue;
                            }
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
                    let response = record.response.as_ref().unwrap().clone();
                    let method = current_request.method.clone();
                    let worker = tokio::task::spawn_blocking(move || {
                        http::unpack_response(
                            &response,
                            &method,
                            max_body_bytes,
                            &task_cancel,
                            deadline,
                        )
                    });
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
        self.sends.insert(
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
