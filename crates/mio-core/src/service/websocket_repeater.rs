use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, bail};
use mio_plugin_api::{WebSocketDirection, WebSocketKind};
use mio_proxy::{SendConfig, WebSocketInjectorRegistry};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    task::{JoinHandle, JoinSet},
    time::timeout_at,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Event, FlowId, Header, ProjectKey, TabId, Uuid, Workspace,
    api::websocket_repeater::{
        MAX_WEBSOCKET_REPEATER_TABS, WebSocketRepeaterCommand, WebSocketRepeaterConnectionMode,
        WebSocketRepeaterEvent, WebSocketRepeaterHistoryItem, WebSocketRepeaterTab,
    },
    preferences::PreferencesStore,
    service::State,
};

const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const JOB_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PEER_QUEUE: usize = 256;

type SharedSession = Arc<AsyncMutex<mio_proxy::WebSocketSession>>;

pub(super) trait ProxyWebSocketInjector: Send + Sync + 'static {
    fn contains(&self, flow_id: FlowId) -> bool;

    fn inject_directed(
        &self,
        flow_id: FlowId,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> PinBoxInject;
}

type PinBoxInject = Pin<
    Box<
        dyn Future<Output = Result<mio_plugin_api::WebSocketMessage, mio_proxy::ProxyError>> + Send,
    >,
>;

#[derive(Clone)]
struct JobContext {
    id: Uuid,
    project: ProjectKey,
    tab_id: TabId,
    operation_id: Uuid,
    cancel: CancellationToken,
}

enum JobOutput {
    Connected {
        context: JobContext,
        session_id: Uuid,
        session: SharedSession,
        reader: JoinHandle<()>,
        response: Option<Box<mio_plugin_api::HttpResponse>>,
    },
    ConnectFailed {
        context: JobContext,
        error: String,
    },
    Sent {
        context: JobContext,
        draft_revision: u64,
        result: Result<WebSocketRepeaterHistoryItem, String>,
    },
    Disconnected {
        context: JobContext,
        result: Result<(), String>,
    },
}

struct ActiveStandalone {
    session_id: Uuid,
    session: SharedSession,
    cancel: CancellationToken,
    reader: JoinHandle<()>,
}

struct TabState {
    metadata: WebSocketRepeaterTab,
    detached_from_source: bool,
    pending_session_id: Option<Uuid>,
    connect_cancel: CancellationToken,
    active: Option<ActiveStandalone>,
}

enum PeerPayload {
    Frame(WebSocketRepeaterHistoryItem),
    Closed(Option<String>),
}

struct PeerUpdate {
    project: ProjectKey,
    tab_id: TabId,
    session_id: Uuid,
    payload: PeerPayload,
}

pub(super) struct WebSocketRepeaterService {
    project: ProjectKey,
    connect_config: Option<SendConfig>,
    injectors: Option<Arc<dyn ProxyWebSocketInjector>>,
    tabs: HashMap<ProjectKey, Vec<TabState>>,
    jobs: JoinSet<JobOutput>,
    job_cancels: HashMap<Uuid, (ProjectKey, CancellationToken)>,
    peers_tx: mpsc::Sender<PeerUpdate>,
    peers_rx: mpsc::Receiver<PeerUpdate>,
    pending_events: VecDeque<WebSocketRepeaterEvent>,
}

impl Default for WebSocketRepeaterService {
    fn default() -> Self {
        Self::new(ProjectKey {
            id: Uuid::nil(),
            generation: 0,
        })
    }
}

impl ProxyWebSocketInjector for WebSocketInjectorRegistry {
    fn contains(&self, flow_id: FlowId) -> bool {
        Self::contains(self, flow_id)
    }

    fn inject_directed(
        &self,
        flow_id: FlowId,
        direction: WebSocketDirection,
        kind: WebSocketKind,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> PinBoxInject {
        let registry = self.clone();
        Box::pin(async move {
            registry
                .inject_directed(flow_id, direction, kind, payload, timeout)
                .await
        })
    }
}

use jobs::read_standalone_session;
use ui_events::{rejected_connected, rejected_disconnected, rejected_sent};

mod jobs;
mod sessions;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fixtures;
#[cfg(test)]
mod tests_history;
#[cfg(test)]
mod tests_runtime;
mod ui_events;
mod workspace;
