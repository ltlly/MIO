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

#[cfg(test)]
pub(in crate::service) async fn run_pre_send_plugin_hooks(
    session_hooks: &[crate::plugins::PluginHandle],
    http_hooks: &[crate::plugins::PluginHandle],
    flow_id: FlowId,
    source: protocol::SourceDto,
    request: &mut HttpRequest,
    deadline: Instant,
    cancel: &CancellationToken,
) -> PreSendHookOutcome {
    let session_entries: Vec<PreSendPluginHook> = session_hooks
        .iter()
        .map(|handle| PreSendPluginHook {
            plugin_id: String::new(),
            identity: handle.identity,
            handle: handle.clone(),
            filter: PluginHookFilter::default(),
            counters: std::sync::Arc::new(PluginHookCounters::default()),
        })
        .collect();
    let http_entries: Vec<PreSendPluginHook> = http_hooks
        .iter()
        .map(|handle| PreSendPluginHook {
            plugin_id: String::new(),
            identity: handle.identity,
            handle: handle.clone(),
            filter: PluginHookFilter::default(),
            counters: std::sync::Arc::new(PluginHookCounters::default()),
        })
        .collect();
    run_pre_send_plugin_hooks_configured(
        &session_entries,
        &http_entries,
        None,
        None,
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
pub(in crate::service) async fn run_pre_send_plugin_hooks_configured(
    session_hooks: &[PreSendPluginHook],
    http_hooks: &[PreSendPluginHook],
    scope: Option<&TargetScope>,
    events: Option<&mpsc::Sender<Event>>,
    flow_id: FlowId,
    source: protocol::SourceDto,
    request: &mut HttpRequest,
    provenance: Option<(&crate::ProvenanceTracker, &HttpRequest)>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> PreSendHookOutcome {
    let mut current = request.clone();
    for entry in session_hooks {
        if !entry
            .filter
            .should_invoke_http(&current.url, false, None, scope)
        {
            entry.counters.inc_filtered_skipped();
            continue;
        }
        let Some((call, _spill)) = super::large_body::prepare_session_action_call(
            source as i32,
            &current,
            entry.handle.protocol_minor(),
            entry.handle.supports_large_body(),
        ) else {
            entry.counters.inc_oversized_bypassed();
            continue;
        };
        entry.counters.inc_invoked();
        let call_cancel = cancel.child_token();
        let _call_cancel_guard = call_cancel.clone().drop_guard();
        let res = match tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            entry.handle.call_session_action(call, call_cancel.clone()),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => {
                call_cancel.cancel();
                if cancel.is_cancelled() {
                    return PreSendHookOutcome::Failed("插件会话钩子已取消，未发送请求".into());
                }
                entry.counters.inc_error_or_timeout();
                let msg = "插件会话钩子超时，未发送请求";
                if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                    entry.counters.inc_fail_open_pass();
                    super::emit_fail_open_diagnostic(
                        events,
                        entry.identity,
                        "Session Action Hook",
                        msg,
                    );
                    continue;
                }
                return PreSendHookOutcome::Failed(msg.into());
            }
        };
        match res {
            Ok(res) if res.modified => {
                match res.request.context("插件会话钩子缺少请求").and_then(|req| {
                    super::large_body::resolve_request_dto(req, &current.body, &_spill)
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
                                    plugin_id: entry.plugin_id.clone(),
                                    hook: "session_action".into(),
                                },
                                &req,
                            );
                        }
                        current = req;
                    }
                    Err(error) => {
                        entry.counters.inc_error_or_timeout();
                        let msg = format!("{error}；未发送请求");
                        if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                            entry.counters.inc_fail_open_pass();
                            super::emit_fail_open_diagnostic(
                                events,
                                entry.identity,
                                "Session Action Hook",
                                &msg,
                            );
                            continue;
                        }
                        return PreSendHookOutcome::Failed(msg);
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                if cancel.is_cancelled() {
                    return PreSendHookOutcome::Failed(format!(
                        "插件会话钩子已取消，未发送请求：{error}"
                    ));
                }
                entry.counters.inc_error_or_timeout();
                let msg = format!("插件会话钩子失败，未发送请求：{error}");
                if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                    entry.counters.inc_fail_open_pass();
                    super::emit_fail_open_diagnostic(
                        events,
                        entry.identity,
                        "Session Action Hook",
                        &msg,
                    );
                    continue;
                }
                return PreSendHookOutcome::Failed(msg);
            }
        }
    }

    if !http_hooks.is_empty() && plugin_hook_depth(&current) <= 1 {
        for entry in http_hooks {
            if !entry
                .filter
                .should_invoke_http(&current.url, false, None, scope)
            {
                entry.counters.inc_filtered_skipped();
                continue;
            }
            let Some((call, _spill)) = super::large_body::prepare_http_hook_call(
                flow_id,
                source as i32,
                String::new(),
                false,
                false,
                &current,
                None,
                entry.handle.protocol_minor(),
                entry.handle.supports_large_body(),
            ) else {
                entry.counters.inc_oversized_bypassed();
                continue;
            };
            entry.counters.inc_invoked();
            let call_cancel = cancel.child_token();
            let _call_cancel_guard = call_cancel.clone().drop_guard();
            let res = match tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                entry.handle.call_http_hook(call, call_cancel.clone()),
            )
            .await
            {
                Ok(Ok(res)) => res,
                Ok(Err(error)) => {
                    if cancel.is_cancelled() {
                        return PreSendHookOutcome::Failed(format!(
                            "插件 HTTP 钩子已取消，未发送请求：{error}"
                        ));
                    }
                    entry.counters.inc_error_or_timeout();
                    let msg = format!("插件 HTTP 钩子失败，未发送请求：{error}");
                    if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                        entry.counters.inc_fail_open_pass();
                        super::emit_fail_open_diagnostic(
                            events,
                            entry.identity,
                            "Pre-Send HTTP Hook",
                            &msg,
                        );
                        continue;
                    }
                    return PreSendHookOutcome::Failed(msg);
                }
                Err(_) => {
                    call_cancel.cancel();
                    if cancel.is_cancelled() {
                        return PreSendHookOutcome::Failed("插件 HTTP 钩子已取消，未发送请求".into());
                    }
                    entry.counters.inc_error_or_timeout();
                    let msg = "插件 HTTP 钩子超时，未发送请求";
                    if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                        entry.counters.inc_fail_open_pass();
                        super::emit_fail_open_diagnostic(
                            events,
                            entry.identity,
                            "Pre-Send HTTP Hook",
                            msg,
                        );
                        continue;
                    }
                    return PreSendHookOutcome::Failed(msg.into());
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
                        entry.counters.inc_error_or_timeout();
                        let msg = "插件声明修改请求但未返回请求；未发送请求";
                        if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                            entry.counters.inc_fail_open_pass();
                            super::emit_fail_open_diagnostic(
                                events,
                                entry.identity,
                                "Pre-Send HTTP Hook",
                                msg,
                            );
                            continue;
                        }
                        return PreSendHookOutcome::Failed(msg.into());
                    };
                    match super::large_body::resolve_request_dto(dto, &current.body, &_spill) {
                        Ok(req) => {
                            if req != current
                                && let Some((tracker, orig)) = provenance
                            {
                                tracker.record_request(
                                    flow_id,
                                    orig,
                                    ProvenanceActor::PluginHook {
                                        plugin_id: entry.plugin_id.clone(),
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
                            entry.counters.inc_error_or_timeout();
                            let msg = format!("插件 HTTP 钩子返回无效请求：{error}；未发送请求");
                            if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                                entry.counters.inc_fail_open_pass();
                                super::emit_fail_open_diagnostic(
                                    events,
                                    entry.identity,
                                    "Pre-Send HTTP Hook",
                                    &msg,
                                );
                                continue;
                            }
                            return PreSendHookOutcome::Failed(msg);
                        }
                    }
                }
                Err(_) => {
                    entry.counters.inc_error_or_timeout();
                    let msg = "插件返回未知 Hook 动作；未发送请求";
                    if entry.filter.failure_policy == PluginHookFailurePolicy::FailOpen {
                        entry.counters.inc_fail_open_pass();
                        super::emit_fail_open_diagnostic(
                            events,
                            entry.identity,
                            "Pre-Send HTTP Hook",
                            msg,
                        );
                        continue;
                    }
                    return PreSendHookOutcome::Failed(msg.into());
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
