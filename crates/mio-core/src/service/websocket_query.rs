use super::http_query::{ActiveQuery, AsyncHistoryQuery, AsyncQueryState, QuerySignal};
use super::*;

type Output = Result<WebSocketHistoryResult>;
type Active = ActiveQuery<WebSocketHistoryQuery, WebSocketHistoryResult>;

pub(super) type QueryState = AsyncQueryState<WebSocketHistoryQuery>;
pub(super) type Signal = QuerySignal<WebSocketHistoryResult>;

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

impl AsyncHistoryQuery for WebSocketHistoryQuery {
    type Output = WebSocketHistoryResult;

    const CANCELLED_MESSAGE: &'static str = "WebSocket 查询已取消";
    const PANIC_PREFIX: &'static str = "WebSocket 查询任务异常退出";
    const SHUTDOWN_TIMEOUT_MESSAGE: &'static str =
        "WebSocket 查询仍在回收；保留任务句柄，未保存或切换项目，请稍后重试";

    fn project(&self) -> ProjectKey {
        self.project
    }

    fn into_event(self, result: Result<Self::Output>) -> Event {
        self.result(result)
    }

    fn event_project(event: &Event) -> Option<ProjectKey> {
        match event {
            Event::WebSocketHistoryPage { project, .. }
            | Event::WebSocketHistoryError { project, .. } => Some(*project),
            _ => None,
        }
    }

    fn convert_pending_event_on_cancel(event: Event) -> Event {
        match event {
            Event::WebSocketHistoryPage {
                project,
                query_revision,
                ..
            } => Event::WebSocketHistoryError {
                project,
                query_revision,
                message: Self::CANCELLED_MESSAGE.into(),
            },
            other => other,
        }
    }
}

impl State {
    pub(super) fn query_websocket_history(&mut self, query: WebSocketHistoryQuery) {
        let valid = self
            .check_project(query.project)
            .and_then(|()| query.validate());
        if let Err(error) = valid {
            self.websocket_query
                .reject_or_record_error(query, self.project, error);
            return;
        }
        self.websocket_history_dirty = false;
        if let Some(query) = self.websocket_query.enqueue_or_take_start(query) {
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
            Signal::Ready(permit) => self.websocket_query.deliver_ready(permit),
            Signal::Finished(result) => {
                if let Some(latest) = self
                    .websocket_query
                    .finish_and_take_next(result, self.project)
                {
                    self.start_websocket_query(latest);
                }
            }
        }
    }

    pub(super) async fn stop_websocket_query_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.websocket_query.stop_until(deadline, self.project).await
    }

    pub(super) async fn finish_websocket_query_shutdown(&mut self) -> Result<()> {
        self.websocket_query.finish_shutdown(self.project).await
    }
}

#[cfg(test)]
mod tests;
