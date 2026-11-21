use mio_plugin_api::{
    Header, HttpRequest, HttpResponse, HttpVersion, Uuid, WebSocketDirection, WebSocketKind,
    WebSocketMessage,
};
use rama::{
    extensions::ExtensionsRef,
    futures::{SinkExt, StreamExt},
    graceful::Shutdown,
    http::{
        HeaderValue, StatusCode,
        headers::{HeaderMapExt, SecWebSocketAccept, SecWebSocketKey},
        io::upgrade::handle_upgrade,
        ws::{
            AsyncWebSocket, Message, ProtocolError,
            protocol::{CloseFrame, Role, WebSocketConfig, frame::coding::CloseCode},
        },
    },
    rt::Executor,
};
use tokio_util::sync::CancellationToken;

use crate::{ProxyError, message};

use super::{SendConfig, WebSocketExchange, exchange};

pub async fn send_websocket(
    url: &str,
    headers: &[Header],
    kind: WebSocketKind,
    payload: Vec<u8>,
    mut config: SendConfig,
    cancel: CancellationToken,
) -> Result<WebSocketExchange, ProxyError> {
    let request = websocket_request(url, headers)?;
    let outgoing_message = websocket_outgoing_message(kind, &payload, config.max_body_bytes)?;
    config.http_version = Some(HttpVersion::Http11);
    config.update_content_length = true;
    config.normalize_http1_line_endings = true;
    config.raw_http1_override = None;
    let cancel = cancel.child_token();
    let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
    let _cancel_on_drop = cancel.clone().drop_guard();
    let exec = Executor::graceful(shutdown.guard());
    let deadline = tokio::time::Instant::now() + config.timeout;
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
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
            let response = exchange(outgoing, &config, cancel.clone(), exec).await?;
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
            let mut socket = AsyncWebSocket::from_raw_socket(upgraded, Role::Client, Some(ws_config)).await;
            socket.send(outgoing_message).await.map_err(ProxyError::network)?;
            let flow_id = Uuid::new_v4();
            let sent = websocket_record(flow_id, 1, WebSocketDirection::ClientToServer, kind, payload);
            let received = loop {
                match socket.next().await {
                    None | Some(Err(ProtocolError::ResetWithoutClosingHandshake)) => break None,
                    Some(Err(error)) => return Err(ProxyError::network(error)),
                    Some(Ok(Message::Ping(_))) => socket.flush().await.map_err(ProxyError::network)?,
                    Some(Ok(message)) => {
                        let (kind, payload) = websocket_received_message(message)?;
                        if kind == WebSocketKind::Close {
                            socket.flush().await.map_err(ProxyError::network)?;
                        }
                        break Some(websocket_record(flow_id, 2, WebSocketDirection::ServerToClient, kind, payload));
                    },
                }
            };
            drop(socket);
            Ok(WebSocketExchange { request, response: response_head, sent, received })
        }) => result.unwrap_or(Err(ProxyError::Timeout)),
    };
    cancel.cancel();
    shutdown.shutdown().await;
    result
}

pub(super) fn websocket_request(url: &str, headers: &[Header]) -> Result<HttpRequest, ProxyError> {
    let (scheme, target) = url
        .split_once("://")
        .ok_or_else(|| ProxyError::invalid("WebSocket 目标必须是完整 WS/WSS URL"))?;
    let scheme = if scheme.eq_ignore_ascii_case("ws") || scheme.eq_ignore_ascii_case("http") {
        "http"
    } else if scheme.eq_ignore_ascii_case("wss") || scheme.eq_ignore_ascii_case("https") {
        "https"
    } else {
        return Err(ProxyError::invalid(
            "WebSocket 目标仅接受 WS/WSS/HTTP/HTTPS URL",
        ));
    };
    let mut headers = message::header_map(headers)?;
    for (name, value) in [
        ("connection", "Upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-version", "13"),
    ] {
        if !headers.contains_key(name) {
            headers.insert(name, HeaderValue::from_static(value));
        }
    }
    if !headers.contains_key("sec-websocket-key") {
        headers.typed_insert(SecWebSocketKey::random());
    }
    if !websocket_has_token(&headers, "connection", "upgrade")
        || !websocket_has_token(&headers, "upgrade", "websocket")
        || headers.get_all("sec-websocket-version").iter().count() != 1
        || headers["sec-websocket-version"] != "13"
        || headers.get_all("sec-websocket-key").iter().count() != 1
        || headers.typed_get::<SecWebSocketKey>().is_none()
    {
        return Err(ProxyError::invalid(
            "WebSocket Upgrade/Connection/Version/Key 握手头无效",
        ));
    }
    for value in headers.get_all("sec-websocket-protocol").iter() {
        for protocol in value.to_str().map_err(ProxyError::invalid)?.split(',') {
            rama::http::HeaderName::from_bytes(protocol.trim().as_bytes())
                .map_err(|_| ProxyError::invalid("WebSocket subprotocol 不是有效 token"))?;
        }
    }
    headers.remove("sec-websocket-extensions");
    Ok(HttpRequest {
        method: "GET".into(),
        url: format!("{scheme}://{target}"),
        version: HttpVersion::Http11,
        headers: message::headers(&headers),
        body: Vec::new(),
        trailers: Vec::new(),
    })
}

fn websocket_has_token(headers: &rama::http::HeaderMap, name: &str, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
}

pub(super) fn validate_websocket_response(
    request_headers: &rama::http::HeaderMap,
    headers: &rama::http::HeaderMap,
    status: StatusCode,
    accept: &SecWebSocketAccept,
) -> Result<(), ProxyError> {
    if status != StatusCode::SWITCHING_PROTOCOLS
        || !websocket_has_token(headers, "connection", "upgrade")
        || !websocket_has_token(headers, "upgrade", "websocket")
        || headers.get_all("sec-websocket-accept").iter().count() != 1
        || headers.typed_get::<SecWebSocketAccept>().as_ref() != Some(accept)
    {
        return Err(ProxyError::network("WebSocket 源站 101 握手与请求不匹配"));
    }
    if headers.contains_key("sec-websocket-extensions") {
        return Err(ProxyError::network(
            "WebSocket 源站选择了未协商的 extension",
        ));
    }
    if headers.get_all("sec-websocket-protocol").iter().count() > 1 {
        return Err(ProxyError::network("WebSocket 源站选择了多个 subprotocol"));
    }
    if let Some(protocol) = headers.get("sec-websocket-protocol") {
        let protocol = protocol.to_str().map_err(ProxyError::network)?;
        let offered = request_headers
            .get_all("sec-websocket-protocol")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.split(',').any(|offered| offered.trim() == protocol));
        if !offered {
            return Err(ProxyError::network(
                "WebSocket 源站选择了未提供的 subprotocol",
            ));
        }
    }
    Ok(())
}

pub(crate) fn websocket_outgoing_message(
    kind: WebSocketKind,
    payload: &[u8],
    limit: usize,
) -> Result<Message, ProxyError> {
    if payload.len() > limit {
        return Err(ProxyError::BodyLimit { limit });
    }
    if matches!(
        kind,
        WebSocketKind::Ping | WebSocketKind::Pong | WebSocketKind::Close
    ) && payload.len() > 125
    {
        return Err(ProxyError::invalid("WebSocket 控制帧正文不得超过 125 字节"));
    }
    Ok(match kind {
        WebSocketKind::Text => Message::Text(
            std::str::from_utf8(payload)
                .map_err(ProxyError::invalid)?
                .into(),
        ),
        WebSocketKind::Binary => Message::Binary(payload.to_vec().into()),
        WebSocketKind::Ping => Message::Ping(payload.to_vec().into()),
        WebSocketKind::Pong => Message::Pong(payload.to_vec().into()),
        WebSocketKind::Close => {
            let close = if payload.is_empty() {
                None
            } else {
                if payload.len() < 2 {
                    return Err(ProxyError::invalid("WebSocket Close 必须包含完整状态码"));
                }
                let code = CloseCode::from(u16::from_be_bytes([payload[0], payload[1]]));
                if !code.is_allowed() {
                    return Err(ProxyError::invalid("WebSocket Close 状态码无效"));
                }
                Some(CloseFrame {
                    code,
                    reason: std::str::from_utf8(&payload[2..])
                        .map_err(ProxyError::invalid)?
                        .into(),
                })
            };
            Message::Close(close)
        }
    })
}

pub(crate) fn websocket_received_message(
    message: Message,
) -> Result<(WebSocketKind, Vec<u8>), ProxyError> {
    Ok(match message {
        Message::Text(text) => (WebSocketKind::Text, text.as_bytes().to_vec()),
        Message::Binary(bytes) => (WebSocketKind::Binary, bytes.to_vec()),
        Message::Ping(bytes) => (WebSocketKind::Ping, bytes.to_vec()),
        Message::Pong(bytes) => (WebSocketKind::Pong, bytes.to_vec()),
        Message::Close(close) => {
            let mut payload = Vec::new();
            if let Some(close) = close {
                payload.extend_from_slice(&u16::from(close.code).to_be_bytes());
                payload.extend_from_slice(close.reason.as_bytes());
            }
            (WebSocketKind::Close, payload)
        }
        Message::Frame(_) => return Err(ProxyError::network("WebSocket SDK 返回了未解码帧")),
    })
}

pub(super) fn websocket_record(
    flow_id: Uuid,
    sequence: u64,
    direction: WebSocketDirection,
    kind: WebSocketKind,
    payload: Vec<u8>,
) -> WebSocketMessage {
    WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id,
        sequence,
        direction,
        kind,
        payload,
        observed_unix_ms: crate::now_unix_ms(),
    }
}
