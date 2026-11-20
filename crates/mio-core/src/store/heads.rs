use super::*;

#[derive(serde::Serialize)]
pub(super) struct RequestHead<'a> {
    pub(super) method: &'a str,
    pub(super) url: &'a str,
    pub(super) version: crate::HttpVersion,
    pub(super) headers: &'a [crate::Header],
    pub(super) trailers: &'a [crate::Header],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) has_params: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) params_known: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) params_error: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) remote_ip: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) time_known: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) capture_number: Option<u64>,
}

impl<'a> From<&'a HttpRequest> for RequestHead<'a> {
    fn from(value: &'a HttpRequest) -> Self {
        Self {
            method: &value.method,
            url: &value.url,
            version: value.version,
            headers: &value.headers,
            trailers: &value.trailers,
            has_params: None,
            params_known: None,
            params_error: None,
            remote_ip: None,
            time_known: None,
            capture_number: None,
        }
    }
}

#[derive(serde::Deserialize)]
pub(super) struct OwnedRequestHead {
    pub(super) method: String,
    pub(super) url: String,
    pub(super) version: crate::HttpVersion,
    pub(super) headers: Vec<crate::Header>,
    pub(super) trailers: Vec<crate::Header>,
    #[serde(default)]
    pub(crate) has_params: Option<bool>,
    #[serde(default)]
    pub(crate) params_known: Option<bool>,
    #[serde(default)]
    pub(crate) params_error: Option<String>,
    #[serde(default)]
    pub(crate) remote_ip: Option<String>,
    #[serde(default)]
    pub(crate) time_known: Option<bool>,
    #[serde(default)]
    pub(crate) capture_number: Option<u64>,
}

#[derive(serde::Serialize)]
pub(super) struct ResponseHead<'a> {
    pub(super) status: u16,
    pub(super) version: crate::HttpVersion,
    pub(super) headers: &'a [crate::Header],
    pub(super) trailers: &'a [crate::Header],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream: Option<&'a StreamedResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason_phrase: Option<&'a [u8]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) body_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mime_type: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title_error: Option<&'a str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) response_received_ms: Option<u64>,
}

impl<'a> From<&'a HttpResponse> for ResponseHead<'a> {
    fn from(value: &'a HttpResponse) -> Self {
        Self {
            status: value.status,
            version: value.version,
            headers: &value.headers,
            trailers: &value.trailers,
            stream: None,
            reason_phrase: value.reason_phrase.as_deref(),
            body_bytes: None,
            mime_type: None,
            title: None,
            title_error: None,
            response_received_ms: None,
        }
    }
}

#[derive(serde::Deserialize)]
pub(super) struct OwnedResponseHead {
    pub(super) status: u16,
    pub(super) version: crate::HttpVersion,
    pub(super) headers: Vec<crate::Header>,
    pub(super) trailers: Vec<crate::Header>,
    #[serde(default)]
    pub(super) stream: Option<StreamedResponse>,
    #[serde(default)]
    pub(crate) reason_phrase: Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) body_bytes: Option<u64>,
    #[serde(default)]
    pub(crate) mime_type: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) title_error: Option<String>,
    #[serde(default)]
    pub(crate) response_received_ms: Option<u64>,
}
