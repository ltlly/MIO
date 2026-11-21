use std::future::pending;
use std::ops::{Deref, DerefMut};

use tokio::task::{JoinError, JoinHandle};

use super::*;

type Output = Result<HttpHistoryResult>;

pub(super) trait AsyncHistoryQuery: Clone + Send + 'static {
    type Output: Send + 'static;

    const CANCELLED_MESSAGE: &'static str;
    const PANIC_PREFIX: &'static str;
    const SHUTDOWN_TIMEOUT_MESSAGE: &'static str;

    fn project(&self) -> ProjectKey;
    fn into_event(self, result: Result<Self::Output>) -> Event;
    fn event_project(event: &Event) -> Option<ProjectKey>;
    fn is_ok_event_preserved_when_idle(_event: &Event) -> bool {
        false
    }
    fn convert_pending_event_on_cancel(event: Event) -> Event {
        event
    }
    fn is_cancelled(&self, active_cancel: &CancellationToken) -> bool {
        active_cancel.is_cancelled()
    }
}

pub(super) struct ActiveQuery<Q, O> {
    pub(super) query: Q,
    pub(super) cancel: CancellationToken,
    pub(super) worker: JoinHandle<Result<O>>,
}

type Active = ActiveQuery<HttpHistoryQuery, HttpHistoryResult>;

pub(super) enum QuerySignal<O> {
    Finished(std::result::Result<Result<O>, JoinError>),
    Ready(mpsc::OwnedPermit<Event>),
}

pub(super) struct AsyncQueryState<Q: AsyncHistoryQuery> {
    pub(super) active: Option<ActiveQuery<Q, Q::Output>>,
    pub(super) latest: Option<Q>,
    pub(super) result: Option<Event>,
}

impl<Q: AsyncHistoryQuery> Default for AsyncQueryState<Q> {
    fn default() -> Self {
        Self {
            active: None,
            latest: None,
            result: None,
        }
    }
}

impl<Q: AsyncHistoryQuery> AsyncQueryState<Q> {
    pub(super) fn has_worker(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn cancel(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.cancel();
        }
        if let Some(query) = self.latest.take() {
            self.result = Some(query.into_event(Err(anyhow::anyhow!(Q::CANCELLED_MESSAGE))));
            return;
        }
        if self.active.is_none()
            && self
                .result
                .as_ref()
                .is_some_and(Q::is_ok_event_preserved_when_idle)
        {
            return;
        }
        let owner = self.active.as_ref().map(|active| active.query.project());
        let result = self.result.take().filter(|result| {
            Q::event_project(result).is_some_and(|project| owner.is_none_or(|o| o == project))
        });
        if let Some(result) = result {
            self.result = Some(Q::convert_pending_event_on_cancel(result));
        } else if let Some(active) = &self.active {
            self.result = Some(
                active
                    .query
                    .clone()
                    .into_event(Err(anyhow::anyhow!(Q::CANCELLED_MESSAGE))),
            );
        }
    }

    pub(super) async fn completion(&mut self) -> std::result::Result<Result<Q::Output>, JoinError> {
        match &mut self.active {
            Some(active) => (&mut active.worker).await,
            None => pending().await,
        }
    }

    pub(super) async fn next_signal(
        &mut self,
        events: &mpsc::Sender<Event>,
        reclaim_only: bool,
    ) -> QuerySignal<Q::Output> {
        // A cancellation reply is terminal only after this reader has actually exited.
        let deliver = self.result.is_some() && !self.has_worker() && !reclaim_only;
        tokio::select! {
            biased;
            result = self.completion() => QuerySignal::Finished(result),
            permit = async {
                if deliver {
                    match events.clone().reserve_owned().await {
                        Ok(permit) => permit,
                        Err(_) => pending().await,
                    }
                } else {
                    pending().await
                }
            } => QuerySignal::Ready(permit),
        }
    }

    pub(super) fn reject_or_record_error(
        &mut self,
        query: Q,
        current_project: ProjectKey,
        error: anyhow::Error,
    ) {
        let project = query.project();
        if project == current_project {
            if let Some(active) = &self.active {
                active.cancel.cancel();
            }
            self.latest = None;
        }
        if self.result.is_none() || project == current_project {
            self.result = Some(query.into_event(Err(error)));
        }
    }

    pub(super) fn enqueue_or_take_start(&mut self, query: Q) -> Option<Q> {
        self.result = None;
        if let Some(active) = &self.active {
            active.cancel.cancel();
            self.latest = Some(query);
            None
        } else {
            Some(query)
        }
    }

    pub(super) fn deliver_ready(&mut self, permit: mpsc::OwnedPermit<Event>) {
        permit.send(self.result.take().expect("query result owns this permit"));
    }

    pub(super) fn finish_and_take_next(
        &mut self,
        result: std::result::Result<Result<Q::Output>, JoinError>,
        current_project: ProjectKey,
    ) -> Option<Q> {
        let active = self.active.take().expect("joined query retains its owner");
        if let Some(latest) = self.latest.take() {
            return Some(latest);
        }
        if active.query.project() == current_project
            && !self
                .result
                .as_ref()
                .is_some_and(|event| Q::event_project(event) == Some(active.query.project()))
        {
            let result = if active.query.is_cancelled(&active.cancel) {
                Err(anyhow::anyhow!(Q::CANCELLED_MESSAGE))
            } else {
                result
                    .map_err(|error| anyhow::anyhow!("{}：{error}", Q::PANIC_PREFIX))
                    .and_then(|result| result)
            };
            self.result = Some(active.query.into_event(result));
        }
        None
    }

    pub(super) async fn stop_until(
        &mut self,
        deadline: tokio::time::Instant,
        current_project: ProjectKey,
    ) -> Result<()> {
        self.cancel();
        if self.has_worker() {
            let result = tokio::time::timeout_at(deadline, self.completion())
                .await
                .context(Q::SHUTDOWN_TIMEOUT_MESSAGE)?;
            let next = self.finish_and_take_next(result, current_project);
            debug_assert!(next.is_none(), "cancelled shutdown never restarts latest");
        }
        Ok(())
    }

    pub(super) async fn finish_shutdown(&mut self, current_project: ProjectKey) -> Result<()> {
        self.cancel();
        if self.has_worker() {
            // A blocking matcher cannot be aborted; retain this owner until its actual exit.
            let result = self.completion().await;
            let failure = result.as_ref().err().map(ToString::to_string);
            let next = self.finish_and_take_next(result, current_project);
            debug_assert!(next.is_none(), "cancelled shutdown never restarts latest");
            if let Some(error) = failure {
                anyhow::bail!("{}：{error}", Q::PANIC_PREFIX);
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct GlobalSearchEntry {
    query: GlobalSearchQuery,
    operation: CancellationToken,
}

#[derive(Default)]
pub(super) struct QueryState {
    pub(super) history: AsyncQueryState<HttpHistoryQuery>,
    global_search: AsyncQueryState<GlobalSearchEntry>,
}

impl Deref for QueryState {
    type Target = AsyncQueryState<HttpHistoryQuery>;

    fn deref(&self) -> &Self::Target {
        &self.history
    }
}

impl DerefMut for QueryState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.history
    }
}

pub(super) enum Signal {
    Finished(std::result::Result<Output, JoinError>),
    Ready(mpsc::OwnedPermit<Event>),
    GlobalSearchFinished(std::result::Result<Result<GlobalSearchResult>, JoinError>),
    GlobalSearchReady(mpsc::OwnedPermit<Event>),
}

impl HttpHistoryQuery {
    fn result(self, result: Output) -> Event {
        Event::HttpHistoryPage {
            query: self,
            result: result.map_err(|error| error.to_string()),
        }
    }
}

impl AsyncHistoryQuery for HttpHistoryQuery {
    type Output = HttpHistoryResult;

    const CANCELLED_MESSAGE: &'static str = "HTTP 历史查询已取消";
    const PANIC_PREFIX: &'static str = "HTTP 历史查询任务异常退出";
    const SHUTDOWN_TIMEOUT_MESSAGE: &'static str =
        "HTTP 历史查询仍在回收；保留任务句柄，未保存或切换项目，请稍后重试";

    fn project(&self) -> ProjectKey {
        self.project
    }

    fn into_event(self, result: Result<Self::Output>) -> Event {
        self.result(result)
    }

    fn event_project(event: &Event) -> Option<ProjectKey> {
        match event {
            Event::HttpHistoryPage { query, .. } => Some(query.project),
            _ => None,
        }
    }

    fn is_ok_event_preserved_when_idle(event: &Event) -> bool {
        matches!(event, Event::HttpHistoryPage { result: Ok(_), .. })
    }

    fn convert_pending_event_on_cancel(event: Event) -> Event {
        match event {
            Event::HttpHistoryPage { query, .. } => {
                query.result(Err(anyhow::anyhow!(Self::CANCELLED_MESSAGE)))
            }
            other => other,
        }
    }
}

impl AsyncHistoryQuery for GlobalSearchEntry {
    type Output = GlobalSearchResult;

    const CANCELLED_MESSAGE: &'static str = "全局搜索已取消";
    const PANIC_PREFIX: &'static str = "全局搜索任务异常退出";
    const SHUTDOWN_TIMEOUT_MESSAGE: &'static str =
        "全局搜索仍在回收；保留任务句柄，未保存或切换项目，请稍后重试";

    fn project(&self) -> ProjectKey {
        self.query.project
    }

    fn into_event(self, result: Result<Self::Output>) -> Event {
        Event::GlobalSearchPage {
            query: self.query,
            result: result.map_err(|error| error.to_string()),
        }
    }

    fn event_project(event: &Event) -> Option<ProjectKey> {
        match event {
            Event::GlobalSearchPage { query, .. } => Some(query.project),
            _ => None,
        }
    }

    fn is_ok_event_preserved_when_idle(event: &Event) -> bool {
        matches!(event, Event::GlobalSearchPage { result: Ok(_), .. })
    }

    fn convert_pending_event_on_cancel(event: Event) -> Event {
        match event {
            Event::GlobalSearchPage { query, .. } => Event::GlobalSearchPage {
                query,
                result: Err(Self::CANCELLED_MESSAGE.to_string()),
            },
            other => other,
        }
    }

    fn is_cancelled(&self, active_cancel: &CancellationToken) -> bool {
        active_cancel.is_cancelled() || self.operation.is_cancelled()
    }
}

impl QueryState {
    pub(super) fn has_worker(&self) -> bool {
        self.history.has_worker() || self.global_search.has_worker()
    }

    pub(super) fn cancel(&mut self) {
        self.history.cancel();
        self.global_search.cancel();
    }

    pub(super) async fn next_signal(
        &mut self,
        events: &mpsc::Sender<Event>,
        reclaim_only: bool,
    ) -> Signal {
        tokio::select! {
            biased;
            signal = self.history.next_signal(events, reclaim_only) => match signal {
                QuerySignal::Finished(result) => Signal::Finished(result),
                QuerySignal::Ready(permit) => Signal::Ready(permit),
            },
            signal = self.global_search.next_signal(events, reclaim_only) => match signal {
                QuerySignal::Finished(result) => Signal::GlobalSearchFinished(result),
                QuerySignal::Ready(permit) => Signal::GlobalSearchReady(permit),
            },
        }
    }
}

impl State {
    pub(super) async fn query_global_search(
        &mut self,
        query: GlobalSearchQuery,
        cancel: CancellationToken,
    ) {
        let entry = GlobalSearchEntry {
            query: query.clone(),
            operation: cancel,
        };
        let valid = self.check_project(query.project).and_then(|()| {
            ensure!(!self.quiesced, "项目正在切换，全局搜索未开始");
            ensure!(!self.cancel.is_cancelled(), "全局搜索已取消");
            ensure!(!entry.operation.is_cancelled(), "全局搜索已取消");
            Ok(())
        });
        if let Err(error) = valid {
            self.queries
                .http
                .global_search
                .reject_or_record_error(entry, self.project, error);
            return;
        }
        if let Some(entry) = self
            .queries
            .http
            .global_search
            .enqueue_or_take_start(entry)
        {
            self.start_global_search(entry);
        }
    }

    fn start_global_search(&mut self, entry: GlobalSearchEntry) {
        debug_assert!(self.queries.http.global_search.active.is_none());
        let snapshot = self.check_project(entry.query.project).and_then(|()| {
            let mut ids = HashSet::new();
            let mut pending = Vec::new();
            for capture in self.storage.recovery.iter().rev() {
                if ids.insert(capture.record.summary.id) {
                    pending.push((
                        capture.record.clone(),
                        self.original_flow(capture.record.summary.id)?,
                    ));
                }
            }
            Ok((
                self.store.query_path(),
                pending,
                self.websocket_recovery_snapshot()?,
            ))
        });
        let (path, pending, websocket) = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.queries.http.global_search.result = Some(entry.into_event(Err(error)));
                return;
            }
        };
        let shutdown = self.cancel.child_token();
        let worker_shutdown = shutdown.clone();
        let operation = entry.operation.clone();
        let worker_query = entry.query.clone();
        let worker = tokio::task::spawn_blocking(move || {
            store::global_search_snapshot(
                &path,
                &worker_query,
                &pending,
                &websocket,
                &worker_shutdown,
                &operation,
            )
        });
        self.queries.http.global_search.active = Some(ActiveQuery {
            query: entry,
            cancel: shutdown,
            worker,
        });
    }

    pub(super) fn http_recovery_snapshot(&self) -> Result<Vec<HttpHistoryPending>> {
        let mut positions = HashMap::<FlowId, usize>::new();
        let mut rows = Vec::<HttpHistoryPending>::new();
        // Keep the latest state, but the first observed request for history filters.
        for capture in &self.storage.recovery {
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
            self.queries
                .http
                .history
                .reject_or_record_error(query, self.project, error);
            return;
        }
        self.storage.history_dirty = false;
        if let Some(query) = self.queries.http.history.enqueue_or_take_start(query) {
            self.start_http_query(query);
        }
    }

    fn start_http_query(&mut self, query: HttpHistoryQuery) {
        debug_assert!(self.queries.http.history.active.is_none());
        let snapshot = self
            .check_project(query.project)
            .and_then(|()| self.http_recovery_snapshot());
        let pending = match snapshot {
            Ok(pending) => pending,
            Err(error) => {
                self.queries.http.history.result = Some(query.result(Err(error)));
                return;
            }
        };
        let path = self.store.query_path();
        let cancel = self.cancel.child_token();
        let worker_cancel = cancel.clone();
        let worker_query = query.clone();
        let worker = tokio::task::spawn_blocking(move || {
            store::http_history_snapshot(&path, &worker_query, &pending, &worker_cancel)
        });
        self.queries.http.history.active = Some(Active {
            query,
            cancel,
            worker,
        });
    }

    pub(super) fn http_query_signal(&mut self, signal: Signal) {
        match signal {
            Signal::Ready(permit) => self.queries.http.history.deliver_ready(permit),
            Signal::Finished(result) => {
                if let Some(latest) = self
                    .queries
                    .http
                    .history
                    .finish_and_take_next(result, self.project)
                {
                    self.start_http_query(latest);
                }
            }
            Signal::GlobalSearchReady(permit) => {
                self.queries.http.global_search.deliver_ready(permit)
            }
            Signal::GlobalSearchFinished(result) => {
                if let Some(latest) = self
                    .queries
                    .http
                    .global_search
                    .finish_and_take_next(result, self.project)
                {
                    self.start_global_search(latest);
                }
            }
        }
    }

    pub(super) async fn stop_http_query_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.queries.http.cancel();
        self.queries
            .http
            .history
            .stop_until(deadline, self.project)
            .await?;
        self.queries
            .http
            .global_search
            .stop_until(deadline, self.project)
            .await?;
        Ok(())
    }

    pub(super) async fn finish_http_query_shutdown(&mut self) -> Result<()> {
        self.queries.http.cancel();
        let history = self
            .queries
            .http
            .history
            .finish_shutdown(self.project)
            .await;
        let global_search = self
            .queries
            .http
            .global_search
            .finish_shutdown(self.project)
            .await;
        history?;
        global_search?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
