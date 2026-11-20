use std::future::pending;

use tokio::task::{JoinError, JoinHandle};

use super::*;

type Output = Result<HttpHistoryResult>;

struct Active {
    query: HttpHistoryQuery,
    cancel: CancellationToken,
    worker: JoinHandle<Output>,
}

#[derive(Default)]
pub(super) struct QueryState {
    active: Option<Active>,
    latest: Option<HttpHistoryQuery>,
    result: Option<Event>,
}

pub(super) enum Signal {
    Finished(std::result::Result<Output, JoinError>),
    Ready(mpsc::OwnedPermit<Event>),
}

impl HttpHistoryQuery {
    fn result(self, result: Output) -> Event {
        Event::HttpHistoryPage {
            query: self,
            result: result.map_err(|error| error.to_string()),
        }
    }
}

impl QueryState {
    pub(super) fn has_worker(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn cancel(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.cancel();
        }
        if let Some(query) = self.latest.take() {
            self.result = Some(query.result(Err(anyhow::anyhow!("HTTP 历史查询已取消"))));
            return;
        }
        if self.active.is_none()
            && matches!(
                &self.result,
                Some(Event::HttpHistoryPage { result: Ok(_), .. })
            )
        {
            return;
        }
        let owner = self.active.as_ref().map(|active| active.query.project);
        let result = self.result.take().filter(|result| {
            matches!(result, Event::HttpHistoryPage { query, .. }
                if owner.is_none_or(|owner| owner == query.project))
        });
        if let Some(Event::HttpHistoryPage { query, .. }) = result {
            self.result = Some(query.result(Err(anyhow::anyhow!("HTTP 历史查询已取消"))));
        } else if let Some(active) = &self.active {
            self.result = Some(
                active
                    .query
                    .clone()
                    .result(Err(anyhow::anyhow!("HTTP 历史查询已取消"))),
            );
        }
    }

    async fn completion(&mut self) -> std::result::Result<Output, JoinError> {
        match &mut self.active {
            Some(active) => (&mut active.worker).await,
            None => pending().await,
        }
    }

    pub(super) async fn next_signal(
        &mut self,
        events: &mpsc::Sender<Event>,
        reclaim_only: bool,
    ) -> Signal {
        // A cancellation reply is terminal only after this reader has actually exited.
        let deliver = self.result.is_some() && !self.has_worker() && !reclaim_only;
        tokio::select! {
            biased;
            result = self.completion() => Signal::Finished(result),
            permit = async {
                if deliver {
                    match events.clone().reserve_owned().await {
                        Ok(permit) => permit,
                        Err(_) => pending().await,
                    }
                } else { pending().await }
            } => Signal::Ready(permit),
        }
    }
}

impl State {
    pub(super) async fn query_global_search(
        &mut self,
        query: GlobalSearchQuery,
        cancel: CancellationToken,
    ) {
        let snapshot = self.check_project(query.project).and_then(|()| {
            ensure!(!self.quiesced, "项目正在切换，全局搜索未开始");
            let mut ids = HashSet::new();
            let mut pending = Vec::new();
            for capture in self.recovery.iter().rev() {
                if ids.insert(capture.record.summary.id) {
                    pending.push((capture.record.clone(), self.original_flow(capture.record.summary.id)?));
                }
            }
            Ok((self.store.websocket_query_path(), pending, self.websocket_recovery_snapshot()?))
        });
        let result = match snapshot {
            Ok((path, pending, websocket)) => {
                let shutdown = self.cancel.clone();
                let operation = cancel.clone();
                let combined = CancellationToken::new();
                if shutdown.is_cancelled() || operation.is_cancelled() { combined.cancel(); }
                let forward = combined.clone();
                let guard = tokio::spawn(async move {
                    tokio::select! {
                        _ = shutdown.cancelled() => forward.cancel(),
                        _ = operation.cancelled() => forward.cancel(),
                    }
                });
                let worker_query = query.clone();
                let result = tokio::task::spawn_blocking(move || {
                    ProjectStore::global_search_snapshot(&path, &worker_query, &pending, &websocket, &combined)
                }).await.expect("global search worker must finish normally");
                guard.abort();
                result
            }
            Err(error) => Err(error),
        };
        self.emit(Event::GlobalSearchPage {
            query,
            result: result.map_err(|error| error.to_string()),
        }).await;
    }

    pub(super) fn http_recovery_snapshot(&self) -> Result<Vec<HttpHistoryPending>> {
        let mut positions = HashMap::<FlowId, usize>::new();
        let mut rows = Vec::<HttpHistoryPending>::new();
        // Keep the latest state, but the first observed request for history filters.
        for capture in &self.recovery {
            let record = &capture.record;
            if record.summary.source != TrafficSource::Proxy {
                ensure!(
                    !positions.contains_key(&record.summary.id),
                    "HTTP 恢复记录的来源发生变化"
                );
                continue;
            }
            let status = if let Some(stream) = &capture.response_stream {
                stream.validate()?;
                ensure!(record.response.is_some(), "流式恢复记录缺少响应头");
                Some(stream.original_status)
            } else {
                record.response.as_ref().map(|response| response.status)
            };
            let original_response = if let Some(stream) = &capture.response_stream {
                let response = record.response.as_ref().context("流式恢复记录缺少响应头")?;
                Some(HttpResponse {
                    status: stream.original_status,
                    version: stream.original_version,
                    reason_phrase: capture.capture_metrics.reason_phrase.clone(),
                    headers: stream.original_headers.clone(),
                    body: response.body.clone(),
                    trailers: response.trailers.clone(),
                })
            } else {
                record.response.clone()
            };
            if let Some(&index) = positions.get(&record.summary.id) {
                rows[index].summary = record.summary.clone();
                rows[index].request = Some(record.request.clone());
                rows[index].response = record.response.clone();
                if rows[index].listener_port.is_none() {
                    rows[index].listener_port = capture.listener_port;
                }
                if capture.capture_metrics.remote_ip.is_some() {
                    rows[index].capture_metrics.remote_ip = capture.capture_metrics.remote_ip;
                }
                if capture.capture_metrics.response_received_ms.is_some() {
                    rows[index].capture_metrics.response_received_ms =
                        capture.capture_metrics.response_received_ms;
                }
                if capture.capture_metrics.reason_phrase.is_some() {
                    rows[index].capture_metrics.reason_phrase =
                        capture.capture_metrics.reason_phrase.clone();
                }
                if rows[index].first_response_status.is_none() {
                    rows[index].first_response_status = status;
                }
                if rows[index].first_response.is_none() || capture.response_stream.is_some() {
                    rows[index].first_response = original_response;
                }
            } else {
                ensure!(
                    rows.len() < 512,
                    "HTTP 恢复摘要超过 512 条；未省略待保存流量"
                );
                positions.insert(record.summary.id, rows.len());
                rows.push(HttpHistoryPending {
                    summary: record.summary.clone(),
                    first_request_method: record.request.method.clone(),
                    first_request_url: record.request.url.clone(),
                    first_response_status: status,
                    first_request: Some(record.request.clone()),
                    first_response: original_response,
                    request: Some(record.request.clone()),
                    response: record.response.clone(),
                    listener_port: capture.listener_port,
                    capture_metrics: capture.capture_metrics.clone(),
                });
            }
        }
        Ok(rows)
    }

    pub(super) fn query_http_history(&mut self, query: HttpHistoryQuery) {
        let valid = self.check_project(query.project).and_then(|()| {
            ensure!(!self.quiesced, "项目正在切换；HTTP 历史查询未开始");
            ensure!(!self.cancel.is_cancelled(), "HTTP 历史查询已取消");
            query.validate()
        });
        if let Err(error) = valid {
            if query.project == self.project {
                if let Some(active) = &self.http_query.active {
                    active.cancel.cancel();
                }
                self.http_query.latest = None;
            }
            if self.http_query.result.is_none() || query.project == self.project {
                self.http_query.result = Some(query.result(Err(error)));
            }
            return;
        }
        self.history_dirty = false;
        self.http_query.result = None;
        if let Some(active) = &self.http_query.active {
            active.cancel.cancel();
            self.http_query.latest = Some(query);
        } else {
            self.start_http_query(query);
        }
    }

    fn start_http_query(&mut self, query: HttpHistoryQuery) {
        debug_assert!(self.http_query.active.is_none());
        let snapshot = self
            .check_project(query.project)
            .and_then(|()| self.http_recovery_snapshot());
        let pending = match snapshot {
            Ok(pending) => pending,
            Err(error) => {
                self.http_query.result = Some(query.result(Err(error)));
                return;
            }
        };
        let path = self.store.websocket_query_path();
        let cancel = self.cancel.child_token();
        let worker_cancel = cancel.clone();
        let worker_query = query.clone();
        let worker = tokio::task::spawn_blocking(move || {
            ProjectStore::http_history_snapshot(&path, &worker_query, &pending, &worker_cancel)
        });
        self.http_query.active = Some(Active {
            query,
            cancel,
            worker,
        });
    }

    pub(super) fn http_query_signal(&mut self, signal: Signal) {
        match signal {
            Signal::Ready(permit) => {
                permit.send(
                    self.http_query
                        .result
                        .take()
                        .expect("query result owns this permit"),
                );
            }
            Signal::Finished(result) => {
                let active = self
                    .http_query
                    .active
                    .take()
                    .expect("joined query retains its owner");
                if let Some(latest) = self.http_query.latest.take() {
                    self.start_http_query(latest);
                } else if active.query.project == self.project
                    && !matches!(&self.http_query.result,
                        Some(Event::HttpHistoryPage { query, .. })
                        if query.project == active.query.project)
                {
                    let result = if active.cancel.is_cancelled() {
                        Err(anyhow::anyhow!("HTTP 历史查询已取消"))
                    } else {
                        result
                            .map_err(|error| anyhow::anyhow!("HTTP 历史查询任务异常退出：{error}"))
                            .and_then(|result| result)
                    };
                    self.http_query.result = Some(active.query.result(result));
                }
            }
        }
    }

    pub(super) async fn stop_http_query_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.http_query.cancel();
        if self.http_query.has_worker() {
            let result = tokio::time::timeout_at(deadline, self.http_query.completion())
                .await
                .context("HTTP 历史查询仍在回收；保留任务句柄，未保存或切换项目，请稍后重试")?;
            self.http_query_signal(Signal::Finished(result));
        }
        Ok(())
    }

    pub(super) async fn finish_http_query_shutdown(&mut self) -> Result<()> {
        self.http_query.cancel();
        if self.http_query.has_worker() {
            let result = self.http_query.completion().await;
            let failure = result.as_ref().err().map(ToString::to_string);
            self.http_query_signal(Signal::Finished(result));
            if let Some(error) = failure {
                anyhow::bail!("HTTP 历史查询任务异常退出：{error}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
