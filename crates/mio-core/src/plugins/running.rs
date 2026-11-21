use super::*;

pub(super) struct RetractedOptimisticPlan {
    pub(super) retracted_invalidated: Vec<u64>,
    pub(super) has_claimed_invalidated: bool,
    pub(super) retracted_on_drift: Vec<u64>,
}

enum PendingUrgentClose {
    NonDestroy {
        urgent_seq: u64,
        panel_id: u64,
        id: Uuid,
        reply: oneshot::Sender<Result<()>>,
    },
    Destroy {
        panel_id: u64,
        id: Uuid,
        replies: Vec<oneshot::Sender<Result<()>>>,
    },
}

impl Running {
    pub(super) fn record_cancelled_request(&mut self, panel_id: u64, request_id: u64) {
        if request_id == 0 {
            return;
        }
        if !self.cancelled_requests.contains(&(panel_id, request_id)) {
            if self.cancelled_requests.len() >= QUEUE_CAPACITY {
                self.cancelled_requests.pop_front();
            }
            self.cancelled_requests.push_back((panel_id, request_id));
        }
    }

    pub(super) fn is_request_cancelled(&self, panel_id: u64, request_id: u64) -> bool {
        if self.cancelled_requests.contains(&(panel_id, request_id)) {
            return true;
        }
        self.requests
            .get(&request_id)
            .is_some_and(|p| p.panel_id == panel_id && p.cancel.is_cancelled())
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

    pub(super) fn record_cancel_notification(
        &mut self,
        panel_id: u64,
        cancel_frame_id: u64,
        for_resync: bool,
        converged_snapshot_rev: Option<u64>,
    ) {
        if cancel_frame_id == 0 {
            return;
        }
        if self.cancel_notifications.len() >= QUEUE_CAPACITY {
            self.cancel_notifications.pop_front();
        }
        self.cancel_notifications.push_back(CancelNotification {
            panel_id,
            cancel_frame_id,
            deadline: Instant::now() + MAX_OPERATION,
            can_restart_resync: !for_resync,
            converged_snapshot_rev,
        });
    }

    pub(super) fn send_cancel_best_effort(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        panel_id: u64,
        request_id: u64,
        for_resync: bool,
        converged_snapshot_rev: Option<u64>,
    ) {
        if self.outbound_queue.len() < OUTBOUND_BUFFER_CAPACITY / 2
            && outgoing.capacity() > 8
            && let Ok(cancel_frame_id) = self.send_on_panel(
                outgoing,
                panel_id,
                envelope::Body::Cancel(protocol::Cancel { request_id }),
                0,
            )
        {
            self.record_cancel_notification(
                panel_id,
                cancel_frame_id,
                for_resync,
                converged_snapshot_rev,
            );
        }
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

    pub(super) fn cancel_pending_request_inner(
        &mut self,
        request_id: u64,
        reason: &str,
        send_cancel: bool,
        expect_late_reply: bool,
    ) -> (bool, bool, Option<(u64, u64, bool, Option<u64>)>) {
        let Some((
            panel_id,
            panel_revision_at_send,
            for_resync,
            is_state_req,
            is_optimistic,
            is_destroy_close,
        )) = self.requests.get(&request_id).map(|p| {
            (
                p.panel_id,
                p.panel_revision_at_send,
                    matches!(
                        p.kind,
                        PendingKind::Event {
                            resync_seq: Some(_),
                            ..
                        }
                    ),
                    if p.panel_id == PANEL_ID {
                        matches!(
                            p.kind,
                            PendingKind::Event {
                                pointer_cancel_seq: Some(_),
                                ..
                            }
                        )
                    } else {
                        matches!(
                            p.kind,
                            PendingKind::Open { .. }
                                | PendingKind::Event {
                                    resync_seq: None,
                                    ..
                                }
                                | PendingKind::Close {
                                    remove_panel: false,
                                    ..
                                }
                        )
                    },
                    matches!(
                        p.kind,
                        PendingKind::Event {
                            resync_seq: None,
                            pointer_cancel_seq: None,
                            is_optimistic: true,
                            ..
                        }
                    ),
                    matches!(
                        p.kind,
                        PendingKind::Close {
                            remove_panel: true,
                            ..
                        }
                    ),
                )
            })
        else {
            return (false, false, None);
        };
        let retracted_unsent =
            !is_destroy_close && self.retract_unsent_outbound_frame(panel_id, request_id);
        let (reverted_optimistic, cancel_to_send) = if retracted_unsent {
            let reverted = self.retract_panel_optimistic_request(panel_id, request_id);
            if reverted && self.has_inflight_resync(panel_id) {
                self.mark_panel_semantic_drift(panel_id);
            }
            (reverted, None)
        } else {
            self.record_cancelled_request(panel_id, request_id);
            let last_snap_rev = self.panel_last_snapshot_revision(panel_id);
            let inherited_snapshot_rev =
                (last_snap_rev > panel_revision_at_send).then_some(last_snap_rev);
            if expect_late_reply {
                let already_confirmed =
                    self.panel_has_confirmed_viewport_request(panel_id, request_id);
                if is_optimistic && !already_confirmed {
                    self.mark_panel_optimistic_request_uncertain(panel_id, request_id);
                }
                if is_state_req {
                    self.record_cancelled_state_request(
                        panel_id,
                        request_id,
                        is_optimistic,
                        inherited_snapshot_rev,
                    );
                } else if for_resync {
                    self.record_cancelled_resync_request(
                        panel_id,
                        request_id,
                        inherited_snapshot_rev,
                    );
                }
            }
            (
                false,
                send_cancel.then_some((panel_id, request_id, for_resync, inherited_snapshot_rev)),
            )
        };
        self.reject_response_with_mode(
            request_id,
            reason,
            true,
            !retracted_unsent && !expect_late_reply,
        );
        (retracted_unsent, reverted_optimistic, cancel_to_send)
    }

    pub(super) fn cancel_pending_request(
        &mut self,
        request_id: u64,
        reason: &str,
        outgoing: Option<&mpsc::Sender<Envelope>>,
        send_cancel: bool,
        expect_late_reply: bool,
    ) -> bool {
        let req_meta = self.requests.get(&request_id).map(|p| {
            (
                p.panel_id,
                p.ui_event_seq(),
                matches!(
                    p.kind,
                    PendingKind::Event {
                        resync_seq: None,
                        pointer_cancel_seq: None,
                        is_optimistic: true,
                        ..
                    }
                ),
            )
        });
        let (retracted_unsent, reverted_optimistic, cancel_to_send) =
            self.cancel_pending_request_inner(request_id, reason, send_cancel, expect_late_reply);
        if let (Some((panel_id, req_id, for_resync, snap_rev)), Some(out)) =
            (cancel_to_send, outgoing)
        {
            self.send_cancel_best_effort(out, panel_id, req_id, for_resync, snap_rev);
        }
        if let Some((panel_id, source_seq, is_optimistic)) = req_meta
            && (reverted_optimistic || (retracted_unsent && is_optimistic))
        {
            self.notify_panel_synced_with_rejected_nonblocking(
                panel_id,
                (source_seq != 0).then_some(source_seq),
            );
        }
        retracted_unsent
    }

    pub(super) fn classify_drifted_host_call(
        &self,
        panel_id: u64,
        call: &HostCall,
        now: Instant,
    ) -> (bool, bool) {
        if call.parent_request_id == 0 {
            return (false, true);
        }
        if let Some(pending) = self
            .requests
            .get(&call.parent_request_id)
            .filter(|p| p.panel_id == panel_id)
        {
            let live = !pending.cancel.is_cancelled() && pending.deadline > now;
            let is_recovery = pending.kind.is_panel_recovery();
            return (false, live && !is_recovery);
        }
        if let Some(notification) = self.cancel_notifications.iter().rev().find(|n| {
            n.panel_id == panel_id && n.cancel_frame_id == call.parent_request_id
        }) {
            let active = notification.deadline > now;
            return (active, active && notification.can_restart_resync);
        }
        (false, false)
    }

    pub(super) fn v14_resync_degrade_failure(detail: Option<&str>) -> Failure {
        let base = "v1.4 旧版插件重同步超时；为避免向阻塞中的旧 Worker 叠加过期恢复帧已停止自动重同步，请重载或升级插件";
        let message = match detail.map(str::trim).filter(|s| !s.is_empty()) {
            Some(reason) => format!("{base}（原始原因：{reason}）"),
            None => base.to_owned(),
        };
        Failure::new(ErrorCode::Deadline, message)
    }

    pub(super) fn panel_revision(&self, panel_id: u64) -> u64 {
        if panel_id == PANEL_ID {
            self.panel.snapshot().revision
        } else {
            self.panels
                .get(&panel_id)
                .and_then(|slot| slot.state.as_ref())
                .map_or(0, |state| state.snapshot().revision)
        }
    }

    pub(super) fn panel_last_snapshot_revision(&self, panel_id: u64) -> u64 {
        if panel_id == PANEL_ID {
            self.main_last_snapshot_rev
        } else {
            self.panels
                .get(&panel_id)
                .map_or(0, |slot| slot.last_snapshot_rev)
        }
    }

    pub(super) fn record_panel_accepted_snapshot_revision(&mut self, panel_id: u64, revision: u64) {
        if panel_id == PANEL_ID {
            self.main_last_snapshot_rev = self.main_last_snapshot_rev.max(revision);
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            slot.last_snapshot_rev = slot.last_snapshot_rev.max(revision);
        }
    }

    pub(super) fn panel_snapshot(&self, panel_id: u64) -> Option<Arc<PanelSnapshot>> {
        if panel_id == PANEL_ID {
            Some(Arc::new(self.panel.snapshot().clone()))
        } else {
            self.panels
                .get(&panel_id)
                .and_then(|slot| slot.state.as_ref())
                .map(|state| Arc::new(state.snapshot().clone()))
        }
    }

    pub(super) fn panel_capture(&self, panel_id: u64) -> Option<(u64, u64)> {
        if panel_id == PANEL_ID {
            self.panel.captured()
        } else {
            self.panels
                .get(&panel_id)
                .and_then(|slot| slot.state.as_ref())
                .and_then(PanelState::captured)
        }
    }

    pub(super) fn clear_panel_capture(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            self.panel.clear_capture();
        } else if let Some(state) = self
            .panels
            .get_mut(&panel_id)
            .and_then(|slot| slot.state.as_mut())
        {
            state.clear_capture();
        }
    }

    pub(super) fn active_resource_for(&self, panel_id: u64) -> Option<(Uuid, u64)> {
        if panel_id == PANEL_ID {
            self.active_resource
        } else {
            self.panels
                .get(&panel_id)
                .and_then(|slot| slot.active_resource)
        }
    }

    pub(super) fn set_active_resource_for(
        &mut self,
        panel_id: u64,
        value: Option<(Uuid, u64)>,
    ) {
        if panel_id == PANEL_ID {
            self.active_resource = value;
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.active_resource = value;
        }
    }

    pub(super) fn opening_request_for(&self, panel_id: u64) -> Option<u64> {
        if panel_id == PANEL_ID {
            self.opening_request
        } else {
            self.panels
                .get(&panel_id)
                .and_then(|slot| slot.opening_request)
        }
    }

    pub(super) fn set_opening_request_for(&mut self, panel_id: u64, value: Option<u64>) {
        if panel_id == PANEL_ID {
            self.opening_request = value;
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.opening_request = value;
        }
    }

    pub(super) fn record_v14_degraded_resync(&mut self, request_id: u64) {
        if request_id == 0 || self.v14_degraded_resync_ids.contains(&request_id) {
            return;
        }
        if self.v14_degraded_resync_ids.len() >= MAX_V14_DEGRADED_RESYNC_IDS {
            self.v14_degraded_resync_ids.pop_front();
        }
        self.v14_degraded_resync_ids.push_back(request_id);
    }

    pub(super) fn take_v14_degraded_resync(&mut self, request_id: u64) -> bool {
        if request_id == 0 {
            return false;
        }
        if let Some(pos) = self
            .v14_degraded_resync_ids
            .iter()
            .position(|id| *id == request_id)
        {
            self.v14_degraded_resync_ids.remove(pos);
            return true;
        }
        false
    }

    pub(super) fn cleanup_removed_panel(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            return;
        }
        if let Some(mut slot) = self.panels.remove(&panel_id) {
            while let Some(cmd) = slot.deferred_commands.pop_front() {
                self.fail_control_command(cmd.command, "多路复用子面板已关闭");
            }
        }
        self.broker
            .panel_resources
            .retain(|(p, _), _| *p != panel_id);
        self.panel_revoked_resources
            .retain(|(p, _), _| *p != panel_id);
        self.panel_resource_open_seq
            .retain(|(p, _), _| *p != panel_id);
        self.panel_revoke_snapshots.remove(&panel_id);
        self.panel_revoke_snapshot_floor.remove(&panel_id);
        self.pending_panel_syncs.remove(&panel_id);
    }

    pub(super) fn record_cancelled_state_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
        is_optimistic: bool,
        converged_snapshot_rev: Option<u64>,
    ) {
        if request_id == 0 {
            return;
        }
        let kind = if is_optimistic {
            CancelledStateRequestKind::OptimisticPending
        } else {
            CancelledStateRequestKind::NonOptimistic
        };
        let queue = if panel_id == PANEL_ID {
            Some(&mut self.main_cancelled_state_requests)
        } else {
            self.panels
                .get_mut(&panel_id)
                .map(|slot| &mut slot.cancelled_state_requests)
        };
        if let Some(queue) = queue
            && !queue.iter().any(|&(id, _)| id == request_id)
        {
            if queue.len() >= QUEUE_CAPACITY {
                queue.pop_front();
            }
            queue.push_back((
                request_id,
                CancelledStateEntry {
                    kind,
                    can_restart_resync: true,
                    converged_snapshot_rev,
                },
            ));
        }
    }

    pub(super) fn take_cancelled_state_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> Option<CancelledStateEntry> {
        if request_id == 0 {
            return None;
        }
        let queue = if panel_id == PANEL_ID {
            Some(&mut self.main_cancelled_state_requests)
        } else {
            self.panels
                .get_mut(&panel_id)
                .map(|slot| &mut slot.cancelled_state_requests)
        };
        if let Some(queue) = queue
            && let Some(pos) = queue.iter().position(|&(id, _)| id == request_id)
        {
            return queue.remove(pos).map(|(_, entry)| entry);
        }
        None
    }

    pub(super) fn record_cancelled_resync_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
        converged_snapshot_rev: Option<u64>,
    ) {
        if request_id == 0 {
            return;
        }
        let queue = if panel_id == PANEL_ID {
            Some(&mut self.main_cancelled_resync_requests)
        } else {
            self.panels
                .get_mut(&panel_id)
                .map(|slot| &mut slot.cancelled_resync_requests)
        };
        if let Some(queue) = queue
            && !queue.iter().any(|e| e.request_id == request_id)
        {
            if queue.len() >= QUEUE_CAPACITY {
                queue.pop_front();
            }
            queue.push_back(CancelledResyncEntry {
                request_id,
                can_restart_resync: false,
                converged_snapshot_rev,
            });
        }
    }

    pub(super) fn take_cancelled_resync_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> Option<CancelledResyncEntry> {
        if request_id == 0 {
            return None;
        }
        let queue = if panel_id == PANEL_ID {
            Some(&mut self.main_cancelled_resync_requests)
        } else {
            self.panels
                .get_mut(&panel_id)
                .map(|slot| &mut slot.cancelled_resync_requests)
        };
        if let Some(queue) = queue
            && let Some(pos) = queue.iter().position(|e| e.request_id == request_id)
        {
            return queue.remove(pos);
        }
        None
    }

    pub(super) fn mark_panel_destroy_failed(
        &mut self,
        panel_id: u64,
        destroy_request_id: Option<u64>,
    ) {
        if panel_id == PANEL_ID {
            return;
        }
        let deferred = if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.state = None;
            slot.active_resource = None;
            slot.opening_request = None;
            slot.closing = true;
            slot.destroy_failed = true;
            slot.destroy_request_id = destroy_request_id;
            slot.resync_needed_seq = 0;
            slot.resync_attempts = 0;
            slot.resync_semantic_drift = false;
            slot.resync_snapshot_confirmed = false;
            slot.last_snapshot_rev = 0;
            slot.cancelled_state_requests.clear();
            slot.cancelled_resync_requests.clear();
            std::mem::take(&mut slot.deferred_commands)
        } else {
            std::collections::VecDeque::new()
        };
        for cmd in deferred {
            self.fail_control_command(cmd.command, "多路复用子面板已关闭");
        }
        self.broker
            .panel_resources
            .retain(|(p, _), _| *p != panel_id);
        self.panel_revoked_resources
            .retain(|(p, _), _| *p != panel_id);
        self.panel_resource_open_seq
            .retain(|(p, _), _| *p != panel_id);
        self.panel_revoke_snapshots.remove(&panel_id);
        self.panel_revoke_snapshot_floor.remove(&panel_id);
        self.pending_panel_syncs.remove(&panel_id);
    }

    pub(super) fn is_panel_resyncing(&self, panel_id: u64) -> bool {
        if self.has_inflight_resync(panel_id) {
            return true;
        }
        if panel_id == PANEL_ID {
            self.main_resync_needed_seq != 0
        } else {
            self.panels
                .get(&panel_id)
                .is_some_and(|slot| slot.resync_needed_seq != 0)
        }
    }

    pub(super) fn has_inflight_non_destroy_close(&self, panel_id: u64) -> bool {
        self.requests.values().any(|p| {
            p.panel_id == panel_id
                && matches!(
                    p.kind,
                    PendingKind::Close {
                        remove_panel: false,
                        ..
                    }
                )
        })
    }

    pub(super) fn has_pending_non_destroy_revoke(&self, panel_id: u64) -> bool {
        self.pending_revokes
            .iter()
            .any(|r| r.panel_id == panel_id && !r.remove_panel)
    }

    fn inflight_pointer_cancel_seq(&self, panel_id: u64) -> Option<u64> {
        self.requests.values().find_map(|p| {
            (p.panel_id == panel_id)
                .then_some(&p.kind)
                .and_then(|k| match k {
                    PendingKind::Event {
                        pointer_cancel_seq: Some(seq),
                        ..
                    } => Some(*seq),
                    _ => None,
                })
        })
    }

    fn has_inflight_pointer_cancel(&self, panel_id: u64) -> bool {
        self.inflight_pointer_cancel_seq(panel_id).is_some()
    }

    pub(super) fn is_panel_pointer_cancelling(&self, panel_id: u64) -> bool {
        if self.has_inflight_pointer_cancel(panel_id) {
            return true;
        }
        if panel_id == PANEL_ID {
            !self.main_pending_pointer_cancels.is_empty()
        } else {
            self.panels
                .get(&panel_id)
                .is_some_and(|slot| !slot.pending_pointer_cancels.is_empty())
        }
    }

    pub(super) fn is_panel_command_barrier_active(&self, panel_id: u64) -> bool {
        self.is_panel_resyncing(panel_id)
            || self.is_panel_pointer_cancelling(panel_id)
            || self.has_inflight_non_destroy_close(panel_id)
            || self.has_pending_non_destroy_revoke(panel_id)
    }

    pub(super) fn has_inflight_panel_state_request(&self, panel_id: u64) -> bool {
        self.requests.values().any(|p| {
            p.panel_id == panel_id
                && matches!(
                    p.kind,
                    PendingKind::Open { .. }
                        | PendingKind::Event { .. }
                        | PendingKind::Close { .. }
                )
        })
    }

    pub(super) fn has_inflight_deferred_panel_state_request(&self, panel_id: u64) -> bool {
        self.requests.values().any(|p| {
            p.panel_id == panel_id
                && p.from_deferred_queue
                && matches!(
                    p.kind,
                    PendingKind::Open { .. }
                        | PendingKind::Event { .. }
                        | PendingKind::Close { .. }
                )
        })
    }

    pub(super) fn has_deferred_panel_commands(&self, panel_id: u64) -> bool {
        if panel_id == PANEL_ID {
            !self.main_deferred_commands.is_empty()
        } else {
            self.panels
                .get(&panel_id)
                .is_some_and(|slot| !slot.deferred_commands.is_empty())
        }
    }

    fn should_defer_panel_command(&self, panel_id: u64) -> bool {
        self.is_panel_command_barrier_active(panel_id)
            || self.has_deferred_panel_commands(panel_id)
            || self.has_inflight_deferred_panel_state_request(panel_id)
    }

    fn push_deferred_panel_command(
        &mut self,
        panel_id: u64,
        command: SequencedControl,
    ) -> std::result::Result<(), SequencedControl> {
        let queue = if panel_id == PANEL_ID {
            &mut self.main_deferred_commands
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            &mut slot.deferred_commands
        } else {
            return Err(command);
        };
        if queue.len() >= MAX_DEFERRED_PANEL_COMMANDS {
            return Err(command);
        }
        queue.push_back(command);
        Ok(())
    }

    pub(super) fn fail_control_command(&mut self, command: Control, reason: &str) {
        match command {
            Control::Event {
                panel_id,
                event,
                event_seq,
                ..
            } => {
                self.record_processed_ui_seq(panel_id, event_seq);
                let _ = self.events.try_send(PluginEvent::Error {
                    identity: self.identity,
                    panel_id,
                    failure: Failure::new(ErrorCode::Stale, reason),
                });
                if Self::is_optimistic_ui_event(&event) {
                    self.notify_panel_synced_with_rejected_nonblocking(
                        panel_id,
                        (event_seq != 0).then_some(event_seq),
                    );
                }
            }
            Control::Open { reply, .. } | Control::OpenResponse { reply, .. } => {
                let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
            }
            Control::Commit { reply, .. } => {
                let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
            }
            Control::CommitResponse { reply, .. } => {
                let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
            }
            Control::Revoke { reply, .. } => {
                let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
            }
            Control::RevokeBatch { items, .. } => {
                for (_, reply) in items {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                }
            }
            Control::Hook { reply, .. } => {
                let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
            }
            Control::CancelPanelOp { .. } => {}
        }
    }

    pub(super) fn fail_deferred_panel_commands(&mut self, panel_id: u64, reason: &str) {
        let drained = if panel_id == PANEL_ID {
            std::mem::take(&mut self.main_deferred_commands)
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            std::mem::take(&mut slot.deferred_commands)
        } else {
            std::collections::VecDeque::new()
        };
        for cmd in drained {
            self.fail_control_command(cmd.command, reason);
        }
    }

    fn control_panel_id(cmd: &Control) -> Option<u64> {
        match cmd {
            Control::Event { panel_id, .. }
            | Control::Open { panel_id, .. }
            | Control::OpenResponse { panel_id, .. }
            | Control::Commit { panel_id, .. }
            | Control::CommitResponse { panel_id, .. } => Some(*panel_id),
            Control::Revoke { .. }
            | Control::RevokeBatch { .. }
            | Control::CancelPanelOp { .. }
            | Control::Hook { .. } => None,
        }
    }

    fn control_resource_id(cmd: &Control) -> Option<Uuid> {
        match cmd {
            Control::Open { id, .. } | Control::OpenResponse { id, .. } => Some(*id),
            Control::Commit { request, .. } => protocol::uuid(&request.resource_id).ok(),
            Control::CommitResponse { request, .. } => protocol::uuid(&request.resource_id).ok(),
            _ => None,
        }
    }

    fn control_matches_op(cmd: &Control, target_op: Uuid) -> bool {
        matches!(
            cmd,
            Control::Commit { op_id, .. } | Control::CommitResponse { op_id, .. }
                if *op_id == target_op
        )
    }

    fn insert_bounded_seq<K: Eq + std::hash::Hash + Clone>(
        map: &mut HashMap<K, u64>,
        floor: &mut u64,
        max_len: usize,
        key: K,
        seq: u64,
    ) {
        if seq == 0 {
            return;
        }
        if let Some(existing) = map.get_mut(&key) {
            *existing = (*existing).max(seq);
            return;
        }
        while map.len() >= max_len {
            let Some((oldest_key, oldest_seq)) = map
                .iter()
                .min_by_key(|&(_, &s)| s)
                .map(|(k, &s)| (k.clone(), s))
            else {
                break;
            };
            map.remove(&oldest_key);
            *floor = (*floor).max(oldest_seq);
        }
        map.insert(key, seq);
    }

    fn record_panel_destroy_seq(&mut self, panel_id: u64, seq: u64) {
        Self::insert_bounded_seq(
            &mut self.panel_destroy_control_seq,
            &mut self.panel_destroy_seq_floor,
            MAX_TRACKED_DESTROY_SEQS,
            panel_id,
            seq,
        );
    }

    fn record_resource_revoke_seq(&mut self, panel_id: u64, resource_id: Uuid, seq: u64) {
        Self::insert_bounded_seq(
            &mut self.resource_revoke_control_seq,
            &mut self.resource_revoke_seq_floor,
            MAX_TRACKED_REVOKE_SEQS,
            (panel_id, resource_id),
            seq,
        );
    }

    fn record_cancelled_op_seq(&mut self, panel_id: u64, op_id: Uuid, seq: u64) {
        Self::insert_bounded_seq(
            &mut self.cancelled_op_control_seq,
            &mut self.cancelled_op_seq_floor,
            MAX_TRACKED_CANCELLED_OPS,
            (panel_id, op_id),
            seq,
        );
    }

    fn record_panel_revoke_snapshot(
        &mut self,
        panel_id: u64,
        seq: u64,
        snapshot: Arc<PanelSnapshot>,
    ) {
        if seq == 0 {
            return;
        }
        let queue = self.panel_revoke_snapshots.entry(panel_id).or_default();
        let pos = queue.partition_point(|(s, _)| *s < seq);
        if queue.get(pos).is_some_and(|(s, _)| *s == seq) {
            queue[pos].1 = snapshot;
        } else {
            queue.insert(pos, (seq, snapshot));
        }
        while queue.len() > MAX_PANEL_REVOKE_SNAPSHOTS {
            if let Some((evicted_seq, _)) = queue.pop_front() {
                let floor = self
                    .panel_revoke_snapshot_floor
                    .entry(panel_id)
                    .or_insert(0);
                *floor = (*floor).max(evicted_seq);
            }
        }
    }

    fn drain_matching_sequenced_controls(
        queue: &mut std::collections::VecDeque<SequencedControl>,
        mut predicate: impl FnMut(&SequencedControl) -> bool,
    ) -> Vec<Control> {
        let mut removed = Vec::new();
        let mut kept = std::collections::VecDeque::with_capacity(queue.len());
        while let Some(item) = queue.pop_front() {
            if predicate(&item) {
                removed.push(item.command);
            } else {
                kept.push_back(item);
            }
        }
        *queue = kept;
        removed
    }

    fn drain_deferred_for_panel(
        &mut self,
        panel_id: u64,
        reason: &str,
        predicate: impl FnMut(&SequencedControl) -> bool,
    ) {
        let to_fail = if panel_id == PANEL_ID {
            Self::drain_matching_sequenced_controls(&mut self.main_deferred_commands, predicate)
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            Self::drain_matching_sequenced_controls(&mut slot.deferred_commands, predicate)
        } else {
            return;
        };
        for cmd in to_fail {
            self.fail_control_command(cmd, reason);
        }
    }

    pub(super) fn drain_urgent_controls(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.apply_urgent_controls(Vec::new(), outgoing)
    }

    pub(super) fn apply_urgent_controls(
        &mut self,
        mut batch: Vec<SequencedControl>,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        while let Some(urgent) = self.urgent_commands.as_mut() {
            match urgent.try_recv() {
                Ok(item) => batch.push(item),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.urgent_commands = None;
                    break;
                }
            }
        }
        if batch.is_empty() {
            return Ok(());
        }

        let mut cancels_to_send = Vec::new();
        let mut closes_to_dispatch: Vec<PendingUrgentClose> = Vec::new();

        for SequencedControl { seq, command } in batch {
            match command {
                Control::Revoke {
                    panel_id,
                    id,
                    remove_panel,
                    reply,
                } => {
                    let remove_panel = remove_panel && panel_id != PANEL_ID;
                    if remove_panel {
                        self.cancel_destroy_panel_revoke(
                            seq,
                            panel_id,
                            id,
                            reply,
                            &mut cancels_to_send,
                            &mut closes_to_dispatch,
                        );
                    } else if self.cancel_non_destroy_resource_revoke(
                        seq,
                        panel_id,
                        id,
                        &mut cancels_to_send,
                    ) {
                        closes_to_dispatch.push(PendingUrgentClose::NonDestroy {
                            urgent_seq: seq,
                            panel_id,
                            id,
                            reply,
                        });
                    } else {
                        let _ = reply.send(Ok(()));
                    }
                }
                Control::RevokeBatch { panel_id, items } => {
                    for (id, reply) in items {
                        if self.cancel_non_destroy_resource_revoke(
                            seq,
                            panel_id,
                            id,
                            &mut cancels_to_send,
                        ) {
                            closes_to_dispatch.push(PendingUrgentClose::NonDestroy {
                                urgent_seq: seq,
                                panel_id,
                                id,
                                reply,
                            });
                        } else {
                            let _ = reply.send(Ok(()));
                        }
                    }
                }
                Control::CancelPanelOp { panel_id, op_id } => {
                    self.cancel_panel_op_collecting(seq, panel_id, op_id, &mut cancels_to_send);
                }
                _ => {}
            }
        }

        for (p, req_id, for_resync, snap_rev) in cancels_to_send {
            self.send_cancel_best_effort(outgoing, p, req_id, for_resync, snap_rev);
        }

        let mut fatal = None;
        for close in closes_to_dispatch {
            let outcome = match close {
                PendingUrgentClose::NonDestroy {
                    urgent_seq,
                    panel_id,
                    id,
                    reply,
                } => self.dispatch_non_destroy_close(urgent_seq, panel_id, id, reply, outgoing),
                PendingUrgentClose::Destroy {
                    panel_id,
                    id,
                    replies,
                } => self.dispatch_destroy_close(panel_id, id, replies, outgoing),
            };
            if let Err(err) = outcome {
                fatal.get_or_insert(err);
            }
        }
        if let Some(err) = fatal {
            return Err(err);
        }
        Ok(())
    }

    fn cancel_panel_op_collecting(
        &mut self,
        urgent_seq: u64,
        panel_id: u64,
        op_id: Uuid,
        cancels_to_send: &mut Vec<(u64, u64, bool, Option<u64>)>,
    ) {
        if panel_id == PANEL_ID {
            return;
        }
        self.record_cancelled_op_seq(panel_id, op_id, urgent_seq);
        self.drain_deferred_for_panel(panel_id, "插件子面板操作已取消", |item| {
            Self::control_matches_op(&item.command, op_id)
                && (urgent_seq == 0 || item.seq == 0 || item.seq <= urgent_seq)
        });
        let pending: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(request_id, pending)| {
                (pending.panel_id == panel_id
                    && matches!(
                        &pending.kind,
                        PendingKind::Commit { op_id: id, .. }
                            | PendingKind::CommitResponse { op_id: id, .. }
                            if *id == op_id
                    ))
                .then_some(*request_id)
            })
            .collect();
        for request_id in pending {
            let (_, _, cancel_to_send) = self.cancel_pending_request_inner(
                request_id,
                "插件子面板操作已取消",
                true,
                false,
            );
            if let Some(cancel) = cancel_to_send {
                cancels_to_send.push(cancel);
            }
            for service in self.service_cancels.values() {
                if service.panel_id == panel_id && service.parent_request_id == 0 {
                    service.cancel.cancel();
                }
            }
        }
    }

    fn cancel_non_destroy_resource_revoke(
        &mut self,
        urgent_seq: u64,
        panel_id: u64,
        id: Uuid,
        cancels_to_send: &mut Vec<(u64, u64, bool, Option<u64>)>,
    ) -> bool {
        self.record_resource_revoke_seq(panel_id, id, urgent_seq);
        self.drain_deferred_for_panel(
            panel_id,
            "资源已撤销；未提交或发送，保留现有编辑内容",
            |item| {
                Self::control_resource_id(&item.command) == Some(id)
                    && (urgent_seq == 0 || item.seq == 0 || item.seq <= urgent_seq)
            },
        );
        if urgent_seq != 0 {
            let has_newer_bound = self
                .panel_resource_open_seq
                .get(&(panel_id, id))
                .is_some_and(|&s| s > urgent_seq);
            let has_newer_inflight = self.requests.values().any(|p| {
                p.panel_id == panel_id
                    && p.source_seq > urgent_seq
                    && matches!(p.kind, PendingKind::Open { resource, .. } if resource == id)
            });
            let deferred_queue = if panel_id == PANEL_ID {
                Some(&self.main_deferred_commands)
            } else {
                self.panels.get(&panel_id).map(|s| &s.deferred_commands)
            };
            let has_newer_deferred = deferred_queue.is_some_and(|q| {
                q.iter().any(|item| {
                    item.seq > urgent_seq
                        && matches!(
                            item.command,
                            Control::Open { id: rid, .. } | Control::OpenResponse { id: rid, .. }
                                if rid == id
                        )
                })
            });
            if has_newer_bound || has_newer_inflight || has_newer_deferred {
                return false;
            }
        }
        self.panel_resource_open_seq.remove(&(panel_id, id));
        let panel_live = panel_id == PANEL_ID
            || self
                .panels
                .get(&panel_id)
                .is_some_and(|slot| !slot.closing && !slot.destroy_failed);
        if panel_live {
            if let Some(snap) = self.panel_snapshot(panel_id) {
                self.record_panel_revoke_snapshot(panel_id, urgent_seq, snap);
            } else if urgent_seq != 0 {
                let floor = self
                    .panel_revoke_snapshot_floor
                    .entry(panel_id)
                    .or_insert(0);
                *floor = (*floor).max(urgent_seq);
            }
        }
        self.broker.remove_panel_resource(panel_id, id);
        if let Some(barrier) = self
            .service_cancels
            .iter()
            .filter_map(|(request, service)| {
                (service.panel_id == panel_id
                    && matches!(service.binding, Some(PendingResource::OpenRepeater)))
                .then_some(*request)
            })
            .max()
        {
            if panel_id == PANEL_ID {
                self.revoked_resources.insert(id, barrier);
            } else {
                self.panel_revoked_resources.insert((panel_id, id), barrier);
            }
        }
        if panel_live {
            let pending: Vec<_> = self
                .requests
                .iter()
                .filter_map(|(request_id, pending)| {
                    if pending.panel_id != panel_id
                        || (urgent_seq != 0
                            && pending.source_seq != 0
                            && pending.source_seq > urgent_seq)
                    {
                        return None;
                    }
                    let matched = matches!(
                        pending.kind,
                        PendingKind::Open { resource, .. }
                            | PendingKind::Commit { resource, .. }
                            | PendingKind::CommitResponse { resource, .. }
                        if resource == id
                    );
                    matched.then_some((
                        *request_id,
                        !matches!(
                            pending.kind,
                            PendingKind::Event { .. } | PendingKind::Close { .. }
                        ),
                    ))
                })
                .collect();
            if self
                .active_resource_for(panel_id)
                .is_some_and(|(resource, _)| resource == id)
                || self
                    .opening_request_for(panel_id)
                    .is_some_and(|opening| pending.iter().any(|(req, _)| *req == opening))
            {
                self.set_active_resource_for(panel_id, None);
            }
            for (request_id, send_cancel) in pending {
                let (_, _, cancel_to_send) = self.cancel_pending_request_inner(
                    request_id,
                    "资源已撤销；未提交或发送，保留现有编辑内容",
                    send_cancel,
                    true,
                );
                if let Some(cancel) = cancel_to_send {
                    cancels_to_send.push(cancel);
                }
            }
            for service in self.service_cancels.values() {
                if service.panel_id == panel_id
                    && matches!(
                        service.binding,
                        Some(PendingResource::UpdateDraft { id: resource, .. }) if resource == id
                    )
                {
                    service.cancel.cancel();
                }
            }
        }
        true
    }

    fn dispatch_non_destroy_close(
        &mut self,
        urgent_seq: u64,
        panel_id: u64,
        id: Uuid,
        reply: oneshot::Sender<Result<()>>,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        if panel_id != PANEL_ID {
            match self.panels.get(&panel_id) {
                None => {
                    let pending: Vec<_> = self
                        .requests
                        .iter()
                        .filter_map(|(req_id, p)| {
                            (p.panel_id == panel_id
                                && (urgent_seq == 0
                                    || p.source_seq == 0
                                    || p.source_seq <= urgent_seq))
                                .then_some(*req_id)
                        })
                        .collect();
                    for req_id in pending {
                        self.cancel_pending_request(
                            req_id,
                            "资源已撤销；未提交或发送，保留现有编辑内容",
                            None,
                            false,
                            false,
                        );
                    }
                    for service in self.service_cancels.values() {
                        if service.panel_id == panel_id {
                            service.cancel.cancel();
                        }
                    }
                    let _ = reply.send(Ok(()));
                    return Ok(());
                }
                Some(slot) if slot.destroy_failed => {
                    let _ = reply.send(Err(anyhow::anyhow!(
                        "多路复用子面板远端关闭失败，槽位保留至共享实例重启"
                    )));
                    return Ok(());
                }
                Some(slot) if slot.closing => {
                    if let Some(PendingKind::Close { replies, .. }) =
                        self.requests.values_mut().find_map(|p| {
                            (p.panel_id == panel_id
                                && matches!(
                                    p.kind,
                                    PendingKind::Close {
                                        remove_panel: true,
                                        ..
                                    }
                                ))
                            .then_some(&mut p.kind)
                        })
                    {
                        replies.push(reply);
                    } else if let Some(pending_revoke) = self
                        .pending_revokes
                        .iter_mut()
                        .find(|d| d.panel_id == panel_id && d.remove_panel)
                    {
                        pending_revoke.replies.push(reply);
                    } else {
                        let _ = reply.send(Err(anyhow::anyhow!(
                            "多路复用子面板正在关闭但缺少待处理销毁请求"
                        )));
                    }
                    return Ok(());
                }
                Some(_) => {}
            }
        }
        if let Some(PendingKind::Close { replies, .. }) =
            self.requests.values_mut().find_map(|p| {
                (p.panel_id == panel_id
                    && matches!(
                        &p.kind,
                        PendingKind::Close {
                            resource,
                            remove_panel: false,
                            ..
                        } if *resource == id
                    ))
                .then_some(&mut p.kind)
            })
        {
            replies.push(reply);
            return Ok(());
        }
        if let Some(pending_revoke) = self
            .pending_revokes
            .iter_mut()
            .find(|d| d.panel_id == panel_id && !d.remove_panel && d.resource == id)
        {
            pending_revoke.replies.push(reply);
            return Ok(());
        }
        if outgoing.is_closed() {
            let _ = reply.send(Err(anyhow::anyhow!("插件输入 pipe 已关闭")));
            bail!("插件输入 pipe 已关闭");
        }
        let defer_non_destroy_close = self.is_panel_resyncing(panel_id)
            || self.has_inflight_non_destroy_close(panel_id)
            || self.has_pending_non_destroy_revoke(panel_id);
        if defer_non_destroy_close
            || self.requests.len() >= QUEUE_CAPACITY
            || self.outbound_queue.len() >= OUTBOUND_CONTROL_SOFT_LIMIT
        {
            self.pending_revokes.push_back(PendingRevoke {
                panel_id,
                resource: id,
                remove_panel: false,
                replies: vec![reply],
            });
            return Ok(());
        }
        self.enqueue_on_panel(
            outgoing,
            panel_id,
            envelope::Body::CloseRequest(protocol::CloseRequest {
                resource_id: id.as_bytes().to_vec(),
                destroy_panel: false,
            }),
            PendingKind::Close {
                resource: id,
                remove_panel: false,
                replies: vec![reply],
            },
        )?;
        Ok(())
    }

    fn cancel_destroy_panel_revoke(
        &mut self,
        urgent_seq: u64,
        panel_id: u64,
        id: Uuid,
        reply: oneshot::Sender<Result<()>>,
        cancels_to_send: &mut Vec<(u64, u64, bool, Option<u64>)>,
        closes_to_dispatch: &mut Vec<PendingUrgentClose>,
    ) {
        self.record_panel_destroy_seq(panel_id, urgent_seq);
        self.drain_deferred_for_panel(panel_id, "多路复用子面板正在关闭", |item| {
            urgent_seq == 0 || item.seq == 0 || item.seq <= urgent_seq
        });
        if urgent_seq != 0 {
            let has_newer_bound = self
                .panel_resource_open_seq
                .iter()
                .any(|(&(p, _), &s)| p == panel_id && s > urgent_seq);
            let has_newer_inflight = self.requests.values().any(|p| {
                p.panel_id == panel_id
                    && p.source_seq > urgent_seq
                    && matches!(p.kind, PendingKind::Open { .. })
            });
            let deferred_queue = self.panels.get(&panel_id).map(|s| &s.deferred_commands);
            let has_newer_deferred = deferred_queue.is_some_and(|q| {
                q.iter().any(|item| {
                    item.seq > urgent_seq
                        && matches!(
                            item.command,
                            Control::Open { .. } | Control::OpenResponse { .. }
                        )
                })
            });
            if has_newer_bound || has_newer_inflight || has_newer_deferred {
                let _ = reply.send(Ok(()));
                return;
            }
        }
        self.panel_revoke_snapshots.remove(&panel_id);
        self.panel_revoke_snapshot_floor.remove(&panel_id);
        self.panel_resource_open_seq
            .retain(|(p, _), _| *p != panel_id);
        self.broker.remove_panel_resource(panel_id, id);
        if let Some(barrier) = self
            .service_cancels
            .iter()
            .filter_map(|(request, service)| {
                (service.panel_id == panel_id
                    && matches!(service.binding, Some(PendingResource::OpenRepeater)))
                .then_some(*request)
            })
            .max()
        {
            self.panel_revoked_resources.insert((panel_id, id), barrier);
        }
        match self.panels.get_mut(&panel_id) {
            None => {
                let pending: Vec<_> = self
                    .requests
                    .iter()
                    .filter_map(|(req_id, p)| {
                        (p.panel_id == panel_id
                            && (urgent_seq == 0
                                || p.source_seq == 0
                                || p.source_seq <= urgent_seq))
                            .then_some(*req_id)
                    })
                    .collect();
                for req_id in pending {
                    self.cancel_pending_request(
                        req_id,
                        "资源已撤销；未提交或发送，保留现有编辑内容",
                        None,
                        false,
                        false,
                    );
                }
                for service in self.service_cancels.values() {
                    if service.panel_id == panel_id {
                        service.cancel.cancel();
                    }
                }
                let _ = reply.send(Ok(()));
                return;
            }
            Some(slot) if slot.destroy_failed => {
                let _ = reply.send(Err(anyhow::anyhow!(
                    "多路复用子面板远端关闭失败，槽位保留至共享实例重启"
                )));
                return;
            }
            Some(slot) if slot.closing => {
                if let Some(PendingUrgentClose::Destroy { replies, .. }) =
                    closes_to_dispatch.iter_mut().find(|item| {
                        matches!(
                            item,
                            PendingUrgentClose::Destroy { panel_id: p, .. } if *p == panel_id
                        )
                    })
                {
                    replies.push(reply);
                } else if let Some(PendingKind::Close { replies, .. }) =
                    self.requests.values_mut().find_map(|p| {
                        (p.panel_id == panel_id
                            && matches!(
                                p.kind,
                                PendingKind::Close {
                                    remove_panel: true,
                                    ..
                                }
                            ))
                        .then_some(&mut p.kind)
                    })
                {
                    replies.push(reply);
                } else if let Some(pending_revoke) = self
                    .pending_revokes
                    .iter_mut()
                    .find(|d| d.panel_id == panel_id && d.remove_panel)
                {
                    pending_revoke.replies.push(reply);
                } else {
                    let _ = reply.send(Err(anyhow::anyhow!(
                        "多路复用子面板正在关闭但缺少待处理销毁请求"
                    )));
                }
                return;
            }
            Some(slot) => {
                slot.closing = true;
                slot.resync_needed_seq = 0;
                slot.resync_attempts = 0;
                slot.resync_semantic_drift = false;
                slot.pending_pointer_cancels.clear();
            }
        }
        let pending: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(request_id, pending)| {
                if pending.panel_id != panel_id
                    || (urgent_seq != 0
                        && pending.source_seq != 0
                        && pending.source_seq > urgent_seq)
                {
                    return None;
                }
                Some((
                    *request_id,
                    !matches!(
                        pending.kind,
                        PendingKind::Event { .. } | PendingKind::Close { .. }
                    ),
                ))
            })
            .collect();
        self.set_active_resource_for(panel_id, None);
        for (request_id, send_cancel) in pending {
            let (_, _, cancel_to_send) = self.cancel_pending_request_inner(
                request_id,
                "资源已撤销；未提交或发送，保留现有编辑内容",
                send_cancel,
                false,
            );
            if let Some(cancel) = cancel_to_send {
                cancels_to_send.push(cancel);
            }
        }
        for service in self.service_cancels.values() {
            if service.panel_id == panel_id {
                service.cancel.cancel();
            }
        }
        let mut replies = vec![reply];
        let mut kept_closes = Vec::with_capacity(closes_to_dispatch.len());
        for item in closes_to_dispatch.drain(..) {
            match item {
                PendingUrgentClose::NonDestroy {
                    panel_id: p,
                    reply: r,
                    ..
                } if p == panel_id => {
                    replies.push(r);
                }
                other => kept_closes.push(other),
            }
        }
        *closes_to_dispatch = kept_closes;
        let mut remaining = std::collections::VecDeque::new();
        while let Some(mut item) = self.pending_revokes.pop_front() {
            if item.panel_id == panel_id && !item.remove_panel {
                replies.append(&mut item.replies);
            } else {
                remaining.push_back(item);
            }
        }
        self.pending_revokes = remaining;
        closes_to_dispatch.push(PendingUrgentClose::Destroy {
            panel_id,
            id,
            replies,
        });
    }

    fn dispatch_destroy_close(
        &mut self,
        panel_id: u64,
        id: Uuid,
        replies: Vec<oneshot::Sender<Result<()>>>,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        if outgoing.is_closed() {
            for r in replies {
                let _ = r.send(Err(anyhow::anyhow!("插件输入 pipe 已关闭")));
            }
            bail!("插件输入 pipe 已关闭");
        }
        if self.requests.len() >= QUEUE_CAPACITY
            || self.outbound_queue.len() >= OUTBOUND_CONTROL_SOFT_LIMIT
        {
            self.pending_revokes.push_back(PendingRevoke {
                panel_id,
                resource: id,
                remove_panel: true,
                replies,
            });
            return Ok(());
        }
        self.enqueue_on_panel(
            outgoing,
            panel_id,
            envelope::Body::CloseRequest(protocol::CloseRequest {
                resource_id: id.as_bytes().to_vec(),
                destroy_panel: true,
            }),
            PendingKind::Close {
                resource: id,
                remove_panel: true,
                replies,
            },
        )?;
        Ok(())
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

    pub(super) fn has_expired_fatal_request(&self, now: Instant) -> bool {
        if self
            .outbound_claims
            .as_deref()
            .is_some_and(OutboundWriteClaims::is_fatal_expired_triggered)
        {
            return true;
        }
        self.requests.values().any(|p| {
            p.deadline <= now
                && ((p.panel_id == PANEL_ID && !p.kind.is_panel_recovery())
                    || matches!(p.kind, PendingKind::Hook { .. }))
        })
    }

    pub(super) fn sync_fatal_deadline_to_claims(&self) {
        if let Some(claims) = self.outbound_claims.as_ref() {
            let earliest = self
                .requests
                .values()
                .filter(|p| {
                    (p.panel_id == PANEL_ID && !p.kind.is_panel_recovery())
                        || matches!(p.kind, PendingKind::Hook { .. })
                })
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
            .filter(|&(&id, p)| {
                id != in_reply_to
                    && ((p.panel_id == PANEL_ID && !p.kind.is_panel_recovery())
                        || matches!(p.kind, PendingKind::Hook { .. }))
            })
            .map(|(_, p)| p.deadline)
            .min();
        if !claims.update_fatal_deadline_unless_triggered(next_earliest) {
            let now = Instant::now();
            let in_reply_is_fatal = self.requests.get(&in_reply_to).is_some_and(|p| {
                (p.panel_id == PANEL_ID && !p.kind.is_panel_recovery())
                    || matches!(p.kind, PendingKind::Hook { .. })
            });
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
                .filter(|&(_, p)| {
                    (p.panel_id == PANEL_ID && !p.kind.is_panel_recovery())
                        || matches!(p.kind, PendingKind::Hook { .. })
                })
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

    pub(super) fn schedule_panel_resync(&mut self, panel_id: u64) {
        let started = if panel_id == PANEL_ID {
            if !self.main_resync_incompatible && self.main_resync_needed_seq == 0 {
                self.next_resync_seq = self.next_resync_seq.saturating_add(1).max(1);
                self.main_resync_needed_seq = self.next_resync_seq;
                self.main_resync_attempts = 0;
                true
            } else {
                false
            }
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && !slot.resync_incompatible
            && slot.state.is_some()
            && slot.resync_needed_seq == 0
        {
            self.next_resync_seq = self.next_resync_seq.saturating_add(1).max(1);
            slot.resync_needed_seq = self.next_resync_seq;
            slot.resync_attempts = 0;
            true
        } else {
            false
        };
        if started {
            self.clear_panel_resync_restart_eligibility(panel_id);
        }
    }

    pub(super) fn processed_ui_seq(&self, panel_id: u64) -> u64 {
        if panel_id == PANEL_ID {
            self.main_processed_ui_seq
        } else {
            self.panels
                .get(&panel_id)
                .map_or(0, |slot| slot.processed_ui_seq)
        }
    }

    pub(super) fn record_processed_ui_seq(&mut self, panel_id: u64, seq: u64) {
        if seq == 0 {
            return;
        }
        if panel_id == PANEL_ID {
            self.main_processed_ui_seq = self.main_processed_ui_seq.max(seq);
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            slot.processed_ui_seq = slot.processed_ui_seq.max(seq);
        }
    }

    fn active_panel_state(&self, panel_id: u64) -> Option<&PanelState> {
        if panel_id == PANEL_ID {
            Some(&self.panel)
        } else {
            self.panels
                .get(&panel_id)
                .filter(|slot| !slot.closing)
                .and_then(|slot| slot.state.as_ref())
        }
    }

    pub(super) fn has_panel_optimistic_request(&self, panel_id: u64, request_id: u64) -> bool {
        self.active_panel_state(panel_id)
            .is_some_and(|state| state.has_optimistic_request(request_id))
    }

    pub(super) fn has_panel_tracked_optimistic_request(
        &self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        self.active_panel_state(panel_id)
            .is_some_and(|state| state.has_tracked_optimistic_request(request_id))
    }

    pub(super) fn panel_has_confirmed_viewport_request(
        &self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        self.active_panel_state(panel_id)
            .is_some_and(|state| state.has_confirmed_viewport_request(request_id))
    }

    pub(super) fn panel_has_detached_viewport(&self, panel_id: u64, request_id: u64) -> bool {
        self.active_panel_state(panel_id)
            .is_some_and(|state| state.has_detached_viewport(request_id))
    }

    pub(super) fn is_optimistic_ui_event(event: &UiEvent) -> bool {
        matches!(
            event.event,
            Some(
                panel::ui_event::Event::Viewport(_)
                    | panel::ui_event::Event::TabSelected(_)
                    | panel::ui_event::Event::SplitResized(_)
                    | panel::ui_event::Event::Toggled(_)
                    | panel::ui_event::Event::SelectedOption(_)
            )
        )
    }

    fn min_unresolved_ui_seq(&self, panel_id: u64) -> Option<u64> {
        let queue = if panel_id == PANEL_ID {
            Some(&self.main_deferred_commands)
        } else {
            self.panels
                .get(&panel_id)
                .map(|slot| &slot.deferred_commands)
        };
        let min_deferred = queue.into_iter().flatten().filter_map(|item| match item.command {
            Control::Event { event_seq, .. } if event_seq != 0 => Some(event_seq),
            _ => None,
        });
        let min_inflight = self.requests.values().filter_map(|p| {
            let seq = p.ui_event_seq();
            (p.panel_id == panel_id && seq != 0).then_some(seq)
        });
        min_deferred.chain(min_inflight).min()
    }

    pub(super) fn contiguous_processed_ui_seq(&self, panel_id: u64) -> u64 {
        let raw = self.processed_ui_seq(panel_id);
        match self.min_unresolved_ui_seq(panel_id) {
            Some(min_seq) => raw.min(min_seq.saturating_sub(1)),
            None => raw,
        }
    }

    pub(super) fn ui_panel_snapshot(&self, panel_id: u64) -> Option<Arc<PanelSnapshot>> {
        let state = if panel_id == PANEL_ID {
            &self.panel
        } else {
            self.panels.get(&panel_id)?.state.as_ref()?
        };
        let active: std::collections::HashSet<u64> = self
            .requests
            .iter()
            .filter_map(|(&req_id, p)| (p.panel_id == panel_id).then_some(req_id))
            .collect();
        Some(Arc::new(state.snapshot_with_confirmed_viewports(&active)))
    }

    pub(super) fn notify_panel_synced_nonblocking(&mut self, panel_id: u64) {
        self.notify_panel_synced_with_rejected_nonblocking(panel_id, None);
    }

    pub(super) fn notify_panel_synced_with_rejected_nonblocking(
        &mut self,
        panel_id: u64,
        rejected_seq: Option<u64>,
    ) {
        if panel_id != PANEL_ID && self.panels.get(&panel_id).is_some_and(|s| s.closing) {
            return;
        }
        if self.defer_panel_sync_for_publish {
            if let Some(seq) = rejected_seq.filter(|&s| s != 0) {
                let entry = self.pending_panel_syncs.entry(panel_id).or_default();
                entry.insert(seq);
                while entry.len() > QUEUE_CAPACITY {
                    entry.pop_first();
                }
            }
            return;
        }
        let Some(snapshot) = self.ui_panel_snapshot(panel_id) else {
            return;
        };
        let synced_ui_seq = self.contiguous_processed_ui_seq(panel_id);
        let mut rejected = self
            .pending_panel_syncs
            .remove(&panel_id)
            .unwrap_or_default();
        if let Some(seq) = rejected_seq.filter(|&s| s != 0) {
            rejected.insert(seq);
        }
        while rejected.len() > QUEUE_CAPACITY {
            rejected.pop_first();
        }
        let rejected_ui_seqs: Vec<u64> = rejected.iter().copied().collect();
        match self.events.try_send(PluginEvent::PanelSynced {
            identity: self.identity,
            panel_id,
            snapshot,
            synced_ui_seq,
            rejected_ui_seqs,
        }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.pending_panel_syncs.insert(panel_id, rejected);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    pub(super) fn retract_panel_optimistic_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        if panel_id == PANEL_ID {
            self.panel.retract_optimistic_request(request_id)
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_mut()
        {
            state.retract_optimistic_request(request_id)
        } else {
            false
        }
    }

    pub(super) fn panel_has_uncertain_optimistic(&self, panel_id: u64) -> bool {
        if panel_id == PANEL_ID {
            self.panel.has_uncertain_optimistic_requests()
        } else {
            self.panels
                .get(&panel_id)
                .filter(|slot| !slot.closing)
                .and_then(|slot| slot.state.as_ref())
                .is_some_and(PanelState::has_uncertain_optimistic_requests)
        }
    }

    pub(super) fn panel_patch_confirms_detached_viewport(
        &self,
        panel_id: u64,
        patch: &panel::PanelPatch,
        request_id: u64,
    ) -> bool {
        if panel_id == PANEL_ID {
            self.panel
                .patch_confirms_detached_viewport(patch, request_id)
        } else {
            self.panels
                .get(&panel_id)
                .filter(|slot| !slot.closing)
                .and_then(|slot| slot.state.as_ref())
                .is_some_and(|state| state.patch_confirms_detached_viewport(patch, request_id))
        }
    }

    pub(super) fn mark_panel_optimistic_request_uncertain(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        if panel_id == PANEL_ID {
            self.panel.mark_optimistic_request_uncertain(request_id)
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_mut()
        {
            state.mark_optimistic_request_uncertain(request_id)
        } else {
            false
        }
    }

    pub(super) fn retract_panel_uncertain_optimistic_requests(&mut self, panel_id: u64) -> bool {
        if panel_id == PANEL_ID {
            self.panel.retract_uncertain_optimistic_requests()
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_mut()
        {
            state.retract_uncertain_optimistic_requests()
        } else {
            false
        }
    }

    pub(super) fn clear_panel_resync_restart_eligibility(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            self.main_resync_snapshot_confirmed = false;
            for (_, cancelled) in &mut self.main_cancelled_state_requests {
                cancelled.can_restart_resync = false;
            }
            for entry in &mut self.main_cancelled_resync_requests {
                entry.can_restart_resync = false;
            }
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.resync_snapshot_confirmed = false;
            for (_, cancelled) in &mut slot.cancelled_state_requests {
                cancelled.can_restart_resync = false;
            }
            for entry in &mut slot.cancelled_resync_requests {
                entry.can_restart_resync = false;
            }
        }
        for notification in &mut self.cancel_notifications {
            if notification.panel_id == panel_id {
                notification.can_restart_resync = false;
            }
        }
    }

    pub(super) fn mark_panel_resync_snapshot_confirmed(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            self.main_resync_snapshot_confirmed = true;
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            slot.resync_snapshot_confirmed = true;
        }
    }

    pub(super) fn take_panel_resync_snapshot_confirmed(&mut self, panel_id: u64) -> bool {
        if panel_id == PANEL_ID {
            std::mem::take(&mut self.main_resync_snapshot_confirmed)
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            std::mem::take(&mut slot.resync_snapshot_confirmed)
        } else {
            false
        }
    }

    pub(super) fn settle_panel_uncertain_optimistic_requests_through(
        &mut self,
        panel_id: u64,
        max_request_id: u64,
    ) -> bool {
        let settled = if panel_id == PANEL_ID {
            let (settled, promoted_ids) = self
                .panel
                .settle_uncertain_optimistic_requests_through_with_promoted(max_request_id);
            if max_request_id != 0 {
                for (req_id, cancelled) in &mut self.main_cancelled_state_requests {
                    if *req_id <= max_request_id {
                        cancelled.can_restart_resync = true;
                        if cancelled.kind == CancelledStateRequestKind::OptimisticPending
                            && !self.panel.has_tracked_optimistic_request(*req_id)
                        {
                            cancelled.kind = if promoted_ids.contains(req_id) {
                                CancelledStateRequestKind::OptimisticPromotedByResyncAck
                            } else {
                                CancelledStateRequestKind::OptimisticDetachedSettledByResyncAck
                            };
                        }
                    }
                }
                for entry in &mut self.main_cancelled_resync_requests {
                    if entry.request_id <= max_request_id {
                        entry.can_restart_resync = true;
                    }
                }
            }
            settled
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_mut()
        {
            let (settled, promoted_ids) =
                state.settle_uncertain_optimistic_requests_through_with_promoted(max_request_id);
            if max_request_id != 0 {
                for (req_id, cancelled) in &mut slot.cancelled_state_requests {
                    if *req_id <= max_request_id {
                        cancelled.can_restart_resync = true;
                        if cancelled.kind == CancelledStateRequestKind::OptimisticPending
                            && !state.has_tracked_optimistic_request(*req_id)
                        {
                            cancelled.kind = if promoted_ids.contains(req_id) {
                                CancelledStateRequestKind::OptimisticPromotedByResyncAck
                            } else {
                                CancelledStateRequestKind::OptimisticDetachedSettledByResyncAck
                            };
                        }
                    }
                }
                for entry in &mut slot.cancelled_resync_requests {
                    if entry.request_id <= max_request_id {
                        entry.can_restart_resync = true;
                    }
                }
            }
            settled
        } else {
            return false;
        };
        if max_request_id != 0 {
            for notification in &mut self.cancel_notifications {
                if notification.panel_id == panel_id
                    && notification.cancel_frame_id <= max_request_id
                {
                    notification.can_restart_resync = true;
                }
            }
        }
        settled
    }

    pub(super) fn mark_panel_cancelled_requests_converged_by_snapshot(
        &mut self,
        panel_id: u64,
        max_request_id: u64,
        snapshot_revision: u64,
        recovered: bool,
    ) {
        if max_request_id == 0 {
            return;
        }
        if panel_id == PANEL_ID {
            for (req_id, cancelled) in &mut self.main_cancelled_state_requests {
                if *req_id <= max_request_id {
                    if !self.panel.has_tracked_optimistic_request(*req_id) {
                        cancelled.kind = CancelledStateRequestKind::NonOptimistic;
                    }
                    if recovered {
                        cancelled.can_restart_resync = true;
                    }
                    cancelled.converged_snapshot_rev = Some(
                        cancelled
                            .converged_snapshot_rev
                            .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                    );
                }
            }
            for entry in &mut self.main_cancelled_resync_requests {
                if entry.request_id <= max_request_id {
                    if recovered {
                        entry.can_restart_resync = true;
                    }
                    entry.converged_snapshot_rev = Some(
                        entry
                            .converged_snapshot_rev
                            .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                    );
                }
            }
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_ref()
        {
            for (req_id, cancelled) in &mut slot.cancelled_state_requests {
                if *req_id <= max_request_id {
                    if !state.has_tracked_optimistic_request(*req_id) {
                        cancelled.kind = CancelledStateRequestKind::NonOptimistic;
                    }
                    if recovered {
                        cancelled.can_restart_resync = true;
                    }
                    cancelled.converged_snapshot_rev = Some(
                        cancelled
                            .converged_snapshot_rev
                            .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                    );
                }
            }
            for entry in &mut slot.cancelled_resync_requests {
                if entry.request_id <= max_request_id {
                    if recovered {
                        entry.can_restart_resync = true;
                    }
                    entry.converged_snapshot_rev = Some(
                        entry
                            .converged_snapshot_rev
                            .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                    );
                }
            }
        } else {
            return;
        }
        for notification in &mut self.cancel_notifications {
            if notification.panel_id == panel_id && notification.cancel_frame_id <= max_request_id {
                if recovered {
                    notification.can_restart_resync = true;
                }
                notification.converged_snapshot_rev = Some(
                    notification
                        .converged_snapshot_rev
                        .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                );
            }
        }
    }

    pub(super) fn mark_panel_cancelled_requests_updated_by_snapshot(
        &mut self,
        panel_id: u64,
        snapshot_revision: u64,
        recovered: bool,
    ) {
        if panel_id == PANEL_ID {
            for (req_id, cancelled) in &mut self.main_cancelled_state_requests {
                if !self.panel.has_tracked_optimistic_request(*req_id)
                    && matches!(
                        cancelled.kind,
                        CancelledStateRequestKind::OptimisticPromotedByResyncAck
                            | CancelledStateRequestKind::OptimisticDetachedSettledByResyncAck
                    )
                {
                    cancelled.kind = CancelledStateRequestKind::NonOptimistic;
                }
                if recovered {
                    cancelled.can_restart_resync = true;
                }
                cancelled.converged_snapshot_rev = Some(
                    cancelled
                        .converged_snapshot_rev
                        .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                );
            }
            for entry in &mut self.main_cancelled_resync_requests {
                if recovered {
                    entry.can_restart_resync = true;
                }
                entry.converged_snapshot_rev = Some(
                    entry
                        .converged_snapshot_rev
                        .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                );
            }
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_ref()
        {
            for (req_id, cancelled) in &mut slot.cancelled_state_requests {
                if !state.has_tracked_optimistic_request(*req_id)
                    && matches!(
                        cancelled.kind,
                        CancelledStateRequestKind::OptimisticPromotedByResyncAck
                            | CancelledStateRequestKind::OptimisticDetachedSettledByResyncAck
                    )
                {
                    cancelled.kind = CancelledStateRequestKind::NonOptimistic;
                }
                if recovered {
                    cancelled.can_restart_resync = true;
                }
                cancelled.converged_snapshot_rev = Some(
                    cancelled
                        .converged_snapshot_rev
                        .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                );
            }
            for entry in &mut slot.cancelled_resync_requests {
                if recovered {
                    entry.can_restart_resync = true;
                }
                entry.converged_snapshot_rev = Some(
                    entry
                        .converged_snapshot_rev
                        .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                );
            }
        } else {
            return;
        }
        for notification in &mut self.cancel_notifications {
            if notification.panel_id == panel_id {
                if recovered {
                    notification.can_restart_resync = true;
                }
                notification.converged_snapshot_rev = Some(
                    notification
                        .converged_snapshot_rev
                        .map_or(snapshot_revision, |prev| prev.max(snapshot_revision)),
                );
            }
        }
    }

    pub(super) fn unclaimed_request_ids_under_barrier(
        &self,
        panel_id: u64,
        barrier: Option<&super::OutboundWriteClaimsGuard<'_>>,
    ) -> std::collections::HashSet<u64> {
        let mut unclaimed = barrier.map_or_else(std::collections::HashSet::new, |guard| {
            guard.pending_request_ids(panel_id)
        });
        for env in &self.outbound_queue {
            if env.panel_id == panel_id && env.request_id != 0 {
                unclaimed.insert(env.request_id);
            }
        }
        unclaimed
    }

    pub(super) fn settle_panel_optimistic_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        if panel_id == PANEL_ID {
            self.panel.settle_optimistic_request(request_id)
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_mut()
        {
            state.settle_optimistic_request(request_id)
        } else {
            false
        }
    }

    pub(super) fn settle_cancelled_panel_optimistic_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        if panel_id == PANEL_ID {
            self.panel.settle_cancelled_optimistic_request(request_id)
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && let Some(state) = slot.state.as_mut()
        {
            state.settle_cancelled_optimistic_request(request_id)
        } else {
            false
        }
    }

    pub(super) fn retract_drift_candidates_under_barrier(
        &mut self,
        panel_id: u64,
        exclude_ids: &BTreeSet<u64>,
        mut barrier: Option<&mut super::OutboundWriteClaimsGuard<'_>>,
    ) -> Vec<u64> {
        let mut candidates: Vec<(u64, u64)> = self
            .requests
            .iter()
            .filter_map(|(&req_id, p)| {
                (p.panel_id == panel_id
                    && !exclude_ids.contains(&req_id)
                    && matches!(
                        p.kind,
                        PendingKind::Event {
                            is_editor_changed: false,
                            resync_seq: None,
                            pointer_cancel_seq: None,
                            ..
                        } | PendingKind::Commit { .. }
                            | PendingKind::CommitResponse { .. }
                    ))
                .then_some((panel_id, req_id))
            })
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }
        candidates.sort_unstable_by_key(|&(_, req_id)| req_id);
        let candidate_ids: BTreeSet<u64> = candidates.iter().map(|&(_, req_id)| req_id).collect();
        let mut retracted_ids = BTreeSet::new();
        if let Some(guard) = barrier.as_deref_mut() {
            for (_, req_id) in guard.try_retract_batch(&candidates) {
                retracted_ids.insert(req_id);
            }
        }
        self.outbound_queue.retain(|env| {
            let remove = env.panel_id == panel_id && candidate_ids.contains(&env.request_id);
            if remove {
                if let Some(guard) = barrier.as_deref_mut() {
                    guard.remove(env.panel_id, env.request_id);
                }
                retracted_ids.insert(env.request_id);
            }
            !remove
        });
        retracted_ids.into_iter().collect()
    }

    pub(super) fn retract_invalidated_optimistic_under_barrier(
        &mut self,
        panel_id: u64,
        request_ids: &[u64],
        force_drift_retract: bool,
        mut barrier: Option<&mut super::OutboundWriteClaimsGuard<'_>>,
    ) -> RetractedOptimisticPlan {
        let mut retracted_set = BTreeSet::new();
        let mut has_claimed_invalidated = false;
        if !request_ids.is_empty() {
            let candidate_ids: BTreeSet<u64> = request_ids.iter().copied().collect();
            let mut candidates: Vec<(u64, u64)> = self
                .requests
                .iter()
                .filter_map(|(&req_id, p)| {
                    (p.panel_id == panel_id
                        && candidate_ids.contains(&req_id)
                        && matches!(
                            p.kind,
                            PendingKind::Event {
                                is_editor_changed: false,
                                resync_seq: None,
                                pointer_cancel_seq: None,
                                ..
                            }
                        ))
                    .then_some((panel_id, req_id))
                })
                .collect();
            if !candidates.is_empty() {
                let candidate_count = candidates.len();
                candidates.sort_unstable_by_key(|&(_, req_id)| req_id);
                let filtered_ids: BTreeSet<u64> =
                    candidates.iter().map(|&(_, req_id)| req_id).collect();
                if let Some(guard) = barrier.as_deref_mut() {
                    for (_, req_id) in guard.try_retract_batch(&candidates) {
                        retracted_set.insert(req_id);
                    }
                }
                self.outbound_queue.retain(|env| {
                    let remove = env.panel_id == panel_id && filtered_ids.contains(&env.request_id);
                    if remove {
                        if let Some(guard) = barrier.as_deref_mut() {
                            guard.remove(env.panel_id, env.request_id);
                        }
                        retracted_set.insert(env.request_id);
                    }
                    !remove
                });
                has_claimed_invalidated = retracted_set.len() < candidate_count;
            }
        }
        let retracted_on_drift = if has_claimed_invalidated || force_drift_retract {
            self.retract_drift_candidates_under_barrier(panel_id, &retracted_set, barrier)
        } else {
            Vec::new()
        };
        RetractedOptimisticPlan {
            retracted_invalidated: retracted_set.into_iter().collect(),
            has_claimed_invalidated,
            retracted_on_drift,
        }
    }

    pub(super) fn finish_retracted_drift_requests(
        &mut self,
        panel_id: u64,
        retracted_ids: Vec<u64>,
    ) {
        if retracted_ids.is_empty() {
            return;
        }
        enum RetractedAction {
            Commit,
            CommitResponse,
            Event {
                capture: PointerCaptureTransition,
                is_optimistic: bool,
                source_seq: u64,
            },
        }
        let mut reverted_any = false;
        let mut retracted_optimistic = false;
        for req_id in retracted_ids.into_iter().rev() {
            let action = match self
                .requests
                .get(&req_id)
                .map(|p| (&p.kind, p.ui_event_seq()))
            {
                Some((PendingKind::Commit { .. }, _)) => Some(RetractedAction::Commit),
                Some((PendingKind::CommitResponse { .. }, _)) => {
                    Some(RetractedAction::CommitResponse)
                }
                Some((
                    PendingKind::Event {
                        pointer_capture,
                        is_optimistic,
                        ..
                    },
                    source_seq,
                )) => Some(RetractedAction::Event {
                    capture: *pointer_capture,
                    is_optimistic: *is_optimistic,
                    source_seq,
                }),
                _ => None,
            };
            match action {
                Some(RetractedAction::Commit) => {
                    self.reject_response(
                        req_id,
                        "插件面板尚未通过最新快照确认状态；已撤回未发出的编辑提交，请刷新或重载面板",
                    );
                }
                Some(RetractedAction::CommitResponse) => {
                    self.reject_response(
                        req_id,
                        "插件面板尚未通过最新快照确认状态；已撤回未发出的响应编辑提交，请刷新或重载面板",
                    );
                }
                Some(RetractedAction::Event {
                    capture,
                    is_optimistic,
                    source_seq,
                }) => {
                    if self.retract_panel_optimistic_request(panel_id, req_id) {
                        reverted_any = true;
                    }
                    if is_optimistic {
                        retracted_optimistic = true;
                        if source_seq != 0 {
                            let entry = self.pending_panel_syncs.entry(panel_id).or_default();
                            entry.insert(source_seq);
                            while entry.len() > QUEUE_CAPACITY {
                                entry.pop_first();
                            }
                        }
                    }
                    self.reject_response(
                        req_id,
                        "插件面板状态已偏离；已撤回尚未发出的旧视图交互操作，请刷新或重载面板",
                    );
                    self.settle_pointer_capture_on_failure(panel_id, capture, true);
                    let _ = self.events.try_send(PluginEvent::Error {
                        identity: self.identity,
                        panel_id,
                        failure: Failure::new(
                            ErrorCode::Stale,
                            "插件面板状态已偏离；已撤回尚未发出的旧视图交互操作，请刷新或重载面板",
                        ),
                    });
                }
                None => {}
            }
        }
        if reverted_any || retracted_optimistic {
            self.notify_panel_synced_nonblocking(panel_id);
        }
    }

    pub(super) fn finish_retracted_optimistic_plan(
        &mut self,
        panel_id: u64,
        plan: RetractedOptimisticPlan,
    ) {
        for req_id in plan.retracted_invalidated.into_iter().rev() {
            let info = self.requests.get(&req_id).and_then(|p| match &p.kind {
                PendingKind::Event {
                    pointer_capture, ..
                } => Some((*pointer_capture, p.ui_event_seq())),
                _ => None,
            });
            if let Some((capture, source_seq)) = info {
                if source_seq != 0 {
                    let entry = self.pending_panel_syncs.entry(panel_id).or_default();
                    entry.insert(source_seq);
                    while entry.len() > QUEUE_CAPACITY {
                        entry.pop_first();
                    }
                }
                self.reject_response(
                    req_id,
                    "插件面板控件语义已变更；已撤回尚未发出的旧视图交互操作，请刷新或重试",
                );
                self.settle_pointer_capture_on_failure(panel_id, capture, true);
                let _ = self.events.try_send(PluginEvent::Error {
                    identity: self.identity,
                    panel_id,
                    failure: Failure::new(
                        ErrorCode::Stale,
                        "插件面板控件语义已变更；已撤回尚未发出的旧视图交互操作，请刷新或重试",
                    ),
                });
            }
        }
        self.finish_retracted_drift_requests(panel_id, plan.retracted_on_drift);
        if plan.has_claimed_invalidated {
            self.schedule_panel_semantic_resync(panel_id);
        }
    }

    fn retract_unclaimed_panel_requests_on_drift(&mut self, panel_id: u64) {
        let claims = self.outbound_claims.clone();
        let retracted_ids = {
            let mut barrier = claims.as_deref().map(super::OutboundWriteClaims::lock_barrier);
            self.retract_drift_candidates_under_barrier(panel_id, &BTreeSet::new(), barrier.as_mut())
        };
        self.finish_retracted_drift_requests(panel_id, retracted_ids);
    }

    pub(super) fn mark_panel_semantic_drift(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            self.main_resync_semantic_drift = true;
            self.main_resync_snapshot_confirmed = false;
            self.retract_unclaimed_panel_requests_on_drift(panel_id);
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && slot.state.is_some()
        {
            slot.resync_semantic_drift = true;
            slot.resync_snapshot_confirmed = false;
            self.retract_unclaimed_panel_requests_on_drift(panel_id);
        }
    }

    pub(super) fn schedule_panel_semantic_resync(&mut self, panel_id: u64) {
        self.mark_panel_semantic_drift(panel_id);
        self.schedule_panel_resync(panel_id);
    }

    pub(super) fn is_panel_semantic_drifted(&self, panel_id: u64) -> bool {
        if panel_id == PANEL_ID {
            self.main_resync_semantic_drift
        } else {
            self.panels
                .get(&panel_id)
                .is_some_and(|slot| slot.resync_semantic_drift)
        }
    }

    pub(super) fn clear_panel_semantic_drift(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            self.main_resync_semantic_drift = false;
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.resync_semantic_drift = false;
        }
    }

    pub(super) fn fail_deferred_interactive_events(&mut self, panel_id: u64, reason: &str) {
        self.drain_deferred_for_panel(panel_id, reason, |item| {
            matches!(
                &item.command,
                Control::Event { event, .. }
                    if !matches!(event.event, Some(panel::ui_event::Event::EditorChanged(_)))
            ) || matches!(
                &item.command,
                Control::Commit { .. } | Control::CommitResponse { .. }
            )
        });
    }

    pub(super) fn complete_panel_resync(&mut self, panel_id: u64, seq: u64) {
        if seq == 0 {
            return;
        }
        if panel_id == PANEL_ID {
            if self.main_resync_needed_seq == seq {
                self.main_resync_needed_seq = 0;
                self.main_resync_attempts = 0;
                self.main_resync_incompatible = false;
                self.main_resync_snapshot_confirmed = false;
            }
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && slot.resync_needed_seq == seq
        {
            slot.resync_needed_seq = 0;
            slot.resync_attempts = 0;
            slot.resync_incompatible = false;
            slot.resync_snapshot_confirmed = false;
        }
    }

    fn enqueue_pointer_cancel_entry(
        queue: &mut std::collections::VecDeque<PendingPointerCancel>,
        next_resync_seq: &mut u64,
        inflight_seq: Option<u64>,
        target: (u64, u64),
    ) {
        let already_queued = queue
            .iter()
            .any(|c| (c.node_id, c.hit_id) == target && inflight_seq != Some(c.seq));
        if already_queued {
            return;
        }
        if queue.len() >= 8
            && let Some(pos) = queue.iter().position(|c| inflight_seq != Some(c.seq))
        {
            queue.remove(pos);
        }
        *next_resync_seq = next_resync_seq.saturating_add(1).max(1);
        queue.push_back(PendingPointerCancel {
            seq: *next_resync_seq,
            node_id: target.0,
            hit_id: target.1,
            attempts: 0,
        });
    }

    pub(super) fn schedule_panel_pointer_cancel(&mut self, panel_id: u64, target: (u64, u64)) {
        let inflight_seq = self.inflight_pointer_cancel_seq(panel_id);
        if panel_id == PANEL_ID {
            Self::enqueue_pointer_cancel_entry(
                &mut self.main_pending_pointer_cancels,
                &mut self.next_resync_seq,
                inflight_seq,
                target,
            );
        } else if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && slot.state.is_some()
        {
            Self::enqueue_pointer_cancel_entry(
                &mut slot.pending_pointer_cancels,
                &mut self.next_resync_seq,
                inflight_seq,
                target,
            );
        }
    }

    fn remove_pending_pointer_cancel_seq(&mut self, panel_id: u64, seq: u64) -> bool {
        let queue = if panel_id == PANEL_ID {
            &mut self.main_pending_pointer_cancels
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            &mut slot.pending_pointer_cancels
        } else {
            return false;
        };
        let before = queue.len();
        queue.retain(|c| c.seq != seq);
        queue.len() != before
    }

    pub(super) fn complete_panel_pointer_cancel(&mut self, panel_id: u64, seq: u64) {
        if seq == 0 {
            return;
        }
        self.remove_pending_pointer_cancel_seq(panel_id, seq);
    }

    pub(super) fn abort_panel_pointer_cancel(&mut self, panel_id: u64, seq: u64) {
        if seq == 0 {
            return;
        }
        if self.remove_pending_pointer_cancel_seq(panel_id, seq) {
            self.fail_deferred_panel_commands(
                panel_id,
                "面板指针手势取消失败；已取消等待中的面板操作，请重试",
            );
        }
    }

    pub(super) fn settle_pointer_capture_on_failure(
        &mut self,
        panel_id: u64,
        transition: PointerCaptureTransition,
        retracted_unsent: bool,
    ) {
        match transition {
            PointerCaptureTransition::None => {}
            PointerCaptureTransition::Acquired { previous, target } => {
                if self.panel_capture(panel_id) == Some(target) {
                    self.clear_panel_capture(panel_id);
                }
                if let Some(prev) = previous {
                    if self.panel_capture(panel_id) == Some(prev) {
                        self.clear_panel_capture(panel_id);
                    }
                    if retracted_unsent || prev != target {
                        self.schedule_panel_pointer_cancel(panel_id, prev);
                    }
                }
                if !retracted_unsent {
                    self.schedule_panel_pointer_cancel(panel_id, target);
                }
            }
            PointerCaptureTransition::Released { previous } => {
                if self.panel_capture(panel_id) == Some(previous) {
                    self.clear_panel_capture(panel_id);
                }
                self.schedule_panel_pointer_cancel(panel_id, previous);
            }
        }
    }

    pub(super) fn fail_deferred_non_destroy_revokes(&mut self, panel_id: u64, reason: &str) {
        let mut remaining = std::collections::VecDeque::with_capacity(self.pending_revokes.len());
        let mut to_fail = Vec::new();
        while let Some(revoke) = self.pending_revokes.pop_front() {
            if revoke.panel_id == panel_id && !revoke.remove_panel {
                to_fail.push(revoke);
            } else {
                remaining.push_back(revoke);
            }
        }
        self.pending_revokes = remaining;
        for revoke in to_fail {
            for reply in revoke.replies {
                let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
            }
        }
    }

    pub(super) fn abort_panel_resync(&mut self, panel_id: u64, seq: u64) {
        let had_inflight_snapshot = self.take_panel_resync_snapshot_confirmed(panel_id);
        self.complete_panel_resync(panel_id, seq);
        self.clear_panel_resync_restart_eligibility(panel_id);
        if self.retract_panel_uncertain_optimistic_requests(panel_id) || had_inflight_snapshot {
            self.mark_panel_semantic_drift(panel_id);
        }
        self.fail_deferred_panel_commands(
            panel_id,
            "面板重同步未完成；已取消等待中的面板操作，请重试",
        );
        self.fail_deferred_non_destroy_revokes(
            panel_id,
            "面板重同步未完成；本地资源授权已撤销，未发送远端关闭通知",
        );
    }

    pub(super) fn mark_panel_resync_incompatible(&mut self, panel_id: u64, seq: u64) {
        self.abort_panel_resync(panel_id, seq);
        if panel_id == PANEL_ID {
            self.main_resync_incompatible = true;
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.resync_incompatible = true;
        }
    }

    pub(super) fn clear_panel_resync_incompatible(&mut self, panel_id: u64) {
        if panel_id == PANEL_ID {
            self.main_resync_incompatible = false;
        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.resync_incompatible = false;
        }
    }

    pub(super) fn has_inflight_resync(&self, panel_id: u64) -> bool {
        self.requests.values().any(|p| {
            p.panel_id == panel_id
                && matches!(
                    p.kind,
                    PendingKind::Event {
                        resync_seq: Some(_),
                        ..
                    }
                )
        })
    }

    pub(super) fn flush_pending_resyncs(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.flush_pending_resyncs_with(outgoing, mpsc::Sender::capacity)
    }

    pub(super) fn flush_pending_resyncs_with(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        mut sample_capacity: impl FnMut(&mpsc::Sender<Envelope>) -> usize,
    ) -> Result<()> {
        self.check_fatal_timeouts(Instant::now(), outgoing)?;
        self.drain_urgent_controls(outgoing)?;
        if outgoing.is_closed()
            && (self.main_resync_needed_seq != 0
                || !self.main_pending_pointer_cancels.is_empty()
                || self.panels.values().any(|s| {
                    (s.resync_needed_seq != 0 || !s.pending_pointer_cancels.is_empty())
                        && !s.closing
                        && s.state.is_some()
                }))
        {
            bail!("插件输入 pipe 已关闭");
        }
        if self.main_resync_needed_seq != 0
            && !self.has_inflight_resync(PANEL_ID)
            && !self.is_main_congested()
        {
            if self.main_resync_attempts >= MAX_RESYNC_RETRIES {
                let had_inflight_snapshot = self.take_panel_resync_snapshot_confirmed(PANEL_ID);
                self.main_resync_needed_seq = 0;
                self.main_resync_attempts = 0;
                self.clear_panel_resync_restart_eligibility(PANEL_ID);
                if self.retract_panel_uncertain_optimistic_requests(PANEL_ID)
                    || had_inflight_snapshot
                {
                    self.mark_panel_semantic_drift(PANEL_ID);
                }
                self.fail_deferred_panel_commands(
                    PANEL_ID,
                    "主面板重同步重试超过上限；已取消等待中的面板操作",
                );
                self.fail_deferred_non_destroy_revokes(
                    PANEL_ID,
                    "主面板重同步重试超过上限；本地资源授权已撤销，未发送远端关闭通知",
                );
                let _ = self.events.try_send(PluginEvent::Error {
                    identity: self.identity,
                    panel_id: PANEL_ID,
                    failure: Failure::new(
                        ErrorCode::Deadline,
                        "主面板重同步重试超过上限；已停止自动重同步",
                    ),
                });
                self.notify_panel_synced_nonblocking(PANEL_ID);
            } else {
                let seq = self.main_resync_needed_seq;
                let snapshot = self.panel.snapshot().clone();
                let resync_revision = snapshot.revision;
                let resync_snapshot = Some(Arc::new(snapshot.clone()));
                let preview = self.preview_envelope_for(
                    PANEL_ID,
                    envelope::Body::Resync(snapshot),
                    0,
                )?;
                if let Err(err) = preview.validate() {
                    let had_inflight_snapshot = self.take_panel_resync_snapshot_confirmed(PANEL_ID);
                    self.main_resync_needed_seq = 0;
                    self.main_resync_attempts = 0;
                    self.clear_panel_resync_restart_eligibility(PANEL_ID);
                    if self.retract_panel_uncertain_optimistic_requests(PANEL_ID)
                        || had_inflight_snapshot
                    {
                        self.mark_panel_semantic_drift(PANEL_ID);
                    }
                    self.fail_deferred_panel_commands(
                        PANEL_ID,
                        "主面板重同步失败；已取消等待中的面板操作",
                    );
                    self.fail_deferred_non_destroy_revokes(
                        PANEL_ID,
                        "主面板重同步失败；本地资源授权已撤销，未发送远端关闭通知",
                    );
                    let _ = self.events.try_send(PluginEvent::Error {
                        identity: self.identity,
                        panel_id: PANEL_ID,
                        failure: Failure::new(
                            ErrorCode::Invalid,
                            format!("主面板完整快照无法装帧重同步：{err}"),
                        ),
                    });
                    self.notify_panel_synced_nonblocking(PANEL_ID);
                } else {
                    self.outgoing_sequence = preview.request_id;
                    self.main_resync_attempts = self.main_resync_attempts.saturating_add(1);
                    self.enqueue_frame(
                        outgoing,
                        PANEL_ID,
                        preview,
                        PendingKind::Event {
                            editor_bases: vec![],
                            resync_seq: Some(seq),
                            resync_revision,
                            resync_snapshot,
                            mutated_host_panel: false,
                            pointer_capture: PointerCaptureTransition::None,
                            pointer_cancel_seq: None,
                            is_editor_changed: false,
                            is_optimistic: false,
                            ui_event_seq: 0,
                        },
                    )?;
                }
            }
        }
        let mut child_panels: Vec<_> = self
            .panels
            .iter()
            .filter_map(|(&pid, slot)| {
                (slot.resync_needed_seq != 0
                    && !slot.closing
                    && slot.state.is_some()
                    && !self.has_inflight_resync(pid))
                .then_some(pid)
            })
            .collect();
        child_panels.sort_unstable();
        for panel_id in child_panels {
            let (congested, low_headroom) = self.child_congestion_at(sample_capacity(outgoing));
            if congested {
                if low_headroom {
                    self.child_headroom_blocked = true;
                }
                break;
            }
            let Some((seq, attempts)) = self
                .panels
                .get(&panel_id)
                .map(|s| (s.resync_needed_seq, s.resync_attempts))
            else {
                continue;
            };
            if attempts >= MAX_RESYNC_RETRIES {
                let had_inflight_snapshot = self.take_panel_resync_snapshot_confirmed(panel_id);
                if let Some(slot) = self.panels.get_mut(&panel_id) {
                    slot.resync_needed_seq = 0;
                    slot.resync_attempts = 0;
                }
                self.clear_panel_resync_restart_eligibility(panel_id);
                if self.retract_panel_uncertain_optimistic_requests(panel_id)
                    || had_inflight_snapshot
                {
                    self.mark_panel_semantic_drift(panel_id);
                }
                self.fail_deferred_panel_commands(
                    panel_id,
                    "子面板重同步重试超过上限；已取消等待中的面板操作",
                );
                self.fail_deferred_non_destroy_revokes(
                    panel_id,
                    "子面板重同步重试超过上限；本地资源授权已撤销，未发送远端关闭通知",
                );
                let _ = self.events.try_send(PluginEvent::Error {
                    identity: self.identity,
                    panel_id,
                    failure: Failure::new(
                        ErrorCode::Deadline,
                        "子面板重同步重试超过上限；已停止自动重同步",
                    ),
                });
                self.notify_panel_synced_nonblocking(panel_id);
                continue;
            }
            if let Some(snapshot) = self.panel_snapshot(panel_id) {
                let resync_revision = snapshot.revision;
                let preview = self.preview_envelope_for(
                    panel_id,
                    envelope::Body::Resync((*snapshot).clone()),
                    0,
                )?;
                if let Err(err) = preview.validate() {
                    let had_inflight_snapshot = self.take_panel_resync_snapshot_confirmed(panel_id);
                    if let Some(slot) = self.panels.get_mut(&panel_id) {
                        slot.resync_needed_seq = 0;
                        slot.resync_attempts = 0;
                    }
                    self.clear_panel_resync_restart_eligibility(panel_id);
                    if self.retract_panel_uncertain_optimistic_requests(panel_id)
                        || had_inflight_snapshot
                    {
                        self.mark_panel_semantic_drift(panel_id);
                    }
                    self.fail_deferred_panel_commands(
                        panel_id,
                        "子面板重同步失败；已取消等待中的面板操作",
                    );
                    self.fail_deferred_non_destroy_revokes(
                        panel_id,
                        "子面板重同步失败；本地资源授权已撤销，未发送远端关闭通知",
                    );
                    let _ = self.events.try_send(PluginEvent::Error {
                        identity: self.identity,
                        panel_id,
                        failure: Failure::new(
                            ErrorCode::Invalid,
                            format!("子面板完整快照超过帧限额，无法发送重同步：{err}"),
                        ),
                    });
                    self.notify_panel_synced_nonblocking(panel_id);
                    continue;
                }
                self.outgoing_sequence = preview.request_id;
                if let Some(slot) = self.panels.get_mut(&panel_id) {
                    slot.resync_attempts = slot.resync_attempts.saturating_add(1);
                }
                self.enqueue_frame(
                    outgoing,
                    panel_id,
                    preview,
                    PendingKind::Event {
                        editor_bases: vec![],
                        resync_seq: Some(seq),
                        resync_revision,
                        resync_snapshot: Some(snapshot),
                        mutated_host_panel: false,
                        pointer_capture: PointerCaptureTransition::None,
                        pointer_cancel_seq: None,
                        is_editor_changed: false,
                        is_optimistic: false,
                        ui_event_seq: 0,
                    },
                )?;
            }
        }
        self.flush_pending_pointer_cancels_with(outgoing, &mut sample_capacity)?;
        Ok(())
    }

    fn flush_panel_pointer_cancels_with(
        &mut self,
        panel_id: u64,
        outgoing: &mpsc::Sender<Envelope>,
        sample_capacity: &mut impl FnMut(&mpsc::Sender<Envelope>) -> usize,
    ) -> Result<bool> {
        loop {
            if self.is_panel_resyncing(panel_id)
                || self.has_inflight_non_destroy_close(panel_id)
                || self.has_pending_non_destroy_revoke(panel_id)
                || self.has_inflight_pointer_cancel(panel_id)
            {
                return Ok(true);
            }
            let cancel = if panel_id == PANEL_ID {
                self.main_pending_pointer_cancels.front().copied()
            } else {
                self.panels
                    .get(&panel_id)
                    .filter(|slot| !slot.closing && slot.state.is_some())
                    .and_then(|slot| slot.pending_pointer_cancels.front().copied())
            };
            let Some(cancel) = cancel else {
                return Ok(true);
            };
            if cancel.attempts >= MAX_RESYNC_RETRIES {
                self.remove_pending_pointer_cancel_seq(panel_id, cancel.seq);
                let deferred_msg = if panel_id == PANEL_ID {
                    "主面板指针手势取消重试超过上限；已取消等待中的面板操作"
                } else {
                    "子面板指针手势取消重试超过上限；已取消等待中的面板操作"
                };
                let error_msg = if panel_id == PANEL_ID {
                    "主面板指针手势取消重试超过上限；已停止自动恢复"
                } else {
                    "子面板指针手势取消重试超过上限；已停止自动恢复"
                };
                self.fail_deferred_panel_commands(panel_id, deferred_msg);
                let _ = self.events.try_send(PluginEvent::Error {
                    identity: self.identity,
                    panel_id,
                    failure: Failure::new(ErrorCode::Deadline, error_msg),
                });
                continue;
            }
            if panel_id == PANEL_ID {
                if self.is_main_congested() {
                    return Ok(true);
                }
            } else {
                let (congested, low_headroom) = self.child_congestion_at(sample_capacity(outgoing));
                if congested {
                    if low_headroom {
                        self.child_headroom_blocked = true;
                    }
                    return Ok(false);
                }
            }
            let event = UiEvent {
                node_id: cancel.node_id,
                editors: vec![],
                event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                    hit_id: cancel.hit_id,
                    phase: panel::PointerPhase::Cancel as i32,
                    x: 0.0,
                    y: 0.0,
                })),
            };
            let preview =
                self.preview_envelope_for(panel_id, envelope::Body::UiEvent(event), 0)?;
            if let Err(err) = preview.validate() {
                self.remove_pending_pointer_cancel_seq(panel_id, cancel.seq);
                self.fail_deferred_panel_commands(
                    panel_id,
                    "面板指针手势取消无法装帧；已取消等待中的面板操作",
                );
                let _ = self.events.try_send(PluginEvent::Error {
                    identity: self.identity,
                    panel_id,
                    failure: Failure::new(
                        ErrorCode::Invalid,
                        format!("面板指针手势取消无法装帧：{err}"),
                    ),
                });
                continue;
            }
            self.outgoing_sequence = preview.request_id;
            let queue = if panel_id == PANEL_ID {
                Some(&mut self.main_pending_pointer_cancels)
            } else {
                self.panels
                    .get_mut(&panel_id)
                    .map(|slot| &mut slot.pending_pointer_cancels)
            };
            if let Some(entry) = queue.and_then(|q| q.front_mut().filter(|e| e.seq == cancel.seq)) {
                entry.attempts = entry.attempts.saturating_add(1);
            }
            self.enqueue_frame(
                outgoing,
                panel_id,
                preview,
                PendingKind::Event {
                    editor_bases: vec![],
                    resync_seq: None,
                    resync_revision: 0,
                    resync_snapshot: None,
                    mutated_host_panel: false,
                    pointer_capture: PointerCaptureTransition::None,
                    pointer_cancel_seq: Some(cancel.seq),
                    is_editor_changed: false,
                    is_optimistic: false,
                    ui_event_seq: 0,
                },
            )?;
            return Ok(true);
        }
    }

    fn flush_pending_pointer_cancels_with(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        sample_capacity: &mut impl FnMut(&mpsc::Sender<Envelope>) -> usize,
    ) -> Result<()> {
        self.flush_panel_pointer_cancels_with(PANEL_ID, outgoing, sample_capacity)?;
        let mut child_panels: Vec<_> = self
            .panels
            .iter()
            .filter_map(|(&pid, slot)| {
                (!slot.pending_pointer_cancels.is_empty() && !slot.closing && slot.state.is_some())
                    .then_some(pid)
            })
            .collect();
        child_panels.sort_unstable();
        for panel_id in child_panels {
            if !self.flush_panel_pointer_cancels_with(panel_id, outgoing, sample_capacity)? {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn flush_pending_revokes(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.check_fatal_timeouts(Instant::now(), outgoing)?;
        self.drain_urgent_controls(outgoing)?;
        let mut remaining = std::collections::VecDeque::with_capacity(self.pending_revokes.len());
        let mut blocked_panels = BTreeSet::new();
        while let Some(mut revoke) = self.pending_revokes.pop_front() {
            if outgoing.is_closed() {
                remaining.push_back(revoke);
                remaining.append(&mut self.pending_revokes);
                self.pending_revokes = remaining;
                bail!("插件输入 pipe 已关闭");
            }
            let front_panel = revoke.panel_id;
            let front_remove = revoke.remove_panel;
            if front_panel != PANEL_ID {
                let slot_state = self
                    .panels
                    .get(&front_panel)
                    .map(|s| (s.closing, s.destroy_failed));
                match slot_state {
                    None => {
                        for reply in revoke.replies {
                            let _ = reply.send(Ok(()));
                        }
                        continue;
                    }
                    Some((_, true)) => {
                        for reply in revoke.replies {
                            let _ = reply.send(Err(anyhow::anyhow!(
                                "多路复用子面板远端关闭失败，槽位保留至共享实例重启"
                            )));
                        }
                        continue;
                    }
                    Some((true, false)) if !front_remove => {
                        if let Some(PendingKind::Close { replies, .. }) =
                            self.requests.values_mut().find_map(|p| {
                                (p.panel_id == front_panel
                                    && matches!(
                                        p.kind,
                                        PendingKind::Close {
                                            remove_panel: true,
                                            ..
                                        }
                                    ))
                                .then_some(&mut p.kind)
                            })
                        {
                            replies.append(&mut revoke.replies);
                        } else if let Some(dest) = self
                            .pending_revokes
                            .iter_mut()
                            .chain(remaining.iter_mut())
                            .find(|d| d.panel_id == front_panel && d.remove_panel)
                        {
                            dest.replies.append(&mut revoke.replies);
                        } else {
                            for reply in revoke.replies {
                                let _ = reply.send(Err(anyhow::anyhow!(
                                    "多路复用子面板正在关闭"
                                )));
                            }
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if !front_remove
                && (self.is_panel_resyncing(front_panel)
                    || self.has_inflight_non_destroy_close(front_panel)
                    || blocked_panels.contains(&front_panel))
            {
                blocked_panels.insert(front_panel);
                remaining.push_back(revoke);
                continue;
            }
            if self.requests.len() >= QUEUE_CAPACITY
                || self.outbound_queue.len() >= OUTBOUND_CONTROL_SOFT_LIMIT
            {
                remaining.push_back(revoke);
                remaining.append(&mut self.pending_revokes);
                break;
            }
            let remove_panel = front_remove && front_panel != PANEL_ID;
            let env = match self.envelope_for(
                front_panel,
                envelope::Body::CloseRequest(protocol::CloseRequest {
                    resource_id: revoke.resource.as_bytes().to_vec(),
                    destroy_panel: remove_panel,
                }),
                0,
            ) {
                Ok(env) => env,
                Err(err) => {
                    remaining.push_back(revoke);
                    remaining.append(&mut self.pending_revokes);
                    self.pending_revokes = remaining;
                    return Err(err);
                }
            };
            if let Err(err) = env.validate().map_err(anyhow::Error::msg) {
                remaining.push_back(revoke);
                remaining.append(&mut self.pending_revokes);
                self.pending_revokes = remaining;
                return Err(err);
            }
            if let Err(err) = self.enqueue_frame(
                outgoing,
                front_panel,
                env,
                PendingKind::Close {
                    resource: revoke.resource,
                    remove_panel,
                    replies: revoke.replies,
                },
            ) {
                remaining.append(&mut self.pending_revokes);
                self.pending_revokes = remaining;
                return Err(err);
            }
        }
        self.pending_revokes = remaining;
        Ok(())
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
        tokio::select! { biased; _ = self.cancel.cancelled() => Ok(()), result = timeout(MAX_OPERATION, self.events.send(event)) => result.context("插件事件消费超时")?.context("插件事件接收方已关闭") }
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
        envelope.validate().map_err(anyhow::Error::msg)?;
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
        ensure!(
            self.requests.len() < limit,
            "插件待完成操作超过限额"
        );
        envelope.validate().map_err(anyhow::Error::msg)?;
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

    pub(super) fn editor_on_panel(
        &self,
        panel_id: u64,
        document_id: u64,
    ) -> Option<&panel::Editor> {
        if panel_id == PANEL_ID {
            self.panel.editor(document_id)
        } else {
            self.panels
                .get(&panel_id)
                .and_then(|slot| slot.state.as_ref())
                .and_then(|state| state.editor(document_id))
        }
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
    fn start_open(
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
            if !self.panels.contains_key(&panel_id) && self.panels.len() >= MAX_MULTIPLEXED_PANELS
            {
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
        if let Err(error) = envelope.validate() {
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
        if panel_id != PANEL_ID {
            self.panels.entry(panel_id).or_default();
        }
        self.set_opening_request_for(panel_id, Some(pending));
        Ok(())
    }

    pub(super) async fn flush_deferred_panel_commands(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.flush_deferred_panel_commands_with(outgoing, mpsc::Sender::capacity)
            .await
    }

    pub(super) async fn flush_deferred_panel_commands_with(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        mut sample_capacity: impl FnMut(&mpsc::Sender<Envelope>) -> usize,
    ) -> Result<()> {
        loop {
            self.check_fatal_timeouts(Instant::now(), outgoing)?;
            self.drain_urgent_controls(outgoing)?;
            if self.is_panel_command_barrier_active(PANEL_ID)
                || self.has_inflight_panel_state_request(PANEL_ID)
                || self.is_main_congested()
            {
                break;
            }
            let Some(item) = self.main_deferred_commands.pop_front() else {
                break;
            };
            self.dispatch_control(item.command, outgoing, true, item.seq)
                .await?;
        }
        let mut child_panels: Vec<_> = self
            .panels
            .iter()
            .filter_map(|(&pid, slot)| (!slot.deferred_commands.is_empty()).then_some(pid))
            .collect();
        child_panels.sort_unstable();
        for panel_id in child_panels {
            loop {
                self.check_fatal_timeouts(Instant::now(), outgoing)?;
                self.drain_urgent_controls(outgoing)?;
                if self.is_panel_command_barrier_active(panel_id)
                    || self.has_inflight_panel_state_request(panel_id)
                {
                    break;
                }
                let has_ready = self
                    .panels
                    .get(&panel_id)
                    .is_some_and(|slot| !slot.closing && !slot.deferred_commands.is_empty());
                if !has_ready {
                    break;
                }
                let (congested, low_headroom) = self.child_congestion_at(sample_capacity(outgoing));
                if congested {
                    if low_headroom {
                        self.child_headroom_blocked = true;
                    }
                    return Ok(());
                }
                let Some(item) = self
                    .panels
                    .get_mut(&panel_id)
                    .filter(|slot| !slot.closing)
                    .and_then(|slot| slot.deferred_commands.pop_front())
                else {
                    break;
                };
                self.dispatch_control(item.command, outgoing, true, item.seq)
                    .await?;
            }
        }
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) async fn control(
        &mut self,
        command: Control,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.control_with_seq(0, command, outgoing).await
    }

    pub(super) async fn control_with_seq(
        &mut self,
        cmd_seq: u64,
        mut command: Control,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        match command {
            Control::Revoke { .. }
            | Control::RevokeBatch { .. }
            | Control::CancelPanelOp { .. } => {
                self.apply_urgent_controls(
                    vec![SequencedControl {
                        seq: cmd_seq,
                        command,
                    }],
                    outgoing,
                )?;
                self.flush_outbound_nonblocking(outgoing)?;
                self.flush_pending_revokes(outgoing)?;
                self.flush_pending_resyncs(outgoing)?;
                self.flush_deferred_panel_commands(outgoing).await?;
                return Ok(());
            }
            _ => {}
        }
        self.drain_urgent_controls(outgoing)?;
        self.flush_outbound_nonblocking(outgoing)?;
        self.flush_pending_revokes(outgoing)?;
        self.flush_pending_resyncs(outgoing)?;
        self.flush_deferred_panel_commands(outgoing).await?;
        self.drain_urgent_controls(outgoing)?;
        if cmd_seq != 0
            && let Some(pid) = Self::control_panel_id(&command)
        {
            if pid != PANEL_ID
                && (self.panel_destroy_seq_floor >= cmd_seq
                    || self
                        .panel_destroy_control_seq
                        .get(&pid)
                        .is_some_and(|&rev_seq| cmd_seq <= rev_seq))
            {
                self.fail_control_command(command, "多路复用子面板正在关闭");
                return Ok(());
            }
            if let Some(rid) = Self::control_resource_id(&command)
                && (self.resource_revoke_seq_floor >= cmd_seq
                    || self
                        .resource_revoke_control_seq
                        .get(&(pid, rid))
                        .is_some_and(|&rev_seq| cmd_seq <= rev_seq))
            {
                self.fail_control_command(command, "资源已撤销；未提交或发送，保留现有编辑内容");
                return Ok(());
            }
            if let Control::Commit { op_id, .. } | Control::CommitResponse { op_id, .. } = &command
                && pid != PANEL_ID
                && (self.cancelled_op_seq_floor >= cmd_seq
                    || self
                        .cancelled_op_control_seq
                        .get(&(pid, *op_id))
                        .is_some_and(|&c_seq| cmd_seq <= c_seq))
            {
                self.fail_control_command(command, "插件子面板操作已取消");
                return Ok(());
            }
        }
        if let Control::Event {
            panel_id,
            event,
            expected_snapshot,
            event_seq,
        } = &mut command
            && expected_snapshot.is_none()
        {
            if cmd_seq != 0
                && !matches!(event.event, Some(panel::ui_event::Event::EditorChanged(_)))
            {
                if self
                    .panel_revoke_snapshot_floor
                    .get(panel_id)
                    .is_some_and(|&floor| cmd_seq <= floor)
                {
                    let panel_id = *panel_id;
                    let optimistic = Self::is_optimistic_ui_event(event);
                    self.record_processed_ui_seq(panel_id, *event_seq);
                    self.diagnostic_on_panel(
                        panel_id,
                        Failure::new(
                            ErrorCode::Stale,
                            "插件面板内容已更新；旧视图的交互操作未执行，请在当前视图重试",
                        ),
                    )
                    .await?;
                    if optimistic {
                        self.notify_panel_synced_with_rejected_nonblocking(
                            panel_id,
                            (*event_seq != 0).then_some(*event_seq),
                        );
                    }
                    return Ok(());
                }
                if let Some((_, pre_snap)) = self
                    .panel_revoke_snapshots
                    .get(panel_id)
                    .and_then(|q| q.iter().find(|(rev_seq, _)| cmd_seq < *rev_seq))
                {
                    *expected_snapshot = Some(pre_snap.clone());
                } else {
                    *expected_snapshot = self.panel_snapshot(*panel_id);
                }
            } else {
                *expected_snapshot = self.panel_snapshot(*panel_id);
            }
        }
        let stamp_pre_revoke_snapshot = match &mut command {
            Control::Commit {
                panel_id,
                expected_snapshot,
                ..
            }
            | Control::CommitResponse {
                panel_id,
                expected_snapshot,
                ..
            } if expected_snapshot.is_none() => Some((*panel_id, expected_snapshot)),
            _ => None,
        };
        if let Some((panel_id, expected_snapshot)) = stamp_pre_revoke_snapshot {
            if cmd_seq != 0
                && self
                    .panel_revoke_snapshot_floor
                    .get(&panel_id)
                    .is_some_and(|&floor| cmd_seq <= floor)
            {
                self.fail_control_command(
                    command,
                    "插件面板内容已更新；旧视图的编辑提交未执行，请在当前视图重试",
                );
                return Ok(());
            }
            if cmd_seq != 0
                && let Some((_, pre_snap)) = self
                    .panel_revoke_snapshots
                    .get(&panel_id)
                    .and_then(|q| q.iter().find(|(rev_seq, _)| cmd_seq < *rev_seq))
            {
                *expected_snapshot = Some(pre_snap.clone());
            } else {
                *expected_snapshot = self.panel_snapshot(panel_id);
            }
        }
        match &command {
            Control::Event { panel_id, .. }
            | Control::Open { panel_id, .. }
            | Control::OpenResponse { panel_id, .. }
            | Control::Commit { panel_id, .. }
            | Control::CommitResponse { panel_id, .. } => {
                let panel_id = *panel_id;
                let cancelled = matches!(
                    &command,
                    Control::Commit { cancelled, .. } | Control::CommitResponse { cancelled, .. }
                        if cancelled.is_cancelled()
                );
                let disallowed = panel_id != PANEL_ID && !self.supports_panel_multiplexing;
                let closing = panel_id != PANEL_ID
                    && self.panels.get(&panel_id).is_some_and(|slot| slot.closing);
                if !cancelled
                    && !outgoing.is_closed()
                    && !disallowed
                    && !closing
                    && self.should_defer_panel_command(panel_id)
                {
                    if let Err(rejected) = self.push_deferred_panel_command(
                        panel_id,
                        SequencedControl {
                            seq: cmd_seq,
                            command,
                        },
                    ) {
                        self.fail_control_command(
                            rejected.command,
                            "面板正在重同步或关闭资源且等待队列已满；请稍后重试",
                        );
                    }
                    return Ok(());
                }
            }
            Control::Hook { .. } => {}
            Control::Revoke { .. }
            | Control::RevokeBatch { .. }
            | Control::CancelPanelOp { .. } => unreachable!(),
        }
        self.dispatch_control(command, outgoing, false, cmd_seq)
            .await?;
        self.flush_pending_revokes(outgoing)?;
        self.flush_pending_resyncs(outgoing)?;
        self.flush_deferred_panel_commands(outgoing).await?;
        Ok(())
    }

    async fn dispatch_control(
        &mut self,
        command: Control,
        outgoing: &mpsc::Sender<Envelope>,
        from_deferred: bool,
        cmd_seq: u64,
    ) -> Result<()> {
        match command {
            Control::Event {
                panel_id,
                event,
                expected_snapshot,
                event_seq,
            } => {
                if outgoing.is_closed() {
                    bail!("插件输入 pipe 已关闭");
                }
                let optimistic = Self::is_optimistic_ui_event(&event);
                if panel_id != PANEL_ID && !self.supports_panel_multiplexing {
                    self.record_processed_ui_seq(panel_id, event_seq);
                    self.diagnostic_on_panel(
                        panel_id,
                        Failure::new(ErrorCode::Stale, "插件未启用多路复用面板"),
                    )
                    .await?;
                    if optimistic {
                        self.notify_panel_synced_with_rejected_nonblocking(
                            panel_id,
                            (event_seq != 0).then_some(event_seq),
                        );
                    }
                    return Ok(());
                }
                if panel_id != PANEL_ID && self.is_child_congested(outgoing) {
                    self.record_processed_ui_seq(panel_id, event_seq);
                    self.diagnostic_on_panel(
                        panel_id,
                        Failure::new(ErrorCode::Internal, "子面板事件队列繁忙；已忽略此次事件"),
                    )
                    .await?;
                    if optimistic {
                        self.notify_panel_synced_with_rejected_nonblocking(
                            panel_id,
                            (event_seq != 0).then_some(event_seq),
                        );
                    }
                    return Ok(());
                }
                if panel_id == PANEL_ID && self.is_main_congested() {
                    self.record_processed_ui_seq(panel_id, event_seq);
                    self.diagnostic_on_panel(
                        panel_id,
                        Failure::new(ErrorCode::Internal, "插件事件队列繁忙；已忽略此次事件"),
                    )
                    .await?;
                    if optimistic {
                        self.notify_panel_synced_with_rejected_nonblocking(
                            panel_id,
                            (event_seq != 0).then_some(event_seq),
                        );
                    }
                    return Ok(());
                }
                if panel_id != PANEL_ID
                    && self
                        .panels
                        .get(&panel_id)
                        .is_some_and(|slot| slot.closing)
                {
                    return self
                        .diagnostic_on_panel(
                            panel_id,
                            Failure::new(ErrorCode::Stale, "多路复用子面板已关闭"),
                        )
                        .await;
                }
                if !matches!(event.event, Some(panel::ui_event::Event::EditorChanged(_))) {
                    if self.is_panel_semantic_drifted(panel_id) {
                        self.record_processed_ui_seq(panel_id, event_seq);
                        self.diagnostic_on_panel(
                            panel_id,
                            Failure::new(
                                ErrorCode::Stale,
                                "插件面板尚未通过最新快照确认状态；旧视图的交互操作未执行，请刷新或重载面板",
                            ),
                        )
                        .await?;
                        if optimistic {
                            self.notify_panel_synced_with_rejected_nonblocking(
                                panel_id,
                                (event_seq != 0).then_some(event_seq),
                            );
                        }
                        return Ok(());
                    }
                    if let Some(expected) = expected_snapshot {
                        let current = self.panel_snapshot(panel_id);
                        let semantic_match = current.as_ref().is_some_and(|cur| {
                            cur.revision == expected.revision
                                || (cur.title == expected.title && cur.root == expected.root)
                        });
                        if !semantic_match {
                            self.record_processed_ui_seq(panel_id, event_seq);
                            self.diagnostic_on_panel(
                                panel_id,
                                Failure::new(
                                    ErrorCode::Stale,
                                    "插件面板内容已更新；旧视图的交互操作未执行，请在当前视图重试",
                                ),
                            )
                            .await?;
                            if optimistic {
                                self.notify_panel_synced_with_rejected_nonblocking(
                                    panel_id,
                                    (event_seq != 0).then_some(event_seq),
                                );
                            }
                            return Ok(());
                        }
                    }
                }
                let preview = self.preview_envelope_for(
                    panel_id,
                    envelope::Body::UiEvent(event.clone()),
                    0,
                )?;
                if let Err(error) = preview.validate() {
                    self.record_processed_ui_seq(panel_id, event_seq);
                    self.diagnostic_on_panel(
                        panel_id,
                        Failure::new(
                            ErrorCode::Invalid,
                            format!("UI 事件超过帧限额：{error}"),
                        ),
                    )
                    .await?;
                    if optimistic {
                        self.notify_panel_synced_with_rejected_nonblocking(
                            panel_id,
                            (event_seq != 0).then_some(event_seq),
                        );
                    }
                    return Ok(());
                }
                let preview_req_id = preview.request_id;
                let before_snapshot = self.panel_snapshot(panel_id);
                let before_capture = self.panel_capture(panel_id);
                let observed = if panel_id == PANEL_ID {
                    self.panel.observe_event_for_request(&event, preview_req_id)
                } else if let Some(slot) = self.panels.get_mut(&panel_id) {
                    if let Some(state) = slot.state.as_mut() {
                        state.observe_event_for_request(&event, preview_req_id)
                    } else {
                        Err("多路复用子面板尚未就绪或已关闭".into())
                    }
                } else {
                    Err("多路复用子面板尚未就绪或已关闭".into())
                };
                if let Err(error) = observed {
                    self.record_processed_ui_seq(panel_id, event_seq);
                    self.diagnostic_on_panel(panel_id, Failure::new(ErrorCode::Stale, error))
                        .await?;
                    if optimistic {
                        self.notify_panel_synced_with_rejected_nonblocking(
                            panel_id,
                            (event_seq != 0).then_some(event_seq),
                        );
                    }
                    return Ok(());
                }
                self.record_processed_ui_seq(panel_id, event_seq);
                let mutated_host_panel =
                    before_snapshot.as_deref() != self.panel_snapshot(panel_id).as_deref();
                let after_capture = self.panel_capture(panel_id);
                let pointer_capture = match (before_capture, after_capture) {
                    (previous, Some(target)) if previous != Some(target) => {
                        PointerCaptureTransition::Acquired { previous, target }
                    }
                    (Some(previous), None) => PointerCaptureTransition::Released { previous },
                    _ => PointerCaptureTransition::None,
                };
                let is_editor_changed =
                    matches!(event.event, Some(panel::ui_event::Event::EditorChanged(_)));
                let mut editor_bases = event.editors;
                if let Some(panel::ui_event::Event::EditorChanged(snapshot)) = &event.event {
                    editor_bases.push(snapshot.clone());
                }
                self.outgoing_sequence = preview.request_id;
                let req_id = match self.enqueue_frame(
                    outgoing,
                    panel_id,
                    preview,
                    PendingKind::Event {
                        editor_bases,
                        resync_seq: None,
                        resync_revision: 0,
                        resync_snapshot: None,
                        mutated_host_panel,
                        pointer_capture,
                        pointer_cancel_seq: None,
                        is_editor_changed,
                        is_optimistic: optimistic,
                        ui_event_seq: event_seq,
                    },
                ) {
                    Ok(req_id) => req_id,
                    Err(err) => {
                        self.retract_panel_optimistic_request(panel_id, preview_req_id);
                        return Err(err);
                    }
                };
                if let Some(p) = self.requests.get_mut(&req_id) {
                    if from_deferred {
                        p.from_deferred_queue = true;
                    }
                    if cmd_seq != 0 {
                        p.source_seq = cmd_seq;
                    }
                }
            }
            Control::Open {
                panel_id,
                id,
                revision,
                read_only,
                request,
                reply,
            } => {
                self.start_open(
                    outgoing,
                    panel_id,
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
                    from_deferred,
                    cmd_seq,
                )?;
            }
            Control::OpenResponse {
                panel_id,
                id,
                revision,
                read_only,
                response,
                reply,
            } => {
                self.start_open(
                    outgoing,
                    panel_id,
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
                    from_deferred,
                    cmd_seq,
                )?;
            }
            Control::Commit {
                panel_id,
                op_id,
                request,
                cancelled,
                expected_snapshot,
                reply,
            } => {
                if cancelled.is_cancelled() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件编辑提交已取消；未发送")));
                    return Ok(());
                }
                if outgoing.is_closed() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件输入 pipe 已关闭")));
                    bail!("插件输入 pipe 已关闭");
                }
                if panel_id != PANEL_ID && self.is_child_congested(outgoing) {
                    let _ = reply.send(Err(anyhow::anyhow!("插件子面板请求队列繁忙；请稍后重试")));
                    return Ok(());
                }
                if panel_id == PANEL_ID && self.is_main_congested() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件主面板请求队列繁忙；请稍后重试")));
                    return Ok(());
                }
                if self.is_panel_semantic_drifted(panel_id) {
                    let _ = reply.send(Err(anyhow::anyhow!(
                        "插件面板尚未通过最新快照确认状态；编辑提交未执行，请刷新或重载面板"
                    )));
                    return Ok(());
                }
                match self.commit_binding(panel_id, &request) {
                    Err(e) => _ = reply.send(Err(e)),
                    Ok((resource, document)) => {
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
                        let env =
                            self.envelope_for(panel_id, envelope::Body::CommitEditor(request), 0)?;
                        if let Err(e) = env.validate() {
                            let _ = reply.send(Err(anyhow::anyhow!(e)));
                        } else {
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
                            if cmd_seq != 0 && let Some(p) = self.requests.get_mut(&req_id) {
                                p.source_seq = cmd_seq;
                            }
                        }
                    }
                }
            }
            Control::CommitResponse {
                panel_id,
                op_id,
                request,
                cancelled,
                expected_snapshot,
                reply,
            } => {
                if cancelled.is_cancelled() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件响应编辑提交已取消")));
                    return Ok(());
                }
                if outgoing.is_closed() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件输入 pipe 已关闭")));
                    bail!("插件输入 pipe 已关闭");
                }
                if panel_id != PANEL_ID && self.is_child_congested(outgoing) {
                    let _ = reply.send(Err(anyhow::anyhow!("插件子面板请求队列繁忙；请稍后重试")));
                    return Ok(());
                }
                if panel_id == PANEL_ID && self.is_main_congested() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件主面板请求队列繁忙；请稍后重试")));
                    return Ok(());
                }
                if self.is_panel_semantic_drifted(panel_id) {
                    let _ = reply.send(Err(anyhow::anyhow!(
                        "插件面板尚未通过最新快照确认状态；响应编辑提交未执行，请刷新或重载面板"
                    )));
                    return Ok(());
                }
                match self.commit_response_binding(panel_id, &request) {
                    Err(e) => _ = reply.send(Err(e)),
                    Ok((resource, document)) => {
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
                        let env = self.envelope_for(
                            panel_id,
                            envelope::Body::CommitResponseEditor(request),
                            0,
                        )?;
                        if let Err(e) = env.validate() {
                            let _ = reply.send(Err(anyhow::anyhow!(e)));
                        } else {
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
                            if cmd_seq != 0 && let Some(p) = self.requests.get_mut(&req_id) {
                                p.source_seq = cmd_seq;
                            }
                        }
                    }
                }
            }
            Control::Revoke { .. }
            | Control::RevokeBatch { .. }
            | Control::CancelPanelOp { .. } => {
                self.apply_urgent_controls(
                    vec![SequencedControl {
                        seq: cmd_seq,
                        command,
                    }],
                    outgoing,
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
                if outgoing.is_closed() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件输入 pipe 已关闭")));
                    bail!("插件输入 pipe 已关闭");
                }
                if self.is_main_congested() {
                    let _ = reply.send(Err(anyhow::anyhow!("插件请求队列繁忙；请稍后重试")));
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
                let env = self.envelope_for(PANEL_ID, body, 0)?;
                if let Err(err) = env.validate() {
                    let _ = reply.send(Err(anyhow::anyhow!(err)));
                    return Ok(());
                }
                self.enqueue_frame(
                    outgoing,
                    PANEL_ID,
                    env,
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
