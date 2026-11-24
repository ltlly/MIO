use super::*;

pub(in crate::service) mod large_body;
mod pre_send;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(in crate::service) use pre_send::run_pre_send_plugin_hooks;
pub(in crate::service) use pre_send::{
    PreSendHookOutcome, plugin_hook_depth, run_pre_send_plugin_hooks_configured, sync_flow_request,
};

#[derive(Clone)]
pub(in crate::service) struct PreSendPluginHook {
    pub plugin_id: String,
    pub identity: ChannelIdentity,
    pub handle: crate::plugins::PluginHandle,
    pub filter: PluginHookFilter,
    pub counters: std::sync::Arc<PluginHookCounters>,
}

pub(super) fn response_content_type(response: &HttpResponse) -> Option<&str> {
    response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .and_then(|header| std::str::from_utf8(&header.value).ok())
}

pub(super) fn emit_fail_open_diagnostic(
    events: Option<&mpsc::Sender<Event>>,
    identity: ChannelIdentity,
    label: &str,
    err: &str,
) {
    if let Some(tx) = events {
        let _ = tx.try_send(Event::PluginDiagnostic {
            identity,
            message: format!("{label} Fail-Open 已放行：{err}"),
        });
    }
}

pub(in crate::service) fn http_request_fits_inline_fast(request: &HttpRequest) -> bool {
    !request.method.is_empty()
        && request.method.len() <= 32
        && !request.url.is_empty()
        && request.url.len() <= 16 * 1024
        && request.body.len() <= protocol::MAX_INLINE_BODY
        && request.headers.len() <= 512
        && request.trailers.len() <= 512
}

pub(in crate::service) fn http_response_fits_inline_fast(response: &HttpResponse) -> bool {
    (100..=599).contains(&response.status)
        && response.body.len() <= protocol::MAX_INLINE_BODY
        && response.headers.len() <= 512
        && response.trailers.len() <= 512
}

impl State {
    fn ordered_suite_plugins(&self) -> Vec<&Instance> {
        self.plugins
            .order
            .iter()
            .filter_map(|id| self.plugins.instances.get(id))
            .filter(|inst| {
                inst.info.tab_id.is_none()
                    && !self
                        .plugins
                        .context_viewers
                        .contains_key(&inst.info.identity.instance_id)
            })
            .collect()
    }

    pub(crate) fn session_action_hooks(&self) -> Vec<crate::plugins::PluginHandle> {
        if self.extensions_disabled {
            return Vec::new();
        }
        self.ordered_suite_plugins()
            .into_iter()
            .filter(|inst| {
                let has_session = inst.info.grants.contains(&Permission::SessionHook)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "session_action");
                let has_http = inst.info.grants.contains(&Permission::TrafficModify)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "http_hook");
                has_session || has_http
            })
            .map(|inst| inst.worker.handle())
            .collect()
    }

    pub(in crate::service) fn strict_session_action_hook_entries(
        &self,
    ) -> Vec<PreSendPluginHook> {
        if self.extensions_disabled {
            return Vec::new();
        }
        self.ordered_suite_plugins()
            .into_iter()
            .filter(|inst| {
                inst.info.grants.contains(&Permission::SessionHook)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "session_action")
            })
            .map(|inst| PreSendPluginHook {
                plugin_id: inst.info.manifest.id.clone(),
                identity: inst.info.identity,
                handle: inst.worker.handle(),
                filter: inst.hook_filter.clone(),
                counters: std::sync::Arc::clone(&inst.hook_counters),
            })
            .collect()
    }

    #[cfg(test)]
    pub(in crate::service) fn strict_session_action_hooks(
        &self,
    ) -> Vec<crate::plugins::PluginHandle> {
        self.strict_session_action_hook_entries()
            .into_iter()
            .map(|entry| entry.handle)
            .collect()
    }

    #[cfg(test)]
    pub(in crate::service) fn tool_http_hooks(&self) -> Vec<crate::plugins::PluginHandle> {
        self.tool_http_hook_entries()
            .into_iter()
            .map(|entry| entry.handle)
            .collect()
    }

    pub(in crate::service) fn tool_http_hook_entries(&self) -> Vec<PreSendPluginHook> {
        if self.extensions_disabled {
            return Vec::new();
        }
        self.ordered_suite_plugins()
            .into_iter()
            .filter(|inst| {
                inst.info.grants.contains(&Permission::TrafficModify)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "http_hook")
            })
            .map(|inst| PreSendPluginHook {
                plugin_id: inst.info.manifest.id.clone(),
                identity: inst.info.identity,
                handle: inst.worker.handle(),
                filter: inst.hook_filter.clone(),
                counters: std::sync::Arc::clone(&inst.hook_counters),
            })
            .collect()
    }

    pub(in crate::service) fn passive_check_hook_entries(
        &self,
    ) -> Vec<(
        ChannelIdentity,
        crate::plugins::PluginHandle,
        PluginHookFilter,
        std::sync::Arc<PluginHookCounters>,
    )> {
        if self.extensions_disabled {
            return Vec::new();
        }
        self.ordered_suite_plugins()
            .into_iter()
            .filter(|inst| {
                inst.info.grants.contains(&Permission::TrafficRead)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "passive_check")
            })
            .map(|inst| {
                (
                    inst.info.identity,
                    inst.worker.handle(),
                    inst.hook_filter.clone(),
                    std::sync::Arc::clone(&inst.hook_counters),
                )
            })
            .collect()
    }

    #[allow(dead_code)]
    pub(in crate::service) fn passive_check_hooks(&self) -> Vec<crate::plugins::PluginHandle> {
        self.passive_check_hook_entries()
            .into_iter()
            .map(|(_, h, _, _)| h)
            .collect()
    }

    pub(in crate::service) fn active_check_hooks(&self) -> Vec<crate::plugins::PluginHandle> {
        if self.extensions_disabled {
            return Vec::new();
        }
        self.ordered_suite_plugins()
            .into_iter()
            .filter(|inst| {
                inst.info.grants.contains(&Permission::TrafficRead)
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "active_check")
            })
            .map(|inst| inst.worker.handle())
            .collect()
    }

    pub(crate) fn build_proxy_plugin_hooks(
        &self,
    ) -> (
        Option<mio_proxy::AsyncFlowHook>,
        Option<mio_proxy::AsyncWebSocketHook>,
    ) {
        let ordered = if self.extensions_disabled {
            Vec::new()
        } else {
            self.ordered_suite_plugins()
        };
        let http_handles: Vec<(
            String,
            ChannelIdentity,
            crate::plugins::PluginHandle,
            PluginHookFilter,
            std::sync::Arc<PluginHookCounters>,
        )> = ordered
            .iter()
            .filter(|inst| {
                (inst.info.grants.contains(&Permission::TrafficRead)
                    || inst.info.grants.contains(&Permission::TrafficModify))
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "http_hook")
            })
            .map(|inst| {
                (
                    inst.info.manifest.id.clone(),
                    inst.info.identity,
                    inst.worker.handle(),
                    inst.hook_filter.clone(),
                    std::sync::Arc::clone(&inst.hook_counters),
                )
            })
            .collect();
        let ws_handles: Vec<(
            ChannelIdentity,
            crate::plugins::PluginHandle,
            PluginHookFilter,
            std::sync::Arc<PluginHookCounters>,
        )> = ordered
            .iter()
            .filter(|inst| {
                (inst.info.grants.contains(&Permission::TrafficRead)
                    || inst.info.grants.contains(&Permission::TrafficModify))
                    && inst
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|c| c == "ws_hook")
            })
            .map(|inst| {
                (
                    inst.info.identity,
                    inst.worker.handle(),
                    inst.hook_filter.clone(),
                    std::sync::Arc::clone(&inst.hook_counters),
                )
            })
            .collect();

        let scope = self
            .store
            .metadata()
            .workspace_arc()
            .map(|ws| ws.scope.clone())
            .unwrap_or_default();
        let plugin_events = self.events.clone();
        let has_session_hooks = self.sessions.runtime.has_proxy_hooks();
        let sessions = self.sessions.runtime.clone();
        let provenance = self.storage.provenance.clone();
        let flow_scope = scope.clone();
        let flow_events = plugin_events.clone();
        let flow_hook =
            (!http_handles.is_empty() || has_session_hooks).then(|| -> mio_proxy::AsyncFlowHook {
                Arc::new(
                    move |mut record, response_stage, response_head_only, cancel, deadline| {
                        let handles = http_handles.clone();
                        let sessions = sessions.clone();
                        let provenance = provenance.clone();
                        let scope = flow_scope.clone();
                        let events = flow_events.clone();
                        Box::pin(async move {
                            let mut modified = false;
                            let mut force_intercept = false;
                            let initial_req = record.request.clone();
                            let initial_resp = record.response.clone();
                            if record.summary.source == TrafficSource::Proxy {
                                if !response_stage {
                                    let send_cfg = sessions.proxy_send_config();
                                    match sessions
                                        .apply_before_send_with_steps(
                                            SessionTool::Proxy,
                                            &mut record.request,
                                            &send_cfg,
                                            &cancel,
                                            deadline,
                                        )
                                        .await
                                    {
                                        Ok((changed, steps)) => {
                                            for (rule_name, snap) in steps {
                                                provenance.record_request(
                                                    record.summary.id,
                                                    &initial_req,
                                                    ProvenanceActor::SessionRule { rule_name },
                                                    &snap,
                                                );
                                            }
                                            modified |= changed;
                                        }
                                        Err(crate::sessions::SessionExecError::Cancelled) => {
                                            return Err(mio_proxy::ProxyError::Cancelled);
                                        }
                                        Err(crate::sessions::SessionExecError::Failed(msg)) => {
                                            return Err(mio_proxy::ProxyError::InvalidMessage(
                                                format!("会话规则拒绝转发：{msg}"),
                                            ));
                                        }
                                    }
                                } else if let Some(resp) = &record.response {
                                    sessions.observe_response(
                                        SessionTool::Proxy,
                                        &record.request.url,
                                        resp,
                                    );
                                }
                            }
                            let (source, plugin_id) = match &record.summary.source {
                                TrafficSource::Proxy => {
                                    (protocol::SourceDto::Proxy as i32, String::new())
                                }
                                TrafficSource::Repeater => {
                                    (protocol::SourceDto::Repeater as i32, String::new())
                                }
                                TrafficSource::Intruder => {
                                    (protocol::SourceDto::Intruder as i32, String::new())
                                }
                                TrafficSource::Scanner => {
                                    (protocol::SourceDto::Scanner as i32, String::new())
                                }
                                TrafficSource::Plugin(id) => {
                                    (protocol::SourceDto::Plugin as i32, id.clone())
                                }
                            };
                            let depth = match plugin_hook_depth(&record.request) {
                                0 => u8::from(source == protocol::SourceDto::Plugin as i32),
                                d => d,
                            };
                            if depth <= 1 {
                                for (hook_plugin_id, identity, handle, filter, counters) in handles {
                                    let resp_ct =
                                        record.response.as_ref().and_then(response_content_type);
                                    if !filter.should_invoke_http(
                                        &record.request.url,
                                        response_stage,
                                        resp_ct,
                                        Some(&scope),
                                    ) {
                                        counters.inc_filtered_skipped();
                                        continue;
                                    }
                                    let Some((call, spill)) = large_body::prepare_http_hook_call(
                                        record.summary.id,
                                        source,
                                        plugin_id.clone(),
                                        response_stage,
                                        response_head_only,
                                        &record.request,
                                        record.response.as_ref(),
                                        handle.protocol_minor(),
                                        handle.supports_large_body(),
                                    ) else {
                                        counters.inc_oversized_bypassed();
                                        continue;
                                    };
                                    counters.inc_invoked();
                                    let res = match handle
                                        .call_http_hook(call, cancel.clone())
                                        .await
                                    {
                                        Ok(res) => res,
                                        Err(e) => {
                                            if cancel.is_cancelled() {
                                                return Err(mio_proxy::ProxyError::Cancelled);
                                            }
                                            counters.inc_error_or_timeout();
                                            let msg = e.to_string();
                                            if filter.failure_policy
                                                == PluginHookFailurePolicy::FailOpen
                                            {
                                                counters.inc_fail_open_pass();
                                                emit_fail_open_diagnostic(
                                                    Some(&events),
                                                    identity,
                                                    if response_stage {
                                                        "Proxy HTTP 响应 Hook"
                                                    } else {
                                                        "Proxy HTTP 请求 Hook"
                                                    },
                                                    &msg,
                                                );
                                                continue;
                                            }
                                            return Err(mio_proxy::ProxyError::InvalidMessage(msg));
                                        }
                                    };
                                    let before_req = record.request.clone();
                                    let before_resp = record.response.clone();
                                    let apply_res: std::result::Result<(), String> = (|| {
                                        match protocol::HookAction::try_from(res.action) {
                                            Ok(protocol::HookAction::Pass) => {}
                                            Ok(protocol::HookAction::Drop) => {
                                                return Err("__DROP__".to_owned());
                                            }
                                            Ok(protocol::HookAction::Intercept) => {
                                                if !response_stage
                                                    && let Some(req) = res.request
                                                {
                                                    record.request = large_body::resolve_request_dto(
                                                        req,
                                                        &before_req.body,
                                                        &spill,
                                                    )?;
                                                    modified = true;
                                                } else if response_stage
                                                    && let Some(resp) = res.response
                                                {
                                                    let orig_resp_body = before_resp
                                                        .as_ref()
                                                        .map_or(&[][..], |r| r.body.as_slice());
                                                    record.response = Some(
                                                        large_body::resolve_response_dto(
                                                            resp,
                                                            orig_resp_body,
                                                            &spill,
                                                        )?,
                                                    );
                                                    modified = true;
                                                }
                                                force_intercept = true;
                                            }
                                            Ok(protocol::HookAction::Modify) => {
                                                if !response_stage {
                                                    let req = res.request.ok_or_else(|| {
                                                        "插件声明修改请求但未返回请求".to_owned()
                                                    })?;
                                                    record.request = large_body::resolve_request_dto(
                                                        req,
                                                        &before_req.body,
                                                        &spill,
                                                    )?;
                                                    modified = true;
                                                } else {
                                                    let resp = res.response.ok_or_else(|| {
                                                        "插件声明修改响应但未返回响应".to_owned()
                                                    })?;
                                                    let orig_resp_body = before_resp
                                                        .as_ref()
                                                        .map_or(&[][..], |r| r.body.as_slice());
                                                    record.response = Some(
                                                        large_body::resolve_response_dto(
                                                            resp,
                                                            orig_resp_body,
                                                            &spill,
                                                        )?,
                                                    );
                                                    modified = true;
                                                }
                                            }
                                            Err(_) => {
                                                return Err("插件返回未知 Hook 动作".to_owned());
                                            }
                                        }
                                        Ok(())
                                    })();
                                    drop(spill);
                                    if let Err(err) = apply_res {
                                        if err == "__DROP__" {
                                            return Ok(mio_proxy::FlowHookDecision::Drop);
                                        }
                                        counters.inc_error_or_timeout();
                                        if filter.failure_policy
                                            == PluginHookFailurePolicy::FailOpen
                                        {
                                            counters.inc_fail_open_pass();
                                            emit_fail_open_diagnostic(
                                                Some(&events),
                                                identity,
                                                if response_stage {
                                                    "Proxy HTTP 响应 Hook"
                                                } else {
                                                    "Proxy HTTP 请求 Hook"
                                                },
                                                &err,
                                            );
                                            continue;
                                        }
                                        return Err(mio_proxy::ProxyError::InvalidMessage(err));
                                    }
                                    let actor = ProvenanceActor::PluginHook {
                                        plugin_id: hook_plugin_id,
                                        hook: "http_hook".into(),
                                    };
                                    if !response_stage && record.request != before_req {
                                        provenance.record_request(
                                            record.summary.id,
                                            &initial_req,
                                            actor,
                                            &record.request,
                                        );
                                    } else if response_stage
                                        && record.response != before_resp
                                        && let (Some(orig), Some(cur)) =
                                            (&initial_resp, &record.response)
                                    {
                                        provenance.record_response(
                                            record.summary.id,
                                            orig,
                                            actor,
                                            cur,
                                        );
                                    }
                                }
                            }
                            Ok(if force_intercept {
                                mio_proxy::FlowHookDecision::ForceIntercept {
                                    request: (!response_stage && modified)
                                        .then_some(record.request),
                                    response: (response_stage && modified)
                                        .then_some(record.response)
                                        .flatten(),
                                }
                            } else if !modified {
                                mio_proxy::FlowHookDecision::Pass
                            } else if !response_stage {
                                mio_proxy::FlowHookDecision::ModifyRequest(record.request)
                            } else {
                                mio_proxy::FlowHookDecision::ModifyResponse(
                                    record.response.expect("response present"),
                                )
                            })
                        })
                    },
                )
            });

        let ws_scope = scope;
        let ws_events = plugin_events;
        let ws_hook =
            (!ws_handles.is_empty()).then(|| -> mio_proxy::AsyncWebSocketHook {
                Arc::new(
                    move |flow_id, msg_id, url, direction, kind, mut payload, cancel, _dl| {
                        let handles = ws_handles.clone();
                        let scope = ws_scope.clone();
                        let events = ws_events.clone();
                        Box::pin(async move {
                            let mut modified = false;
                            let mut force_intercept = false;
                            for (identity, handle, filter, counters) in handles {
                                if !filter.should_invoke_ws(&url, Some(&scope)) {
                                    counters.inc_filtered_skipped();
                                    continue;
                                }
                                let Some((call, spill)) = large_body::prepare_ws_hook_call(
                                    flow_id,
                                    msg_id,
                                    &url,
                                    direction,
                                    kind,
                                    &payload,
                                    handle.protocol_minor(),
                                    handle.supports_large_body(),
                                ) else {
                                    counters.inc_oversized_bypassed();
                                    continue;
                                };
                                counters.inc_invoked();
                                let res = match handle.call_ws_hook(call, cancel.clone()).await {
                                    Ok(res) => res,
                                    Err(e) => {
                                        if cancel.is_cancelled() {
                                            return Err(mio_proxy::ProxyError::Cancelled);
                                        }
                                        counters.inc_error_or_timeout();
                                        let msg = e.to_string();
                                        if filter.failure_policy
                                            == PluginHookFailurePolicy::FailOpen
                                        {
                                            counters.inc_fail_open_pass();
                                            emit_fail_open_diagnostic(
                                                Some(&events),
                                                identity,
                                                "Proxy WebSocket Hook",
                                                &msg,
                                            );
                                            continue;
                                        }
                                        return Err(mio_proxy::ProxyError::InvalidMessage(msg));
                                    }
                                };
                                let apply_ws: std::result::Result<(), String> = (|| {
                                    match protocol::HookAction::try_from(res.action) {
                                        Ok(protocol::HookAction::Pass) => {}
                                        Ok(protocol::HookAction::Drop) => {
                                            return Err("__DROP__".to_owned());
                                        }
                                        Ok(protocol::HookAction::Intercept) => {
                                            if res.payload_omitted || !res.payload.is_empty() {
                                                let resolved = large_body::resolve_ws_result(
                                                    res,
                                                    &payload,
                                                    &spill,
                                                )?;
                                                if resolved != payload {
                                                    payload = resolved;
                                                    modified = true;
                                                }
                                            }
                                            force_intercept = true;
                                        }
                                        Ok(protocol::HookAction::Modify) => {
                                            payload = large_body::resolve_ws_result(
                                                res,
                                                &payload,
                                                &spill,
                                            )?;
                                            modified = true;
                                        }
                                        Err(_) => {
                                            return Err("插件返回未知 WS Hook 动作".into());
                                        }
                                    }
                                    Ok(())
                                })();
                                drop(spill);
                                if let Err(err) = apply_ws {
                                    if err == "__DROP__" {
                                        return Ok(mio_proxy::WebSocketHookDecision::Drop);
                                    }
                                    counters.inc_error_or_timeout();
                                    if filter.failure_policy
                                        == PluginHookFailurePolicy::FailOpen
                                    {
                                        counters.inc_fail_open_pass();
                                        emit_fail_open_diagnostic(
                                            Some(&events),
                                            identity,
                                            "Proxy WebSocket Hook",
                                            &err,
                                        );
                                        continue;
                                    }
                                    return Err(mio_proxy::ProxyError::InvalidMessage(err));
                                }
                            }
                            Ok(if force_intercept {
                                mio_proxy::WebSocketHookDecision::ForceIntercept(
                                    modified.then_some(payload),
                                )
                            } else if modified {
                                mio_proxy::WebSocketHookDecision::Modify(payload)
                            } else {
                                mio_proxy::WebSocketHookDecision::Pass
                            })
                        })
                    },
                )
            });
        (flow_hook, ws_hook)
    }

    pub(crate) fn sync_proxy_plugin_hooks(&self) {
        let (flow_hook, ws_hook) = self.build_proxy_plugin_hooks();
        for proxy in self.proxy.handles() {
            proxy.set_plugin_hooks(flow_hook.clone(), ws_hook.clone());
        }
    }

    pub(crate) fn sync_sessions_workspace(&self, workspace: &Workspace) {
        self.sessions.runtime.sync_workspace(self.project, workspace);
        if let Ok(config) = self.build_sessions_send_config(workspace) {
            self.sessions.runtime.set_proxy_send_config(config);
        }
        self.sync_proxy_plugin_hooks();
    }
}
