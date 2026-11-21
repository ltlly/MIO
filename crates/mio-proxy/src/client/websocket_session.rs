use std::time::Duration;

use mio_plugin_api::{
    Header, HttpRequest, HttpResponse, Uuid, WebSocketDirection, WebSocketKind, WebSocketMessage,
};
use rama::{
    futures::{SinkExt, StreamExt},
    graceful::Shutdown,
    http::ws::{AsyncWebSocket, Message, ProtocolError},
    rt::Executor,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::ProxyError;

use super::{
    SendConfig,
    websocket::{
        establish_client_websocket, prepare_websocket_send_config, websocket_outgoing_message,
        websocket_received_message, websocket_record,
    },
};

type SessionReply = oneshot::Sender<Result<WebSocketMessage, ProxyError>>;

enum SessionCommand {
    Send {
        kind: WebSocketKind,
        payload: Vec<u8>,
        message: Message,
        reply: SessionReply,
    },
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
        let message = websocket_outgoing_message(kind, &payload, self.max_body_bytes)?;
        let (reply, receive) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            _ = self.cancel.cancelled() => return Err(ProxyError::Cancelled),
            sent = tokio::time::timeout_at(
                deadline,
                self.outgoing.send(SessionCommand::Send {
                    kind,
                    payload,
                    message,
                    reply,
                }),
            ) => {
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
        let mut close_outcome: Option<Result<(), ProxyError>> = None;
        if !self.outgoing.is_closed() && !self.cancel.is_cancelled() {
            let (reply, receive) = oneshot::channel();
            if tokio::time::timeout(
                SESSION_CLOSE_TIMEOUT,
                self.outgoing.send(SessionCommand::Disconnect(reply)),
            )
            .await
            .is_ok_and(|sent| sent.is_ok())
            {
                close_outcome = match tokio::time::timeout(SESSION_CLOSE_TIMEOUT, receive).await {
                    Ok(Ok(outcome)) => Some(outcome),
                    Ok(Err(_)) => None,
                    Err(_) => Some(Err(ProxyError::Timeout)),
                };
            }
        }
        self.cancel.cancel();
        let Some(driver) = self.driver.as_mut() else {
            return close_outcome.unwrap_or(Ok(()));
        };
        match tokio::time::timeout(SESSION_CLOSE_TIMEOUT, driver).await {
            Ok(result) => {
                self.driver.take();
                result.map_err(ProxyError::network)?;
                close_outcome.unwrap_or(Ok(()))
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
    if config.limits.max_body_bytes == 0 || config.limits.timeout.is_zero() {
        return Err(ProxyError::invalid(
            "WebSocket 会话正文预算与请求期限必须大于零",
        ));
    }
    prepare_websocket_send_config(&mut config);
    let session_cancel = cancel.child_token();
    let shutdown = Shutdown::new(session_cancel.clone().cancelled_owned());
    let cancel_on_drop = session_cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let deadline = tokio::time::Instant::now() + config.limits.timeout;
    let (request, response, socket) = match establish_client_websocket(
        url,
        headers,
        &config,
        deadline,
        session_cancel.clone(),
        exec,
    )
    .await
    {
        Ok(established) => established,
        Err(error) => {
            drop(cancel_on_drop);
            shutdown.shutdown().await;
            return Err(error);
        }
    };
    let flow_id = Uuid::new_v4();
    let max_body_bytes = config.limits.max_body_bytes;
    let request_timeout = config.limits.timeout;
    let (outgoing_tx, outgoing_rx) = mpsc::channel(16);
    let (incoming_tx, incoming_rx) = mpsc::channel(64);
    let driver_cancel = session_cancel.clone();
    let driver = tokio::spawn(async move {
        let _cancel_on_drop = cancel_on_drop;
        drive_websocket_session(
            socket,
            flow_id,
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
                Some(SessionCommand::Send { kind, payload, message, reply }) => {
                    let outcome = match tokio::time::timeout(request_timeout, socket.send(message)).await {
                        Ok(Ok(())) => Ok(websocket_record(
                            flow_id,
                            next_seq(),
                            WebSocketDirection::ClientToServer,
                            kind,
                            payload,
                        )),
                        Ok(Err(error)) => Err(ProxyError::network(error)),
                        Err(_) => Err(ProxyError::Timeout),
                    };
                    let sent_close = outcome.is_ok() && kind == WebSocketKind::Close;
                    let fatal = matches!(
                        &outcome,
                        Err(ProxyError::Network(_) | ProxyError::Timeout | ProxyError::Cancelled)
                    );
                    let _ = reply.send(outcome);
                    if sent_close {
                        if let Err(error) = drain_session_close(
                            &mut socket,
                            flow_id,
                            &mut next_seq,
                            &incoming,
                            &cancel,
                        )
                        .await
                        {
                            let _ = incoming.try_send(Err(error));
                        }
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
                    match tokio::time::timeout_at(deadline, socket.flush()).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) if error.is_connection_error() => return Ok(()),
                        Ok(Err(error)) => return Err(ProxyError::network(error)),
                        Err(_) => return Err(ProxyError::Timeout),
                    }
                }
                let (kind, payload) = websocket_received_message(message)?;
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
