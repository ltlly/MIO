use super::*;

pub(in crate::service) fn sync_flow_request(record: &mut FlowRecord, request: HttpRequest) {
    record.request = request;
    record.summary.method.clone_from(&record.request.method);
    record.summary.url.clone_from(&record.request.url);
    record.summary.request_bytes = record.request.body.len() as u64;
}

pub(in crate::service) enum PreSendHookOutcome {
    Proceed { mutated: bool },
    Dropped,
    Failed(String),
}

/// Fails closed on duplicate maxima, malformed UTF-8/numeric headers, or u8 overflow.
pub(in crate::service) fn plugin_hook_depth(request: &HttpRequest) -> u8 {
    let mut max_depth = 0u8;
    for header in &request.headers {
        if !header.name.eq_ignore_ascii_case("x-mio-plugin-depth") {
            continue;
        }
        let Some(parsed) = std::str::from_utf8(&header.value)
            .ok()
            .and_then(|raw| raw.trim().parse::<u8>().ok())
        else {
            return u8::MAX;
        };
        max_depth = max_depth.max(parsed);
    }
    max_depth
}

fn tool_name(source: protocol::SourceDto) -> &'static str {
    match source {
        protocol::SourceDto::Repeater => "Repeater",
        protocol::SourceDto::Intruder => "Intruder",
        protocol::SourceDto::Proxy => "Proxy",
        protocol::SourceDto::Plugin => "Plugin",
        protocol::SourceDto::Scanner => "Scanner",
    }
}

pub(in crate::service) async fn run_pre_send_plugin_hooks(
    session_hooks: &[crate::plugins::PluginHandle],
    http_hooks: &[crate::plugins::PluginHandle],
    flow_id: FlowId,
    source: protocol::SourceDto,
    request: &mut HttpRequest,
    deadline: Instant,
    cancel: &CancellationToken,
) -> PreSendHookOutcome {
    let named_session: Vec<(String, crate::plugins::PluginHandle)> = session_hooks
        .iter()
        .map(|h| (String::new(), h.clone()))
        .collect();
    let named_http: Vec<(String, crate::plugins::PluginHandle)> = http_hooks
        .iter()
        .map(|h| (String::new(), h.clone()))
        .collect();
    run_pre_send_plugin_hooks_with_provenance(
        &named_session,
        &named_http,
        flow_id,
        source,
        request,
        None,
        deadline,
        cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(in crate::service) async fn run_pre_send_plugin_hooks_with_provenance(
    session_hooks: &[(String, crate::plugins::PluginHandle)],
    http_hooks: &[(String, crate::plugins::PluginHandle)],
    flow_id: FlowId,
    source: protocol::SourceDto,
    request: &mut HttpRequest,
    provenance: Option<(&crate::ProvenanceTracker, &HttpRequest)>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> PreSendHookOutcome {
    let mut current = request.clone();
    for (plugin_id, handle) in session_hooks {
        if !super::http_request_fits_inline_fast(&current) {
            break;
        }
        let call = protocol::SessionActionCall {
            source: source as i32,
            request: Some(current.clone().into()),
        };
        let Ok(protocol::envelope::Body::SessionActionCall(call)) = protocol::try_into_wire_body(
            protocol::envelope::Body::SessionActionCall(call),
            protocol::PROTOCOL_MINOR,
        ) else {
            break;
        };
        let call_cancel = cancel.child_token();
        let _call_cancel_guard = call_cancel.clone().drop_guard();
        let res = match tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            handle.call_session_action(call, call_cancel.clone()),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => {
                call_cancel.cancel();
                return PreSendHookOutcome::Failed("插件会话钩子超时，未发送请求".into());
            }
        };
        match res {
            Ok(res) if res.modified => {
                match res.request.context("插件会话钩子缺少请求").and_then(|req| {
                    req.try_into()
                        .map_err(|error| anyhow::anyhow!("插件会话钩子返回无效请求：{error}"))
                }) {
                    Ok(req) => {
                        if req != current
                            && let Some((tracker, orig)) = provenance
                        {
                            tracker.record_request(
                                flow_id,
                                orig,
                                ProvenanceActor::PluginHook {
                                    plugin_id: plugin_id.clone(),
                                    hook: "session_action".into(),
                                },
                                &req,
                            );
                        }
                        current = req;
                    }
                    Err(error) => {
                        return PreSendHookOutcome::Failed(format!("{error}；未发送请求"));
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                return PreSendHookOutcome::Failed(format!(
                    "插件会话钩子失败，未发送请求：{error}"
                ));
            }
        }
    }

    if !http_hooks.is_empty() && plugin_hook_depth(&current) <= 1 {
        for (plugin_id, handle) in http_hooks {
            if !super::http_request_fits_inline_fast(&current) {
                break;
            }
            let call = protocol::HttpHookCall {
                flow_id: flow_id.as_bytes().to_vec(),
                source: source as i32,
                plugin_id: String::new(),
                response_stage: false,
                response_head_only: false,
                request: Some(current.clone().into()),
                response: None,
            };
            let Ok(protocol::envelope::Body::HttpHookCall(call)) = protocol::try_into_wire_body(
                protocol::envelope::Body::HttpHookCall(call),
                protocol::PROTOCOL_MINOR,
            ) else {
                break;
            };
            let call_cancel = cancel.child_token();
            let _call_cancel_guard = call_cancel.clone().drop_guard();
            let res = match tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                handle.call_http_hook(call, call_cancel.clone()),
            )
            .await
            {
                Ok(Ok(res)) => res,
                Ok(Err(error)) => {
                    return PreSendHookOutcome::Failed(format!(
                        "插件 HTTP 钩子失败，未发送请求：{error}"
                    ));
                }
                Err(_) => {
                    call_cancel.cancel();
                    return PreSendHookOutcome::Failed("插件 HTTP 钩子超时，未发送请求".into());
                }
            };
            match protocol::HookAction::try_from(res.action) {
                Ok(protocol::HookAction::Pass) => {}
                Ok(protocol::HookAction::Drop) => {
                    let mutated = current != *request;
                    if mutated {
                        *request = current;
                    }
                    return PreSendHookOutcome::Dropped;
                }
                Ok(protocol::HookAction::Intercept) => {
                    return PreSendHookOutcome::Failed(format!(
                        "{} 不支持挂起手动拦截动作（Intercept）；插件请求钩子已拒绝，未发送请求",
                        tool_name(source)
                    ));
                }
                Ok(protocol::HookAction::Modify) => {
                    let Some(dto) = res.request else {
                        return PreSendHookOutcome::Failed(
                            "插件声明修改请求但未返回请求；未发送请求".into(),
                        );
                    };
                    match dto.try_into() {
                        Ok(req) => {
                            if req != current
                                && let Some((tracker, orig)) = provenance
                            {
                                tracker.record_request(
                                    flow_id,
                                    orig,
                                    ProvenanceActor::PluginHook {
                                        plugin_id: plugin_id.clone(),
                                        hook: "http_hook".into(),
                                    },
                                    &req,
                                );
                            }
                            current = req;
                            if plugin_hook_depth(&current) > 1 {
                                break;
                            }
                        }
                        Err(error) => {
                            return PreSendHookOutcome::Failed(format!(
                                "插件 HTTP 钩子返回无效请求：{error}；未发送请求"
                            ));
                        }
                    }
                }
                Err(_) => {
                    return PreSendHookOutcome::Failed("插件返回未知 Hook 动作；未发送请求".into());
                }
            }
        }
    }

    let mutated = current != *request;
    if mutated {
        *request = current;
    }
    PreSendHookOutcome::Proceed { mutated }
}
