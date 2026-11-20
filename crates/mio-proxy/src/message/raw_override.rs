use crate::{ProxyError, SendConfig, UpstreamRoute};
use mio_plugin_api::{Header, HttpRequest, HttpVersion};
use rama::bytes::BytesMut;
use rama::http::{
    HeaderMap, HeaderName, HeaderValue, Method,
    headers::{HeaderMapExt, ProxyAuthorization},
};
use rama::net::{uri::Uri, user::ProxyCredential};
use std::sync::Arc;

struct RawHeaderLine<'a> {
    raw_line: &'a [u8],
    ending: &'a [u8],
    name: &'a str,
    value: &'a [u8],
}

fn next_raw_line(input: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let lf = input.iter().position(|byte| *byte == b'\n')?;
    let (content, ending) = if lf > 0 && input[lf - 1] == b'\r' {
        (&input[..lf - 1], &input[lf - 1..=lf])
    } else {
        (&input[..lf], &input[lf..=lf])
    };
    Some((content, ending, &input[lf + 1..]))
}

fn parse_raw_header_line<'a>(
    raw_line: &'a [u8],
    ending: &'a [u8],
) -> Result<RawHeaderLine<'a>, ProxyError> {
    let colon = raw_line
        .iter()
        .position(|byte| *byte == b':')
        .ok_or_else(|| ProxyError::invalid("原始 HTTP/1 header 缺少冒号"))?;
    let name_bytes = &raw_line[..colon];
    let _ = HeaderName::from_bytes(name_bytes).map_err(ProxyError::invalid)?;
    let name = std::str::from_utf8(name_bytes).map_err(ProxyError::invalid)?;
    let value = raw_line[colon + 1..].trim_ascii();
    let _ = HeaderValue::from_bytes(value).map_err(ProxyError::invalid)?;
    Ok(RawHeaderLine {
        raw_line,
        ending,
        name,
        value,
    })
}

fn validate_raw_body_and_trailers(
    mut rest: &[u8],
    chunked: bool,
    expected_body: &[u8],
    expected_trailers: &[Header],
) -> Result<(), ProxyError> {
    if !chunked {
        if !expected_trailers.is_empty() {
            return Err(ProxyError::invalid(
                "非 chunked 原始 HTTP/1 报文与请求 trailers 不一致",
            ));
        }
        if rest != expected_body {
            return Err(ProxyError::invalid("原始 HTTP/1 正文与已解析请求不一致"));
        }
        return Ok(());
    }
    let mut body_offset = 0usize;
    loop {
        let (size_line, _, after_size) =
            next_raw_line(rest).ok_or_else(|| ProxyError::invalid("原始 chunked 长度行未结束"))?;
        if size_line.len() > 1024 {
            return Err(ProxyError::invalid("原始 chunked 长度行过长"));
        }
        let size_text = size_line
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or_default();
        if size_text.is_empty() || !size_text.iter().all(u8::is_ascii_hexdigit) {
            return Err(ProxyError::invalid("原始 chunked 长度无效"));
        }
        let size_str = std::str::from_utf8(size_text).map_err(ProxyError::invalid)?;
        let size = usize::from_str_radix(size_str, 16).map_err(ProxyError::invalid)?;
        rest = after_size;
        if size == 0 {
            if rest.len() > crate::StreamedResponse::MAX_HEAD_BYTES {
                return Err(ProxyError::invalid("原始 chunked trailers 过长"));
            }
            let mut parsed_trailers = Vec::new();
            loop {
                let (line, _, after_line) = next_raw_line(rest)
                    .ok_or_else(|| ProxyError::invalid("原始 chunked trailers 未结束"))?;
                rest = after_line;
                if line.is_empty() {
                    break;
                }
                if parsed_trailers.len() >= crate::StreamedResponse::MAX_HEADERS {
                    return Err(ProxyError::invalid("原始 chunked trailers 数量超限"));
                }
                let header = parse_raw_header_line(line, b"\r\n")?;
                if ["content-length", "transfer-encoding", "host"]
                    .iter()
                    .any(|forbidden| header.name.eq_ignore_ascii_case(forbidden))
                {
                    return Err(ProxyError::invalid(
                        "原始 chunked trailers 含禁止的 framing header",
                    ));
                }
                parsed_trailers.push(Header {
                    name: header.name.to_owned(),
                    value: header.value.to_vec(),
                });
            }
            if !rest.is_empty() {
                return Err(ProxyError::invalid("原始 chunked 报文末尾存在额外字节"));
            }
            if body_offset != expected_body.len() || parsed_trailers != expected_trailers {
                return Err(ProxyError::invalid(
                    "原始 chunked 正文或 trailers 与已解析请求不一致",
                ));
            }
            return Ok(());
        }
        let next_body_offset = body_offset
            .checked_add(size)
            .ok_or_else(|| ProxyError::invalid("原始 chunked 正文长度溢出"))?;
        if next_body_offset > expected_body.len()
            || rest.len() < size
            || rest[..size] != expected_body[body_offset..next_body_offset]
        {
            return Err(ProxyError::invalid(
                "原始 chunked 正文内容与已解析请求不一致",
            ));
        }
        let (chunk_tail, _, after_chunk) = next_raw_line(&rest[size..])
            .ok_or_else(|| ProxyError::invalid("原始 chunked 数据块缺少行尾"))?;
        if !chunk_tail.is_empty() {
            return Err(ProxyError::invalid("原始 chunked 数据块长度不匹配"));
        }
        body_offset = next_body_offset;
        rest = after_chunk;
    }
}

pub(super) fn build_raw_h1_from_override(
    raw_override: &[u8],
    request: &HttpRequest,
    method: &Method,
    uri: &Uri,
    version: HttpVersion,
    config: &SendConfig,
) -> Result<Arc<[u8]>, ProxyError> {
    if matches!(version, HttpVersion::Http2) {
        return Err(ProxyError::invalid("原始 HTTP/1 发送不接受 HTTP/2"));
    }
    let (first_line, first_ending, mut cursor) = next_raw_line(raw_override)
        .ok_or_else(|| ProxyError::invalid("原始 HTTP/1 报文缺少请求行"))?;
    let first_str = std::str::from_utf8(first_line)
        .map_err(|_| ProxyError::invalid("原始请求行必须为 UTF-8"))?;
    let parts: Vec<&str> = first_str.split_ascii_whitespace().collect();
    if parts.len() != 3 || parts[0] != method.as_str() {
        return Err(ProxyError::invalid("原始请求行方法与已解析请求不一致"));
    }
    let mut expected_origin = BytesMut::new();
    uri.write_http_origin_form(&mut expected_origin)
        .map_err(ProxyError::invalid)?;
    let mut expected_absolute = BytesMut::new();
    uri.write_http_absolute_form(&mut expected_absolute)
        .map_err(ProxyError::invalid)?;
    if parts[1].as_bytes() != &expected_origin[..]
        && parts[1].as_bytes() != &expected_absolute[..]
        && parts[1] != request.url
    {
        return Err(ProxyError::invalid("原始请求行目标与已解析请求不一致"));
    }
    let mut raw_headers = Vec::new();
    let blank_ending = loop {
        let (line, ending, next_cursor) = next_raw_line(cursor)
            .ok_or_else(|| ProxyError::invalid("原始 HTTP/1 报文需要空行分隔 header 与正文"))?;
        let consumed_head = raw_override.len() - next_cursor.len();
        if consumed_head > crate::StreamedResponse::MAX_HEAD_BYTES {
            return Err(ProxyError::invalid("原始 HTTP/1 header 超过 128 KiB"));
        }
        cursor = next_cursor;
        if line.is_empty() {
            break ending;
        }
        if raw_headers.len() >= crate::StreamedResponse::MAX_HEADERS {
            return Err(ProxyError::invalid("原始 HTTP/1 header 数量超过 1024"));
        }
        raw_headers.push(parse_raw_header_line(line, ending)?);
    };
    let mut transfer_encoding_chunked = false;
    for header in &raw_headers {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            if !header.value.eq_ignore_ascii_case(b"chunked") {
                return Err(ProxyError::invalid(
                    "当前发送路径不支持此 Transfer-Encoding",
                ));
            }
            transfer_encoding_chunked = true;
        }
    }
    let should_update_cl =
        config.update_content_length && request.trailers.is_empty() && !transfer_encoding_chunked;
    let expected_cl = request.body.len().to_string().into_bytes();
    let mut expected_raw_headers: Vec<(String, Vec<u8>)> =
        Vec::with_capacity(raw_headers.len() + 1);
    let mut updated_cl = false;
    for header in &raw_headers {
        if should_update_cl && header.name.eq_ignore_ascii_case("content-length") {
            if !updated_cl {
                expected_raw_headers.push((header.name.to_owned(), expected_cl.clone()));
                updated_cl = true;
            }
        } else {
            expected_raw_headers.push((header.name.to_owned(), header.value.to_vec()));
        }
    }
    if should_update_cl && !updated_cl && !request.body.is_empty() {
        expected_raw_headers.push(("Content-Length".into(), expected_cl.clone()));
    }
    if expected_raw_headers.len() != request.headers.len()
        || expected_raw_headers.iter().zip(&request.headers).any(
            |((raw_name, raw_val), req_hdr)| raw_name != &req_hdr.name || raw_val != &req_hdr.value,
        )
    {
        return Err(ProxyError::invalid("原始 HTTP/1 header 与已解析请求不一致"));
    }
    validate_raw_body_and_trailers(
        cursor,
        transfer_encoding_chunked,
        &request.body,
        &request.trailers,
    )?;

    let effective_route = uri.host_str().map_or_else(
        || config.upstream_route.clone(),
        |host| config.upstream_route.resolve_for_host(&host),
    );
    let route = effective_route.rama()?;
    let via_http_proxy = matches!(
        effective_route,
        UpstreamRoute::Http(_) | UpstreamRoute::Https { .. }
    ) && uri.scheme_str() == Some("http");
    let target = if via_http_proxy {
        &expected_absolute[..]
    } else if parts[1].as_bytes() == &expected_origin[..] {
        parts[1].as_bytes()
    } else {
        &expected_origin[..]
    };
    let mut out = Vec::with_capacity(raw_override.len() + 128);
    out.extend_from_slice(method.as_str().as_bytes());
    out.push(b' ');
    out.extend_from_slice(target);
    out.extend_from_slice(match version {
        HttpVersion::Http10 => b" HTTP/1.0",
        HttpVersion::Http11 => b" HTTP/1.1",
        HttpVersion::Http2 => unreachable!(),
    });
    out.extend_from_slice(first_ending);

    let nominated: Vec<String> = raw_headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("connection"))
        .filter_map(|header| std::str::from_utf8(header.value).ok())
        .flat_map(|value| value.split(',').map(|name| name.trim().to_owned()))
        .filter(|name| !name.is_empty())
        .collect();
    let mut has_host = false;
    let mut wrote_cl = false;
    for header in &raw_headers {
        let is_nominated = nominated.iter().any(|nom| {
            header.name.eq_ignore_ascii_case(nom)
                && (config.update_content_length || !nom.eq_ignore_ascii_case("content-length"))
        });
        let is_proxy_header = config.strip_proxy_headers
            && header
                .name
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("proxy-"));
        let is_stripped_expect = config.strip_100_continue_headers
            && header.name.eq_ignore_ascii_case("expect")
            && header.value.eq_ignore_ascii_case(b"100-continue");
        let is_hop_by_hop = ["connection", "keep-alive", "upgrade"]
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name))
            || is_proxy_header
            || is_stripped_expect
            || (!transfer_encoding_chunked
                && header.name.eq_ignore_ascii_case("transfer-encoding"))
            || (request.trailers.is_empty() && header.name.eq_ignore_ascii_case("trailer"))
            || ((!request.trailers.is_empty() || transfer_encoding_chunked)
                && config.update_content_length
                && header.name.eq_ignore_ascii_case("content-length"));
        if is_nominated || is_hop_by_hop {
            continue;
        }
        if header.name.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        if should_update_cl && header.name.eq_ignore_ascii_case("content-length") {
            if wrote_cl {
                continue;
            }
            out.extend_from_slice(header.name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(&expected_cl);
            out.extend_from_slice(header.ending);
            wrote_cl = true;
        } else {
            out.extend_from_slice(header.raw_line);
            out.extend_from_slice(header.ending);
        }
    }
    if should_update_cl && !wrote_cl && !request.body.is_empty() {
        out.extend_from_slice(b"Content-Length: ");
        out.extend_from_slice(&expected_cl);
        out.extend_from_slice(blank_ending);
    }
    if !has_host {
        let authority = uri
            .authority()
            .ok_or_else(|| ProxyError::invalid("URL 缺少 authority"))?;
        out.extend_from_slice(b"host: ");
        out.extend_from_slice(authority.to_string().as_bytes());
        out.extend_from_slice(blank_ending);
    }
    if via_http_proxy
        && let Some(pa) = route.proxy_address()
        && let Some(ProxyCredential::Basic(basic)) = pa.credential.clone()
    {
        let mut auth_map = HeaderMap::new();
        auth_map.typed_insert(ProxyAuthorization(basic));
        if let Some(value) = auth_map.get("proxy-authorization") {
            out.extend_from_slice(b"proxy-authorization: ");
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(blank_ending);
        }
    }
    if !config.http1_keep_alive {
        out.extend_from_slice(b"Connection: close");
        out.extend_from_slice(blank_ending);
    }
    out.extend_from_slice(blank_ending);
    out.extend_from_slice(cursor);
    Ok(Arc::from(out))
}
