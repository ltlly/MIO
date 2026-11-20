use super::*;

impl Running {
    pub(super) fn envelope(&mut self, body: envelope::Body, in_reply_to: u64) -> Result<Envelope> {
        self.outgoing_sequence = self
            .outgoing_sequence
            .checked_add(1)
            .context("插件消息序号耗尽")?;
        let mut envelope = Envelope::new(
            self.identity,
            self.outgoing_sequence,
            self.panel.snapshot().revision,
            body,
            MAX_OPERATION,
        );
        envelope.in_reply_to = in_reply_to;
        Ok(envelope)
    }

    pub(super) async fn emit(&self, event: PluginEvent) -> Result<()> {
        tokio::select! { biased; _ = self.cancel.cancelled() => Ok(()), result = timeout(MAX_OPERATION, self.events.send(event)) => result.context("插件事件消费超时")?.context("插件事件接收方已关闭") }
    }

    pub(super) async fn diagnostic(&self, failure: Failure) -> Result<()> {
        self.emit(PluginEvent::Error {
            identity: self.identity,
            failure,
        })
        .await
    }

    pub(super) async fn publish(&self, editor_bases: Vec<EditorSnapshot>) -> Result<()> {
        self.emit(PluginEvent::Panel {
            identity: self.identity,
            snapshot: Arc::new(self.panel.snapshot().clone()),
            editor_bases,
        })
        .await
    }

    pub(super) fn send(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        body: envelope::Body,
        in_reply_to: u64,
    ) -> Result<u64> {
        let envelope = self.envelope(body, in_reply_to)?;
        envelope.validate().map_err(anyhow::Error::msg)?;
        let id = envelope.request_id;
        outgoing
            .try_send(envelope)
            .map_err(|_| anyhow::anyhow!("插件发送队列已满或 pipe 已关闭"))?;
        Ok(id)
    }

    pub(super) fn enqueue(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        body: envelope::Body,
        kind: PendingKind,
    ) -> Result<u64> {
        let envelope = self.envelope(body, 0)?;
        self.enqueue_frame(outgoing, envelope, kind)
    }

    pub(super) fn enqueue_frame(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        envelope: Envelope,
        kind: PendingKind,
    ) -> Result<u64> {
        ensure!(
            self.requests.len() < QUEUE_CAPACITY,
            "插件待完成操作超过限额"
        );
        envelope.validate().map_err(anyhow::Error::msg)?;
        let id = envelope.request_id;
        outgoing
            .try_send(envelope)
            .map_err(|_| anyhow::anyhow!("插件发送队列已满或 pipe 已关闭"))?;
        let deadline = Instant::now() + MAX_OPERATION;
        self.requests.insert(id, Pending { deadline, kind });
        Ok(id)
    }

    pub(super) fn editor(&self, document_id: u64) -> Option<&panel::Editor> {
        self.panel.editor(document_id)
    }

    fn check_editor_commit(
        &self,
        raw_id: &[u8],
        message_revision: u64,
        editor: Option<EditorSnapshot>,
    ) -> Result<(Uuid, EditorSnapshot)> {
        ensure!(
            self.broker.grants.contains(&Permission::DraftWrite),
            "插件尚未获准 draft.write；未提交或发送"
        );
        let id = protocol::uuid(raw_id).map_err(anyhow::Error::msg)?;
        ensure!(
            self.active_resource == Some((id, message_revision)),
            "编辑上下文已切换；拒绝旧提交"
        );
        ensure!(
            self.opening_request.is_none(),
            "正在打开新的编辑上下文；未提交或发送"
        );
        self.broker
            .writable(id, message_revision)
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        let snapshot = editor.context("缺少最新编辑快照")?;
        snapshot.validate().map_err(anyhow::Error::msg)?;
        let editor = self
            .editor(snapshot.document_id)
            .context("编辑器不属于此插件")?;
        ensure!(!editor.read_only, "只读编辑器拒绝提交");
        ensure!(snapshot.revision >= editor.revision, "编辑快照已过期");
        ensure!(
            snapshot.revision != editor.revision || snapshot.contents == editor.contents,
            "同一 editor revision 的内容不一致"
        );
        Ok((id, snapshot))
    }

    pub(super) fn commit_binding(&self, request: &CommitEditor) -> Result<(Uuid, EditorSnapshot)> {
        let binding = self.check_editor_commit(
            &request.resource_id,
            request.message_revision,
            request.editor.clone(),
        )?;
        let _: HttpRequest = request
            .request
            .clone()
            .context("缺少基准请求")?
            .try_into()
            .map_err(anyhow::Error::msg)?;
        Ok(binding)
    }

    pub(super) fn commit_response_binding(
        &self,
        request: &protocol::CommitResponseEditor,
    ) -> Result<(Uuid, EditorSnapshot)> {
        let binding = self.check_editor_commit(
            &request.resource_id,
            request.message_revision,
            request.editor.clone(),
        )?;
        let _: HttpResponse = request
            .response
            .clone()
            .context("缺少基准响应")?
            .try_into()
            .map_err(anyhow::Error::msg)?;
        Ok(binding)
    }

    fn start_open(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        id: Uuid,
        revision: u64,
        read_only: bool,
        body: envelope::Body,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    ) -> Result<()> {
        if let Err(error) = self.broker.authorize_open(id, revision) {
            let _ = reply.send(Err(error));
            return Ok(());
        }
        let envelope = self.envelope(body, 0)?;
        if let Err(error) = envelope.validate() {
            let _ = reply.send(Err(anyhow::anyhow!(error)));
            return Ok(());
        }
        let reserved = self.broker.reserve_resource(Some(id))?;
        let pending = self.enqueue_frame(
            outgoing,
            envelope,
            PendingKind::Open {
                resource: id,
                revision,
                read_only,
                reserved,
                reply,
            },
        )?;
        self.opening_request = Some(pending);
        Ok(())
    }

    pub(super) async fn control(
        &mut self,
        command: Control,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        match command {
            Control::Event(event) => {
                if let Err(error) = self.panel.observe_event(&event) {
                    return self.diagnostic(Failure::new(ErrorCode::Stale, error)).await;
                }
                let mut editor_bases = event.editors.clone();
                if let Some(panel::ui_event::Event::EditorChanged(snapshot)) = &event.event {
                    editor_bases.push(snapshot.clone());
                }
                self.enqueue(
                    outgoing,
                    envelope::Body::UiEvent(event),
                    PendingKind::Event { editor_bases },
                )?;
            }
            Control::Open {
                id,
                revision,
                read_only,
                request,
                reply,
            } => {
                self.start_open(
                    outgoing,
                    id,
                    revision,
                    read_only,
                    envelope::Body::OpenRequest(protocol::OpenRequest {
                        resource_id: id.as_bytes().to_vec(),
                        message_revision: revision,
                        read_only,
                        request: Some(request.into()),
                    }),
                    reply,
                )?;
            }
            Control::OpenResponse {
                id,
                revision,
                read_only,
                response,
                reply,
            } => {
                self.start_open(
                    outgoing,
                    id,
                    revision,
                    read_only,
                    envelope::Body::OpenResponse(protocol::OpenResponse {
                        resource_id: id.as_bytes().to_vec(),
                        message_revision: revision,
                        read_only,
                        response: Some(response.into()),
                    }),
                    reply,
                )?;
            }
            Control::Commit { request, reply } => match self.commit_binding(&request) {
                Err(e) => _ = reply.send(Err(e)),
                Ok((resource, document)) => {
                    let message_revision = request.message_revision;
                    let env = self.envelope(envelope::Body::CommitEditor(request), 0)?;
                    if let Err(e) = env.validate() {
                        let _ = reply.send(Err(anyhow::anyhow!(e)));
                    } else {
                        self.enqueue_frame(
                            outgoing,
                            env,
                            PendingKind::Commit {
                                resource,
                                message_revision,
                                document,
                                reply,
                            },
                        )?;
                    }
                }
            },
            Control::CommitResponse { request, reply } => match self
                .commit_response_binding(&request)
            {
                Err(e) => _ = reply.send(Err(e)),
                Ok((resource, document)) => {
                    let message_revision = request.message_revision;
                    let env = self.envelope(envelope::Body::CommitResponseEditor(request), 0)?;
                    if let Err(e) = env.validate() {
                        let _ = reply.send(Err(anyhow::anyhow!(e)));
                    } else {
                        self.enqueue_frame(
                            outgoing,
                            env,
                            PendingKind::CommitResponse {
                                resource,
                                message_revision,
                                document,
                                reply,
                            },
                        )?;
                    }
                }
            },
            Control::Revoke { id, reply } => {
                self.broker.resources.remove(&id);
                if let Some(barrier) = self
                    .service_cancels
                    .iter()
                    .filter_map(|(request, service)| {
                        matches!(service.binding, Some(PendingResource::OpenRepeater))
                            .then_some(*request)
                    })
                    .max()
                {
                    self.revoked_resources.insert(id, barrier);
                }
                let pending: Vec<_> = self
                    .requests
                    .iter()
                    .filter_map(|(request_id, pending)| match pending.kind {
                        PendingKind::Open { resource, .. }
                        | PendingKind::Commit { resource, .. }
                        | PendingKind::CommitResponse { resource, .. }
                            if resource == id =>
                        {
                            Some(*request_id)
                        }
                        _ => None,
                    })
                    .collect();
                if self
                    .active_resource
                    .is_some_and(|(resource, _)| resource == id)
                    || self
                        .opening_request
                        .is_some_and(|opening| pending.contains(&opening))
                {
                    self.active_resource = None;
                }
                for request_id in pending {
                    self.reject_response(request_id, "资源已撤销；未提交或发送，保留现有编辑内容");
                }
                for service in self.service_cancels.values() {
                    if matches!(service.binding, Some(PendingResource::UpdateDraft { id: resource, .. }) if resource == id)
                    {
                        service.cancel.cancel();
                    }
                }
                self.enqueue(
                    outgoing,
                    envelope::Body::CloseRequest(protocol::CloseRequest {
                        resource_id: id.as_bytes().to_vec(),
                    }),
                    PendingKind::Close { reply },
                )?;
            }
            Control::Hook {
                permission,
                body,
                cancelled,
                reply,
            } => {
                let allowed = self.broker.grants.contains(&permission)
                    || (permission == Permission::TrafficRead
                        && self.broker.grants.contains(&Permission::TrafficModify));
                if !allowed {
                    let _ = reply.send(Err(anyhow::anyhow!("插件未获准此 Hook 能力")));
                    return Ok(());
                }
                let depth = match &body {
                    envelope::Body::HttpHookCall(call) => call
                        .request
                        .as_ref()
                        .and_then(|req| {
                            req.headers
                                .iter()
                                .find(|h| h.name.eq_ignore_ascii_case("x-mio-plugin-depth"))
                        })
                        .and_then(|h| std::str::from_utf8(&h.value).ok())
                        .and_then(|v| v.trim().parse::<u8>().ok())
                        .unwrap_or(0),
                    _ => 0,
                };
                self.enqueue(
                    outgoing,
                    body,
                    PendingKind::Hook {
                        depth,
                        cancelled,
                        reply,
                    },
                )?;
            }
        }
        Ok(())
    }
}
