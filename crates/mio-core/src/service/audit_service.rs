use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::{
    FlowRecord, FlowState, FlowSummary, HttpRequest, HttpResponse, TrafficSource,
};
use mio_proxy::{SendConfig, prepare_send, send};
use tokio::{
    sync::{Semaphore, mpsc, watch},
    task::{AbortHandle, Id, JoinError, JoinSet},
    time::{Instant as TokioInstant, timeout_at},
};
use tokio_util::sync::CancellationToken;

use super::State;
use crate::{
    ACTIVE_CANARY, ACTIVE_REDIRECT_TARGET, ACTIVE_SPLIT_HEADER, AuditCommand, AuditEvent,
    AuditFinding, AuditStateSnapshot, AuditTaskMode, AuditTaskState, AuditTaskSummary,
    CustomScanCheck, Draft, Event, FlowId, InsertionPoint, MAX_AUDIT_TASKS, ProjectKey,
    TargetScope, Uuid,
    audit_engine::{
        CrawlConfig, build_probe_request, discover_same_origin_links_and_forms,
        evaluate_active_check, evaluate_custom_scan_checks, extract_insertion_points,
        inspect_passive,
    },
    http,
    store::audit::merge_audit_findings,
};

const SIGNAL_CHANNEL_CAPACITY: usize = 64;
const ACTIVE_CONCURRENCY: usize = 4;
const MAX_CONCURRENT_SCANS: usize = 8;
const MAX_OAST_PROBES: usize = 256;
const STOP_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub(super) struct OastProbeRecord {
    pub(super) project: ProjectKey,
    pub(super) token: String,
    pub(super) callback_url: String,
    pub(super) task_id: Uuid,
    pub(super) seed_flow_id: Option<FlowId>,
    pub(super) target_url: String,
}

pub(super) struct AuditSignal {
    project: ProjectKey,
    task_id: Uuid,
    kind: AuditSignalKind,
}

enum AuditSignalKind {
    Step {
        requests_sent: u64,
        flow: Option<Box<FlowRecord>>,
        findings: Vec<AuditFinding>,
    },
    Finished {
        requests_sent: u64,
        state: AuditTaskState,
        error: Option<String>,
    },
}

struct ScanPlan {
    project: ProjectKey,
    task_id: Uuid,
    request: HttpRequest,
    config: SendConfig,
    mode: AuditTaskMode,
    crawl_same_origin: bool,
    scope: TargetScope,
    insertion_points: Vec<InsertionPoint>,
    unpack_compressed_responses: bool,
    oast_callback_url: Option<String>,
    plugin_active_hooks: Vec<crate::plugins::PluginHandle>,
    custom_scan_checks: Vec<CustomScanCheck>,
    sessions: crate::sessions::SessionRuntime,
}

struct ActiveScanJob {
    project: ProjectKey,
    task_id: Uuid,
    tokio_id: Id,
    cancel: CancellationToken,
    pause_tx: watch::Sender<bool>,
    abort: AbortHandle,
    requests_sent: u64,
}

struct JobDone {
    project: ProjectKey,
    task_id: Uuid,
    requests_sent: u64,
    state: AuditTaskState,
    error: Option<String>,
}

pub(super) struct AuditService {
    active: HashMap<Uuid, ActiveScanJob>,
    jobs: JoinSet<JobDone>,
    signal_tx: mpsc::Sender<AuditSignal>,
    signal_rx: mpsc::Receiver<AuditSignal>,
    pending_signals: VecDeque<AuditSignal>,
    oast_probes: Vec<OastProbeRecord>,
}

impl Default for AuditService {
    fn default() -> Self {
        let (signal_tx, signal_rx) = mpsc::channel(SIGNAL_CHANNEL_CAPACITY);
        Self {
            active: HashMap::new(),
            jobs: JoinSet::new(),
            signal_tx,
            signal_rx,
            pending_signals: VecDeque::new(),
            oast_probes: Vec::new(),
        }
    }
}

mod active;
mod command;
mod crawl;
mod oast;
mod state;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_crawl;
#[cfg(test)]
mod tests_oast;

use active::{now_unix_ms, run_scan};
