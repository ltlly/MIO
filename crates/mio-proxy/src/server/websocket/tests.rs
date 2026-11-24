use super::*;
use mio_plugin_api::{Header, HttpRequest};
use rama::http::HeaderMap;
use std::time::Duration;

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.into(),
        value: value.as_bytes().to_vec(),
    }
}

fn request() -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: "ws://localhost:1234/socket".into(),
        version: HttpVersion::Http11,
        headers: vec![
            header("connection", "keep-alive, Upgrade"),
            header("upgrade", "websocket"),
            header("sec-websocket-version", "13"),
            header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            header("sec-websocket-protocol", "chat, CaseSensitive"),
        ],
        body: Vec::new(),
        trailers: Vec::new(),
    }
}

fn response_headers() -> HeaderMap {
    message::header_map(&[
        header("connection", "Upgrade"),
        header("upgrade", "websocket"),
        header("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        header("sec-websocket-protocol", "CaseSensitive"),
    ])
    .unwrap()
}

#[test]
fn handshake_requires_nonce_version_method_and_empty_body() {
    let good = request();
    let (_, handshake) = request_handshake(&good).unwrap();
    handshake.validate_response(&response_headers()).unwrap();
    let mut bad = good.clone();
    bad.method = "POST".into();
    assert!(request_handshake(&bad).is_err());
    let mut bad = good.clone();
    bad.headers
        .push(header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="));
    assert!(request_handshake(&bad).is_err());
    let mut bad = good.clone();
    bad.headers.retain(|header| header.name != "connection");
    assert!(request_handshake(&bad).is_err());
    let mut bad = good.clone();
    bad.headers[3].value = b"not-a-base64-nonce".to_vec();
    assert!(request_handshake(&bad).is_err());
    let mut bad = good.clone();
    bad.headers[2].value = b"12".to_vec();
    assert!(request_handshake(&bad).is_err());
    let mut bad = good.clone();
    bad.version = HttpVersion::Http2;
    assert!(request_handshake(&bad).is_err());
    let mut bad = good;
    bad.body.push(1);
    assert!(request_handshake(&bad).is_err());
}

#[test]
fn response_rejects_wrong_accept_unoffered_protocol_and_extensions() {
    let (_, handshake) = request_handshake(&request()).unwrap();
    for (name, value) in [
        ("sec-websocket-accept", "wrong"),
        ("sec-websocket-protocol", "casesensitive"),
        ("sec-websocket-protocol", "chat, CaseSensitive"),
        ("sec-websocket-extensions", "permessage-deflate"),
        ("upgrade", "not-websocket"),
        ("connection", "close"),
    ] {
        let mut headers = response_headers();
        headers.insert(name, HeaderValue::from_static(value));
        assert!(handshake.validate_response(&headers).is_err(), "{name}");
    }
    let mut headers = response_headers();
    headers.append("sec-websocket-protocol", HeaderValue::from_static("chat"));
    assert!(handshake.validate_response(&headers).is_err());
}

#[test]
fn request_preserves_security_headers_but_declines_unimplemented_extensions() {
    let mut model = request();
    model
        .headers
        .push(header("sec-websocket-extensions", "permessage-deflate"));
    model
        .headers
        .push(header("proxy-authorization", "Basic c2VjcmV0"));
    let (outgoing, _) = outgoing_request(&model, 64).unwrap();
    assert_eq!(outgoing.uri().scheme_str(), Some("http"));
    assert_eq!(outgoing.headers()["upgrade"], "websocket");
    assert_eq!(outgoing.headers()["connection"], "Upgrade");
    assert_eq!(
        outgoing.headers()["sec-websocket-key"],
        "dGhlIHNhbXBsZSBub25jZQ=="
    );
    assert_eq!(
        outgoing.headers()["sec-websocket-protocol"],
        "chat, CaseSensitive"
    );
    assert!(!outgoing.headers().contains_key("sec-websocket-extensions"));
    assert!(!outgoing.headers().contains_key("proxy-authorization"));
    assert!(!outgoing.headers().contains_key("content-length"));

    let mut cfg = crate::SendConfig::default();
    cfg.limits.max_body_bytes = 64;
    let (kept_outgoing, kept_handshake) = outgoing_request_with_options(&model, &cfg, false).unwrap();
    assert_eq!(
        kept_outgoing.headers()["sec-websocket-extensions"],
        "permessage-deflate"
    );
    let mut resp_headers = response_headers();
    resp_headers.insert(
        "sec-websocket-extensions",
        HeaderValue::from_static("permessage-deflate"),
    );
    assert!(kept_handshake.validate_response(&resp_headers).is_ok());
}

#[tokio::test]
async fn injector_registry_routes_direction_and_reclaims_entry_on_guard_drop() {
    let registry = WebSocketInjectorRegistry::new();
    let flow_id = Uuid::new_v4();
    assert!(!registry.contains(flow_id));
    let (tx, mut rx) = mpsc::channel(4);
    let guard = registry.register(flow_id, tx);
    assert!(registry.contains(flow_id));

    let worker = tokio::spawn(async move {
        let req = rx.recv().await.unwrap();
        assert_eq!(req.direction, WebSocketDirection::ServerToClient);
        assert_eq!(req.kind, WebSocketKind::Text);
        assert_eq!(req.payload, b"injected");
        req.reply
            .send(Ok(WebSocketMessage {
                id: Uuid::new_v4(),
                revision: 1,
                flow_id,
                sequence: 7,
                direction: req.direction,
                kind: req.kind,
                payload: req.payload,
                observed_unix_ms: 123,
            }))
            .unwrap();
    });
    let sent = registry
        .inject_directed(
            flow_id,
            WebSocketDirection::ServerToClient,
            WebSocketKind::Text,
            b"injected".to_vec(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(sent.sequence, 7);
    assert_eq!(sent.direction, WebSocketDirection::ServerToClient);
    worker.await.unwrap();

    drop(guard);
    assert!(!registry.contains(flow_id));
    assert!(
        registry
            .inject_directed(
                flow_id,
                WebSocketDirection::ClientToServer,
                WebSocketKind::Text,
                b"after-drop".to_vec(),
                Duration::from_secs(1),
            )
            .await
            .is_err()
    );
}
