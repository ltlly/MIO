//! Protocol v1: a four-byte big-endian length followed by one typed Protobuf envelope.

use std::time::{SystemTime, UNIX_EPOCH};

use prost::{Enumeration, Message, Oneof};
use serde::{Deserialize, Serialize};

use crate::{ProjectKey, Uuid, panel::*};

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 6;
pub const LEGACY_COMPAT_MINOR: u32 = 5;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_INLINE_BODY: usize = 512 * 1024;
pub const MAX_STORAGE_KEY_BYTES: usize = 256;
pub const MAX_STORAGE_VALUE_BYTES: usize = 64 * 1024;
pub const PANEL_ID: u64 = 1;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Enumeration, Serialize, Deserialize,
)]
#[repr(i32)]
pub enum Permission {
    #[serde(rename = "traffic.read")]
    TrafficRead = 0,
    #[serde(rename = "draft.write")]
    DraftWrite = 1,
    #[serde(rename = "repeater.open")]
    RepeaterOpen = 2,
    #[serde(rename = "network.send")]
    NetworkSend = 3,
    #[serde(rename = "traffic.modify")]
    TrafficModify = 4,
    #[serde(rename = "session.hook")]
    SessionHook = 5,
    #[serde(rename = "payload.process")]
    PayloadProcess = 6,
    #[serde(rename = "storage.kv")]
    Storage = 7,
    #[serde(rename = "audit.write")]
    AuditWrite = 8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum Component {
    Row = 0,
    Column = 1,
    Split = 2,
    Tabs = 3,
    Text = 4,
    Button = 5,
    Editor = 6,
    VirtualTable = 7,
    Canvas = 8,
    ScrollColumn = 9,
    Checkbox = 10,
    Select = 11,
    Badge = 12,
    Progress = 13,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelIdentity {
    pub instance_id: Uuid,
    pub project: ProjectKey,
}

#[derive(Clone, PartialEq, Message)]
pub struct Envelope {
    #[prost(uint32, tag = "1")]
    pub major: u32,
    #[prost(uint32, tag = "2")]
    pub minor: u32,
    /// Strictly increasing per sender; replies use in_reply_to rather than reusing IDs.
    #[prost(uint64, tag = "3")]
    pub request_id: u64,
    #[prost(bytes = "vec", tag = "4")]
    pub instance_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub project_id: Vec<u8>,
    #[prost(uint64, tag = "6")]
    pub project_generation: u64,
    #[prost(uint64, tag = "7")]
    pub panel_id: u64,
    #[prost(uint64, tag = "8")]
    pub revision: u64,
    #[prost(uint64, tag = "9")]
    pub deadline_unix_ms: u64,
    #[prost(uint64, tag = "10")]
    pub in_reply_to: u64,
    #[prost(
        oneof = "envelope::Body",
        tags = "20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53"
    )]
    pub body: Option<envelope::Body>,
}

pub mod envelope {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Body {
        #[prost(message, tag = "20")]
        Hello(Hello),
        #[prost(message, tag = "21")]
        Register(Register),
        #[prost(message, tag = "22")]
        Ready(Empty),
        #[prost(message, tag = "23")]
        Snapshot(PanelSnapshot),
        #[prost(message, tag = "24")]
        Patch(PanelPatch),
        #[prost(message, tag = "25")]
        UiEvent(UiEvent),
        #[prost(message, tag = "26")]
        OpenRequest(OpenRequest),
        #[prost(message, tag = "27")]
        CommitEditor(CommitEditor),
        #[prost(message, tag = "28")]
        Committed(Committed),
        #[prost(message, tag = "29")]
        HostCall(HostCall),
        #[prost(message, tag = "30")]
        HostResult(HostResult),
        #[prost(message, tag = "31")]
        Cancel(Cancel),
        #[prost(message, tag = "32")]
        Shutdown(Empty),
        #[prost(message, tag = "33")]
        Error(Failure),
        #[prost(message, tag = "34")]
        Resync(PanelSnapshot),
        #[prost(message, tag = "35")]
        Ack(Empty),
        #[prost(message, tag = "36")]
        CloseRequest(CloseRequest),
        #[prost(message, tag = "37")]
        HttpHookCall(HttpHookCall),
        #[prost(message, tag = "38")]
        HttpHookResult(HttpHookResult),
        #[prost(message, tag = "39")]
        WebSocketHookCall(WebSocketHookCall),
        #[prost(message, tag = "40")]
        WebSocketHookResult(WebSocketHookResult),
        #[prost(message, tag = "41")]
        SessionActionCall(SessionActionCall),
        #[prost(message, tag = "42")]
        SessionActionResult(SessionActionResult),
        #[prost(message, tag = "43")]
        PayloadCall(PayloadCall),
        #[prost(message, tag = "44")]
        PayloadResult(PayloadResult),
        #[prost(message, tag = "45")]
        OpenResponse(OpenResponse),
        #[prost(message, tag = "46")]
        CommitResponseEditor(CommitResponseEditor),
        #[prost(message, tag = "47")]
        ResponseCommitted(ResponseCommitted),
        #[prost(message, tag = "48")]
        ContextMenuCall(ContextMenuCall),
        #[prost(message, tag = "49")]
        ContextMenuResult(ContextMenuResult),
        #[prost(message, tag = "50")]
        PassiveCheckCall(PassiveCheckCall),
        #[prost(message, tag = "51")]
        PassiveCheckResult(PassiveCheckResult),
        #[prost(message, tag = "52")]
        ActiveCheckCall(ActiveCheckCall),
        #[prost(message, tag = "53")]
        ActiveCheckResult(ActiveCheckResult),
    }
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub fn uuid(bytes: &[u8]) -> Result<Uuid, String> {
    let id = Uuid::from_slice(bytes).map_err(|_| "UUID 必须为 16 字节")?;
    if id.is_nil() {
        return Err("UUID 不能为 nil".into());
    }
    Ok(id)
}

mod dto;
mod hooks;
#[cfg(test)]
mod tests;
mod wire;

pub use dto::*;
pub use hooks::*;
pub use wire::*;
