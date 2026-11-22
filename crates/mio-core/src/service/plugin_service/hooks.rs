use super::*;

mod pre_send;
#[cfg(test)]
mod tests;

pub(in crate::service) use pre_send::{
    PreSendHookOutcome, plugin_hook_depth, run_pre_send_plugin_hooks,
    run_pre_send_plugin_hooks_with_provenance, sync_flow_request,
};

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

    pub(in crate::service) fn strict_session_action_hooks(
        &self,
    ) -> Vec<crate::plugins::PluginHandle> {
        self.strict_session_action_hooks_named()
            .into_iter()
            .map(|(_, h)| h)
            .collect()
    }

    pub(in crate::service) fn strict_session_action_hooks_named(
        &self,
    ) -> Vec<(String, crate::plugins::PluginHandle)> {
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
            .map(|inst| (inst.info.manifest.id.clone(), inst.worker.handle()))
            .collect()
    }

    pub(in crate::service) fn tool_http_hooks(&self) -> Vec<crate::plugins::PluginHandle> {
        self.tool_http_hooks_named()
            .into_iter()
            .map(|(_, h)| h)
            .collect()
    }

    pub(in crate::service) fn tool_http_hooks_named(
        &self,
    ) -> Vec<(String, crate::plugins::PluginHandle)> {
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
            .map(|inst| (inst.info.manifest.id.clone(), inst.worker.handle()))
            .collect()
    }

    pub(in crate::service) fn passive_check_hooks(&self) -> Vec<crate::plugins::PluginHandle> {
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
            .map(|inst| inst.worker.handle())
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
        let http_handles: Vec<(String, crate::plugins::PluginHandle)> = ordered
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
            .map(|inst| (inst.info.manifest.id.clone(), inst.worker.handle()))
            .collect();
        let ws_handles: Vec<crate::plugins::PluginHandle> = ordered
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
            .map(|inst| inst.worker.handle())
            .collect();

        let has_session_hooks = self.sessions.runtime.has_proxy_hooks();
        let sessions = self.sessions.runtime.clone();
        let provenance = self.storage.provenance.clone();
        let flow_hook =
            (!http_handles.is_empty() || has_session_hooks).then(|| -> mio_proxy::AsyncFlowHook {
                Arc::new(
                    move |mut record, response_stage, response_head_only, cancel, deadline| {
                        let handles = http_handles.clone();
                        let sessions = sessions.clone();
                        let provenance = provenance.clone();
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
                            if depth > 1 {
                                return Ok(mio_proxy::FlowHookDecision::Pass);
                            }
                            for (hook_plugin_id, handle) in handles {
                                let call = protocol::HttpHookCall {
                                    flow_id: record.summary.id.as_bytes().to_vec(),
                                    source,
                                    plugin_id: plugin_id.clone(),
                                    response_stage,
                                    response_head_only,
                                    request: Some(record.request.clone().into()),
                                    response: record.response.clone().map(Into::into),
                                };
                                let res =
                                    handle.call_http_hook(call, cancel.clone()).await.map_err(
                                        |e| mio_proxy::ProxyError::InvalidMessage(e.to_string()),
                                    )?;
                                let before_req = record.request.clone();
                                let before_resp = record.response.clone();
                                match protocol::HookAction::try_from(res.action) {
                                    Ok(protocol::HookAction::Pass) => {}
                                    Ok(protocol::HookAction::Drop) => {
                                        return Ok(mio_proxy::FlowHookDecision::Drop);
                                    }
                                    Ok(protocol::HookAction::Intercept) => {
                                        if !response_stage && let Some(req) = res.request {
                                            record.request = req
                                                .try_into()
                                                .map_err(mio_proxy::ProxyError::InvalidMessage)?;
                                            modified = true;
                                        } else if response_stage && let Some(resp) = res.response {
                                            record.response =
                                                Some(resp.try_into().map_err(
                                                    mio_proxy::ProxyError::InvalidMessage,
                                                )?);
                                            modified = true;
                                        }
                                        force_intercept = true;
                                    }
                                    Ok(protocol::HookAction::Modify) => {
                                        if !response_stage {
                                            let req = res.request.ok_or_else(|| {
                                                mio_proxy::ProxyError::InvalidMessage(
                                                    "插件声明修改请求但未返回请求".into(),
                                                )
                                            })?;
                                            record.request = req
                                                .try_into()
                                                .map_err(mio_proxy::ProxyError::InvalidMessage)?;
                                            modified = true;
                                        } else {
                                            let resp = res.response.ok_or_else(|| {
                                                mio_proxy::ProxyError::InvalidMessage(
                                                    "插件声明修改响应但未返回响应".into(),
                                                )
                                            })?;
                                            record.response =
                                                Some(resp.try_into().map_err(
                                                    mio_proxy::ProxyError::InvalidMessage,
                                                )?);
                                            modified = true;
                                        }
                                    }
                                    Err(_) => {
                                        return Err(mio_proxy::ProxyError::InvalidMessage(
                                            "插件返回未知 Hook 动作".into(),
                                        ));
                                    }
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
                                    provenance.record_response(record.summary.id, orig, actor, cur);
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

        let ws_hook =
            (!ws_handles.is_empty()).then(|| -> mio_proxy::AsyncWebSocketHook {
                Arc::new(
                    move |flow_id, msg_id, url, direction, kind, mut payload, cancel, _dl| {
                        let handles = ws_handles.clone();
                        Box::pin(async move {
                            let mut modified = false;
                            let mut force_intercept = false;
                            for handle in handles {
                                let call = protocol::WebSocketHookCall {
                                    flow_id: flow_id.as_bytes().to_vec(),
                                    message_id: msg_id.as_bytes().to_vec(),
                                    url: url.clone(),
                                    client_to_server: direction
                                        == mio_plugin_api::WebSocketDirection::ClientToServer,
                                    is_binary: kind == mio_plugin_api::WebSocketKind::Binary,
                                    payload: payload.clone(),
                                };
                                let res = handle.call_ws_hook(call, cancel.clone()).await.map_err(
                                    |e| mio_proxy::ProxyError::InvalidMessage(e.to_string()),
                                )?;
                                match protocol::HookAction::try_from(res.action) {
                                    Ok(protocol::HookAction::Pass) => {}
                                    Ok(protocol::HookAction::Drop) => {
                                        return Ok(mio_proxy::WebSocketHookDecision::Drop);
                                    }
                                    Ok(protocol::HookAction::Intercept) => {
                                        if !res.payload.is_empty() && res.payload != payload {
                                            payload = res.payload;
                                            modified = true;
                                        }
                                        force_intercept = true;
                                    }
                                    Ok(protocol::HookAction::Modify) => {
                                        payload = res.payload;
                                        modified = true;
                                    }
                                    Err(_) => {
                                        return Err(mio_proxy::ProxyError::InvalidMessage(
                                            "插件返回未知 WS Hook 动作".into(),
                                        ));
                                    }
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
        if let Ok(upstream_tls) = self
            .preferences
            .effective_upstream_tls(&workspace.upstream_tls)
            && let Ok(upstream_route) = workspace
                .upstream_proxy
                .to_route_with_tls(upstream_tls.clone())
            && let Ok(hostname_overrides) = workspace.upstream_proxy.compiled_hostname_overrides()
        {
            self.sessions.runtime.set_proxy_send_config(SendConfig {
                network: mio_proxy::SendNetworkConfig {
                    upstream_route,
                    hostname_overrides,
                },
                tls: mio_proxy::UpstreamTlsConfig {
                    trust: upstream_tls,
                    options: workspace.upstream_tls.to_proxy_tls_options(),
                    client_certificates: self.tls.compiled_client_certs.clone(),
                    ..Default::default()
                },
                limits: mio_proxy::SendLimits {
                    timeout: Duration::from_secs(
                        workspace
                            .repeater_settings
                            .request_timeout_secs
                            .clamp(1, 3600),
                    ),
                    streaming_timeout: workspace.upstream_proxy.effective_streaming_timeout(),
                    ..Default::default()
                },
                ..SendConfig::default()
            });
        }
        self.sync_proxy_plugin_hooks();
    }
}
