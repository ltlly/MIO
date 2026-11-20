use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use url::Url;

use crate::{
    MAX_ORGANISER_ITEMS, MAX_ORGANISER_MESSAGE_BYTES, MAX_ORGANISER_NOTES_BYTES,
    MAX_ORGANISER_TOTAL_BYTES, OrganiserItem,
};

const MAX_ORGANISER_SOURCE_TOOL_BYTES: usize = 64;
const MAX_ORGANISER_METHOD_BYTES: usize = 64;
const MAX_ORGANISER_TARGET_BYTES: usize = 4096;
const MAX_ORGANISER_URL_BYTES: usize = 64 * 1024;

pub fn validate_organiser_items(items: &[OrganiserItem]) -> Result<()> {
    ensure!(
        items.len() <= MAX_ORGANISER_ITEMS,
        "Organiser 条目数量超过 {MAX_ORGANISER_ITEMS} 条上限"
    );
    let mut ids = HashSet::with_capacity(items.len());
    let mut numbers = HashSet::with_capacity(items.len());
    let mut total_bytes = 0usize;
    for item in items {
        item.validate()?;
        ensure!(ids.insert(item.id), "Organiser 条目 ID 重复");
        ensure!(numbers.insert(item.number), "Organiser 条目序号重复");
        let bytes = item.payload_bytes()?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .filter(|&sum| sum <= MAX_ORGANISER_TOTAL_BYTES)
            .ok_or_else(|| anyhow::anyhow!("Organiser 总存储字节超过 32 MiB 上限"))?;
    }
    Ok(())
}

pub fn next_organiser_number(items: &[OrganiserItem]) -> Result<u64> {
    items
        .iter()
        .map(|item| item.number)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("Organiser 条目序号已耗尽")
}

pub(crate) fn validate_organiser_item(item: &OrganiserItem) -> Result<()> {
    ensure!(!item.id.is_nil(), "Organiser 条目 ID 不能为空");
    ensure!(item.number > 0, "Organiser 条目序号必须大于零");
    ensure!(item.added_unix_ms >= 0, "Organiser 条目时间不能为负数");
    let _ = normalize_source_tool(&item.source_tool)?;
    ensure!(
        item.target.len() <= MAX_ORGANISER_TARGET_BYTES
            && !has_forbidden_single_line_control(&item.target),
        "Organiser 连接目标超过长度上限或包含控制字符"
    );
    if !item.target.is_empty() {
        let parsed = Url::parse(&item.target).context("Organiser 连接目标格式无效")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "Organiser 连接目标必须为不带凭据的 HTTP/HTTPS 地址"
        );
    }
    ensure!(
        !item.method.is_empty()
            && item.method.len() <= MAX_ORGANISER_METHOD_BYTES
            && crate::http::token(item.method.as_bytes()),
        "Organiser 请求方法无效"
    );
    ensure!(
        !item.url.is_empty()
            && item.url.len() <= MAX_ORGANISER_URL_BYTES
            && !has_forbidden_single_line_control(&item.url),
        "Organiser URL 为空、过长或包含控制字符"
    );
    if let Some(code) = item.status_code {
        ensure!(
            (100..=599).contains(&code),
            "Organiser 响应状态码必须在 100..=599 范围内"
        );
    }
    ensure!(
        !item.request_raw.is_empty() && item.request_raw.len() <= MAX_ORGANISER_MESSAGE_BYTES,
        "Organiser 请求报文不能为空且不能超过 4 MiB"
    );
    if let Some(resp) = &item.response_raw {
        ensure!(
            resp.len() <= MAX_ORGANISER_MESSAGE_BYTES,
            "Organiser 响应报文超过 4 MiB 上限"
        );
    }
    ensure!(
        item.notes.len() <= MAX_ORGANISER_NOTES_BYTES && !has_forbidden_note_control(&item.notes),
        "Organiser 备注超过 64 KiB 上限或包含非法控制字符"
    );
    ensure!(
        item.highlight.is_none_or(|color| (1..=9).contains(&color)),
        "Organiser 高亮颜色编号必须在 1..=9 范围内"
    );
    Ok(())
}

pub(crate) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

pub(crate) fn normalize_source_tool(source_tool: &str) -> Result<String> {
    let trimmed = source_tool.trim();
    ensure!(
        !trimmed.is_empty()
            && trimmed.len() <= MAX_ORGANISER_SOURCE_TOOL_BYTES
            && !has_forbidden_single_line_control(trimmed),
        "Organiser 来源工具名称无效"
    );
    Ok(trimmed.to_owned())
}

fn has_forbidden_single_line_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

fn has_forbidden_note_control(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

pub(crate) fn extract_origin(candidate: &str) -> String {
    let Ok(parsed) = Url::parse(candidate) else {
        return String::new();
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return String::new();
    }
    let Some(host) = parsed.host_str() else {
        return String::new();
    };
    match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    }
}

pub(crate) fn parse_request_start_line(raw: &[u8]) -> Result<(String, String)> {
    let end = raw
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(raw.len())
        .min(MAX_ORGANISER_URL_BYTES + 128);
    let line = raw[..end].strip_suffix(b"\r").unwrap_or(&raw[..end]);
    let line_str = std::str::from_utf8(line).context("请求起始行必须为有效 UTF-8")?;
    let mut parts = line_str.split_ascii_whitespace();
    let Some(method) = parts.next() else {
        bail!("请求报文缺少请求方法");
    };
    ensure!(
        method.len() <= MAX_ORGANISER_METHOD_BYTES && crate::http::token(method.as_bytes()),
        "请求报文方法名称无效"
    );
    let target = parts.next().unwrap_or("/");
    ensure!(
        !target.is_empty()
            && target.len() <= MAX_ORGANISER_URL_BYTES
            && !has_forbidden_single_line_control(target),
        "请求报文目标路径无效"
    );
    Ok((method.to_owned(), target.to_owned()))
}

fn extract_host_header(raw: &[u8]) -> Option<String> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(raw.len().min(16 * 1024));
    for line in raw[..head_end].split(|&b| b == b'\n').skip(1) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        if line[..colon].eq_ignore_ascii_case(b"host") {
            let host = std::str::from_utf8(line[colon + 1..].trim_ascii()).ok()?;
            if !host.is_empty() && !has_forbidden_single_line_control(host) {
                return Some(host.to_owned());
            }
        }
    }
    None
}

pub(crate) fn resolve_display_url(
    origin: &str,
    target_or_url: &str,
    request_target: &str,
    request_raw: &[u8],
) -> String {
    if request_target.starts_with("http://") || request_target.starts_with("https://") {
        return request_target.to_owned();
    }
    if !origin.is_empty() {
        return if request_target.starts_with('/') {
            format!("{origin}{request_target}")
        } else {
            format!("{origin}/{request_target}")
        };
    }
    if (target_or_url.starts_with("http://") || target_or_url.starts_with("https://"))
        && !has_forbidden_single_line_control(target_or_url)
    {
        return target_or_url.to_owned();
    }
    if let Some(host) = extract_host_header(request_raw) {
        return if request_target.starts_with('/') {
            format!("http://{host}{request_target}")
        } else {
            format!("http://{host}/{request_target}")
        };
    }
    request_target.to_owned()
}

pub(crate) fn parse_response_status_code(raw: &[u8]) -> Option<u16> {
    let end = raw
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(raw.len())
        .min(256);
    let line = std::str::from_utf8(raw[..end].strip_suffix(b"\r").unwrap_or(&raw[..end])).ok()?;
    let mut parts = line.split_ascii_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts
        .next()?
        .parse::<u16>()
        .ok()
        .filter(|code| (100..=599).contains(code))
}
