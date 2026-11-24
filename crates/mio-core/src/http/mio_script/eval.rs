use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::{FlowRecord, Header, HttpRequest, HttpResponse, HttpVersion};
use rhai::{AST, Array, Dynamic, Engine, ImmutableString, Map, Scope};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::super::{
    MAX_BODY, MAX_HEAD, detect_request_line_version, head, is_chunked, parse_imported_request,
    parse_imported_response, parse_repeater_request, parse_response, request_to_raw,
    response_to_raw, token, valid_header_value_bytes,
};
use super::builtins::{ScriptHeaders, ScriptUtilities, string_field};
use super::engine::{
    ActiveCancelGuard, PrintCaptureGuard, SCRIPT_CANCELLED_MESSAGE, compile_cached,
    format_eval_error, with_shared_engine,
};
use crate::{HttpExtensionFilter, HttpHistoryFilter, WebSocketDirection, WebSocketHistoryFilter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightFilterState {
    Boolean(bool),
    ColorCode(u8),
}

impl Default for HighlightFilterState {
    fn default() -> Self {
        Self::Boolean(false)
    }
}

#[derive(Clone, Debug)]
pub struct FilterScriptInput<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub status: Option<u16>,
    pub length: u64,
    pub request_raw: Option<&'a [u8]>,
    pub response_raw: Option<&'a [u8]>,
    pub notes: &'a str,
    pub highlight: HighlightFilterState,
    pub listener_port: Option<u16>,
    pub in_scope: bool,
    pub cancel: Option<&'a CancellationToken>,
}

#[derive(Clone, Debug)]
pub struct WsFilterScriptInput<'a> {
    pub url: &'a str,
    pub direction: WebSocketDirection,
    pub payload: &'a [u8],
    pub edited: bool,
    pub notes: &'a str,
    pub listener_port: Option<u16>,
    pub in_scope: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomActionOutcome {
    pub output: String,
    pub updated_request: Option<HttpRequest>,
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

fn build_request_map(
    request: &HttpRequest,
    raw_override: Option<&[u8]>,
    include_raw: bool,
) -> Result<(Map, Option<String>)> {
    let url = Url::parse(&request.url).context("MioScript 请求 URL 无效")?;
    let path_with_query = match url.query() {
        Some(q) if !q.is_empty() => format!("{}?{q}", url.path()),
        _ => url.path().to_owned(),
    };
    let mut map: Map = [
        ("method", request.method.clone().into()),
        ("url", request.url.clone().into()),
        ("path", url.path().into()),
        ("path_with_query", path_with_query.into()),
        ("query", url.query().unwrap_or_default().into()),
        (
            "host",
            url.host_str()
                .context("MioScript 请求 URL 缺少主机")?
                .into(),
        ),
        ("version", request.version.as_str().into()),
        (
            "headers",
            Dynamic::from(ScriptHeaders::from_headers(&request.headers)),
        ),
        (
            "body",
            String::from_utf8_lossy(&request.body).into_owned().into(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();

    let original_raw = if include_raw {
        let raw_str = match raw_override {
            Some(raw) => String::from_utf8_lossy(raw).into_owned(),
            None => String::from_utf8_lossy(&request_to_raw(request)).into_owned(),
        };
        map.insert("raw".into(), raw_str.clone().into());
        Some(raw_str)
    } else {
        None
    };

    Ok((map, original_raw))
}

fn build_response_map(
    response: Option<&HttpResponse>,
    raw_override: Option<&[u8]>,
    include_raw: bool,
) -> (Map, Option<String>) {
    let headers = response
        .map(|resp| ScriptHeaders::from_headers(&resp.headers))
        .unwrap_or_default();
    let mut map: Map = [
        (
            "status",
            response.map_or(0, |resp| i64::from(resp.status)).into(),
        ),
        (
            "version",
            response.map_or("", |resp| resp.version.as_str()).into(),
        ),
        ("headers", Dynamic::from(headers)),
        (
            "body",
            response
                .map(|resp| String::from_utf8_lossy(&resp.body).into_owned())
                .unwrap_or_default()
                .into(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();

    let original_raw = if include_raw {
        let raw_str = match (raw_override, response) {
            (Some(raw), _) => String::from_utf8_lossy(raw).into_owned(),
            (None, Some(resp)) => String::from_utf8_lossy(&response_to_raw(resp)).into_owned(),
            (None, None) => String::new(),
        };
        map.insert("raw".into(), raw_str.clone().into());
        Some(raw_str)
    } else {
        None
    };

    (map, original_raw)
}

pub(in crate::http) fn version_from_script(version: &str) -> Result<HttpVersion> {
    match version {
        "HTTP/1.0" => Ok(HttpVersion::Http10),
        "HTTP/1.1" => Ok(HttpVersion::Http11),
        "HTTP/2" | "HTTP/2.0" => Ok(HttpVersion::Http2),
        _ => bail!("MioScript HTTP 版本无效：{version}"),
    }
}

fn sync_headers(headers: &mut Vec<Header>, map: &Map) -> Result<()> {
    let container = map
        .get("headers")
        .context("MioScript headers 必须为 Headers 或 Map")?;
    if let Some(script_headers) = container.clone().try_cast::<ScriptHeaders>() {
        script_headers.validate_all()?;
        *headers = script_headers.into_headers();
        return Ok(());
    }
    let desired = container
        .clone()
        .try_cast::<Map>()
        .context("MioScript headers 必须为 Headers 或 Map")?;
    let mut original = Map::new();
    for header in headers.iter() {
        original.insert(
            header.name.clone().into(),
            Dynamic::from(String::from_utf8_lossy(&header.value).into_owned()),
        );
    }
    headers.retain(|header| desired.contains_key(header.name.as_str()));
    for (name, value) in desired {
        let text = value
            .clone()
            .try_cast::<ImmutableString>()
            .context("MioScript 请求头值必须为字符串")?;
        ensure!(
            token(name.as_bytes()) && valid_header_value_bytes(text.as_bytes()),
            "MioScript 请求头名称或值无效"
        );
        if original
            .get(&name)
            .is_some_and(|old| old.clone().try_cast::<ImmutableString>() == Some(text.clone()))
        {
            continue;
        }
        headers.retain(|header| !header.name.eq_ignore_ascii_case(&name));
        headers.push(Header {
            name: name.to_string(),
            value: text.as_bytes().to_vec(),
        });
    }
    ensure!(
        headers.len() <= 1024
            && headers
                .iter()
                .map(|header| header.name.len() + header.value.len() + 4)
                .sum::<usize>()
                <= MAX_HEAD,
        "MioScript HTTP 请求头超过限额"
    );
    Ok(())
}

fn sync_content_length(headers: &mut Vec<Header>, body_len: usize) {
    if is_chunked(headers) {
        return;
    }
    let mut present = false;
    headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case("content-length") {
            return true;
        }
        if present {
            return false;
        }
        present = true;
        header.value = body_len.to_string().into_bytes();
        true
    });
    if !present && body_len > 0 {
        headers.push(Header {
            name: "Content-Length".into(),
            value: body_len.to_string().into_bytes(),
        });
    }
}

fn apply_request_map(
    request: &mut HttpRequest,
    map: &Map,
    original_raw: Option<&str>,
) -> Result<()> {
    if let Some(raw_val) = map.get("raw") {
        let raw = raw_val
            .clone()
            .try_cast::<ImmutableString>()
            .context("MioScript 字段 raw 必须为字符串")?;
        if original_raw.is_none_or(|orig| raw.as_str() != orig) {
            let version =
                detect_request_line_version(raw.as_bytes()).context("MioScript 请求首行无效")?;
            *request = parse_repeater_request(raw.as_bytes(), &request.url, version, true)?;
            return Ok(());
        }
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
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && !next_url
                .chars()
                .any(|character| character.is_control() || character.is_whitespace()),
        "MioScript 请求 URL 无效"
    );
    request.method = method;
    request.url = next_url;
    request.version = version_from_script(&string_field(map, "version")?)?;
    sync_headers(&mut request.headers, map)?;
    let body = string_field(map, "body")?;
    if body != String::from_utf8_lossy(&request.body) {
        request.body = body.into_bytes();
    }
    ensure!(
        request.body.len() <= MAX_BODY,
        "MioScript 请求正文超过 16 MiB 上限"
    );
    sync_content_length(&mut request.headers, request.body.len());
    Ok(())
}

fn apply_response_map(
    response: &mut HttpResponse,
    map: &Map,
    original_raw: Option<&str>,
) -> Result<()> {
    if let Some(raw_val) = map.get("raw") {
        let raw = raw_val
            .clone()
            .try_cast::<ImmutableString>()
            .context("MioScript 字段 raw 必须为字符串")?;
        if original_raw.is_none_or(|orig| raw.as_str() != orig) {
            let first = head(raw.as_bytes())?.0;
            let version = std::str::from_utf8(&first)?
                .split_ascii_whitespace()
                .next()
                .context("MioScript 响应首行缺失")?;
            *response = parse_response(raw.as_bytes(), version_from_script(version)?)?;
            sync_content_length(&mut response.headers, response.body.len());
            return Ok(());
        }
    }

    let status = map
        .get("status")
        .and_then(|value| value.clone().try_cast::<i64>())
        .context("MioScript status 必须为整数")?;
    ensure!((100..=599).contains(&status), "MioScript 响应状态码无效");
    response.status = status as u16;
    response.version = version_from_script(&string_field(map, "version")?)?;
    sync_headers(&mut response.headers, map)?;
    let body = string_field(map, "body")?;
    if body != String::from_utf8_lossy(&response.body) {
        response.body = body.into_bytes();
    }
    ensure!(
        response.body.len() <= MAX_BODY,
        "MioScript 响应正文超过 16 MiB 上限"
    );
    sync_content_length(&mut response.headers, response.body.len());
    Ok(())
}

/// Executes a Match and Replace MioScript against `record` for request or response stage.
pub fn execute_http_match_replace_script(
    script: &str,
    record: &mut FlowRecord,
    response_stage: bool,
) -> Result<bool> {
    let compiled = compile_cached(script)?;
    let (req_map, original_req_raw) =
        build_request_map(&record.request, None, compiled.requirements.needs_raw)?;
    let (resp_map, original_resp_raw) = build_response_map(
        record.response.as_ref(),
        None,
        compiled.requirements.needs_raw,
    );

    let mut scope = Scope::new();
    scope.push("request", req_map);
    scope.push("response", resp_map);

    let returned = with_shared_engine(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(None);
        engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, compiled.ast.as_ref())
            .map_err(|error| format_eval_error(error, "MioScript 执行错误"))
    })?;

    let name = if response_stage {
        "response"
    } else {
        "request"
    };
    if let Some(map) = returned.clone().try_cast::<Map>() {
        scope.set_value(name, map);
    } else if let Some(raw) = returned.clone().try_cast::<ImmutableString>() {
        let mut map = scope
            .get_value::<Map>(name)
            .context("MioScript 消息对象缺失")?;
        map.insert("raw".into(), raw.into());
        scope.set_value(name, map);
    } else {
        ensure!(
            returned.is_unit(),
            "MioScript 替换脚本必须返回消息 Map、原始报文字符串或空值"
        );
    }

    let mut candidate = record.clone();
    if response_stage {
        let response = candidate
            .response
            .as_mut()
            .context("MioScript 响应阶段缺少响应")?;
        let final_resp_map = scope
            .get_value::<Map>("response")
            .context("MioScript response 必须为 Map")?;
        apply_response_map(response, &final_resp_map, original_resp_raw.as_deref())?;
        candidate.summary.response_bytes = response.body.len() as u64;
        candidate.summary.status = Some(response.status);
    } else {
        let final_req_map = scope
            .get_value::<Map>("request")
            .context("MioScript request 必须为 Map")?;
        apply_request_map(
            &mut candidate.request,
            &final_req_map,
            original_req_raw.as_deref(),
        )?;
        candidate.summary.request_bytes = candidate.request.body.len() as u64;
        candidate.summary.method = candidate.request.method.clone();
        candidate.summary.url = candidate.request.url.clone();
    }

    let changed = candidate != *record;
    *record = candidate;
    Ok(changed)
}

fn parse_filter_url(method: &str, url: &str) -> Result<Url> {
    if method.eq_ignore_ascii_case("CONNECT")
        && !url.contains("://")
        && let Ok(parsed) = Url::parse(&format!("https://{url}/"))
        && parsed.host_str().is_some()
    {
        return Ok(parsed);
    }
    Url::parse(url).context("MioScript 请求 URL 无效")
}

/// Evaluates an HTTP filter script against `FilterScriptInput`.
pub fn eval_http_filter_script_input(script: &str, input: &FilterScriptInput<'_>) -> Result<bool> {
    let (highlighted, highlight_name, highlight_color_unknown) = match input.highlight {
        HighlightFilterState::ColorCode(color) => (
            (1..=9).contains(&color),
            highlight_color_name(color)?,
            false,
        ),
        HighlightFilterState::Boolean(false) => (false, "NONE", false),
        HighlightFilterState::Boolean(true) => (true, "", true),
    };

    let fallback_cancel = CancellationToken::new();
    let cancel = input.cancel.unwrap_or(&fallback_cancel);
    ensure!(!cancel.is_cancelled(), "{SCRIPT_CANCELLED_MESSAGE}");
    let compiled = compile_cached(script)?;
    ensure!(!cancel.is_cancelled(), "{SCRIPT_CANCELLED_MESSAGE}");

    let reqs = compiled.requirements;
    let parsed_request = if let Some(raw) = input.request_raw {
        let (request, _) =
            parse_imported_request(raw, Some(input.url)).context("MioScript 历史请求无法解析")?;
        request
    } else {
        let normalized_url = if !input.url.is_empty()
            && input.method.eq_ignore_ascii_case("CONNECT")
            && !input.url.contains("://")
        {
            parse_filter_url(input.method, input.url)?.into()
        } else {
            input.url.into()
        };
        HttpRequest {
            method: input.method.into(),
            url: normalized_url,
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        }
    };

    let (mut request, req_raw_str) = if input.url.is_empty() && input.request_raw.is_none() {
        let mut empty_map: Map = [
            ("method", input.method.into()),
            ("url", "".into()),
            ("path", "".into()),
            ("path_with_query", "".into()),
            ("query", "".into()),
            ("host", "".into()),
            ("version", "".into()),
            ("headers", Dynamic::from(ScriptHeaders::default())),
            ("body", "".into()),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect();
        let raw_opt = if reqs.needs_raw {
            empty_map.insert("raw".into(), "".into());
            Some(String::new())
        } else {
            None
        };
        (empty_map, raw_opt)
    } else {
        let (mut map, raw_opt) =
            build_request_map(&parsed_request, input.request_raw, reqs.needs_raw)?;
        map.insert("method".into(), input.method.into());
        map.insert("url".into(), input.url.into());
        if !input.url.is_empty() {
            let parsed_url = parse_filter_url(input.method, input.url)?;
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
        (map, raw_opt)
    };

    let response = input
        .response_raw
        .map(|raw| {
            parse_imported_response(raw)
                .map(|(response, _)| response)
                .context("MioScript 历史响应无法解析")
        })
        .transpose()?;
    let (mut response_data, resp_raw_str) =
        build_response_map(response.as_ref(), input.response_raw, reqs.needs_raw);
    response_data.insert("status".into(), i64::from(input.status.unwrap_or(0)).into());
    response_data.insert("length".into(), i64::try_from(input.length)?.into());

    if !input.url.is_empty() {
        let _ = parse_filter_url(input.method, input.url)?;
    }
    let parameterized =
        crate::store::ext_archive::http_query::filters::url_has_parameters(input.url, cancel)?
            || crate::store::ext_archive::http_query::filters::history_body_has_parameters(
                &parsed_request.headers,
                &parsed_request.body,
                cancel,
            )?;

    let mime_index = response.as_ref().map_or(7_i64, |response| {
        crate::store::ext_archive::http_query::filters::infer_history_mime(
            &response.headers,
            &response.body,
        )
        .index() as i64
    });
    let mime_label = mio_script_mime_type(response.as_ref());

    request.insert("in_scope".into(), input.in_scope.into());
    request.insert("parameterized".into(), parameterized.into());
    response_data.insert("mime_type".into(), mime_label.into());

    let search_text = if reqs.needs_search_text {
        Some(format!(
            "{}\n{}",
            req_raw_str.as_deref().unwrap_or_default(),
            resp_raw_str.as_deref().unwrap_or_default()
        ))
    } else {
        None
    };

    let annotations: Map = [
        ("notes", input.notes.to_owned().into()),
        ("highlighted", highlighted.into()),
        ("highlight_color", highlight_name.into()),
        ("highlight_color_unknown", highlight_color_unknown.into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();

    let mut scope = Scope::new();
    if reqs.needs_request_response {
        let mut request_response: Map = [
            ("request", Dynamic::from(request.clone())),
            ("response", Dynamic::from(response_data.clone())),
            ("annotations", Dynamic::from(annotations.clone())),
            ("method", string_field(&request, "method")?.into()),
            ("url", string_field(&request, "url")?.into()),
            ("path", string_field(&request, "path")?.into()),
            (
                "path_with_query",
                string_field(&request, "path_with_query")?.into(),
            ),
            ("query", string_field(&request, "query")?.into()),
            ("host", string_field(&request, "host")?.into()),
            ("status", i64::from(input.status.unwrap_or(0)).into()),
            ("mime_type", mime_label.into()),
            ("notes", input.notes.to_owned().into()),
            ("highlighted", highlighted.into()),
            ("highlight_color", highlight_name.into()),
            ("highlight_color_unknown", highlight_color_unknown.into()),
            (
                "listener_port",
                i64::from(input.listener_port.unwrap_or(0)).into(),
            ),
            ("in_scope", input.in_scope.into()),
            ("parameterized", parameterized.into()),
            ("has_response", input.status.is_some().into()),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect();
        if let Some(ref st) = search_text {
            request_response.insert("search_text".into(), st.clone().into());
        }
        scope.push("requestResponse", request_response);
    }

    if let Some(st) = search_text {
        scope.push("search_text", st);
    }
    scope.push("request", request);
    scope.push("response", response_data);
    scope.push("annotations", annotations);
    scope.push("utilities", ScriptUtilities);
    scope.push("notes", input.notes.to_owned());
    scope.push("highlighted", highlighted);
    scope.push("listener_port", i64::from(input.listener_port.unwrap_or(0)));
    scope.push("in_scope", input.in_scope);
    scope.push("parameterized", parameterized);
    scope.push("has_response", input.status.is_some());
    scope.push("mime_index", mime_index);

    with_shared_engine(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(input.cancel.cloned());
        eval_filter(engine, compiled.ast.as_ref(), &mut scope)
    })
}

/// Evaluates a WebSocket filter script against `WsFilterScriptInput`.
pub fn eval_ws_filter_script_input(script: &str, input: &WsFilterScriptInput<'_>) -> Result<bool> {
    ensure!(
        input.payload.len() <= MAX_BODY,
        "MioScript WebSocket 正文超过 16 MiB 上限"
    );
    let compiled = compile_cached(script)?;
    let direction = match input.direction {
        WebSocketDirection::ClientToServer => "c2s",
        WebSocketDirection::ServerToClient => "s2c",
    };
    let message: Map = [
        ("url", input.url.into()),
        ("direction", direction.into()),
        (
            "payload",
            String::from_utf8_lossy(input.payload).into_owned().into(),
        ),
        ("length", (input.payload.len() as i64).into()),
        ("edited", input.edited.into()),
        ("notes", input.notes.into()),
        (
            "listener_port",
            i64::from(input.listener_port.unwrap_or(0)).into(),
        ),
        ("in_scope", input.in_scope.into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect();
    let mut scope = Scope::new();
    scope.push("message", message);
    scope.push("in_scope", input.in_scope);
    with_shared_engine(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(None);
        eval_filter(engine, compiled.ast.as_ref(), &mut scope)
    })
}

fn eval_filter(engine: &Engine, ast: &AST, scope: &mut Scope<'_>) -> Result<bool> {
    engine
        .eval_ast_with_scope::<bool>(scope, ast)
        .map_err(|error| format_eval_error(error, "MioScript 过滤脚本必须返回 bool"))
}

fn expression(conditions: Vec<String>) -> String {
    if conditions.is_empty() {
        "true".into()
    } else {
        conditions.join("\n    && ")
    }
}

fn search_expression(
    text: &str,
    term: &str,
    regex: bool,
    case_sensitive: bool,
    negative: bool,
) -> String {
    let pattern = if regex {
        term.to_owned()
    } else {
        regex::escape(term)
    };
    let pattern = if case_sensitive {
        pattern
    } else {
        format!("(?i){pattern}")
    };
    let quoted = serde_json::to_string(&pattern).expect("字符串序列化成功");
    format!(
        "{}regex_matches({text}, {quoted})",
        if negative { "!" } else { "" }
    )
}

/// Converts a structured [`HttpHistoryFilter`] into an equivalent MioScript filter expression.
pub fn convert_http_filter_to_script(filter: &HttpHistoryFilter) -> String {
    let mut conditions = Vec::new();
    if filter.in_scope_only {
        conditions.push("in_scope".into());
    }
    if filter.hide_without_response {
        conditions.push("has_response".into());
    }
    if filter.hide_connect {
        conditions.push("!regex_matches(request.method, \"(?i)^CONNECT$\")".into());
    }
    if filter.parameterized_only {
        conditions.push("parameterized".into());
    }
    if filter.status_classes != [true; 4] {
        let classes: Vec<String> = filter
            .status_classes
            .iter()
            .enumerate()
            .filter(|(_, enabled)| **enabled)
            .map(|(index, _)| format!("response.status / 100 == {}", index + 2))
            .collect();
        conditions.push(format!(
            "(!has_response || ({}))",
            if classes.is_empty() {
                "false".into()
            } else {
                classes.join(" || ")
            }
        ));
    }
    if filter.mime_types != [true; 8] {
        let indexes: Vec<String> = filter
            .mime_types
            .iter()
            .enumerate()
            .filter(|(_, enabled)| **enabled)
            .map(|(index, _)| format!("mime_index == {index}"))
            .collect();
        conditions.push(format!(
            "({})",
            if indexes.is_empty() {
                "false".into()
            } else {
                indexes.join(" || ")
            }
        ));
    }
    if let HttpExtensionFilter::Only(extensions) | HttpExtensionFilter::Hide(extensions) =
        &filter.extensions
    {
        let patterns: Vec<_> = extensions
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|value| !value.is_empty())
            .map(|value| regex::escape(value.trim_start_matches('.')))
            .collect();
        let pattern = format!("(?i)\\.(?:{})$", patterns.join("|"));
        if !patterns.is_empty() {
            conditions.push(format!(
                "{}regex_matches(request.path, {})",
                if matches!(filter.extensions, HttpExtensionFilter::Hide(_)) {
                    "!"
                } else {
                    ""
                },
                serde_json::to_string(&pattern).expect("字符串序列化成功")
            ));
        }
    }
    if filter.only_with_notes {
        conditions.push("!notes.is_empty()".into());
    }
    if filter.only_highlighted {
        conditions.push("highlighted".into());
    }
    if let Some(port) = filter.listener_port {
        conditions.push(format!("listener_port == {port}"));
    }
    if !filter.search_term.is_empty() {
        conditions.push(search_expression(
            "search_text",
            &filter.search_term,
            filter.regex,
            filter.case_sensitive,
            filter.negative_search,
        ));
    }
    expression(conditions)
}

/// Converts a structured [`WebSocketHistoryFilter`] into an equivalent MioScript filter expression.
pub fn convert_ws_filter_to_script(
    filter: &WebSocketHistoryFilter,
    in_scope_only: bool,
) -> String {
    let mut conditions = Vec::new();
    if in_scope_only {
        conditions.push("in_scope".into());
    }
    if filter.hide_incoming {
        conditions.push("message.direction != \"s2c\"".into());
    }
    if filter.hide_outgoing {
        conditions.push("message.direction != \"c2s\"".into());
    }
    if filter.only_with_notes {
        conditions.push("!message.notes.is_empty()".into());
    }
    if let Some(port) = filter.listener_port {
        conditions.push(format!("message.listener_port == {port}"));
    }
    if !filter.term.is_empty() {
        conditions.push(search_expression(
            "message.payload",
            &filter.term,
            filter.regex,
            filter.case_sensitive,
            filter.negative,
        ));
    }
    expression(conditions)
}

/// Evaluates a custom passive/active scan check script against `request` and `response`.
pub fn eval_custom_scan_check_script(
    script: &str,
    request: &HttpRequest,
    response: &HttpResponse,
) -> Result<Option<String>> {
    let trimmed = script.trim();
    ensure!(!trimmed.is_empty(), "自定义扫描检查脚本不能为空");
    ensure!(trimmed.len() <= 64 * 1024, "自定义扫描检查脚本超过 64 KiB 上限");

    let compiled = compile_cached(trimmed)?;
    let (req_map, _) = build_request_map(request, None, compiled.requirements.needs_raw)?;
    let (resp_map, _) =
        build_response_map(Some(response), None, compiled.requirements.needs_raw);

    let mut scope = Scope::new();
    if compiled.requirements.needs_request_response {
        let mut rr = Map::new();
        rr.insert("request".into(), Dynamic::from(req_map.clone()));
        rr.insert("response".into(), Dynamic::from(resp_map.clone()));
        rr.insert("has_response".into(), true.into());
        scope.push("requestResponse", rr);
    }
    scope.push("request", req_map);
    scope.push("response", resp_map);
    scope.push("utilities", ScriptUtilities);

    let val = with_shared_engine(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(None);
        engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, compiled.ast.as_ref())
            .map_err(|err| format_eval_error(err, "自定义扫描检查脚本执行失败"))
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
    if let (Some(fa), Some(fb)) = (a.clone().try_cast::<f64>(), b.clone().try_cast::<f64>()) {
        return fa == fb;
    }
    if let (Some(ba), Some(bb)) = (a.clone().try_cast::<bool>(), b.clone().try_cast::<bool>()) {
        return ba == bb;
    }
    if let (Some(ha), Some(hb)) = (
        a.clone().try_cast::<ScriptHeaders>(),
        b.clone().try_cast::<ScriptHeaders>(),
    ) {
        return ha == hb;
    }
    if let (Some(ma), Some(mb)) = (a.clone().try_cast::<Map>(), b.clone().try_cast::<Map>()) {
        return dynamic_map_eq(&ma, &mb);
    }
    if let (Some(aa), Some(ab)) = (a.clone().try_cast::<Array>(), b.clone().try_cast::<Array>()) {
        return aa.len() == ab.len() && aa.iter().zip(ab.iter()).all(|(x, y)| dynamic_eq(x, y));
    }
    a.is_unit() && b.is_unit()
}

fn dynamic_map_eq(a: &Map, b: &Map) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(k, va)| b.get(k).is_some_and(|vb| dynamic_eq(va, vb)))
}

/// Executes a Repeater custom action script, capturing `print` output and request mutations.
pub fn eval_custom_action_with_mutation(
    script: &str,
    request: &HttpRequest,
    response: Option<&HttpResponse>,
) -> Result<CustomActionOutcome> {
    let trimmed = script.trim();
    ensure!(!trimmed.is_empty(), "自定义动作脚本不能为空");
    ensure!(trimmed.len() <= 64 * 1024, "自定义动作脚本超过 64 KiB 上限");

    let compiled = compile_cached(trimmed)?;
    let (req_map, original_req_raw) =
        build_request_map(request, None, compiled.requirements.needs_raw)?;
    let initial_req_map = req_map.clone();
    let (resp_map, _) = build_response_map(response, None, compiled.requirements.needs_raw);

    let mut scope = Scope::new();
    if compiled.requirements.needs_request_response {
        let mut rr = Map::new();
        rr.insert("request".into(), Dynamic::from(req_map.clone()));
        rr.insert("response".into(), Dynamic::from(resp_map.clone()));
        rr.insert("has_response".into(), response.is_some().into());
        scope.push("requestResponse", rr);
    }
    scope.push("request", req_map);
    scope.push("response", resp_map);
    scope.push("utilities", ScriptUtilities);

    let print_guard = PrintCaptureGuard::start();
    let eval_res = with_shared_engine(|engine| {
        let _cancel_guard = ActiveCancelGuard::set(None);
        engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, compiled.ast.as_ref())
            .map_err(|err| format_eval_error(err, "自定义动作执行失败"))
    });
    let mut lines = print_guard.finish();
    let val = eval_res?;

    let returned_request_map = val.clone().try_cast::<Map>().filter(|map| {
        map.contains_key("method") && map.contains_key("url") && map.contains_key("headers")
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
        apply_request_map(&mut candidate, &final_req_map, original_req_raw.as_deref())?;
        if candidate != *request {
            updated_request = Some(candidate);
        }
    }

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

/// Executes a custom action script and returns only its rendered text output.
pub fn eval_custom_action_script(
    script: &str,
    request: &HttpRequest,
    response: Option<&HttpResponse>,
) -> Result<String> {
    eval_custom_action_with_mutation(script, request, response).map(|outcome| outcome.output)
}
