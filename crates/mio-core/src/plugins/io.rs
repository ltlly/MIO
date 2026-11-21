use super::*;
use super::running::RetractedOptimisticPlan;

impl Running {
    fn is_superseded_resync_reply(&self, panel_id: u64, in_reply_to: u64) -> bool {
        in_reply_to != 0
            && self.requests.get(&in_reply_to).is_some_and(|p| {
                p.panel_id == panel_id
                    && matches!(
                        &p.kind,
                        PendingKind::Event {
                            resync_seq: Some(_),
                            resync_revision,
                            resync_snapshot,
                            ..
                        } if *resync_revision != self.panel_revision(panel_id)
                            || resync_snapshot.as_deref()
                                != self.panel_snapshot(panel_id).as_deref()
                    )
            })
    }

    fn expired_reply_drift_flags(
        &self,
        panel_id: u64,
        envelope: &Envelope,
        check_pointer_cancel: bool,
    ) -> (u64, bool, bool) {
        let host_rev = self.panel_revision(panel_id);
        let superseded_by_snapshot_rev = (envelope.in_reply_to != 0)
            .then(|| {
                self.requests
                    .get(&envelope.in_reply_to)
                    .filter(|p| p.panel_id == panel_id)
                    .map(|p| p.panel_revision_at_send)
            })
            .flatten()
            .and_then(|base_rev| {
                let snap_rev = self.panel_last_snapshot_revision(panel_id);
                (snap_rev > base_rev).then_some(snap_rev)
            });
        let frame_revision = match &envelope.body {
            Some(envelope::Body::Snapshot(snap)) => envelope.revision.max(snap.revision),
            Some(envelope::Body::Patch(patch)) => {
                envelope.revision.max(patch.revision).max(patch.base_revision)
            }
            _ => envelope.revision,
        };
        let is_tree_drift = matches!(
            envelope.body,
            Some(envelope::Body::Snapshot(_) | envelope::Body::Patch(_))
        ) && superseded_by_snapshot_rev
            .is_none_or(|snap_rev| frame_revision >= snap_rev);
        let pointer_cancel_rev_drift = check_pointer_cancel
            && superseded_by_snapshot_rev
                .map_or(envelope.revision != host_rev, |_| envelope.revision > host_rev);
        (host_rev, is_tree_drift, pointer_cancel_rev_drift)
    }

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
            .check_channel_header(self.identity, self.incoming_sequence)
            .map_err(anyhow::Error::msg)?;
        ensure!(
            envelope.minor <= self.protocol_minor,
            "插件消息协议版本超过协商上限"
        );
        self.incoming_sequence = envelope.request_id;
        let now = Instant::now();
        let expired = envelope.is_expired();
        let panel_id = envelope.panel_id;
        ensure!(
            panel_id == PANEL_ID || self.supports_panel_multiplexing,
            "插件未声明支持多路复用却使用了非主面板 panel_id"
        );
        let is_main_recovery_reply = panel_id == PANEL_ID
            && envelope.in_reply_to != 0
            && self
                .requests
                .get(&envelope.in_reply_to)
                .is_some_and(|p| p.panel_id == PANEL_ID && p.kind.is_panel_recovery());
        let is_fatal_main_expired_frame = panel_id == PANEL_ID
            && !is_main_recovery_reply
            && ((expired
                && (envelope.in_reply_to == 0
                    || self
                        .requests
                        .get(&envelope.in_reply_to)
                        .is_some_and(|p| p.panel_id == PANEL_ID)))
                || (envelope.in_reply_to != 0
                    && self
                        .requests
                        .get(&envelope.in_reply_to)
                        .is_some_and(|p| p.panel_id == PANEL_ID && now >= p.deadline)));
        if !is_fatal_main_expired_frame {
            if self
                .outbound_claims
                .as_deref()
                .is_some_and(OutboundWriteClaims::is_fatal_expired_triggered)
            {
                return self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing);
            }
            self.check_fatal_timeouts(now, outgoing)?;
            self.drain_urgent_controls(outgoing)?;
        }
        if panel_id != PANEL_ID
            && envelope.in_reply_to != 0
            && !self.requests.contains_key(&envelope.in_reply_to)
            && self.panels.get(&panel_id).is_some_and(|s| {
                s.closing && s.destroy_failed && s.destroy_request_id == Some(envelope.in_reply_to)
            })
        {
            match &envelope.body {
                Some(
                    envelope::Body::Ack(_)
                    | envelope::Body::Snapshot(_)
                    | envelope::Body::Patch(_),
                ) => {
                    self.cleanup_removed_panel(panel_id);
                    self.emit(PluginEvent::PanelClosed {
                        identity: self.identity,
                        panel_id,
                    })
                    .await?;
                    self.flush_pending_revokes(outgoing)?;
                    self.flush_pending_resyncs(outgoing)?;
                    self.flush_deferred_panel_commands(outgoing).await?;
                    return Ok(());
                }
                Some(envelope::Body::Error(_)) => {
                    if let Some(slot) = self.panels.get_mut(&panel_id) {
                        slot.destroy_request_id = None;
                    }
                    return self
                        .diagnostic_on_panel(
                            panel_id,
                            Failure::new(
                                ErrorCode::Stale,
                                "多路复用子面板迟到关闭结果返回错误；保留故障槽位",
                            ),
                        )
                        .await;
                }
                _ => {}
            }
        }
        if envelope.in_reply_to != 0
            && let Some(pos) = self
                .cancel_notifications
                .iter()
                .position(|n| n.panel_id == panel_id && n.cancel_frame_id == envelope.in_reply_to)
        {
            let notification = self.cancel_notifications.remove(pos).unwrap();
            let frame_revision = match &envelope.body {
                Some(envelope::Body::Snapshot(snap)) => envelope.revision.max(snap.revision),
                Some(envelope::Body::Patch(patch)) => {
                    envelope.revision.max(patch.revision).max(patch.base_revision)
                }
                _ => envelope.revision,
            };
            let should_schedule_resync = notification.can_restart_resync;
            match envelope.body {
                Some(envelope::Body::Ack(_)) => {
                    if envelope.revision > self.panel_revision(panel_id) {
                        if should_schedule_resync {
                            self.schedule_panel_semantic_resync(panel_id);
                        } else {
                            self.mark_panel_semantic_drift(panel_id);
                        }
                    }
                    self.flush_pending_revokes(outgoing)?;
                    self.flush_pending_resyncs(outgoing)?;
                    self.flush_deferred_panel_commands(outgoing).await?;
                    return Ok(());
                }
                Some(envelope::Body::Error(failure)) => {
                    ensure!(
                        failure.message.len() <= 4096
                            && protocol::ErrorCode::try_from(failure.code).is_ok(),
                        "插件错误消息无效"
                    );
                    if envelope.revision > self.panel_revision(panel_id) {
                        if should_schedule_resync {
                            self.schedule_panel_semantic_resync(panel_id);
                        } else {
                            self.mark_panel_semantic_drift(panel_id);
                        }
                    }
                    self.diagnostic_on_panel(panel_id, failure).await?;
                    self.flush_pending_revokes(outgoing)?;
                    self.flush_pending_resyncs(outgoing)?;
                    self.flush_deferred_panel_commands(outgoing).await?;
                    return Ok(());
                }
                Some(envelope::Body::Snapshot(_) | envelope::Body::Patch(_)) => {
                    let is_tree_drift = match notification.converged_snapshot_rev {
                        Some(snap_rev) => frame_revision >= snap_rev,
                        None => frame_revision != self.panel_revision(panel_id),
                    };
                    if is_tree_drift {
                        if should_schedule_resync {
                            self.schedule_panel_semantic_resync(panel_id);
                        } else {
                            self.mark_panel_semantic_drift(panel_id);
                        }
                    }
                    self.flush_pending_revokes(outgoing)?;
                    self.flush_pending_resyncs(outgoing)?;
                    self.flush_deferred_panel_commands(outgoing).await?;
                    return Ok(());
                }
                _ => {}
            }
        }
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
            if panel_id == PANEL_ID
                && self.protocol_minor < protocol::PROTOCOL_MINOR
                && self.take_v14_degraded_resync(envelope.in_reply_to)
                && let Some(envelope::Body::Error(err)) = &envelope.body
            {
                ensure!(
                    err.message.len() <= 4096
                        && protocol::ErrorCode::try_from(err.code).is_ok(),
                    "插件错误消息无效"
                );
                if err.code == ErrorCode::Deadline as i32 {
                    return self
                        .diagnostic_on_panel(
                            panel_id,
                            Self::v14_resync_degrade_failure(Some(&err.message)),
                        )
                        .await;
                }
            }
            let host_rev = self.panel_revision(panel_id);
            let frame_revision = match &envelope.body {
                Some(envelope::Body::Snapshot(snap)) => envelope.revision.max(snap.revision),
                Some(envelope::Body::Patch(patch)) => {
                    envelope.revision.max(patch.revision).max(patch.base_revision)
                }
                _ => envelope.revision,
            };
            let drift_evidence_for = |converged_snapshot_rev: Option<u64>| match &envelope.body {
                Some(envelope::Body::Snapshot(_) | envelope::Body::Patch(_)) => {
                    converged_snapshot_rev.is_none_or(|snap_rev| frame_revision >= snap_rev)
                }
                Some(envelope::Body::Ack(_) | envelope::Body::Error(_)) => {
                    envelope.revision > host_rev
                }
                _ => false,
            };
            let mut reverted_optimistic = false;
            let mut settled_optimistic = false;
            let mut cleared_tracked_optimistic = false;
            let mut ambiguous_callback_error = false;
            if let Some(cancelled_entry) =
                self.take_cancelled_state_request(panel_id, envelope.in_reply_to)
            {
                let is_drift_evidence = drift_evidence_for(cancelled_entry.converged_snapshot_rev);
                let was_optimistic_pending =
                    cancelled_entry.kind == CancelledStateRequestKind::OptimisticPending;
                let was_optimistic_promoted =
                    cancelled_entry.kind == CancelledStateRequestKind::OptimisticPromotedByResyncAck;
                let was_optimistic_detached_settled = cancelled_entry.kind
                    == CancelledStateRequestKind::OptimisticDetachedSettledByResyncAck;
                if let Some(envelope::Body::Error(ref err)) = envelope.body {
                    let had_optimistic =
                        self.has_panel_optimistic_request(panel_id, envelope.in_reply_to);
                    let had_tracked = had_optimistic
                        || self.has_panel_tracked_optimistic_request(panel_id, envelope.in_reply_to);
                    if had_tracked || was_optimistic_pending {
                        reverted_optimistic =
                            self.retract_panel_optimistic_request(panel_id, envelope.in_reply_to);
                        if reverted_optimistic && self.has_inflight_resync(panel_id) {
                            self.mark_panel_semantic_drift(panel_id);
                        }
                        cleared_tracked_optimistic = true;
                    }
                    if was_optimistic_promoted
                        || ((was_optimistic_pending || was_optimistic_detached_settled || had_tracked)
                            && err.code != ErrorCode::Deadline as i32)
                    {
                        ambiguous_callback_error = true;
                    }
                } else if matches!(envelope.body, Some(envelope::Body::Ack(_))) {
                    settled_optimistic = self
                        .settle_cancelled_panel_optimistic_request(panel_id, envelope.in_reply_to);
                    if was_optimistic_pending && !settled_optimistic {
                        ambiguous_callback_error = true;
                    }
                } else if matches!(
                    envelope.body,
                    Some(envelope::Body::Snapshot(_) | envelope::Body::Patch(_))
                ) {
                    let _ = self.retract_panel_optimistic_request(panel_id, envelope.in_reply_to);
                }
                if is_drift_evidence || ambiguous_callback_error {
                    if cancelled_entry.can_restart_resync {
                        self.schedule_panel_semantic_resync(panel_id);
                        self.flush_pending_resyncs(outgoing)?;
                        self.flush_deferred_panel_commands(outgoing).await?;
                    } else {
                        self.mark_panel_semantic_drift(panel_id);
                    }
                }
            } else if let Some(cancelled_resync) =
                self.take_cancelled_resync_request(panel_id, envelope.in_reply_to)
                && drift_evidence_for(cancelled_resync.converged_snapshot_rev)
            {
                if cancelled_resync.can_restart_resync {
                    self.schedule_panel_semantic_resync(panel_id);
                    self.flush_pending_resyncs(outgoing)?;
                    self.flush_deferred_panel_commands(outgoing).await?;
                } else {
                    self.mark_panel_semantic_drift(panel_id);
                }
            }
            // A cancelled operation must not resurrect its resource or overwrite retained editors.
            self.diagnostic_on_panel(
                panel_id,
                Failure::new(
                    ErrorCode::Stale,
                    "已丢弃无有效关联操作的迟到插件结果",
                ),
            )
            .await?;
            if reverted_optimistic || settled_optimistic || cleared_tracked_optimistic {
                self.notify_panel_synced_nonblocking(panel_id);
            }
            return Ok(());
        }
        if envelope.in_reply_to != 0
            && let Some(pending) = self.requests.get(&envelope.in_reply_to)
        {
            ensure!(
                pending.panel_id == panel_id,
                "插件响应的 panel_id 与关联请求不匹配"
            );
        }
        if panel_id != PANEL_ID {
            let slot = self.panels.get(&panel_id);
            let missing = slot.is_none();
            let closing_unsolicited = slot.is_some_and(|s| {
                s.closing
                    && (envelope.in_reply_to == 0
                        || matches!(envelope.body, Some(envelope::Body::HostCall(_)))
                        || !self.requests.contains_key(&envelope.in_reply_to))
            });
            if missing || closing_unsolicited {
                if let Some(envelope::Body::HostCall(_)) = &envelope.body {
                    let failure = Failure::new(
                        ErrorCode::Stale,
                        "多路复用子面板已关闭；未执行宿主调用",
                    );
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
                return self
                    .diagnostic_on_panel(
                        panel_id,
                        Failure::new(ErrorCode::Stale, "多路复用子面板已关闭或未知"),
                    )
                    .await;
            }
        }
        let local_pending_expired = envelope.in_reply_to != 0
            && self
                .requests
                .get(&envelope.in_reply_to)
                .is_some_and(|p| p.panel_id == panel_id && now >= p.deadline);
        let verified_destroy_ack = panel_id != PANEL_ID
            && envelope.in_reply_to != 0
            && matches!(
                envelope.body,
                Some(
                    envelope::Body::Ack(_)
                        | envelope::Body::Snapshot(_)
                        | envelope::Body::Patch(_)
                )
            )
            && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                p.panel_id == panel_id && matches!(p.kind, PendingKind::Close { remove_panel: true, .. })
            });
        if panel_id != PANEL_ID && (expired || local_pending_expired) && !verified_destroy_ack {
            let is_destroy_close = envelope.in_reply_to != 0
                && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    p.panel_id == panel_id
                        && matches!(
                            p.kind,
                            PendingKind::Close {
                                remove_panel: true,
                                ..
                            }
                        )
                });
            let was_non_resync_state_req = envelope.in_reply_to != 0
                && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    p.panel_id == panel_id
                        && matches!(
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
                        )
                });
            let was_pointer_cancel_req = envelope.in_reply_to != 0
                && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    p.panel_id == panel_id
                        && matches!(
                            p.kind,
                            PendingKind::Event {
                                pointer_cancel_seq: Some(_),
                                ..
                            }
                        )
                });
            let was_resync_req = envelope.in_reply_to != 0
                && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    p.panel_id == panel_id
                        && matches!(
                            p.kind,
                            PendingKind::Event {
                                resync_seq: Some(_),
                                ..
                            }
                        )
                });
            let was_normal_ui_event = envelope.in_reply_to != 0
                && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    p.panel_id == panel_id
                        && matches!(
                            p.kind,
                            PendingKind::Event {
                                resync_seq: None,
                                pointer_cancel_seq: None,
                                ..
                            }
                        )
                });
            let was_optimistic_event = envelope.in_reply_to != 0
                && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    p.panel_id == panel_id
                        && matches!(
                            p.kind,
                            PendingKind::Event {
                                resync_seq: None,
                                pointer_cancel_seq: None,
                                is_optimistic: true,
                                ..
                            }
                        )
                });
            let pointer_capture = if envelope.in_reply_to != 0 {
                self.requests
                    .get(&envelope.in_reply_to)
                    .map_or(PointerCaptureTransition::None, |p| match p.kind {
                        PendingKind::Event {
                            resync_seq: None,
                            pointer_capture,
                            ..
                        } => pointer_capture,
                        _ => PointerCaptureTransition::None,
                    })
            } else {
                PointerCaptureTransition::None
            };
            let source_seq = self
                .requests
                .get(&envelope.in_reply_to)
                .map_or(0, Pending::ui_event_seq);
            let had_optimistic = was_normal_ui_event
                && self.has_panel_optimistic_request(panel_id, envelope.in_reply_to);
            let had_confirmed_viewport = was_normal_ui_event
                && self.panel_has_confirmed_viewport_request(panel_id, envelope.in_reply_to);
            let had_detached_viewport = was_normal_ui_event
                && self.panel_has_detached_viewport(panel_id, envelope.in_reply_to);
            let is_callback_error = matches!(
                envelope.body,
                Some(envelope::Body::Error(ref err)) if err.code != ErrorCode::Deadline as i32
            );
            let missing_optimistic_ack = was_optimistic_event
                && !had_optimistic
                && !had_confirmed_viewport
                && !had_detached_viewport
                && matches!(envelope.body, Some(envelope::Body::Ack(_)));
            let mut reverted_optimistic = false;
            if was_normal_ui_event && matches!(envelope.body, Some(envelope::Body::Error(_))) {
                reverted_optimistic =
                    self.retract_panel_optimistic_request(panel_id, envelope.in_reply_to);
                if reverted_optimistic && self.has_inflight_resync(panel_id) {
                    self.mark_panel_semantic_drift(panel_id);
                }
            }
            let (host_rev, is_tree_drift, pointer_cancel_rev_drift) =
                self.expired_reply_drift_flags(panel_id, &envelope, was_pointer_cancel_req);
            let expired_confirmed_harmless = had_confirmed_viewport
                && (matches!(envelope.body, Some(envelope::Body::Ack(_)))
                    || (matches!(
                        envelope.body,
                        Some(envelope::Body::Snapshot(_) | envelope::Body::Patch(_))
                    ) && !is_tree_drift));
            if expired_confirmed_harmless {
                let _ = self.retract_panel_optimistic_request(panel_id, envelope.in_reply_to);
            }
            let retracted_unsent =
                if envelope.in_reply_to != 0 && self.requests.contains_key(&envelope.in_reply_to) {
                    self.cancel_pending_request(
                        envelope.in_reply_to,
                        "插件子面板响应已过期",
                        Some(outgoing),
                        !is_destroy_close && !expired_confirmed_harmless,
                        false,
                    )
                } else {
                    false
                };
            self.settle_pointer_capture_on_failure(panel_id, pointer_capture, retracted_unsent);
            if was_resync_req {
                if is_tree_drift || envelope.revision > host_rev {
                    self.mark_panel_semantic_drift(panel_id);
                }
            } else if is_tree_drift
                || pointer_cancel_rev_drift
                || (was_non_resync_state_req && envelope.revision > host_rev)
                || ((was_optimistic_event || had_optimistic) && is_callback_error)
                || missing_optimistic_ack
                || (was_optimistic_event
                    && !expired_confirmed_harmless
                    && !had_detached_viewport
                    && matches!(envelope.body, Some(envelope::Body::Ack(_))))
            {
                self.schedule_panel_semantic_resync(panel_id);
            } else if was_non_resync_state_req && !expired_confirmed_harmless {
                self.schedule_panel_resync(panel_id);
            }
            self.flush_pending_revokes(outgoing)?;
            self.flush_pending_resyncs(outgoing)?;
            self.flush_deferred_panel_commands(outgoing).await?;
            self.check_fatal_timeouts(Instant::now(), outgoing)?;
            if matches!(envelope.body, Some(envelope::Body::HostCall(_))) {
                let failure = Failure::new(
                    ErrorCode::Deadline,
                    "子面板宿主调用消息已过期；未执行",
                );
                self.send_on_panel(
                    outgoing,
                    panel_id,
                    envelope::Body::HostResult(HostResult {
                        outcome: Some(host_result::Outcome::Error(failure)),
                    }),
                    envelope.request_id,
                )?;
            }
            self.diagnostic_on_panel(
                panel_id,
                Failure::new(ErrorCode::Deadline, "已丢弃过期的子面板插件消息"),
            )
            .await?;
            if !retracted_unsent
                && (reverted_optimistic
                    || ((was_optimistic_event || had_optimistic)
                        && (matches!(envelope.body, Some(envelope::Body::Error(_)))
                            || !expired_confirmed_harmless)))
            {
                self.notify_panel_synced_with_rejected_nonblocking(
                    panel_id,
                    (!expired_confirmed_harmless && source_seq != 0).then_some(source_seq),
                );
            } else if !retracted_unsent && expired_confirmed_harmless {
                self.notify_panel_synced_nonblocking(panel_id);
            }
            return Ok(());
        }
        if panel_id == PANEL_ID && (expired || local_pending_expired) {
            let main_resync_seq = if envelope.in_reply_to != 0 {
                self.requests
                    .get(&envelope.in_reply_to)
                    .and_then(|p| match p.kind {
                        PendingKind::Event {
                            resync_seq: Some(seq),
                            ..
                        } if p.panel_id == PANEL_ID => Some(seq),
                        _ => None,
                    })
            } else {
                None
            };
            if let Some(seq) = main_resync_seq {
                let legacy_v14 = self.protocol_minor < protocol::PROTOCOL_MINOR;
                let err_detail = if legacy_v14 {
                    match &envelope.body {
                        Some(envelope::Body::Error(err)) => {
                            ensure!(
                                err.message.len() <= 4096
                                    && protocol::ErrorCode::try_from(err.code).is_ok(),
                                "插件错误消息无效"
                            );
                            Some(err.message.clone())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let (host_rev, is_tree_drift, _) =
                    self.expired_reply_drift_flags(PANEL_ID, &envelope, false);
                self.cancel_pending_request(
                    envelope.in_reply_to,
                    "主面板重同步响应已超时",
                    Some(outgoing),
                    !legacy_v14,
                    false,
                );
                if is_tree_drift || envelope.revision > host_rev {
                    self.mark_panel_semantic_drift(PANEL_ID);
                }
                if legacy_v14 {
                    self.mark_panel_resync_incompatible(PANEL_ID, seq);
                }
                self.flush_pending_revokes(outgoing)?;
                self.flush_pending_resyncs(outgoing)?;
                self.flush_deferred_panel_commands(outgoing).await?;
                let failure = if legacy_v14 {
                    Self::v14_resync_degrade_failure(err_detail.as_deref())
                } else {
                    Failure::new(ErrorCode::Deadline, "已丢弃已超时的主面板重同步响应")
                };
                return self.diagnostic_on_panel(panel_id, failure).await;
            }
            let main_pointer_cancel_seq = if envelope.in_reply_to != 0 {
                self.requests
                    .get(&envelope.in_reply_to)
                    .and_then(|p| match p.kind {
                        PendingKind::Event {
                            pointer_cancel_seq: Some(seq),
                            ..
                        } if p.panel_id == PANEL_ID => Some(seq),
                        _ => None,
                    })
            } else {
                None
            };
            if main_pointer_cancel_seq.is_some() {
                let (_, is_tree_drift, pointer_cancel_rev_drift) =
                    self.expired_reply_drift_flags(PANEL_ID, &envelope, true);
                self.cancel_pending_request(
                    envelope.in_reply_to,
                    "主面板指针手势取消响应已超时",
                    Some(outgoing),
                    true,
                    false,
                );
                if is_tree_drift || pointer_cancel_rev_drift {
                    self.schedule_panel_semantic_resync(PANEL_ID);
                }
                self.flush_pending_revokes(outgoing)?;
                self.flush_pending_resyncs(outgoing)?;
                self.flush_deferred_panel_commands(outgoing).await?;
                return self
                    .diagnostic_on_panel(
                        panel_id,
                        Failure::new(
                            ErrorCode::Deadline,
                            "已丢弃已超时的主面板指针手势取消响应",
                        ),
                    )
                    .await;
            }
            if expired {
                self.retract_all_unclaimed_outbound();
                bail!("插件消息 deadline 已过期");
            }
            let mut expired_targets = Vec::new();
            let mut reject_target = None;
            if envelope.in_reply_to != 0 && self.requests.contains_key(&envelope.in_reply_to) {
                expired_targets.push((panel_id, envelope.in_reply_to));
                reject_target = Some((
                    envelope.in_reply_to,
                    "主面板操作已超时；已取消并回收实例",
                ));
            }
            return self.abort_fatal_timeout(&expired_targets, outgoing, reject_target);
        }
        match envelope.body.clone().context("缺少插件消息")? {
            envelope::Body::Patch(patch) => {
                ensure!(
                    envelope.revision == patch.revision,
                    "Panel envelope revision 不匹配"
                );
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                enum PatchBarrierOutcome {
                    Superseded {
                        newer_drift: bool,
                        drift_retracted: Vec<u64>,
                    },
                    Applied {
                        was_resync: bool,
                        is_optimistic_event: bool,
                        had_optimistic_before_patch: bool,
                        plan: RetractedOptimisticPlan,
                    },
                    ResyncErr {
                        error: String,
                        drift_retracted: Vec<u64>,
                    },
                    FatalErr(String),
                }
                let claims = self.outbound_claims.clone();
                let outcome = {
                    let mut barrier = claims.as_deref().map(OutboundWriteClaims::lock_barrier);
                    if self.is_superseded_resync_reply(panel_id, envelope.in_reply_to) {
                        let newer_drift = envelope
                            .revision
                            .max(patch.revision)
                            .max(patch.base_revision)
                            >= self.panel_revision(panel_id);
                        let drift_retracted = if newer_drift {
                            self.retract_drift_candidates_under_barrier(
                                panel_id,
                                &BTreeSet::new(),
                                barrier.as_mut(),
                            )
                        } else {
                            Vec::new()
                        };
                        PatchBarrierOutcome::Superseded {
                            newer_drift,
                            drift_retracted,
                        }
                    } else {
                        let was_resync = envelope.in_reply_to != 0
                            && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                                p.panel_id == panel_id
                                    && matches!(
                                        p.kind,
                                        PendingKind::Event {
                                            resync_seq: Some(_),
                                            ..
                                        }
                                    )
                            });
                        let is_optimistic_event = !was_resync
                            && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                                matches!(
                                    p.kind,
                                    PendingKind::Event {
                                        resync_seq: None,
                                        pointer_cancel_seq: None,
                                        is_optimistic: true,
                                        ..
                                    }
                                )
                            });
                        let had_optimistic_before_patch = is_optimistic_event
                            && (self.has_panel_optimistic_request(panel_id, envelope.in_reply_to)
                                || self.panel_has_confirmed_viewport_request(
                                    panel_id,
                                    envelope.in_reply_to,
                                )
                                || self.panel_has_detached_viewport(
                                    panel_id,
                                    envelope.in_reply_to,
                                )
                                || self.panel_patch_confirms_detached_viewport(
                                    panel_id,
                                    &patch,
                                    envelope.in_reply_to,
                                ));
                        let unclaimed =
                            self.unclaimed_request_ids_under_barrier(panel_id, barrier.as_ref());
                        let applied = if panel_id == PANEL_ID {
                            self.panel.apply_for_request_with_unclaimed(
                                &patch,
                                envelope.in_reply_to,
                                &unclaimed,
                            )
                        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
                            if slot.closing {
                                Ok(Vec::new())
                            } else if let Some(state) = slot.state.as_mut() {
                                state.apply_for_request_with_unclaimed(
                                    &patch,
                                    envelope.in_reply_to,
                                    &unclaimed,
                                )
                            } else {
                                Err("多路复用子面板需要完整 Snapshot 初始化".into())
                            }
                        } else {
                            Err("多路复用子面板需要完整 Snapshot 初始化".into())
                        };
                        match applied {
                            Ok(invalidated) => PatchBarrierOutcome::Applied {
                                was_resync,
                                is_optimistic_event,
                                had_optimistic_before_patch,
                                plan: self.retract_invalidated_optimistic_under_barrier(
                                    panel_id,
                                    &invalidated,
                                    is_optimistic_event && !had_optimistic_before_patch,
                                    barrier.as_mut(),
                                ),
                            },
                            Err(error) => {
                                if error.contains("revision") || error.contains("较新") {
                                    let drift_retracted = self
                                        .retract_drift_candidates_under_barrier(
                                            panel_id,
                                            &BTreeSet::new(),
                                            barrier.as_mut(),
                                        );
                                    PatchBarrierOutcome::ResyncErr {
                                        error,
                                        drift_retracted,
                                    }
                                } else {
                                    PatchBarrierOutcome::FatalErr(error)
                                }
                            }
                        }
                    }
                };
                let (was_resync, is_optimistic_event, had_optimistic_before_patch, retract_plan) =
                    match outcome {
                        PatchBarrierOutcome::Superseded {
                            newer_drift,
                            drift_retracted,
                        } => {
                            self.finish_retracted_drift_requests(panel_id, drift_retracted);
                            self.reject_response(
                                envelope.in_reply_to,
                                "重同步补丁基于已被新状态取代的旧版本，未覆盖当前面板",
                            );
                            if newer_drift {
                                self.schedule_panel_semantic_resync(panel_id);
                            } else {
                                self.schedule_panel_resync(panel_id);
                            }
                            self.flush_pending_revokes(outgoing)?;
                            self.flush_pending_resyncs(outgoing)?;
                            self.flush_deferred_panel_commands(outgoing).await?;
                            return Ok(());
                        }
                        PatchBarrierOutcome::ResyncErr {
                            error,
                            drift_retracted,
                        } => {
                            self.finish_retracted_drift_requests(panel_id, drift_retracted);
                            return self
                                .resync_panel(panel_id, outgoing, envelope.in_reply_to, error)
                                .await;
                        }
                        PatchBarrierOutcome::FatalErr(error) => bail!(error),
                        PatchBarrierOutcome::Applied {
                            was_resync,
                            is_optimistic_event,
                            had_optimistic_before_patch,
                            plan,
                        } => (
                            was_resync,
                            is_optimistic_event,
                            had_optimistic_before_patch,
                            plan,
                        ),
                    };
                let editor_bases =
                    self.accept_panel_response(panel_id, envelope.in_reply_to, now)?;
                self.defer_panel_sync_for_publish = true;
                self.finish_retracted_optimistic_plan(panel_id, retract_plan);
                if is_optimistic_event && !had_optimistic_before_patch {
                    self.schedule_panel_semantic_resync(panel_id);
                }
                if was_resync && self.is_panel_semantic_drifted(panel_id) {
                    self.fail_deferred_interactive_events(
                        panel_id,
                        "插件重同步仅返回局部补丁未确认完整快照；旧视图的交互操作未执行，请刷新或重载面板",
                    );
                }
                self.clear_panel_resync_incompatible(panel_id);
                self.defer_panel_sync_for_publish = false;
                self.publish_panel(panel_id, editor_bases).await?;
            }
            envelope::Body::Snapshot(snapshot) => {
                ensure!(
                    envelope.revision == snapshot.revision,
                    "Panel snapshot revision 不匹配"
                );
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                enum SnapshotBarrierOutcome {
                    ValidationErr(String),
                    Superseded {
                        newer_drift: bool,
                        drift_retracted: Vec<u64>,
                    },
                    Replaced {
                        was_resync: bool,
                        plan: RetractedOptimisticPlan,
                    },
                    ResyncErr {
                        error: String,
                        drift_retracted: Vec<u64>,
                    },
                }
                let claims = self.outbound_claims.clone();
                let outcome = {
                    let mut barrier = claims.as_deref().map(OutboundWriteClaims::lock_barrier);
                    if let Err(err) = snapshot.validate() {
                        SnapshotBarrierOutcome::ValidationErr(err)
                    } else if self.is_superseded_resync_reply(panel_id, envelope.in_reply_to) {
                        let newer_drift =
                            envelope.revision.max(snapshot.revision) >= self.panel_revision(panel_id);
                        let drift_retracted = if newer_drift {
                            self.retract_drift_candidates_under_barrier(
                                panel_id,
                                &BTreeSet::new(),
                                barrier.as_mut(),
                            )
                        } else {
                            Vec::new()
                        };
                        SnapshotBarrierOutcome::Superseded {
                            newer_drift,
                            drift_retracted,
                        }
                    } else {
                        let was_resync = envelope.in_reply_to != 0
                            && self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                                p.panel_id == panel_id
                                    && matches!(
                                        p.kind,
                                        PendingKind::Event {
                                            resync_seq: Some(_),
                                            ..
                                        }
                                    )
                            });
                        let unclaimed =
                            self.unclaimed_request_ids_under_barrier(panel_id, barrier.as_ref());
                        let replaced = if panel_id == PANEL_ID {
                            if was_resync {
                                self.panel.replace_for_resync_with_unclaimed(
                                    snapshot,
                                    envelope.in_reply_to,
                                    &unclaimed,
                                )
                            } else {
                                self.panel.replace_for_request_with_unclaimed(
                                    snapshot,
                                    envelope.in_reply_to,
                                    &unclaimed,
                                )
                            }
                        } else if let Some(slot) = self.panels.get_mut(&panel_id) {
                            if slot.closing {
                                Ok(Vec::new())
                            } else {
                                match &mut slot.state {
                                    Some(state) if was_resync => state
                                        .replace_for_resync_with_unclaimed(
                                            snapshot,
                                            envelope.in_reply_to,
                                            &unclaimed,
                                        ),
                                    Some(state) => state.replace_for_request_with_unclaimed(
                                        snapshot,
                                        envelope.in_reply_to,
                                        &unclaimed,
                                    ),
                                    None => PanelState::new(snapshot).map(|state| {
                                        slot.state = Some(state);
                                        Vec::new()
                                    }),
                                }
                            }
                        } else {
                            Err("未知多路复用子面板".into())
                        };
                        match replaced {
                            Ok(invalidated) => SnapshotBarrierOutcome::Replaced {
                                was_resync,
                                plan: self.retract_invalidated_optimistic_under_barrier(
                                    panel_id,
                                    &invalidated,
                                    false,
                                    barrier.as_mut(),
                                ),
                            },
                            Err(error) => {
                                let drift_retracted = self.retract_drift_candidates_under_barrier(
                                    panel_id,
                                    &BTreeSet::new(),
                                    barrier.as_mut(),
                                );
                                SnapshotBarrierOutcome::ResyncErr {
                                    error,
                                    drift_retracted,
                                }
                            }
                        }
                    }
                };
                let (was_resync, retract_plan) = match outcome {
                    SnapshotBarrierOutcome::ValidationErr(err) => {
                        return Err(anyhow::Error::msg(err));
                    }
                    SnapshotBarrierOutcome::Superseded {
                        newer_drift,
                        drift_retracted,
                    } => {
                        self.finish_retracted_drift_requests(panel_id, drift_retracted);
                        self.reject_response(
                            envelope.in_reply_to,
                            "重同步快照基于已被新状态取代的旧版本，未覆盖当前面板",
                        );
                        if newer_drift {
                            self.schedule_panel_semantic_resync(panel_id);
                        } else {
                            self.schedule_panel_resync(panel_id);
                        }
                        self.flush_pending_revokes(outgoing)?;
                        self.flush_pending_resyncs(outgoing)?;
                        self.flush_deferred_panel_commands(outgoing).await?;
                        return Ok(());
                    }
                    SnapshotBarrierOutcome::ResyncErr {
                        error,
                        drift_retracted,
                    } => {
                        self.finish_retracted_drift_requests(panel_id, drift_retracted);
                        return self
                            .resync_panel(panel_id, outgoing, envelope.in_reply_to, error)
                            .await;
                    }
                    SnapshotBarrierOutcome::Replaced { was_resync, plan } => (was_resync, plan),
                };
                let editor_bases =
                    self.accept_panel_response(panel_id, envelope.in_reply_to, now)?;
                let panel_active = panel_id == PANEL_ID
                    || self.panels.get(&panel_id).is_some_and(|s| !s.closing);
                if panel_active {
                    self.record_panel_accepted_snapshot_revision(panel_id, envelope.revision);
                }
                let recovered = panel_active
                    && !self.panel_has_uncertain_optimistic(panel_id)
                    && !self.is_panel_resyncing(panel_id);
                if was_resync {
                    self.mark_panel_cancelled_requests_converged_by_snapshot(
                        panel_id,
                        envelope.in_reply_to,
                        envelope.revision,
                        recovered,
                    );
                } else if panel_active {
                    self.mark_panel_cancelled_requests_updated_by_snapshot(
                        panel_id,
                        envelope.revision,
                        recovered,
                    );
                    if !recovered
                        && !self.panel_has_uncertain_optimistic(panel_id)
                        && self.is_panel_resyncing(panel_id)
                    {
                        self.mark_panel_resync_snapshot_confirmed(panel_id);
                    }
                }
                self.defer_panel_sync_for_publish = true;
                if recovered {
                    self.clear_panel_semantic_drift(panel_id);
                }
                self.finish_retracted_optimistic_plan(panel_id, retract_plan);
                if recovered {
                    self.clear_panel_resync_incompatible(panel_id);
                }
                self.defer_panel_sync_for_publish = false;
                self.publish_panel(panel_id, editor_bases).await?;
            }
            envelope::Body::Committed(committed) => {
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
                    ensure!(!cancelled.is_cancelled(), "编辑提交已取消；丢弃结果");
                    ensure!(panel_id == req_panel, "Committed panel_id 不匹配");
                    ensure!(now < pending.deadline, "编辑提交已超时");
                    ensure!(
                        self.opening_request_for(req_panel).is_none(),
                        "编辑上下文正在切换；丢弃旧提交结果"
                    );
                    ensure!(
                        self.active_resource_for(req_panel) == Some((resource, message_revision)),
                        "编辑上下文已切换；丢弃旧提交结果"
                    );
                    self.broker
                        .writable_panel(req_panel, resource, message_revision)
                        .map_err(|f| anyhow::anyhow!(f.message))?;
                    ensure!(
                        protocol::uuid(&committed.resource_id).ok() == Some(resource)
                            && committed.message_revision == message_revision
                            && committed.document_id == document.document_id
                            && committed.document_revision == document.revision,
                        "编辑提交结果身份或 revision 不匹配"
                    );
                    ensure!(
                        !self.is_panel_semantic_drifted(req_panel),
                        "插件面板尚未通过最新快照确认状态；丢弃未确认状态的编辑提交结果"
                    );
                    ensure!(
                        envelope.revision == self.panel_revision(req_panel),
                        "编辑提交结果面板 revision 与宿主已确认视图不一致；丢弃结果"
                    );
                    ensure!(
                        expected_snapshot.as_deref().is_none_or(|expected| {
                            panel::commit_panel_semantically_matches(
                                self.panel_snapshot(req_panel).as_deref(),
                                expected,
                                &document,
                            )
                        }),
                        "插件面板内容已更新；丢弃旧视图的编辑提交结果"
                    );
                    ensure!(
                        self.editor_on_panel(req_panel, document.document_id)
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
                if envelope.revision > self.panel_revision(req_panel) {
                    self.schedule_panel_semantic_resync(req_panel);
                }
            }
            envelope::Body::ResponseCommitted(committed) => {
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
                    ensure!(!cancelled.is_cancelled(), "响应编辑提交已取消；丢弃结果");
                    ensure!(panel_id == req_panel, "ResponseCommitted panel_id 不匹配");
                    ensure!(now < pending.deadline, "响应编辑提交已超时");
                    ensure!(
                        self.opening_request_for(req_panel).is_none()
                            && self.active_resource_for(req_panel)
                                == Some((resource, message_revision)),
                        "响应编辑上下文已切换；丢弃旧提交结果"
                    );
                    self.broker
                        .writable_panel(req_panel, resource, message_revision)
                        .map_err(|f| anyhow::anyhow!(f.message))?;
                    ensure!(
                        protocol::uuid(&committed.resource_id).ok() == Some(resource)
                            && committed.message_revision == message_revision
                            && committed.document_id == document.document_id
                            && committed.document_revision == document.revision,
                        "响应编辑提交结果身份或 revision 不匹配"
                    );
                    ensure!(
                        !self.is_panel_semantic_drifted(req_panel),
                        "插件面板尚未通过最新快照确认状态；丢弃未确认状态的响应编辑提交结果"
                    );
                    ensure!(
                        envelope.revision == self.panel_revision(req_panel),
                        "响应编辑提交结果面板 revision 与宿主已确认视图不一致；丢弃结果"
                    );
                    ensure!(
                        expected_snapshot.as_deref().is_none_or(|expected| {
                            panel::commit_panel_semantically_matches(
                                self.panel_snapshot(req_panel).as_deref(),
                                expected,
                                &document,
                            )
                        }),
                        "插件面板内容已更新；丢弃旧视图的响应编辑提交结果"
                    );
                    ensure!(
                        self.editor_on_panel(req_panel, document.document_id)
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
                if envelope.revision > self.panel_revision(req_panel) {
                    self.schedule_panel_semantic_resync(req_panel);
                }
            }
            envelope::Body::HostCall(call) => {
                let host_rev = self.panel_revision(panel_id);
                if envelope.revision != host_rev {
                    let (allow_cancel_cleanup, can_drive_resync) =
                        self.classify_drifted_host_call(panel_id, &call, now);
                    if envelope.revision > host_rev && can_drive_resync {
                        self.schedule_panel_semantic_resync(panel_id);
                        self.flush_pending_resyncs(outgoing)?;
                    }
                    if !allow_cancel_cleanup {
                        let failure = Failure::new(
                            ErrorCode::Stale,
                            "服务调用来自旧面板 revision；未执行且不会自动重试",
                        );
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
                }
                self.host_call(&envelope, call, outgoing).await?;
            }
            envelope::Body::Cancel(cancel) => {
                if let Some(service) = self
                    .service_cancels
                    .get(&cancel.request_id)
                    .filter(|service| service.panel_id == panel_id)
                {
                    service.cancel.cancel();
                } else {
                    self.diagnostic_on_panel(
                        panel_id,
                        Failure::new(
                            ErrorCode::Cancelled,
                            "取消的任务已结束或不属于此实例；未重试请求",
                        ),
                    )
                    .await?;
                }
            }
            envelope::Body::Ack(_) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                let pending = self
                    .requests
                    .remove(&envelope.in_reply_to)
                    .context("无对应事件的 Ack")?;
                self.sync_fatal_deadline_to_claims();
                match pending.kind {
                    PendingKind::Event {
                        resync_seq,
                        resync_revision,
                        resync_snapshot,
                        pointer_cancel_seq,
                        is_optimistic,
                        ..
                    } => {
                        let settled_optimistic = self
                            .settle_panel_optimistic_request(pending.panel_id, envelope.in_reply_to);
                        if let Some(seq) = pointer_cancel_seq {
                            let host_rev = self.panel_revision(pending.panel_id);
                            self.complete_panel_pointer_cancel(pending.panel_id, seq);
                            if envelope.revision != host_rev {
                                self.schedule_panel_semantic_resync(pending.panel_id);
                                if !self.is_panel_resyncing(pending.panel_id) {
                                    self.fail_deferred_panel_commands(
                                        pending.panel_id,
                                        "插件面板版本已偏离且无法自动重同步；已取消等待中的面板操作，请重试",
                                    );
                                    self.diagnostic_on_panel(
                                        pending.panel_id,
                                        Failure::new(
                                            ErrorCode::Stale,
                                            format!(
                                                "指针手势取消回执版本 ({}) 与宿主面板版本 ({}) 不一致且无法自动重同步，请重载插件",
                                                envelope.revision, host_rev
                                            ),
                                        ),
                                    )
                                    .await?;
                                }
                            }
                        }
                        let mut resync_terminated = false;
                        if let Some(seq) = resync_seq {
                            let host_rev = self.panel_revision(pending.panel_id);
                            let baseline_matches = resync_snapshot.as_deref()
                                == self.panel_snapshot(pending.panel_id).as_deref();
                            if envelope.revision == host_rev && baseline_matches {
                                let snapshot_confirmed =
                                    self.take_panel_resync_snapshot_confirmed(pending.panel_id);
                                self.complete_panel_resync(pending.panel_id, seq);
                                if snapshot_confirmed
                                    && !self.panel_has_uncertain_optimistic(pending.panel_id)
                                {
                                    self.clear_panel_semantic_drift(pending.panel_id);
                                }
                                if !self.is_panel_semantic_drifted(pending.panel_id) {
                                    self.settle_panel_uncertain_optimistic_requests_through(
                                        pending.panel_id,
                                        envelope.in_reply_to,
                                    );
                                } else {
                                    self.clear_panel_resync_restart_eligibility(pending.panel_id);
                                }
                                resync_terminated = true;
                                if self.is_panel_semantic_drifted(pending.panel_id) {
                                    self.fail_deferred_interactive_events(
                                        pending.panel_id,
                                        "插件重同步仅确认版本未返回最新面板快照；旧视图的交互操作未执行，请刷新或重载面板",
                                    );
                                }
                            } else if envelope.revision == resync_revision
                                || (!baseline_matches && envelope.revision == host_rev)
                            {
                                self.schedule_panel_resync(pending.panel_id);
                            } else {
                                self.mark_panel_semantic_drift(pending.panel_id);
                                self.mark_panel_resync_incompatible(pending.panel_id, seq);
                                resync_terminated = true;
                                self.diagnostic_on_panel(
                                    pending.panel_id,
                                    Failure::new(
                                        ErrorCode::Stale,
                                        format!(
                                            "插件重同步回执版本 ({}) 未回退到宿主确认版本 ({})；旧版插件可能不支持版本回退，请重载或升级插件",
                                            envelope.revision, host_rev
                                        ),
                                    ),
                                )
                                .await?;
                            }
                        }
                        if resync_seq.is_none()
                            && pointer_cancel_seq.is_none()
                            && (envelope.revision > self.panel_revision(pending.panel_id)
                                || (is_optimistic && !settled_optimistic))
                        {
                            self.schedule_panel_semantic_resync(pending.panel_id);
                        }
                        if settled_optimistic || resync_terminated {
                            self.notify_panel_synced_nonblocking(pending.panel_id);
                        }
                    }
                    PendingKind::Close {
                        remove_panel,
                        replies,
                        ..
                    } => {
                        if remove_panel {
                            self.cleanup_removed_panel(pending.panel_id);
                        }
                        for reply in replies {
                            let _ = reply.send(Ok(()));
                        }
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
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                let diag_panel = self
                    .requests
                    .get(&envelope.in_reply_to)
                    .map_or(panel_id, |p| p.panel_id);
                let resync_pending = self.requests.get(&envelope.in_reply_to).and_then(|p| {
                    match &p.kind {
                        PendingKind::Event {
                            resync_seq: Some(seq),
                            resync_revision,
                            resync_snapshot,
                            ..
                        } => Some((p.panel_id, *seq, *resync_revision, resync_snapshot.clone())),
                        _ => None,
                    }
                });
                let pointer_cancel_pending = self.requests.get(&envelope.in_reply_to).and_then(|p| {
                    match p.kind {
                        PendingKind::Event {
                            pointer_cancel_seq: Some(seq),
                            ..
                        } => Some((p.panel_id, seq)),
                        _ => None,
                    }
                });
                let pointer_capture_error = self.requests.get(&envelope.in_reply_to).and_then(|p| {
                    match p.kind {
                        PendingKind::Event {
                            resync_seq: None,
                            pointer_capture,
                            ..
                        } if pointer_capture != PointerCaptureTransition::None => {
                            Some((p.panel_id, pointer_capture))
                        }
                        _ => None,
                    }
                });
                let is_normal_ui_event = self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    matches!(
                        p.kind,
                        PendingKind::Event {
                            resync_seq: None,
                            pointer_cancel_seq: None,
                            ..
                        }
                    )
                });
                let is_optimistic_event = self.requests.get(&envelope.in_reply_to).is_some_and(|p| {
                    matches!(
                        p.kind,
                        PendingKind::Event {
                            resync_seq: None,
                            pointer_cancel_seq: None,
                            is_optimistic: true,
                            ..
                        }
                    )
                });
                let diag_source_seq = self
                    .requests
                    .get(&envelope.in_reply_to)
                    .map_or(0, Pending::ui_event_seq);
                let had_optimistic = is_normal_ui_event
                    && self.has_panel_optimistic_request(diag_panel, envelope.in_reply_to);
                let had_tracked_optimistic = is_normal_ui_event
                    && (had_optimistic
                        || self.has_panel_tracked_optimistic_request(
                            diag_panel,
                            envelope.in_reply_to,
                        ));
                let is_explicit_non_exec = failure.code == ErrorCode::Deadline as i32;
                let ambiguous_callback_error =
                    (is_optimistic_event || had_tracked_optimistic) && !is_explicit_non_exec;
                let reverted_optimistic = if had_tracked_optimistic || is_optimistic_event {
                    let reverted =
                        self.retract_panel_optimistic_request(diag_panel, envelope.in_reply_to);
                    if reverted && self.has_inflight_resync(diag_panel) {
                        self.mark_panel_semantic_drift(diag_panel);
                    }
                    reverted
                } else {
                    false
                };
                self.reject_response(envelope.in_reply_to, &failure.message);
                if let Some((p_panel, seq)) = pointer_cancel_pending {
                    if failure.code != ErrorCode::Deadline as i32 {
                        self.abort_panel_pointer_cancel(p_panel, seq);
                    }
                    if envelope.revision != self.panel_revision(p_panel) {
                        self.schedule_panel_semantic_resync(p_panel);
                    }
                } else if resync_pending.is_none()
                    && (envelope.revision > self.panel_revision(diag_panel)
                        || ambiguous_callback_error)
                {
                    self.schedule_panel_semantic_resync(diag_panel);
                }
                if let Some((p_panel, transition)) = pointer_capture_error {
                    self.settle_pointer_capture_on_failure(p_panel, transition, false);
                }
                let mut diag_failure = failure;
                let mut resync_terminated = false;
                if let Some((r_panel, seq, resync_revision, resync_snapshot)) = resync_pending {
                    let legacy_v14 =
                        r_panel == PANEL_ID && self.protocol_minor < protocol::PROTOCOL_MINOR;
                    if legacy_v14 && diag_failure.code == ErrorCode::Deadline as i32 {
                        self.mark_panel_resync_incompatible(r_panel, seq);
                        resync_terminated = true;
                        diag_failure = Self::v14_resync_degrade_failure(Some(&diag_failure.message));
                    } else if diag_failure.code != ErrorCode::Deadline as i32 {
                        let superseded = envelope.revision == resync_revision
                            && (self.panel_revision(r_panel) != resync_revision
                                || resync_snapshot.as_deref()
                                    != self.panel_snapshot(r_panel).as_deref());
                        if superseded {
                            self.schedule_panel_resync(r_panel);
                        } else {
                            self.abort_panel_resync(r_panel, seq);
                            resync_terminated = true;
                        }
                    }
                }
                self.diagnostic_on_panel(diag_panel, diag_failure).await?;
                if reverted_optimistic
                    || had_tracked_optimistic
                    || is_optimistic_event
                    || resync_terminated
                {
                    self.notify_panel_synced_with_rejected_nonblocking(
                        diag_panel,
                        (diag_source_seq != 0).then_some(diag_source_seq),
                    );
                }
            }
            envelope::Body::HttpHookResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                let action = HookAction::try_from(result.action).map_err(anyhow::Error::msg)?;
                self.accept_hook_result(envelope.in_reply_to, action, HookReply::Http(result), now)
                    .await?;
            }
            envelope::Body::WebSocketHookResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                let action = HookAction::try_from(result.action).map_err(anyhow::Error::msg)?;
                self.accept_hook_result(envelope.in_reply_to, action, HookReply::Ws(result), now)
                    .await?;
            }
            envelope::Body::SessionActionResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::Session(result),
                    now,
                )
                .await?;
            }
            envelope::Body::PayloadResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::Payload(result),
                    now,
                )
                .await?;
            }
            envelope::Body::ContextMenuResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
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
                    now,
                )
                .await?;
            }
            envelope::Body::PassiveCheckResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::PassiveCheck(result),
                    now,
                )
                .await?;
            }
            envelope::Body::ActiveCheckResult(result) => {
                self.settle_request_deadline_before_reply(envelope.in_reply_to, outgoing)?;
                self.accept_hook_result(
                    envelope.in_reply_to,
                    HookAction::Pass,
                    HookReply::ActiveCheck(result),
                    now,
                )
                .await?;
            }
            _ => bail!("worker 发送了不允许的宿主方向消息"),
        }
        self.flush_pending_revokes(outgoing)?;
        self.flush_pending_resyncs(outgoing)?;
        self.flush_deferred_panel_commands(outgoing).await?;
        Ok(())
    }

    pub(super) async fn accept_hook_result(
        &mut self,
        request_id: u64,
        action: HookAction,
        value: HookReply,
        now: Instant,
    ) -> Result<()> {
        let Some(pending) = self.requests.remove(&request_id) else {
            return self
                .diagnostic(Failure::new(
                    ErrorCode::Stale,
                    "已丢弃无有效关联操作的迟到 Hook 结果",
                ))
                .await;
        };
        self.sync_fatal_deadline_to_claims();
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
        } else if now >= pending.deadline {
            let _ = reply.send(Err(anyhow::anyhow!("插件 Hook 调用已超时")));
        } else {
            let _ = reply.send(Ok(value));
        }
        Ok(())
    }

    pub(super) fn accept_panel_response(
        &mut self,
        expected_panel_id: u64,
        request_id: u64,
        now: Instant,
    ) -> Result<Vec<EditorSnapshot>> {
        if let Some(pending) = self.requests.remove(&request_id) {
            self.sync_fatal_deadline_to_claims();
            ensure!(
                pending.panel_id == expected_panel_id,
                "插件响应的 panel_id 与关联请求不匹配"
            );
            let panel_id = pending.panel_id;
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
                    self.broker.release_panel_resource(panel_id, reserved);
                    let result = (|| {
                        ensure!(
                            self.opening_request_for(panel_id) == Some(request_id),
                            "打开请求已被新的上下文取代"
                        );
                        ensure!(now < pending.deadline, "打开请求已超时");
                        self.broker
                            .bind_panel_resource(panel_id, resource, revision, read_only)?;
                        if pending.source_seq != 0 {
                            self.panel_resource_open_seq
                                .entry((panel_id, resource))
                                .and_modify(|s| *s = (*s).max(pending.source_seq))
                                .or_insert(pending.source_seq);
                        }
                        self.set_active_resource_for(panel_id, Some((resource, revision)));
                        self.panel_snapshot(panel_id)
                            .context("多路复用子面板首次打开必须返回完整 Snapshot")
                    })();
                    if self.opening_request_for(panel_id) == Some(request_id) {
                        self.set_opening_request_for(panel_id, None);
                    }
                    let _ = reply.send(result);
                }
                PendingKind::Event {
                    editor_bases,
                    resync_seq,
                    pointer_cancel_seq,
                    ..
                } => {
                    ensure!(now < pending.deadline, "插件 UI 事件已超时");
                    self.settle_panel_optimistic_request(panel_id, request_id);
                    if let Some(seq) = resync_seq {
                        let snapshot_confirmed =
                            self.take_panel_resync_snapshot_confirmed(panel_id);
                        self.complete_panel_resync(panel_id, seq);
                        if snapshot_confirmed && !self.panel_has_uncertain_optimistic(panel_id) {
                            self.clear_panel_semantic_drift(panel_id);
                        }
                        if !self.is_panel_semantic_drifted(panel_id) {
                            self.settle_panel_uncertain_optimistic_requests_through(
                                panel_id, request_id,
                            );
                        } else {
                            self.clear_panel_resync_restart_eligibility(panel_id);
                        }
                    }
                    if let Some(seq) = pointer_cancel_seq {
                        self.complete_panel_pointer_cancel(panel_id, seq);
                    }
                    return Ok(editor_bases);
                }
                PendingKind::Close {
                    remove_panel,
                    replies,
                    ..
                } => {
                    if remove_panel {
                        self.cleanup_removed_panel(panel_id);
                    }
                    for reply in replies {
                        let _ = reply.send(Ok(()));
                    }
                }
                PendingKind::Hook { .. } => bail!("Hook 调用不能返回面板响应"),
            }
        }
        Ok(vec![])
    }
}
