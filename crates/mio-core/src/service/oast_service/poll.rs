use base64::Engine as _;

use super::*;

#[derive(Deserialize)]
struct RemotePollEnvelope {
    #[serde(default)]
    interactions: Vec<RemotePollItem>,
    #[serde(default, alias = "cursor")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawRequestWire {
    Bytes(Vec<u8>),
    Text(String),
}

#[derive(Deserialize)]
struct RemotePollItem {
    #[serde(default)]
    id: Option<Uuid>,
    #[serde(default)]
    token: Option<String>,
    protocol: String,
    #[serde(default, alias = "remote_address")]
    peer_addr: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    raw_request: Option<RawRequestWire>,
    #[serde(default)]
    raw_request_base64: Option<String>,
    #[serde(default, alias = "timestamp_ms")]
    observed_unix_ms: Option<i64>,
}

fn parse_protocol_str(raw: &str) -> std::result::Result<OastProtocol, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "http" | "https" => Ok(OastProtocol::Http),
        "dns" => Ok(OastProtocol::Dns),
        other => Err(format!("不支持的 OAST 交互协议：{other}")),
    }
}

fn deterministic_remote_id(
    protocol: OastProtocol,
    peer_addr: &str,
    summary: &str,
    observed_unix_ms: i64,
    raw_request: &[u8],
) -> Uuid {
    let mut h = Sha256::new();
    h.update([protocol as u8]);
    h.update(peer_addr.as_bytes());
    h.update(summary.as_bytes());
    h.update(observed_unix_ms.to_be_bytes());
    h.update(raw_request);
    let digest = h.finalize();
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap_or_default();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn convert_remote_item(item: RemotePollItem) -> std::result::Result<OastInteraction, String> {
    let protocol = parse_protocol_str(&item.protocol)?;
    let raw_request = if let Some(b64) = item.raw_request_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("解码 remote OAST raw_request_base64 失败：{e}"))?
    } else {
        match item.raw_request {
            Some(RawRequestWire::Bytes(b)) => b,
            Some(RawRequestWire::Text(s)) => s.into_bytes(),
            None => Vec::new(),
        }
    };
    let observed_unix_ms = item
        .observed_unix_ms
        .unwrap_or_else(crate::codec_util::now_unix_ms);
    let id = item.id.unwrap_or_else(|| {
        deterministic_remote_id(
            protocol,
            &item.peer_addr,
            &item.summary,
            observed_unix_ms,
            &raw_request,
        )
    });
    Ok(OastInteraction {
        id,
        token: item.token.filter(|t| !t.trim().is_empty()),
        protocol,
        peer_addr: item.peer_addr,
        summary: item.summary,
        raw_request,
        observed_unix_ms,
    })
}

pub(super) fn parse_poll_response(
    body: &[u8],
) -> std::result::Result<(Vec<OastInteraction>, Option<String>), String> {
    if let Ok(items) = serde_json::from_slice::<Vec<OastInteraction>>(body) {
        return Ok((items, None));
    }
    if let Ok(envelope) = serde_json::from_slice::<RemotePollEnvelope>(body) {
        let mut out = Vec::with_capacity(envelope.interactions.len());
        for item in envelope.interactions {
            out.push(convert_remote_item(item)?);
        }
        return Ok((out, envelope.next_cursor));
    }
    let items = serde_json::from_slice::<Vec<RemotePollItem>>(body)
        .map_err(|error| format!("Collaborator 轮询响应 JSON 解析失败：{error}"))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(convert_remote_item(item)?);
    }
    Ok((out, None))
}

impl CollaboratorService {
    pub(super) fn spawn_poll(
        &mut self,
        project: ProjectKey,
        request: HttpRequest,
        send_config: SendConfig,
        parent_cancel: CancellationToken,
    ) {
        for (job_project, cancel) in self.active_polls.values() {
            if *job_project == project {
                cancel.cancel();
            }
        }
        let job_id = Uuid::new_v4();
        let cancel = parent_cancel.child_token();
        let job_cancel = cancel.clone();
        self.poll_jobs.spawn(async move {
            let result = execute_poll_request(request, send_config, job_cancel).await;
            PollJobOutput {
                job_id,
                project,
                result,
            }
        });
        self.active_polls.insert(job_id, (project, cancel));
    }

    pub(super) fn accept_poll_output(
        &mut self,
        output: std::result::Result<PollJobOutput, JoinError>,
    ) {
        match output {
            Ok(PollJobOutput {
                job_id,
                project,
                result,
            }) => {
                let Some((_, cancel)) = self.active_polls.remove(&job_id) else {
                    return;
                };
                if cancel.is_cancelled() {
                    return;
                }
                if self.project != Some(project) {
                    return;
                }
                match result {
                    Ok((items, next_cursor)) => {
                        {
                            let mut telemetry = self
                                .poll_telemetry
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner);
                            if let Some(c) = next_cursor.filter(|c| !c.trim().is_empty()) {
                                telemetry.cursor = Some(c);
                            }
                            telemetry.last_poll_unix_ms = Some(crate::codec_util::now_unix_ms());
                            telemetry.last_poll_error = None;
                        }
                        for mut interaction in items {
                            if self.record_interaction(&mut interaction) {
                                self.pending_events.push_back(
                                    CollaboratorEvent::InteractionObserved {
                                        project,
                                        interaction: Box::new(interaction),
                                    },
                                );
                            }
                        }
                        self.pending_events.push_back(CollaboratorEvent::Status {
                            project,
                            status: Box::new(self.status()),
                        });
                    }
                    Err(_) if cancel.is_cancelled() => {}
                    Err(message) => {
                        {
                            let mut telemetry = self
                                .poll_telemetry
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner);
                            telemetry.last_poll_error = Some(message.clone());
                        }
                        self.pending_events
                            .push_back(CollaboratorEvent::Error { project, message });
                    }
                }
            }
            Err(error) => {
                if error.is_cancelled() {
                    return;
                }
                if let Some(project) = self.project {
                    self.pending_events.push_back(CollaboratorEvent::Error {
                        project,
                        message: format!("Collaborator 轮询任务异常退出：{error}"),
                    });
                }
            }
        }
    }

    pub(super) async fn join_poll_jobs(&mut self, deadline: TokioInstant) -> Result<()> {
        let mut timed_out = false;
        while !self.poll_jobs.is_empty() {
            match timeout_at(deadline, self.poll_jobs.join_next()).await {
                Ok(Some(output)) => self.accept_poll_output(output),
                Ok(None) => break,
                Err(_) => {
                    timed_out = true;
                    break;
                }
            }
        }
        if timed_out {
            self.poll_jobs.abort_all();
            let abort_deadline = TokioInstant::now() + Duration::from_millis(500);
            while !self.poll_jobs.is_empty() {
                match timeout_at(abort_deadline, self.poll_jobs.join_next()).await {
                    Ok(Some(output)) => self.accept_poll_output(output),
                    Ok(None) => break,
                    Err(_) => {
                        self.active_polls.clear();
                        bail!("Collaborator 轮询任务回收超过期限");
                    }
                }
            }
        }
        self.active_polls.clear();
        Ok(())
    }
}

pub(super) async fn execute_poll_request(
    request: HttpRequest,
    config: SendConfig,
    cancel: CancellationToken,
) -> std::result::Result<(Vec<OastInteraction>, Option<String>), String> {
    let response = send(request, config, cancel)
        .await
        .map_err(|error| format!("Collaborator 轮询请求失败：{error}"))?;
    if !(200..300).contains(&response.status) {
        return Err(format!("Collaborator 轮询返回 HTTP {}", response.status));
    }
    parse_poll_response(&response.body)
}

pub(super) fn build_poll_request(
    polling_url: &Url,
    auth_header: Option<&str>,
    cursor: Option<&str>,
) -> HttpRequest {
    let mut url = polling_url.clone();
    if let Some(c) = cursor.map(str::trim).filter(|s| !s.is_empty()) {
        url.query_pairs_mut().append_pair("cursor", c);
    }
    let mut headers = vec![Header {
        name: "Accept".to_owned(),
        value: b"application/json".to_vec(),
    }];
    if let Some(auth) = auth_header.map(str::trim).filter(|value| !value.is_empty()) {
        let (name, value) = auth
            .split_once(':')
            .filter(|(header_name, _)| header_name.trim().eq_ignore_ascii_case("authorization"))
            .map_or(("Authorization", auth), |(_, header_val)| {
                ("Authorization", header_val.trim())
            });
        headers.push(Header {
            name: name.to_owned(),
            value: value.as_bytes().to_vec(),
        });
    }
    HttpRequest {
        method: "GET".to_owned(),
        url: url.as_str().to_owned(),
        version: HttpVersion::Http11,
        headers,
        body: Vec::new(),
        trailers: Vec::new(),
    }
}
