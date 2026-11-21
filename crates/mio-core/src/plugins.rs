//! Trusted native workers. The broker controls MIO services, not the worker's OS access.
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::{
    FlowId, FlowRecord, FlowSummary, HttpRequest, HttpResponse, ProjectKey, Uuid,
    panel::{self, EditorSnapshot, PanelSnapshot, PanelState, UiEvent},
    protocol::{
        self, ChannelIdentity, CommitEditor, Envelope, ErrorCode, Failure, HookAction, HostCall,
        HostResult, HttpHookResult, PANEL_ID, PayloadResult, Permission, SessionActionResult,
        WebSocketHookResult, envelope, host_call, host_result,
    },
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::{Instant, timeout, timeout_at},
};
use tokio_util::sync::CancellationToken;

const QUEUE_CAPACITY: usize = 128;
const CHILD_QUEUE_SOFT_LIMIT: usize = 96;
const OUTGOING_CAPACITY: usize = QUEUE_CAPACITY * 2;
const OUTBOUND_BUFFER_CAPACITY: usize = QUEUE_CAPACITY * 4;
const OUTBOUND_CONTROL_SOFT_LIMIT: usize = OUTBOUND_BUFFER_CAPACITY - MAX_SERVICE_CALLS - 16;
const RESERVED_CONTROL_OUTGOING: usize = 32;
const MAX_RESYNC_RETRIES: u8 = 3;
const MAX_DEFERRED_PANEL_COMMANDS: usize = 32;
const MAX_SERVICE_CALLS: usize = 16;
const MAX_MULTIPLEXED_PANELS: usize = 64;
const MAX_TRACKED_DESTROY_SEQS: usize = 256;
const MAX_TRACKED_REVOKE_SEQS: usize = 512;
const MAX_TRACKED_CANCELLED_OPS: usize = 256;
const MAX_PANEL_REVOKE_SNAPSHOTS: usize = 16;
const MAX_OPERATION: Duration = Duration::from_secs(5);
const STOP_GRACE: Duration = Duration::from_millis(150);
const REAP_DEADLINE: Duration = Duration::from_secs(2);
const MAX_MANIFEST_BYTES: usize = 16 * 1024;

mod archive;
mod broker;
mod control;
mod editor_ops;
mod frame_dispatch;
mod handle;
mod outbound;
mod package;
mod panel_sync;
mod prefs_store;
mod registry;
mod running_host;
mod runtime;
mod supervisor;

pub use broker::{PermissionBroker, PluginEvent, ServiceOperation, ServiceRequest, ServiceResult};
use handle::ControlOrderState;
pub use handle::PluginHandle;
pub(crate) use package::read_manifest;
pub use package::{NativeTrust, PluginManifest, VerifiedPackage};
pub use prefs_store::{PluginPreferencesStore, default_plugin_preferences_path};
pub use registry::{
    InstalledPackageStaging, InstalledPluginEntry, PluginRegistryStore, default_plugins_dir,
};
pub use supervisor::PluginSupervisor;
#[cfg(test)]
pub(crate) use supervisor::{
    spawn_test_duplex_supervisor, spawn_test_multiplex_supervisor, test_supervisor,
};

#[derive(Clone, Copy, Debug)]
struct ResourceBinding {
    revision: u64,
    read_only: bool,
}

#[derive(Clone, Copy)]
enum PendingResource {
    UpdateDraft { id: Uuid, revision: u64 },
    OpenRepeater,
}

enum Control {
    Event {
        panel_id: u64,
        event: UiEvent,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        event_seq: u64,
    },
    Open {
        panel_id: u64,
        id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    },
    OpenResponse {
        panel_id: u64,
        id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    },
    Commit {
        panel_id: u64,
        op_id: Uuid,
        request: CommitEditor,
        cancelled: CancellationToken,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        reply: oneshot::Sender<Result<HttpRequest>>,
    },
    CommitResponse {
        panel_id: u64,
        op_id: Uuid,
        request: protocol::CommitResponseEditor,
        cancelled: CancellationToken,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        reply: oneshot::Sender<Result<HttpResponse>>,
    },
    Revoke {
        panel_id: u64,
        id: Uuid,
        remove_panel: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    RevokeBatch {
        panel_id: u64,
        items: Vec<(Uuid, oneshot::Sender<Result<()>>)>,
    },
    CancelPanelOp {
        panel_id: u64,
        op_id: Uuid,
    },
    Hook {
        permission: Permission,
        body: envelope::Body,
        cancelled: CancellationToken,
        reply: oneshot::Sender<Result<HookReply>>,
    },
}

struct SequencedControl {
    seq: u64,
    command: Control,
}

enum HookReply {
    Http(HttpHookResult),
    Ws(WebSocketHookResult),
    Session(SessionActionResult),
    Payload(PayloadResult),
    ContextMenu(protocol::ContextMenuResult),
    PassiveCheck(protocol::PassiveCheckResult),
    ActiveCheck(protocol::ActiveCheckResult),
}

#[cfg(windows)]
pub(super) struct WindowsJob(isize);

#[cfg(windows)]
unsafe impl Send for WindowsJob {}
#[cfg(windows)]
unsafe impl Sync for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    pub(super) fn create_and_assign(child: &Child) -> Result<Self> {
        #[repr(C)]
        #[derive(Default)]
        struct IoCounters {
            read_operation_count: u64,
            write_operation_count: u64,
            other_operation_count: u64,
            read_transfer_count: u64,
            write_transfer_count: u64,
            other_transfer_count: u64,
        }
        #[repr(C)]
        #[derive(Default)]
        struct BasicLimitInformation {
            per_process_user_time_limit: i64,
            per_job_user_time_limit: i64,
            limit_flags: u32,
            minimum_working_set_size: usize,
            maximum_working_set_size: usize,
            active_process_limit: u32,
            affinity: usize,
            priority_class: u32,
            scheduling_class: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct ExtendedLimitInformation {
            basic_limit_information: BasicLimitInformation,
            io_info: IoCounters,
            process_memory_limit: usize,
            job_memory_limit: usize,
            peak_process_memory_used: usize,
            peak_job_memory_used: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateJobObjectW(attrs: *const std::ffi::c_void, name: *const u16) -> isize;
            fn SetInformationJobObject(
                job: isize,
                class: i32,
                info: *const std::ffi::c_void,
                len: u32,
            ) -> i32;
            fn AssignProcessToJobObject(job: isize, process: *mut std::ffi::c_void) -> i32;
        }
        const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
        const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        ensure!(job != 0, "无法创建 Windows 插件进程组 JobObject");
        let guard = Self(job);
        let mut info = ExtendedLimitInformation::default();
        info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&raw const info).cast(),
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            )
        };
        ensure!(ok != 0, "无法配置 Windows 插件进程组自动回收标志");
        if let Some(raw) = child.raw_handle() {
            let _ = unsafe { AssignProcessToJobObject(job, raw) };
        }
        Ok(guard)
    }

    pub(super) fn terminate(&self) -> Result<()> {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn TerminateJobObject(job: isize, exit_code: u32) -> i32;
        }
        if self.0 != 0 {
            let _ = unsafe { TerminateJobObject(self.0, 1) };
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: isize) -> i32;
        }
        if self.0 != 0 {
            let _ = self.terminate();
            unsafe {
                let _ = CloseHandle(self.0);
            }
            self.0 = 0;
        }
    }
}

enum WorkerTransport {
    Native {
        child: Child,
        #[cfg(windows)]
        job: Option<WindowsJob>,
        stdin: tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
    },
    #[cfg(test)]
    Duplex {
        stdin: tokio::io::DuplexStream,
        stdout: tokio::io::DuplexStream,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerCaptureTransition {
    None,
    Acquired {
        previous: Option<(u64, u64)>,
        target: (u64, u64),
    },
    Released {
        previous: (u64, u64),
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingPointerCancel {
    seq: u64,
    node_id: u64,
    hit_id: u64,
    attempts: u8,
}

enum PendingKind {
    Event {
        editor_bases: Vec<EditorSnapshot>,
        resync_seq: Option<u64>,
        resync_revision: u64,
        resync_snapshot: Option<Arc<PanelSnapshot>>,
        mutated_host_panel: bool,
        pointer_capture: PointerCaptureTransition,
        pointer_cancel_seq: Option<u64>,
        is_editor_changed: bool,
        is_optimistic: bool,
        ui_event_seq: u64,
    },
    Close {
        resource: Uuid,
        remove_panel: bool,
        replies: Vec<oneshot::Sender<Result<()>>>,
    },
    Open {
        resource: Uuid,
        revision: u64,
        read_only: bool,
        reserved: bool,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    },
    Commit {
        op_id: Uuid,
        resource: Uuid,
        message_revision: u64,
        document: EditorSnapshot,
        cancelled: CancellationToken,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        reply: oneshot::Sender<Result<HttpRequest>>,
    },
    CommitResponse {
        op_id: Uuid,
        resource: Uuid,
        message_revision: u64,
        document: EditorSnapshot,
        cancelled: CancellationToken,
        expected_snapshot: Option<Arc<PanelSnapshot>>,
        reply: oneshot::Sender<Result<HttpResponse>>,
    },
    Hook {
        depth: u8,
        cancelled: CancellationToken,
        reply: oneshot::Sender<Result<HookReply>>,
    },
}

impl PendingKind {
    fn is_panel_recovery(&self) -> bool {
        matches!(
            self,
            PendingKind::Event {
                resync_seq: Some(_),
                ..
            } | PendingKind::Event {
                pointer_cancel_seq: Some(_),
                ..
            }
        )
    }
}

struct Pending {
    panel_id: u64,
    panel_revision_at_send: u64,
    deadline: Instant,
    cancel: CancellationToken,
    from_deferred_queue: bool,
    source_seq: u64,
    kind: PendingKind,
}

impl Pending {
    pub(super) fn ui_event_seq(&self) -> u64 {
        match self.kind {
            PendingKind::Event { ui_event_seq, .. } => ui_event_seq,
            _ => 0,
        }
    }
}

struct PendingRevoke {
    panel_id: u64,
    resource: Uuid,
    remove_panel: bool,
    replies: Vec<oneshot::Sender<Result<()>>>,
}

struct ServiceCompletion {
    request_id: u64,
    panel_id: u64,
    resource_binding: Option<PendingResource>,
    result: std::result::Result<ServiceResult, Failure>,
}

struct ActiveService {
    panel_id: u64,
    parent_request_id: u64,
    cancel: CancellationToken,
    binding: Option<PendingResource>,
}

struct CancelNotification {
    panel_id: u64,
    cancel_frame_id: u64,
    deadline: Instant,
    can_restart_resync: bool,
    converged_snapshot_rev: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CancelledStateRequestKind {
    NonOptimistic,
    OptimisticPending,
    OptimisticPromotedByResyncAck,
    OptimisticDetachedSettledByResyncAck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CancelledStateEntry {
    pub(super) kind: CancelledStateRequestKind,
    pub(super) can_restart_resync: bool,
    pub(super) converged_snapshot_rev: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CancelledResyncEntry {
    pub(super) request_id: u64,
    pub(super) can_restart_resync: bool,
    pub(super) converged_snapshot_rev: Option<u64>,
}

#[derive(Default)]
struct PanelSlot {
    state: Option<PanelState>,
    active_resource: Option<(Uuid, u64)>,
    opening_request: Option<u64>,
    closing: bool,
    destroy_failed: bool,
    destroy_request_id: Option<u64>,
    resync_needed_seq: u64,
    resync_attempts: u8,
    resync_semantic_drift: bool,
    resync_snapshot_confirmed: bool,
    last_snapshot_rev: u64,
    pending_pointer_cancels: std::collections::VecDeque<PendingPointerCancel>,
    cancelled_state_requests: std::collections::VecDeque<(u64, CancelledStateEntry)>,
    cancelled_resync_requests: std::collections::VecDeque<CancelledResyncEntry>,
    processed_ui_seq: u64,
    deferred_commands: std::collections::VecDeque<SequencedControl>,
}

pub(super) enum Wire {
    Frame(Box<Envelope>),
    Failed(&'static str),
}

struct Running {
    identity: ChannelIdentity,
    plugin_id: String,
    supports_panel_multiplexing: bool,
    panels: HashMap<u64, PanelSlot>,
    broker: PermissionBroker,
    services: mpsc::Sender<ServiceRequest>,
    events: mpsc::Sender<PluginEvent>,
    cancel: CancellationToken,
    outgoing_sequence: u64,
    incoming_sequence: u64,
    rate_epoch: Instant,
    frame_count: usize,
    requests: HashMap<u64, Pending>,
    cancelled_requests: std::collections::VecDeque<(u64, u64)>,
    cancel_notifications: std::collections::VecDeque<CancelNotification>,
    outbound_queue: std::collections::VecDeque<Envelope>,
    outbound_claims: Option<Arc<OutboundWriteClaims>>,
    pending_revokes: std::collections::VecDeque<PendingRevoke>,
    next_resync_seq: u64,
    pending_panel_syncs: std::collections::BTreeMap<u64, BTreeSet<u64>>,
    defer_panel_sync_for_publish: bool,
    child_headroom_blocked: bool,
    urgent_commands: Option<mpsc::UnboundedReceiver<SequencedControl>>,
    panel_destroy_control_seq: HashMap<u64, u64>,
    panel_destroy_seq_floor: u64,
    resource_revoke_control_seq: HashMap<(u64, Uuid), u64>,
    resource_revoke_seq_floor: u64,
    cancelled_op_control_seq: HashMap<(u64, Uuid), u64>,
    cancelled_op_seq_floor: u64,
    panel_resource_open_seq: HashMap<(u64, Uuid), u64>,
    panel_revoke_snapshots: HashMap<u64, std::collections::VecDeque<(u64, Arc<PanelSnapshot>)>>,
    panel_revoke_snapshot_floor: HashMap<u64, u64>,
    service_cancels: HashMap<u64, ActiveService>,
    panel_revoked_resources: HashMap<(u64, Uuid), u64>,
    service_tasks: JoinSet<ServiceCompletion>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutboundClaim {
    Pending,
    Retracted,
    ExpiredSkipped,
    ShutdownSkipped,
}

#[derive(Default)]
struct OutboundWriteClaimsState {
    entries: HashMap<(u64, u64), OutboundClaim>,
    earliest_fatal_deadline: Option<Instant>,
    fatal_expired_triggered: bool,
    shutting_down: bool,
    stop_cancel: Option<CancellationToken>,
}

#[derive(Default)]
pub(crate) struct OutboundWriteClaims {
    state: std::sync::Mutex<OutboundWriteClaimsState>,
}

pub(crate) struct OutboundWriteClaimsGuard<'a> {
    state: std::sync::MutexGuard<'a, OutboundWriteClaimsState>,
}

impl OutboundWriteClaimsGuard<'_> {
    pub(crate) fn remove(&mut self, panel_id: u64, request_id: u64) {
        self.state.entries.remove(&(panel_id, request_id));
    }

    pub(crate) fn pending_request_ids(&self, panel_id: u64) -> std::collections::HashSet<u64> {
        self.state
            .entries
            .iter()
            .filter_map(|(&(pid, req_id), claim)| {
                (pid == panel_id && *claim == OutboundClaim::Pending).then_some(req_id)
            })
            .collect()
    }

    pub(crate) fn try_retract_batch(&mut self, targets: &[(u64, u64)]) -> Vec<(u64, u64)> {
        let mut retracted = Vec::new();
        for &(panel_id, request_id) in targets {
            match self.state.entries.get_mut(&(panel_id, request_id)) {
                Some(entry) if *entry == OutboundClaim::Pending => {
                    *entry = OutboundClaim::Retracted;
                    retracted.push((panel_id, request_id));
                }
                Some(entry)
                    if matches!(
                        *entry,
                        OutboundClaim::ExpiredSkipped | OutboundClaim::ShutdownSkipped
                    ) =>
                {
                    self.state.entries.remove(&(panel_id, request_id));
                    retracted.push((panel_id, request_id));
                }
                _ => {}
            }
        }
        retracted
    }
}

impl OutboundWriteClaims {
    pub(crate) fn lock_barrier(&self) -> OutboundWriteClaimsGuard<'_> {
        OutboundWriteClaimsGuard {
            state: self.state.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    pub(crate) fn set_stop_cancel(&self, cancel: CancellationToken) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stop_cancel = Some(cancel);
    }

    pub(crate) fn register(&self, panel_id: u64, request_id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let claim = if state.shutting_down
            || state
                .stop_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            OutboundClaim::Retracted
        } else {
            OutboundClaim::Pending
        };
        state.entries.insert((panel_id, request_id), claim);
    }

    pub(crate) fn remove(&self, panel_id: u64, request_id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.entries.remove(&(panel_id, request_id));
    }

    pub(crate) fn set_earliest_fatal_deadline(&self, deadline: Option<Instant>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.earliest_fatal_deadline = deadline;
    }

    pub(crate) fn is_fatal_expired_triggered(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.fatal_expired_triggered
    }

    pub(crate) fn update_fatal_deadline_unless_triggered(&self, deadline: Option<Instant>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.fatal_expired_triggered {
            return false;
        }
        state.earliest_fatal_deadline = deadline;
        true
    }

    pub(crate) fn try_retract(&self, panel_id: u64, request_id: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match state.entries.get_mut(&(panel_id, request_id)) {
            Some(entry) if *entry == OutboundClaim::Pending => {
                *entry = OutboundClaim::Retracted;
                true
            }
            Some(entry)
                if matches!(
                    *entry,
                    OutboundClaim::ExpiredSkipped | OutboundClaim::ShutdownSkipped
                ) =>
            {
                state.entries.remove(&(panel_id, request_id));
                true
            }
            _ => false,
        }
    }

    pub(crate) fn claim_for_write(&self, panel_id: u64, request_id: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let stopping = state.shutting_down
            || state
                .stop_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled);
        let fatal_expired = state.fatal_expired_triggered
            || state
                .earliest_fatal_deadline
                .is_some_and(|deadline| Instant::now() >= deadline);
        match state.entries.get_mut(&(panel_id, request_id)) {
            Some(entry) if *entry == OutboundClaim::Pending && stopping => {
                *entry = OutboundClaim::ShutdownSkipped;
                state.shutting_down = true;
                false
            }
            Some(entry) if *entry == OutboundClaim::Pending && fatal_expired => {
                *entry = OutboundClaim::ExpiredSkipped;
                state.fatal_expired_triggered = true;
                false
            }
            Some(_) => matches!(
                state.entries.remove(&(panel_id, request_id)),
                Some(OutboundClaim::Pending)
            ),
            None => true,
        }
    }

    pub(crate) fn retract_all_pending(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shutting_down = true;
        state.entries.retain(|_, entry| match *entry {
            OutboundClaim::Pending => {
                *entry = OutboundClaim::Retracted;
                true
            }
            OutboundClaim::Retracted => true,
            OutboundClaim::ExpiredSkipped | OutboundClaim::ShutdownSkipped => false,
        });
    }

    pub(crate) fn retract_all_pending_returning_already_claimed(
        &self,
        targets: &[(u64, u64)],
    ) -> Vec<(u64, u64)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shutting_down = true;
        let mut already_claimed = Vec::new();
        for &(panel_id, request_id) in targets {
            match state.entries.get_mut(&(panel_id, request_id)) {
                Some(entry) if *entry == OutboundClaim::Pending => {
                    *entry = OutboundClaim::Retracted;
                }
                Some(entry)
                    if matches!(
                        *entry,
                        OutboundClaim::ExpiredSkipped | OutboundClaim::ShutdownSkipped
                    ) =>
                {
                    state.entries.remove(&(panel_id, request_id));
                }
                Some(entry) if *entry == OutboundClaim::Retracted => {}
                Some(_) => unreachable!(),
                None => already_claimed.push((panel_id, request_id)),
            }
        }
        state.entries.retain(|_, entry| match *entry {
            OutboundClaim::Pending => {
                *entry = OutboundClaim::Retracted;
                true
            }
            OutboundClaim::Retracted => true,
            OutboundClaim::ExpiredSkipped | OutboundClaim::ShutdownSkipped => false,
        });
        already_claimed
    }
}

async fn read_async_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Envelope> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix).await?;
    let mut payload = vec![0; protocol::frame_length(prefix)?];
    reader.read_exact(&mut payload).await?;
    Ok(protocol::decode_frame(&payload)?)
}

fn kill_worker_group(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    {
        let pid = child
            .id()
            .filter(|pid| *pid > 1 && *pid <= i32::MAX as u32)
            .context("插件进程已回收，不能安全定位原进程组")?;
        let result = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        let error = (result != 0).then(std::io::Error::last_os_error);
        let _ = child.start_kill();
        match error {
            Some(error) if error.raw_os_error() != Some(libc::ESRCH) => {
                Err(error).context("无法终止插件进程组")
            }
            _ => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        let _ = child.start_kill();
        Ok(())
    }
}

#[cfg(test)]
mod prefs_store_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_package;
#[cfg(test)]
mod tests_registry;
#[cfg(test)]
mod tests_runtime;
#[cfg(test)]
mod tests_runtime2;
#[cfg(test)]
mod tests_runtime_body;
