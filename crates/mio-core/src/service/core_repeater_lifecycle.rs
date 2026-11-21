use super::*;

pub(super) async fn next_repeater_progress(
    sends: &mut HashMap<TabId, ActiveSend>,
    events: &mpsc::Sender<Event>,
    can_emit: bool,
) -> TabId {
    if !can_emit {
        return std::future::pending().await;
    }
    for (&tab_id, active) in sends.iter() {
        if active
            .progress_rx
            .as_ref()
            .is_some_and(|rx| rx.has_changed().unwrap_or(false))
        {
            if events.capacity() <= 4 {
                match events.reserve_many(5).await {
                    Ok(permits) => drop(permits),
                    Err(_) => return std::future::pending().await,
                }
            }
            return tab_id;
        }
    }
    let mut receivers: Vec<(TabId, watch::Receiver<Option<ProgressPayload>>)> = sends
        .iter()
        .filter_map(|(&tab_id, active)| Some((tab_id, active.progress_rx.as_ref()?.clone())))
        .collect();
    if receivers.is_empty() {
        return std::future::pending().await;
    }
    let mut futures: Vec<_> = receivers
        .iter_mut()
        .map(|(tab_id, rx)| (*tab_id, Box::pin(rx.changed())))
        .collect();
    let tab_id = std::future::poll_fn(|cx| {
        for (tab_id, fut) in &mut futures {
            if let std::task::Poll::Ready(Ok(())) = fut.as_mut().poll(cx) {
                return std::task::Poll::Ready(*tab_id);
            }
        }
        std::task::Poll::Pending
    })
    .await;
    if events.capacity() <= 4 {
        match events.reserve_many(5).await {
            Ok(permits) => drop(permits),
            Err(_) => return std::future::pending().await,
        }
    }
    tab_id
}

impl State {
    pub(super) async fn repeater_completed(
        &mut self,
        result: std::result::Result<Completion, tokio::task::JoinError>,
    ) {
        match result {
            Ok(completion) => {
                if let Err(error) = self.complete(completion).await {
                    self.queue_repeater_event(Event::Error {
                        project: Some(self.project),
                        operation: "send",
                        message: error.to_string(),
                    });
                }
            }
            Err(error) => self.failed_job(error).await,
        }
    }

    pub(super) fn emit_repeater_progress(&mut self, tab_id: TabId) {
        if !self.repeater.notifications.is_empty() || self.events.capacity() <= 4 {
            return;
        }
        let Some(active) = self.repeater.sends.get_mut(&tab_id) else {
            return;
        };
        let Some(rx) = &mut active.progress_rx else {
            return;
        };
        if let Some((response, elapsed_ms)) = rx.borrow_and_update().clone() {
            let _ = self.events.try_send(Event::SendProgress {
                project: self.project,
                tab_id,
                run_id: active.flow_id,
                draft_revision: active.draft_revision,
                response: Box::new(response),
                elapsed_ms,
            });
        }
    }

    pub(super) async fn complete(&mut self, completion: Completion) -> Result<()> {
        if completion.project != self.project
            || self.repeater.sends.get(&completion.tab_id).is_none_or(|active| {
                active.flow_id != completion.record.summary.id
                    || active.draft_revision != completion.draft_revision
            })
        {
            return Ok(());
        }
        self.repeater.sends.remove(&completion.tab_id);
        let saved =
            self.persist_repeater_completion(completion.received.as_deref(), &completion.record);
        let audit_event = saved
            .is_ok()
            .then(|| self.audit_inspect_live_flow(&completion.record))
            .flatten();
        if saved.is_ok() {
            self.spawn_plugin_passive_checks(&completion.record);
        }
        self.queue_repeater_event(Event::SendFinished {
            project: completion.project,
            tab_id: completion.tab_id,
            draft_revision: completion.draft_revision,
            record: Box::new(completion.record),
            persisted: saved.is_ok(),
        });
        if !self.storage.history_dirty {
            self.storage.history_dirty = true;
            self.queue_repeater_event(Event::HistoryChanged {
                project: self.project,
            });
        }
        if let Some(event) = audit_event {
            self.queue_repeater_event(Event::Audit(event));
        }
        if saved.is_ok() {
            self.refresh_project_disk_bytes_throttled();
        }
        saved
    }

    pub(super) async fn failed_job(&mut self, error: tokio::task::JoinError) {
        let Some(tab_id) = self
            .repeater
            .sends
            .iter()
            .find_map(|(id, active)| (active.task_id == error.id()).then_some(*id))
        else {
            self.queue_repeater_event(Event::Error {
                project: Some(self.project),
                operation: "send",
                message: format!("发送任务异常结束：{error}"),
            });
            return;
        };
        let active = self.repeater.sends.get(&tab_id).unwrap();
        active.cancel.cancel();
        let (flow_id, draft_revision) = (active.flow_id, active.draft_revision);
        let message = format!("发送任务异常结束，结果未知，未自动重发：{error}");
        match self.read_flow(flow_id) {
            Ok(mut record) => {
                record.summary.revision += 1;
                record.summary.state = FlowState::Failed(message);
                let completion = Completion {
                    project: self.project,
                    tab_id,
                    draft_revision,
                    received: None,
                    record,
                };
                if let Err(error) = self.complete(completion).await {
                    self.queue_repeater_event(Event::Error {
                        project: Some(self.project),
                        operation: "send",
                        message: error.to_string(),
                    });
                }
            }
            Err(error) => {
                self.repeater.sends.remove(&tab_id);
                self.queue_repeater_event(Event::SendRejected {
                    project: self.project,
                    tab_id,
                    run_id: flow_id,
                    draft_revision,
                    message: format!("{message}；读取流量失败：{error}"),
                })
            }
        }
    }

    pub(super) async fn stop_repeater_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.repeater.cancel_all();
        let mut send_error = None;
        while !self.repeater.jobs.is_empty() {
            match tokio::time::timeout_at(deadline, self.repeater.jobs.join_next()).await {
                Ok(Some(Ok(completion))) => {
                    if let Err(error) = self.complete(completion).await {
                        send_error.get_or_insert(error);
                    }
                }
                Ok(Some(Err(error))) => self.failed_job(error).await,
                Ok(None) => break,
                Err(_) => anyhow::bail!(
                    "Repeater 仍在回收；保留任务句柄，未确认停止或切换项目，请稍后重试"
                ),
            }
        }
        send_error.map_or(Ok(()), Err)
    }

    pub(super) async fn finish_repeater_shutdown(&mut self) -> Result<()> {
        // The final owner must join the same send futures even after Stop's deadline.
        let mut failure = None;
        while !self.repeater.jobs.is_empty() {
            if let Err(error) = self
                .stop_repeater_until(tokio::time::Instant::now() + Duration::from_secs(3))
                .await
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
