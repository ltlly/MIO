use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    FlowId, Header, HttpResponse, ProjectKey, TabId, Uuid, WebSocketDirection, WebSocketKind,
    WebSocketMessage,
};

pub const MAX_WEBSOCKET_REPEATER_TABS: usize = 64;
pub const MAX_WEBSOCKET_REPEATER_HISTORY: usize = 512;
pub const MAX_WEBSOCKET_REPEATER_HISTORY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSocketRepeaterConnectionMode {
    #[default]
    Disconnected,
    ProxyShared {
        flow_id: FlowId,
    },
    Standalone {
        flow_id: FlowId,
    },
}

impl WebSocketRepeaterConnectionMode {
    pub fn is_connected(self) -> bool {
        !matches!(self, Self::Disconnected)
    }

    pub fn active_flow_id(self) -> Option<FlowId> {
        match self {
            Self::Disconnected => None,
            Self::ProxyShared { flow_id } | Self::Standalone { flow_id } => Some(flow_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketRepeaterTab {
    pub id: TabId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Uuid>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handshake_headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_flow_id: Option<FlowId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<Uuid>,
    pub direction: WebSocketDirection,
    pub draft_revision: u64,
    pub draft_payload: Vec<u8>,
    pub draft_kind: WebSocketKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<WebSocketRepeaterHistoryItem>,
    #[serde(default)]
    pub closed: bool,
}

impl WebSocketRepeaterTab {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.title.is_empty() && self.title.len() <= 4096,
            "WebSocket Repeater 标签标题长度必须为 1–4096 字节"
        );
        ensure!(
            self.url.len() <= 64 * 1024,
            "WebSocket Repeater 目标 URL 超过 64 KiB 限额"
        );
        ensure!(
            self.handshake_headers.len() <= 256,
            "WebSocket Repeater 握手标头超过 256 条限额"
        );
        let mut head_bytes = 0usize;
        for header in &self.handshake_headers {
            ensure!(
                crate::http::token(header.name.as_bytes()),
                "WebSocket Repeater 握手标头名称无效"
            );
            head_bytes = head_bytes
                .saturating_add(header.name.len())
                .saturating_add(header.value.len());
            ensure!(
                head_bytes <= crate::http::MAX_HEAD,
                "WebSocket Repeater 握手标头总长度超过 128 KiB 限额"
            );
        }
        ensure!(
            self.draft_revision > 0,
            "WebSocket Repeater 草稿 revision 必须大于零"
        );
        ensure!(
            self.draft_payload.len() <= crate::http::MAX_BODY,
            "WebSocket Repeater 草稿超过 16 MiB 限额"
        );
        if self.draft_kind == WebSocketKind::Text {
            // Draft bytes stay binary-safe while editing; Send enforces valid UTF-8 on wire.
        }
        ensure!(
            self.history.len() <= MAX_WEBSOCKET_REPEATER_HISTORY,
            "WebSocket Repeater 历史消息超过 {} 条限额",
            MAX_WEBSOCKET_REPEATER_HISTORY
        );
        let mut total_history_bytes = 0usize;
        for item in &self.history {
            ensure!(
                item.payload.len() <= crate::http::MAX_BODY,
                "WebSocket Repeater 历史消息超过 16 MiB 限额"
            );
            total_history_bytes = total_history_bytes
                .checked_add(item.payload.len())
                .filter(|&sum| sum <= MAX_WEBSOCKET_REPEATER_HISTORY_BYTES)
                .ok_or_else(|| {
                    anyhow::anyhow!("WebSocket Repeater 单标签历史总字节数超过 32 MiB 限额")
                })?;
        }
        Ok(())
    }

    pub fn push_history(&mut self, item: WebSocketRepeaterHistoryItem) {
        self.history.push(item);
        while self.history.len() > MAX_WEBSOCKET_REPEATER_HISTORY {
            self.history.remove(0);
        }
        let mut total: usize = self.history.iter().map(|entry| entry.payload.len()).sum();
        while total > MAX_WEBSOCKET_REPEATER_HISTORY_BYTES && self.history.len() > 1 {
            let removed = self.history.remove(0);
            total = total.saturating_sub(removed.payload.len());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketRepeaterHistoryItem {
    pub id: Uuid,
    pub message_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<FlowId>,
    #[serde(default)]
    pub sequence: u64,
    pub direction: WebSocketDirection,
    pub kind: WebSocketKind,
    #[serde(default)]
    pub payload: Vec<u8>,
    pub observed_unix_ms: i64,
}

impl WebSocketRepeaterHistoryItem {
    pub fn from_message(message: &WebSocketMessage) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_id: message.id,
            flow_id: Some(message.flow_id),
            sequence: message.sequence,
            direction: message.direction,
            kind: message.kind,
            payload: message.payload.clone(),
            observed_unix_ms: message.observed_unix_ms,
        }
    }
}

#[derive(Debug)]
pub enum WebSocketRepeaterCommand {
    Open {
        project: ProjectKey,
        operation_id: Uuid,
        title: Option<String>,
        url: String,
        handshake_headers: Vec<Header>,
        source_flow_id: Option<FlowId>,
        source_message_id: Option<Uuid>,
        source_direction: WebSocketDirection,
        source_kind: WebSocketKind,
        source_payload: Vec<u8>,
    },
    OpenFromHistory {
        project: ProjectKey,
        operation_id: Uuid,
        message_id: Uuid,
        use_original: bool,
        payload_override: Option<(WebSocketKind, Vec<u8>)>,
    },
    Connect {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        url: String,
        handshake_headers: Vec<Header>,
        source_flow_id: Option<FlowId>,
        cancel: CancellationToken,
    },
    Send {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        url: String,
        source_flow_id: Option<FlowId>,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        draft_revision: u64,
        cancel: CancellationToken,
    },
    Disconnect {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
    },
    Close {
        project: ProjectKey,
        tab_id: TabId,
    },
}

#[derive(Clone, Debug)]
pub enum WebSocketRepeaterEvent {
    Opened {
        project: ProjectKey,
        operation_id: Uuid,
        result: Result<WebSocketRepeaterTab, String>,
        mode: WebSocketRepeaterConnectionMode,
    },
    Connected {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        mode: WebSocketRepeaterConnectionMode,
        response: Option<Box<HttpResponse>>,
        result: Result<(), String>,
    },
    Sent {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        draft_revision: u64,
        mode: WebSocketRepeaterConnectionMode,
        result: Result<WebSocketRepeaterHistoryItem, String>,
    },
    Received {
        project: ProjectKey,
        tab_id: TabId,
        flow_id: FlowId,
        item: WebSocketRepeaterHistoryItem,
    },
    SessionClosed {
        project: ProjectKey,
        tab_id: TabId,
        mode: WebSocketRepeaterConnectionMode,
        reason: Option<String>,
    },
    Disconnected {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        result: Result<(), String>,
    },
    Closed {
        project: ProjectKey,
        tab_id: TabId,
    },
}
