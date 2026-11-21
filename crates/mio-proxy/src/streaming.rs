use std::{sync::Arc, time::Duration};

use mio_plugin_api::Header;
use rama::http::Response;

pub type StreamingScopeMatcher = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub(crate) fn is_event_stream_mime(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
}

pub(crate) fn is_streaming_headers(headers: &[Header]) -> bool {
    let is_event_stream = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .and_then(|header| std::str::from_utf8(&header.value).ok())
        .is_some_and(is_event_stream_mime);
    if is_event_stream {
        return true;
    }
    let has_content_length = headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("content-length"));
    let is_chunked = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
        .filter_map(|header| std::str::from_utf8(&header.value).ok())
        .any(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
        });
    !has_content_length && is_chunked
}

pub(crate) fn is_streaming_response_head(response: &Response) -> bool {
    let is_event_stream = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_event_stream_mime);
    if is_event_stream {
        return true;
    }
    let has_content_length = response.headers().contains_key("content-length");
    let is_chunked = crate::message::has_header_token(
        response.headers(),
        "transfer-encoding",
        "chunked",
    );
    !has_content_length && is_chunked
}

pub(crate) fn response_body_deadline(
    is_streaming: bool,
    streaming_timeout: Option<Duration>,
    default_deadline: tokio::time::Instant,
) -> Option<tokio::time::Instant> {
    match (is_streaming, streaming_timeout) {
        (true, Some(limit)) if limit.is_zero() => None,
        (true, Some(limit)) => Some(tokio::time::Instant::now() + limit),
        _ => Some(default_deadline),
    }
}
