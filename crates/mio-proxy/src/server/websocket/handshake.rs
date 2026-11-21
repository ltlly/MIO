use super::*;
use crate::SendConfig;
use mio_plugin_api::HttpRequest;
use rama::http::{HeaderMap, HeaderName, headers::SecWebSocketKey};

#[cfg(test)]
pub(super) fn request_handshake(
    request: &HttpRequest,
) -> Result<(HeaderMap, Handshake), ProxyError> {
    request_handshake_with_options(request, true)
}

pub(super) fn request_handshake_with_options(
    request: &HttpRequest,
    strip_extensions: bool,
) -> Result<(HeaderMap, Handshake), ProxyError> {
    if request.method != "GET"
        || request.version != HttpVersion::Http11
        || !request.body.is_empty()
        || !request.trailers.is_empty()
    {
        return Err(ProxyError::invalid(
            "WebSocket 升级要求无正文的 HTTP/1.1 GET",
        ));
    }
    let headers = message::header_map(&request.headers)?;
    if !message::has_header_token(&headers, "connection", "upgrade")
        || !message::has_header_token(&headers, "upgrade", "websocket")
        || headers.get_all("sec-websocket-version").iter().count() != 1
        || headers
            .get("sec-websocket-version")
            .is_none_or(|value| value.as_bytes() != b"13")
        || headers.get_all("sec-websocket-key").iter().count() != 1
    {
        return Err(ProxyError::invalid(
            "WebSocket Upgrade/Connection/Version/Key 握手头无效",
        ));
    }
    let key = headers
        .typed_get::<SecWebSocketKey>()
        .ok_or_else(|| ProxyError::invalid("WebSocket Key 必须是 16 字节随机值的 Base64"))?;
    let mut protocols = Vec::new();
    for value in headers.get_all("sec-websocket-protocol").iter() {
        for protocol in value.to_str().map_err(ProxyError::invalid)?.split(',') {
            let protocol = protocol.trim();
            HeaderName::from_bytes(protocol.as_bytes())
                .map_err(|_| ProxyError::invalid("WebSocket subprotocol 不是有效 token"))?;
            protocols.push(protocol.to_owned());
        }
    }
    let offered_extensions = !strip_extensions && headers.contains_key("sec-websocket-extensions");
    Ok((
        headers,
        Handshake {
            accept: SecWebSocketAccept::try_from(key).map_err(ProxyError::invalid)?,
            protocols,
            offered_extensions,
        },
    ))
}

impl Handshake {
    pub(super) fn validate_response(&self, headers: &HeaderMap) -> Result<(), ProxyError> {
        if !message::has_header_token(headers, "connection", "upgrade")
            || !message::has_header_token(headers, "upgrade", "websocket")
            || headers.get_all("sec-websocket-accept").iter().count() != 1
            || headers.typed_get::<SecWebSocketAccept>().as_ref() != Some(&self.accept)
        {
            return Err(ProxyError::Network(
                "WebSocket 源站 101 握手与请求不匹配".into(),
            ));
        }
        if !self.offered_extensions && headers.contains_key("sec-websocket-extensions") {
            return Err(ProxyError::Network(
                "WebSocket 源站选择了未协商的 extension".into(),
            ));
        }
        if headers.get_all("sec-websocket-protocol").iter().count() > 1 {
            return Err(ProxyError::Network(
                "WebSocket 源站选择了多个 subprotocol".into(),
            ));
        }
        if let Some(protocol) = headers.get("sec-websocket-protocol") {
            let protocol = protocol.to_str().map_err(ProxyError::network)?;
            if !self.protocols.iter().any(|offered| offered == protocol) {
                return Err(ProxyError::Network(
                    "WebSocket 源站选择了未提供的 subprotocol".into(),
                ));
            }
        }
        Ok(())
    }
}

fn normalized_http_url(url: &str) -> String {
    if let Some((scheme, target)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("ws") {
            return format!("http://{target}");
        }
        if scheme.eq_ignore_ascii_case("wss") {
            return format!("https://{target}");
        }
    }
    url.to_owned()
}

pub(in crate::server) fn validate_outgoing_request(
    request: &HttpRequest,
    max: usize,
    strip_extensions: bool,
) -> Result<(), ProxyError> {
    let _ = request_handshake_with_options(request, strip_extensions)?;
    let normalized = HttpRequest {
        method: request.method.clone(),
        url: normalized_http_url(&request.url),
        version: request.version,
        headers: request.headers.clone(),
        body: Vec::new(),
        trailers: Vec::new(),
    };
    message::validate_request(&normalized, max)
}

#[cfg(test)]
pub(in crate::server) fn outgoing_request(
    request: &HttpRequest,
    max: usize,
) -> Result<(Request, Handshake), ProxyError> {
    outgoing_request_with_options(request, max, true)
}

pub(in crate::server) fn outgoing_request_with_options(
    request: &HttpRequest,
    max: usize,
    strip_extensions: bool,
) -> Result<(Request, Handshake), ProxyError> {
    let (_, handshake) = request_handshake_with_options(request, strip_extensions)?;
    let normalized = HttpRequest {
        method: request.method.clone(),
        url: normalized_http_url(&request.url),
        version: request.version,
        headers: request.headers.clone(),
        body: Vec::new(),
        trailers: Vec::new(),
    };
    let mut cfg = SendConfig::default();
    cfg.limits.max_body_bytes = max;
    let outgoing = message::to_websocket_upgrade_request(&normalized, &cfg, strip_extensions)?;
    Ok((outgoing, handshake))
}
