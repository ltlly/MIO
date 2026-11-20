use std::time::{Duration, Instant};

use anyhow::Result;
use mio_proxy::SendConfig;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedParam {
    pub name: String,
    pub value: String,
    pub target: MacroApplyTarget,
}

pub async fn execute_macro(
    mac: &SessionMacro,
    cookie_jar: &mut Vec<CookieJarEntry>,
    base_config: &SendConfig,
    cancel: &CancellationToken,
    outer_deadline: Instant,
) -> std::result::Result<Vec<ExtractedParam>, SessionExecError> {
    if cancel.is_cancelled() {
        return Err(SessionExecError::Cancelled);
    }
    if mac.steps.is_empty() {
        return Err(SessionExecError::Failed(format!(
            "Macro '{}' 不包含任何请求步骤",
            mac.name
        )));
    }
    let macro_budget = Duration::from_millis(mac.timeout_ms.clamp(100, 60_000));
    let macro_deadline = Instant::now()
        .checked_add(macro_budget)
        .map_or(outer_deadline, |dl| dl.min(outer_deadline));

    let max_attempts: usize = match mac.failure_policy {
        MacroFailurePolicy::Abort => 1,
        MacroFailurePolicy::Retry { max_retries } => 1 + usize::from(max_retries.clamp(1, 3)),
    };

    let mut last_error = String::new();
    for _attempt in 0..max_attempts {
        if cancel.is_cancelled() {
            return Err(SessionExecError::Cancelled);
        }
        let mut attempt_jar = cookie_jar.clone();
        match run_macro_once(mac, &mut attempt_jar, base_config, cancel, macro_deadline).await {
            Ok(extracted) => {
                *cookie_jar = attempt_jar;
                return Ok(extracted);
            }
            Err(SessionExecError::Cancelled) => return Err(SessionExecError::Cancelled),
            Err(SessionExecError::Failed(err)) => {
                last_error = err;
                if Instant::now() >= macro_deadline {
                    break;
                }
            }
        }
    }
    Err(SessionExecError::Failed(last_error))
}

async fn run_macro_once(
    mac: &SessionMacro,
    jar: &mut Vec<CookieJarEntry>,
    base_config: &SendConfig,
    cancel: &CancellationToken,
    deadline: Instant,
) -> std::result::Result<Vec<ExtractedParam>, SessionExecError> {
    let mut extracted: Vec<ExtractedParam> = Vec::new();
    for (idx, step) in mac.steps.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SessionExecError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SessionExecError::Failed(format!(
                "Macro '{}' 在步骤 {} 前已超时",
                mac.name,
                idx + 1
            )));
        }
        let mut req = HttpRequest {
            method: step.method.clone(),
            url: step.url.clone(),
            version: HttpVersion::Http11,
            headers: step.headers.clone(),
            body: step.body.clone(),
            trailers: Vec::new(),
        };
        if !extracted.is_empty() {
            apply_extractions_to_step(&mut req, &extracted).map_err(|e| {
                SessionExecError::Failed(format!(
                    "Macro '{}' 步骤 {} 参数回填失败：{e}",
                    mac.name,
                    idx + 1
                ))
            })?;
        }
        if step.use_cookie_jar {
            apply_cookie_jar_to_request(jar, &mut req, true, true, current_unix_ms()).map_err(
                |e| {
                    SessionExecError::Failed(format!(
                        "Macro '{}' 步骤 {} Cookie Jar 注入失败：{e}",
                        mac.name,
                        idx + 1
                    ))
                },
            )?;
        }
        rules::sync_content_length_if_present(&mut req);

        let mut step_config = base_config.clone();
        step_config.timeout = remaining;
        step_config.http_version = None;
        step_config.update_content_length = true;
        step_config.normalize_http1_line_endings = true;
        step_config.raw_http1_override = None;

        let tokio_deadline = tokio::time::Instant::from_std(deadline);
        let send_res = tokio::time::timeout_at(
            tokio_deadline,
            mio_proxy::send(req.clone(), step_config.clone(), cancel.clone()),
        )
        .await;
        let mut response = match send_res {
            Ok(Ok(resp)) => resp,
            Ok(Err(mio_proxy::ProxyError::Cancelled)) => return Err(SessionExecError::Cancelled),
            Err(_) if cancel.is_cancelled() => return Err(SessionExecError::Cancelled),
            Err(_) => {
                return Err(SessionExecError::Failed(format!(
                    "Macro '{}' 步骤 {} 请求超时",
                    mac.name,
                    idx + 1
                )));
            }
            Ok(Err(err)) => {
                return Err(SessionExecError::Failed(format!(
                    "Macro '{}' 步骤 {} 网络发送失败：{err}",
                    mac.name,
                    idx + 1
                )));
            }
        };

        if response
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-encoding"))
        {
            match crate::http::unpack_response(
                &response,
                &req.method,
                step_config.max_body_bytes,
                cancel,
                deadline,
            ) {
                Ok(Some(unpacked)) => response = unpacked,
                Ok(None) => {}
                Err(crate::http::UnpackError::Cancelled) => {
                    return Err(SessionExecError::Cancelled);
                }
                Err(err) => {
                    return Err(SessionExecError::Failed(format!(
                        "Macro '{}' 步骤 {} 响应解压失败：{err:?}",
                        mac.name,
                        idx + 1
                    )));
                }
            }
        }

        if step.use_cookie_jar {
            ingest_set_cookie_headers(jar, &req.url, &response, current_unix_ms());
        }

        if let Some(expected) = step.expected_status
            && response.status != expected
        {
            return Err(SessionExecError::Failed(format!(
                "Macro '{}' 步骤 {} 状态码 {} 与预期 {} 不匹配",
                mac.name,
                idx + 1,
                response.status,
                expected
            )));
        }

        for ext in &step.extractions {
            let val = extract_from_response(&response, &ext.source).ok_or_else(|| {
                SessionExecError::Failed(format!(
                    "Macro '{}' 步骤 {} 未能提取必需参数 '{}'",
                    mac.name,
                    idx + 1,
                    ext.name
                ))
            })?;
            if val.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
                return Err(SessionExecError::Failed(format!(
                    "Macro '{}' 提取的参数 '{}' 含非法控制字符",
                    mac.name, ext.name
                )));
            }
            if let Some(existing) = extracted.iter_mut().find(|item| item.name == ext.name) {
                existing.value = val;
                existing.target = ext.target.clone();
            } else {
                extracted.push(ExtractedParam {
                    name: ext.name.clone(),
                    value: val,
                    target: ext.target.clone(),
                });
            }
        }
    }
    Ok(extracted)
}

pub fn extract_from_response(
    response: &HttpResponse,
    source: &MacroExtractSource,
) -> Option<String> {
    match source {
        MacroExtractSource::ResponseHeader { header_name } => response
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(header_name))
            .and_then(|h| std::str::from_utf8(&h.value).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        MacroExtractSource::Delimiter { start, end } => {
            let body_text = String::from_utf8_lossy(&response.body);
            extract_between(&body_text, start, end).or_else(|| {
                let raw_bytes = crate::response_to_raw(response);
                let raw_text = String::from_utf8_lossy(&raw_bytes);
                extract_between(&raw_text, start, end)
            })
        }
        MacroExtractSource::Regex { pattern } => {
            let re = regex::RegexBuilder::new(pattern)
                .size_limit(512 * 1024)
                .build()
                .ok()?;
            let body_text = String::from_utf8_lossy(&response.body);
            if let Some(caps) = re.captures(&body_text) {
                let m = caps.get(1).or_else(|| caps.get(0))?;
                return Some(m.as_str().to_owned());
            }
            let raw_bytes = crate::response_to_raw(response);
            let raw_text = String::from_utf8_lossy(&raw_bytes);
            let caps = re.captures(&raw_text)?;
            let m = caps.get(1).or_else(|| caps.get(0))?;
            Some(m.as_str().to_owned())
        }
    }
}

fn extract_between(haystack: &str, start: &str, end: &str) -> Option<String> {
    if start.is_empty() || end.is_empty() {
        return None;
    }
    let start_idx = haystack.find(start)? + start.len();
    let rest = &haystack[start_idx..];
    let end_idx = rest.find(end)?;
    Some(rest[..end_idx].to_owned())
}

fn replace_placeholders(request: &mut HttpRequest, extractions: &[ExtractedParam]) -> bool {
    let mut changed = false;
    for item in extractions {
        let token = format!("{{{{{}}}}}", item.name);
        if request.url.contains(&token) {
            let encoded: String =
                url::form_urlencoded::byte_serialize(item.value.as_bytes()).collect();
            request.url = request.url.replace(&token, &encoded);
            changed = true;
        }
        for header in &mut request.headers {
            if let Ok(val_str) = std::str::from_utf8(&header.value)
                && val_str.contains(&token)
            {
                header.value = val_str.replace(&token, &item.value).into_bytes();
                changed = true;
            }
        }
        if let Ok(body_str) = std::str::from_utf8(&request.body)
            && body_str.contains(&token)
        {
            let replacement = if rules::is_form_urlencoded(&request.headers) {
                url::form_urlencoded::byte_serialize(item.value.as_bytes()).collect()
            } else {
                item.value.clone()
            };
            request.body = body_str.replace(&token, &replacement).into_bytes();
            rules::sync_content_length_if_present(request);
            changed = true;
        }
    }
    changed
}

fn apply_extractions_to_step(
    request: &mut HttpRequest,
    extractions: &[ExtractedParam],
) -> Result<bool> {
    let mut changed = replace_placeholders(request, extractions);
    for item in extractions {
        match &item.target {
            MacroApplyTarget::UrlOrBodyParam => {
                if rules::rewrite_urlencoded_body_if_present(request, &item.name, &item.value) {
                    changed = true;
                }
                let (next_url, url_changed) =
                    rewrite_url_query_param(&request.url, &item.name, &item.value, false, true)?;
                if url_changed {
                    request.url = next_url;
                    changed = true;
                }
            }
            MacroApplyTarget::Header { header_name } => {
                let target_name = if header_name.is_empty() {
                    &item.name
                } else {
                    header_name
                };
                if set_request_header(request, target_name, &item.value, false)? {
                    changed = true;
                }
            }
            MacroApplyTarget::Cookie { cookie_name } => {
                let target_name = if cookie_name.is_empty() {
                    &item.name
                } else {
                    cookie_name
                };
                if set_or_replace_request_cookie(request, target_name, &item.value)? {
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}

pub fn apply_extractions_to_final_request(
    request: &mut HttpRequest,
    extractions: &[ExtractedParam],
) -> Result<bool> {
    let mut changed = replace_placeholders(request, extractions);
    for item in extractions {
        match &item.target {
            MacroApplyTarget::UrlOrBodyParam => {
                let body_updated =
                    rules::rewrite_urlencoded_body_if_present(request, &item.name, &item.value);
                let (next_url, url_updated) = rewrite_url_query_param(
                    &request.url,
                    &item.name,
                    &item.value,
                    false,
                    body_updated,
                )?;
                if url_updated {
                    request.url = next_url;
                }
                changed |= body_updated || url_updated;
            }
            MacroApplyTarget::Header { header_name } => {
                let target_name = if header_name.is_empty() {
                    &item.name
                } else {
                    header_name
                };
                if set_request_header(request, target_name, &item.value, false)? {
                    changed = true;
                }
            }
            MacroApplyTarget::Cookie { cookie_name } => {
                let target_name = if cookie_name.is_empty() {
                    &item.name
                } else {
                    cookie_name
                };
                if set_or_replace_request_cookie(request, target_name, &item.value)? {
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}
