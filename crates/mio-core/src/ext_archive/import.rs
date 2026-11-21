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

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    fs::{self, File, Metadata},
    io,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
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
pub(super) struct PendingCleanup {
    path: PathBuf,
    error: String,
}

impl PendingCleanup {
    fn remove(&self) -> io::Result<()> {
        match fs::remove_dir_all(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// Failed staging-directory cleanup remains owned by the service and can be retried.
#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupFailure {
    pub(super) directories: Vec<PendingCleanup>,
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
                    directory.error = error.to_string();
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

pub(super) fn create_stage_dir(parent: &Path, prefix: &str) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    builder.permissions(fs::Permissions::from_mode(0o700));
    Ok(builder.tempdir_in(parent.canonicalize()?)?)
}

pub(super) fn close_stage_dir(dir: tempfile::TempDir) -> Result<()> {
    let path = dir.path().to_path_buf();
    match dir.close() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CleanupFailure {
            directories: vec![PendingCleanup {
                path,
                error: error.to_string(),
            }],
        }
        .into()),
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
    #[cfg(test)]
    pub(crate) released: Option<tokio::sync::oneshot::Sender<()>>,
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

    pub fn release(self) {
        drop(self.source);
        #[cfg(test)]
        if let Some(released) = self.released {
            let _ = released.send(());
        }
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
