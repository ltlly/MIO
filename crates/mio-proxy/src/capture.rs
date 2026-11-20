use mio_plugin_api::{FlowRecord, Header, HttpResponse, HttpVersion};

use crate::{ProxyError, message};

/// Capture provenance, not an editable HTTP message. It never owns another body buffer.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamedResponse {
    pub original_status: u16,
    pub original_version: HttpVersion,
    pub original_headers: Vec<Header>,
    /// True only after observing the source body EOS, never merely after stopping a task.
    pub body_complete: bool,
}

impl StreamedResponse {
    pub const MAX_HEAD_BYTES: usize = 128 * 1024;
    pub const MAX_HEADERS: usize = 1024;

    pub(crate) fn new(response: &HttpResponse) -> Result<Self, ProxyError> {
        Self::check_headers(&response.headers)?;
        Ok(Self {
            original_status: response.status,
            original_version: response.version,
            original_headers: response.headers.clone(),
            body_complete: false,
        })
    }

    pub fn validate(&self) -> Result<(), ProxyError> {
        if !(200..=999).contains(&self.original_status) {
            return Err(ProxyError::invalid("流式原始响应必须是最终响应头"));
        }
        Self::check_headers(&self.original_headers)?;
        message::header_map(&self.original_headers)?;
        Ok(())
    }

    fn check_headers(headers: &[Header]) -> Result<(), ProxyError> {
        let bytes = headers.iter().fold(0usize, |total, header| {
            total
                .saturating_add(header.name.len())
                .saturating_add(header.value.len())
                .saturating_add(4)
        });
        if headers.len() > Self::MAX_HEADERS || bytes > Self::MAX_HEAD_BYTES {
            return Err(ProxyError::invalid(
                "流式原始响应头超过 128 KiB / 1024 个字段的保存预算",
            ));
        }
        Ok(())
    }

    pub fn head_was_edited(&self, response: &HttpResponse) -> bool {
        self.original_status != response.status
            || self.original_version != response.version
            || self.original_headers != response.headers
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedFlowMetrics {
    pub remote_ip: Option<std::net::IpAddr>,
    pub response_received_ms: Option<u64>,
    pub reason_phrase: Option<Vec<u8>>,
}

impl Default for CapturedFlowMetrics {
    fn default() -> Self {
        Self { remote_ip: None, response_received_ms: None, reason_phrase: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedFlow {
    pub record: FlowRecord,
    pub response_stream: Option<StreamedResponse>,
    pub listener_port: Option<u16>,
    pub capture_metrics: CapturedFlowMetrics,
}

impl From<FlowRecord> for CapturedFlow {
    fn from(record: FlowRecord) -> Self {
        Self {
            record,
            response_stream: None,
            listener_port: None,
            capture_metrics: CapturedFlowMetrics::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_head_budget_is_checked_before_copying_headers() {
        let response = HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "x-large".into(),
                value: vec![b'x'; StreamedResponse::MAX_HEAD_BYTES],
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        };
        assert!(StreamedResponse::new(&response).is_err());
        let mut response = response;
        response.headers = vec![
            Header {
                name: "x".into(),
                value: Vec::new()
            };
            1025
        ];
        assert!(StreamedResponse::new(&response).is_err());
    }
}
