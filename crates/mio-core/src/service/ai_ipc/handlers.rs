use std::{collections::HashMap, path::Path, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};

use super::{
    AiIpcCall, AiIpcEnvelope, AiIpcFlowDetail, AiIpcPayload, AiIpcPendingItem, AiIpcRequest,
    IpcRepeaterCompletion, IpcRepeaterOutcome, MAX_CONCURRENT_IPC_CLIENTS,
};
use crate::{
    Command, Decision, Event, FlowId, FlowRecord, FlowState, FlowSummary, Header, HttpRequest,
    HttpVersion, InterceptStage, ProvenanceActor, SessionTool, SlimSaveOptions, TrafficSource,
    WorkTab, Workspace, authority, detect_request_line_version, parse_repeater_request,
    request_to_raw, response_to_raw,
    service::{
        State,
        core_repeater_send::{
            RepeaterSendRuntimeConfig, apply_repeater_protocol_options,
            apply_repeater_session_rules, pending_repeater_flow_record, spawn_unpack_response,
        },
    },
};

impl State {
    pub(in crate::service) async fn stop_ipc_jobs(&mut self) {
        self.ai_ipc.job_cancel.cancel();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while !self.ai_ipc.jobs.is_empty() {
            match tokio::time::timeout_at(deadline, self.ai_ipc.jobs.join_next()).await {
                Ok(Some(done)) => {
                    if let Ok(done) = done {
                        let _ = done
                            .reply
                            .send(AiIpcEnvelope::err("服务正在停止或切换项目，请求已取消"));
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    self.ai_ipc.jobs.abort_all();
                    while self.ai_ipc.jobs.join_next().await.is_some() {}
                    break;
                }
            }
        }
        self.ai_ipc.job_cancel = self.cancel.child_token();
    }

    pub(in crate::service) async fn accept_ai_ipc_call(&mut self, call: AiIpcCall) {
        if let AiIpcRequest::RepeaterSend {
            flow_id,
            use_original,
            target,
            raw_request,
            add_headers,
            remove_headers,
            override_body,
            insecure,
        } = call.request
        {
            let reply = call.reply;
            if let Err(error) = self.spawn_ipc_repeater_send(
                reply,
                flow_id,
                use_original,
                target,
                raw_request,
                add_headers,
                remove_headers,
                override_body,
                insecure,
            ) {
                let (reply, err) = error;
                let _ = reply.send(AiIpcEnvelope::err(format!("{err:#}")));
            }
            return;
        }
        let response = self.handle_ai_ipc(call.request).await;
        let _ = call.reply.send(response);
    }

    pub(in crate::service) fn complete_ipc_repeater(
        &mut self,
        done: std::result::Result<IpcRepeaterCompletion, tokio::task::JoinError>,
    ) {
        let Ok(done) = done else {
            return;
        };
        if done.project != self.project {
            let _ = done
                .reply
                .send(AiIpcEnvelope::err("项目已切换，IPC Repeater 请求已取消"));
            return;
        }
        match done.outcome {
            Ok(IpcRepeaterOutcome {
                initial_pending,
                mutated_pending,
                received,
                record,
                original_request,
                original_response,
                provenance,
            }) => {
                let saved = if let Some(initial) = &initial_pending {
                    self.persist(initial)
                        .and_then(|()| match &mutated_pending {
                            Some(mutated) => self.persist(mutated),
                            None => Ok(()),
                        })
                        .and_then(|()| self.persist_repeater_completion(received.as_ref(), &record))
                } else {
                    self.persist_repeater_completion(received.as_ref(), &record)
                };
                if let Err(error) = saved {
                    let _ = done
                        .reply
                        .send(AiIpcEnvelope::err(format!("保存 Repeater 请求记录失败：{error:#}")));
                    return;
                }
                if let Some(event) = self.audit_inspect_live_flow(&record) {
                    self.queue_repeater_event(Event::Audit(event));
                }
                self.spawn_plugin_passive_checks(&record);
                if !self.storage.history_dirty {
                    self.storage.history_dirty = true;
                    self.queue_repeater_event(Event::HistoryChanged {
                        project: self.project,
                    });
                }
                self.refresh_project_disk_bytes_throttled();
                let request_raw_text =
                    Some(String::from_utf8_lossy(&request_to_raw(&record.request)).into_owned());
                let response_raw_text = record
                    .response
                    .as_ref()
                    .map(|r| String::from_utf8_lossy(&response_to_raw(r)).into_owned());
                let _ = done
                    .reply
                    .send(AiIpcEnvelope::ok(AiIpcPayload::FlowsGet(Box::new(
                        AiIpcFlowDetail {
                            summary: record.summary,
                            request_raw_text,
                            response_raw_text,
                            request: record.request,
                            response: record.response,
                            original_request,
                            original_response,
                            provenance,
                        },
                    ))));
            }
            Err(error) => {
                let _ = done.reply.send(AiIpcEnvelope::err(format!("{error:#}")));
            }
        }
    }

    pub(in crate::service) async fn handle_ai_ipc(&mut self, req: AiIpcRequest) -> AiIpcEnvelope {
        match self.dispatch_ai_ipc(req).await {
            Ok(payload) => AiIpcEnvelope::ok(payload),
            Err(error) => AiIpcEnvelope::err(format!("{error:#}")),
        }
    }

    async fn dispatch_ai_ipc(&mut self, req: AiIpcRequest) -> Result<AiIpcPayload> {
        match req {
            AiIpcRequest::Status => self.ipc_status(),
            AiIpcRequest::FlowsList {
                offset,
                limit,
                filter,
                method,
                status,
            } => self.ipc_flows_list(offset, limit, &filter, method.as_deref(), status),
            AiIpcRequest::FlowsGet { flow_id } => self.ipc_flows_get(flow_id),
            AiIpcRequest::RepeaterOpen {
                flow_id,
                use_original,
                target,
                raw_request,
                title,
            } => {
                self.ipc_repeater_open(flow_id, use_original, target, raw_request, title)
                    .await
            }
            AiIpcRequest::RepeaterSend { .. } => {
                unreachable!("RepeaterSend is already handled in accept_ai_ipc_call")
            }
            AiIpcRequest::SlimSave { path, options } => self.ipc_slim_save(&path, options).await,
            AiIpcRequest::ExportCa { path, pem } => self.ipc_export_ca(&path, pem).await,
            AiIpcRequest::ProxyStart { bind_address } => self.ipc_proxy_start(bind_address).await,
            AiIpcRequest::ProxyStop => self.ipc_proxy_stop().await,
            AiIpcRequest::ProxyIntercept {
                requests,
                responses,
            } => self.ipc_proxy_intercept(requests, responses).await,
            AiIpcRequest::ProxyPending => Ok(self.ipc_proxy_pending()),
            AiIpcRequest::ProxyDecide {
                flow_id,
                revision,
                drop_flow,
                add_headers,
                remove_headers,
                override_body,
                override_status,
            } => {
                self.ipc_proxy_decide(
                    flow_id,
                    revision,
                    drop_flow,
                    add_headers,
                    remove_headers,
                    override_body,
                    override_status,
                )
                .await
            }
            AiIpcRequest::WorkspaceGet => {
                let ws = self.store.metadata().workspace()?;
                Ok(AiIpcPayload::WorkspaceState {
                    workspace: serde_json::to_value(&ws)?,
                })
            }
            AiIpcRequest::WorkspacePatch { workspace_json } => {
                let workspace: Workspace =
                    serde_json::from_str(&workspace_json).context("解析工作区 JSON 失败")?;
                ensure!(
                    !self.quiesced,
                    "项目正在切换或静默保存，暂无法修改工作区配置"
                );
                self.apply_save_workspace(self.project, workspace).await?;
                let saved = self.store.metadata().workspace()?;
                Ok(AiIpcPayload::WorkspaceState {
                    workspace: serde_json::to_value(&saved)?,
                })
            }
            AiIpcRequest::WebSocketList { limit } => self.ipc_websocket_list(limit),
            AiIpcRequest::Shutdown => self.ipc_shutdown(),
        }
    }

    fn latest_recovery_summaries(&self) -> HashMap<FlowId, FlowSummary> {
        let mut map = HashMap::new();
        for capture in &self.storage.recovery {
            map.insert(capture.record.summary.id, capture.record.summary.clone());
        }
        map
    }

    fn ipc_status(&mut self) -> Result<AiIpcPayload> {
        let mut total_flows = self.store.flows().flow_count()?;
        let recovery = self.latest_recovery_summaries();
        for &id in recovery.keys() {
            if !self.store.flows().has_flow(id)? {
                total_flows = total_flows.saturating_add(1);
            }
        }
        let project_disk_bytes = match self.store.total_bytes() {
            Ok(bytes) => {
                self.storage.project_disk_bytes = bytes;
                self.storage.last_disk_refresh = Some(std::time::Instant::now());
                if self.events.capacity() > 4 {
                    let _ = self.events.try_send(Event::ProjectDiskUsage {
                        project: self.project,
                        bytes,
                    });
                }
                bytes
            }
            Err(_) => self.storage.project_disk_bytes,
        };
        let proxy_address = self.proxy.primary_address().map(|addr| addr.to_string());
        Ok(AiIpcPayload::Status {
            project_id: self.project.id,
            project_generation: self.project.generation,
            proxy_address,
            intercept_requests: self.proxy.intercept_requests,
            intercept_responses: self.proxy.intercept_responses,
            total_flows,
            project_disk_bytes,
            process_memory_bytes: crate::current_process_memory_bytes(),
        })
    }

    fn ipc_flows_list(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
        method: Option<&str>,
        status: Option<u16>,
    ) -> Result<AiIpcPayload> {
        let clamped_limit = limit.clamp(1, 500);
        let recovery = self.latest_recovery_summaries();
        let (total, flows) =
            self.store
                .flows()
                .query_ai_flows(offset, clamped_limit, filter, method, status, &recovery)?;
        Ok(AiIpcPayload::FlowsList { total, flows })
    }

    fn ipc_flows_get(&self, flow_id: FlowId) -> Result<AiIpcPayload> {
        let record = self.read_flow(flow_id)?;
        let original = self.original_flow(flow_id)?;
        let request_raw_text =
            Some(String::from_utf8_lossy(&request_to_raw(&record.request)).into_owned());
        let response_raw_text = record
            .response
            .as_ref()
            .map(|r| String::from_utf8_lossy(&response_to_raw(r)).into_owned());
        Ok(AiIpcPayload::FlowsGet(Box::new(AiIpcFlowDetail {
            summary: record.summary,
            request_raw_text,
            response_raw_text,
            request: record.request,
            response: record.response,
            original_request: original.request,
            original_response: original.response,
            provenance: original.provenance,
        })))
    }

    async fn ipc_repeater_open(
        &mut self,
        flow_id: Option<FlowId>,
        use_original: bool,
        target: Option<String>,
        raw_request: Option<String>,
        title: Option<String>,
    ) -> Result<AiIpcPayload> {
        ensure!(
            !self.quiesced,
            "项目正在切换或静默保存，暂无法打开 Repeater 标签"
        );
        let request = resolve_repeater_open_request(
            self,
            flow_id,
            use_original,
            target.as_deref(),
            raw_request.as_deref(),
        )?;
        let mut workspace = self.store.metadata().workspace()?;
        let resolved_title = match title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
            Some(custom) => custom,
            None => next_repeater_tab_title(&workspace, &request),
        };
        let tab = WorkTab::from_request(Some(&request), resolved_title);
        if self.unsaved_count() == 0 {
            let next_rev = workspace.revision.saturating_add(1);
            workspace.revision = next_rev;
            workspace.active = Some(tab.id);
            workspace.active_websocket_tab = None;
            workspace.tabs.push(tab.clone());
            if self.store.metadata_mut().save_workspace(&workspace).is_ok() {
                self.ai_ipc.repeater_revisions.insert((tab.id, next_rev));
            }
        }
        let tab_id = tab.id;
        let tab_title = tab.title.clone();
        let tab_target = tab.draft.target.clone();
        self.emit(Event::RepeaterOpened {
            project: self.project,
            tab,
        })
        .await;
        Ok(AiIpcPayload::RepeaterOpen {
            tab_id,
            title: tab_title,
            target: tab_target,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_ipc_repeater_send(
        &mut self,
        reply: tokio::sync::oneshot::Sender<AiIpcEnvelope>,
        flow_id: Option<FlowId>,
        use_original: bool,
        target: Option<String>,
        raw_request: Option<String>,
        add_headers: Vec<(String, String)>,
        remove_headers: Vec<String>,
        override_body: Option<String>,
        insecure: bool,
    ) -> std::result::Result<(), (tokio::sync::oneshot::Sender<AiIpcEnvelope>, anyhow::Error)> {
        let prepared = (|| -> Result<_> {
            self.require_saved()?;
            ensure!(
                !self.quiesced,
                "项目正在切换或静默保存，暂无法执行 Repeater 发送"
            );
            ensure!(
                self.ai_ipc.jobs.len() < MAX_CONCURRENT_IPC_CLIENTS,
                "并发 IPC Repeater 请求已达上限"
            );
            let mut request = resolve_repeater_open_request(
                self,
                flow_id,
                use_original,
                target.as_deref(),
                raw_request.as_deref(),
            )?;
            let base_request = request.clone();
            apply_header_and_body_overrides(
                &mut request.headers,
                &mut request.body,
                remove_headers,
                add_headers,
                override_body,
            );
            let workspace = self.store.metadata().workspace_arc()?;
            let settings = workspace.repeater_settings.clone();
            let runtime_cfg = self.build_repeater_send_runtime_config(
                &workspace,
                &settings,
                300,
                insecure,
                false,
                false,
            )?;
            let started_unix_ms = crate::codec_util::now_unix_ms();
            Ok((
                base_request,
                request,
                settings,
                runtime_cfg,
                started_unix_ms,
            ))
        })();
        let (base_request, request, settings, runtime_cfg, started_unix_ms) = match prepared {
            Ok(val) => val,
            Err(err) => return Err((reply, err)),
        };
        let project = self.project;
        let sessions = self.sessions.runtime.clone();
        let provenance = self.storage.provenance.clone();
        self.ai_ipc.refresh_cancel_if_idle(&self.cancel);
        let cancel = self.ai_ipc.job_cancel.child_token();
        self.ai_ipc.jobs.spawn(async move {
            let outcome = execute_ipc_repeater_send(
                base_request,
                request,
                settings,
                runtime_cfg,
                started_unix_ms,
                sessions,
                provenance,
                cancel,
            )
            .await;
            IpcRepeaterCompletion {
                project,
                reply,
                outcome,
            }
        });
        Ok(())
    }

    async fn ipc_slim_save(
        &mut self,
        path: &str,
        options: Option<SlimSaveOptions>,
    ) -> Result<AiIpcPayload> {
        let trimmed = path.trim();
        ensure!(!trimmed.is_empty(), "瘦身保存目标路径不能为空");
        let dest = PathBuf::from(trimmed);
        let opts = options.unwrap_or_default();
        let pending_prov = self.storage.provenance.snapshot_all();
        let report = self.store.slim_save_as(
            Path::new(&dest),
            &opts,
            self.storage.recovery.make_contiguous(),
            self.storage.websocket_recovery.make_contiguous(),
            &self.intruder.recovery,
            &pending_prov,
            &self.cancel,
        )?;
        self.emit(Event::ProjectSlimSaved {
            project: self.project,
            path: dest,
            result: Ok(report.clone()),
        })
        .await;
        Ok(AiIpcPayload::SlimSave { report })
    }

    async fn ipc_export_ca(&mut self, path: &str, pem: Option<bool>) -> Result<AiIpcPayload> {
        let trimmed = path.trim();
        ensure!(!trimmed.is_empty(), "导出 CA 目标路径不能为空");
        let dest = PathBuf::from(trimmed);
        let use_pem = pem.unwrap_or_else(|| {
            dest.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("crt")
            })
        });
        let ca = self.authority().await?;
        let der = ca.certificate_der();
        let subject = mio_proxy::UpstreamTls::certificate_display_name(&der)
            .unwrap_or_else(|_| "MIO local CA".into());
        let bytes = if use_pem {
            ca.certificate_pem().into_bytes()
        } else {
            der
        };
        let output = dest.clone();
        tokio::task::spawn_blocking(move || authority::export(&bytes, &output)).await??;
        self.emit(Event::CaExported { path: dest.clone() }).await;
        Ok(AiIpcPayload::CaExported {
            ca_path: dest.to_string_lossy().into_owned(),
            ca_format: if use_pem { "pem".into() } else { "der".into() },
            ca_subject: subject,
        })
    }

    async fn ipc_proxy_start(&mut self, bind_address: Option<String>) -> Result<AiIpcPayload> {
        let explicit_override: Option<std::net::SocketAddr> =
            match bind_address.as_deref().map(str::trim) {
                Some(addr) if !addr.is_empty() => Some(
                    addr.parse()
                        .context("无效的监听 SocketAddr（例如 127.0.0.1:8080）")?,
                ),
                _ => None,
            };
        let listen = explicit_override
            .unwrap_or_else(|| "127.0.0.1:8080".parse().expect("valid default loopback"));
        if self.proxy.is_running() {
            self.stop_proxy().await?;
        }
        self.start_proxy_with_override(self.project, listen, explicit_override)
            .await?;
        Ok(AiIpcPayload::ProxyControl {
            proxy_running: self.proxy.primary.is_some(),
            proxy_address: self.proxy.primary_address().map(|addr| addr.to_string()),
            intercept_requests: self.proxy.intercept_requests,
            intercept_responses: self.proxy.intercept_responses,
        })
    }

    async fn ipc_proxy_stop(&mut self) -> Result<AiIpcPayload> {
        self.command_proxy_intercept(Command::StopProxy {
            project: self.project,
        })
        .await?;
        Ok(AiIpcPayload::ProxyControl {
            proxy_running: self.proxy.primary.is_some(),
            proxy_address: self.proxy.primary_address().map(|addr| addr.to_string()),
            intercept_requests: self.proxy.intercept_requests,
            intercept_responses: self.proxy.intercept_responses,
        })
    }

    async fn ipc_proxy_intercept(
        &mut self,
        requests: Option<bool>,
        responses: Option<bool>,
    ) -> Result<AiIpcPayload> {
        if requests.is_some() || responses.is_some() {
            let reqs = requests.unwrap_or(self.proxy.intercept_requests);
            let resps = responses.unwrap_or(self.proxy.intercept_responses);
            self.command_proxy_intercept(Command::SetIntercept {
                project: self.project,
                requests: reqs,
                responses: resps,
            })
            .await?;
        }
        Ok(AiIpcPayload::ProxyControl {
            proxy_running: self.proxy.primary.is_some(),
            proxy_address: self.proxy.primary_address().map(|addr| addr.to_string()),
            intercept_requests: self.proxy.intercept_requests,
            intercept_responses: self.proxy.intercept_responses,
        })
    }

    fn ipc_proxy_pending(&self) -> AiIpcPayload {
        let mut items: Vec<AiIpcPendingItem> = self
            .proxy
            .pending
            .iter()
            .filter(|(_, p)| !p.reply.is_closed())
            .map(|(&flow_id, p)| AiIpcPendingItem {
                flow_id,
                revision: p.revision,
                stage: match p.stage {
                    InterceptStage::Request => "request".into(),
                    InterceptStage::Response => "response".into(),
                },
                response_head_only: p.response_head_only,
                method: p.intercepted_request.method.clone(),
                url: p.intercepted_request.url.clone(),
                status: p.intercepted_response.as_ref().map(|r| r.status),
            })
            .collect();
        items.sort_by_key(|i| i.flow_id);
        AiIpcPayload::ProxyPending {
            pending_items: items,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn ipc_proxy_decide(
        &mut self,
        flow_id: FlowId,
        revision: u64,
        drop_flow: bool,
        add_headers: Vec<(String, String)>,
        remove_headers: Vec<String>,
        override_body: Option<String>,
        override_status: Option<u16>,
    ) -> Result<AiIpcPayload> {
        let decision = if drop_flow {
            Decision::Drop
        } else {
            let pending = self
                .proxy
                .pending
                .get(&flow_id)
                .context("拦截流量不存在或已结束")?;
            match pending.stage {
                InterceptStage::Request => {
                    let mut req = pending.intercepted_request.clone();
                    apply_header_and_body_overrides(
                        &mut req.headers,
                        &mut req.body,
                        remove_headers,
                        add_headers,
                        override_body,
                    );
                    Decision::ForwardRequest(req)
                }
                InterceptStage::Response => {
                    let mut resp = pending
                        .intercepted_response
                        .clone()
                        .context("响应阶段缺少被拦截响应报文")?;
                    if let Some(code) = override_status {
                        resp.status = code;
                    }
                    apply_header_and_body_overrides(
                        &mut resp.headers,
                        &mut resp.body,
                        remove_headers,
                        add_headers,
                        override_body.filter(|_| !pending.response_head_only),
                    );
                    Decision::ForwardResponse(resp)
                }
            }
        };
        self.command_proxy_intercept(Command::Decide {
            project: self.project,
            flow_id,
            revision,
            decision,
        })
        .await?;
        Ok(AiIpcPayload::ProxyDecided {
            decided_flow_id: flow_id,
            dropped: drop_flow,
        })
    }

    fn ipc_websocket_list(&self, limit: usize) -> Result<AiIpcPayload> {
        let (mut total, mut messages) = self.store.websocket().list_messages(limit)?;
        for cap in self.storage.websocket_recovery.iter().rev() {
            if !messages.iter().any(|m| m.id == cap.record.id) {
                total = total.saturating_add(1);
                if messages.len() < limit.clamp(1, 500) {
                    messages.insert(0, cap.record.as_ref().clone());
                }
            }
        }
        let ws_messages_text = messages
            .iter()
            .map(|m| String::from_utf8_lossy(&m.payload).into_owned())
            .collect();
        Ok(AiIpcPayload::WebSocketList {
            ws_total: total,
            ws_messages: messages,
            ws_messages_text,
        })
    }

    fn ipc_shutdown(&mut self) -> Result<AiIpcPayload> {
        self.cancel.cancel();
        Ok(AiIpcPayload::Shutdown {
            shutting_down: true,
        })
    }
}

fn resolve_repeater_open_request(
    state: &State,
    flow_id: Option<FlowId>,
    use_original: bool,
    target: Option<&str>,
    raw_request: Option<&str>,
) -> Result<HttpRequest> {
    match (flow_id, target, raw_request) {
        (Some(id), None, None) => {
            let record = state.read_flow(id)?;
            if use_original && let Some(orig_req) = state.original_flow(id)?.request {
                Ok(orig_req)
            } else {
                Ok(record.request)
            }
        }
        (None, Some(origin), Some(raw)) => {
            let origin = origin.trim();
            ensure!(!origin.is_empty(), "Repeater 目标 origin 不能为空");
            let normalized_raw = ensure_header_terminator(raw.as_bytes());
            let version =
                detect_request_line_version(&normalized_raw).unwrap_or(HttpVersion::Http11);
            parse_repeater_request(&normalized_raw, origin, version, true)
                .context("解析自定义 HTTP 请求报文失败")
        }
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            bail!("不能同时指定 flow_id 与自定义 target/raw_request")
        }
        _ => bail!("必须指定 flow_id，或同时提供 target 与 raw_request"),
    }
}

fn ensure_header_terminator(raw: &[u8]) -> Vec<u8> {
    if raw.windows(4).any(|w| w == b"\r\n\r\n") || raw.windows(2).any(|w| w == b"\n\n") {
        return raw.to_vec();
    }
    let mut out = raw.to_vec();
    if out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    } else if out.ends_with(b"\n") {
        out.push(b'\n');
    } else {
        out.extend_from_slice(b"\r\n\r\n");
    }
    out
}

fn next_repeater_tab_title(workspace: &Workspace, request: &HttpRequest) -> String {
    let mut number = 1usize;
    while workspace
        .tabs
        .iter()
        .chain(&workspace.closed)
        .any(|existing| {
            existing.title == number.to_string()
                || existing.title.starts_with(&format!("{number}: "))
        })
    {
        number += 1;
    }
    let path = concise_request_path(&request.url);
    format!("{number}: {path}")
}

fn concise_request_path(url: &str) -> String {
    let trimmed = url.trim();
    let raw_path = if let Some((_, tail)) = trimmed.split_once("://") {
        match tail.find(['/', '?', '#']) {
            Some(idx) if tail.as_bytes()[idx] == b'/' => &tail[idx..],
            _ => "/",
        }
    } else if trimmed.starts_with('/') {
        trimmed
    } else {
        "/"
    };
    let without_query = raw_path.split(['?', '#']).next().unwrap_or("/").trim();
    let clean = if without_query.is_empty() {
        "/"
    } else {
        without_query
    };
    if clean.chars().count() <= 28 {
        clean.to_owned()
    } else {
        let head: String = clean.chars().take(27).collect();
        format!("{head}…")
    }
}

fn apply_header_and_body_overrides(
    headers: &mut Vec<Header>,
    body: &mut Vec<u8>,
    remove_headers: Vec<String>,
    add_headers: Vec<(String, String)>,
    override_body: Option<String>,
) {
    for name in remove_headers {
        headers.retain(|h| !h.name.eq_ignore_ascii_case(&name));
    }
    for (name, value) in add_headers {
        headers.retain(|h| !h.name.eq_ignore_ascii_case(&name));
        headers.push(Header {
            name,
            value: value.into_bytes(),
        });
    }
    if let Some(new_body) = override_body {
        let bytes = new_body.into_bytes();
        headers.retain(|h| !h.name.eq_ignore_ascii_case("content-length"));
        headers.push(Header {
            name: "Content-Length".into(),
            value: bytes.len().to_string().into_bytes(),
        });
        *body = bytes;
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_ipc_repeater_send(
    original_request: HttpRequest,
    mut request: HttpRequest,
    settings: crate::RepeaterSettings,
    runtime_cfg: RepeaterSendRuntimeConfig,
    started_unix_ms: i64,
    sessions: crate::sessions::SessionRuntime,
    provenance: crate::ProvenanceTracker,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<IpcRepeaterOutcome> {
    let run_id = crate::Uuid::new_v4();
    if request != original_request {
        provenance.record_request(
            run_id,
            &original_request,
            ProvenanceActor::ManualInterceptEdit,
            &request,
        );
    }
    let req_version = request.version;
    apply_repeater_protocol_options(
        run_id,
        &original_request,
        &mut request,
        &settings,
        req_version,
        &provenance,
    );
    let mut record = pending_repeater_flow_record(run_id, request, started_unix_ms);
    let started = std::time::Instant::now();
    let deadline = started + runtime_cfg.request_timeout;
    let send_config = runtime_cfg.build_send_config(
        Some(&record.request.url),
        runtime_cfg.request_timeout,
        Some(record.request.version),
    );
    if apply_repeater_session_rules(
        &sessions,
        SessionTool::MioAi,
        &provenance,
        run_id,
        &original_request,
        &mut record,
        &settings,
        None,
        true,
        &send_config,
        &cancel,
        deadline,
        started,
    )
    .await
    .is_none()
    {
        provenance.remove(run_id);
        let msg = match &record.summary.state {
            FlowState::Failed(reason) => reason.clone(),
            _ => "Session 规则或宏前置请求失败，已拒绝裸发请求".into(),
        };
        bail!(msg);
    }
    let response =
        match mio_proxy::send(record.request.clone(), send_config.clone(), cancel.clone())
            .await
            .context("Repeater 发送请求失败")
        {
            Ok(resp) => resp,
            Err(err) => {
                provenance.remove(run_id);
                return Err(err);
            }
        };
    sessions.observe_response(SessionTool::MioAi, &record.request.url, &response);
    let original_response = response.clone();
    let unpacked = if settings.unpack_compressed_responses {
        spawn_unpack_response(
            response.clone(),
            record.request.method.clone(),
            send_config.limits.max_body_bytes,
            cancel.clone(),
            deadline,
        )
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .filter(|u| u != &original_response)
    } else {
        None
    };
    let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let request_mutated = record.request != original_request;
    let initial_pending = request_mutated.then(|| {
        pending_repeater_flow_record(run_id, original_request.clone(), started_unix_ms)
    });
    let mutated_pending = request_mutated.then(|| {
        let mut mutated =
            pending_repeater_flow_record(run_id, record.request.clone(), started_unix_ms);
        mutated.summary.revision = 2;
        mutated
    });
    let base_rev = if request_mutated { 3 } else { 1 };
    let response_unpacked = unpacked.is_some();
    let (received, final_response, final_rev) = if let Some(unpacked_resp) = unpacked {
        provenance.record_response(
            run_id,
            &original_response,
            ProvenanceActor::UnpackCompressed,
            &unpacked_resp,
        );
        let recv_record = FlowRecord {
            summary: FlowSummary {
                id: run_id,
                revision: base_rev,
                source: TrafficSource::Repeater,
                method: record.request.method.clone(),
                url: record.request.url.clone(),
                status: Some(original_response.status),
                state: FlowState::Complete,
                request_bytes: record.request.body.len() as u64,
                response_bytes: original_response.body.len() as u64,
                started_unix_ms,
                elapsed_ms,
            },
            request: record.request.clone(),
            response: Some(original_response.clone()),
        };
        (Some(recv_record), unpacked_resp, base_rev + 1)
    } else {
        (None, response, base_rev)
    };
    record.summary.status = Some(final_response.status);
    record.summary.response_bytes = final_response.body.len() as u64;
    record.summary.state = FlowState::Complete;
    record.summary.revision = final_rev;
    record.summary.elapsed_ms = elapsed_ms;
    record.response = Some(final_response);
    let flow_provenance = provenance.get(run_id).unwrap_or_default();
    Ok(IpcRepeaterOutcome {
        initial_pending,
        mutated_pending,
        received,
        record,
        original_request: request_mutated.then_some(original_request),
        original_response: response_unpacked.then_some(original_response),
        provenance: flow_provenance,
    })
}
