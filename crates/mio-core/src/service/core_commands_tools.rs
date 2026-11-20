use super::*;

impl State {
    pub(super) async fn command_prefs_ext_tools(&mut self, command: Command) -> Result<()> {
        match command {
            Command::QueryUserPreferences { operation_id } => {
                let state = self.preferences.query();
                self.emit(Event::UserPreferences {
                    operation_id,
                    state,
                })
                .await;
            }
            Command::SetUserPreferences {
                operation_id,
                expected_revision,
                running,
                finished,
            } => {
                let result = if operation_id.is_nil() {
                    Err("用户偏好保存缺少有效 operation ID；未写入。".into())
                } else {
                    self.preferences
                        .set(expected_revision, running, finished)
                        .map_err(|error| format!("{error:#}"))
                };
                self.emit(Event::UserPreferencesSaved {
                    operation_id,
                    result,
                })
                .await;
            }
            Command::SetUserWorkbenchPreferences {
                operation_id,
                expected_revision,
                display,
                message_editor,
                misc,
            } => {
                let result = if operation_id.is_nil() {
                    Err("用户偏好保存缺少有效 operation ID；未写入。".into())
                } else {
                    self.preferences
                        .set_workbench_settings(expected_revision, display, message_editor, misc)
                        .map_err(|error| format!("{error:#}"))
                };
                self.emit(Event::UserPreferencesSaved {
                    operation_id,
                    result,
                })
                .await;
            }
            Command::SetUserStartupPreferences {
                operation_id,
                expected_revision,
                default_config,
                disable_extensions,
            } => {
                let result = if operation_id.is_nil() {
                    Err("用户偏好保存缺少有效 operation ID；未写入。".into())
                } else {
                    self.preferences
                        .set_startup(expected_revision, default_config, disable_extensions)
                        .map_err(|error| format!("{error:#}"))
                };
                self.emit(Event::UserPreferencesSaved {
                    operation_id,
                    result,
                })
                .await;
            }
            Command::SetUserTlsCertificates {
                operation_id,
                expected_revision,
                certificates,
            } => {
                let result = if operation_id.is_nil() {
                    Err("TLS 用户设置保存缺少有效 operation ID；未写入。".into())
                } else {
                    self.preferences
                        .set_tls_certificates(expected_revision, certificates)
                        .map_err(|error| format!("{error:#}"))
                };
                if result.is_ok()
                    && let Ok(workspace) = self.store.workspace()
                {
                    if let Ok(config) = Self::ws_repeater_send_config(
                        &self.preferences,
                        &workspace,
                        self.compiled_client_certs.clone(),
                    ) {
                        self.ws_repeater.set_connect_config(config);
                    }
                    if (self.proxy.is_some() || !self.extra_proxies.is_empty())
                        && let Err(error) = self.reload_running_proxy_from_workspace(&workspace).await
                    {
                        self.emit(Event::Error {
                            project: Some(self.project),
                            operation: "set_user_tls_certificates",
                            message: format!("更新用户 TLS 证书后重新加载代理失败：{error:#}"),
                        })
                        .await;
                    }
                }
                self.emit(Event::UserPreferencesSaved {
                    operation_id,
                    result,
                })
                .await;
            }
            Command::InspectExternal {
                project,
                job_id,
                path,
                cancel,
            } => {
                self.inspect_ext(project, job_id, path, cancel).await;
            }
            Command::ConvertExternal {
                project,
                job_id,
                inspection_id,
                destination,
                cancel,
            } => {
                self.convert_ext(project, job_id, inspection_id, destination, cancel)
                    .await;
            }
            Command::ReleaseExtInspection {
                project,
                job_id,
                inspection_id,
            } => {
                self.release_ext(project, job_id, inspection_id).await;
            }
            Command::QueryExtReport {
                project,
                query_revision,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        let mut sources = self.store.ext_sources()?;
                        ensure!(
                            sources.len() <= 1,
                            "此项目包含多个 外部来源，当前读取入口不能合并它们"
                        );
                        Ok(sources.pop())
                    })
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtReport {
                    project,
                    query_revision,
                    result,
                })
                .await;
            }
            Command::QueryExtItems {
                project,
                query_revision,
                source_id,
                collection,
                offset,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        self.store
                            .ext_item_page(source_id, collection.as_deref(), offset, limit)
                    })
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtItems {
                    project,
                    query_revision,
                    source_id,
                    collection,
                    offset,
                    result,
                })
                .await;
            }
            Command::ReadExtItem {
                project,
                selection_revision,
                source_id,
                item_id,
                message_offset,
                message_limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        self.store.ext_item_detail(
                            source_id,
                            item_id,
                            message_offset,
                            message_limit,
                        )
                    })
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtItem {
                    project,
                    selection_revision,
                    source_id,
                    item_id,
                    message_offset,
                    result,
                })
                .await;
            }
            Command::ReadHistoryTransfer {
                project,
                operation_id,
                workspace_revision,
                entries,
                cancel,
            } => {
                self.read_history_transfer(
                    project,
                    operation_id,
                    workspace_revision,
                    &entries,
                    &cancel,
                )
                .await;
            }
            Command::OpenExtRepeater {
                project,
                operation_id,
                source_id,
                item_id,
                message_id,
                workspace_revision,
            } => {
                self.open_ext_repeater(
                    project,
                    operation_id,
                    source_id,
                    item_id,
                    message_id,
                    workspace_revision,
                )
                .await;
            }
            Command::QueryExtMessages {
                project,
                query_revision,
                source_id,
                collection,
                offset,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        self.store.ext_message_page(
                            source_id,
                            collection.as_deref(),
                            offset,
                            limit,
                        )
                    })
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtMessages {
                    project,
                    query_revision,
                    source_id,
                    collection,
                    offset,
                    result,
                })
                .await;
            }
            Command::ReadExtMessage {
                project,
                selection_revision,
                message_id,
                offset,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.store.ext_message_fragment(message_id, offset, limit))
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtMessage {
                    project,
                    selection_revision,
                    message_id,
                    offset,
                    result,
                })
                .await;
            }
            Command::ReadExtResponse {
                project,
                selection_revision,
                reference,
                offset,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.store.ext_response_fragment(&reference, offset, limit))
                    .map_err(|error| error.to_string());
                self.emit(Event::ExtResponse {
                    project,
                    selection_revision,
                    reference,
                    offset,
                    result,
                })
                .await;
            }
            Command::Plugin {
                project,
                operation_id,
                action,
            } => {
                if let Err(error) = self.plugin_command(project, operation_id, action).await {
                    self.plugin_result(project, operation_id, Err(error.to_string()))
                        .await;
                }
            }
            Command::WebSocketRepeater(command) => {
                self.websocket_repeater_command(command).await;
            }
            Command::Sequencer(command) => {
                self.sequencer_command(command).await;
            }
            Command::Audit(command) => {
                self.audit_command(command).await?;
            }
            Command::Collaborator(command) => {
                self.collaborator_command(command).await?;
            }
            Command::QuerySiteMap {
                project,
                query_revision,
                query,
                cancel,
            } => {
                let combined = CancellationToken::new();
                if self.cancel.is_cancelled() || cancel.is_cancelled() {
                    combined.cancel();
                }
                let shutdown = self.cancel.clone();
                let operation = cancel.clone();
                let forward = combined.clone();
                let guard = tokio::spawn(async move {
                    tokio::select! {
                        _ = shutdown.cancelled() => forward.cancel(),
                        _ = operation.cancelled() => forward.cancel(),
                    }
                });
                self.store.cancel_queries_with(combined);
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        let workspace = self.store.workspace()?;
                        let pending = self.http_recovery_snapshot()?;
                        self.store
                            .target_site_map_snapshot(&workspace.scope, &query, &pending)
                    })
                    .map_err(|error| error.to_string());
                guard.abort();
                self.store.cancel_queries_with(self.cancel.clone());
                self.emit(Event::SiteMapPage {
                    project,
                    query_revision,
                    query,
                    result,
                })
                .await;
            }
            Command::AddSiteMapEntry { project, operation_id, workspace_revision, entry, cancel } => {
                self.store.cancel_queries_with_operation(self.cancel.clone(), cancel.clone());
                let result = (|| -> Result<SiteMapEntryReceipt> {
                    self.check_project(project)?;
                    ensure!(!operation_id.is_nil() && !self.quiesced, "站点地图操作身份或项目状态无效");
                    ensure!(!self.cancel.is_cancelled() && !cancel.is_cancelled(), "站点地图添加已取消");
                    let (owner, row, detail, draft) = match entry {
                        SiteMapEntryInput::History(HistoryTransferEntry::Live { flow_id, revision, original }) => {
                            let mut record = self.read_flow(flow_id)?;
                            ensure!(record.summary.revision == revision, "站点地图请求 revision 已变化");
                            let before = self.original_flow(flow_id)?;
                            if original {
                                record.request = before.request.clone().unwrap_or(record.request);
                                record.response = before.response.clone().or(record.response);
                                record.summary.method = record.request.method.clone();
                                record.summary.url = record.request.url.clone();
                                record.summary.status = record.response.as_ref().map(|response| response.status);
                                record.summary.request_bytes = record.request.body.len() as u64;
                                record.summary.response_bytes = record.response.as_ref().map_or(0, |response| response.body.len() as u64);
                            }
                            let detail = SiteMapEntryDetail {
                                row_id: HistoryEntryId::Live(flow_id), request_raw: request_to_raw(&record.request),
                                response_raw: record.response.as_ref().map(response_to_raw),
                                original_request_raw: before.request.as_ref().map(request_to_raw),
                                original_response_raw: before.response.as_ref().map(response_to_raw), record: record.clone(), original: before,
                            };
                            (format!("live:{flow_id}"), HistoryRow::Live(record.summary), detail, false)
                        }
                        SiteMapEntryInput::History(HistoryTransferEntry::External { source_id, item_id, message_id }) => {
                            let (row, detail) = self.store.site_map_ext_detail(source_id, item_id, message_id, &cancel)?;
                            (format!("ext_archive:{source_id}:{item_id}"), row, detail, false)
                        }
                        SiteMapEntryInput::History(HistoryTransferEntry::SiteMap { .. }) => anyhow::bail!("所选请求已经属于站点地图"),
                        SiteMapEntryInput::Draft { owner_id, source, draft, response_raw } => {
                            let detail = SiteMapEntryDetail::from_draft(owner_id, source.clone(), &draft, response_raw)?;
                            (format!("draft:{source:?}:{owner_id}"), HistoryRow::Live(detail.record.summary.clone()), detail, true)
                        }
                    };
                    self.store.add_site_map_entry(workspace_revision, &owner, row, detail, draft, &cancel)
                })().map_err(|error| format!("{error:#}"));
                self.store.cancel_queries_with(self.cancel.clone());
                self.emit(Event::SiteMapEntryAdded { project, operation_id, workspace_revision, result }).await;
            }
            Command::ReadSiteMapEntry { project, selection_revision, entry_id, revision, cancel } => {
                self.store.cancel_queries_with_operation(self.cancel.clone(), cancel.clone());
                let result = self.check_project(project).and_then(|()| {
                    ensure!(!self.cancel.is_cancelled() && !cancel.is_cancelled(), "站点地图读取已取消");
                    self.store.read_site_map_entry(entry_id, revision)
                }).map_err(|error| format!("{error:#}"));
                self.store.cancel_queries_with(self.cancel.clone());
                self.emit(Event::SiteMapEntryRead { project, selection_revision, entry_id, revision, result }).await;
            }
            _ => unreachable!("routed by State::command"),
        }
        Ok(())
    }
}
