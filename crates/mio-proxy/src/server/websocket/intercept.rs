use super::relay::RelayLeg;
use super::*;
use crate::server::{DynamicProxyState, WebSocketHookDecision, WebSocketInterceptDecision};
use rama::{futures::StreamExt, http::ws::Message};
use std::{sync::atomic::Ordering, time::Instant};
use tokio::sync::oneshot;

struct HookStageOutcome {
    working_bytes: Vec<u8>,
    hook_modified: bool,
    force_intercept: bool,
}

impl RelayLeg<'_> {
    async fn apply_hooks_and_transform(
        &self,
        id: Uuid,
        kind: WebSocketKind,
        original_bytes: &[u8],
    ) -> Result<Option<HookStageOutcome>, ProxyError> {
        let mut working_bytes = original_bytes.to_vec();
        let mut hook_modified = false;
        let mut force_intercept = false;
        if let Some(next) =
            self.relay
                .apply_ws_transform(self.direction, kind, self.upgrade_request, &working_bytes)?
            && next != original_bytes
        {
            hook_modified = true;
            working_bytes = next;
        }
        let ws_hook = self.relay.state.dynamic.borrow().async_websocket_hook.clone();
        if let Some(hook) = ws_hook {
            let deadline = Instant::now()
                .checked_add(self.relay.state.config.limits.request_timeout)
                .ok_or(ProxyError::Timeout)?;
            let decision = tokio::select! {
                biased;
                _ = self.relay.state.cancel.cancelled() => return Err(ProxyError::Cancelled),
                res = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    hook(
                        self.relay.flow_id,
                        id,
                        self.upgrade_request.url.clone(),
                        self.direction,
                        kind,
                        working_bytes.clone(),
                        self.relay.state.cancel.child_token(),
                        deadline,
                    ),
                ) => res.map_err(|_| ProxyError::Timeout)??,
            };
            let max_bytes = self.relay.state.config.limits.max_body_bytes;
            match decision {
                WebSocketHookDecision::Pass => {}
                WebSocketHookDecision::Drop => return Ok(None),
                WebSocketHookDecision::Modify(next) => {
                    client::validate_websocket_payload(kind, &next, max_bytes)?;
                    if next != original_bytes {
                        hook_modified = true;
                        working_bytes = next;
                    }
                }
                WebSocketHookDecision::ForceIntercept(Some(next)) => {
                    client::validate_websocket_payload(kind, &next, max_bytes)?;
                    if next != original_bytes {
                        hook_modified = true;
                        working_bytes = next;
                    }
                    force_intercept = true;
                }
                WebSocketHookDecision::ForceIntercept(None) => force_intercept = true,
            }
        }
        Ok(Some(HookStageOutcome {
            working_bytes,
            hook_modified,
            force_intercept,
        }))
    }

    async fn wait_for_intercept_decision(
        &mut self,
        force_intercept: bool,
        mut receive: oneshot::Receiver<WebSocketInterceptDecision>,
    ) -> Result<Option<WebSocketInterceptDecision>, ProxyError> {
        let mut dynamic_rx = self.relay.state.dynamic.clone();
        let master_enabled =
            |state: &DynamicProxyState| state.intercept.requests || state.intercept.responses;
        let timeout = tokio::time::sleep(self.relay.state.config.limits.intercept_timeout);
        tokio::pin!(timeout);
        loop {
            if self.relay.state.cancel.is_cancelled() {
                return Err(ProxyError::Cancelled);
            }
            if self.relay.closing.is_cancelled()
                || (!force_intercept && !master_enabled(&dynamic_rx.borrow_and_update()))
            {
                return Ok(None);
            }
            tokio::select! {
                biased;
                _ = self.relay.state.cancel.cancelled() => return Err(ProxyError::Cancelled),
                _ = &mut timeout => return Err(ProxyError::Timeout),
                changed = dynamic_rx.changed() => {
                    if changed.is_err() {
                        return Err(ProxyError::Cancelled);
                    }
                }
                decision = &mut receive => {
                    return decision.map(Some).map_err(|_| ProxyError::EventConsumerClosed);
                }
                next = self.reader.next(), if !self.reader_eof && self.queued.len() < MAX_QUEUED_DATA_MESSAGES => {
                    match next {
                        Some(Ok(Message::Frame(_))) => {
                            return Err(ProxyError::Network("WebSocket SDK 返回了未解码帧".into()));
                        }
                        Some(Ok(next_message @ (Message::Close(_) | Message::Ping(_) | Message::Pong(_)))) => {
                            self.handle_control(next_message).await?;
                            if self.relay.closing.is_cancelled() {
                                return Ok(None);
                            }
                        }
                        Some(Ok(next_message)) => self.queued.push_back(next_message),
                        Some(Err(error)) => return Err(ProxyError::network(error)),
                        None => self.reader_eof = true,
                    }
                }
            }
        }
    }

    async fn emit_and_forward_frame(
        &self,
        record: WebSocketMessage,
        original_payload: Option<Vec<u8>>,
        outgoing: Message,
    ) -> Result<(), ProxyError> {
        self.relay.emit_message(record, original_payload).await?;
        if !self.relay.closing.is_cancelled() {
            self.relay
                .send_message(self.writer, outgoing, false, self.destination_closed)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn intercept_or_forward(&mut self, message: Message) -> Result<(), ProxyError> {
        let (kind, original_bytes) = match &message {
            Message::Text(text) => (WebSocketKind::Text, text.as_bytes().to_vec()),
            Message::Binary(bytes) => (WebSocketKind::Binary, bytes.to_vec()),
            _ => unreachable!("only data messages reach WebSocket interception"),
        };
        let id = Uuid::new_v4();
        let Some(HookStageOutcome {
            working_bytes,
            hook_modified,
            force_intercept,
        }) = self
            .apply_hooks_and_transform(id, kind, &original_bytes)
            .await?
        else {
            return Ok(());
        };
        if self.relay.closing.is_cancelled() {
            let record = WebSocketMessage {
                id,
                revision: 1,
                flow_id: self.relay.flow_id,
                sequence: self.relay.sequence.fetch_add(1, Ordering::Relaxed),
                direction: self.direction,
                kind,
                payload: working_bytes,
                observed_unix_ms: crate::now_unix_ms(),
            };
            self.relay
                .emit_message(record, hook_modified.then_some(original_bytes))
                .await?;
            return Ok(());
        }
        let should_intercept =
            force_intercept || self.relay.should_intercept(self.direction, self.upgrade_request)?;
        let sequence = self.relay.sequence.fetch_add(1, Ordering::Relaxed);
        let observed_unix_ms = crate::now_unix_ms();
        let max_bytes = self.relay.state.config.limits.max_body_bytes;
        if !should_intercept {
            let outgoing = if hook_modified {
                client::websocket_outgoing_message(kind, &working_bytes, max_bytes)?
            } else {
                message
            };
            let record = WebSocketMessage {
                id,
                revision: 1,
                flow_id: self.relay.flow_id,
                sequence,
                direction: self.direction,
                kind,
                payload: working_bytes,
                observed_unix_ms,
            };
            return self
                .emit_and_forward_frame(
                    record,
                    hook_modified.then_some(original_bytes),
                    outgoing,
                )
                .await;
        }
        let record = WebSocketMessage {
            id,
            revision: 1,
            flow_id: self.relay.flow_id,
            sequence,
            direction: self.direction,
            kind,
            payload: working_bytes.clone(),
            observed_unix_ms,
        };
        let (reply, receive) = oneshot::channel();
        self.relay
            .state
            .emit(ProxyEvent::WebSocketIntercepted {
                project: self.relay.state.project,
                url: self.upgrade_request.url.clone(),
                record,
                listener_port: self.relay.state.listener_port,
                reply,
            })
            .await?;
        let outcome = self
            .wait_for_intercept_decision(force_intercept, receive)
            .await;
        match outcome {
            Ok(Some(WebSocketInterceptDecision::Drop)) => Ok(()),
            Ok(Some(WebSocketInterceptDecision::Forward(payload))) => {
                let forwarded = client::websocket_outgoing_message(kind, &payload, max_bytes)?;
                let original_payload = (payload != original_bytes).then_some(original_bytes);
                let record = WebSocketMessage {
                    id,
                    revision: 1,
                    flow_id: self.relay.flow_id,
                    sequence,
                    direction: self.direction,
                    kind,
                    payload,
                    observed_unix_ms,
                };
                self.emit_and_forward_frame(record, original_payload, forwarded)
                    .await
            }
            Ok(None) => {
                self.relay.emit_resolved(id).await?;
                let outgoing = if hook_modified {
                    client::websocket_outgoing_message(kind, &working_bytes, max_bytes)?
                } else {
                    message
                };
                let original_payload = (working_bytes != original_bytes).then_some(original_bytes);
                let record = WebSocketMessage {
                    id,
                    revision: 1,
                    flow_id: self.relay.flow_id,
                    sequence,
                    direction: self.direction,
                    kind,
                    payload: working_bytes,
                    observed_unix_ms,
                };
                self.emit_and_forward_frame(record, original_payload, outgoing)
                    .await
            }
            Err(error) => {
                let _ = self.relay.emit_resolved(id).await;
                Err(error)
            }
        }
    }
}
