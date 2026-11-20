use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryEntryId {
    Live(FlowId),
    External { source_id: Uuid, item_id: Uuid },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpExtensionFilter {
    #[default]
    Any,
    Only(String),
    Hide(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHistoryFilter {
    pub in_scope_only: bool,
    pub hide_without_response: bool,
    pub parameterized_only: bool,
    pub status_classes: [bool; 4],
    pub mime_types: [bool; 8],
    pub extensions: HttpExtensionFilter,
    pub only_with_notes: bool,
    pub only_highlighted: bool,
    pub listener_port: Option<u16>,
    pub search_term: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub negative_search: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter_script: String,
}

impl Default for HttpHistoryFilter {
    fn default() -> Self {
        Self {
            in_scope_only: false,
            hide_without_response: false,
            parameterized_only: false,
            status_classes: [true; 4],
            mime_types: [true; 8],
            extensions: HttpExtensionFilter::Any,
            only_with_notes: false,
            only_highlighted: false,
            listener_port: None,
            search_term: String::new(),
            regex: false,
            case_sensitive: false,
            negative_search: false,
            filter_script: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HttpHistorySortColumn {
    #[default]
    Number,
    Host,
    Method,
    Url,
    Params,
    Edited,
    Status,
    Length,
    MimeType,
    Extension,
    Title,
    Notes,
    Tls,
    Ip,
    Cookies,
    Time,
    ListenerPort,
    StartResponseTimer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpHistorySort {
    pub column: HttpHistorySortColumn,
    pub ascending: bool,
}

impl Default for HttpHistorySort {
    fn default() -> Self {
        Self {
            column: HttpHistorySortColumn::Number,
            ascending: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpHistoryQuery {
    pub project: ProjectKey,
    pub query_revision: u64,
    pub offset: usize,
    pub limit: usize,
    pub url_filter: String,
    pub filter: HttpHistoryFilter,
    pub sort: HttpHistorySort,
    pub selected_ids: Vec<HistoryEntryId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpHistoryResult {
    pub total: usize,
    pub rows: Vec<HistoryRow>,
    pub selected_rows: Vec<HistoryRow>,
    pub selected_matches: Vec<(HistoryEntryId, bool)>,
    pub selected_ranks: Vec<(HistoryEntryId, usize)>,
    pub live_annotations: Vec<LiveHttpAnnotationEntry>,
    pub live_numbers: Vec<(FlowId, u64)>,
    pub row_metadata: Vec<(HistoryEntryId, HttpHistoryRowMetadata)>,
}

pub type LiveHttpAnnotationEntry = (FlowId, u64, String, ExtHttpHighlight);

impl HttpHistoryResult {
    pub fn live_annotations(&self) -> &[LiveHttpAnnotationEntry] {
        &self.live_annotations
    }

    pub fn row_metadata(&self) -> &[(HistoryEntryId, HttpHistoryRowMetadata)] {
        &self.row_metadata
    }

    pub fn row_metadata_for(&self, id: HistoryEntryId) -> Option<&HttpHistoryRowMetadata> {
        self.row_metadata
            .iter()
            .find(|(entry_id, _)| *entry_id == id)
            .map(|(_, meta)| meta)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHistoryRowMetadata {
    #[serde(default)]
    pub has_params: bool,
    #[serde(default = "default_params_known")]
    pub params_known: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_error: Option<String>,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub has_original_request: bool,
    #[serde(default)]
    pub has_original_response: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_length: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_error: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_ip: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cookies: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_received_ms: Option<u64>,
    #[serde(default = "default_time_known")]
    pub time_known: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_number: Option<u64>,
}

const fn default_params_known() -> bool {
    true
}

const fn default_time_known() -> bool {
    true
}

impl HttpHistoryRowMetadata {
    pub const fn params_state(&self) -> Option<bool> {
        if self.params_known {
            Some(self.has_params)
        } else {
            None
        }
    }

    pub const fn params_cell_text(&self) -> &'static str {
        match self.params_state() {
            Some(true) => "✓",
            Some(false) => "",
            None => "?",
        }
    }
}

impl Default for HttpHistoryRowMetadata {
    fn default() -> Self {
        Self {
            has_params: false,
            params_known: false,
            params_error: None,
            edited: false,
            has_original_request: false,
            has_original_response: false,
            response_length: None,
            mime_type: String::new(),
            title: String::new(),
            title_error: None,
            remote_ip: String::new(),
            cookies: String::new(),
            cookies_error: None,
            listener_port: None,
            response_received_ms: None,
            time_known: true,
            capture_number: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HttpHistoryPending {
    pub summary: FlowSummary,
    pub first_request_method: String,
    pub first_request_url: String,
    pub first_response_status: Option<u16>,
    pub first_request: Option<HttpRequest>,
    pub first_response: Option<HttpResponse>,
    pub request: Option<HttpRequest>,
    pub response: Option<HttpResponse>,
    pub listener_port: Option<u16>,
    pub capture_metrics: mio_proxy::CapturedFlowMetrics,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryTransferEntry {
    Live {
        flow_id: FlowId,
        revision: u64,
        original: bool,
    },
    External {
        source_id: Uuid,
        item_id: Uuid,
        message_id: Uuid,
    },
    SiteMap {
        row_id: HistoryEntryId,
        entry_id: Uuid,
        revision: u64,
    },
}

impl HistoryTransferEntry {
    pub fn id(&self) -> HistoryEntryId {
        match *self {
            Self::SiteMap { row_id, .. } => row_id,
            Self::Live { flow_id, .. } => HistoryEntryId::Live(flow_id),
            Self::External {
                source_id, item_id, ..
            } => HistoryEntryId::External { source_id, item_id },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryRow {
    Live(FlowSummary),
    ExtProxy(Box<ExtHistoryRow>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtHistoryRow {
    pub item: ExtItemSummary,
    pub request: Option<ExtMessageSummary>,
    pub edited_request: Option<ExtMessageSummary>,
    pub response: Option<ExtMessageSummary>,
    pub edited_response: Option<ExtMessageSummary>,
    pub origin: Option<String>,
    pub time_unix_ms: Option<i64>,
    pub time_error: Option<String>,
    pub notes_revision: u64,
    pub notes_preview: String,
    pub notes_error: Option<String>,
    pub highlight: Option<ExtHttpHighlight>,
    pub highlight_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtHttpHighlight {
    pub revision: u64,
    pub color: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHttpHighlightChange {
    pub source_id: Uuid,
    pub item_id: Uuid,
    pub expected_highlight_revision: u64,
    pub color: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHttpHighlightError {
    pub message: String,
    pub stored: Vec<(Uuid, Uuid, ExtHttpHighlight)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtHttpNotes {
    pub revision: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHttpNotesChange {
    pub source_id: Uuid,
    pub item_id: Uuid,
    pub expected_notes_revision: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHttpNotesError {
    pub message: String,
    pub stored: Option<ExtHttpNotes>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHttpNotesBatch {
    pub states: Vec<(Uuid, Uuid, u64, bool)>,
    pub common_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtHttpNotesBatchError {
    pub message: String,
    pub stored: Vec<(Uuid, Uuid, u64, bool)>,
}

impl HistoryRow {
    pub fn id(&self) -> HistoryEntryId {
        match self {
            Self::Live(flow) => HistoryEntryId::Live(flow.id),
            Self::ExtProxy(row) => HistoryEntryId::External {
                source_id: row.item.source_id,
                item_id: row.item.id,
            },
        }
    }

    pub fn live(&self) -> Option<&FlowSummary> {
        match self {
            Self::Live(flow) => Some(flow),
            Self::ExtProxy(_) => None,
        }
    }
}

impl From<FlowSummary> for HistoryRow {
    fn from(value: FlowSummary) -> Self {
        Self::Live(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSearchQuery {
    pub project: ProjectKey,
    pub query_revision: u64,
    pub term: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub in_scope_only: bool,
    pub limit: usize,
    pub workspace: Workspace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlobalSearchTarget {
    ProxyHttp(HistoryEntryId),
    ProxyWebSocket(Uuid),
    TargetSiteMap {
        node_key: String,
        entry_id: HistoryEntryId,
    },
    RepeaterHttp(TabId),
    RepeaterWebSocket(TabId),
    IntruderTab(TabId),
    OrganiserItem(Uuid),
    LoggerFlow(FlowId),
    AuditIssue(Uuid),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSearchHit {
    pub target: GlobalSearchTarget,
    pub tool_label: String,
    pub location_label: String,
    pub method_or_direction: String,
    pub url: String,
    pub status: Option<u16>,
    pub matched_in: String,
    pub snippet: String,
    pub notes_preview: String,
    pub history_row: Option<HistoryRow>,
    pub websocket_summary: Option<WebSocketSummary>,
    pub site_map_entry: Option<(Uuid, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSearchResult {
    pub total: usize,
    pub hits: Vec<GlobalSearchHit>,
}
