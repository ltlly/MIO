use super::*;

impl FlowsRepo<&mut ProjectStore> {
    #[cfg(test)]
    pub fn put(&mut self, record: &FlowRecord) -> Result<()> {
        self.put_capture(record, None)
    }

    pub fn put_repeater_completion(
        &mut self,
        received: &FlowRecord,
        record: &FlowRecord,
    ) -> Result<()> {
        self.0
            .intruder()
            .reject_session_flow_write(record.summary.id)?;
        let before_tc = query_total_changes(&self.0.connection)?;
        let tx = self.0.connection.transaction()?;
        write_repeater_completion(&tx, received, record)?;
        tx.commit()?;
        self.0.advance_workspace_cache_changes(before_tc)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn put_capture(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
    ) -> Result<()> {
        self.put_captured_flow(record, response_stream, None)
    }

    #[cfg(test)]
    pub(crate) fn put_captured_flow(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
        listener_port: Option<u16>,
    ) -> Result<()> {
        self.0
            .intruder()
            .reject_session_flow_write(record.summary.id)?;
        let before_tc = query_total_changes(&self.0.connection)?;
        let tx = self.0.connection.transaction()?;
        write_capture_with_port(&tx, record, response_stream, listener_port)?;
        tx.commit()?;
        self.0.advance_workspace_cache_changes(before_tc)?;
        Ok(())
    }

    pub(crate) fn put_captured_flow_with_metrics(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
        listener_port: Option<u16>,
        metrics: &mio_proxy::CapturedFlowMetrics,
    ) -> Result<()> {
        self.0
            .intruder()
            .reject_session_flow_write(record.summary.id)?;
        let before_tc = query_total_changes(&self.0.connection)?;
        let tx = self.0.connection.transaction()?;
        super::messages::write_capture_with_metrics(
            &tx,
            record,
            response_stream,
            listener_port,
            metrics,
        )?;
        tx.commit()?;
        self.0.advance_workspace_cache_changes(before_tc)?;
        Ok(())
    }

    pub fn finish_pending(&mut self, crashed: bool, only_proxy: bool) -> Result<usize> {
        let before_tc = query_total_changes(&self.0.connection)?;
        let tx = self.0.connection.transaction()?;
        let changed = finish_pending_records(&tx, crashed, only_proxy)?;
        tx.commit()?;
        self.0.advance_workspace_cache_changes(before_tc)?;
        Ok(changed)
    }
}

impl<S: std::ops::Deref<Target = ProjectStore>> FlowsRepo<S> {
    pub fn page(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
    ) -> Result<(usize, Vec<FlowSummary>)> {
        ensure!((1..=500).contains(&limit), "分页大小必须为 1–500");
        ensure!(filter.len() <= 4096, "筛选文本过长");
        let total: i64 = self.0.connection.query_row("SELECT count(*) FROM flows WHERE ?1='' OR instr(lower(url),lower(?1))>0 OR instr(lower(method),lower(?1))>0", [filter], |row| row.get(0))?;
        let mut statement = self.0.connection.prepare("SELECT summary FROM flows WHERE ?1='' OR instr(lower(url),lower(?1))>0 OR instr(lower(method),lower(?1))>0 ORDER BY started DESC,id LIMIT ?2 OFFSET ?3")?;
        let values = statement.query_map(
            params![filter, limit as i64, i64::try_from(offset)?],
            |row| row.get::<_, Vec<u8>>(0),
        )?;
        let mut rows = Vec::with_capacity(limit);
        for value in values {
            rows.push(serde_json::from_slice(&value?)?);
        }
        Ok((usize::try_from(total)?, rows))
    }

    pub(crate) fn flow_count(&self) -> Result<usize> {
        let total: i64 = self
            .0
            .connection
            .query_row("SELECT count(*) FROM flows", [], |row| row.get(0))?;
        Ok(usize::try_from(total)?)
    }

    pub(crate) fn has_flow(&self, id: FlowId) -> Result<bool> {
        let exists: bool = self.0.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM flows WHERE id=?1)",
            [id.to_string()],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub(crate) fn query_ai_flows(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
        method: Option<&str>,
        status: Option<u16>,
        recovery_map: &std::collections::HashMap<FlowId, FlowSummary>,
    ) -> Result<(usize, Vec<FlowSummary>)> {
        ensure!((1..=500).contains(&limit), "分页大小必须为 1–500");
        ensure!(filter.len() <= 4096, "筛选文本过长");
        let method = method.map(str::trim).filter(|m| !m.is_empty());
        if recovery_map.is_empty() && method.is_none() && status.is_none() {
            return self.page(offset, limit, filter);
        }
        let needle = filter.to_ascii_lowercase();
        let matches_summary = |s: &FlowSummary| -> bool {
            if !needle.is_empty()
                && !s.url.to_ascii_lowercase().contains(&needle)
                && !s.method.to_ascii_lowercase().contains(&needle)
            {
                return false;
            }
            if let Some(m) = method
                && !s.method.eq_ignore_ascii_case(m)
            {
                return false;
            }
            if let Some(code) = status
                && s.status != Some(code)
            {
                return false;
            }
            true
        };
        let mut combined: Vec<FlowSummary> = recovery_map
            .values()
            .filter(|s| matches_summary(s))
            .cloned()
            .collect();
        let mut stmt = self.0.connection.prepare(
            "SELECT id, summary FROM flows
             WHERE (?1='' OR instr(lower(url),lower(?1))>0 OR instr(lower(method),lower(?1))>0)
               AND (?2 IS NULL OR lower(method)=lower(?2))
             ORDER BY started DESC, id ASC",
        )?;
        let rows = stmt.query_map(params![filter, method], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (raw_id, bytes) = row?;
            let id = Uuid::parse_str(&raw_id).context("流量 ID 损坏")?;
            if recovery_map.contains_key(&id) {
                continue;
            }
            let summary: FlowSummary = serde_json::from_slice(&bytes)?;
            if matches_summary(&summary) {
                combined.push(summary);
            }
        }
        combined.sort_by(|a, b| {
            b.started_unix_ms
                .cmp(&a.started_unix_ms)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });
        let total = combined.len();
        let page = combined.into_iter().skip(offset).take(limit).collect();
        Ok((total, page))
    }

    pub fn read(&self, id: FlowId) -> Result<FlowRecord> {
        self.read_optional(id)?.context("流量不存在")
    }

    pub fn read_optional(&self, id: FlowId) -> Result<Option<FlowRecord>> {
        if let Some(record) = self.0.intruder().read_session_flow(id)? {
            return Ok(Some(record));
        }
        read_record(&self.0.connection, id)
    }

    pub fn original(&self, id: FlowId) -> Result<OriginalMessages> {
        if let Some(original) = self.0.intruder().original_session_flow(id)? {
            return Ok(original);
        }
        read_original(&self.0.connection, id)
    }
}

pub(super) fn put_message(
    connection: &Connection,
    id: &str,
    side: u8,
    head: &[u8],
    body: &[u8],
) -> Result<()> {
    // Keep the first pre-edit message, but do not duplicate unchanged bodies.
    connection.execute(
        "INSERT OR IGNORE INTO messages SELECT flow,?2+2,head,body FROM messages
        WHERE flow=?1 AND side=?2 AND (head!=?3 OR body!=?4)",
        params![id, side, head, body],
    )?;
    put_current_message(connection, id, side, head, body)
}

pub(super) fn put_current_message(
    connection: &Connection,
    id: &str,
    side: u8,
    head: &[u8],
    body: &[u8],
) -> Result<()> {
    ensure!(
        head.len() <= MAX_MESSAGE_HEAD_BYTES,
        "消息头编码超过 4 MiB 保存限额"
    );
    connection.execute(
        "INSERT INTO messages VALUES(?1,?2,?3,?4)
        ON CONFLICT(flow,side) DO UPDATE SET head=excluded.head,body=excluded.body
        WHERE messages.head!=excluded.head OR messages.body!=excluded.body",
        params![id, side, head, body],
    )?;
    Ok(())
}

pub(super) type MessageParts = (Vec<u8>, Vec<u8>);

pub(super) fn read_message(
    connection: &Connection,
    id: &str,
    side: u8,
) -> Result<Option<MessageParts>> {
    Ok(connection
        .query_row(
            "SELECT CASE WHEN typeof(head)='blob' AND length(head)<=?3 THEN head END,body FROM messages WHERE flow=?1 AND side=?2",
            params![id, side, MAX_MESSAGE_HEAD_BYTES],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

pub(super) fn request_from_parts((head, body): MessageParts) -> Result<HttpRequest> {
    let head: OwnedRequestHead = serde_json::from_slice(&head)?;
    Ok(HttpRequest {
        method: head.method,
        url: head.url,
        version: head.version,
        headers: head.headers,
        trailers: head.trailers,
        body,
    })
}

pub(super) fn response_from_parts((head, body): MessageParts) -> Result<HttpResponse> {
    Ok(response_with_stream((head, body))?.0)
}

pub(super) fn response_with_stream(
    (head, body): MessageParts,
) -> Result<(HttpResponse, Option<StreamedResponse>)> {
    let head: OwnedResponseHead = serde_json::from_slice(&head)?;
    if let Some(stream) = &head.stream {
        stream.validate()?;
    }
    Ok((
        HttpResponse {
            status: head.status,
            version: head.version,
            reason_phrase: head.reason_phrase,
            headers: head.headers,
            trailers: head.trailers,
            body,
        },
        head.stream,
    ))
}

pub(super) fn read_response_head(
    connection: &Connection,
    id: &str,
    side: u8,
) -> Result<Option<OwnedResponseHead>> {
    let bytes: Option<Vec<u8>> = connection.query_row(
        "SELECT CASE WHEN typeof(head)='blob' AND length(head)<=?3 THEN head END FROM messages WHERE flow=?1 AND side=?2",
        params![id, side, MAX_MESSAGE_HEAD_BYTES],
        |row| row.get(0),
    ).optional()?;
    bytes
        .map(|bytes| {
            let head: OwnedResponseHead = serde_json::from_slice(&bytes)?;
            if let Some(stream) = &head.stream {
                stream.validate()?;
            }
            Ok(head)
        })
        .transpose()
}

pub(super) fn read_record(connection: &Connection, flow_id: FlowId) -> Result<Option<FlowRecord>> {
    let id = flow_id.to_string();
    let summary: Option<Vec<u8>> = connection
        .query_row("SELECT summary FROM flows WHERE id=?1", [&id], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(summary) = summary else {
        return Ok(None);
    };
    let request = request_from_parts(read_message(connection, &id, 0)?.context("请求正文缺失")?)?;
    let response = read_message(connection, &id, 1)?
        .map(response_from_parts)
        .transpose()?;
    Ok(Some(FlowRecord {
        summary: serde_json::from_slice(&summary)?,
        request,
        response,
    }))
}

pub(super) fn read_original(connection: &Connection, flow_id: FlowId) -> Result<OriginalMessages> {
    let id = flow_id.to_string();
    let response = read_message(connection, &id, 3)?
        .map(response_with_stream)
        .transpose()?;
    let request = read_message(connection, &id, 2)?
        .map(request_from_parts)
        .transpose()?;
    let provenance = provenance::read_flow_provenance(connection, flow_id)?.unwrap_or_default();
    let mut original = OriginalMessages {
        request,
        response_body_complete: response
            .as_ref()
            .and_then(|(_, stream)| stream.as_ref().map(|stream| stream.body_complete)),
        response: response.map(|(response, _)| response),
        provenance,
    };
    if original.request.is_some() || original.response.is_some() || !original.provenance.is_empty()
    {
        let final_req = read_message(connection, &id, 0)?
            .map(request_from_parts)
            .transpose()?;
        let final_resp = read_message(connection, &id, 1)?
            .map(response_from_parts)
            .transpose()?;
        original.synthesize_provenance(final_req.as_ref(), final_resp.as_ref());
    }
    Ok(original)
}

pub(super) fn state_code(state: &crate::FlowState) -> &'static str {
    match state {
        crate::FlowState::Pending => "pending",
        crate::FlowState::Intercepted => "intercepted",
        crate::FlowState::Complete => "complete",
        crate::FlowState::Dropped => "dropped",
        crate::FlowState::Cancelled => "cancelled",
        crate::FlowState::Failed(_) => "failed",
    }
}
