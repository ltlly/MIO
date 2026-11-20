use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Write as _,
    ops::Range,
    rc::Rc,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;
use crate::{
    HttpExtensionFilter, HttpHistoryFilter, WebSocketDirection, WebSocketHistoryFilter,
    intruder::{PayloadProcessingRule, apply_payload_processing_rule},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rhai::{
    AST, Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position, Scope, Token,
};
use sha2::{Digest, Sha256};

pub const DEFAULT_REQUEST_SCRIPT_TEMPLATE: &str =
    "request.headers[\"X-Forwarded-For\"] = \"127.0.0.1\";\nrequest";
pub const DEFAULT_RESPONSE_SCRIPT_TEMPLATE: &str =
    "response.headers[\"X-Frame-Options\"] = \"DENY\";\nresponse";
pub const DEFAULT_HTTP_FILTER_SCRIPT_TEMPLATE: &str =
    "request.method == \"POST\" && response.status == 200";
pub const DEFAULT_WS_FILTER_SCRIPT_TEMPLATE: &str =
    "message.direction == \"c2s\" && message.payload.contains(\"token\")";

type ScriptResult<T> = std::result::Result<T, Box<EvalAltResult>>;

fn runtime_error(error: impl std::fmt::Display) -> Box<EvalAltResult> {
    error.to_string().into()
}

fn string_field(map: &Map, name: &str) -> Result<String> {
    map.get(name)
        .and_then(|value| value.clone().try_cast::<ImmutableString>())
        .map(|value| value.to_string())
        .with_context(|| format!("MioScript 字段 {name} 必须为字符串"))
}

fn header_map(map: &Map) -> ScriptResult<Map> {
    if let Some(headers) = map.get("headers") {
        headers.clone().try_cast::<Map>().ok_or_else(|| runtime_error("headers 必须为 Map"))
    } else {
        Ok(map.clone())
    }
}

fn header_value(map: Map, name: &str) -> ScriptResult<String> {
    let headers = header_map(&map)?;
    match headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)) {
        Some((_, value)) => value.clone().try_cast::<ImmutableString>()
            .map(|value| value.to_string()).ok_or_else(|| runtime_error("请求头值必须为字符串")),
        None => Ok(String::new()),
    }
}

fn remove_header(map: &mut Map, name: &str) -> ScriptResult<()> {
    let mut headers = header_map(map)?;
    headers.retain(|key, _| !key.eq_ignore_ascii_case(name));
    if map.contains_key("headers") {
        map.insert("headers".into(), headers.into());
    } else {
        *map = headers;
    }
    Ok(())
}

fn set_header(map: &mut Map, name: &str, value: &str) -> ScriptResult<()> {
    if !token(name.as_bytes()) || !valid_header_value_bytes(value.as_bytes()) {
        return Err(runtime_error("HTTP 请求头名称或值无效"));
    }
    remove_header(map, name)?;
    if let Some(headers) = map.get_mut("headers") {
        let mut headers_map = headers.clone().try_cast::<Map>()
            .ok_or_else(|| runtime_error("headers 必须为 Map"))?;
        headers_map.insert(name.into(), value.into());
        *headers = headers_map.into();
    } else {
        map.insert(name.into(), value.into());
    }
    Ok(())
}

fn query_param(map: Map, name: &str) -> ScriptResult<String> {
    let url = Url::parse(&string_field(&map, "url").map_err(runtime_error)?).map_err(runtime_error)?;
    Ok(url.query_pairs().find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned()).unwrap_or_default())
}

fn has_urlencoded_content_type(map: &Map) -> ScriptResult<bool> {
    let headers = header_map(map)?;
    Ok(headers.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("content-type")
            && value
                .clone()
                .try_cast::<ImmutableString>()
                .is_some_and(|text| {
                    text.split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .eq_ignore_ascii_case("application/x-www-form-urlencoded")
                })
    }))
}

fn request_parameter_lookup(map: &Map, name: &str) -> ScriptResult<Option<String>> {
    let url_str = map_string_checked(map, "url")?;
    if !url_str.is_empty() {
        let url = Url::parse(&url_str).map_err(runtime_error)?;
        if let Some((_, value)) = url.query_pairs().find(|(key, _)| key == name) {
            return Ok(Some(value.into_owned()));
        }
    }
    if has_urlencoded_content_type(map)? {
        let body = map_string_checked(map, "body")?;
        if let Some((_, value)) = form_urlencoded::parse(body.as_bytes()).find(|(key, _)| key == name) {
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn sync_request_url_fields(map: &mut Map, url: &Url) {
    let query_str = url.query().unwrap_or_default().to_owned();
    let path_with_query = if query_str.is_empty() {
        url.path().to_owned()
    } else {
        format!("{}?{query_str}", url.path())
    };
    map.insert("path".into(), url.path().into());
    map.insert("path_with_query".into(), path_with_query.into());
    map.insert("query".into(), query_str.into());
    map.insert("url".into(), url.to_string().into());
}

fn set_query_param(map: &mut Map, name: &str, value: &str) -> ScriptResult<()> {
    let mut url =
        Url::parse(&string_field(map, "url").map_err(runtime_error)?).map_err(runtime_error)?;
    let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let mut found = false;
    for (key, current) in &mut pairs {
        if key == name {
            *current = value.into();
            found = true;
        }
    }
    if !found {
        pairs.push((name.into(), value.into()));
    }
    url.query_pairs_mut().clear().extend_pairs(pairs);
    sync_request_url_fields(map, &url);
    Ok(())
}

fn remove_query_param(map: &mut Map, name: &str) -> ScriptResult<()> {
    let mut url =
        Url::parse(&string_field(map, "url").map_err(runtime_error)?).map_err(runtime_error)?;
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != name)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    sync_request_url_fields(map, &url);
    Ok(())
}

fn remove_headers(map: &mut Map, names: Array) -> ScriptResult<()> {
    for item in names {
        let name = item
            .try_cast::<ImmutableString>()
            .ok_or_else(|| runtime_error("remove_headers 参数数组元素必须为字符串"))?;
        remove_header(map, name.as_str())?;
    }
    Ok(())
}

fn parse_cookie_header(raw: &str) -> Vec<(String, Option<String>)> {
    raw.split(';')
        .filter_map(|segment| {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(match trimmed.split_once('=') {
                Some((k, v)) => (k.trim().to_owned(), Some(v.trim().to_owned())),
                None => (trimmed.to_owned(), None),
            })
        })
        .collect()
}

fn join_cookie_header(pairs: &[(String, Option<String>)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| match v {
            Some(val) => format!("{k}={val}"),
            None => k.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn remove_all_cookies(map: &mut Map) -> ScriptResult<()> {
    remove_header(map, "Cookie")
}

fn remove_cookie(map: &mut Map, name: &str) -> ScriptResult<()> {
    let target = name.trim();
    if target.is_empty() || target == "*" {
        return remove_all_cookies(map);
    }
    let existing = header_value(map.clone(), "Cookie")?;
    if existing.is_empty() {
        return Ok(());
    }
    let mut pairs = parse_cookie_header(&existing);
    let before = pairs.len();
    pairs.retain(|(k, _)| k != target);
    if pairs.len() == before {
        return Ok(());
    }
    if pairs.is_empty() {
        remove_header(map, "Cookie")
    } else {
        set_header(map, "Cookie", &join_cookie_header(&pairs))
    }
}

fn set_cookie(map: &mut Map, name: &str, value: &str) -> ScriptResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains([';', '=', '\r', '\n', '\0']) {
        return Err(runtime_error("Cookie 名称无效"));
    }
    if value.contains([';', '\r', '\n', '\0']) {
        return Err(runtime_error("Cookie 值无效"));
    }
    let existing = header_value(map.clone(), "Cookie")?;
    let mut pairs = if existing.is_empty() {
        Vec::new()
    } else {
        parse_cookie_header(&existing)
    };
    if let Some((_, existing_val)) = pairs.iter_mut().find(|(k, _)| k == trimmed) {
        *existing_val = Some(value.to_owned());
    } else {
        pairs.push((trimmed.to_owned(), Some(value.to_owned())));
    }
    set_header(map, "Cookie", &join_cookie_header(&pairs))
}

fn base64url_encode(text: &str) -> String {
    URL_SAFE_NO_PAD.encode(text)
}

fn base64url_decode(text: &str) -> ScriptResult<String> {
    let mut normalized = String::with_capacity(text.len() + 3);
    for ch in text.chars() {
        if ch.is_ascii_whitespace() {
            continue;
        }
        match ch {
            '-' => normalized.push('+'),
            '_' => normalized.push('/'),
            other => normalized.push(other),
        }
    }
    let rem = normalized.len() % 4;
    if rem == 1 {
        return Err(runtime_error("Base64URL 编码长度无效"));
    }
    if rem > 0 {
        for _ in 0..(4 - rem) {
            normalized.push('=');
        }
    }
    let decoded = STANDARD.decode(normalized.as_bytes()).map_err(runtime_error)?;
    String::from_utf8(decoded).map_err(runtime_error)
}

const MAX_JSON_SCRIPT_DEPTH: usize = 32;

fn json_value_to_dynamic(value: serde_json::Value, depth: usize) -> ScriptResult<Dynamic> {
    if depth > MAX_JSON_SCRIPT_DEPTH {
        return Err(runtime_error("JSON 嵌套深度超过 32 层上限"));
    }
    Ok(match value {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(b) => Dynamic::from(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else if let Some(u) = n.as_u64().and_then(|u| i64::try_from(u).ok()) {
                Dynamic::from(u)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from(f)
            } else {
                return Err(runtime_error("JSON 数值超出范围"));
            }
        }
        serde_json::Value::String(s) => Dynamic::from(s),
        serde_json::Value::Array(items) => {
            let mut out = Array::with_capacity(items.len());
            for item in items {
                out.push(json_value_to_dynamic(item, depth + 1)?);
            }
            Dynamic::from_array(out)
        }
        serde_json::Value::Object(obj) => {
            let mut out = Map::new();
            for (k, v) in obj {
                out.insert(k.into(), json_value_to_dynamic(v, depth + 1)?);
            }
            Dynamic::from_map(out)
        }
    })
}

fn dynamic_to_json_value(value: &Dynamic, depth: usize) -> ScriptResult<serde_json::Value> {
    if depth > MAX_JSON_SCRIPT_DEPTH {
        return Err(runtime_error("JSON 嵌套深度超过 32 层上限"));
    }
    if value.is_unit() {
        return Ok(serde_json::Value::Null);
    }
    if let Some(b) = value.clone().try_cast::<bool>() {
        return Ok(serde_json::Value::Bool(b));
    }
    if let Some(i) = value.clone().try_cast::<i64>() {
        return Ok(serde_json::Value::Number(i.into()));
    }
    if let Some(f) = value.clone().try_cast::<f64>() {
        return Ok(serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null));
    }
    if let Some(s) = value.clone().try_cast::<ImmutableString>() {
        return Ok(serde_json::Value::String(s.into_owned()));
    }
    if let Some(arr) = value.clone().try_cast::<Array>() {
        let mut out = Vec::with_capacity(arr.len());
        for item in &arr {
            out.push(dynamic_to_json_value(item, depth + 1)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Some(map) = value.clone().try_cast::<Map>() {
        let mut out = serde_json::Map::new();
        for (k, v) in &map {
            out.insert(k.to_string(), dynamic_to_json_value(v, depth + 1)?);
        }
        return Ok(serde_json::Value::Object(out));
    }
    Ok(serde_json::Value::String(value.to_string()))
}

fn json_parse(text: &str) -> ScriptResult<Dynamic> {
    if text.len() > MAX_BODY {
        return Err(runtime_error("JSON 文本超过 16 MiB 上限"));
    }
    let val: serde_json::Value = serde_json::from_str(text).map_err(runtime_error)?;
    json_value_to_dynamic(val, 0)
}

fn json_stringify(value: Dynamic) -> ScriptResult<String> {
    let val = dynamic_to_json_value(&value, 0)?;
    let s = serde_json::to_string(&val).map_err(runtime_error)?;
    if s.len() > MAX_BODY {
        return Err(runtime_error("JSON 序列化结果超过 16 MiB 上限"));
    }
    Ok(s)
}

fn json_stringify_pretty(value: Dynamic) -> ScriptResult<String> {
    let val = dynamic_to_json_value(&value, 0)?;
    let s = serde_json::to_string_pretty(&val).map_err(runtime_error)?;
    if s.len() > MAX_BODY {
        return Err(runtime_error("JSON 序列化结果超过 16 MiB 上限"));
    }
    Ok(s)
}

fn json_get(text: &str, path: &str) -> ScriptResult<Dynamic> {
    if text.len() > MAX_BODY {
        return Err(runtime_error("JSON 文本超过 16 MiB 上限"));
    }
    let root: serde_json::Value = serde_json::from_str(text).map_err(runtime_error)?;
    let mut cur = &root;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        match cur {
            serde_json::Value::Object(obj) => {
                let Some(next) = obj.get(seg) else {
                    return Ok(Dynamic::UNIT);
                };
                cur = next;
            }
            serde_json::Value::Array(arr) => {
                let Ok(idx) = seg.parse::<usize>() else {
                    return Ok(Dynamic::UNIT);
                };
                let Some(next) = arr.get(idx) else {
                    return Ok(Dynamic::UNIT);
                };
                cur = next;
            }
            _ => return Ok(Dynamic::UNIT),
        }
    }
    json_value_to_dynamic(cur.clone(), 0)
}

fn set_json_field(map: &mut Map, path: &str, value: Dynamic) -> ScriptResult<()> {
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(runtime_error("JSON 字段路径不能为空"));
    }
    let body = map_string_checked(map, "body")?;
    let mut root = if body.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str::<serde_json::Value>(&body).map_err(runtime_error)?
    };
    let leaf_value = dynamic_to_json_value(&value, 0)?;
    let mut cur = &mut root;
    for (idx, seg) in segments.iter().enumerate() {
        if idx + 1 == segments.len() {
            match cur {
                serde_json::Value::Object(obj) => {
                    obj.insert((*seg).to_owned(), leaf_value);
                }
                serde_json::Value::Array(arr) => {
                    let arr_ix = seg
                        .parse::<usize>()
                        .map_err(|_| runtime_error(format!("JSON 数组索引无效：{seg}")))?;
                    let slot = arr
                        .get_mut(arr_ix)
                        .ok_or_else(|| runtime_error(format!("JSON 数组索引越界：{seg}")))?;
                    *slot = leaf_value;
                }
                _ => {
                    return Err(runtime_error(format!(
                        "JSON 路径 `{seg}` 父节点不是对象或数组"
                    )));
                }
            }
            break;
        }
        match cur {
            serde_json::Value::Object(obj) => {
                let entry = obj
                    .entry((*seg).to_owned())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                if entry.is_null() {
                    *entry = serde_json::Value::Object(serde_json::Map::new());
                }
                cur = entry;
            }
            serde_json::Value::Array(arr) => {
                let arr_ix = seg
                    .parse::<usize>()
                    .map_err(|_| runtime_error(format!("JSON 数组索引无效：{seg}")))?;
                cur = arr
                    .get_mut(arr_ix)
                    .ok_or_else(|| runtime_error(format!("JSON 数组索引越界：{seg}")))?;
            }
            _ => {
                return Err(runtime_error(format!(
                    "JSON 路径 `{seg}` 父节点不是对象或数组"
                )));
            }
        }
    }
    let serialized = serde_json::to_string(&root).map_err(runtime_error)?;
    if serialized.len() > MAX_BODY {
        return Err(runtime_error("JSON 正文超过 16 MiB 上限"));
    }
    map.insert("body".into(), serialized.into());
    Ok(())
}

fn hex_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.as_bytes() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_decode(hex: &str) -> ScriptResult<String> {
    let clean: Vec<u8> = hex
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !clean.len().is_multiple_of(2) {
        return Err(runtime_error("Hex 编码长度必须为偶数"));
    }
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for chunk in clean.chunks_exact(2) {
        let hi = (chunk[0] as char)
            .to_digit(16)
            .ok_or_else(|| runtime_error("Hex 编码包含非法字符"))?;
        let lo = (chunk[1] as char)
            .to_digit(16)
            .ok_or_else(|| runtime_error("Hex 编码包含非法字符"))?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    String::from_utf8(bytes).map_err(runtime_error)
}

fn html_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let digits = entity.strip_prefix('#')?;
            let code = if let Some(hex) = digits
                .strip_prefix('x')
                .or_else(|| digits.strip_prefix('X'))
            {
                if hex.is_empty() {
                    return None;
                }
                u32::from_str_radix(hex, 16).ok()?
            } else {
                if digits.is_empty() {
                    return None;
                }
                digits.parse::<u32>().ok()?
            };
            char::from_u32(code)
        }
    }
}

fn html_decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        if let Some(rel_amp) = rest.find('&') {
            out.push_str(&rest[..rel_amp]);
            let after_amp = &rest[rel_amp + 1..];
            let search_window = after_amp.len().min(16);
            if let Some(semi) = after_amp[..search_window].find(';')
                && !after_amp[..semi].contains('&')
                && let Some(ch) = decode_html_entity(&after_amp[..semi])
            {
                out.push(ch);
                i += rel_amp + 1 + semi + 1;
                continue;
            }
            out.push('&');
            i += rel_amp + 1;
        } else {
            out.push_str(rest);
            break;
        }
    }
    out
}

fn hmac_sha256_bytes(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block_key = [0u8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        block_key[..32].copy_from_slice(&digest);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for idx in 0..64 {
        ipad[idx] ^= block_key[idx];
        opad[idx] ^= block_key[idx];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn hmac_sha256_hex(key: &str, message: &str) -> String {
    let mac = hmac_sha256_bytes(key.as_bytes(), message.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in mac {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hmac_sha256_base64(key: &str, message: &str) -> String {
    STANDARD.encode(hmac_sha256_bytes(key.as_bytes(), message.as_bytes()))
}

fn hmac_sha256_base64url(key: &str, message: &str) -> String {
    URL_SAFE_NO_PAD.encode(hmac_sha256_bytes(key.as_bytes(), message.as_bytes()))
}

fn jwt_decode(token: &str) -> ScriptResult<Map> {
    let mut trimmed = token.trim();
    if let Some(prefix) = trimmed.get(..6)
        && prefix.eq_ignore_ascii_case("bearer")
        && trimmed
            .as_bytes()
            .get(6)
            .is_some_and(u8::is_ascii_whitespace)
    {
        trimmed = trimmed[6..].trim();
    }
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() != 3 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(runtime_error("JWT 格式无效：必须包含 Header.Payload.Signature 三段"));
    }
    let raw_header = base64url_decode(parts[0])?;
    let raw_payload = base64url_decode(parts[1])?;
    let header = json_parse(&raw_header)?;
    let payload = json_parse(&raw_payload)?;
    let mut out = Map::new();
    out.insert("header".into(), header);
    out.insert("payload".into(), payload);
    out.insert("signature".into(), parts[2].to_owned().into());
    out.insert("raw_header".into(), raw_header.into());
    out.insert("raw_payload".into(), raw_payload.into());
    Ok(out)
}

fn parse_cookies(map: &mut Map) -> ScriptResult<Map> {
    let raw = header_value(map.clone(), "Cookie")?;
    let mut out = Map::new();
    for (name, value) in parse_cookie_header(&raw) {
        out.insert(name.into(), value.unwrap_or_default().into());
    }
    Ok(out)
}

fn form_param(map: &mut Map, name: &str) -> ScriptResult<String> {
    let body = map_string_checked(map, "body")?;
    Ok(form_urlencoded::parse(body.as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default())
}

fn set_form_param(map: &mut Map, name: &str, value: &str) -> ScriptResult<()> {
    let body = map_string_checked(map, "body")?;
    let mut pairs: Vec<(String, String)> = form_urlencoded::parse(body.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut found = false;
    for (key, current) in &mut pairs {
        if key == name {
            *current = value.to_owned();
            found = true;
        }
    }
    if !found {
        pairs.push((name.to_owned(), value.to_owned()));
    }
    let serialized = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    if serialized.len() > MAX_BODY {
        return Err(runtime_error("表单正文超过 16 MiB 上限"));
    }
    map.insert("body".into(), serialized.into());
    Ok(())
}

fn remove_form_param(map: &mut Map, name: &str) -> ScriptResult<()> {
    let body = map_string_checked(map, "body")?;
    let pairs: Vec<(String, String)> = form_urlencoded::parse(body.as_bytes())
        .filter(|(key, _)| key != name)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let serialized = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    map.insert("body".into(), serialized.into());
    Ok(())
}

const MIO_SCRIPT_REGEX_CACHE_LIMIT: usize = 256;

fn script_regex(pattern: &str) -> ScriptResult<Regex> {
    if pattern.len() > 4096 {
        return Err(runtime_error("MioScript 正则表达式超过 4096 字节"));
    }
    static CACHE: OnceLock<Mutex<HashMap<String, std::result::Result<Regex, String>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = guard.get(pattern) {
            return cached.clone().map_err(runtime_error);
        }
    }
    let compiled = RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .map_err(|err| err.to_string());
    {
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.len() >= MIO_SCRIPT_REGEX_CACHE_LIMIT && !guard.contains_key(pattern) {
            guard.clear();
        }
        guard.insert(pattern.to_owned(), compiled.clone());
    }
    compiled.map_err(runtime_error)
}

fn map_string_checked(map: &Map, key: &str) -> ScriptResult<String> {
    map.get(key)
        .and_then(|value| value.clone().try_cast::<ImmutableString>())
        .map(|value| value.to_string())
        .ok_or_else(|| runtime_error(format!("MioScript 字段 {key} 缺失或类型无效")))
}

fn map_bool_checked(map: &Map, key: &str) -> ScriptResult<bool> {
    map.get(key)
        .and_then(|value| value.clone().try_cast::<bool>())
        .ok_or_else(|| runtime_error(format!("MioScript 布尔字段 {key} 缺失或类型无效")))
}

fn map_int_checked(map: &Map, key: &str) -> ScriptResult<i64> {
    map.get(key)
        .and_then(|value| value.clone().try_cast::<i64>())
        .ok_or_else(|| runtime_error(format!("MioScript 整数字段 {key} 缺失或类型无效")))
}

fn map_contains_text(map: &Map, pattern: &str, case_sensitive: bool) -> ScriptResult<bool> {
    let haystack = if map.contains_key("search_text") {
        map_string_checked(map, "search_text")?
    } else {
        map_string_checked(map, "raw")?
    };
    Ok(if case_sensitive {
        haystack.contains(pattern)
    } else {
        haystack
            .to_ascii_lowercase()
            .contains(&pattern.to_ascii_lowercase())
    })
}

fn highlight_color_name(color: u8) -> Result<&'static str> {
    match color {
        0 => Ok("NONE"),
        1 => Ok("RED"),
        2 => Ok("ORANGE"),
        3 => Ok("YELLOW"),
        4 => Ok("GREEN"),
        5 => Ok("CYAN"),
        6 => Ok("BLUE"),
        7 => Ok("PINK"),
        8 => Ok("MAGENTA"),
        9 => Ok("GRAY"),
        other => bail!("MioScript 高亮颜色编码无效：{other}"),
    }
}

#[derive(Clone, Copy)]
enum HighlightFilterState {
    Boolean(bool),
    ColorCode(u8),
}

fn mio_script_mime_type(response: Option<&HttpResponse>) -> &'static str {
    let Some(response) = response else {
        return "NONE";
    };
    if crate::http::infer_history_display_mime_type(&response.headers, &response.body) == "JSON" {
        return "JSON";
    }
    for header in response
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("content-type"))
    {
        let essence = String::from_utf8_lossy(
            header
                .value
                .split(|&byte| byte == b';')
                .next()
                .unwrap_or_default()
                .trim_ascii(),
        )
        .to_ascii_lowercase();
        match essence.as_str() {
            "image/png" => return "IMAGE_PNG",
            "image/jpeg" | "image/jpg" => return "IMAGE_JPEG",
            "image/gif" => return "IMAGE_GIF",
            "image/bmp" => return "IMAGE_BMP",
            "image/tiff" => return "IMAGE_TIFF",
            "image/svg+xml" => return "IMAGE_SVG_XML",
            "font/woff" | "application/font-woff" => return "FONT_WOFF",
            "font/woff2" | "application/font-woff2" => return "FONT_WOFF2",
            "text/event-stream" => return "SSE",
            "application/yaml" | "text/yaml" | "application/x-yaml" => return "YAML",
            "application/rtf" | "text/rtf" => return "RTF",
            _ => {}
        }
    }
    let body = &response.body;
    if body.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "IMAGE_PNG";
    }
    if body.starts_with(b"\xff\xd8\xff") {
        return "IMAGE_JPEG";
    }
    if body.starts_with(b"GIF87a") || body.starts_with(b"GIF89a") {
        return "IMAGE_GIF";
    }
    if body.starts_with(b"BM") {
        return "IMAGE_BMP";
    }
    match crate::store::ext_archive::http_query::filters::infer_history_mime(&response.headers, body)
        .index()
    {
        0 => "HTML",
        1 => "SCRIPT",
        2 => "XML",
        3 => "CSS",
        4 => "PLAIN_TEXT",
        5 => "IMAGE_UNKNOWN",
        6 => "APPLICATION_FLASH",
        _ => {
            if body.is_empty()
                && !response
                    .headers
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case("content-type"))
            {
                "NONE"
            } else {
                "UNRECOGNIZED"
            }
        }
    }
}

fn register_script_methods(engine: &mut Engine) {
    engine.register_fn("request", |map: &mut Map| -> ScriptResult<Map> {
        map.get("request")
            .and_then(|value| value.clone().try_cast::<Map>())
            .ok_or_else(|| runtime_error("MioScript request 对象缺失"))
    });
    engine.register_fn("response", |map: &mut Map| -> ScriptResult<Map> {
        if !map_bool_checked(map, "has_response")? {
            return Err(runtime_error("当前记录没有响应，无法访问 response()"));
        }
        map.get("response")
            .and_then(|value| value.clone().try_cast::<Map>())
            .ok_or_else(|| runtime_error("MioScript response 对象缺失"))
    });
    engine.register_fn("annotations", |map: &mut Map| -> ScriptResult<Map> {
        map.get("annotations")
            .and_then(|value| value.clone().try_cast::<Map>())
            .ok_or_else(|| runtime_error("MioScript annotations 对象缺失"))
    });
    engine.register_fn("hasResponse", |map: &mut Map| -> ScriptResult<bool> {
        map_bool_checked(map, "has_response")
    });
    engine.register_fn("isInScope", |map: &mut Map| -> ScriptResult<bool> {
        map_bool_checked(map, "in_scope")
    });
    engine.register_fn("hasParameters", |map: &mut Map| -> ScriptResult<bool> {
        map_bool_checked(map, "parameterized")
    });
    engine.register_fn("method", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "method")
    });
    engine.register_fn("path", |map: &mut Map| -> ScriptResult<String> {
        if map.contains_key("path_with_query") {
            map_string_checked(map, "path_with_query")
        } else {
            map_string_checked(map, "path")
        }
    });
    engine.register_fn("pathWithoutQuery", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "path")
    });
    engine.register_fn("query", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "query")
    });
    engine.register_fn("url", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "url")
    });
    engine.register_fn("host", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "host")
    });
    engine.register_fn("httpVersion", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "version")
    });
    engine.register_fn("bodyToString", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "body")
    });
    engine.register_fn("statusCode", |map: &mut Map| -> ScriptResult<i64> {
        let status = map_int_checked(map, "status")?;
        if status <= 0 {
            return Err(runtime_error("当前记录没有有效响应状态码"));
        }
        Ok(status)
    });
    engine.register_fn("mimeType", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "mime_type")
    });
    engine.register_fn("listenerPort", |map: &mut Map| -> ScriptResult<i64> {
        let port = map_int_checked(map, "listener_port")?;
        if port <= 0 {
            return Err(runtime_error("当前记录缺少有效监听端口"));
        }
        Ok(port)
    });
    engine.register_fn("notes", |map: &mut Map| -> ScriptResult<String> {
        map_string_checked(map, "notes")
    });
    engine.register_fn("hasNotes", |map: &mut Map| -> ScriptResult<bool> {
        Ok(!map_string_checked(map, "notes")?.is_empty())
    });
    engine.register_fn("highlightColor", |map: &mut Map| -> ScriptResult<String> {
        if map
            .get("highlight_color_unknown")
            .and_then(|value| value.clone().try_cast::<bool>())
            == Some(true)
        {
            return Err(runtime_error(
                "当前入口未提供具体高亮颜色，无法调用 highlightColor()",
            ));
        }
        map_string_checked(map, "highlight_color")
    });
    engine.register_fn("hasHighlightColor", |map: &mut Map| -> ScriptResult<bool> {
        map_bool_checked(map, "highlighted")
    });
    engine.register_fn("headerValue", |map: &mut Map, name: &str| -> ScriptResult<String> {
        header_value(map.clone(), name)
    });
    engine.register_fn("hasHeader", |map: &mut Map, name: &str| -> ScriptResult<bool> {
        Ok(header_map(map)?.keys().any(|key| key.eq_ignore_ascii_case(name)))
    });
    engine.register_fn("parameterValue", |map: &mut Map, name: &str| -> ScriptResult<String> {
        Ok(request_parameter_lookup(map, name)?.unwrap_or_default())
    });
    engine.register_fn("hasParameter", |map: &mut Map, name: &str| -> ScriptResult<bool> {
        Ok(request_parameter_lookup(map, name)?.is_some())
    });
    engine.register_fn(
        "contains",
        |map: &mut Map, pattern: &str, case_sensitive: bool| -> ScriptResult<bool> {
            map_contains_text(map, pattern, case_sensitive)
        },
    );
    engine.register_fn("contains", |map: &mut Map, pattern: &str| -> ScriptResult<bool> {
        map_contains_text(map, pattern, true)
    });
    engine.register_fn("urlEncode", |_: &mut Map, text: &str| -> String {
        form_urlencoded::byte_serialize(text.as_bytes()).collect()
    });
    engine.register_fn("urlDecode", |_: &mut Map, text: &str| -> ScriptResult<String> {
        let encoded = format!("value={text}");
        let (_, value) = form_urlencoded::parse(encoded.as_bytes())
            .next()
            .ok_or_else(|| runtime_error("URL 解码结果缺失"))?;
        Ok(value.into_owned())
    });
    engine.register_fn("base64Encode", |_: &mut Map, text: &str| -> String {
        STANDARD.encode(text)
    });
    engine.register_fn("base64Decode", |_: &mut Map, text: &str| -> ScriptResult<String> {
        String::from_utf8(STANDARD.decode(text).map_err(runtime_error)?).map_err(runtime_error)
    });
    engine.register_fn("base64UrlEncode", |_: &mut Map, text: &str| -> String {
        base64url_encode(text)
    });
    engine.register_fn(
        "base64UrlDecode",
        |_: &mut Map, text: &str| -> ScriptResult<String> { base64url_decode(text) },
    );
    engine.register_fn("setHeader", set_header);
    engine.register_fn("removeHeader", remove_header);
    engine.register_fn("removeHeaders", remove_headers);
    engine.register_fn("removeAllCookies", remove_all_cookies);
    engine.register_fn("removeCookie", remove_cookie);
    engine.register_fn("setCookie", set_cookie);
    engine.register_fn("setQueryParam", set_query_param);
    engine.register_fn("removeQueryParam", remove_query_param);
    engine.register_fn("sha256Hex", |_: &mut Map, text: &str| -> ScriptResult<String> {
        let out = apply_payload_processing_rule(
            &[text.as_bytes().to_vec()],
            &PayloadProcessingRule::Sha256Hex,
        )
        .map_err(runtime_error)?;
        String::from_utf8(out.into_iter().next().unwrap_or_default()).map_err(runtime_error)
    });
    engine.register_fn("md5Hex", |_: &mut Map, text: &str| -> ScriptResult<String> {
        let out = apply_payload_processing_rule(
            &[text.as_bytes().to_vec()],
            &PayloadProcessingRule::Md5Hex,
        )
        .map_err(runtime_error)?;
        String::from_utf8(out.into_iter().next().unwrap_or_default()).map_err(runtime_error)
    });
    engine.register_fn("jsonParse", |_: &mut Map, text: &str| -> ScriptResult<Dynamic> {
        json_parse(text)
    });
    engine.register_fn(
        "jsonStringify",
        |_: &mut Map, value: Dynamic| -> ScriptResult<String> { json_stringify(value) },
    );
    engine.register_fn(
        "jsonStringifyPretty",
        |_: &mut Map, value: Dynamic| -> ScriptResult<String> { json_stringify_pretty(value) },
    );
    engine.register_fn(
        "jsonGet",
        |_: &mut Map, text: &str, path: &str| -> ScriptResult<Dynamic> { json_get(text, path) },
    );
    engine.register_fn("hexEncode", |_: &mut Map, text: &str| -> String {
        hex_encode(text)
    });
    engine.register_fn("hexDecode", |_: &mut Map, hex: &str| -> ScriptResult<String> {
        hex_decode(hex)
    });
    engine.register_fn("htmlEncode", |_: &mut Map, text: &str| -> String {
        html_encode(text)
    });
    engine.register_fn("htmlDecode", |_: &mut Map, text: &str| -> String {
        html_decode(text)
    });
    engine.register_fn(
        "hmacSha256Hex",
        |_: &mut Map, key: &str, message: &str| -> String { hmac_sha256_hex(key, message) },
    );
    engine.register_fn(
        "hmacSha256Base64",
        |_: &mut Map, key: &str, message: &str| -> String { hmac_sha256_base64(key, message) },
    );
    engine.register_fn(
        "hmacSha256Base64Url",
        |_: &mut Map, key: &str, message: &str| -> String { hmac_sha256_base64url(key, message) },
    );
    engine.register_fn("jwtDecode", |_: &mut Map, token: &str| -> ScriptResult<Map> {
        jwt_decode(token)
    });
    engine.register_fn("setJsonField", set_json_field);
    engine.register_fn("parseCookies", parse_cookies);
    engine.register_fn("formParam", form_param);
    engine.register_fn("setFormParam", set_form_param);
    engine.register_fn("removeFormParam", remove_form_param);
}

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(50_000);
    engine.set_max_string_size(MAX_BODY);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(4096);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(64, 32);
    engine.register_fn("url_encode", |text: &str| -> String {
        form_urlencoded::byte_serialize(text.as_bytes()).collect()
    });
    engine.register_fn("url_decode", |text: &str| -> ScriptResult<String> {
        let encoded = format!("value={text}");
        let (_, value) = form_urlencoded::parse(encoded.as_bytes()).next()
            .ok_or_else(|| runtime_error("URL 解码结果缺失"))?;
        Ok(value.into_owned())
    });
    engine.register_fn("base64_encode", |text: &str| -> String { STANDARD.encode(text) });
    engine.register_fn("base64_decode", |text: &str| -> ScriptResult<String> {
        String::from_utf8(STANDARD.decode(text).map_err(runtime_error)?).map_err(runtime_error)
    });
    engine.register_fn("base64url_encode", base64url_encode);
    engine.register_fn("base64url_decode", base64url_decode);
    engine.register_fn("sha256_hex", |text: &str| -> ScriptResult<String> {
        let out = apply_payload_processing_rule(&[text.as_bytes().to_vec()], &PayloadProcessingRule::Sha256Hex)
            .map_err(runtime_error)?;
        String::from_utf8(out.into_iter().next().unwrap_or_default()).map_err(runtime_error)
    });
    engine.register_fn("md5_hex", |text: &str| -> ScriptResult<String> {
        let out = apply_payload_processing_rule(&[text.as_bytes().to_vec()], &PayloadProcessingRule::Md5Hex)
            .map_err(runtime_error)?;
        String::from_utf8(out.into_iter().next().unwrap_or_default()).map_err(runtime_error)
    });
    engine.register_fn("crc32_hex", |text: &str| -> String { format!("{:08x}", crc32fast::hash(text.as_bytes())) });
    engine.register_fn("regex_matches", |text: &str, pattern: &str| -> ScriptResult<bool> {
        Ok(script_regex(pattern)?.is_match(text))
    });
    engine.register_fn("regex_replace", |text: &str, pattern: &str, replacement: &str| -> ScriptResult<String> {
        let rule = MatchAndReplaceRule {
            match_pattern: pattern.into(), replace_with: replacement.into(), regex: true,
            ..Default::default()
        };
        let regex = script_regex(pattern)?;
        Ok(replace_text_with_rule(text, &rule, Some(&regex), MAX_BODY, &|| Ok(()))
            .map_err(runtime_error)?.unwrap_or_else(|| text.to_owned()))
    });
    engine.register_fn("unix_timestamp", || -> ScriptResult<i64> {
        i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).map_err(runtime_error)?.as_secs())
            .map_err(runtime_error)
    });
    engine.register_fn("unix_timestamp_ms", || -> ScriptResult<i64> {
        i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).map_err(runtime_error)?.as_millis())
            .map_err(runtime_error)
    });
    engine.register_fn("header_value", header_value);
    engine.register_fn("set_header", set_header);
    engine.register_fn("remove_header", remove_header);
    engine.register_fn("remove_headers", remove_headers);
    engine.register_fn("remove_all_cookies", remove_all_cookies);
    engine.register_fn("remove_cookie", remove_cookie);
    engine.register_fn("set_cookie", set_cookie);
    engine.register_fn("has_header", |map: Map, name: &str| -> ScriptResult<bool> {
        Ok(header_map(&map)?.keys().any(|key| key.eq_ignore_ascii_case(name)))
    });
    engine.register_fn("query_param", query_param);
    engine.register_fn("set_query_param", set_query_param);
    engine.register_fn("remove_query_param", remove_query_param);
    engine.register_fn("json_parse", json_parse);
    engine.register_fn("json_stringify", json_stringify);
    engine.register_fn("json_stringify_pretty", json_stringify_pretty);
    engine.register_fn("json_get", json_get);
    engine.register_fn("set_json_field", set_json_field);
    engine.register_fn("hex_encode", hex_encode);
    engine.register_fn("hex_decode", hex_decode);
    engine.register_fn("html_encode", html_encode);
    engine.register_fn("html_decode", html_decode);
    engine.register_fn("hmac_sha256_hex", hmac_sha256_hex);
    engine.register_fn("hmac_sha256_base64", hmac_sha256_base64);
    engine.register_fn("hmac_sha256_base64url", hmac_sha256_base64url);
    engine.register_fn("jwt_decode", jwt_decode);
    engine.register_fn("parse_cookies", parse_cookies);
    engine.register_fn("form_param", form_param);
    engine.register_fn("set_form_param", set_form_param);
    engine.register_fn("remove_form_param", remove_form_param);
    register_script_methods(&mut engine);
    engine
}

const MIO_SCRIPT_AST_CACHE_LIMIT: usize = 128;

thread_local! {
    static ACTIVE_CANCEL: RefCell<Option<tokio_util::sync::CancellationToken>> =
        const { RefCell::new(None) };
    static MIO_SCRIPT_ENGINE: Engine = {
        let mut eng = engine();
        eng.on_progress(|_| {
            ACTIVE_CANCEL.with(|cell| {
                if cell
                    .borrow()
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
                {
                    Some(Dynamic::from("HTTP 历史查询已取消"))
                } else {
                    None
                }
            })
        });
        eng
    };
    static MIO_SCRIPT_AST_CACHE: RefCell<HashMap<String, Rc<AST>>> = RefCell::new(HashMap::new());
}

struct ActiveCancelGuard(Option<tokio_util::sync::CancellationToken>);

impl ActiveCancelGuard {
    fn set(token: Option<tokio_util::sync::CancellationToken>) -> Self {
        let previous = ACTIVE_CANCEL.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), token));
        Self(previous)
    }
}

impl Drop for ActiveCancelGuard {
    fn drop(&mut self) {
        ACTIVE_CANCEL.with(|cell| {
            *cell.borrow_mut() = self.0.take();
        });
    }
}

fn compile(engine: &Engine, script: &str) -> Result<AST> {
    ensure!(!script.trim().is_empty(), "脚本不能为空");
    ensure!(script.len() <= 64 * 1024, "脚本超过 64 KiB 上限");
    engine.compile(script).map_err(|error| anyhow::anyhow!("MioScript 编译错误：{error}"))
}

fn compile_cached(engine: &Engine, script: &str) -> Result<Rc<AST>> {
    ensure!(!script.trim().is_empty(), "脚本不能为空");
    ensure!(script.len() <= 64 * 1024, "脚本超过 64 KiB 上限");
    if let Some(ast) = MIO_SCRIPT_AST_CACHE.with(|cache| cache.borrow().get(script).cloned()) {
        return Ok(ast);
    }
    let ast = Rc::new(compile(engine, script)?);
    MIO_SCRIPT_AST_CACHE.with(|cache| {
        let mut map = cache.borrow_mut();
        if map.len() >= MIO_SCRIPT_AST_CACHE_LIMIT && !map.contains_key(script) {
            map.clear();
        }
        map.insert(script.to_owned(), Rc::clone(&ast));
    });
    Ok(ast)
}

pub fn validate_filter_script(script: &str) -> Result<()> {
    MIO_SCRIPT_ENGINE.with(|engine| compile_cached(engine, script).map(|_| ()))
}

fn headers_to_map(headers: &[Header]) -> Map {
    headers.iter().map(|header| (header.name.clone().into(),
        Dynamic::from(String::from_utf8_lossy(&header.value).into_owned()))).collect()
}

fn request_map(request: &HttpRequest) -> Result<Map> {
    let url = Url::parse(&request.url).context("MioScript 请求 URL 无效")?;
    let path_with_query = match url.query() {
        Some(q) if !q.is_empty() => format!("{}?{q}", url.path()),
        _ => url.path().to_owned(),
    };
    Ok([
        ("method", request.method.clone().into()),
        ("url", request.url.clone().into()),
        ("path", url.path().into()),
        ("path_with_query", path_with_query.into()),
        ("query", url.query().unwrap_or_default().into()),
        ("host", url.host_str().context("MioScript 请求 URL 缺少主机")?.into()),
        ("version", request.version.as_str().into()),
        ("headers", headers_to_map(&request.headers).into()),
        ("body", String::from_utf8_lossy(&request.body).into_owned().into()),
        ("raw", String::from_utf8_lossy(&request_to_raw(request)).into_owned().into()),
    ].into_iter().map(|(key, value)| (key.into(), value)).collect())
}

fn response_map(response: Option<&HttpResponse>) -> Map {
    [
        ("status", response.map_or(0, |response| i64::from(response.status)).into()),
        ("version", response.map_or("", |response| response.version.as_str()).into()),
        ("headers", response.map(|response| headers_to_map(&response.headers)).unwrap_or_default().into()),
        ("body", response.map(|response| String::from_utf8_lossy(&response.body).into_owned()).unwrap_or_default().into()),
        ("raw", response.map(|response| String::from_utf8_lossy(&response_to_raw(response)).into_owned()).unwrap_or_default().into()),
    ].into_iter().map(|(key, value)| (key.into(), value)).collect()
}

pub(super) fn version_from_script(version: &str) -> Result<HttpVersion> {
    match version {
        "HTTP/1.0" => Ok(HttpVersion::Http10),
        "HTTP/1.1" => Ok(HttpVersion::Http11),
        "HTTP/2" | "HTTP/2.0" => Ok(HttpVersion::Http2),
        _ => bail!("MioScript HTTP 版本无效：{version}"),
    }
}

fn sync_headers(headers: &mut Vec<Header>, map: &Map) -> Result<()> {
    let desired = map.get("headers").and_then(|value| value.clone().try_cast::<Map>())
        .context("MioScript headers 必须为 Map")?;
    let original = headers_to_map(headers);
    headers.retain(|header| desired.contains_key(header.name.as_str()));
    for (name, value) in desired {
        let text = value.clone().try_cast::<ImmutableString>().context("MioScript 请求头值必须为字符串")?;
        ensure!(token(name.as_bytes()) && valid_header_value_bytes(text.as_bytes()), "MioScript 请求头名称或值无效");
        if original.get(&name).is_some_and(|old| old.clone().try_cast::<ImmutableString>() == Some(text.clone())) {
            continue;
        }
        headers.retain(|header| !header.name.eq_ignore_ascii_case(&name));
        headers.push(Header { name: name.to_string(), value: text.as_bytes().to_vec() });
    }
    ensure!(headers.len() <= 1024 && headers.iter().map(|header| header.name.len() + header.value.len() + 4).sum::<usize>() <= MAX_HEAD,
        "MioScript HTTP 请求头超过限额");
    Ok(())
}

pub(super) fn sync_content_length(headers: &mut Vec<Header>, body_len: usize) {
    if is_chunked(headers) { return; }
    let mut present = false;
    headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case("content-length") { return true; }
        if present { return false; }
        present = true;
        header.value = body_len.to_string().into_bytes();
        true
    });
    if !present && body_len > 0 {
        headers.push(Header { name: "Content-Length".into(), value: body_len.to_string().into_bytes() });
    }
}

fn apply_request_map(request: &mut HttpRequest, map: &Map) -> Result<()> {
    let raw = string_field(map, "raw")?;
    if raw != String::from_utf8_lossy(&request_to_raw(request)) {
        let version = detect_request_line_version(raw.as_bytes()).context("MioScript 请求首行无效")?;
        *request = parse_repeater_request(raw.as_bytes(), &request.url, version, true)?;
        return Ok(());
    }
    let method = string_field(map, "method")?;
    ensure!(token(method.as_bytes()), "MioScript 请求方法无效");
    let mut next_url = string_field(map, "url")?;
    let mut url = Url::parse(&next_url).context("MioScript 请求 URL 无效")?;
    let original = Url::parse(&request.url)?;
    if next_url == request.url {
        let path = string_field(map, "path")?;
        let query = string_field(map, "query")?;
        if path != original.path() || query != original.query().unwrap_or_default() {
            url.set_path(&path);
            url.set_query((!query.is_empty()).then_some(query.as_str()));
            next_url = url.to_string();
        }
    }
    ensure!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
        && url.username().is_empty() && url.password().is_none() && url.fragment().is_none()
        && !next_url.chars().any(|character| character.is_control() || character.is_whitespace()), "MioScript 请求 URL 无效");
    request.method = method;
    request.url = next_url;
    request.version = version_from_script(&string_field(map, "version")?)?;
    sync_headers(&mut request.headers, map)?;
    let body = string_field(map, "body")?;
    if body != String::from_utf8_lossy(&request.body) { request.body = body.into_bytes(); }
    ensure!(request.body.len() <= MAX_BODY, "MioScript 请求正文超过 16 MiB 上限");
    sync_content_length(&mut request.headers, request.body.len());
    Ok(())
}

fn apply_response_map(response: &mut HttpResponse, map: &Map) -> Result<()> {
    let raw = string_field(map, "raw")?;
    if raw != String::from_utf8_lossy(&response_to_raw(response)) {
        let first = head(raw.as_bytes())?.0;
        let version = std::str::from_utf8(&first)?.split_ascii_whitespace().next().context("MioScript 响应首行缺失")?;
        *response = parse_response(raw.as_bytes(), version_from_script(version)?)?;
        sync_content_length(&mut response.headers, response.body.len());
        return Ok(());
    }
    let status = map.get("status").and_then(|value| value.clone().try_cast::<i64>()).context("MioScript status 必须为整数")?;
    ensure!((100..=599).contains(&status), "MioScript 响应状态码无效");
    response.status = status as u16;
    response.version = version_from_script(&string_field(map, "version")?)?;
    sync_headers(&mut response.headers, map)?;
    let body = string_field(map, "body")?;
    if body != String::from_utf8_lossy(&response.body) { response.body = body.into_bytes(); }
    ensure!(response.body.len() <= MAX_BODY, "MioScript 响应正文超过 16 MiB 上限");
    sync_content_length(&mut response.headers, response.body.len());
    Ok(())
}

pub fn execute_http_match_replace_script(script: &str, record: &mut FlowRecord, response_stage: bool) -> Result<bool> {
    let mut scope = Scope::new();
    let returned = MIO_SCRIPT_ENGINE.with(|engine| -> Result<Dynamic> {
        let ast = compile_cached(engine, script)?;
        scope.push("request", request_map(&record.request)?);
        scope.push("response", response_map(record.response.as_ref()));
        let _cancel_guard = ActiveCancelGuard::set(None);
        engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, ast.as_ref())
            .map_err(|error| anyhow::anyhow!("MioScript 执行错误：{error}"))
    })?;
    let name = if response_stage { "response" } else { "request" };
    if let Some(map) = returned.clone().try_cast::<Map>() {
        scope.set_value(name, map);
    } else if let Some(raw) = returned.clone().try_cast::<ImmutableString>() {
        let mut map = scope.get_value::<Map>(name).context("MioScript 消息对象缺失")?;
        map.insert("raw".into(), raw.into());
        scope.set_value(name, map);
    } else {
        ensure!(returned.is_unit(), "MioScript 替换脚本必须返回消息 Map、原始报文字符串或空值");
    }
    let mut candidate = record.clone();
    if response_stage {
        let response = candidate.response.as_mut().context("MioScript 响应阶段缺少响应")?;
        apply_response_map(response, &scope.get_value::<Map>("response").context("MioScript response 必须为 Map")?)?;
        candidate.summary.response_bytes = response.body.len() as u64;
        candidate.summary.status = Some(response.status);
    } else {
        apply_request_map(&mut candidate.request, &scope.get_value::<Map>("request").context("MioScript request 必须为 Map")?)?;
        candidate.summary.request_bytes = candidate.request.body.len() as u64;
        candidate.summary.method = candidate.request.method.clone();
        candidate.summary.url = candidate.request.url.clone();
    }
    let changed = candidate != *record;
    *record = candidate;
    Ok(changed)
}

#[allow(clippy::too_many_arguments)]
pub fn eval_http_filter_script(script: &str, method: &str, url: &str, status: Option<u16>, length: u64,
    request_raw: Option<&[u8]>, response_raw: Option<&[u8]>, notes: &str, highlighted: bool,
    listener_port: Option<u16>) -> Result<bool> {
    eval_http_filter_script_with_scope(
        script,
        method,
        url,
        status,
        length,
        request_raw,
        response_raw,
        notes,
        highlighted,
        listener_port,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_http_filter_script_with_scope(script: &str, method: &str, url: &str, status: Option<u16>, length: u64,
    request_raw: Option<&[u8]>, response_raw: Option<&[u8]>, notes: &str, highlighted: bool,
    listener_port: Option<u16>, in_scope: bool) -> Result<bool> {
    let cancel = tokio_util::sync::CancellationToken::new();
    eval_http_filter_script_with_scope_and_cancel(
        script,
        method,
        url,
        status,
        length,
        request_raw,
        response_raw,
        notes,
        highlighted,
        listener_port,
        in_scope,
        &cancel,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_http_filter_script_with_scope_and_cancel(
    script: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    length: u64,
    request_raw: Option<&[u8]>,
    response_raw: Option<&[u8]>,
    notes: &str,
    highlighted: bool,
    listener_port: Option<u16>,
    in_scope: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    eval_http_filter_script_inner(
        script,
        method,
        url,
        status,
        length,
        request_raw,
        response_raw,
        notes,
        HighlightFilterState::Boolean(highlighted),
        listener_port,
        in_scope,
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_http_filter_script_with_highlight_color_and_cancel(
    script: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    length: u64,
    request_raw: Option<&[u8]>,
    response_raw: Option<&[u8]>,
    notes: &str,
    highlight_color: u8,
    listener_port: Option<u16>,
    in_scope: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    eval_http_filter_script_inner(
        script,
        method,
        url,
        status,
        length,
        request_raw,
        response_raw,
        notes,
        HighlightFilterState::ColorCode(highlight_color),
        listener_port,
        in_scope,
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
fn eval_http_filter_script_inner(
    script: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    length: u64,
    request_raw: Option<&[u8]>,
    response_raw: Option<&[u8]>,
    notes: &str,
    highlight_state: HighlightFilterState,
    listener_port: Option<u16>,
    in_scope: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    let (highlighted, highlight_name, highlight_color_unknown) = match highlight_state {
        HighlightFilterState::ColorCode(color) => (
            (1..=9).contains(&color),
            highlight_color_name(color)?,
            false,
        ),
        HighlightFilterState::Boolean(false) => (false, "NONE", false),
        HighlightFilterState::Boolean(true) => (true, "", true),
    };
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    let ast = MIO_SCRIPT_ENGINE.with(|engine| compile_cached(engine, script))?;
    ensure!(!cancel.is_cancelled(), "HTTP 历史查询已取消");
    let parsed_request = if let Some(raw) = request_raw {
        let (request, _) = parse_imported_request(raw, Some(url)).context("MioScript 历史请求无法解析")?;
        request
    } else {
        HttpRequest {
            method: method.into(),
            url: url.into(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        }
    };
    let request = if url.is_empty() && request_raw.is_none() {
        [
            ("method", method.into()),
            ("url", "".into()),
            ("path", "".into()),
            ("path_with_query", "".into()),
            ("query", "".into()),
            ("host", "".into()),
            ("version", "".into()),
            ("headers", Map::new().into()),
            ("body", "".into()),
            ("raw", "".into()),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect()
    } else {
        let mut map = request_map(&parsed_request)?;
        map.insert("method".into(), method.into());
        map.insert("url".into(), url.into());
        if !url.is_empty() {
            let parsed_url = Url::parse(url).context("MioScript 请求 URL 无效")?;
            let path_with_query = match parsed_url.query() {
                Some(q) if !q.is_empty() => format!("{}?{q}", parsed_url.path()),
                _ => parsed_url.path().to_owned(),
            };
            map.insert("path".into(), parsed_url.path().into());
            map.insert("path_with_query".into(), path_with_query.into());
            map.insert("query".into(), parsed_url.query().unwrap_or_default().into());
            map.insert(
                "host".into(),
                parsed_url
                    .host_str()
                    .context("MioScript 请求 URL 缺少主机")?
                    .into(),
            );
        }
        if let Some(raw) = request_raw {
            map.insert(
                "raw".into(),
                String::from_utf8_lossy(raw).into_owned().into(),
            );
        }
        map
    };
    let response = response_raw.map(|raw| parse_imported_response(raw).map(|(response, _)| response)
        .context("MioScript 历史响应无法解析")).transpose()?;
    let mut response_data = response_map(response.as_ref());
    response_data.insert("status".into(), i64::from(status.unwrap_or(0)).into());
    response_data.insert("length".into(), i64::try_from(length)?.into());
    if let Some(raw) = response_raw {
        response_data.insert(
            "raw".into(),
            String::from_utf8_lossy(raw).into_owned().into(),
        );
    }
    if !url.is_empty() {
        let _ = Url::parse(url)?;
    }
    let parameterized = crate::store::ext_archive::http_query::filters::url_has_parameters(url, cancel)?
        || crate::store::ext_archive::http_query::filters::history_body_has_parameters(
            &parsed_request.headers,
            &parsed_request.body,
            cancel,
        )?;
    let mime_index = response.as_ref().map_or(7_i64, |response| {
        crate::store::ext_archive::http_query::filters::infer_history_mime(&response.headers, &response.body).index() as i64
    });
    let mime_label = mio_script_mime_type(response.as_ref());
    let mut request = request;
    request.insert("in_scope".into(), in_scope.into());
    request.insert("parameterized".into(), parameterized.into());
    response_data.insert("mime_type".into(), mime_label.into());
    let search_text = format!(
        "{}\n{}",
        string_field(&request, "raw")?,
        string_field(&response_data, "raw")?
    );
    let annotations: Map = [
        ("notes", notes.to_owned().into()),
        ("highlighted", highlighted.into()),
        ("highlight_color", highlight_name.into()),
        ("highlight_color_unknown", highlight_color_unknown.into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();
    let request_response: Map = [
        ("request", Dynamic::from(request.clone())),
        ("response", Dynamic::from(response_data.clone())),
        ("annotations", Dynamic::from(annotations.clone())),
        ("method", string_field(&request, "method")?.into()),
        ("url", string_field(&request, "url")?.into()),
        ("path", string_field(&request, "path")?.into()),
        ("path_with_query", string_field(&request, "path_with_query")?.into()),
        ("query", string_field(&request, "query")?.into()),
        ("host", string_field(&request, "host")?.into()),
        ("status", i64::from(status.unwrap_or(0)).into()),
        ("mime_type", mime_label.into()),
        ("notes", notes.to_owned().into()),
        ("highlighted", highlighted.into()),
        ("highlight_color", highlight_name.into()),
        ("highlight_color_unknown", highlight_color_unknown.into()),
        ("listener_port", i64::from(listener_port.unwrap_or(0)).into()),
        ("in_scope", in_scope.into()),
        ("parameterized", parameterized.into()),
        ("has_response", status.is_some().into()),
        ("search_text", search_text.clone().into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();
    let mut scope = Scope::new();
    scope.push("search_text", search_text);
    scope.push("request", request);
    scope.push("response", response_data);
    scope.push("annotations", annotations);
    scope.push("requestResponse", request_response);
    scope.push("utilities", Map::new());
    scope.push("notes", notes.to_owned());
    scope.push("highlighted", highlighted);
    scope.push("listener_port", i64::from(listener_port.unwrap_or(0)));
    scope.push("in_scope", in_scope);
    scope.push("parameterized", parameterized);
    scope.push("has_response", status.is_some());
    scope.push("mime_index", mime_index);
    MIO_SCRIPT_ENGINE.with(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(Some(cancel.clone()));
        eval_filter(engine, ast.as_ref(), &mut scope)
    })
}

pub fn eval_ws_filter_script(script: &str, url: &str, direction: WebSocketDirection, payload: &[u8],
    edited: bool, notes: &str, listener_port: Option<u16>) -> Result<bool> {
    eval_ws_filter_script_with_scope(script, url, direction, payload, edited, notes, listener_port, false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_ws_filter_script_with_scope(script: &str, url: &str, direction: WebSocketDirection, payload: &[u8],
    edited: bool, notes: &str, listener_port: Option<u16>, in_scope: bool) -> Result<bool> {
    ensure!(payload.len() <= MAX_BODY, "MioScript WebSocket 正文超过 16 MiB 上限");
    let ast = MIO_SCRIPT_ENGINE.with(|engine| compile_cached(engine, script))?;
    let direction = match direction { WebSocketDirection::ClientToServer => "c2s", WebSocketDirection::ServerToClient => "s2c" };
    let message: Map = [
        ("url", url.into()), ("direction", direction.into()),
        ("payload", String::from_utf8_lossy(payload).into_owned().into()),
        ("length", (payload.len() as i64).into()), ("edited", edited.into()),
        ("notes", notes.into()), ("listener_port", i64::from(listener_port.unwrap_or(0)).into()),
        ("in_scope", in_scope.into()),
    ].into_iter().map(|(key, value)| (key.into(), value)).collect();
    let mut scope = Scope::new();
    scope.push("message", message);
    scope.push("in_scope", in_scope);
    MIO_SCRIPT_ENGINE.with(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(None);
        eval_filter(engine, ast.as_ref(), &mut scope)
    })
}

fn eval_filter(engine: &Engine, ast: &AST, scope: &mut Scope<'_>) -> Result<bool> {
    engine.eval_ast_with_scope::<bool>(scope, ast)
        .map_err(|error| anyhow::anyhow!("MioScript 过滤脚本必须返回 bool：{error}"))
}

fn expression(conditions: Vec<String>) -> String {
    if conditions.is_empty() { "true".into() } else { conditions.join("\n    && ") }
}

fn search_expression(text: &str, term: &str, regex: bool, case_sensitive: bool, negative: bool) -> String {
    let pattern = if regex { term.to_owned() } else { regex::escape(term) };
    let pattern = if case_sensitive { pattern } else { format!("(?i){pattern}") };
    let quoted = serde_json::to_string(&pattern).expect("字符串序列化成功");
    format!("{}regex_matches({text}, {quoted})", if negative { "!" } else { "" })
}

pub fn convert_http_filter_to_script(filter: &HttpHistoryFilter) -> String {
    let mut conditions = Vec::new();
    if filter.in_scope_only { conditions.push("in_scope".into()); }
    if filter.hide_without_response { conditions.push("has_response".into()); }
    if filter.parameterized_only { conditions.push("parameterized".into()); }
    if filter.status_classes != [true; 4] {
        let classes: Vec<String> = filter.status_classes.iter().enumerate().filter(|(_, enabled)| **enabled)
            .map(|(index, _)| format!("response.status / 100 == {}", index + 2)).collect();
        conditions.push(format!("(!has_response || ({}))", if classes.is_empty() { "false".into() } else { classes.join(" || ") }));
    }
    if filter.mime_types != [true; 8] {
        let indexes: Vec<String> = filter.mime_types.iter().enumerate().filter(|(_, enabled)| **enabled)
            .map(|(index, _)| format!("mime_index == {index}")).collect();
        conditions.push(format!("({})", if indexes.is_empty() { "false".into() } else { indexes.join(" || ") }));
    }
    if let HttpExtensionFilter::Only(extensions) | HttpExtensionFilter::Hide(extensions) = &filter.extensions {
        let patterns: Vec<_> = extensions.split(|character: char| character == ',' || character.is_whitespace())
            .filter(|value| !value.is_empty()).map(|value| regex::escape(value.trim_start_matches('.'))).collect();
        let pattern = format!("(?i)\\.(?:{})$", patterns.join("|"));
        if !patterns.is_empty() {
            conditions.push(format!("{}regex_matches(request.path, {})", if matches!(filter.extensions, HttpExtensionFilter::Hide(_)) { "!" } else { "" }, serde_json::to_string(&pattern).expect("字符串序列化成功")));
        }
    }
    if filter.only_with_notes { conditions.push("!notes.is_empty()".into()); }
    if filter.only_highlighted { conditions.push("highlighted".into()); }
    if let Some(port) = filter.listener_port { conditions.push(format!("listener_port == {port}")); }
    if !filter.search_term.is_empty() { conditions.push(search_expression("search_text", &filter.search_term, filter.regex, filter.case_sensitive, filter.negative_search)); }
    expression(conditions)
}

pub fn convert_ws_filter_to_script(filter: &WebSocketHistoryFilter, in_scope_only: bool) -> String {
    let mut conditions = Vec::new();
    if in_scope_only { conditions.push("in_scope".into()); }
    if filter.hide_incoming { conditions.push("message.direction != \"s2c\"".into()); }
    if filter.hide_outgoing { conditions.push("message.direction != \"c2s\"".into()); }
    if filter.only_with_notes { conditions.push("!message.notes.is_empty()".into()); }
    if let Some(port) = filter.listener_port { conditions.push(format!("message.listener_port == {port}")); }
    if !filter.term.is_empty() { conditions.push(search_expression("message.payload", &filter.term, filter.regex, filter.case_sensitive, filter.negative)); }
    expression(conditions)
}

pub fn script_keyword_ranges(script: &str) -> Vec<Range<usize>> {
    if script.is_empty() || script.len() > 64 * 1024 {
        return Vec::new();
    }
    let mut line_starts: Vec<Vec<usize>> = vec![Vec::new(), vec![0]];
    let mut cur_line = 1usize;
    for (byte_ix, ch) in script.char_indices() {
        let next_byte = byte_ix + ch.len_utf8();
        line_starts[cur_line].push(next_byte);
        if ch == '\n' {
            cur_line += 1;
            if cur_line >= 65535 {
                break;
            }
            line_starts.push(vec![next_byte]);
        }
    }
    let pos_to_byte = |pos: Position| -> Option<usize> {
        let line = pos.line()?;
        let col = pos.position()?;
        line_starts
            .get(line)
            .and_then(|cols| cols.get(col.saturating_sub(1)).copied())
    };
    let engine = Engine::new_raw();
    let inputs = [script];
    let (tokens, control) = engine.lex(&inputs);
    let mut interp_brace_stack: Vec<usize> = Vec::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (token, pos) in tokens {
        match &token {
            Token::EOF => break,
            Token::InterpolatedString(_) => {
                interp_brace_stack.push(0);
            }
            Token::LeftBrace | Token::MapStart => {
                if let Some(depth) = interp_brace_stack.last_mut() {
                    *depth += 1;
                }
            }
            Token::RightBrace => {
                if let Some(depth) = interp_brace_stack.last_mut() {
                    *depth = depth.saturating_sub(1);
                    if *depth == 0 {
                        interp_brace_stack.pop();
                        control.borrow_mut().is_within_text = true;
                    }
                }
            }
            _ => {}
        }
        let kw = match token {
            Token::Return
            | Token::True
            | Token::False
            | Token::Let
            | Token::Const
            | Token::If
            | Token::Else
            | Token::Switch
            | Token::Do
            | Token::While
            | Token::Until
            | Token::Loop
            | Token::For
            | Token::In
            | Token::Continue
            | Token::Break
            | Token::Throw
            | Token::Try
            | Token::Catch => token.literal_syntax(),
            _ => continue,
        };
        if let Some(start) = pos_to_byte(pos) {
            let end = start + kw.len();
            if script.get(start..end) == Some(kw)
                && ranges.last().is_none_or(|prev| prev.end <= start)
            {
                ranges.push(start..end);
            }
        }
    }
    ranges
}

pub fn eval_custom_scan_check_script(
    script: &str,
    request: &HttpRequest,
    response: &HttpResponse,
) -> Result<Option<String>> {
    let trimmed = script.trim();
    ensure!(!trimmed.is_empty(), "自定义扫描检查脚本不能为空");
    ensure!(trimmed.len() <= 64 * 1024, "自定义扫描检查脚本超过 64 KiB 上限");
    let ast = MIO_SCRIPT_ENGINE.with(|engine| compile_cached(engine, trimmed))?;
    let req_map = request_map(request)?;
    let resp_map = response_map(Some(response));
    let mut rr = Map::new();
    rr.insert("request".into(), Dynamic::from(req_map.clone()));
    rr.insert("response".into(), Dynamic::from(resp_map.clone()));
    let mut scope = Scope::new();
    scope.push("request", req_map);
    scope.push("response", resp_map);
    scope.push("requestResponse", rr);
    scope.push("utilities", Map::new());
    let val = MIO_SCRIPT_ENGINE.with(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(None);
        engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, ast.as_ref())
            .map_err(|err| anyhow::anyhow!("自定义扫描检查脚本执行失败：{err}"))
    })?;
    if val.is_unit() {
        return Ok(None);
    }
    if let Some(b) = val.clone().try_cast::<bool>() {
        return Ok(b.then(|| "自定义扫描检查脚本命中".to_owned()));
    }
    if let Some(s) = val.clone().try_cast::<ImmutableString>() {
        let s = s.trim();
        return Ok((!s.is_empty()).then(|| s.to_owned()));
    }
    if let Some(map) = val.clone().try_cast::<Map>() {
        let matched = map
            .get("matched")
            .or_else(|| map.get("hit"))
            .and_then(|v| v.clone().try_cast::<bool>())
            .unwrap_or(true);
        if !matched {
            return Ok(None);
        }
        if let Some(ev) = map
            .get("evidence")
            .or_else(|| map.get("detail"))
            .and_then(|v| v.clone().try_cast::<ImmutableString>())
        {
            let ev = ev.trim();
            if !ev.is_empty() {
                return Ok(Some(ev.to_owned()));
            }
        }
        return Ok(Some("自定义扫描检查脚本命中".to_owned()));
    }
    let rendered = val.to_string();
    Ok((!rendered.trim().is_empty() && rendered != "false" && rendered != "0").then_some(rendered))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomActionOutcome {
    pub output: String,
    pub updated_request: Option<HttpRequest>,
}

fn dynamic_eq(a: &Dynamic, b: &Dynamic) -> bool {
    if let (Some(sa), Some(sb)) = (
        a.clone().try_cast::<ImmutableString>(),
        b.clone().try_cast::<ImmutableString>(),
    ) {
        return sa == sb;
    }
    if let (Some(ia), Some(ib)) = (a.clone().try_cast::<i64>(), b.clone().try_cast::<i64>()) {
        return ia == ib;
    }
    if let (Some(ba), Some(bb)) = (a.clone().try_cast::<bool>(), b.clone().try_cast::<bool>()) {
        return ba == bb;
    }
    if let (Some(ma), Some(mb)) = (a.clone().try_cast::<Map>(), b.clone().try_cast::<Map>()) {
        return dynamic_map_eq(&ma, &mb);
    }
    if let (Some(aa), Some(ab)) = (
        a.clone().try_cast::<rhai::Array>(),
        b.clone().try_cast::<rhai::Array>(),
    ) {
        return aa.len() == ab.len() && aa.iter().zip(ab.iter()).all(|(x, y)| dynamic_eq(x, y));
    }
    a.is_unit() && b.is_unit()
}

fn dynamic_map_eq(a: &Map, b: &Map) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(k, va)| b.get(k).is_some_and(|vb| dynamic_eq(va, vb)))
}

pub fn eval_custom_action_with_mutation(
    script: &str,
    request: &HttpRequest,
    response: Option<&HttpResponse>,
) -> Result<CustomActionOutcome> {
    let trimmed = script.trim();
    ensure!(!trimmed.is_empty(), "自定义动作脚本不能为空");
    ensure!(trimmed.len() <= 64 * 1024, "自定义动作脚本超过 64 KiB 上限");
    let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let logs_capture = logs.clone();
    let mut engine = engine();
    engine.on_print(move |text| {
        if let Ok(mut guard) = logs_capture.lock()
            && guard.len() < 256
        {
            guard.push(text.to_owned());
        }
    });
    let ast = compile(&engine, trimmed)?;
    let req_map = request_map(request)?;
    let initial_req_map = req_map.clone();
    let resp_map = response_map(response);
    let mut rr = Map::new();
    rr.insert("request".into(), Dynamic::from(req_map.clone()));
    rr.insert("response".into(), Dynamic::from(resp_map.clone()));
    let mut scope = Scope::new();
    scope.push("request", req_map);
    scope.push("response", resp_map);
    scope.push("requestResponse", rr);
    scope.push("utilities", Map::new());
    let val = engine
        .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
        .map_err(|err| anyhow::anyhow!("自定义动作执行失败：{err}"))?;

    let returned_request_map = val.clone().try_cast::<Map>().filter(|map| {
        map.contains_key("method")
            && map.contains_key("url")
            && map.contains_key("headers")
            && map.contains_key("raw")
    });
    let effective_req_map = match returned_request_map.as_ref() {
        Some(map) => Some(map.clone()),
        None => scope.get_value::<Map>("request"),
    };
    let mut updated_request = None;
    if let Some(final_req_map) = effective_req_map
        && !dynamic_map_eq(&final_req_map, &initial_req_map)
    {
        let mut candidate = request.clone();
        apply_request_map(&mut candidate, &final_req_map)?;
        if candidate != *request {
            updated_request = Some(candidate);
        }
    }

    let mut lines = logs
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    if !val.is_unit() && returned_request_map.is_none() {
        lines.push(val.to_string());
    }
    let output = if lines.is_empty() {
        if updated_request.is_some() {
            "已更新当前请求报文。".into()
        } else {
            "脚本执行完成（无输出）".into()
        }
    } else {
        lines.join("\n")
    };
    Ok(CustomActionOutcome {
        output,
        updated_request,
    })
}

pub fn eval_custom_action_script(
    script: &str,
    request: &HttpRequest,
    response: Option<&HttpResponse>,
) -> Result<String> {
    eval_custom_action_with_mutation(script, request, response).map(|outcome| outcome.output)
}

#[cfg(test)]
mod keyword_range_tests {
    use super::*;

    #[test]
    fn script_keyword_ranges_lexes_keywords_and_skips_strings_comments_and_unicode_crlf() {
        assert!(script_keyword_ranges("").is_empty());
        let simple = "return true;";
        let slices: Vec<&str> = script_keyword_ranges(simple)
            .into_iter()
            .map(|r| &simple[r])
            .collect();
        assert_eq!(slices, vec!["return", "true"]);

        let complex = concat!(
            "// comment with return true false\r\n",
            "/* block /* nested return */ false */\r\n",
            "let 备注 = \"return true\";\r\n",
            "let tpl = `prefix return ${if true { false } else { true }} suffix return`;\r\n",
            "if !false { return true; } else { return ("
        );
        let ranges = script_keyword_ranges(complex);
        let matched: Vec<&str> = ranges.iter().map(|r| &complex[r.clone()]).collect();
        assert_eq!(
            matched,
            vec![
                "let", "let", "if", "true", "false", "else", "true", "if", "false", "return",
                "true", "else", "return"
            ]
        );
        for w in ranges.windows(2) {
            assert!(w[0].end <= w[1].start);
        }
    }
}

#[cfg(test)]
mod custom_action_mutation_tests {
    use super::*;

    fn sample_request() -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url: "https://api.example.com/v1/profile?tab=info&debug=1".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"api.example.com".to_vec(),
                },
                Header {
                    name: "Cookie".into(),
                    value: b"session=secret123; theme=dark".to_vec(),
                },
                Header {
                    name: "Authorization".into(),
                    value: b"Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyMSJ9.sig".to_vec(),
                },
                Header {
                    name: "If-None-Match".into(),
                    value: b"\"etag-1\"".to_vec(),
                },
            ],
            body: Vec::new(),
            trailers: Vec::new(),
        }
    }

    #[test]
    fn custom_action_strips_all_cookies_and_returns_updated_request() {
        let req = sample_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            remove_all_cookies(request);
            print("removed");
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "removed");
        let updated = outcome
            .updated_request
            .expect("Cookie removal must produce updated_request");
        assert!(
            !updated
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("cookie"))
        );
        assert!(
            updated
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization"))
        );
    }

    #[test]
    fn custom_action_cookie_header_and_query_helpers_roundtrip_cleanly() {
        let req = sample_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            request.removeCookie("session");
            request.setCookie("role", "guest");
            request.removeHeaders(["Authorization", "If-None-Match"]);
            request.removeQueryParam("debug");
            request.setQueryParam("_cb", "42");
            print(base64url_decode(base64url_encode("hello+world/=")));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "hello+world/=");
        let updated = outcome.updated_request.unwrap();
        let cookie_val = updated
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("cookie"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned());
        assert_eq!(cookie_val.as_deref(), Some("theme=dark; role=guest"));
        assert!(
            !updated
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization")
                    || h.name.eq_ignore_ascii_case("if-none-match"))
        );
        assert_eq!(
            updated.url,
            "https://api.example.com/v1/profile?tab=info&_cb=42"
        );
    }

    #[test]
    fn readonly_custom_action_leaves_updated_request_none() {
        let req = sample_request();
        let outcome = eval_custom_action_with_mutation(
            r#"print(request.method + " " + request.url);"#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(
            outcome.output,
            "GET https://api.example.com/v1/profile?tab=info&debug=1"
        );
        assert_eq!(outcome.updated_request, None);
    }
}

#[cfg(test)]
mod expanded_security_helpers_tests {
    use super::*;

    fn json_post_request() -> HttpRequest {
        let body = br#"{"user":{"role":"guest","tags":["read"]},"active":true}"#.to_vec();
        HttpRequest {
            method: "POST".into(),
            url: "https://api.example.com/v1/users".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"api.example.com".to_vec(),
                },
                Header {
                    name: "Content-Type".into(),
                    value: b"application/json".to_vec(),
                },
                Header {
                    name: "Cookie".into(),
                    value: b"sid=abc123; prefs=compact".to_vec(),
                },
                Header {
                    name: "Authorization".into(),
                    value: b"Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJhZG1pbiJ9.sig_ok".to_vec(),
                },
                Header {
                    name: "Content-Length".into(),
                    value: body.len().to_string().into_bytes(),
                },
            ],
            body,
            trailers: Vec::new(),
        }
    }

    #[test]
    fn json_helpers_parse_query_stringify_and_mutate_request_body() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let obj = json_parse(request.body);
            let tag0 = json_get(request.body, "user.tags.0");
            let missing = utilities.jsonGet(request.body, "user.missing.0");
            let compact = utilities.jsonStringify(obj);
            let pretty = utilities.jsonStringifyPretty(#{ ok: true });
            request.setJsonField("user.role", "admin");
            request.setJsonField("meta.source", "mio");
            request.setJsonField("user.tags.0", "write");
            print(obj.user.role + "|" + tag0 + "|" + (missing == ()) + "|" + compact.contains("guest") + "|" + pretty.contains("\n"));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "guest|read|true|true|true");
        let updated = outcome.updated_request.expect("JSON mutation updates body");
        let parsed: serde_json::Value = serde_json::from_slice(&updated.body).unwrap();
        assert_eq!(parsed["user"]["role"], "admin");
        assert_eq!(parsed["user"]["tags"][0], "write");
        assert_eq!(parsed["meta"]["source"], "mio");
        let cl = updated
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-length"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned());
        assert_eq!(cl.as_deref(), Some(updated.body.len().to_string().as_str()));
    }

    #[test]
    fn hex_and_html_encode_decode_work_and_validate() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let hx = hex_encode("Hi!");
            let dec = hex_decode("48 69 21");
            let ent = html_encode("<a href=\"x\">'&'</a>");
            let plain = utilities.htmlDecode(ent + " &#65; &#x42; &apos; &unknown;");
            print(hx + "|" + dec + "|" + utilities.hexEncode("A") + "|" + utilities.hexDecode("41") + "|" + plain);
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(
            outcome.output,
            "486921|Hi!|41|A|<a href=\"x\">'&'</a> A B ' &unknown;"
        );
        assert_eq!(outcome.updated_request, None);

        assert!(eval_custom_action_with_mutation(r#"hex_decode("abc");"#, &req, None).is_err());
        assert!(eval_custom_action_with_mutation(r#"hex_decode("zz");"#, &req, None).is_err());
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_and_long_key_consistency() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let rfc = hmac_sha256_hex("Jefe", "what do ya want for nothing?");
            let b64 = utilities.hmacSha256Base64("Jefe", "what do ya want for nothing?");
            let b64u = utilities.hmacSha256Base64Url("Jefe", "what do ya want for nothing?");
            let long_key = "k0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
            let long_hex = utilities.hmacSha256Hex(long_key, "payload");
            let long_b64 = hmac_sha256_base64(long_key, "payload");
            print(rfc + "|" + b64 + "|" + b64u + "|" + long_hex.len() + "|" + (long_b64 == hmac_sha256_base64(long_key, "payload")));
            "#,
            &req,
            None,
        )
        .unwrap();
        let expected_b64 = STANDARD.encode([
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ]);
        let expected_b64u = URL_SAFE_NO_PAD.encode([
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ]);
        assert_eq!(
            outcome.output,
            format!(
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843|{expected_b64}|{expected_b64u}|64|true"
            )
        );
    }

    #[test]
    fn jwt_decode_handles_bearer_and_raw_tokens() {
        let req = json_post_request();
        let outcome = eval_custom_action_with_mutation(
            r#"
            let auth = request.headerValue("Authorization");
            let decoded = jwt_decode(auth);
            let raw_tok = auth.sub_string(7);
            let decoded2 = utilities.jwtDecode(raw_tok);
            print(decoded.header.alg + "|" + decoded.payload.sub + "|" + decoded.signature + "|" + decoded2.payload.role + "|" + decoded.raw_payload.contains("user1"));
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "HS256|user1|sig_ok|admin|true");
        assert!(
            eval_custom_action_with_mutation(r#"jwt_decode("bad.token");"#, &req, None).is_err()
        );
    }

    #[test]
    fn cookie_and_form_helpers_read_and_mutate_request() {
        let mut req = json_post_request();
        req.headers
            .retain(|h| !h.name.eq_ignore_ascii_case("content-type"));
        req.headers.push(Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        });
        req.body = b"user=alice&role=user&debug=1".to_vec();

        let outcome = eval_custom_action_with_mutation(
            r#"
            let cookies = request.parseCookies();
            let before_role = request.formParam("role");
            request.setFormParam("role", "admin");
            request.setFormParam("next", "/dash");
            request.removeFormParam("debug");
            print(cookies.sid + "|" + cookies.prefs + "|" + before_role);
            "#,
            &req,
            None,
        )
        .unwrap();
        assert_eq!(outcome.output, "abc123|compact|user");
        let updated = outcome
            .updated_request
            .expect("form mutation updates request");
        assert_eq!(
            String::from_utf8_lossy(&updated.body),
            "user=alice&role=admin&next=%2Fdash"
        );
        let cl = updated
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-length"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned());
        assert_eq!(cl.as_deref(), Some(updated.body.len().to_string().as_str()));
    }
}
