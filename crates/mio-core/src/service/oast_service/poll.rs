use super::*;

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
                if self.project != Some(project) {
                    return;
                }
                match result {
                    Ok(items) => {
                        for interaction in items {
                            if self.record_interaction(interaction.clone()) {
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
            while !self.poll_jobs.is_empty() {
                match timeout_at(deadline, self.poll_jobs.join_next()).await {
                    Ok(Some(output)) => self.accept_poll_output(output),
                    Ok(None) => break,
                    Err(_) => bail!("Collaborator 轮询任务回收超过期限"),
                }
            }
        }
        self.active_polls.clear();
        Ok(())
    }
}

async fn execute_poll_request(
    request: HttpRequest,
    config: SendConfig,
    cancel: CancellationToken,
) -> std::result::Result<Vec<OastInteraction>, String> {
    let response = send(request, config, cancel)
        .await
        .map_err(|error| format!("Collaborator 轮询请求失败：{error}"))?;
    if !(200..300).contains(&response.status) {
        return Err(format!("Collaborator 轮询返回 HTTP {}", response.status));
    }
    serde_json::from_slice::<Vec<OastInteraction>>(&response.body)
        .map_err(|error| format!("Collaborator 轮询响应 JSON 解析失败：{error}"))
}

pub(super) fn build_poll_request(polling_url: &Url, auth_header: Option<&str>) -> HttpRequest {
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
        url: polling_url.as_str().to_owned(),
        version: HttpVersion::Http11,
        headers,
        body: Vec::new(),
        trailers: Vec::new(),
    }
}
