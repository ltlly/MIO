use super::*;

impl State {
    pub(super) async fn failure(&self, request: HttpRequest, error: &ProxyError) {
        let mut record = new_record(request);
        record.summary.state = FlowState::Failed(error.to_string());
        let _ = self
            .emit(ProxyEvent::Captured {
                project: self.project,
                record,
                response_stream: None,
                listener_port: (self.listener_port != 0).then_some(self.listener_port),
                capture_metrics: CapturedFlowMetrics::default(),
            })
            .await;
    }

    pub(super) async fn event_slot(&self) -> Result<mpsc::Permit<'_, ProxyEvent>, ProxyError> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => self.events.try_reserve().map_err(|_| ProxyError::Cancelled),
            permit = self.events.reserve() => permit.map_err(|_| {
                self.cancel.cancel();
                ProxyError::EventConsumerClosed
            }),
        }
    }

    pub(super) async fn terminal(
        &self,
        record: &mut Option<FlowRecord>,
        response_stream: &mut Option<StreamedResponse>,
        capture_metrics: &CapturedFlowMetrics,
        permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) {
        let slot = self.event_slot().await;
        let record = record.take().expect("active flow owns its terminal record");
        let response_stream = response_stream.take();
        if let Ok(slot) = slot {
            slot.send(ProxyEvent::Captured {
                project: self.project,
                record,
                response_stream,
                capture_metrics: capture_metrics.clone(),
                listener_port: (self.listener_port != 0).then_some(self.listener_port),
            });
        } else {
            self.retain_terminal(
                CapturedFlow {
                    record,
                    response_stream,
                    capture_metrics: capture_metrics.clone(),
                    listener_port: (self.listener_port != 0).then_some(self.listener_port),
                },
                permit,
            );
        }
    }

    pub(super) fn retain_terminal(
        &self,
        capture: CapturedFlow,
        permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) {
        let mut records = crate::lock_unpoison(&self.terminal_records);
        records.push(TerminalRecord {
            capture,
            _permit: permit,
        });
    }

    pub(super) async fn emit(&self, event: ProxyEvent) -> Result<(), ProxyError> {
        self.event_slot().await?.send(event);
        Ok(())
    }
}
