//! Versioned, SDK-independent data exchanged by MIO services and plugins.

pub mod panel;
pub mod protocol;
pub use protocol::{MatchAndReplaceRule, MatchReplaceTarget};

use serde::{Deserialize, Serialize};
pub use uuid::Uuid;

pub const API_VERSION: u32 = 1;
pub type FlowId = Uuid;
pub type TabId = Uuid;
pub type ProjectId = Uuid;
pub type TaskId = Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectKey {
    pub id: ProjectId,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpVersion {
    Http10,
    #[default]
    Http11,
    Http2,
}

impl HttpVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http10 => "HTTP/1.0",
            Self::Http11 => "HTTP/1.1",
            Self::Http2 => "HTTP/2",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub version: HttpVersion,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    pub trailers: Vec<Header>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub version: HttpVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_phrase: Option<Vec<u8>>,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    pub trailers: Vec<Header>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrafficSource {
    Proxy,
    Repeater,
    Intruder,
    Scanner,
    Plugin(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowState {
    Pending,
    Intercepted,
    Complete,
    Dropped,
    Cancelled,
    Failed(String),
}

impl FlowState {
    pub fn label(&self) -> &str {
        match self {
            Self::Pending => "等待响应",
            Self::Intercepted => "等待拦截决定",
            Self::Complete => "完成",
            Self::Dropped => "已丢弃",
            Self::Cancelled => "已取消",
            Self::Failed(_) => "失败",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowSummary {
    pub id: FlowId,
    pub revision: u64,
    pub source: TrafficSource,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub state: FlowState,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub started_unix_ms: i64,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRecord {
    pub summary: FlowSummary,
    pub request: HttpRequest,
    pub response: Option<HttpResponse>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebSocketDirection {
    ClientToServer,
    ServerToClient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebSocketKind {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
}

/// A received message, not an acknowledgement of delivery to the other peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketMessage {
    pub id: Uuid,
    pub revision: u64,
    pub flow_id: FlowId,
    pub sequence: u64,
    pub direction: WebSocketDirection,
    pub kind: WebSocketKind,
    /// Close payloads retain the optional big-endian status code followed by reason bytes.
    pub payload: Vec<u8>,
    pub observed_unix_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_bodies_and_duplicate_headers_are_not_text_normalized() {
        let request = HttpRequest {
            method: "POST".into(),
            url: "https://localhost/".into(),
            version: HttpVersion::Http2,
            headers: vec![
                Header {
                    name: "x-test".into(),
                    value: b"first".to_vec(),
                },
                Header {
                    name: "x-test".into(),
                    value: vec![0xff],
                },
            ],
            body: vec![0, 0xff, 0xc3],
            trailers: Vec::new(),
        };
        assert_eq!(request.headers.len(), 2);
        assert_eq!(request.body, [0, 0xff, 0xc3]);
        assert_eq!(HttpVersion::Http2.as_str(), "HTTP/2");
    }
}
