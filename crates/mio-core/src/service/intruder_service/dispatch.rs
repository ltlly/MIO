use super::*;

impl State {
    pub(in crate::service) async fn intruder_signal(&mut self, signal: Signal) -> Result<()> {
        match signal {
            Signal::Notification(permit) => {
                permit.send(
                    self.intruder
                        .notifications
                        .pop_front()
                        .expect("reserved capacity belongs to a retained Intruder notification"),
                );
                Ok(())
            }
            Signal::Notify(permit) => {
                self.intruder.dirty = false;
                self.intruder.notified = true;
                permit.send(Event::IntruderChanged {
                    project: self.project,
                });
                Ok(())
            }
            Signal::Completed(result) => self.complete_intruder(*result).await,
            Signal::Prepared(result) => self.complete_intruder_prep(*result).await,
            Signal::Advance(id) => self.advance_intruder(id).await,
        }
    }

    pub(super) async fn advance_intruder(&mut self, id: Uuid) -> Result<()> {
        let run = self
            .intruder
            .runs
            .get(&id)
            .context("Intruder 运行任务不存在")?;
        if run.cancel.is_cancelled()
            && matches!(
                run.summary.state,
                IntruderRunState::Running | IntruderRunState::Paused
            )
        {
            self.persist_intruder_state(id, IntruderRunState::Stopping)?;
            return Ok(());
        }
        if run.active == 0
            && !run.preparing
            && (run.summary.state == IntruderRunState::Stopping
                || run.summary.dispatched == run.summary.total)
        {
            let state = if let Some(error) = &run.failure {
                IntruderRunState::Failed(error.clone())
            } else if run.summary.state == IntruderRunState::Stopping {
                IntruderRunState::Stopped
            } else {
                IntruderRunState::Completed
            };
            let saved = self.persist_intruder_state(id, state);
            self.intruder.runs.remove(&id);
            return saved;
        }
        let now = PoolInstant::now();
        if !self.intruder.can_accept_commands()
            || run.summary.state != IntruderRunState::Running
            || run.preparing
            || run.waiting_for_capacity
            || !self.intruder.pool_has_capacity(run)
            || self.intruder.pool_ready_at(run, now) > now
            || self.intruder.requests.len() + self.intruder.prep_jobs.len()
                >= MAX_REQUESTS_IN_FLIGHT
            || run.summary.dispatched >= run.summary.total
        {
            return Ok(());
        }
        self.require_saved()?;
        if let Err(error) = self.intruder.check_pool_send(run, now) {
            return self.fail_intruder_timing(id, error, None);
        }
        let attack_settings = run.attack_settings.clone();
        let update_host_header = run.update_host_header;
        let ordinal = run.summary.dispatched;
        let request = run
            .plan
            .request_with_baseline(
                ordinal,
                run.summary.baseline.map(|baseline| baseline.ordinal),
            )
            .and_then(|raw| {
                let mut request = parse_request(&raw, &run.target, run.version)?;
                apply_intruder_request_headers(&mut request, update_host_header, &attack_settings);
                Ok(request)
            });
        let config: Result<SendConfig> = (|| {
            let workspace = self.store.metadata().workspace_arc()?;
            let mut send_cfg = self.build_tool_send_config_for_url(&workspace, &run.target)?;
            send_cfg.limits.timeout = run.timeout;
            send_cfg.http.version = Some(run.version);
            Ok(send_cfg)
        })();
        let config = match config {
            Ok(config) => config,
            Err(error) => {
                let message = format!("Intruder 网络配置读取或验证失败，未发送下一请求：{error}");
                let run = self.intruder.runs.get_mut(&id).unwrap();
                run.failure.get_or_insert_with(|| message.clone());
                run.cancel.cancel();
                self.persist_intruder_state(id, IntruderRunState::Stopping)?;
                return Err(error.context(message));
            }
        };
        let response_codes = match run.summary.resource_pool {
            Some(pool) => self
                .intruder
                .resource_pools
                .find(pool)
                .context("Intruder 资源池不存在；未开始请求")?
                .automatic_throttling
                .response_codes()?,
            None => Vec::new(),
        };
        self.intruder.last_dispatched = Some(id);
        let request = match request {
            Ok(request) => request,
            Err(error) => return self.persist_intruder_result(id, ordinal, Err(error.to_string())),
        };
        let record = FlowRecord {
            summary: FlowSummary {
                id: Uuid::new_v4(),
                revision: 1,
                source: TrafficSource::Intruder,
                method: request.method.clone(),
                url: request.url.clone(),
                status: None,
                state: FlowState::Pending,
                request_bytes: request.body.len() as u64,
                response_bytes: 0,
                started_unix_ms: crate::codec_util::now_unix_ms(),
                elapsed_ms: 0,
            },
            request,
            response: None,
        };
        if self.sessions.runtime.has_enabled_rules(SessionTool::Intruder)
            || !self.session_action_hooks().is_empty()
        {
            self.spawn_intruder_pre_send(id, ordinal, record, config, response_codes);
            return Ok(());
        }
        self.admit_and_spawn_intruder_send(id, ordinal, record, config, response_codes)
    }

    pub(super) fn admit_and_spawn_intruder_send(
        &mut self,
        id: Uuid,
        ordinal: u64,
        record: FlowRecord,
        config: SendConfig,
        response_codes: Vec<u16>,
    ) -> Result<()> {
        let prepared = mio_proxy::prepare_send(&record.request, config.clone());
        match self
            .store
            .intruder_mut()
            .preflight_result(id, ordinal, &record)
        {
            Ok(true) => {}
            Ok(false) => {
                self.intruder
                    .runs
                    .get_mut(&id)
                    .unwrap()
                    .waiting_for_capacity = true;
                return Ok(());
            }
            Err(error) => {
                let run = self.intruder.runs.get_mut(&id).unwrap();
                run.failure = Some(format!("Intruder 结果容量不足；未发送下一请求：{error}"));
                run.cancel.cancel();
                self.persist_intruder_state(id, IntruderRunState::Stopping)?;
                return Err(error);
            }
        }
        if record.summary.state != FlowState::Pending {
            let saved = self.persist_intruder_result(id, ordinal, Ok(Box::new(record)));
            self.intruder_history_changed();
            saved?;
            if self.intruder.runs[&id].cancel.is_cancelled() {
                return self.persist_intruder_state(id, IntruderRunState::Stopping);
            }
            return Ok(());
        }
        let workspace_snapshot = self.store.metadata().workspace_arc()?;
        // Commit admission to the selected backing before opening its socket.
        self.persist_intruder_result(id, ordinal, Ok(Box::new(record.clone())))?;
        if self.intruder.runs[&id].cancel.is_cancelled() {
            let mut record = record;
            record.summary.revision = next_revision(record.summary.revision);
            record.summary.state = FlowState::Cancelled;
            let saved = self.persist_intruder_result(id, ordinal, Ok(Box::new(record)));
            self.intruder_history_changed();
            saved?;
            return self.persist_intruder_state(id, IntruderRunState::Stopping);
        }
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                // Keep the original request detail and Pending -> Failed lifecycle even though
                // local validation prevented a network attempt and must not charge pool timing.
                let mut record = record;
                record.summary.revision = next_revision(record.summary.revision);
                record.summary.state = FlowState::Failed(error.to_string());
                let saved = self.persist_intruder_result(id, ordinal, Ok(Box::new(record)));
                self.intruder_history_changed();
                return saved;
            }
        };
        // Storage time is not part of the send-to-send interval. Re-anchor immediately before
        // starting the sender, while still refusing the socket if the monotonic deadline overflows.
        let timing = match self
            .intruder
            .prepared_pool_send(&self.intruder.runs[&id], PoolInstant::now())
        {
            Ok(timing) => timing,
            Err(error) => return self.fail_intruder_timing(id, error, Some((ordinal, record))),
        };
        if let Some((pool, timing)) = timing {
            self.intruder.pool_timings.insert(pool, timing);
        }
        let sessions = self.sessions.runtime.clone();
        let run = self.intruder.runs.get_mut(&id).unwrap();
        let attack_settings = run.attack_settings.clone();
        let update_host_header = run.update_host_header;
        let initial_origin = run.target.clone();
        let cancel = run.cancel.clone();
        let flow_id = record.summary.id;
        let project = self.project;
        let job = self.intruder.jobs.spawn(async move {
            let mut record = record;
            let mut reason_phrase = None;
            // Successful sends record the SDK's receipt time; failures and cancellations keep
            // the full logical elapsed time, including explicit automatic retry waits.
            let started = Instant::now();
            let max_body_bytes = config.limits.max_body_bytes;
            let max_net_retries = attack_settings.network_failure_retries.min(10);
            let send_outcome = if max_net_retries == 0
                && attack_settings.follow_redirects == RepeaterRedirectMode::Never
            {
                auto_throttle::send(
                    prepared,
                    &record.request,
                    config,
                    response_codes,
                    cancel.clone(),
                )
                .await
            } else {
                let max_redirect_hops =
                    workspace_snapshot.repeater_settings.effective_max_redirect_hops();
                let cross_domain_protocol_redirects =
                    workspace_snapshot.repeater_settings.cross_domain_protocol_redirects;
                let network_http = workspace_snapshot.network_http.clone();
                let project_scope = workspace_snapshot.scope.clone();
                let mut current_origin = initial_origin;
                let mut current_request = record.request.clone();
                let mut current_config = config.clone();
                let mut next_prepared = Some(prepared);
                let mut hop = 0usize;
                loop {
                    let mut net_retries = 0u8;
                    let hop_result = loop {
                        let prep = match next_prepared.take() {
                            Some(prep) => prep,
                            None => {
                                match mio_proxy::prepare_send(
                                    &current_request,
                                    current_config.clone(),
                                ) {
                                    Ok(prep) => prep,
                                    Err(err) => break Err(err),
                                }
                            }
                        };
                        match auto_throttle::send(
                            prep,
                            &current_request,
                            current_config.clone(),
                            response_codes.clone(),
                            cancel.clone(),
                        )
                        .await
                        {
                            Ok(metrics) => break Ok(metrics),
                            Err(mio_proxy::ProxyError::Cancelled) => {
                                break Err(mio_proxy::ProxyError::Cancelled);
                            }
                            Err(_) if net_retries < max_net_retries => {
                                net_retries += 1;
                                let pause = Duration::from_millis(attack_settings.retry_pause_ms);
                                if !pause.is_zero() {
                                    tokio::select! {
                                        biased;
                                        _ = cancel.cancelled() => break Err(mio_proxy::ProxyError::Cancelled),
                                        _ = tokio::time::sleep(pause) => {}
                                    }
                                }
                            }
                            Err(err) => break Err(err),
                        }
                    };
                    match hop_result {
                        Ok(metrics)
                            if attack_settings.follow_redirects != RepeaterRedirectMode::Never
                                && hop < max_redirect_hops =>
                        {
                            let resolved = if attack_settings.follow_redirects
                                == RepeaterRedirectMode::InScopeOnly
                            {
                                crate::http::resolve_redirect_request_with_options(
                                    &current_origin,
                                    &current_request,
                                    &metrics.response,
                                    RepeaterRedirectMode::Always,
                                    &network_http,
                                    attack_settings.process_cookies_in_redirections,
                                    cross_domain_protocol_redirects,
                                )
                                .map(|opt| {
                                    opt.filter(|(_, next_request, _)| {
                                        project_scope.is_url_in_scope(&next_request.url)
                                    })
                                })
                            } else {
                                crate::http::resolve_redirect_request_with_options(
                                    &current_origin,
                                    &current_request,
                                    &metrics.response,
                                    attack_settings.follow_redirects,
                                    &network_http,
                                    attack_settings.process_cookies_in_redirections,
                                    cross_domain_protocol_redirects,
                                )
                            };
                            match resolved {
                                Ok(Some((next_origin, mut next_request, _))) => {
                                    sessions.observe_response(
                                        SessionTool::Intruder,
                                        &current_request.url,
                                        &metrics.response,
                                    );
                                    apply_intruder_request_headers(
                                        &mut next_request,
                                        update_host_header,
                                        &attack_settings,
                                    );
                                    let route = match workspace_snapshot
                                        .upstream_proxy
                                        .route_for_url_with_tls(
                                            &next_origin,
                                            current_config.tls.trust.clone(),
                                        ) {
                                        Ok(route) => route,
                                        Err(err) => {
                                            break Err(mio_proxy::ProxyError::InvalidMessage(
                                                err.to_string(),
                                            ));
                                        }
                                    };
                                    current_config.network.upstream_route = route;
                                    current_origin = next_origin;
                                    current_request = next_request;
                                    hop += 1;
                                    continue;
                                }
                                Ok(None) => break Ok(metrics),
                                Err(err) => {
                                    break Err(mio_proxy::ProxyError::InvalidMessage(
                                        err.to_string(),
                                    ));
                                }
                            }
                        }
                        other => break other,
                    }
                }
            };
            let elapsed_ms = match send_outcome {
                Ok(metrics) => {
                    let unpacked = if metrics
                        .response
                        .headers
                        .iter()
                        .any(|header| header.name.eq_ignore_ascii_case("content-encoding"))
                    {
                        let response = metrics.response;
                        let method = record.request.method.clone();
                        let task_cancel = cancel.clone();
                        let deadline = Instant::now() + Duration::from_secs(2);
                        match tokio::task::spawn_blocking(move || {
                            let unpacked = crate::http::unpack_response(
                                &response,
                                &method,
                                max_body_bytes,
                                &task_cancel,
                                deadline,
                            );
                            (response, unpacked)
                        })
                        .await
                        {
                            Ok((_, Ok(Some(unpacked)))) => Some(unpacked),
                            Ok((_, Err(crate::http::UnpackError::Cancelled))) => None,
                            Ok((response, _)) => Some(response),
                            Err(_) => None,
                        }
                    } else {
                        Some(metrics.response)
                    };
                    match unpacked {
                        Some(response) => {
                            sessions.observe_response(
                                SessionTool::Intruder,
                                &record.request.url,
                                &response,
                            );
                            record.summary.status = Some(response.status);
                            record.summary.response_bytes = response.body.len() as u64;
                            record.summary.state = FlowState::Complete;
                            record.response = Some(response);
                            reason_phrase = metrics.reason_phrase;
                            metrics.response_received_ms
                        }
                        None => {
                            record.summary.state = FlowState::Cancelled;
                            reason_phrase = None;
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64
                        }
                    }
                }
                Err(mio_proxy::ProxyError::Cancelled) => {
                    record.summary.state = FlowState::Cancelled;
                    started.elapsed().as_millis().min(u64::MAX as u128) as u64
                }
                Err(error) => {
                    record.summary.state = FlowState::Failed(error.to_string());
                    started.elapsed().as_millis().min(u64::MAX as u128) as u64
                }
            };
            record.summary.elapsed_ms = elapsed_ms;
            record.summary.revision += 1;
            Completion {
                project,
                record,
                reason_phrase,
            }
        });
        self.intruder.requests.insert(
            job.id(),
            ActiveRequest {
                run_id: id,
                ordinal,
                flow_id,
                resource_pool: run.summary.resource_pool,
            },
        );
        run.active += 1;
        self.intruder_history_changed();
        Ok(())
    }

    pub(super) fn fail_intruder_timing(
        &mut self,
        id: Uuid,
        error: anyhow::Error,
        admitted: Option<(u64, FlowRecord)>,
    ) -> Result<()> {
        let message = format!("Intruder 资源池计时失败，未发送请求：{error}");
        let run = self
            .intruder
            .runs
            .get_mut(&id)
            .context("Intruder 计时失败的任务不存在")?;
        run.failure = Some(message.clone());
        run.cancel.cancel();
        if let Some((ordinal, mut record)) = admitted {
            record.summary.revision = next_revision(record.summary.revision);
            record.summary.state = FlowState::Failed(message.clone());
            self.persist_intruder_result(id, ordinal, Ok(Box::new(record)))?;
        }
        self.persist_intruder_state(id, IntruderRunState::Stopping)?;
        Err(error.context(message))
    }
}

fn apply_intruder_request_headers(
    request: &mut HttpRequest,
    update_host_header: bool,
    attack_settings: &IntruderAttackSettings,
) {
    if update_host_header {
        request
            .headers
            .retain(|header| !header.name.eq_ignore_ascii_case("host"));
    }
    if attack_settings.update_content_length {
        let len_bytes = request.body.len().to_string().into_bytes();
        let mut updated = false;
        request.headers.retain_mut(|header| {
            if header.name.eq_ignore_ascii_case("content-length") {
                if !updated {
                    header.value = len_bytes.clone();
                    updated = true;
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
        if !updated && !request.body.is_empty() {
            request.headers.push(Header {
                name: "Content-Length".into(),
                value: len_bytes,
            });
        }
    }
    if attack_settings.set_connection_close {
        let mut updated = false;
        request.headers.retain_mut(|header| {
            if header.name.eq_ignore_ascii_case("connection") {
                if !updated {
                    header.value = "close".into();
                    updated = true;
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
        if !updated {
            request.headers.push(Header {
                name: "Connection".into(),
                value: "close".into(),
            });
        }
    }
}
