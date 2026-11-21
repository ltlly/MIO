use anyhow::{Context, Result, ensure};
use mio_plugin_api::{FlowRecord, Header, HttpVersion};
use serde_json::{Map, Value};

#[cfg(test)]
mod tests;

const MAX_RECORDS: usize = 500;
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn export_flows_to_har(records: &[FlowRecord]) -> Result<Vec<u8>> {
    ensure!(
        records.len() <= MAX_RECORDS,
        "HAR 导出最多支持 {MAX_RECORDS} 条记录；当前有 {} 条，未导出",
        records.len()
    );
    let mut raw_budget = 0usize;
    for record in records {
        raw_budget = raw_budget
            .checked_add(estimated_record_bytes(record))
            .context("HAR 导出总字节数溢出")?;
        ensure!(
            raw_budget <= MAX_OUTPUT_BYTES,
            "HAR 导出结果超过 64 MiB 上限；未导出"
        );
    }
    let entries = records.iter().map(har_entry).collect::<Result<Vec<_>>>()?;
    let mut log = Map::new();
    log.insert("version".into(), Value::String("1.2".into()));
    log.insert("creator".into(), creator());
    log.insert("entries".into(), Value::Array(entries));
    let mut root = Map::new();
    root.insert("log".into(), Value::Object(log));
    let mut writer = BoundedWriter::new(MAX_OUTPUT_BYTES);
    serde_json::to_writer_pretty(&mut writer, &Value::Object(root)).map_err(|error| {
        if writer.exceeded {
            anyhow::anyhow!("HAR 导出结果超过 64 MiB 上限；未导出")
        } else {
            error.into()
        }
    })?;
    Ok(writer.buf)
}

struct BoundedWriter {
    buf: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedWriter {
    fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl std::io::Write for BoundedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let Some(next_len) = self.buf.len().checked_add(data.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::other("HAR output limit exceeded"));
        };
        if next_len > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("HAR output limit exceeded"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn estimated_record_bytes(record: &FlowRecord) -> usize {
    let request_body = encoded_body_len(&record.request.body);
    let response_body = record
        .response
        .as_ref()
        .map_or(0, |response| encoded_body_len(&response.body));
    request_body
        .saturating_add(response_body)
        .saturating_add(256)
}

fn encoded_body_len(body: &[u8]) -> usize {
    if !body.contains(&0) && std::str::from_utf8(body).is_ok() {
        body.len()
    } else {
        body.len().div_ceil(3).saturating_mul(4)
    }
}

fn creator() -> Value {
    let mut creator = Map::new();
    creator.insert("name".into(), Value::String("MIO".into()));
    creator.insert("version".into(), Value::String(env!("CARGO_PKG_VERSION").into()));
    Value::Object(creator)
}

fn har_entry(record: &FlowRecord) -> Result<Value> {
    let started = Value::String(started_date_time(record.summary.started_unix_ms)?);
    let elapsed = Value::Number(serde_json::Number::from(record.summary.elapsed_ms));
    let request = har_request(&record.request);
    let response = record
        .response
        .as_ref()
        .map(har_response)
        .unwrap_or_else(empty_har_response);
    let mut timings = Map::new();
    timings.insert("send".into(), Value::Number(0.into()));
    timings.insert(
        "wait".into(),
        Value::Number(serde_json::Number::from(record.summary.elapsed_ms)),
    );
    timings.insert("receive".into(), Value::Number(0.into()));
    let mut entry = Map::new();
    entry.insert("startedDateTime".into(), started);
    entry.insert("time".into(), elapsed);
    entry.insert("request".into(), request);
    entry.insert("response".into(), response);
    entry.insert("cache".into(), Value::Object(Map::new()));
    entry.insert("timings".into(), Value::Object(timings));
    Ok(Value::Object(entry))
}

fn har_request(request: &mio_plugin_api::HttpRequest) -> Value {
    let mut entry = Map::new();
    entry.insert("method".into(), Value::String(request.method.clone()));
    entry.insert("url".into(), Value::String(request.url.clone()));
    entry.insert(
        "httpVersion".into(),
        Value::String(http_version(request.version).into()),
    );
    entry.insert("cookies".into(), request_cookies(&request.headers));
    entry.insert("headers".into(), har_headers(&request.headers));
    entry.insert(
        "queryString".into(),
        Value::Array(query_string(&request.url)),
    );
    if !request.body.is_empty() {
        entry.insert("postData".into(), har_post_data(request));
    }
    entry.insert("headersSize".into(), Value::Number((-1).into()));
    entry.insert("bodySize".into(), body_size(request.body.len()));
    Value::Object(entry)
}

fn har_response(response: &mio_plugin_api::HttpResponse) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "status".into(),
        Value::Number(serde_json::Number::from(response.status)),
    );
    entry.insert(
        "statusText".into(),
        Value::String(status_text(response.status).into()),
    );
    entry.insert(
        "httpVersion".into(),
        Value::String(http_version(response.version).into()),
    );
    entry.insert("cookies".into(), Value::Array(Vec::new()));
    entry.insert("headers".into(), har_headers(&response.headers));
    entry.insert("content".into(), har_content(response));
    entry.insert(
        "redirectURL".into(),
        Value::String(redirect_url(&response.headers)),
    );
    entry.insert("headersSize".into(), Value::Number((-1).into()));
    entry.insert("bodySize".into(), body_size(response.body.len()));
    Value::Object(entry)
}

fn empty_har_response() -> Value {
    let mut content = Map::new();
    content.insert("size".into(), Value::Number(0.into()));
    content.insert("mimeType".into(), Value::String("x-unknown".into()));
    let mut entry = Map::new();
    entry.insert("status".into(), Value::Number(0.into()));
    entry.insert("statusText".into(), Value::String(String::new()));
    entry.insert("httpVersion".into(), Value::String(String::new()));
    entry.insert("cookies".into(), Value::Array(Vec::new()));
    entry.insert("headers".into(), Value::Array(Vec::new()));
    entry.insert("content".into(), Value::Object(content));
    entry.insert("redirectURL".into(), Value::String(String::new()));
    entry.insert("headersSize".into(), Value::Number((-1).into()));
    entry.insert("bodySize".into(), Value::Number((-1).into()));
    Value::Object(entry)
}

fn http_version(version: HttpVersion) -> &'static str {
    match version {
        HttpVersion::Http10 => "HTTP/1.0",
        HttpVersion::Http11 => "HTTP/1.1",
        HttpVersion::Http2 => "HTTP/2.0",
    }
}

fn har_headers(headers: &[Header]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|header| {
                let mut entry = Map::new();
                entry.insert("name".into(), Value::String(header.name.clone()));
                entry.insert("value".into(), Value::String(header_text(&header.value)));
                Value::Object(entry)
            })
            .collect(),
    )
}

fn request_cookies(headers: &[Header]) -> Value {
    let mut cookies = Vec::new();
    for header in headers {
        if !header.name.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let raw = header_text(&header.value);
        for part in raw
            .split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            let mut item = Map::new();
            item.insert("name".into(), Value::String(name.to_owned()));
            item.insert("value".into(), Value::String(value.to_owned()));
            cookies.push(Value::Object(item));
        }
    }
    Value::Array(cookies)
}

fn query_string(url: &str) -> Vec<Value> {
    url.rsplit_once('#')
        .map_or(url, |(without_fragment, _)| without_fragment)
        .split_once('?')
        .map(|(_, query)| query)
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = part_decode(pair);
            let mut item = Map::new();
            item.insert("name".into(), Value::String(name));
            item.insert("value".into(), Value::String(value));
            Value::Object(item)
        })
        .collect()
}

fn part_decode(pair: &str) -> (String, String) {
    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
    (percent_decode_query(name), percent_decode_query(value))
}

fn percent_decode_query(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'+' {
            decoded.push(b' ');
            index += 1;
        } else if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                crate::codec_util::hex_nibble(bytes[index + 1]),
                crate::codec_util::hex_nibble(bytes[index + 2]),
            )
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| input.to_owned())
}

fn body_size(size: usize) -> Value {
    Value::Number(serde_json::Number::from(
        u64::try_from(size).unwrap_or(u64::MAX),
    ))
}

fn har_post_data(request: &mio_plugin_api::HttpRequest) -> Value {
    let mut post = Map::new();
    post.insert(
        "mimeType".into(),
        Value::String(header_mime_type(
            &request.headers,
            "application/octet-stream",
        )),
    );
    insert_body_text(&mut post, &request.body);
    Value::Object(post)
}

fn har_content(response: &mio_plugin_api::HttpResponse) -> Value {
    let mut entry = Map::new();
    entry.insert("size".into(), body_size(response.body.len()));
    entry.insert(
        "mimeType".into(),
        Value::String(header_mime_type(
            &response.headers,
            "application/octet-stream",
        )),
    );
    if !response.body.is_empty() {
        insert_body_text(&mut entry, &response.body);
    }
    Value::Object(entry)
}

fn insert_body_text(target: &mut Map<String, Value>, body: &[u8]) {
    if let Ok(text) = std::str::from_utf8(body)
        && !body.contains(&0)
    {
        target.insert("text".into(), Value::String(text.to_owned()));
    } else {
        target.insert("encoding".into(), Value::String("base64".into()));
        target.insert(
            "text".into(),
            Value::String(crate::codec_util::base64_encode(body)),
        );
    }
}

fn header_mime_type(headers: &[Header], fallback: &str) -> String {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header_text(&header.value))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn redirect_url(headers: &[Header]) -> String {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("location"))
        .map(|header| header_text(&header.value))
        .unwrap_or_default()
}

fn status_text(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

fn started_date_time(unix_ms: i64) -> Result<String> {
    Ok(crate::codec_util::format_utc_iso8601(unix_ms))
}

fn header_text(value: &[u8]) -> String {
    match std::str::from_utf8(value) {
        Ok(text) => text.to_owned(),
        Err(_) => value.iter().map(|&byte| char::from(byte)).collect(),
    }
}
