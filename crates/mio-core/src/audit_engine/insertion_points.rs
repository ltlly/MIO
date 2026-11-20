use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::{InsertionLocation, InsertionPoint};
use crate::{Header, HttpRequest, intruder::parse_intercept_content_type};

mod xml_multipart;

use xml_multipart::{
    parse_multipart_spans, parse_xml_spans, rebuild_multipart_body, rebuild_xml_body,
};

pub const MAX_STRUCTURED_PARAMETERS: usize = 64;
const MAX_STRUCTURED_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 32;

fn header_str<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .and_then(|h| std::str::from_utf8(&h.value).ok())
        .map(str::trim)
}

fn push_point(
    pts: &mut Vec<InsertionPoint>,
    location: InsertionLocation,
    name: String,
    value: String,
) {
    if pts.len() >= MAX_STRUCTURED_PARAMETERS || name.is_empty() {
        return;
    }
    let occurrence = pts
        .iter()
        .filter(|p| {
            p.location == location
                && if location == InsertionLocation::Header {
                    p.name.eq_ignore_ascii_case(&name)
                } else {
                    p.name == name
                }
        })
        .count();
    pts.push(InsertionPoint {
        location,
        name,
        value,
        occurrence,
    });
}

fn collect_json_leaves(val: &Value, path: &str, depth: usize, pts: &mut Vec<InsertionPoint>) {
    if depth > MAX_JSON_DEPTH || pts.len() >= MAX_STRUCTURED_PARAMETERS {
        return;
    }
    match val {
        Value::Object(map) => {
            for (k, child) in map {
                let next = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                collect_json_leaves(child, &next, depth + 1, pts);
            }
        }
        Value::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                let next = format!("{path}[{idx}]");
                collect_json_leaves(child, &next, depth + 1, pts);
            }
        }
        Value::String(s) => push_point(pts, InsertionLocation::Json, path.to_owned(), s.clone()),
        Value::Number(n) => {
            push_point(pts, InsertionLocation::Json, path.to_owned(), n.to_string())
        }
        Value::Bool(b) => push_point(pts, InsertionLocation::Json, path.to_owned(), b.to_string()),
        Value::Null => push_point(
            pts,
            InsertionLocation::Json,
            path.to_owned(),
            "null".to_owned(),
        ),
    }
}

fn mutate_json_leaf(
    val: &mut Value,
    path: &str,
    depth: usize,
    target_name: &str,
    target_occ: usize,
    seen: &mut usize,
    new_value: &str,
) -> bool {
    if depth > MAX_JSON_DEPTH {
        return false;
    }
    match val {
        Value::Object(map) => {
            for (k, child) in map.iter_mut() {
                let next = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if mutate_json_leaf(
                    child,
                    &next,
                    depth + 1,
                    target_name,
                    target_occ,
                    seen,
                    new_value,
                ) {
                    return true;
                }
            }
            false
        }
        Value::Array(items) => {
            for (idx, child) in items.iter_mut().enumerate() {
                let next = format!("{path}[{idx}]");
                if mutate_json_leaf(
                    child,
                    &next,
                    depth + 1,
                    target_name,
                    target_occ,
                    seen,
                    new_value,
                ) {
                    return true;
                }
            }
            false
        }
        _ => {
            if path == target_name {
                if *seen == target_occ {
                    *val = Value::String(new_value.to_owned());
                    return true;
                }
                *seen += 1;
            }
            false
        }
    }
}

fn extract_cookie_points(headers: &[Header], pts: &mut Vec<InsertionPoint>) {
    for h in headers {
        if !h.name.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&h.value) else {
            continue;
        };
        for part in raw.split(';') {
            let trimmed = part.trim();
            let Some((k, v)) = trimmed.split_once('=') else {
                continue;
            };
            let name = k.trim();
            if name.is_empty() {
                continue;
            }
            let val = v.trim();
            let unquoted = val
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(val);
            push_point(
                pts,
                InsertionLocation::Cookie,
                name.to_owned(),
                unquoted.to_owned(),
            );
        }
    }
}

fn decode_url_component(raw: &str, plus_as_space: bool) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' if plus_as_space => {
                out.push(b' ');
                idx += 1;
            }
            b'%' if idx + 2 < bytes.len() => {
                let hi = (bytes[idx + 1] as char).to_digit(16);
                let lo = (bytes[idx + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push(((h << 4) | l) as u8);
                    idx += 3;
                } else {
                    out.push(b'%');
                    idx += 1;
                }
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn extract_structured_parameters(req: &HttpRequest) -> Vec<InsertionPoint> {
    let mut pts = Vec::new();
    let head = req.url.split('#').next().unwrap_or(&req.url);
    if let Some((_, query)) = head.split_once('?') {
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            push_point(
                &mut pts,
                InsertionLocation::Query,
                k.into(),
                decode_url_component(v, false),
            );
        }
    }

    if !req.body.is_empty() && req.body.len() <= MAX_STRUCTURED_BODY_BYTES {
        let ct_raw = header_str(&req.headers, "content-type").unwrap_or_default();
        let ct_lower = ct_raw.to_ascii_lowercase();
        let parsed_ct = parse_intercept_content_type(ct_raw.as_bytes()).ok();
        let mp_boundary = parsed_ct
            .as_ref()
            .and_then(|c| c.multipart_boundary().ok().flatten())
            .map(<[u8]>::to_vec);

        if ct_lower.contains("application/x-www-form-urlencoded")
            && let Ok(form) = std::str::from_utf8(&req.body)
        {
            for pair in form.split('&').filter(|p| !p.is_empty()) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                push_point(
                    &mut pts,
                    InsertionLocation::Form,
                    k.into(),
                    decode_url_component(v, true),
                );
            }
        } else if let Some(boundary) = mp_boundary
            && let Ok(spans) = parse_multipart_spans(&req.body, &boundary)
        {
            for span in spans {
                push_point(
                    &mut pts,
                    InsertionLocation::Multipart,
                    span.name,
                    span.value,
                );
            }
        } else {
            let looks_json = ct_lower.contains("json")
                || (ct_lower.is_empty()
                    && matches!(req.body.trim_ascii().first(), Some(b'{' | b'[')));
            if looks_json {
                let body_slice = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
                if let Ok(root) = serde_json::from_slice::<Value>(body_slice) {
                    collect_json_leaves(&root, "", 0, &mut pts);
                }
            } else {
                let looks_xml = ct_lower.contains("xml")
                    || (ct_lower.is_empty() && req.body.trim_ascii().starts_with(b"<"));
                if looks_xml && let Ok(spans) = parse_xml_spans(&req.body) {
                    for span in spans {
                        push_point(&mut pts, InsertionLocation::Xml, span.name, span.value);
                    }
                }
            }
        }
    }

    extract_cookie_points(&req.headers, &mut pts);

    for h in &req.headers {
        if h.name.len() > 2
            && h.name
                .get(..2)
                .is_some_and(|p| p.eq_ignore_ascii_case("x-"))
            && let Ok(val) = std::str::from_utf8(&h.value)
        {
            push_point(
                &mut pts,
                InsertionLocation::Header,
                h.name.clone(),
                val.trim().to_owned(),
            );
        }
    }
    pts
}

fn sync_content_length(headers: &mut [Header], body_len: usize) {
    let len_bytes = body_len.to_string().into_bytes();
    for h in headers {
        if h.name.eq_ignore_ascii_case("content-length") {
            h.value.clone_from(&len_bytes);
        }
    }
}

pub fn rebuild_structured_parameter(
    req: &HttpRequest,
    point: &InsertionPoint,
    value: &str,
) -> Result<HttpRequest> {
    match point.location {
        InsertionLocation::Query | InsertionLocation::Form => {
            super::checks::build_probe_request(req, point, value)
        }
        InsertionLocation::Json => {
            ensure!(
                req.body.len() <= MAX_STRUCTURED_BODY_BYTES,
                "JSON 正文超过大小上限"
            );
            let body_slice = req.body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&req.body);
            let mut root: Value =
                serde_json::from_slice(body_slice).context("请求正文不是有效 JSON")?;
            let mut seen = 0usize;
            let ok = mutate_json_leaf(
                &mut root,
                "",
                0,
                &point.name,
                point.occurrence,
                &mut seen,
                value,
            );
            ensure!(
                ok,
                "JSON 参数点不存在: {}[{}]",
                point.name,
                point.occurrence
            );
            let mut next = req.clone();
            next.body = serde_json::to_vec(&root).context("序列化 JSON 正文失败")?;
            sync_content_length(&mut next.headers, next.body.len());
            Ok(next)
        }
        InsertionLocation::Xml => {
            ensure!(
                req.body.len() <= MAX_STRUCTURED_BODY_BYTES,
                "XML 正文超过大小上限"
            );
            let mut next = req.clone();
            next.body = rebuild_xml_body(&req.body, &point.name, point.occurrence, value)?;
            sync_content_length(&mut next.headers, next.body.len());
            Ok(next)
        }
        InsertionLocation::Multipart => {
            ensure!(
                req.body.len() <= MAX_STRUCTURED_BODY_BYTES,
                "Multipart 正文超过大小上限"
            );
            let ct_raw = header_str(&req.headers, "content-type")
                .context("Multipart 请求缺少 Content-Type")?;
            let parsed = parse_intercept_content_type(ct_raw.as_bytes())?;
            let boundary = parsed
                .multipart_boundary()?
                .context("Multipart 请求缺少 boundary")?
                .to_vec();
            let mut next = req.clone();
            next.body =
                rebuild_multipart_body(&req.body, &boundary, &point.name, point.occurrence, value)?;
            sync_content_length(&mut next.headers, next.body.len());
            Ok(next)
        }
        InsertionLocation::Cookie => {
            ensure!(
                !value.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0 | b';')),
                "Cookie 参数值不能包含分号、换行或 NUL"
            );
            let mut next = req.clone();
            let mut seen = 0usize;
            let mut matched = false;
            for h in &mut next.headers {
                if !h.name.eq_ignore_ascii_case("cookie") {
                    continue;
                }
                let raw = std::str::from_utf8(&h.value).context("Cookie 头不是有效 UTF-8")?;
                let mut rebuilt_parts = Vec::new();
                for part in raw.split(';') {
                    let trimmed = part.trim();
                    if let Some((k, v)) = trimmed.split_once('=') {
                        let k_trim = k.trim();
                        if k_trim == point.name {
                            if seen == point.occurrence {
                                let v_trim = v.trim();
                                let quoted = v_trim.starts_with('"')
                                    && v_trim.ends_with('"')
                                    && v_trim.len() >= 2;
                                if quoted {
                                    ensure!(
                                        !value.contains('"'),
                                        "引号包裹的 Cookie 值不能包含双引号"
                                    );
                                    rebuilt_parts.push(format!("{k_trim}=\"{value}\""));
                                } else {
                                    rebuilt_parts.push(format!("{k_trim}={value}"));
                                }
                                matched = true;
                            } else {
                                rebuilt_parts.push(trimmed.to_owned());
                            }
                            seen += 1;
                            continue;
                        }
                    }
                    if !trimmed.is_empty() {
                        rebuilt_parts.push(trimmed.to_owned());
                    }
                }
                if matched {
                    h.value = rebuilt_parts.join("; ").into_bytes();
                    return Ok(next);
                }
            }
            bail!("Cookie 参数点不存在: {}[{}]", point.name, point.occurrence)
        }
        InsertionLocation::Header => {
            ensure!(
                point.name.len() > 2
                    && point
                        .name
                        .get(..2)
                        .is_some_and(|p| p.eq_ignore_ascii_case("x-")),
                "仅允许修改自定义 X-* 请求头"
            );
            ensure!(
                !value.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)),
                "请求头值不能包含换行或 NUL"
            );
            let mut next = req.clone();
            let mut seen = 0usize;
            for h in &mut next.headers {
                if h.name.eq_ignore_ascii_case(&point.name) {
                    if seen == point.occurrence {
                        h.value = value.as_bytes().to_vec();
                        return Ok(next);
                    }
                    seen += 1;
                }
            }
            bail!("Header 参数点不存在: {}[{}]", point.name, point.occurrence)
        }
    }
}
