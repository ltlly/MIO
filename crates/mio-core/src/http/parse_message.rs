use super::*;

pub fn absolute_request_target(url: &str) -> Option<String> {
    let (_, remainder) = url.split_once("://")?;
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    // WHATWG accepts a backslash as a path boundary; the wire URI does not.
    if authority_end == 0 || remainder[..authority_end].contains('\\') {
        return None;
    }
    let path = remainder[authority_end..].split('#').next()?;
    Some(if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    })
}

pub fn http_history_path_and_query(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with('/') {
        return raw.split('#').next().unwrap_or(raw).to_owned();
    }
    Url::parse(raw)
        .ok()
        .and_then(|_| absolute_request_target(raw))
        .unwrap_or_else(|| raw.split('#').next().unwrap_or(raw).to_owned())
}

/// Validates an imported connection origin without interpreting the request's Host header.
pub fn validate_target_origin(target: &str) -> Result<()> {
    let (scheme, authority) = target
        .split_once("://")
        .context("归档连接目标必须为 HTTP 或 HTTPS origin")?;
    ensure!(
        target.len() <= 4096
            && matches!(scheme, "http" | "https")
            && !authority.is_empty()
            && !authority.contains(['/', '\\', '?', '#', '@', '%'])
            && !authority
                .chars()
                .any(|c| c.is_whitespace() || c.is_control()),
        "归档连接目标只能包含协议、主机和显式端口"
    );
    let (host, port) = authority.rsplit_once(':').context("归档连接目标缺少端口")?;
    ensure!(
        !port.is_empty()
            && port.bytes().all(|c| c.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port > 0),
        "归档连接目标端口必须为 1–65535"
    );
    let parsed = Url::parse(target).context("归档连接目标的主机或端口无效")?;
    if matches!(parsed.host(), Some(url::Host::Ipv4(_))) {
        ensure!(
            host.parse::<std::net::Ipv4Addr>().is_ok(),
            "归档目标包含会被重新解释的非标准 IPv4 地址"
        );
    }
    ensure!(
        parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "归档连接目标的主机无效"
    );
    Ok(())
}

pub fn parse_request(raw: &[u8], target: &str, version: HttpVersion) -> Result<HttpRequest> {
    let (first, fields, rest) = head(raw)?;
    let first = std::str::from_utf8(&first).context("请求行必须为 UTF-8")?;
    let parts = first.split_ascii_whitespace().collect::<Vec<_>>();
    ensure!(
        parts.len() == 3 && token(parts[0].as_bytes()),
        "请求行应为 METHOD /path HTTP/1.1"
    );
    ensure!(
        ["HTTP/1.0", "HTTP/1.1", "HTTP/2", "HTTP/2.0"].contains(&parts[2]),
        "不支持的 HTTP 版本"
    );
    let mut base = Url::parse(target).context("连接目标必须为完整 http:// 或 https:// URL")?;
    ensure!(
        matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
        "连接目标必须使用 HTTP 或 HTTPS"
    );
    ensure!(
        base.username().is_empty() && base.password().is_none(),
        "连接目标不能包含凭据；请使用请求 header"
    );
    base.set_fragment(None);
    let url = if parts[1].starts_with('/') {
        ensure!(!parts[1].starts_with("//"), "请求路径不能更换连接目标");
        base.join(parts[1])?
    } else if parts[1].starts_with("http://") || parts[1].starts_with("https://") {
        Url::parse(parts[1])?
    } else {
        bail!("当前发送路径需要 origin-form 路径或绝对 URL");
    };
    ensure!(url.origin() == base.origin(), "请求行与连接目标不一致");
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "请求路径不能包含凭据；请使用请求 header"
    );
    ensure!(url.fragment().is_none(), "请求路径不能包含 URL fragment");
    // WHATWG parsing validates the authority but must not rewrite manual dot segments.
    let request_target = if parts[1].starts_with('/') {
        parts[1].to_owned()
    } else {
        absolute_request_target(parts[1]).context("绝对 URL authority 为空或含反斜线")?
    };
    let (body, trailers) = body(&fields, rest)?;
    Ok(HttpRequest {
        method: parts[0].to_owned(),
        url: format!("{}{request_target}", &base[..Position::BeforePath]),
        version,
        headers: fields,
        body,
        trailers,
    })
}

pub fn parse_repeater_request(
    raw: &[u8],
    target: &str,
    version: HttpVersion,
    update_content_length: bool,
) -> Result<HttpRequest> {
    let mut request = parse_request(raw, target, version)?;
    if update_content_length && request.trailers.is_empty() && !is_chunked(&request.headers) {
        let value = request.body.len().to_string().into_bytes();
        let mut updated = false;
        request.headers.retain_mut(|header| {
            if !header.name.eq_ignore_ascii_case("content-length") {
                return true;
            }
            if updated {
                return false;
            }
            header.value = value.clone();
            updated = true;
            true
        });
        if !updated && !request.body.is_empty() {
            request.headers.push(Header {
                name: "Content-Length".into(),
                value,
            });
        }
    }
    Ok(request)
}

pub fn copy_url_for_request(raw: &[u8], target: &str, version: HttpVersion) -> Result<String> {
    let request = parse_repeater_request(raw, target, version, false)?;
    ensure!(
        (request.url.starts_with("http://") || request.url.starts_with("https://"))
            && !request.url.chars().any(char::is_control),
        "请求 URL 必须为 http:// 或 https:// URL 且不含控制字符"
    );
    let parsed = Url::parse(&request.url).context("请求 URL 无效")?;
    ensure!(parsed.host_str().is_some(), "请求 URL 缺少主机");
    Ok(request.url)
}

fn bash_dollar_quote(bytes: &[u8]) -> String {
    let mut quoted = String::with_capacity(bytes.len() + 3);
    quoted.push_str("$'");
    for &byte in bytes {
        match byte {
            b'\\' => quoted.push_str(r"\\"),
            b'\'' => quoted.push_str(r"\'"),
            b'\r' => quoted.push_str(r"\r"),
            b'\n' => quoted.push_str(r"\n"),
            b'\t' => quoted.push_str(r"\t"),
            0x20..=0x7e => quoted.push(byte as char),
            _ => quoted.push_str(&format!(r"\x{byte:02x}")),
        }
    }
    quoted.push('\'');
    quoted
}

fn is_sensitive_curl_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("proxy-authorization")
        || name.eq_ignore_ascii_case("cookie")
        || name.eq_ignore_ascii_case("x-api-key")
}

pub fn curl_command_for_request_with_options(
    raw: &[u8],
    target: &str,
    version: HttpVersion,
    redact_sensitive_headers: bool,
) -> Result<String> {
    let request = parse_repeater_request(raw, target, version, false)?;
    let mut command = String::from("curl --path-as-is -i -s -k");
    match version {
        HttpVersion::Http10 => command.push_str(" --http1.0"),
        HttpVersion::Http11 => {}
        HttpVersion::Http2 => command.push_str(" --http2"),
    }
    command.push_str(" -X ");
    command.push_str(&bash_dollar_quote(request.method.as_bytes()));
    for header in &request.headers {
        if header.name.starts_with(':')
            || header.name.eq_ignore_ascii_case("content-length")
            || header.name.eq_ignore_ascii_case("transfer-encoding")
        {
            continue;
        }
        let value: &[u8] = if redact_sensitive_headers && is_sensitive_curl_header(&header.name) {
            b"<REDACTED>"
        } else {
            &header.value
        };
        let mut line = Vec::with_capacity(header.name.len() + 2 + value.len());
        line.extend_from_slice(header.name.as_bytes());
        line.extend_from_slice(b": ");
        line.extend_from_slice(value);
        command.push_str(" -H ");
        command.push_str(&bash_dollar_quote(&line));
    }
    if !request.body.is_empty() {
        command.push_str(" --data-binary ");
        command.push_str(&bash_dollar_quote(&request.body));
    }
    command.push(' ');
    command.push_str(&bash_dollar_quote(request.url.as_bytes()));
    Ok(command)
}

pub fn curl_command_for_request(raw: &[u8], target: &str, version: HttpVersion) -> Result<String> {
    curl_command_for_request_with_options(raw, target, version, false)
}

fn parse_redirect_url(candidate: &str) -> Option<Url> {
    if candidate.is_empty()
        || candidate.contains('\\')
        || candidate
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return None;
    }
    let (raw_scheme, remainder) = candidate.split_once("://")?;
    let scheme = raw_scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return None;
    }
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.is_empty() || authority.contains(['@', '%']) {
        return None;
    }
    let parsed = Url::parse(candidate).ok()?;
    let port = parsed.port_or_known_default()?;
    validate_target_origin(&format!(
        "{scheme}://{authority_with_port}",
        authority_with_port = if parsed.port().is_some() {
            authority.to_owned()
        } else {
            let host = authority
                .strip_suffix(":80")
                .filter(|_| scheme == "http")
                .or_else(|| authority.strip_suffix(":443").filter(|_| scheme == "https"))
                .unwrap_or(authority);
            format!("{host}:{port}")
        }
    ))
    .ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    Some(parsed)
}

fn resolve_raw_redirect_target(current_url: &str, raw_target: &str) -> Option<String> {
    let location = raw_target.trim().trim_matches(['"', '\'']);
    if location.is_empty()
        || location.contains('\\')
        || location
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return None;
    }
    let base = parse_redirect_url(current_url)?;
    let is_absolute = location
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || location
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let mut resolved = if is_absolute {
        parse_redirect_url(location)?
    } else if location.starts_with("//") {
        parse_redirect_url(&format!("{}:{location}", base.scheme()))?
    } else {
        if let Some((prefix, _)) = location.split_once(':')
            && !prefix.contains(['/', '?', '#'])
        {
            return None;
        }
        let joined = base.join(location).ok()?;
        parse_redirect_url(joined.as_str())?
    };
    resolved.set_fragment(None);
    Some(resolved.into())
}

fn extract_location_header(response: &HttpResponse) -> Option<&str> {
    let mut location_value = None;
    for header in &response.headers {
        if !header.name.eq_ignore_ascii_case("location") {
            continue;
        }
        let text = std::str::from_utf8(&header.value).ok()?;
        if text.chars().any(char::is_control) {
            return None;
        }
        if location_value.is_some_and(|previous| previous != text) {
            return None;
        }
        location_value = Some(text);
    }
    location_value
}

fn extract_refresh_url(value: &str) -> Option<&str> {
    for part in value.split(';').skip(1) {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=')
            && k.trim().eq_ignore_ascii_case("url")
        {
            let target = v.trim().trim_matches(['"', '\'']);
            if !target.is_empty() {
                return Some(target);
            }
        }
    }
    None
}

fn extract_meta_refresh_url(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(&body[..body.len().min(64 * 1024)]).ok()?;
    let lower = text.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(pos) = lower[cursor..].find("<meta") {
        let start = cursor + pos;
        let end = lower[start..].find('>')? + start;
        let tag = &text[start..=end];
        let tag_lower = &lower[start..=end];
        if tag_lower.contains("http-equiv") && tag_lower.contains("refresh") {
            if let Some(c_pos) = tag_lower.find("content") {
                let after = &tag[c_pos + "content".len()..];
                if let Some(eq) = after.find('=') {
                    let val = after[eq + 1..].trim_start();
                    let quoted = if let Some(rest) = val.strip_prefix('"') {
                        rest.split('"').next()
                    } else if let Some(rest) = val.strip_prefix('\'') {
                        rest.split('\'').next()
                    } else {
                        val.split_whitespace().next()
                    };
                    if let Some(content) = quoted
                        && let Some(u) = extract_refresh_url(content)
                    {
                        return Some(u.to_owned());
                    }
                }
            }
        }
        cursor = end + 1;
    }
    None
}

fn extract_js_redirect_url(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(&body[..body.len().min(64 * 1024)]).ok()?;
    for needle in [
        "window.location.href",
        "window.location.replace(",
        "window.location.assign(",
        "document.location.href",
        "window.location",
        "location.href",
        "location.replace(",
    ] {
        if let Some(idx) = text.find(needle) {
            let tail = &text[idx + needle.len()..];
            let trimmed = tail.trim_start_matches([' ', '\t', '=', '(']);
            let extracted = if let Some(rest) = trimmed.strip_prefix('"') {
                rest.split('"').next()
            } else if let Some(rest) = trimmed.strip_prefix('\'') {
                rest.split('\'').next()
            } else {
                None
            };
            if let Some(target) = extracted
                && !target.trim().is_empty()
            {
                return Some(target.trim().to_owned());
            }
        }
    }
    None
}

pub fn redirect_location_url(current_url: &str, response: &HttpResponse) -> Option<String> {
    if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = extract_location_header(response)?;
    resolve_raw_redirect_target(current_url, location)
}

pub fn redirect_location_url_with_settings(
    current_url: &str,
    response: &HttpResponse,
    http_settings: &crate::NetworkHttpSettings,
) -> Option<String> {
    if http_settings.redirect_3xx && matches!(response.status, 300..=399) {
        if let Some(loc) = extract_location_header(response)
            && let Some(resolved) = resolve_raw_redirect_target(current_url, loc)
        {
            return Some(resolved);
        }
    }
    if http_settings.redirect_any_location
        && let Some(loc) = extract_location_header(response)
        && let Some(resolved) = resolve_raw_redirect_target(current_url, loc)
    {
        return Some(resolved);
    }
    if http_settings.redirect_refresh_header {
        for h in &response.headers {
            if h.name.eq_ignore_ascii_case("refresh")
                && let Ok(val) = std::str::from_utf8(&h.value)
                && let Some(target) = extract_refresh_url(val)
                && let Some(resolved) = resolve_raw_redirect_target(current_url, target)
            {
                return Some(resolved);
            }
        }
    }
    if http_settings.redirect_meta_refresh
        && let Some(target) = extract_meta_refresh_url(&response.body)
        && let Some(resolved) = resolve_raw_redirect_target(current_url, &target)
    {
        return Some(resolved);
    }
    if http_settings.redirect_javascript
        && let Some(target) = extract_js_redirect_url(&response.body)
        && let Some(resolved) = resolve_raw_redirect_target(current_url, &target)
    {
        return Some(resolved);
    }
    None
}

pub fn resolve_redirect_request(
    current_origin: &str,
    current_request: &HttpRequest,
    response: &HttpResponse,
    mode: crate::RepeaterRedirectMode,
) -> Result<Option<(String, HttpRequest, Vec<u8>)>> {
    resolve_redirect_request_with_options(
        current_origin,
        current_request,
        response,
        mode,
        &crate::NetworkHttpSettings::default(),
        false,
        false,
    )
}

pub fn resolve_redirect_request_with_options(
    current_origin: &str,
    current_request: &HttpRequest,
    response: &HttpResponse,
    mode: crate::RepeaterRedirectMode,
    http_settings: &crate::NetworkHttpSettings,
    process_cookies: bool,
    cross_domain_protocol_redirects: bool,
) -> Result<Option<(String, HttpRequest, Vec<u8>)>> {
    if mode == crate::RepeaterRedirectMode::Never {
        return Ok(None);
    }
    let Some(next_url) =
        redirect_location_url_with_settings(&current_request.url, response, http_settings)
    else {
        return Ok(None);
    };
    let current = parse_redirect_url(current_origin).context("当前连接 origin 无效")?;
    let next = parse_redirect_url(&next_url).context("重定向目标 URL 无效")?;
    let same_origin = current.origin() == next.origin();
    let same_host = current
        .host_str()
        .map(str::to_ascii_lowercase)
        == next.host_str().map(str::to_ascii_lowercase);
    if matches!(
        mode,
        crate::RepeaterRedirectMode::OnSiteOnly | crate::RepeaterRedirectMode::InScopeOnly
    ) && !same_origin
        && !(cross_domain_protocol_redirects && same_host)
    {
        return Ok(None);
    }
    if !same_origin
        && !cross_domain_protocol_redirects
        && !current.scheme().eq_ignore_ascii_case(next.scheme())
        && mode == crate::RepeaterRedirectMode::OnSiteOnly
    {
        return Ok(None);
    }
    let next_origin = format!(
        "{}://{}:{}",
        next.scheme(),
        next.host_str().context("重定向目标缺少主机")?,
        next.port_or_known_default().context("重定向目标缺少端口")?
    );
    validate_target_origin(&next_origin)?;
    let mut next_request = current_request.clone();
    next_request.url = next_url;
    if (matches!(response.status, 301..=303) || !(300..=399).contains(&response.status))
        && !matches!(current_request.method.as_str(), "GET" | "HEAD")
    {
        next_request.method = "GET".into();
        next_request.body.clear();
        next_request.trailers.clear();
        next_request.headers.retain(|header| {
            ![
                "content-length",
                "content-type",
                "transfer-encoding",
                "trailer",
            ]
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name))
        });
    }
    if !same_origin {
        next_request.headers.retain(|header| {
            !["authorization", "proxy-authorization", "cookie"]
                .iter()
                .any(|name| header.name.eq_ignore_ascii_case(name))
        });
    }
    if process_cookies {
        let mut jar_pairs: Vec<(String, String)> = Vec::new();
        for h in &next_request.headers {
            if h.name.eq_ignore_ascii_case("cookie")
                && let Ok(s) = std::str::from_utf8(&h.value)
            {
                for part in s.split(';') {
                    if let Some((k, v)) = part.split_once('=') {
                        let k = k.trim();
                        if !k.is_empty() {
                            jar_pairs.push((k.to_owned(), v.trim().to_owned()));
                        }
                    }
                }
            }
        }
        let mut added_cookie = false;
        for h in &response.headers {
            if h.name.eq_ignore_ascii_case("set-cookie")
                && let Ok(s) = std::str::from_utf8(&h.value)
                && let Some(first) = s.split(';').next()
                && let Some((k, v)) = first.split_once('=')
            {
                let k = k.trim();
                let v = v.trim();
                if !k.is_empty() {
                    if let Some(existing) = jar_pairs.iter_mut().find(|(ek, _)| ek == k) {
                        existing.1 = v.to_owned();
                    } else {
                        jar_pairs.push((k.to_owned(), v.to_owned()));
                    }
                    added_cookie = true;
                }
            }
        }
        if added_cookie && !jar_pairs.is_empty() {
            let cookie_val = jar_pairs
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            next_request
                .headers
                .retain(|h| !h.name.eq_ignore_ascii_case("cookie"));
            next_request.headers.push(Header {
                name: "Cookie".into(),
                value: cookie_val.into_bytes(),
            });
        }
    }
    let authority = next_origin
        .split_once("://")
        .context("重定向 origin 无效")?
        .1;
    let mut host_updated = false;
    next_request.headers.retain_mut(|header| {
        if !header.name.eq_ignore_ascii_case("host") {
            return true;
        }
        if host_updated {
            return false;
        }
        header.value = authority.as_bytes().to_vec();
        host_updated = true;
        true
    });
    if !host_updated {
        next_request.headers.insert(
            0,
            Header {
                name: "Host".into(),
                value: authority.as_bytes().to_vec(),
            },
        );
    }
    if next.scheme() == "http" && next_request.version == HttpVersion::Http2 {
        next_request.version = HttpVersion::Http11;
    }
    let next_raw = request_to_raw(&next_request);
    Ok(Some((next_origin, next_request, next_raw)))
}

pub fn parse_response(raw: &[u8], version: HttpVersion) -> Result<HttpResponse> {
    let (first, fields, rest) = head(raw)?;
    let first = std::str::from_utf8(&first).context("响应状态行无效")?;
    let mut parts = first.splitn(3, ' ');
    ensure!(
        matches!(
            parts.next(),
            Some("HTTP/1.0" | "HTTP/1.1" | "HTTP/2" | "HTTP/2.0")
        ),
        "响应 HTTP 版本无效"
    );
    let status: u16 = parts.next().context("缺少响应状态码")?.parse()?;
    ensure!((100..=599).contains(&status), "响应状态码无效");
    let reason_phrase = parts
        .next()
        .map(str::trim_ascii)
        .filter(|r| !r.is_empty())
        .map(|r| r.as_bytes().to_vec());
    let (body, trailers) = match body(&fields, rest) {
        Ok(parsed) => parsed,
        Err(err) if is_chunked(&fields) => {
            let first_line_end = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
            let first_line = rest[..first_line_end]
                .strip_suffix(b"\r")
                .unwrap_or(&rest[..first_line_end]);
            let size_candidate = first_line.split(|c| *c == b';').next().unwrap_or_default();
            let looks_like_chunk_header = first_line_end < rest.len()
                && !size_candidate.is_empty()
                && size_candidate.len() <= 16
                && size_candidate.iter().all(u8::is_ascii_hexdigit);
            if looks_like_chunk_header {
                return Err(err);
            }
            ensure!(rest.len() <= MAX_BODY, "可编辑正文超过 16 MiB 限额");
            (rest.to_vec(), Vec::new())
        }
        Err(err) => return Err(err),
    };
    Ok(HttpResponse {
        status,
        version,
        reason_phrase,
        headers: fields,
        body,
        trailers,
    })
}

fn normalize_origin_candidate(raw: &str, default_tls: bool) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        let scheme = if default_tls || trimmed.ends_with(":443") {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{trimmed}")
    };
    let parsed = Url::parse(&candidate).ok()?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default()?;
    let origin = format!("{scheme}://{host}:{port}");
    validate_target_origin(&origin).ok()?;
    Some(origin)
}

fn extract_host_header_origin(headers: &[Header], default_tls: bool) -> Option<String> {
    for name in ["host", ":authority"] {
        if let Some(header) = headers.iter().find(|h| h.name.eq_ignore_ascii_case(name))
            && let Ok(text) = std::str::from_utf8(&header.value)
            && let Some(origin) = normalize_origin_candidate(text, default_tls)
        {
            return Some(origin);
        }
    }
    None
}

fn sanitize_request_path(raw_target: &str) -> String {
    let without_fragment = raw_target.split('#').next().unwrap_or("/");
    if without_fragment == "*" {
        return "/*".into();
    }
    let path_source = if without_fragment.starts_with("http://")
        || without_fragment.starts_with("https://")
    {
        Url::parse(without_fragment)
            .ok()
            .and_then(|_| absolute_request_target(without_fragment))
            .unwrap_or_else(|| "/".into())
    } else if without_fragment.starts_with('/') {
        if without_fragment.starts_with("//") {
            format!("/{}", without_fragment.trim_start_matches('/'))
        } else {
            without_fragment.to_owned()
        }
    } else {
        "/".into()
    };
    let mut out = String::with_capacity(path_source.len().max(1));
    for byte in path_source.bytes() {
        match byte {
            0x21..=0x7e if byte != b'\\' && byte != b'#' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    if out.is_empty() || !out.starts_with('/') {
        format!("/{out}")
    } else {
        out
    }
}

fn decode_imported_body(mut headers: Vec<Header>, raw_body: &[u8]) -> (Vec<Header>, Vec<u8>, Vec<Header>) {
    if is_chunked(&headers)
        && let Ok((decoded, trailers)) = body(&headers, raw_body)
    {
        return (headers, decoded, trailers);
    }
    headers.retain(|h| !h.name.eq_ignore_ascii_case("transfer-encoding"));
    let truncated = &raw_body[..raw_body.len().min(MAX_BODY)];
    (headers, truncated.to_vec(), Vec::new())
}

fn filter_imported_headers<'a>(
    parsed: impl Iterator<Item = (&'a str, &'a [u8])>,
) -> Vec<Header> {
    let mut result = Vec::new();
    for (name, raw_val) in parsed {
        if result.len() >= 1024 {
            break;
        }
        let trimmed_name = name.trim();
        let name_ok = token(trimmed_name.as_bytes())
            || (trimmed_name.starts_with(':') && token(&trimmed_name.as_bytes()[1..]));
        if !name_ok {
            continue;
        }
        let value = raw_val.trim_ascii();
        let clean_value: Vec<u8> = if valid_header_value_bytes(value) {
            value.to_vec()
        } else {
            value
                .iter()
                .copied()
                .filter(|b| (*b >= 32 && *b != 127) || *b == b'\t')
                .collect()
        };
        result.push(Header {
            name: trimmed_name.to_owned(),
            value: clean_value,
        });
    }
    result
}

pub(crate) fn parse_imported_request(
    raw: &[u8],
    service_origin: Option<&str>,
) -> Option<(HttpRequest, String)> {
    if raw.is_empty() {
        return None;
    }
    let head_limit = raw.len().min(MAX_HEAD + 4);
    let mut header_slots = [httparse::EMPTY_HEADER; 1024];
    let mut req = httparse::Request::new(&mut header_slots);
    let parsed_httparse = httparse::ParserConfig::default()
        .allow_multiple_spaces_in_request_line_delimiters(true)
        .allow_space_before_first_header_name(true)
        .ignore_invalid_headers_in_requests(true)
        .parse_request(&mut req, &raw[..head_limit]);

    let (raw_method, raw_target, version, parsed_headers, raw_body) = match parsed_httparse {
        Ok(httparse::Status::Complete(body_offset)) => {
            let method = req.method.unwrap_or("GET").to_owned();
            let target = req.path.unwrap_or("/").to_owned();
            let ver = match req.version {
                Some(0) => HttpVersion::Http10,
                Some(2) => HttpVersion::Http2,
                _ => HttpVersion::Http11,
            };
            let hdrs = filter_imported_headers(req.headers.iter().map(|h| (h.name, h.value)));
            (method, target, ver, hdrs, &raw[body_offset..])
        }
        _ => {
            let (head_end, delim_len) = raw[..head_limit]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| (p, 4))
                .into_iter()
                .chain(
                    raw[..head_limit]
                        .windows(2)
                        .position(|w| w == b"\n\n")
                        .map(|p| (p, 2)),
                )
                .min_by_key(|(p, _)| *p)
                .unwrap_or((raw.len().min(MAX_HEAD), 0));
            let head_slice = &raw[..head_end];
            let body_slice = &raw[(head_end + delim_len).min(raw.len())..];
            let mut lines = head_slice
                .split(|b| *b == b'\n')
                .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
            let first_line = lines.next().unwrap_or_default();
            let first_str = String::from_utf8_lossy(first_line);
            let mut parts = first_str.split_ascii_whitespace();
            let m = parts.next().unwrap_or("GET").to_owned();
            let t = parts.next().unwrap_or("/").to_owned();
            let v = match parts.next() {
                Some("HTTP/1.0") => HttpVersion::Http10,
                Some("HTTP/2" | "HTTP/2.0") => HttpVersion::Http2,
                _ => HttpVersion::Http11,
            };
            let raw_pairs: Vec<(String, Vec<u8>)> = lines
                .filter_map(|line| {
                    let colon = line.iter().position(|b| *b == b':')?;
                    let name = String::from_utf8_lossy(&line[..colon]).into_owned();
                    Some((name, line[colon + 1..].to_vec()))
                })
                .collect();
            let hdrs = filter_imported_headers(
                raw_pairs.iter().map(|(n, val)| (n.as_str(), val.as_slice())),
            );
            (m, t, v, hdrs, body_slice)
        }
    };

    let method = if token(raw_method.as_bytes()) {
        raw_method
    } else {
        let cleaned: String = raw_method
            .bytes()
            .filter(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(c))
            .map(|c| c as char)
            .collect();
        if cleaned.is_empty() {
            "GET".into()
        } else {
            cleaned
        }
    };

    let service_valid = service_origin
        .and_then(|s| normalize_origin_candidate(s, s.starts_with("https://")));
    let target_origin = if raw_target.starts_with("http://") || raw_target.starts_with("https://") {
        normalize_origin_candidate(&raw_target, raw_target.starts_with("https://"))
    } else if method.eq_ignore_ascii_case("CONNECT") {
        normalize_origin_candidate(&raw_target, raw_target.ends_with(":443"))
    } else {
        None
    };
    let host_origin = extract_host_header_origin(
        &parsed_headers,
        service_valid
            .as_deref()
            .is_some_and(|o| o.starts_with("https://")),
    );
    let resolved_origin = service_valid
        .or(target_origin)
        .or(host_origin);
    let origin_for_url = resolved_origin
        .clone()
        .unwrap_or_else(|| "http://localhost:80".into());
    let target_for_draft = resolved_origin.unwrap_or_default();
    let path = sanitize_request_path(&raw_target);
    let full_url = format!("{origin_for_url}{path}");
    let verified_url = if Url::parse(&full_url).is_ok() && !full_url.chars().any(char::is_control) {
        full_url
    } else {
        format!("{origin_for_url}/")
    };

    let (headers, body, trailers) = decode_imported_body(parsed_headers, raw_body);
    Some((
        HttpRequest {
            method,
            url: verified_url,
            version,
            headers,
            body,
            trailers,
        },
        target_for_draft,
    ))
}

pub(crate) fn parse_imported_response(raw: &[u8]) -> Option<(HttpResponse, Option<Vec<u8>>)> {
    if raw.is_empty() {
        return None;
    }
    let head_limit = raw.len().min(MAX_HEAD + 4);
    let mut header_slots = [httparse::EMPTY_HEADER; 1024];
    let mut resp = httparse::Response::new(&mut header_slots);
    let parsed_httparse = httparse::ParserConfig::default()
        .allow_spaces_after_header_name_in_responses(true)
        .allow_obsolete_multiline_headers_in_responses(true)
        .allow_multiple_spaces_in_response_status_delimiters(true)
        .allow_space_before_first_header_name(true)
        .ignore_invalid_headers_in_responses(true)
        .parse_response(&mut resp, &raw[..head_limit]);

    let (status, version, reason_phrase, parsed_headers, raw_body) = match parsed_httparse {
        Ok(httparse::Status::Complete(body_offset)) => {
            let code = resp.code.filter(|c| (100..=999).contains(c)).unwrap_or(200);
            let ver = match resp.version {
                Some(0) => HttpVersion::Http10,
                _ => HttpVersion::Http11,
            };
            let reason = resp
                .reason
                .map(str::trim_ascii)
                .filter(|r| !r.is_empty())
                .map(|r| r.as_bytes().to_vec());
            let hdrs = filter_imported_headers(resp.headers.iter().map(|h| (h.name, h.value)));
            (code, ver, reason, hdrs, &raw[body_offset..])
        }
        _ => {
            let (head_end, delim_len) = raw[..head_limit]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| (p, 4))
                .into_iter()
                .chain(
                    raw[..head_limit]
                        .windows(2)
                        .position(|w| w == b"\n\n")
                        .map(|p| (p, 2)),
                )
                .min_by_key(|(p, _)| *p)
                .unwrap_or((raw.len().min(MAX_HEAD), 0));
            let head_slice = &raw[..head_end];
            let body_slice = &raw[(head_end + delim_len).min(raw.len())..];
            let mut lines = head_slice
                .split(|b| *b == b'\n')
                .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
            let first_line = lines.next()?;
            if !first_line.starts_with(b"HTTP/") {
                return None;
            }
            let first_str = String::from_utf8_lossy(first_line);
            let mut parts = first_str.splitn(3, ' ');
            let v = match parts.next() {
                Some("HTTP/1.0") => HttpVersion::Http10,
                Some("HTTP/2" | "HTTP/2.0") => HttpVersion::Http2,
                _ => HttpVersion::Http11,
            };
            let code = parts
                .next()
                .and_then(|s| s.trim().parse::<u16>().ok())
                .filter(|c| (100..=999).contains(c))
                .unwrap_or(200);
            let reason = parts
                .next()
                .map(str::trim_ascii)
                .filter(|r| !r.is_empty() && !r.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)))
                .map(|r| r.as_bytes().to_vec());
            let raw_pairs: Vec<(String, Vec<u8>)> = lines
                .filter_map(|line| {
                    let colon = line.iter().position(|b| *b == b':')?;
                    let name = String::from_utf8_lossy(&line[..colon]).into_owned();
                    Some((name, line[colon + 1..].to_vec()))
                })
                .collect();
            let hdrs = filter_imported_headers(
                raw_pairs.iter().map(|(n, val)| (n.as_str(), val.as_slice())),
            );
            (code, v, reason, hdrs, body_slice)
        }
    };

    let (headers, body, trailers) = decode_imported_body(parsed_headers, raw_body);
    Some((
        HttpResponse {
            status,
            version,
            reason_phrase: reason_phrase.clone(),
            headers,
            body,
            trailers,
        },
        reason_phrase,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_command_redacts_sensitive_headers_only_when_requested() {
        let raw = b"GET /profile HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nAuthorization: Bearer secret123\r\nproxy-authorization: Basic abc\r\nCookie: sid=xyz\r\nX-Api-Key: k999\r\nX-Custom: keep-me\r\n\r\n";
        let unredacted = curl_command_for_request_with_options(
            raw,
            "http://127.0.0.1:8080",
            HttpVersion::Http11,
            false,
        )
        .unwrap();
        assert_eq!(
            unredacted,
            curl_command_for_request(raw, "http://127.0.0.1:8080", HttpVersion::Http11).unwrap()
        );
        assert!(unredacted.contains("Bearer secret123"));
        assert!(unredacted.contains("Basic abc"));
        assert!(unredacted.contains("sid=xyz"));
        assert!(unredacted.contains("k999"));

        let redacted = curl_command_for_request_with_options(
            raw,
            "http://127.0.0.1:8080",
            HttpVersion::Http11,
            true,
        )
        .unwrap();
        assert!(redacted.contains("-H $'Authorization: <REDACTED>'"));
        assert!(redacted.contains("-H $'proxy-authorization: <REDACTED>'"));
        assert!(redacted.contains("-H $'Cookie: <REDACTED>'"));
        assert!(redacted.contains("-H $'X-Api-Key: <REDACTED>'"));
        assert!(redacted.contains("-H $'X-Custom: keep-me'"));
        assert!(!redacted.contains("secret123"));
        assert!(!redacted.contains("sid=xyz"));
        assert!(!redacted.contains("k999"));
    }

    #[test]
    fn multi_token_transfer_encoding_dechunks_imported_requests_and_responses() {
        let raw_req = b"POST /upload HTTP/1.1\r\nHost: example.com\r\nTransfer-Encoding: gzip, chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let (req, target) =
            parse_imported_request(raw_req, Some("https://example.com:443")).unwrap();
        assert_eq!(target, "https://example.com:443");
        assert_eq!(req.body, b"hello world");
        assert!(super::is_chunked(&req.headers));

        let raw_resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n4\r\npong\r\n0\r\n\r\n";
        let (resp, reason) = parse_imported_response(raw_resp).unwrap();
        assert_eq!(reason.as_deref(), Some(b"OK".as_slice()));
        assert_eq!(resp.body, b"pong");
        assert!(super::is_chunked(&resp.headers));
    }
}
