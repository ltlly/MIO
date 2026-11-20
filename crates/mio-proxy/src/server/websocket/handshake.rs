use super::*;
use mio_plugin_api::HttpRequest;
use rama::http::{HeaderMap, HeaderName, headers::SecWebSocketKey};

pub(super) fn has_token(headers: &HeaderMap, name: &str, token: &str) -> bool {
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
    if !has_token(&headers, "connection", "upgrade")
        || !has_token(&headers, "upgrade", "websocket")
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
        if !has_token(headers, "connection", "upgrade")
            || !has_token(headers, "upgrade", "websocket")
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
    let (headers, handshake) = request_handshake_with_options(request, strip_extensions)?;
    let mut request = request.clone();
    if let Some((scheme, target)) = request.url.split_once("://") {
        if scheme.eq_ignore_ascii_case("ws") {
            request.url = format!("http://{target}");
        } else if scheme.eq_ignore_ascii_case("wss") {
            request.url = format!("https://{target}");
        }
    }
    let mut outgoing = message::to_request(&request, max)?;
    outgoing
        .headers_mut()
        .insert("connection", HeaderValue::from_static("Upgrade"));
    outgoing
        .headers_mut()
        .insert("upgrade", HeaderValue::from_static("websocket"));
    outgoing
        .headers_mut()
        .insert("sec-websocket-version", HeaderValue::from_static("13"));
    outgoing
        .headers_mut()
        .insert("sec-websocket-key", headers["sec-websocket-key"].clone());
    outgoing.headers_mut().remove("sec-websocket-protocol");
    for protocol in headers.get_all("sec-websocket-protocol").iter() {
        outgoing
            .headers_mut()
            .append("sec-websocket-protocol", protocol.clone());
    }
    if strip_extensions {
        outgoing.headers_mut().remove("sec-websocket-extensions");
    } else {
        outgoing.headers_mut().remove("sec-websocket-extensions");
        for ext in headers.get_all("sec-websocket-extensions").iter() {
            outgoing
                .headers_mut()
                .append("sec-websocket-extensions", ext.clone());
        }
    }
    outgoing.headers_mut().remove("content-length");
    Ok((outgoing, handshake))
}
