use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::Arc,
    sync::RwLock,
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, ensure};
use mio_proxy::{
    CapturedFlow, ClientCertificateRule, InterceptDecision, MitmAuthority, ProxyConfig, ProxyEvent,
    ProxyHandle, SendConfig, StreamedResponse,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    authority,
    store::{CapturedWebSocket, ProjectStore},
    *,
};

pub(crate) mod ai_ipc;
#[cfg(test)]
mod ai_ipc_tests;
mod audit_service;
#[cfg(test)]
mod auto_positions_tests;
mod ext_repeater;
mod ext_service;
mod core_commands;
mod core_commands_annotations;
mod core_commands_tools;
mod core_commands_history;
mod core_commands_intruder;
mod core_commands_project;
mod core_commands_proxy;
pub use core_commands_proxy::build_configured_browser_args;
mod core_lifecycle;
mod core_proxy;
mod core_repeater_lifecycle;
mod core_repeater_send;
mod core_runtime;
mod core_storage;
#[cfg(test)]
mod core_tests_recovery_and_runtime;
#[cfg(test)]
mod core_tests_startup_and_send;
#[cfg(test)]
mod flow_detail_tests;
mod history_transfer;
#[cfg(test)]
mod http_highlights_tests;
mod http_query;
mod intruder_result_query;
mod intruder_service;
mod oast_service;
mod plugin_service;
#[cfg(test)]
mod repeater_history_tests;
#[cfg(test)]
mod repeater_terminal_tests;
mod sequencer_service;
#[cfg(test)]
mod shutdown_cleanup_tests;
#[cfg(test)]
mod streaming_tests;
#[cfg(test)]
mod terminal_tests;
pub(crate) mod websocket;
mod websocket_query;
mod websocket_repeater;

const PROXY_EVENT_CAPACITY: usize = 128;
const PROXY_MAX_CONNECTIONS: usize = 64;
pub(crate) const MAX_PROXY_LISTENERS: usize = 32;
const MAX_REPEATER_SENDS: usize = 16;
// Terminal + passive audit + error per admitted send, plus coalesced History and delivery.
const MAX_REPEATER_NOTIFICATIONS: usize = 4 * (MAX_REPEATER_SENDS + 16) + 4;
// Bounded across up to MAX_PROXY_LISTENERS active listeners, shared event queue, and intercepts.
const MAX_PROXY_NOTIFICATIONS: usize = 3
    * (PROXY_EVENT_CAPACITY + MAX_PROXY_LISTENERS * PROXY_MAX_CONNECTIONS)
    + MAX_PROXY_LISTENERS * PROXY_MAX_CONNECTIONS
    + 16;

pub struct Runtime {
    cancel: CancellationToken,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

struct Pending {
    revision: u64,
    stage: InterceptStage,
    response_head_only: bool,
    intercepted_request: HttpRequest,
    intercepted_response: Option<HttpResponse>,
    reply: oneshot::Sender<InterceptDecision>,
}

struct PendingWebSocket {
    flow_id: FlowId,
    kind: WebSocketKind,
    reply: oneshot::Sender<mio_proxy::WebSocketInterceptDecision>,
}

struct Completion {
    project: ProjectKey,
    tab_id: TabId,
    draft_revision: u64,
    received: Option<Box<FlowRecord>>,
    record: FlowRecord,
}

type ProgressPayload = (HttpResponse, u64);

struct ActiveSend {
    cancel: CancellationToken,
    task_id: tokio::task::Id,
    flow_id: FlowId,
    draft_revision: u64,
    progress_rx: Option<watch::Receiver<Option<ProgressPayload>>>,
}

struct ProxyRuntimeState {
    events: mpsc::Receiver<ProxyEvent>,
    sender: mpsc::Sender<ProxyEvent>,
    notifications: VecDeque<Event>,
    primary: Option<ProxyHandle>,
    primary_id: Option<Uuid>,
    extra: Vec<(Uuid, ProxyHandle)>,
    ca: Option<MitmAuthority>,
    intercept_requests: bool,
    intercept_responses: bool,
    pending: HashMap<FlowId, Pending>,
    pending_websockets: HashMap<Uuid, PendingWebSocket>,
}

impl ProxyRuntimeState {
    fn new() -> Self {
        let (sender, events) = mpsc::channel(PROXY_EVENT_CAPACITY);
        Self {
            events,
            sender,
            notifications: VecDeque::new(),
            primary: None,
            primary_id: None,
            extra: Vec::new(),
            ca: None,
            intercept_requests: false,
            intercept_responses: false,
            pending: HashMap::new(),
            pending_websockets: HashMap::new(),
        }
    }

    fn is_running(&self) -> bool {
        self.primary.is_some() || !self.extra.is_empty()
    }

    fn handles(&self) -> impl Iterator<Item = &ProxyHandle> {
        self.primary
            .iter()
            .chain(self.extra.iter().map(|(_, proxy)| proxy))
    }

    fn request_stop_all(&self) {
        for proxy in self.handles() {
            proxy.request_stop();
        }
    }

    fn set_intercept(&mut self, requests: bool, responses: bool) {
        self.intercept_requests = requests;
        self.intercept_responses = responses;
        for proxy in self.handles() {
            proxy.set_intercept(requests, responses);
        }
    }

    fn clear_handles(&mut self) {
        self.primary = None;
        self.primary_id = None;
        self.extra.clear();
    }

    fn primary_address(&self) -> Option<SocketAddr> {
        self.primary.as_ref().map(ProxyHandle::address)
    }

    fn primary_cancel(&self) -> Option<CancellationToken> {
        self.primary.as_ref().map(ProxyHandle::cancellation_token)
    }

    fn websocket_injectors(&self) -> Option<mio_proxy::WebSocketInjectorRegistry> {
        self.primary.as_ref().map(ProxyHandle::websocket_injectors)
    }

    fn active_listeners(&self) -> Vec<(Uuid, SocketAddr)> {
        let mut listeners = Vec::new();
        if let Some(proxy) = &self.primary {
            listeners.push((
                self.primary_id.unwrap_or(Uuid::from_u128(1)),
                proxy.address(),
            ));
        }
        for (id, proxy) in &self.extra {
            listeners.push((*id, proxy.address()));
        }
        listeners
    }
}

struct StorageState {
    project_disk_bytes: u64,
    last_disk_refresh: Option<Instant>,
    history_dirty: bool,
    websocket_history_dirty: bool,
    // The first fault stops producers; only the bounded event queue and active jobs can add records.
    recovery: VecDeque<CapturedFlow>,
    websocket_recovery: VecDeque<CapturedWebSocket>,
    error: Option<String>,
    annotated_flows: Arc<RwLock<HashSet<FlowId>>>,
    provenance: ProvenanceTracker,
}

impl StorageState {
    fn new() -> Self {
        Self {
            project_disk_bytes: 0,
            last_disk_refresh: None,
            history_dirty: false,
            websocket_history_dirty: false,
            recovery: VecDeque::new(),
            websocket_recovery: VecDeque::new(),
            error: None,
            annotated_flows: Arc::new(RwLock::new(HashSet::new())),
            provenance: ProvenanceTracker::new(),
        }
    }

    fn clear_recovery(&mut self) {
        self.recovery.clear();
        self.websocket_recovery.clear();
        self.error = None;
    }

    fn reset_for_project(&mut self, restored_annotated: HashSet<FlowId>) {
        self.history_dirty = false;
        self.websocket_history_dirty = false;
        self.project_disk_bytes = 0;
        self.last_disk_refresh = None;
        self.provenance.clear();
        *self
            .annotated_flows
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = restored_annotated;
    }
}

struct RepeaterState {
    sends: HashMap<TabId, ActiveSend>,
    jobs: JoinSet<Completion>,
    notifications: VecDeque<Event>,
}

impl RepeaterState {
    fn new() -> Self {
        Self {
            sends: HashMap::new(),
            jobs: JoinSet::new(),
            notifications: VecDeque::new(),
        }
    }

    fn cancel_all(&mut self) {
        for send in self.sends.values_mut() {
            send.cancel.cancel();
            send.progress_rx = None;
        }
    }
}

#[derive(Default)]
struct QueryWorkersState {
    http: http_query::QueryState,
    websocket: websocket_query::QueryState,
}

impl QueryWorkersState {
    fn cancel_all(&mut self) {
        self.http.cancel();
        self.websocket.cancel();
    }

    fn has_workers(&self) -> bool {
        self.http.has_worker() || self.websocket.has_worker()
    }
}

struct AiIpcState {
    repeater_revisions: HashSet<(TabId, u64)>,
    rx: Option<mpsc::Receiver<ai_ipc::AiIpcCall>>,
    server: Option<ai_ipc::AiIpcServer>,
    job_cancel: CancellationToken,
    jobs: JoinSet<ai_ipc::IpcRepeaterCompletion>,
}

impl AiIpcState {
    fn new() -> Self {
        Self {
            repeater_revisions: HashSet::new(),
            rx: None,
            server: None,
            job_cancel: CancellationToken::new(),
            jobs: JoinSet::new(),
        }
    }

    fn refresh_cancel_if_idle(&mut self, parent: &CancellationToken) {
        if self.jobs.is_empty() && self.job_cancel.is_cancelled() {
            self.job_cancel = parent.child_token();
        }
    }
}

struct TlsRuntimeState {
    compiled_client_certs: Arc<[(ClientCertificateRule, mio_proxy::ParsedClientIdentity)]>,
    server_cert_tx: mpsc::Sender<(ProjectKey, ObservedServerCert)>,
    server_cert_rx: mpsc::Receiver<(ProjectKey, ObservedServerCert)>,
    observed_cert_keys: HashSet<(String, u16, String)>,
    tls_failure_tx: mpsc::Sender<(ProjectKey, String, u16)>,
    tls_failure_rx: mpsc::Receiver<(ProjectKey, String, u16)>,
}

impl TlsRuntimeState {
    fn new() -> Self {
        let (server_cert_tx, server_cert_rx) = mpsc::channel(128);
        let (tls_failure_tx, tls_failure_rx) = mpsc::channel(64);
        Self {
            compiled_client_certs: Arc::from([]),
            server_cert_tx,
            server_cert_rx,
            observed_cert_keys: HashSet::new(),
            tls_failure_tx,
            tls_failure_rx,
        }
    }

    fn build_server_cert_callback(
        &self,
        project: ProjectKey,
    ) -> mio_proxy::ServerCertObservedCallback {
        let tx = self.server_cert_tx.clone();
        Arc::new(move |info: mio_proxy::ObservedCertInfo| {
            let _ = tx.try_send((project, ObservedServerCert::from(info)));
        })
    }

    fn build_client_tls_failure_callback(
        &self,
        project: ProjectKey,
    ) -> mio_proxy::ClientTlsFailureCallback {
        let tx = self.tls_failure_tx.clone();
        Arc::new(move |host: String, port: u16| {
            let _ = tx.try_send((project, host, port));
        })
    }
}

struct SessionServiceState {
    runtime: crate::sessions::SessionRuntime,
    trace_rx: mpsc::Receiver<(ProjectKey, SessionTraceEntry)>,
}

impl SessionServiceState {
    fn from_workspace(project: ProjectKey, workspace: &Workspace) -> Self {
        let runtime = crate::sessions::SessionRuntime::from_workspace(project, workspace);
        let (trace_tx, trace_rx) = mpsc::channel(128);
        runtime.set_trace_sender(Some(trace_tx));
        Self { runtime, trace_rx }
    }
}

struct State {
    preferences: crate::preferences::PreferencesStore,
    user_level_connections_cache: UserLevelConnectionsCache,
    project: ProjectKey,
    store: ProjectStore,
    storage: StorageState,
    events: mpsc::Sender<Event>,
    awaiting_event_slot: bool,
    cancel: CancellationToken,
    quiesced: bool,
    automated_tasks_paused: bool,
    extensions_disabled: bool,
    proxy: ProxyRuntimeState,
    repeater: RepeaterState,
    queries: QueryWorkersState,
    ai_ipc: AiIpcState,
    tls: TlsRuntimeState,
    sessions: SessionServiceState,
    last_scheduled_fire: HashMap<Uuid, i64>,
    plugins: plugin_service::PluginState,
    ext_archive: ext_service::ExtState,
    intruder: intruder_service::IntruderState,
    ws_repeater: websocket_repeater::WebSocketRepeaterService,
    sequencer: sequencer_service::SequencerService,
    audit: audit_service::AuditService,
    collaborator: oast_service::CollaboratorService,
}

enum Incoming {
    Command(Box<Command>),
    Proxy(ProxyEvent),
    ProxyFinished(Result<(), mio_proxy::ProxyError>),
    Completed(std::result::Result<Completion, tokio::task::JoinError>),
    RepeaterProgress(TabId),
    Plugin(plugin_service::Signal),
    WebSocketRepeater(WebSocketRepeaterEvent),
    Sequencer(SequencerEvent),
    Audit(audit_service::AuditSignal),
    Collaborator(CollaboratorEvent),
    Ext(ext_service::Signal),
    Intruder(intruder_service::Signal),
    WebSocketQuery(websocket_query::Signal),
    HttpQuery(http_query::Signal),
    AiIpc(ai_ipc::AiIpcCall),
    IpcRepeaterCompleted(
        Box<std::result::Result<ai_ipc::IpcRepeaterCompletion, tokio::task::JoinError>>,
    ),
    SessionTrace((ProjectKey, SessionTraceEntry)),
    ServerCertObserved((ProjectKey, ObservedServerCert)),
    ClientTlsFailure((ProjectKey, String, u16)),
    SchedulerTick,
    Stop,
}

use core_commands::command_context;
use core_repeater_lifecycle::next_repeater_progress;
#[cfg(test)]
use core_repeater_send::finish_repeater_unpack;
use core_runtime::{annotated_flows_tracker, extra_proxy_completion, proxy_completion};

#[cfg(test)]
mod tests {
    use super::*;

    pub(in crate::service) fn completed_flow() -> FlowRecord {
        FlowRecord {
            summary: FlowSummary {
                id: Uuid::new_v4(),
                revision: 2,
                source: TrafficSource::Repeater,
                method: "POST".into(),
                url: "http://localhost/".into(),
                status: Some(200),
                state: FlowState::Complete,
                request_bytes: 3,
                response_bytes: 2,
                started_unix_ms: 1,
                elapsed_ms: 1,
            },
            request: HttpRequest {
                method: "POST".into(),
                url: "http://localhost/".into(),
                version: HttpVersion::Http11,
                headers: vec![],
                body: vec![0, 255, 1],
                trailers: vec![],
            },
            response: Some(HttpResponse {
                status: 200,
                version: HttpVersion::Http11,
                reason_phrase: None,
                headers: vec![],
                body: b"OK".to_vec(),
                trailers: vec![],
            }),
        }
    }
}
