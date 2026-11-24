use std::{
    borrow::Cow,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

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
    #[prost(bool, tag = "7")]
    pub body_omitted: bool,
    #[prost(uint64, tag = "8")]
    pub original_body_len: u64,
    #[prost(string, tag = "9")]
    pub body_file_path: String,
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
    #[prost(bool, tag = "6")]
    pub body_omitted: bool,
    #[prost(uint64, tag = "7")]
    pub original_body_len: u64,
    #[prost(string, tag = "8")]
    pub body_file_path: String,
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

static SPILL_SEQ: AtomicU64 = AtomicU64::new(1);

pub(super) fn validate_omitted_fields(
    minor: u32,
    inline_len: usize,
    omitted: bool,
    original_len: u64,
    file_path: &str,
) -> Result<(), String> {
    if minor < PROTOCOL_MINOR {
        if omitted || original_len != 0 || !file_path.is_empty() {
            return Err("当前兼容协议版本不支持大报文省略/共享文件字段，请升级插件至协议 v1.7".into());
        }
        return Ok(());
    }
    if !omitted {
        if original_len != 0 || !file_path.is_empty() {
            return Err("未设置 body_omitted 时不能携带大报文元信息或共享文件路径".into());
        }
        return Ok(());
    }
    if inline_len > 0 {
        return Err("设置 body_omitted 时内联正文必须为空".into());
    }
    if file_path.len() > MAX_BODY_SPILL_PATH_BYTES || file_path.contains('\0') {
        return Err("共享大报文文件路径无效或超限".into());
    }
    Ok(())
}

fn validate_spill_path(path: &str) -> io::Result<()> {
    if path.is_empty() || path.contains('\0') || path.len() > MAX_BODY_SPILL_PATH_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "共享大报文文件路径无效或超限",
        ));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "共享大报文路径不是有效的常规文件",
        ));
    }
    if meta.len() > MAX_SPILL_BODY_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "共享大报文文件超过 64 MiB 限额",
        ));
    }
    Ok(())
}

pub(super) fn read_spill_file_bounded(path: &str) -> io::Result<Vec<u8>> {
    validate_spill_path(path)?;
    let meta = fs::symlink_metadata(path)?;
    let mut buf = Vec::with_capacity((meta.len() as usize).min(MAX_SPILL_BODY_BYTES));
    fs::File::open(path)?
        .take(MAX_SPILL_BODY_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    if buf.len() > MAX_SPILL_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "共享大报文文件超过 64 MiB 限额",
        ));
    }
    Ok(buf)
}

pub(super) fn open_spill_reader(path: &str) -> io::Result<Box<dyn Read>> {
    validate_spill_path(path)?;
    Ok(Box::new(
        fs::File::open(path)?.take(MAX_SPILL_BODY_BYTES as u64),
    ))
}

pub(super) fn write_spill_sibling(
    existing_path: &str,
    prefix: &str,
    bytes: &[u8],
) -> io::Result<String> {
    if bytes.len() > MAX_SPILL_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "改写后的大报文正文超过 64 MiB 限额",
        ));
    }
    if existing_path.is_empty() || existing_path.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "缺少宿主共享大报文目录路径",
        ));
    }
    let parent = match Path::new(existing_path).parent() {
        Some(p) if p.as_os_str().is_empty() => Path::new("."),
        Some(p) => p,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "无法解析宿主共享大报文目录",
            ));
        }
    };
    let seq = SPILL_SEQ.fetch_add(1, Ordering::Relaxed);
    let candidate = parent.join(format!("{prefix}-{}-{seq}.bin", std::process::id()));
    let candidate_str = candidate
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "共享路径包含非 UTF-8 字符"))?;
    if candidate_str.len() > MAX_BODY_SPILL_PATH_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "共享大报文文件路径超限",
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(candidate_str.to_owned())
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
            body_omitted: false,
            original_body_len: 0,
            body_file_path: String::new(),
        }
    }
}

impl RequestDto {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_for_minor(PROTOCOL_MINOR)
    }

    pub fn validate_for_minor(&self, minor: u32) -> Result<(), String> {
        if self.method.is_empty()
            || self.method.len() > 32
            || self.url.is_empty()
            || self.url.len() > 16 * 1024
            || self.body.len() > MAX_INLINE_BODY
        {
            return Err("HTTP 请求超过插件内联限额；内容未截断".into());
        }
        validate_omitted_fields(
            minor,
            self.body.len(),
            self.body_omitted,
            self.original_body_len,
            &self.body_file_path,
        )?;
        version_from_wire(self.version)?;
        validate_headers(&self.headers)?;
        validate_headers(&self.trailers)
    }

    pub fn read_body_bytes(&self) -> io::Result<Cow<'_, [u8]>> {
        if self.body_omitted {
            read_spill_file_bounded(&self.body_file_path).map(Cow::Owned)
        } else {
            Ok(Cow::Borrowed(&self.body))
        }
    }

    pub fn open_body_reader(&self) -> io::Result<Box<dyn Read + '_>> {
        if self.body_omitted {
            open_spill_reader(&self.body_file_path)
        } else {
            Ok(Box::new(&self.body[..]))
        }
    }

    pub fn set_body_inline(&mut self, body: Vec<u8>) {
        self.body = body;
        self.body_omitted = false;
        self.original_body_len = 0;
        self.body_file_path.clear();
    }

    pub fn set_body_or_spill(&mut self, body: Vec<u8>) -> io::Result<()> {
        if body.len() <= MAX_INLINE_BODY {
            self.set_body_inline(body);
            return Ok(());
        }
        let len = body.len() as u64;
        let path = write_spill_sibling(&self.body_file_path, "modified-body", &body)?;
        self.body.clear();
        self.body_omitted = true;
        self.original_body_len = len;
        self.body_file_path = path;
        Ok(())
    }

    pub fn try_into_with_resolved_body(
        self,
        resolved_body: Option<Vec<u8>>,
    ) -> Result<HttpRequest, String> {
        self.validate()?;
        let body = if self.body_omitted {
            let resolved = resolved_body
                .ok_or_else(|| "报文正文已省略（body_omitted），缺少完整大报文正文".to_owned())?;
            if resolved.len() > MAX_SPILL_BODY_BYTES {
                return Err("解析后的大报文正文超过 64 MiB 限额".into());
            }
            resolved
        } else {
            self.body
        };
        Ok(HttpRequest {
            method: self.method,
            url: self.url,
            version: version_from_wire(self.version)?,
            headers: headers_from_wire(self.headers),
            body,
            trailers: headers_from_wire(self.trailers),
        })
    }
}

impl TryFrom<RequestDto> for HttpRequest {
    type Error = String;
    fn try_from(r: RequestDto) -> Result<Self, Self::Error> {
        if r.body_omitted {
            return Err("报文正文已省略（body_omitted），不能直接转换为完整 HTTP 报文".into());
        }
        r.try_into_with_resolved_body(None)
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
            body_omitted: false,
            original_body_len: 0,
            body_file_path: String::new(),
        }
    }
}

impl ResponseDto {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_for_minor(PROTOCOL_MINOR)
    }

    pub fn validate_for_minor(&self, minor: u32) -> Result<(), String> {
        if !(100..=599).contains(&self.status) {
            return Err("HTTP 响应状态无效".into());
        }
        if self.body.len() > MAX_INLINE_BODY {
            return Err("HTTP 响应超过插件内联限额；内容未截断".into());
        }
        validate_omitted_fields(
            minor,
            self.body.len(),
            self.body_omitted,
            self.original_body_len,
            &self.body_file_path,
        )?;
        version_from_wire(self.version)?;
        validate_headers(&self.headers)?;
        validate_headers(&self.trailers)
    }

    pub fn read_body_bytes(&self) -> io::Result<Cow<'_, [u8]>> {
        if self.body_omitted {
            read_spill_file_bounded(&self.body_file_path).map(Cow::Owned)
        } else {
            Ok(Cow::Borrowed(&self.body))
        }
    }

    pub fn open_body_reader(&self) -> io::Result<Box<dyn Read + '_>> {
        if self.body_omitted {
            open_spill_reader(&self.body_file_path)
        } else {
            Ok(Box::new(&self.body[..]))
        }
    }

    pub fn set_body_inline(&mut self, body: Vec<u8>) {
        self.body = body;
        self.body_omitted = false;
        self.original_body_len = 0;
        self.body_file_path.clear();
    }

    pub fn set_body_or_spill(&mut self, body: Vec<u8>) -> io::Result<()> {
        if body.len() <= MAX_INLINE_BODY {
            self.set_body_inline(body);
            return Ok(());
        }
        let len = body.len() as u64;
        let path = write_spill_sibling(&self.body_file_path, "modified-body", &body)?;
        self.body.clear();
        self.body_omitted = true;
        self.original_body_len = len;
        self.body_file_path = path;
        Ok(())
    }

    pub fn try_into_with_resolved_body(
        self,
        resolved_body: Option<Vec<u8>>,
    ) -> Result<HttpResponse, String> {
        self.validate()?;
        let body = if self.body_omitted {
            let resolved = resolved_body
                .ok_or_else(|| "报文正文已省略（body_omitted），缺少完整大报文正文".to_owned())?;
            if resolved.len() > MAX_SPILL_BODY_BYTES {
                return Err("解析后的大报文正文超过 64 MiB 限额".into());
            }
            resolved
        } else {
            self.body
        };
        Ok(HttpResponse {
            status: self.status as u16,
            version: version_from_wire(self.version)?,
            reason_phrase: None,
            headers: headers_from_wire(self.headers),
            body,
            trailers: headers_from_wire(self.trailers),
        })
    }
}

impl TryFrom<ResponseDto> for HttpResponse {
    type Error = String;
    fn try_from(r: ResponseDto) -> Result<Self, Self::Error> {
        if r.body_omitted {
            return Err("报文正文已省略（body_omitted），不能直接转换为完整 HTTP 报文".into());
        }
        r.try_into_with_resolved_body(None)
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
