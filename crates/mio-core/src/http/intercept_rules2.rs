use super::*;

pub(super) fn intercept_pairs_match<'a>(
    pairs: impl Iterator<Item = &'a [u8]>,
    matcher: &regex::bytes::Regex,
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    check()?;
    for pair in pairs {
        check()?;
        let matched = matcher.is_match(pair);
        check()?;
        if matched {
            return Ok(true);
        }
    }
    check()?;
    Ok(false)
}

pub(super) fn check_intercept_control(
    cancel: &tokio_util::sync::CancellationToken,
    deadline: std::time::Instant,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(UnpackError::Cancelled.into());
    }
    if std::time::Instant::now() >= deadline {
        return Err(UnpackError::Deadline.into());
    }
    Ok(())
}

pub(super) fn intercept_control_error(error: anyhow::Error) -> mio_proxy::ProxyError {
    match error.downcast_ref::<UnpackError>() {
        Some(UnpackError::Cancelled) => mio_proxy::ProxyError::Cancelled,
        Some(UnpackError::Deadline) => mio_proxy::ProxyError::Timeout,
        _ => mio_proxy::ProxyError::InvalidMessage(format!(
            "Proxy 消息处理或条件拦截无法完成，未自动放行：{error:#}"
        )),
    }
}

pub(super) fn validate_intercept_headers(
    headers: &[Header],
    cancel: &tokio_util::sync::CancellationToken,
    deadline: std::time::Instant,
) -> Result<()> {
    ensure!(
        headers.len() <= 1024,
        "Proxy 拦截 header 超过 1024 个字段限额"
    );
    let mut head_bytes = 0_usize;
    for header in headers {
        check_intercept_control(cancel, deadline)?;
        head_bytes = head_bytes
            .checked_add(header.name.len())
            .and_then(|length| length.checked_add(header.value.len()))
            .and_then(|length| length.checked_add(4))
            .context("Proxy 拦截 header 字节计数溢出")?;
        ensure!(
            head_bytes <= MAX_HEAD,
            "Proxy 拦截 header 超过 128 KiB 限额"
        );
        ensure!(
            token(header.name.as_bytes()) && valid_header_value_bytes(&header.value),
            "Proxy 拦截 header 格式无效，未自动放行"
        );
    }
    Ok(())
}

pub(super) fn intercept_cookie_pairs<'a>(
    headers: &'a [Header],
    in_response: bool,
    cancel: &tokio_util::sync::CancellationToken,
    deadline: std::time::Instant,
) -> Result<Vec<(&'a [u8], &'a [u8])>> {
    validate_intercept_headers(headers, cancel, deadline)?;
    let mut pairs = Vec::new();
    for header in headers {
        let name = if in_response { "set-cookie" } else { "cookie" };
        if !header.name.eq_ignore_ascii_case(name) {
            continue;
        }
        let mut parts = header.value.split(|byte| *byte == b';');
        for pair in &mut parts {
            check_intercept_control(cancel, deadline)?;
            let pair = pair.trim_ascii();
            if !pair.is_empty() {
                ensure!(
                    pairs.len() < 256,
                    crate::intruder::AutomaticParameterLimit(
                        "Proxy Cookie 参数超过 256 个限额，未返回部分匹配"
                    )
                );
                let (name, value) = pair
                    .iter()
                    .position(|byte| *byte == b'=')
                    .map_or((pair, b"".as_slice()), |equal| {
                        (&pair[..equal], &pair[equal + 1..])
                    });
                pairs.push((name.trim_ascii(), value.trim_ascii()));
            }
            if in_response {
                break;
            }
        }
    }
    check_intercept_control(cancel, deadline)?;
    Ok(pairs)
}

pub(super) fn push_intercept_raw_pairs<'a>(
    raw: &'a [u8],
    out: &mut InterceptParameters<'a>,
    cancel: &tokio_util::sync::CancellationToken,
    deadline: std::time::Instant,
) -> Result<()> {
    for pair in raw.split(|byte| *byte == b'&') {
        check_intercept_control(cancel, deadline)?;
        if pair.is_empty() {
            continue;
        }
        ensure!(
            out.len() < crate::intruder::MAX_POSITIONS,
            crate::intruder::AutomaticParameterLimit(
                "Proxy 请求参数超过 256 个限额，未返回部分匹配"
            )
        );
        let (name, value) = pair
            .iter()
            .position(|byte| *byte == b'=')
            .map_or((pair, b"".as_slice()), |equal| {
                (&pair[..equal], &pair[equal + 1..])
            });
        out.push((std::borrow::Cow::Borrowed(name), value));
    }
    check_intercept_control(cancel, deadline)?;
    Ok(())
}

pub(super) fn intercept_request_parameters<'a>(
    request: &'a HttpRequest,
    cancel: &tokio_util::sync::CancellationToken,
    limits: (std::time::Instant, usize),
) -> Result<InterceptParameters<'a>> {
    let (deadline, max_body) = limits;
    check_intercept_control(cancel, deadline)?;
    ensure!(
        request.url.len() <= MAX_HEAD,
        crate::intruder::AutomaticParameterLimit("Proxy 参数条件请求 URL 超过 128 KiB 限额")
    );
    validate_intercept_headers(&request.headers, cancel, deadline)?;
    ensure!(
        request.body.len() <= max_body.min(crate::intruder::MAX_AUTO_BYTES),
        "Proxy 请求正文超过参数条件匹配限额，未截断后匹配"
    );
    let mut pairs = Vec::new();
    let target = request.url.split('#').next().unwrap_or_default();
    if let Some((_, query)) = target.split_once('?') {
        push_intercept_raw_pairs(query.as_bytes(), &mut pairs, cancel, deadline)?;
    }
    if request.body.is_empty() {
        return Ok(pairs);
    }
    let mut content_type_headers = request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-type"));
    let content_type = content_type_headers
        .next()
        .map(|header| crate::intruder::parse_intercept_content_type(&header.value))
        .transpose()?;
    ensure!(
        content_type_headers.next().is_none(),
        "Proxy 请求遇到重复 Content-Type header，未猜测参数格式"
    );
    let compressed = request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|header| header.value.split(|byte| *byte == b','))
        .map(<[u8]>::trim_ascii)
        .any(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case(b"identity"));
    if let Some(content_type) = &content_type {
        if content_type.is_json_or_xml() {
            return Ok(pairs);
        }
        if let Some(boundary) = content_type.multipart_boundary()? {
            ensure!(
                !compressed,
                "Proxy 压缩 multipart 请求正文尚未解包，未按原始压缩字节猜测参数"
            );
            crate::intruder::intercept_multipart_parameters(
                &request.body,
                boundary,
                &mut pairs,
                &|| check_intercept_control(cancel, deadline),
            )?;
            return Ok(pairs);
        }
    }
    if crate::intruder::intercept_body_is_structured(&request.body, &|| {
        check_intercept_control(cancel, deadline)
    })? {
        return Ok(pairs);
    }
    push_intercept_raw_pairs(&request.body, &mut pairs, cancel, deadline)?;
    Ok(pairs)
}

pub(super) fn request_has_intercept_parameters(request: &HttpRequest) -> Result<bool> {
    let url = request.url.split('#').next().unwrap_or_default();
    if let Some((_, query)) = url.split_once('?')
        && crate::intruder::has_http_parameters(
            format!("GET /?{query} HTTP/1.1\r\n\r\n").as_bytes(),
        )?
    {
        return Ok(true);
    }
    let mut raw = b"POST / HTTP/1.1\r\n".to_vec();
    for header in &request.headers {
        if header.name.eq_ignore_ascii_case("content-type")
            || header.name.eq_ignore_ascii_case("content-encoding")
        {
            raw.extend_from_slice(header.name.as_bytes());
            raw.extend_from_slice(b": ");
            raw.extend_from_slice(&header.value);
            raw.extend_from_slice(b"\r\n");
        }
    }
    raw.extend_from_slice(b"\r\n");
    ensure!(
        raw.len() <= MAX_HEAD,
        crate::intruder::AutomaticParameterLimit("Auto § header 超过 128 KiB")
    );
    ensure!(
        request.body.len() <= crate::intruder::MAX_AUTO_BYTES.saturating_sub(raw.len()),
        crate::intruder::AutomaticParameterLimit("Auto § 最多解析 4 MiB（MIO 资源限额）")
    );
    raw.extend_from_slice(&request.body);
    match crate::intruder::has_http_parameters(&raw) {
        Ok(matched) => Ok(matched),
        Err(error) if error.is::<crate::intruder::AutomaticParameterLimit>() => Err(error),
        Err(_) => Ok(false),
    }
}
