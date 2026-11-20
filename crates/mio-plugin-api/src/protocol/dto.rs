use prost::{Enumeration, Message};

use crate::{
    FlowRecord, FlowState, FlowSummary, Header, HttpRequest, HttpResponse, HttpVersion,
    TrafficSource,
};

use super::*;

#[derive(Clone, PartialEq, Message)]
pub struct HistoryPage {
    #[prost(uint64, tag = "1")]
    pub offset: u64,
    #[prost(uint64, tag = "2")]
    pub total: u64,
    #[prost(message, repeated, tag = "3")]
    pub rows: Vec<SummaryDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ResourceRevision {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub revision: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Cancel {
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum ErrorCode {
    Invalid = 0,
    Denied = 1,
    Stale = 2,
    Cancelled = 3,
    Deadline = 4,
    Internal = 5,
}

#[derive(Clone, PartialEq, Message)]
pub struct Failure {
    #[prost(enumeration = "ErrorCode", tag = "1")]
    pub code: i32,
    #[prost(string, tag = "2")]
    pub message: String,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code as i32,
            message: message.into(),
        }
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct HeaderDto {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(bytes = "vec", tag = "2")]
    pub value: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum HttpVersionDto {
    Http10 = 0,
    Http11 = 1,
    Http2 = 2,
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestDto {
    #[prost(string, tag = "1")]
    pub method: String,
    #[prost(string, tag = "2")]
    pub url: String,
    #[prost(enumeration = "HttpVersionDto", tag = "3")]
    pub version: i32,
    #[prost(message, repeated, tag = "4")]
    pub headers: Vec<HeaderDto>,
    #[prost(bytes = "vec", tag = "5")]
    pub body: Vec<u8>,
    #[prost(message, repeated, tag = "6")]
    pub trailers: Vec<HeaderDto>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ResponseDto {
    #[prost(uint32, tag = "1")]
    pub status: u32,
    #[prost(enumeration = "HttpVersionDto", tag = "2")]
    pub version: i32,
    #[prost(message, repeated, tag = "3")]
    pub headers: Vec<HeaderDto>,
    #[prost(bytes = "vec", tag = "4")]
    pub body: Vec<u8>,
    #[prost(message, repeated, tag = "5")]
    pub trailers: Vec<HeaderDto>,
}

pub type HttpRequestDto = RequestDto;
pub type HttpResponseDto = ResponseDto;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum SourceDto {
    Proxy = 0,
    Repeater = 1,
    Intruder = 2,
    Plugin = 3,
    Scanner = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum StateDto {
    Pending = 0,
    Intercepted = 1,
    Complete = 2,
    Dropped = 3,
    Cancelled = 4,
    Failed = 5,
}

#[derive(Clone, PartialEq, Message)]
pub struct SummaryDto {
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub revision: u64,
    #[prost(enumeration = "SourceDto", tag = "3")]
    pub source: i32,
    #[prost(string, tag = "4")]
    pub plugin_id: String,
    #[prost(string, tag = "5")]
    pub method: String,
    #[prost(string, tag = "6")]
    pub url: String,
    #[prost(uint32, optional, tag = "7")]
    pub status: Option<u32>,
    #[prost(enumeration = "StateDto", tag = "8")]
    pub state: i32,
    #[prost(string, tag = "9")]
    pub failure: String,
    #[prost(uint64, tag = "10")]
    pub request_bytes: u64,
    #[prost(uint64, tag = "11")]
    pub response_bytes: u64,
    #[prost(sint64, tag = "12")]
    pub started_unix_ms: i64,
    #[prost(uint64, tag = "13")]
    pub elapsed_ms: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct RecordDto {
    #[prost(message, optional, tag = "1")]
    pub summary: Option<SummaryDto>,
    #[prost(message, optional, tag = "2")]
    pub request: Option<RequestDto>,
    #[prost(message, optional, tag = "3")]
    pub response: Option<ResponseDto>,
}

fn version_to_wire(version: HttpVersion) -> i32 {
    match version {
        HttpVersion::Http10 => 0,
        HttpVersion::Http11 => 1,
        HttpVersion::Http2 => 2,
    }
}

fn version_from_wire(version: i32) -> Result<HttpVersion, String> {
    match HttpVersionDto::try_from(version) {
        Ok(HttpVersionDto::Http10) => Ok(HttpVersion::Http10),
        Ok(HttpVersionDto::Http11) => Ok(HttpVersion::Http11),
        Ok(HttpVersionDto::Http2) => Ok(HttpVersion::Http2),
        Err(_) => Err("未知 HTTP version".into()),
    }
}

fn headers_to_wire(headers: Vec<Header>) -> Vec<HeaderDto> {
    headers
        .into_iter()
        .map(|h| HeaderDto {
            name: h.name,
            value: h.value,
        })
        .collect()
}

fn validate_headers(headers: &[HeaderDto]) -> Result<(), String> {
    if headers.len() > 512
        || headers
            .iter()
            .any(|h| h.name.is_empty() || h.name.len() > 256 || h.value.len() > 64 * 1024)
    {
        return Err("HTTP header 数量或长度超过插件协议限额".into());
    }
    Ok(())
}

fn headers_from_wire(headers: Vec<HeaderDto>) -> Vec<Header> {
    headers
        .into_iter()
        .map(|h| Header {
            name: h.name,
            value: h.value,
        })
        .collect()
}

impl From<HttpRequest> for RequestDto {
    fn from(r: HttpRequest) -> Self {
        Self {
            method: r.method,
            url: r.url,
            version: version_to_wire(r.version),
            headers: headers_to_wire(r.headers),
            body: r.body,
            trailers: headers_to_wire(r.trailers),
        }
    }
}

impl RequestDto {
    pub fn validate(&self) -> Result<(), String> {
        if self.method.is_empty()
            || self.method.len() > 32
            || self.url.is_empty()
            || self.url.len() > 16 * 1024
            || self.body.len() > MAX_INLINE_BODY
        {
            return Err("HTTP 请求超过插件内联限额；内容未截断".into());
        }
        version_from_wire(self.version)?;
        validate_headers(&self.headers)?;
        validate_headers(&self.trailers)
    }
}

impl TryFrom<RequestDto> for HttpRequest {
    type Error = String;
    fn try_from(r: RequestDto) -> Result<Self, Self::Error> {
        r.validate()?;
        Ok(Self {
            method: r.method,
            url: r.url,
            version: version_from_wire(r.version)?,
            headers: headers_from_wire(r.headers),
            body: r.body,
            trailers: headers_from_wire(r.trailers),
        })
    }
}

impl From<HttpResponse> for ResponseDto {
    fn from(r: HttpResponse) -> Self {
        Self {
            status: u32::from(r.status),
            version: version_to_wire(r.version),
            headers: headers_to_wire(r.headers),
            body: r.body,
            trailers: headers_to_wire(r.trailers),
        }
    }
}

impl ResponseDto {
    pub fn validate(&self) -> Result<(), String> {
        if !(100..=599).contains(&self.status) {
            return Err("HTTP 响应状态无效".into());
        }
        if self.body.len() > MAX_INLINE_BODY {
            return Err("HTTP 响应超过插件内联限额；内容未截断".into());
        }
        version_from_wire(self.version)?;
        validate_headers(&self.headers)?;
        validate_headers(&self.trailers)
    }
}

impl TryFrom<ResponseDto> for HttpResponse {
    type Error = String;
    fn try_from(r: ResponseDto) -> Result<Self, Self::Error> {
        r.validate()?;
        Ok(Self {
            status: r.status as u16,
            version: version_from_wire(r.version)?,
            reason_phrase: None,
            headers: headers_from_wire(r.headers),
            body: r.body,
            trailers: headers_from_wire(r.trailers),
        })
    }
}

impl From<FlowSummary> for SummaryDto {
    fn from(s: FlowSummary) -> Self {
        let (source, plugin_id) = match s.source {
            TrafficSource::Proxy => (0, String::new()),
            TrafficSource::Repeater => (1, String::new()),
            TrafficSource::Intruder => (2, String::new()),
            TrafficSource::Plugin(id) => (3, id),
            TrafficSource::Scanner => (4, String::new()),
        };
        let (state, failure) = match s.state {
            FlowState::Pending => (0, String::new()),
            FlowState::Intercepted => (1, String::new()),
            FlowState::Complete => (2, String::new()),
            FlowState::Dropped => (3, String::new()),
            FlowState::Cancelled => (4, String::new()),
            FlowState::Failed(error) => (5, error),
        };
        Self {
            id: s.id.as_bytes().to_vec(),
            revision: s.revision,
            source,
            plugin_id,
            method: s.method,
            url: s.url,
            status: s.status.map(u32::from),
            state,
            failure,
            request_bytes: s.request_bytes,
            response_bytes: s.response_bytes,
            started_unix_ms: s.started_unix_ms,
            elapsed_ms: s.elapsed_ms,
        }
    }
}

impl TryFrom<SummaryDto> for FlowSummary {
    type Error = String;
    fn try_from(s: SummaryDto) -> Result<Self, Self::Error> {
        let source = match SourceDto::try_from(s.source).map_err(|_| "未知流量来源")? {
            SourceDto::Proxy => TrafficSource::Proxy,
            SourceDto::Repeater => TrafficSource::Repeater,
            SourceDto::Intruder => TrafficSource::Intruder,
            SourceDto::Plugin => TrafficSource::Plugin(s.plugin_id),
            SourceDto::Scanner => TrafficSource::Scanner,
        };
        let state = match StateDto::try_from(s.state).map_err(|_| "未知流量状态")? {
            StateDto::Pending => FlowState::Pending,
            StateDto::Intercepted => FlowState::Intercepted,
            StateDto::Complete => FlowState::Complete,
            StateDto::Dropped => FlowState::Dropped,
            StateDto::Cancelled => FlowState::Cancelled,
            StateDto::Failed => FlowState::Failed(s.failure),
        };
        if s.revision == 0 || s.status.is_some_and(|s| !(100..=599).contains(&s)) {
            return Err("流量 revision 或 status 无效".into());
        }
        Ok(Self {
            id: uuid(&s.id)?,
            revision: s.revision,
            source,
            method: s.method,
            url: s.url,
            status: s.status.map(|s| s as u16),
            state,
            request_bytes: s.request_bytes,
            response_bytes: s.response_bytes,
            started_unix_ms: s.started_unix_ms,
            elapsed_ms: s.elapsed_ms,
        })
    }
}

impl From<FlowRecord> for RecordDto {
    fn from(r: FlowRecord) -> Self {
        Self {
            summary: Some(r.summary.into()),
            request: Some(r.request.into()),
            response: r.response.map(Into::into),
        }
    }
}

impl TryFrom<RecordDto> for FlowRecord {
    type Error = String;
    fn try_from(r: RecordDto) -> Result<Self, Self::Error> {
        Ok(Self {
            summary: r.summary.ok_or("流量缺少 summary")?.try_into()?,
            request: r.request.ok_or("流量缺少 request")?.try_into()?,
            response: r.response.map(TryInto::try_into).transpose()?,
        })
    }
}
