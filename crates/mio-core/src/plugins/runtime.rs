use super::*;

impl Running {
    pub(super) fn reject_response(&mut self, request_id: u64, reason: &str) {
        self.reject_response_with_mode(request_id, reason, false, true);
    }

    pub(super) fn reject_response_with_mode(
        &mut self,
        request_id: u64,
        reason: &str,
        retain_destroy_corr: bool,
        settle_optimistic: bool,
    ) {
        if let Some(pending) = self.requests.remove(&request_id) {
            self.sync_fatal_deadline_to_claims();
            let panel_id = pending.panel_id;
            pending.cancel.cancel();
            for service in self.service_cancels.values() {
                if service.panel_id == panel_id && service.parent_request_id == request_id {
                    service.cancel.cancel();
                }
            }
            if self.opening_request_for(panel_id) == Some(request_id) {
                self.set_opening_request_for(panel_id, None);
            }
            match pending.kind {
                PendingKind::Open {
                    reply, reserved, ..
                } => {
                    self.broker.release_panel_resource(panel_id, reserved);
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                }
                PendingKind::Close {
                    resource: _,
                    remove_panel,
                    replies,
                } => {
                    if remove_panel {
                        self.mark_panel_destroy_failed(
                            panel_id,
                            retain_destroy_corr.then_some(request_id),
                        );
                    }
                    for reply in replies {
                        let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                    }
                }
                PendingKind::Commit { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                }
                PendingKind::CommitResponse { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                }
                PendingKind::Hook { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                }
                PendingKind::Event { .. } => {
                    if settle_optimistic {
                        self.retract_panel_optimistic_request(panel_id, request_id);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn resync(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        reply: u64,
        reason: String,
    ) -> Result<()> {
        let panel_id = self
            .requests
            .get(&reply)
            .map_or(PANEL_ID, |p| p.panel_id);
        self.resync_panel(panel_id, outgoing, reply, reason).await
    }

    pub(super) async fn resync_panel(
        &mut self,
        panel_id: u64,
        outgoing: &mpsc::Sender<Envelope>,
        reply: u64,
        reason: String,
    ) -> Result<()> {
        let mut pointer_cancel_seq = None;
        let mut rejected_optimistic_seq = None;
        if let Some(pending) = self.requests.get(&reply) {
            ensure!(
                !matches!(
                    pending.kind,
                    PendingKind::Commit { .. } | PendingKind::CommitResponse { .. }
                ),
                "CommitEditor 必须返回明确提交结果"
            );
            if let PendingKind::Event {
                resync_seq: None,
                pointer_cancel_seq: None,
                is_optimistic: true,
                ..
            } = pending.kind
            {
                let seq = pending.ui_event_seq();
                if seq != 0 {
                    rejected_optimistic_seq = Some(seq);
                }
            }
            if let PendingKind::Event {
                pointer_cancel_seq: seq,
                ..
            } = pending.kind
            {
                pointer_cancel_seq = seq;
            }
        }
        self.reject_response(reply, &reason);
        if let Some(seq) = pointer_cancel_seq {
            self.complete_panel_pointer_cancel(panel_id, seq);
        }
        self.schedule_panel_semantic_resync(panel_id);
        self.flush_pending_revokes(outgoing)?;
        self.flush_pending_resyncs(outgoing)?;
        self.flush_deferred_panel_commands(outgoing).await?;
        self.diagnostic_on_panel(
            panel_id,
            Failure::new(
                ErrorCode::Stale,
                format!("{reason}；保留最新编辑并请求重新同步"),
            ),
        )
        .await?;
        if let Some(seq) = rejected_optimistic_seq {
            self.notify_panel_synced_with_rejected_nonblocking(panel_id, Some(seq));
        }
        Ok(())
    }

    pub(super) fn complete_service(
        &mut self,
        mut completed: ServiceCompletion,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.drain_urgent_controls(outgoing)?;
        let panel_id = completed.panel_id;
        if matches!(
            completed.resource_binding,
            Some(PendingResource::OpenRepeater)
        ) {
            self.broker.release_panel_resource(panel_id, true);
            if let Ok(ServiceResult::Resource { id, .. }) = &completed.result {
                let revoked = self
                    .panel_revoked_resources
                    .get(&(panel_id, *id))
                    .is_some_and(|barrier| completed.request_id <= *barrier);
                if revoked {
                    completed.result = Err(Failure::new(
                        ErrorCode::Stale,
                        "新标签已关闭；迟到结果未重新授权资源",
                    ));
                }
            }
        }
        if self
            .service_cancels
            .remove(&completed.request_id)
            .is_some_and(|service| service.cancel.is_cancelled())
        {
            completed.result = Err(Failure::new(
                ErrorCode::Cancelled,
                "宿主调用已取消；迟到结果未重新授权资源",
            ));
        }
        // Retain close barriers only while an earlier resource-creating call can still complete.
        self.panel_revoked_resources.retain(|(p, _), barrier| {
            self.service_cancels
                .iter()
                .filter_map(|(request, service)| {
                    (service.panel_id == *p
                        && matches!(service.binding, Some(PendingResource::OpenRepeater)))
                    .then_some(*request)
                })
                .min()
                .is_some_and(|oldest| oldest <= *barrier)
        });
        if let (Some(expected), Ok(ServiceResult::Resource { id, revision })) =
            (completed.resource_binding, &completed.result)
        {
            match self
                .broker
                .complete_panel_resource(panel_id, expected, *id, *revision)
            {
                Ok(()) => {
                    if self
                        .active_resource_for(panel_id)
                        .is_some_and(|(current, _)| current == *id)
                    {
                        self.set_active_resource_for(panel_id, Some((*id, *revision)));
                    }
                }
                Err(failure) => completed.result = Err(failure),
            }
        }
        let result = match completed.result {
            Ok(result) => result.into_wire(),
            Err(failure) => HostResult {
                outcome: Some(host_result::Outcome::Error(failure)),
            },
        };
        let mut envelope = self.envelope_for(
            panel_id,
            envelope::Body::HostResult(result),
            completed.request_id,
        )?;
        if let Err(error) = envelope.validate() {
            envelope.body = Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(Failure::new(
                    ErrorCode::Invalid,
                    format!("{error}；内容未截断，已执行的操作不会自动重试"),
                ))),
            }));
        }
        envelope.validate().map_err(anyhow::Error::msg)?;
        self.push_outbound(outgoing, envelope)?;
        Ok(())
    }

    pub(super) async fn handle_timeouts(
        &mut self,
        now: Instant,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        self.check_fatal_timeouts(now, outgoing)?;
        self.drain_urgent_controls(outgoing)?;
        let mut expired: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(id, p)| {
                let (resync_seq, is_pointer_cancel) = match p.kind {
                    PendingKind::Event {
                        resync_seq,
                        pointer_cancel_seq,
                        ..
                    } => (resync_seq, pointer_cancel_seq.is_some()),
                    _ => (None, false),
                };
                (p.deadline <= now).then_some((
                    *id,
                    p.panel_id,
                    matches!(p.kind, PendingKind::Hook { .. }),
                    resync_seq,
                    is_pointer_cancel,
                ))
            })
            .collect();
        expired.sort_unstable_by_key(|&(id, _, _, _, _)| id);
        if expired
            .iter()
            .any(|(_, panel_id, is_hook, resync_seq, is_pointer_cancel)| {
                (*panel_id == PANEL_ID && resync_seq.is_none() && !*is_pointer_cancel) || *is_hook
            })
        {
            let expired_targets: Vec<_> = expired
                .iter()
                .map(|&(id, panel_id, _, _, _)| (panel_id, id))
                .collect();
            return self.abort_fatal_timeout(&expired_targets, outgoing, None);
        }
        let mut cancels_to_send = Vec::new();
        let mut diagnostics = Vec::with_capacity(expired.len());
        let mut sync_panels = BTreeSet::new();
        for (id, panel_id, _, resync_seq, is_pointer_cancel) in expired {
            let is_resync = resync_seq.is_some();
            let is_destroy_close = self.requests.get(&id).is_some_and(|p| {
                matches!(
                    p.kind,
                    PendingKind::Close {
                        remove_panel: true,
                        ..
                    }
                )
            });
            let (needs_new_resync, mutated_host_panel, pointer_capture, is_optimistic, ui_event_seq) =
                self.requests.get(&id).map_or(
                    (false, false, PointerCaptureTransition::None, false, 0),
                    |p| match p.kind {
                        PendingKind::Open { .. }
                        | PendingKind::Close {
                            remove_panel: false,
                            ..
                        } => (true, false, PointerCaptureTransition::None, false, 0),
                        PendingKind::Event {
                            resync_seq: None,
                            pointer_cancel_seq: None,
                            mutated_host_panel,
                            pointer_capture,
                            is_optimistic,
                            ui_event_seq,
                            ..
                        } => (
                            true,
                            mutated_host_panel,
                            pointer_capture,
                            is_optimistic,
                            ui_event_seq,
                        ),
                        _ => (false, false, PointerCaptureTransition::None, false, 0),
                    },
                );
            let already_confirmed_viewport =
                self.panel_has_confirmed_viewport_request(panel_id, id);
            let (retracted_unsent, reverted_optimistic, cancel_to_send) =
                self.cancel_pending_request_inner(
                    id,
                    if is_resync {
                        "面板重同步操作超时"
                    } else if is_pointer_cancel {
                        "面板指针手势取消操作超时"
                    } else {
                        "插件子面板操作超时；已取消此面板请求，未影响共享实例"
                    },
                    !is_destroy_close && !already_confirmed_viewport,
                    true,
                );
            if reverted_optimistic || is_optimistic {
                sync_panels.insert(panel_id);
                if (reverted_optimistic || (is_optimistic && !already_confirmed_viewport))
                    && ui_event_seq != 0
                {
                    let entry = self.pending_panel_syncs.entry(panel_id).or_default();
                    entry.insert(ui_event_seq);
                    while entry.len() > QUEUE_CAPACITY {
                        entry.pop_first();
                    }
                }
            }
            if let Some(cancel) = cancel_to_send {
                cancels_to_send.push(cancel);
            }
            if needs_new_resync
                && !already_confirmed_viewport
                && (!retracted_unsent || mutated_host_panel)
            {
                self.schedule_panel_resync(panel_id);
            }
            self.settle_pointer_capture_on_failure(panel_id, pointer_capture, retracted_unsent);
            let diag = Failure::new(
                ErrorCode::Cancelled,
                if is_resync {
                    "面板重同步操作超时"
                } else if is_pointer_cancel {
                    "面板指针手势取消操作超时"
                } else {
                    "插件子面板操作超时"
                },
            );
            diagnostics.push((panel_id, diag));
        }
        for (c_panel, c_req, for_resync, snap_rev) in cancels_to_send {
            self.send_cancel_best_effort(outgoing, c_panel, c_req, for_resync, snap_rev);
        }
        for (panel_id, diag) in diagnostics {
            let _ = self.diagnostic_on_panel(panel_id, diag).await;
        }
        for panel_id in sync_panels {
            self.notify_panel_synced_nonblocking(panel_id);
        }
        self.check_fatal_timeouts(Instant::now(), outgoing)?;
        self.flush_pending_revokes(outgoing)?;
        self.flush_pending_resyncs(outgoing)?;
        self.flush_deferred_panel_commands(outgoing).await?;
        Ok(())
    }

    pub(super) fn spawn_stream_io<W, R>(
        mut stdin: W,
        mut stdout: R,
        mut writes: mpsc::Receiver<Envelope>,
        wire: &mpsc::Sender<Wire>,
        outbound_claims: Arc<OutboundWriteClaims>,
        write_progress: tokio::sync::watch::Sender<u64>,
        io_cancel: &CancellationToken,
        io_tasks: &mut JoinSet<()>,
    ) where
        W: AsyncWrite + Unpin + Send + 'static,
        R: AsyncRead + Unpin + Send + 'static,
    {
        let reader_cancel = io_cancel.clone();
        let reader_events = wire.clone();
        io_tasks.spawn(async move {
            loop {
                let result = tokio::select! {
                    biased;
                    _ = reader_cancel.cancelled() => break,
                    result = read_async_frame(&mut stdout) => result,
                };
                let message = match result {
                    Ok(frame) => Wire::Frame(Box::new(frame)),
                    Err(_) => Wire::Failed("插件输出 pipe 关闭或 Protobuf 帧无效"),
                };
                let failed = matches!(message, Wire::Failed(_));
                tokio::select! {
                    biased;
                    _ = reader_cancel.cancelled() => break,
                    result = reader_events.send(message) => if result.is_err() { break; }
                }
                if failed {
                    break;
                }
            }
        });
        let writer_cancel = io_cancel.clone();
        let writer_events = wire.clone();
        io_tasks.spawn(async move {
            loop {
                let envelope = tokio::select! {
                    biased;
                    _ = writer_cancel.cancelled() => break,
                    next = writes.recv() => match next { Some(value) => value, None => break }
                };
                if OUTGOING_CAPACITY.saturating_sub(writes.len()) > RESERVED_CONTROL_OUTGOING {
                    write_progress.send_modify(|seq| *seq = seq.wrapping_add(1));
                }
                if !outbound_claims.claim_for_write(envelope.panel_id, envelope.request_id) {
                    continue;
                }
                let result = async {
                    stdin.write_all(&protocol::encode_frame(&envelope)?).await?;
                    stdin.flush().await?;
                    Ok::<_, anyhow::Error>(())
                };
                let result = tokio::select! {
                    biased;
                    _ = writer_cancel.cancelled() => break,
                    result = timeout(MAX_OPERATION, result) => result,
                };
                if !matches!(result, Ok(Ok(()))) {
                    let _ = writer_events.try_send(Wire::Failed("插件输入 pipe 超时或关闭"));
                    break;
                }
            }
        });
    }

    async fn recv_urgent(
        urgent_commands: &mut Option<mpsc::UnboundedReceiver<SequencedControl>>,
    ) -> Option<SequencedControl> {
        match urgent_commands.as_mut() {
            Some(rx) => rx.recv().await,
            None => None,
        }
    }

    pub(super) async fn run(
        mut self,
        transport: WorkerTransport,
        mut commands: mpsc::Receiver<SequencedControl>,
        urgent_commands: mpsc::UnboundedReceiver<SequencedControl>,
    ) -> Result<()> {
        self.urgent_commands = Some(urgent_commands);
        let outbound_claims = Arc::new(OutboundWriteClaims::default());
        outbound_claims.set_stop_cancel(self.cancel.clone());
        self.outbound_claims = Some(outbound_claims.clone());
        self.sync_fatal_deadline_to_claims();
        let (outgoing, writes) = mpsc::channel::<Envelope>(OUTGOING_CAPACITY);
        let (wire, mut incoming) = mpsc::channel(QUEUE_CAPACITY);
        let (write_progress_tx, mut write_progress_rx) = tokio::sync::watch::channel(0u64);
        let io_cancel = CancellationToken::new();
        let mut io_tasks = JoinSet::new();
        let mut native_child = match transport {
            WorkerTransport::Native {
                child,
                #[cfg(windows)]
                job,
                stdin,
                stdout,
                mut stderr,
            } => {
                Self::spawn_stream_io(
                    stdin,
                    stdout,
                    writes,
                    &wire,
                    outbound_claims.clone(),
                    write_progress_tx.clone(),
                    &io_cancel,
                    &mut io_tasks,
                );
                let stderr_cancel = io_cancel.clone();
                io_tasks.spawn(async move {
                    let mut buffer = [0; 4096];
                    loop {
                        // Drain stderr continuously so long-running workers never block on OS pipe buffers,
                        // while never echoing arbitrary stderr into application logs (it may contain secrets).
                        tokio::select! {
                            biased;
                            _ = stderr_cancel.cancelled() => break,
                            result = stderr.read(&mut buffer) => match result {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                    }
                });
                #[cfg(windows)]
                let pair = (child, job);
                #[cfg(not(windows))]
                let pair = (child, ());
                Some(pair)
            }
            #[cfg(test)]
            WorkerTransport::Duplex { stdin, stdout } => {
                Self::spawn_stream_io(
                    stdin,
                    stdout,
                    writes,
                    &wire,
                    outbound_claims.clone(),
                    write_progress_tx.clone(),
                    &io_cancel,
                    &mut io_tasks,
                );
                None
            }
        };
        let result: Result<()> = async {
    let events = self.events.clone();
    self.publish(vec![]).await?;
    loop {
        let _ = write_progress_rx.borrow_and_update();
        self.child_headroom_blocked = false;
        let now = Instant::now();
        self.check_fatal_timeouts(now, &outgoing)?;
        if self.requests.values().any(|p| p.deadline <= now) {
            self.handle_timeouts(now, &outgoing).await?;
        }
        self.drain_urgent_controls(&outgoing)?;
        self.flush_outbound_nonblocking(&outgoing)?;
        self.flush_pending_revokes(&outgoing)?;
        self.flush_pending_resyncs(&outgoing)?;
        self.flush_deferred_panel_commands(&outgoing).await?;
        if self.child_headroom_blocked && outgoing.capacity() > RESERVED_CONTROL_OUTGOING {
            continue;
        }
        let deadline = self.requests.values().map(|p| p.deadline).min().unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => break,
            _ = events.closed() => break,
            _ = tokio::time::sleep_until(deadline), if !self.requests.is_empty() => {
                self.handle_timeouts(Instant::now(), &outgoing).await?;
            }
            permit = events.reserve(), if !self.pending_panel_syncs.is_empty() => {
                match permit {
                    Ok(permit) => {
                        let now = Instant::now();
                        if let Err(err) = self.check_fatal_timeouts(now, &outgoing) {
                            drop(permit);
                            return Err(err);
                        }
                        if let Some((panel_id, rejected)) = self.pending_panel_syncs.pop_first()
                            && !self.panels.get(&panel_id).is_some_and(|s| s.closing)
                            && let Some(snapshot) = self.ui_panel_snapshot(panel_id)
                        {
                            let synced_ui_seq = self.contiguous_processed_ui_seq(panel_id);
                            permit.send(PluginEvent::PanelSynced {
                                identity: self.identity,
                                panel_id,
                                snapshot,
                                synced_ui_seq,
                                rejected_ui_seqs: rejected.into_iter().collect(),
                            });
                        } else {
                            drop(permit);
                        }
                    }
                    Err(_) => break,
                }
            }
            urgent = Self::recv_urgent(&mut self.urgent_commands), if self.urgent_commands.is_some() => match urgent {
                Some(cmd) => {
                    self.check_fatal_timeouts(Instant::now(), &outgoing)?;
                    self.control_with_seq(cmd.seq, cmd.command, &outgoing).await?;
                }
                None => {
                    self.urgent_commands = None;
                }
            },
            permit = outgoing.reserve(), if !self.outbound_queue.is_empty() => {
                let permit = permit.map_err(|_| anyhow::anyhow!("插件输入 pipe 已关闭"))?;
                let now = Instant::now();
                self.check_fatal_timeouts(now, &outgoing)?;
                if self.requests.values().any(|p| p.deadline <= now) {
                    drop(permit);
                    self.handle_timeouts(now, &outgoing).await?;
                } else {
                    self.drain_urgent_controls(&outgoing)?;
                    if let Some(env) = self.outbound_queue.pop_front() {
                        permit.send(env);
                    } else {
                        drop(permit);
                    }
                    self.flush_outbound_nonblocking(&outgoing)?;
                }
            }
            Ok(()) = write_progress_rx.changed(), if self.child_headroom_blocked => {}
            command = commands.recv() => match command {
                Some(cmd) => {
                    self.check_fatal_timeouts(Instant::now(), &outgoing)?;
                    self.control_with_seq(cmd.seq, cmd.command, &outgoing).await?;
                }
                None => break,
            },
            frame = incoming.recv() => match frame {
                Some(Wire::Frame(frame)) => self.frame(*frame, &outgoing).await?,
                Some(Wire::Failed(message)) => bail!(message),
                None => bail!("插件 pipe 任务已退出"),
            },
            Some(completed) = self.service_tasks.join_next(), if !self.service_tasks.is_empty() => {
                self.check_fatal_timeouts(Instant::now(), &outgoing)?;
                self.complete_service(completed.context("插件宿主服务回调异常退出")?, &outgoing)?;
            }
        }
    }
    Ok(())
}.await;
        self.prepare_shutdown(&outgoing);
        let reason = result
            .as_ref()
            .err()
            .map_or_else(|| "插件已停止".to_owned(), ToString::to_string);
        if let Some(mut urgent) = self.urgent_commands.take() {
            urgent.close();
            while let Ok(cmd) = urgent.try_recv() {
                self.fail_control_command(cmd.command, &reason);
            }
        }
        let mut deferred = std::collections::VecDeque::new();
        for slot in self.panels.values_mut() {
            deferred.extend(slot.deferred_commands.drain(..));
        }
        for cmd in deferred {
            self.fail_control_command(cmd.command, &reason);
        }
        for revoke in self.pending_revokes.drain(..) {
            for reply in revoke.replies {
                let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
            }
        }
        for (_, pending) in self.requests.drain() {
            pending.cancel.cancel();
            match pending.kind {
                PendingKind::Open { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
                }
                PendingKind::Close { replies, .. } => {
                    for reply in replies {
                        let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
                    }
                }
                PendingKind::Commit { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
                }
                PendingKind::CommitResponse { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
                }
                PendingKind::Hook { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
                }
                PendingKind::Event { .. } => {}
            }
        }
        self.sync_fatal_deadline_to_claims();
        if native_child.is_some() {
            // Pipe closure gives cooperative shutdown a bounded grace without reaping its group leader.
            let _ = timeout(STOP_GRACE, async {
                while let Some(message) = incoming.recv().await {
                    if matches!(message, Wire::Failed(_)) {
                        break;
                    }
                }
            })
            .await;
        }
        let (terminated, reaped) = if let Some((mut child, job)) = native_child.take() {
            #[cfg(not(windows))]
            let _ = job;
            #[cfg(windows)]
            if let Some(ref j) = job {
                let _ = j.terminate();
            }
            #[cfg(unix)]
            let group = child.id();
            let mut terminated = kill_worker_group(&mut child);
            let reaped = timeout(REAP_DEADLINE, child.wait())
                .await
                .context("插件已发出终止但回收进程超时")
                .and_then(|result| result.context("插件回收进程失败"))
                .map(|_| ());
            // macOS can return EPERM for a group containing only its zombie leader; verify absence, never signal it again.
            #[cfg(unix)]
            if reaped.is_ok()
        && terminated.as_ref().err().is_some_and(|error| {
            error.downcast_ref::<std::io::Error>().is_some_and(|error| error.raw_os_error() == Some(libc::EPERM))
        })
        && group.is_some_and(|pid| unsafe { libc::kill(-(pid as i32), 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
    {
        terminated = Ok(());
    }
            #[cfg(not(unix))]
            let _ = &mut terminated;
            (terminated, reaped)
        } else {
            (Ok(()), Ok(()))
        };
        io_cancel.cancel();
        io_tasks.abort_all();
        while io_tasks.join_next().await.is_some() {}
        self.service_tasks.abort_all();
        while self.service_tasks.join_next().await.is_some() {}
        self.cancel.cancel();
        let result = result.and(terminated).and(reaped);
        let reason = result.as_ref().err().map_or(reason, ToString::to_string);
        let _ = self.events.try_send(PluginEvent::Stopped {
            identity: self.identity,
            reason,
        });
        result
    }
}
