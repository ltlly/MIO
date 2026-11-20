use std::future::pending;

use tokio::task::{JoinError, JoinHandle};

use super::*;

type Output = Result<WebSocketHistoryResult>;

struct Active {
    query: WebSocketHistoryQuery,
    cancel: CancellationToken,
    worker: JoinHandle<Output>,
}

#[derive(Default)]
pub(super) struct QueryState {
    active: Option<Active>,
    latest: Option<WebSocketHistoryQuery>,
    result: Option<Event>,
}

pub(super) enum Signal {
    Finished(std::result::Result<Output, JoinError>),
    Ready(mpsc::OwnedPermit<Event>),
}

impl WebSocketHistoryQuery {
    fn result(self, result: Output) -> Event {
        match result {
            Ok(result) => Event::WebSocketHistoryPage {
                project: self.project,
                query_revision: self.query_revision,
                filter: self.filter,
                flow_id: self.flow_id,
                offset: self.offset,
                total: result.total,
                rows: result.rows,
                selected_match: result.selected_match,
            },
            Err(error) => Event::WebSocketHistoryError {
                project: self.project,
                query_revision: self.query_revision,
                message: error.to_string(),
            },
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
            self.result = Some(query.result(Err(anyhow::anyhow!("WebSocket 查询已取消"))));
            return;
        }
        let owner = self.active.as_ref().map(|active| active.query.project);
        let result = self.result.take().filter(|result| match result {
            Event::WebSocketHistoryPage { project, .. }
            | Event::WebSocketHistoryError { project, .. } => {
                owner.is_none_or(|owner| owner == *project)
            }
            _ => false,
        });
        if let Some(result) = result {
            self.result = Some(match result {
                Event::WebSocketHistoryPage {
                    project,
                    query_revision,
                    ..
                } => Event::WebSocketHistoryError {
                    project,
                    query_revision,
                    message: "WebSocket 查询已取消".into(),
                },
                result => result,
            });
        } else if let Some(active) = &self.active {
            self.result = Some(
                active
                    .query
                    .clone()
                    .result(Err(anyhow::anyhow!("WebSocket 查询已取消"))),
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
        let deliver = self.result.is_some() && !reclaim_only;
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
    pub(super) fn query_websocket_history(&mut self, query: WebSocketHistoryQuery) {
        let valid = self
            .check_project(query.project)
            .and_then(|()| query.validate());
        if let Err(error) = valid {
            if query.project == self.project {
                if let Some(active) = &self.websocket_query.active {
                    active.cancel.cancel();
                }
                self.websocket_query.latest = None;
            }
            if self.websocket_query.result.is_none() || query.project == self.project {
                self.websocket_query.result = Some(query.result(Err(error)));
            }
            return;
        }
        self.websocket_history_dirty = false;
        self.websocket_query.result = None;
        if let Some(active) = &self.websocket_query.active {
            active.cancel.cancel();
            self.websocket_query.latest = Some(query);
        } else {
            self.start_websocket_query(query);
        }
    }

    fn start_websocket_query(&mut self, query: WebSocketHistoryQuery) {
        debug_assert!(self.websocket_query.active.is_none());
        let snapshot = self
            .check_project(query.project)
            .and_then(|()| self.websocket_recovery_snapshot());
        let pending = match snapshot {
            Ok(pending) => pending,
            Err(error) => {
                self.websocket_query.result = Some(query.result(Err(error)));
                return;
            }
        };
        let path = self.store.websocket_query_path();
        let cancel = self.cancel.child_token();
        let worker_cancel = cancel.clone();
        let worker_query = query.clone();
        let worker = tokio::task::spawn_blocking(move || {
            ProjectStore::websocket_query_snapshot(&path, &worker_query, &pending, &worker_cancel)
        });
        self.websocket_query.active = Some(Active {
            query,
            cancel,
            worker,
        });
    }

    pub(super) fn websocket_query_signal(&mut self, signal: Signal) {
        match signal {
            Signal::Ready(permit) => {
                permit.send(
                    self.websocket_query
                        .result
                        .take()
                        .expect("query result owns this permit"),
                );
            }
            Signal::Finished(result) => {
                let active = self
                    .websocket_query
                    .active
                    .take()
                    .expect("joined query retains its owner");
                if let Some(latest) = self.websocket_query.latest.take() {
                    self.start_websocket_query(latest);
                } else if active.query.project == self.project
                    && !matches!(&self.websocket_query.result,
                        Some(Event::WebSocketHistoryPage { project, .. }
                            | Event::WebSocketHistoryError { project, .. })
                        if *project == active.query.project)
                {
                    let result = if active.cancel.is_cancelled() {
                        Err(anyhow::anyhow!("WebSocket 查询已取消"))
                    } else {
                        result
                            .map_err(|error| anyhow::anyhow!("WebSocket 查询任务异常退出：{error}"))
                            .and_then(|result| result)
                    };
                    self.websocket_query.result = Some(active.query.result(result));
                }
            }
        }
    }

    pub(super) async fn stop_websocket_query_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.websocket_query.cancel();
        if self.websocket_query.has_worker() {
            let result = tokio::time::timeout_at(deadline, self.websocket_query.completion())
                .await
                .context("WebSocket 查询仍在回收；保留任务句柄，未保存或切换项目，请稍后重试")?;
            self.websocket_query_signal(Signal::Finished(result));
        }
        Ok(())
    }

    pub(super) async fn finish_websocket_query_shutdown(&mut self) -> Result<()> {
        self.websocket_query.cancel();
        if self.websocket_query.has_worker() {
            // A blocking matcher cannot be aborted; retain this owner until its actual exit.
            let result = self.websocket_query.completion().await;
            let failure = result.as_ref().err().map(ToString::to_string);
            self.websocket_query_signal(Signal::Finished(result));
            if let Some(error) = failure {
                anyhow::bail!("WebSocket 查询任务异常退出：{error}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
