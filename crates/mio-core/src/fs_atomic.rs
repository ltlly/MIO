#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use tempfile::NamedTempFile;

pub(crate) fn is_cross_platform_absolute(path: &Path) -> bool {
    path.is_absolute() || path.as_os_str().as_encoded_bytes().starts_with(b"/")
}

pub(crate) fn validate_parent_dir(path: &Path, label: &str) -> Result<PathBuf> {
    ensure!(
        is_cross_platform_absolute(path),
        "{label}路径必须是绝对路径"
    );
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir)),
        "{label}路径不能包含父目录跳转"
    );
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .with_context(|| format!("{label}路径缺少父目录"))?;
    for ancestor in parent.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "{label}目录不得经过符号链接或特殊文件"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("无法检查{label}目录")),
        }
    }
    Ok(parent.to_owned())
}

pub(crate) struct OwnedDir {
    #[cfg(unix)]
    file: File,
    #[cfg(unix)]
    identity: fs::Metadata,
}

impl OwnedDir {
    pub(crate) fn verify_unchanged(&self, parent: &Path, label: &str) -> Result<()> {
        #[cfg(unix)]
        {
            let visible = fs::symlink_metadata(parent)?;
            ensure!(
                visible.dev() == self.identity.dev() && visible.ino() == self.identity.ino(),
                "{label}目录已替换；未保存"
            );
        }
        #[cfg(not(unix))]
        let _ = (parent, label);
        Ok(())
    }

    pub(crate) fn sync_all(&self, context_msg: &str) -> Result<()> {
        #[cfg(unix)]
        self.file
            .sync_all()
            .with_context(|| context_msg.to_owned())?;
        #[cfg(not(unix))]
        let _ = context_msg;
        Ok(())
    }
}

pub(crate) fn ensure_owned_parent_dir(path: &Path, label: &str) -> Result<(PathBuf, OwnedDir)> {
    let parent = validate_parent_dir(path, label)?;
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&parent)?;
    // Re-check ancestors after recursive creation in case a missing segment raced.
    validate_parent_dir(path, label)?;
    #[cfg(unix)]
    let dir = {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&parent)?;
        let identity = file.metadata()?;
        ensure!(
            identity.uid() == unsafe { libc::geteuid() }
                && identity.mode() & 0o002 == 0
                && (identity.mode() & 0o020 == 0 || identity.gid() == unsafe { libc::getegid() }),
            "{label}目录必须由当前用户拥有且不可被其他用户写入"
        );
        OwnedDir { file, identity }
    };
    #[cfg(not(unix))]
    let dir = OwnedDir {};
    Ok((parent, dir))
}

pub(crate) fn check_regular_owned_file(file: &File, label: &str) -> Result<fs::Metadata> {
    let metadata = file.metadata()?;
    #[cfg(unix)]
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o002 == 0
            && (metadata.mode() & 0o020 == 0 || metadata.gid() == unsafe { libc::getegid() }),
        "{label}必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    #[cfg(not(unix))]
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{label}必须是当前用户拥有且其他用户不可写的普通单链接文件"
    );
    Ok(metadata)
}

pub(crate) fn read_bounded_owned_file(
    path: &Path,
    max_bytes: u64,
    validate_parent: bool,
    label: &str,
) -> Result<Option<Vec<u8>>> {
    if validate_parent {
        validate_parent_dir(path, label)?;
    }
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "无法安全打开{label}文件"
        );
    }
    let mut open_opts = OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    open_opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = match open_opts.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("无法安全打开{label}文件")),
    };
    ensure!(
        check_regular_owned_file(&file, label)?.len() <= max_bytes,
        "{label}文件超过有界读取限额"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= max_bytes,
        "{label}文件在读取时超过有界读取限额"
    );
    Ok(Some(bytes))
}

pub(crate) struct SidecarLock {
    file: File,
}

impl Drop for SidecarLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub(crate) fn acquire_sidecar_lock(
    parent: &Path,
    lock_name: &str,
    label: &str,
) -> Result<SidecarLock> {
    let lock_path = parent.join(lock_name);
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(&lock_path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "{label}锁文件无效"
        );
    }
    let mut lock_opts = OpenOptions::new();
    lock_opts
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    #[cfg(unix)]
    lock_opts
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = lock_opts.open(&lock_path)?;
    let identity = check_regular_owned_file(&file, label)?;
    file.try_lock()
        .map_err(|error| anyhow::anyhow!("{label}正由其他实例保存或锁不可用：{error}"))?;
    let visible = fs::symlink_metadata(&lock_path)?;
    #[cfg(unix)]
    ensure!(
        visible.dev() == identity.dev() && visible.ino() == identity.ino(),
        "{label}锁文件已替换；未保存"
    );
    #[cfg(not(unix))]
    ensure!(
        visible.is_file() && visible.len() == identity.len(),
        "{label}锁文件已替换；未保存"
    );
    Ok(SidecarLock { file })
}

pub(crate) fn atomic_write_bytes(
    parent: &Path,
    target: &Path,
    bytes: &[u8],
    dir: &OwnedDir,
    label: &str,
) -> Result<()> {
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    dir.verify_unchanged(parent, label)?;
    temporary.persist(target).map_err(|error| error.error)?;
    dir.sync_all(&format!("{label}已发布但目录同步失败；请重新读取确认"))?;
    Ok(())
}
