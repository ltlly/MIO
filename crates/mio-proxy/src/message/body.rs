use crate::ProxyError;
use mio_plugin_api::{Header, HttpRequest};
use rama::http::{Body, HeaderMap, HeaderName, HeaderValue, Request};

pub(crate) fn request_head_with_fallback(
    request: &Request,
    tunnel: Option<&str>,
    fallback_host: Option<&str>,
) -> Result<HttpRequest, ProxyError> {
    let uri = request.uri();
    let url = if uri.scheme().is_some() {
        uri.to_string()
    } else {
        let authority = tunnel
            .map(str::to_owned)
            .or_else(|| {
                request
                    .headers()
                    .get("host")
                    .and_then(|host| host.to_str().ok())
                    .map(str::trim)
                    .filter(|host| !host.is_empty())
                    .map(str::to_owned)
            })
            .or_else(|| {
                fallback_host
                    .map(str::trim)
                    .filter(|host| !host.is_empty())
                    .map(|host| {
                        if host.contains(':') && !host.starts_with('[') && !host.ends_with(']') {
                            format!("[{host}]")
                        } else {
                            host.to_owned()
                        }
                    })
            })
            .ok_or_else(|| ProxyError::invalid("缺少请求目标 authority"))?;
        format!(
            "{}://{}{}",
            if tunnel.is_some() { "https" } else { "http" },
            authority,
            uri
        )
    };
    Ok(HttpRequest {
        method: request.method().to_string(),
        url,
        version: super::model_version(request.version()),
        headers: super::headers(request.headers()),
        body: Vec::new(),
        trailers: Vec::new(),
    })
}

pub(crate) fn invalid_request_head(request: &Request) -> HttpRequest {
    HttpRequest {
        method: request.method().to_string(),
        url: request.uri().to_string(),
        version: super::model_version(request.version()),
        headers: super::headers(request.headers()),
        body: Vec::new(),
        trailers: Vec::new(),
    }
}

pub(crate) fn normalize_headers(
    headers: &mut HeaderMap,
    length: usize,
    has_trailers: bool,
    request: bool,
) {
    normalize_headers_with_options(headers, length, has_trailers, request, true, true, false);
}

pub(super) fn normalize_headers_with_options(
    headers: &mut HeaderMap,
    length: usize,
    has_trailers: bool,
    request: bool,
    update_content_length: bool,
    strip_proxy_headers: bool,
    strip_100_continue: bool,
) {
    let nominated: Vec<String> = headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(',').map(|name| name.trim().to_owned()))
        .collect();
    for name in nominated {
        if update_content_length || !name.eq_ignore_ascii_case("content-length") {
            headers.remove(name);
        }
    }
    for name in ["connection", "keep-alive", "transfer-encoding", "upgrade"] {
        headers.remove(name);
    }
    if strip_proxy_headers {
        headers.remove("proxy-connection");
        if request {
            let proxy_names: Vec<HeaderName> = headers
                .keys()
                .filter(|name| {
                    name.as_str()
                        .get(..6)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("proxy-"))
                })
                .cloned()
                .collect();
            for name in proxy_names {
                headers.remove(name);
            }
        }
    }
    if strip_100_continue && request {
        strip_expect_100_continue_map(headers);
    }
    if has_trailers {
        headers.remove("content-length");
    } else {
        headers.remove("trailer");
        if update_content_length {
            headers.remove("content-length");
            headers.insert("content-length", HeaderValue::from(length));
        }
    }
}

fn strip_expect_100_continue_map(headers: &mut HeaderMap) {
    if !headers.contains_key("expect") {
        return;
    }
    let kept: Vec<HeaderValue> = headers
        .get_all("expect")
        .iter()
        .filter_map(|value| {
            let Ok(text) = value.to_str() else {
                return Some(value.clone());
            };
            let remaining: Vec<&str> = text
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty() && !token.eq_ignore_ascii_case("100-continue"))
                .collect();
            if remaining.is_empty() {
                None
            } else {
                HeaderValue::from_str(&remaining.join(", ")).ok()
            }
        })
        .collect();
    headers.remove("expect");
    for value in kept {
        headers.append("expect", value);
    }
}

pub(crate) fn strip_expect_100_continue_vec(headers: &mut Vec<Header>) {
    headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case("expect") {
            return true;
        }
        let Ok(text) = std::str::from_utf8(&header.value) else {
            return true;
        };
        let remaining: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty() && !token.eq_ignore_ascii_case("100-continue"))
            .collect();
        if remaining.is_empty() {
            false
        } else {
            header.value = remaining.join(", ").into_bytes();
            true
        }
    });
}

pub(crate) fn outgoing_body(bytes: Vec<u8>, trailers: &[Header]) -> Result<Body, ProxyError> {
    if trailers.is_empty() {
        return Ok(Body::from(bytes));
    }
    // Unknown size keeps HTTP/1 chunked; an exact-size body would suppress trailers.
    let body = Body::from_stream(rama::futures::stream::iter([Ok::<_, std::io::Error>(
        bytes,
    )]));
    Ok(body.with_trailer_headers(super::header_map(trailers)?))
}
