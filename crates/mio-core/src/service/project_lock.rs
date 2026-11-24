use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

static PROCESS_HELD_PROJECT_LOCKS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub(in crate::service) struct ActiveProjectLock {
    key: String,
    file: File,
}

impl ActiveProjectLock {
    pub(in crate::service) fn matches_path(&self, candidate: &Path) -> bool {
        canonical_project_key(candidate).is_ok_and(|key| key == self.key)
    }
}

impl Drop for ActiveProjectLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        if let Ok(mut held) = PROCESS_HELD_PROJECT_LOCKS.lock() {
            held.remove(&self.key);
        }
    }
}

fn canonical_project_key(path: &Path) -> Result<String> {
    let canonical = if let Ok(canon) = fs::canonicalize(path) {
        canon
    } else {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .context("无法获取当前工作目录")?
                .join(path)
        };
        if let Some(parent) = abs.parent()
            && let Ok(canon_parent) = fs::canonicalize(parent)
            && let Some(name) = abs.file_name()
        {
            canon_parent.join(name)
        } else {
            abs
        }
    };
    let digest: [u8; 32] = Sha256::digest(canonical.as_os_str().as_encoded_bytes()).into();
    Ok(hex::encode(digest))
}

fn project_locks_dir() -> Result<PathBuf> {
    if let Some(run_dir) = crate::fs_atomic::platform_runtime_dir() {
        return Ok(run_dir.join("project-locks"));
    }
    #[cfg(unix)]
    {
        let euid = unsafe { libc::geteuid() };
        Ok(PathBuf::from(format!("/tmp/mio-{euid}/project-locks")))
    }
    #[cfg(not(unix))]
    {
        Ok(std::env::temp_dir().join("mio-run").join("project-locks"))
    }
}

pub(in crate::service) fn acquire_active_project_lock(path: &Path) -> Result<ActiveProjectLock> {
    let key = canonical_project_key(path)?;
    {
        let mut held = PROCESS_HELD_PROJECT_LOCKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !held.insert(key.clone()) {
            anyhow::bail!(
                "项目文件已在另一个 MIO 窗口中打开：{}；为避免数据损坏，请切换到对应窗口或另存为新项目",
                path.display()
            );
        }
    }
    let acquired = (|| -> Result<ActiveProjectLock> {
        let dir = project_locks_dir()?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .with_context(|| format!("无法创建项目锁目录 {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let meta = fs::symlink_metadata(&dir)?;
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == unsafe { libc::geteuid() },
                "项目锁目录无效或不属于当前用户：{}",
                dir.display()
            );
        }
        let lock_path = dir.join(format!("{key}.lock"));
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = opts
            .open(&lock_path)
            .with_context(|| format!("无法打开项目锁文件 {}", lock_path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!(
                    "项目文件已在另一个 MIO 窗口中打开：{}；为避免数据损坏，请切换到对应窗口或另存为新项目",
                    path.display()
                );
            }
            Err(std::fs::TryLockError::Error(err)) => {
                return Err(err).with_context(|| {
                    format!("锁定项目锁文件失败 {}", lock_path.display())
                });
            }
        }
        Ok(ActiveProjectLock {
            key: key.clone(),
            file,
        })
    })();
    if acquired.is_err()
        && let Ok(mut held) = PROCESS_HELD_PROJECT_LOCKS.lock()
    {
        held.remove(&key);
    }
    acquired
}
