use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, ensure};
use mio_plugin_api::Header;
use regex::{Regex, RegexBuilder};
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map};
use url::Url;

use super::super::{
    MAX_BODY, MAX_HEAD, MatchAndReplaceRule, replace_text_with_rule, token,
    valid_header_value_bytes,
};
use super::engine::BoundedLruCache;
use crate::codec_util;

pub(super) type ScriptResult<T> = std::result::Result<T, Box<EvalAltResult>>;

const MAX_JSON_SCRIPT_DEPTH: usize = 32;
const MIO_SCRIPT_REGEX_CACHE_LIMIT: usize = 256;

/// Dedicated Rhai receiver type for `utilities.*` helpers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ScriptUtilities;

/// Explicit ordered HTTP header collection that preserves duplicate header names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ScriptHeaders {
    entries: Vec<Header>,
}

impl ScriptHeaders {
    pub(super) fn from_headers(headers: &[Header]) -> Self {
        Self {
            entries: headers.to_vec(),
        }
    }

    pub(super) fn into_headers(self) -> Vec<Header> {
        self.entries
    }

    pub(super) fn get_first(&self, name: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
    }

    pub(super) fn get_all(&self, name: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
            .collect()
    }

    pub(super) fn has(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(name))
    }

    pub(super) fn set(&mut self, name: &str, value: &str) -> ScriptResult<()> {
        validate_header_pair(name, value)?;
        let mut next = self.entries.clone();
        next.retain(|h| !h.name.eq_ignore_ascii_case(name));
        next.push(Header {
            name: name.to_owned(),
            value: value.as_bytes().to_vec(),
        });
        if next.len() > 1024
            || next.iter().map(|h| h.name.len() + h.value.len() + 4).sum::<usize>()
                > crate::http::MAX_HEAD
        {
            return Err("请求或响应标头数量或总大小超过上限".into());
        }
        self.entries = next;
        Ok(())
    }

    pub(super) fn add(&mut self, name: &str, value: &str) -> ScriptResult<()> {
        validate_header_pair(name, value)?;
        let total_bytes: usize = self
            .entries
            .iter()
            .map(|h| h.name.len() + h.value.len() + 4)
            .sum();
        if self.entries.len() >= 1024
            || total_bytes.saturating_add(name.len() + value.len() + 4) > crate::http::MAX_HEAD
        {
            return Err("请求或响应标头数量或总大小超过上限".into());
        }
        self.entries.push(Header {
            name: name.to_owned(),
            value: value.as_bytes().to_vec(),
        });
        Ok(())
    }

    pub(super) fn remove(&mut self, name: &str) -> Option<String> {
        let removed = self.get_first(name);
        if removed.is_some() {
            self.entries.retain(|h| !h.name.eq_ignore_ascii_case(name));
        }
        removed
    }

    pub(super) fn keys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for header in &self.entries {
            if !out
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&header.name))
            {
                out.push(header.name.clone());
            }
        }
        out
    }

    pub(super) fn values(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
            .collect()
    }

    pub(super) fn cookie_header_joined(&self) -> String {
        self.get_all("Cookie").join("; ")
    }

    pub(super) fn validate_all(&self) -> Result<()> {
        for header in &self.entries {
            ensure!(
                token(header.name.as_bytes()) && valid_header_value_bytes(&header.value),
                "MioScript 请求头名称或值无效"
            );
        }
        let total_bytes: usize = self
            .entries
            .iter()
            .map(|header| header.name.len() + header.value.len() + 4)
            .sum();
        ensure!(
            self.entries.len() <= 1024 && total_bytes <= MAX_HEAD,
            "MioScript HTTP 请求头超过限额"
        );
        Ok(())
    }

    pub(super) fn to_json_value(&self) -> serde_json::Value {
        let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
        for header in &self.entries {
            let val = String::from_utf8_lossy(&header.value).into_owned();
            if let Some((_, list)) = grouped
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(&header.name))
            {
                list.push(val);
            } else {
                grouped.push((header.name.clone(), vec![val]));
            }
        }
        let mut obj = serde_json::Map::new();
        for (name, mut vals) in grouped {
            let json_val = if vals.len() == 1 {
                serde_json::Value::String(vals.remove(0))
            } else {
                serde_json::Value::Array(vals.into_iter().map(serde_json::Value::String).collect())
            };
            obj.insert(name, json_val);
        }
        serde_json::Value::Object(obj)
    }
}

pub(super) fn runtime_error(error: impl std::fmt::Display) -> Box<EvalAltResult> {
    error.to_string().into()
}

pub(super) fn string_field(map: &Map, name: &str) -> Result<String> {
    map.get(name)
        .and_then(|value| value.clone().try_cast::<ImmutableString>())
        .map(|value| value.to_string())
        .with_context(|| format!("MioScript 字段 {name} 必须为字符串"))
}

fn validate_header_pair(name: &str, value: &str) -> ScriptResult<()> {
    if !token(name.as_bytes()) || !valid_header_value_bytes(value.as_bytes()) {
        return Err(runtime_error("HTTP 请求头名称或值无效"));
    }
    Ok(())
}

enum HeaderContainerKind {
    ScriptHeaders,
    NestedMap,
    FlatMap,
    Invalid,
}

fn classify_header_container(map: &Map) -> HeaderContainerKind {
    match map.get("headers") {
        Some(val) if val.read_lock::<ScriptHeaders>().is_some() => {
            HeaderContainerKind::ScriptHeaders
        }
        Some(val) if val.read_lock::<Map>().is_some() => HeaderContainerKind::NestedMap,
        Some(val) if val.is_string() => HeaderContainerKind::FlatMap,
        Some(_) => HeaderContainerKind::Invalid,
        None => HeaderContainerKind::FlatMap,
    }
}

enum HeadersRef<'a> {
    Script(&'a ScriptHeaders),
    Map(&'a Map),
}

enum HeadersMut<'a> {
    Script(&'a mut ScriptHeaders),
    Map(&'a mut Map),
}

fn with_headers_ref<R>(
    map: &Map,
    f: impl FnOnce(HeadersRef<'_>) -> ScriptResult<R>,
) -> ScriptResult<R> {
    match classify_header_container(map) {
        HeaderContainerKind::ScriptHeaders => {
            let lock = map
                .get("headers")
                .expect("classified as ScriptHeaders")
                .read_lock::<ScriptHeaders>()
                .ok_or_else(|| runtime_error("headers 必须为 Headers 或 Map"))?;
            f(HeadersRef::Script(&lock))
        }
        HeaderContainerKind::NestedMap => {
            let lock = map
                .get("headers")
                .expect("classified as NestedMap")
                .read_lock::<Map>()
                .ok_or_else(|| runtime_error("headers 必须为 Headers 或 Map"))?;
            f(HeadersRef::Map(&lock))
        }
        HeaderContainerKind::FlatMap => f(HeadersRef::Map(map)),
        HeaderContainerKind::Invalid => Err(runtime_error("headers 必须为 Headers 或 Map")),
    }
}

fn with_headers_mut<R>(
    map: &mut Map,
    f: impl FnOnce(HeadersMut<'_>) -> ScriptResult<R>,
) -> ScriptResult<R> {
    match classify_header_container(map) {
        HeaderContainerKind::ScriptHeaders => {
            let mut lock = map
                .get_mut("headers")
                .expect("classified as ScriptHeaders")
                .write_lock::<ScriptHeaders>()
                .ok_or_else(|| runtime_error("headers 必须为 Headers 或 Map"))?;
            f(HeadersMut::Script(&mut lock))
        }
        HeaderContainerKind::NestedMap => {
            let mut lock = map
                .get_mut("headers")
                .expect("classified as NestedMap")
                .write_lock::<Map>()
                .ok_or_else(|| runtime_error("headers 必须为 Headers 或 Map"))?;
            f(HeadersMut::Map(&mut lock))
        }
        HeaderContainerKind::FlatMap => f(HeadersMut::Map(map)),
        HeaderContainerKind::Invalid => Err(runtime_error("headers 必须为 Headers 或 Map")),
    }
}

fn map_header_value(headers: &Map, name: &str) -> ScriptResult<String> {
    match headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)) {
        Some((_, value)) => value
            .clone()
            .try_cast::<ImmutableString>()
            .map(|value| value.to_string())
            .ok_or_else(|| runtime_error("请求头值必须为字符串")),
        None => Ok(String::new()),
    }
}

fn map_header_values(headers: &Map, name: &str) -> ScriptResult<Vec<String>> {
    let mut out = Vec::new();
    for (key, value) in headers {
        if key.eq_ignore_ascii_case(name) {
            let text = value
                .clone()
                .try_cast::<ImmutableString>()
                .ok_or_else(|| runtime_error("请求头值必须为字符串"))?;
            out.push(text.to_string());
        }
    }
    Ok(out)
}

fn header_value(map: &mut Map, name: &str) -> ScriptResult<String> {
    with_headers_ref(map, |headers| match headers {
        HeadersRef::Script(h) => Ok(h.get_first(name).unwrap_or_default()),
        HeadersRef::Map(m) => map_header_value(m, name),
    })
}

fn header_values(map: &mut Map, name: &str) -> ScriptResult<Array> {
    let vals = with_headers_ref(map, |headers| match headers {
        HeadersRef::Script(h) => Ok(h.get_all(name)),
        HeadersRef::Map(m) => map_header_values(m, name),
    })?;
    Ok(vals.into_iter().map(Dynamic::from).collect())
}

fn has_header(map: &mut Map, name: &str) -> ScriptResult<bool> {
    with_headers_ref(map, |headers| match headers {
        HeadersRef::Script(h) => Ok(h.has(name)),
        HeadersRef::Map(m) => Ok(m.keys().any(|key| key.eq_ignore_ascii_case(name))),
    })
}

fn remove_header(map: &mut Map, name: &str) -> ScriptResult<()> {
    with_headers_mut(map, |headers| match headers {
        HeadersMut::Script(h) => {
            h.remove(name);
            Ok(())
        }
        HeadersMut::Map(m) => {
            m.retain(|key, _| !key.eq_ignore_ascii_case(name));
            Ok(())
        }
    })
}

fn set_header(map: &mut Map, name: &str, value: &str) -> ScriptResult<()> {
    validate_header_pair(name, value)?;
    with_headers_mut(map, |headers| match headers {
        HeadersMut::Script(h) => h.set(name, value),
        HeadersMut::Map(m) => {
            m.retain(|key, _| !key.eq_ignore_ascii_case(name));
            m.insert(name.into(), value.into());
            Ok(())
        }
    })
}

fn add_header(map: &mut Map, name: &str, value: &str) -> ScriptResult<()> {
    validate_header_pair(name, value)?;
    with_headers_mut(map, |headers| match headers {
        HeadersMut::Script(h) => h.add(name, value),
        HeadersMut::Map(m) => {
            m.retain(|key, _| !key.eq_ignore_ascii_case(name));
            m.insert(name.into(), value.into());
            Ok(())
        }
    })
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

fn cookie_header_joined(map: &Map) -> ScriptResult<String> {
    with_headers_ref(map, |headers| match headers {
        HeadersRef::Script(h) => Ok(h.cookie_header_joined()),
        HeadersRef::Map(m) => map_header_value(m, "Cookie"),
    })
}

fn has_urlencoded_content_type(map: &Map) -> ScriptResult<bool> {
    let ct = with_headers_ref(map, |headers| match headers {
        HeadersRef::Script(h) => Ok(h.get_first("content-type").unwrap_or_default()),
        HeadersRef::Map(m) => map_header_value(m, "content-type"),
    })?;
    Ok(ct
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("application/x-www-form-urlencoded"))
}

fn query_param(map: &mut Map, name: &str) -> ScriptResult<String> {
    let url = Url::parse(&string_field(map, "url").map_err(runtime_error)?).map_err(runtime_error)?;
    Ok(url
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default())
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
        if let Some((_, value)) =
            form_urlencoded::parse(body.as_bytes()).find(|(key, _)| key == name)
        {
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
    let existing = cookie_header_joined(map)?;
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
    let existing = cookie_header_joined(map)?;
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

fn url_encode_str(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn url_decode_str(text: &str) -> ScriptResult<String> {
    let encoded = format!("value={text}");
    let (_, value) = form_urlencoded::parse(encoded.as_bytes())
        .next()
        .ok_or_else(|| runtime_error("URL 解码结果缺失"))?;
    Ok(value.into_owned())
}

fn base64_encode_str(text: &str) -> String {
    codec_util::base64_encode(text.as_bytes())
}

fn base64_decode_str(text: &str) -> ScriptResult<String> {
    let decoded = codec_util::base64_decode(text.as_bytes()).map_err(runtime_error)?;
    String::from_utf8(decoded).map_err(runtime_error)
}

fn base64url_encode(text: &str) -> String {
    codec_util::base64url_encode(text.as_bytes())
}

fn base64url_decode(text: &str) -> ScriptResult<String> {
    let decoded = codec_util::base64url_decode(text.as_bytes()).map_err(runtime_error)?;
    String::from_utf8(decoded).map_err(runtime_error)
}

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
    if let Some(headers) = value.clone().try_cast::<ScriptHeaders>() {
        return Ok(headers.to_json_value());
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
    codec_util::hex_encode(text.as_bytes())
}

fn hex_decode(hex: &str) -> ScriptResult<String> {
    let bytes = codec_util::hex_decode(hex).map_err(runtime_error)?;
    String::from_utf8(bytes).map_err(runtime_error)
}

fn html_encode(text: &str) -> String {
    codec_util::html_encode(text)
}

fn html_decode(text: &str) -> String {
    codec_util::html_decode(text)
}

fn sha256_hex_str(text: &str) -> String {
    codec_util::hex_encode(&codec_util::sha256_digest(text.as_bytes()))
}

fn md5_hex_str(text: &str) -> String {
    codec_util::hex_encode(&codec_util::md5_digest(text.as_bytes()))
}

fn hmac_sha256_hex(key: &str, message: &str) -> String {
    codec_util::hex_encode(&codec_util::hmac_sha256(key.as_bytes(), message.as_bytes()))
}

fn hmac_sha256_base64(key: &str, message: &str) -> String {
    codec_util::base64_encode(&codec_util::hmac_sha256(key.as_bytes(), message.as_bytes()))
}

fn hmac_sha256_base64url(key: &str, message: &str) -> String {
    codec_util::base64url_encode(&codec_util::hmac_sha256(key.as_bytes(), message.as_bytes()))
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
    let raw = cookie_header_joined(map)?;
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

fn script_regex(pattern: &str) -> ScriptResult<Regex> {
    if pattern.len() > 4096 {
        return Err(runtime_error("MioScript 正则表达式超过 4096 字节"));
    }
    static CACHE: OnceLock<Mutex<BoundedLruCache<String, std::result::Result<Regex, String>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BoundedLruCache::new(MIO_SCRIPT_REGEX_CACHE_LIMIT)));
    {
        let mut guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = guard.get(pattern) {
            return cached.map_err(runtime_error);
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

fn register_script_utilities(engine: &mut Engine) {
    engine
        .register_type_with_name::<ScriptUtilities>("Utilities")
        .register_fn("to_string", |_: &mut ScriptUtilities| -> String {
            "Utilities".to_owned()
        })
        .register_fn("to_debug", |_: &mut ScriptUtilities| -> String {
            "Utilities".to_owned()
        })
        .register_fn("==", |_: &mut ScriptUtilities, _: ScriptUtilities| true)
        .register_fn("!=", |_: &mut ScriptUtilities, _: ScriptUtilities| false)
        .register_fn("urlEncode", |_: &mut ScriptUtilities, text: &str| -> String {
            url_encode_str(text)
        })
        .register_fn(
            "urlDecode",
            |_: &mut ScriptUtilities, text: &str| -> ScriptResult<String> { url_decode_str(text) },
        )
        .register_fn(
            "base64Encode",
            |_: &mut ScriptUtilities, text: &str| -> String { base64_encode_str(text) },
        )
        .register_fn(
            "base64Decode",
            |_: &mut ScriptUtilities, text: &str| -> ScriptResult<String> {
                base64_decode_str(text)
            },
        )
        .register_fn(
            "base64UrlEncode",
            |_: &mut ScriptUtilities, text: &str| -> String { base64url_encode(text) },
        )
        .register_fn(
            "base64UrlDecode",
            |_: &mut ScriptUtilities, text: &str| -> ScriptResult<String> {
                base64url_decode(text)
            },
        )
        .register_fn("sha256Hex", |_: &mut ScriptUtilities, text: &str| -> String {
            sha256_hex_str(text)
        })
        .register_fn("md5Hex", |_: &mut ScriptUtilities, text: &str| -> String {
            md5_hex_str(text)
        })
        .register_fn(
            "jsonParse",
            |_: &mut ScriptUtilities, text: &str| -> ScriptResult<Dynamic> { json_parse(text) },
        )
        .register_fn(
            "jsonStringify",
            |_: &mut ScriptUtilities, value: Dynamic| -> ScriptResult<String> {
                json_stringify(value)
            },
        )
        .register_fn(
            "jsonStringifyPretty",
            |_: &mut ScriptUtilities, value: Dynamic| -> ScriptResult<String> {
                json_stringify_pretty(value)
            },
        )
        .register_fn(
            "jsonGet",
            |_: &mut ScriptUtilities, text: &str, path: &str| -> ScriptResult<Dynamic> {
                json_get(text, path)
            },
        )
        .register_fn("hexEncode", |_: &mut ScriptUtilities, text: &str| -> String {
            hex_encode(text)
        })
        .register_fn(
            "hexDecode",
            |_: &mut ScriptUtilities, hex: &str| -> ScriptResult<String> { hex_decode(hex) },
        )
        .register_fn(
            "htmlEncode",
            |_: &mut ScriptUtilities, text: &str| -> String { html_encode(text) },
        )
        .register_fn(
            "htmlDecode",
            |_: &mut ScriptUtilities, text: &str| -> String { html_decode(text) },
        )
        .register_fn(
            "hmacSha256Hex",
            |_: &mut ScriptUtilities, key: &str, message: &str| -> String {
                hmac_sha256_hex(key, message)
            },
        )
        .register_fn(
            "hmacSha256Base64",
            |_: &mut ScriptUtilities, key: &str, message: &str| -> String {
                hmac_sha256_base64(key, message)
            },
        )
        .register_fn(
            "hmacSha256Base64Url",
            |_: &mut ScriptUtilities, key: &str, message: &str| -> String {
                hmac_sha256_base64url(key, message)
            },
        )
        .register_fn(
            "jwtDecode",
            |_: &mut ScriptUtilities, token: &str| -> ScriptResult<Map> { jwt_decode(token) },
        );
}

fn register_script_headers(engine: &mut Engine) {
    engine
        .register_type_with_name::<ScriptHeaders>("Headers")
        .register_indexer_get(|headers: &mut ScriptHeaders, name: ImmutableString| -> Dynamic {
            headers
                .get_first(name.as_str())
                .map_or(Dynamic::UNIT, Dynamic::from)
        })
        .register_indexer_set(
            |headers: &mut ScriptHeaders,
             name: ImmutableString,
             value: Dynamic|
             -> ScriptResult<()> {
                let text = value
                    .try_cast::<ImmutableString>()
                    .ok_or_else(|| runtime_error("请求头值必须为字符串"))?;
                headers.set(name.as_str(), text.as_str())
            },
        )
        .register_fn("keys", |headers: &mut ScriptHeaders| -> Array {
            headers.keys().into_iter().map(Dynamic::from).collect()
        })
        .register_fn("values", |headers: &mut ScriptHeaders| -> Array {
            headers.values().into_iter().map(Dynamic::from).collect()
        })
        .register_fn("entries", |headers: &mut ScriptHeaders| -> Array {
            headers
                .entries
                .iter()
                .map(|h| {
                    let mut item = Map::new();
                    item.insert("name".into(), h.name.clone().into());
                    item.insert(
                        "value".into(),
                        String::from_utf8_lossy(&h.value).into_owned().into(),
                    );
                    Dynamic::from_map(item)
                })
                .collect()
        })
        .register_fn("len", |headers: &mut ScriptHeaders| -> i64 {
            headers.entries.len() as i64
        })
        .register_fn("is_empty", |headers: &mut ScriptHeaders| -> bool {
            headers.entries.is_empty()
        })
        .register_fn("clear", |headers: &mut ScriptHeaders| {
            headers.entries.clear();
        })
        .register_fn("contains", |headers: &mut ScriptHeaders, name: &str| -> bool {
            headers.has(name)
        })
        .register_fn("remove", |headers: &mut ScriptHeaders, name: &str| -> Dynamic {
            headers.remove(name).map_or(Dynamic::UNIT, Dynamic::from)
        })
        .register_fn("==", |a: &mut ScriptHeaders, b: ScriptHeaders| *a == b)
        .register_fn("!=", |a: &mut ScriptHeaders, b: ScriptHeaders| *a != b)
        .register_fn("to_string", |headers: &mut ScriptHeaders| -> String {
            serde_json::to_string(&headers.to_json_value()).unwrap_or_default()
        })
        .register_fn("to_debug", |headers: &mut ScriptHeaders| -> String {
            serde_json::to_string(&headers.to_json_value()).unwrap_or_default()
        })
        .register_fn(
            "header_value",
            |headers: &mut ScriptHeaders, name: &str| -> String {
                headers.get_first(name).unwrap_or_default()
            },
        )
        .register_fn(
            "headerValue",
            |headers: &mut ScriptHeaders, name: &str| -> String {
                headers.get_first(name).unwrap_or_default()
            },
        )
        .register_fn(
            "header_values",
            |headers: &mut ScriptHeaders, name: &str| -> Array {
                headers
                    .get_all(name)
                    .into_iter()
                    .map(Dynamic::from)
                    .collect()
            },
        )
        .register_fn(
            "headerValues",
            |headers: &mut ScriptHeaders, name: &str| -> Array {
                headers
                    .get_all(name)
                    .into_iter()
                    .map(Dynamic::from)
                    .collect()
            },
        )
        .register_fn("has_header", |headers: &mut ScriptHeaders, name: &str| -> bool {
            headers.has(name)
        })
        .register_fn("hasHeader", |headers: &mut ScriptHeaders, name: &str| -> bool {
            headers.has(name)
        })
        .register_fn(
            "set_header",
            |headers: &mut ScriptHeaders, name: &str, value: &str| -> ScriptResult<()> {
                headers.set(name, value)
            },
        )
        .register_fn(
            "setHeader",
            |headers: &mut ScriptHeaders, name: &str, value: &str| -> ScriptResult<()> {
                headers.set(name, value)
            },
        )
        .register_fn(
            "add_header",
            |headers: &mut ScriptHeaders, name: &str, value: &str| -> ScriptResult<()> {
                headers.add(name, value)
            },
        )
        .register_fn(
            "addHeader",
            |headers: &mut ScriptHeaders, name: &str, value: &str| -> ScriptResult<()> {
                headers.add(name, value)
            },
        )
        .register_fn(
            "remove_header",
            |headers: &mut ScriptHeaders, name: &str| -> ScriptResult<()> {
                headers.remove(name);
                Ok(())
            },
        )
        .register_fn(
            "removeHeader",
            |headers: &mut ScriptHeaders, name: &str| -> ScriptResult<()> {
                headers.remove(name);
                Ok(())
            },
        )
        .register_fn(
            "remove_headers",
            |headers: &mut ScriptHeaders, names: Array| -> ScriptResult<()> {
                for item in names {
                    let name = item
                        .try_cast::<ImmutableString>()
                        .ok_or_else(|| runtime_error("remove_headers 参数数组元素必须为字符串"))?;
                    headers.remove(name.as_str());
                }
                Ok(())
            },
        )
        .register_fn(
            "removeHeaders",
            |headers: &mut ScriptHeaders, names: Array| -> ScriptResult<()> {
                for item in names {
                    let name = item
                        .try_cast::<ImmutableString>()
                        .ok_or_else(|| runtime_error("remove_headers 参数数组元素必须为字符串"))?;
                    headers.remove(name.as_str());
                }
                Ok(())
            },
        );
}

fn register_script_methods(engine: &mut Engine) {
    engine
        .register_fn("request", |map: &mut Map| -> ScriptResult<Map> {
            map.get("request")
                .and_then(|value| value.clone().try_cast::<Map>())
                .ok_or_else(|| runtime_error("MioScript request 对象缺失"))
        })
        .register_fn("response", |map: &mut Map| -> ScriptResult<Map> {
            if !map_bool_checked(map, "has_response")? {
                return Err(runtime_error("当前记录没有响应，无法访问 response()"));
            }
            map.get("response")
                .and_then(|value| value.clone().try_cast::<Map>())
                .ok_or_else(|| runtime_error("MioScript response 对象缺失"))
        })
        .register_fn("annotations", |map: &mut Map| -> ScriptResult<Map> {
            map.get("annotations")
                .and_then(|value| value.clone().try_cast::<Map>())
                .ok_or_else(|| runtime_error("MioScript annotations 对象缺失"))
        })
        .register_fn("hasResponse", |map: &mut Map| -> ScriptResult<bool> {
            map_bool_checked(map, "has_response")
        })
        .register_fn("isInScope", |map: &mut Map| -> ScriptResult<bool> {
            map_bool_checked(map, "in_scope")
        })
        .register_fn("hasParameters", |map: &mut Map| -> ScriptResult<bool> {
            map_bool_checked(map, "parameterized")
        })
        .register_fn("method", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "method")
        })
        .register_fn("path", |map: &mut Map| -> ScriptResult<String> {
            if map.contains_key("path_with_query") {
                map_string_checked(map, "path_with_query")
            } else {
                map_string_checked(map, "path")
            }
        })
        .register_fn("pathWithoutQuery", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "path")
        })
        .register_fn("query", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "query")
        })
        .register_fn("url", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "url")
        })
        .register_fn("host", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "host")
        })
        .register_fn("httpVersion", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "version")
        })
        .register_fn("bodyToString", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "body")
        })
        .register_fn("statusCode", |map: &mut Map| -> ScriptResult<i64> {
            let status = map_int_checked(map, "status")?;
            if status <= 0 {
                return Err(runtime_error("当前记录没有有效响应状态码"));
            }
            Ok(status)
        })
        .register_fn("mimeType", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "mime_type")
        })
        .register_fn("listenerPort", |map: &mut Map| -> ScriptResult<i64> {
            let port = map_int_checked(map, "listener_port")?;
            if port <= 0 {
                return Err(runtime_error("当前记录缺少有效监听端口"));
            }
            Ok(port)
        })
        .register_fn("notes", |map: &mut Map| -> ScriptResult<String> {
            map_string_checked(map, "notes")
        })
        .register_fn("hasNotes", |map: &mut Map| -> ScriptResult<bool> {
            Ok(!map_string_checked(map, "notes")?.is_empty())
        })
        .register_fn("highlightColor", |map: &mut Map| -> ScriptResult<String> {
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
        })
        .register_fn("hasHighlightColor", |map: &mut Map| -> ScriptResult<bool> {
            map_bool_checked(map, "highlighted")
        })
        .register_fn("headerValue", header_value)
        .register_fn("headerValues", header_values)
        .register_fn("hasHeader", has_header)
        .register_fn("parameterValue", |map: &mut Map, name: &str| -> ScriptResult<String> {
            Ok(request_parameter_lookup(map, name)?.unwrap_or_default())
        })
        .register_fn("hasParameter", |map: &mut Map, name: &str| -> ScriptResult<bool> {
            Ok(request_parameter_lookup(map, name)?.is_some())
        })
        .register_fn(
            "contains",
            |map: &mut Map, pattern: &str, case_sensitive: bool| -> ScriptResult<bool> {
                map_contains_text(map, pattern, case_sensitive)
            },
        )
        .register_fn("contains", |map: &mut Map, pattern: &str| -> ScriptResult<bool> {
            map_contains_text(map, pattern, true)
        })
        .register_fn("setHeader", set_header)
        .register_fn("addHeader", add_header)
        .register_fn("removeHeader", remove_header)
        .register_fn("removeHeaders", remove_headers)
        .register_fn("removeAllCookies", remove_all_cookies)
        .register_fn("removeCookie", remove_cookie)
        .register_fn("setCookie", set_cookie)
        .register_fn("setQueryParam", set_query_param)
        .register_fn("removeQueryParam", remove_query_param)
        .register_fn("setJsonField", set_json_field)
        .register_fn("parseCookies", parse_cookies)
        .register_fn("formParam", form_param)
        .register_fn("setFormParam", set_form_param)
        .register_fn("removeFormParam", remove_form_param);
}

pub(super) fn register_builtins(engine: &mut Engine) {
    engine
        .register_fn("url_encode", url_encode_str)
        .register_fn("url_decode", url_decode_str)
        .register_fn("base64_encode", base64_encode_str)
        .register_fn("base64_decode", base64_decode_str)
        .register_fn("base64url_encode", base64url_encode)
        .register_fn("base64url_decode", base64url_decode)
        .register_fn("sha256_hex", sha256_hex_str)
        .register_fn("md5_hex", md5_hex_str)
        .register_fn("crc32_hex", |text: &str| -> String {
            format!("{:08x}", codec_util::crc32_checksum(text.as_bytes()))
        })
        .register_fn("regex_matches", |text: &str, pattern: &str| -> ScriptResult<bool> {
            Ok(script_regex(pattern)?.is_match(text))
        })
        .register_fn(
            "regex_replace",
            |text: &str, pattern: &str, replacement: &str| -> ScriptResult<String> {
                let rule = MatchAndReplaceRule {
                    match_pattern: pattern.into(),
                    replace_with: replacement.into(),
                    regex: true,
                    ..Default::default()
                };
                let regex = script_regex(pattern)?;
                Ok(replace_text_with_rule(text, &rule, Some(&regex), MAX_BODY, &|| Ok(()))
                    .map_err(runtime_error)?
                    .unwrap_or_else(|| text.to_owned()))
            },
        )
        .register_fn("unix_timestamp", || -> ScriptResult<i64> {
            Ok(codec_util::now_unix_ms() / 1000)
        })
        .register_fn("unix_timestamp_ms", || -> ScriptResult<i64> {
            Ok(codec_util::now_unix_ms())
        })
        .register_fn("header_value", header_value)
        .register_fn("header_values", header_values)
        .register_fn("set_header", set_header)
        .register_fn("add_header", add_header)
        .register_fn("remove_header", remove_header)
        .register_fn("remove_headers", remove_headers)
        .register_fn("remove_all_cookies", remove_all_cookies)
        .register_fn("remove_cookie", remove_cookie)
        .register_fn("set_cookie", set_cookie)
        .register_fn("has_header", has_header)
        .register_fn("query_param", query_param)
        .register_fn("set_query_param", set_query_param)
        .register_fn("remove_query_param", remove_query_param)
        .register_fn("json_parse", json_parse)
        .register_fn("json_stringify", json_stringify)
        .register_fn("json_stringify_pretty", json_stringify_pretty)
        .register_fn("json_get", json_get)
        .register_fn("set_json_field", set_json_field)
        .register_fn("hex_encode", hex_encode)
        .register_fn("hex_decode", hex_decode)
        .register_fn("html_encode", html_encode)
        .register_fn("html_decode", html_decode)
        .register_fn("hmac_sha256_hex", hmac_sha256_hex)
        .register_fn("hmac_sha256_base64", hmac_sha256_base64)
        .register_fn("hmac_sha256_base64url", hmac_sha256_base64url)
        .register_fn("jwt_decode", jwt_decode)
        .register_fn("parse_cookies", parse_cookies)
        .register_fn("form_param", form_param)
        .register_fn("set_form_param", set_form_param)
        .register_fn("remove_form_param", remove_form_param);

    register_script_utilities(engine);
    register_script_headers(engine);
    register_script_methods(engine);
}
