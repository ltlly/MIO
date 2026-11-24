use super::*;

impl SequencerService {
    pub(super) fn capture_is_active(&self) -> bool {
        self.active_capture.is_some()
    }

    pub(super) fn stop_capture(&mut self, project: ProjectKey, run_id: Uuid) {
        if let Some(active) = self.active_capture.as_ref()
            && active.project == project
            && active.run_id == run_id
        {
            active.cancel.cancel();
        }
    }

    pub(super) async fn stop_active_capture(&mut self) {
        let Some(active) = self.active_capture.as_ref() else {
            return;
        };
        active.cancel.cancel();
        let deadline = TokioInstant::now() + JOB_WAIT_TIMEOUT;
        while self.active_capture.is_some() {
            match timeout_at(deadline, self.jobs.join_next()).await {
                Ok(Some(output)) => {
                    let events = self.accept_job(output);
                    self.pending_events.extend(events);
                }
                Ok(None) => break,
                Err(_) => {
                    if let Some(active) = self.active_capture.take() {
                        active.abort.abort();
                        self.pending_events.push_back(fallback_capture_finished(
                            active.project,
                            active.run_id,
                            "前序 Sequencer 采样回收超过期限",
                        ));
                    }
                    break;
                }
            }
        }
    }

    pub(super) fn start_capture(
        &mut self,
        plan: CapturePlan,
        cancel: CancellationToken,
    ) -> Result<()> {
        ensure!(
            self.active_capture.is_none(),
            "已有 Sequencer 采样任务正在运行"
        );
        let job_id = Uuid::new_v4();
        let project = plan.project;
        let run_id = plan.run_id;
        let job_cancel = cancel.child_token();
        let progress_tx = self.progress_tx.clone();
        let handle = self.jobs.spawn(async move {
            let event = run_capture(plan, job_cancel, progress_tx).await;
            JobOutput::Capture { job_id, event }
        });
        let task_id = handle.id();
        self.active_capture = Some(ActiveCapture {
            job_id,
            project,
            run_id,
            task_id,
            cancel,
            abort: handle,
        });
        Ok(())
    }

    pub(super) fn start_analysis(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        samples: Vec<String>,
        cancel: CancellationToken,
    ) -> Result<()> {
        ensure!(
            !operation_id.is_nil(),
            "Sequencer 统计分析缺少有效 operation ID"
        );
        ensure!(!cancel.is_cancelled(), "Sequencer 统计分析已取消");
        let job_id = Uuid::new_v4();
        let output_job_id = job_id;
        let job_cancel = cancel.child_token();
        let handle = self.jobs.spawn(async move {
            let mut worker = tokio::task::spawn_blocking(move || analyze_tokens(&samples));
            let mut was_cancelled = false;
            let result = tokio::select! {
                biased;
                _ = job_cancel.cancelled() => {
                    was_cancelled = true;
                    Err("Sequencer 统计分析已取消".to_owned())
                }
                result = &mut worker => match result {
                    Ok(Ok(analysis)) => Ok(Box::new(analysis)),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(error) => Err(format!("Sequencer 统计分析任务异常结束：{error}")),
                },
            };
            if was_cancelled {
                let _ = timeout_at(TokioInstant::now() + STOP_DEADLINE, worker).await;
            }
            JobOutput::Analysis {
                job_id: output_job_id,
                event: SequencerEvent::AnalysisReady {
                    project,
                    operation_id,
                    result,
                },
            }
        });
        let task_id = handle.id();
        self.analyses.insert(
            job_id,
            AnalysisJob {
                project,
                operation_id,
                task_id,
                cancel,
                abort: handle,
            },
        );
        Ok(())
    }

    pub(in crate::service) fn drain_pending_events(
        &mut self,
        project: ProjectKey,
    ) -> Vec<SequencerEvent> {
        let mut events = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(event) = self.pending_events.pop_front() {
            if event.project() == project {
                events.push(event);
            } else {
                retained.push_back(event);
            }
        }
        self.pending_events = retained;
        events
    }

    pub(in crate::service) fn cancel_all(&mut self) {
        if let Some(active) = self.active_capture.as_ref() {
            active.cancel.cancel();
        }
        for analysis in self.analyses.values() {
            analysis.cancel.cancel();
        }
    }

    pub(in crate::service) fn drop_project(&mut self, project: ProjectKey) {
        if self
            .active_capture
            .as_ref()
            .is_some_and(|active| active.project == project)
            && let Some(active) = self.active_capture.take()
        {
            active.cancel.cancel();
            active.abort.abort();
        }
        let stale_ids = self
            .analyses
            .iter()
            .filter(|(_, analysis)| analysis.project == project)
            .map(|(job_id, _)| *job_id)
            .collect::<Vec<_>>();
        for job_id in stale_ids {
            if let Some(analysis) = self.analyses.remove(&job_id) {
                analysis.cancel.cancel();
                analysis.abort.abort();
            }
        }
        self.pending_events
            .retain(|event| event.project() != project);
        self.drain_progress_channel(Some(project));
    }

    pub(in crate::service) async fn quiesce_project(&mut self, project: ProjectKey) -> Result<()> {
        if let Some(active) = self.active_capture.as_ref()
            && active.project == project
        {
            active.cancel.cancel();
        }
        for analysis in self.analyses.values() {
            if analysis.project == project {
                analysis.cancel.cancel();
            }
        }
        let result = self.join_all(TokioInstant::now() + JOB_WAIT_TIMEOUT).await;
        self.drain_progress_channel(Some(project));
        result
    }

    pub(in crate::service) async fn shutdown(&mut self) -> Result<()> {
        self.cancel_all();
        let result = self.join_all(TokioInstant::now() + JOB_WAIT_TIMEOUT).await;
        while self.progress_rx.try_recv().is_ok() {}
        self.pending_events.clear();
        result
    }

    pub(in crate::service) fn has_resources(&self) -> bool {
        self.active_capture.is_some() || !self.analyses.is_empty() || !self.jobs.is_empty()
    }

    pub(in crate::service) fn has_in_flight_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    pub(in crate::service) async fn next_signal(&mut self) -> Option<SequencerEvent> {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                return Some(event);
            }
            tokio::select! {
                biased;
                Some(output) = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    let events = self.accept_job(output);
                    self.pending_events.extend(events);
                }
                Some(progress) = self.progress_rx.recv() => {
                    if let Some(event) = self.accept_progress(progress) {
                        self.pending_events.push_back(event);
                    }
                }
            }
        }
    }

    pub(in crate::service) async fn reclaim_signal(&mut self) {
        match self.jobs.join_next().await {
            Some(output) => {
                let events = self.accept_job(output);
                self.pending_events.extend(events);
            }
            None => std::future::pending().await,
        }
    }

    async fn join_all(&mut self, deadline: TokioInstant) -> Result<()> {
        let mut timed_out = false;
        while !self.jobs.is_empty() {
            match timeout_at(deadline, self.jobs.join_next()).await {
                Ok(Some(output)) => {
                    let events = self.accept_job(output);
                    self.pending_events.extend(events);
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
                        let events = self.accept_job(output);
                        self.pending_events.extend(events);
                    }
                    Ok(None) => break,
                    Err(_) => bail!("Sequencer 后台任务回收超过期限"),
                }
            }
        }
        Ok(())
    }

    fn drain_progress_channel(&mut self, drop_project: Option<ProjectKey>) {
        while let Ok(progress) = self.progress_rx.try_recv() {
            if drop_project.is_some_and(|project| progress.project() == project) {
                continue;
            }
            if let Some(event) = self.accept_progress(progress) {
                self.pending_events.push_back(event);
            }
        }
    }

    fn accept_progress(&self, event: SequencerEvent) -> Option<SequencerEvent> {
        let SequencerEvent::CaptureProgress {
            project, run_id, ..
        } = &event
        else {
            return Some(event);
        };
        self.active_capture
            .as_ref()
            .is_some_and(|active| active.project == *project && active.run_id == *run_id)
            .then_some(event)
    }

    fn accept_job(
        &mut self,
        output: std::result::Result<JobOutput, JoinError>,
    ) -> Vec<SequencerEvent> {
        match output {
            Ok(JobOutput::Capture { job_id, event }) => {
                if self
                    .active_capture
                    .as_ref()
                    .is_some_and(|active| active.job_id == job_id)
                {
                    self.active_capture = None;
                    self.pending_events.retain(|queued| {
                        !matches!(
                            queued,
                            SequencerEvent::CaptureProgress { project, run_id, .. }
                                if *project == event.project()
                                    && matches!(&event, SequencerEvent::CaptureFinished { run_id: finished_id, .. } if finished_id == run_id)
                        )
                    });
                    vec![event]
                } else {
                    Vec::new()
                }
            }
            Ok(JobOutput::Analysis { job_id, event }) => {
                if self.analyses.remove(&job_id).is_some() {
                    vec![event]
                } else {
                    Vec::new()
                }
            }
            Err(error) => {
                let task_id = error.id();
                if self
                    .active_capture
                    .as_ref()
                    .is_some_and(|active| active.task_id == task_id)
                    && let Some(active) = self.active_capture.take()
                {
                    return vec![fallback_capture_finished(
                        active.project,
                        active.run_id,
                        &format!("Sequencer 采样任务异常结束：{error}"),
                    )];
                }
                let mut failed_analysis = None;
                self.analyses.retain(|_, analysis| {
                    if analysis.task_id == task_id {
                        failed_analysis = Some((analysis.project, analysis.operation_id));
                        false
                    } else {
                        true
                    }
                });
                if let Some((project, operation_id)) = failed_analysis {
                    return vec![SequencerEvent::AnalysisReady {
                        project,
                        operation_id,
                        result: Err(format!("Sequencer 统计分析任务异常结束：{error}")),
                    }];
                }
                Vec::new()
            }
        }
    }
}
