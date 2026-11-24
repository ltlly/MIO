use super::*;

impl AuditService {
    pub(in crate::service) fn has_in_flight_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub(in crate::service) fn has_resources(&self) -> bool {
        !self.active.is_empty() || !self.jobs.is_empty() || !self.pending_signals.is_empty()
    }

    pub(in crate::service) fn cancel_all(&mut self) {
        for job in self.active.values() {
            job.cancel.cancel();
        }
    }

    pub(super) fn stop_task(&mut self, project: ProjectKey, task_id: Uuid) -> bool {
        if let Some(job) = self.active.get(&task_id)
            && job.project == project
        {
            job.cancel.cancel();
            return true;
        }
        false
    }

    pub(super) fn set_task_paused(
        &mut self,
        project: ProjectKey,
        task_id: Uuid,
        paused: bool,
    ) -> bool {
        if let Some(job) = self.active.get(&task_id)
            && job.project == project
            && !job.cancel.is_cancelled()
        {
            let _ = job.pause_tx.send(paused);
            return true;
        }
        false
    }

    pub(super) fn start_scan(&mut self, plan: ScanPlan, cancel: CancellationToken) -> Result<()> {
        ensure!(
            !self.active.contains_key(&plan.task_id),
            "审计任务 ID 已在运行中"
        );
        ensure!(
            self.active.len() < MAX_CONCURRENT_SCANS,
            "并发审计任务已达 {MAX_CONCURRENT_SCANS} 个上限"
        );
        let project = plan.project;
        let task_id = plan.task_id;
        let job_cancel = cancel.child_token();
        let (pause_tx, pause_rx) = watch::channel(false);
        let signal_tx = self.signal_tx.clone();
        let abort = self
            .jobs
            .spawn(async move { run_scan(plan, job_cancel, pause_rx, signal_tx).await });
        let tokio_id = abort.id();
        self.active.insert(
            task_id,
            ActiveScanJob {
                project,
                task_id,
                tokio_id,
                cancel,
                pause_tx,
                abort,
                requests_sent: 0,
            },
        );
        Ok(())
    }

    pub(in crate::service) fn drop_project(&mut self, project: ProjectKey) {
        let ids: Vec<Uuid> = self
            .active
            .iter()
            .filter(|(_, job)| job.project == project)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(job) = self.active.remove(&id) {
                job.cancel.cancel();
                job.abort.abort();
            }
        }
        self.oast_probes.retain(|probe| probe.project != project);
        self.pending_signals.retain(|sig| sig.project != project);
        self.drain_channel(Some(project));
    }

    pub(in crate::service) async fn quiesce_project(&mut self, project: ProjectKey) -> Result<()> {
        for job in self.active.values() {
            if job.project == project {
                job.cancel.cancel();
            }
        }
        let result = self.join_all(TokioInstant::now() + STOP_DEADLINE).await;
        self.drain_channel(None);
        result
    }

    pub(in crate::service) async fn shutdown(&mut self) -> Result<()> {
        self.cancel_all();
        let result = self.join_all(TokioInstant::now() + STOP_DEADLINE).await;
        while self.signal_rx.try_recv().is_ok() {}
        self.pending_signals.clear();
        self.oast_probes.clear();
        result
    }

    pub(in crate::service) fn drain_pending_signals(
        &mut self,
        project: ProjectKey,
    ) -> Vec<AuditSignal> {
        self.drain_channel(None);
        let mut ready = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(signal) = self.pending_signals.pop_front() {
            if signal.project == project {
                ready.push(signal);
            } else {
                retained.push_back(signal);
            }
        }
        self.pending_signals = retained;
        ready
    }

    pub(in crate::service) async fn next_signal(&mut self) -> Option<AuditSignal> {
        loop {
            if let Some(signal) = self.pending_signals.pop_front() {
                return Some(signal);
            }
            if self.active.is_empty() && self.jobs.is_empty() {
                return std::future::pending().await;
            }
            tokio::select! {
                biased;
                Some(signal) = self.signal_rx.recv() => {
                    if self.accept_step(&signal) {
                        return Some(signal);
                    }
                }
                Some(output) = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    self.drain_channel(None);
                    if let Some(signal) = self.accept_job_done(output) {
                        self.pending_signals.push_back(signal);
                    }
                }
            }
        }
    }

    pub(in crate::service) async fn reclaim_signal(&mut self) {
        match self.jobs.join_next().await {
            Some(output) => {
                self.drain_channel(None);
                if let Some(signal) = self.accept_job_done(output) {
                    self.pending_signals.push_back(signal);
                }
            }
            None => std::future::pending().await,
        }
    }

    async fn join_all(&mut self, deadline: TokioInstant) -> Result<()> {
        let mut timed_out = false;
        while !self.jobs.is_empty() {
            match timeout_at(deadline, self.jobs.join_next()).await {
                Ok(Some(output)) => {
                    self.drain_channel(None);
                    if let Some(signal) = self.accept_job_done(output) {
                        self.pending_signals.push_back(signal);
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    timed_out = true;
                    break;
                }
            }
        }
        if timed_out {
            self.jobs.abort_all();
            let abort_deadline = TokioInstant::now() + Duration::from_millis(500);
            while !self.jobs.is_empty() {
                match timeout_at(abort_deadline, self.jobs.join_next()).await {
                    Ok(Some(output)) => {
                        self.drain_channel(None);
                        if let Some(signal) = self.accept_job_done(output) {
                            self.pending_signals.push_back(signal);
                        }
                    }
                    Ok(None) => break,
                    Err(_) => bail!("审计扫描后台任务回收超过期限"),
                }
            }
        }
        Ok(())
    }

    fn drain_channel(&mut self, drop_project: Option<ProjectKey>) {
        while let Ok(signal) = self.signal_rx.try_recv() {
            if drop_project.is_some_and(|p| signal.project == p) {
                continue;
            }
            if self.accept_step(&signal) {
                self.pending_signals.push_back(signal);
            }
        }
    }

    fn accept_step(&mut self, signal: &AuditSignal) -> bool {
        let Some(job) = self.active.get_mut(&signal.task_id) else {
            return false;
        };
        if job.project != signal.project {
            return false;
        }
        if let AuditSignalKind::Step { requests_sent, .. } = &signal.kind {
            job.requests_sent = job.requests_sent.max(*requests_sent);
        }
        true
    }

    fn accept_job_done(
        &mut self,
        output: std::result::Result<JobDone, JoinError>,
    ) -> Option<AuditSignal> {
        match output {
            Ok(done) => {
                let job = self.active.remove(&done.task_id)?;
                job.cancel.cancel();
                if job.project != done.project {
                    return None;
                }
                Some(AuditSignal {
                    project: done.project,
                    task_id: done.task_id,
                    kind: AuditSignalKind::Finished {
                        requests_sent: done.requests_sent.max(job.requests_sent),
                        state: done.state,
                        error: done.error,
                    },
                })
            }
            Err(error) => {
                let tokio_id = error.id();
                let task_id = self
                    .active
                    .iter()
                    .find_map(|(id, job)| (job.tokio_id == tokio_id).then_some(*id))?;
                let job = self.active.remove(&task_id)?;
                let was_cancelled = error.is_cancelled() || job.cancel.is_cancelled();
                job.cancel.cancel();
                let (state, msg) = if was_cancelled {
                    (AuditTaskState::Cancelled, None)
                } else {
                    (
                        AuditTaskState::Failed,
                        Some(format!("审计任务异常退出：{error}")),
                    )
                };
                Some(AuditSignal {
                    project: job.project,
                    task_id: job.task_id,
                    kind: AuditSignalKind::Finished {
                        requests_sent: job.requests_sent,
                        state,
                        error: msg,
                    },
                })
            }
        }
    }
}
