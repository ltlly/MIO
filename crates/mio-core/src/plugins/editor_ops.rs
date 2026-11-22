use super::*;

pub(super) struct CommittedReplyCheck<'a> {
    pub(super) is_response: bool,
    pub(super) panel_id: u64,
    pub(super) req_panel: u64,
    pub(super) envelope_revision: u64,
    pub(super) now: Instant,
    pub(super) deadline: Instant,
    pub(super) resource: Uuid,
    pub(super) message_revision: u64,
    pub(super) document: &'a EditorSnapshot,
    pub(super) cancelled: &'a CancellationToken,
    pub(super) expected_snapshot: Option<&'a PanelSnapshot>,
    pub(super) raw_resource_id: &'a [u8],
    pub(super) reply_message_revision: u64,
    pub(super) reply_document_id: u64,
    pub(super) reply_document_revision: u64,
}

impl Running {
    pub(super) fn editor_on_panel(
        &self,
        panel_id: u64,
        document_id: u64,
    ) -> Option<&panel::Editor> {
        self.panels
            .get(&panel_id)
            .and_then(|slot| slot.state.as_ref())
            .and_then(|state| state.editor(document_id))
    }

    fn check_editor_commit(
        &self,
        panel_id: u64,
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
            self.active_resource_for(panel_id) == Some((id, message_revision)),
            "编辑上下文已切换；拒绝旧提交"
        );
        ensure!(
            self.opening_request_for(panel_id).is_none(),
            "正在打开新的编辑上下文；未提交或发送"
        );
        self.broker
            .writable_panel(panel_id, id, message_revision)
            .map_err(|failure| anyhow::anyhow!(failure.message))?;
        let snapshot = editor.context("缺少最新编辑快照")?;
        snapshot.validate().map_err(anyhow::Error::msg)?;
        let editor = self
            .editor_on_panel(panel_id, snapshot.document_id)
            .context("编辑器不属于此插件")?;
        ensure!(!editor.read_only, "只读编辑器拒绝提交");
        ensure!(snapshot.revision >= editor.revision, "编辑快照已过期");
        ensure!(
            snapshot.revision != editor.revision || snapshot.contents == editor.contents,
            "同一 editor revision 的内容不一致"
        );
        Ok((id, snapshot))
    }

    pub(super) fn commit_binding(
        &self,
        panel_id: u64,
        request: &CommitEditor,
    ) -> Result<(Uuid, EditorSnapshot)> {
        let binding = self.check_editor_commit(
            panel_id,
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
        panel_id: u64,
        request: &protocol::CommitResponseEditor,
    ) -> Result<(Uuid, EditorSnapshot)> {
        let binding = self.check_editor_commit(
            panel_id,
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_open(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        panel_id: u64,
        id: Uuid,
        revision: u64,
        read_only: bool,
        body: envelope::Body,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
        from_deferred: bool,
        cmd_seq: u64,
    ) -> Result<()> {
        if outgoing.is_closed() {
            let _ = reply.send(Err(anyhow::anyhow!("插件输入 pipe 已关闭")));
            bail!("插件输入 pipe 已关闭");
        }
        if panel_id != PANEL_ID {
            if !self.supports_panel_multiplexing {
                let _ = reply.send(Err(anyhow::anyhow!("此插件不支持多路复用子面板")));
                return Ok(());
            }
            if self.panels.get(&panel_id).is_some_and(|slot| slot.closing) {
                let _ = reply.send(Err(anyhow::anyhow!("多路复用子面板正在关闭")));
                return Ok(());
            }
            let child_panel_count = self.panels.keys().filter(|&&pid| pid != PANEL_ID).count();
            if !self.panels.contains_key(&panel_id) && child_panel_count >= MAX_MULTIPLEXED_PANELS {
                let _ = reply.send(Err(anyhow::anyhow!("多路复用子面板数量超过限额")));
                return Ok(());
            }
            if self.is_child_congested(outgoing) {
                let _ = reply.send(Err(anyhow::anyhow!("插件多路复用请求队列繁忙；请稍后重试")));
                return Ok(());
            }
        } else if self.is_main_congested() {
            let _ = reply.send(Err(anyhow::anyhow!("插件主面板请求队列繁忙；请稍后重试")));
            return Ok(());
        }
        if let Err(error) = self.broker.authorize_open_panel(panel_id, id, revision) {
            let _ = reply.send(Err(error));
            return Ok(());
        }
        let envelope = self.envelope_for(panel_id, body, 0)?;
        if let Err(error) = envelope.validate_for_minor(self.protocol_minor) {
            let _ = reply.send(Err(anyhow::anyhow!(error)));
            return Ok(());
        }
        let reserved = self.broker.reserve_panel_resource(panel_id, Some(id))?;
        let pending = match self.enqueue_frame(
            outgoing,
            panel_id,
            envelope,
            PendingKind::Open {
                resource: id,
                revision,
                read_only,
                reserved,
                reply,
            },
        ) {
            Ok(id) => id,
            Err(error) => {
                self.broker.release_panel_resource(panel_id, reserved);
                return Err(error);
            }
        };
        if let Some(p) = self.requests.get_mut(&pending) {
            if from_deferred {
                p.from_deferred_queue = true;
            }
            if cmd_seq != 0 {
                p.source_seq = cmd_seq;
            }
        }
        self.panels.entry(panel_id).or_default();
        self.set_opening_request_for(panel_id, Some(pending));
        Ok(())
    }

    fn check_commit_dispatch_preconditions(
        &self,
        panel_id: u64,
        is_response: bool,
        cancelled: &CancellationToken,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<Option<&'static str>> {
        if cancelled.is_cancelled() {
            return Ok(Some(if is_response {
                "插件响应编辑提交已取消"
            } else {
                "插件编辑提交已取消；未发送"
            }));
        }
        if outgoing.is_closed() {
            bail!("插件输入 pipe 已关闭");
        }
        if panel_id != PANEL_ID && self.is_child_congested(outgoing) {
            return Ok(Some("插件子面板请求队列繁忙；请稍后重试"));
        }
        if panel_id == PANEL_ID && self.is_main_congested() {
            return Ok(Some("插件主面板请求队列繁忙；请稍后重试"));
        }
        if self.is_panel_semantic_drifted(panel_id) {
            return Ok(Some(if is_response {
                "插件面板尚未通过最新快照确认状态；响应编辑提交未执行，请刷新或重载面板"
            } else {
                "插件面板尚未通过最新快照确认状态；编辑提交未执行，请刷新或重载面板"
            }));
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_commit_request(
        &mut self,
        panel_id: u64,
        op_id: Uuid,
        request: CommitEditor,
        cancelled: CancellationToken,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        reply: oneshot::Sender<Result<HttpRequest>>,
        outgoing: &mpsc::Sender<Envelope>,
        cmd_seq: u64,
    ) -> Result<()> {
        match self.check_commit_dispatch_preconditions(panel_id, false, &cancelled, outgoing) {
            Err(err) => {
                let _ = reply.send(Err(anyhow::anyhow!(err.to_string())));
                return Err(err);
            }
            Ok(Some(msg)) => {
                let _ = reply.send(Err(anyhow::anyhow!(msg)));
                return Ok(());
            }
            Ok(None) => {}
        }
        let (resource, document) = match self.commit_binding(panel_id, &request) {
            Ok(binding) => binding,
            Err(err) => {
                let _ = reply.send(Err(err));
                return Ok(());
            }
        };
        if let Some(expected) = expected_snapshot.as_deref()
            && !panel::commit_panel_semantically_matches(
                self.panel_snapshot(panel_id).as_deref(),
                expected,
                &document,
            )
        {
            let _ = reply.send(Err(anyhow::anyhow!(
                "插件面板内容已更新；旧视图的编辑提交未执行，请在当前视图重试"
            )));
            return Ok(());
        }
        let message_revision = request.message_revision;
        let env = self.envelope_for(panel_id, envelope::Body::CommitEditor(request), 0)?;
        if let Err(err) = env.validate_for_minor(self.protocol_minor) {
            let _ = reply.send(Err(anyhow::anyhow!(err)));
            return Ok(());
        }
        let req_id = self.enqueue_frame(
            outgoing,
            panel_id,
            env,
            PendingKind::Commit {
                op_id,
                resource,
                message_revision,
                document,
                cancelled,
                expected_snapshot,
                reply,
            },
        )?;
        if cmd_seq != 0
            && let Some(p) = self.requests.get_mut(&req_id)
        {
            p.source_seq = cmd_seq;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch_commit_response(
        &mut self,
        panel_id: u64,
        op_id: Uuid,
        request: protocol::CommitResponseEditor,
        cancelled: CancellationToken,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        reply: oneshot::Sender<Result<HttpResponse>>,
        outgoing: &mpsc::Sender<Envelope>,
        cmd_seq: u64,
    ) -> Result<()> {
        match self.check_commit_dispatch_preconditions(panel_id, true, &cancelled, outgoing) {
            Err(err) => {
                let _ = reply.send(Err(anyhow::anyhow!(err.to_string())));
                return Err(err);
            }
            Ok(Some(msg)) => {
                let _ = reply.send(Err(anyhow::anyhow!(msg)));
                return Ok(());
            }
            Ok(None) => {}
        }
        let (resource, document) = match self.commit_response_binding(panel_id, &request) {
            Ok(binding) => binding,
            Err(err) => {
                let _ = reply.send(Err(err));
                return Ok(());
            }
        };
        if let Some(expected) = expected_snapshot.as_deref()
            && !panel::commit_panel_semantically_matches(
                self.panel_snapshot(panel_id).as_deref(),
                expected,
                &document,
            )
        {
            let _ = reply.send(Err(anyhow::anyhow!(
                "插件面板内容已更新；旧视图的响应编辑提交未执行，请在当前视图重试"
            )));
            return Ok(());
        }
        let message_revision = request.message_revision;
        let env = self.envelope_for(panel_id, envelope::Body::CommitResponseEditor(request), 0)?;
        if let Err(err) = env.validate_for_minor(self.protocol_minor) {
            let _ = reply.send(Err(anyhow::anyhow!(err)));
            return Ok(());
        }
        let req_id = self.enqueue_frame(
            outgoing,
            panel_id,
            env,
            PendingKind::CommitResponse {
                op_id,
                resource,
                message_revision,
                document,
                cancelled,
                expected_snapshot,
                reply,
            },
        )?;
        if cmd_seq != 0
            && let Some(p) = self.requests.get_mut(&req_id)
        {
            p.source_seq = cmd_seq;
        }
        Ok(())
    }

    pub(super) fn validate_committed_reply(&self, check: &CommittedReplyCheck<'_>) -> Result<()> {
        let resp = check.is_response;
        ensure!(
            !check.cancelled.is_cancelled(),
            if resp {
                "响应编辑提交已取消；丢弃结果"
            } else {
                "编辑提交已取消；丢弃结果"
            }
        );
        ensure!(
            check.panel_id == check.req_panel,
            if resp {
                "ResponseCommitted panel_id 不匹配"
            } else {
                "Committed panel_id 不匹配"
            }
        );
        ensure!(
            check.now < check.deadline,
            if resp {
                "响应编辑提交已超时"
            } else {
                "编辑提交已超时"
            }
        );
        if resp {
            ensure!(
                self.opening_request_for(check.req_panel).is_none()
                    && self.active_resource_for(check.req_panel)
                        == Some((check.resource, check.message_revision)),
                "响应编辑上下文已切换；丢弃旧提交结果"
            );
        } else {
            ensure!(
                self.opening_request_for(check.req_panel).is_none(),
                "编辑上下文正在切换；丢弃旧提交结果"
            );
            ensure!(
                self.active_resource_for(check.req_panel)
                    == Some((check.resource, check.message_revision)),
                "编辑上下文已切换；丢弃旧提交结果"
            );
        }
        self.broker
            .writable_panel(check.req_panel, check.resource, check.message_revision)
            .map_err(|f| anyhow::anyhow!(f.message))?;
        ensure!(
            protocol::uuid(check.raw_resource_id).ok() == Some(check.resource)
                && check.reply_message_revision == check.message_revision
                && check.reply_document_id == check.document.document_id
                && check.reply_document_revision == check.document.revision,
            if resp {
                "响应编辑提交结果身份或 revision 不匹配"
            } else {
                "编辑提交结果身份或 revision 不匹配"
            }
        );
        ensure!(
            !self.is_panel_semantic_drifted(check.req_panel),
            if resp {
                "插件面板尚未通过最新快照确认状态；丢弃未确认状态的响应编辑提交结果"
            } else {
                "插件面板尚未通过最新快照确认状态；丢弃未确认状态的编辑提交结果"
            }
        );
        ensure!(
            check.envelope_revision == self.panel_revision(check.req_panel),
            if resp {
                "响应编辑提交结果面板 revision 与宿主已确认视图不一致；丢弃结果"
            } else {
                "编辑提交结果面板 revision 与宿主已确认视图不一致；丢弃结果"
            }
        );
        ensure!(
            check.expected_snapshot.is_none_or(|expected| {
                panel::commit_panel_semantically_matches(
                    self.panel_snapshot(check.req_panel).as_deref(),
                    expected,
                    check.document,
                )
            }),
            if resp {
                "插件面板内容已更新；丢弃旧视图的响应编辑提交结果"
            } else {
                "插件面板内容已更新；丢弃旧视图的编辑提交结果"
            }
        );
        ensure!(
            self.editor_on_panel(check.req_panel, check.document.document_id)
                .is_some_and(|e| !e.read_only && e.revision <= check.document.revision),
            "编辑器在等待时已改变"
        );
        Ok(())
    }

    pub(super) fn handle_committed_frame(
        &mut self,
        panel_id: u64,
        envelope: &Envelope,
        committed: protocol::Committed,
        now: Instant,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
        let pending = self
            .requests
            .remove(&envelope.in_reply_to)
            .context("收到无对应提交或过期的 Committed")?;
        self.sync_fatal_deadline_to_claims();
        let req_panel = pending.panel_id;
        let PendingKind::Commit {
            op_id: _,
            resource,
            message_revision,
            document,
            cancelled,
            expected_snapshot,
            reply,
        } = pending.kind
        else {
            bail!("Committed 的关联操作不是编辑提交");
        };
        let result = (|| {
            self.validate_committed_reply(&CommittedReplyCheck {
                is_response: false,
                panel_id,
                req_panel,
                envelope_revision: envelope.revision,
                now,
                deadline: pending.deadline,
                resource,
                message_revision,
                document: &document,
                cancelled: &cancelled,
                expected_snapshot: expected_snapshot.as_deref(),
                raw_resource_id: &committed.resource_id,
                reply_message_revision: committed.message_revision,
                reply_document_id: committed.document_id,
                reply_document_revision: committed.document_revision,
            })?;
            committed
                .request
                .context("提交结果没有请求")?
                .try_into()
                .map_err(anyhow::Error::msg)
        })();
        let _ = reply.send(result);
        if envelope.revision > self.panel_revision(req_panel) {
            self.schedule_panel_semantic_resync(req_panel);
        }
        Ok(())
    }

    pub(super) fn handle_response_committed_frame(
        &mut self,
        panel_id: u64,
        envelope: &Envelope,
        committed: protocol::ResponseCommitted,
        now: Instant,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
        let pending = self
            .requests
            .remove(&envelope.in_reply_to)
            .context("收到无对应提交或过期的 ResponseCommitted")?;
        self.sync_fatal_deadline_to_claims();
        let req_panel = pending.panel_id;
        let PendingKind::CommitResponse {
            op_id: _,
            resource,
            message_revision,
            document,
            cancelled,
            expected_snapshot,
            reply,
        } = pending.kind
        else {
            bail!("ResponseCommitted 的关联操作不是响应编辑提交");
        };
        let result = (|| {
            self.validate_committed_reply(&CommittedReplyCheck {
                is_response: true,
                panel_id,
                req_panel,
                envelope_revision: envelope.revision,
                now,
                deadline: pending.deadline,
                resource,
                message_revision,
                document: &document,
                cancelled: &cancelled,
                expected_snapshot: expected_snapshot.as_deref(),
                raw_resource_id: &committed.resource_id,
                reply_message_revision: committed.message_revision,
                reply_document_id: committed.document_id,
                reply_document_revision: committed.document_revision,
            })?;
            committed
                .response
                .context("提交结果没有响应")?
                .try_into()
                .map_err(anyhow::Error::msg)
        })();
        let _ = reply.send(result);
        if envelope.revision > self.panel_revision(req_panel) {
            self.schedule_panel_semantic_resync(req_panel);
        }
        Ok(())
    }
}
