use super::relay::now_unix_ms;
use super::*;
use crate::server::{InterceptMode, WebSocketHookDecision, WebSocketInterceptDecision};
use rama::{
    futures::{
        StreamExt,
        stream::{SplitSink, SplitStream},
    },
    http::ws::{AsyncWebSocket, Message},
};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use tokio::sync::Mutex as AsyncMutex;

impl Relay<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn intercept_or_forward(
        &self,
        message: Message,
        direction: WebSocketDirection,
        upgrade_request: &mio_plugin_api::HttpRequest,
        reader: &mut SplitStream<AsyncWebSocket>,
        writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        source_closed: &AtomicBool,
        destination_closed: &AtomicBool,
        queued: &mut VecDeque<Message>,
        reader_eof: &mut bool,
    ) -> Result<(), ProxyError> {
        let (kind, original_bytes) = match &message {
            Message::Text(text) => (WebSocketKind::Text, text.as_bytes().to_vec()),
            Message::Binary(bytes) => (WebSocketKind::Binary, bytes.to_vec()),
            _ => unreachable!("only data messages reach WebSocket interception"),
        };
        let id = Uuid::new_v4();
        let mut working_bytes = original_bytes.clone();
        let mut hook_modified = false;
        let mut force_intercept = false;
        if let Some(next) =
            self.apply_ws_transform(direction, kind, upgrade_request, &working_bytes)?
            && next != original_bytes
        {
            hook_modified = true;
            working_bytes = next;
        }
        let (_, ws_hook) = self.state.plugin_hooks.borrow().clone();
        if let Some(hook) = ws_hook {
            let deadline = Instant::now()
                .checked_add(self.state.config.request_timeout)
                .ok_or(ProxyError::Timeout)?;
            let decision = tokio::select! {
                biased;
                _ = self.state.cancel.cancelled() => return Err(ProxyError::Cancelled),
                res = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    hook(
                        self.flow_id,
                        id,
                        upgrade_request.url.clone(),
                        direction,
                        kind,
                        working_bytes.clone(),
                        self.state.cancel.child_token(),
                        deadline,
                    ),
                ) => res.map_err(|_| ProxyError::Timeout)??,
            };
            match decision {
                WebSocketHookDecision::Pass => {}
                WebSocketHookDecision::Drop => return Ok(()),
                WebSocketHookDecision::Modify(next) => {
                    client::websocket_outgoing_message(
                        kind,
                        &next,
                        self.state.config.max_body_bytes,
                    )?;
                    if next != original_bytes {
                        hook_modified = true;
                        working_bytes = next;
                    }
                }
                WebSocketHookDecision::ForceIntercept(Some(next)) => {
                    client::websocket_outgoing_message(
                        kind,
                        &next,
                        self.state.config.max_body_bytes,
                    )?;
                    if next != original_bytes {
                        hook_modified = true;
                        working_bytes = next;
                    }
                    force_intercept = true;
                }
                WebSocketHookDecision::ForceIntercept(None) => force_intercept = true,
            }
        }
        if self.closing.is_cancelled() {
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
            self.emit_message(
                id,
                sequence,
                now_unix_ms(),
                direction,
                kind,
                working_bytes.clone(),
                hook_modified.then_some(original_bytes),
            )
            .await?;
            return Ok(());
        }
        let should_intercept =
            force_intercept || self.should_intercept(direction, upgrade_request)?;
        if !should_intercept {
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
            if hook_modified {
                let outgoing = client::websocket_outgoing_message(
                    kind,
                    &working_bytes,
                    self.state.config.max_body_bytes,
                )?;
                self.emit_message(
                    id,
                    sequence,
                    now_unix_ms(),
                    direction,
                    kind,
                    working_bytes,
                    Some(original_bytes),
                )
                .await?;
                self.send_message(writer, outgoing, false, destination_closed)
                    .await?;
            } else {
                self.emit_message(
                    id,
                    sequence,
                    now_unix_ms(),
                    direction,
                    kind,
                    working_bytes,
                    None,
                )
                .await?;
                self.send_message(writer, message, false, destination_closed)
                    .await?;
            }
            return Ok(());
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let observed_unix_ms = now_unix_ms();
        let record = WebSocketMessage {
            id,
            revision: 1,
            flow_id: self.flow_id,
            sequence,
            direction,
            kind,
            payload: working_bytes.clone(),
            observed_unix_ms,
        };
        let (reply, mut receive) = tokio::sync::oneshot::channel();
        self.state
            .emit(ProxyEvent::WebSocketIntercepted {
                project: self.state.project,
                url: upgrade_request.url.clone(),
                record,
                listener_port: self.state.listener_port,
                reply,
            })
            .await?;
        let mut mode = self.state.intercept.clone();
        let master_enabled = |mode: InterceptMode| mode.requests || mode.responses;
        let timeout = tokio::time::sleep(self.state.config.intercept_timeout);
        tokio::pin!(timeout);
        let outcome: Result<Option<WebSocketInterceptDecision>, ProxyError> = loop {
            if self.state.cancel.is_cancelled() {
                break Err(ProxyError::Cancelled);
            }
            if self.closing.is_cancelled()
                || (!force_intercept && !master_enabled(*mode.borrow_and_update()))
            {
                break Ok(None);
            }
            tokio::select! {
                biased;
                _ = self.state.cancel.cancelled() => break Err(ProxyError::Cancelled),
                _ = &mut timeout => break Err(ProxyError::Timeout),
                changed = mode.changed() => {
                    if changed.is_err() {
                        break Err(ProxyError::Cancelled);
                    }
                }
                decision = &mut receive => {
                    break match decision {
                        Ok(decision) => Ok(Some(decision)),
                        Err(_) => Err(ProxyError::EventConsumerClosed),
                    };
                }
                next = reader.next(), if !*reader_eof && queued.len() < MAX_QUEUED_DATA_MESSAGES => {
                    match next {
                        Some(result) => match result {
                            Ok(next_message) => match next_message {
                                Message::Frame(_) => {
                                    break Err(ProxyError::Network("WebSocket SDK 返回了未解码帧".into()));
                                }
                                next_message @ (Message::Close(_) | Message::Ping(_) | Message::Pong(_)) => {
                                    self.handle_control(
                                        next_message,
                                        writer,
                                        source_closed,
                                        destination_closed,
                                        direction,
                                    )
                                    .await?;
                                    if self.closing.is_cancelled() {
                                        break Ok(None);
                                    }
                                }
                                next_message => queued.push_back(next_message),
                            },
                            Err(error) => break Err(ProxyError::network(error)),
                        },
                        None => *reader_eof = true,
                    }
                }
            }
        };
        match outcome {
            Ok(Some(WebSocketInterceptDecision::Drop)) => Ok(()),
            Ok(Some(WebSocketInterceptDecision::Forward(payload))) => {
                if payload.len() > self.state.config.max_body_bytes {
                    return Err(ProxyError::BodyLimit {
                        limit: self.state.config.max_body_bytes,
                    });
                }
                let forwarded = match kind {
                    WebSocketKind::Text => Message::Text(
                        std::str::from_utf8(&payload)
                            .map_err(ProxyError::invalid)?
                            .into(),
                    ),
                    WebSocketKind::Binary => Message::Binary(payload.clone().into()),
                    _ => unreachable!("only data messages reach WebSocket interception"),
                };
                let original_payload = if payload == original_bytes {
                    None
                } else {
                    Some(original_bytes)
                };
                self.emit_message(
                    id,
                    sequence,
                    observed_unix_ms,
                    direction,
                    kind,
                    payload,
                    original_payload,
                )
                .await?;
                if !self.closing.is_cancelled() {
                    self.send_message(writer, forwarded, false, destination_closed)
                        .await?;
                }
                Ok(())
            }
            Ok(None) => {
                self.emit_resolved(id).await?;
                let outgoing = if hook_modified {
                    client::websocket_outgoing_message(
                        kind,
                        &working_bytes,
                        self.state.config.max_body_bytes,
                    )?
                } else {
                    message
                };
                let original_payload = if working_bytes == original_bytes {
                    None
                } else {
                    Some(original_bytes)
                };
                self.emit_message(
                    id,
                    sequence,
                    observed_unix_ms,
                    direction,
                    kind,
                    working_bytes,
                    original_payload,
                )
                .await?;
                if !self.closing.is_cancelled() {
                    self.send_message(writer, outgoing, false, destination_closed)
                        .await?;
                }
                Ok(())
            }
            Err(error) => {
                let _ = self.emit_resolved(id).await;
                Err(error)
            }
        }
    }
}
