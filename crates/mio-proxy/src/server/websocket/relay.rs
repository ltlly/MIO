use super::*;
use rama::{
    futures::{
        SinkExt, StreamExt,
        stream::{SplitSink, SplitStream},
    },
    http::{
        io::upgrade::Upgraded,
        ws::{
            AsyncWebSocket, Message, ProtocolError,
            protocol::{Role, WebSocketConfig},
        },
    },
};
use std::time::Instant;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex as AsyncMutex;

pub(super) async fn relay(
    ingress: Upgraded,
    egress: Upgraded,
    state: &State,
    flow_id: Uuid,
    upgrade_request: mio_plugin_api::HttpRequest,
) -> Result<(), ProxyError> {
    let config = WebSocketConfig::default()
        .with_read_buffer_size(4096)
        .with_write_buffer_size(0)
        .with_max_write_buffer_size(state.config.max_body_bytes.saturating_add(256))
        .with_max_message_size(state.config.max_body_bytes)
        .with_max_frame_size(state.config.max_body_bytes);
    let ingress = AsyncWebSocket::from_raw_socket(ingress, Role::Server, Some(config)).await;
    let egress = AsyncWebSocket::from_raw_socket(egress, Role::Client, Some(config)).await;
    let (ingress_writer, ingress_reader) = ingress.split();
    let (egress_writer, egress_reader) = egress.split();
    let ingress_writer = AsyncMutex::new(ingress_writer);
    let egress_writer = AsyncMutex::new(egress_writer);
    let ingress_closed = AtomicBool::new(false);
    let egress_closed = AtomicBool::new(false);
    let relay = Relay {
        state,
        flow_id,
        sequence: AtomicU64::new(1),
        closing: CancellationToken::new(),
    };
    let (command_sender, mut commands) = mpsc::channel(8);
    let _registration = state.injectors.register(flow_id, command_sender);
    tokio::select! {
        result = async {
            tokio::try_join!(
                relay.direction(
                    ingress_reader,
                    &egress_writer,
                    &ingress_closed,
                    &egress_closed,
                    WebSocketDirection::ClientToServer,
                    upgrade_request.clone(),
                ),
                relay.direction(
                    egress_reader,
                    &ingress_writer,
                    &egress_closed,
                    &ingress_closed,
                    WebSocketDirection::ServerToClient,
                    upgrade_request,
                ),
            )?;
            Ok(())
        } => result,
        command = async {
            while let Some(request) = commands.recv().await {
                let outcome = relay
                    .inject_frame(
                        request.direction,
                        request.kind,
                        request.payload,
                        &ingress_writer,
                        &egress_writer,
                        &ingress_closed,
                        &egress_closed,
                    )
                    .await;
                let _ = request.reply.send(outcome);
            }
            std::future::pending::<Result<(), ProxyError>>().await
        } => command,
        _ = async {
            relay.closing.cancelled().await;
            tokio::time::sleep(state.config.stop_timeout).await;
        } => Err(ProxyError::Network("WebSocket 关闭握手超过停止期限".into())),
    }
}

impl Relay<'_> {
    async fn direction(
        &self,
        mut reader: SplitStream<AsyncWebSocket>,
        writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        source_closed: &AtomicBool,
        destination_closed: &AtomicBool,
        direction: WebSocketDirection,
        upgrade_request: mio_plugin_api::HttpRequest,
    ) -> Result<(), ProxyError> {
        let mut queued = VecDeque::with_capacity(MAX_QUEUED_DATA_MESSAGES);
        let mut reader_eof = false;
        loop {
            let message = if let Some(message) = queued.pop_front() {
                message
            } else if reader_eof {
                break;
            } else {
                match reader.next().await {
                    Some(result) => result.map_err(ProxyError::network)?,
                    None => {
                        break;
                    }
                }
            };
            match message {
                Message::Frame(_) => {
                    return Err(ProxyError::Network("WebSocket SDK 返回了未解码帧".into()));
                }
                message @ (Message::Close(_) | Message::Ping(_) | Message::Pong(_)) => {
                    self.handle_control(
                        message,
                        writer,
                        source_closed,
                        destination_closed,
                        direction,
                    )
                    .await?;
                }
                message @ (Message::Text(_) | Message::Binary(_)) => {
                    self.intercept_or_forward(
                        message,
                        direction,
                        &upgrade_request,
                        &mut reader,
                        writer,
                        source_closed,
                        destination_closed,
                        &mut queued,
                        &mut reader_eof,
                    )
                    .await?;
                }
            }
        }
        if source_closed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ProxyError::Network(
                "WebSocket 连接未完成 Close 握手即断开".into(),
            ))
        }
    }

    pub(super) async fn handle_control(
        &self,
        message: Message,
        writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        source_closed: &AtomicBool,
        destination_closed: &AtomicBool,
        direction: WebSocketDirection,
    ) -> Result<(), ProxyError> {
        let close = matches!(message, Message::Close(_));
        if close {
            source_closed.store(true, Ordering::Release);
            self.closing.cancel();
        }
        self.observe_control(&message, direction).await?;
        let forward = if close {
            !destination_closed.load(Ordering::Acquire)
        } else {
            // Rama acknowledges Ping on its receiving leg; forwarding Pong would duplicate it.
            false
        };
        if !forward {
            return Ok(());
        }
        self.send_message(writer, message, close, destination_closed)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn inject_frame(
        &self,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        ingress_writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        egress_writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        ingress_closed: &AtomicBool,
        egress_closed: &AtomicBool,
    ) -> Result<WebSocketMessage, ProxyError> {
        if self.state.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let message =
            client::websocket_outgoing_message(kind, &payload, self.state.config.max_body_bytes)?;
        let (writer, source_closed, destination_closed) = match direction {
            WebSocketDirection::ClientToServer => (egress_writer, ingress_closed, egress_closed),
            WebSocketDirection::ServerToClient => (ingress_writer, egress_closed, ingress_closed),
        };
        let is_close = kind == WebSocketKind::Close;
        if destination_closed.load(Ordering::Acquire)
            || source_closed.load(Ordering::Acquire)
            || (self.closing.is_cancelled() && !is_close)
        {
            return Err(ProxyError::Network("WebSocket 对端已关闭".into()));
        }
        self.send_message(writer, message, is_close, destination_closed)
            .await?;
        if is_close {
            source_closed.store(true, Ordering::Release);
            self.closing.cancel();
        }
        let id = Uuid::new_v4();
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let observed_unix_ms = now_unix_ms();
        self.emit_message(
            id,
            sequence,
            observed_unix_ms,
            direction,
            kind,
            payload.clone(),
            None,
        )
        .await?;
        Ok(WebSocketMessage {
            id,
            revision: 1,
            flow_id: self.flow_id,
            sequence,
            direction,
            kind,
            payload,
            observed_unix_ms,
        })
    }

    pub(super) async fn send_message(
        &self,
        writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        message: Message,
        close: bool,
        destination_closed: &AtomicBool,
    ) -> Result<(), ProxyError> {
        let deadline = tokio::time::Instant::now() + self.state.config.request_timeout;
        let mut guard = tokio::select! {
            biased;
            _ = self.state.cancel.cancelled() => return Err(ProxyError::Cancelled),
            locked = tokio::time::timeout_at(deadline, writer.lock()) => locked.map_err(|_| ProxyError::Timeout)?,
        };
        let result = tokio::select! {
            biased;
            _ = self.state.cancel.cancelled() => return Err(ProxyError::Cancelled),
            sent = tokio::time::timeout_at(deadline, guard.send(message)) => sent.map_err(|_| ProxyError::Timeout)?,
        };
        match result {
            Ok(()) => Ok(()),
            Err(ProtocolError::SendAfterClosing)
                if close && destination_closed.load(Ordering::Acquire) =>
            {
                Ok(())
            }
            Err(error) => Err(ProxyError::network(error)),
        }
    }

    pub(super) fn should_intercept(
        &self,
        direction: WebSocketDirection,
        upgrade_request: &mio_plugin_api::HttpRequest,
    ) -> Result<bool, ProxyError> {
        if self.state.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let mode = *self.state.intercept.borrow();
        if !(mode.requests || mode.responses) {
            return Ok(false);
        }
        let websocket_filter = self.state.processing.borrow().2.clone();
        match websocket_filter {
            Some(filter) => {
                let result = filter(upgrade_request, direction, &self.state.cancel)?;
                if self.state.cancel.is_cancelled() {
                    return Err(ProxyError::Cancelled);
                }
                Ok(result)
            }
            None => Ok(false),
        }
    }

    pub(super) fn apply_ws_transform(
        &self,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        upgrade_request: &mio_plugin_api::HttpRequest,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>, ProxyError> {
        if self.state.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let transform = self.state.websocket_transform.borrow().clone();
        let Some(transform) = transform else {
            return Ok(None);
        };
        let deadline = Instant::now()
            .checked_add(self.state.config.request_timeout)
            .ok_or(ProxyError::Timeout)?;
        let replaced = transform(
            upgrade_request,
            direction,
            kind,
            payload,
            &self.state.cancel,
            (deadline, self.state.config.max_body_bytes),
        )?;
        if self.state.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ProxyError::Timeout);
        }
        if let Some(ref next) = replaced {
            client::websocket_outgoing_message(kind, next, self.state.config.max_body_bytes)?;
        }
        Ok(replaced)
    }

    pub(super) async fn emit_resolved(&self, id: Uuid) -> Result<(), ProxyError> {
        let slot = self.state.event_slot().await?;
        slot.send(ProxyEvent::WebSocketInterceptResolved {
            project: self.state.project,
            message_id: id,
        });
        Ok(())
    }

    async fn observe_control(
        &self,
        message: &Message,
        direction: WebSocketDirection,
    ) -> Result<(), ProxyError> {
        let (kind, payload) = match message {
            Message::Ping(bytes) => (WebSocketKind::Ping, bytes.to_vec()),
            Message::Pong(bytes) => (WebSocketKind::Pong, bytes.to_vec()),
            Message::Close(close) => {
                let mut bytes = Vec::new();
                if let Some(close) = close {
                    bytes.extend_from_slice(&u16::from(close.code).to_be_bytes());
                    bytes.extend_from_slice(close.reason.as_bytes());
                }
                (WebSocketKind::Close, bytes)
            }
            _ => unreachable!("only control frames reach observe_control"),
        };
        self.emit_message(
            Uuid::new_v4(),
            self.sequence.fetch_add(1, Ordering::Relaxed),
            now_unix_ms(),
            direction,
            kind,
            payload,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn emit_message(
        &self,
        id: Uuid,
        sequence: u64,
        observed_unix_ms: i64,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        original_payload: Option<Vec<u8>>,
    ) -> Result<(), ProxyError> {
        let slot = self.state.event_slot().await?;
        slot.send(ProxyEvent::WebSocketMessage {
            project: self.state.project,
            listener_port: self.state.listener_port,
            original_payload,
            record: WebSocketMessage {
                id,
                revision: 1,
                flow_id: self.flow_id,
                sequence,
                direction,
                kind,
                payload,
                observed_unix_ms,
            },
        });
        Ok(())
    }
}

pub(super) use crate::now_unix_ms;
