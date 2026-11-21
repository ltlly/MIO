use super::*;

const DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

impl State {
    pub(super) fn refresh_project_disk_bytes(&mut self) {
        self.refresh_project_disk_bytes_inner(true);
    }

    pub(super) fn refresh_project_disk_bytes_throttled(&mut self) {
        self.refresh_project_disk_bytes_inner(false);
    }

    fn refresh_project_disk_bytes_inner(&mut self, force: bool) {
        let now = Instant::now();
        if !force
            && let Some(prev) = self.last_disk_refresh
            && now.saturating_duration_since(prev) < DISK_REFRESH_INTERVAL
        {
            return;
        }
        self.last_disk_refresh = Some(now);
        if let Ok(bytes) = self.store.total_bytes() {
            self.project_disk_bytes = bytes;
            if self.events.capacity() > 4 {
                let _ = self.events.try_send(Event::ProjectDiskUsage {
                    project: self.project,
                    bytes,
                });
            }
        }
    }

    pub(super) fn flush_provenance_for(&mut self, flow_id: FlowId, terminal: bool) {
        let Some(mut prov) = self.provenance.get(flow_id) else {
            return;
        };
        if !terminal {
            let has_persisted_edit = self
                .store
                .original(flow_id)
                .ok()
                .is_some_and(|o| o.request.is_some() || o.response.is_some());
            if has_persisted_edit {
                let _ = self.store.put_flow_provenance(flow_id, &prov);
            }
            return;
        }
        if let Ok(Some(record)) = self.store.read_optional(flow_id) {
            if prov
                .request_steps
                .first()
                .is_some_and(|first| first.request == record.request)
                && self
                    .store
                    .original(flow_id)
                    .ok()
                    .and_then(|o| o.request)
                    .is_none()
            {
                prov.request_steps.clear();
            }
            if let Some(resp) = &record.response
                && prov
                    .response_steps
                    .first()
                    .is_some_and(|first| &first.response == resp)
                && self
                    .store
                    .original(flow_id)
                    .ok()
                    .and_then(|o| o.response)
                    .is_none()
            {
                prov.response_steps.clear();
            }
        }
        if self.store.put_flow_provenance(flow_id, &prov).is_ok() && terminal {
            self.provenance.remove(flow_id);
        } else {
            self.provenance.set(flow_id, prov);
        }
    }

    pub(super) fn flush_all_provenance_to_store(&mut self) {
        for (flow_id, prov) in self.provenance.snapshot_all() {
            if self.store.read_optional(flow_id).ok().flatten().is_some()
                && self.store.put_flow_provenance(flow_id, &prov).is_ok()
            {
                self.provenance.remove(flow_id);
            }
        }
    }

    pub(super) fn emit_audit_if_capacity(&mut self, event: Option<AuditEvent>) {
        let Some(event) = event else { return };
        if self.events.capacity() > 4 {
            let _ = self.events.try_send(Event::Audit(event));
        }
    }

    pub(super) fn require_ext_released(&self) -> Result<()> {
        ensure!(
            self.ext_archive.can_quiesce(),
            "外部导入或预检资源尚未回收；请先取消导入或关闭预检，等待清理完成后操作项目文件"
        );
        Ok(())
    }

    pub(super) fn require_saved(&self) -> Result<()> {
        ensure!(
            self.unsaved_count() == 0,
            "有 {} 条流量或消息变更尚未写盘；请保存或另存为后继续，网络请求不会自动重发",
            self.unsaved_count()
        );
        Ok(())
    }

    pub(super) fn persist_repeater_completion(
        &mut self,
        received: Option<&FlowRecord>,
        record: &FlowRecord,
    ) -> Result<()> {
        self.store
            .reject_session_intruder_flow_write(record.summary.id)?;
        let Some(received) = received else {
            return self.persist(record);
        };
        store::validate_repeater_completion(received, record)?;
        if let Some(pending) = self
            .recovery
            .iter()
            .rev()
            .find(|capture| capture.record.summary.id == record.summary.id)
            && pending.record.summary.revision >= record.summary.revision
        {
            ensure!(
                pending.record == *record
                    && pending.response_stream.is_none()
                    && self.original_flow(record.summary.id)?.response == received.response,
                "Repeater 待保存结果或原始响应冲突"
            );
            return Err(anyhow::anyhow!("先前流量仍待写盘；已停止接收新请求"));
        }
        let result = if self.unsaved_count() == 0 {
            self.store.put_repeater_completion(received, record)
        } else {
            Err(anyhow::anyhow!("先前流量仍待写盘；已停止接收新请求"))
        };
        if let Err(error) = &result {
            self.recovery.push_back(received.clone().into());
            self.recovery.push_back(record.clone().into());
            self.storage_failed(error);
        } else {
            self.flush_provenance_for(record.summary.id, true);
            self.refresh_project_disk_bytes_throttled();
        }
        result
    }

    pub(super) fn persist(&mut self, record: &FlowRecord) -> Result<()> {
        self.persist_capture(record, None, None)
    }

    pub(super) fn persist_capture(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
        listener_port: Option<u16>,
    ) -> Result<()> {
        self.persist_capture_with_metrics(record, response_stream, listener_port, &mio_proxy::CapturedFlowMetrics::default())
    }

    pub(super) fn persist_capture_with_metrics(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
        listener_port: Option<u16>,
        metrics: &mio_proxy::CapturedFlowMetrics,
    ) -> Result<()> {
        self.store
            .reject_session_intruder_flow_write(record.summary.id)?;
        let result = if self.unsaved_count() == 0 {
            self.store
                .put_captured_flow_with_metrics(record, response_stream, listener_port, metrics)
        } else {
            Err(anyhow::anyhow!("先前流量仍待写盘；已停止接收新请求"))
        };
        if let Err(error) = &result {
            self.recovery.push_back(CapturedFlow {
                record: record.clone(),
                response_stream: response_stream.cloned(),
                listener_port,
                capture_metrics: metrics.clone(),
            });
            self.storage_failed(error);
        } else {
            let terminal = !matches!(
                record.summary.state,
                FlowState::Pending | FlowState::Intercepted
            );
            self.flush_provenance_for(record.summary.id, terminal);
            self.refresh_project_disk_bytes_throttled();
        }
        result
    }

    pub(super) fn unsaved_count(&self) -> usize {
        self.recovery.len() + self.websocket_recovery.len() + self.intruder.recovery.len()
    }

    pub(super) fn storage_failed(&mut self, error: &anyhow::Error) {
        self.storage_error.get_or_insert_with(|| error.to_string());
        if let Some(proxy) = &self.proxy {
            proxy.request_stop();
        }
        for (_, proxy) in &self.extra_proxies {
            proxy.request_stop();
        }
        for send in self.sends.values_mut() {
            send.cancel.cancel();
            send.progress_rx = None;
        }
        self.ipc_job_cancel.cancel();
        self.plugins.cancel_network();
        self.intruder.cancel_all();
        self.ws_repeater.cancel_all();
        self.sequencer.cancel_all();
        self.audit.cancel_all();
        self.collaborator.cancel_all();
    }

    pub(super) fn read_flow(&self, id: FlowId) -> Result<FlowRecord> {
        if let Some(record) = self.unsaved_intruder_flow(id) {
            return Ok(record.clone());
        }
        if let Some(record) = self
            .recovery
            .iter()
            .rev()
            .find(|capture| capture.record.summary.id == id)
        {
            return Ok(record.record.clone());
        }
        self.store.read(id)
    }

    pub(super) fn original_flow(&self, id: FlowId) -> Result<OriginalMessages> {
        let mut original = self.store.original(id)?;
        if let Some(in_mem) = self.provenance.get(id)
            && !in_mem.is_empty()
        {
            if !in_mem.request_steps.is_empty() {
                original.provenance.request_steps = in_mem.request_steps;
            }
            if !in_mem.response_steps.is_empty() {
                original.provenance.response_steps = in_mem.response_steps;
            }
        }
        if !self
            .recovery
            .iter()
            .any(|capture| capture.record.summary.id == id)
        {
            if !original.provenance.is_empty()
                && let Ok(latest) = self.read_flow(id)
            {
                original.synthesize_provenance(Some(&latest.request), latest.response.as_ref());
            }
            return Ok(original);
        }
        let stored = self.store.read_optional(id)?;
        let mut previous = stored.as_ref();
        for capture in self
            .recovery
            .iter()
            .filter(|capture| capture.record.summary.id == id)
        {
            let record = &capture.record;
            if let Some(before) = previous {
                if original.request.is_none() && before.request != record.request {
                    original.request = Some(before.request.clone());
                }
                if capture.response_stream.is_none()
                    && original.response.is_none()
                    && before.response != record.response
                {
                    original.response = before.response.clone();
                }
            }
            if let Some(stream) = &capture.response_stream {
                stream.validate()?;
                let response = record.response.as_ref().context("流式响应缺少响应头")?;
                if stream.head_was_edited(response) || original.response.is_some() {
                    original.response = Some(HttpResponse {
                        status: stream.original_status,
                        version: stream.original_version,
                        reason_phrase: capture.capture_metrics.reason_phrase.clone(),
                        headers: stream.original_headers.clone(),
                        body: response.body.clone(),
                        trailers: response.trailers.clone(),
                    });
                    original.response_body_complete = Some(stream.body_complete);
                }
            }
            previous = Some(record);
        }
        if let Some(latest) = previous {
            original.synthesize_provenance(Some(&latest.request), latest.response.as_ref());
        }
        Ok(original)
    }

    pub(super) async fn storage_state(&mut self) {
        self.emit(Event::StorageState {
            project: self.project,
            pending_records: self.unsaved_count(),
            message: self.storage_error.clone(),
        })
        .await;
    }

    pub(super) async fn history_changed(&mut self) {
        self.refresh_project_disk_bytes_throttled();
        if !self.history_dirty {
            self.history_dirty = true;
            self.emit(Event::HistoryChanged {
                project: self.project,
            })
            .await;
        }
    }
}
