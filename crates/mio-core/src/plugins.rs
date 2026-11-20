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
        HostResult, HttpHookResult, PayloadResult, Permission, SessionActionResult,
        WebSocketHookResult, envelope, host_call, host_result,
    },
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::{Instant, timeout, timeout_at},
};
use tokio_util::sync::CancellationToken;

const QUEUE_CAPACITY: usize = 64;
const MAX_SERVICE_CALLS: usize = 16;
const MAX_OPERATION: Duration = Duration::from_secs(5);
const STOP_GRACE: Duration = Duration::from_millis(150);
const REAP_DEADLINE: Duration = Duration::from_secs(2);
const MAX_MANIFEST_BYTES: usize = 16 * 1024;

mod broker;
mod handle;
mod io;
mod package;
mod prefs_store;
mod running;
mod running_host;
mod runtime;
mod supervisor;
pub mod wasm;

pub use broker::{PermissionBroker, PluginEvent, ServiceOperation, ServiceRequest, ServiceResult};
pub use handle::PluginHandle;
#[cfg(test)]
use package::read_manifest;
pub use package::{NativeTrust, PluginManifest, VerifiedPackage};
pub use prefs_store::{PluginPreferencesStore, default_plugin_preferences_path};
pub use supervisor::PluginSupervisor;
#[cfg(test)]
pub(crate) use supervisor::test_supervisor;

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
    Event(UiEvent),
    Open {
        id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    },
    OpenResponse {
        id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    },
    Commit {
        request: CommitEditor,
        reply: oneshot::Sender<Result<HttpRequest>>,
    },
    CommitResponse {
        request: protocol::CommitResponseEditor,
        reply: oneshot::Sender<Result<HttpResponse>>,
    },
    Revoke {
        id: Uuid,
        reply: oneshot::Sender<Result<()>>,
    },
    Hook {
        permission: Permission,
        body: envelope::Body,
        cancelled: CancellationToken,
        reply: oneshot::Sender<Result<HookReply>>,
    },
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
    Wasm {
        module: wasm::WasmModule,
    },
}

enum PendingKind {
    Event {
        editor_bases: Vec<EditorSnapshot>,
    },
    Close {
        reply: oneshot::Sender<Result<()>>,
    },
    Open {
        resource: Uuid,
        revision: u64,
        read_only: bool,
        reserved: bool,
        reply: oneshot::Sender<Result<Arc<PanelSnapshot>>>,
    },
    Commit {
        resource: Uuid,
        message_revision: u64,
        document: EditorSnapshot,
        reply: oneshot::Sender<Result<HttpRequest>>,
    },
    CommitResponse {
        resource: Uuid,
        message_revision: u64,
        document: EditorSnapshot,
        reply: oneshot::Sender<Result<HttpResponse>>,
    },
    Hook {
        depth: u8,
        cancelled: CancellationToken,
        reply: oneshot::Sender<Result<HookReply>>,
    },
}

struct Pending {
    deadline: Instant,
    kind: PendingKind,
}

struct ServiceCompletion {
    request_id: u64,
    resource_binding: Option<PendingResource>,
    result: std::result::Result<ServiceResult, Failure>,
}

struct ActiveService {
    cancel: CancellationToken,
    binding: Option<PendingResource>,
}

pub(super) enum Wire {
    Frame(Box<Envelope>),
    Failed(&'static str),
}

struct Running {
    identity: ChannelIdentity,
    plugin_id: String,
    panel: PanelState,
    broker: PermissionBroker,
    services: mpsc::Sender<ServiceRequest>,
    events: mpsc::Sender<PluginEvent>,
    cancel: CancellationToken,
    outgoing_sequence: u64,
    incoming_sequence: u64,
    active_resource: Option<(Uuid, u64)>,
    opening_request: Option<u64>,
    rate_epoch: Instant,
    frame_count: usize,
    requests: HashMap<u64, Pending>,
    service_cancels: HashMap<u64, ActiveService>,
    revoked_resources: HashMap<Uuid, u64>,
    service_tasks: JoinSet<ServiceCompletion>,
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
mod tests_runtime;
#[cfg(test)]
mod tests_runtime2;
#[cfg(test)]
mod tests_runtime_body;
