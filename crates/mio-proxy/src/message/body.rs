use crate::ProxyError;
use mio_plugin_api::{Header, HttpRequest};
use rama::http::{Body, HeaderMap, HeaderName, HeaderValue, Request};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TunnelTarget {
    Tls(String),
    PlainHttp(String),
}

pub(crate) fn request_head_with_fallback(
    request: &Request,
    tunnel: Option<&TunnelTarget>,
    fallback_host: Option<&str>,
) -> Result<HttpRequest, ProxyError> {
    let uri = request.uri();
    let url = if uri.scheme().is_some() {
        uri.to_string()
    } else {
        let (scheme, tunnel_authority) = match tunnel {
            Some(TunnelTarget::Tls(authority)) => ("https", Some(authority.as_str())),
            Some(TunnelTarget::PlainHttp(authority)) => ("http", Some(authority.as_str())),
            None => ("http", None),
        };
        let authority = tunnel_authority
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
                    .map(crate::route::format_host_for_authority)
            })
            .ok_or_else(|| ProxyError::invalid("缺少请求目标 authority"))?;
        let raw_target = uri.to_string();
        if raw_target == "*" {
            format!("{scheme}://{authority}/")
        } else if raw_target.starts_with('/') || raw_target.is_empty() {
            format!("{scheme}://{authority}{raw_target}")
        } else {
            format!("{scheme}://{authority}/{raw_target}")
        }
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

pub(super) struct HeaderNormOptions {
    pub update_content_length: bool,
    pub strip_proxy_headers: bool,
    pub strip_100_continue: bool,
    pub preserve_websocket_upgrade: bool,
    pub strip_ws_extensions: bool,
}

pub(crate) fn has_header_token(headers: &HeaderMap, name: &str, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
}

pub(crate) fn strip_header_token_str(text: &str, token: &str) -> (bool, Option<String>) {
    let mut removed = false;
    let remaining: Vec<&str> = text
        .split(',')
        .map(str::trim)
        .filter(|part| {
            if part.is_empty() {
                return false;
            }
            if part.eq_ignore_ascii_case(token) {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect();
    let replacement = if remaining.is_empty() {
        None
    } else {
        Some(remaining.join(", "))
    };
    (removed, replacement)
}

pub(crate) fn normalize_headers(
    headers: &mut HeaderMap,
    length: usize,
    has_trailers: bool,
    request: bool,
) {
    normalize_headers_with_options(
        headers,
        length,
        has_trailers,
        request,
        HeaderNormOptions {
            update_content_length: true,
            strip_proxy_headers: true,
            strip_100_continue: false,
            preserve_websocket_upgrade: false,
            strip_ws_extensions: false,
        },
    );
}

pub(super) fn normalize_headers_with_options(
    headers: &mut HeaderMap,
    length: usize,
    has_trailers: bool,
    request: bool,
    opts: HeaderNormOptions,
) {
    let nominated: Vec<String> = headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(',').map(|name| name.trim().to_owned()))
        .collect();
    for name in nominated {
        let keep_upgrade = opts.preserve_websocket_upgrade && name.eq_ignore_ascii_case("upgrade");
        let keep_cl = !opts.update_content_length && name.eq_ignore_ascii_case("content-length");
        if !keep_upgrade && !keep_cl {
            headers.remove(name);
        }
    }
    let hop_by_hop: &[&str] = if opts.preserve_websocket_upgrade {
        &["keep-alive", "transfer-encoding"]
    } else {
        &["connection", "keep-alive", "transfer-encoding", "upgrade"]
    };
    for name in hop_by_hop {
        headers.remove(*name);
    }
    if opts.strip_proxy_headers {
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
    if opts.strip_100_continue && request {
        strip_expect_100_continue_map(headers);
    }
    if opts.preserve_websocket_upgrade {
        headers.insert("connection", HeaderValue::from_static("Upgrade"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.remove("content-length");
        headers.remove("trailer");
        if opts.strip_ws_extensions {
            headers.remove("sec-websocket-extensions");
        }
        return;
    }
    if has_trailers {
        headers.remove("content-length");
    } else {
        headers.remove("trailer");
        if opts.update_content_length {
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
            match strip_header_token_str(text, "100-continue").1 {
                None => None,
                Some(remaining) => HeaderValue::from_str(&remaining).ok(),
            }
        })
        .collect();
    headers.remove("expect");
    for value in kept {
        headers.append("expect", value);
    }
}

pub(crate) fn strip_header_token_vec(headers: &mut Vec<Header>, header_name: &str, token: &str) {
    headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case(header_name) {
            return true;
        }
        let Ok(text) = std::str::from_utf8(&header.value) else {
            return true;
        };
        match strip_header_token_str(text, token).1 {
            None => false,
            Some(remaining) => {
                header.value = remaining.into_bytes();
                true
            }
        }
    });
}

pub(crate) fn strip_expect_100_continue_vec(headers: &mut Vec<Header>) {
    strip_header_token_vec(headers, "expect", "100-continue");
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
