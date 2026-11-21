use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::Arc,
    sync::Mutex,
    sync::RwLock,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

struct State {
    preferences: crate::preferences::PreferencesStore,
    project: ProjectKey,
    store: ProjectStore,
    events: mpsc::Sender<Event>,
    awaiting_event_slot: bool,
    cancel: CancellationToken,
    proxy_events: mpsc::Receiver<ProxyEvent>,
    proxy_sender: mpsc::Sender<ProxyEvent>,
    proxy_notifications: VecDeque<Event>,
    proxy: Option<ProxyHandle>,
    primary_proxy_id: Option<Uuid>,
    ca: Option<MitmAuthority>,
    intercept_requests: bool,
    intercept_responses: bool,
    pending: HashMap<FlowId, Pending>,
    pending_websockets: HashMap<Uuid, PendingWebSocket>,
    sends: HashMap<TabId, ActiveSend>,
    jobs: JoinSet<Completion>,
    repeater_notifications: VecDeque<Event>,
    history_dirty: bool,
    websocket_history_dirty: bool,
    // The first fault stops producers; only the bounded event queue and active jobs can add records.
    recovery: VecDeque<CapturedFlow>,
    websocket_recovery: VecDeque<CapturedWebSocket>,
    websocket_query: websocket_query::QueryState,
    http_query: http_query::QueryState,
    storage_error: Option<String>,
    quiesced: bool,
    extra_proxies: Vec<(Uuid, ProxyHandle)>,
    annotated_flows: Arc<RwLock<HashSet<FlowId>>>,
    compiled_client_certs: Arc<[(ClientCertificateRule, mio_proxy::ParsedClientIdentity)]>,
    provenance: ProvenanceTracker,
    ipc_repeater_revisions: HashSet<(TabId, u64)>,
    ai_ipc_rx: Option<mpsc::Receiver<ai_ipc::AiIpcCall>>,
    ai_ipc_server: Option<ai_ipc::AiIpcServer>,
    ipc_job_cancel: CancellationToken,
    ipc_jobs: JoinSet<ai_ipc::IpcRepeaterCompletion>,
    sessions: crate::sessions::SessionRuntime,
    session_trace_rx: mpsc::Receiver<(ProjectKey, SessionTraceEntry)>,
    server_cert_tx: mpsc::Sender<(ProjectKey, ObservedServerCert)>,
    server_cert_rx: mpsc::Receiver<(ProjectKey, ObservedServerCert)>,
    observed_cert_keys: HashSet<(String, u16, String)>,
    tls_failure_tx: mpsc::Sender<(ProjectKey, String, u16)>,
    tls_failure_rx: mpsc::Receiver<(ProjectKey, String, u16)>,
    last_scheduled_fire: HashMap<Uuid, i64>,
    extensions_disabled: bool,
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
        std::result::Result<ai_ipc::IpcRepeaterCompletion, tokio::task::JoinError>,
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
