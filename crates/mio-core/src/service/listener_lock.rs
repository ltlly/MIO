use std::{
    fs::{self, File, OpenOptions},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, ensure};

static NEXT_LOCK_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS_HELD_LISTENER_LOCKS: LazyLock<Mutex<Vec<(u64, SocketAddr)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub(in crate::service) struct ActiveListenerLock {
    id: u64,
    _port_lock: Option<File>,
    _addr_lock: Option<File>,
}

impl Drop for ActiveListenerLock {
    fn drop(&mut self) {
        if let Ok(mut held) = PROCESS_HELD_LISTENER_LOCKS.lock()
            && let Some(pos) = held.iter().position(|(id, _)| *id == self.id)
        {
            held.swap_remove(pos);
        }
    }
}

#[derive(Debug)]
pub(in crate::service) struct ListenerConflictError(pub String);

impl std::fmt::Display for ListenerConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ListenerConflictError {}

pub(in crate::service) fn listener_conflict_message(addr: SocketAddr) -> String {
    let suggested = if addr.port() < u16::MAX && addr.port() > 0 {
        addr.port() + 1
    } else {
        8081
    };
    format!(
        "Proxy 监听地址 {addr} 已被占用（可能正由另一个 MIO 窗口或其他程序监听）；请在“Proxy 设置 > Proxy listeners”中修改为其他空闲端口（如 {suggested}）后重试"
    )
}

pub(in crate::service) fn is_addr_in_use_or_conflict(err: &anyhow::Error) -> bool {
    if err.downcast_ref::<ListenerConflictError>().is_some() {
        return true;
    }
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::AddrInUse)
    })
}

fn family_prefix(addr: SocketAddr) -> &'static str {
    if addr.is_ipv6() { "v6" } else { "v4" }
}

fn sanitize_ip_filename_component(addr: SocketAddr) -> String {
    addr.ip().to_string().replace(':', "_")
}

fn open_lock_file(path: &Path) -> Result<File> {
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "监听锁文件无效：{}",
            path.display()
        );
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    opts.open(path)
        .with_context(|| format!("无法打开监听锁文件 {}", path.display()))
}

fn ensure_listener_locks_dir(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .with_context(|| format!("无法创建监听锁目录 {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let meta = fs::symlink_metadata(dir)?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == unsafe { libc::geteuid() },
            "监听锁目录无效或不属于当前用户：{}",
            dir.display()
        );
    }
    #[cfg(not(unix))]
    {
        let meta = fs::symlink_metadata(dir)?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "监听锁目录无效：{}",
            dir.display()
        );
    }
    Ok(())
}

pub(in crate::service) fn resolve_listener_locks_dir(
    preferences_path: Option<&Path>,
) -> Option<PathBuf> {
    let prefs = preferences_path?;
    if std::env::var_os("MIO_USER_PREFERENCES_PATH").is_none_or(|v| v.is_empty())
        && let Ok(default_cfg_dir) =
            crate::fs_atomic::platform_app_dir(crate::fs_atomic::AppDirKind::Config)
        && prefs.parent() == Some(default_cfg_dir.as_path())
        && let Some(run_dir) = crate::fs_atomic::platform_runtime_dir()
    {
        return Some(run_dir.join("listener-locks"));
    }
    prefs
        .parent()
        .map(|parent| parent.join("run").join("listener-locks"))
}

pub(in crate::service) fn acquire_listener_lock(
    lock_dir: Option<&Path>,
    addr: SocketAddr,
) -> Result<Option<ActiveListenerLock>> {
    if addr.port() == 0 {
        return Ok(None);
    }
    let id = NEXT_LOCK_ID.fetch_add(1, Ordering::Relaxed);
    {
        let mut held = PROCESS_HELD_LISTENER_LOCKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held
            .iter()
            .any(|(_, existing)| crate::proxy_listener_addrs_overlap(*existing, addr))
        {
            return Err(ListenerConflictError(listener_conflict_message(addr)).into());
        }
        held.push((id, addr));
    }

    let acquired = (|| -> Result<Option<ActiveListenerLock>> {
        let Some(dir) = lock_dir else {
            return Ok(Some(ActiveListenerLock {
                id,
                _port_lock: None,
                _addr_lock: None,
            }));
        };
        ensure_listener_locks_dir(dir)?;
        let fam = family_prefix(addr);
        let port_path = dir.join(format!("{fam}-port-{}.lock", addr.port()));
        let port_file = open_lock_file(&port_path)?;
        if addr.ip().is_unspecified() {
            port_file
                .try_lock()
                .map_err(|_| anyhow::Error::new(ListenerConflictError(listener_conflict_message(addr))))?;
            return Ok(Some(ActiveListenerLock {
                id,
                _port_lock: Some(port_file),
                _addr_lock: None,
            }));
        }
        port_file
            .try_lock_shared()
            .map_err(|_| anyhow::Error::new(ListenerConflictError(listener_conflict_message(addr))))?;
        let ip_tag = sanitize_ip_filename_component(addr);
        let addr_path = dir.join(format!("{fam}-{ip_tag}-{}.lock", addr.port()));
        let addr_file = open_lock_file(&addr_path)?;
        addr_file
            .try_lock()
            .map_err(|_| anyhow::Error::new(ListenerConflictError(listener_conflict_message(addr))))?;
        Ok(Some(ActiveListenerLock {
            id,
            _port_lock: Some(port_file),
            _addr_lock: Some(addr_file),
        }))
    })();

    if acquired.is_err()
        && let Ok(mut held) = PROCESS_HELD_LISTENER_LOCKS.lock()
        && let Some(pos) = held.iter().position(|(held_id, _)| *held_id == id)
    {
        held.swap_remove(pos);
    }
    acquired
}
