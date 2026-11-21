use super::*;

type CancelledPendingMeta = (u64, u64, bool, Option<u64>);

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

    pub(super) fn cancel_pending_request_inner(
        &mut self,
        request_id: u64,
        reason: &str,
        send_cancel: bool,
        expect_late_reply: bool,
    ) -> (bool, bool, Option<CancelledPendingMeta>) {
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
        if let Some(notification) = self
            .cancel_notifications
            .iter()
            .rev()
            .find(|n| n.panel_id == panel_id && n.cancel_frame_id == call.parent_request_id)
        {
            let active = notification.deadline > now;
            return (active, active && notification.can_restart_resync);
        }
        (false, false)
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
        self.panels
            .get(&panel_id)
            .is_some_and(|slot| !slot.deferred_commands.is_empty())
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
    ) -> std::result::Result<(), Box<SequencedControl>> {
        let Some(slot) = self.panels.get_mut(&panel_id).filter(|slot| !slot.closing) else {
            return Err(Box::new(command));
        };
        if slot.deferred_commands.len() >= MAX_DEFERRED_PANEL_COMMANDS {
            return Err(Box::new(command));
        }
        slot.deferred_commands.push_back(command);
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
        let drained = self
            .panels
            .get_mut(&panel_id)
            .map(|slot| std::mem::take(&mut slot.deferred_commands))
            .unwrap_or_default();
        for cmd in drained {
            self.fail_control_command(cmd.command, reason);
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
        let Some(slot) = self.panels.get_mut(&panel_id) else {
            return;
        };
        let to_fail =
            Self::drain_matching_sequenced_controls(&mut slot.deferred_commands, predicate);
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
            let has_newer_deferred = self.panels.get(&panel_id).is_some_and(|slot| {
                slot.deferred_commands.iter().any(|item| {
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
        let panel_live = self
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
            self.panel_revoked_resources.insert((panel_id, id), barrier);
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
            let has_newer_deferred = self.panels.get(&panel_id).is_some_and(|slot| {
                slot.deferred_commands.iter().any(|item| {
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
        let mut ready_panels: Vec<_> = self
            .panels
            .iter()
            .filter_map(|(&pid, slot)| (!slot.deferred_commands.is_empty()).then_some(pid))
            .collect();
        ready_panels.sort_unstable();
        for panel_id in ready_panels {
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
                if panel_id == PANEL_ID {
                    if self.is_main_congested() {
                        break;
                    }
                } else {
                    let (congested, low_headroom) =
                        self.child_congestion_at(sample_capacity(outgoing));
                    if congested {
                        if low_headroom {
                            self.child_headroom_blocked = true;
                        }
                        return Ok(());
                    }
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
        if !self.stamp_expected_snapshot(cmd_seq, &mut command).await? {
            return Ok(());
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
                let closing = self.panels.get(&panel_id).is_some_and(|slot| slot.closing);
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

    async fn stamp_expected_snapshot(
        &mut self,
        cmd_seq: u64,
        command: &mut Control,
    ) -> Result<bool> {
        if let Control::Event {
            panel_id,
            event,
            expected_snapshot,
            event_seq,
        } = command
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
                    let event_seq = *event_seq;
                    self.reject_ui_event(
                        panel_id,
                        event_seq,
                        optimistic,
                        Failure::new(
                            ErrorCode::Stale,
                            "插件面板内容已更新；旧视图的交互操作未执行，请在当前视图重试",
                        ),
                    )
                    .await?;
                    return Ok(false);
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
        let stamp_commit = match command {
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
        if let Some((panel_id, expected_snapshot)) = stamp_commit {
            if cmd_seq != 0
                && self
                    .panel_revoke_snapshot_floor
                    .get(&panel_id)
                    .is_some_and(|&floor| cmd_seq <= floor)
            {
                let placeholder = Control::CancelPanelOp {
                    panel_id,
                    op_id: Uuid::nil(),
                };
                let rejected = std::mem::replace(command, placeholder);
                self.fail_control_command(
                    rejected,
                    "插件面板内容已更新；旧视图的编辑提交未执行，请在当前视图重试",
                );
                return Ok(false);
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
        Ok(true)
    }

    async fn reject_ui_event(
        &mut self,
        panel_id: u64,
        event_seq: u64,
        optimistic: bool,
        failure: Failure,
    ) -> Result<()> {
        self.record_processed_ui_seq(panel_id, event_seq);
        self.diagnostic_on_panel(panel_id, failure).await?;
        if optimistic {
            self.notify_panel_synced_with_rejected_nonblocking(
                panel_id,
                (event_seq != 0).then_some(event_seq),
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_ui_event(
        &mut self,
        panel_id: u64,
        event: UiEvent,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        event_seq: u64,
        outgoing: &mpsc::Sender<Envelope>,
        from_deferred: bool,
        cmd_seq: u64,
    ) -> Result<()> {
        if outgoing.is_closed() {
            bail!("插件输入 pipe 已关闭");
        }
        let optimistic = Self::is_optimistic_ui_event(&event);
        if panel_id != PANEL_ID && !self.supports_panel_multiplexing {
            return self
                .reject_ui_event(
                    panel_id,
                    event_seq,
                    optimistic,
                    Failure::new(ErrorCode::Stale, "插件未启用多路复用面板"),
                )
                .await;
        }
        if panel_id != PANEL_ID && self.is_child_congested(outgoing) {
            return self
                .reject_ui_event(
                    panel_id,
                    event_seq,
                    optimistic,
                    Failure::new(ErrorCode::Internal, "子面板事件队列繁忙；已忽略此次事件"),
                )
                .await;
        }
        if panel_id == PANEL_ID && self.is_main_congested() {
            return self
                .reject_ui_event(
                    panel_id,
                    event_seq,
                    optimistic,
                    Failure::new(ErrorCode::Internal, "插件事件队列繁忙；已忽略此次事件"),
                )
                .await;
        }
        if self.panels.get(&panel_id).is_some_and(|slot| slot.closing) {
            return self
                .diagnostic_on_panel(
                    panel_id,
                    Failure::new(ErrorCode::Stale, "多路复用子面板已关闭"),
                )
                .await;
        }
        if !matches!(event.event, Some(panel::ui_event::Event::EditorChanged(_))) {
            if self.is_panel_semantic_drifted(panel_id) {
                return self
                    .reject_ui_event(
                        panel_id,
                        event_seq,
                        optimistic,
                        Failure::new(
                            ErrorCode::Stale,
                            "插件面板尚未通过最新快照确认状态；旧视图的交互操作未执行，请刷新或重载面板",
                        ),
                    )
                    .await;
            }
            if let Some(expected) = expected_snapshot {
                let current = self.panel_snapshot(panel_id);
                let semantic_match = current.as_ref().is_some_and(|cur| {
                    cur.revision == expected.revision
                        || (cur.title == expected.title && cur.root == expected.root)
                });
                if !semantic_match {
                    return self
                        .reject_ui_event(
                            panel_id,
                            event_seq,
                            optimistic,
                            Failure::new(
                                ErrorCode::Stale,
                                "插件面板内容已更新；旧视图的交互操作未执行，请在当前视图重试",
                            ),
                        )
                        .await;
                }
            }
        }
        let preview =
            self.preview_envelope_for(panel_id, envelope::Body::UiEvent(event.clone()), 0)?;
        if let Err(error) = preview.validate() {
            return self
                .reject_ui_event(
                    panel_id,
                    event_seq,
                    optimistic,
                    Failure::new(ErrorCode::Invalid, format!("UI 事件超过帧限额：{error}")),
                )
                .await;
        }
        let preview_req_id = preview.request_id;
        let before_snapshot = self.panel_snapshot(panel_id);
        let before_capture = self.panel_capture(panel_id);
        let observed = if let Some(state) = self
            .panels
            .get_mut(&panel_id)
            .and_then(|slot| slot.state.as_mut())
        {
            state.observe_event_for_request(&event, preview_req_id)
        } else {
            Err("多路复用子面板尚未就绪或已关闭".into())
        };
        if let Err(error) = observed {
            return self
                .reject_ui_event(
                    panel_id,
                    event_seq,
                    optimistic,
                    Failure::new(ErrorCode::Stale, error),
                )
                .await;
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
        Ok(())
    }

    fn dispatch_hook(
        &mut self,
        permission: Permission,
        body: envelope::Body,
        cancelled: CancellationToken,
        reply: oneshot::Sender<Result<HookReply>>,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
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
                self.dispatch_ui_event(
                    panel_id,
                    event,
                    expected_snapshot,
                    event_seq,
                    outgoing,
                    from_deferred,
                    cmd_seq,
                )
                .await?;
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
                self.dispatch_commit_request(
                    panel_id,
                    op_id,
                    request,
                    cancelled,
                    expected_snapshot,
                    reply,
                    outgoing,
                    cmd_seq,
                )?;
            }
            Control::CommitResponse {
                panel_id,
                op_id,
                request,
                cancelled,
                expected_snapshot,
                reply,
            } => {
                self.dispatch_commit_response(
                    panel_id,
                    op_id,
                    request,
                    cancelled,
                    expected_snapshot,
                    reply,
                    outgoing,
                    cmd_seq,
                )?;
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
                self.dispatch_hook(permission, body, cancelled, reply, outgoing)?;
            }
        }
        Ok(())
    }
}
