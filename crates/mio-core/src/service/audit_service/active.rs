use super::*;

use crate::{
    AuditPhase,
    service::plugin_service::passive::{PLUGIN_ACTIVE_ISSUE_ID, convert_plugin_findings_for_phase},
};
use mio_plugin_api::protocol;

pub(super) use crate::codec_util::now_unix_ms;

pub(super) fn build_scanner_flow(
    id: FlowId,
    request: HttpRequest,
    response: HttpResponse,
    started_unix_ms: i64,
    elapsed_ms: u64,
) -> FlowRecord {
    FlowRecord {
        summary: FlowSummary {
            id,
            revision: 1,
            source: TrafficSource::Scanner,
            method: request.method.clone(),
            url: request.url.clone(),
            status: Some(response.status),
            state: FlowState::Complete,
            request_bytes: request.body.len() as u64,
            response_bytes: response.body.len() as u64,
            started_unix_ms,
            elapsed_ms,
        },
        request,
        response: Some(response),
    }
}

fn active_payloads_for(point: &InsertionPoint, oast_url: Option<&str>) -> Vec<String> {
    let mut out = vec![
        format!("{}<{ACTIVE_CANARY}>", point.value),
        ACTIVE_REDIRECT_TARGET.to_owned(),
        format!("{}\r\n{ACTIVE_SPLIT_HEADER}: 1", point.value),
        format!("{}'\"", point.value),
    ];
    if let Some(url) = oast_url {
        out.push(url.to_owned());
    }
    out
}

pub(super) async fn maybe_unpack_response(
    request: &HttpRequest,
    config: &SendConfig,
    unpack: bool,
    response: HttpResponse,
    cancel: &CancellationToken,
) -> HttpResponse {
    let encoded = response
        .headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("content-encoding"));
    if !unpack || !encoded {
        return response;
    }
    let cloned = response.clone();
    let method = request.method.clone();
    let max_body = config.max_body_bytes;
    let deadline = Instant::now() + config.timeout;
    let cancel = cancel.clone();
    let worker = tokio::task::spawn_blocking(move || {
        http::unpack_response(&cloned, &method, max_body, &cancel, deadline)
    });
    match timeout_at(TokioInstant::now() + config.timeout, worker).await {
        Ok(Ok(Ok(Some(unpacked)))) => unpacked,
        _ => response,
    }
}

struct ProbeOutcome {
    sent: bool,
    flow_and_findings: Option<(Box<FlowRecord>, Vec<AuditFinding>)>,
}

pub(super) async fn send_audit_signal(
    signal_tx: &mpsc::Sender<AuditSignal>,
    cancel: &CancellationToken,
    signal: AuditSignal,
) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        res = signal_tx.send(signal) => res.is_ok(),
    }
}

pub(super) async fn wait_until_resumed(
    pause_rx: &mut watch::Receiver<bool>,
    cancel: &CancellationToken,
) -> bool {
    while *pause_rx.borrow() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return false,
            res = pause_rx.changed() => {
                if res.is_err() {
                    return !cancel.is_cancelled();
                }
            }
        }
    }
    !cancel.is_cancelled()
}

pub(super) async fn run_scan(
    plan: ScanPlan,
    cancel: CancellationToken,
    mut pause_rx: watch::Receiver<bool>,
    signal_tx: mpsc::Sender<AuditSignal>,
) -> JobDone {
    let ScanPlan {
        project,
        task_id,
        request: mut base_req,
        config,
        mode,
        crawl_same_origin,
        scope,
        insertion_points,
        unpack_compressed_responses,
        oast_callback_url,
        plugin_active_hooks,
        custom_scan_checks,
        sessions,
    } = plan;
    if !wait_until_resumed(&mut pause_rx, &cancel).await {
        return JobDone {
            project,
            task_id,
            requests_sent: 0,
            state: AuditTaskState::Cancelled,
            error: None,
        };
    }
    let base_started_ms = now_unix_ms();
    let base_timer = Instant::now();
    let base_deadline = base_timer + config.timeout;
    if let Err(err) = sessions
        .apply_before_send(
            crate::SessionTool::Scanner,
            &mut base_req,
            &config,
            &cancel,
            base_deadline,
        )
        .await
    {
        return JobDone {
            project,
            task_id,
            requests_sent: 0,
            state: match err {
                crate::sessions::SessionExecError::Cancelled => AuditTaskState::Cancelled,
                crate::sessions::SessionExecError::Failed(_) => AuditTaskState::Failed,
            },
            error: match err {
                crate::sessions::SessionExecError::Cancelled => None,
                crate::sessions::SessionExecError::Failed(msg) => Some(msg),
            },
        };
    }
    let base_res = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return JobDone {
                project,
                task_id,
                requests_sent: 0,
                state: AuditTaskState::Cancelled,
                error: None,
            };
        }
        res = send(base_req.clone(), config.clone(), cancel.child_token()) => res,
    };
    let base_raw = match base_res {
        Ok(resp) => resp,
        Err(_) if cancel.is_cancelled() => {
            return JobDone {
                project,
                task_id,
                requests_sent: 0,
                state: AuditTaskState::Cancelled,
                error: None,
            };
        }
        Err(err) => {
            return JobDone {
                project,
                task_id,
                requests_sent: 1,
                state: AuditTaskState::Failed,
                error: Some(format!("基线请求失败：{err}")),
            };
        }
    };
    let base_elapsed = base_timer.elapsed().as_millis().min(u64::MAX as u128) as u64;
    sessions.observe_response(crate::SessionTool::Scanner, &base_req.url, &base_raw);
    let base_resp = maybe_unpack_response(
        &base_req,
        &config,
        unpack_compressed_responses,
        base_raw,
        &cancel,
    )
    .await;
    let base_flow_id = Uuid::new_v4();
    let now_ms = now_unix_ms();
    let mut base_findings = inspect_passive(Some(base_flow_id), &base_req, &base_resp, now_ms);
    if !custom_scan_checks.is_empty() {
        base_findings.extend(evaluate_custom_scan_checks(
            Some(base_flow_id),
            &base_req,
            &base_resp,
            &custom_scan_checks,
            AuditPhase::Passive,
            now_ms,
        ));
    }
    let base_flow = build_scanner_flow(
        base_flow_id,
        base_req.clone(),
        base_resp.clone(),
        base_started_ms,
        base_elapsed,
    );
    let mut requests_sent = 1_u64;
    let _ = send_audit_signal(
        &signal_tx,
        &cancel,
        AuditSignal {
            project,
            task_id,
            kind: AuditSignalKind::Step {
                requests_sent,
                flow: Some(Box::new(base_flow)),
                findings: base_findings,
            },
        },
    )
    .await;

    if !wait_until_resumed(&mut pause_rx, &cancel).await {
        return JobDone {
            project,
            task_id,
            requests_sent,
            state: AuditTaskState::Cancelled,
            error: None,
        };
    }

    let crawl_stage = super::crawl::collect_active_targets(
        project,
        task_id,
        &base_req,
        &base_resp,
        insertion_points,
        mode,
        crawl_same_origin,
        &scope,
        &config,
        unpack_compressed_responses,
        &mut requests_sent,
        &cancel,
        &signal_tx,
    )
    .await;

    if crawl_stage.cancelled {
        return JobDone {
            project,
            task_id,
            requests_sent,
            state: AuditTaskState::Cancelled,
            error: None,
        };
    }

    if mode == AuditTaskMode::PassiveOnly || crawl_stage.targets.is_empty() {
        return JobDone {
            project,
            task_id,
            requests_sent,
            state: if cancel.is_cancelled() {
                AuditTaskState::Cancelled
            } else {
                AuditTaskState::Completed
            },
            error: None,
        };
    }

    let mut probe_config = config.clone();
    probe_config.raw_http1_override = None;
    probe_config.update_content_length = true;
    let semaphore = Arc::new(Semaphore::new(ACTIVE_CONCURRENCY));
    let mut probes = JoinSet::<ProbeOutcome>::new();

    for target in crawl_stage.targets {
        for point in target.points {
            if !plugin_active_hooks.is_empty() && !cancel.is_cancelled() {
                let hooks = plugin_active_hooks.clone();
                let req_copy = target.request.clone();
                let resp_copy = target.base_response.clone();
                let pt = point.clone();
                let child_cancel = cancel.child_token();
                probes.spawn(async move {
                    let probe_flow_id = Uuid::new_v4();
                    let req_dto: protocol::HttpRequestDto = req_copy.clone().into();
                    let resp_dto: protocol::HttpResponseDto = (*resp_copy).clone().into();
                    if req_dto.validate().is_err() || resp_dto.validate().is_err() {
                        return ProbeOutcome {
                            sent: false,
                            flow_and_findings: None,
                        };
                    }
                    let mut plugin_hits = Vec::new();
                    for handle in hooks {
                        if child_cancel.is_cancelled() {
                            break;
                        }
                        let call = protocol::ActiveCheckCall {
                            flow_id: probe_flow_id.as_bytes().to_vec(),
                            base_request: Some(req_dto.clone()),
                            base_response: Some(resp_dto.clone()),
                            parameter_name: pt.name.clone(),
                            parameter_value: pt.value.clone(),
                        };
                        if let Ok(res) = handle
                            .call_active_check_with_cancel(call, child_cancel.child_token())
                            .await
                            && !res.findings.is_empty()
                        {
                            plugin_hits.extend(convert_plugin_findings_for_phase(
                                probe_flow_id,
                                &req_copy.url,
                                &resp_copy,
                                res.findings,
                                AuditPhase::Active,
                                PLUGIN_ACTIVE_ISSUE_ID,
                            ));
                        }
                    }
                    let flow_and_findings = (!plugin_hits.is_empty()).then(|| {
                        let flow = build_scanner_flow(
                            probe_flow_id,
                            req_copy,
                            (*resp_copy).clone(),
                            now_unix_ms(),
                            0,
                        );
                        (Box::new(flow), plugin_hits)
                    });
                    ProbeOutcome {
                        sent: false,
                        flow_and_findings,
                    }
                });
            }
            for payload in active_payloads_for(&point, oast_callback_url.as_deref()) {
                if cancel.is_cancelled() {
                    break;
                }
                let Ok(mut probe_req) = build_probe_request(&target.request, &point, &payload) else {
                    continue;
                };
                let sem = semaphore.clone();
                let cfg = probe_config.clone();
                let base = target.base_response.clone();
                let pt = point.clone();
                let child_cancel = cancel.child_token();
                let mut probe_pause = pause_rx.clone();
                let probe_sessions = sessions.clone();
                let probe_custom_checks = custom_scan_checks.clone();
                probes.spawn(async move {
                    let permit = tokio::select! {
                        biased;
                        _ = child_cancel.cancelled() => return ProbeOutcome { sent: false, flow_and_findings: None },
                        p = sem.acquire_owned() => p,
                    };
                    let Ok(_permit) = permit else {
                        return ProbeOutcome {
                            sent: false,
                            flow_and_findings: None,
                        };
                    };
                    if !wait_until_resumed(&mut probe_pause, &child_cancel).await {
                        return ProbeOutcome {
                            sent: false,
                            flow_and_findings: None,
                        };
                    }
                    let started_ms = now_unix_ms();
                    let timer = Instant::now();
                    if probe_sessions
                        .apply_before_send(
                            crate::SessionTool::Scanner,
                            &mut probe_req,
                            &cfg,
                            &child_cancel,
                            timer + cfg.timeout,
                        )
                        .await
                        .is_err()
                    {
                        return ProbeOutcome {
                            sent: false,
                            flow_and_findings: None,
                        };
                    }
                    let Ok(raw_resp) =
                        send(probe_req.clone(), cfg.clone(), child_cancel.child_token()).await
                    else {
                        return ProbeOutcome {
                            sent: !child_cancel.is_cancelled(),
                            flow_and_findings: None,
                        };
                    };
                    probe_sessions.observe_response(
                        crate::SessionTool::Scanner,
                        &probe_req.url,
                        &raw_resp,
                    );
                    let elapsed = timer.elapsed().as_millis().min(u64::MAX as u128) as u64;
                    let probe_resp = maybe_unpack_response(
                        &probe_req,
                        &cfg,
                        unpack_compressed_responses,
                        raw_resp,
                        &child_cancel,
                    )
                    .await;
                    let eval_now = now_unix_ms();
                    let mut hits = evaluate_active_check(
                        None,
                        &probe_req,
                        &pt,
                        &payload,
                        &base,
                        &probe_resp,
                        eval_now,
                    );
                    if !probe_custom_checks.is_empty() {
                        hits.extend(evaluate_custom_scan_checks(
                            None,
                            &probe_req,
                            &probe_resp,
                            &probe_custom_checks,
                            AuditPhase::Active,
                            eval_now,
                        ));
                    }
                    let flow_and_findings = if hits.is_empty() {
                        None
                    } else {
                        let probe_flow_id = Uuid::new_v4();
                        for f in &mut hits {
                            f.flow_id = Some(probe_flow_id);
                        }
                        let flow = build_scanner_flow(
                            probe_flow_id,
                            probe_req,
                            probe_resp,
                            started_ms,
                            elapsed,
                        );
                        Some((Box::new(flow), hits))
                    };
                    ProbeOutcome {
                        sent: true,
                        flow_and_findings,
                    }
                });
            }
        }
    }

    let mut cancelled = cancel.is_cancelled();
    while !cancelled {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                cancelled = true;
            }
            next = probes.join_next() => match next {
                Some(Ok(outcome)) => {
                    if outcome.sent {
                        requests_sent = requests_sent.saturating_add(1);
                    }
                    let (flow, findings) = match outcome.flow_and_findings {
                        Some((f, hits)) => (Some(f), hits),
                        None => (None, Vec::new()),
                    };
                    if outcome.sent || flow.is_some() {
                        let _ = send_audit_signal(
                            &signal_tx,
                            &cancel,
                            AuditSignal {
                                project,
                                task_id,
                                kind: AuditSignalKind::Step {
                                    requests_sent,
                                    flow,
                                    findings,
                                },
                            },
                        )
                        .await;
                    }
                }
                Some(Err(_)) => {}
                None => break,
            }
        }
    }

    cancel.cancel();
    let drain_deadline = TokioInstant::now() + STOP_DEADLINE;
    let mut drain_timed_out = false;
    while !probes.is_empty() {
        match timeout_at(drain_deadline, probes.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                drain_timed_out = true;
                break;
            }
        }
    }
    let mut error = None;
    if drain_timed_out {
        probes.abort_all();
        let abort_deadline = TokioInstant::now() + Duration::from_millis(200);
        while !probes.is_empty() {
            match timeout_at(abort_deadline, probes.join_next()).await {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        cancelled = true;
        error = Some("停止期限内未能回收全部审计探测连接".to_owned());
    }
    JobDone {
        project,
        task_id,
        requests_sent,
        state: if cancelled {
            AuditTaskState::Cancelled
        } else {
            AuditTaskState::Completed
        },
        error,
    }
}
