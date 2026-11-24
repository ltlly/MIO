use super::*;

impl State {
    pub(super) fn plugin_check_call(&self, call: &ServiceRequest) -> Result<()> {
        ensure!(
            !call.cancel.is_cancelled() && !call.reply.is_closed(),
            "插件宿主调用已取消"
        );
        ensure!(
            tokio::time::Instant::now() < call.deadline,
            "插件宿主调用已超时"
        );
        let instance = self.plugin_instance(call.identity.instance_id)?;
        ensure!(
            instance.info.identity == call.identity && instance.info.manifest.id == call.plugin_id,
            "插件服务身份不匹配"
        );
        let permission = call.operation.required_permission();
        ensure!(
            instance.info.grants.contains(&permission),
            "插件未获准此宿主能力"
        );
        if let ServiceOperation::UpdateDraft {
            tab_id,
            expected_revision,
            ..
        } = &call.operation
        {
            if call.panel_id == protocol::PANEL_ID {
                ensure!(
                    instance.resources.get(tab_id) == Some(expected_revision),
                    "插件资源 handle 已撤权或版本过期"
                );
            } else {
                let virtual_id = self
                    .plugins
                    .panel_routes
                    .get(&(call.identity.instance_id, call.panel_id))
                    .context("多路复用子面板已关闭")?;
                let child = self
                    .plugins
                    .multiplexed
                    .get(virtual_id)
                    .context("多路复用子面板不存在")?;
                ensure!(
                    child.resources.get(tab_id) == Some(expected_revision),
                    "多路复用子面板资源 handle 已撤权或版本过期"
                );
            }
            self.plugin_tab(*tab_id, *expected_revision)?;
        } else if call.panel_id != protocol::PANEL_ID {
            let virtual_id = self
                .plugins
                .panel_routes
                .get(&(call.identity.instance_id, call.panel_id))
                .context("多路复用子面板已关闭")?;
            ensure!(
                self.plugins.multiplexed.contains_key(virtual_id),
                "多路复用子面板不存在"
            );
        }
        Ok(())
    }

    pub(super) async fn plugin_service(&mut self, call: ServiceRequest) {
        if let Err(error) = self.plugin_check_call(&call) {
            let _ = call
                .reply
                .send(Err(Failure::new(ErrorCode::Denied, error.to_string())));
            return;
        }
        match &call.operation {
            ServiceOperation::QueryHistory {
                offset,
                limit,
                filter,
            } => {
                let result = if *limit == 0 || *limit > panel::MAX_TABLE_ROWS || filter.len() > 1024
                {
                    Err(anyhow::anyhow!("History 查询超过限额"))
                } else {
                    self.store
                        .flows()
                        .page(*offset, *limit, filter)
                        .map(|(total, rows)| ServiceResult::History {
                            offset: *offset,
                            total,
                            rows,
                        })
                };
                let result = result.and_then(|result| {
                    self.plugin_check_call(&call)?;
                    Ok(result)
                });
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::ReadFlow { flow_id } => {
                let result = self.read_flow(*flow_id).and_then(|record| {
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Record(Box::new(record)))
                });
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::UpdateDraft {
                tab_id,
                expected_revision,
                request,
            } => {
                let edit = PluginWorkspaceEdit::UpdateDraft {
                    tab_id: *tab_id,
                    expected_revision: *expected_revision,
                    request: request.clone(),
                };
                self.plugin_queue_workspace(call, edit).await;
            }
            ServiceOperation::OpenRepeater { request } => {
                let edit = PluginWorkspaceEdit::OpenRepeater {
                    request: request.clone(),
                };
                self.plugin_queue_workspace(call, edit).await;
            }
            ServiceOperation::OpenIntruder { request } => {
                let edit = PluginWorkspaceEdit::OpenIntruder {
                    request: request.clone(),
                };
                self.plugin_queue_workspace(call, edit).await;
            }
            ServiceOperation::StorageGet { key } => {
                let result = self
                    .store
                    .plugin_storage()
                    .get(self.project, &call.plugin_id, key)
                    .and_then(|val| {
                        self.plugin_check_call(&call)?;
                        Ok(match val {
                            Some(value) => ServiceResult::StorageValue {
                                present: true,
                                value,
                            },
                            None => ServiceResult::StorageValue {
                                present: false,
                                value: vec![],
                            },
                        })
                    });
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::StorageSet { key, value } => {
                let result: Result<ServiceResult> = (|| {
                    self.require_saved()?;
                    self.store.plugin_storage_mut().set(
                        self.project,
                        &call.plugin_id,
                        key,
                        value,
                        &call.cancel,
                    )?;
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Done)
                })();
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::PreferencesGet { key } => {
                let result = self
                    .plugins
                    .prefs
                    .get(&call.plugin_id, key)
                    .and_then(|val| {
                        self.plugin_check_call(&call)?;
                        Ok(match val {
                            Some(value) => ServiceResult::StorageValue {
                                present: true,
                                value,
                            },
                            None => ServiceResult::StorageValue {
                                present: false,
                                value: vec![],
                            },
                        })
                    });
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::PreferencesSet { key, value } => {
                let result: Result<ServiceResult> = (|| {
                    self.plugins
                        .prefs
                        .set(&call.plugin_id, key, value, &call.cancel)?;
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Done)
                })();
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::NetworkSend { .. } => self.plugin_send(call).await,
            ServiceOperation::ScopeCheck { url } => {
                let result: Result<ServiceResult> = (|| {
                    let workspace = self.store.metadata().workspace()?;
                    let in_scope = workspace.scope.is_url_in_scope(url);
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::ScopeStatus { in_scope })
                })();
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::ScopeUpdate {
                url_prefix,
                exclude,
                include_subdomains,
            } => {
                let result: Result<ServiceResult> = async {
                    self.require_saved()?;
                    let mut workspace = self.store.metadata().workspace()?;
                    let prefix = url_prefix.trim().to_owned();
                    let list = if *exclude {
                        &mut workspace.scope.exclude
                    } else {
                        &mut workspace.scope.include
                    };
                    if !list.iter().any(|r| {
                        r.enabled && r.prefix == prefix && r.include_subdomains == *include_subdomains
                    }) {
                        list.push(crate::ScopeRule {
                            id: Uuid::new_v4(),
                            enabled: true,
                            prefix,
                            include_subdomains: *include_subdomains,
                        });
                        workspace.revision += 1;
                        workspace.validate()?;
                        self.apply_save_workspace(self.project, workspace).await?;
                        self.plugin_check_call(&call)?;
                    }
                    Ok(ServiceResult::Done)
                }
                .await;
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::QuerySiteMap {
                prefix,
                in_scope_only,
                offset,
                limit,
            } => {
                let result: Result<ServiceResult> = (|| {
                    let workspace = self.store.metadata().workspace()?;
                    let selected_node_key =
                        (!prefix.trim().is_empty()).then(|| prefix.trim().to_owned());
                    let query = crate::SiteMapQuery {
                        selected_node_key,
                        filter: crate::SiteMapFilter {
                            in_scope_only: *in_scope_only,
                            ..crate::SiteMapFilter::default()
                        },
                        offset: *offset,
                        limit: *limit,
                    };
                    let snapshot = self.store.site_map().snapshot(
                        &workspace.scope,
                        &query,
                        &[],
                    )?;
                    let mut rows = Vec::with_capacity(snapshot.items.len());
                    for item in snapshot.items {
                        match item {
                            crate::HistoryRow::Live(summary) => {
                                let in_scope = workspace.scope.is_url_in_scope(&summary.url);
                                let notes_preview = self
                                    .store
                                    .ext_archive()
                                    .http_notes(self.project, Uuid::nil(), summary.id)
                                    .map(|n| n.text.chars().take(128).collect())
                                    .unwrap_or_default();
                                let highlight_color = self
                                    .store
                                    .ext_archive()
                                    .http_highlights(
                                        self.project,
                                        &[crate::ExtHttpHighlightChange {
                                            source_id: Uuid::nil(),
                                            item_id: summary.id,
                                            expected_highlight_revision: 0,
                                            color: 0,
                                        }],
                                    )
                                    .ok()
                                    .and_then(|v| v.into_iter().next().map(|(_, _, h)| u32::from(h.color)))
                                    .unwrap_or(0);
                                rows.push(protocol::SiteMapRowDto {
                                    id: summary.id.as_bytes().to_vec(),
                                    method: summary.method,
                                    url: summary.url,
                                    status: summary.status.map(u32::from),
                                    response_bytes: summary.response_bytes,
                                    in_scope,
                                    notes_preview,
                                    highlight_color,
                                });
                            }
                            crate::HistoryRow::ExtProxy(row) => {
                                let method = row
                                    .request
                                    .as_ref()
                                    .and_then(|m| m.method.clone())
                                    .unwrap_or_default();
                                let url = match (
                                    &row.origin,
                                    row.request.as_ref().and_then(|m| m.request_target.as_deref()),
                                ) {
                                    (Some(o), Some(t)) if t.starts_with('/') => format!("{o}{t}"),
                                    (Some(o), _) => o.clone(),
                                    (None, Some(t)) => t.to_owned(),
                                    _ => String::new(),
                                };
                                let in_scope = workspace.scope.is_url_in_scope(&url);
                                rows.push(protocol::SiteMapRowDto {
                                    id: row.item.id.as_bytes().to_vec(),
                                    method,
                                    url,
                                    status: row.response.as_ref().and_then(|r| r.status).map(u32::from),
                                    response_bytes: row.response.as_ref().map_or(0, |r| r.raw_bytes),
                                    in_scope,
                                    notes_preview: row.notes_preview,
                                    highlight_color: row.highlight.map_or(0, |h| u32::from(h.color)),
                                });
                            }
                        }
                    }
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::SiteMap {
                        offset: snapshot.offset,
                        total: snapshot.total_items,
                        rows,
                    })
                })();
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::ReadAnnotations { flow_id } => {
                let result: Result<ServiceResult> = (|| {
                    let notes = self
                        .store
                        .ext_archive()
                        .http_notes(self.project, Uuid::nil(), *flow_id)?;
                    let hl = self.store.ext_archive().http_highlights(
                        self.project,
                        &[crate::ExtHttpHighlightChange {
                            source_id: Uuid::nil(),
                            item_id: *flow_id,
                            expected_highlight_revision: 0,
                            color: 0,
                        }],
                    )?;
                    let (highlight_color, highlight_revision) = hl
                        .into_iter()
                        .next()
                        .map_or((0, 0), |(_, _, h)| (h.color, h.revision));
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Annotations {
                        flow_id: *flow_id,
                        notes: notes.text,
                        highlight_color,
                        notes_revision: notes.revision,
                        highlight_revision,
                    })
                })();
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::UpdateAnnotations {
                flow_id,
                notes,
                highlight_color,
            } => {
                let result: Result<ServiceResult> = (|| {
                    self.require_saved()?;
                    if let Some(text) = notes {
                        let cur = self
                            .store
                            .ext_archive()
                            .http_notes(self.project, Uuid::nil(), *flow_id)?;
                        self.store
                            .ext_archive_mut()
                            .set_http_notes(
                                self.project,
                                Uuid::nil(),
                                *flow_id,
                                cur.revision,
                                text,
                                &call.cancel,
                            )
                            .map_err(|err| anyhow::anyhow!(err.message))?;
                    }
                    if let Some(color) = highlight_color {
                        let cur = self.store.ext_archive().http_highlights(
                            self.project,
                            &[crate::ExtHttpHighlightChange {
                                source_id: Uuid::nil(),
                                item_id: *flow_id,
                                expected_highlight_revision: 0,
                                color: 0,
                            }],
                        )?;
                        let expected_highlight_revision =
                            cur.into_iter().next().map_or(0, |(_, _, h)| h.revision);
                        self.store
                            .ext_archive_mut()
                            .set_http_highlights(
                                self.project,
                                &[crate::ExtHttpHighlightChange {
                                    source_id: Uuid::nil(),
                                    item_id: *flow_id,
                                    expected_highlight_revision,
                                    color: *color,
                                }],
                                &call.cancel,
                            )
                            .map_err(|err| anyhow::anyhow!(err.message))?;
                    }
                    let final_notes = self
                        .store
                        .ext_archive()
                        .http_notes(self.project, Uuid::nil(), *flow_id)?;
                    let final_hl = self.store.ext_archive().http_highlights(
                        self.project,
                        &[crate::ExtHttpHighlightChange {
                            source_id: Uuid::nil(),
                            item_id: *flow_id,
                            expected_highlight_revision: 0,
                            color: 0,
                        }],
                    )?;
                    let (final_color, final_hl_rev) = final_hl
                        .into_iter()
                        .next()
                        .map_or((0, 0), |(_, _, h)| (h.color, h.revision));
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Annotations {
                        flow_id: *flow_id,
                        notes: final_notes.text,
                        highlight_color: final_color,
                        notes_revision: final_notes.revision,
                        highlight_revision: final_hl_rev,
                    })
                })();
                if result.is_ok() {
                    self.history_changed().await;
                }
                let result = result.and_then(|res| {
                    self.plugin_check_call(&call)?;
                    Ok(res)
                });
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::OpenOrganiser {
                request,
                response,
                notes,
                highlight_color,
            } => {
                let result: Result<ServiceResult> = async {
                    self.require_saved()?;
                    let mut workspace = self.store.metadata().workspace()?;
                    let number = crate::next_organiser_number(&workspace.organiser_items);
                    let mut item = crate::OrganiserItem::from_raw_parts(
                        number,
                        &format!("Plugin: {}", call.plugin_id),
                        &request.url,
                        request_to_raw(request),
                        response.as_ref().map(response_to_raw),
                        response.as_ref().map(|r| r.status),
                        request.version,
                    )?;
                    item.notes.clone_from(notes);
                    item.highlight = highlight_color.filter(|&c| (1..=9).contains(&c));
                    item.validate()?;
                    let item_id = item.id;
                    let item_number = item.number;
                    workspace.organiser_items.push(item);
                    crate::validate_organiser_items(&workspace.organiser_items)?;
                    workspace.revision += 1;
                    workspace.validate()?;
                    self.apply_save_workspace(self.project, workspace).await?;
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Resource {
                        id: item_id,
                        revision: item_number,
                    })
                }
                .await;
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::SubmitAuditIssue {
                flow_id,
                url,
                finding,
            } => {
                let mut emitted_state = None;
                let result: Result<ServiceResult> = (|| {
                    self.require_saved()?;
                    let mut state = self.store.audit().read_state()?;
                    let resp = flow_id
                        .and_then(|id| self.read_flow(id).ok())
                        .and_then(|r| r.response)
                        .unwrap_or(HttpResponse {
                            status: 0,
                            version: HttpVersion::Http11,
                            reason_phrase: None,
                            headers: Vec::new(),
                            body: Vec::new(),
                            trailers: Vec::new(),
                        });
                    let incoming = super::passive::convert_plugin_findings_for_phase(
                        flow_id.unwrap_or_else(Uuid::new_v4),
                        url,
                        &resp,
                        vec![finding.clone()],
                        crate::AuditPhase::Passive,
                        "0x00900102",
                    );
                    let added = crate::store::audit::merge_audit_findings(&mut state.findings, incoming);
                    if !added.is_empty() {
                        state.revision += 1;
                        self.store.audit_mut().write_state(&state, &call.cancel)?;
                        emitted_state = Some(state);
                    }
                    self.plugin_check_call(&call)?;
                    Ok(ServiceResult::Done)
                })();
                if let Some(state) = emitted_state {
                    self.emit(Event::Audit(crate::AuditEvent::State {
                        project: self.project,
                        query_revision: 0,
                        result: Ok(Box::new(state)),
                    }))
                    .await;
                }
                let result = result.and_then(|res| {
                    self.plugin_check_call(&call)?;
                    Ok(res)
                });
                let _ = call.reply.send(
                    result.map_err(|error| Failure::new(ErrorCode::Invalid, error.to_string())),
                );
            }
            ServiceOperation::CollaboratorGenerate { label } => {
                let result: Result<(ServiceResult, crate::oast::OastPayloadToken)> = async {
                    self.collaborator.ensure_default_started(self.project).await?;
                    self.plugin_check_call(&call)?;
                    let eff_label = if label.trim().is_empty() {
                        call.plugin_id.as_str()
                    } else {
                        label.trim()
                    };
                    let token = self
                        .collaborator
                        .issue_internal_payload(eff_label)
                        .context("Collaborator 本地服务未就绪")?;
                    Ok((ServiceResult::CollaboratorPayload(token.clone()), token))
                }
                .await;
                match result {
                    Ok((res, token)) => {
                        self.emit(Event::Collaborator(
                            crate::CollaboratorEvent::PayloadsGenerated {
                                project: self.project,
                                payloads: vec![token],
                            },
                        ))
                        .await;
                        self.emit_collaborator_status(self.project).await;
                        let reply = self
                            .plugin_check_call(&call)
                            .map(|()| res)
                            .map_err(|error| Failure::new(ErrorCode::Cancelled, error.to_string()));
                        let _ = call.reply.send(reply);
                    }
                    Err(error) => {
                        let _ = call
                            .reply
                            .send(Err(Failure::new(ErrorCode::Internal, error.to_string())));
                    }
                }
            }
            ServiceOperation::CollaboratorPoll {
                token_filter,
                limit,
            } => {
                let observed = self.collaborator.drain_local_rx();
                for interaction in observed {
                    let event = crate::CollaboratorEvent::InteractionObserved {
                        project: self.project,
                        interaction: Box::new(interaction),
                    };
                    self.on_collaborator_event_for_audit(&event).await;
                    self.emit(Event::Collaborator(event)).await;
                }
                let filter_trimmed = token_filter.trim();
                let status = self.collaborator.status();
                let items: Vec<crate::oast::OastInteraction> = status
                    .interactions
                    .into_iter()
                    .filter(|item| {
                        filter_trimmed.is_empty()
                            || item
                                .token
                                .as_deref()
                                .is_some_and(|t| t == filter_trimmed || t.contains(filter_trimmed))
                    })
                    .take(*limit)
                    .collect();
                let reply = self
                    .plugin_check_call(&call)
                    .map(|()| ServiceResult::CollaboratorInteractions(items))
                    .map_err(|error| Failure::new(ErrorCode::Cancelled, error.to_string()));
                let _ = call.reply.send(reply);
            }
        }
    }

    async fn plugin_send(&mut self, call: ServiceRequest) {
        let key = (call.identity.instance_id, call.request_id);
        let result = (|| {
            self.require_saved()?;
            ensure!(
                self.plugins.network.len() < MAX_NETWORK,
                "插件并发网络请求达到 16 个限额"
            );
            ensure!(
                !self.plugins.network.contains_key(&key),
                "插件网络请求 ID 重复；未再次发送"
            );
            let workspace = self.store.metadata().workspace()?;
            let upstream_tls = self
                .preferences
                .effective_upstream_tls(&workspace.upstream_tls)?;
            let ServiceOperation::NetworkSend { request } = &call.operation else {
                unreachable!()
            };
            let upstream_route = workspace
                .upstream_proxy
                .route_for_url_with_tls(&request.url, upstream_tls.clone())?;
            let hostname_overrides = workspace.upstream_proxy.compiled_hostname_overrides()?;
            let mut outbound = request.clone();
            let next_depth = super::hooks::plugin_hook_depth(&outbound).saturating_add(1);
            outbound
                .headers
                .retain(|h| !h.name.eq_ignore_ascii_case("x-mio-plugin-depth"));
            outbound.headers.push(Header {
                name: "x-mio-plugin-depth".into(),
                value: next_depth.to_string().into_bytes(),
            });
            let record = FlowRecord {
                summary: FlowSummary {
                    id: Uuid::new_v4(),
                    revision: 1,
                    source: TrafficSource::Plugin(call.plugin_id.clone()),
                    method: outbound.method.clone(),
                    url: outbound.url.clone(),
                    status: None,
                    state: FlowState::Pending,
                    request_bytes: outbound.body.len() as u64,
                    response_bytes: 0,
                    started_unix_ms: crate::codec_util::now_unix_ms(),
                    elapsed_ms: 0,
                },
                request: outbound,
                response: None,
            };
            self.persist(&record)?;
            let hook_scope = workspace.scope.clone();
            Ok::<_, anyhow::Error>((
                record,
                upstream_tls,
                upstream_route,
                hostname_overrides,
                hook_scope,
            ))
        })();
        let (mut record, upstream_tls, upstream_route, hostname_overrides, hook_scope) =
            match result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = call
                    .reply
                    .send(Err(Failure::new(ErrorCode::Denied, error.to_string())));
                return;
            }
        };
        let cancel = call.cancel.clone();
        let config = SendConfig {
            network: mio_proxy::SendNetworkConfig {
                upstream_route,
                hostname_overrides,
            },
            tls: mio_proxy::UpstreamTlsConfig {
                trust: upstream_tls,
                client_certificates: self.tls.compiled_client_certs.clone(),
                ..Default::default()
            },
            limits: mio_proxy::SendLimits {
                timeout: call
                    .deadline
                    .saturating_duration_since(tokio::time::Instant::now()),
                ..Default::default()
            },
            http: mio_proxy::SendHttpConfig {
                version: Some(record.request.version),
                ..Default::default()
            },
            ..SendConfig::default()
        };
        let flow_id = record.summary.id;
        let sessions = self.sessions.runtime.clone();
        let deadline = Instant::now() + config.limits.timeout;
        let session_hooks = self.strict_session_action_hook_entries();
        let http_hooks = self.tool_http_hook_entries();
        let plugin_events = self.events.clone();
        let task = tokio::spawn(async move {
            let started = Instant::now();
            if sessions
                .apply_to_flow_record(
                    crate::SessionTool::Extensions,
                    &mut record,
                    &config,
                    &cancel,
                    deadline,
                    started,
                )
                .await
                .is_none()
            {
                return record;
            }
            if !session_hooks.is_empty() || !http_hooks.is_empty() {
                let mut current = record.request.clone();
                match super::hooks::run_pre_send_plugin_hooks_configured(
                    &session_hooks,
                    &http_hooks,
                    Some(&hook_scope),
                    Some(&plugin_events),
                    flow_id,
                    protocol::SourceDto::Plugin,
                    &mut current,
                    None,
                    deadline,
                    &cancel,
                )
                .await
                {
                    super::hooks::PreSendHookOutcome::Proceed { .. } => {
                        super::hooks::sync_flow_request(&mut record, current);
                    }
                    super::hooks::PreSendHookOutcome::Dropped => {
                        super::hooks::sync_flow_request(&mut record, current);
                        record.summary.state = FlowState::Dropped;
                        record.summary.revision += 1;
                        record.summary.elapsed_ms =
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        return record;
                    }
                    super::hooks::PreSendHookOutcome::Failed(message) => {
                        super::hooks::sync_flow_request(&mut record, current);
                        record.summary.state = if cancel.is_cancelled() {
                            FlowState::Cancelled
                        } else {
                            FlowState::Failed(message)
                        };
                        record.summary.revision += 1;
                        record.summary.elapsed_ms =
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        return record;
                    }
                }
            }
            match mio_proxy::send(record.request.clone(), config, cancel).await {
                Ok(response) => {
                    sessions.observe_response(
                        crate::SessionTool::Extensions,
                        &record.request.url,
                        &response,
                    );
                    record.summary.status = Some(response.status);
                    record.summary.response_bytes = response.body.len() as u64;
                    record.summary.state = FlowState::Complete;
                    record.response = Some(response);
                }
                Err(mio_proxy::ProxyError::Cancelled) => {
                    record.summary.state = FlowState::Cancelled
                }
                Err(error) => record.summary.state = FlowState::Failed(error.to_string()),
            }
            record.summary.revision += 1;
            record.summary.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            record
        });
        self.plugins.network.insert(
            key,
            Network {
                call,
                flow_id,
                task,
            },
        );
        self.history_changed().await;
    }

    pub(super) async fn finish_plugin_network(
        &mut self,
        call: ServiceRequest,
        flow_id: FlowId,
        result: Result<FlowRecord, String>,
    ) -> Result<()> {
        let record = match result {
            Ok(record) => record,
            Err(error) => {
                let mut record = self.read_flow(flow_id)?;
                record.summary.revision += 1;
                record.summary.state = FlowState::Failed(format!(
                    "插件网络任务异常退出，结果未知，未自动重发：{error}"
                ));
                record
            }
        };
        let saved = self.persist(&record);
        if saved.is_ok() {
            self.spawn_plugin_passive_checks(&record);
        }
        self.history_changed().await;
        let reply = match &saved {
            Err(error) => Err(Failure::new(
                ErrorCode::Internal,
                format!("网络任务已结束，但流量尚未写盘：{error}"),
            )),
            Ok(()) => match self.plugin_check_call(&call) {
                Err(error) => Err(Failure::new(ErrorCode::Cancelled, error.to_string())),
                Ok(()) => match record.response {
                    Some(response) => Ok(ServiceResult::Response(response)),
                    None => Err(Failure::new(
                        ErrorCode::Internal,
                        record.summary.state.label(),
                    )),
                },
            },
        };
        let _ = call.reply.send(reply);
        saved
    }
}
