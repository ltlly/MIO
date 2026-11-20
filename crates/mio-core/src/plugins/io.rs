use super::*;

impl Running {
    pub(super) async fn frame(
        &mut self,
        envelope: Envelope,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        if self.rate_epoch.elapsed() >= Duration::from_secs(1) {
            self.rate_epoch = Instant::now();
            self.frame_count = 0;
        }
        self.frame_count += 1;
        ensure!(self.frame_count <= 512, "插件消息频率超过每秒 512 帧限额");
        envelope
            .check_channel(self.identity, self.incoming_sequence)
            .map_err(anyhow::Error::msg)?;
        self.incoming_sequence = envelope.request_id;
        if envelope.in_reply_to != 0
            && matches!(
                envelope.body,
                Some(
                    envelope::Body::Snapshot(_)
                        | envelope::Body::Patch(_)
                        | envelope::Body::Committed(_)
                        | envelope::Body::ResponseCommitted(_)
                        | envelope::Body::Ack(_)
                        | envelope::Body::Error(_)
                        | envelope::Body::HttpHookResult(_)
                        | envelope::Body::WebSocketHookResult(_)
                        | envelope::Body::SessionActionResult(_)
                        | envelope::Body::PayloadResult(_)
                        | envelope::Body::ContextMenuResult(_)
                        | envelope::Body::PassiveCheckResult(_)
                        | envelope::Body::ActiveCheckResult(_)
                )
            )
            && !self.requests.contains_key(&envelope.in_reply_to)
        {
            // A cancelled operation must not resurrect its resource or overwrite retained editors.
            return self
                .diagnostic(Failure::new(
                    ErrorCode::Stale,
                    "已丢弃无有效关联操作的迟到插件结果",
                ))
                .await;
        }
        match envelope.body.clone().context("缺少插件消息")? {
            envelope::Body::Patch(patch) => {
                ensure!(
                    envelope.revision == patch.revision,
                    "Panel envelope revision 不匹配"
                );
                if let Err(error) = self.panel.apply(&patch) {
                    if error.contains("revision") || error.contains("较新") {
                        return self.resync(outgoing, envelope.in_reply_to, error).await;
                    }
                    bail!(error);
                }
                let editor_bases = self.accept_panel_response(envelope.in_reply_to)?;
                self.publish(editor_bases).await?;
            }
            envelope::Body::Snapshot(snapshot) => {
                ensure!(
                    envelope.revision == snapshot.revision,
                    "Panel snapshot revision 不匹配"
                );
                snapshot.validate().map_err(anyhow::Error::msg)?;
                if let Err(error) = self.panel.replace(snapshot) {
                    return self.resync(outgoing, envelope.in_reply_to, error).await;
                }
                let editor_bases = self.accept_panel_response(envelope.in_reply_to)?;
                self.publish(editor_bases).await?;
            }
            envelope::Body::Committed(committed) => {
                let pending = self
                    .requests
                    .remove(&envelope.in_reply_to)
                    .context("收到无对应提交或过期的 Committed")?;
                let PendingKind::Commit {
                    resource,
                    message_revision,
                    document,
                    reply,
                } = pending.kind
                else {
                    bail!("Committed 的关联操作不是编辑提交");
                };
                let result = (|| {
                    ensure!(Instant::now() < pending.deadline, "编辑提交已超时");
                    ensure!(
                        self.opening_request.is_none(),
                        "编辑上下文正在切换；丢弃旧提交结果"
                    );
                    ensure!(
                        self.active_resource == Some((resource, message_revision)),
                        "编辑上下文已切换；丢弃旧提交结果"
                    );
                    self.broker
                        .writable(resource, message_revision)
                        .map_err(|f| anyhow::anyhow!(f.message))?;
                    ensure!(
                        protocol::uuid(&committed.resource_id).ok() == Some(resource)
                            && committed.message_revision == message_revision
                            && committed.document_id == document.document_id
                            && committed.document_revision == document.revision,
                        "编辑提交结果身份或 revision 不匹配"
                    );
                    ensure!(
                        self.editor(document.document_id)
                            .is_some_and(|e| !e.read_only && e.revision <= document.revision),
                        "编辑器在等待时已改变"
                    );
                    committed
                        .request
                        .context("提交结果没有请求")?
                        .try_into()
                        .map_err(anyhow::Error::msg)
                })();
                let _ = reply.send(result);
            }
            envelope::Body::ResponseCommitted(committed) => {
                let pending = self
                    .requests
                    .remove(&envelope.in_reply_to)
                    .context("收到无对应提交或过期的 ResponseCommitted")?;
                let PendingKind::CommitResponse {
                    resource,
                    message_revision,
                    document,
                    reply,
                } = pending.kind
                else {
                    bail!("ResponseCommitted 的关联操作不是响应编辑提交");
                };
                let result = (|| {
                    ensure!(Instant::now() < pending.deadline, "响应编辑提交已超时");
                    ensure!(
                        self.opening_request.is_none()
                            && self.active_resource == Some((resource, message_revision)),
                        "响应编辑上下文已切换；丢弃旧提交结果"
                    );
                    self.broker
                        .writable(resource, message_revision)
                        .map_err(|f| anyhow::anyhow!(f.message))?;
                    ensure!(
                        protocol::uuid(&committed.resource_id).ok() == Some(resource)
                            && committed.message_revision == message_revision
                            && committed.document_id == document.document_id
                            && committed.document_revision == document.revision,
                        "响应编辑提交结果身份或 revision 不匹配"
                    );
                    ensure!(
                        self.editor(document.document_id)
                            .is_some_and(|e| !e.read_only && e.revision <= document.revision),
                        "编辑器在等待时已改变"
                    );
                    committed
                        .response
                        .context("提交结果没有响应")?
                        .try_into()
                        .map_err(anyhow::Error::msg)
                })();
                let _ = reply.send(result);
            }
            envelope::Body::HostCall(call) => {
                if envelope.revision != self.panel.snapshot().revision {
                    let failure = Failure::new(
                        ErrorCode::Stale,
                        "服务调用来自旧面板 revision；未执行且不会自动重试",
                    );
                    self.send(
                        outgoing,
                        envelope::Body::HostResult(HostResult {
                            outcome: Some(host_result::Outcome::Error(failure.clone())),
                        }),
                        envelope.request_id,
                    )?;
                    return self.diagnostic(failure).await;
                }
                self.host_call(&envelope, call, outgoing).await?;
            }
            envelope::Body::Cancel(cancel) => {
                if let Some(service) = self.service_cancels.get(&cancel.request_id) {
                    service.cancel.cancel();
                } else {
                    self.diagnostic(Failure::new(
                        ErrorCode::Cancelled,
                        "取消的任务已结束或不属于此实例；未重试请求",
                    ))
                    .await?;
                }
            }
            envelope::Body::Ack(_) => {
                let pending = self
                    .requests
                    .remove(&envelope.in_reply_to)
                    .context("无对应事件的 Ack")?;
                match pending.kind {
                    PendingKind::Event { .. } => {}
                    PendingKind::Close { reply } => {
                        let _ = reply.send(Ok(()));
                    }
                    _ => bail!("只有 UI 事件和关闭资源可以返回 Ack；不能冒充编辑提交或打开完成"),
                }
            }
            envelope::Body::Error(failure) => {
                ensure!(
                    failure.message.len() <= 4096
                        && protocol::ErrorCode::try_from(failure.code).is_ok(),
                    "插件错误消息无效"
                );
                self.reject_response(envelope.in_reply_to, &failure.message);
                self.diagnostic(failure).await?;
            }
            envelope::Body::HttpHookResult(result) => {
                let action = HookAction::try_from(result.action).map_err(anyhow::Error::msg)?;
                self.accept_hook_result(envelope.in_reply_to, action, HookReply::Http(result))
                    .await?;
            }
            envelope::Body::WebSocketHookResult(result) => {
                let action = HookAction::try_from(result.action).map_err(anyhow::Error::msg)?;
                self.accept_hook_result(envelope.in_reply_to, action, HookReply::Ws(result))
                    .await?;
            }
            envelope::Body::SessionActionResult(result) => {
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::Session(result),
                )
                .await?;
            }
            envelope::Body::PayloadResult(result) => {
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::Payload(result),
                )
                .await?;
            }
            envelope::Body::ContextMenuResult(result) => {
                if result.open_repeater.is_some()
                    && !self.broker.grants.contains(&Permission::RepeaterOpen)
                {
                    let msg = "插件未获授 repeater.open；拒绝由上下文菜单打开 Repeater";
                    self.reject_response(envelope.in_reply_to, msg);
                    return self.diagnostic(Failure::new(ErrorCode::Denied, msg)).await;
                }
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::ContextMenu(result),
                )
                .await?;
            }
            envelope::Body::PassiveCheckResult(result) => {
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::PassiveCheck(result),
                )
                .await?;
            }
            envelope::Body::ActiveCheckResult(result) => {
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::ActiveCheck(result),
                )
                .await?;
            }
            _ => bail!("worker 发送了不允许的宿主方向消息"),
        }
        Ok(())
    }

    pub(super) async fn accept_hook_result(
        &mut self,
        request_id: u64,
        action: HookAction,
        value: HookReply,
    ) -> Result<()> {
        let Some(pending) = self.requests.remove(&request_id) else {
            return self
                .diagnostic(Failure::new(
                    ErrorCode::Stale,
                    "已丢弃无有效关联操作的迟到 Hook 结果",
                ))
                .await;
        };
        let PendingKind::Hook {
            cancelled, reply, ..
        } = pending.kind
        else {
            bail!("Hook 结果的关联操作不是 Hook 调用");
        };
        if action != HookAction::Pass && !self.broker.grants.contains(&Permission::TrafficModify) {
            let msg = "插件未获授 traffic.modify；拒绝修改、拦截或丢弃流量";
            let _ = reply.send(Err(anyhow::anyhow!(msg)));
            return self.diagnostic(Failure::new(ErrorCode::Denied, msg)).await;
        }
        if cancelled.is_cancelled() {
            let _ = reply.send(Err(anyhow::anyhow!("插件 Hook 调用已取消")));
        } else if Instant::now() >= pending.deadline {
            let _ = reply.send(Err(anyhow::anyhow!("插件 Hook 调用已超时")));
        } else {
            let _ = reply.send(Ok(value));
        }
        Ok(())
    }

    pub(super) fn accept_panel_response(&mut self, request_id: u64) -> Result<Vec<EditorSnapshot>> {
        if let Some(pending) = self.requests.remove(&request_id) {
            match pending.kind {
                PendingKind::Commit { .. } | PendingKind::CommitResponse { .. } => {
                    bail!("CommitEditor 必须返回明确的提交结果")
                }
                PendingKind::Open {
                    resource,
                    revision,
                    read_only,
                    reserved,
                    reply,
                } => {
                    self.broker.release_resource(reserved);
                    let result = (|| {
                        ensure!(
                            self.opening_request == Some(request_id),
                            "打开请求已被新的上下文取代"
                        );
                        ensure!(Instant::now() < pending.deadline, "打开请求已超时");
                        self.broker.bind_resource(resource, revision, read_only)?;
                        self.active_resource = Some((resource, revision));
                        Ok(Arc::new(self.panel.snapshot().clone()))
                    })();
                    if self.opening_request == Some(request_id) {
                        self.opening_request = None;
                    }
                    let _ = reply.send(result);
                }
                PendingKind::Event { editor_bases } => {
                    ensure!(Instant::now() < pending.deadline, "插件 UI 事件已超时");
                    return Ok(editor_bases);
                }
                PendingKind::Close { reply } => {
                    let _ = reply.send(Ok(()));
                }
                PendingKind::Hook { .. } => bail!("Hook 调用不能返回面板响应"),
            }
        }
        Ok(vec![])
    }
}
