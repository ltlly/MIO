//! Stable local snapshots and atomic, non-overwriting conversion into a MIO project.

mod convert;
mod coverage;
mod project_config;
#[cfg(test)]
#[path = "import/tests.rs"]
pub(crate) mod tests;

pub(crate) use convert::{convert, failed, inspect};
pub(crate) use coverage::category_coverage;
pub use project_config::extract_embedded_ext_project_config;

#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use tokio_util::sync::CancellationToken;

use crate::ExtInspection;

pub(super) type Progress<'a> = &'a mut dyn FnMut(crate::ExtImportPhase, u64, Option<u64>);

#[derive(Debug)]
pub(super) struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("外部导入已取消")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Clone, Debug)]
pub(super) struct CleanupDirectory {
    path: PathBuf,
    #[cfg(unix)]
    directory: Arc<File>,
    #[cfg(target_os = "macos")]
    changes: Arc<OwnedFd>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    removed_flag: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(windows)]
    win_file_id: (u32, u64),
    error: String,
}

impl CleanupDirectory {
    fn removed(&self) -> Result<bool> {
        #[cfg(target_os = "macos")]
        {
            let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
            let timeout = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // The descriptor stays open; NOTE_DELETE refers to this inode even after rename/replacement.
            let count = unsafe {
                libc::kevent(
                    self.changes.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    event.as_mut_ptr(),
                    1,
                    &timeout,
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error()).context("无法确认临时目录删除事件");
            }
            if count == 0 {
                return Ok(false);
            }
            let event = unsafe { event.assume_init() };
            let event_error = event.data;
            ensure!(
                event.flags & libc::EV_ERROR == 0,
                "临时目录删除事件返回错误：{}",
                event_error
            );
            ensure!(
                event.ident == self.directory.as_raw_fd() as usize
                    && event.filter == libc::EVFILT_VNODE,
                "临时目录删除事件归属不匹配"
            );
            Ok(event.fflags & libc::NOTE_DELETE != 0)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Ok(self.directory.metadata()?.nlink() == 0)
        }
        #[cfg(not(unix))]
        {
            Ok(self.removed_flag.load(std::sync::atomic::Ordering::Acquire))
        }
    }

    fn remove(&self) -> Result<()> {
        if self.removed()? {
            return Ok(());
        }
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                #[cfg(not(unix))]
                {
                    self.removed_flag
                        .store(true, std::sync::atomic::Ordering::Release);
                    return Ok(());
                }
                #[cfg(unix)]
                anyhow::bail!("临时目录路径已消失，但所持目录尚未删除；未确认回收")
            }
            Err(error) => return Err(error.into()),
        };
        #[cfg(unix)]
        ensure!(
            metadata.is_dir() && metadata.dev() == self.device && metadata.ino() == self.inode,
            "临时目录已被替换；拒绝删除其它数据"
        );
        #[cfg(not(unix))]
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "临时目录已被替换；拒绝删除其它数据"
        );
        #[cfg(windows)]
        ensure!(
            windows_path_file_id(&self.path).ok() == Some(self.win_file_id),
            "临时目录已被替换；拒绝删除其它数据"
        );
        fs::remove_dir_all(&self.path)?;
        #[cfg(not(unix))]
        self.removed_flag
            .store(true, std::sync::atomic::Ordering::Release);
        ensure!(self.removed()?, "临时目录在回收期间被移动；未确认回收");
        Ok(())
    }
}

/// Failed cleanup remains owned by the service and can be retried without guessing paths.
#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupFailure {
    pub(super) directories: Vec<CleanupDirectory>,
}

impl CleanupFailure {
    pub(crate) fn is_empty(&self) -> bool {
        self.directories.is_empty()
    }

    pub(crate) fn retry(&mut self) -> Result<()> {
        self.directories
            .retain_mut(|directory| match directory.remove() {
                Ok(()) => false,
                Err(error) => {
                    directory.error = format!("{error:#}");
                    true
                }
            });
        if self.is_empty() {
            Ok(())
        } else {
            Err(self.clone().into())
        }
    }
}

impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} 个 External 临时目录尚未回收", self.directories.len())?;
        for directory in &self.directories {
            write!(f, "；{}：{}", directory.path.display(), directory.error)?;
        }
        Ok(())
    }
}

impl std::error::Error for CleanupFailure {}

pub(super) struct PrivateDirectory {
    directory: Option<CleanupDirectory>,
}

impl PrivateDirectory {
    pub(super) fn new(parent: &Path, prefix: &str) -> Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(prefix);
        #[cfg(unix)]
        builder.permissions(fs::Permissions::from_mode(0o700));
        let temporary = builder.tempdir_in(parent.canonicalize()?)?;
        #[cfg(target_os = "macos")]
        {
            let directory = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(temporary.path())?;
            let metadata = directory.metadata()?;
            ensure!(metadata.is_dir(), "新建的 外部导入临时路径不是目录");
            let changes = unsafe { libc::kqueue() };
            if changes < 0 {
                return Err(io::Error::last_os_error()).context("不能监测临时目录的实际删除");
            }
            let changes = unsafe { OwnedFd::from_raw_fd(changes) };
            if unsafe { libc::fcntl(changes.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
                return Err(io::Error::last_os_error()).context("不能限制临时目录监测描述符继承");
            }
            let change = libc::kevent {
                ident: directory.as_raw_fd() as usize,
                filter: libc::EVFILT_VNODE,
                // Keep the deletion notification latched for retries/cloned cleanup ownership.
                flags: libc::EV_ADD,
                fflags: libc::NOTE_DELETE,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            if unsafe {
                libc::kevent(
                    changes.as_raw_fd(),
                    &change,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error()).context("不能注册临时目录删除监测");
            }
            Ok(Self {
                directory: Some(CleanupDirectory {
                    path: temporary.keep(),
                    directory: Arc::new(directory),
                    changes: Arc::new(changes),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    error: String::new(),
                }),
            })
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let directory = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(temporary.path())?;
            let metadata = directory.metadata()?;
            ensure!(metadata.is_dir(), "新建的 外部导入临时路径不是目录");
            Ok(Self {
                directory: Some(CleanupDirectory {
                    path: temporary.keep(),
                    directory: Arc::new(directory),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    error: String::new(),
                }),
            })
        }
        #[cfg(not(unix))]
        {
            let metadata = fs::symlink_metadata(temporary.path())?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "新建的 外部导入临时路径不是目录"
            );
            #[cfg(windows)]
            let win_file_id = windows_path_file_id(temporary.path())?;
            Ok(Self {
                directory: Some(CleanupDirectory {
                    path: temporary.keep(),
                    removed_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    #[cfg(windows)]
                    win_file_id,
                    error: String::new(),
                }),
            })
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self
            .directory
            .as_ref()
            .expect("owned temporary directory")
            .path
    }

    pub(super) fn close(mut self) -> Result<()> {
        let mut cleanup = CleanupFailure {
            directories: vec![self.directory.take().expect("owned temporary directory")],
        };
        cleanup.retry()
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if let Some(directory) = &self.directory {
            let _ = directory.remove();
        }
    }
}

pub(crate) fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(Cancelled.into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    #[cfg(unix)]
    pub(super) device: u64,
    #[cfg(unix)]
    pub(super) inode: u64,
    pub(super) length: u64,
    #[cfg(unix)]
    pub(super) modified: (i64, i64),
    #[cfg(unix)]
    pub(super) changed: (i64, i64),
    #[cfg(not(unix))]
    pub(super) modified_sys: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    pub(super) created_sys: Option<std::time::SystemTime>,
}

impl Identity {
    pub(super) fn of(metadata: &Metadata) -> Result<Self> {
        ensure!(metadata.is_file(), "External 源必须是普通文件");
        #[cfg(unix)]
        {
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                length: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                length: metadata.len(),
                modified_sys: metadata.modified().ok(),
                created_sys: metadata.created().ok(),
            })
        }
    }
}

pub(crate) struct Inspection {
    pub info: ExtInspection,
    pub(super) source_path: PathBuf,
    pub(super) source: File,
    pub(super) identity: Identity,
    #[cfg(windows)]
    pub(super) win_file_id: (u32, u64),
    pub(super) snapshot: Option<PrivateDirectory>,
}

impl Inspection {
    pub(super) fn check_source(&self) -> Result<()> {
        ensure!(
            Identity::of(&self.source.metadata()?)? == self.identity
                && Identity::of(&fs::symlink_metadata(&self.source_path)?)? == self.identity,
            "External 源文件在预检后已变化；未发布转换结果，请重新预检"
        );
        #[cfg(windows)]
        ensure!(
            windows_handle_file_id(&self.source).ok() == Some(self.win_file_id)
                && windows_path_file_id(&self.source_path).ok() == Some(self.win_file_id),
            "External 源文件在预检后已变化；未发布转换结果，请重新预检"
        );
        Ok(())
    }

    pub fn release(self) -> Result<()> {
        drop(self.source);
        if let Some(snapshot) = self.snapshot {
            snapshot.close().context("无法回收 外部归档只读快照")?;
        }
        Ok(())
    }
}

pub(super) fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

#[cfg(windows)]
pub(super) fn windows_handle_file_id(file: &File) -> io::Result<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    windows_raw_handle_file_id(file.as_raw_handle() as isize)
}

#[cfg(windows)]
pub(super) fn windows_path_file_id(path: &Path) -> io::Result<(u32, u64)> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            sec: *const std::ffi::c_void,
            disp: u32,
            flags: u32,
            template: isize,
        ) -> isize;
        fn CloseHandle(handle: isize) -> i32;
    }
    const OPEN_EXISTING: u32 = 3;
    const FILE_SHARE_ALL: u32 = 0x0000_0007;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const INVALID_HANDLE_VALUE: isize = -1;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_ALL,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let res = windows_raw_handle_file_id(handle);
    unsafe {
        let _ = CloseHandle(handle);
    }
    res
}

#[cfg(windows)]
fn windows_raw_handle_file_id(handle: isize) -> io::Result<(u32, u64)> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: isize, info: *mut ByHandleFileInformation) -> i32;
    }
    let mut info = ByHandleFileInformation::default();
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let idx = (u64::from(info.file_index_high) << 32) | u64::from(info.file_index_low);
    Ok((info.volume_serial_number, idx))
}

#[cfg(test)]
mod deep_compat_tests;
