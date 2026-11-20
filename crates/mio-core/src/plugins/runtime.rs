use super::*;

impl Running {
    pub(super) fn reject_response(&mut self, request_id: u64, reason: &str) {
        if self.opening_request == Some(request_id) {
            self.opening_request = None;
        }
        if let Some(pending) = self.requests.remove(&request_id) {
            match pending.kind {
                PendingKind::Open {
                    reply, reserved, ..
                } => {
                    self.broker.release_resource(reserved);
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
                }
                PendingKind::Close { reply } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.to_owned())));
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
                PendingKind::Event { .. } => {}
            }
        }
    }

    pub(super) async fn resync(
        &mut self,
        outgoing: &mpsc::Sender<Envelope>,
        reply: u64,
        reason: String,
    ) -> Result<()> {
        if let Some(pending) = self.requests.get(&reply) {
            ensure!(
                !matches!(
                    pending.kind,
                    PendingKind::Commit { .. } | PendingKind::CommitResponse { .. }
                ),
                "CommitEditor 必须返回明确提交结果"
            );
        }
        self.reject_response(reply, &reason);
        self.enqueue(
            outgoing,
            envelope::Body::Resync(self.panel.snapshot().clone()),
            PendingKind::Event {
                editor_bases: vec![],
            },
        )?;
        self.diagnostic(Failure::new(
            ErrorCode::Stale,
            format!("{reason}；保留最新编辑并请求重新同步"),
        ))
        .await
    }

    pub(super) fn complete_service(
        &mut self,
        mut completed: ServiceCompletion,
        outgoing: &mpsc::Sender<Envelope>,
    ) -> Result<()> {
        if matches!(
            completed.resource_binding,
            Some(PendingResource::OpenRepeater)
        ) {
            self.broker.release_resource(true);
            if let Ok(ServiceResult::Resource { id, .. }) = &completed.result
                && self
                    .revoked_resources
                    .get(id)
                    .is_some_and(|barrier| completed.request_id <= *barrier)
            {
                completed.result = Err(Failure::new(
                    ErrorCode::Stale,
                    "新标签已关闭；迟到结果未重新授权资源",
                ));
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
        let oldest_open = self
            .service_cancels
            .iter()
            .filter_map(|(request, service)| {
                matches!(service.binding, Some(PendingResource::OpenRepeater)).then_some(*request)
            })
            .min();
        // Retain close barriers only while an earlier resource-creating call can still complete.
        self.revoked_resources
            .retain(|_, barrier| oldest_open.is_some_and(|oldest| oldest <= *barrier));
        if let (Some(expected), Ok(ServiceResult::Resource { id, revision })) =
            (completed.resource_binding, &completed.result)
        {
            match self.broker.complete_resource(expected, *id, *revision) {
                Ok(()) => {
                    if self
                        .active_resource
                        .is_some_and(|(current, _)| current == *id)
                    {
                        self.active_resource = Some((*id, *revision));
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
        let mut envelope =
            self.envelope(envelope::Body::HostResult(result), completed.request_id)?;
        if let Err(error) = envelope.validate() {
            envelope.body = Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(Failure::new(
                    ErrorCode::Invalid,
                    format!("{error}；内容未截断，已执行的操作不会自动重试"),
                ))),
            }));
        }
        envelope.validate().map_err(anyhow::Error::msg)?;
        outgoing
            .try_send(envelope)
            .map_err(|_| anyhow::anyhow!("插件发送队列已满或 pipe 已关闭"))?;
        Ok(())
    }

    pub(super) async fn run(
        mut self,
        transport: WorkerTransport,
        mut commands: mpsc::Receiver<Control>,
    ) -> Result<()> {
        let (outgoing, writes) = mpsc::channel::<Envelope>(QUEUE_CAPACITY);
        let (wire, mut incoming) = mpsc::channel(QUEUE_CAPACITY);
        let io_cancel = CancellationToken::new();
        let mut io_tasks = JoinSet::new();
        let (mut native_child, mut wasm_worker) = match transport {
            WorkerTransport::Native {
                child,
                #[cfg(windows)]
                job,
                mut stdin,
                mut stdout,
                mut stderr,
            } => {
                let mut writes = writes;
                let reader_cancel = io_cancel.clone();
                let reader_events = wire.clone();
                io_tasks.spawn(async move {
        loop {
            let result = tokio::select! { biased; _ = reader_cancel.cancelled() => break, result = read_async_frame(&mut stdout) => result };
            let message = match result { Ok(frame) => Wire::Frame(Box::new(frame)), Err(_) => Wire::Failed("插件输出 pipe 关闭或 Protobuf 帧无效") };
            let failed = matches!(message, Wire::Failed(_));
            tokio::select! { biased; _ = reader_cancel.cancelled() => break, result = reader_events.send(message) => if result.is_err() { break; } }
            if failed { break; }
        }
    });
                let writer_cancel = io_cancel.clone();
                let writer_events = wire.clone();
                io_tasks.spawn(async move {
        loop {
            let envelope = tokio::select! { biased; _ = writer_cancel.cancelled() => break, next = writes.recv() => match next { Some(value) => value, None => break } };
            let result = async {
                stdin.write_all(&protocol::encode_frame(&envelope)?).await?;
                stdin.flush().await?;
                Ok::<_, anyhow::Error>(())
            };
            let result = tokio::select! { biased; _ = writer_cancel.cancelled() => break, result = timeout(MAX_OPERATION, result) => result };
            if !matches!(result, Ok(Ok(()))) {
                let _ = writer_events.try_send(Wire::Failed("插件输入 pipe 超时或关闭"));
                break;
            }
        }
    });
                let stderr_cancel = io_cancel.clone();
                let _ = wire;
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
                (Some(pair), None)
            }
            WorkerTransport::Wasm { module } => {
                let worker = wasm::spawn_sandbox_worker(
                    self.identity,
                    self.plugin_id.clone(),
                    module,
                    writes,
                    wire,
                    io_cancel.clone(),
                );
                (None, Some(worker))
            }
        };
        let result: Result<()> = async {
    self.publish(vec![]).await?;
    loop {
        let deadline = self.requests.values().map(|p| p.deadline).min().unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => break,
            _ = self.events.closed() => break,
            _ = tokio::time::sleep_until(deadline), if !self.requests.is_empty() => {
                let expired: Vec<_> = self.requests.iter().filter_map(|(id, p)| (p.deadline <= Instant::now()).then_some(*id)).collect();
                for id in expired { let _ = self.send(&outgoing, envelope::Body::Cancel(protocol::Cancel { request_id: id }), 0); }
                bail!("插件操作超时；已取消并回收实例，不会发送旧稿");
            }
            command = commands.recv() => match command { Some(command) => self.control(command, &outgoing).await?, None => break },
            frame = incoming.recv() => match frame {
                Some(Wire::Frame(frame)) => self.frame(*frame, &outgoing).await?,
                Some(Wire::Failed(message)) => bail!(message),
                None => bail!("插件 pipe 任务已退出"),
            },
            Some(completed) = self.service_tasks.join_next(), if !self.service_tasks.is_empty() => {
                self.complete_service(completed.context("插件宿主服务回调异常退出")?, &outgoing)?;
            }
        }
    }
    Ok(())
}.await;
        for service in self.service_cancels.values() {
            service.cancel.cancel();
        }
        let _ = self.send(&outgoing, envelope::Body::Shutdown(panel::Empty {}), 0);
        let reason = result
            .as_ref()
            .err()
            .map_or_else(|| "插件已停止".to_owned(), ToString::to_string);
        for (_, pending) in self.requests.drain() {
            match pending.kind {
                PendingKind::Open { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
                }
                PendingKind::Close { reply } => {
                    let _ = reply.send(Err(anyhow::anyhow!(reason.clone())));
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
        if let Some(worker) = wasm_worker.take() {
            let _ = timeout(REAP_DEADLINE, worker).await;
        }
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
