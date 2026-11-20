use super::*;

pub fn build_flow_transform(
    rules: Vec<MatchAndReplaceRule>,
    messages: crate::ProxyMessageSettings,
) -> Result<Option<mio_proxy::FlowTransform>> {
    build_flow_transform_with_scope_and_provenance(rules, messages, crate::TargetScope::default(), None)
}

fn sync_content_length_header(headers: &mut Vec<Header>, body_len: usize, force_if_nonempty: bool) {
    let has_chunked = headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("transfer-encoding")
            && String::from_utf8_lossy(&h.value)
                .to_ascii_lowercase()
                .contains("chunked")
    });
    if has_chunked {
        return;
    }
    let len_bytes = body_len.to_string().into_bytes();
    let mut found = false;
    headers.retain_mut(|h| {
        if !h.name.eq_ignore_ascii_case("content-length") {
            return true;
        }
        if found {
            return false;
        }
        h.value = len_bytes.clone();
        found = true;
        true
    });
    if !found && force_if_nonempty && body_len > 0 {
        headers.push(Header {
            name: "Content-Length".into(),
            value: len_bytes,
        });
    }
}

fn fix_request_newlines_in_place(request: &mut HttpRequest) -> bool {
    let mut changed = false;
    for header in &mut request.headers {
        if header.value.iter().any(|&b| b == b'\r' || b == b'\n') {
            header.value.retain(|&b| b != b'\r' && b != b'\n');
            changed = true;
        }
    }
    let is_multipart = request.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("content-type")
            && String::from_utf8_lossy(&h.value)
                .to_ascii_lowercase()
                .contains("multipart/")
    });
    if is_multipart && !request.body.is_empty() && !request.body.ends_with(b"\r\n") {
        if request.body.ends_with(b"\n") {
            request.body.pop();
        }
        request.body.extend_from_slice(b"\r\n");
        changed = true;
    }
    changed
}

pub(crate) fn build_flow_transform_with_scope_and_provenance(
    rules: Vec<MatchAndReplaceRule>,
    messages: crate::ProxyMessageSettings,
    scope: crate::TargetScope,
    tracker: Option<crate::ProvenanceTracker>,
) -> Result<Option<mio_proxy::FlowTransform>> {
    let compiled_rules = rules
        .into_iter()
        .filter(|rule| rule.enabled && !crate::is_advanced_proxy_rule(rule))
        .map(|rule| -> Result<_> {
            if rule.item_type.is_script() {
                validate_filter_script(&rule.filter_script)
                    .context("Proxy Match and replace 脚本无效，原配置未应用")?;
            }
            if rule.regex && !rule.match_pattern.is_empty() {
                let regex = RegexBuilder::new(&rule.match_pattern)
                    .size_limit(1024 * 1024)
                    .dfa_size_limit(1024 * 1024)
                    .build()
                    .context("Proxy Match and replace 正则表达式无效，原配置未应用")?;
                Ok((rule, Some(regex)))
            } else {
                Ok((rule, None))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    if compiled_rules.is_empty()
        && !messages.unpack_compressed_requests
        && !messages.unpack_compressed_responses
        && messages.response_modification.is_default()
        && !messages.auto_fix_request_newlines
    {
        return Ok(None);
    }
    let response_modification = messages.response_modification;
    let unpack_requests = messages.unpack_compressed_requests;
    let unpack_responses = messages.unpack_compressed_responses;
    let match_replace_in_scope_only = messages.match_replace_in_scope_only;
    let auto_update_req_cl = messages.auto_update_request_content_length;
    let auto_fix_req_newlines = messages.auto_fix_request_newlines;
    let auto_update_resp_cl = messages.auto_update_response_content_length;
    Ok(Some(std::sync::Arc::new(
        move |record, response_stage, response_head_only, cancel, limits| {
            check_intercept_control(cancel, limits.0).map_err(intercept_control_error)?;
            if response_stage && record.response.is_none() {
                return Err(mio_proxy::ProxyError::InvalidMessage(
                    "Proxy 响应阶段缺少实际响应，未使用空响应替代".into(),
                ));
            }
            let initial_req =
                (!response_stage && tracker.is_some()).then(|| record.request.clone());
            let initial_resp = (response_stage && tracker.is_some())
                .then(|| record.response.clone())
                .flatten();
            let mut staged_req_steps: Vec<(crate::ProvenanceActor, HttpRequest)> = Vec::new();
            let mut staged_resp_steps: Vec<(crate::ProvenanceActor, HttpResponse)> = Vec::new();
            let mut changed = false;
            if !response_stage && unpack_requests {
                if let Some(request) = unpack_request(&record.request, limits.1, cancel, limits.0)
                    .map_err(|error| intercept_control_error(error.into()))?
                {
                    record.summary.request_bytes = request.body.len() as u64;
                    record.request = request;
                    if tracker.is_some() {
                        staged_req_steps.push((
                            crate::ProvenanceActor::UnpackCompressed,
                            record.request.clone(),
                        ));
                    }
                    changed = true;
                }
            } else if response_stage && !response_head_only && unpack_responses {
                let response = record.response.as_ref().ok_or_else(|| {
                    mio_proxy::ProxyError::InvalidMessage(
                        "Proxy 响应解包缺少实际响应，未使用空响应替代".into(),
                    )
                })?;
                if let Some(response) =
                    unpack_response(response, &record.request.method, limits.1, cancel, limits.0)
                        .map_err(|error| intercept_control_error(error.into()))?
                {
                    record.summary.response_bytes = response.body.len() as u64;
                    record.response = Some(response);
                    if let (true, Some(resp)) = (tracker.is_some(), record.response.as_ref()) {
                        staged_resp_steps
                            .push((crate::ProvenanceActor::UnpackCompressed, resp.clone()));
                    }
                    changed = true;
                }
            }
            if response_stage && !response_modification.is_default() {
                let mod_budget = if response_head_only { 0 } else { limits.1 };
                let mod_changed = proxy_transforms::apply_response_modification(
                    record,
                    response_modification,
                    mod_budget,
                    cancel,
                    limits.0,
                )
                .map_err(intercept_control_error)?;
                if mod_changed {
                    if let (true, Some(resp)) = (tracker.is_some(), record.response.as_ref()) {
                        staged_resp_steps
                            .push((crate::ProvenanceActor::ResponseModification, resp.clone()));
                    }
                    changed = true;
                }
            }
            let allow_match_replace =
                !match_replace_in_scope_only || scope.is_url_in_scope(&record.request.url);
            if allow_match_replace {
                for (rule, regex) in &compiled_rules {
                    check_intercept_control(cancel, limits.0).map_err(intercept_control_error)?;
                    let rule_changed = apply_single_rule(
                        record,
                        response_stage,
                        response_head_only,
                        rule,
                        regex.as_ref(),
                        limits.1,
                        &|| check_intercept_control(cancel, limits.0),
                    )
                    .map_err(intercept_control_error)?;
                    if rule_changed {
                        let body_bytes = if response_stage {
                            record
                                .response
                                .as_ref()
                                .ok_or_else(|| {
                                    mio_proxy::ProxyError::InvalidMessage(
                                        "Proxy 响应阶段缺少实际响应，未使用空响应替代".into(),
                                    )
                                })?
                                .body
                                .len()
                        } else {
                            record.request.body.len()
                        };
                        if body_bytes > limits.1 {
                            return Err(mio_proxy::ProxyError::InvalidMessage(
                                "Proxy Match and replace 正文超过实际消息限额，未转发原消息替代".into(),
                            ));
                        }
                        if tracker.is_some() {
                            let actor = crate::ProvenanceActor::MatchAndReplace {
                                rule_id: rule.id,
                                comment: rule.comment.clone(),
                            };
                            if response_stage {
                                if let Some(resp) = record.response.as_ref() {
                                    staged_resp_steps.push((actor, resp.clone()));
                                }
                            } else {
                                staged_req_steps.push((actor, record.request.clone()));
                            }
                        }
                        changed = true;
                    }
                }
            }
            if !response_stage && auto_fix_req_newlines && fix_request_newlines_in_place(&mut record.request) {
                changed = true;
            }
            if changed {
                if !response_stage {
                    record.summary.request_bytes = record.request.body.len() as u64;
                    if auto_update_req_cl {
                        let body_len = record.request.body.len();
                        sync_content_length_header(&mut record.request.headers, body_len, true);
                    }
                } else if !response_head_only
                    && let Some(resp) = record.response.as_mut()
                {
                    record.summary.response_bytes = resp.body.len() as u64;
                    if auto_update_resp_cl {
                        let body_len = resp.body.len();
                        sync_content_length_header(&mut resp.headers, body_len, false);
                    }
                }
            }
            check_intercept_control(cancel, limits.0).map_err(intercept_control_error)?;
            if changed && let Some(tracker) = &tracker {
                if let Some(orig) = &initial_req {
                    for (actor, snap) in staged_req_steps {
                        tracker.record_request(record.summary.id, orig, actor, &snap);
                    }
                }
                if let Some(orig) = &initial_resp {
                    for (actor, snap) in staged_resp_steps {
                        tracker.record_response(record.summary.id, orig, actor, &snap);
                    }
                }
            }
            Ok(changed)
        },
    )))
}

pub(super) type InterceptParameters<'a> = Vec<(std::borrow::Cow<'a, [u8]>, &'a [u8])>;

pub(crate) struct CompiledInterceptRules {
    enabled: bool,
    rules: Vec<(crate::ProxyInterceptRule, Option<regex::bytes::Regex>)>,
}

pub(crate) fn validate_proxy_interception_shape(
    settings: &crate::ProxyInterceptionSettings,
) -> Result<()> {
    ensure!(
        settings.request.rules.len() + settings.response.rules.len() <= 256,
        "Proxy 拦截规则总数量超过 256 条限额"
    );
    let mut ids = std::collections::HashSet::new();
    let mut condition_bytes = 0_usize;
    use crate::ProxyInterceptRelation::*;
    use crate::ProxyInterceptType::*;
    for (set, response) in [(&settings.request, false), (&settings.response, true)] {
        for rule in &set.rules {
            ensure!(
                !rule.id.is_nil() && ids.insert(rule.id),
                "Proxy 拦截规则 ID 无效或重复"
            );
            ensure!(
                rule.condition.len() <= 4096,
                "Proxy 拦截条件超过 4096 字节限额"
            );
            condition_bytes = condition_bytes
                .checked_add(rule.condition.len())
                .context("Proxy 拦截条件总字节计数溢出")?;
            ensure!(
                condition_bytes <= 128 * 1024,
                "Proxy 拦截条件总长度超过 128 KiB 限额（包括禁用规则）"
            );
            let allowed = if matches!(rule.relationship, Matches | DoesNotMatch) {
                matches!(
                    rule.match_type,
                    DomainName
                        | IpAddress
                        | HttpMethod
                        | FileExtension
                        | Url
                        | Header
                        | ParamName
                        | ParamValue
                        | ListenerPort
                ) || (response
                    && matches!(
                        rule.match_type,
                        ContentTypeHeader | StatusCode | MimeType | PageTitle
                    ))
                    || (!response && matches!(rule.match_type, CookieName | CookieValue | Body))
            } else if matches!(
                rule.relationship,
                RequestMatches | RequestDoesNotMatch | ResponseMatches | ResponseDoesNotMatch
            ) {
                response && matches!(rule.match_type, CookieName | CookieValue | Body)
            } else {
                match (rule.match_type, rule.relationship) {
                    (Protocol, IsHttp | IsHttps) => true,
                    (Url, InScope | NotInScope) => true,
                    (Request, ContainsParameters | DoesNotContainParameters) => true,
                    (
                        Request,
                        WasModified | WasNotModified | WasIntercepted | WasNotIntercepted
                        | WasAnnotated | WasNotAnnotated,
                    ) => response,
                    _ => false,
                }
            };
            ensure!(
                allowed,
                "Proxy 拦截条件的匹配类型、关系与请求/响应阶段不兼容"
            );
        }
    }
    Ok(())
}

fn compile_proxy_interception(
    settings: &crate::ProxyInterceptionSettings,
) -> Result<(CompiledInterceptRules, CompiledInterceptRules)> {
    validate_proxy_interception_shape(settings)?;
    let compile = |set: &crate::ProxyInterceptRuleSet| -> Result<CompiledInterceptRules> {
        let mut rules = Vec::with_capacity(set.rules.len());
        for rule in &set.rules {
            let matcher = if rule.relationship.is_regex() {
                Some(regex::bytes::RegexBuilder::new(&rule.condition)
                    .case_insensitive(true)
                    .unicode(false)
                    .size_limit(2 * 1024 * 1024)
                    .dfa_size_limit(2 * 1024 * 1024)
                    .nest_limit(128)
                    .build()
                    .map_err(|_| anyhow::anyhow!(
                        "Proxy 拦截正则表达式无效或超过资源限额；当前不支持 Java lookaround/backreference，原配置未应用"
                    ))?)
            } else {
                None
            };
            rules.push((rule.clone(), matcher));
        }
        Ok(CompiledInterceptRules {
            enabled: set.enabled,
            rules,
        })
    };
    Ok((compile(&settings.request)?, compile(&settings.response)?))
}

pub(crate) fn validate_proxy_interception(
    settings: &crate::ProxyInterceptionSettings,
) -> Result<()> {
    compile_proxy_interception(settings).map(|_| ())
}

pub(crate) fn build_intercept_filter(
    settings: crate::ProxyInterceptionSettings,
    scope: crate::TargetScope,
) -> Result<mio_proxy::InterceptFilter> {
    scope.validate()?;
    let (request, response) = compile_proxy_interception(&settings)?;
    Ok(std::sync::Arc::new(
        move |record,
              response_stage,
              response_head_only,
              was_intercepted,
              was_modified,
              eval_ctx,
              cancel,
              limits| {
            let rules = if response_stage { &response } else { &request };
            rules
                .matches(
                    record,
                    &scope,
                    (
                        response_stage,
                        response_head_only,
                        was_intercepted,
                        was_modified,
                        eval_ctx,
                    ),
                    cancel,
                    limits,
                )
                .map_err(intercept_control_error)
        },
    ))
}

fn alternate_websocket_scheme(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let mapped = if scheme.eq_ignore_ascii_case("ws") {
        "http"
    } else if scheme.eq_ignore_ascii_case("wss") {
        "https"
    } else if scheme.eq_ignore_ascii_case("http") {
        "ws"
    } else if scheme.eq_ignore_ascii_case("https") {
        "wss"
    } else {
        return None;
    };
    Some(format!("{mapped}://{rest}"))
}

fn websocket_url_in_scope(scope: &crate::TargetScope, url: &str) -> bool {
    let alternate = alternate_websocket_scheme(url);
    scope.is_url_in_scope(url)
        || alternate
            .as_deref()
            .is_some_and(|alternate| scope.is_url_in_scope(alternate))
}

pub(crate) fn build_websocket_intercept_filter(
    settings: crate::ProxyWebSocketInterceptionSettings,
    scope: crate::TargetScope,
) -> Result<mio_proxy::WebSocketInterceptFilter> {
    scope.validate()?;
    Ok(std::sync::Arc::new(
        move |request: &crate::HttpRequest, direction, cancel| {
            let _ = cancel;
            let allowed = match direction {
                mio_plugin_api::WebSocketDirection::ClientToServer => {
                    settings.client_to_server_messages
                }
                mio_plugin_api::WebSocketDirection::ServerToClient => {
                    settings.server_to_client_messages
                }
            };
            if !allowed {
                return Ok(false);
            }
            if settings.intercept_in_scope_only && !websocket_url_in_scope(&scope, &request.url) {
                return Ok(false);
            }
            Ok(true)
        },
    ))
}

impl CompiledInterceptRules {
    fn matches(
        &self,
        record: &FlowRecord,
        scope: &crate::TargetScope,
        context: (bool, bool, bool, bool, mio_proxy::InterceptEvalContext<'_>),
        cancel: &tokio_util::sync::CancellationToken,
        limits: (std::time::Instant, usize),
    ) -> Result<bool> {
        if !self.enabled {
            return Ok(false);
        }
        check_intercept_control(cancel, limits.0)?;
        ensure!(
            !context.0 || record.response.is_some(),
            "Proxy 响应尚未可用，不能虚构响应拦截阶段"
        );
        let mut result: Option<bool> = None;
        let mut parameters = None;
        for (rule, matcher) in self.rules.iter().filter(|(rule, _)| rule.enabled) {
            check_intercept_control(cancel, limits.0)?;
            if let Some(previous) = result
                && ((rule.operator == crate::ProxyInterceptOperator::And && !previous)
                    || (rule.operator == crate::ProxyInterceptOperator::Or && previous))
            {
                continue;
            }
            result = Some(intercept_rule_matches(
                (rule, matcher.as_ref()),
                record,
                scope,
                context,
                cancel,
                limits,
                &mut parameters,
            )?);
            check_intercept_control(cancel, limits.0)?;
        }
        check_intercept_control(cancel, limits.0)?;
        Ok(result.unwrap_or(true))
    }
}
