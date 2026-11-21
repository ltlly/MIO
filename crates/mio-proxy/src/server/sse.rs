use super::{ActiveFlow, ProxyEvent};
use crate::{ProxyError, StreamedResponse, message};
use mio_plugin_api::{HttpRequest, HttpResponse, HttpVersion};
use rama::http::{
    Body, HeaderValue, Response, StatusCode,
    body::{CaptureHandle, CaptureLimit, CaptureOutcome, Frame, SizeHint, StreamingBody},
};
use std::{
    fmt::Write as _,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
};
use tokio_util::sync::{CancellationToken, DropGuard};

pub(super) fn detected(response: &Response, request: &HttpRequest) -> bool {
    !request.method.eq_ignore_ascii_case("HEAD")
        && has_body(response.status().as_u16())
        && response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(crate::streaming::is_event_stream_mime)
}

pub(super) fn detected_or_in_scope(
    response: &Response,
    request: &HttpRequest,
    treat_event_stream: bool,
    scope_matcher: Option<&crate::StreamingScopeMatcher>,
) -> bool {
    if treat_event_stream && detected(response, request) {
        return true;
    }
    !request.method.eq_ignore_ascii_case("HEAD")
        && has_body(response.status().as_u16())
        && scope_matcher.is_some_and(|matcher| matcher(&request.url))
}

fn has_body(status: u16) -> bool {
    !(100..200).contains(&status) && !matches!(status, 204 | 304)
}

pub(super) async fn handle(
    upstream: Response,
    mut active: ActiveFlow,
    ingress_version: HttpVersion,
    cancel: CancellationToken,
    cancel_on_drop: DropGuard,
) -> Response {
    let state = active.state.clone();
    let response_client_version = if state.config.http.use_http10_in_responses
        && ingress_version != HttpVersion::Http2
    {
        HttpVersion::Http10
    } else {
        ingress_version
    };
    let result = async {
        let was_chunked =
            message::has_header_token(upstream.headers(), "transfer-encoding", "chunked");
        let reason_phrase = message::reason_phrase(&upstream);
        let mut response_headers = message::headers(upstream.headers());
        if state.config.http.strip_chunked_metadata_in_streaming {
            message::strip_header_token_vec(&mut response_headers, "transfer-encoding", "chunked");
        }
        let record = active.record.as_mut().expect("active flow owns its record");
        record.summary.status = Some(upstream.status().as_u16());
        record.summary.revision += 1;
        record.response = Some(HttpResponse {
            status: upstream.status().as_u16(),
            version: message::model_version(upstream.version()),
            reason_phrase,
            headers: response_headers,
            body: Vec::new(),
            trailers: Vec::new(),
        });
        active.response_stream = Some(StreamedResponse::new(
            record.response.as_ref().expect("SSE response is present"),
        )?);
        state
            .emit(ProxyEvent::Captured {
                project: state.project,
                record: record.clone(),
                response_stream: active.response_stream.clone(),
                listener_port: (state.listener_port != 0).then_some(state.listener_port),
                capture_metrics: active.capture_metrics.clone(),
            })
            .await?;
        state
            .intercept(
                record,
                true,
                active.response_stream.as_ref(),
                &active.capture_metrics,
                active.request_was_intercepted,
                active.request_was_modified,
            )
            .await?;
        let model = record
            .response
            .as_ref()
            .expect("upstream response is present");
        if !has_body(model.status) {
            let mut outgoing = message::to_response(
                model,
                state.config.limits.max_body_bytes,
                response_client_version,
                false,
            )?;
            if state.config.http.set_response_connection_close
                && response_client_version != HttpVersion::Http2
            {
                outgoing
                    .headers_mut()
                    .insert("connection", HeaderValue::from_static("close"));
            }
            return Ok((outgoing, None));
        }
        let mut headers = message::header_map(&model.headers)?;
        message::normalize_headers(&mut headers, 0, true, false);
        if state.config.http.set_response_connection_close
            && response_client_version != HttpVersion::Http2
        {
            headers.insert("connection", HeaderValue::from_static("close"));
        }
        let preserve_chunked = !state.config.http.strip_chunked_metadata_in_streaming && was_chunked;
        let (body, capture) = capture_with_options(
            upstream.into_body(),
            state.config.limits.max_body_bytes,
            preserve_chunked,
        );
        let cancel_stream = cancel.clone();
        let mut response = Response::new(body.on_drop(move || cancel_stream.cancel()));
        *response.status_mut() = StatusCode::from_u16(model.status).map_err(ProxyError::invalid)?;
        *response.version_mut() = message::version(response_client_version);
        *response.headers_mut() = headers;
        Ok((response, Some(capture)))
    }
    .await;

    match result {
        Err(error) => {
            cancel.cancel();
            active.finish(Err(&error)).await;
            state.error_response(&error)
        }
        Ok((response, None)) => {
            cancel.cancel();
            active.finish(Ok(())).await;
            response
        }
        Ok((response, Some(capture))) => {
            let streaming_timeout = state.config.limits.streaming_timeout;
            state.tasks.spawn(async move {
                let _cancel_on_drop = cancel_on_drop;
                let result = match streaming_timeout {
                    Some(limit) if !limit.is_zero() => {
                        match tokio::time::timeout(limit, capture.finish(&mut active)).await {
                            Ok(res) => res,
                            Err(_) => Err(ProxyError::Timeout),
                        }
                    }
                    _ => capture.finish(&mut active).await,
                };
                cancel.cancel();
                active.finish(result.as_ref().map(|_| ())).await;
            });
            response
        }
    }
}

struct Capture {
    handle: CaptureHandle,
    failure: Arc<Mutex<Option<String>>>,
    chunked_body: Option<Arc<Mutex<Vec<u8>>>>,
    limit: usize,
}

#[cfg(test)]
fn capture(body: Body, limit: usize) -> (Body, Capture) {
    capture_with_options(body, limit, false)
}

fn capture_with_options(
    body: Body,
    limit: usize,
    preserve_chunked_framing: bool,
) -> (Body, Capture) {
    let failure = Arc::new(Mutex::new(None));
    let chunked_body = preserve_chunked_framing.then(|| Arc::new(Mutex::new(Vec::new())));
    // Rama bounds captured DATA only; trailers need their own cumulative budget.
    let trailer_limit = limit.min(64 * 1024);
    let body = Body::new(ObservedBody {
        body,
        failure: failure.clone(),
        chunked_body: chunked_body.clone(),
        chunked_limit: limit,
        trailer_limit,
        trailer_remaining: trailer_limit,
        ended: false,
    });
    let (body, handle) = body.capture_buffered(CaptureLimit::max_bytes(limit));
    (
        body.limited(limit),
        Capture {
            handle,
            failure,
            chunked_body,
            limit,
        },
    )
}

impl Capture {
    async fn finish(self, active: &mut ActiveFlow) -> Result<(), ProxyError> {
        let captured = self.handle.wait().await.map_err(ProxyError::network)?;
        let record = active.record.as_mut().expect("active flow owns its record");
        let response = record.response.as_mut().expect("SSE response is present");
        response.body = if let Some(chunked) = self.chunked_body {
            std::mem::take(&mut *crate::lock_unpoison(&chunked))
        } else {
            captured.bytes().to_vec()
        };
        response.trailers = captured
            .trailers()
            .map(message::headers)
            .unwrap_or_default();
        record.summary.response_bytes = captured.total_bytes();
        if captured.is_truncated() {
            return Err(ProxyError::Network(format!(
                "SSE 正文超过配置的 {} 字节上限；已观察 {} 字节并保留前 {} 字节，流式转发已中止",
                self.limit,
                captured.total_bytes(),
                response.body.len(),
            )));
        }
        match captured.outcome() {
            CaptureOutcome::Complete => {
                active
                    .response_stream
                    .as_mut()
                    .expect("SSE capture metadata")
                    .body_complete = true;
                Ok(())
            }
            // HTTP trailers terminate the message; H2 senders need not poll another frame.
            CaptureOutcome::Aborted if captured.trailers().is_some() => {
                active
                    .response_stream
                    .as_mut()
                    .expect("SSE capture metadata")
                    .body_complete = true;
                Ok(())
            }
            CaptureOutcome::Aborted => Err(ProxyError::Cancelled),
            CaptureOutcome::Error if active.state.cancel.is_cancelled() => {
                Err(ProxyError::Cancelled)
            }
            CaptureOutcome::Error => Err(ProxyError::Network(
                crate::lock_unpoison(&self.failure)
                    .take()
                    .unwrap_or_else(|| "SSE 源站正文读取失败".into()),
            )),
        }
    }
}

struct ObservedBody {
    body: Body,
    failure: Arc<Mutex<Option<String>>>,
    chunked_body: Option<Arc<Mutex<Vec<u8>>>>,
    chunked_limit: usize,
    trailer_limit: usize,
    trailer_remaining: usize,
    ended: bool,
}

fn push_capped(buf: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(buf.len());
    if remaining > 0 {
        buf.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
    }
}

impl StreamingBody for ObservedBody {
    type Data = <Body as StreamingBody>::Data;
    type Error = <Body as StreamingBody>::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        let Some(frame) = ready!(Pin::new(&mut this.body).poll_frame(cx)) else {
            this.ended = true;
            if let Some(chunked) = &this.chunked_body {
                let mut guard = crate::lock_unpoison(chunked);
                push_capped(&mut guard, b"0\r\n\r\n", this.chunked_limit);
            }
            return Poll::Ready(None);
        };
        let frame = frame.and_then(|frame| {
            if let Some(data) = frame.data_ref()
                && !data.is_empty()
                && let Some(chunked) = &this.chunked_body
            {
                let mut prefix = String::new();
                let _ = write!(prefix, "{:x}\r\n", data.len());
                let mut guard = crate::lock_unpoison(chunked);
                push_capped(&mut guard, prefix.as_bytes(), this.chunked_limit);
                push_capped(&mut guard, data, this.chunked_limit);
                push_capped(&mut guard, b"\r\n", this.chunked_limit);
            }
            if let Some(trailers) = frame.trailers_ref() {
                for (name, value) in trailers.ordered_iter() {
                    let bytes = name
                        .as_str()
                        .len()
                        .saturating_add(value.as_bytes().len())
                        .saturating_add(4);
                    if bytes > this.trailer_remaining {
                        return Err(ProxyError::Network(format!(
                            "SSE trailers 超过 {} 字节安全预算（含字段名与分隔符）",
                            this.trailer_limit,
                        ))
                        .into());
                    }
                    this.trailer_remaining -= bytes;
                }
            }
            Ok(frame)
        });
        if let Err(error) = &frame {
            this.ended = true;
            *crate::lock_unpoison(&this.failure) = Some(error.to_string());
        }
        Poll::Ready(Some(frame))
    }

    fn is_end_stream(&self) -> bool {
        self.ended
    }

    fn size_hint(&self) -> SizeHint {
        // Poll the real EOS even for finite Content-Length, then retain trailers and Complete.
        SizeHint::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio_plugin_api::HttpVersion;
    use rama::http::{HeaderMap, body::util::BodyExt};

    #[test]
    fn only_original_sse_mime_with_a_body_selects_streaming() {
        let mut request = HttpRequest {
            method: "GET".into(),
            url: "http://localhost/events".into(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let mut response = Response::new(Body::empty());
        response.headers_mut().insert(
            "content-type",
            " Text/Event-Stream; charset=utf-8".parse().unwrap(),
        );
        assert!(detected(&response, &request));
        *response.status_mut() = StatusCode::NO_CONTENT;
        assert!(!detected(&response, &request));
        *response.status_mut() = StatusCode::OK;
        request.method = "HEAD".into();
        assert!(!detected(&response, &request));
        request.method = "GET".into();
        response
            .headers_mut()
            .insert("content-type", "text/event-stream-extra".parse().unwrap());
        assert!(!detected(&response, &request));
    }

    #[tokio::test]
    async fn overlimit_capture_keeps_prefix_but_stream_returns_an_error() {
        let (mut body, capture) = capture(Body::from(b"123456".to_vec()), 4);
        assert!(body.frame().await.unwrap().is_err());
        drop(body);
        let captured = capture.handle.wait().await.unwrap();
        assert_eq!(captured.bytes().as_ref(), b"1234");
        assert_eq!(captured.total_bytes(), 6);
        assert!(captured.is_truncated());
    }

    #[tokio::test]
    async fn exact_budget_keeps_a_complete_body_instead_of_failing_at_the_boundary() {
        let (mut body, capture) = capture(Body::from(b"1234".to_vec()), 4);
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            b"1234".as_slice()
        );
        assert!(body.frame().await.is_none());
        let captured = capture.handle.wait().await.unwrap();
        assert_eq!(captured.outcome(), CaptureOutcome::Complete);
        assert_eq!(captured.total_bytes(), 4);
        assert!(!captured.is_truncated());
    }

    #[tokio::test]
    async fn capture_reads_only_when_the_downstream_polls() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let polls = Arc::new(AtomicUsize::new(0));
        let observed = polls.clone();
        let source = Body::from_stream(rama::futures::stream::poll_fn(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
            Poll::Ready(Some(Ok::<_, std::io::Error>(b"x".to_vec())))
        }));
        let (mut body, capture) = capture(source, 4);
        assert_eq!(polls.load(Ordering::Relaxed), 0);
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            b"x".as_slice()
        );
        assert_eq!(polls.load(Ordering::Relaxed), 1);
        drop(body);
        let captured = capture.handle.wait().await.unwrap();
        assert_eq!(captured.outcome(), CaptureOutcome::Aborted);
        assert_eq!(captured.bytes().as_ref(), b"x");
        assert_eq!(polls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn trailer_budget_rejects_before_the_capture_sink_can_accumulate_it() {
        let mut trailers = HeaderMap::new();
        trailers.insert("x-large", "123456789".parse().unwrap());
        let (mut body, capture) = capture(Body::empty().with_trailer_headers(trailers), 8);
        assert!(body.frame().await.unwrap().is_err());
        drop(body);
        let captured = capture.handle.wait().await.unwrap();
        assert_eq!(captured.outcome(), CaptureOutcome::Error);
        assert!(captured.trailers().is_none());
        assert!(
            capture
                .failure
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("trailers")
        );
    }
}
