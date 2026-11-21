use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use tokio_util::sync::CancellationToken;

use super::*;

pub(super) fn extension_values(filter: &HttpExtensionFilter) -> Result<(i64, Vec<String>)> {
    let (mode, value) = match filter {
        HttpExtensionFilter::Any => return Ok((0, Vec::new())),
        HttpExtensionFilter::Only(value) => (1, value),
        HttpExtensionFilter::Hide(value) => (2, value),
    };
    ensure!(value.len() <= 4096, "扩展名筛选超过 4096 字节");
    let mut extensions = Vec::new();
    for extension in value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        let extension = extension.strip_prefix('.').unwrap_or(extension);
        ensure!(
            !extension.is_empty()
                && extension.is_ascii()
                && extension
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
            "扩展名筛选包含无效条目"
        );
        if !extensions
            .iter()
            .any(|entry: &String| entry.eq_ignore_ascii_case(extension))
        {
            ensure!(extensions.len() < 256, "扩展名筛选超过 256 项");
            extensions.push(extension.to_ascii_lowercase());
        }
    }
    Ok((mode, extensions))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryMime {
    Html,
    Script,
    Xml,
    Css,
    OtherText,
    Images,
    Flash,
    OtherBinary,
}

impl HistoryMime {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Html => 0,
            Self::Script => 1,
            Self::Xml => 2,
            Self::Css => 3,
            Self::OtherText => 4,
            Self::Images => 5,
            Self::Flash => 6,
            Self::OtherBinary => 7,
        }
    }
}

pub(crate) fn infer_history_mime(headers: &[Header], body: &[u8]) -> HistoryMime {
    if let Some(content_type) = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
    {
        let essence = String::from_utf8_lossy(&content_type.value)
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if let Some(mime) = history_mime_from_content_type(&essence) {
            return mime;
        }
    }
    if body.is_empty() {
        return HistoryMime::OtherBinary;
    }
    sniff_history_mime(body).unwrap_or(HistoryMime::OtherBinary)
}

pub(super) fn history_mime_from_live(
    connection: &Connection,
    id: &Uuid,
    pending: Option<&HttpHistoryPending>,
    cancel: &CancellationToken,
) -> Result<HistoryMime> {
    ensure!(!cancel.is_cancelled(), "HTTP MIME 筛选已取消");
    type RawMessageRow = (String, Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>);
    let id = id.to_string();
    let row: Option<RawMessageRow> = connection
        .query_row(
            "SELECT typeof(head),
                CASE WHEN typeof(body)='blob' THEN length(body) END,
                CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END,
                CASE WHEN typeof(body)='blob' AND length(body)<=?3 THEN body END
             FROM messages WHERE flow=?1 AND side IN (3, 1) ORDER BY side DESC LIMIT 1",
            params![
                &id,
                crate::store::MAX_MESSAGE_HEAD_BYTES as i64,
                MAX_BODY as i64
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((kind, length, head, body)) = row else {
        if let Some(response) = pending.and_then(|pending| {
            pending
                .first_response
                .as_ref()
                .or(pending.response.as_ref())
        }) {
            return Ok(infer_history_mime(&response.headers, &response.body));
        }
        return Ok(HistoryMime::OtherBinary);
    };
    ensure!(kind == "blob", "HTTP 历史响应头不是二进制");
    let head = head.context("HTTP 历史响应头超过限额")?;
    let head: crate::store::OwnedResponseHead = serde_json::from_slice(&head)
        .map_err(|_| anyhow::anyhow!("HTTP 历史原始响应头结构无效"))?;
    if let Some(stream) = &head.stream {
        stream.validate()?;
    }
    let body = match length {
        None => Vec::new(),
        Some(length) if length <= MAX_BODY as i64 => body.context("HTTP 历史响应正文缺失")?,
        Some(_) => bail!("HTTP MIME 筛选响应正文超过 16 MiB；未返回部分结果"),
    };
    Ok(infer_history_mime(&head.headers, &body))
}

pub(super) fn history_mime_from_archive(
    connection: &Connection,
    row: &ExtHistoryRow,
    chunk_cache: &mut super::search::ChunkCache,
    cancel: &CancellationToken,
) -> Result<HistoryMime> {
    ensure!(!cancel.is_cancelled(), "HTTP MIME 筛选已取消");
    let Some(message) = row.response.as_ref() else {
        return Ok(HistoryMime::OtherBinary);
    };
    let raw =
        super::search::read_archive_message_bytes_cached(connection, message, chunk_cache, cancel)?;
    let Ok((_, headers, body)) = crate::http::head(&raw) else {
        bail!("HTTP 归档原始响应头无效；无法分类 MIME")
    };
    if body.len() > MAX_BODY {
        bail!("HTTP MIME 筛选归档响应正文超过 16 MiB；未返回部分结果")
    }
    Ok(infer_history_mime(&headers, body))
}

fn history_mime_from_content_type(essence: &str) -> Option<HistoryMime> {
    let value = essence.strip_suffix('+').unwrap_or(essence);
    match value {
        "text/html" | "application/xhtml+xml" => Some(HistoryMime::Html),
        "application/javascript"
        | "text/javascript"
        | "application/ecmascript"
        | "text/ecmascript" => Some(HistoryMime::Script),
        "application/xml" | "text/xml" => Some(HistoryMime::Xml),
        "text/css" => Some(HistoryMime::Css),
        "image/png" | "image/gif" | "image/jpeg" | "image/bmp" | "image/webp" | "image/x-icon"
        | "image/svg+xml" => Some(HistoryMime::Images),
        "application/x-shockwave-flash" | "application/futuresplash" => Some(HistoryMime::Flash),
        _ if value.starts_with("text/") || value.ends_with("+xml") || value.ends_with("+json") => {
            Some(HistoryMime::OtherText)
        }
        _ => None,
    }
}

fn sniff_history_mime(body: &[u8]) -> Option<HistoryMime> {
    const MAGIC: &[(&[u8], HistoryMime)] = &[
        (b"%PDF-", HistoryMime::OtherBinary),
        (b"\x89PNG\r\n\x1a\n", HistoryMime::Images),
        (b"GIF89a", HistoryMime::Images),
        (b"GIF87a", HistoryMime::Images),
        (b"\xff\xd8\xff", HistoryMime::Images),
        (b"BM", HistoryMime::Images),
        (b"FWS", HistoryMime::Flash),
        (b"CWS", HistoryMime::Flash),
        (b"ZWS", HistoryMime::Flash),
        (b"PK\x03\x04", HistoryMime::OtherBinary),
        (b"OggS", HistoryMime::OtherBinary),
        (b"RIFF", HistoryMime::OtherBinary),
        (b"{\n", HistoryMime::OtherText),
        (b"{\"", HistoryMime::OtherText),
        (b"<?xml", HistoryMime::Xml),
        (b"<!DOCTYPE html", HistoryMime::Html),
        (b"<html", HistoryMime::Html),
    ];
    for (prefix, mime) in MAGIC {
        if body.starts_with(prefix) {
            return Some(*mime);
        }
    }
    std::str::from_utf8(body)
        .is_ok()
        .then_some(HistoryMime::OtherText)
}

pub(super) fn search_matcher(filter: &HttpHistoryFilter) -> Result<Option<regex::bytes::Regex>> {
    if filter.search_term.is_empty() {
        return Ok(None);
    }
    let pattern = if filter.regex {
        filter.search_term.clone()
    } else {
        regex::escape(&filter.search_term)
    };
    let matcher = regex::bytes::RegexBuilder::new(&pattern)
        .unicode(false)
        .case_insensitive(!filter.case_sensitive)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .nest_limit(128)
        .build()
        .map_err(|error| {
            anyhow::anyhow!(
                "HTTP 正则表达式无效或超出限制；当前不支持 Java lookaround / backreference 等语法：{error}"
            )
        })?;
    Ok(Some(matcher))
}

pub(crate) fn url_has_parameters(url: &str, cancel: &CancellationToken) -> Result<bool> {
    let Some((_, query)) = url.split('#').next().unwrap_or_default().split_once('?') else {
        return Ok(false);
    };
    if query.is_empty() {
        return Ok(false);
    }
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    history_form_has_parameters(query.as_bytes())
}

#[cfg(test)]
pub(in crate::store) fn parsed_parameters(raw: &[u8], cancel: &CancellationToken) -> Result<bool> {
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    ensure!(raw.len() <= crate::intruder::MAX_AUTO_BYTES,
        crate::intruder::AutomaticParameterLimit("HTTP 参数分析最多解析 4 MiB"));
    let mut slots = [httparse::EMPTY_HEADER; 1024];
    let mut request = httparse::Request::new(&mut slots);
    let httparse::Status::Complete(offset) = request.parse(raw)? else {
        anyhow::bail!("HTTP 参数分析请求头未完整接收");
    };
    ensure!(offset <= crate::http::MAX_HEAD + 4,
        crate::intruder::AutomaticParameterLimit("HTTP 参数分析请求头超过 128 KiB"));
    if url_has_parameters(request.path.context("HTTP 参数分析缺少请求目标")?, cancel)? {
        return Ok(true);
    }
    let headers = request.headers.iter().map(|header| crate::Header {
        name: header.name.to_owned(), value: header.value.to_vec(),
    }).collect::<Vec<_>>();
    history_body_has_parameters(&headers, &raw[offset..], cancel)
}

fn history_form_has_parameters(bytes: &[u8]) -> Result<bool> {
    ensure!(bytes.len() <= crate::intruder::MAX_AUTO_BYTES,
        crate::intruder::AutomaticParameterLimit("HTTP 参数分析最多解析 4 MiB"));
    let count = form_urlencoded::parse(bytes).count();
    ensure!(count <= crate::intruder::MAX_POSITIONS,
        crate::intruder::AutomaticParameterLimit("HTTP 参数分析超过 256 个参数"));
    Ok(count > 0)
}

pub(crate) fn history_body_has_parameters(
    headers: &[crate::Header],
    body: &[u8],
    cancel: &CancellationToken,
) -> Result<bool> {
    use mime::Mime;
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    ensure!(body.len() <= crate::intruder::MAX_AUTO_BYTES,
        crate::intruder::AutomaticParameterLimit("HTTP 参数分析最多解析 4 MiB"));
    if body.is_empty() {
        return Ok(false);
    }
    let mut content_type: Option<Mime> = None;
    for header in headers.iter().filter(|header| header.name.eq_ignore_ascii_case("content-type")) {
        let value: Mime = std::str::from_utf8(&header.value)
            .context("HTTP 参数分析 Content-Type 需要有效 UTF-8")?
            .parse().context("HTTP 参数分析 Content-Type 无效")?;
        ensure!(content_type.as_ref().is_none_or(|previous| previous == &value),
            "HTTP 参数分析 Content-Type 存在冲突");
        content_type = Some(value);
    }
    let content = body.trim_ascii_start().strip_prefix(b"\xef\xbb\xbf").unwrap_or(body.trim_ascii_start()).trim_ascii();
    let essence = content_type.as_ref().map(Mime::essence_str);
    let structured_charset = || -> Result<()> {
        ensure!(content_type.as_ref().and_then(|value| value.get_param(mime::CHARSET))
            .is_none_or(|charset| charset == mime::UTF_8), "HTTP 参数分析 JSON/XML charset 必须是 UTF-8");
        Ok(())
    };
    let result = match essence {
        Some("application/x-www-form-urlencoded") => history_form_has_parameters(body),
        Some("multipart/form-data") => {
            let boundary = content_type.as_ref().and_then(|value| value.get_param(mime::BOUNDARY))
                .context("HTTP multipart 缺少 boundary")?;
            history_multipart_has_parameters(body, boundary.as_str(), cancel)
        }
        Some("application/json") | Some("text/json") => {
            structured_charset()?;
            if !matches!(content.first(), Some(b'{' | b'[')) { return Ok(false); }
            history_json_has_parameters(content, cancel)
        }
        Some(value) if value.ends_with("+json") => {
            structured_charset()?;
            if !matches!(content.first(), Some(b'{' | b'[')) { return Ok(false); }
            history_json_has_parameters(content, cancel)
        }
        Some("application/xml" | "text/xml") => {
            structured_charset()?;
            history_xml_has_parameters(content, cancel)
        }
        Some(value) if value.ends_with("+xml") => {
            structured_charset()?;
            history_xml_has_parameters(content, cancel)
        }
        _ => match content.first() {
            Some(b'{' | b'[') => history_json_has_parameters(content, cancel),
            Some(b'<') => history_xml_has_parameters(content, cancel),
            _ => history_form_has_parameters(body),
        },
    };
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    result.context("HTTP 参数分析无法完整解析请求")
}

fn history_json_has_parameters(body: &[u8], cancel: &CancellationToken) -> Result<bool> {
    let value: serde_json::Value = serde_json::from_slice(body).context("HTTP 参数 JSON 解析失败")?;
    let mut remaining = vec![(&value, 0usize)];
    let mut nodes = 0usize;
    let mut parameters = 0usize;
    while let Some((value, depth)) = remaining.pop() {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        nodes += 1;
        ensure!(depth <= 64 && nodes <= 8192,
            crate::intruder::AutomaticParameterLimit("HTTP 参数 JSON 超过深度 64 或节点 8192 限额"));
        match value {
            serde_json::Value::Object(fields) => remaining.extend(fields.values().map(|value| (value, depth + 1))),
            serde_json::Value::Array(fields) => remaining.extend(fields.iter().map(|value| (value, depth + 1))),
            _ => parameters += 1,
        }
        ensure!(parameters <= crate::intruder::MAX_POSITIONS,
            crate::intruder::AutomaticParameterLimit("HTTP 参数分析超过 256 个参数"));
    }
    Ok(parameters > 0)
}

fn history_xml_has_parameters(body: &[u8], cancel: &CancellationToken) -> Result<bool> {
    let text = std::str::from_utf8(body).context("HTTP 参数 XML 需要有效 UTF-8")?;
    let document = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions {
        nodes_limit: 8192, ..Default::default()
    }).context("HTTP 参数 XML 解析失败")?;
    let mut parameters = 0usize;
    for node in document.descendants() {
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        ensure!(node.ancestors().count() <= 65,
            crate::intruder::AutomaticParameterLimit("HTTP 参数 XML 超过深度 64 限额"));
        parameters += node.attributes().len() + usize::from(node.is_text());
        ensure!(parameters <= crate::intruder::MAX_POSITIONS,
            crate::intruder::AutomaticParameterLimit("HTTP 参数分析超过 256 个参数"));
    }
    Ok(parameters > 0)
}

fn history_multipart_has_parameters(body: &[u8], boundary: &str, cancel: &CancellationToken) -> Result<bool> {
    use futures_util::{FutureExt, stream};
    let bytes = multer::bytes::Bytes::copy_from_slice(body);
    let stream = stream::iter([Ok::<_, std::convert::Infallible>(bytes)]);
    let limits = multer::Constraints::new().size_limit(multer::SizeLimit::new()
        .whole_stream(crate::intruder::MAX_AUTO_BYTES as u64)
        .per_field(crate::intruder::MAX_AUTO_BYTES as u64));
    let mut multipart = multer::Multipart::with_constraints(stream, boundary, limits);
    let analysis = async {
        let mut parameters = 0usize;
        while let Some(mut field) = multipart.next_field().await? {
            ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
            field.name().context("HTTP multipart 字段缺少名称")?;
            parameters += 1;
            ensure!(parameters <= crate::intruder::MAX_POSITIONS,
                crate::intruder::AutomaticParameterLimit("HTTP 参数分析超过 256 个参数"));
            while field.chunk().await?.is_some() {
                ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
            }
        }
        Ok(parameters > 0)
    };
    analysis.now_or_never().context("HTTP 内存 multipart 解析未同步完成")?
}

pub(super) fn parameterized_live_request(
    connection: &Connection,
    id: Uuid,
    pending: Option<&HttpHistoryPending>,
    cancel: &CancellationToken,
) -> Result<bool> {
    let id = id.to_string();
    let head: Option<(u8, Option<Vec<u8>>)> = connection
        .query_row(
            "SELECT side,CASE WHEN typeof(head)='blob' AND length(head)<=?2 THEN head END
         FROM messages WHERE flow=?1 AND side IN (0,2) ORDER BY side DESC LIMIT 1",
            params![&id, crate::store::MAX_MESSAGE_HEAD_BYTES as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((side, head)) = head {
        let head: crate::store::OwnedRequestHead =
            serde_json::from_slice(&head.context("HTTP 历史原始请求头损坏或超过 4 MiB 限额")?)?;
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        let (kind, length, body): (String, Option<usize>, Option<Vec<u8>>) = connection
            .query_row(
                "SELECT typeof(body),length(body),CASE WHEN typeof(body)='blob' AND length(body)<=?2 THEN body END
             FROM messages WHERE flow=?1 AND side=?3",
                params![&id, crate::intruder::MAX_AUTO_BYTES as i64, side],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        ensure!(kind == "blob", "HTTP 原始请求正文不是二进制");
        ensure!(
            length.is_some_and(|length| length <= crate::intruder::MAX_AUTO_BYTES),
            crate::intruder::AutomaticParameterLimit("Auto § 最多解析 4 MiB（MIO 资源限额）")
        );
        let body = body.context("HTTP 原始请求正文缺失")?;
        history_body_has_parameters(&head.headers, &body, cancel)
    } else {
        let pending = pending.context("HTTP 参数筛选缺少原始请求；未使用编辑版替代")?;
        let request = pending
            .first_request
            .as_ref()
            .context("HTTP 参数筛选缺少原始请求；未使用编辑版替代")?;
        ensure!(
            request.method == pending.first_request_method
                && request.url == pending.first_request_url,
            "HTTP 参数筛选原始请求与摘要不一致"
        );
        history_body_has_parameters(&request.headers, &request.body, cancel)
    }
}

#[cfg(test)]
pub(super) fn parameterized_archive_request(
    connection: &Connection,
    message: &ExtMessageSummary,
    cancel: &CancellationToken,
) -> Result<bool> {
    let mut cache = super::search::ChunkCache::default();
    parameterized_archive_request_cached(connection, message, &mut cache, cancel)
}

pub(super) fn parameterized_archive_request_cached(
    connection: &Connection,
    message: &ExtMessageSummary,
    chunk_cache: &mut super::search::ChunkCache,
    cancel: &CancellationToken,
) -> Result<bool> {
    ensure!(
        message
            .body_offset
            .is_none_or(|offset| offset <= (crate::http::MAX_HEAD + 4) as u64),
        crate::intruder::AutomaticParameterLimit("Auto § header 超过 128 KiB")
    );
    let length = message
        .body_offset
        .unwrap_or(message.raw_bytes)
        .min((crate::http::MAX_HEAD + 4) as u64);
    let head = super::search::read_archive_message_prefix_cached(
        connection,
        message,
        length,
        chunk_cache,
        cancel,
    )?;
    let Ok((_, headers, rest)) = crate::http::head(&head) else {
        ensure!(
            head.len() < crate::http::MAX_HEAD + 4,
            crate::intruder::AutomaticParameterLimit("Auto § header 超过 128 KiB")
        );
        bail!("HTTP 归档原始请求头无效，参数状态未知");
    };
    let body_offset = (head.len() - rest.len()) as u64;
    ensure!(
        message
            .body_offset
            .is_none_or(|offset| offset == body_offset),
        "HTTP 归档原始请求正文偏移与消息不一致"
    );
    if body_offset == message.raw_bytes {
        return Ok(false);
    }
    ensure!(
        message.raw_bytes - body_offset <= crate::intruder::MAX_AUTO_BYTES as u64,
        crate::intruder::AutomaticParameterLimit("Auto § 最多解析 4 MiB（MIO 资源限额）")
    );
    let bytes = super::search::read_archive_message_prefix_cached(
        connection,
        message,
        message.raw_bytes,
        chunk_cache,
        cancel,
    )?;
    if headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
    {
        ensure!(
            bytes.len() <= crate::intruder::MAX_AUTO_BYTES,
            crate::intruder::AutomaticParameterLimit("Auto § 最多解析 4 MiB（MIO 资源限额）")
        );
        let request = crate::parse_request(&bytes, "http://history.invalid", message.declared_version.context("HTTP 归档原始请求协议未知")?)?;
        ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
        return history_body_has_parameters(&request.headers, &request.body, cancel);
    }
    history_body_has_parameters(&headers, &bytes[body_offset as usize..], cancel)
}
