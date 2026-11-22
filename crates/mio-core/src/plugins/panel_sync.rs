use super::*;

pub(super) struct RetractedOptimisticPlan {
    pub(super) retracted_invalidated: Vec<u64>,
    pub(super) has_claimed_invalidated: bool,
    pub(super) retracted_on_drift: Vec<u64>,
}

impl Running {
    #[cfg(test)]
    pub(super) fn main_slot(&self) -> &PanelSlot {
        self.panels
            .get(&PANEL_ID)
            .expect("main panel slot must always exist")
    }

    #[cfg(test)]
    pub(super) fn main_slot_mut(&mut self) -> &mut PanelSlot {
        self.panels
            .get_mut(&PANEL_ID)
            .expect("main panel slot must always exist")
    }

    #[cfg(test)]
    pub(super) fn main_panel(&self) -> &PanelState {
        self.main_slot()
            .state
            .as_ref()
            .expect("main panel state must always exist")
    }

    #[cfg(test)]
    pub(super) fn main_panel_mut(&mut self) -> &mut PanelState {
        self.main_slot_mut()
            .state
            .as_mut()
            .expect("main panel state must always exist")
    }

    pub(super) fn active_panel_state(&self, panel_id: u64) -> Option<&PanelState> {
        self.panels
            .get(&panel_id)
            .filter(|slot| !slot.closing)
            .and_then(|slot| slot.state.as_ref())
    }

    pub(super) fn active_panel_state_mut(&mut self, panel_id: u64) -> Option<&mut PanelState> {
        self.panels
            .get_mut(&panel_id)
            .filter(|slot| !slot.closing)
            .and_then(|slot| slot.state.as_mut())
    }

    pub(super) fn panel_revision(&self, panel_id: u64) -> u64 {
        self.panels
            .get(&panel_id)
            .and_then(|slot| slot.state.as_ref())
            .map_or(0, |state| state.snapshot().revision)
    }

    pub(super) fn panel_last_snapshot_revision(&self, panel_id: u64) -> u64 {
        self.panels
            .get(&panel_id)
            .map_or(0, |slot| slot.last_snapshot_rev)
    }

    pub(super) fn record_panel_accepted_snapshot_revision(&mut self, panel_id: u64, revision: u64) {
        if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            slot.last_snapshot_rev = slot.last_snapshot_rev.max(revision);
        }
    }

    pub(super) fn panel_snapshot(&self, panel_id: u64) -> Option<Arc<PanelSnapshot>> {
        self.panels
            .get(&panel_id)
            .and_then(|slot| slot.state.as_ref())
            .map(|state| Arc::new(state.snapshot().clone()))
    }

    pub(super) fn panel_capture(&self, panel_id: u64) -> Option<(u64, u64)> {
        self.panels
            .get(&panel_id)
            .and_then(|slot| slot.state.as_ref())
            .and_then(PanelState::captured)
    }

    pub(super) fn clear_panel_capture(&mut self, panel_id: u64) {
        if let Some(state) = self
            .panels
            .get_mut(&panel_id)
            .and_then(|slot| slot.state.as_mut())
        {
            state.clear_capture();
        }
    }

    pub(super) fn active_resource_for(&self, panel_id: u64) -> Option<(Uuid, u64)> {
        self.panels
            .get(&panel_id)
            .and_then(|slot| slot.active_resource)
    }

    pub(super) fn set_active_resource_for(
        &mut self,
        panel_id: u64,
        value: Option<(Uuid, u64)>,
    ) {
        if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.active_resource = value;
        }
    }

    pub(super) fn opening_request_for(&self, panel_id: u64) -> Option<u64> {
        self.panels
            .get(&panel_id)
            .and_then(|slot| slot.opening_request)
    }

    pub(super) fn set_opening_request_for(&mut self, panel_id: u64, value: Option<u64>) {
        if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.opening_request = value;
        }
    }

    pub(super) fn processed_ui_seq(&self, panel_id: u64) -> u64 {
        self.panels
            .get(&panel_id)
            .map_or(0, |slot| slot.processed_ui_seq)
    }

    pub(super) fn record_processed_ui_seq(&mut self, panel_id: u64, seq: u64) {
        if seq == 0 {
            return;
        }
        if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            slot.processed_ui_seq = slot.processed_ui_seq.max(seq);
        }
    }

    fn min_unresolved_ui_seq(&self, panel_id: u64) -> Option<u64> {
        let queue = self
            .panels
            .get(&panel_id)
            .map(|slot| &slot.deferred_commands);
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
        let state = self.panels.get(&panel_id)?.state.as_ref()?;
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
        if self.panels.get(&panel_id).is_some_and(|s| s.closing) {
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
        if let Some(queue) = self
            .panels
            .get_mut(&panel_id)
            .map(|slot| &mut slot.cancelled_state_requests)
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
        let queue = &mut self.panels.get_mut(&panel_id)?.cancelled_state_requests;
        let pos = queue.iter().position(|&(id, _)| id == request_id)?;
        queue.remove(pos).map(|(_, entry)| entry)
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
        if let Some(queue) = self
            .panels
            .get_mut(&panel_id)
            .map(|slot| &mut slot.cancelled_resync_requests)
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
        let queue = &mut self.panels.get_mut(&panel_id)?.cancelled_resync_requests;
        let pos = queue.iter().position(|e| e.request_id == request_id)?;
        queue.remove(pos)
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

    pub(super) fn panel_has_uncertain_optimistic(&self, panel_id: u64) -> bool {
        self.active_panel_state(panel_id)
            .is_some_and(PanelState::has_uncertain_optimistic_requests)
    }

    pub(super) fn panel_patch_confirms_detached_viewport(
        &self,
        panel_id: u64,
        patch: &panel::PanelPatch,
        request_id: u64,
    ) -> bool {
        self.active_panel_state(panel_id)
            .is_some_and(|state| state.patch_confirms_detached_viewport(patch, request_id))
    }

    pub(super) fn retract_panel_optimistic_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        self.active_panel_state_mut(panel_id)
            .is_some_and(|state| state.retract_optimistic_request(request_id))
    }

    pub(super) fn mark_panel_optimistic_request_uncertain(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        self.active_panel_state_mut(panel_id)
            .is_some_and(|state| state.mark_optimistic_request_uncertain(request_id))
    }

    pub(super) fn retract_panel_uncertain_optimistic_requests(&mut self, panel_id: u64) -> bool {
        self.active_panel_state_mut(panel_id)
            .is_some_and(PanelState::retract_uncertain_optimistic_requests)
    }

    pub(super) fn settle_panel_optimistic_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        self.active_panel_state_mut(panel_id)
            .is_some_and(|state| state.settle_optimistic_request(request_id))
    }

    pub(super) fn settle_cancelled_panel_optimistic_request(
        &mut self,
        panel_id: u64,
        request_id: u64,
    ) -> bool {
        self.active_panel_state_mut(panel_id)
            .is_some_and(|state| state.settle_cancelled_optimistic_request(request_id))
    }

    pub(super) fn clear_panel_resync_restart_eligibility(&mut self, panel_id: u64) {
        if let Some(slot) = self.panels.get_mut(&panel_id) {
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
        if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
        {
            slot.resync_snapshot_confirmed = true;
        }
    }

    pub(super) fn take_panel_resync_snapshot_confirmed(&mut self, panel_id: u64) -> bool {
        self.panels
            .get_mut(&panel_id)
            .filter(|slot| !slot.closing)
            .is_some_and(|slot| std::mem::take(&mut slot.resync_snapshot_confirmed))
    }

    pub(super) fn settle_panel_uncertain_optimistic_requests_through(
        &mut self,
        panel_id: u64,
        max_request_id: u64,
    ) -> bool {
        let Some(slot) = self.panels.get_mut(&panel_id).filter(|slot| !slot.closing) else {
            return false;
        };
        let Some(state) = slot.state.as_mut() else {
            return false;
        };
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
        let Some(slot) = self.panels.get_mut(&panel_id).filter(|slot| !slot.closing) else {
            return;
        };
        let Some(state) = slot.state.as_ref() else {
            return;
        };
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
        let Some(slot) = self.panels.get_mut(&panel_id).filter(|slot| !slot.closing) else {
            return;
        };
        let Some(state) = slot.state.as_ref() else {
            return;
        };
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
        barrier: Option<&OutboundWriteClaimsGuard<'_>>,
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

    pub(super) fn retract_drift_candidates_under_barrier(
        &mut self,
        panel_id: u64,
        exclude_ids: &BTreeSet<u64>,
        mut barrier: Option<&mut OutboundWriteClaimsGuard<'_>>,
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
        mut barrier: Option<&mut OutboundWriteClaimsGuard<'_>>,
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
            let mut barrier = claims.as_deref().map(OutboundWriteClaims::lock_barrier);
            self.retract_drift_candidates_under_barrier(panel_id, &BTreeSet::new(), barrier.as_mut())
        };
        self.finish_retracted_drift_requests(panel_id, retracted_ids);
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

    pub(super) fn is_panel_resyncing(&self, panel_id: u64) -> bool {
        self.has_inflight_resync(panel_id)
            || self
                .panels
                .get(&panel_id)
                .is_some_and(|slot| slot.resync_needed_seq != 0)
    }

    pub(super) fn schedule_panel_resync(&mut self, panel_id: u64) {
        let started = if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
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

    pub(super) fn mark_panel_semantic_drift(&mut self, panel_id: u64) {
        let should_retract = if let Some(slot) = self.panels.get_mut(&panel_id)
            && !slot.closing
            && slot.state.is_some()
        {
            slot.resync_semantic_drift = true;
            slot.resync_snapshot_confirmed = false;
            true
        } else {
            false
        };
        if should_retract {
            self.retract_unclaimed_panel_requests_on_drift(panel_id);
        }
    }

    pub(super) fn schedule_panel_semantic_resync(&mut self, panel_id: u64) {
        self.mark_panel_semantic_drift(panel_id);
        self.schedule_panel_resync(panel_id);
    }

    pub(super) fn is_panel_semantic_drifted(&self, panel_id: u64) -> bool {
        self.panels
            .get(&panel_id)
            .is_some_and(|slot| slot.resync_semantic_drift)
    }

    pub(super) fn clear_panel_semantic_drift(&mut self, panel_id: u64) {
        if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.resync_semantic_drift = false;
        }
    }

    pub(super) fn complete_panel_resync(&mut self, panel_id: u64, seq: u64) {
        if seq == 0 {
            return;
        }
        if let Some(slot) = self.panels.get_mut(&panel_id)
            && slot.resync_needed_seq == seq
        {
            slot.resync_needed_seq = 0;
            slot.resync_attempts = 0;
            slot.resync_snapshot_confirmed = false;
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

    fn abandon_panel_resync(
        &mut self,
        panel_id: u64,
        deferred_reason: &str,
        revoke_reason: &str,
        failure: Failure,
    ) {
        let had_inflight_snapshot = self.take_panel_resync_snapshot_confirmed(panel_id);
        if let Some(slot) = self.panels.get_mut(&panel_id) {
            slot.resync_needed_seq = 0;
            slot.resync_attempts = 0;
        }
        self.clear_panel_resync_restart_eligibility(panel_id);
        if self.retract_panel_uncertain_optimistic_requests(panel_id) || had_inflight_snapshot {
            self.mark_panel_semantic_drift(panel_id);
        }
        self.fail_deferred_panel_commands(panel_id, deferred_reason);
        self.fail_deferred_non_destroy_revokes(panel_id, revoke_reason);
        let _ = self.events.try_send(PluginEvent::Error {
            identity: self.identity,
            panel_id,
            failure,
        });
        self.notify_panel_synced_nonblocking(panel_id);
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
            && self.panels.values().any(|s| {
                (s.resync_needed_seq != 0 || !s.pending_pointer_cancels.is_empty())
                    && !s.closing
                    && s.state.is_some()
            })
        {
            bail!("插件输入 pipe 已关闭");
        }
        let mut ready_panels: Vec<_> = self
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
        ready_panels.sort_unstable();
        for panel_id in ready_panels {
            if panel_id == PANEL_ID {
                if self.is_main_congested() {
                    continue;
                }
            } else {
                let (congested, low_headroom) = self.child_congestion_at(sample_capacity(outgoing));
                if congested {
                    if low_headroom {
                        self.child_headroom_blocked = true;
                    }
                    break;
                }
            }
            let Some((seq, attempts)) = self
                .panels
                .get(&panel_id)
                .map(|s| (s.resync_needed_seq, s.resync_attempts))
            else {
                continue;
            };
            let is_main = panel_id == PANEL_ID;
            if attempts >= MAX_RESYNC_RETRIES {
                self.abandon_panel_resync(
                    panel_id,
                    if is_main {
                        "主面板重同步重试超过上限；已取消等待中的面板操作"
                    } else {
                        "子面板重同步重试超过上限；已取消等待中的面板操作"
                    },
                    if is_main {
                        "主面板重同步重试超过上限；本地资源授权已撤销，未发送远端关闭通知"
                    } else {
                        "子面板重同步重试超过上限；本地资源授权已撤销，未发送远端关闭通知"
                    },
                    Failure::new(
                        ErrorCode::Deadline,
                        if is_main {
                            "主面板重同步重试超过上限；已停止自动重同步"
                        } else {
                            "子面板重同步重试超过上限；已停止自动重同步"
                        },
                    ),
                );
                continue;
            }
            let Some(snapshot) = self.panel_snapshot(panel_id) else {
                continue;
            };
            let resync_revision = snapshot.revision;
            let preview = self.preview_envelope_for(
                panel_id,
                envelope::Body::Resync((*snapshot).clone()),
                0,
            )?;
            if let Err(err) = preview.validate_for_minor(self.protocol_minor) {
                self.abandon_panel_resync(
                    panel_id,
                    if is_main {
                        "主面板重同步失败；已取消等待中的面板操作"
                    } else {
                        "子面板重同步失败；已取消等待中的面板操作"
                    },
                    if is_main {
                        "主面板重同步失败；本地资源授权已撤销，未发送远端关闭通知"
                    } else {
                        "子面板重同步失败；本地资源授权已撤销，未发送远端关闭通知"
                    },
                    Failure::new(
                        ErrorCode::Invalid,
                        if is_main {
                            format!("主面板完整快照无法装帧重同步：{err}")
                        } else {
                            format!("子面板完整快照超过帧限额，无法发送重同步：{err}")
                        },
                    ),
                );
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
        self.flush_pending_pointer_cancels_with(outgoing, &mut sample_capacity)?;
        Ok(())
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
        self.has_inflight_pointer_cancel(panel_id)
            || self
                .panels
                .get(&panel_id)
                .is_some_and(|slot| !slot.pending_pointer_cancels.is_empty())
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
        if let Some(slot) = self.panels.get_mut(&panel_id)
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
        let Some(slot) = self.panels.get_mut(&panel_id) else {
            return false;
        };
        let before = slot.pending_pointer_cancels.len();
        slot.pending_pointer_cancels.retain(|c| c.seq != seq);
        slot.pending_pointer_cancels.len() != before
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
            let Some(cancel) = self
                .panels
                .get(&panel_id)
                .filter(|slot| !slot.closing && slot.state.is_some())
                .and_then(|slot| slot.pending_pointer_cancels.front().copied())
            else {
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
            if let Err(err) = preview.validate_for_minor(self.protocol_minor) {
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
            if let Some(entry) = self
                .panels
                .get_mut(&panel_id)
                .and_then(|slot| slot.pending_pointer_cancels.front_mut())
                .filter(|e| e.seq == cancel.seq)
            {
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
        let mut ready_panels: Vec<_> = self
            .panels
            .iter()
            .filter_map(|(&pid, slot)| {
                (!slot.pending_pointer_cancels.is_empty() && !slot.closing && slot.state.is_some())
                    .then_some(pid)
            })
            .collect();
        ready_panels.sort_unstable();
        for panel_id in ready_panels {
            if !self.flush_panel_pointer_cancels_with(panel_id, outgoing, sample_capacity)? {
                break;
            }
        }
        Ok(())
    }
}
