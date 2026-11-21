use super::{ActiveFlow, CapturedFlowMetrics, ProxyEvent, State};
use crate::{ProxyError, client, message};
use mio_plugin_api::{
    HttpResponse, HttpVersion, Uuid, WebSocketDirection, WebSocketKind, WebSocketMessage,
};
use rama::{
    extensions::ExtensionsRef,
    http::{
        HeaderValue, Request, Response, StatusCode,
        headers::{HeaderMapExt, SecWebSocketAccept},
        io::upgrade::handle_upgrade,
        ws::Message,
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicU64},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

mod handshake;
mod intercept;
mod registry;
mod relay;
#[cfg(test)]
mod tests;

#[cfg(test)]
use handshake::outgoing_request;
#[cfg(test)]
use handshake::request_handshake;
use handshake::request_handshake_with_options;
pub(super) use handshake::{outgoing_request_with_options, validate_outgoing_request};
use relay::relay;

pub(super) fn requested(request: &Request) -> bool {
    message::has_header_token(request.headers(), "upgrade", "websocket")
        || request.headers().contains_key("sec-websocket-key")
        || request.headers().contains_key("sec-websocket-version")
}

pub(super) async fn handle(request: Request, mut active: ActiveFlow) -> Response {
    let state = active.state.clone();
    let cancel = state.cancel.child_token();
    let cancel_on_drop = cancel.clone().drop_guard();
    let ingress_upgrade = handle_upgrade(request.extensions().clone());
    let result = async {
        let record = active.record.as_mut().expect("active flow owns its record");
        let strip_ws_extensions = state.config.http.strip_sec_websocket_extensions;
        let (_, ingress_handshake) =
            request_handshake_with_options(&record.request, strip_ws_extensions)?;
        let (body, trailers) = tokio::select! {
            biased;
            _ = state.cancel.cancelled() => return Err(ProxyError::Cancelled),
            result = tokio::time::timeout(
                state.config.limits.request_timeout,
                message::body(request.into_body(), state.config.limits.max_body_bytes),
            ) => result.map_err(|_| ProxyError::Timeout)??,
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
                capture_metrics: active.capture_metrics.clone(),
            })
            .await?;
        (active.request_was_intercepted, active.request_was_modified) = state
            .intercept(
                record,
                false,
                None,
                &active.capture_metrics,
                false,
                false,
            )
            .await?;
        let (request, egress_handshake) = outgoing_request_with_options(
            &record.request,
            state.config.limits.max_body_bytes,
            strip_ws_extensions,
        )?;
        let config = state.send_config(&record.request.url, record.request.version, true);
        let attempt_started = std::time::Instant::now();
        let (mut upstream, timing) = tokio::select! {
            biased;
            _ = state.cancel.cancelled() => return Err(ProxyError::Cancelled),
            result = tokio::time::timeout(
                config.limits.timeout,
                client::exchange_with_metrics(request, &config, cancel.clone(), state.exec.clone()),
            ) => result.map_err(|_| ProxyError::Timeout)??,
        };
        let reason_phrase = message::reason_phrase(&upstream);
        let (remote_ip, received) = {
            let guard = crate::lock_unpoison(&timing);
            (guard.peer_ip(), guard.first_response_byte())
        };
        let response_received_ms = received.map(|received| {
            received
                .saturating_duration_since(attempt_started)
                .as_millis()
                .min(u64::MAX as u128) as u64
        });
        active.capture_metrics = CapturedFlowMetrics {
            remote_ip,
            response_received_ms,
            reason_phrase: reason_phrase.clone(),
        };
        let upgraded = upstream.status() == StatusCode::SWITCHING_PROTOCOLS;
        let egress_upgrade = upgraded.then(|| handle_upgrade(upstream.extensions().clone()));
        if upgraded {
            egress_handshake.validate_response(upstream.headers())?;
            // Request interception may change the upstream nonce, but not the client's nonce.
            upstream
                .headers_mut()
                .typed_insert(ingress_handshake.accept.clone());
            ingress_handshake.validate_response(upstream.headers())?;
        }
        let status = upstream.status().as_u16();
        let version = message::model_version(upstream.version());
        let headers = message::headers(upstream.headers());
        let (body, trailers) = if upgraded {
            (Vec::new(), Vec::new())
        } else {
            tokio::select! {
                biased;
                _ = state.cancel.cancelled() => return Err(ProxyError::Cancelled),
                result = tokio::time::timeout(
                    config.limits.timeout,
                    message::body(upstream.into_body(), config.limits.max_body_bytes),
                ) => result.map_err(|_| ProxyError::Timeout)??,
            }
        };
        record.summary.status = Some(status);
        record.summary.response_bytes = body.len() as u64;
        record.response = Some(HttpResponse {
            status,
            version,
            reason_phrase,
            headers,
            body,
            trailers,
        });
        state
            .intercept(
                record,
                true,
                None,
                &active.capture_metrics,
                active.request_was_intercepted,
                active.request_was_modified,
            )
            .await?;
        let response = record
            .response
            .as_mut()
            .expect("upstream response is present");
        if response.status != 101 {
            return Ok((
                message::to_response(
                    response,
                    config.limits.max_body_bytes,
                    HttpVersion::Http11,
                    false,
                )?,
                None,
            ));
        }
        if !upgraded || !response.body.is_empty() || !response.trailers.is_empty() {
            return Err(ProxyError::invalid(
                "WebSocket 101 必须对应真实升级且不携带 HTTP 正文",
            ));
        }
        let headers = message::header_map(&response.headers)?;
        ingress_handshake.validate_response(&headers)?;
        let mut outgoing = message::to_response(
            response,
            config.limits.max_body_bytes,
            HttpVersion::Http11,
            false,
        )?;
        outgoing
            .headers_mut()
            .insert("connection", HeaderValue::from_static("Upgrade"));
        outgoing
            .headers_mut()
            .insert("upgrade", HeaderValue::from_static("websocket"));
        outgoing
            .headers_mut()
            .typed_insert(ingress_handshake.accept);
        if let Some(protocol) = headers.get("sec-websocket-protocol") {
            outgoing
                .headers_mut()
                .insert("sec-websocket-protocol", protocol.clone());
        }
        response.headers = message::headers(outgoing.headers());
        record.summary.revision += 1;
        state
            .emit(ProxyEvent::Captured {
                project: state.project,
                record: record.clone(),
                response_stream: None,
                listener_port: (state.listener_port != 0).then_some(state.listener_port),
                capture_metrics: active.capture_metrics.clone(),
            })
            .await?;
        Ok((outgoing, egress_upgrade))
    }
    .await;

    match result {
        Err(error) => {
            active.finish(Err(&error)).await;
            state.error_response(&error)
        }
        Ok((response, None)) => {
            active.finish(Ok(())).await;
            response
        }
        Ok((response, Some(egress_upgrade))) => {
            let task_state = state.clone();
            let record = active.record.as_ref().expect("active flow owns its record");
            let flow_id = record.summary.id;
            let upgrade_request = record.request.clone();
            state.tasks.spawn(async move {
                let _cancel_on_drop = cancel_on_drop;
                let result = tokio::select! {
                    biased;
                    _ = task_state.cancel.cancelled() => Err(ProxyError::Cancelled),
                    result = async {
                        let (ingress, egress) = tokio::time::timeout(
                            task_state.config.limits.request_timeout,
                            async { tokio::try_join!(ingress_upgrade, egress_upgrade) },
                        )
                        .await
                        .map_err(|_| ProxyError::Timeout)?
                        .map_err(ProxyError::network)?;
                        relay(ingress, egress, &task_state, flow_id, upgrade_request).await
                    } => result,
                };
                cancel.cancel();
                active.finish(result.as_ref().map(|_| ())).await;
            });
            response
        }
    }
}

pub(super) struct Handshake {
    accept: SecWebSocketAccept,
    protocols: Vec<String>,
    offered_extensions: bool,
}

pub(super) struct InjectRequest {
    direction: WebSocketDirection,
    kind: WebSocketKind,
    payload: Vec<u8>,
    message: Option<Message>,
    reply: oneshot::Sender<Result<WebSocketMessage, ProxyError>>,
}

struct WebSocketInjector {
    token: u64,
    sender: mpsc::Sender<InjectRequest>,
}

struct InjectorInner {
    next_token: u64,
    entries: HashMap<Uuid, WebSocketInjector>,
}

pub(super) struct InjectorRegistrationGuard {
    registry: WebSocketInjectorRegistry,
    flow_id: Uuid,
    token: u64,
}

#[derive(Clone)]
pub struct WebSocketInjectorRegistry {
    inner: Arc<Mutex<InjectorInner>>,
}

struct Relay<'a> {
    state: &'a State,
    flow_id: Uuid,
    sequence: AtomicU64,
    closing: CancellationToken,
}

const MAX_QUEUED_DATA_MESSAGES: usize = 8;
