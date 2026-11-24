use std::borrow::Cow;
use std::ops::Range;

use mio_plugin_api::{Header, HttpVersion};

const MAX_RAW_HEAD_SCAN: usize = 512 * 1024;
const SMALL_HEADER_SLOTS: usize = 64;
const HIGHLIGHT_HEADER_SLOTS: usize = 512;
const LARGE_HEADER_SLOTS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpHeaderSpan {
    pub name: Range<usize>,
    pub value: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedHttpResponseHead {
    pub status_line: String,
    pub status_code: Option<u16>,
    pub content_type: Option<String>,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

pub(crate) struct ParsedRawRequest<'a> {
    pub method: &'a str,
    pub target: &'a str,
    pub version: &'static str,
    pub headers: Vec<Header>,
    pub body: &'a [u8],
}

fn lenient_parser_config() -> httparse::ParserConfig {
    let mut config = httparse::ParserConfig::default();
    config
        .allow_multiple_spaces_in_request_line_delimiters(true)
        .allow_multiple_spaces_in_response_status_delimiters(true)
        .allow_space_before_first_header_name(true)
        .allow_obsolete_multiline_headers_in_responses(true);
    config
}

fn find_head_end(bytes: &[u8]) -> Option<usize> {
    let limit = bytes.len().min(MAX_RAW_HEAD_SCAN);
    let scan = &bytes[..limit];
    let mut best: Option<usize> = None;
    let mut consider = |pos: usize, delim_len: usize| {
        let end = pos + delim_len;
        if best.is_none_or(|prev| end < prev) {
            best = Some(end);
        }
    };
    if let Some(pos) = scan.windows(4).position(|w| w == b"\r\n\r\n") {
        consider(pos, 4);
    }
    if let Some(pos) = scan.windows(2).position(|w| w == b"\n\n") {
        consider(pos, 2);
    }
    if let Some(pos) = scan.windows(3).position(|w| w == b"\r\n\n") {
        consider(pos, 3);
    }
    if let Some(pos) = scan.windows(3).position(|w| w == b"\n\r\n") {
        consider(pos, 3);
    }
    best
}

fn normalize_http2_start_line(line: &[u8]) -> Option<Vec<u8>> {
    if let Some(rest) = line.strip_prefix(b"HTTP/2.0 ") {
        let mut normalized = Vec::with_capacity(line.len());
        normalized.extend_from_slice(b"HTTP/1.1 ");
        normalized.extend_from_slice(rest);
        return Some(normalized);
    }
    if let Some(rest) = line.strip_prefix(b"HTTP/2 ") {
        let mut normalized = Vec::with_capacity(line.len() + 2);
        normalized.extend_from_slice(b"HTTP/1.1 ");
        normalized.extend_from_slice(rest);
        return Some(normalized);
    }
    if let Some(prefix) = line.strip_suffix(b" HTTP/2.0") {
        let mut normalized = Vec::with_capacity(line.len());
        normalized.extend_from_slice(prefix);
        normalized.extend_from_slice(b" HTTP/1.1");
        return Some(normalized);
    }
    if let Some(prefix) = line.strip_suffix(b" HTTP/2") {
        let mut normalized = Vec::with_capacity(line.len() + 2);
        normalized.extend_from_slice(prefix);
        normalized.extend_from_slice(b" HTTP/1.1");
        return Some(normalized);
    }
    None
}

fn normalize_http2_head_slice(bytes: &[u8]) -> Option<(Cow<'_, [u8]>, usize)> {
    let head_limit = find_head_end(bytes).unwrap_or_else(|| bytes.len().min(MAX_RAW_HEAD_SCAN));
    let head_slice = &bytes[..head_limit];
    let newline = head_slice.iter().position(|&b| b == b'\n')?;
    let eol_len = if newline > 0 && head_slice[newline - 1] == b'\r' {
        2
    } else {
        1
    };
    let line_end = newline + 1 - eol_len;
    let first_line = &head_slice[..line_end];
    if let Some(mut normalized_line) = normalize_http2_start_line(first_line) {
        let added = normalized_line.len() - first_line.len();
        normalized_line.extend_from_slice(&head_slice[line_end..]);
        Some((Cow::Owned(normalized_line), added))
    } else {
        Some((Cow::Borrowed(head_slice), 0))
    }
}

fn parse_head_with_slots<'a, T>(
    input: &'a [u8],
    headers: &mut [httparse::Header<'a>],
    map_headers: impl FnOnce(&[httparse::Header<'a>]) -> T,
) -> Result<(usize, T), httparse::Error> {
    let config = lenient_parser_config();
    let (status, parsed_slice) = if input.starts_with(b"HTTP/") {
        let mut resp = httparse::Response::new(headers);
        let status = config.parse_response(&mut resp, input)?;
        (status, resp.headers as &[httparse::Header<'a>])
    } else {
        let mut req = httparse::Request::new(headers);
        let status = config.parse_request(&mut req, input)?;
        (status, req.headers as &[httparse::Header<'a>])
    };
    match status {
        httparse::Status::Complete(offset) => Ok((offset, map_headers(parsed_slice))),
        httparse::Status::Partial => Err(httparse::Error::HeaderValue),
    }
}

fn collect_headers(parsed: &[httparse::Header<'_>]) -> Vec<Header> {
    parsed
        .iter()
        .map(|h| Header {
            name: h.name.to_owned(),
            value: h.value.to_vec(),
        })
        .collect()
}

pub fn parse_http_head_info(bytes: &[u8]) -> Option<(usize, Vec<Header>)> {
    if bytes.is_empty() {
        return None;
    }
    let (input, added) = normalize_http2_head_slice(bytes)?;
    let mut small = [httparse::EMPTY_HEADER; SMALL_HEADER_SLOTS];
    match parse_head_with_slots(&input, &mut small, collect_headers) {
        Ok((offset, list)) => Some((offset - added, list)),
        Err(httparse::Error::TooManyHeaders) => {
            let mut large = vec![httparse::EMPTY_HEADER; LARGE_HEADER_SLOTS];
            let (offset, list) = parse_head_with_slots(&input, &mut large, collect_headers).ok()?;
            Some((offset - added, list))
        }
        Err(_) => None,
    }
}

pub fn find_http_body_offset(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() {
        return None;
    }
    let (input, added) = normalize_http2_head_slice(bytes)?;
    let mut small = [httparse::EMPTY_HEADER; SMALL_HEADER_SLOTS];
    match parse_head_with_slots(&input, &mut small, |_| ()) {
        Ok((offset, ())) => Some(offset - added),
        Err(httparse::Error::TooManyHeaders) => {
            let mut large = vec![httparse::EMPTY_HEADER; LARGE_HEADER_SLOTS];
            let (offset, ()) = parse_head_with_slots(&input, &mut large, |_| ()).ok()?;
            Some(offset - added)
        }
        Err(_) => None,
    }
}

fn charset_from_content_type_str(value: &str) -> Option<String> {
    for part in value.split(';').skip(1) {
        let trimmed = part.trim();
        if let Some((key, raw_val)) = trimmed.split_once('=')
            && key.trim().eq_ignore_ascii_case("charset")
        {
            let cleaned = raw_val
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_ascii_lowercase());
            }
        }
    }
    None
}

fn extract_charset_from_headers(parsed: &[httparse::Header<'_>]) -> Option<String> {
    for header in parsed {
        if !header.name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        let value = std::str::from_utf8(header.value).ok()?;
        if let Some(charset) = charset_from_content_type_str(value) {
            return Some(charset);
        }
    }
    None
}

pub fn extract_http_header_charset(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let (input, _) = normalize_http2_head_slice(bytes)?;
    let mut small = [httparse::EMPTY_HEADER; SMALL_HEADER_SLOTS];
    match parse_head_with_slots(&input, &mut small, extract_charset_from_headers) {
        Ok((_, charset)) => charset,
        Err(httparse::Error::TooManyHeaders) => {
            let mut large = vec![httparse::EMPTY_HEADER; LARGE_HEADER_SLOTS];
            parse_head_with_slots(&input, &mut large, extract_charset_from_headers)
                .ok()
                .and_then(|(_, charset)| charset)
        }
        Err(_) => None,
    }
}

pub fn extract_content_type_essence_and_charset(content_type: &str) -> (String, Option<String>) {
    let parsed_mime = content_type.parse::<mime::Mime>().ok();
    let essence = parsed_mime
        .as_ref()
        .map(|m| m.essence_str().to_owned())
        .unwrap_or_default();
    let charset = parsed_mime
        .as_ref()
        .and_then(|m| {
            m.get_param(mime::CHARSET)
                .map(|v| v.as_str().to_owned())
        })
        .or_else(|| charset_from_content_type_str(content_type));
    (essence, charset)
}

fn collect_header_spans(base: &[u8], parsed: &[httparse::Header<'_>]) -> Vec<HttpHeaderSpan> {
    let base_ptr = base.as_ptr() as usize;
    let mut spans = Vec::with_capacity(parsed.len());
    for header in parsed {
        if header.name.is_empty() {
            continue;
        }
        let name_start = header.name.as_ptr() as usize - base_ptr;
        let name = name_start..name_start + header.name.len();
        let value = if header.value.is_empty() {
            name.end..name.end
        } else {
            let val_start = header.value.as_ptr() as usize - base_ptr;
            val_start..val_start + header.value.len()
        };
        spans.push(HttpHeaderSpan { name, value });
    }
    spans
}

pub fn parse_http_header_spans(bytes: &[u8]) -> Option<(usize, Vec<HttpHeaderSpan>)> {
    let mut small = [httparse::EMPTY_HEADER; HIGHLIGHT_HEADER_SLOTS];
    match httparse::parse_headers(bytes, &mut small) {
        Ok(httparse::Status::Complete((rel_end, parsed))) => {
            Some((rel_end, collect_header_spans(bytes, parsed)))
        }
        Err(httparse::Error::TooManyHeaders) => {
            let mut large = vec![httparse::EMPTY_HEADER; LARGE_HEADER_SLOTS];
            let Ok(httparse::Status::Complete((rel_end, parsed))) =
                httparse::parse_headers(bytes, &mut large)
            else {
                return None;
            };
            Some((rel_end, collect_header_spans(bytes, parsed)))
        }
        _ => None,
    }
}

fn slice_offset(base: &[u8], sub: &[u8]) -> usize {
    sub.as_ptr() as usize - base.as_ptr() as usize
}

fn header_matches_filter(name: &str, rules: Option<&[String]>, default_rules: &[&str]) -> bool {
    match rules {
        Some(custom) => custom.iter().any(|rule| name.eq_ignore_ascii_case(rule)),
        None => default_rules
            .iter()
            .any(|rule| name.eq_ignore_ascii_case(rule)),
    }
}

fn rebuild_filtered_head(
    raw: &[u8],
    parsed_input: &[u8],
    added_start_line_bytes: usize,
    body_offset: usize,
    headers: &[httparse::Header<'_>],
    rules: Option<&[String]>,
    default_rules: &[&str],
) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::with_capacity(raw.len());
    let mut line_numbers = Vec::new();
    let mut retained_start = 0;
    let mut current_line = 1usize;
    for header in headers {
        if !header_matches_filter(header.name, rules, default_rules) {
            continue;
        }
        let name_start = slice_offset(parsed_input, header.name.as_bytes()) - added_start_line_bytes;
        let value_end = slice_offset(parsed_input, header.value) + header.value.len()
            - added_start_line_bytes;
        let Some(prev_nl) = raw[..name_start].iter().rposition(|&byte| byte == b'\n') else {
            continue;
        };
        let line_start = prev_nl + 1;
        let line_end = match raw
            .get(value_end..body_offset)
            .and_then(|slice| slice.iter().position(|&byte| byte == b'\n'))
        {
            Some(rel) => value_end + rel + 1,
            None => body_offset.min(raw.len()),
        };
        if line_start < retained_start || line_end < line_start {
            continue;
        }
        let retained_slice = &raw[retained_start..line_start];
        let retained_lines = retained_slice.iter().filter(|&&byte| byte == b'\n').count();
        line_numbers.extend(current_line..current_line + retained_lines);
        current_line += retained_lines;
        out.extend_from_slice(retained_slice);
        let skipped_lines = raw[line_start..line_end]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        current_line += skipped_lines;
        retained_start = line_end;
    }
    let tail = &raw[retained_start..];
    let tail_newlines = tail.iter().filter(|&&byte| byte == b'\n').count();
    line_numbers.extend(current_line..=current_line + tail_newlines);
    out.extend_from_slice(tail);
    (out, line_numbers)
}

fn filter_with_slots<'a>(
    raw: &[u8],
    parsed_input: &'a [u8],
    added: usize,
    rules: Option<&[String]>,
    default_request: &[&str],
    default_response: &[&str],
    headers: &mut [httparse::Header<'a>],
) -> Result<(Vec<u8>, Vec<usize>), Result<httparse::Error, String>> {
    let config = lenient_parser_config();
    if raw.starts_with(b"HTTP/") {
        let mut response = httparse::Response::new(headers);
        let status = config
            .parse_response(&mut response, parsed_input)
            .map_err(|err| {
                if matches!(err, httparse::Error::TooManyHeaders) {
                    Ok(err)
                } else {
                    Err(format!("HTTP 响应解析失败（{err}），无法隐藏指定标头。"))
                }
            })?;
        let httparse::Status::Complete(offset) = status else {
            return Err(Err(
                "HTTP 响应标头未以空行完整结束，无法隐藏指定标头。".into()
            ));
        };
        Ok(rebuild_filtered_head(
            raw,
            parsed_input,
            added,
            offset - added,
            response.headers,
            rules,
            default_response,
        ))
    } else {
        let mut request = httparse::Request::new(headers);
        let status = config
            .parse_request(&mut request, parsed_input)
            .map_err(|err| {
                if matches!(err, httparse::Error::TooManyHeaders) {
                    Ok(err)
                } else {
                    Err(format!("HTTP 请求解析失败（{err}），无法隐藏指定标头。"))
                }
            })?;
        let httparse::Status::Complete(offset) = status else {
            return Err(Err(
                "HTTP 请求标头未以空行完整结束，无法隐藏指定标头。".into()
            ));
        };
        Ok(rebuild_filtered_head(
            raw,
            parsed_input,
            added,
            offset - added,
            request.headers,
            rules,
            default_request,
        ))
    }
}

pub fn filter_http_headers_bytes(
    raw: &[u8],
    rules: Option<&[String]>,
    default_request: &[&str],
    default_response: &[&str],
) -> Result<(Vec<u8>, Vec<usize>), String> {
    if raw.is_empty() {
        return Ok((Vec::new(), vec![1]));
    }
    let (parsed_input, added) = normalize_http2_head_slice(raw)
        .ok_or_else(|| "消息不包含可解析的 HTTP 起始行与标头块。".to_owned())?;
    let mut small = [httparse::EMPTY_HEADER; SMALL_HEADER_SLOTS];
    match filter_with_slots(
        raw,
        &parsed_input,
        added,
        rules,
        default_request,
        default_response,
        &mut small,
    ) {
        Ok(res) => Ok(res),
        Err(Ok(httparse::Error::TooManyHeaders)) => {
            let mut large = vec![httparse::EMPTY_HEADER; LARGE_HEADER_SLOTS];
            filter_with_slots(
                raw,
                &parsed_input,
                added,
                rules,
                default_request,
                default_response,
                &mut large,
            )
            .map_err(|err| match err {
                Ok(e) => {
                    if raw.starts_with(b"HTTP/") {
                        format!("HTTP 响应解析失败（{e}），无法隐藏指定标头。")
                    } else {
                        format!("HTTP 请求解析失败（{e}），无法隐藏指定标头。")
                    }
                }
                Err(msg) => msg,
            })
        }
        Err(Ok(_)) => unreachable!(),
        Err(Err(msg)) => Err(msg),
    }
}

fn parse_lenient_response_with_slots<'a>(
    raw: &'a [u8],
    headers_buf: &mut [httparse::Header<'a>],
) -> Result<ParsedHttpResponseHead, httparse::Error> {
    let mut parsed = httparse::Response::new(headers_buf);
    let status = parsed.parse(raw)?;
    let httparse::Status::Complete(offset) = status else {
        return Err(httparse::Error::HeaderValue);
    };
    let ver = match parsed.version {
        Some(0) => "HTTP/1.0",
        _ => "HTTP/1.1",
    };
    let code = parsed.code;
    let reason = parsed.reason.unwrap_or("").trim();
    let status_line = match (code, reason.is_empty()) {
        (Some(c), false) => format!("{ver} {c} {reason}"),
        (Some(c), true) => format!("{ver} {c}"),
        _ => ver.to_owned(),
    };
    let mut ct = None;
    let mut loc = None;
    for h in parsed.headers.iter() {
        if h.name.eq_ignore_ascii_case("content-type") && ct.is_none() {
            ct = Some(String::from_utf8_lossy(h.value).trim().to_owned());
        } else if h.name.eq_ignore_ascii_case("location") && loc.is_none() {
            loc = Some(String::from_utf8_lossy(h.value).trim().to_owned());
        }
    }
    Ok(ParsedHttpResponseHead {
        status_line,
        status_code: code,
        content_type: ct,
        location: loc,
        body: raw[offset..].to_vec(),
    })
}

pub fn parse_http_response_parts(raw: &[u8]) -> ParsedHttpResponseHead {
    if let Ok(resp) = super::parse_response(raw, HttpVersion::Http11) {
        let version = match resp.version {
            HttpVersion::Http10 => "HTTP/1.0",
            HttpVersion::Http2 => "HTTP/2",
            HttpVersion::Http11 => "HTTP/1.1",
        };
        let mut ct = None;
        let mut loc = None;
        for header in &resp.headers {
            if header.name.eq_ignore_ascii_case("content-type") && ct.is_none() {
                ct = Some(String::from_utf8_lossy(&header.value).trim().to_owned());
            } else if header.name.eq_ignore_ascii_case("location") && loc.is_none() {
                loc = Some(String::from_utf8_lossy(&header.value).trim().to_owned());
            }
        }
        return ParsedHttpResponseHead {
            status_line: format!("{version} {}", resp.status),
            status_code: Some(resp.status),
            content_type: ct,
            location: loc,
            body: resp.body,
        };
    }

    let mut small = [httparse::EMPTY_HEADER; SMALL_HEADER_SLOTS];
    match parse_lenient_response_with_slots(raw, &mut small) {
        Ok(res) => res,
        Err(httparse::Error::TooManyHeaders) => {
            let mut large = vec![httparse::EMPTY_HEADER; 2048];
            parse_lenient_response_with_slots(raw, &mut large).unwrap_or_else(|_| {
                ParsedHttpResponseHead {
                    status_line: "无标准 HTTP 状态行".to_owned(),
                    status_code: None,
                    content_type: None,
                    location: None,
                    body: raw.to_vec(),
                }
            })
        }
        Err(_) => ParsedHttpResponseHead {
            status_line: "无标准 HTTP 状态行".to_owned(),
            status_code: None,
            content_type: None,
            location: None,
            body: raw.to_vec(),
        },
    }
}

fn parse_raw_request_with_slots<'a>(
    raw: &'a [u8],
    storage: &mut [httparse::Header<'a>],
) -> Result<ParsedRawRequest<'a>, Result<httparse::Error, String>> {
    let mut request = httparse::Request::new(storage);
    let status = request.parse(raw).map_err(|error| {
        if matches!(error, httparse::Error::TooManyHeaders) {
            Ok(error)
        } else {
            Err(format!("HTTP 请求解析失败：{error}"))
        }
    })?;
    let offset = match status {
        httparse::Status::Complete(pos) => pos,
        httparse::Status::Partial => return Err(Err("HTTP 请求头未完整结束。".into())),
    };
    let method = request.method.ok_or_else(|| Err("HTTP 请求缺少方法。".into()))?;
    let target = request.path.ok_or_else(|| Err("HTTP 请求缺少路径。".into()))?;
    let version = match request.version {
        Some(0) => "HTTP/1.0",
        Some(1) => "HTTP/1.1",
        _ => return Err(Err("HTTP 请求版本无效。".into())),
    };
    let headers = collect_headers(request.headers);
    if headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("transfer-encoding")
            || header.name.eq_ignore_ascii_case("content-encoding")
    }) {
        return Err(Err("请求使用传输编码或压缩编码，请先完成解包。".into()));
    }
    Ok(ParsedRawRequest {
        method,
        target,
        version,
        headers,
        body: &raw[offset..],
    })
}

pub(crate) fn parse_raw_request_for_codec(raw: &[u8]) -> Result<ParsedRawRequest<'_>, String> {
    let mut small = [httparse::EMPTY_HEADER; SMALL_HEADER_SLOTS];
    match parse_raw_request_with_slots(raw, &mut small) {
        Ok(parsed) => Ok(parsed),
        Err(Ok(httparse::Error::TooManyHeaders)) => {
            let mut large = vec![httparse::EMPTY_HEADER; LARGE_HEADER_SLOTS];
            parse_raw_request_with_slots(raw, &mut large).map_err(|err| match err {
                Ok(e) => format!("HTTP 请求解析失败：{e}"),
                Err(msg) => msg,
            })
        }
        Err(Ok(_)) => unreachable!(),
        Err(Err(msg)) => Err(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http2_start_line_head_normalization_handles_large_bodies() {
        let mut raw = b"GET /items HTTP/2\r\nHost: example.com\r\nContent-Type: text/plain; charset=\"GBK\"\r\n\r\n".to_vec();
        let head_len = raw.len();
        raw.extend(vec![b'x'; 256 * 1024]);

        let (offset, headers) = parse_http_head_info(&raw).unwrap();
        assert_eq!(offset, head_len);
        assert_eq!(headers.len(), 2);
        assert_eq!(find_http_body_offset(&raw), Some(head_len));
        assert_eq!(extract_http_header_charset(&raw).as_deref(), Some("gbk"));

        let resp_raw = b"HTTP/2.0 200 OK\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}";
        let (resp_off, resp_headers) = parse_http_head_info(resp_raw).unwrap();
        assert_eq!(&resp_raw[resp_off..], b"{}");
        assert_eq!(resp_headers.len(), 1);
        assert_eq!(
            extract_http_header_charset(resp_raw).as_deref(),
            Some("utf-8")
        );
    }

    #[test]
    fn parse_http_header_spans_and_filter_http_headers_bytes_preserve_line_map() {
        let raw = b"POST /api HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\nX-Keep: 1\r\n\r\nbody";
        let header_start = raw.windows(2).position(|w| w == b"\r\n").unwrap() + 2;
        let (rel_end, spans) = parse_http_header_spans(&raw[header_start..]).unwrap();
        assert_eq!(&raw[header_start + rel_end..], b"body");
        assert_eq!(spans.len(), 3);
        assert_eq!(&raw[header_start..][spans[0].name.clone()], b"Host");
        assert_eq!(&raw[header_start..][spans[0].value.clone()], b"example.com");

        let (filtered, line_map) =
            filter_http_headers_bytes(raw, None, &["user-agent"], &[]).unwrap();
        assert_eq!(
            std::str::from_utf8(&filtered).unwrap(),
            "POST /api HTTP/1.1\r\nHost: example.com\r\nX-Keep: 1\r\n\r\nbody"
        );
        assert_eq!(line_map, vec![1, 2, 4, 5, 6]);
    }

    #[test]
    fn parse_http_response_parts_falls_back_cleanly() {
        let non_std = b"HTTP/1.1 200 Custom Reason\r\nContent-Type: text/html; charset=latin1\r\nLocation: /next\r\n\r\n<h1>ok</h1>";
        let parsed = parse_http_response_parts(non_std);
        assert_eq!(parsed.status_code, Some(200));
        assert_eq!(parsed.content_type.as_deref(), Some("text/html; charset=latin1"));
        assert_eq!(parsed.location.as_deref(), Some("/next"));
        assert_eq!(parsed.body, b"<h1>ok</h1>");

        let invalid = b"not an http message";
        let fallback = parse_http_response_parts(invalid);
        assert_eq!(fallback.status_code, None);
        assert_eq!(fallback.body, invalid);
    }
}
