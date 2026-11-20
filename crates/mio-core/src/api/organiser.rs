use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    Draft, FlowRecord, HttpResponse, HttpVersion, Uuid,
    organiser::{
        extract_origin, normalize_source_tool, now_unix_ms, parse_request_start_line,
        parse_response_status_code, resolve_display_url, validate_organiser_item,
    },
    request_to_raw, response_to_raw,
};

pub const MAX_ORGANISER_ITEMS: usize = 512;
pub const MAX_ORGANISER_COLLECTIONS: usize = 128;
pub const MAX_ORGANISER_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ORGANISER_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ORGANISER_NOTES_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrganiserCollection {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub item_ids: Vec<Uuid>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganiserStatus {
    #[default]
    Unknown,
    New,
    InProgress,
    Postponed,
    Done,
    Ignored,
}

impl OrganiserStatus {
    pub const ALL: [Self; 6] = [
        Self::Unknown,
        Self::New,
        Self::InProgress,
        Self::Postponed,
        Self::Done,
        Self::Ignored,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未处理",
            Self::New => "新建",
            Self::InProgress => "处理中",
            Self::Postponed => "延后",
            Self::Done => "已完成",
            Self::Ignored => "已忽略",
        }
    }

    pub const fn sort_weight(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::New => 1,
            Self::InProgress => 2,
            Self::Postponed => 3,
            Self::Done => 4,
            Self::Ignored => 5,
        }
    }

    pub const fn is_terminal_cleanup_target(self) -> bool {
        matches!(self, Self::Done | Self::Ignored)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganiserItem {
    pub id: Uuid,
    pub number: u64,
    pub added_unix_ms: i64,
    pub source_tool: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub response_len: u64,
    pub request_raw: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_raw: Option<Vec<u8>>,
    #[serde(default)]
    pub http_version: HttpVersion,
    #[serde(default)]
    pub workflow_status: OrganiserStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<u8>,
}

impl OrganiserItem {
    pub fn from_flow_record(number: u64, source_tool: &str, record: &FlowRecord) -> Result<Self> {
        ensure!(number > 0, "Organiser 条目序号必须大于零");
        let source_tool = normalize_source_tool(source_tool)?;
        let request_raw = request_to_raw(&record.request);
        ensure!(
            !request_raw.is_empty() && request_raw.len() <= MAX_ORGANISER_MESSAGE_BYTES,
            "Organiser 请求报文不能为空且不能超过 4 MiB"
        );
        let response_raw = record.response.as_ref().map(response_to_raw);
        if let Some(resp) = &response_raw {
            ensure!(
                resp.len() <= MAX_ORGANISER_MESSAGE_BYTES,
                "Organiser 响应报文超过 4 MiB 上限"
            );
        }
        let target = extract_origin(&record.request.url);
        let url = if !record.request.url.is_empty() {
            record.request.url.clone()
        } else {
            record.summary.url.clone()
        };
        let method = if !record.request.method.is_empty() {
            record.request.method.clone()
        } else {
            record.summary.method.clone()
        };
        let status_code = record
            .response
            .as_ref()
            .map(|resp| resp.status)
            .or(record.summary.status);
        let response_len = response_raw
            .as_ref()
            .map_or(record.summary.response_bytes, |raw| raw.len() as u64);
        let added_unix_ms = if record.summary.started_unix_ms > 0 {
            record.summary.started_unix_ms
        } else {
            now_unix_ms()
        };
        let item = Self {
            id: Uuid::new_v4(),
            number,
            added_unix_ms,
            source_tool,
            target,
            method,
            url,
            status_code,
            response_len,
            request_raw,
            response_raw,
            http_version: record.request.version,
            workflow_status: OrganiserStatus::Unknown,
            notes: String::new(),
            highlight: None,
        };
        item.validate()?;
        Ok(item)
    }

    pub fn from_draft(
        number: u64,
        source_tool: &str,
        draft: &Draft,
        response: Option<&HttpResponse>,
    ) -> Result<Self> {
        let response_raw = response.map(response_to_raw);
        Self::from_raw_parts(
            number,
            source_tool,
            &draft.target,
            draft.raw.clone(),
            response_raw,
            response.map(|resp| resp.status),
            draft.version,
        )
    }

    pub fn from_raw_parts(
        number: u64,
        source_tool: &str,
        target_or_url: &str,
        request_raw: Vec<u8>,
        response_raw: Option<Vec<u8>>,
        status_override: Option<u16>,
        http_version: HttpVersion,
    ) -> Result<Self> {
        ensure!(number > 0, "Organiser 条目序号必须大于零");
        let source_tool = normalize_source_tool(source_tool)?;
        ensure!(
            !request_raw.is_empty() && request_raw.len() <= MAX_ORGANISER_MESSAGE_BYTES,
            "Organiser 请求报文不能为空且不能超过 4 MiB"
        );
        if let Some(resp) = &response_raw {
            ensure!(
                resp.len() <= MAX_ORGANISER_MESSAGE_BYTES,
                "Organiser 响应报文超过 4 MiB 上限"
            );
        }
        let (method, request_target) = parse_request_start_line(&request_raw)?;
        let target = extract_origin(target_or_url.trim());
        let url = resolve_display_url(&target, target_or_url.trim(), &request_target, &request_raw);
        let status_code = status_override
            .or_else(|| response_raw.as_deref().and_then(parse_response_status_code));
        let response_len = response_raw.as_ref().map_or(0, |raw| raw.len() as u64);
        let item = Self {
            id: Uuid::new_v4(),
            number,
            added_unix_ms: now_unix_ms(),
            source_tool,
            target,
            method,
            url,
            status_code,
            response_len,
            request_raw,
            response_raw,
            http_version,
            workflow_status: OrganiserStatus::Unknown,
            notes: String::new(),
            highlight: None,
        };
        item.validate()?;
        Ok(item)
    }

    pub fn payload_bytes(&self) -> Result<usize> {
        self.request_raw
            .len()
            .checked_add(self.response_raw.as_ref().map_or(0, Vec::len))
            .and_then(|sum| sum.checked_add(self.notes.len()))
            .context("Organiser 条目字节大小溢出")
    }

    pub fn notes_preview(&self) -> String {
        self.notes
            .replace("\r\n", " ")
            .replace(['\r', '\n', '\t'], " ")
            .chars()
            .take(160)
            .collect()
    }

    pub fn host(&self) -> &str {
        if let Some((_, tail)) = self.url.split_once("://") {
            return tail.split(['/', '?', '#']).next().unwrap_or_default();
        }
        if let Some((_, tail)) = self.target.split_once("://") {
            return tail.split(['/', '?', '#']).next().unwrap_or_default();
        }
        ""
    }

    pub fn validate(&self) -> Result<()> {
        validate_organiser_item(self)
    }
}

#[cfg(test)]
#[path = "organiser_tests.rs"]
mod tests;
