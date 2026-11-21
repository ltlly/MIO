use super::*;

impl State {
    pub(super) async fn command_history_queries(&mut self, command: Command) -> Result<()> {
        match command {
            Command::QueryHttpHistory { query } => self.query_http_history(query),
            Command::DeleteHttpHistory {
                project,
                operation_id,
                flow_ids,
            } => {
                let outcome = async {
                    self.check_project(project)?;
                    ensure!(
                        !operation_id.is_nil(),
                        "删除 HTTP 历史缺少有效 operation ID"
                    );
                    ensure!(!self.quiesced, "正在保存或切换项目；未删除 HTTP 历史");
                    self.require_saved()?;
                    for flow_id in &flow_ids {
                        ensure!(
                            !self.proxy.pending.contains_key(flow_id),
                            "HTTP 历史记录 {flow_id} 仍在拦截等待决定，不能删除"
                        );
                        ensure!(
                            !self
                                .proxy
                                .pending_websockets
                                .values()
                                .any(|ws| ws.flow_id == *flow_id),
                            "HTTP 历史记录 {flow_id} 的 WebSocket 消息仍在拦截等待决定，不能删除"
                        );
                    }
                    self.queries.http.cancel();
                    self.stop_http_query_until(
                        tokio::time::Instant::now() + Duration::from_secs(3),
                    )
                    .await?;
                    self.store.flows_mut().delete_live_proxy_flows(&flow_ids)
                }
                .await;
                match outcome {
                    Ok(deleted_ids) => {
                        let count = deleted_ids.len();
                        if !deleted_ids.is_empty() {
                            self.history_changed().await;
                        }
                        self.emit(Event::HttpHistoryDeleted {
                            project,
                            operation_id,
                            deleted_ids,
                            result: Ok(count),
                        })
                        .await;
                    }
                    Err(error) => {
                        self.emit(Event::HttpHistoryDeleted {
                            project,
                            operation_id,
                            deleted_ids: Vec::new(),
                            result: Err(error.to_string()),
                        })
                        .await;
                    }
                }
            }
            Command::ClearLiveProxyHistory {
                project,
                operation_id,
            } => {
                let outcome = async {
                    self.check_project(project)?;
                    ensure!(
                        !operation_id.is_nil(),
                        "清空 HTTP 历史缺少有效 operation ID"
                    );
                    ensure!(!self.quiesced, "正在保存或切换项目；未清空 HTTP 历史");
                    self.require_saved()?;
                    self.queries.http.cancel();
                    self.stop_http_query_until(
                        tokio::time::Instant::now() + Duration::from_secs(3),
                    )
                    .await?;
                    self.store.flows_mut().clear_live_proxy_history()
                }
                .await;
                match outcome {
                    Ok(deleted_ids) => {
                        let count = deleted_ids.len();
                        if !deleted_ids.is_empty() {
                            self.history_changed().await;
                        }
                        self.emit(Event::HttpHistoryDeleted {
                            project,
                            operation_id,
                            deleted_ids,
                            result: Ok(count),
                        })
                        .await;
                    }
                    Err(error) => {
                        self.emit(Event::HttpHistoryDeleted {
                            project,
                            operation_id,
                            deleted_ids: Vec::new(),
                            result: Err(error.to_string()),
                        })
                        .await;
                    }
                }
            }
            Command::QueryHistory {
                project,
                query_revision,
                offset,
                limit,
                filter,
            } => {
                self.check_project(project)?;
                self.storage.history_dirty = false;
                let (total, rows) = self.store.flows().history_page(offset, limit, &filter)?;
                self.emit(Event::HistoryPage {
                    project,
                    query_revision,
                    offset,
                    total,
                    rows,
                })
                .await;
            }
            Command::QueryLoggerHistory {
                project,
                query_revision,
                offset,
                limit,
                filter,
                source,
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
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        self.store.flows().logger_history_page(
                            offset,
                            limit,
                            &filter,
                            source.as_ref(),
                            &combined,
                        )
                    })
                    .map_err(|error| error.to_string());
                guard.abort();
                self.store.cancel_queries_with(self.cancel.clone());
                self.emit(Event::LoggerHistoryPage {
                    project,
                    query_revision,
                    offset,
                    result,
                })
                .await;
            }
            Command::QueryRepeaterHistory {
                project,
                query_revision,
                tab_id,
                start,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        let mut page = self.store.repeater_history().page(tab_id, start, limit)?;
                        for row in &mut page.rows {
                            let RepeaterHistoryEntryId::Live(id) = row.id else {
                                continue;
                            };
                            if let Some(capture) = self
                                .storage
                                .recovery
                                .iter()
                                .rev()
                                .find(|capture| capture.record.summary.id == id)
                            {
                                let summary = &capture.record.summary;
                                row.status = summary.status;
                                row.diagnostic = match &summary.state {
                                    FlowState::Complete => None,
                                    FlowState::Failed(reason) => Some(reason.clone()),
                                    state => Some(state.label().into()),
                                };
                            }
                        }
                        Ok(page)
                    })
                    .map_err(|error| error.to_string());
                self.emit(Event::RepeaterHistoryPage {
                    project,
                    query_revision,
                    tab_id,
                    result,
                })
                .await;
            }
            Command::SelectRepeaterHistory {
                project,
                operation_id,
                tab_id,
                entry,
                workspace_revision,
                draft_revision,
                cancel,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| {
                        ensure!(!operation_id.is_nil(), "历史选择缺少有效 operation ID");
                        self.store.repeater_history().load(
                            tab_id,
                            entry,
                            workspace_revision,
                            draft_revision,
                            &cancel,
                        )
                    })
                    .map_err(|error| error.to_string());
                self.emit(Event::RepeaterHistorySelected {
                    project,
                    operation_id,
                    tab_id,
                    entry,
                    workspace_revision,
                    draft_revision,
                    result,
                })
                .await;
            }
            Command::ReadFlow {
                project,
                selection_revision,
                flow_id,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| Ok((self.read_flow(flow_id)?, self.original_flow(flow_id)?)));
                self.emit(match result {
                    Ok((record, original)) => Event::FlowDetail {
                        project,
                        selection_revision,
                        record: Box::new(record),
                        original: Box::new(original),
                    },
                    Err(error) => Event::FlowDetailError {
                        project,
                        selection_revision,
                        flow_id,
                        message: error.to_string(),
                    },
                })
                .await;
            }
            Command::QueryWebSocketHistory {
                project,
                query_revision,
                filter,
                flow_id,
                offset,
                limit,
                selected_message_id,
            } => {
                self.query_websocket_history(WebSocketHistoryQuery {
                    project,
                    query_revision,
                    filter,
                    flow_id,
                    offset,
                    limit,
                    selected_message_id,
                });
            }
            Command::ReadWebSocket {
                project,
                selection_revision,
                message_id,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.read_websocket_detail(message_id));
                self.emit(match result {
                    Ok((record, original_payload)) => Event::WebSocketDetail {
                        project,
                        selection_revision,
                        record: Box::new(record),
                        original_payload,
                    },
                    Err(error) => Event::WebSocketDetailError {
                        project,
                        selection_revision,
                        message_id,
                        message: error.to_string(),
                    },
                })
                .await;
            }
            Command::ReadWebSocketNotes {
                project,
                selection_revision,
                message_id,
            } => {
                let result = self
                    .read_websocket_notes(project, message_id)
                    .map_err(|error| error.to_string());
                self.emit(Event::WebSocketNotes {
                    project,
                    selection_revision,
                    message_id,
                    result,
                })
                .await;
            }
            _ => unreachable!("routed by State::command"),
        }
        Ok(())
    }
}
