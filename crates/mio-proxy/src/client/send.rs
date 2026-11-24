use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use mio_plugin_api::{HttpRequest, HttpResponse, HttpVersion};
use rama::{
    graceful::Shutdown,
    http::{Request, Response, body::util::BodyExt},
    rt::Executor,
};
use tokio_util::sync::CancellationToken;

use crate::{ProxyError, message, transport::ResponseTiming};

use super::{
    PreparedPayload, PreparedSend, SendConfig, SendInput, SendMetrics, SendStreamOutcome,
    exchange::EphemeralNtlmClient, exchange_with_metrics,
};

pub(crate) fn find_matching_platform_auth<'a>(
    url: &str,
    config: &'a SendConfig,
) -> Option<&'a crate::PlatformAuthRuleConfig> {
    if !config.auth.platform_enabled || config.auth.platform_rules.is_empty() {
        return None;
    }
    let uri: rama::net::uri::Uri = url.parse().ok()?;
    let host = uri.host_str()?;
    config
        .auth
        .platform_rules
        .iter()
        .find(|rule| rule.matches_host(&host))
}

fn upstream_http_proxy_endpoint(url: &str, config: &SendConfig) -> Option<crate::ProxyEndpoint> {
    let uri: rama::net::uri::Uri = url.parse().ok()?;
    if !uri
        .scheme_str()
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http"))
    {
        return None;
    }
    let host = uri.host_str()?;
    match config.network.upstream_route.resolve_for_host(&host) {
        crate::UpstreamRoute::Http(ep) | crate::UpstreamRoute::Https { endpoint: ep, .. } => {
            Some(ep)
        }
        _ => None,
    }
}

pub(crate) fn should_send_proxy_auth_on_request(url: &str, config: &SendConfig) -> bool {
    upstream_http_proxy_endpoint(url, config).is_some()
}

pub(crate) fn find_matching_upstream_auth<'a>(
    url: &str,
    config: &'a SendConfig,
) -> Option<&'a crate::UpstreamAuthRuleConfig> {
    if config.auth.upstream_rules.is_empty() {
        return None;
    }
    let uri: rama::net::uri::Uri = url.parse().ok()?;
    let host = uri.host_str()?;
    let proxy_ep = upstream_http_proxy_endpoint(url, config)?;
    config
        .auth
        .upstream_rules
        .iter()
        .find(|rule| rule.matches_destination(&host) && rule.matches_proxy(&proxy_ep))
}

pub(crate) fn apply_preemptive_auth_to_model(
    request: &mut HttpRequest,
    config: &SendConfig,
) -> bool {
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
        return true;
    }
    false
}

fn resolve_preemptive_proxy_auth(request: &HttpRequest, config: &SendConfig) -> Option<String> {
    if !should_send_proxy_auth_on_request(&request.url, config) {
        return None;
    }
    let rule = find_matching_upstream_auth(&request.url, config)?;
    if rule.credentials.scheme != crate::HttpAuthScheme::Basic
        || request
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("proxy-authorization"))
    {
        return None;
    }
    Some(crate::build_basic_authorization_value(
        &rule.credentials.username,
        &rule.credentials.password,
    ))
}

fn build_request_with_proxy_auth(
    model: &HttpRequest,
    config: &SendConfig,
    proxy_auth: Option<&str>,
) -> Result<Request, ProxyError> {
    if let Some(val) = proxy_auth {
        let mut patched = model.clone();
        if config.http.strip_proxy_headers {
            patched.headers.retain(|h| {
                !h.name
                    .get(..6)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("proxy-"))
            });
        }
        crate::auth::set_or_replace_header(&mut patched.headers, "Proxy-Authorization", val.to_owned());
        let mut patched_cfg = config.clone();
        patched_cfg.http.strip_proxy_headers = false;
        patched_cfg.http.raw_http1_override = None;
        return message::to_request_with_config(&patched, &patched_cfg);
    }
    message::to_request_with_config(model, config)
}

async fn drain_auth_challenge_body(
    mut body: rama::http::Body,
    deadline: tokio::time::Instant,
    cancel: &CancellationToken,
    require_complete: bool,
) -> Result<(), ProxyError> {
    const AUTH_CHALLENGE_DRAIN_LIMIT: usize = 256 * 1024;
    let mut drained = 0usize;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            frame = tokio::time::timeout_at(deadline, body.frame()) => match frame {
                Ok(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        drained = drained.saturating_add(data.len());
                        if drained > AUTH_CHALLENGE_DRAIN_LIMIT {
                            if require_complete {
                                return Err(ProxyError::BodyLimit {
                                    limit: AUTH_CHALLENGE_DRAIN_LIMIT,
                                });
                            }
                            return Ok(());
                        }
                    }
                }
                Ok(Some(Err(err))) => return Err(ProxyError::network(err)),
                Ok(None) => return Ok(()),
                Err(_) => return Err(ProxyError::Timeout),
            },
        }
    }
}

async fn exchange_before_deadline(
    request: Request,
    config: &SendConfig,
    deadline: tokio::time::Instant,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        res = tokio::time::timeout_at(
            deadline,
            Box::pin(exchange_with_metrics(request, config, cancel.clone(), exec)),
        ) => res.map_err(|_| ProxyError::Timeout)?,
    }
}

async fn ephemeral_exchange_before_deadline(
    client: &EphemeralNtlmClient,
    request: Request,
    config: &SendConfig,
    deadline: tokio::time::Instant,
    cancel: CancellationToken,
) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        res = tokio::time::timeout_at(
            deadline,
            Box::pin(client.exchange(request, config, cancel.clone())),
        ) => res.map_err(|_| ProxyError::Timeout)?,
    }
}

fn collect_header_values(response: &Response, name: &str) -> Vec<String> {
    response
        .headers()
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|s| s.trim().to_owned())
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn exchange_ntlm_handshake(
    model: &mut HttpRequest,
    config: &SendConfig,
    creds: &crate::AuthCredentials,
    is_proxy: bool,
    proxy_auth_header: &mut Option<String>,
    auth_hdr_name: &str,
    challenge_hdr_name: &str,
    is_v2: bool,
    deadline: tokio::time::Instant,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
    let mut ntlm_config = config.clone();
    ntlm_config.http.raw_http1_override = None;
    ntlm_config.http.http1_keep_alive = true;
    ntlm_config.http.http2_reuse = false;
    ntlm_config.http.default_to_http2 = false;
    if ntlm_config.http.version != Some(HttpVersion::Http10) {
        ntlm_config.http.version = Some(HttpVersion::Http11);
    }
    let type1_val = crate::build_ntlm_type1_header_value(&creds.domain, &creds.workstation);
    if is_proxy {
        *proxy_auth_header = Some(type1_val);
    } else {
        crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, type1_val);
    }
    let mut type1_req =
        build_request_with_proxy_auth(model, &ntlm_config, proxy_auth_header.as_deref())?;
    let ntlm_client = EphemeralNtlmClient::for_request(
        &mut type1_req,
        &ntlm_config,
        cancel.clone(),
        exec,
    )?;
    let (type2_resp, type2_timing) = ephemeral_exchange_before_deadline(
        &ntlm_client,
        type1_req,
        &ntlm_config,
        deadline,
        cancel.clone(),
    )
    .await?;
    let type2_headers = collect_header_values(&type2_resp, challenge_hdr_name);
    let Some(type2_chal) = type2_headers
        .iter()
        .find_map(|raw| crate::NtlmType2Challenge::parse_from_header(raw).ok())
    else {
        return Ok((type2_resp, type2_timing));
    };
    drain_auth_challenge_body(type2_resp.into_body(), deadline, &cancel, true).await?;
    let (_, client_challenge, timestamp_100ns) = crate::auth::next_cnonce_and_challenge();
    let type3_val = crate::build_ntlm_type3_header_value(
        &type2_chal,
        creds,
        is_v2,
        client_challenge,
        timestamp_100ns,
    );
    if is_proxy {
        *proxy_auth_header = Some(type3_val);
    } else {
        crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, type3_val);
    }
    let type3_req =
        build_request_with_proxy_auth(model, &ntlm_config, proxy_auth_header.as_deref())?;
    ephemeral_exchange_before_deadline(&ntlm_client, type3_req, &ntlm_config, deadline, cancel)
        .await
}

pub(crate) async fn exchange_model_with_auth(
    mut model: HttpRequest,
    base_config: &SendConfig,
    deadline: tokio::time::Instant,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<(Response, Arc<Mutex<ResponseTiming>>), ProxyError> {
    let mut config = base_config.clone();
    let mut proxy_auth_header = resolve_preemptive_proxy_auth(&model, &config);
    if apply_preemptive_auth_to_model(&mut model, &config) || proxy_auth_header.is_some() {
        config.http.raw_http1_override = None;
    }
    let initial_req =
        build_request_with_proxy_auth(&model, &config, proxy_auth_header.as_deref())?;
    let (mut response, mut timing) = exchange_before_deadline(
        initial_req,
        &config,
        deadline,
        cancel.clone(),
        exec.clone(),
    )
    .await?;
    config.group.single_packet_barrier = None;

    let mut attempted_proxy = false;
    let mut attempted_origin = false;
    for _step in 0..2 {
        let status = response.status().as_u16();
        let (is_proxy, creds) = if status == 407 {
            if attempted_proxy || !should_send_proxy_auth_on_request(&model.url, &config) {
                break;
            }
            let Some(rule) = find_matching_upstream_auth(&model.url, &config) else {
                break;
            };
            attempted_proxy = true;
            (true, rule.credentials.clone())
        } else if status == 401 {
            if attempted_origin {
                break;
            }
            let Some(rule) = find_matching_platform_auth(&model.url, &config) else {
                break;
            };
            attempted_origin = true;
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
        let challenges = collect_header_values(&response, challenge_hdr_name);
        if challenges.is_empty() {
            break;
        }
        match creds.scheme {
            crate::HttpAuthScheme::None => break,
            crate::HttpAuthScheme::Basic => {
                let already_sent = if is_proxy {
                    proxy_auth_header.is_some()
                        || (!config.http.strip_proxy_headers
                            && model
                                .headers
                                .iter()
                                .any(|h| h.name.eq_ignore_ascii_case("proxy-authorization")))
                } else {
                    model
                        .headers
                        .iter()
                        .any(|h| h.name.eq_ignore_ascii_case(auth_hdr_name))
                };
                if already_sent {
                    break;
                }
                drain_auth_challenge_body(response.into_body(), deadline, &cancel, false).await?;
                let val = crate::build_basic_authorization_value(&creds.username, &creds.password);
                if is_proxy {
                    proxy_auth_header = Some(val);
                } else {
                    crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, val);
                }
                config.http.raw_http1_override = None;
                let retry_req =
                    build_request_with_proxy_auth(&model, &config, proxy_auth_header.as_deref())?;
                (response, timing) = exchange_before_deadline(
                    retry_req,
                    &config,
                    deadline,
                    cancel.clone(),
                    exec.clone(),
                )
                .await?;
            }
            crate::HttpAuthScheme::Digest => {
                let Some(digest_chal) = challenges
                    .iter()
                    .find_map(|raw| crate::DigestChallenge::parse_from_header(raw))
                else {
                    break;
                };
                drain_auth_challenge_body(response.into_body(), deadline, &cancel, false).await?;
                let (cnonce, _, _) = crate::auth::next_cnonce_and_challenge();
                let uri_path = if is_proxy && model.url.to_ascii_lowercase().starts_with("http://") {
                    model
                        .url
                        .split('#')
                        .next()
                        .unwrap_or(&model.url)
                        .to_owned()
                } else {
                    crate::auth::request_uri_path_and_query(&model.url)
                };
                let val = digest_chal.build_authorization_value(&crate::auth::DigestRequest {
                    username: &creds.username,
                    password: &creds.password,
                    method: &model.method,
                    uri_path: &uri_path,
                    body: &model.body,
                    nc: 1,
                    cnonce: &cnonce,
                });
                if is_proxy {
                    proxy_auth_header = Some(val);
                } else {
                    crate::auth::set_or_replace_header(&mut model.headers, auth_hdr_name, val);
                }
                config.http.raw_http1_override = None;
                let retry_req =
                    build_request_with_proxy_auth(&model, &config, proxy_auth_header.as_deref())?;
                (response, timing) = exchange_before_deadline(
                    retry_req,
                    &config,
                    deadline,
                    cancel.clone(),
                    exec.clone(),
                )
                .await?;
            }
            crate::HttpAuthScheme::NtlmV1 | crate::HttpAuthScheme::NtlmV2 => {
                let is_v2 = creds.scheme == crate::HttpAuthScheme::NtlmV2;
                if let Some(type2_chal) = challenges
                    .iter()
                    .find_map(|raw| crate::NtlmType2Challenge::parse_from_header(raw).ok())
                {
                    drain_auth_challenge_body(
                        response.into_body(),
                        deadline,
                        &cancel,
                        config.http.use_connection_pool,
                    )
                    .await?;
                    let (_, client_challenge, timestamp_100ns) =
                        crate::auth::next_cnonce_and_challenge();
                    let type3_val = crate::build_ntlm_type3_header_value(
                        &type2_chal,
                        &creds,
                        is_v2,
                        client_challenge,
                        timestamp_100ns,
                    );
                    if is_proxy {
                        proxy_auth_header = Some(type3_val);
                    } else {
                        crate::auth::set_or_replace_header(
                            &mut model.headers,
                            auth_hdr_name,
                            type3_val,
                        );
                    }
                    config.http.raw_http1_override = None;
                    let type3_req = build_request_with_proxy_auth(
                        &model,
                        &config,
                        proxy_auth_header.as_deref(),
                    )?;
                    (response, timing) = exchange_before_deadline(
                        type3_req,
                        &config,
                        deadline,
                        cancel.clone(),
                        exec.clone(),
                    )
                    .await?;
                    continue;
                }
                if !challenges.iter().any(|raw| {
                    raw.eq_ignore_ascii_case("ntlm")
                        || raw.starts_with("NTLM")
                        || raw.starts_with("ntlm")
                }) {
                    break;
                }
                drain_auth_challenge_body(response.into_body(), deadline, &cancel, false).await?;
                (response, timing) = Box::pin(exchange_ntlm_handshake(
                    &mut model,
                    &config,
                    &creds,
                    is_proxy,
                    &mut proxy_auth_header,
                    auth_hdr_name,
                    challenge_hdr_name,
                    is_v2,
                    deadline,
                    cancel.clone(),
                    exec.clone(),
                ))
                .await?;
            }
        }
    }
    Ok((response, timing))
}

pub fn prepare_send(
    request: &HttpRequest,
    mut config: SendConfig,
) -> Result<PreparedSend, ProxyError> {
    if config.auth.is_active() {
        let mut model = request.clone();
        if apply_preemptive_auth_to_model(&mut model, &config)
            || resolve_preemptive_proxy_auth(&model, &config).is_some()
        {
            config.http.raw_http1_override = None;
        }
        message::validate_request_with_config(&model, &config)?;
        Ok(PreparedSend {
            payload: PreparedPayload::WithAuth(model),
            config,
        })
    } else {
        Ok(PreparedSend {
            payload: PreparedPayload::Ready(message::to_request_with_config(request, &config)?),
            config,
        })
    }
}

impl PreparedSend {
    fn into_input(self) -> SendInput {
        match self.payload {
            PreparedPayload::Ready(req) => SendInput::Prepared(req),
            PreparedPayload::WithAuth(model) => SendInput::Message(model),
        }
    }

    /// Sends once and retains the original cancellation and SDK-drain ownership contract.
    pub async fn send(self, cancel: CancellationToken) -> Result<HttpResponse, ProxyError> {
        let cancel = cancel.child_token();
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        let config = self.config.clone();
        send_owned(self.into_input(), config, cancel, shutdown).await
    }

    /// Sends once and reports the first-byte response latency alongside the response.
    pub async fn send_with_metrics(
        self,
        cancel: CancellationToken,
    ) -> Result<SendMetrics, ProxyError> {
        let cancel = cancel.child_token();
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        let config = self.config.clone();
        send_owned_metrics(self.into_input(), config, cancel, shutdown).await
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
    let deadline = tokio::time::Instant::now() + config.limits.timeout;
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
        reason_phrase: message::reason_phrase(&response),
        headers: message::headers(response.headers()),
        body: Vec::new(),
        trailers: Vec::new(),
    };
    on_progress(&acc);
    let is_streaming_response = config.http.force_streaming_response
        || crate::streaming::is_streaming_headers(&acc.headers);
    let body_deadline = crate::streaming::response_body_deadline(
        is_streaming_response,
        config.limits.streaming_timeout,
        deadline,
    );
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
                        let remaining = config.limits.max_body_bytes.saturating_sub(acc.body.len());
                        if data.len() > remaining {
                            if remaining > 0 {
                                acc.body.extend_from_slice(&data[..remaining]);
                                on_progress(&acc);
                            }
                            error = Some(ProxyError::BodyLimit { limit: config.limits.max_body_bytes });
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
    send_owned_metrics(input, config, cancel, shutdown)
        .await
        .map(|metrics| metrics.response)
}

async fn send_owned_metrics(
    input: SendInput,
    config: SendConfig,
    cancel: CancellationToken,
    shutdown: Shutdown,
) -> Result<SendMetrics, ProxyError> {
    let _cancel_on_drop = cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        result = send_once_metrics(input, &config, cancel.clone(), exec) => result,
    };
    cancel.cancel();
    // Stop deadlines belong to the owner; returning early would forget unjoined SDK workers.
    shutdown.shutdown().await;
    result
}

async fn send_once_metrics(
    input: SendInput,
    config: &SendConfig,
    cancel: CancellationToken,
    exec: Executor,
) -> Result<SendMetrics, ProxyError> {
    let attempt_started = Instant::now();
    let deadline = tokio::time::Instant::now() + config.limits.timeout;
    let (response, timing) = match input {
        SendInput::Message(model) => {
            exchange_model_with_auth(model, config, deadline, cancel, exec).await?
        }
        SendInput::Prepared(request) => {
            exchange_before_deadline(request, config, deadline, cancel, exec).await?
        }
    };
    let exchange_returned = Instant::now();
    let is_streaming = config.http.force_streaming_response
        || crate::streaming::is_streaming_response_head(&response);
    let body_deadline = if config.limits.streaming_timeout.is_some() {
        crate::streaming::response_body_deadline(
            is_streaming,
            config.limits.streaming_timeout,
            deadline,
        )
    } else {
        Some(deadline)
    };
    let response = match body_deadline {
        Some(dl) => tokio::time::timeout_at(
            dl,
            collect_response(response, config.limits.max_body_bytes),
        )
        .await
        .map_err(|_| ProxyError::Timeout)??,
        None => collect_response(response, config.limits.max_body_bytes).await?,
    };
    let received = crate::lock_unpoison(&timing)
        .first_response_byte()
        .unwrap_or(exchange_returned);
    let response_received_ms = received
        .saturating_duration_since(attempt_started)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let reason_phrase = response.reason_phrase.clone();
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
    let reason_phrase = message::reason_phrase(&response);
    let headers = message::headers(response.headers());
    let (body, trailers) =
        message::body_with_header_budget(response.into_body(), max_body_bytes, Some(header_budget))
            .await?;
    Ok(HttpResponse {
        status,
        version,
        reason_phrase,
        headers,
        body,
        trailers,
    })
}
