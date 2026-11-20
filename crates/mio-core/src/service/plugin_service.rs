use std::{future::Future, path::{Path, PathBuf}, pin::Pin, task::Poll};

use mio_plugin_api::{
    panel::PanelSnapshot,
    protocol::{ChannelIdentity, CommitEditor, ErrorCode, Failure, Permission},
};
use tokio::task::{JoinError, JoinHandle};

use super::*;
use crate::plugins::{
    InstalledPackageStaging, InstalledPluginEntry, NativeTrust, PluginEvent,
    PluginPreferencesStore, PluginRegistryStore, PluginSupervisor, ServiceOperation,
    ServiceRequest, ServiceResult, VerifiedPackage,
};

const MAX_INSTANCES: usize = 16;
const MAX_OPERATIONS: usize = 32;
const MAX_NETWORK: usize = 16;
const MAX_WORKSPACE_REQUESTS: usize = 32;
const MAX_RESOURCES: usize = 64;
type CallKey = (Uuid, u64);

struct Instance {
    worker: PluginSupervisor,
    services: mpsc::Receiver<ServiceRequest>,
    package: VerifiedPackage,
    manifest_modified: SystemTime,
    manifest_size: u64,
    entry_modified: SystemTime,
    entry_size: u64,
    auto_reload: bool,
    trust: NativeTrust,
    info: LoadedPlugin,
    panel: std::sync::Arc<PanelSnapshot>,
    source: Option<Uuid>,
    resources: HashMap<Uuid, u64>,
}

struct PersistInstallCommit {
    staging: InstalledPackageStaging,
    original_source: Option<PathBuf>,
}

struct Candidate {
    instance: Instance,
    replace: Option<Uuid>,
    reloaded: bool,
    autoload: bool,
    enable_installed: bool,
    persist_install: Option<PersistInstallCommit>,
    open: Option<(TabId, u64)>,
    context_open: Option<(Uuid, u64, bool, HttpRequest)>,
    response_open: Option<(Uuid, u64, bool, HttpResponse)>,
}

enum LaunchTarget {
    Suite {
        replace: Option<Uuid>,
        reloaded: bool,
    },
    Editor {
        source: Uuid,
        tab_id: TabId,
        revision: u64,
    },
    ContextViewer {
        source: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    },
    ResponseViewer {
        source: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    },
}

struct Operation {
    project: ProjectKey,
    instance: Option<Uuid>,
    cancel: CancellationToken,
    task: tokio::task::Id,
    report: bool,
    cleanup_failed: bool,
    background_passive: bool,
    replaces_existing: bool,
    autoload_plugin_id: Option<String>,
    binding: Option<(TabId, u64, Option<u64>)>,
}

struct Network {
    call: ServiceRequest,
    flow_id: FlowId,
    task: JoinHandle<FlowRecord>,
}

struct PendingWorkspace {
    call: ServiceRequest,
    expired: Pin<Box<dyn Future<Output = ()> + Send>>,
}

struct Retired {
    identity: ChannelIdentity,
    result: Result<(), String>,
    network: Vec<(ServiceRequest, FlowId, Result<FlowRecord, String>)>,
}

enum Output {
    Inspected(VerifiedPackage),
    Started(Box<Candidate>),
    Opened {
        identity: ChannelIdentity,
        tab_id: TabId,
        revision: u64,
        loading: bool,
        panel: std::sync::Arc<PanelSnapshot>,
    },
    Committed {
        identity: ChannelIdentity,
        tab_id: TabId,
        revision: u64,
        request: HttpRequest,
    },
    ContextOpened {
        identity: ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        panel: std::sync::Arc<PanelSnapshot>,
    },
    ContextCommitted {
        identity: ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        request: HttpRequest,
    },
    ResponseOpened {
        identity: ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        panel: std::sync::Arc<PanelSnapshot>,
    },
    ResponseCommitted {
        identity: ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        response: HttpResponse,
    },
    ContextMenuInvoked {
        identity: ChannelIdentity,
        status_message: String,
        log_line: String,
        open_repeater: Option<HttpRequest>,
        replace_request: Option<Vec<u8>>,
    },
    PayloadsGenerated {
        identity: ChannelIdentity,
        tab_id: TabId,
        payloads: Vec<Vec<u8>>,
    },
    PayloadsProcessed {
        identity: ChannelIdentity,
        tab_id: TabId,
        position: Option<usize>,
        payloads: Vec<Vec<u8>>,
    },
    PassiveChecked {
        project: ProjectKey,
        flow_id: FlowId,
        url: String,
        response: Box<HttpResponse>,
        findings: Vec<protocol::PluginFindingDto>,
    },
    Retired {
        retired: Vec<Retired>,
        candidate: Option<Box<Candidate>>,
        disabled: Option<ChannelIdentity>,
        uninstalled_plugin_id: Option<String>,
        reason: String,
    },
    Revoked,
}

pub(super) struct Completion {
    operation_id: Uuid,
    result: Result<Output, String>,
}

pub(super) enum Signal {
    Job(Result<Completion, JoinError>),
    Worker(PluginEvent),
    Service(ServiceRequest),
    Network(CallKey, Result<FlowRecord, JoinError>),
    WorkspaceExpired(CallKey),
    PassiveFlow(ProjectKey, Box<FlowRecord>),
    AutoReloadTick,
}

pub(super) struct PluginState {
    pub(crate) prefs: PluginPreferencesStore,
    pub(crate) registry: PluginRegistryStore,
    pub(crate) passive_tx: mpsc::Sender<(ProjectKey, Box<FlowRecord>)>,
    passive_rx: mpsc::Receiver<(ProjectKey, Box<FlowRecord>)>,
    last_auto_reload_check: Option<Instant>,
    auto_reload_timer: Option<Pin<Box<tokio::time::Sleep>>>,
    inspections: HashMap<Uuid, VerifiedPackage>,
    instances: HashMap<Uuid, Instance>,
    order: Vec<Uuid>,
    context_viewers: HashMap<Uuid, (Uuid, u64, bool)>,
    operations: HashMap<Uuid, Operation>,
    jobs: JoinSet<Completion>,
    network: HashMap<CallKey, Network>,
    workspace: HashMap<CallKey, PendingWorkspace>,
    stopping: bool,
    cleanup_error: Option<String>,
}

impl Default for PluginState {
    fn default() -> Self {
        Self::with_preferences(PluginPreferencesStore::default())
    }
}

mod commands;
pub(super) mod hooks;
mod host_service;
mod jobs;
mod lifecycle;
pub(in crate::service) mod passive;
mod payloads;
mod state;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_r4;
#[cfg(test)]
mod tests_r4_passive;
#[cfg(test)]
mod tests_v14_hostcalls;
mod viewers;
mod workspace;

use lifecycle::{retire, retire_with_uninstall};
