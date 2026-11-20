use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_traffic_and_intercept_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<ControlFlow<()>, Box<Event>> {
        match event {
            ref event @ Event::FlowDetail {
                project,
                selection_revision,
                ..
            } if self.project == Some(project)
                && self.has_intruder_result_read(selection_revision) =>
            {
                self.on_intruder_result_event(event, window, cx);
            }
            Event::FlowDetailError {
                project,
                selection_revision,
                flow_id,
                message,
            } if self.project == Some(project)
                && self.has_intruder_result_read(selection_revision) =>
            {
                self.on_intruder_detail_error(selection_revision, flow_id, &message, cx);
            }
            Event::FlowDetailError {
                project,
                selection_revision,
                flow_id,
                message,
            } if self.project == Some(project)
                && self.has_intruder_transfer(selection_revision) =>
            {
                self.receive_intruder_transfer_error(selection_revision, flow_id, &message, cx);
            }
            Event::FlowDetailError {
                project,
                selection_revision,
                flow_id,
                ref message,
            } if self.on_audit_flow_detail_error(
                project,
                selection_revision,
                flow_id,
                message,
                window,
                cx,
            ) => {}
            Event::FlowDetailError {
                project,
                selection_revision,
                flow_id,
                message,
            } => self.on_flow_detail_error(project, selection_revision, flow_id, &message, cx),
            Event::FlowDetail {
                project,
                selection_revision,
                record,
                original,
            } if self.project == Some(project)
                && self.has_intruder_transfer(selection_revision) =>
            {
                self.receive_intruder_transfer(selection_revision, &record, &original, window, cx);
            }
            Event::FlowDetail {
                project,
                selection_revision,
                record,
                original,
            } if self.project == Some(project)
                && self.repeater_reads.contains_key(&selection_revision) =>
            {
                let id = self.repeater_reads[&selection_revision];
                if self
                    .workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.id == id && tab.last_flow == Some(record.summary.id))
                    && let Some(editors) = self.tab_editors.get_mut(&id)
                {
                    self.repeater_reads.remove(&selection_revision);
                    editors.response.load(
                        &record
                            .response
                            .as_ref()
                            .map(response_to_raw)
                            .unwrap_or_default(),
                        window,
                        cx,
                    );
                    editors.result = Some(format!(
                        "{} · {} ms · {} 字节",
                        record.summary.state.label(),
                        record.summary.elapsed_ms,
                        record.summary.response_bytes
                    ));
                    editors.response_revision = self
                        .workspace
                        .tabs
                        .iter()
                        .find(|tab| tab.id == id)
                        .and_then(|tab| tab.history.as_ref())
                        .and_then(|history| history.cursor.as_ref())
                        .map(|cursor| cursor.draft_revision);
                    editors.loaded_flow = Some(record);
                    editors.loaded_original = original;
                }
            }
            Event::FlowDetail {
                project,
                selection_revision,
                record,
                original,
            } if self.project == Some(project)
                && selection_revision == self.history_detail_revision
                && self.selected_flow == Some(record.summary.id)
                && self
                    .detail
                    .as_ref()
                    .is_none_or(|loaded| loaded.summary.revision <= record.summary.revision) =>
            {
                self.record_logger_finished(&record, window, cx);
                if self
                    .history
                    .read(cx)
                    .delegate()
                    .row_by_id(HistoryEntryId::Live(record.summary.id))
                    .and_then(HistoryRow::live)
                    .is_some_and(|row| row.revision <= record.summary.revision)
                {
                    let latest = &record.summary;
                    self.history.update(cx, |table, cx| {
                        for row in table.delegate_mut().cached_rows_mut() {
                            if let HistoryRow::Live(row) = row
                                && row.id == latest.id
                                && row.revision <= latest.revision
                            {
                                row.revision = latest.revision;
                                row.state = latest.state.clone();
                                row.elapsed_ms = latest.elapsed_ms;
                                row.request_bytes = latest.request_bytes;
                                row.response_bytes = latest.response_bytes;
                            }
                        }
                        cx.notify();
                    });
                    self.detail = Some(record);
                    self.detail_error = None;
                    self.detail_original = original;
                    self.reload_history_detail_versions(window, cx);
                } else {
                    self.detail_error = Some("消息版本已改变，请重试读取最新内容。".into());
                }
            }
            Event::FlowDetail {
                project,
                selection_revision,
                record,
                original,
            } if self.project == Some(project)
                && self.extra_tools.logger_pending_read
                    == Some((project, selection_revision, record.summary.id)) =>
            {
                self.extra_tools.logger_pending_read = None;
                self.record_logger_flow(&record.summary);
                if self.extra_tools.logger_selected == Some(record.summary.id) {
                    self.extra_tools.logger_request.load(
                        &request_to_raw(&record.request),
                        window,
                        cx,
                    );
                    self.extra_tools.logger_response.load(
                        &record
                            .response
                            .as_ref()
                            .map(response_to_raw)
                            .unwrap_or_default(),
                        window,
                        cx,
                    );
                    self.extra_tools.logger_loaded_detail = Some((*record, *original));
                }
                self.advance_logger_batch_op(window, cx);
            }
            Event::FlowDetail {
                project,
                selection_revision,
                ref record,
                ref original,
            } if self.on_target_flow_detail(
                project,
                selection_revision,
                record,
                original,
                window,
                cx,
            ) => {}
            Event::FlowDetail {
                project,
                selection_revision,
                ref record,
                ..
            } if self.on_audit_flow_detail(project, selection_revision, record, window, cx) => {}
            Event::ProxyState {
                project,
                address,
                cancel,
                intercept_requests,
                intercept_responses,
            } if self.project == Some(project) => {
                self.proxy_address = address;
                self.proxy_cancel = cancel;
                self.proxy_busy = self
                    .proxy_cancel
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled);
                self.intercept_requests = intercept_requests;
                self.intercept_responses = intercept_responses;
                if let Some(addr) = address {
                    if self.proxy_listeners_bound.is_empty() {
                        self.proxy_listeners_bound
                            .push((mio_core::Uuid::from_u128(1), addr));
                    }
                } else {
                    self.proxy_listeners_bound.clear();
                    self.websocket_repeater_proxy_stopped();
                }
                self.status = if self.proxy_busy {
                    "正在停止 Proxy；等待后台确认。".into()
                } else {
                    address.map_or_else(
                        || "Proxy 已停止".into(),
                        |address| format!("Proxy 正在监听 {address}"),
                    )
                };
            }
            Event::ProxyListeners { project, listeners } if self.project == Some(project) => {
                self.proxy_listeners_bound = listeners;
                if !self.proxy_busy && self.proxy_listeners_bound.len() > 1 {
                    let joined = self
                        .proxy_listeners_bound
                        .iter()
                        .map(|(_, addr)| addr.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.status = format!(
                        "Proxy 正在监听 {} 个端口：{joined}",
                        self.proxy_listeners_bound.len()
                    );
                }
            }
            Event::BrowserLaunched {
                project,
                address,
                executable,
            } if self.project == Some(project) => {
                self.error = None;
                self.status = format!("已启动配置浏览器（代理 {address}）：{executable}");
            }
            Event::LiveFlowAnnotated {
                project,
                flow_id,
                annotated,
            } if self.project == Some(project) => {
                if annotated {
                    self.annotated_live_flows.insert(flow_id);
                } else {
                    self.annotated_live_flows.remove(&flow_id);
                }
            }
            Event::InterceptPending {
                project,
                stage,
                response_head_only,
                record,
            } if self.project == Some(project) => {
                self.record_logger_finished(&record, window, cx);
                let id = record.summary.id;
                let raw = match stage {
                    InterceptStage::Request => request_to_raw(&record.request),
                    InterceptStage::Response => record
                        .response
                        .as_ref()
                        .map(response_to_raw)
                        .unwrap_or_default(),
                };
                let mut editor = MessageEditor::new(window, cx);
                editor.load(&raw, window, cx);
                if self.selected_intercept == Some(id) {
                    self.clear_context_viewer("intercept", cx);
                }
                self.intercepts.retain(|pending| pending.id() != id);
                self.intercepts.push(PendingIntercept {
                    target: InterceptTarget::Http {
                        stage,
                        response_head_only,
                        record,
                    },
                    editor,
                    deciding: false,
                });
                self.selected_intercept.get_or_insert(id);
            }
            Event::InterceptResolved { project, flow_id } if self.project == Some(project) => {
                self.resolve_intercept(flow_id, window, cx);
            }
            Event::WebSocketInterceptPending {
                project,
                url,
                record,
            } if self.project == Some(project) => {
                let id = record.id;
                let mut editor = MessageEditor::new(window, cx);
                editor.load(&record.payload, window, cx);
                if self.selected_intercept == Some(id) {
                    self.clear_context_viewer("intercept", cx);
                }
                self.intercepts.retain(|pending| pending.id() != id);
                self.intercepts.push(PendingIntercept {
                    target: InterceptTarget::WebSocket { url, record },
                    editor,
                    deciding: false,
                });
                self.selected_intercept.get_or_insert(id);
            }
            Event::WebSocketInterceptResolved {
                project,
                message_id,
            } if self.project == Some(project) => {
                self.resolve_intercept(message_id, window, cx);
            }
            event => return Err(Box::new(event)),
        }
        Ok(ControlFlow::Continue(()))
    }
}
