use super::*;

impl State {
    pub(crate) fn capture_websocket(
        &mut self,
        project: ProjectKey,
        message: CapturedWebSocket,
    ) -> Result<()> {
        if project != self.project {
            return Ok(());
        }
        let saved = if self.unsaved_count() == 0 {
            self.store.put_captured_websocket(&message)
        } else {
            Err(anyhow::anyhow!("先前流量仍待写盘；已停止接收新消息"))
        };
        if let Err(error) = &saved {
            self.websocket_recovery.push_back(message);
            self.storage_failed(error);
        }
        if !self.websocket_history_dirty {
            self.websocket_history_dirty = true;
            self.queue_proxy_notification(Event::WebSocketHistoryChanged {
                project: self.project,
            });
        }
        saved
    }

    pub(crate) async fn websocket_history_changed(&mut self) {
        if !self.websocket_history_dirty {
            self.websocket_history_dirty = true;
            self.emit(Event::WebSocketHistoryChanged {
                project: self.project,
            })
            .await;
        }
    }

    #[cfg(test)]
    pub(crate) fn read_websocket(&self, id: Uuid) -> Result<WebSocketMessage> {
        if let Some(record) = self
            .websocket_recovery
            .iter()
            .rev()
            .find(|capture| capture.record.id == id)
        {
            return Ok(record.record.as_ref().clone());
        }
        self.store.read_websocket(id)
    }

    pub(crate) fn read_websocket_detail(
        &self,
        id: Uuid,
    ) -> Result<(WebSocketMessage, Option<Vec<u8>>)> {
        if let Some(capture) = self
            .websocket_recovery
            .iter()
            .rev()
            .find(|capture| capture.record.id == id)
        {
            let original = capture.edited_original().map(|original| original.to_vec());
            return Ok((capture.record.as_ref().clone(), original));
        }
        self.store.read_websocket_detail(id)
    }

    #[cfg(test)]
    pub(crate) fn websocket_page(
        &self,
        flow_id: Option<FlowId>,
        offset: usize,
        limit: usize,
    ) -> Result<(usize, Vec<WebSocketSummary>)> {
        self.websocket_page_with_filter(flow_id, offset, limit, &WebSocketHistoryFilter::default())
    }

    #[cfg(test)]
    pub(crate) fn websocket_page_with_filter(
        &self,
        flow_id: Option<FlowId>,
        offset: usize,
        limit: usize,
        filter: &WebSocketHistoryFilter,
    ) -> Result<(usize, Vec<WebSocketSummary>)> {
        ensure!((1..=100_000).contains(&limit), "分页大小必须为 1–100000");
        ensure!(filter.listener_port != Some(0), "监听端口必须为 1–65535");
        if !filter.filter_script.trim().is_empty() {
            crate::http::validate_filter_script(&filter.filter_script)?;
        }
        let pending = self.websocket_recovery_snapshot()?;
        let rows: Vec<_> = pending.into_iter().map(|(row, _)| row).collect();
        self.store
            .websocket_page_with_recovery(flow_id, offset, limit, &[], &rows, filter)
    }

    pub(crate) fn websocket_recovery_snapshot(
        &self,
    ) -> Result<Vec<(WebSocketSummary, CapturedWebSocket)>> {
        // Faults stop producers; only the bounded queue can append unsaved messages.
        let mut pending = Vec::new();
        let mut positions = HashMap::new();
        for capture in &self.websocket_recovery {
            if let Some(index) = positions.get(&capture.record.id) {
                pending[*index] = capture;
            } else {
                positions.insert(capture.record.id, pending.len());
                pending.push(capture);
            }
        }
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        let (mut next_ordinal, mut next_connection) = self.store.websocket_numbers()?;
        let mut connections = HashMap::new();
        let mut rows = Vec::with_capacity(pending.len());
        // Save As copies sqlite_sequence then writes this queue in the same order.
        // Allocate before filtering so a selected connection never renumbers pending rows.
        for capture in pending {
            let record = &capture.record;
            let ordinal = if let Some(ordinal) = self.store.websocket_ordinal(record.id)? {
                ordinal
            } else {
                let ordinal = next_ordinal;
                next_ordinal += 1;
                ordinal
            };
            let (connection_id, listener_port) =
                if let Some(metadata) = connections.get(&record.flow_id) {
                    *metadata
                } else {
                    let metadata =
                        if let Some(metadata) = self.store.websocket_connection(record.flow_id)? {
                            metadata
                        } else {
                            let id = next_connection;
                            next_connection += 1;
                            (id, capture.listener_port)
                        };
                    connections.insert(record.flow_id, metadata);
                    metadata
                };
            let url = self
                .recovery
                .iter()
                .rev()
                .find(|flow| flow.record.summary.id == record.flow_id)
                .map(|flow| Ok(flow.record.summary.url.clone()))
                .unwrap_or_else(|| self.store.flow_url(record.flow_id))?;
            rows.push((
                WebSocketSummary::from_message(
                    record,
                    url,
                    ordinal,
                    connection_id,
                    listener_port,
                    capture.edited_original().is_some(),
                ),
                capture.clone(),
            ));
        }
        Ok(rows)
    }
}
