use super::*;

impl PanelSlot {
    pub(super) fn with_state(state: PanelState) -> Self {
        Self {
            state: Some(state),
            ..Self::default()
        }
    }
}

impl Running {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        identity: ChannelIdentity,
        plugin_id: String,
        supports_panel_multiplexing: bool,
        panel_state: PanelState,
        broker: PermissionBroker,
        services: mpsc::Sender<ServiceRequest>,
        events: mpsc::Sender<PluginEvent>,
        cancel: CancellationToken,
        outgoing_sequence: u64,
        incoming_sequence: u64,
    ) -> Self {
        Self {
            identity,
            plugin_id,
            protocol_minor: protocol::PROTOCOL_MINOR,
            supports_panel_multiplexing,
            panels: HashMap::from([(PANEL_ID, PanelSlot::with_state(panel_state))]),
            broker,
            services,
            events,
            cancel,
            outgoing_sequence,
            incoming_sequence,
            rate_epoch: Instant::now(),
            frame_count: 0,
            requests: HashMap::new(),
            cancelled_requests: std::collections::VecDeque::new(),
            cancel_notifications: std::collections::VecDeque::new(),
            outbound_queue: std::collections::VecDeque::new(),
            outbound_claims: None,
            pending_revokes: std::collections::VecDeque::new(),
            next_resync_seq: 0,
            pending_panel_syncs: std::collections::BTreeMap::new(),
            defer_panel_sync_for_publish: false,
            child_headroom_blocked: false,
            urgent_commands: None,
            panel_destroy_control_seq: HashMap::new(),
            panel_destroy_seq_floor: 0,
            resource_revoke_control_seq: HashMap::new(),
            resource_revoke_seq_floor: 0,
            cancelled_op_control_seq: HashMap::new(),
            cancelled_op_seq_floor: 0,
            panel_resource_open_seq: HashMap::new(),
            panel_revoke_snapshots: HashMap::new(),
            panel_revoke_snapshot_floor: HashMap::new(),
            service_cancels: HashMap::new(),
            panel_revoked_resources: HashMap::new(),
            service_tasks: JoinSet::new(),
        }
    }

    pub(super) fn flush_outbound_nonblocking(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        while !self.outbound_queue.is_empty() {
            match outgoing.try_reserve() {
                Ok(permit) => {
                    if let Some(env) = self.outbound_queue.pop_front() {
                        permit.send(env);
                    }
                }
                Err(mpsc::error::TrySendError::Full(())) => break,
                Err(mpsc::error::TrySendError::Closed(())) => {
                    bail!("插件输入 pipe 已关闭");
                }
            }
        }
        Ok(())
    }

    pub(super) fn push_outbound(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        envelope: Envelope,
    ) -> Result<()> {
        self.flush_outbound_nonblocking(outgoing)?;
        if self.outbound_queue.is_empty() {
            match outgoing.try_send(envelope) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::TrySendError::Full(env)) => {
                    self.outbound_queue.push_back(env);
                    return Ok(());
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    bail!("插件输入 pipe 已关闭");
                }
            }
        }
        if outgoing.is_closed() {
            bail!("插件输入 pipe 已关闭");
        }
        ensure!(
            self.outbound_queue.len() < OUTBOUND_BUFFER_CAPACITY,
            "插件出站缓冲队列已满"
        );
        self.outbound_queue.push_back(envelope);
        Ok(())
    }

    pub(super) fn child_congestion_at(&self, outgoing_capacity: usize) -> (bool, bool) {
        let low_headroom = outgoing_capacity <= RESERVED_CONTROL_OUTGOING;
        let congested = low_headroom
            || self.requests.len() >= CHILD_QUEUE_SOFT_LIMIT
            || !self.outbound_queue.is_empty()
            || self.pending_revokes.iter().any(|r| {
                r.remove_panel
                    || (!self.is_panel_resyncing(r.panel_id)
                        && !self.has_inflight_non_destroy_close(r.panel_id))
            });
        (congested, low_headroom)
    }

    pub(super) fn is_child_congested(&self, outgoing: &mpsc::Sender<Envelope>) -> bool {
        self.child_congestion_at(outgoing.capacity()).0
    }

    pub(super) fn is_main_congested(&self) -> bool {
        self.requests.len() >= QUEUE_CAPACITY
            || self.outbound_queue.len() >= OUTBOUND_CONTROL_SOFT_LIMIT
    }

    pub(super) fn retract_unsent_outbound_frame(&mut self, panel_id: u64, request_id: u64) -> bool {
        if let Some(pos) = self
            .outbound_queue
            .iter()
            .position(|env| env.panel_id == panel_id && env.request_id == request_id)
        {
            self.outbound_queue.remove(pos);
            if let Some(claims) = self.outbound_claims.as_ref() {
                claims.remove(panel_id, request_id);
            }
            return true;
        }
        self.outbound_claims
            .as_ref()
            .is_some_and(|claims| claims.try_retract(panel_id, request_id))
    }

    pub(super) fn retract_all_unclaimed_outbound(&mut self) {
        if let Some(claims) = self.outbound_claims.as_ref() {
            claims.retract_all_pending();
        }
        self.outbound_queue.retain(|env| {
            let keep = matches!(
                env.body,
                Some(
                    envelope::Body::Cancel(_)
                        | envelope::Body::Shutdown(_)
                        | envelope::Body::HostResult(_)
                )
            );
            if !keep && let Some(claims) = self.outbound_claims.as_ref() {
                claims.remove(env.panel_id, env.request_id);
            }
            keep
        });
    }

    pub(super) fn is_fatal_pending(pending: &Pending) -> bool {
        pending.panel_id == PANEL_ID
            && !pending.kind.is_panel_recovery()
            && !matches!(pending.kind, PendingKind::Hook { .. })
    }

    pub(super) fn has_expired_fatal_request(&self, now: Instant) -> bool {
        if self
            .outbound_claims
            .as_deref()
            .is_some_and(OutboundWriteClaims::is_fatal_expired_triggered)
        {
            return true;
        }
        self.requests
            .values()
            .any(|p| p.deadline <= now && Self::is_fatal_pending(p))
    }

    pub(super) fn sync_fatal_deadline_to_claims(&self) {
        if let Some(claims) = self.outbound_claims.as_ref() {
            let earliest = self
                .requests
                .values()
                .filter(|p| Self::is_fatal_pending(p))
                .map(|p| p.deadline)
                .min();
            claims.set_earliest_fatal_deadline(earliest);
        }
    }

    pub(super) fn settle_request_deadline_before_reply(
        &mut self,
        in_reply_to: u64,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        let Some(claims) = self.outbound_claims.clone() else {
            return Ok(());
        };
        if in_reply_to == 0 {
            if claims.is_fatal_expired_triggered() {
                return self.check_fatal_timeouts(Instant::now(), outgoing);
            }
            return Ok(());
        }
        let next_earliest = self
            .requests
            .iter()
            .filter(|&(&id, p)| id != in_reply_to && Self::is_fatal_pending(p))
            .map(|(_, p)| p.deadline)
            .min();
        if !claims.update_fatal_deadline_unless_triggered(next_earliest) {
            let now = Instant::now();
            let in_reply_is_fatal = self
                .requests
                .get(&in_reply_to)
                .is_some_and(Self::is_fatal_pending);
            let mut expired_targets: Vec<_> = self
                .requests
                .iter()
                .filter_map(|(&id, p)| {
                    (p.deadline <= now || (id == in_reply_to && in_reply_is_fatal))
                        .then_some((p.panel_id, id))
                })
                .collect();
            expired_targets.sort_unstable_by_key(|&(_, id)| id);
            let reject_target = in_reply_is_fatal
                .then_some((in_reply_to, "主面板操作已超时；已取消并回收实例"));
            return self.abort_fatal_timeout(&expired_targets, outgoing, reject_target);
        }
        Ok(())
    }

    pub(super) fn check_fatal_timeouts(
        &mut self,
        now: Instant,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        let fatal_triggered = self
            .outbound_claims
            .as_deref()
            .is_some_and(OutboundWriteClaims::is_fatal_expired_triggered);
        let check_now = if fatal_triggered {
            now.max(Instant::now())
        } else {
            now
        };
        if !fatal_triggered && !self.has_expired_fatal_request(check_now) {
            return Ok(());
        }
        let mut expired_targets: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(&id, p)| (p.deadline <= check_now).then_some((p.panel_id, id)))
            .collect();
        expired_targets.sort_unstable_by_key(|&(_, id)| id);
        if fatal_triggered
            && expired_targets.is_empty()
            && let Some((&id, p)) = self
                .requests
                .iter()
                .filter(|&(_, p)| Self::is_fatal_pending(p))
                .min_by_key(|&(&id, p)| (p.deadline, id))
        {
            expired_targets.push((p.panel_id, id));
        }
        self.abort_fatal_timeout(&expired_targets, outgoing, None)
    }

    pub(super) fn abort_fatal_timeout(
        &mut self,
        expired_targets: &[(u64, u64)],
        outgoing: &mpsc::Sender<Envelope>,
        reject_target: Option<(u64, &str)>,
    ) -> Result<()> {
        let mut cancels_to_send = if let Some(claims) = self.outbound_claims.as_ref() {
            claims.retract_all_pending_returning_already_claimed(expired_targets)
        } else {
            let mut already = Vec::new();
            for &(panel_id, id) in expired_targets {
                if !self.retract_unsent_outbound_frame(panel_id, id) {
                    already.push((panel_id, id));
                }
            }
            already
        };
        cancels_to_send.retain(|&(panel_id, id)| {
            !self
                .outbound_queue
                .iter()
                .any(|env| env.panel_id == panel_id && env.request_id == id)
        });
        for &(_, id) in expired_targets {
            if let Some(pending) = self.requests.get(&id) {
                pending.cancel.cancel();
            }
        }
        self.retract_all_unclaimed_outbound();
        if let Some((req_id, reason)) = reject_target {
            self.reject_response_with_mode(req_id, reason, true, true);
        }
        for (panel_id, id) in cancels_to_send {
            let _ = self.send_on_panel(
                outgoing,
                panel_id,
                envelope::Body::Cancel(protocol::Cancel { request_id: id }),
                0,
            );
        }
        bail!("插件操作超时；已取消并回收实例，不会发送旧稿");
    }

    pub(super) fn prepare_shutdown(&mut self, outgoing: &mpsc::Sender<Envelope>) {
        for service in self.service_cancels.values() {
            service.cancel.cancel();
        }
        self.retract_all_unclaimed_outbound();
        let _ = self.send(outgoing, envelope::Body::Shutdown(panel::Empty {}), 0);
    }

    pub(super) fn preview_envelope_for(
        &self,
        panel_id: u64,
        body: envelope::Body,
        in_reply_to: u64,
    ) -> Result<Envelope> {
        let seq = self
            .outgoing_sequence
            .checked_add(1)
            .context("插件消息序号耗尽")?;
        let mut envelope = Envelope::new(
            self.identity,
            seq,
            self.panel_revision(panel_id),
            body,
            MAX_OPERATION,
        )
        .with_minor(self.protocol_minor)
        .with_panel_id(panel_id);
        envelope.in_reply_to = in_reply_to;
        Ok(envelope)
    }

    pub(super) fn envelope_for(
        &mut self,
        panel_id: u64,
        body: envelope::Body,
        in_reply_to: u64,
    ) -> Result<Envelope> {
        let envelope = self.preview_envelope_for(panel_id, body, in_reply_to)?;
        self.outgoing_sequence = envelope.request_id;
        Ok(envelope)
    }

    pub(super) async fn emit(&self, event: PluginEvent) -> Result<()> {
        self.sync_fatal_deadline_to_claims();
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Ok(()),
            result = timeout(MAX_OPERATION, self.events.send(event)) => {
                result
                    .context("插件事件消费超时")?
                    .context("插件事件接收方已关闭")
            }
        }
    }

    pub(super) async fn diagnostic(&self, failure: Failure) -> Result<()> {
        self.diagnostic_on_panel(PANEL_ID, failure).await
    }

    pub(super) async fn diagnostic_on_panel(&self, panel_id: u64, failure: Failure) -> Result<()> {
        self.emit(PluginEvent::Error {
            identity: self.identity,
            panel_id,
            failure,
        })
        .await
    }

    pub(super) async fn publish(&mut self, editor_bases: Vec<EditorSnapshot>) -> Result<()> {
        self.publish_panel(PANEL_ID, editor_bases).await
    }

    pub(super) async fn publish_panel(
        &mut self,
        panel_id: u64,
        editor_bases: Vec<EditorSnapshot>,
    ) -> Result<()> {
        let Some(snapshot) = self.ui_panel_snapshot(panel_id) else {
            self.pending_panel_syncs.remove(&panel_id);
            return Ok(());
        };
        let synced_ui_seq = self.contiguous_processed_ui_seq(panel_id);
        let rejected_ui_seqs = self
            .pending_panel_syncs
            .remove(&panel_id)
            .map(|mut rejected| {
                while rejected.len() > QUEUE_CAPACITY {
                    rejected.pop_first();
                }
                rejected.into_iter().collect()
            })
            .unwrap_or_default();
        self.emit(PluginEvent::Panel {
            identity: self.identity,
            panel_id,
            snapshot,
            editor_bases,
            synced_ui_seq,
            rejected_ui_seqs,
        })
        .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn send(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        body: envelope::Body,
        in_reply_to: u64,
    ) -> Result<u64> {
        self.send_on_panel(outgoing, PANEL_ID, body, in_reply_to)
    }

    pub(super) fn send_on_panel(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        panel_id: u64,
        body: envelope::Body,
        in_reply_to: u64,
    ) -> Result<u64> {
        let envelope = self.envelope_for(panel_id, body, in_reply_to)?;
        envelope
            .validate_for_minor(self.protocol_minor)
            .map_err(anyhow::Error::msg)?;
        let id = envelope.request_id;
        self.push_outbound(outgoing, envelope)?;
        Ok(id)
    }

    pub(super) fn enqueue_on_panel(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        panel_id: u64,
        body: envelope::Body,
        kind: PendingKind,
    ) -> Result<u64> {
        let envelope = self.envelope_for(panel_id, body, 0)?;
        self.enqueue_frame(outgoing, panel_id, envelope, kind)
    }

    pub(super) fn enqueue_frame(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        panel_id: u64,
        envelope: Envelope,
        kind: PendingKind,
    ) -> Result<u64> {
        let limit = if panel_id != PANEL_ID && !matches!(kind, PendingKind::Close { .. }) {
            CHILD_QUEUE_SOFT_LIMIT
        } else {
            QUEUE_CAPACITY
        };
        ensure!(self.requests.len() < limit, "插件待完成操作超过限额");
        envelope
            .validate_for_minor(self.protocol_minor)
            .map_err(anyhow::Error::msg)?;
        let id = envelope.request_id;
        if let Some(claims) = self.outbound_claims.as_ref() {
            claims.register(panel_id, id);
        }
        if let Err(err) = self.push_outbound(outgoing, envelope) {
            if let Some(claims) = self.outbound_claims.as_ref() {
                claims.remove(panel_id, id);
            }
            return Err(err);
        }
        let deadline = Instant::now() + MAX_OPERATION;
        let cancel = match &kind {
            PendingKind::Commit { cancelled, .. }
            | PendingKind::CommitResponse { cancelled, .. }
            | PendingKind::Hook { cancelled, .. } => cancelled.child_token(),
            _ => self.cancel.child_token(),
        };
        let panel_revision_at_send = self.panel_revision(panel_id);
        self.requests.insert(
            id,
            Pending {
                panel_id,
                panel_revision_at_send,
                deadline,
                cancel,
                from_deferred_queue: false,
                source_seq: 0,
                kind,
            },
        );
        self.sync_fatal_deadline_to_claims();
        Ok(id)
    }
}
