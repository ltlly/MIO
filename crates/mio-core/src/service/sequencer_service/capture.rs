use super::*;

pub(super) async fn run_capture(
    plan: CapturePlan,
    cancel: CancellationToken,
    progress_tx: mpsc::Sender<SequencerEvent>,
) -> SequencerEvent {
    let CapturePlan {
        project,
        run_id,
        request,
        config,
        location,
        target_samples,
        concurrency,
        unpack_compressed_responses,
        sessions,
    } = plan;
    let mut requests = JoinSet::<Attempt>::new();
    let mut requests_sent = 0_usize;
    let mut extraction_misses = 0_usize;
    let mut request_errors = 0_usize;
    let mut samples = Vec::new();
    let mut total_sample_bytes = 0_usize;
    let mut consecutive_non_tokens = 0_usize;
    let mut last_progress_requests = 0_usize;
    let mut last_progress_at = Instant::now();
    let mut terminal = None::<(bool, Option<String>)>;

    while terminal.is_none() {
        if cancel.is_cancelled() {
            terminal = Some((true, None));
            break;
        }
        let needed = target_samples.saturating_sub(samples.len());
        while requests.len() < concurrency.min(needed) {
            let mut request = request.clone();
            let request_config = config.clone();
            let request_cancel = cancel.child_token();
            let sessions = sessions.clone();
            requests.spawn(async move {
                let deadline = Instant::now() + request_config.limits.timeout;
                if let Err(err) = sessions
                    .apply_before_send(
                        crate::SessionTool::Sequencer,
                        &mut request,
                        &request_config,
                        &request_cancel,
                        deadline,
                    )
                    .await
                {
                    let proxy_err = match err {
                        crate::sessions::SessionExecError::Cancelled => ProxyError::Cancelled,
                        crate::sessions::SessionExecError::Failed(msg) => {
                            ProxyError::InvalidMessage(msg)
                        }
                    };
                    return Attempt {
                        result: Err(proxy_err),
                    };
                }
                let result = send(request.clone(), request_config, request_cancel).await;
                if let Ok(ref resp) = result {
                    sessions.observe_response(crate::SessionTool::Sequencer, &request.url, resp);
                }
                Attempt { result }
            });
        }
        let attempt = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                terminal = Some((true, None));
                break;
            }
            result = requests.join_next() => match result {
                Some(Ok(attempt)) => attempt,
                None => {
                    terminal = Some((false, None));
                    break;
                }
                Some(Err(error)) => {
                    terminal = Some((true, Some(format!("Sequencer 采样请求任务异常结束：{error}"))));
                    break;
                }
            },
        };
        if cancel.is_cancelled() {
            continue;
        }
        match attempt.result {
            Ok(response) => {
                requests_sent = requests_sent.saturating_add(1);
                let response = decoded_response(
                    &request,
                    &config,
                    unpack_compressed_responses,
                    &response,
                    &cancel,
                )
                .await
                .unwrap_or(response);
                match extract_token(&response, &location) {
                    Some(token) => {
                        let next_bytes = total_sample_bytes.saturating_add(token.len());
                        if next_bytes <= MAX_TOTAL_SAMPLE_BYTES {
                            total_sample_bytes = next_bytes;
                            samples.push(token);
                            consecutive_non_tokens = 0;
                            if samples.len() >= target_samples {
                                terminal = Some((false, None));
                            } else if total_sample_bytes == MAX_TOTAL_SAMPLE_BYTES {
                                terminal = Some((
                                    true,
                                    Some("Sequencer 样本总字节已达上限；停止继续采样".to_owned()),
                                ));
                            }
                        } else {
                            terminal = Some((
                                true,
                                Some("Sequencer 样本总字节超过上限；未保留该样本".to_owned()),
                            ));
                        }
                    }
                    None => {
                        extraction_misses = extraction_misses.saturating_add(1);
                        consecutive_non_tokens = consecutive_non_tokens.saturating_add(1);
                    }
                }
            }
            Err(_) => {
                requests_sent = requests_sent.saturating_add(1);
                request_errors = request_errors.saturating_add(1);
                consecutive_non_tokens = consecutive_non_tokens.saturating_add(1);
            }
        }
        if terminal.is_none()
            && ((samples.is_empty() && requests_sent >= target_samples.saturating_mul(3))
                || consecutive_non_tokens >= target_samples.saturating_mul(3))
        {
            terminal = Some((
                true,
                Some(if request_errors >= extraction_misses {
                    "Sequencer 采样请求连续失败过多；停止继续采样".to_owned()
                } else {
                    "Sequencer 采样响应连续缺少目标 Token；停止继续采样".to_owned()
                }),
            ));
        }
        if terminal.is_none()
            && (requests_sent.saturating_sub(last_progress_requests) >= PROGRESS_BATCH_REQUESTS
                || last_progress_at.elapsed() >= PROGRESS_INTERVAL)
        {
            let latest_samples = samples[samples.len().saturating_sub(PROGRESS_PREVIEW)..].to_vec();
            let _ = progress_tx.try_send(SequencerEvent::CaptureProgress {
                project,
                run_id,
                requests_sent,
                tokens_collected: samples.len(),
                extraction_misses,
                request_errors,
                latest_samples,
            });
            last_progress_requests = requests_sent;
            last_progress_at = Instant::now();
        }
    }

    cancel.cancel();
    let drain_deadline = TokioInstant::now() + STOP_DEADLINE;
    let mut timed_out = false;
    while !requests.is_empty() {
        match timeout_at(drain_deadline, requests.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }
    let (mut stopped_early, mut error) = terminal.unwrap_or((false, None));
    if timed_out {
        requests.abort_all();
        while !requests.is_empty() {
            match timeout_at(drain_deadline, requests.join_next()).await {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        stopped_early = true;
        if error.is_none() {
            error = Some("停止期限内未能回收全部采样连接".to_owned());
        }
    }
    SequencerEvent::CaptureFinished {
        project,
        run_id,
        requests_sent,
        tokens_collected: samples.len(),
        extraction_misses,
        request_errors,
        samples,
        stopped_early,
        error,
    }
}

async fn decoded_response(
    request: &HttpRequest,
    config: &SendConfig,
    unpack_compressed_responses: bool,
    response: &HttpResponse,
    cancel: &CancellationToken,
) -> Option<HttpResponse> {
    let encoded = response
        .headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("content-encoding"));
    if !unpack_compressed_responses || !encoded {
        return None;
    }
    let response = response.clone();
    let method = request.method.clone();
    let max_body_bytes = config.limits.max_body_bytes;
    let deadline = Instant::now() + config.limits.timeout;
    let cancel = cancel.clone();
    let worker = tokio::task::spawn_blocking(move || {
        http::unpack_response(&response, &method, max_body_bytes, &cancel, deadline)
    });
    match timeout_at(TokioInstant::now() + config.limits.timeout, worker).await {
        Ok(Ok(Ok(Some(response)))) => Some(response),
        _ => None,
    }
}

pub(super) fn fallback_capture_finished(
    project: ProjectKey,
    run_id: Uuid,
    error: &str,
) -> SequencerEvent {
    SequencerEvent::CaptureFinished {
        project,
        run_id,
        requests_sent: 0,
        tokens_collected: 0,
        extraction_misses: 0,
        request_errors: 0,
        samples: Vec::new(),
        stopped_early: true,
        error: Some(error.to_owned()),
    }
}
