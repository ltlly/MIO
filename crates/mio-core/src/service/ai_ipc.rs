#[cfg(unix)]
use std::os::unix::{
    fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    net::UnixStream as StdUnixStream,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
#[cfg(not(unix))]
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    FlowProvenance, FlowSummary, HttpRequest, HttpResponse, SlimSaveOptions, SlimSaveReport, TabId,
    Uuid, WebSocketMessage,
};

mod handlers;

pub const ENV_MIO_IPC_SOCKET: &str = "MIO_IPC_SOCKET";
#[cfg(unix)]
const MAX_SUN_PATH_SAFE_LEN: usize = 96;
const MAX_IPC_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONCURRENT_IPC_CLIENTS: usize = 16;
#[cfg(not(unix))]
const MAX_UNAUTHENTICATED_HANDSHAKES: usize = 64;
const IPC_IO_TIMEOUT: Duration = Duration::from_secs(30);
const IPC_AUTH_TIMEOUT: Duration = Duration::from_secs(2);
const IPC_REPEATER_REPLY_TIMEOUT: Duration = Duration::from_secs(305);

fn default_flows_limit() -> usize {
    50
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AiIpcRequest {
    Status,
    FlowsList {
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_flows_limit")]
        limit: usize,
        #[serde(default)]
        filter: String,
        #[serde(default)]
        method: Option<String>,
        #[serde(default)]
        status: Option<u16>,
    },
    FlowsGet {
        flow_id: Uuid,
    },
    RepeaterOpen {
        #[serde(default)]
        flow_id: Option<Uuid>,
        #[serde(default)]
        use_original: bool,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        raw_request: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    RepeaterSend {
        #[serde(default)]
        flow_id: Option<Uuid>,
        #[serde(default)]
        use_original: bool,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        raw_request: Option<String>,
        #[serde(default)]
        add_headers: Vec<(String, String)>,
        #[serde(default)]
        remove_headers: Vec<String>,
        #[serde(default)]
        override_body: Option<String>,
        #[serde(default)]
        insecure: bool,
    },
    SlimSave {
        path: String,
        #[serde(default)]
        options: Option<SlimSaveOptions>,
    },
    ExportCa {
        path: String,
        #[serde(default)]
        pem: Option<bool>,
    },
    ProxyStart {
        #[serde(default)]
        bind_address: Option<String>,
    },
    ProxyStop,
    ProxyIntercept {
        #[serde(default)]
        requests: Option<bool>,
        #[serde(default)]
        responses: Option<bool>,
    },
    ProxyPending,
    ProxyDecide {
        flow_id: Uuid,
        revision: u64,
        drop_flow: bool,
        #[serde(default)]
        add_headers: Vec<(String, String)>,
        #[serde(default)]
        remove_headers: Vec<String>,
        #[serde(default)]
        override_body: Option<String>,
        #[serde(default)]
        override_status: Option<u16>,
    },
    WorkspaceGet,
    WorkspacePatch {
        workspace_json: String,
    },
    WebSocketList {
        #[serde(default = "default_flows_limit")]
        limit: usize,
    },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiIpcEnvelope {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub payload: Option<AiIpcPayload>,
}

impl AiIpcEnvelope {
    pub fn ok(payload: AiIpcPayload) -> Self {
        Self {
            ok: true,
            error: None,
            payload: Some(payload),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            payload: None,
        }
    }

    pub fn err_json(message: impl Into<String>) -> String {
        serde_json::to_string(&Self::err(message))
            .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"IPC 错误\"}".into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiIpcFlowDetail {
    pub summary: FlowSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_raw_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_raw_text: Option<String>,
    pub request: HttpRequest,
    pub response: Option<HttpResponse>,
    pub original_request: Option<HttpRequest>,
    pub original_response: Option<HttpResponse>,
    pub provenance: FlowProvenance,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiIpcPendingItem {
    pub flow_id: Uuid,
    pub revision: u64,
    pub stage: String,
    pub response_head_only: bool,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AiIpcPayload {
    Status {
        project_id: Uuid,
        project_generation: u64,
        proxy_address: Option<String>,
        #[serde(default)]
        intercept_requests: bool,
        #[serde(default)]
        intercept_responses: bool,
        total_flows: usize,
        project_disk_bytes: u64,
        process_memory_bytes: u64,
    },
    FlowsList {
        total: usize,
        flows: Vec<FlowSummary>,
    },
    FlowsGet(Box<AiIpcFlowDetail>),
    RepeaterOpen {
        tab_id: TabId,
        title: String,
        target: String,
    },
    SlimSave {
        report: SlimSaveReport,
    },
    CaExported {
        ca_path: String,
        ca_format: String,
        ca_subject: String,
    },
    ProxyControl {
        proxy_running: bool,
        proxy_address: Option<String>,
        intercept_requests: bool,
        intercept_responses: bool,
    },
    ProxyPending {
        pending_items: Vec<AiIpcPendingItem>,
    },
    ProxyDecided {
        decided_flow_id: Uuid,
        dropped: bool,
    },
    WorkspaceState {
        workspace: serde_json::Value,
    },
    WebSocketList {
        ws_total: usize,
        ws_messages: Vec<WebSocketMessage>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ws_messages_text: Vec<String>,
    },
    Shutdown {
        shutting_down: bool,
    },
}

pub(in crate::service) struct AiIpcCall {
    pub request: AiIpcRequest,
    pub reply: oneshot::Sender<AiIpcEnvelope>,
}

pub(in crate::service) struct IpcRepeaterOutcome {
    pub initial_pending: Option<crate::FlowRecord>,
    pub mutated_pending: Option<crate::FlowRecord>,
    pub received: Option<crate::FlowRecord>,
    pub record: crate::FlowRecord,
    pub original_request: Option<HttpRequest>,
    pub original_response: Option<HttpResponse>,
    pub provenance: FlowProvenance,
}

pub(in crate::service) struct IpcRepeaterCompletion {
    pub project: crate::ProjectKey,
    pub reply: oneshot::Sender<AiIpcEnvelope>,
    pub outcome: Result<IpcRepeaterOutcome>,
}

struct OwnedSocketGuard {
    path: PathBuf,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(not(unix))]
    token: String,
}

impl Drop for OwnedSocketGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(meta) = fs::symlink_metadata(&self.path)
            && meta.file_type().is_socket()
            && meta.dev() == self.dev
            && meta.ino() == self.ino
        {
            let _ = fs::remove_file(&self.path);
        }
        #[cfg(not(unix))]
        if let Ok(raw) = fs::read_to_string(&self.path)
            && raw.contains(&self.token)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(in crate::service) struct AiIpcServer {
    cancel: CancellationToken,
    accept_task: Option<JoinHandle<()>>,
    guard: Option<OwnedSocketGuard>,
}

impl AiIpcServer {
    pub async fn shutdown(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.accept_task.take() {
            let _ = task.await;
        }
        self.guard.take();
    }
}

#[cfg(unix)]
fn current_euid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and is thread-safe on POSIX/macOS.
    unsafe { libc::geteuid() }
}

pub fn env_ipc_socket_path() -> Option<PathBuf> {
    std::env::var_os(ENV_MIO_IPC_SOCKET)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn default_ipc_socket_path() -> PathBuf {
    if let Some(explicit) = env_ipc_socket_path() {
        return explicit;
    }
    #[cfg(unix)]
    {
        if let Some(sock) = crate::fs_atomic::platform_runtime_dir().map(|d| d.join("mio.sock"))
            && sock.as_os_str().as_encoded_bytes().len() <= MAX_SUN_PATH_SAFE_LEN
        {
            return sock;
        }
        PathBuf::from(format!("/tmp/mio-{}/mio.sock", current_euid()))
    }
    #[cfg(not(unix))]
    {
        if let Some(dir) = crate::fs_atomic::platform_runtime_dir() {
            return dir.join("mio.sock");
        }
        std::env::temp_dir().join("mio-run").join("mio.sock")
    }
}

#[cfg(unix)]
fn ensure_private_parent_dir(parent: &Path, strict_owner_mode: bool) -> Result<()> {
    if !parent.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .with_context(|| format!("无法创建 IPC 目录 {}", parent.display()))?;
    }
    if strict_owner_mode {
        let meta = fs::symlink_metadata(parent)
            .with_context(|| format!("无法检查 IPC 目录 {}", parent.display()))?;
        ensure!(
            meta.file_type().is_dir(),
            "IPC 目录 {} 不是真实目录（拒绝符号链接）",
            parent.display()
        );
        let euid = current_euid();
        ensure!(
            meta.uid() == euid,
            "IPC 目录 {} 所有者 ({}) 与当前用户 ({euid}) 不一致",
            parent.display(),
            meta.uid()
        );
        if (meta.mode() & 0o077) != 0 {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("无法收紧 IPC 目录权限 {}", parent.display()))?;
            let refreshed = fs::symlink_metadata(parent)?;
            ensure!(
                (refreshed.mode() & 0o077) == 0,
                "IPC 目录 {} 权限仍对其它用户开放",
                parent.display()
            );
        }
    } else {
        let meta = fs::metadata(parent)
            .with_context(|| format!("IPC 父目录不存在或不可访问：{}", parent.display()))?;
        ensure!(meta.is_dir(), "IPC 父路径不是目录：{}", parent.display());
    }
    Ok(())
}

#[cfg(unix)]
fn bind_unix_socket(path: &Path, strict_parent: bool) -> Result<(UnixListener, OwnedSocketGuard)> {
    let raw_bytes = path.as_os_str().as_encoded_bytes();
    ensure!(
        !raw_bytes.is_empty() && !raw_bytes.contains(&0) && raw_bytes.len() <= 100,
        "IPC socket 路径无效或超过 Unix domain socket 长度上限：{}",
        path.display()
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure_private_parent_dir(parent, strict_parent)?;

    let euid = current_euid();
    if let Ok(existing) = fs::symlink_metadata(path) {
        ensure!(
            existing.file_type().is_socket(),
            "IPC 路径 {} 已存在且不是 Unix socket，拒绝覆盖",
            path.display()
        );
        ensure!(
            existing.uid() == euid,
            "IPC socket {} 不属于当前用户",
            path.display()
        );
        if StdUnixStream::connect(path).is_ok() {
            bail!(
                "IPC socket {} 已被另一个运行中的 MIO 实例占用",
                path.display()
            );
        }
        fs::remove_file(path)
            .with_context(|| format!("无法清理陈旧 IPC socket {}", path.display()))?;
    }

    let std_listener = std::os::unix::net::UnixListener::bind(path)
        .with_context(|| format!("无法绑定 IPC socket {}", path.display()))?;
    if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(path);
        return Err(err)
            .with_context(|| format!("无法设置 IPC socket 0600 权限 {}", path.display()));
    }
    std_listener.set_nonblocking(true)?;
    let bound_meta = fs::symlink_metadata(path)?;
    let listener = UnixListener::from_std(std_listener)?;
    let guard = OwnedSocketGuard {
        path: path.to_path_buf(),
        dev: bound_meta.dev(),
        ino: bound_meta.ino(),
    };
    Ok((listener, guard))
}

#[cfg(not(unix))]
#[derive(Serialize, Deserialize)]
struct WindowsIpcEndpoint {
    addr: String,
    token: String,
}

#[cfg(not(unix))]
fn bind_tcp_endpoint(
    path: &Path,
    _strict_parent: bool,
) -> Result<(TcpListener, String, OwnedSocketGuard)> {
    let raw_bytes = path.as_os_str().as_encoded_bytes();
    ensure!(
        !raw_bytes.is_empty() && !raw_bytes.contains(&0) && raw_bytes.len() <= 4096,
        "IPC 端点路径无效：{}",
        path.display()
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("无法创建 IPC 目录 {}", parent.display()))?;
    if let Ok(existing_meta) = fs::symlink_metadata(path) {
        ensure!(
            existing_meta.is_file() && !existing_meta.file_type().is_symlink(),
            "IPC 路径 {} 已存在且不是普通端点文件，拒绝覆盖",
            path.display()
        );
        ensure!(
            existing_meta.len() <= 4096,
            "IPC 路径 {} 已存在且不是 MIO 端点描述文件，拒绝删除",
            path.display()
        );
        let bytes = fs::read(path)
            .with_context(|| format!("无法读取已存在的 IPC 路径 {}", path.display()))?;
        let parsed: WindowsIpcEndpoint = serde_json::from_slice(&bytes)
            .with_context(|| format!("IPC 路径 {} 不是有效的 MIO 端点描述文件，拒绝删除", path.display()))?;
        ensure!(
            !parsed.addr.trim().is_empty() && !parsed.token.trim().is_empty(),
            "IPC 路径 {} 缺少有效端点字段，拒绝删除",
            path.display()
        );
        if call_ai_ipc(path, &AiIpcRequest::Status, Duration::from_millis(250)).is_ok() {
            bail!(
                "IPC 端点 {} 已被另一个运行中的 MIO 实例占用",
                path.display()
            );
        }
        let _ = fs::remove_file(path);
    }
    let std_listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("无法在本地回环绑定 IPC 端口")?;
    let addr = std_listener.local_addr()?;
    std_listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(std_listener)?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let endpoint = WindowsIpcEndpoint {
        addr: addr.to_string(),
        token: token.clone(),
    };
    let encoded = serde_json::to_vec(&endpoint)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, &encoded)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(path).map_err(|e| e.error)?;
    let guard = OwnedSocketGuard {
        path: path.to_path_buf(),
        token: token.clone(),
    };
    Ok((listener, token, guard))
}

pub(in crate::service) fn start_ipc_server(
    socket_path: &Path,
    strict_parent: bool,
    parent_cancel: &CancellationToken,
) -> Result<(AiIpcServer, mpsc::Receiver<AiIpcCall>)> {
    #[cfg(unix)]
    {
        let (listener, guard) = bind_unix_socket(socket_path, strict_parent)?;
        let (tx, rx) = mpsc::channel(32);
        let cancel = parent_cancel.child_token();
        let loop_cancel = cancel.clone();
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_IPC_CLIENTS));
        let euid = current_euid();
        let accept_task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = loop_cancel.cancelled() => break,
                    res = listener.accept() => res,
                };
                let Ok((stream, _)) = accepted else {
                    continue;
                };
                let Ok(cred) = stream.peer_cred() else {
                    continue;
                };
                if cred.uid() != euid {
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let tx = tx.clone();
                let conn_cancel = loop_cancel.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let (r, w) = UnixStream::into_split(stream);
                    let _ = serve_ipc_split(r, w, None, None, tx, conn_cancel).await;
                });
            }
        });
        Ok((
            AiIpcServer {
                cancel,
                accept_task: Some(accept_task),
                guard: Some(guard),
            },
            rx,
        ))
    }
    #[cfg(not(unix))]
    {
        let (listener, token, guard) = bind_tcp_endpoint(socket_path, strict_parent)?;
        let (tx, rx) = mpsc::channel(32);
        let cancel = parent_cancel.child_token();
        let loop_cancel = cancel.clone();
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_IPC_CLIENTS));
        let handshake_permits = Arc::new(Semaphore::new(MAX_UNAUTHENTICATED_HANDSHAKES));
        let expected_token = Arc::<str>::from(token);
        let accept_task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = loop_cancel.cancelled() => break,
                    res = listener.accept() => res,
                };
                let Ok((stream, peer)) = accepted else {
                    continue;
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Ok(hs_permit) = handshake_permits.clone().try_acquire_owned() else {
                    continue;
                };
                let tx = tx.clone();
                let conn_cancel = loop_cancel.clone();
                let tok = expected_token.clone();
                let client_permits = permits.clone();
                tokio::spawn(async move {
                    let (r, w) = TcpStream::into_split(stream);
                    let _ = serve_ipc_split(
                        r,
                        w,
                        Some(tok),
                        Some((hs_permit, client_permits)),
                        tx,
                        conn_cancel,
                    )
                    .await;
                });
            }
        });
        Ok((
            AiIpcServer {
                cancel,
                accept_task: Some(accept_task),
                guard: Some(guard),
            },
            rx,
        ))
    }
}

async fn serve_ipc_split<R, W>(
    reader_half: R,
    mut writer_half: W,
    auth_token: Option<Arc<str>>,
    deferred_permits: Option<(tokio::sync::OwnedSemaphorePermit, Arc<Semaphore>)>,
    tx: mpsc::Sender<AiIpcCall>,
    cancel: CancellationToken,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader_half);
    if let Some(expected) = auth_token {
        let mut auth_line = String::new();
        let mut limited = (&mut reader).take(256);
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            res = tokio::time::timeout(IPC_AUTH_TIMEOUT, limited.read_line(&mut auth_line)) => res,
        };
        let Ok(Ok(n)) = res else {
            return Ok(());
        };
        if n == 0 || auth_line.trim() != expected.as_ref() {
            return Ok(());
        }
    }
    let _authenticated_permit = if let Some((hs_permit, client_permits)) = deferred_permits {
        drop(hs_permit);
        let Ok(permit) = client_permits.try_acquire_owned() else {
            return Ok(());
        };
        Some(permit)
    } else {
        None
    };
    loop {
        let mut line = String::new();
        let mut limited = (&mut reader).take((MAX_IPC_LINE_BYTES + 1) as u64);
        let read_res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            res = tokio::time::timeout(IPC_IO_TIMEOUT, limited.read_line(&mut line)) => res,
        };
        let n = match read_res {
            Ok(Ok(n)) => n,
            Ok(Err(err)) => return Err(err.into()),
            Err(_) => return Ok(()),
        };
        if n == 0 {
            return Ok(());
        }
        let response = if line.len() > MAX_IPC_LINE_BYTES
            || !line.ends_with('\n') && line.len() == MAX_IPC_LINE_BYTES
        {
            AiIpcEnvelope::err("IPC 请求单行超过大小上限")
        } else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<AiIpcRequest>(trimmed) {
                Ok(request) => {
                    let reply_timeout = if matches!(request, AiIpcRequest::RepeaterSend { .. }) {
                        IPC_REPEATER_REPLY_TIMEOUT
                    } else {
                        IPC_IO_TIMEOUT
                    };
                    let (reply_tx, reply_rx) = oneshot::channel();
                    if tx
                        .send(AiIpcCall {
                            request,
                            reply: reply_tx,
                        })
                        .await
                        .is_err()
                    {
                        AiIpcEnvelope::err("MIO 后台服务已关闭")
                    } else {
                        match tokio::time::timeout(reply_timeout, reply_rx).await {
                            Ok(Ok(env)) => env,
                            Ok(Err(_)) => AiIpcEnvelope::err("MIO 后台服务未返回响应"),
                            Err(_) => AiIpcEnvelope::err("MIO 处理 IPC 请求超时"),
                        }
                    }
                }
                Err(err) => AiIpcEnvelope::err(format!("无效的 IPC JSON 请求：{err}")),
            }
        };
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        tokio::time::timeout(IPC_IO_TIMEOUT, writer_half.write_all(&encoded)).await??;
        tokio::time::timeout(IPC_IO_TIMEOUT, writer_half.flush()).await??;
    }
}

pub fn call_ai_ipc(
    socket_path: &Path,
    request: &AiIpcRequest,
    timeout: Duration,
) -> Result<(AiIpcEnvelope, String)> {
    use std::io::{BufRead as _, BufReader as StdBufReader, Write as _};
    #[cfg(unix)]
    {
        let mut stream = StdUnixStream::connect(socket_path).with_context(|| {
            format!(
                "无法连接到 MIO 本地服务 socket ({})；请确认 MIO 正在运行",
                socket_path.display()
            )
        })?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let mut encoded = serde_json::to_vec(request)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        stream.flush()?;
        let mut reader = StdBufReader::new(stream);
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        ensure!(n > 0, "MIO 本地服务未返回响应即关闭连接");
        let env: AiIpcEnvelope =
            serde_json::from_str(&line).context("解析 MIO IPC 响应 JSON 失败")?;
        Ok((env, line))
    }
    #[cfg(not(unix))]
    {
        let raw = fs::read_to_string(socket_path).with_context(|| {
            format!(
                "无法读取 MIO 本地服务端点 ({})；请确认 MIO 正在运行",
                socket_path.display()
            )
        })?;
        let ep: WindowsIpcEndpoint =
            serde_json::from_str(&raw).context("解析 MIO 本地服务端点失败")?;
        let addr: std::net::SocketAddr = ep.addr.parse().context("MIO 本地服务端点地址无效")?;
        let mut stream =
            std::net::TcpStream::connect_timeout(&addr, timeout).with_context(|| {
                format!(
                    "无法连接到 MIO 本地服务端点 ({})；请确认 MIO 正在运行",
                    socket_path.display()
                )
            })?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.write_all(ep.token.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut encoded = serde_json::to_vec(request)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        stream.flush()?;
        let mut reader = StdBufReader::new(stream);
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        ensure!(n > 0, "MIO 本地服务未返回响应即关闭连接");
        let env: AiIpcEnvelope =
            serde_json::from_str(&line).context("解析 MIO IPC 响应 JSON 失败")?;
        Ok((env, line))
    }
}

pub(in crate::service) async fn next_ipc_call(
    rx: &mut Option<mpsc::Receiver<AiIpcCall>>,
) -> Option<AiIpcCall> {
    match rx {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}
