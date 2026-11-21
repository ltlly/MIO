use super::*;

pub(in crate::store) const HTTP_HISTORY_META_PREFIX: &str = "http_history_meta:";
pub(in crate::store) const NEXT_PROXY_CAPTURE_NUMBER_KEY: &str = "next_proxy_capture_number";
pub(in crate::store) const MAX_HTTP_HISTORY_META_BYTES: usize = 256 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct StoredHttpHistoryMetadata {
    pub flow_id: FlowId,
    pub revision: u64,
    pub metadata: crate::HttpHistoryRowMetadata,
}

pub(in crate::store) fn read_http_history_metadata(
    connection: &Connection,
    flow_id: FlowId,
) -> Result<Option<StoredHttpHistoryMetadata>> {
    let bytes: Option<Option<Vec<u8>>> = connection.query_row(
        "SELECT CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
         FROM main.metadata WHERE key=?1 COLLATE BINARY",
        params![format!("{HTTP_HISTORY_META_PREFIX}{flow_id}"), MAX_HTTP_HISTORY_META_BYTES],
        |row| row.get(0),
    ).optional()?;
    bytes.map(|bytes| {
        let bytes = bytes.context("HTTP 历史元数据类型无效或超过 256 KiB")?;
        let stored: StoredHttpHistoryMetadata = serde_json::from_slice(&bytes)?;
        validate_http_history_metadata(&stored, flow_id)?;
        Ok(stored)
    }).transpose()
}

fn validate_http_history_metadata(stored: &StoredHttpHistoryMetadata, flow_id: FlowId) -> Result<()> {
    ensure!(!flow_id.is_nil() && stored.flow_id == flow_id && stored.revision > 0, "HTTP 历史元数据身份或 revision 无效");
    let metadata = &stored.metadata;
    ensure!(metadata.listener_port != Some(0), "HTTP 历史监听端口无效");
    ensure!(metadata.capture_number != Some(0), "HTTP 历史捕获序号不能为 0");
    ensure!(metadata.remote_ip.is_empty() || metadata.remote_ip.parse::<std::net::IpAddr>().is_ok(), "HTTP 历史连接 IP 无效");
    ensure!(metadata.params_error.is_none() || (!metadata.params_known && !metadata.has_params), "HTTP 参数分析状态与诊断冲突");
    ensure!(metadata.title_error.is_none() || metadata.title.is_empty(), "HTTP 标题分析状态与诊断冲突");
    ensure!(metadata.cookies_error.is_none() || metadata.cookies.is_empty(), "HTTP Cookie 分析状态与诊断冲突");
    ensure!([&metadata.params_error, &metadata.title_error, &metadata.cookies_error].iter().all(|error| error.as_ref().is_none_or(|error| !error.is_empty() && error.len() <= 4096)), "HTTP 分析诊断为空或超过 4096 字节");
    Ok(())
}

pub(in crate::store) fn read_next_proxy_capture_number(connection: &Connection) -> Result<Option<u64>> {
    let bytes: Option<Option<Vec<u8>>> = connection
        .query_row(
            "SELECT CASE WHEN typeof(value)='blob' AND length(value)<=64 THEN value END
             FROM main.metadata WHERE key=?1 COLLATE BINARY",
            [NEXT_PROXY_CAPTURE_NUMBER_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let bytes = bytes.context("HTTP 历史捕获序号计数器类型无效或超过 64 字节")?;
    let next: u64 = serde_json::from_slice(&bytes).context("HTTP 历史捕获序号计数器 JSON 损坏")?;
    ensure!(
        next > 0 && i64::try_from(next).is_ok(),
        "HTTP 历史捕获序号计数器无效或溢出"
    );
    Ok(Some(next))
}

pub(in crate::store) fn write_next_proxy_capture_number(
    tx: &rusqlite::Transaction<'_>,
    next: u64,
) -> Result<()> {
    ensure!(
        next > 0 && i64::try_from(next).is_ok(),
        "HTTP 历史捕获序号计数器无效或溢出"
    );
    let bytes = serde_json::to_vec(&next)?;
    tx.execute(
        "INSERT INTO main.metadata(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![NEXT_PROXY_CAPTURE_NUMBER_KEY, bytes],
    )?;
    Ok(())
}

fn existing_proxy_capture_number(
    tx: &rusqlite::Transaction<'_>,
    flow_id: FlowId,
    stored: Option<&StoredHttpHistoryMetadata>,
) -> Result<Option<u64>> {
    if let Some(num) = stored.and_then(|s| s.metadata.capture_number).filter(|&n| n > 0) {
        return Ok(Some(num));
    }
    let id = flow_id.to_string();
    for side in [2u8, 0u8] {
        if let Some((head_bytes, _)) = read_message(tx, &id, side)? {
            let head: OwnedRequestHead = serde_json::from_slice(&head_bytes)?;
            if let Some(num) = head.capture_number.filter(|&n| n > 0) {
                return Ok(Some(num));
            }
        }
    }
    Ok(None)
}

fn allocate_proxy_capture_number(tx: &rusqlite::Transaction<'_>) -> Result<u64> {
    if let Some(assigned) = read_next_proxy_capture_number(tx)? {
        let next = assigned.checked_add(1).context("HTTP 历史捕获序号已耗尽")?;
        write_next_proxy_capture_number(tx, next)?;
        return Ok(assigned);
    }
    let has_ext_items: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name='ext_items')",
        [],
        |row| row.get(0),
    )?;
    let max_ext_item: i64 = if has_ext_items {
        tx.query_row(
            "SELECT coalesce((SELECT max(item_index) + 1 FROM main.ext_items WHERE collection IN ('root.f1.f0','root.f1.f1')), 0)",
            [],
            |row| row.get(0),
        )?
    } else {
        0
    };
    let max_existing: i64 = tx.query_row(
        "SELECT max(
            ?1,
            coalesce((SELECT max(json_extract(CAST(value AS TEXT), '$.metadata.capture_number')) FROM main.metadata
                WHERE key GLOB 'http_history_meta:*'
                  AND typeof(value)='blob' AND length(value)<=262144
                  AND json_valid(CAST(value AS TEXT))
                  AND json_type(CAST(value AS TEXT), '$.metadata.capture_number')='integer'), 0),
            coalesce((SELECT max(json_extract(CAST(head AS TEXT), '$.capture_number')) FROM main.messages
                WHERE side IN (0, 2)
                  AND typeof(head)='blob' AND length(head)<=4194304
                  AND json_valid(CAST(head AS TEXT))
                  AND json_type(CAST(head AS TEXT), '$.capture_number')='integer'), 0)
        )",
        [max_ext_item],
        |row| row.get(0),
    )?;
    let floor = u64::try_from(max_existing.max(0))?
        .checked_add(1)
        .context("HTTP 历史捕获序号已耗尽")?;
    let next = floor.checked_add(1).context("HTTP 历史捕获序号已耗尽")?;
    write_next_proxy_capture_number(tx, next)?;
    Ok(floor)
}

pub(in crate::store) fn write_http_history_metadata(
    tx: &rusqlite::Transaction<'_>,
    flow_id: FlowId,
    revision: u64,
    metadata: &crate::HttpHistoryRowMetadata,
) -> Result<()> {
    let stored = StoredHttpHistoryMetadata { flow_id, revision, metadata: metadata.clone() };
    validate_http_history_metadata(&stored, flow_id)?;
    let bytes = serde_json::to_vec(&stored)?;
    ensure!(bytes.len() <= MAX_HTTP_HISTORY_META_BYTES, "HTTP 历史元数据超过 256 KiB");
    tx.execute(
        "INSERT INTO main.metadata(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![format!("{HTTP_HISTORY_META_PREFIX}{flow_id}"), bytes],
    )?;
    Ok(())
}

fn parse_cookie_header_str(bytes: &[u8]) -> Result<&str> {
    ensure!(
        bytes.iter().all(|&b| (32..127).contains(&b) || b == b'\t'),
        "HTTP Cookie header 字节无效"
    );
    std::str::from_utf8(bytes).context("HTTP Cookie header 字节无效")
}

fn split_cookie_segments(value: &str) -> impl Iterator<Item = &str> {
    let mut in_quotes = false;
    value
        .split(move |c| {
            if in_quotes {
                if c == '"' {
                    in_quotes = false;
                }
                false
            } else if c == ';' {
                true
            } else {
                if c == '"' {
                    in_quotes = true;
                }
                false
            }
        })
        .map(str::trim)
}

pub(in crate::store) fn history_cookie_names(headers: &[crate::Header], response: bool) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for header in headers.iter().filter(|header| header.name.eq_ignore_ascii_case(if response { "set-cookie" } else { "cookie" })) {
        ensure!(header.value.len() <= crate::http::MAX_HEAD, "HTTP Cookie header 超过 128 KiB");
        let value = parse_cookie_header_str(&header.value)?;
        if response {
            let head = split_cookie_segments(value)
                .next()
                .context("HTTP Set-Cookie 缺少名称和值")?;
            let (name, _) = head
                .split_once('=')
                .context("HTTP Set-Cookie 缺少名称和值")?;
            append_history_cookie_name(&mut names, name.trim())?;
        } else {
            let segments: Vec<&str> = split_cookie_segments(value).collect();
            ensure!(
                segments.iter().all(|segment| segment.contains('=')),
                "HTTP Cookie 包含缺少等号的字段"
            );
            for segment in segments {
                let (name, _) = segment
                    .split_once('=')
                    .context("HTTP Cookie 包含缺少等号的字段")?;
                append_history_cookie_name(&mut names, name.trim())?;
            }
        }
    }
    Ok(names)
}

fn append_history_cookie_name(names: &mut Vec<String>, name: &str) -> Result<()> {
    ensure!(crate::http::token(name.as_bytes()), "HTTP Cookie 名称无效");
    if !names.iter().any(|existing| existing == name) {
        ensure!(names.len() < 1024, "HTTP Cookie 名称超过 1024 项");
        ensure!(names.iter().map(String::len).sum::<usize>() + name.len() <= 64 * 1024, "HTTP Cookie 名称文本超过 64 KiB");
        names.push(name.to_owned());
    }
    Ok(())
}

pub(in crate::store) fn history_analysis_diagnostic(error: &anyhow::Error) -> String {
    let diagnostic = format!("{error:#}");
    diagnostic[..diagnostic.floor_char_boundary(4096)].to_owned()
}

pub(in crate::store) fn history_has_parameters(request: &HttpRequest) -> Result<bool> {
    let cancel = CancellationToken::new();
    if ext_archive::http_query::filters::url_has_parameters(&request.url, &cancel)? {
        return Ok(true);
    }
    ext_archive::http_query::filters::history_body_has_parameters(&request.headers, &request.body, &cancel)
}

static TITLE_SELECTOR: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();

pub(in crate::store) fn response_history_values(response: &HttpResponse, request_method: &str) -> (String, Result<String>) {
    let mime = crate::http::infer_history_display_mime_type(&response.headers, &response.body).to_owned();
    let title = if mime == "html" && !response.body.is_empty() {
        (|| -> Result<String> {
        let cancel = CancellationToken::new();
        let unpacked = crate::http::unpack_response(
            response,
            request_method,
            crate::http::MAX_BODY,
            &cancel,
            std::time::Instant::now() + Duration::from_secs(5),
        )?;
        let body = unpacked.as_ref().map_or(response.body.as_slice(), |response| response.body.as_slice());
        ensure!(body.len() <= 256 * 1024, "HTTP 标题分析正文超过 256 KiB");
        let html = std::str::from_utf8(body).context("HTTP 标题分析正文需要有效 UTF-8")?;
        let document = scraper::Html::parse_document(html);
        ensure!(document.errors.is_empty(), "HTTP 标题 HTML 解析失败：{}", document.errors.iter().map(|error| error.as_ref()).collect::<Vec<_>>().join("；"));
        let selector = TITLE_SELECTOR.get_or_init(|| {
            scraper::Selector::parse("title").expect("title 是有效的静态 CSS selector")
        });
        let Some(title) = document.select(selector).next() else { return Ok(String::new()); };
        let text = title.text().collect::<String>();
        let normalized = text.split_ascii_whitespace().collect::<Vec<_>>().join(" ");
        ensure!(normalized.len() <= 16 * 1024, "HTTP 标题超过 16 KiB");
        Ok(normalized)
        })()
    } else { Ok(String::new()) };
    (mime, title)
}

pub(crate) fn validate_repeater_completion(
    received: &FlowRecord,
    record: &FlowRecord,
) -> Result<()> {
    ensure!(
        received.summary.id == record.summary.id
            && received.summary.source == crate::TrafficSource::Repeater
            && record.summary.source == crate::TrafficSource::Repeater
            && received.request == record.request
            && received.summary.revision.checked_add(1) == Some(record.summary.revision)
            && received.summary.started_unix_ms == record.summary.started_unix_ms
            && received.summary.elapsed_ms <= record.summary.elapsed_ms,
        "Repeater 收到版与解包结果的归属、请求或 revision 不一致"
    );
    for version in [received, record] {
        let response = version
            .response
            .as_ref()
            .context("Repeater 解包结果缺少完整响应")?;
        ensure!(
            version.summary.state == crate::FlowState::Complete
                && version.summary.method == version.request.method
                && version.summary.url == version.request.url
                && version.summary.request_bytes == version.request.body.len() as u64
                && version.summary.status == Some(response.status)
                && version.summary.response_bytes == response.body.len() as u64,
            "Repeater 解包结果或消息摘要不完整"
        );
    }
    let before = received.response.as_ref().unwrap();
    let after = record.response.as_ref().unwrap();
    ensure!(
        before != after && before.status == after.status && before.version == after.version,
        "Repeater 解包不能改变响应状态或协议，也不能生成未变换的版本"
    );
    Ok(())
}

pub(super) fn write_repeater_completion(
    tx: &rusqlite::Transaction<'_>,
    received: &FlowRecord,
    record: &FlowRecord,
) -> Result<()> {
    validate_repeater_completion(received, record)?;
    let current = read_record(tx, record.summary.id)?;
    if let Some(current) = &current {
        ensure!(
            current.request == received.request
                && current.summary.source == received.summary.source
                && current.summary.started_unix_ms == received.summary.started_unix_ms,
            "Repeater 已保存流量与收到版归属或请求冲突"
        );
    }
    if current
        .as_ref()
        .is_none_or(|current| current.summary.revision != record.summary.revision)
    {
        write_record(tx, received)?;
    }
    write_record(tx, record)?;
    let original = read_message(tx, &record.summary.id.to_string(), 3)?
        .map(response_with_stream)
        .transpose()?;
    ensure!(
        original.as_ref().is_some_and(|(response, stream)| {
            Some(response) == received.response.as_ref() && stream.is_none()
        }),
        "Repeater 已保存原始响应与收到版冲突"
    );
    Ok(())
}

pub(in crate::store) fn write_record(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
) -> Result<()> {
    write_record_with_reason_phrase(tx, record, None)
}

pub(crate) fn write_record_with_reason_phrase(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    reason_phrase: Option<&[u8]>,
) -> Result<()> {
    write_capture_with_reason_phrase(tx, record, None, reason_phrase, None)
}

    #[cfg(test)]
pub(super) fn write_capture_with_port(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    response_stream: Option<&StreamedResponse>,
    listener_port: Option<u16>,
) -> Result<()> {
    write_capture_with_reason_phrase(tx, record, response_stream, None, listener_port)
}

pub(in crate::store) fn write_capture_with_metrics(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    response_stream: Option<&StreamedResponse>,
    listener_port: Option<u16>,
    metrics: &mio_proxy::CapturedFlowMetrics,
) -> Result<()> {
    write_capture_with_reason_phrase_and_metrics(
        tx,
        record,
        response_stream,
        metrics.reason_phrase.as_deref(),
        listener_port,
        Some(metrics),
    )
}

pub(in crate::store) fn source_code(source: &crate::TrafficSource) -> &'static str {
    match source {
        crate::TrafficSource::Proxy => "Proxy",
        crate::TrafficSource::Repeater => "Repeater",
        crate::TrafficSource::Intruder => "Intruder",
        crate::TrafficSource::Scanner => "Scanner",
        crate::TrafficSource::Plugin(_) => "Plugin",
    }
}

pub(in crate::store) struct ImportedFlowHeadMeta<'a> {
    pub time_known: bool,
    pub capture_number: u64,
    pub remote_ip: &'a str,
    pub response_received_ms: Option<u64>,
}

pub(in crate::store) fn write_imported_capture(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    reason_phrase: Option<&[u8]>,
    listener_port: Option<u16>,
    imported_meta: &ImportedFlowHeadMeta<'_>,
) -> Result<()> {
    write_capture_inner(
        tx,
        record,
        None,
        reason_phrase,
        listener_port,
        None,
        Some(imported_meta),
    )
}

pub(in crate::store) fn write_capture_with_reason_phrase(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    response_stream: Option<&StreamedResponse>,
    reason_phrase: Option<&[u8]>,
    listener_port: Option<u16>,
) -> Result<()> {
    write_capture_with_reason_phrase_and_metrics(
        tx,
        record,
        response_stream,
        reason_phrase,
        listener_port,
        None,
    )
}

fn write_capture_with_reason_phrase_and_metrics(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    response_stream: Option<&StreamedResponse>,
    reason_phrase: Option<&[u8]>,
    listener_port: Option<u16>,
    metrics: Option<&mio_proxy::CapturedFlowMetrics>,
) -> Result<()> {
    write_capture_inner(
        tx,
        record,
        response_stream,
        reason_phrase,
        listener_port,
        metrics,
        None,
    )
}

fn write_capture_inner(
    tx: &rusqlite::Transaction<'_>,
    record: &FlowRecord,
    response_stream: Option<&StreamedResponse>,
    reason_phrase: Option<&[u8]>,
    listener_port: Option<u16>,
    metrics: Option<&mio_proxy::CapturedFlowMetrics>,
    imported_meta: Option<&ImportedFlowHeadMeta<'_>>,
) -> Result<()> {
    ensure!(
        listener_port != Some(0),
        "HTTP 流量监听端口不能为 0；必须是 1–65535"
    );
    let id = record.summary.id.to_string();
    let previous: Option<(u64, Option<u16>)> = tx
        .query_row(
            "SELECT revision, listener_port FROM flows WHERE id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let fast_new_import = imported_meta.is_some() && previous.is_none();
    let before = if fast_new_import {
        None
    } else {
        read_response_head(tx, &id, 1)?
    };
    let stored_meta = if fast_new_import {
        None
    } else {
        read_http_history_metadata(tx, record.summary.id)?
    };
    if let Some(stored) = &stored_meta {
        ensure!(previous.is_some_and(|(revision, _)| revision == stored.revision), "HTTP 历史元数据 revision 与保存前流量不一致");
    }
    let effective_reason_phrase = reason_phrase.or_else(|| {
        record
            .response
            .as_ref()
            .and_then(|resp| resp.reason_phrase.as_deref())
    });
    if let Some(stream) = response_stream {
        stream.validate()?;
        ensure!(record.response.is_some(), "流式捕获缺少响应头");
    }
    if let Some((previous_revision, stored_port)) = previous {
        if let (Some(existing), Some(incoming)) = (stored_port, listener_port) {
            ensure!(existing == incoming, "同一流量的监听端口不能改变");
        }
        if previous_revision == record.summary.revision {
            ensure!(
                listener_port.is_none() || stored_port == listener_port,
                "相同流量 revision 不能修改监听端口"
            );
            let mut expected_record = record.clone();
            if let Some(resp) = &mut expected_record.response
                && resp.reason_phrase.is_none()
            {
                resp.reason_phrase = effective_reason_phrase.map(<[u8]>::to_vec);
            }
            ensure!(
                read_record(tx, record.summary.id)?.as_ref() == Some(&expected_record),
                "相同流量 revision 不能替换内容"
            );
            ensure!(
                before.as_ref().and_then(|head| head.stream.as_ref()) == response_stream,
                "相同流量 revision 不能替换流式捕获语义"
            );
            ensure!(
                before
                    .as_ref()
                    .and_then(|head| head.reason_phrase.as_deref())
                    == effective_reason_phrase,
                "相同流量 revision 不能替换响应 reason phrase"
            );
            if record.summary.source == crate::TrafficSource::Proxy
                && let Some(metrics) = metrics
            {
                let ip_changed = metrics.remote_ip.is_some_and(|ip| {
                    stored_meta
                        .as_ref()
                        .is_none_or(|s| s.metadata.remote_ip != ip.to_string())
                });
                let recv_changed = metrics.response_received_ms.is_some_and(|ms| {
                    stored_meta
                        .as_ref()
                        .is_none_or(|s| s.metadata.response_received_ms != Some(ms))
                });
                if stored_meta.is_none() || ip_changed || recv_changed {
                    let mut metadata =
                        ext_archive::http_query::live_row_metadata_with_stored(
                            tx,
                            &record.summary,
                            listener_port,
                            stored_meta.as_ref(),
                        )?;
                    if let Some(ip) = metrics.remote_ip {
                        metadata.remote_ip = ip.to_string();
                    }
                    if metrics.response_received_ms.is_some() {
                        metadata.response_received_ms = metrics.response_received_ms;
                    }
                    write_http_history_metadata(
                        tx,
                        record.summary.id,
                        record.summary.revision,
                        &metadata,
                    )?;
                }
            }
            return Ok(());
        }
        ensure!(
            previous_revision < record.summary.revision,
            "流量 revision 已过期"
        );
    }
    if let Some(head) = &before {
        match (&head.stream, response_stream) {
            (Some(old), Some(stream)) => {
                ensure!(
                    old.original_status == stream.original_status
                        && old.original_version == stream.original_version
                        && old.original_headers == stream.original_headers
                        && (!old.body_complete || stream.body_complete),
                    "流式原始响应头或已完成状态不能被替换"
                );
                let response = record.response.as_ref().context("流式捕获缺少响应头")?;
                let (prefix, old_bytes): (bool, usize) = tx.query_row(
                    "SELECT typeof(body)='blob' AND CASE WHEN length(body)=0 THEN 1 ELSE body=substr(?2,1,length(body)) END,length(body) FROM messages WHERE flow=?1 AND side=1",
                    params![id, response.body],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                ensure!(prefix, "流式正文不能覆盖已观察的前缀");
                ensure!(
                    (!old.body_complete || old_bytes == response.body.len())
                        && (head.trailers == response.trailers
                            || (!old.body_complete && head.trailers.is_empty())),
                    "已完成的流式正文或 trailers 不能被修改"
                );
            }
            (None, None) => (),
            _ => bail!("流式捕获元数据缺失或改变了既有响应的语义"),
        }
    }
    tx.execute(
        "INSERT INTO flows(id, revision, started, url, method, state, summary, listener_port, source, status, response_bytes, elapsed_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO UPDATE SET revision=excluded.revision, url=excluded.url, method=excluded.method, state=excluded.state, summary=excluded.summary, listener_port=COALESCE(excluded.listener_port, flows.listener_port), source=excluded.source, status=excluded.status, response_bytes=excluded.response_bytes, elapsed_ms=excluded.elapsed_ms",
        params![
            id,
            record.summary.revision,
            record.summary.started_unix_ms,
            record.summary.url,
            record.summary.method,
            state_code(&record.summary.state),
            serde_json::to_vec(&record.summary)?,
            listener_port,
            source_code(&record.summary.source),
            record.summary.status.map(i64::from),
            i64::try_from(record.summary.response_bytes)?,
            i64::try_from(record.summary.elapsed_ms)?,
        ],
    )?;
    // Body bytes never enter the summary query or its JSON representation.
    let is_proxy = record.summary.source == crate::TrafficSource::Proxy;
    let proxy_capture_number = if is_proxy {
        if let Some(meta) = imported_meta {
            Some(meta.capture_number)
        } else {
            match existing_proxy_capture_number(tx, record.summary.id, stored_meta.as_ref())? {
                Some(num) => Some(num),
                None => Some(allocate_proxy_capture_number(tx)?),
            }
        }
    } else {
        None
    };
    let prev_req_head: Option<OwnedRequestHead> = if is_proxy && !fast_new_import {
        if let Some((bytes, _)) = read_message(tx, &id, 2)? {
            Some(serde_json::from_slice(&bytes)?)
        } else if let Some((bytes, _)) = read_message(tx, &id, 0)? {
            Some(serde_json::from_slice(&bytes)?)
        } else {
            None
        }
    } else {
        None
    };
    let mut request_head = RequestHead::from(&record.request);
    let remote_ip_str = metrics
        .and_then(|m| m.remote_ip)
        .map(|ip| ip.to_string())
        .or_else(|| {
            imported_meta
                .filter(|m| !m.remote_ip.is_empty())
                .map(|m| m.remote_ip.to_owned())
        })
        .or_else(|| {
            prev_req_head
                .as_ref()
                .and_then(|h| h.remote_ip.clone())
                .filter(|ip| !ip.is_empty())
        })
        .or_else(|| {
            stored_meta
                .as_ref()
                .map(|s| s.metadata.remote_ip.clone())
                .filter(|ip| !ip.is_empty())
        });
    let params_analysis = is_proxy.then(|| history_has_parameters(&record.request));
    let params_error = params_analysis.as_ref().and_then(|result| result.as_ref().err()).map(history_analysis_diagnostic);
    if is_proxy {
        request_head.has_params = params_analysis.as_ref().and_then(|result| result.as_ref().ok()).copied();
        request_head.params_known = Some(params_error.is_none());
        request_head.params_error = params_error.as_deref();
        request_head.remote_ip = remote_ip_str.as_deref();
        request_head.time_known = imported_meta
            .map(|m| m.time_known)
            .or_else(|| prev_req_head.as_ref().and_then(|h| h.time_known))
            .or_else(|| stored_meta.as_ref().map(|s| s.metadata.time_known));
        request_head.capture_number = proxy_capture_number;
    }
    let same_request = !fast_new_import
        && read_message(tx, &id, 0)?.map(request_from_parts).transpose()?.as_ref() == Some(&record.request);
    let write_request = if same_request || fast_new_import {
        put_current_message
    } else {
        put_message
    };
    write_request(
        tx,
        &id,
        0,
        &serde_json::to_vec(&request_head)?,
        &record.request.body,
    )?;
    if let Some(response) = &record.response {
        let (mime, title_analysis) = if is_proxy { response_history_values(response, &record.request.method) } else { (String::new(), Ok(String::new())) };
        let (title, title_error) = match title_analysis {
            Ok(title) => (title, None),
            Err(error) => (String::new(), Some(history_analysis_diagnostic(&error))),
        };
        let recv_ms = metrics
            .and_then(|m| m.response_received_ms)
            .or_else(|| imported_meta.and_then(|m| m.response_received_ms))
            .or_else(|| before.as_ref().and_then(|b| b.response_received_ms));
        if let Some(stream) = response_stream {
            let mut head = ResponseHead::from(response);
            head.stream = Some(stream);
            head.reason_phrase = effective_reason_phrase;
            if is_proxy {
                head.body_bytes = Some(response.body.len() as u64);
                head.mime_type = Some(&mime);
                head.title = title_error.is_none().then_some(title.as_str());
                head.title_error = title_error.as_deref();
                head.response_received_ms = recv_ms;
            }
            put_current_message(tx, &id, 1, &serde_json::to_vec(&head)?, &response.body)?;
            let had_original: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE flow=?1 AND side=3)",
                [&id],
                |row| row.get(0),
            )?;
            if stream.head_was_edited(response) || had_original {
                head.status = stream.original_status;
                head.version = stream.original_version;
                head.headers = &stream.original_headers;
                let orig_reason = read_response_head(tx, &id, 3)?
                    .and_then(|h| h.reason_phrase)
                    .or_else(|| before.as_ref().and_then(|h| h.reason_phrase.clone()));
                head.reason_phrase = orig_reason.as_deref();
                if is_proxy {
                    let original_response = HttpResponse {
                        status: stream.original_status,
                        version: stream.original_version,
                        reason_phrase: orig_reason.clone(),
                        headers: stream.original_headers.clone(),
                        body: response.body.clone(),
                        trailers: response.trailers.clone(),
                    };
                    let (original_mime, original_title) = response_history_values(&original_response, &record.request.method);
                    let (original_title, original_title_error) = match original_title {
                        Ok(title) => (title, None),
                        Err(error) => (String::new(), Some(history_analysis_diagnostic(&error))),
                    };
                    head.mime_type = Some(&original_mime);
                    head.title = original_title_error.is_none().then_some(original_title.as_str());
                    head.title_error = original_title_error.as_deref();
                    put_current_message(tx, &id, 3, &serde_json::to_vec(&head)?, &response.body)?;
                } else {
                // Both versions refer to the same captured source bytes, not two in-memory bodies.
                put_current_message(tx, &id, 3, &serde_json::to_vec(&head)?, &response.body)?;
                }
            }
        } else {
            let mut head = ResponseHead::from(response);
            head.reason_phrase = effective_reason_phrase;
            if is_proxy {
                head.body_bytes = Some(response.body.len() as u64);
                head.mime_type = Some(&mime);
                head.title = title_error.is_none().then_some(title.as_str());
                head.title_error = title_error.as_deref();
                head.response_received_ms = recv_ms;
            }
            let same_response = !fast_new_import && {
                let mut current_response = response.clone();
                if current_response.reason_phrase.is_none() {
                    current_response.reason_phrase = effective_reason_phrase.map(<[u8]>::to_vec);
                }
                read_message(tx, &id, 1)?.map(response_from_parts).transpose()?.as_ref() == Some(&current_response)
            };
            let write_response = if same_response || fast_new_import {
                put_current_message
            } else {
                put_message
            };
            write_response(tx, &id, 1, &serde_json::to_vec(&head)?, &response.body)?;
        }
    } else {
        tx.execute("INSERT OR IGNORE INTO messages SELECT flow,3,head,body FROM messages WHERE flow=?1 AND side=1", [&id])?;
        tx.execute("DELETE FROM messages WHERE flow=?1 AND side=1", [&id])?;
    }
    if is_proxy {
        let mut metadata = ext_archive::http_query::live_row_metadata_with_stored(
            tx,
            &record.summary,
            listener_port,
            stored_meta.as_ref(),
        )?;
        if metadata.capture_number.is_none() {
            metadata.capture_number = proxy_capture_number;
        }
        if let Some(metrics) = metrics {
            if let Some(ip) = metrics.remote_ip {
                metadata.remote_ip = ip.to_string();
            }
            if metrics.response_received_ms.is_some() {
                metadata.response_received_ms = metrics.response_received_ms;
            }
        }
        if let Some(meta) = imported_meta {
            metadata.time_known = meta.time_known;
            metadata.capture_number = Some(meta.capture_number);
            if !meta.remote_ip.is_empty() {
                metadata.remote_ip = meta.remote_ip.to_owned();
            }
            if meta.response_received_ms.is_some() {
                metadata.response_received_ms = meta.response_received_ms;
            }
        }
        write_http_history_metadata(tx, record.summary.id, record.summary.revision, &metadata)?;
        tx.execute(
            "UPDATE flows SET capture_number=COALESCE(?1, capture_number), response_length=?2 WHERE id=?3",
            params![
                metadata.capture_number.and_then(|v| i64::try_from(v).ok()),
                metadata.response_length.and_then(|v| i64::try_from(v).ok()),
                id,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn finish_pending_records(
    tx: &rusqlite::Transaction<'_>,
    crashed: bool,
    only_proxy: bool,
) -> Result<usize> {
    let pending = {
        let mut query =
            tx.prepare("SELECT summary FROM flows WHERE state IN ('pending','intercepted')")?;
        query
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut changed = 0;
    for bytes in &pending {
        let mut summary: FlowSummary = serde_json::from_slice(bytes)?;
        let stored = read_http_history_metadata(tx, summary.id)?;
        if let Some(stored) = &stored {
            ensure!(stored.revision == summary.revision, "HTTP 历史元数据 revision 与待收尾流量不一致");
        }
        // Intruder owns its flow/ordinal/counters as one transaction, including recovery.
        if summary.source == crate::TrafficSource::Intruder
            || (only_proxy && summary.source != crate::TrafficSource::Proxy)
        {
            continue;
        }
        summary.revision += 1;
        summary.state = if crashed {
            crate::FlowState::Failed("上次会话退出时仍在途；结果未知，未自动重发".into())
        } else {
            crate::FlowState::Cancelled
        };
        tx.execute(
            "UPDATE flows SET revision=?1, state=?2, summary=?3 WHERE id=?4",
            params![
                summary.revision,
                state_code(&summary.state),
                serde_json::to_vec(&summary)?,
                summary.id.to_string()
            ],
        )?;
        if let Some(stored) = stored {
            write_http_history_metadata(tx, summary.id, summary.revision, &stored.metadata)?;
        }
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod cookie_tests {
    use super::history_cookie_names;

    fn hdr(name: &str, value: &[u8]) -> crate::Header {
        crate::Header {
            name: name.to_owned(),
            value: value.to_vec(),
        }
    }

    #[test]
    fn parses_request_cookies_and_quoted_semicolons() {
        let headers = vec![
            hdr("Cookie", b"sid=alpha; theme=dark"),
            hdr("cookie", b"quoted=\"a;b\"; theme=override; token=ok"),
        ];
        assert_eq!(
            history_cookie_names(&headers, false).unwrap(),
            vec!["sid", "theme", "quoted", "token"]
        );
    }

    #[test]
    fn rejects_invalid_request_cookies() {
        let bare = history_cookie_names(&[hdr("Cookie", b"sid=1; bare_without_equals")], false)
            .unwrap_err()
            .to_string();
        assert!(bare.contains("等号"), "{bare}");

        let trailing = history_cookie_names(&[hdr("Cookie", b"a=1;")], false)
            .unwrap_err()
            .to_string();
        assert!(trailing.contains("等号"), "{trailing}");

        let empty = history_cookie_names(&[hdr("Cookie", b"")], false)
            .unwrap_err()
            .to_string();
        assert!(empty.contains("等号"), "{empty}");

        let ctl = history_cookie_names(&[hdr("Cookie", b"sid=1\r\n")], false)
            .unwrap_err()
            .to_string();
        assert!(ctl.contains("字节无效"), "{ctl}");

        let non_ascii = history_cookie_names(&[hdr("Cookie", b"sid=\x80")], false)
            .unwrap_err()
            .to_string();
        assert!(non_ascii.contains("字节无效"), "{non_ascii}");

        let bad_name = history_cookie_names(&[hdr("Cookie", b"bad name=1")], false)
            .unwrap_err()
            .to_string();
        assert!(bad_name.contains("名称无效"), "{bad_name}");
    }

    #[test]
    fn parses_and_validates_set_cookie_headers() {
        let headers = vec![
            hdr("Set-Cookie", b"csrf=token123; Path=/; HttpOnly"),
            hdr("set-cookie", b"quoted=\"a;b\"; Path=/"),
            hdr("Set-Cookie", b"csrf=renewed; Secure"),
        ];
        assert_eq!(
            history_cookie_names(&headers, true).unwrap(),
            vec!["csrf", "quoted"]
        );

        let missing_eq = history_cookie_names(&[hdr("Set-Cookie", b"HttpOnly; Path=/")], true)
            .unwrap_err()
            .to_string();
        assert!(missing_eq.contains("缺少名称和值"), "{missing_eq}");
    }
}
