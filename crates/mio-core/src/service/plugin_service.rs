use std::{collections::HashSet, future::Future, path::{Path, PathBuf}, pin::Pin, task::Poll};

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
const MAX_CHILD_EDITORS: usize = 64;
const MAX_OPERATIONS: usize = 64;
const MAX_NETWORK: usize = 16;
const MAX_WORKSPACE_REQUESTS: usize = 32;
const MAX_RESOURCES: usize = 64;
type CallKey = (Uuid, u64);

struct MultiplexedChild {
    source_instance: Uuid,
    panel_id: u64,
    primary_resource: Uuid,
    info: LoadedPlugin,
    panel: std::sync::Arc<PanelSnapshot>,
    resources: HashMap<Uuid, u64>,
}

struct Instance {
    worker: PluginSupervisor,
    services: mpsc::Receiver<ServiceRequest>,
    package: VerifiedPackage,
    manifest_modified: SystemTime,
    manifest_size: u64,
    entry_modified: SystemTime,
    entry_size: u64,
    auto_reload: bool,
    hook_filter: PluginHookFilter,
    hook_counters: std::sync::Arc<PluginHookCounters>,
    info: LoadedPlugin,
    panel: std::sync::Arc<PanelSnapshot>,
    resources: HashMap<Uuid, u64>,
}

#[derive(Debug, Default)]
pub(in crate::service) struct PluginHookCounters {
    invoked: std::sync::atomic::AtomicU64,
    filtered_skipped: std::sync::atomic::AtomicU64,
    oversized_bypassed: std::sync::atomic::AtomicU64,
    errors_or_timeouts: std::sync::atomic::AtomicU64,
    fail_open_passes: std::sync::atomic::AtomicU64,
}

impl PluginHookCounters {
    pub(in crate::service) fn snapshot(&self) -> PluginHookMetricsSnapshot {
        use std::sync::atomic::Ordering::Relaxed;
        PluginHookMetricsSnapshot {
            invoked: self.invoked.load(Relaxed),
            filtered_skipped: self.filtered_skipped.load(Relaxed),
            oversized_bypassed: self.oversized_bypassed.load(Relaxed),
            errors_or_timeouts: self.errors_or_timeouts.load(Relaxed),
            fail_open_passes: self.fail_open_passes.load(Relaxed),
        }
    }

    pub(in crate::service) fn inc_invoked(&self) {
        self.invoked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(in crate::service) fn inc_filtered_skipped(&self) {
        self.filtered_skipped
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(in crate::service) fn inc_oversized_bypassed(&self) {
        self.oversized_bypassed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(in crate::service) fn inc_error_or_timeout(&self) {
        self.errors_or_timeouts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(in crate::service) fn inc_fail_open_pass(&self) {
        self.fail_open_passes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
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
}

struct Operation {
    project: ProjectKey,
    instance: Option<Uuid>,
    cancel: CancellationToken,
    task: tokio::task::Id,
    report: bool,
    cleanup_failed: bool,
    background_passive: bool,
    is_suite_start: bool,
    replaces_existing: bool,
    autoload_plugin_id: Option<String>,
    binding: Option<(TabId, u64, Option<u64>)>,
    multiplexed_cleanup_on_fail: Option<Uuid>,
    closing_panel: Option<(Uuid, u64)>,
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
        retired_multiplexed: Vec<ChannelIdentity>,
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
    AutoReloadTick,
}

pub(super) struct PluginState {
    pub(crate) prefs: PluginPreferencesStore,
    pub(crate) registry: PluginRegistryStore,
    last_auto_reload_check: Option<Instant>,
    auto_reload_timer: Option<Pin<Box<tokio::time::Sleep>>>,
    inspections: HashMap<Uuid, VerifiedPackage>,
    instances: HashMap<Uuid, Instance>,
    multiplexed: HashMap<Uuid, MultiplexedChild>,
    panel_routes: HashMap<(Uuid, u64), Uuid>,
    closing_panels: HashSet<(Uuid, u64)>,
    next_panel_id: u64,
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
mod tests_multiplex;
#[cfg(test)]
mod tests_r4;
#[cfg(test)]
mod tests_r4_passive;
#[cfg(test)]
mod tests_v14_hostcalls;
mod viewers;
mod workspace;

use lifecycle::{retire, retire_with_uninstall};
