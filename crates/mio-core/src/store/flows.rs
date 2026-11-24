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

    fn load_numbered_ai_flows(
        &self,
        recovery_map: &std::collections::HashMap<FlowId, FlowSummary>,
    ) -> Result<Vec<(Option<u64>, FlowSummary)>> {
        let mut rows_out: Vec<(Option<u64>, FlowSummary)> = Vec::new();
        let mut stmt = self
            .0
            .connection
            .prepare("SELECT id, summary, capture_number FROM flows")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (raw_id, bytes, raw_num) = row?;
            let id = Uuid::parse_str(&raw_id).context("流量 ID 损坏")?;
            let num = raw_num.and_then(|n| u64::try_from(n).ok()).filter(|&n| n > 0);
            if let Some(recovered) = recovery_map.get(&id) {
                rows_out.push((num, recovered.clone()));
            } else {
                let summary: FlowSummary = serde_json::from_slice(&bytes)?;
                rows_out.push((num, summary));
            }
        }
        let seen: std::collections::HashSet<FlowId> =
            rows_out.iter().map(|(_, s)| s.id).collect();
        for summary in recovery_map.values() {
            if !seen.contains(&summary.id) {
                rows_out.push((None, summary.clone()));
            }
        }
        let mut next_num = rows_out
            .iter()
            .filter_map(|(n, _)| *n)
            .max()
            .unwrap_or(0);
        // Assign deterministic monotonic 1-based numbers to flows without a stored capture_number.
        let mut unnumbered_indices: Vec<usize> = rows_out
            .iter()
            .enumerate()
            .filter_map(|(idx, (n, _))| n.is_none().then_some(idx))
            .collect();
        unnumbered_indices.sort_by(|&a, &b| {
            let sa = &rows_out[a].1;
            let sb = &rows_out[b].1;
            sa.started_unix_ms
                .cmp(&sb.started_unix_ms)
                .then_with(|| sa.id.to_string().cmp(&sb.id.to_string()))
        });
        for idx in unnumbered_indices {
            next_num = next_num.saturating_add(1);
            rows_out[idx].0 = Some(next_num);
        }
        Ok(rows_out)
    }

    pub(crate) fn flow_number_for(
        &self,
        id: FlowId,
        recovery_map: &std::collections::HashMap<FlowId, FlowSummary>,
    ) -> Result<Option<u64>> {
        let stored: Option<Option<i64>> = self
            .0
            .connection
            .query_row(
                "SELECT capture_number FROM flows WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(Some(num)) = stored
            && let Ok(n) = u64::try_from(num)
            && n > 0
        {
            return Ok(Some(n));
        }
        let all = self.load_numbered_ai_flows(recovery_map)?;
        Ok(all
            .into_iter()
            .find(|(_, s)| s.id == id)
            .and_then(|(num, _)| num))
    }

    pub(crate) fn resolve_flow_by_number(
        &self,
        flow_number: u64,
        recovery_map: &std::collections::HashMap<FlowId, FlowSummary>,
    ) -> Result<Option<FlowId>> {
        if flow_number == 0 {
            return Ok(None);
        }
        if let Ok(num_i64) = i64::try_from(flow_number)
            && let Some(raw_id) = self
                .0
                .connection
                .query_row(
                    "SELECT id FROM flows WHERE capture_number=?1 LIMIT 1",
                    [num_i64],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        {
            return Ok(Some(Uuid::parse_str(&raw_id).context("流量 ID 损坏")?));
        }
        let all = self.load_numbered_ai_flows(recovery_map)?;
        Ok(all
            .into_iter()
            .find(|(num, _)| *num == Some(flow_number))
            .map(|(_, s)| s.id))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn query_ai_flows(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
        method: Option<&str>,
        status: Option<u16>,
        hide_connect: bool,
        host: Option<&str>,
        source: Option<&str>,
        state: Option<&str>,
        order: Option<&str>,
        recovery_map: &std::collections::HashMap<FlowId, FlowSummary>,
    ) -> Result<(usize, Vec<crate::AiIpcFlowItem>)> {
        ensure!((1..=500).contains(&limit), "分页大小必须为 1–500");
        ensure!(filter.len() <= 4096, "筛选文本过长");
        let method = method.map(str::trim).filter(|m| !m.is_empty());
        let host_filter = host
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_ascii_lowercase);
        let source_filter = match source.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => Some(match raw.to_ascii_lowercase().as_str() {
                "proxy" => crate::TrafficSource::Proxy,
                "repeater" => crate::TrafficSource::Repeater,
                "intruder" => crate::TrafficSource::Intruder,
                "scanner" => crate::TrafficSource::Scanner,
                "plugin" => crate::TrafficSource::Plugin(String::new()),
                other => bail!(
                    "无效的 --source 筛选值 `{other}`（支持：proxy、repeater、intruder、scanner、plugin）"
                ),
            }),
            None => None,
        };
        let state_filter = match state.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                let norm = raw.to_ascii_lowercase();
                ensure!(
                    matches!(
                        norm.as_str(),
                        "complete"
                            | "pending"
                            | "intercepted"
                            | "dropped"
                            | "cancelled"
                            | "failed"
                            | "error"
                            | "abnormal"
                    ),
                    "无效的 --state 筛选值 `{raw}`（支持：complete、pending、intercepted、dropped、cancelled、failed、abnormal）"
                );
                Some(norm)
            }
            None => None,
        };
        let ascending = match order.map(str::trim).filter(|o| !o.is_empty()) {
            Some(raw) if raw.eq_ignore_ascii_case("asc") => true,
            Some(raw) if raw.eq_ignore_ascii_case("desc") => false,
            Some(other) => bail!("无效的 --order 排序值 `{other}`（支持：asc、desc）"),
            None => false,
        };
        let needle = filter.to_ascii_lowercase();
        let all = self.load_numbered_ai_flows(recovery_map)?;
        let mut combined: Vec<crate::AiIpcFlowItem> = all
            .into_iter()
            .filter(|(_, s)| {
                if hide_connect && s.method.eq_ignore_ascii_case("CONNECT") {
                    return false;
                }
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
                if let Some(want_host) = &host_filter
                    && !flow_matches_host(&s.url, want_host)
                {
                    return false;
                }
                if let Some(want_src) = &source_filter {
                    let matched = match (want_src, &s.source) {
                        (crate::TrafficSource::Plugin(_), crate::TrafficSource::Plugin(_)) => true,
                        _ => want_src == &s.source,
                    };
                    if !matched {
                        return false;
                    }
                }
                if let Some(want_state) = &state_filter
                    && !flow_matches_state(&s.state, want_state)
                {
                    return false;
                }
                true
            })
            .map(|(number, summary)| crate::AiIpcFlowItem { number, summary })
            .collect();
        combined.sort_by(|a, b| {
            let ord = a
                .summary
                .started_unix_ms
                .cmp(&b.summary.started_unix_ms)
                .then_with(|| a.number.cmp(&b.number))
                .then_with(|| a.summary.id.to_string().cmp(&b.summary.id.to_string()));
            if ascending { ord } else { ord.reverse() }
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

fn flow_matches_host(url: &str, want_host_lower: &str) -> bool {
    if let Ok(parsed) = url::Url::parse(url)
        && let Some(host) = parsed.host_str()
    {
        let host_lo = host.to_ascii_lowercase();
        if host_lo == want_host_lower || host_lo.contains(want_host_lower) {
            return true;
        }
        if let Some(port) = parsed.port() {
            let host_port = format!("{host_lo}:{port}");
            if host_port == want_host_lower || host_port.contains(want_host_lower) {
                return true;
            }
        }
        return false;
    }
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    authority == want_host_lower || authority.contains(want_host_lower)
}

fn flow_matches_state(state: &crate::FlowState, want_state_lower: &str) -> bool {
    match want_state_lower {
        "complete" => matches!(state, crate::FlowState::Complete),
        "pending" => matches!(state, crate::FlowState::Pending),
        "intercepted" => matches!(state, crate::FlowState::Intercepted),
        "dropped" => matches!(state, crate::FlowState::Dropped),
        "cancelled" => matches!(state, crate::FlowState::Cancelled),
        "failed" | "error" => matches!(state, crate::FlowState::Failed(_)),
        "abnormal" => matches!(
            state,
            crate::FlowState::Failed(_) | crate::FlowState::Dropped | crate::FlowState::Cancelled
        ),
        _ => false,
    }
}
