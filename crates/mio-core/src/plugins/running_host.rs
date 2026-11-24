use super::*;

impl Running {
    pub(super) async fn host_call(
        &mut self,
        envelope: &Envelope,
        call: HostCall,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        let panel_id = envelope.panel_id;
        let parent_req = call.parent_request_id;
        let now = Instant::now();
        let send_error = |this: &mut Self, code: ErrorCode, msg: String| {
            this.send_on_panel(
                outgoing,
                panel_id,
                envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Error(Failure::new(code, msg))),
                }),
                envelope.request_id,
            )
        };
        let (parent_cancel, parent_deadline) = if parent_req != 0 {
            let valid = self
                .requests
                .get(&parent_req)
                .filter(|p| p.panel_id == panel_id)
                .map(|p| (p.cancel.clone(), p.deadline));
            match valid {
                None => {
                    let cancel_deadline = self
                        .cancel_notifications
                        .iter()
                        .rev()
                        .find(|n| n.panel_id == panel_id && n.cancel_frame_id == parent_req)
                        .map(|n| n.deadline);
                    if let Some(cancel_deadline) = cancel_deadline {
                        if cancel_deadline <= now {
                            send_error(
                                self,
                                ErrorCode::Deadline,
                                "取消清理回调期限已过；拒绝宿主调用".into(),
                            )?;
                            return Ok(());
                        }
                        let allowed_in_cancel = matches!(
                            call.operation,
                            Some(
                                host_call::Operation::QueryHistory(_)
                                    | host_call::Operation::ReadFlow(_)
                                    | host_call::Operation::ScopeCheck(_)
                                    | host_call::Operation::QuerySiteMap(_)
                                    | host_call::Operation::ReadAnnotations(_)
                                    | host_call::Operation::StorageGet(_)
                                    | host_call::Operation::StorageSet(_)
                                    | host_call::Operation::PreferencesGet(_)
                                    | host_call::Operation::PreferencesSet(_)
                            )
                        );
                        if !allowed_in_cancel {
                            send_error(
                                self,
                                ErrorCode::Cancelled,
                                "取消清理回调仅允许只读查询与 KV/偏好清理操作".into(),
                            )?;
                            return Ok(());
                        }
                        (None, Some(cancel_deadline))
                    } else if self.is_request_cancelled(panel_id, parent_req) {
                        send_error(
                            self,
                            ErrorCode::Cancelled,
                            "父请求已取消；拒绝派生宿主调用".into(),
                        )?;
                        return Ok(());
                    } else {
                        send_error(
                            self,
                            ErrorCode::Stale,
                            "未找到存活的同面板父请求；拒绝派生宿主调用".into(),
                        )?;
                        return Ok(());
                    }
                }
                Some((token, _)) if token.is_cancelled() => {
                    send_error(
                        self,
                        ErrorCode::Cancelled,
                        "父请求已取消；拒绝派生宿主调用".into(),
                    )?;
                    return Ok(());
                }
                Some((_, p_deadline)) if p_deadline <= now => {
                    let (
                        is_main_resync,
                        is_main_pointer_cancel,
                        is_hook,
                        is_destroy_close,
                        needs_new_resync,
                        pointer_capture,
                    ) = self.requests.get(&parent_req).map_or(
                        (
                            false,
                            false,
                            false,
                            false,
                            false,
                            PointerCaptureTransition::None,
                        ),
                        |p| {
                            let is_main_resync = panel_id == PANEL_ID
                                && matches!(
                                    p.kind,
                                    PendingKind::Event {
                                        resync_seq: Some(_),
                                        ..
                                    }
                                );
                            let is_main_pointer_cancel = panel_id == PANEL_ID
                                && matches!(
                                    p.kind,
                                    PendingKind::Event {
                                        pointer_cancel_seq: Some(_),
                                        ..
                                    }
                                );
                            let is_hook = matches!(p.kind, PendingKind::Hook { .. });
                            let is_destroy_close = matches!(
                                p.kind,
                                PendingKind::Close {
                                    remove_panel: true,
                                    ..
                                }
                            );
                            let needs_new_resync = matches!(
                                p.kind,
                                PendingKind::Open { .. }
                                    | PendingKind::Event {
                                        resync_seq: None,
                                        pointer_cancel_seq: None,
                                        ..
                                    }
                                    | PendingKind::Close {
                                        remove_panel: false,
                                        ..
                                    }
                            );
                            let pointer_capture = match p.kind {
                                PendingKind::Event {
                                    resync_seq: None,
                                    pointer_capture,
                                    ..
                                } => pointer_capture,
                                _ => PointerCaptureTransition::None,
                            };
                            (
                                is_main_resync,
                                is_main_pointer_cancel,
                                is_hook,
                                is_destroy_close,
                                needs_new_resync,
                                pointer_capture,
                            )
                        },
                    );
                    if panel_id == PANEL_ID
                        && !is_main_resync
                        && !is_main_pointer_cancel
                        && !is_hook
                    {
                        return self.abort_fatal_timeout(
                            &[(panel_id, parent_req)],
                            outgoing,
                            Some((parent_req, "主面板操作已超时；已取消并回收实例")),
                        );
                    }
                    if is_hook {
                        self.cancel_pending_request(
                            parent_req,
                            "插件 Hook 调用已超时",
                            Some(outgoing),
                            true,
                            false,
                        );
                        self.flush_pending_revokes(outgoing)?;
                        self.flush_pending_resyncs(outgoing)?;
                        self.flush_deferred_panel_commands(outgoing).await?;
                        self.check_fatal_timeouts(Instant::now(), outgoing)?;
                    } else if panel_id == PANEL_ID && (is_main_resync || is_main_pointer_cancel) {
                        self.cancel_pending_request(
                            parent_req,
                            if is_main_resync {
                                "主面板重同步操作超时"
                            } else {
                                "主面板指针手势取消操作超时"
                            },
                            Some(outgoing),
                            true,
                            true,
                        );
                        self.flush_pending_revokes(outgoing)?;
                        self.flush_pending_resyncs(outgoing)?;
                        self.flush_deferred_panel_commands(outgoing).await?;
                        self.check_fatal_timeouts(Instant::now(), outgoing)?;
                    } else if panel_id != PANEL_ID {
                        let retracted_unsent = self.cancel_pending_request(
                            parent_req,
                            "插件子面板操作超时；已取消此面板请求，未影响共享实例",
                            Some(outgoing),
                            !is_destroy_close,
                            true,
                        );
                        self.settle_pointer_capture_on_failure(
                            panel_id,
                            pointer_capture,
                            retracted_unsent,
                        );
                        if needs_new_resync {
                            self.schedule_panel_resync(panel_id);
                        }
                        self.flush_pending_revokes(outgoing)?;
                        self.flush_pending_resyncs(outgoing)?;
                        self.flush_deferred_panel_commands(outgoing).await?;
                        self.check_fatal_timeouts(Instant::now(), outgoing)?;
                    }
                    send_error(
                        self,
                        ErrorCode::Deadline,
                        "父请求已超时；拒绝派生宿主调用".into(),
                    )?;
                    return Ok(());
                }
                Some((token, p_deadline)) => (Some(token), Some(p_deadline)),
            }
        } else if panel_id != PANEL_ID {
            let mut active: Option<(CancellationToken, Instant)> = None;
            for p in self.requests.values().filter(|p| {
                p.panel_id == panel_id
                    && matches!(
                        &p.kind,
                        PendingKind::Commit { .. } | PendingKind::CommitResponse { .. }
                    )
            }) {
                if p.cancel.is_cancelled() {
                    send_error(
                        self,
                        ErrorCode::Cancelled,
                        "子面板操作已取消；拒绝派生宿主调用".into(),
                    )?;
                    return Ok(());
                }
                if p.deadline <= now {
                    send_error(
                        self,
                        ErrorCode::Deadline,
                        "子面板操作已超时；拒绝派生宿主调用".into(),
                    )?;
                    return Ok(());
                }
                if active.is_none() {
                    active = Some((p.cancel.clone(), p.deadline));
                }
            }
            match active {
                Some((token, p_deadline)) => (Some(token), Some(p_deadline)),
                None => (None, None),
            }
        } else {
            (None, None)
        };
        let mut operation = match self.broker.authorize(envelope, call) {
            Ok(operation) => operation,
            Err(failure) => {
                self.send_on_panel(
                    outgoing,
                    panel_id,
                    envelope::Body::HostResult(HostResult {
                        outcome: Some(host_result::Outcome::Error(failure.clone())),
                    }),
                    envelope.request_id,
                )?;
                return self.diagnostic_on_panel(panel_id, failure).await;
            }
        };
        if let ServiceOperation::NetworkSend { request } = &mut operation {
            let hook_depth = self
                .requests
                .values()
                .filter_map(|p| match p.kind {
                    PendingKind::Hook { depth, .. } => Some(depth),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            if hook_depth > 0 {
                let existing = request
                    .headers
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case("x-mio-plugin-depth"))
                    .and_then(|h| std::str::from_utf8(&h.value).ok())
                    .and_then(|v| v.trim().parse::<u8>().ok())
                    .unwrap_or(0);
                if existing < hook_depth {
                    request
                        .headers
                        .retain(|h| !h.name.eq_ignore_ascii_case("x-mio-plugin-depth"));
                    request.headers.push(mio_plugin_api::Header {
                        name: "x-mio-plugin-depth".into(),
                        value: hook_depth.to_string().into_bytes(),
                    });
                }
            }
        }
        if self.service_cancels.len() >= MAX_SERVICE_CALLS {
            send_error(
                self,
                ErrorCode::Denied,
                "插件宿主调用超过并发限额".into(),
            )?;
            return Ok(());
        }
        let wire_remaining = Duration::from_millis(
            envelope
                .deadline_unix_ms
                .saturating_sub(protocol::unix_ms()),
        );
        let mut deadline = now + wire_remaining.min(MAX_OPERATION);
        if let Some(p_deadline) = parent_deadline {
            deadline = deadline.min(p_deadline);
        }
        if deadline <= now {
            send_error(
                self,
                ErrorCode::Deadline,
                "父请求剩余期限不足；拒绝派生宿主调用".into(),
            )?;
            return Ok(());
        }
        let cancel = parent_cancel.map_or_else(
            || self.cancel.child_token(),
            |token| token.child_token(),
        );
        let (reply, response) = oneshot::channel();
        let resource_binding = match &operation {
            ServiceOperation::UpdateDraft {
                tab_id,
                expected_revision,
                ..
            } => Some(PendingResource::UpdateDraft {
                id: *tab_id,
                revision: *expected_revision,
            }),
            ServiceOperation::OpenRepeater { .. } => Some(PendingResource::OpenRepeater),
            _ => None,
        };
        let creates_resource = matches!(resource_binding, Some(PendingResource::OpenRepeater));
        if creates_resource && let Err(error) = self.broker.reserve_panel_resource(panel_id, None) {
            send_error(self, ErrorCode::Denied, error.to_string())?;
            return Ok(());
        }
        let request = ServiceRequest {
            identity: self.identity,
            plugin_id: self.plugin_id.clone(),
            panel_id,
            request_id: envelope.request_id,
            deadline,
            operation,
            cancel: cancel.clone(),
            reply,
        };
        if self.services.try_send(request).is_err() {
            self.broker
                .release_panel_resource(panel_id, creates_resource);
            send_error(self, ErrorCode::Denied, "宿主服务队列已满或关闭".into())?;
            return Ok(());
        }
        self.service_cancels.insert(
            envelope.request_id,
            ActiveService {
                panel_id,
                parent_request_id: parent_req,
                cancel: cancel.clone(),
                binding: resource_binding,
            },
        );
        let request_id = envelope.request_id;
        self.service_tasks.spawn(async move {
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(Failure::new(ErrorCode::Cancelled, "插件服务调用已取消")),
                result = timeout_at(deadline, response) => match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => Err(Failure::new(ErrorCode::Internal, "宿主服务未返回结果")),
                    Err(_) => { cancel.cancel(); Err(Failure::new(ErrorCode::Deadline, "插件服务调用超时，已取消相关 I/O")) },
                },
            };
            ServiceCompletion { request_id, panel_id, resource_binding, result }
        });
        Ok(())
    }
}
