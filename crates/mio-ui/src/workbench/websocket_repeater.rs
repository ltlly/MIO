use std::collections::{HashMap, HashSet};

use gpui_kit::component::{
    button::ButtonVariants,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
};
use gpui_kit::{AnyElement, Context, Entity, Subscription, Window};
use mio_core::{
    Command, Header, TabId, Uuid, WebSocketDirection, WebSocketKind, WebSocketRepeaterCommand,
    WebSocketRepeaterConnectionMode, WebSocketRepeaterEvent, WebSocketRepeaterTab,
};
use tokio_util::sync::CancellationToken;

use super::*;

mod actions;
mod events;
mod render;
mod state;

const MAX_WS_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_WS_URL_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct WebSocketRepeaterState {
    pub(super) editors: HashMap<TabId, WsRepeaterEditors>,
}

pub(super) struct WsRepeaterEditors {
    pub(super) draft: MessageEditor,
    pub(super) history: MessageEditor,
    pub(super) url: Entity<InputState>,
    pub(super) header_name: Entity<InputState>,
    pub(super) header_value: Entity<InputState>,
    pub(super) headers_expanded: bool,
    pub(super) auto_follow_latest: bool,
    pub(super) mode: WebSocketRepeaterConnectionMode,
    pub(super) connecting: Option<CancellationToken>,
    pub(super) sending: Option<(u64, CancellationToken)>,
    pub(super) disconnecting: bool,
    pub(super) selected_history: Option<Uuid>,
    pub(super) dirty: bool,
    pub(super) loaded_revision: u64,
    pub(super) result: Option<String>,
    pub(super) _subscriptions: Vec<Subscription>,
}

pub(super) struct WsDraftSnapshot {
    pub(super) id: TabId,
    pub(super) url: String,
    pub(super) payload: Vec<u8>,
}

fn mode_label(mode: WebSocketRepeaterConnectionMode) -> &'static str {
    match mode {
        WebSocketRepeaterConnectionMode::Disconnected => "未连接",
        WebSocketRepeaterConnectionMode::ProxyShared { .. } => "复用代理连接",
        WebSocketRepeaterConnectionMode::Standalone { .. } => "独立会话已连接",
    }
}

fn kind_label(kind: WebSocketKind) -> &'static str {
    match kind {
        WebSocketKind::Text => "Text",
        WebSocketKind::Binary => "Binary",
        WebSocketKind::Ping => "Ping",
        WebSocketKind::Pong => "Pong",
        WebSocketKind::Close => "Close",
    }
}

fn preview_payload(payload: &[u8]) -> String {
    const MAX_PREVIEW_CHARS: usize = 96;
    match std::str::from_utf8(payload) {
        Ok(text) => {
            let cleaned: String = text
                .chars()
                .map(|ch| {
                    if matches!(ch, '\r' | '\n' | '\t') {
                        ' '
                    } else {
                        ch
                    }
                })
                .take(MAX_PREVIEW_CHARS + 1)
                .collect();
            if cleaned.chars().count() > MAX_PREVIEW_CHARS {
                let head: String = cleaned.chars().take(MAX_PREVIEW_CHARS).collect();
                format!("{head}…")
            } else {
                cleaned
            }
        }
        Err(_) => {
            let head = &payload[..payload.len().min(24)];
            let hex = editing::hex_encode(head);
            if payload.len() > 24 {
                format!("{hex} …")
            } else {
                hex
            }
        }
    }
}
