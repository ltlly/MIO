use super::*;
use crate::{ProxyError, SendConfig, UpstreamRoute};
use mio_plugin_api::{HttpRequest, HttpResponse, HttpVersion};
use rama::extensions::ExtensionsRef;
use rama::http::{
    Body, HeaderMap, HeaderValue, Method, Request, Response, StatusCode,
    proto::h1::ext::ReasonPhrase,
};
use rama::net::uri::Uri;
use std::sync::{Arc, atomic::AtomicBool};

struct ValidatedRequestHead {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    effective_version: HttpVersion,
    effective_route: UpstreamRoute,
}

fn validate_and_build_request_head(
    request: &HttpRequest,
    config: &SendConfig,
    preserve_websocket_upgrade: bool,
    strip_ws_extensions: bool,
) -> Result<ValidatedRequestHead, ProxyError> {
    if request.body.len() > config.limits.max_body_bytes {
        return Err(ProxyError::BodyLimit {
            limit: config.limits.max_body_bytes,
        });
    }
    let uri: Uri = request.url.parse().map_err(ProxyError::invalid)?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
        return Err(ProxyError::invalid("发送目标必须是完整 HTTP/HTTPS URL"));
    }
    // shortcut: Rama encodes raw backslashes; accept them only after a lossless writer is verified.
    if request
        .url
        .split('#')
        .next()
        .unwrap_or_default()
        .contains('\\')
    {
        return Err(ProxyError::invalid(
            "当前 SDK 无法原样发送路径或查询中的反斜线；请显式使用 %5C",
        ));
    }
    let method = Method::from_bytes(request.method.as_bytes()).map_err(ProxyError::invalid)?;
    if method == Method::CONNECT {
        return Err(ProxyError::invalid("普通 HTTP 发送不接受 CONNECT 隧道请求"));
    }
    let effective_version = config.http.version.unwrap_or(request.version);
    let has_req_trailers =
        effective_version != HttpVersion::Http10 && !request.trailers.is_empty();
    let mut headers = header_map(&request.headers)?;
    normalize_headers_with_options(
        &mut headers,
        request.body.len(),
        has_req_trailers,
        true,
        body::HeaderNormOptions {
            update_content_length: config.http.update_content_length,
            strip_proxy_headers: config.http.strip_proxy_headers,
            strip_100_continue: config.http.strip_100_continue_headers,
            preserve_websocket_upgrade,
            strip_ws_extensions,
        },
    );
    if !headers.contains_key("host") {
        let host_str = uri
            .host_str()
            .ok_or_else(|| ProxyError::invalid("URL 缺少主机名"))?;
        let host_authority = match uri.port_u16() {
            Some(port) => format!("{}:{port}", crate::route::format_host_for_authority(&host_str)),
            None => crate::route::format_host_for_authority(&host_str),
        };
        headers.insert(
            "host",
            HeaderValue::from_str(&host_authority).map_err(ProxyError::invalid)?,
        );
    }
    // effective_version computed above
    if !preserve_websocket_upgrade && effective_version != HttpVersion::Http2 {
        if !config.http.http1_keep_alive {
            headers.insert("connection", HeaderValue::from_static("close"));
        } else if config.http.set_connection_keep_alive {
            headers.insert("connection", HeaderValue::from_static("keep-alive"));
        }
    }
    let effective_route = uri.host_str().map_or_else(
        || config.network.upstream_route.clone(),
        |host| config.network.upstream_route.resolve_for_host(&host),
    );
    Ok(ValidatedRequestHead {
        method,
        uri,
        headers,
        effective_version,
        effective_route,
    })
}

pub(crate) fn validate_request(request: &HttpRequest, max: usize) -> Result<(), ProxyError> {
    let mut config = SendConfig::default();
    config.limits.max_body_bytes = max;
    validate_request_with_config(request, &config)
}

pub(crate) fn validate_request_with_config(
    request: &HttpRequest,
    config: &SendConfig,
) -> Result<(), ProxyError> {
    let head = validate_and_build_request_head(request, config, false, false)?;
    if let Some(raw_override) = config
        .http
        .raw_http1_override
        .as_deref()
        .filter(|_| !config.http.normalize_http1_line_endings)
        && head.effective_version != HttpVersion::Http2
    {
        let _ = build_raw_h1_from_override(
            raw_override,
            request,
            &head.method,
            &head.uri,
            head.effective_version,
            config,
        )?;
        return Ok(());
    }
    let has_req_trailers =
        head.effective_version != HttpVersion::Http10 && !request.trailers.is_empty();
    if !config.http.update_content_length
        && !has_req_trailers
        && head.effective_version != HttpVersion::Http2
    {
        let _ = head.effective_route.rama()?;
    }
    if !request.trailers.is_empty() {
        let _ = header_map(&request.trailers)?;
    }
    Ok(())
}

pub(crate) fn to_websocket_upgrade_request(
    request: &HttpRequest,
    config: &SendConfig,
    strip_ws_extensions: bool,
) -> Result<Request, ProxyError> {
    let head = validate_and_build_request_head(request, config, true, strip_ws_extensions)?;
    let mut result = Request::new(Body::empty());
    *result.method_mut() = head.method;
    *result.uri_mut() = head.uri;
    *result.version_mut() = version(head.effective_version);
    *result.headers_mut() = head.headers;
    Ok(result)
}

pub(crate) fn to_request_with_config(
    request: &HttpRequest,
    config: &SendConfig,
) -> Result<Request, ProxyError> {
    let head = validate_and_build_request_head(request, config, false, false)?;
    let req_trailers = if head.effective_version != HttpVersion::Http10 {
        request.trailers.as_slice()
    } else {
        &[]
    };
    let (body, raw_h1) = if let Some(raw_override) = config
        .http
        .raw_http1_override
        .as_deref()
        .filter(|_| !config.http.normalize_http1_line_endings)
    {
        if head.effective_version != HttpVersion::Http2 {
            let suppressed = Arc::new(AtomicBool::new(false));
            let bytes = build_raw_h1_from_override(
                raw_override,
                request,
                &head.method,
                &head.uri,
                head.effective_version,
                config,
            )?;
            (
                Body::new(RawBody::new(request.body.clone(), suppressed.clone())),
                Some(RawH1Request { bytes, suppressed }),
            )
        } else {
            (
                outgoing_body(request.body.clone(), req_trailers)?,
                None,
            )
        }
    } else if !config.http.update_content_length && req_trailers.is_empty() {
        let suppressed = Arc::new(AtomicBool::new(false));
        let raw_h1 = if head.effective_version != HttpVersion::Http2 {
            Some(RawH1Request {
                bytes: build_raw_h1_request(
                    &head.method,
                    &head.uri,
                    head.effective_version,
                    &head.headers,
                    &request.body,
                    &head.effective_route,
                )?,
                suppressed: suppressed.clone(),
            })
        } else {
            None
        };
        (
            Body::new(RawBody::new(request.body.clone(), suppressed)),
            raw_h1,
        )
    } else {
        (
            outgoing_body(request.body.clone(), req_trailers)?,
            None,
        )
    };
    let mut result = Request::new(body);
    *result.method_mut() = head.method;
    *result.uri_mut() = head.uri;
    *result.version_mut() = version(head.effective_version);
    *result.headers_mut() = head.headers;
    if let Some(raw_h1) = raw_h1 {
        result.extensions().insert(raw_h1);
    }
    Ok(result)
}

pub(crate) fn validate_response(response: &HttpResponse, max: usize) -> Result<(), ProxyError> {
    if response.body.len() > max {
        return Err(ProxyError::BodyLimit { limit: max });
    }
    StatusCode::from_u16(response.status).map_err(ProxyError::invalid)?;
    let _ = header_map(&response.headers)?;
    let no_body = (100..200).contains(&response.status) || matches!(response.status, 204 | 304);
    if !no_body && !response.trailers.is_empty() {
        let _ = header_map(&response.trailers)?;
    }
    Ok(())
}

pub(crate) fn to_response(
    response: &HttpResponse,
    max: usize,
    client_version: HttpVersion,
    head: bool,
) -> Result<Response, ProxyError> {
    if response.body.len() > max {
        return Err(ProxyError::BodyLimit { limit: max });
    }
    let mut headers = header_map(&response.headers)?;
    let no_body =
        head || (100..200).contains(&response.status) || matches!(response.status, 204 | 304);
    let original_length = headers.get("content-length").cloned();
    let has_resp_trailers =
        !no_body && client_version != HttpVersion::Http10 && !response.trailers.is_empty();
    normalize_headers(
        &mut headers,
        response.body.len(),
        has_resp_trailers,
        false,
    );
    if no_body {
        headers.remove("content-length");
        if (head || response.status == 304)
            && let Some(length) = original_length
        {
            headers.insert("content-length", length);
        }
    }
    let body = if no_body {
        Body::empty()
    } else {
        outgoing_body(
            response.body.clone(),
            if has_resp_trailers {
                &response.trailers
            } else {
                &[]
            },
        )?
    };
    let mut result = Response::new(body);
    *result.status_mut() = StatusCode::from_u16(response.status).map_err(ProxyError::invalid)?;
    *result.version_mut() = version(client_version);
    *result.headers_mut() = headers;
    if client_version != HttpVersion::Http2
        && let Some(reason) = response.reason_phrase.as_deref().filter(|r| !r.is_empty())
        && let Ok(rp) = ReasonPhrase::try_from(reason)
    {
        result.extensions().insert(rp);
    }
    Ok(result)
}

pub(crate) fn error_response_with_options(error: &ProxyError, suppress: bool) -> Response {
    if suppress {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::BAD_GATEWAY;
        return response;
    }
    let status = match error {
        ProxyError::InvalidMessage(_) | ProxyError::InterceptStage => StatusCode::BAD_REQUEST,
        ProxyError::BodyLimit { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        ProxyError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ProxyError::Dropped => StatusCode::FORBIDDEN,
        ProxyError::Cancelled | ProxyError::EventConsumerClosed => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_GATEWAY,
    };
    let mut response = Response::new(Body::from(error.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}
