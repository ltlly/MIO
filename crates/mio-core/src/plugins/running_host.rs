use super::*;

impl Running {
    pub(super) async fn host_call(
        &mut self,
        envelope: &Envelope,
        call: HostCall,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        let mut operation = match self.broker.authorize(envelope, call) {
            Ok(operation) => operation,
            Err(failure) => {
                self.send(
                    outgoing,
                    envelope::Body::HostResult(HostResult {
                        outcome: Some(host_result::Outcome::Error(failure.clone())),
                    }),
                    envelope.request_id,
                )?;
                return self.diagnostic(failure).await;
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
        let send_denied = |this: &mut Self, msg: String| {
            this.send(
                outgoing,
                envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Error(Failure::new(
                        ErrorCode::Denied,
                        msg,
                    ))),
                }),
                envelope.request_id,
            )
        };
        if self.service_cancels.len() >= MAX_SERVICE_CALLS {
            send_denied(self, "插件宿主调用超过并发限额".into())?;
            return Ok(());
        }
        let wire_remaining = Duration::from_millis(
            envelope
                .deadline_unix_ms
                .saturating_sub(protocol::unix_ms()),
        );
        let deadline = Instant::now() + wire_remaining.min(MAX_OPERATION);
        let cancel = self.cancel.child_token();
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
        if creates_resource && let Err(error) = self.broker.reserve_resource(None) {
            send_denied(self, error.to_string())?;
            return Ok(());
        }
        let request = ServiceRequest {
            identity: self.identity,
            plugin_id: self.plugin_id.clone(),
            request_id: envelope.request_id,
            deadline,
            operation,
            cancel: cancel.clone(),
            reply,
        };
        if self.services.try_send(request).is_err() {
            self.broker.release_resource(creates_resource);
            send_denied(self, "宿主服务队列已满或关闭".into())?;
            return Ok(());
        }
        self.service_cancels.insert(
            envelope.request_id,
            ActiveService {
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
            ServiceCompletion { request_id, resource_binding, result }
        });
        Ok(())
    }
}
