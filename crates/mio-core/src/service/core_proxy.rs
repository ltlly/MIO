use super::*;

impl State {
    pub(super) async fn capture(&mut self, event: ProxyEvent) -> Result<()> {
        let result = self.capture_without_waiting_for_ui(event, false);
        self.flush_proxy_notifications().await;
        result
    }

    pub(super) fn capture_without_waiting_for_ui(
        &mut self,
        event: ProxyEvent,
        stopping: bool,
    ) -> Result<()> {
        match event {
            ProxyEvent::WebSocketMessage {
                project,
                record,
                original_payload,
                listener_port,
            } => {
                if project == self.project
                    && let Some(pending) = self.pending_websockets.remove(&record.id)
                    && !pending.reply.is_closed()
                    && pending
                        .reply
                        .send(mio_proxy::WebSocketInterceptDecision::Forward(
                            record.payload.clone(),
                        ))
                        .is_err()
                {
                    self.queue_proxy_notification(Event::WebSocketInterceptResolved {
                        project,
                        message_id: record.id,
                    });
                }
                return self.capture_websocket(
                    project,
                    CapturedWebSocket {
                        record: record.into(),
                        listener_port: Some(listener_port),
                        original_payload: original_payload.map(Arc::new),
                    },
                );
            }
            ProxyEvent::Captured {
                project,
                mut record,
                response_stream,
                listener_port,
                capture_metrics,
            } => {
                if project != self.project {
                    return Ok(());
                }
                let ws_opt = self.store.workspace_arc().ok();
                let skip_history = ws_opt.as_ref().is_some_and(|ws| {
                    ws.proxy_messages.misc.disable_history_logging
                        || (ws.proxy_messages.misc.disable_out_of_scope_history_logging
                            && !ws.scope.is_url_in_scope(&record.request.url))
                });
                if record.summary.state != FlowState::Intercepted
                    && self.pending.remove(&record.summary.id).is_some()
                {
                    self.queue_proxy_notification(Event::InterceptResolved {
                        project,
                        flow_id: record.summary.id,
                    });
                }
                if skip_history {
                    return Ok(());
                }
                let in_streaming_scope = ws_opt.as_ref().is_some_and(|ws| {
                    ws.network_http
                        .is_url_in_streaming_scope(&record.request.url)
                });
                let suppress_stream_body = ws_opt
                    .as_ref()
                    .is_some_and(|ws| !ws.network_http.store_streaming_responses)
                    && (response_stream.is_some() || in_streaming_scope);
                if suppress_stream_body && let Some(resp) = record.response.as_mut() {
                    resp.body.clear();
                    record.summary.response_bytes = 0;
                }
                let saved = self.persist_capture_with_metrics(
                    &record,
                    if suppress_stream_body {
                        None
                    } else {
                        response_stream.as_ref()
                    },
                    listener_port,
                    &capture_metrics,
                );
                saved?;
                if !stopping {
                    let event = self.audit_inspect_live_flow(&record);
                    self.emit_audit_if_capacity(event);
                    self.spawn_plugin_passive_checks(&record);
                }
            }
            ProxyEvent::Intercepted {
                project,
                record,
                reply,
                response_head_only,
                response_stream,
                listener_port,
                capture_metrics,
            } => {
                if project != self.project {
                    return Ok(());
                }
                self.persist_capture_with_metrics(&record, response_stream.as_ref(), listener_port, &capture_metrics)?;
                if !stopping && !reply.is_closed() {
                    let stage = if record.response.is_some() {
                        InterceptStage::Response
                    } else {
                        InterceptStage::Request
                    };
                    self.pending.insert(
                        record.summary.id,
                        Pending {
                            revision: record.summary.revision,
                            stage,
                            response_head_only,
                            intercepted_request: record.request.clone(),
                            intercepted_response: record.response.clone(),
                            reply,
                        },
                    );
                    self.queue_proxy_notification(Event::InterceptPending {
                        project,
                        stage,
                        response_head_only,
                        record: Box::new(record),
                    });
                }
            }
            ProxyEvent::WebSocketIntercepted {
                project,
                url,
                record,
                reply,
                ..
            } => {
                if project != self.project {
                    return Ok(());
                }
                if !stopping && !reply.is_closed() {
                    let flow_id = record.flow_id;
                    let kind = record.kind;
                    self.pending_websockets.insert(
                        record.id,
                        PendingWebSocket {
                            flow_id,
                            kind,
                            reply,
                        },
                    );
                    self.queue_proxy_notification(Event::WebSocketInterceptPending {
                        project,
                        url,
                        record: Box::new(record),
                    });
                }
            }
            ProxyEvent::WebSocketInterceptResolved {
                project,
                message_id,
            } => {
                if project != self.project {
                    return Ok(());
                }
                if self.pending_websockets.remove(&message_id).is_some() {
                    self.queue_proxy_notification(Event::WebSocketInterceptResolved {
                        project,
                        message_id,
                    });
                }
            }
        }
        if !self.history_dirty {
            self.history_dirty = true;
            self.queue_proxy_notification(Event::HistoryChanged {
                project: self.project,
            });
        }
        Ok(())
    }

    pub(super) fn proxy_finished(&mut self, result: Result<(), mio_proxy::ProxyError>) {
        if let Some(proxy) = &self.proxy
            && !proxy.is_joined()
        {
            proxy.request_stop();
        }
        for (_, proxy) in &self.extra_proxies {
            if !proxy.is_joined() {
                proxy.request_stop();
            }
        }
        let all_joined = self.proxy.as_ref().is_none_or(ProxyHandle::is_joined)
            && self
                .extra_proxies
                .iter()
                .all(|(_, proxy)| proxy.is_joined());
        if let Err(error) = self.finish_proxy_stop(all_joined, result.map_err(anyhow::Error::from))
        {
            self.queue_proxy_notification(Event::Error {
                project: Some(self.project),
                operation: "stop_proxy",
                message: error.to_string(),
            });
        }
    }

    pub(super) async fn stop_proxy(&mut self) -> Result<()> {
        if let Some(proxy) = self.proxy.as_mut() {
            proxy.request_stop();
        }
        for (_, proxy) in &self.extra_proxies {
            proxy.request_stop();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut joined = true;
        let mut result = Ok(());
        if let Some(proxy) = self.proxy.as_mut() {
            let stop = tokio::time::timeout_at(deadline, proxy.stop())
                .await
                .unwrap_or(Err(mio_proxy::ProxyError::StopDeadline));
            joined &= proxy.is_joined();
            result = stop.map_err(anyhow::Error::from);
        }
        for (_, proxy) in &mut self.extra_proxies {
            let stop = tokio::time::timeout_at(deadline, proxy.stop())
                .await
                .unwrap_or(Err(mio_proxy::ProxyError::StopDeadline));
            joined &= proxy.is_joined();
            if result.is_ok() {
                result = stop.map_err(anyhow::Error::from);
            }
        }
        self.finish_proxy_stop(joined, result)
    }

    pub(super) async fn finish_proxy_shutdown(&mut self) -> Result<()> {
        if self.proxy.is_none() && self.extra_proxies.is_empty() {
            return Ok(());
        }
        if let Some(proxy) = &self.proxy {
            proxy.request_stop();
        }
        for (_, proxy) in &self.extra_proxies {
            proxy.request_stop();
        }
        let mut result = match &mut self.proxy {
            Some(proxy) => proxy.join().await,
            None => Ok(()),
        };
        for (_, proxy) in &mut self.extra_proxies {
            if let Err(err) = proxy.join().await
                && result.is_ok()
            {
                result = Err(err);
            }
        }
        self.finish_proxy_stop(true, result.map_err(anyhow::Error::from))
    }

    pub(super) fn finish_proxy_stop(&mut self, joined: bool, mut result: Result<()>) -> Result<()> {
        while let Ok(event) = self.proxy_events.try_recv() {
            if let Err(error) = self.capture_without_waiting_for_ui(event, true) {
                self.queue_proxy_notification(Event::Error {
                    project: Some(self.project),
                    operation: "capture",
                    message: error.to_string(),
                });
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
        let mut terminal = self
            .proxy
            .as_mut()
            .map(ProxyHandle::drain_terminal_records)
            .unwrap_or_default();
        for (_, proxy) in &mut self.extra_proxies {
            terminal.extend(proxy.drain_terminal_records());
        }
        for capture in terminal {
            if let Err(error) = self.capture_without_waiting_for_ui(
                ProxyEvent::Captured {
                    project: self.project,
                    record: capture.record,
                    response_stream: capture.response_stream,
                    listener_port: capture.listener_port,
                    capture_metrics: capture.capture_metrics,
                },
                true,
            ) {
                self.queue_proxy_notification(Event::Error {
                    project: Some(self.project),
                    operation: "capture",
                    message: error.to_string(),
                });
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
        if joined {
            self.proxy = None;
            self.primary_proxy_id = None;
            self.extra_proxies.clear();
            self.ws_repeater.set_injectors(None);
            for (flow_id, _) in self.pending.drain().collect::<Vec<_>>() {
                self.queue_proxy_notification(Event::InterceptResolved {
                    project: self.project,
                    flow_id,
                });
            }
            for (message_id, pending) in self.pending_websockets.drain().collect::<Vec<_>>() {
                let _ = pending.reply.is_closed();
                self.queue_proxy_notification(Event::WebSocketInterceptResolved {
                    project: self.project,
                    message_id,
                });
            }
            if self.unsaved_count() == 0
                && let Err(error) = self.store.finish_pending(false, true)
                && result.is_ok()
            {
                result = Err(error);
            }
        }
        if !self.history_dirty {
            self.history_dirty = true;
            self.queue_proxy_notification(Event::HistoryChanged {
                project: self.project,
            });
        }
        self.queue_proxy_notification(Event::ProxyState {
            project: self.project,
            address: self.proxy.as_ref().map(ProxyHandle::address),
            cancel: self.proxy.as_ref().map(ProxyHandle::cancellation_token),
            intercept_requests: self.intercept_requests,
            intercept_responses: self.intercept_responses,
        });
        result
    }
}
