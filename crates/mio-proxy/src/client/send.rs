use std::time::Instant;

use mio_plugin_api::{HttpRequest, HttpResponse};
use rama::{
    extensions::ExtensionsRef,
    graceful::Shutdown,
    http::{Response, body::util::BodyExt},
    rt::Executor,
};
use tokio_util::sync::CancellationToken;

use crate::{ProxyError, message};

use super::{
    PreparedSend, SendConfig, SendInput, SendMetrics, SendStreamOutcome, exchange,
    exchange_with_metrics,
};

pub(crate) fn find_matching_platform_auth<'a>(
    url: &str,
    config: &'a SendConfig,
) -> Option<&'a crate::PlatformAuthRuleConfig> {
    if !config.platform_auth_enabled || config.platform_auth_rules.is_empty() {
        return None;
    }
    let uri: rama::net::uri::Uri = url.parse().ok()?;
    let host = uri.host_str()?;
    config
        .platform_auth_rules
        .iter()
        .find(|rule| rule.matches_host(&host))
}

pub(crate) fn find_matching_upstream_auth<'a>(
    url: &str,
    config: &'a SendConfig,
) -> Option<&'a crate::UpstreamAuthRuleConfig> {
    if config.upstream_auth_rules.is_empty() {
        return None;
    }
    let uri: rama::net::uri::Uri = url.parse().ok()?;
    let host = uri.host_str()?;
    config
        .upstream_auth_rules
        .iter()
        .find(|rule| rule.matches_destination(&host))
}

pub(crate) fn apply_preemptive_auth_to_model(
    request: &mut HttpRequest,
    config: &SendConfig,
) -> bool {
    let mut mutated = false;
    if let Some(rule) = find_matching_platform_auth(&request.url, config)
        && rule.credentials.scheme == crate::HttpAuthScheme::Basic
        && !request
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("authorization"))
    {
        let val = crate::build_basic_authorization_value(
            &rule.credentials.username,
            &rule.credentials.password,
        );
        crate::auth::set_or_replace_header(&mut request.headers, "Authorization", val);
        mutated = true;
    }
    if let Some(rule) = find_matching_upstream_auth(&request.url, config)
        && rule.credentials.scheme == crate::HttpAuthScheme::Basic
        && !request
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("proxy-authorization"))
    {
        let val = crate::build_basic_authorization_value(
            &rule.credentials.username,
            &rule.credentials.password,
        );
        crate::auth::set_or_replace_header(&mut request.headers, "Proxy-Authorization", val);
        mutated = true;
    }
    mutated
}

async fn drain_auth_challenge_body(
    mut body: rama::http::Body,
    deadline: tokio::time::Instant,
) -> Result<(), ProxyError> {
    const AUTH_CHALLENGE_DRAIN_LIMIT: usize = 256 * 1024;
    let mut drained = 0usize;
    loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    drained = drained.saturating_add(data.len());
                    if drained > AUTH_CHALLENGE_DRAIN_LIMIT {
                        return Ok(());
                    }
                }
            }
            Ok(Some(Err(err))) => return Err(ProxyError::network(err)),
            Ok(None) => return Ok(()),
            Err(_) => return Err(ProxyError::Timeout),
        }
    }
}

pub(crate) async fn exchange_model_with_auth(
    mut model: HttpRequest,
    base_config: &SendConfig,
    deadline: tokio::time::Instant,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<(Response, std::sync::Arc<std::sync::Mutex<crate::transport::ResponseTiming>>), ProxyError> {
    let mut config = base_config.clone();
    if apply_preemptive_auth_to_model(&mut model, &config) {
        config.raw_http1_override = None;
    }
    let initial_req = message::to_request_with_config(&model, &config)?;
    let (mut response, mut timing) = tokio::time::timeout_at(
        deadline,
        Box::pin(exchange_with_metrics(
            initial_req,
            &config,
            cancel.clone(),
            exec.clone(),
        )),
    )
    .await
    .map_err(|_| ProxyError::Timeout)??;

    for _step in 0..2 {
        let status = response.status().as_u16();
        let (is_proxy, creds) = if status == 407 {
            let Some(rule) = find_matching_upstream_auth(&model.url, &config) else {
                break;
            };
            (true, rule.credentials.clone())
        } else if status == 401 {
            let Some(rule) = find_matching_platform_auth(&model.url, &config) else {
                break;
            };
            (false, rule.credentials.clone())
        } else {
            break;
        };
        let challenge_hdr_name = if is_proxy {
            "proxy-authenticate"
        } else {
            "www-authenticate"
        };
        let auth_hdr_name = if is_proxy {
            "Proxy-Authorization"
        } else {
            "Authorization"
        };
        let challenges: Vec<String> = response
            .headers()
            .get_all(challenge_hdr_name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|s| s.trim().to_owned())
            .collect();
        if challenges.is_empty() {
            break;
        }
        match creds.scheme {
            crate::HttpAuthScheme::None => break,
            crate::HttpAuthScheme::Basic => {
                if model
                    .headers
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case(auth_hdr_name))
                {
                    break;
                }
                drain_auth_challenge_body(response.into_body(), deadline).await?;
                let val = crate::build_basic_authorization_value(&creds.username, &creds.password);
                crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, val);
                config.raw_http1_override = None;
                let retry_req = message::to_request_with_config(&model, &config)?;
                (response, timing) = tokio::time::timeout_at(
                    deadline,
                    Box::pin(exchange_with_metrics(
                        retry_req,
                        &config,
                        cancel.clone(),
                        exec.clone(),
                    )),
                )
                .await
                .map_err(|_| ProxyError::Timeout)??;
            }
            crate::HttpAuthScheme::Digest => {
                let Some(digest_chal) = challenges
                    .iter()
                    .find_map(|raw| crate::DigestChallenge::parse_from_header(raw))
                else {
                    break;
                };
                drain_auth_challenge_body(response.into_body(), deadline).await?;
                let (cnonce, _, _) = crate::auth::next_cnonce_and_challenge();
                let uri_path = crate::auth::request_uri_path_and_query(&model.url);
                let val = digest_chal.build_authorization_value(
                    &creds.username,
                    &creds.password,
                    &model.method,
                    &uri_path,
                    &model.body,
                    1,
                    &cnonce,
                )?;
                crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, val);
                config.raw_http1_override = None;
                let retry_req = message::to_request_with_config(&model, &config)?;
                (response, timing) = tokio::time::timeout_at(
                    deadline,
                    Box::pin(exchange_with_metrics(
                        retry_req,
                        &config,
                        cancel.clone(),
                        exec.clone(),
                    )),
                )
                .await
                .map_err(|_| ProxyError::Timeout)??;
                break;
            }
            crate::HttpAuthScheme::NtlmV1 | crate::HttpAuthScheme::NtlmV2 => {
                let is_v2 = creds.scheme == crate::HttpAuthScheme::NtlmV2;
                config.raw_http1_override = None;
                config.use_connection_pool = true;
                config.http1_keep_alive = true;
                if config.connection_pool_group_id.is_none() {
                    config.connection_pool_group_id = Some(mio_plugin_api::Uuid::new_v4().as_u128());
                }
                let type2_chal = if let Some(chal) = challenges
                    .iter()
                    .find_map(|raw| crate::NtlmType2Challenge::parse_from_header(raw).ok())
                {
                    drain_auth_challenge_body(response.into_body(), deadline).await?;
                    chal
                } else if challenges
                    .iter()
                    .any(|raw| raw.eq_ignore_ascii_case("ntlm") || raw.starts_with("NTLM") || raw.starts_with("ntlm"))
                {
                    drain_auth_challenge_body(response.into_body(), deadline).await?;
                    let type1_val =
                        crate::build_ntlm_type1_header_value(&creds.domain, &creds.workstation);
                    crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, type1_val);
                    let type1_req = message::to_request_with_config(&model, &config)?;
                    let (type2_resp, type2_timing) = tokio::time::timeout_at(
                        deadline,
                        Box::pin(exchange_with_metrics(
                            type1_req,
                            &config,
                            cancel.clone(),
                            exec.clone(),
                        )),
                    )
                    .await
                    .map_err(|_| ProxyError::Timeout)??;
                    let type2_headers: Vec<String> = type2_resp
                        .headers()
                        .get_all(challenge_hdr_name)
                        .iter()
                        .filter_map(|v| v.to_str().ok())
                        .map(|s| s.trim().to_owned())
                        .collect();
                    let Some(chal) = type2_headers
                        .iter()
                        .find_map(|raw| crate::NtlmType2Challenge::parse_from_header(raw).ok())
                    else {
                        response = type2_resp;
                        timing = type2_timing;
                        break;
                    };
                    drain_auth_challenge_body(type2_resp.into_body(), deadline).await?;
                    chal
                } else {
                    break;
                };
                let (_, client_challenge, timestamp_100ns) =
                    crate::auth::next_cnonce_and_challenge();
                let type3_val = crate::build_ntlm_type3_header_value(
                    &type2_chal,
                    &creds,
                    is_v2,
                    client_challenge,
                    timestamp_100ns,
                )?;
                crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, type3_val);
                let type3_req = message::to_request_with_config(&model, &config)?;
                (response, timing) = tokio::time::timeout_at(
                    deadline,
                    Box::pin(exchange_with_metrics(
                        type3_req,
                        &config,
                        cancel.clone(),
                        exec.clone(),
                    )),
                )
                .await
                .map_err(|_| ProxyError::Timeout)??;
                break;
            }
        }
    }
    Ok((response, timing))
}

pub fn prepare_send(request: &HttpRequest, config: SendConfig) -> Result<PreparedSend, ProxyError> {
    Ok(PreparedSend {
        request: message::to_request_with_config(request, &config)?,
        config,
    })
}

impl PreparedSend {
    /// Sends once and retains the original cancellation and SDK-drain ownership contract.
    pub async fn send(self, cancel: CancellationToken) -> Result<HttpResponse, ProxyError> {
        let cancel = cancel.child_token();
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        send_owned(
            SendInput::Prepared(self.request),
            self.config,
            cancel,
            shutdown,
        )
        .await
    }

    /// Sends once and reports the first-byte response latency alongside the response.
    pub async fn send_with_metrics(
        self,
        cancel: CancellationToken,
    ) -> Result<SendMetrics, ProxyError> {
        let cancel = cancel.child_token();
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        send_owned_with_metrics(
            SendInput::Prepared(self.request),
            self.config,
            cancel,
            shutdown,
        )
        .await
    }
}

/// Sends once, with no implicit retry or redirects. Cancellation closes its socket.
/// The caller must retain this future until the SDK workers have finished draining.
pub async fn send(
    request: HttpRequest,
    config: SendConfig,
    cancel: CancellationToken,
) -> Result<HttpResponse, ProxyError> {
    let cancel = cancel.child_token();
    let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
    send_owned(SendInput::Message(request), config, cancel, shutdown).await
}

pub async fn send_streaming(
    request: HttpRequest,
    config: SendConfig,
    cancel: CancellationToken,
    mut on_progress: impl FnMut(&HttpResponse) + Send,
) -> SendStreamOutcome {
    let cancel = cancel.child_token();
    let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
    let _cancel_on_drop = cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let deadline = tokio::time::Instant::now() + config.timeout;
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        result = exchange_model_with_auth(request, &config, deadline, cancel.clone(), exec) => result.map(|(resp, _)| resp),
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            cancel.cancel();
            shutdown.shutdown().await;
            return SendStreamOutcome {
                response: None,
                error: Some(error),
            };
        }
    };
    let mut header_budget = (
        crate::StreamedResponse::MAX_HEADERS,
        crate::StreamedResponse::MAX_HEAD_BYTES,
    );
    if let Err(error) = message::consume_header_budget(response.headers(), &mut header_budget) {
        drop(response);
        cancel.cancel();
        shutdown.shutdown().await;
        return SendStreamOutcome {
            response: None,
            error: Some(error),
        };
    }
    let mut acc = HttpResponse {
        status: response.status().as_u16(),
        version: message::model_version(response.version()),
        reason_phrase: None,
        headers: message::headers(response.headers()),
        body: Vec::new(),
        trailers: Vec::new(),
    };
    on_progress(&acc);
    let is_streaming_response = config.force_streaming_response || is_streaming_headers(&acc.headers);
    let body_deadline = match (is_streaming_response, config.streaming_timeout) {
        (true, Some(limit)) if limit.is_zero() => None,
        (true, Some(limit)) => Some(tokio::time::Instant::now() + limit),
        _ => Some(deadline),
    };
    let mut body = response.into_body();
    let mut error = None;
    loop {
        let sleep_deadline = async {
            match body_deadline {
                Some(dl) => tokio::time::sleep_until(dl).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                error = Some(ProxyError::Cancelled);
                break;
            },
            _ = sleep_deadline => {
                error = Some(ProxyError::Timeout);
                break;
            },
            frame = body.frame() => match frame {
                None => break,
                Some(Err(err)) => {
                    error = Some(if cancel.is_cancelled() {
                        ProxyError::Cancelled
                    } else {
                        ProxyError::network(err)
                    });
                    break;
                },
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => {
                        let remaining = config.max_body_bytes.saturating_sub(acc.body.len());
                        if data.len() > remaining {
                            if remaining > 0 {
                                acc.body.extend_from_slice(&data[..remaining]);
                                on_progress(&acc);
                            }
                            error = Some(ProxyError::BodyLimit { limit: config.max_body_bytes });
                            break;
                        }
                        acc.body.extend_from_slice(&data);
                        on_progress(&acc);
                    },
                    Err(frame) => {
                        if let Ok(map) = frame.into_trailers() {
                            if let Err(err) = message::consume_header_budget(&map, &mut header_budget) {
                                error = Some(err);
                                break;
                            }
                            acc.trailers.extend(message::headers(&map));
                            on_progress(&acc);
                        }
                    },
                },
            },
        }
    }
    drop(body);
    cancel.cancel();
    shutdown.shutdown().await;
    SendStreamOutcome {
        response: Some(acc),
        error,
    }
}

pub(super) async fn send_owned(
    input: SendInput,
    config: SendConfig,
    cancel: CancellationToken,
    shutdown: Shutdown,
) -> Result<HttpResponse, ProxyError> {
    let _cancel_on_drop = cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let result = if config.streaming_timeout.is_some() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = send_once_with_streaming_timeout(input, &config, cancel.clone(), exec) => result,
        }
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = tokio::time::timeout(config.timeout, send_once(input, &config, cancel.clone(), exec)) => {
                result.unwrap_or(Err(ProxyError::Timeout))
            }
        }
    };
    cancel.cancel();
    // Stop deadlines belong to the owner; returning early would forget unjoined SDK workers.
    shutdown.shutdown().await;
    result
}

async fn send_owned_with_metrics(
    input: SendInput,
    config: SendConfig,
    cancel: CancellationToken,
    shutdown: Shutdown,
) -> Result<SendMetrics, ProxyError> {
    let _cancel_on_drop = cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let result = if config.streaming_timeout.is_some() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = send_once_with_metrics_and_streaming_timeout(input, &config, cancel.clone(), exec) => result,
        }
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = tokio::time::timeout(config.timeout, send_once_with_metrics(input, &config, cancel.clone(), exec)) => {
                result.unwrap_or(Err(ProxyError::Timeout))
            }
        }
    };
    cancel.cancel();
    shutdown.shutdown().await;
    result
}

fn is_streaming_headers(headers: &[mio_plugin_api::Header]) -> bool {
    let is_event_stream = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .and_then(|header| std::str::from_utf8(&header.value).ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    if is_event_stream {
        return true;
    }
    let has_content_length = headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("content-length"));
    let is_chunked = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
        .filter_map(|header| std::str::from_utf8(&header.value).ok())
        .any(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
        });
    !has_content_length && is_chunked
}

fn is_streaming_response_head(response: &Response) -> bool {
    let is_event_stream = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    if is_event_stream {
        return true;
    }
    let has_content_length = response.headers().contains_key("content-length");
    let is_chunked = response
        .headers()
        .get_all("transfer-encoding")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
        });
    !has_content_length && is_chunked
}

async fn send_once_with_streaming_timeout(
    input: SendInput,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<HttpResponse, ProxyError> {
    let deadline = tokio::time::Instant::now() + config.timeout;
    let response = match input {
        SendInput::Message(model) => {
            exchange_model_with_auth(model, config, deadline, cancel, exec)
                .await?
                .0
        }
        SendInput::Prepared(request) => {
            tokio::time::timeout_at(deadline, Box::pin(exchange(request, config, cancel, exec)))
                .await
                .map_err(|_| ProxyError::Timeout)??
        }
    };
    let is_streaming = config.force_streaming_response || is_streaming_response_head(&response);
    let body_deadline = match (is_streaming, config.streaming_timeout) {
        (true, Some(limit)) if limit.is_zero() => None,
        (true, Some(limit)) => Some(tokio::time::Instant::now() + limit),
        _ => Some(deadline),
    };
    match body_deadline {
        Some(dl) => tokio::time::timeout_at(dl, collect_response(response, config.max_body_bytes))
            .await
            .map_err(|_| ProxyError::Timeout)?,
        None => collect_response(response, config.max_body_bytes).await,
    }
}

async fn send_once_with_metrics_and_streaming_timeout(
    input: SendInput,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<SendMetrics, ProxyError> {
    let attempt_started = Instant::now();
    let deadline = tokio::time::Instant::now() + config.timeout;
    let (response, timing) = match input {
        SendInput::Message(model) => {
            exchange_model_with_auth(model, config, deadline, cancel, exec).await?
        }
        SendInput::Prepared(request) => tokio::time::timeout_at(
            deadline,
            Box::pin(exchange_with_metrics(request, config, cancel, exec)),
        )
        .await
        .map_err(|_| ProxyError::Timeout)??,
    };
    let exchange_returned = Instant::now();
    let reason_phrase = response
        .extensions()
        .get_ref::<rama::http::proto::h1::ext::ReasonPhrase>()
        .map(|reason| reason.as_bytes().to_vec());
    let is_streaming = config.force_streaming_response || is_streaming_response_head(&response);
    let body_deadline = match (is_streaming, config.streaming_timeout) {
        (true, Some(limit)) if limit.is_zero() => None,
        (true, Some(limit)) => Some(tokio::time::Instant::now() + limit),
        _ => Some(deadline),
    };
    let response = match body_deadline {
        Some(dl) => tokio::time::timeout_at(dl, collect_response(response, config.max_body_bytes))
            .await
            .map_err(|_| ProxyError::Timeout)??,
        None => collect_response(response, config.max_body_bytes).await?,
    };
    let received = timing
        .lock()
        .unwrap()
        .first_response_byte()
        .unwrap_or(exchange_returned);
    let response_received_ms = received
        .saturating_duration_since(attempt_started)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    Ok(SendMetrics {
        response,
        response_received_ms,
        reason_phrase,
    })
}

async fn send_once(
    input: SendInput,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<HttpResponse, ProxyError> {
    let deadline = tokio::time::Instant::now() + config.timeout;
    let response = match input {
        SendInput::Message(model) => {
            exchange_model_with_auth(model, config, deadline, cancel, exec)
                .await?
                .0
        }
        SendInput::Prepared(request) => Box::pin(exchange(request, config, cancel, exec)).await?,
    };
    collect_response(response, config.max_body_bytes).await
}

async fn send_once_with_metrics(
    input: SendInput,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<SendMetrics, ProxyError> {
    let attempt_started = Instant::now();
    let deadline = tokio::time::Instant::now() + config.timeout;
    let (response, timing) = match input {
        SendInput::Message(model) => {
            exchange_model_with_auth(model, config, deadline, cancel, exec).await?
        }
        SendInput::Prepared(request) => {
            Box::pin(exchange_with_metrics(request, config, cancel, exec)).await?
        }
    };
    let exchange_returned = Instant::now();
    let reason_phrase = response
        .extensions()
        .get_ref::<rama::http::proto::h1::ext::ReasonPhrase>()
        .map(|reason| reason.as_bytes().to_vec());
    let response = collect_response(response, config.max_body_bytes).await?;
    let received = timing
        .lock()
        .unwrap()
        .first_response_byte()
        .unwrap_or(exchange_returned);
    let response_received_ms = received
        .saturating_duration_since(attempt_started)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    Ok(SendMetrics {
        response,
        response_received_ms,
        reason_phrase,
    })
}

pub(super) async fn collect_response(
    response: Response,
    max_body_bytes: usize,
) -> Result<HttpResponse, ProxyError> {
    let mut header_budget = (
        crate::StreamedResponse::MAX_HEADERS,
        crate::StreamedResponse::MAX_HEAD_BYTES,
    );
    message::consume_header_budget(response.headers(), &mut header_budget)?;
    let status = response.status().as_u16();
    let version = message::model_version(response.version());
    let headers = message::headers(response.headers());
    let (body, trailers) =
        message::body_with_header_budget(response.into_body(), max_body_bytes, Some(header_budget))
            .await?;
    Ok(HttpResponse {
        status,
        version,
        reason_phrase: None,
        headers,
        body,
        trailers,
    })
}
