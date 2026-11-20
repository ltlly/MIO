use super::*;

pub(super) fn intercept_rule_matches<'a>(
    condition: (&crate::ProxyInterceptRule, Option<&regex::bytes::Regex>),
    record: &'a FlowRecord,
    scope: &crate::TargetScope,
    context: (bool, bool, bool, bool, mio_proxy::InterceptEvalContext<'_>),
    cancel: &tokio_util::sync::CancellationToken,
    limits: (std::time::Instant, usize),
    parameters: &mut Option<InterceptParameters<'a>>,
) -> Result<bool> {
    use crate::ProxyInterceptRelation::*;
    use crate::ProxyInterceptType::*;
    let (rule, matcher) = condition;
    let (response, response_head_only, was_intercepted, was_modified, eval_ctx) = context;
    let matched = match rule.relationship {
        IsHttp | IsHttps => {
            let scheme = record
                .request
                .url
                .split_once("://")
                .map(|(scheme, _)| scheme)
                .context("Proxy 协议条件缺少目标 URL scheme，未自动放行")?;
            let secure =
                if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") {
                    true
                } else if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("ws") {
                    false
                } else {
                    bail!("Proxy 协议条件不支持此目标 URL scheme，未自动放行")
                };
            secure == (rule.relationship == IsHttps)
        }
        ContainsParameters | DoesNotContainParameters => {
            request_has_intercept_parameters(&record.request)?
        }
        WasModified | WasNotModified => was_modified,
        WasIntercepted | WasNotIntercepted => was_intercepted,
        WasAnnotated | WasNotAnnotated => extra_conditions::matches_annotation_condition(
            eval_ctx.was_annotated,
            matches!(rule.relationship, WasAnnotated),
        ),
        InScope | NotInScope => scope.is_url_in_scope(&record.request.url),
        Matches | DoesNotMatch | RequestMatches | RequestDoesNotMatch | ResponseMatches
        | ResponseDoesNotMatch => {
            let matcher = matcher.context("Proxy 拦截正则未编译，未自动放行")?;
            match rule.match_type {
                DomainName => {
                    let url = url::Url::parse(&record.request.url)
                        .context("Proxy 拦截请求 URL 无法解析域名")?;
                    matcher.is_match(
                        url.host_str()
                            .context("Proxy 拦截请求 URL 缺少主机名")?
                            .as_bytes(),
                    )
                }
                IpAddress => {
                    let ip = extra_conditions::normalize_ip_candidate(eval_ctx.target_ip)
                        .or_else(|| {
                            url::Url::parse(&record.request.url).ok().and_then(|url| {
                                extra_conditions::normalize_ip_candidate(url.host_str())
                            })
                        })
                        .context("目标 IP 尚未确定；未按负向条件自动放行")?;
                    matcher.is_match(ip.as_bytes())
                }
                HttpMethod => matcher.is_match(record.request.method.as_bytes()),
                ListenerPort => {
                    ensure!(
                        eval_ctx.listener_port != 0,
                        "Proxy 实际监听端口尚未可用，不能按端口 0 匹配"
                    );
                    matcher.is_match(eval_ctx.listener_port.to_string().as_bytes())
                }
                FileExtension => {
                    let url = record
                        .request
                        .url
                        .split(['?', '#'])
                        .next()
                        .unwrap_or_default();
                    let path = if let Some((_, remainder)) = url.split_once("://") {
                        remainder.find('/').map_or("", |start| &remainder[start..])
                    } else {
                        url
                    };
                    let name = path.rsplit('/').next().unwrap_or_default();
                    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
                    matcher.is_match(extension.as_bytes())
                }
                Url => matcher.is_match(record.request.url.as_bytes()),
                Header | ContentTypeHeader => {
                    let headers = if response {
                        &record
                            .response
                            .as_ref()
                            .context("Proxy 响应头尚未可用，不能按空响应匹配")?
                            .headers
                    } else {
                        &record.request.headers
                    };
                    validate_intercept_headers(headers, cancel, limits.0)?;
                    if rule.match_type == ContentTypeHeader {
                        headers
                            .iter()
                            .filter(|header| header.name.eq_ignore_ascii_case("content-type"))
                            .any(|header| matcher.is_match(&header.value))
                    } else {
                        headers.iter().any(|header| {
                            let mut line =
                                Vec::with_capacity(header.name.len() + header.value.len() + 2);
                            line.extend_from_slice(header.name.as_bytes());
                            line.extend_from_slice(b": ");
                            line.extend_from_slice(&header.value);
                            matcher.is_match(&line)
                        })
                    }
                }
                StatusCode => matcher.is_match(
                    record
                        .response
                        .as_ref()
                        .context("Proxy 响应状态尚未可用，不能按空响应匹配")?
                        .status
                        .to_string()
                        .as_bytes(),
                ),
                CookieName | CookieValue => {
                    let in_response =
                        matches!(rule.relationship, ResponseMatches | ResponseDoesNotMatch);
                    let headers = if in_response {
                        &record
                            .response
                            .as_ref()
                            .context("Proxy 响应 Cookie 尚未可用，未按空值匹配")?
                            .headers
                    } else {
                        &record.request.headers
                    };
                    let pairs = intercept_cookie_pairs(headers, in_response, cancel, limits.0)?;
                    intercept_pairs_match(
                        pairs.iter().map(|(name, value)| {
                            if rule.match_type == CookieName {
                                *name
                            } else {
                                *value
                            }
                        }),
                        matcher,
                        &|| check_intercept_control(cancel, limits.0),
                    )?
                }
                Body => {
                    let in_response =
                        matches!(rule.relationship, ResponseMatches | ResponseDoesNotMatch);
                    if in_response {
                        ensure!(
                            !response_head_only,
                            "SSE 响应正文尚未完整接收，不能按空正文判断拦截条件"
                        );
                        let body = &record
                            .response
                            .as_ref()
                            .context("Proxy 响应正文尚未可用，未使用空正文替代")?
                            .body;
                        ensure!(
                            body.len() <= limits.1,
                            "Proxy 响应正文超过条件匹配限额，未截断后匹配"
                        );
                        matcher.is_match(body)
                    } else {
                        ensure!(
                            record.request.body.len() <= limits.1,
                            "Proxy 请求正文超过条件匹配限额，未截断后匹配"
                        );
                        matcher.is_match(&record.request.body)
                    }
                }
                ParamName | ParamValue => {
                    if parameters.is_none() {
                        *parameters = Some(intercept_request_parameters(
                            &record.request,
                            cancel,
                            limits,
                        )?);
                    }
                    let pairs = parameters
                        .as_ref()
                        .context("Proxy 请求参数尚未解析，未使用空参数替代")?;
                    intercept_pairs_match(
                        pairs.iter().map(|(name, value)| {
                            if rule.match_type == ParamName {
                                name.as_ref()
                            } else {
                                *value
                            }
                        }),
                        matcher,
                        &|| check_intercept_control(cancel, limits.0),
                    )?
                }
                MimeType => {
                    let response = record
                        .response
                        .as_ref()
                        .context("Proxy 响应尚未可用，不能按空响应匹配 MIME 类型")?;
                    validate_intercept_headers(&response.headers, cancel, limits.0)?;
                    let body = if response_head_only {
                        &[][..]
                    } else {
                        ensure!(
                            response.body.len() <= limits.1,
                            "Proxy 响应正文超过条件匹配限额，未截断后嗅探 MIME 类型"
                        );
                        &response.body
                    };
                    let mime = extra_conditions::infer_proxy_mime_type(&response.headers, body);
                    ensure!(
                        !response_head_only || !mime.is_empty(),
                        "SSE 流式响应头缺少可判定 Content-Type，无法嗅探流式响应体，未自动放行"
                    );
                    matcher.is_match(mime.as_bytes())
                }
                PageTitle => {
                    ensure!(
                        !response_head_only,
                        "SSE 响应正文尚未完整接收，不能按空正文判断页面标题条件"
                    );
                    let body = &record
                        .response
                        .as_ref()
                        .context("Proxy 响应正文尚未可用，未使用空标题替代")?
                        .body;
                    ensure!(
                        body.len() <= limits.1,
                        "Proxy 响应正文超过条件匹配限额，未截断后解析页面标题"
                    );
                    let title = extra_conditions::extract_html_page_title_checked(body, &|| {
                        check_intercept_control(cancel, limits.0)
                    })?;
                    matcher.is_match(title.as_bytes())
                }
                Request => bail!("Proxy 请求匹配关系不支持正则，未自动放行"),
                Protocol => bail!("Proxy 协议匹配关系不支持正则，未自动放行"),
            }
        }
    };
    Ok(
        if matches!(
            rule.relationship,
            DoesNotMatch
                | RequestDoesNotMatch
                | ResponseDoesNotMatch
                | DoesNotContainParameters
                | WasNotModified
                | WasNotIntercepted
                | NotInScope
        ) {
            !matched
        } else {
            matched
        },
    )
}
