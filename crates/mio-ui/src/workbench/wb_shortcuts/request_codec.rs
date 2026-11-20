use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyEncodingTarget {
    Toggle,
    FormUrlEncoded,
    MultipartFormData,
    ApplicationJson,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetectedBodyEncoding {
    FormUrlEncoded,
    MultipartFormData,
    ApplicationJson,
}

struct ParsedHttpRequest<'a> {
    method: &'a str,
    target: &'a str,
    version: &'static str,
    headers: Vec<httparse::Header<'a>>,
    body: &'a [u8],
}

fn parse_http_request_parts(raw: &[u8]) -> Result<ParsedHttpRequest<'_>, String> {
    let mut storage = vec![httparse::EMPTY_HEADER; 4096];
    let mut request = httparse::Request::new(&mut storage);
    let status = request
        .parse(raw)
        .map_err(|error| format!("HTTP 请求解析失败：{error}"))?;
    let offset = match status {
        httparse::Status::Complete(pos) => pos,
        httparse::Status::Partial => return Err("HTTP 请求头未完整结束。".into()),
    };
    let method = request.method.ok_or("HTTP 请求缺少方法。")?;
    let target = request.path.ok_or("HTTP 请求缺少路径。")?;
    let version = match request.version {
        Some(0) => "HTTP/1.0",
        Some(1) => "HTTP/1.1",
        _ => return Err("HTTP 请求版本无效。".into()),
    };
    let headers = request.headers.to_vec();
    if headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("transfer-encoding")
            || header.name.eq_ignore_ascii_case("content-encoding")
    }) {
        return Err("请求使用传输编码或压缩编码，请先完成解包。".into());
    }
    Ok(ParsedHttpRequest {
        method,
        target,
        version,
        headers,
        body: &raw[offset..],
    })
}

fn percent_encode_component(input: &[u8]) -> String {
    form_urlencoded::byte_serialize(input)
        .collect::<String>()
        .replace('+', "%20")
}

fn parsed_content_type(parsed: &ParsedHttpRequest<'_>) -> Option<mime::Mime> {
    parsed.headers.iter().find_map(|h| {
        if h.name.eq_ignore_ascii_case("content-type") {
            std::str::from_utf8(h.value)
                .ok()
                .and_then(|s| s.trim().parse::<mime::Mime>().ok())
        } else {
            None
        }
    })
}

fn flatten_json_value(value: &serde_json::Value, prefix: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json_value(v, &next, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                let next = format!("{prefix}[{idx}]");
                flatten_json_value(item, &next, out);
            }
        }
        serde_json::Value::String(s) => {
            if !prefix.is_empty() {
                out.push((prefix.to_owned(), s.clone()));
            }
        }
        scalar => {
            if !prefix.is_empty() {
                out.push((
                    prefix.to_owned(),
                    serde_json::to_string(scalar).unwrap_or_default(),
                ));
            }
        }
    }
}

fn extract_multipart_pairs(body: &[u8], boundary: &str) -> Option<Vec<(String, String)>> {
    let body_owned = body.to_vec();
    let boundary_owned = boundary.to_owned();
    futures::executor::block_on(async move {
        let stream = futures::stream::once(async move {
            Ok::<Vec<u8>, std::convert::Infallible>(body_owned)
        });
        let mut multipart = multer::Multipart::new(stream, boundary_owned);
        let mut pairs = Vec::new();
        while let Ok(Some(field)) = multipart.next_field().await {
            let name = field.name().unwrap_or("").to_owned();
            if let Ok(bytes) = field.bytes().await {
                let val = String::from_utf8_lossy(&bytes).into_owned();
                pairs.push((name, val));
            }
        }
        Some(pairs)
    })
}

fn extract_request_body_pairs(raw: &[u8], parsed: &ParsedHttpRequest<'_>) -> Vec<(String, String)> {
    let trimmed_body = parsed.body.trim_ascii();
    if trimmed_body.is_empty() {
        return Vec::new();
    }
    if let Some(ct) = parsed_content_type(parsed) {
        let essence = ct.essence_str();
        if essence.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            return form_urlencoded::parse(trimmed_body)
                .into_owned()
                .collect();
        }
        if essence.eq_ignore_ascii_case("multipart/form-data")
            && let Some(boundary) = ct.get_param(mime::BOUNDARY)
            && let Some(pairs) = extract_multipart_pairs(parsed.body, boundary.as_str())
        {
            return pairs;
        }
        if (essence.eq_ignore_ascii_case("application/json") || essence.ends_with("+json"))
            && let Ok(val) = serde_json::from_slice::<serde_json::Value>(trimmed_body)
        {
            let mut out = Vec::new();
            flatten_json_value(&val, "", &mut out);
            return out;
        }
    }
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(trimmed_body)
        && matches!(val, serde_json::Value::Object(_) | serde_json::Value::Array(_))
    {
        let mut out = Vec::new();
        flatten_json_value(&val, "", &mut out);
        return out;
    }
    let inspected = mio_core::inspect_http_messages(raw, None);
    if !inspected.body_params.is_empty() {
        return inspected
            .body_params
            .into_iter()
            .map(|item| (item.name, item.value))
            .collect();
    }
    form_urlencoded::parse(trimmed_body).into_owned().collect()
}

fn detect_current_body_encoding(parsed: &ParsedHttpRequest<'_>) -> DetectedBodyEncoding {
    if let Some(ct) = parsed_content_type(parsed) {
        let essence = ct.essence_str();
        if essence.eq_ignore_ascii_case("multipart/form-data") {
            return DetectedBodyEncoding::MultipartFormData;
        }
        if essence.eq_ignore_ascii_case("application/json") || essence.ends_with("+json") {
            return DetectedBodyEncoding::ApplicationJson;
        }
        if essence.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            return DetectedBodyEncoding::FormUrlEncoded;
        }
    }
    let trimmed = parsed.body.trim_ascii_start();
    if serde_json::from_slice::<serde_json::Value>(trimmed).is_ok()
        && (trimmed.starts_with(b"{") || trimmed.starts_with(b"["))
    {
        DetectedBodyEncoding::ApplicationJson
    } else if trimmed.starts_with(b"--") {
        DetectedBodyEncoding::MultipartFormData
    } else {
        DetectedBodyEncoding::FormUrlEncoded
    }
}

fn format_header_line(h: &httparse::Header<'_>) -> String {
    format!("{}: {}", h.name, String::from_utf8_lossy(h.value).trim())
}

pub(crate) fn toggle_http_request_method_bytes(
    raw: &[u8],
) -> Result<(Vec<u8>, &'static str), String> {
    let parsed = parse_http_request_parts(raw)?;
    if parsed.method.eq_ignore_ascii_case("GET") || parsed.method.eq_ignore_ascii_case("OPTIONS") {
        set_http_request_method_bytes(raw, "POST")
    } else {
        set_http_request_method_bytes(raw, "GET")
    }
}

pub(crate) fn set_http_request_method_bytes(
    raw: &[u8],
    desired_method: &'static str,
) -> Result<(Vec<u8>, &'static str), String> {
    let parsed = parse_http_request_parts(raw)?;
    if desired_method.eq_ignore_ascii_case("GET") || desired_method.eq_ignore_ascii_case("OPTIONS")
    {
        let pairs = extract_request_body_pairs(raw, &parsed);
        let query_suffix = if !pairs.is_empty() {
            pairs
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        percent_encode_component(k.as_bytes()),
                        percent_encode_component(v.as_bytes())
                    )
                })
                .collect::<Vec<_>>()
                .join("&")
        } else {
            let trimmed = parsed.body.trim_ascii();
            if trimmed.is_empty() {
                String::new()
            } else if let Ok(text) = std::str::from_utf8(trimmed)
                && !text.contains(['\r', '\n', ' '])
            {
                text.to_owned()
            } else {
                format!("body={}", percent_encode_component(trimmed))
            }
        };
        let next_target = if query_suffix.is_empty() {
            parsed.target.to_owned()
        } else if parsed.target.contains('?') {
            if parsed.target.ends_with(['?', '&']) {
                format!("{}{query_suffix}", parsed.target)
            } else {
                format!("{}&{query_suffix}", parsed.target)
            }
        } else {
            format!("{}?{query_suffix}", parsed.target)
        };
        let mut headers = Vec::new();
        for h in &parsed.headers {
            if h.name.eq_ignore_ascii_case("content-length")
                || h.name.eq_ignore_ascii_case("content-type")
            {
                continue;
            }
            headers.push(format_header_line(h));
        }
        let header_block = if headers.is_empty() {
            String::new()
        } else {
            format!("{}\r\n", headers.join("\r\n"))
        };
        let out = format!(
            "{desired_method} {next_target} {}\r\n{header_block}\r\n",
            parsed.version
        );
        return Ok((out.into_bytes(), desired_method));
    }

    if parsed.method.eq_ignore_ascii_case("GET") || parsed.method.eq_ignore_ascii_case("OPTIONS") {
        let (base_target, query) = match parsed.target.split_once('?') {
            Some((path, q)) => (if path.is_empty() { "/" } else { path }, q),
            None => (parsed.target, ""),
        };
        let body_str = String::from_utf8_lossy(parsed.body);
        let trimmed_body = body_str.trim_matches(['\r', '\n']);
        let next_body = match (!query.is_empty(), !trimmed_body.is_empty()) {
            (true, true) => format!("{query}&{trimmed_body}"),
            (true, false) => query.to_owned(),
            (false, _) => trimmed_body.to_owned(),
        };
        let mut headers = Vec::new();
        let mut has_content_type = false;
        for h in &parsed.headers {
            if h.name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            if h.name.eq_ignore_ascii_case("content-type") {
                has_content_type = true;
            }
            headers.push(format_header_line(h));
        }
        if !has_content_type {
            headers.push("Content-Type: application/x-www-form-urlencoded".into());
        }
        headers.push(format!("Content-Length: {}", next_body.len()));
        let out = format!(
            "{desired_method} {base_target} {}\r\n{}\r\n\r\n{next_body}",
            parsed.version,
            headers.join("\r\n")
        );
        return Ok((out.into_bytes(), desired_method));
    }

    let mut headers = Vec::new();
    for h in &parsed.headers {
        if h.name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        headers.push(format_header_line(h));
    }
    headers.push(format!("Content-Length: {}", parsed.body.len()));
    let mut out = format!(
        "{desired_method} {} {}\r\n{}\r\n\r\n",
        parsed.target,
        parsed.version,
        headers.join("\r\n")
    )
    .into_bytes();
    out.extend_from_slice(parsed.body);
    Ok((out, desired_method))
}

pub(crate) fn change_http_body_encoding_bytes(
    raw: &[u8],
    target: BodyEncodingTarget,
) -> Result<(Vec<u8>, &'static str), String> {
    let parsed = parse_http_request_parts(raw)?;
    let current = detect_current_body_encoding(&parsed);
    let desired = match target {
        BodyEncodingTarget::Toggle => match current {
            DetectedBodyEncoding::FormUrlEncoded => DetectedBodyEncoding::MultipartFormData,
            DetectedBodyEncoding::MultipartFormData => DetectedBodyEncoding::ApplicationJson,
            DetectedBodyEncoding::ApplicationJson => DetectedBodyEncoding::FormUrlEncoded,
        },
        BodyEncodingTarget::FormUrlEncoded => DetectedBodyEncoding::FormUrlEncoded,
        BodyEncodingTarget::MultipartFormData => DetectedBodyEncoding::MultipartFormData,
        BodyEncodingTarget::ApplicationJson => DetectedBodyEncoding::ApplicationJson,
    };
    let pairs = extract_request_body_pairs(raw, &parsed);
    let next_method =
        if parsed.method.eq_ignore_ascii_case("GET") || parsed.method.eq_ignore_ascii_case("HEAD") {
            "POST"
        } else {
            parsed.method
        };
    let (content_type_value, next_body, label) = match desired {
        DetectedBodyEncoding::FormUrlEncoded => {
            let mut serializer = form_urlencoded::Serializer::new(String::new());
            for (k, v) in &pairs {
                serializer.append_pair(k, v);
            }
            let encoded = serializer.finish().replace('+', "%20");
            (
                "application/x-www-form-urlencoded".to_owned(),
                encoded.into_bytes(),
                "application/x-www-form-urlencoded",
            )
        }
        DetectedBodyEncoding::MultipartFormData => {
            let boundary = choose_multipart_boundary(&pairs);
            let mut body_buf = Vec::new();
            for (k, v) in &pairs {
                let safe_name = k.replace(['"', '\r', '\n'], "_");
                body_buf.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                body_buf.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{safe_name}\"\r\n\r\n")
                        .as_bytes(),
                );
                body_buf.extend_from_slice(v.as_bytes());
                body_buf.extend_from_slice(b"\r\n");
            }
            body_buf.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            (
                format!("multipart/form-data; boundary={boundary}"),
                body_buf,
                "multipart/form-data",
            )
        }
        DetectedBodyEncoding::ApplicationJson => {
            let seed =
                b"POST / HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
                    .to_vec();
            let mut built = seed;
            let mut used_inspector = !pairs.is_empty();
            for (k, v) in &pairs {
                let mut_res = mio_core::apply_inspector_mutation(
                    &built,
                    mio_core::InspectorMutation::Add {
                        section: mio_core::InspectorSection::BodyParams,
                        name: k.clone(),
                        value: v.clone(),
                    },
                );
                match mut_res {
                    Ok(updated) => built = updated,
                    Err(_) => {
                        used_inspector = false;
                        break;
                    }
                }
            }
            let json_bytes = if pairs.is_empty() {
                b"{}".to_vec()
            } else if used_inspector {
                parse_http_request_parts(&built)
                    .map(|p| p.body.to_vec())
                    .unwrap_or_else(|_| build_flat_json_object(&pairs))
            } else {
                build_flat_json_object(&pairs)
            };
            (
                "application/json".to_owned(),
                json_bytes,
                "application/json",
            )
        }
    };

    let mut headers = Vec::new();
    for h in &parsed.headers {
        if h.name.eq_ignore_ascii_case("content-type")
            || h.name.eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        headers.push(format_header_line(h));
    }
    headers.push(format!("Content-Type: {content_type_value}"));
    headers.push(format!("Content-Length: {}", next_body.len()));
    let mut out = format!(
        "{next_method} {} {}\r\n{}\r\n\r\n",
        parsed.target,
        parsed.version,
        headers.join("\r\n")
    )
    .into_bytes();
    out.extend_from_slice(&next_body);
    Ok((out, label))
}

fn choose_multipart_boundary(pairs: &[(String, String)]) -> String {
    let mut suffix = 1u32;
    loop {
        let candidate = if suffix == 1 {
            "----MioFormBoundary7MA4YWxkTrZu0gW".to_owned()
        } else {
            format!("----MioFormBoundary7MA4YWxkTrZu0gW{suffix}")
        };
        let collision = pairs
            .iter()
            .any(|(k, v)| k.contains(&candidate) || v.contains(&candidate));
        if !collision {
            return candidate;
        }
        suffix += 1;
    }
}

fn parse_scalar_or_string(val: &str) -> serde_json::Value {
    let trimmed = val.trim();
    if (matches!(trimmed, "true" | "false" | "null")
        || serde_json::from_str::<serde_json::Number>(trimmed).is_ok())
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        return parsed;
    }
    serde_json::Value::String(val.to_owned())
}

fn build_flat_json_object(pairs: &[(String, String)]) -> Vec<u8> {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert(k.clone(), parse_scalar_or_string(v));
    }
    serde_json::to_vec(&serde_json::Value::Object(map)).unwrap_or_else(|_| b"{}".to_vec())
}

pub(super) fn parse_clipboard_url_as_request(clipboard: &str) -> Result<(String, Vec<u8>), String> {
    let trimmed = clipboard.trim();
    if trimmed.is_empty() {
        return Err("剪贴板中没有可用的 URL 文本。".into());
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("剪贴板内容包含空白或控制字符，不是有效的 HTTP(S) URL。".into());
    }
    let Some((raw_scheme, after_scheme)) = trimmed.split_once("://") else {
        return Err("剪贴板内容必须以 http:// 或 https:// 开头。".into());
    };
    let scheme = raw_scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("仅支持粘贴 http:// 或 https:// URL 为请求。".into());
    }
    if after_scheme.contains('\\') {
        return Err("URL 主机部分无效或包含不支持的凭据字符。".into());
    }
    let parsed_url =
        url::Url::parse(trimmed).map_err(|err| format!("剪贴板 URL 无效：{err}"))?;
    if !parsed_url.username().is_empty() || parsed_url.password().is_some() {
        return Err("URL 主机部分无效或包含不支持的凭据字符。".into());
    }
    let host = parsed_url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "URL 主机部分无效。".to_owned())?;
    let authority = match parsed_url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let raw_path = parsed_url.path();
    if raw_path.starts_with("//") {
        return Err("URL 路径不能以 // 开头。".into());
    }
    let path_and_query = match parsed_url.query() {
        Some(q) => format!("{raw_path}?{q}"),
        None => raw_path.to_owned(),
    };
    let origin = format!("{scheme}://{authority}");
    let raw = format!("GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\n\r\n").into_bytes();
    mio_core::parse_request(&raw, &origin, HttpVersion::Http11)
        .map_err(|error| format!("剪贴板 URL 无效：{error}"))?;
    Ok((origin, raw))
}
