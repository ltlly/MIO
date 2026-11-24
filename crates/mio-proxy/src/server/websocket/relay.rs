use super::*;
use rama::{
    futures::{
        SinkExt, StreamExt,
        stream::{SplitSink, SplitStream},
    },
    http::{
        io::upgrade::Upgraded,
        ws::{AsyncWebSocket, Message, ProtocolError, protocol::Role},
    },
};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use tokio::sync::Mutex as AsyncMutex;

pub(super) struct RelayLeg<'a> {
    pub(super) relay: &'a Relay<'a>,
    pub(super) direction: WebSocketDirection,
    pub(super) upgrade_request: &'a mio_plugin_api::HttpRequest,
    pub(super) reader: SplitStream<AsyncWebSocket>,
    pub(super) writer: &'a AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
    pub(super) source_closed: &'a AtomicBool,
    pub(super) destination_closed: &'a AtomicBool,
    pub(super) queued: VecDeque<Message>,
    pub(super) reader_eof: bool,
}

struct RelayPeers<'a> {
    ingress_writer: &'a AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
    egress_writer: &'a AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
    ingress_closed: &'a AtomicBool,
    egress_closed: &'a AtomicBool,
}

pub(super) async fn relay(
    ingress: Upgraded,
    egress: Upgraded,
    state: &State,
    flow_id: Uuid,
    upgrade_request: mio_plugin_api::HttpRequest,
) -> Result<(), ProxyError> {
    let config = client::websocket_config(state.config.limits.max_body_bytes);
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
    let c2s = RelayLeg {
        relay: &relay,
        direction: WebSocketDirection::ClientToServer,
        upgrade_request: &upgrade_request,
        reader: ingress_reader,
        writer: &egress_writer,
        source_closed: &ingress_closed,
        destination_closed: &egress_closed,
        queued: VecDeque::with_capacity(MAX_QUEUED_DATA_MESSAGES),
        reader_eof: false,
    };
    let s2c = RelayLeg {
        relay: &relay,
        direction: WebSocketDirection::ServerToClient,
        upgrade_request: &upgrade_request,
        reader: egress_reader,
        writer: &ingress_writer,
        source_closed: &egress_closed,
        destination_closed: &ingress_closed,
        queued: VecDeque::with_capacity(MAX_QUEUED_DATA_MESSAGES),
        reader_eof: false,
    };
    let peers = RelayPeers {
        ingress_writer: &ingress_writer,
        egress_writer: &egress_writer,
        ingress_closed: &ingress_closed,
        egress_closed: &egress_closed,
    };
    tokio::select! {
        result = async {
            tokio::try_join!(c2s.run(), s2c.run())?;
            Ok(())
        } => result,
        command = async {
            while let Some(request) = commands.recv().await {
                let outcome = relay.inject_frame(request, &peers).await;
                let _ = outcome.reply.send(outcome.result);
            }
            std::future::pending::<Result<(), ProxyError>>().await
        } => command,
        _ = async {
            relay.closing.cancelled().await;
            tokio::time::sleep(state.config.limits.stop_timeout).await;
        } => Err(ProxyError::Network("WebSocket 关闭握手超过停止期限".into())),
    }
}

struct InjectOutcome {
    reply: oneshot::Sender<Result<WebSocketMessage, ProxyError>>,
    result: Result<WebSocketMessage, ProxyError>,
}

impl RelayLeg<'_> {
    async fn run(mut self) -> Result<(), ProxyError> {
        loop {
            let message = if let Some(message) = self.queued.pop_front() {
                message
            } else if self.reader_eof {
                break;
            } else {
                match self.reader.next().await {
                    Some(result) => result.map_err(ProxyError::network)?,
                    None => break,
                }
            };
            match message {
                Message::Frame(_) => {
                    return Err(ProxyError::Network("WebSocket SDK 返回了未解码帧".into()));
                }
                message @ (Message::Close(_) | Message::Ping(_) | Message::Pong(_)) => {
                    self.handle_control(message).await?;
                }
                message @ (Message::Text(_) | Message::Binary(_)) => {
                    self.intercept_or_forward(message).await?;
                }
            }
        }
        if self.source_closed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ProxyError::Network(
                "WebSocket 连接未完成 Close 握手即断开".into(),
            ))
        }
    }

    pub(super) async fn handle_control(&self, message: Message) -> Result<(), ProxyError> {
        let close = matches!(message, Message::Close(_));
        if close {
            self.source_closed.store(true, Ordering::Release);
            self.relay.closing.cancel();
        }
        self.relay
            .observe_control(&message, self.direction)
            .await?;
        let forward = close && !self.destination_closed.load(Ordering::Acquire);
        if !forward {
            return Ok(());
        }
        self.relay
            .send_message(self.writer, message, true, self.destination_closed)
            .await
    }
}

impl Relay<'_> {
    async fn inject_frame(&self, request: InjectRequest, peers: &RelayPeers<'_>) -> InjectOutcome {
        let InjectRequest {
            direction,
            kind,
            payload,
            message,
            reply,
        } = request;
        let result = self
            .inject_frame_inner(direction, kind, payload, message, peers)
            .await;
        InjectOutcome { reply, result }
    }

    async fn inject_frame_inner(
        &self,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        preencoded: Option<Message>,
        peers: &RelayPeers<'_>,
    ) -> Result<WebSocketMessage, ProxyError> {
        if self.state.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let message = match preencoded {
            Some(msg) => {
                if payload.len() > self.state.config.limits.max_body_bytes {
                    return Err(ProxyError::BodyLimit {
                        limit: self.state.config.limits.max_body_bytes,
                    });
                }
                msg
            }
            None => client::websocket_outgoing_message(
                kind,
                &payload,
                self.state.config.limits.max_body_bytes,
            )?,
        };
        let (writer, source_closed, destination_closed) = match direction {
            WebSocketDirection::ClientToServer => (
                peers.egress_writer,
                peers.ingress_closed,
                peers.egress_closed,
            ),
            WebSocketDirection::ServerToClient => (
                peers.ingress_writer,
                peers.egress_closed,
                peers.ingress_closed,
            ),
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
        let record = WebSocketMessage {
            id: Uuid::new_v4(),
            revision: 1,
            flow_id: self.flow_id,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            direction,
            kind,
            payload,
            observed_unix_ms: crate::now_unix_ms(),
        };
        self.emit_message(record.clone(), None).await?;
        Ok(record)
    }

    pub(super) async fn send_message(
        &self,
        writer: &AsyncMutex<SplitSink<AsyncWebSocket, Message>>,
        message: Message,
        close: bool,
        destination_closed: &AtomicBool,
    ) -> Result<(), ProxyError> {
        let deadline = tokio::time::Instant::now() + self.state.config.limits.request_timeout;
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
                if close
                    && (destination_closed.load(Ordering::Acquire) || self.closing.is_cancelled()) =>
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
        let (mode, websocket_filter) = {
            let dyn_state = self.state.dynamic.borrow();
            (
                dyn_state.intercept,
                dyn_state.websocket_intercept_filter.clone(),
            )
        };
        if !(mode.requests || mode.responses) {
            return Ok(false);
        }
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
        let transform = self.state.dynamic.borrow().websocket_transform.clone();
        let Some(transform) = transform else {
            return Ok(None);
        };
        let deadline = Instant::now()
            .checked_add(self.state.config.limits.request_timeout)
            .ok_or(ProxyError::Timeout)?;
        let replaced = transform(
            upgrade_request,
            direction,
            kind,
            payload,
            &self.state.cancel,
            (deadline, self.state.config.limits.max_body_bytes),
        )?;
        if self.state.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ProxyError::Timeout);
        }
        if let Some(ref next) = replaced {
            client::validate_websocket_payload(
                kind,
                next,
                self.state.config.limits.max_body_bytes,
            )?;
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
            WebSocketMessage {
                id: Uuid::new_v4(),
                revision: 1,
                flow_id: self.flow_id,
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
                direction,
                kind,
                payload,
                observed_unix_ms: crate::now_unix_ms(),
            },
            None,
        )
        .await
    }

    pub(super) async fn emit_message(
        &self,
        record: WebSocketMessage,
        original_payload: Option<Vec<u8>>,
    ) -> Result<(), ProxyError> {
        let slot = self.state.event_slot().await?;
        slot.send(ProxyEvent::WebSocketMessage {
            project: self.state.project,
            listener_port: self.state.listener_port,
            original_payload,
            record,
        });
        Ok(())
    }
}
