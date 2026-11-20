use prost::{Enumeration, Message, Oneof};

use crate::FlowSummary;

use super::*;
#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    #[prost(uint32, tag = "1")]
    pub max_frame_bytes: u32,
    #[prost(uint32, tag = "2")]
    pub max_operation_ms: u32,
    #[prost(enumeration = "Permission", repeated, tag = "3")]
    pub granted_permissions: Vec<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Register {
    #[prost(string, tag = "1")]
    pub plugin_id: String,
    #[prost(string, tag = "2")]
    pub plugin_version: String,
    #[prost(enumeration = "Component", repeated, tag = "3")]
    pub required_components: Vec<i32>,
    #[prost(message, optional, tag = "4")]
    pub panel: Option<PanelSnapshot>,
}

#[derive(Clone, PartialEq, Message)]
pub struct OpenRequest {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub message_revision: u64,
    #[prost(bool, tag = "3")]
    pub read_only: bool,
    #[prost(message, optional, tag = "4")]
    pub request: Option<RequestDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct OpenResponse {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub message_revision: u64,
    #[prost(bool, tag = "3")]
    pub read_only: bool,
    #[prost(message, optional, tag = "4")]
    pub response: Option<ResponseDto>,
}

/// Detach this resource without discarding editor contents or reusing document revisions.
/// Reply with a correlated panel update, or Ack if no current context matches.
#[derive(Clone, PartialEq, Message)]
pub struct CloseRequest {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommitEditor {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub message_revision: u64,
    #[prost(message, optional, tag = "3")]
    pub request: Option<RequestDto>,
    /// Captured by the native editor after composition is committed, not a debounced worker copy.
    #[prost(message, optional, tag = "4")]
    pub editor: Option<EditorSnapshot>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommitResponseEditor {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub message_revision: u64,
    #[prost(message, optional, tag = "3")]
    pub response: Option<ResponseDto>,
    #[prost(message, optional, tag = "4")]
    pub editor: Option<EditorSnapshot>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Committed {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub message_revision: u64,
    #[prost(uint64, tag = "3")]
    pub document_id: u64,
    #[prost(uint64, tag = "4")]
    pub document_revision: u64,
    #[prost(message, optional, tag = "5")]
    pub request: Option<RequestDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ResponseCommitted {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub message_revision: u64,
    #[prost(uint64, tag = "3")]
    pub document_id: u64,
    #[prost(uint64, tag = "4")]
    pub document_revision: u64,
    #[prost(message, optional, tag = "5")]
    pub response: Option<ResponseDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct HostCall {
    #[prost(
        oneof = "host_call::Operation",
        tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19"
    )]
    pub operation: Option<host_call::Operation>,
}

impl HostCall {
    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::decode(bytes).map_err(|_| "无法解码 HostCall".into())
    }
}

pub mod host_call {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Operation {
        #[prost(message, tag = "1")]
        QueryHistory(HistoryQuery),
        #[prost(bytes, tag = "2")]
        ReadFlow(Vec<u8>),
        #[prost(message, tag = "3")]
        UpdateDraft(DraftUpdate),
        #[prost(message, tag = "4")]
        OpenRepeater(RequestDto),
        #[prost(message, tag = "5")]
        NetworkSend(RequestDto),
        #[prost(string, tag = "6")]
        StorageGet(String),
        #[prost(message, tag = "7")]
        StorageSet(StorageEntry),
        #[prost(message, tag = "8")]
        OpenIntruder(RequestDto),
        #[prost(string, tag = "9")]
        PreferencesGet(String),
        #[prost(message, tag = "10")]
        PreferencesSet(StorageEntry),
        #[prost(string, tag = "11")]
        ScopeCheck(String),
        #[prost(message, tag = "12")]
        ScopeUpdate(ScopeUpdateDto),
        #[prost(message, tag = "13")]
        QuerySiteMap(SiteMapQueryDto),
        #[prost(bytes, tag = "14")]
        ReadAnnotations(Vec<u8>),
        #[prost(message, tag = "15")]
        UpdateAnnotations(UpdateAnnotationsDto),
        #[prost(message, tag = "16")]
        OpenOrganiser(OpenOrganiserDto),
        #[prost(message, tag = "17")]
        SubmitAuditIssue(SubmitAuditIssueDto),
        #[prost(message, tag = "18")]
        CollaboratorGenerate(CollaboratorGenerateDto),
        #[prost(message, tag = "19")]
        CollaboratorPoll(CollaboratorPollDto),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct HistoryQuery {
    #[prost(uint64, tag = "1")]
    pub offset: u64,
    #[prost(uint32, tag = "2")]
    pub limit: u32,
    #[prost(string, tag = "3")]
    pub filter: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct DraftUpdate {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub expected_revision: u64,
    #[prost(message, optional, tag = "3")]
    pub request: Option<RequestDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScopeUpdateDto {
    #[prost(string, tag = "1")]
    pub url_prefix: String,
    #[prost(bool, tag = "2")]
    pub exclude: bool,
    #[prost(bool, tag = "3")]
    pub include_subdomains: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScopeStatusDto {
    #[prost(bool, tag = "1")]
    pub in_scope: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct SiteMapQueryDto {
    #[prost(string, tag = "1")]
    pub prefix: String,
    #[prost(bool, tag = "2")]
    pub in_scope_only: bool,
    #[prost(uint64, tag = "3")]
    pub offset: u64,
    #[prost(uint32, tag = "4")]
    pub limit: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct SiteMapRowDto {
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    #[prost(string, tag = "2")]
    pub method: String,
    #[prost(string, tag = "3")]
    pub url: String,
    #[prost(uint32, optional, tag = "4")]
    pub status: Option<u32>,
    #[prost(uint64, tag = "5")]
    pub response_bytes: u64,
    #[prost(bool, tag = "6")]
    pub in_scope: bool,
    #[prost(string, tag = "7")]
    pub notes_preview: String,
    #[prost(uint32, tag = "8")]
    pub highlight_color: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct SiteMapPageDto {
    #[prost(uint64, tag = "1")]
    pub offset: u64,
    #[prost(uint64, tag = "2")]
    pub total: u64,
    #[prost(message, repeated, tag = "3")]
    pub rows: Vec<SiteMapRowDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AnnotationsDto {
    #[prost(bytes = "vec", tag = "1")]
    pub flow_id: Vec<u8>,
    #[prost(string, tag = "2")]
    pub notes: String,
    #[prost(uint32, tag = "3")]
    pub highlight_color: u32,
    #[prost(uint64, tag = "4")]
    pub notes_revision: u64,
    #[prost(uint64, tag = "5")]
    pub highlight_revision: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct UpdateAnnotationsDto {
    #[prost(bytes = "vec", tag = "1")]
    pub flow_id: Vec<u8>,
    #[prost(string, optional, tag = "2")]
    pub notes: Option<String>,
    #[prost(uint32, optional, tag = "3")]
    pub highlight_color: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct OpenOrganiserDto {
    #[prost(message, optional, tag = "1")]
    pub request: Option<RequestDto>,
    #[prost(message, optional, tag = "2")]
    pub response: Option<ResponseDto>,
    #[prost(string, tag = "3")]
    pub notes: String,
    #[prost(uint32, optional, tag = "4")]
    pub highlight_color: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SubmitAuditIssueDto {
    #[prost(bytes = "vec", tag = "1")]
    pub flow_id: Vec<u8>,
    #[prost(string, tag = "2")]
    pub url: String,
    #[prost(message, optional, tag = "3")]
    pub finding: Option<PluginFindingDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CollaboratorGenerateDto {
    #[prost(string, tag = "1")]
    pub label: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct CollaboratorPayloadDto {
    #[prost(string, tag = "1")]
    pub token: String,
    #[prost(string, tag = "2")]
    pub label: String,
    #[prost(string, tag = "3")]
    pub http_callback_url: String,
    #[prost(string, tag = "4")]
    pub dns_qname: String,
    #[prost(sint64, tag = "5")]
    pub created_unix_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct CollaboratorPollDto {
    #[prost(string, tag = "1")]
    pub token_filter: String,
    #[prost(uint32, tag = "2")]
    pub limit: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct CollaboratorInteractionDto {
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    #[prost(string, tag = "2")]
    pub token: String,
    #[prost(string, tag = "3")]
    pub protocol: String,
    #[prost(string, tag = "4")]
    pub peer_addr: String,
    #[prost(string, tag = "5")]
    pub summary: String,
    #[prost(bytes = "vec", tag = "6")]
    pub raw_request: Vec<u8>,
    #[prost(sint64, tag = "7")]
    pub observed_unix_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct CollaboratorInteractionsDto {
    #[prost(message, repeated, tag = "1")]
    pub interactions: Vec<CollaboratorInteractionDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct HostResult {
    #[prost(
        oneof = "host_result::Outcome",
        tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12"
    )]
    pub outcome: Option<host_result::Outcome>,
}

impl HostResult {
    pub fn encode_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::decode(bytes).map_err(|_| "无法解码 HostResult".into())
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.outcome.as_ref().ok_or("宿主结果为空")? {
            host_result::Outcome::History(page) => {
                if page.rows.len() > MAX_TABLE_ROWS
                    || page.offset > page.total
                    || page.rows.len() as u64 > page.total - page.offset
                {
                    return Err("宿主流量分页无效".into());
                }
                for row in &page.rows {
                    let _: FlowSummary = row.clone().try_into()?;
                }
            }
            host_result::Outcome::Record(record) => {
                let _: FlowSummary = record
                    .summary
                    .clone()
                    .ok_or("流量缺少 summary")?
                    .try_into()?;
                record
                    .request
                    .as_ref()
                    .ok_or("流量缺少 request")?
                    .validate()?;
                if let Some(response) = &record.response {
                    response.validate()?;
                }
            }
            host_result::Outcome::Response(response) => response.validate()?,
            host_result::Outcome::Resource(resource) => {
                uuid(&resource.resource_id)?;
                if resource.revision == 0 {
                    return Err("资源 revision 不能为零".into());
                }
            }
            host_result::Outcome::Error(failure) => {
                if ErrorCode::try_from(failure.code).is_err() || failure.message.len() > 4096 {
                    return Err("宿主错误消息无效".into());
                }
            }
            host_result::Outcome::Done(_) => {}
            host_result::Outcome::StorageValue(val) => {
                if val.value.len() > MAX_STORAGE_VALUE_BYTES
                    || (!val.present && !val.value.is_empty())
                {
                    return Err("插件 KV 存储返回值无效或超限".into());
                }
            }
            host_result::Outcome::ScopeStatus(_) => {}
            host_result::Outcome::SiteMap(page) => {
                if page.rows.len() > MAX_TABLE_ROWS
                    || page.offset > page.total
                    || page.rows.len() as u64 > page.total - page.offset
                {
                    return Err("宿主 SiteMap 分页无效".into());
                }
                for row in &page.rows {
                    uuid(&row.id)?;
                    if row.url.len() > 16 * 1024 || row.highlight_color > 9 {
                        return Err("宿主 SiteMap 行字段无效或超限".into());
                    }
                }
            }
            host_result::Outcome::Annotations(a) => {
                uuid(&a.flow_id)?;
                if a.notes.len() > 64 * 1024 || a.highlight_color > 9 {
                    return Err("宿主注解字段无效或超限".into());
                }
            }
            host_result::Outcome::CollaboratorPayload(p) => {
                if p.token.is_empty()
                    || p.token.len() > 256
                    || p.http_callback_url.len() > 2048
                    || p.dns_qname.len() > 512
                {
                    return Err("宿主 Collaborator payload 无效或超限".into());
                }
            }
            host_result::Outcome::CollaboratorInteractions(list) => {
                if list.interactions.len() > MAX_TABLE_ROWS {
                    return Err("宿主 Collaborator 交互列表超过限额".into());
                }
                for item in &list.interactions {
                    uuid(&item.id)?;
                    if item.raw_request.len() > MAX_INLINE_BODY {
                        return Err("宿主 Collaborator 交互报文超过内联限额".into());
                    }
                }
            }
        }
        Ok(())
    }
}

pub mod host_result {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Outcome {
        #[prost(message, tag = "1")]
        History(HistoryPage),
        #[prost(message, boxed, tag = "2")]
        Record(Box<RecordDto>),
        #[prost(message, tag = "3")]
        Response(ResponseDto),
        #[prost(message, tag = "4")]
        Resource(ResourceRevision),
        #[prost(message, tag = "5")]
        Done(Empty),
        #[prost(message, tag = "6")]
        Error(Failure),
        #[prost(message, tag = "7")]
        StorageValue(StorageValue),
        #[prost(message, tag = "8")]
        ScopeStatus(ScopeStatusDto),
        #[prost(message, tag = "9")]
        SiteMap(SiteMapPageDto),
        #[prost(message, tag = "10")]
        Annotations(AnnotationsDto),
        #[prost(message, tag = "11")]
        CollaboratorPayload(CollaboratorPayloadDto),
        #[prost(message, tag = "12")]
        CollaboratorInteractions(CollaboratorInteractionsDto),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct StorageEntry {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(bytes = "vec", tag = "2")]
    pub value: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct StorageValue {
    #[prost(bool, tag = "1")]
    pub present: bool,
    #[prost(bytes = "vec", tag = "2")]
    pub value: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum HookAction {
    Pass = 0,
    Modify = 1,
    Intercept = 2,
    Drop = 3,
}

#[derive(Clone, PartialEq, Message)]
pub struct HttpHookCall {
    #[prost(bytes = "vec", tag = "1")]
    pub flow_id: Vec<u8>,
    #[prost(enumeration = "SourceDto", tag = "2")]
    pub source: i32,
    #[prost(string, tag = "3")]
    pub plugin_id: String,
    #[prost(bool, tag = "4")]
    pub response_stage: bool,
    #[prost(bool, tag = "5")]
    pub response_head_only: bool,
    #[prost(message, optional, tag = "6")]
    pub request: Option<RequestDto>,
    #[prost(message, optional, tag = "7")]
    pub response: Option<ResponseDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct HttpHookResult {
    #[prost(enumeration = "HookAction", tag = "1")]
    pub action: i32,
    #[prost(message, optional, tag = "2")]
    pub request: Option<RequestDto>,
    #[prost(message, optional, tag = "3")]
    pub response: Option<ResponseDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WebSocketHookCall {
    #[prost(bytes = "vec", tag = "1")]
    pub flow_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub message_id: Vec<u8>,
    #[prost(string, tag = "3")]
    pub url: String,
    #[prost(bool, tag = "4")]
    pub client_to_server: bool,
    #[prost(bool, tag = "5")]
    pub is_binary: bool,
    #[prost(bytes = "vec", tag = "6")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WebSocketHookResult {
    #[prost(enumeration = "HookAction", tag = "1")]
    pub action: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SessionActionCall {
    #[prost(enumeration = "SourceDto", tag = "1")]
    pub source: i32,
    #[prost(message, optional, tag = "2")]
    pub request: Option<RequestDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SessionActionResult {
    #[prost(bool, tag = "1")]
    pub modified: bool,
    #[prost(message, optional, tag = "2")]
    pub request: Option<RequestDto>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum PayloadOp {
    Generate = 0,
    Process = 1,
}

#[derive(Clone, PartialEq, Message)]
pub struct PayloadCall {
    #[prost(enumeration = "PayloadOp", tag = "1")]
    pub op: i32,
    #[prost(string, tag = "2")]
    pub config: String,
    #[prost(bytes = "vec", repeated, tag = "3")]
    pub inputs: Vec<Vec<u8>>,
    #[prost(uint32, tag = "4")]
    pub max_items: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct PayloadResult {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub outputs: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContextMenuCall {
    #[prost(enumeration = "SourceDto", tag = "1")]
    pub source: i32,
    #[prost(string, tag = "2")]
    pub action_id: String,
    #[prost(uint32, tag = "3")]
    pub selection_start: u32,
    #[prost(uint32, tag = "4")]
    pub selection_end: u32,
    #[prost(bytes = "vec", tag = "5")]
    pub selected_bytes: Vec<u8>,
    #[prost(message, optional, tag = "6")]
    pub request: Option<RequestDto>,
    #[prost(message, optional, tag = "7")]
    pub response: Option<ResponseDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContextMenuResult {
    #[prost(string, tag = "1")]
    pub status_message: String,
    #[prost(string, tag = "2")]
    pub log_line: String,
    #[prost(message, optional, tag = "3")]
    pub open_repeater: Option<RequestDto>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub replace_request: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PassiveCheckCall {
    #[prost(string, tag = "1")]
    pub flow_id: String,
    #[prost(string, tag = "2")]
    pub url: String,
    #[prost(message, optional, tag = "3")]
    pub request: Option<HttpRequestDto>,
    #[prost(message, optional, tag = "4")]
    pub response: Option<HttpResponseDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PluginFindingDto {
    #[prost(string, tag = "1")]
    pub title: String,
    #[prost(string, tag = "2")]
    pub detail: String,
    #[prost(string, tag = "3")]
    pub remediation: String,
    #[prost(string, tag = "4")]
    pub severity: String,
    #[prost(string, tag = "5")]
    pub confidence: String,
    #[prost(string, tag = "6")]
    pub parameter: String,
    #[prost(string, tag = "7")]
    pub evidence: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct PassiveCheckResult {
    #[prost(message, repeated, tag = "1")]
    pub findings: Vec<PluginFindingDto>,
    #[prost(string, tag = "2")]
    pub log_line: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ActiveCheckCall {
    #[prost(uint64, tag = "1")]
    pub call_id: u64,
    #[prost(string, tag = "2")]
    pub flow_id: String,
    #[prost(message, optional, tag = "3")]
    pub base_request: Option<HttpRequestDto>,
    #[prost(message, optional, tag = "4")]
    pub base_response: Option<HttpResponseDto>,
    #[prost(string, tag = "5")]
    pub parameter_name: String,
    #[prost(string, tag = "6")]
    pub parameter_value: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ActiveCheckResult {
    #[prost(uint64, tag = "1")]
    pub call_id: u64,
    #[prost(message, repeated, tag = "2")]
    pub findings: Vec<PluginFindingDto>,
}
