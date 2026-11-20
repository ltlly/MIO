use std::time::Duration;

use mio_plugin_api::{
    Header, HttpRequest, HttpResponse, HttpVersion, Uuid, WebSocketDirection, WebSocketKind,
    WebSocketMessage,
};
use rama::{
    extensions::ExtensionsRef,
    futures::{SinkExt, StreamExt},
    graceful::Shutdown,
    http::{
        headers::{HeaderMapExt, SecWebSocketAccept, SecWebSocketKey},
        io::upgrade::handle_upgrade,
        ws::{
            AsyncWebSocket, Message, ProtocolError,
            protocol::{Role, WebSocketConfig},
        },
    },
    rt::Executor,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{ProxyError, message};

use super::{
    SendConfig, exchange,
    websocket::{
        validate_websocket_response, websocket_outgoing_message, websocket_received_message,
        websocket_record, websocket_request,
    },
};

type SessionReply = oneshot::Sender<Result<WebSocketMessage, ProxyError>>;

enum SessionCommand {
    Send(WebSocketKind, Vec<u8>, SessionReply),
    Disconnect(oneshot::Sender<Result<(), ProxyError>>),
}

pub struct WebSocketSession {
    pub flow_id: Uuid,
    pub request: HttpRequest,
    pub response: HttpResponse,
    max_body_bytes: usize,
    request_timeout: Duration,
    outgoing: mpsc::Sender<SessionCommand>,
    incoming: Option<mpsc::Receiver<Result<WebSocketMessage, ProxyError>>>,
    cancel: CancellationToken,
    driver: Option<JoinHandle<()>>,
}

const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

impl std::fmt::Debug for WebSocketSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebSocketSession")
            .field("flow_id", &self.flow_id)
            .field("request", &self.request)
            .field("response", &self.response)
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl WebSocketSession {
    pub fn is_closed(&self) -> bool {
        self.cancel.is_cancelled() || self.outgoing.is_closed()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn take_receiver(
        &mut self,
    ) -> Option<mpsc::Receiver<Result<WebSocketMessage, ProxyError>>> {
        self.incoming.take()
    }

    pub async fn recv(&mut self) -> Option<Result<WebSocketMessage, ProxyError>> {
        self.incoming.as_mut()?.recv().await
    }

    pub async fn send(
        &self,
        kind: WebSocketKind,
        payload: Vec<u8>,
        cancel: CancellationToken,
    ) -> Result<WebSocketMessage, ProxyError> {
        if cancel.is_cancelled() || self.cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let _ = websocket_outgoing_message(kind, &payload, self.max_body_bytes)?;
        let (reply, receive) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            _ = self.cancel.cancelled() => return Err(ProxyError::Cancelled),
            sent = tokio::time::timeout_at(deadline, self.outgoing.send(SessionCommand::Send(kind, payload, reply))) => {
                sent.map_err(|_| ProxyError::Timeout)?
                    .map_err(|_| ProxyError::network("WebSocket 会话已断开"))?;
            }
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProxyError::Cancelled),
            result = tokio::time::timeout_at(deadline, receive) => match result {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(_)) => Err(if self.cancel.is_cancelled() {
                    ProxyError::Cancelled
                } else {
                    ProxyError::network("WebSocket 会话已断开")
                }),
                Err(_) => Err(ProxyError::Timeout),
            },
        }
    }

    pub async fn disconnect(&mut self) -> Result<(), ProxyError> {
        if !self.outgoing.is_closed() && !self.cancel.is_cancelled() {
            let (reply, receive) = oneshot::channel();
            if tokio::time::timeout(
                SESSION_CLOSE_TIMEOUT,
                self.outgoing.send(SessionCommand::Disconnect(reply)),
            )
            .await
            .is_ok_and(|sent| sent.is_ok())
            {
                let _ = tokio::time::timeout(SESSION_CLOSE_TIMEOUT, receive).await;
            }
        }
        self.cancel.cancel();
        let Some(driver) = self.driver.as_mut() else {
            return Ok(());
        };
        match tokio::time::timeout(SESSION_CLOSE_TIMEOUT, driver).await {
            Ok(result) => {
                self.driver.take();
                result.map_err(ProxyError::network)
            }
            Err(_) => Err(ProxyError::StopDeadline),
        }
    }
}

impl Drop for WebSocketSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub async fn connect_websocket_session(
    url: &str,
    headers: &[Header],
    mut config: SendConfig,
    cancel: CancellationToken,
) -> Result<WebSocketSession, ProxyError> {
    if config.max_body_bytes == 0 || config.timeout.is_zero() {
        return Err(ProxyError::invalid(
            "WebSocket 会话正文预算与请求期限必须大于零",
        ));
    }
    let request = websocket_request(url, headers)?;
    config.http_version = Some(HttpVersion::Http11);
    config.update_content_length = true;
    config.normalize_http1_line_endings = true;
    config.raw_http1_override = None;
    let session_cancel = cancel.child_token();
    let shutdown = Shutdown::new(session_cancel.clone().cancelled_owned());
    let cancel_on_drop = session_cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let deadline = tokio::time::Instant::now() + config.timeout;
    let handshake = tokio::select! {
        biased;
        _ = session_cancel.cancelled() => Err(ProxyError::Cancelled),
        result = tokio::time::timeout_at(deadline, async {
            let mut outgoing = message::to_request_with_config(&request, &config)?;
            let handshake_headers = message::header_map(&request.headers)?;
            for name in ["connection", "upgrade", "sec-websocket-version", "sec-websocket-key"] {
                outgoing.headers_mut().remove(name);
                for value in handshake_headers.get_all(name).iter() {
                    outgoing.headers_mut().append(name, value.clone());
                }
            }
            outgoing.headers_mut().remove("content-length");
            outgoing.headers_mut().remove("sec-websocket-extensions");
            let key = handshake_headers
                .typed_get::<SecWebSocketKey>()
                .ok_or_else(|| ProxyError::invalid("WebSocket Key 必须是 16 字节随机值的 Base64"))?;
            let accept = SecWebSocketAccept::try_from(key).map_err(ProxyError::invalid)?;
            let response = exchange(outgoing, &config, session_cancel.clone(), exec).await?;
            let mut header_budget = (
                crate::StreamedResponse::MAX_HEADERS,
                crate::StreamedResponse::MAX_HEAD_BYTES,
            );
            message::consume_header_budget(response.headers(), &mut header_budget)?;
            validate_websocket_response(&handshake_headers, response.headers(), response.status(), &accept)?;
            let response_head = HttpResponse {
                status: response.status().as_u16(),
                version: message::model_version(response.version()),
                reason_phrase: None,
                headers: message::headers(response.headers()),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            let upgraded = handle_upgrade(response.extensions().clone()).await.map_err(ProxyError::network)?;
            drop(response);
            let ws_config = WebSocketConfig::default()
                .with_read_buffer_size(4096)
                .with_write_buffer_size(0)
                .with_max_write_buffer_size(config.max_body_bytes.saturating_add(256))
                .with_max_message_size(config.max_body_bytes)
                .with_max_frame_size(config.max_body_bytes);
            let socket = AsyncWebSocket::from_raw_socket(upgraded, Role::Client, Some(ws_config)).await;
            Ok((response_head, socket))
        }) => result.unwrap_or(Err(ProxyError::Timeout)),
    };
    let (response, socket) = match handshake {
        Ok(established) => established,
        Err(error) => {
            drop(cancel_on_drop);
            shutdown.shutdown().await;
            return Err(error);
        }
    };
    let flow_id = Uuid::new_v4();
    let max_body_bytes = config.max_body_bytes;
    let request_timeout = config.timeout;
    let (outgoing_tx, outgoing_rx) = mpsc::channel(16);
    let (incoming_tx, incoming_rx) = mpsc::channel(64);
    let driver_cancel = session_cancel.clone();
    let driver = tokio::spawn(async move {
        let _cancel_on_drop = cancel_on_drop;
        drive_websocket_session(
            socket,
            flow_id,
            max_body_bytes,
            request_timeout,
            outgoing_rx,
            incoming_tx,
            driver_cancel.clone(),
        )
        .await;
        driver_cancel.cancel();
        shutdown.shutdown().await;
    });
    Ok(WebSocketSession {
        flow_id,
        request,
        response,
        max_body_bytes,
        request_timeout,
        outgoing: outgoing_tx,
        incoming: Some(incoming_rx),
        cancel: session_cancel,
        driver: Some(driver),
    })
}

async fn drive_websocket_session(
    mut socket: AsyncWebSocket,
    flow_id: Uuid,
    max_body_bytes: usize,
    request_timeout: Duration,
    mut outgoing: mpsc::Receiver<SessionCommand>,
    incoming: mpsc::Sender<Result<WebSocketMessage, ProxyError>>,
    cancel: CancellationToken,
) {
    let mut sequence: u64 = 0;
    let mut next_seq = || {
        sequence = sequence.saturating_add(1);
        sequence
    };
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            command = outgoing.recv() => match command {
                None => break,
                Some(SessionCommand::Send(kind, payload, reply)) => {
                    let outcome = match websocket_outgoing_message(kind, &payload, max_body_bytes) {
                        Ok(message) => match tokio::time::timeout(request_timeout, socket.send(message)).await {
                            Ok(Ok(())) => Ok(websocket_record(
                                flow_id,
                                next_seq(),
                                WebSocketDirection::ClientToServer,
                                kind,
                                payload,
                            )),
                            Ok(Err(error)) => Err(ProxyError::network(error)),
                            Err(_) => Err(ProxyError::Timeout),
                        },
                        Err(error) => Err(error),
                    };
                    let sent_close = outcome.is_ok() && kind == WebSocketKind::Close;
                    let fatal = matches!(
                        &outcome,
                        Err(ProxyError::Network(_) | ProxyError::Timeout | ProxyError::Cancelled)
                    );
                    let _ = reply.send(outcome);
                    if sent_close {
                        let _ = drain_session_close(
                            &mut socket,
                            flow_id,
                            &mut next_seq,
                            &incoming,
                            &cancel,
                        )
                        .await;
                        break;
                    }
                    if fatal {
                        break;
                    }
                }
                Some(SessionCommand::Disconnect(reply)) => {
                    let result = graceful_session_close(
                        &mut socket,
                        flow_id,
                        &mut next_seq,
                        &incoming,
                        &cancel,
                    )
                    .await;
                    let _ = reply.send(result);
                    break;
                }
            },
            frame = socket.next() => match frame {
                None | Some(Err(ProtocolError::ResetWithoutClosingHandshake)) => break,
                Some(Err(error)) => {
                    let err = if cancel.is_cancelled() {
                        ProxyError::Cancelled
                    } else {
                        ProxyError::network(error)
                    };
                    let _ = incoming.try_send(Err(err));
                    break;
                }
                Some(Ok(message)) => {
                    let needs_flush = matches!(message, Message::Ping(_) | Message::Close(_));
                    let parsed = websocket_received_message(message);
                    if needs_flush
                        && let Err(error) = tokio::time::timeout(request_timeout, socket.flush())
                            .await
                            .map_err(|_| ProxyError::Timeout)
                            .and_then(|res| res.map_err(ProxyError::network))
                    {
                        let _ = incoming.try_send(Err(error));
                        break;
                    }
                    match parsed {
                        Ok((kind, payload)) => {
                            let record = websocket_record(
                                flow_id,
                                next_seq(),
                                WebSocketDirection::ServerToClient,
                                kind,
                                payload,
                            );
                            let closed = kind == WebSocketKind::Close;
                            tokio::select! {
                                biased;
                                _ = cancel.cancelled() => break,
                                sent = incoming.send(Ok(record)) => {
                                    if sent.is_err() || closed {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            let _ = incoming.try_send(Err(error));
                            break;
                        }
                    }
                }
            },
        }
    }
}

async fn graceful_session_close(
    socket: &mut AsyncWebSocket,
    flow_id: Uuid,
    next_seq: &mut impl FnMut() -> u64,
    incoming: &mpsc::Sender<Result<WebSocketMessage, ProxyError>>,
    cancel: &CancellationToken,
) -> Result<(), ProxyError> {
    let deadline = tokio::time::Instant::now() + SESSION_CLOSE_TIMEOUT;
    match tokio::time::timeout_at(deadline, socket.close(None)).await {
        Ok(Ok(())) | Ok(Err(ProtocolError::SendAfterClosing)) => {}
        Ok(Err(error)) if error.is_connection_error() => return Ok(()),
        Ok(Err(error)) => return Err(ProxyError::network(error)),
        Err(_) => return Err(ProxyError::Timeout),
    }
    tokio::time::timeout_at(
        deadline,
        drain_session_close(socket, flow_id, next_seq, incoming, cancel),
    )
    .await
    .unwrap_or(Ok(()))
}

async fn drain_session_close(
    socket: &mut AsyncWebSocket,
    flow_id: Uuid,
    next_seq: &mut impl FnMut() -> u64,
    incoming: &mpsc::Sender<Result<WebSocketMessage, ProxyError>>,
    cancel: &CancellationToken,
) -> Result<(), ProxyError> {
    let deadline = tokio::time::Instant::now() + SESSION_CLOSE_TIMEOUT;
    loop {
        let frame = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            res = tokio::time::timeout_at(deadline, socket.next()) => match res {
                Ok(item) => item,
                Err(_) => return Ok(()),
            },
        };
        match frame {
            None | Some(Err(ProtocolError::ResetWithoutClosingHandshake)) => return Ok(()),
            Some(Err(error)) if error.is_connection_error() => return Ok(()),
            Some(Err(error)) => return Err(ProxyError::network(error)),
            Some(Ok(message)) => {
                if matches!(message, Message::Ping(_) | Message::Close(_)) {
                    let _ = tokio::time::timeout_at(deadline, socket.flush()).await;
                }
                if let Ok((kind, payload)) = websocket_received_message(message) {
                    let is_close = kind == WebSocketKind::Close;
                    let record = websocket_record(
                        flow_id,
                        next_seq(),
                        WebSocketDirection::ServerToClient,
                        kind,
                        payload,
                    );
                    let _ = incoming.try_send(Ok(record));
                    if is_close {
                        return Ok(());
                    }
                }
            }
        }
    }
}
