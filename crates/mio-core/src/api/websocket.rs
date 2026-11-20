use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketSummary {
    pub id: Uuid,
    pub ordinal: u64,
    pub connection_id: u64,
    pub listener_port: Option<u16>,
    pub revision: u64,
    pub flow_id: FlowId,
    pub sequence: u64,
    pub direction: WebSocketDirection,
    pub kind: WebSocketKind,
    pub payload_bytes: u64,
    pub observed_unix_ms: i64,
    pub url: String,
    pub edited: bool,
    pub notes_revision: u64,
    pub notes_preview: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSocketNotes {
    pub revision: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSocketNotesError {
    pub message: String,
    pub stored: Option<WebSocketNotes>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSocketNotesChange {
    pub message_id: Uuid,
    pub expected_notes_revision: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketHistoryFilter {
    pub hide_incoming: bool,
    pub hide_outgoing: bool,
    pub listener_port: Option<u16>,
    pub term: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub negative: bool,
    pub only_with_notes: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter_script: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WebSocketHistoryQuery {
    pub project: ProjectKey,
    pub query_revision: u64,
    pub filter: WebSocketHistoryFilter,
    pub flow_id: Option<FlowId>,
    pub offset: usize,
    pub limit: usize,
    pub selected_message_id: Option<Uuid>,
}

#[derive(Debug)]
pub(crate) struct WebSocketHistoryResult {
    pub total: usize,
    pub rows: Vec<WebSocketSummary>,
    pub selected_match: Option<(Uuid, bool)>,
}

impl WebSocketSummary {
    pub(crate) fn from_message(
        message: &WebSocketMessage,
        url: String,
        ordinal: u64,
        connection_id: u64,
        listener_port: Option<u16>,
        edited: bool,
    ) -> Self {
        Self {
            id: message.id,
            ordinal,
            connection_id,
            listener_port,
            revision: message.revision,
            flow_id: message.flow_id,
            sequence: message.sequence,
            direction: message.direction,
            kind: message.kind,
            payload_bytes: message.payload.len() as u64,
            observed_unix_ms: message.observed_unix_ms,
            url,
            edited,
            notes_revision: 0,
            notes_preview: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketDecision {
    Forward(Vec<u8>),
    Drop,
}
