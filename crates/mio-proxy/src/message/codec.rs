use super::*;
use crate::{ProxyError, SendConfig};
use mio_plugin_api::{HttpRequest, HttpResponse, HttpVersion};
use rama::extensions::ExtensionsRef;
use rama::http::{Body, HeaderValue, Method, Request, Response, StatusCode};
use rama::net::uri::Uri;
use std::sync::{Arc, atomic::AtomicBool};

pub(crate) fn to_request(request: &HttpRequest, max: usize) -> Result<Request, ProxyError> {
    to_request_with_config(
        request,
        &SendConfig {
            max_body_bytes: max,
            ..SendConfig::default()
        },
    )
}

pub(crate) fn to_request_with_config(
    request: &HttpRequest,
    config: &SendConfig,
) -> Result<Request, ProxyError> {
    if request.body.len() > config.max_body_bytes {
        return Err(ProxyError::BodyLimit {
            limit: config.max_body_bytes,
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
    let mut headers = header_map(&request.headers)?;
    normalize_headers_with_options(
        &mut headers,
        request.body.len(),
        !request.trailers.is_empty(),
        true,
        config.update_content_length,
        config.strip_proxy_headers,
        config.strip_100_continue_headers,
    );
    if !headers.contains_key("host") {
        let authority = uri
            .authority()
            .ok_or_else(|| ProxyError::invalid("URL 缺少 authority"))?;
        headers.insert(
            "host",
            HeaderValue::from_str(&authority.to_string()).map_err(ProxyError::invalid)?,
        );
    }
    let effective_version = config.http_version.unwrap_or(request.version);
    if effective_version != HttpVersion::Http2 {
        if !config.http1_keep_alive {
            headers.insert("connection", HeaderValue::from_static("close"));
        } else if config.set_connection_keep_alive {
            headers.insert("connection", HeaderValue::from_static("keep-alive"));
        }
    }
    let effective_route = uri
        .host_str()
        .map_or_else(
            || config.upstream_route.clone(),
            |host| config.upstream_route.resolve_for_host(&host),
        );
    let (body, raw_h1) = if let Some(raw_override) = config
        .raw_http1_override
        .as_deref()
        .filter(|_| !config.normalize_http1_line_endings)
    {
        if effective_version != HttpVersion::Http2 {
            let suppressed = Arc::new(AtomicBool::new(false));
            let bytes = build_raw_h1_from_override(
                raw_override,
                request,
                &method,
                &uri,
                effective_version,
                config,
            )?;
            (
                Body::new(RawBody::new(request.body.clone(), suppressed.clone())),
                Some(RawH1Request { bytes, suppressed }),
            )
        } else {
            (
                outgoing_body(request.body.clone(), &request.trailers)?,
                None,
            )
        }
    } else if !config.update_content_length && request.trailers.is_empty() {
        let suppressed = Arc::new(AtomicBool::new(false));
        let raw_h1 = if effective_version != HttpVersion::Http2 {
            Some(RawH1Request {
                bytes: build_raw_h1_request(
                    &method,
                    &uri,
                    effective_version,
                    &headers,
                    &request.body,
                    &effective_route,
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
            outgoing_body(request.body.clone(), &request.trailers)?,
            None,
        )
    };
    let mut result = Request::new(body);
    *result.method_mut() = method;
    *result.uri_mut() = uri;
    *result.version_mut() = version(request.version);
    *result.headers_mut() = headers;
    if let Some(raw_h1) = raw_h1 {
        result.extensions().insert(raw_h1);
    }
    Ok(result)
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
    normalize_headers(
        &mut headers,
        response.body.len(),
        !response.trailers.is_empty(),
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
        outgoing_body(response.body.clone(), &response.trailers)?
    };
    let mut result = Response::new(body);
    *result.status_mut() = StatusCode::from_u16(response.status).map_err(ProxyError::invalid)?;
    *result.version_mut() = version(client_version);
    *result.headers_mut() = headers;
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
